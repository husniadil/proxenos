//! What the daemon's own lifetime is made of: `run`, `stop`, and the recording
//! run every capture mode shares.

use super::Answering;
use super::RESTART_WINDOW;
use super::STOP_WINDOW;
use super::account_store;
use super::answering;
use super::serving_account;
use super::version_of;
use super::watch;
use crate::cli::RunArgs;
use crate::cli::StartArgs;
use anyhow::Result;
use anyhow::bail;
use proxenos::config::Config;
use proxenos::control;
use proxenos::daemon;
use proxenos::ingress::AppState;
use proxenos::upstream::http::HttpTransport;
use std::sync::Arc;

/// Ask the running daemon to stop, then say what actually happened.
///
/// The observation is the useful half. Under a supervisor a stop is how a
/// running daemon is replaced by the build on disk, which is the answer to "the
/// binary is new and nothing changed". Whether anything restarts it is not this
/// verb's doing, so it reports what it saw rather than claiming to have done it.
///
/// **It watches the instance, not the silence.** A socket falling quiet is a
/// statement about timing: a supervisor quick enough leaves no gap to see, and
/// one that throttles leaves a gap longer than any sensible wait. The daemon
/// mints an id at startup, so a different id is a different process no matter
/// how the two overlapped.
/// Re-read config.toml into the running daemon, and print both halves of the
/// answer.
///
/// Both, always. A reload that printed only what it applied would leave an
/// operator who edited `port` believing it took effect, and the whole point of
/// the verb is that the daemon keeps running.
pub(crate) async fn reload() -> Result<()> {
    let result = control::call(&control::default_path(), "config.reload", None).await?;
    println!("{}", proxenos::render::reloaded_config(&result));
    Ok(())
}

pub(crate) async fn stop() -> Result<()> {
    let before = answering().await;
    // Read from the daemon that is about to go, because it is the only one
    // that can say what started it. What answers afterwards is a different
    // process, and on a supervised machine it is often not answering yet.
    let supervision = before.as_ref().and_then(|now| now.supervised);

    let result = match control::call(&control::default_path(), "shutdown", None).await {
        Ok(result) => result,
        // The chicken and the egg, stated rather than papered over. This verb
        // exists to replace a daemon older than the binary asking, and it
        // cannot replace one older than the verb: that daemon has no method to
        // ask. Nothing here can fix it, so it says which situation this is.
        Err(error) if error.status == axum::http::StatusCode::NOT_FOUND => {
            anyhow::bail!(
                "The running daemon is from an older build and has no `stop`. End that \
                 process however it was started, then `proxenos run`."
            );
        }
        Err(error) => return Err(error.into()),
    };
    let was = version_of(&result).unwrap_or_else(|| "the running daemon".to_owned());

    // Gone, or already replaced. An id that changed says the second even when
    // there was never a moment where nothing answered.
    let mut seen = None;
    let went = watch(STOP_WINDOW, |now| {
        seen = now.clone();
        now.is_none() || (now.is_some() && now != before)
    })
    .await;

    if !went {
        println!("asked {was} to stop; it is still answering after {STOP_WINDOW:?}");
        return Ok(());
    }

    if let Some(replacement) = seen {
        println!("{}", report_replacement(&was, &replacement, supervision));
        return Ok(());
    }

    // It went and nothing has taken its place yet. Wait long enough to outlast
    // a throttled respawn before saying nothing did.
    let mut back = None;
    watch(RESTART_WINDOW, |now| {
        back = now;
        back.is_some()
    })
    .await;

    match back {
        Some(replacement) => println!("{}", report_replacement(&was, &replacement, supervision)),
        None => println!("stopped {was}; nothing started it again within {RESTART_WINDOW:?}"),
    }
    Ok(())
}

/// What the stop produced, as a sentence.
///
/// The supervisor is named where the daemon that went said it had one. Under
/// launchd a stop *is* how a running daemon is replaced by the build on disk,
/// so "something started it again" describes the mechanism the operator
/// installed as though it were a coincidence. Where supervision was not
/// established — a platform with no supervisor here, or a process launchd
/// started under some other label — it stays "something", because naming
/// launchd there would be a claim nothing checked.
///
/// The build is named unless the string is identical, which with a build id
/// on it (§3) means the same build and not merely the same version.
fn report_replacement(was: &str, now: &Answering, supervised: Option<bool>) -> String {
    let actor = if supervised == Some(true) {
        "launchd"
    } else {
        "something"
    };
    if now.version == was {
        format!("stopped {was}; {actor} started it again, on the same build")
    } else {
        format!("stopped {was}; {actor} started it again as {}", now.version)
    }
}

