//! `docs/api.md` §3 — the control socket.

pub mod handler;
pub mod protocol;

use crate::error::ProxyError;
use handler::ControlState;
use protocol::Request;
use protocol::Response;
use protocol::codes;
use std::path::Path;
use std::path::PathBuf;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;

/// The most bytes a unix socket address can carry on this platform.
///
/// `sun_path` is 104 bytes on the BSDs and macOS and 108 on Linux, one of which
/// is the terminator. Nothing in the standard library exposes it, and exceeding
/// it fails at `bind` and at `connect` rather than at the point the path was
/// chosen.
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub const PATH_LIMIT: usize = 103;
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub const PATH_LIMIT: usize = 107;

/// Where the socket lives.
///
/// **One derivation, used by the daemon's bind and by every CLI call**, so the
/// pair cannot split: a CLI that derived its own path would talk to a daemon
/// that is not the one its configuration belongs to.
///
/// `PROXENOS_HOME` is what isolates a daemon from the operator's own, and the
/// socket is part of what has to move with it. When it named only the
/// configuration, an isolated CLI sharing a `TMPDIR` still reached the real
/// daemon — and every login path ends in `accounts.select` over that socket,
/// so an isolated login could switch the account the operator's daemon serves.
///
/// With no home named, the path is unchanged: an operator's running daemon is
/// addressed by the path it already bound.
pub fn default_path() -> PathBuf {
    path_for(
        std::env::var_os("PROXENOS_HOME")
            .map(PathBuf::from)
            .as_deref(),
        Some(std::env::temp_dir()).as_deref(),
    )
}

/// The same derivation, over an environment named rather than read.
///
/// **This exists so a supervisor unit and the operator's shell cannot split.**
/// A process launchd starts does not necessarily see the `TMPDIR` a login shell
/// does, and a daemon that binds one path while the CLI dials another comes up
/// healthy on the HTTP port while every verb reports connection refused. The
/// unit is rendered by putting the installing shell's values through this
/// function and carrying them explicitly, so both halves are the same call over
/// the same inputs rather than two derivations that happen to agree.
pub fn path_for(home: Option<&Path>, tmpdir: Option<&Path>) -> PathBuf {
    match home {
        Some(home) => home.join("proxenos.sock"),
        None => tmpdir
            .unwrap_or_else(|| Path::new(FALLBACK_TMPDIR))
            .join("proxenos.sock"),
    }
}

/// Where the socket goes when nothing names a temporary directory.
///
/// Named rather than inlined because a caller that has to *record* the
/// derivation's fallback — a supervisor unit, whose job is started with an
/// environment it did not choose — needs the same value the derivation used.
pub const FALLBACK_TMPDIR: &str = "/tmp";

/// Refuse a path the platform cannot address, by name.
///
/// The silent variant of this costs a real debugging session: the bind fails
/// while the HTTP port comes up fine, so the daemon serves turns and looks
/// healthy while every CLI verb gets connection refused and `start` times
/// out waiting for a socket that will never appear. Both ends check, because
/// either can be the first to be given the path.
pub fn ensure_addressable(path: &Path) -> Result<(), ProxyError> {
    let length = path.as_os_str().len();
    if length <= PATH_LIMIT {
        return Ok(());
    }

    Err(ProxyError::invalid_request(format!(
        "the control socket path is {length} bytes, over the {PATH_LIMIT}-byte limit this \
         platform puts on a unix socket address: {}. Point `PROXENOS_HOME` at a shorter \
         directory, or unset it and use a shorter `TMPDIR`.",
        path.display()
    )))
}

/// The environment variable that puts the CLI in client mode (`api.md` §2).
///
/// A URL, not a host: the scheme decides whether the hop is encrypted, and a
/// daemon reached over anything but a private network wants `https` in front
/// of it. There is no configuration key for this — client mode is a property
/// of the machine the CLI is invoked on, and config.toml on that machine is
/// the *daemon's* configuration shape, not a client's.
pub const DAEMON_URL_VAR: &str = "PROXENOS_DAEMON";

