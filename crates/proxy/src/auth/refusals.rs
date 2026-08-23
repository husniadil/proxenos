//! `docs/proxy-behavior.md` §8.4 — the backend's own answer about a
//! credential, remembered per account.
//!
//! It exists because of a gap on the second provider. A Claude profile records
//! the date its login stops working and this daemon counts down to it; a Codex
//! profile records no such date, and `codex login status` does not supply one
//! either — it reads `auth.json` and reports "logged in" for a profile whose
//! tokens are junk, which was measured rather than assumed. So the only thing
//! that can say a Codex profile needs signing in again is the backend
//! refusing it, and that answer arrives on a turn nobody is watching.
//!
//! What is kept is the refusal, never the credential: a status, a sentence the
//! backend wrote, and when it arrived.
//!
//! **A refusal is cleared by the next turn that works.** Signing in again is
//! what fixes one, and the profile is read fresh on every turn — so a stale
//! warning would outlive the problem and send an operator to renew something
//! that already works.

use std::collections::HashMap;
use std::sync::Mutex;

/// What the backend said, and when.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Refusal {
    /// The status it arrived as: 401 and 403 mean different things to whoever
    /// has to act on it, and reporting one as the other sends them to the
    /// wrong remedy.
    pub status: u16,
    /// The backend's own sentence. Never this proxy's paraphrase of it: the
    /// operator is about to search for it.
    pub detail: String,
    /// Epoch seconds, so "and it is still refusing" and "it refused once last
    /// week" are distinguishable.
    pub at: u64,
}

/// Every account the backend has refused since this daemon started.
///
/// Not persisted. A refusal is a fact about a credential as the backend saw it
/// on one turn, and the first turn after a restart re-establishes it — where a
/// warning restored from disk would describe a credential that may have been
/// renewed in between.
#[derive(Default)]
pub struct Refusals {
    by_account: Mutex<HashMap<String, Refusal>>,
    /// How the account serving turns is named, for a turn that was made as
    /// "whoever is serving". Resolved at the moment it is needed, and only
    /// then: on a borrowed profile every resolution is a store read.
    serving: Option<crate::usage::ServingAccount>,
}

impl Refusals {
    /// Bound to the store, so an unpinned turn's refusal is filed under the
    /// account that was actually serving when it happened.
    pub fn for_accounts(store: std::sync::Arc<dyn crate::auth::store::AccountStore>) -> Self {
        Self::default().serving(std::sync::Arc::new(move || {
            store
                .accounts()
                .ok()?
                .into_iter()
                .find(|account| account.selected)
                .map(|account| account.name)
        }))
    }

    /// Bind it to whoever is serving turns. The same seam the token tally
    /// uses, and it exists for the same reason: without one, a turn made as
    /// the serving account has no name to be filed under.
    #[must_use]
    pub fn serving(mut self, serving: crate::usage::ServingAccount) -> Self {
        self.serving = Some(serving);
        self
    }

    /// File a refusal against the account that was spent.
    ///
    /// `None` is the serving account, resolved here rather than left for
    /// whoever reads it later: which account was serving is a fact about the
    /// moment of the turn, and a switch afterwards would move the blame.
    pub fn record(&self, account: Option<&str>, status: u16, detail: impl Into<String>) {
        let Some(name) = self.name_for(account) else {
            return;
        };
        if let Ok(mut by_account) = self.by_account.lock() {
            by_account.insert(
                name,
                Refusal {
                    status,
                    detail: detail.into(),
                    at: now(),
                },
            );
        }
    }

    /// A turn worked, so whatever was said about that credential is over.
    ///
    /// Returns early where nothing is held, which is the ordinary case and the
    /// reason this can be called on every successful turn: resolving the
    /// serving account is a store read, and a daemon that made one per turn
    /// would spawn `security` per turn on a borrowed Claude profile.
    pub fn clear(&self, account: Option<&str>) {
        if self
            .by_account
            .lock()
            .is_ok_and(|by_account| by_account.is_empty())
        {
            return;
        }
        let Some(name) = self.name_for(account) else {
            return;
        };
        if let Ok(mut by_account) = self.by_account.lock() {
            by_account.remove(&name);
        }
    }

    /// What the backend last said about one account, if anything.
    pub fn get(&self, account: &str) -> Option<Refusal> {
        self.by_account.lock().ok()?.get(account).cloned()
    }

    /// Drop what is held about an account that no longer exists.
    pub fn forget(&self, account: &str) {
        if let Ok(mut by_account) = self.by_account.lock() {
            by_account.remove(account);
        }
    }

    fn name_for(&self, account: Option<&str>) -> Option<String> {
        account
            .map(str::to_owned)
            .or_else(|| self.serving.as_ref().and_then(|serving| serving()))
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
