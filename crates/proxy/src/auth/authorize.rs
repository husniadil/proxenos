//! `docs/proxy-behavior.md` §8 — turning a stored credential into headers.
//!
//! Two kinds of credential reach two different endpoints, and the difference
//! between them is a header set. Resolving that in one place is what keeps a
//! subscription header off a key request: every path that authenticates asks
//! here rather than assembling its own.

use super::grants::Grants;
use super::store::AccountStore;
use super::store::Credential;
use super::store::Provider;
use crate::error::ProxyError;
use std::sync::Arc;

/// Which family of endpoint a credential belongs to.
///
/// Not a label on the credential so much as a statement about where it may be
/// spent. A key sent to a subscription endpoint is refused there, and the
/// refusal names an expired token, which is not what happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Subscription,
    Key,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Key => "key",
        }
    }
}

/// One credential, resolved into what goes on the wire.
#[derive(Clone, Debug)]
pub struct Authorization {
    pub kind: Kind,
    /// Whose endpoints this credential belongs to.
    ///
    /// Separate from `kind`, and both are needed: `kind` decides which of one
    /// provider's two endpoints a credential reaches, and this decides whose
    /// endpoints they are at all. A borrowed subscription grant on the second
    /// provider is a `Subscription` that must never be sent to the first
    /// provider's backend, and before this field there was no way to say so.
    pub provider: Provider,
    /// The account this resolved, where a pinned tier named one. `None` is the
    /// account serving turns.
    ///
    /// Carried so a refusal can name it. A mismatch message that says "the
    /// account serving turns" when a tier pinned a different one sends whoever
    /// reads it to check the selection, and the selection is not the half that
    /// is wrong.
    pub account: Option<String>,
    /// Every header this credential requires, including the one identifying
    /// the client where the endpoint expects it. `Debug` is derived, so this
    /// carries a bearer token: it is never logged, and the two types it is
    /// built from redact themselves for that reason.
    pub headers: Vec<(String, String)>,
}

impl Authorization {
    /// Put these headers on a request.
    pub fn apply(&self, mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        request
    }

    /// This credential, if it belongs where it is about to be spent.
    ///
    /// A refusal rather than a fallback: a key sent to a subscription endpoint
    /// comes back as a message about an invalid token, which sends whoever
    /// reads it looking for the wrong problem. Both halves are named, because
    /// either one could be the half that is wrong.
    /// The same authorization, remembering which account produced it.
    fn named(mut self, account: &str) -> Self {
        self.account = Some(account.to_owned());
        self
    }

    /// Refuse a credential that belongs to the other provider's endpoints.
    ///
    /// The relay asks this rather than asking about `kind`: what makes a
    /// credential belong there is whose account it is, and a grant and a key
    /// on that provider are both spent at the same endpoint.
    pub fn for_provider(self, expected: Provider) -> Result<Self, ProxyError> {
        if self.provider == expected {
            return Ok(self);
        }
        let whose = match &self.account {
            Some(account) => format!("the account `{account}` this tier is pinned to"),
            None => "the account serving turns".to_owned(),
        };
        Err(ProxyError::invalid_request(format!(
            "{whose} is on {}, and this endpoint belongs to {}; \
             select an account on the other provider, or point that endpoint somewhere it belongs",
            self.provider.as_str(),
            expected.as_str(),
        )))
    }

    pub fn for_endpoint(self, expected: Kind) -> Result<Self, ProxyError> {
        if self.kind == expected {
            return Ok(self);
        }
        let whose = match &self.account {
            Some(account) => format!("the account `{account}` this tier is pinned to"),
            None => "the account serving turns".to_owned(),
        };
        Err(ProxyError::invalid_request(format!(
            "{whose} holds a {} credential, and this endpoint takes a {} one; \
             select an account of the other kind, or point the {} endpoint somewhere it belongs",
            self.kind.as_str(),
            expected.as_str(),
            expected.as_str()
        )))
    }
}