/// Where the token comes from, in order of precedence.
///
/// Never a flag: an argument is visible in `ps` to every process on the
/// machine, and the whole point of the token is that it is not.
pub const TOKEN_VAR: &str = "PROXENOS_TOKEN";
pub const TOKEN_FILE_VAR: &str = "PROXENOS_TOKEN_FILE";

/// Which daemon a verb is talking to.
///
/// **One value, resolved from the environment, rather than a path threaded
/// through every verb.** Every verb already derived its socket path from one
/// function; this replaces that function's answer with a fuller one, so client
/// mode reaches a verb by being resolved rather than by being plumbed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// The daemon on this machine, over the §3 socket.
    Local(PathBuf),
    /// A daemon on another machine, over `POST /control`.
    Remote {
        url: String,
        /// `None` reaches a daemon that configured no token, which is only
        /// possible over loopback — so a remote endpoint with no token will be
        /// refused by the daemon rather than here. Refusing here would also be
        /// correct and would say less: the daemon's refusal names the key.
        token: Option<String>,
    },
}

impl Endpoint {
    /// Read the environment and say which daemon this process talks to.
    pub fn resolve() -> Result<Self, ProxyError> {
        let Some(url) = std::env::var(DAEMON_URL_VAR)
            .ok()
            .filter(|url| !url.is_empty())
        else {
            return Ok(Self::Local(default_path()));
        };
        // A user name or password in the URL would ride into every child's
        // `ANTHROPIC_BASE_URL` and back out of `inspect`; the token has two
        // variables of its own, and a URL is not one of them.
        let parsed = url::Url::parse(&url).map_err(|error| {
            ProxyError::invalid_request(format!("{DAEMON_URL_VAR} is not a URL: {error}"))
        })?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(ProxyError::invalid_request(format!(
                "{DAEMON_URL_VAR} carries a user name or password; a token goes in \
                 {TOKEN_VAR} or {TOKEN_FILE_VAR}, never in the URL"
            )));
        }
        Ok(Self::Remote {
            url: url.trim_end_matches('/').to_owned(),
            token: token_from_environment()?,
        })
    }

    /// Where the daemon is, for a caller that has to say so. `None` is local,
    /// which is what `status` reports by leaving the field out.
    #[must_use]
    pub fn remote_url(&self) -> Option<&str> {
        match self {
            Self::Local(_) => None,
            Self::Remote { url, .. } => Some(url),
        }
    }

    #[must_use]
    pub fn token(&self) -> Option<&str> {
        match self {
            Self::Local(_) => None,
            Self::Remote { token, .. } => token.as_deref(),
        }
    }

    /// Refuse a verb that only means something on the daemon's own machine.
    ///
    /// **Refused rather than forwarded.** `accounts login` runs somebody
    /// else's client and reads the profile it wrote; `supervisor install`
    /// writes a launchd plist; `run` binds a port. Each of those would happen
    /// on the wrong machine, and each would look like it worked.
    pub fn refuse_remote(&self, verb: &str) -> Result<(), ProxyError> {
        match self.remote_url() {
            None => Ok(()),
            Some(url) => Err(ProxyError::invalid_request(format!(
                "`{verb}` acts on the machine the daemon runs on, and {DAEMON_URL_VAR} points \
                 this CLI at {url}. Run it on that host."
            ))),
        }
    }
}

