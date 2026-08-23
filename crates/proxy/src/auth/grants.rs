//! `docs/proxy-behavior.md` §8.4 — what a turn authenticates with.
//!
//! This replaces a refreshing token source, and the difference is the whole
//! point: a grant read out of another program's profile is never exchanged
//! here. There is no single-flight, no retirement of a refused token, and no
//! state to keep between turns, because every read goes back to the profile
//! and sees whatever the owning program has since written there.
//!
//! What that costs is one read per turn — a file, or one `security` spawn.
//! What it buys is that a client refreshing in the background is picked up on
//! the next turn without anything here noticing, which is exactly the
//! behaviour a borrowed grant needs.

use crate::auth::store::CredentialStore;
use crate::error::ProxyError;
use std::sync::Arc;

/// How close to expiry a grant stops being usable.
///
/// Smaller than a refresh margin would be, and for a different reason: nothing
/// here can act early, so this is only the width of a turn that must not be
/// started with a token about to lapse mid-request.
pub const EXPIRY_MARGIN_SECONDS: u64 = 60;

/// A clock, so expiry can be tested without waiting for it.
pub trait Clock: Send + Sync {
    fn now_unix(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
    }
}

/// The access token of whichever account a store answers for.
pub struct Grants {
    store: Arc<dyn CredentialStore>,
    clock: Arc<dyn Clock>,
}

impl Grants {
    pub fn new(store: Arc<dyn CredentialStore>, clock: Arc<dyn Clock>) -> Self {
        Self { store, clock }
    }

    /// The same reader against a different store: what a pinned tier needs,
    /// since its turns authenticate as an account the selection does not name.
    pub fn rebind(&self, store: Arc<dyn CredentialStore>) -> Self {
        Self {
            store,
            clock: Arc::clone(&self.clock),
        }
    }

    /// The account the current grant belongs to, where it carries one.
    pub fn account_id(&self) -> Option<String> {
        self.store
            .load()
            .ok()
            .flatten()
            .and_then(|credentials| credentials.account_id)
    }

    /// Whether the next turn would fail on the credential.
    ///
    /// The question a front-end asks is unchanged — will a dispatch work — but
    /// what makes the answer `true` is different now. There is no refresh token
    /// to be retired here, so this is "the grant cannot be spent as it stands":
    /// unreadable, or lapsed and waiting on the program that owns it.
    pub fn is_dead(&self) -> bool {
        self.access_token().is_err()
    }

    /// The bearer for the next turn.
    ///
    /// An expired grant is reported rather than repaired. The remedy is in the
    /// program that owns the profile, and the refusal says so: this side
    /// exchanging the refresh token would rotate the value that program still
    /// holds and log the operator out of it (§8.4).
    pub fn access_token(&self) -> Result<String, ProxyError> {
        let credentials = self.store.load()?.ok_or_else(|| {
            ProxyError::authentication(
                "no grant is available. `accounts` lists the declared profiles.".to_owned(),
            )
        })?;

        if credentials.needs_refresh(self.clock.now_unix(), EXPIRY_MARGIN_SECONDS) {
            return Err(ProxyError::authentication(
                "the borrowed grant has expired. The program that owns the profile refreshes \
                 it: run it once, then try again."
                    .to_owned(),
            ));
        }

        Ok(credentials.access_token)
    }
}