#[async_trait::async_trait]
pub trait Authorizer: Send + Sync {
    /// The headers for one upstream request.
    ///
    /// Resolved per request rather than captured: a token taken once goes
    /// stale at the next refresh, and the account serving turns can change
    /// between one request and the next.
    ///
    /// `account` is the account this request belongs to, which a pinned tier
    /// names (`proxy-behavior.md` §7.1). `None` is the account serving turns,
    /// which is what every request meant before a tier could pin one. It is a
    /// parameter rather than a default because a pin the authorizer silently
    /// ignored would spend the serving account's quota and say nothing.
    async fn authorize(&self, account: Option<&str>) -> Result<Authorization, ProxyError>;
}

/// Which kind the account serving turns holds, for a caller that has to pick
/// an endpoint before it has a request to authorize.
///
/// A store that cannot be read answers `Subscription`, which is what this
/// project was before there was a second kind. The mistake surfaces at the
/// next request as a refusal naming both halves rather than as a turn sent
/// somewhere it does not belong.
pub fn selected_kind(store: &Arc<dyn AccountStore>) -> Kind {
    match store.credential() {
        Ok(Some(Credential::Key(_))) => Kind::Key,
        _ => Kind::Subscription,
    }
}

/// The credential of whichever account is serving turns.
///
/// The store is read on every request, so a switch or a login reaches the next
/// request without anything being rebuilt. A grant goes through the token
/// source, which is where refresh and refusal live; a key is spent as it is,
/// because there is nothing to refresh and nothing to expire.
pub struct AccountAuthorizer {
    store: Arc<dyn AccountStore>,
    grants: Arc<Grants>,
    /// One token source per pinned account, kept for the life of the
    /// authorizer.
    ///
    /// Built on demand and then reused, because refresh state is what makes it
    /// worth anything: a source discarded after each turn collapses no
    /// concurrent refreshes and forgets which token the backend refused, so
    /// every turn would retry a grant that is already gone.
    pinned: std::sync::Mutex<std::collections::HashMap<String, Arc<Grants>>>,
}