/// What a run captures, beyond the empty streams §5.4 always records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    /// Nothing extra.
    Nothing,
    /// What the client sent, before translation. Free.
    Ingress,
    /// The exchange with the backend. Spends quota, and holds both halves of
    /// what a fixture needs.
    Upstream,
}

pub(crate) async fn run(args: RunArgs) -> Result<()> {
    run_with(args, Capture::Nothing).await
}

/// What to say about a daemon that is already answering.
///
/// A pure function over what the socket reported, so the sentence is a test
/// rather than something read off a machine that happens to have a daemon on
/// it. The process and the supervision are said where the payload carries
/// them and left out where it does not: inventing a pid for a daemon that
/// reports none would be worse than a shorter line, and "not supervised" is a
/// claim only a daemon that can tell is entitled to make.
fn already_running(now: &Answering) -> String {
    let pid = now
        .pid
        .map_or_else(String::new, |pid| format!(" (pid {pid})"));
    let supervised = match now.supervised {
        Some(true) => ", supervised",
        Some(false) => ", not supervised",
        None => "",
    };
    format!("already running: {}{pid}{supervised}", now.version)
}

/// Start the daemon as its own process and return once it answers.
///
/// The child is a plain `run` of this same binary in its own process group,
/// its output appended to a log file, because a backgrounded process's
/// terminal is gone the moment this command returns. Success is observed, not
/// assumed: this returns 0 only once the daemon answers the control socket,
/// and a child that dies first has its log quoted rather than summarized.
///
/// A daemon already answering is the state this verb was asked to produce, so
/// it says what is there and exits 0 rather than failing. Nothing is started:
/// the control socket is one per path, and a second daemon would take over the
/// first one's socket file while the first kept the port.
pub(crate) async fn start(args: StartArgs) -> Result<()> {
    use anyhow::Context;

    let socket = control::default_path();
    if let Some(now) = answering().await {
        println!("{}", already_running(&now));
        return Ok(());
    }

    let log_path = proxenos::config::config_dir().join("daemon.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("could not open the log at {}", log_path.display()))?;
    let stderr_log = log.try_clone().context("could not clone the log handle")?;

    // Where this start's writes begin. The log is appended across starts, so a
    // failure must quote only what this child wrote — the tail of an earlier
    // run presented as the reason this one died would be a lie with evidence.
    let baseline = log.metadata().map(|meta| meta.len()).unwrap_or(0);

    let own = std::env::current_exe().context("could not find this binary's own path")?;
    let mut command = std::process::Command::new(own);
    command.arg("run");
    if let Some(port) = args.port {
        command.args(["--port", &port.to_string()]);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(stderr_log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: no console window, and
        // the terminal's Ctrl-C no longer reaches it.
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    let mut child = command.spawn().context("could not start the daemon")?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        // Exit first: a child that died must be reported from its log, and
        // checking the socket first would race an unrelated answerer.
        if let Ok(Some(status)) = child.try_wait() {
            bail!(
                "the daemon exited before it started answering ({status}). Its log ends with:\n{}",
                log_tail(&log_path, baseline)
            );
        }
        if control::call(&socket, "status", None).await.is_ok() {
            println!(
                "daemon running (pid {}), logging to {}\nstop it with `proxenos stop`",
                child.id(),
                log_path.display()
            );
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            // Ended rather than left behind: exiting nonzero while the daemon
            // quietly finishes coming up would leave the report and the
            // machine disagreeing about whether anything is running.
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "the daemon did not start answering within 10s and was ended. Its log ends with:\n{}",
                log_tail(&log_path, baseline)
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// The last few lines the daemon wrote after `since`, for an error message
/// that shows the failure instead of describing it.
fn log_tail(path: &std::path::Path, since: u64) -> String {
    let content = std::fs::read(path).unwrap_or_default();
    let start = usize::try_from(since)
        .unwrap_or(usize::MAX)
        .min(content.len());
    let text = String::from_utf8_lossy(content.get(start..).unwrap_or(&[]));
    let count = text.lines().count();
    if count == 0 {
        return "(nothing was written this start)".to_owned();
    }
    text.lines()
        .skip(count.saturating_sub(12))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) async fn run_with(args: RunArgs, capture: Capture) -> Result<()> {
    let switches = Arc::new(proxenos::recorder::Switches::new(match capture {
        Capture::Nothing => None,
        Capture::Ingress => Some(proxenos::recorder::Mode::Ingress),
        Capture::Upstream => Some(proxenos::recorder::Mode::Upstream),
    }));
    let config = Config::load()?;
    let port = args.port.unwrap_or(config.port);

    // Values that parse but cannot work, refused before binding rather than
    // clamped — a clamp makes an operator's mistake look like it was accepted.
    config.validate()?;

    let credentials: Arc<dyn proxenos::auth::store::AccountStore> = Arc::new(account_store()?);

    // §8.3 — a quota figure is filed under the account that earned it, and an
    // unpinned turn's account is whoever this store has selected when the turn
    // is served.
    // §6.1 — the token tally is read back at startup and written as it grows.
    // The quota snapshots are not persisted: upstream still holds those and an
    // ask recovers them, where a percentage restored from disk would describe
    // a window that may have reset since.
    let usage = Arc::new(
        proxenos::usage::UsageStore::for_accounts(Arc::clone(&credentials))
            .tallying_at(proxenos::config::tally_path())
            .remembering_at(proxenos::config::quota_path()),
    );

    // Bound to the same store, for the same reason: a turn made as "whoever is
    // serving" has to be filed under the account that was serving when it
    // happened, not whichever one is selected by the time somebody asks.
    let refusals = Arc::new(proxenos::auth::refusals::Refusals::for_accounts(
        Arc::clone(&credentials),
    ));

    // Which account's mapping is in force. Read before the mapping is resolved
    // rather than after, because a catalog is one account's menu (§7.0) and a
    // mapping written for one account can name a model another is not offered.
    // Nothing selected — no login yet — takes the shared tables, which is the
    // only answer available and the one a first run wants.
    let serving = serving_account(&credentials);

    // Refused before binding: a daemon that starts with an incomplete mapping
    // breaks WebFetch in a way that looks unrelated to tier mapping (§7.1).
    let mut tiers = config
        .tiers_for(serving.as_deref())
        .resolve(config.cross_account_policy())?;

    // Also refused before binding, so a mistyped ceiling is caught at startup
    // rather than silently spending at full rate.
    let effort_ceiling = config.effort_ceiling_for(serving.as_deref())?;
    match effort_ceiling {
        Some(effort) => tracing::info!(?effort, "reasoning effort is capped"),
        None => tracing::info!("reasoning effort is whatever the client asks for"),
    }

    let listener = daemon::bind(port).await?;
    let addr = listener.local_addr()?;
    tracing::info!(%addr, "listening");

    let tokens = Arc::new(proxenos::auth::grants::Grants::new(
        Arc::clone(&credentials) as Arc<dyn proxenos::auth::store::CredentialStore>,
        Arc::new(proxenos::auth::grants::SystemClock),
    ));

    // One authorizer for every path that authenticates: it reads the store per
    // request, so a switch or a login reaches the next request with nothing
    // rebuilt.
    let authorizer: Arc<dyn proxenos::auth::authorize::Authorizer> =
        Arc::new(proxenos::auth::authorize::AccountAuthorizer::new(
            Arc::clone(&credentials),
            Arc::clone(&tokens),
        ));

    // §7.0 — fetched with credentials when there are any. An unreachable
    // catalog falls back rather than failing: fetch failure is not evidence
    // that a model went away (§7.1), and a daemon that will not start because
    // the network blinked is the worse failure.
    //
    // In a slot holding both endpoints, not a value: a catalog describes the
    // plan of the account it was fetched for, so switching accounts on a
    // running daemon has to be able to replace it — from the endpoint that
    // account's kind belongs to, which is why the pair travels together.
    // Shared with the ingress, or a replacement would move what the socket
    // reports and not what routes turns.
    let catalog = Arc::new(proxenos::catalog::CatalogSource::new(
        proxenos::catalog::Catalog::fallback(),
        config.upstream.catalog.clone(),
        config.upstream.key.catalog.clone(),
        config.upstream.client_version.clone(),
        config.upstream.effective_window_percent,
    ));
    match authorizer.authorize(None).await {
        Ok(authorization) => {
            catalog.refresh(&authorization).await;
        }
        Err(error) => {
            tracing::info!(%error, "not authenticated; the model list is the fallback");
        }
    }

    // A shipped default naming a model this account cannot see is replaced
    // rather than refused: `gpt-5.6-sol` is plan-gated, so the default mapping
    // would otherwise fail to start for most accounts. A model the operator
    // stated is left alone and validated below.
    let substituted = catalog
        .current()
        .substitute_unavailable_defaults(&mut tiers);
    if !substituted.is_empty() {
        tracing::info!(
            tiers = substituted.join(", "),
            "some default tier mappings were substituted for models this account has"
        );
    }

    // An unknown model id is refused here, and the error names what the catalog
    // does have — which is the fastest way to find the id you actually meant.
    //
    // Over the tiers this catalog is a menu for. A tier whose turns are relayed
    // names a model on the second provider (§9.1), absent from this list by
    // construction, and refusing the daemon's start over it would name a menu
    // the id was never offered on.
    catalog
        .current()
        .validate(&proxenos::upstream::relay::validated_models(
            &credentials.accounts().unwrap_or_default(),
            &tiers,
        ))?;

    // One policy, shared. The control socket can move the tier mapping and the
    // effort ceiling on a running daemon, and the ingress routes turns from the
    // same value — so a change moves what actually happens rather than only
    // what `status` reports.
    let policy = Arc::new(proxenos::policy::Policy::new(
        proxenos::policy::Snapshot::new(
            tiers.clone(),
            effort_ceiling,
            config.cross_account_policy(),
        ),
    ));

    // One signal, shared: asking over the socket has to move this process, not
    // merely answer about it.
    let shutdown = Arc::new(proxenos::daemon::Shutdown::default());

    // One store, shared with the ingress: a switch that cleared a set of
    // conversations the ingress does not serve would clear nothing.
    let sessions = Arc::new(proxenos::session::SessionStore::new());

    let control_state = proxenos::control::handler::ControlState {
        port: addr.port(),
        policy: Arc::clone(&policy),
        catalog: Arc::clone(&catalog),
        credentials: Arc::clone(&credentials),
        capture: Arc::clone(&switches),
        usage: Arc::clone(&usage),
        refusals: Arc::clone(&refusals),
        config: Arc::new(config.clone()),
        shutdown: Arc::clone(&shutdown),
        tokens: Some(Arc::clone(&tokens)),
        usage_endpoint: config.upstream.usage.clone(),
        anthropic_usage_endpoint: config.upstream.anthropic.usage.clone(),
        sessions: Arc::clone(&sessions),
        config_path: Some(proxenos::config::config_path()),
    };
    let socket_path = control::default_path();
    tokio::spawn(async move {
        if let Err(error) = control::serve(&socket_path, control_state).await {
            tracing::warn!(%error, "the control socket stopped");
        }
    });

    // Credentials are fetched per request rather than captured here, so a
    // refresh part-way through a session is transparent to both transports.
    // One conduit per conversation, built on its first turn. The binding is per
    // session because latching, the pooled connection, and the previous
    // response id all belong to one conversation.
    let websocket_enabled = config.transport.websocket;
    let compression = config.transport.compression;
    let conduit_authorizer = Arc::clone(&authorizer);
    let conduit_credentials = Arc::clone(&credentials);
    let conduit_endpoint = config.upstream.endpoint.clone();
    let conduit_websocket = config.upstream.websocket.clone();
    let conduit_key = config.upstream.key.endpoint.clone();
    let conduits: proxenos::ingress::ConduitFactory = Arc::new(move |session_id| {
        // Which endpoint this conversation belongs to is decided by the
        // account serving turns when it starts. A switch drops every live
        // session (§3), so a conversation never outlives the kind it was
        // opened for.
        let kind = proxenos::auth::authorize::selected_kind(&conduit_credentials);
        let (endpoint, socket) = match kind {
            proxenos::auth::authorize::Kind::Key => (&conduit_key, None),
            proxenos::auth::authorize::Kind::Subscription => {
                (&conduit_endpoint, Some(&conduit_websocket))
            }
        };

        let http = Arc::new(
            HttpTransport::new(endpoint)
                .for_endpoint(kind)
                .with_credentials(Arc::clone(&conduit_authorizer))
                .with_compression(compression),
        );
        let websocket = websocket_enabled.then_some(socket).flatten().map(|socket| {
            Arc::new(
                proxenos::upstream::websocket::WebSocketTransport::new(socket)
                    .with_credentials(Arc::clone(&conduit_authorizer))
                    .with_compression(compression),
            )
        });
        Arc::new(proxenos::upstream::conduit::Conduit::new(
            http, websocket, session_id,
        ))
    });

    // The fetched catalog, not a fresh fallback. Everything that depends on
    // knowing a model — the window guard (§7.2) and both effort caps (§2.7) —
    // reads it from here, and a fallback entry states neither, so handing one
    // over leaves all three silently doing nothing.
    tracing::info!(
        models = catalog.current().ids().len(),
        authoritative = catalog.current().authoritative,
        "model catalog in use"
    );

    let state = AppState {
        policy: Arc::clone(&policy),
        catalog: Arc::clone(&catalog),
        // Only reached if the factory is absent, which it is not here.
        transport: Arc::new(
            HttpTransport::new(&config.upstream.endpoint).with_credentials(Arc::clone(&authorizer)),
        ),
        conduits: Some(conduits),
        // §5.4 — empty streams are recorded whether or not capture was asked
        // for, because an empty stream is always a defect and is otherwise
        // invisible.
        recorder: Some(proxenos::recorder::Recorder::new(
            proxenos::recorder::Recorder::default_directory(),
        )),
        capture: Arc::clone(&switches),
        usage: Arc::clone(&usage),
        refusals: Arc::clone(&refusals),
        instructions: Arc::new(config.instructions.clone()),
        sessions,
        // §9 — always built, never conditional on what the store holds today.
        // Whether a turn takes this path is decided per request from the
        // mapping and the account behind it, so an account stored while the
        // daemon runs is routed by the next turn rather than the next start.
        relay: Some(Arc::new(proxenos::upstream::relay::Relay::new(
            &config.upstream.anthropic.endpoint,
            Arc::clone(&credentials),
            Arc::clone(&authorizer),
        ))),
    };

    // Whichever comes first: the listener stopping on its own, or a stop asked
    // for over the socket. An in-flight turn is cut — a person typing `stop`
    // means it, and the client's own retry handles a dropped connection.
    tokio::select! {
        result = daemon::serve(listener, state) => result?,
        () = shutdown.wait() => tracing::info!("stopping, as asked over the control socket"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answering_as(version: &str, pid: Option<u64>, supervised: Option<bool>) -> Answering {
        Answering {
            version: version.to_owned(),
            instance: Some("an id".to_owned()),
            pid,
            supervised,
        }
    }

    /// The whole answer to `start` where one is already up: what build, which
    /// process, and whether anything brings it back.
    #[test]
    fn an_answering_daemon_is_named_rather_than_replaced() {
        assert_eq!(
            already_running(&answering_as("0.12.0", Some(4711), Some(true))),
            "already running: 0.12.0 (pid 4711), supervised"
        );
        assert_eq!(
            already_running(&answering_as("0.12.0", Some(4711), Some(false))),
            "already running: 0.12.0 (pid 4711), not supervised"
        );
    }

    /// A supervised daemon's replacement is the supervisor's doing, and saying
    /// so is the answer to "the binary is new and nothing changed".
    #[test]
    fn a_supervised_stop_names_the_supervisor() {
        let now = answering_as("0.12.1", Some(4712), Some(true));
        assert_eq!(
            report_replacement("0.12.0", &now, Some(true)),
            "stopped 0.12.0; launchd started it again as 0.12.1"
        );
        assert_eq!(
            report_replacement("0.12.1", &now, Some(true)),
            "stopped 0.12.1; launchd started it again, on the same build"
        );
    }

    /// Unsupervised, and unknown, keep the word that claims nothing. A
    /// platform where supervision cannot be established has no standing to
    /// name a supervisor it never saw.
    #[test]
    fn an_unestablished_supervisor_is_not_named() {
        let now = answering_as("0.12.1", Some(4712), None);
        assert_eq!(
            report_replacement("0.12.0", &now, None),
            "stopped 0.12.0; something started it again as 0.12.1"
        );
        assert_eq!(
            report_replacement("0.12.0", &now, Some(false)),
            "stopped 0.12.0; something started it again as 0.12.1"
        );
        assert_eq!(
            report_replacement("0.12.1", &now, Some(false)),
            "stopped 0.12.1; something started it again, on the same build"
        );
    }

    /// A daemon that cannot answer half the question is not made to. The
    /// platform with no supervisor here says nothing about supervision, and a
    /// build predating `pid` gets no invented number.
    #[test]
    fn what_the_daemon_cannot_say_is_left_unsaid() {
        assert_eq!(
            already_running(&answering_as("0.12.0", Some(4711), None)),
            "already running: 0.12.0 (pid 4711)"
        );
        assert_eq!(
            already_running(&answering_as("0.11.0", None, None)),
            "already running: 0.11.0"
        );
    }
}
