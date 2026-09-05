//! `docs/api.md` §2 — client mode: the CLI as a client of a daemon on another
//! machine.
//!
//! These drive the shipping binary against a stand-in HTTP daemon, for the
//! reason `policy_cli.rs` gives about the socket: what is asserted is exactly
//! what the verb sends and exactly what it prints, because a caller of this
//! CLI has to be able to trust both. Nothing reaches the network — the
//! stand-in is a loopback listener the test owns.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::Mutex;

const TOKEN: &str = "a-long-random-string";

/// What the stand-in daemon answers, per method.
fn result_for(method: &str, request: &serde_json::Value) -> serde_json::Value {
    match method {
        "status" => serde_json::json!({
            "port": 8787,
            "base_url": "http://127.0.0.1:8787",
            "version": "test",
            "auth": { "connected": false, "accounts": [] },
            "tiers": { "opus": "gpt-5.6-terra" },
            "effort_ceiling": serde_json::Value::Null,
        }),
        "env" => serde_json::json!({
            "variables": [
                ["ANTHROPIC_BASE_URL", "http://127.0.0.1:8787"],
                ["ANTHROPIC_AUTH_TOKEN", "unused"],
                ["ANTHROPIC_DEFAULT_OPUS_MODEL", "gpt-5.6-terra"],
            ],
            "settings": {},
        }),
        "models" => serde_json::json!({ "models": [], "curated": false }),
        "accounts" => serde_json::json!({ "accounts": [] }),
        "tiers.set" => serde_json::json!({
            "tiers": { "opus": request["params"]["tiers"]["opus"].clone() },
            "persisted": request["params"]["persist"].as_bool().unwrap_or(false),
            "account": serde_json::Value::Null,
            "detail": "in effect until the daemon stops; the configuration file is unchanged",
        }),
        _ => serde_json::json!({}),
    }
}

/// One request as it arrived, with the authorization header it carried.
type Seen = Arc<Mutex<Vec<(serde_json::Value, Option<String>)>>>;

/// The requests a socket stand-in recorded.
type Asked = Arc<Mutex<Vec<serde_json::Value>>>;

struct StandIn {
    url: String,
    /// Every request whole, plus the authorization header it arrived with —
    /// the token has to travel, and it has to travel in the one header the
    /// ingress already reads.
    seen: Seen,
}

impl StandIn {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let recorded = Arc::clone(&recorded);
                std::thread::spawn(move || serve(stream, &recorded));
            }
        });

        Self { url, seen }
    }

    fn methods(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(request, _)| {
                request
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect()
    }

    fn request(&self, method: &str) -> serde_json::Value {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|(request, _)| request["method"] == method)
            .map(|(request, _)| request.clone())
            .unwrap_or_else(|| panic!("`{method}` was never asked for"))
    }

    fn authorization(&self) -> Option<String> {
        self.seen
            .lock()
            .unwrap()
            .first()
            .and_then(|(_, auth)| auth.clone())
    }
}

/// One connection, one or more requests. Enough HTTP to be a daemon and no
/// more: the point is what the CLI sends, not a second web server.
fn serve(mut stream: std::net::TcpStream, recorded: &Seen) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }

        let mut length = 0_usize;
        let mut authorization = None;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let lower = header.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
            if lower.starts_with("authorization:") {
                authorization = header
                    .split_once(':')
                    .map(|(_, value)| value.trim().to_owned());
            }
        }

        let mut body = vec![0_u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        recorded
            .lock()
            .unwrap()
            .push((request.clone(), authorization));

        let answer = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": result_for(&method, &request),
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{answer}",
            answer.len()
        );
        if stream.write_all(response.as_bytes()).is_err() {
            return;
        }
        let _ = stream.flush();
    }
}

fn run(dir: &std::path::Path, daemon: Option<&str>, args: &[&str]) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"));
    command
        .args(args)
        .env("PROXENOS_HOME", dir)
        .env("TMPDIR", dir);
    if let Some(daemon) = daemon {
        command
            .env("PROXENOS_DAEMON", daemon)
            .env("PROXENOS_TOKEN", TOKEN);
    }
    command.output().expect("the binary should run")
}

/// A read verb reaches the remote daemon, and says where it is: a front-end
/// reads `daemon_at` to show "connected to macbook", and a local daemon leaves
/// the field out rather than reporting itself.
#[test]
fn a_read_verb_dials_the_remote_daemon_and_says_where_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    let output = run(dir.path(), Some(&daemon.url), &["status", "--json"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "status failed: {stdout}");
    assert_eq!(daemon.methods(), vec!["status".to_owned()]);
    let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(payload["daemon_at"], daemon.url);
}

/// And the rendered report says it too, since that is what an operator reads.
#[test]
fn the_rendered_status_names_the_remote_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    let output = run(dir.path(), Some(&daemon.url), &["status"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "status failed: {stdout}");
    assert!(
        stdout.contains(&format!("daemon at  {}", daemon.url)),
        "{stdout}"
    );
}