impl AccountAuthorizer {
    pub fn new(store: Arc<dyn AccountStore>, grants: Arc<Grants>) -> Self {
        Self {
            store,
            grants,
            pinned: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Which provider a named account is on, for deciding what its credential
    /// puts on the wire.
    fn provider_of(&self, account: &str) -> Provider {
        provider_named(
            self.store
                .accounts()
                .ok()
                .and_then(|accounts| accounts.into_iter().find(|stored| stored.name == account))
                .map(|stored| stored.provider),
        )
    }

    /// The token source for one named account, built once.
    fn grants_for(&self, account: &str) -> Arc<Grants> {
        let mut pinned = match self.pinned.lock() {
            Ok(pinned) => pinned,
            // Nothing panics while this is held. If it somehow did, a source
            // without the shared refresh state still authorizes correctly —
            // it only loses the collapsing — and taking the daemon down over
            // it would be the worse answer.
            Err(poisoned) => poisoned.into_inner(),
        };
        Arc::clone(pinned.entry(account.to_owned()).or_insert_with(|| {
            Arc::new(self.grants.rebind(Arc::new(super::store::AccountSlot::new(
                Arc::clone(&self.store),
                account,
            ))))
        }))
    }
}

#[async_trait::async_trait]
impl Authorizer for AccountAuthorizer {
    async fn authorize(&self, account: Option<&str>) -> Result<Authorization, ProxyError> {
        // A pinned account is read by name and refused by name. The store's
        // refusal names both the account and what is stored, which is what a
        // mapping and a store edited separately need in order to say which of
        // the two is wrong.
        if let Some(account) = account {
            let provider = self.provider_of(account);
            return match self.store.credential_for(account)? {
                Credential::Grant(_) => grant_authorization(&self.grants_for(account), provider)
                    .map(|authorization| authorization.named(account)),
                Credential::Key(key) => Ok(key_authorization(&key, provider).named(account)),
            };
        }

        // One listing, and the account it names is then read once by name.
        // Asking the store for the selected credential as well would resolve
        // the selection a second time and read every profile again.
        let serving = self
            .store
            .accounts()?
            .into_iter()
            .find(|account| account.selected)
            .ok_or_else(|| {
                ProxyError::authentication(
                    "no account is serving turns; declare a profile under `[profiles]` or store \
                     a key with `login --key --as NAME`",
                )
            })?;
        let provider = provider_named(Some(serving.provider));

        match self.store.credential_for(&serving.name)? {
            Credential::Grant(_) => grant_authorization(&self.grants, provider),
            Credential::Key(key) => Ok(key_authorization(&key, provider)),
        }
    }
}

/// What a key puts on the wire.
///
/// One header. A key identifies nothing but itself: no account to name, and no
/// client to identify to an endpoint that does not ask.
fn key_authorization(key: &super::store::ApiKey, provider: Provider) -> Authorization {
    Authorization {
        kind: Kind::Key,
        provider,
        account: None,
        headers: vec![(
            axum::http::header::AUTHORIZATION.to_string(),
            format!("Bearer {}", key.value()),
        )],
    }
}

/// What a subscription grant puts on the wire, which differs by provider.
///
/// The first provider wants an originator and the account id on every request;
/// the second wants the beta header its OAuth grants are gated behind and
/// nothing else. Sending either provider's extras to the other is how a
/// borrowed grant would fail with a message about the wrong half.
fn grant_authorization(grants: &Grants, provider: Provider) -> Result<Authorization, ProxyError> {
    // One read. The bearer and the account id come out of the same grant,
    // because on a profile whose credential lives in a keychain each read is a
    // process spawn.
    let credentials = grants.credentials()?;
    let mut headers = vec![(
        axum::http::header::AUTHORIZATION.to_string(),
        format!("Bearer {}", credentials.access_token),
    )];

    match provider {
        Provider::Codex => {
            // §2.8 — required on every subscription path, and its absence is a
            // bare 400 that names nothing.
            headers.push((
                "originator".to_owned(),
                crate::upstream::http::ORIGINATOR.to_owned(),
            ));
            if let Some(account) = credentials.account_id {
                headers.push(("chatgpt-account-id".to_owned(), account));
            }
        }
        Provider::Anthropic => {
            headers.push((
                ANTHROPIC_OAUTH_BETA.0.to_owned(),
                ANTHROPIC_OAUTH_BETA.1.to_owned(),
            ));
        }
    }

    Ok(Authorization {
        kind: Kind::Subscription,
        provider,
        account: None,
        headers,
    })
}

/// The beta the second provider gates its OAuth grants behind. Measured: a
/// request carrying the grant and this header answers 200 from any process.
pub const ANTHROPIC_OAUTH_BETA: (&str, &str) = ("anthropic-beta", "oauth-2025-04-20");

/// A provider name as the store reports it, back into the enum. An unknown
/// name is the first provider, which is what every stored account was before
/// there was a second.
fn provider_named(name: Option<&str>) -> Provider {
    match name {
        Some(name) if name == Provider::Anthropic.as_str() => Provider::Anthropic,
        _ => Provider::Codex,
    }
}

#[async_trait::async_trait]
impl Authorizer for Grants {
    /// A reader is already bound to one account's store, so the account a
    /// caller names has been resolved before it gets here.
    ///
    /// Answers for the first provider. Nothing here knows whose account it
    /// reads — a store yields a credential, not an account — so a caller that
    /// serves both providers goes through `AccountAuthorizer`, which does.
    async fn authorize(&self, _account: Option<&str>) -> Result<Authorization, ProxyError> {
        grant_authorization(self, Provider::Codex)
    }
}