/// The token this process presents, from whichever variable states it.
fn token_from_environment() -> Result<Option<String>, ProxyError> {
    if let Some(token) = std::env::var(TOKEN_VAR).ok().filter(|t| !t.is_empty()) {
        return Ok(Some(token));
    }
    let Some(path) = std::env::var_os(TOKEN_FILE_VAR).filter(|path| !path.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    let raw = std::fs::read_to_string(&path).map_err(|error| {
        ProxyError::invalid_request(format!(
            "could not read {TOKEN_FILE_VAR} at {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(raw.trim().to_owned()).filter(|token| !token.is_empty()))
}

/// Ask whichever daemon this process is configured for.
///
/// What every verb calls. Resolving per call rather than once at startup keeps
/// the shape the socket path already had — one derivation, read where it is
/// needed — and costs two environment reads.
pub async fn ask(
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, ProxyError> {
    dial(&Endpoint::resolve()?, method, params).await
}

/// One request to a named endpoint.
pub async fn dial(
    endpoint: &Endpoint,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, ProxyError> {
    match endpoint {
        Endpoint::Local(path) => call(path, method, params).await,
        Endpoint::Remote { url, token } => call_http(url, token.as_deref(), method, params).await,
    }
}

/// The same vocabulary, over HTTP.
///
/// One request per body, and the same JSON-RPC response read the same way —
/// including the code, which is what lets a caller tell "this daemon has no
/// such method" from "that method refused what you asked".
pub async fn call_http(
    url: &str,
    token: Option<&str>,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, ProxyError> {
    let endpoint = format!("{url}/control");
    let mut request = reqwest::Client::new()
        .post(&endpoint)
        .json(&Request::new(1, method, params));
    if let Some(token) = token {
        // The same header and the same shape the ingress reads, so one token
        // serves both surfaces and there is nothing to keep in step.
        request = request.bearer_auth(crate::ingress::auth_token_value(Some(token), None));
    }

    let response = request.send().await.map_err(|error| {
        ProxyError::invalid_request(format!(
            "could not reach the daemon at {endpoint}: {error}. Is it running, and is \
             {DAEMON_URL_VAR} right?"
        ))
    })?;

    let status = response.status();
    let body = response.text().await.map_err(|error| {
        ProxyError::invalid_request(format!("could not read the daemon's answer: {error}"))
    })?;

    if status == axum::http::StatusCode::UNAUTHORIZED {
        return Err(ProxyError::authentication(format!(
            "the daemon at {url} refused this CLI's token. Set {TOKEN_VAR} (or \
             {TOKEN_FILE_VAR}) to the secret its `listen.token` names."
        )));
    }
    if !status.is_success() {
        return Err(ProxyError::invalid_request(format!(
            "the daemon at {endpoint} answered {status}: {}",
            body.trim()
        )));
    }

    let response: Response = serde_json::from_str(&body).map_err(|error| {
        ProxyError::invalid_request(format!("unreadable response from the daemon: {error}"))
    })?;
    match (response.result, response.error) {
        (Some(result), _) => Ok(result),
        (None, Some(error)) if error.code == codes::METHOD_NOT_FOUND => {
            Err(ProxyError::not_found(error.message))
        }
        (None, Some(error)) => Err(ProxyError::invalid_request(error.message)),
        (None, None) => Err(ProxyError::invalid_request(
            "the daemon returned neither a result nor an error",
        )),
    }
}

/// This binary's version, build id included. One file is both the daemon and
/// the CLI, so the daemon answering a socket is not necessarily the build that
/// asked — and within one version number the id is the only thing that says so
/// (`crate::version`).
pub fn version() -> &'static str {
    crate::version::build()
}

/// Refuse a payload from a daemon that predates client policy.
///
/// **Read as a capability, not a version comparison.** Comparing version
/// strings forces a policy about which differences matter, and gets it wrong
/// for a patched build or a forgotten bump. The question being asked is whether
/// this daemon can answer for the policy, and its payload answers that
/// directly: the key is always present on a daemon that knows about it, empty
/// when there is none to state.
///
/// Verbs whose correctness depends on the policy call this and stop. Producing
/// a settings document that silently lacks a permission rule is exactly the
/// shape of failure this proxy exists to prevent — it would look like a
/// complete document and behave like an incomplete one.
pub fn require_client_policy(result: &serde_json::Value) -> Result<(), ProxyError> {
    if result.get("settings").is_some() {
        return Ok(());
    }

    Err(ProxyError::invalid_request(
        "The running daemon is from an older build and has no client policy to give. Restart \
         the daemon and try again.",
    ))
}

/// Serve the control socket until the process stops.
#[cfg(unix)]
pub async fn serve(path: &Path, state: ControlState) -> Result<(), ProxyError> {
    ensure_addressable(path)?;

    // A socket left behind by a crashed daemon would refuse the bind. Removing
    // it is safe here because the port bind has already established that no
    // other daemon is running.
    let _ = std::fs::remove_file(path);

    let listener = tokio::net::UnixListener::bind(path).map_err(|error| {
        ProxyError::invalid_request(format!("could not bind the control socket: {error}"))
    })?;

    // The socket carries no authentication, so the filesystem is the access
    // control: owner-only, like the credentials it can clear.
    restrict(path);

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle_connection(stream, state).await;
        });
    }
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

/// One connection, one or more requests, each a line of JSON.
#[cfg(unix)]
async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: ControlState,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = answer(&state, &line).await;
        let mut body = serde_json::to_string(&response).unwrap_or_default();
        body.push('\n');
        writer.write_all(body.as_bytes()).await?;
        writer.flush().await?;

        // Only now. A stop that released the run loop before this flush would
        // race the process against its own answer, and a caller reading a
        // closed connection cannot tell a clean stop from a crash.
        if state.shutdown.requested() {
            state.shutdown.release();
            break;
        }
    }

    Ok(())
}

/// Turn one request line into one response. Pure, so the whole vocabulary is
/// testable without a socket.
pub async fn answer(state: &ControlState, line: &str) -> Response {
    let request: Request = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(error) => {
            return Response::failed(
                serde_json::Value::Null,
                codes::PARSE_ERROR,
                format!("malformed request: {error}"),
            );
        }
    };

    if request.jsonrpc != protocol::VERSION {
        return Response::failed(
            request.id,
            codes::INVALID_REQUEST,
            format!("unsupported jsonrpc version `{}`", request.jsonrpc),
        );
    }

    match handler::dispatch(state, &request.method, request.params.as_ref()).await {
        Ok(result) => Response::ok(request.id, result),
        Err(error) => {
            let code = if error.status == axum::http::StatusCode::NOT_FOUND {
                codes::METHOD_NOT_FOUND
            } else {
                codes::APPLICATION_ERROR
            };
            Response::failed(request.id, code, error.message)
        }
    }
}

/// Send one request to a running daemon and return its result.
#[cfg(unix)]
pub async fn call(
    path: &Path,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, ProxyError> {
    ensure_addressable(path)?;

    let stream = tokio::net::UnixStream::connect(path).await.map_err(|error| {
        ProxyError::invalid_request(format!(
            "could not reach the daemon at {}: {error}. Is it running? Start it with `proxenos run`.",
            path.display()
        ))
    })?;

    let (reader, mut writer) = stream.into_split();
    let mut body = serde_json::to_string(&Request::new(1, method, params)).unwrap_or_default();
    body.push('\n');
    writer
        .write_all(body.as_bytes())
        .await
        .map_err(|error| ProxyError::invalid_request(format!("could not send: {error}")))?;
    writer
        .flush()
        .await
        .map_err(|error| ProxyError::invalid_request(format!("could not send: {error}")))?;

    let mut lines = BufReader::new(reader).lines();
    let line = lines
        .next_line()
        .await
        .map_err(|error| ProxyError::invalid_request(format!("could not read: {error}")))?
        .ok_or_else(|| ProxyError::invalid_request("the daemon closed the connection"))?;

    let response: Response = serde_json::from_str(&line).map_err(|error| {
        ProxyError::invalid_request(format!("unreadable response from the daemon: {error}"))
    })?;

    match (response.result, response.error) {
        (Some(result), _) => Ok(result),
        // The code survives the trip. Flattening every failure into one kind
        // reads as tidy and costs a caller the one distinction it acts on:
        // "this daemon does not have that method" is a different situation from
        // "that method refused what you asked", and only the first is answered
        // by replacing the daemon.
        (None, Some(error)) if error.code == codes::METHOD_NOT_FOUND => {
            Err(ProxyError::not_found(error.message))
        }
        (None, Some(error)) => Err(ProxyError::invalid_request(error.message)),
        (None, None) => Err(ProxyError::invalid_request(
            "the daemon returned neither a result nor an error",
        )),
    }
}

// Windows serves the same vocabulary over a named pipe. The protocol and the
// handler are shared; only the listener differs.
#[cfg(windows)]
pub async fn serve(path: &Path, state: ControlState) -> Result<(), ProxyError> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let name = pipe_name(path);
    loop {
        let server = ServerOptions::new().create(&name).map_err(|error| {
            ProxyError::invalid_request(format!("could not create the control pipe: {error}"))
        })?;
        server.connect().await.map_err(|error| {
            ProxyError::invalid_request(format!("control pipe failed: {error}"))
        })?;

        let state = state.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let response = answer(&state, &line).await;
                let mut body = serde_json::to_string(&response).unwrap_or_default();
                body.push('\n');
                if writer.write_all(body.as_bytes()).await.is_err() {
                    break;
                }
                let _ = writer.flush().await;

                // Same order as the other listener: released only once the
                // answer is out. Without this a stop is answered and never
                // acted on, which is the worse of the two failures because the
                // caller was told it worked.
                if state.shutdown.requested() {
                    state.shutdown.release();
                    break;
                }
            }
        });
    }
}