/// Local is unchanged: no `daemon_at`, and nothing dialed over HTTP.
#[test]
fn a_local_daemon_reports_no_address() {
    let dir = tempfile::tempdir().unwrap();
    let (_requests, server) = local_stand_in(&dir.path().join("proxenos.sock"), 2);

    let output = run(dir.path(), None, &["status", "--json"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "status failed: {stdout}");
    let payload: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(payload.get("daemon_at").is_none(), "{payload}");

    // And nothing about a daemon elsewhere is printed either.
    let rendered = run(dir.path(), None, &["status"]);
    let printed = String::from_utf8_lossy(&rendered.stdout);
    assert!(rendered.status.success(), "status failed: {printed}");
    assert!(
        !printed.contains("daemon at"),
        "a local daemon should say nothing about where it is: {printed}"
    );
    server.join().unwrap();
}

/// The token travels in the header the ingress already reads, under the
/// `proxenos-token:` tag — one grammar for both surfaces.
#[test]
fn the_token_travels_in_the_authorization_header() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    run(dir.path(), Some(&daemon.url), &["status"]);

    assert_eq!(
        daemon.authorization().as_deref(),
        Some(format!("Bearer proxenos-token:{TOKEN}").as_str())
    );
}

/// A setter reaches the remote daemon with exactly what was typed. `--persist`
/// writes the file on the daemon's machine, which is where the configuration
/// it changes lives.
#[test]
fn a_setter_reaches_the_remote_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    let output = run(
        dir.path(),
        Some(&daemon.url),
        &["tiers", "set", "opus", "gpt-5.6-terra", "--persist"],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "tiers set failed: {stdout}");
    let sent = daemon.request("tiers.set");
    assert_eq!(sent["params"]["tiers"]["opus"], "gpt-5.6-terra");
    assert_eq!(sent["params"]["persist"], true);
}

/// `exec` points the client at the remote daemon and hands it the token in the
/// one header the client sends — beside the account tag, in the same value.
#[test]
fn exec_points_the_client_at_the_remote_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    let output = run(dir.path(), Some(&daemon.url), &["exec", "/usr/bin/env"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "exec failed: {stdout}");
    assert!(
        stdout.contains(&format!("ANTHROPIC_BASE_URL={}", daemon.url)),
        "the child should talk to the remote daemon: {stdout}"
    );
    assert!(
        stdout.contains(&format!("ANTHROPIC_AUTH_TOKEN=proxenos-token:{TOKEN}")),
        "the child should carry the token: {stdout}"
    );
}

/// The two travel together, in the one header the client offers.
#[test]
fn exec_carries_the_token_beside_the_account_tag() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();
    // The stand-in holds no accounts, so `--account` is refused before
    // anything starts — which is itself the §2.3 behaviour. Assert the
    // refusal names the account rather than the daemon.
    let output = run(
        dir.path(),
        Some(&daemon.url),
        &["exec", "--account", "work", "/usr/bin/env"],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("`work` names no stored account"),
        "{stderr}"
    );

    // And the value the launcher would have built, asserted where it is built.
    assert_eq!(
        proxenos::ingress::auth_token_value(Some(TOKEN), Some("work")),
        format!("proxenos-token:{TOKEN} proxenos-account:work")
    );
}

/// **Local callers need no token, and a token in the environment does not
/// make one appear.** The loopback door asks nothing (`api.md` §1), so a local
/// `exec` is exactly what it was before tokens existed — asserted here rather
/// than read off the code, because the failure it guards against is a local
/// session shut out of its own daemon.
#[test]
fn a_local_exec_carries_no_token_even_with_one_in_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    // Four calls: `env`, `models`, `accounts` for the serving line, and the
    // socket has to stay up across all of them.
    let (_asked, server) = local_stand_in(&dir.path().join("proxenos.sock"), 4);

    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"));
    let output = command
        .args(["exec", "/usr/bin/env"])
        .env("PROXENOS_HOME", dir.path())
        .env("TMPDIR", dir.path())
        // Set, and deliberately ignored: without PROXENOS_DAEMON this CLI is
        // local, and a local daemon's loopback door demands nothing.
        .env("PROXENOS_TOKEN", TOKEN)
        .env_remove("PROXENOS_DAEMON")
        .output()
        .expect("the binary should run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "exec failed: {stdout}");
    assert!(
        stdout.contains("ANTHROPIC_BASE_URL=http://127.0.0.1:8787"),
        "a local launch keeps the daemon's own base URL: {stdout}"
    );
    assert!(
        stdout.contains("ANTHROPIC_AUTH_TOKEN=unused"),
        "a local launch sends the word the client needs and this daemon ignores: {stdout}"
    );
    // Scoped to what this claims. `PROXENOS_TOKEN` is in the child's
    // environment because the child INHERITS this process's — that is a
    // variable the operator exported, not something the launcher put there —
    // so the assertion is that nothing the launcher sets carries it.
    for line in stdout.lines().filter(|line| line.starts_with("ANTHROPIC_")) {
        assert!(
            !line.contains(TOKEN),
            "a local launch must not put the token in {line}"
        );
    }
    drop(server);
}

