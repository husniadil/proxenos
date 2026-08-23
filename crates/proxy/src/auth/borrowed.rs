//! `docs/proxy-behavior.md` §8 — grants this process reads and never owns.
//!
//! A borrowed grant lives in another program's profile directory. The ChatGPT
//! app and the `codex` CLI keep exactly one of them per `CODEX_HOME`, in
//! `auth.json`, so that directory *is* the identity: point at it and the
//! account it holds is the account turns are spent against. An operator who has
//! already signed in over there does not sign in again here.
//!
//! **Nothing in this module writes, and nothing built on it may refresh.** The
//! refresh token in that file is single-use: exchanging it rotates the stored
//! value and the previous one is refused afterwards, which is the failure
//! `tokens.rs` already names `refresh_token_reused`. Spending it here would
//! therefore log the operator out of the program that owns the file, and the
//! symptom would appear over there rather than here. The owning program
//! refreshes on its own next turn; this side reads whatever it finds, and an
//! expired grant is reported as expired rather than repaired.
//!
//! The decisions live here as pure functions over the file's text, in the shape
//! `setup_token` uses: what is wrong with a credential is decided without I/O,
//! and the caller supplies the path for the message.

use crate::auth::jwt;
use crate::auth::store::Credentials;
use serde::Deserialize;

/// The `auth_mode` a ChatGPT subscription is filed under.
///
/// The other modes that string can hold are credentials of a different kind
/// against a different endpoint, and none of them is borrowed: see
/// `BorrowedError::NotASubscription`.
pub const SUBSCRIPTION_AUTH_MODE: &str = "chatgpt";

/// Why an `auth.json` yielded no grant.
///
/// Every variant is a refusal, never a repair. A file this cannot read is a
/// file the owning program still owns, and guessing at it would put a
/// credential of the wrong kind on the wire.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BorrowedError {
    #[error("{0} is not valid JSON: {1}")]
    Malformed(String, String),
    /// The file exists but holds no grant, which is what a profile that has
    /// never completed a sign-in looks like.
    #[error(
        "{0} holds no tokens. Sign in to that profile first, in the ChatGPT app or with `codex login`"
    )]
    NotSignedIn(String),
    /// A profile authenticating with an API key rather than a subscription.
    /// Refused rather than borrowed: a key is spent against a different
    /// endpoint with different billing, and this proxy already has a place to
    /// put one that does not involve reading another program's file.
    #[error(
        "{0} authenticates with an API key (auth_mode `{1}`) rather than a ChatGPT subscription. \
         Store a key with `proxenos login --key --as NAME` instead"
    )]
    NotASubscription(String, String),
    /// A grant missing the half that authenticates, or the half that outlives
    /// it. Both are required, and a blank one is a broken file rather than an
    /// absent field.
    #[error("{0} holds an empty {1}. Sign in to that profile again")]
    EmptyToken(String, &'static str),
}

/// `auth.json` as far as a grant is concerned.
///
/// Deliberately partial: the file carries fields this has no business reading,
/// and `serde` ignores what is not named here. `last_refresh` is one of them —
/// it records when the owning program last rotated, and nothing on this side
/// acts on it.
#[derive(Deserialize)]
struct AuthFile {
    #[serde(default)]
    auth_mode: Option<String>,
    #[serde(default)]
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

/// The grant stored in a `CODEX_HOME`, as this process will spend it.
///
/// `source` names the file and appears in every refusal. It is a label rather
/// than a handle: the read happened before this was called, so a profile that
/// moved is described by the path it was expected at.
pub fn codex(raw: &str, source: &str) -> Result<Credentials, BorrowedError> {
    let parsed: AuthFile = serde_json::from_str(raw)
        .map_err(|error| BorrowedError::Malformed(source.to_owned(), error.to_string()))?;

    // Read before the tokens are: a profile in API-key mode can still carry a
    // stale `tokens` block from a previous sign-in, and borrowing that would
    // spend an identity the operator has since replaced.
    let mode = parsed.auth_mode.as_deref().unwrap_or_default();
    if !mode.is_empty() && mode != SUBSCRIPTION_AUTH_MODE {
        return Err(BorrowedError::NotASubscription(
            source.to_owned(),
            mode.to_owned(),
        ));
    }

    let tokens = parsed
        .tokens
        .ok_or_else(|| BorrowedError::NotSignedIn(source.to_owned()))?;

    for (value, name) in [
        (&tokens.access_token, "access_token"),
        (&tokens.refresh_token, "refresh_token"),
    ] {
        if value.trim().is_empty() {
            return Err(BorrowedError::EmptyToken(source.to_owned(), name));
        }
    }

    // The file records no expiry of its own, so it comes from the access
    // token's `exp` claim — the same place `tokens::refresh` reads it from, and
    // for the same reason. A token that yields none is treated as expired by
    // `needs_refresh`, which on this path means "ask the owning program to
    // refresh" rather than "refresh it here".
    let expires_at = jwt::expiry(&tokens.access_token);

    // `tokens.account_id` and the id token's `chatgpt_account_id` claim carry
    // the same value — checked against three signed-in profiles on one machine,
    // all three equal. The field wins because the owning program writes it
    // deliberately, and the claim covers a file written before it existed.
    let account_id = tokens
        .account_id
        .filter(|id| !id.trim().is_empty())
        .or_else(|| jwt::account_id(tokens.id_token.as_deref()));

    Ok(Credentials {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        account_id,
        expires_at,
    })
}
