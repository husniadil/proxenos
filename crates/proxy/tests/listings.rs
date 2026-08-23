//! `docs/proxy-behavior.md` §8.4 — how often one method asks the store.
//!
//! A listing is free against a file and is not against a borrowed profile: on
//! macOS every Claude profile in it is a `security` spawn, and a keychain the
//! operator has to unlock is a prompt per spawn. So a method that reports on
//! the accounts asks for them once and passes what it got down, and this is
//! what says so — nothing else about the answers would change if it went back
//! four more times.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use proxenos::auth::store::Account;
use proxenos::auth::store::AccountStore;
use proxenos::auth::store::Credential;
use proxenos::auth::store::CredentialStore;
use proxenos::auth::store::Credentials;
use proxenos::auth::store::FileStore;
use proxenos::auth::store::Provider;
use proxenos::control::handler::ControlState;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

/// A store that answers like any other and counts how often it was read.
///
/// Every read counts, not only a listing: loading the serving credential is a
/// keychain spawn on a borrowed profile too, and asking for it again to learn
/// something a listed row already carries costs exactly as much.
struct Counting {
    inner: FileStore,
    reads: AtomicUsize,
}

impl CredentialStore for Counting {
    fn load(&self) -> Result<Option<Credentials>, proxenos::error::ProxyError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.load()
    }

    fn save(&self, credentials: &Credentials) -> Result<(), proxenos::error::ProxyError> {
        self.inner.save(credentials)
    }

    fn clear(&self) -> Result<(), proxenos::error::ProxyError> {
        self.inner.clear()
    }
}

impl AccountStore for Counting {
    fn accounts(&self) -> Result<Vec<Account>, proxenos::error::ProxyError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.accounts()
    }

    fn add(
        &self,
        credentials: &Credentials,
        label: Option<&str>,
    ) -> Result<String, proxenos::error::ProxyError> {
        self.inner.add(credentials, label)
    }

    fn select(&self, name: &str) -> Result<(), proxenos::error::ProxyError> {
        self.inner.select(name)
    }

    fn remove(&self, name: &str) -> Result<(), proxenos::error::ProxyError> {
        self.inner.remove(name)
    }

    fn credential(&self) -> Result<Option<Credential>, proxenos::error::ProxyError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.credential()
    }

    fn credential_for(&self, name: &str) -> Result<Credential, proxenos::error::ProxyError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.credential_for(name)
    }

    fn add_key(
        &self,
        name: &str,
        key: &str,
        provider: Provider,
    ) -> Result<(), proxenos::error::ProxyError> {
        self.inner.add_key(name, key, provider)
    }

    fn save_for(
        &self,
        name: &str,
        credentials: &Credentials,
    ) -> Result<(), proxenos::error::ProxyError> {
        self.inner.save_for(name, credentials)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), proxenos::error::ProxyError> {
        self.inner.rename(from, to)
    }
}

/// A state whose store counts, over one stored account.
fn state(dir: &std::path::Path) -> (ControlState, Arc<Counting>) {
    let store = Arc::new(Counting {
        inner: FileStore::new(dir.join("credentials.json")),
        reads: AtomicUsize::new(0),
    });
    store
        .add(
            &Credentials {
                access_token: "a".to_owned(),
                refresh_token: "r".to_owned(),
                id_token: None,
                account_id: Some("acct_one".to_owned()),
                expires_at: None,
            },
            Some("one"),
        )
        .expect("stored");
    store.reads.store(0, Ordering::SeqCst);

    let state = ControlState {
        port: 8787,
        policy: Arc::new(proxenos::policy::Policy::new(
            proxenos::policy::Snapshot::new(
                // A real mapping, because an empty one short-circuits the
                // question the accounts are listed to answer.
                vec![proxenos::config::ResolvedTier {
                    defaulted: false,
                    account: None,
                    tier: "opus",
                    model: "gpt-5.6-terra".to_owned(),
                }],
                None,
                proxenos::config::CrossAccountTiers::Refused,
            ),
        )),
        catalog: Arc::new(proxenos::catalog::CatalogSource::fixed(
            proxenos::catalog::Catalog::parse(
                r#"{"data":[{"id":"gpt-5.6-terra","context_window":272000}]}"#,
                95.0,
            )
            .expect("a catalog"),
        )),
        credentials: Arc::clone(&store) as Arc<dyn AccountStore>,
        capture: Arc::new(proxenos::recorder::Switches::default()),
        usage: Arc::new(proxenos::usage::UsageStore::default()),
        config: Arc::new(proxenos::config::Config::default()),
        shutdown: Arc::new(proxenos::daemon::Shutdown::default()),
        tokens: None,
        usage_endpoint: String::new(),
        anthropic_usage_endpoint: String::new(),
        sessions: Arc::new(proxenos::session::SessionStore::new()),
        config_path: None,
    };
    (state, store)
}

async fn reads_for(method: &str) -> usize {
    let dir = tempfile::tempdir().expect("tempdir");
    let (state, store) = state(dir.path());

    proxenos::control::handler::dispatch(&state, method, None)
        .await
        .expect("the method answers");

    store.reads.load(Ordering::SeqCst)
}

/// `status` reports on the accounts from end to end — which one serves, what
/// its provider is, whether its tiers relay, which models are withheld — and
/// every one of those questions used to go back to the store for its own copy.
#[tokio::test]
async fn status_reads_the_store_once() {
    assert_eq!(reads_for("status").await, 1);
}

/// `env` has two halves and the second decides its client policy from the
/// accounts. One listing serves both.
#[tokio::test]
async fn env_reads_the_store_once() {
    assert_eq!(reads_for("env").await, 1);
}

/// `models` asks whether the serving account relays, whose list it is, and
/// whether the list is stale. Three questions, one read: the id it used to
/// load the credential for is on the row it already had.
#[tokio::test]
async fn models_reads_the_store_once() {
    assert_eq!(reads_for("models").await, 1);
}