/// `env` prints the remote base URL and never the token: these exports are
/// what an operator pastes into a shell, and a shell's history.
#[test]
fn env_points_at_the_remote_daemon_without_printing_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    let output = run(dir.path(), Some(&daemon.url), &["env"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "env failed: {stdout}");
    assert!(
        stdout.contains(&format!("export ANTHROPIC_BASE_URL={}", daemon.url)),
        "{stdout}"
    );
    assert!(!stdout.contains(TOKEN), "env printed the token: {stdout}");
    assert!(
        !stdout
            .lines()
            .any(|line| line.starts_with("export ANTHROPIC_AUTH_TOKEN=")),
        "the export should be left out entirely: {stdout}"
    );
    assert!(
        stdout.contains("$PROXENOS_TOKEN"),
        "it should say how to set it: {stdout}"
    );
}

/// `status` never prints the token either, in any of its forms.
#[test]
fn status_never_prints_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    for args in [&["status"][..], &["status", "--json"][..]] {
        let output = run(dir.path(), Some(&daemon.url), args);
        let printed = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !printed.contains(TOKEN),
            "{args:?} printed the token: {printed}"
        );
    }
}

/// `settings` is refused rather than printed: the document is one blob a
/// client reads whole, so it either carries the secret or does not work.
#[test]
fn settings_is_refused_in_client_mode() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    let output = run(dir.path(), Some(&daemon.url), &["settings"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("proxenos exec"), "{stderr}");
    assert!(!stderr.contains(TOKEN), "{stderr}");
}

/// No flag anywhere takes the token. An argument is visible in `ps` to every
/// process on the machine, which is the one place a secret must never be.
#[test]
fn no_verb_takes_the_token_as_an_argument() {
    let dir = tempfile::tempdir().unwrap();
    for verb in ["--help", "status", "exec", "tiers", "accounts", "run"] {
        let output = run(dir.path(), None, &[verb, "--help"]);
        let help = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
        assert!(
            !help.contains("--token"),
            "`{verb} --help` offers a token flag"
        );
    }
}

/// The verbs that act on the daemon's own machine are refused, by name, with
/// the sentence that says where to run them. Forwarded, each would happen on
/// the wrong machine and look like it worked.
#[test]
fn the_verbs_that_belong_to_the_daemons_machine_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    for args in [
        &["run"][..],
        &["start"][..],
        &["accounts", "login", "--provider", "codex", "work"][..],
        &["accounts", "add-key", "work", "--provider", "codex"][..],
        &["supervisor", "status"][..],
    ] {
        let output = run(dir.path(), Some(&daemon.url), args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "`{args:?}` should be refused in client mode"
        );
        assert!(
            stderr.contains(&daemon.url) && stderr.contains("Run it on that host"),
            "`{args:?}` should say where to run it: {stderr}"
        );
    }
}

/// `stop` is not refused: it is a control method like any other, the daemon
/// acts on it itself, and an operator who can already switch that daemon's
/// account can stop it.
#[test]
fn stop_is_allowed_in_client_mode() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = StandIn::start();

    let output = run(dir.path(), Some(&daemon.url), &["stop"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stop failed: {stderr}");
    assert!(
        daemon.methods().contains(&"shutdown".to_owned()),
        "stop should have asked: {:?}",
        daemon.methods()
    );
}

/// A daemon that does not answer is one sentence naming the address, not a
/// connection error from somewhere inside reqwest.
#[test]
fn an_unreachable_daemon_is_named() {
    let dir = tempfile::tempdir().unwrap();

    let output = run(dir.path(), Some("http://127.0.0.1:1"), &["status"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("http://127.0.0.1:1/control"), "{stderr}");
    assert!(stderr.contains("PROXENOS_DAEMON"), "{stderr}");
}

/// A stand-in on the §3 socket, for the local half of these assertions.
fn local_stand_in(
    socket: &std::path::Path,
    connections: usize,
) -> (Asked, std::thread::JoinHandle<()>) {
    use std::os::unix::net::UnixListener;

    let listener = UnixListener::bind(socket).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);

    let handle = std::thread::spawn(move || {
        for stream in listener.incoming().take(connections) {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
            let method = request
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            recorded.lock().unwrap().push(request.clone());
            let _ = writeln!(
                stream,
                "{}",
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": result_for(&method, &request),
                })
            );
            let _ = stream.flush();
        }
    });

    (requests, handle)
}