#[cfg(windows)]
pub async fn call(
    path: &Path,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, ProxyError> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let client = ClientOptions::new()
        .open(pipe_name(path))
        .map_err(|error| {
            ProxyError::invalid_request(format!(
                "could not reach the daemon: {error}. Is it running? Start it with `proxenos run`."
            ))
        })?;

    let (reader, mut writer) = tokio::io::split(client);
    let mut body = serde_json::to_string(&Request::new(1, method, params)).unwrap_or_default();
    body.push('\n');
    writer
        .write_all(body.as_bytes())
        .await
        .map_err(|error| ProxyError::invalid_request(format!("could not send: {error}")))?;
    writer
        .flush()
        .await
        .map_err(|error| ProxyError::invalid_request(format!("could not send: {error}")))?;

    let mut lines = BufReader::new(reader).lines();
    let line = lines
        .next_line()
        .await
        .map_err(|error| ProxyError::invalid_request(format!("could not read: {error}")))?
        .ok_or_else(|| ProxyError::invalid_request("the daemon closed the connection"))?;

    let response: Response = serde_json::from_str(&line).map_err(|error| {
        ProxyError::invalid_request(format!("unreadable response from the daemon: {error}"))
    })?;

    match (response.result, response.error) {
        (Some(result), _) => Ok(result),
        // The code survives the trip. Flattening every failure into one kind
        // reads as tidy and costs a caller the one distinction it acts on:
        // "this daemon does not have that method" is a different situation from
        // "that method refused what you asked", and only the first is answered
        // by replacing the daemon.
        (None, Some(error)) if error.code == codes::METHOD_NOT_FOUND => {
            Err(ProxyError::not_found(error.message))
        }
        (None, Some(error)) => Err(ProxyError::invalid_request(error.message)),
        (None, None) => Err(ProxyError::invalid_request(
            "the daemon returned neither a result nor an error",
        )),
    }
}

#[cfg(windows)]
fn pipe_name(path: &Path) -> String {
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("proxenos");
    format!(r"\\.\pipe\{stem}")
}
