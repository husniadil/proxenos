//! `docs/proxy-behavior.md` §8.4 — asking the owning program to refresh.
//!
//! The one thing this side may do about a lapsed borrowed grant. It does not
//! refresh anything itself: it runs the program that owns the profile, waits
//! for it to exit, and reads the profile again. The rotation happens inside
//! that program, which is the only process allowed to perform it.
//!
//! Two rules, both measured, both load-bearing:
//!
//! - **Only Claude is ever asked.** A Codex grant is refreshed only by a real
//!   turn (`codex exec`), which spends the operator's quota and rotates the
//!   refresh token — and one failing run was seen sending fourteen refresh
//!   requests in a row. Its access token also lasts ten days, so the case
//!   barely arises. Nothing here runs it.
//! - **A dead refresh token is never poked.** When the client fails to
//!   refresh, it overwrites its stored item with an empty access token and a
//!   zero expiry. Asking a profile whose refresh token has already lapsed
//!   therefore destroys what is left of it, and `refreshTokenExpiresAt` is
//!   readable locally, for free, before anything is run.

use crate::auth::store::Provider;
use crate::error::ProxyError;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

/// How long to wait for the client before giving up on it.
///
/// A bound rather than a guess at how long it takes: the client reads a
/// keychain item, exchanges a token and exits, and anything past this is a
/// process that is not going to finish. Waiting forever would hang whatever
/// asked.
pub const DEADLINE: Duration = Duration::from_secs(60);

/// What the client is called when the operator has not said where it is.
///
/// A bare name, so it is resolved through the running daemon's `PATH`. That is
/// the shell's `PATH` only when the daemon was started from a shell;
/// `claude_program` in the configuration file is how a daemon started by
/// launchd is told where the client actually is (§4).
pub const PROGRAM: &str = "claude";

/// What to do about a grant that cannot be spent.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// It can be spent. Nothing to do.
    Usable,
    /// Run the owning client once, then read the profile again.
    Ask,
    /// Nothing here can improve it, and this is what to tell the operator.
    Hopeless(&'static str),
}

/// Whether asking is worth anything for this grant.
///
/// `refresh_token_expires_at` is `None` on a profile written before the client
/// recorded it. That is treated as worth asking: unknown is not the same as
/// dead, and if it turns out to be dead the operator has to sign in again
/// either way — which is exactly what they would have to do had they run the
/// client themselves.
pub fn decide(
    provider: Provider,
    expires_at: Option<u64>,
    refresh_token_expires_at: Option<u64>,
    now: u64,
) -> Decision {
    if expires_at.is_some_and(|expiry| expiry > now) {
        return Decision::Usable;
    }
    match provider {
        Provider::Codex => Decision::Hopeless(
            "a Codex grant is refreshed only by a real turn, which spends quota and rotates \
             the token, so nothing here runs one. Open the ChatGPT app or run `codex` once \
             in that profile.",
        ),
        Provider::Anthropic => {
            if refresh_token_expires_at.is_some_and(|expiry| expiry <= now) {
                return Decision::Hopeless(
                    "its refresh token has expired too, and asking the client to refresh \
                     would only blank what is left of the stored grant. Sign in again in \
                     that profile.",
                );
            }
            Decision::Ask
        }
    }
}

/// Runs the program that owns a profile, once.
///
/// A trait so the rules above are tested without a signed-in profile and
/// without spending a turn.
pub trait Client: Send + Sync {
    /// Run it against `config_dir`, returning when it has exited. `None` is
    /// the stock profile: the one it uses with no variable set.
    fn refresh(&self, config_dir: Option<&Path>) -> Result<(), ProxyError>;
}

/// The real one: `claude -p`, with a deadline.
pub struct ClaudeClient {
    program: PathBuf,
    deadline: Duration,
}

impl Default for ClaudeClient {
    fn default() -> Self {
        Self {
            program: PathBuf::from(PROGRAM),
            deadline: DEADLINE,
        }
    }
}

impl ClaudeClient {
    pub fn new(program: impl Into<PathBuf>, deadline: Duration) -> Self {
        Self {
            program: program.into(),
            deadline,
        }
    }
}

impl Client for ClaudeClient {
    fn refresh(&self, config_dir: Option<&Path>) -> Result<(), ProxyError> {
        let mut command = std::process::Command::new(&self.program);
        command
            .arg("-p")
            .arg("ok")
            // The cheapest tier there is. This turn exists to make the client
            // authenticate, and what it answers is thrown away.
            .arg("--model")
            .arg("haiku")
            // Measured: without this the client waits several seconds for
            // input that is never coming, on every run.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(config_dir) = config_dir {
            command.env("CLAUDE_CONFIG_DIR", config_dir);
        }

        let mut child = command.spawn().map_err(|error| {
            ProxyError::authentication(format!(
                "could not run `{}`: {error}",
                self.program.display()
            ))
        })?;

        let started = Instant::now();
        loop {
            match child.try_wait() {
                // Its exit status is deliberately not checked. A refresh that
                // failed leaves the profile exactly as unusable as it was, and
                // the read that follows is what says so — in the vocabulary of
                // the store rather than of a process that was run behind the
                // operator's back.
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {}
                Err(error) => {
                    return Err(ProxyError::authentication(format!(
                        "could not wait for `{}`: {error}",
                        self.program.display()
                    )));
                }
            }
            if started.elapsed() >= self.deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProxyError::authentication(format!(
                    "`{}` did not finish within {} seconds, so the profile was left alone",
                    self.program.display(),
                    self.deadline.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Ask once, under a lock, so ten callers produce one run.
///
/// The lock is held for the whole run and released on the way out. A second
/// caller blocks on it rather than starting its own client, and by the time it
/// acquires the lock the first run has already written whatever it was going
/// to write — so it reads the profile and finds it fresh.
pub fn under_lock(
    client: &dyn Client,
    lock_path: &Path,
    config_dir: Option<&Path>,
) -> Result<(), ProxyError> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            ProxyError::authentication(format!("could not create {}: {error}", parent.display()))
        })?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(lock_path)
        .map_err(|error| {
            ProxyError::authentication(format!("could not open {}: {error}", lock_path.display()))
        })?;
    file.lock().map_err(|error| {
        ProxyError::authentication(format!("could not lock {}: {error}", lock_path.display()))
    })?;

    let outcome = client.refresh(config_dir);
    // Released either way: a lock held past a failure would make the next
    // caller wait for a run that is not happening.
    let _ = file.unlock();
    outcome
}

/// Where one profile's lock lives, named by the profile it belongs to.
///
/// Per profile rather than one for the daemon: two profiles refreshing at once
/// are two clients writing two different keychain items, and nothing about
/// that needs serialising.
pub fn lock_path(config_dir: &Path, profile: &str) -> std::path::PathBuf {
    config_dir.join(format!("refresh-{profile}.lock"))
}
