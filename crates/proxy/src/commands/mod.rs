//! The command line's verbs.
//!
//! One module per family. What lives here is what more than one of them needs.

pub(crate) mod accounts;
pub(crate) mod daemon;
pub(crate) mod doctor;
pub(crate) mod inspect;
pub(crate) mod launch;
pub(crate) mod policy;
pub(crate) mod process;
pub(crate) mod record;
pub(crate) mod supervisor;

use anyhow::Result;
use proxenos::control;
use std::sync::Arc;

/// The name of the account serving turns, which is what an account section in
/// the configuration is keyed by.
///
/// The name rather than the account id: a key account has no id, and the name
/// is the string every account verb already takes.
pub(crate) fn serving_account(
    store: &Arc<dyn proxenos::auth::store::AccountStore>,
) -> Option<String> {
    store
        .accounts()
        .ok()?
        .into_iter()
        .find(|account| account.selected)
        .map(|account| account.name)
}

/// Where credentials live. Never the configuration file (§8).
///
/// The same directory the configuration is read from, resolved by the same
/// function: two copies of this rule drift, and the copy that drifts sends the
/// daemon looking for credentials somewhere the login never wrote them.
/// The accounts this daemon serves: the declared profiles, and the keys it
/// holds itself.
///
/// Built per command rather than shared, the same way the credential file used
/// to be opened per command. Nothing is cached between them, so a profile the
/// operator has just signed into is visible to the next one.
pub(crate) fn account_store() -> Result<proxenos::auth::accounts::Accounts> {
    let config = proxenos::config::Config::load()?;
    Ok(proxenos::auth::accounts::Accounts::from_config(
        &config,
        &proxenos::config::config_dir(),
    )?)
}

/// How long to watch for the daemon to go, and then for anything to bring it
/// back.
///
/// The second is the longer of the two on purpose: launchd throttles a respawn
/// to ten seconds after the last start, and a window under that would report
/// "nothing started it again" moments before something did — sending the reader
/// to `run` straight into the port the supervisor is about to take. It returns
/// as soon as it sees the answer, so the wait is only paid when there is
/// genuinely nothing there.
pub(crate) const STOP_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
pub(crate) const RESTART_WINDOW: std::time::Duration = std::time::Duration::from_secs(12);

/// What is answering the socket right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Answering {
    pub(crate) version: String,
    pub(crate) instance: Option<String>,
    /// The process serving the socket, where the daemon reports one. A daemon
    /// predating the field omits it, and nothing is invented for it.
    pub(crate) pid: Option<u64>,
    /// Whether the supervisor of §2.6 started it. `None` is the platform, or
    /// the process, that cannot answer — silence rather than a claim.
    pub(crate) supervised: Option<bool>,
}

/// Poll until `settled` accepts what it sees, or the window closes.
pub(crate) async fn watch(
    window: std::time::Duration,
    mut settled: impl FnMut(Option<Answering>) -> bool,
) -> bool {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if settled(answering().await) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
    }
}

pub(crate) async fn answering() -> Option<Answering> {
    let result = control::ask("status", None).await.ok()?;
    Some(Answering {
        version: version_of(&result).unwrap_or_else(|| "a build that does not say".to_owned()),
        instance: result
            .get("instance")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        pid: result.get("pid").and_then(serde_json::Value::as_u64),
        supervised: result
            .get("supervised")
            .and_then(serde_json::Value::as_bool),
    })
}

pub(crate) fn version_of(result: &serde_json::Value) -> Option<String> {
    result
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}
