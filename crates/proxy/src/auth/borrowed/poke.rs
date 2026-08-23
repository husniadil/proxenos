//! `docs/proxy-behavior.md` §8.4 — asking the owning program to refresh.
//!
//! The one thing this side may do about a lapsed borrowed grant. It does not
//! refresh anything itself: it runs the program that owns the profile, waits
//! for it to exit, and reads the profile again. The rotation happens inside
//! that program, which is the only process allowed to perform it.
//!
//! Two rules, both load-bearing:
//!
//! - **Each provider is asked through its own program.** Claude refreshes on a
//!   cheap `claude -p` turn; Codex refreshes on a cheap `codex exec` turn. Both
//!   spend a little of the operator's quota and rotate the grant, and in both
//!   cases the rotation happens *inside* that program — the one process allowed
//!   to write the profile — so the daemon never exchanges a borrowed refresh
//!   token itself. A Codex access token lasts ten days, so the case is rarer
//!   there, but a borrowed profile no other session drives reaches it all the
//!   same.
//! - **A dead refresh token is never poked.** When a client fails to refresh,
//!   it can overwrite its stored item with an empty access token and a zero
//!   expiry. Asking a profile whose refresh token has already lapsed therefore
//!   destroys what is left of it, and the deadline is readable locally, for
//!   free, before anything is run.

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

/// What the Codex client is called when the operator has not said where it is.
/// A bare name, resolved through the daemon's `PATH` like `PROGRAM`; a daemon
/// started by launchd is told where it is with `codex_program` (§4).
pub const CODEX_PROGRAM: &str = "codex";

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
    // The same split for both providers: a lapsed access token is worth asking
    // about, unless the refresh token behind it has lapsed too. `provider` is
    // kept because the run itself differs — a different program, a different
    // turn — and the caller carries it to the client.
    let _ = provider;
    if refresh_token_expires_at.is_some_and(|expiry| expiry <= now) {
        return Decision::Hopeless(
            "its refresh token has expired too, and asking the owning program to refresh \
             would only blank what is left of the stored grant. Sign in again in that \
             profile.",
        );
    }
    Decision::Ask
}

/// Runs the program that owns a profile, once.
///
/// A trait so the rules above are tested without a signed-in profile and
/// without spending a turn.
pub trait Client: Send + Sync {
    /// Run the program that owns `provider`'s profile against `config_dir`,
    /// returning when it has exited. `None` is the stock profile: the one that
    /// program uses with no variable set.
    fn refresh(&self, provider: Provider, config_dir: Option<&Path>) -> Result<(), ProxyError>;
}

/// The real one: a cheap turn against whichever program owns the profile, with
/// a deadline. Claude runs `claude -p ok`; Codex runs `codex exec ok`. Each
/// exists to make that program authenticate — and, where the grant is stale,
/// rotate it — and what it answers is thrown away.
pub struct OwningClient {
    claude_program: PathBuf,
    codex_program: PathBuf,
    deadline: Duration,
}

impl Default for OwningClient {
    fn default() -> Self {
        Self {
            claude_program: PathBuf::from(PROGRAM),
            codex_program: PathBuf::from(CODEX_PROGRAM),
            deadline: DEADLINE,
        }
    }
}

impl OwningClient {
    pub fn new(
        claude_program: impl Into<PathBuf>,
        codex_program: impl Into<PathBuf>,
        deadline: Duration,
    ) -> Self {
        Self {
            claude_program: claude_program.into(),
            codex_program: codex_program.into(),
            deadline,
        }
    }

    /// The program for one provider, and the variable that points it at a
    /// profile directory. The two clients name that directory differently —
    /// `CLAUDE_CONFIG_DIR`, `CODEX_HOME` — the same variables the daemon reads
    /// the grant back from, so what is signed in and what is read cannot drift.
    fn command_for(&self, provider: Provider) -> (std::process::Command, &'static str) {
        match provider {
            Provider::Anthropic => {
                let mut command = std::process::Command::new(&self.claude_program);
                // The cheapest tier there is.
                command.arg("-p").arg("ok").arg("--model").arg("haiku");
                (command, "CLAUDE_CONFIG_DIR")
            }
            Provider::Codex => {
                let mut command = std::process::Command::new(&self.codex_program);
                // No `--model`: an id names one plan's catalog and would go
                // stale, and the turn exists to authenticate, not to compute.
                // `--skip-git-repo-check` because the daemon's working
                // directory is not a repository.
                command.arg("exec").arg("--skip-git-repo-check").arg("ok");
                (command, "CODEX_HOME")
            }
        }
    }
}

impl Client for OwningClient {
    fn refresh(&self, provider: Provider, config_dir: Option<&Path>) -> Result<(), ProxyError> {
        let (mut command, directory_variable) = self.command_for(provider);
        let program = command.get_program().to_owned();
        command
            // Measured on Claude: without this the client waits several seconds
            // for input that is never coming. Codex reads a prompt from stdin
            // too, so the same closes it for both.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(config_dir) = config_dir {
            command.env(directory_variable, config_dir);
        }

        let mut child = command.spawn().map_err(|error| {
            ProxyError::authentication(format!(
                "could not run `{}`: {error}",
                program.to_string_lossy()
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
                        program.to_string_lossy()
                    )));
                }
            }
            if started.elapsed() >= self.deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProxyError::authentication(format!(
                    "`{}` did not finish within {} seconds, so the profile was left alone",
                    program.to_string_lossy(),
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
    provider: Provider,
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

    let outcome = client.refresh(provider, config_dir);
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
