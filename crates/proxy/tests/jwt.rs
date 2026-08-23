//! `docs/proxy-behavior.md` §8 — the claims a grant carries.
//!
//! Nothing here verifies a signature and nothing should: these tokens arrived
//! over TLS from the server that issued them, and what is being read is when
//! they lapse and which account they belong to.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use base64::Engine;
use pretty_assertions::assert_eq;
use proxenos::auth::jwt;

/// Build an unsigned JWT with the given payload. Signature verification is not
/// performed and is not wanted — see the note on the jwt module.
fn token_with(payload: serde_json::Value) -> String {
    let encode = |value: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value);
    format!(
        "{}.{}.{}",
        encode(br#"{"alg":"none"}"#),
        encode(payload.to_string().as_bytes()),
        encode(b"signature")
    )
}

/// token's own claim. Without this every request looks due for refresh.
#[test]
fn expiry_comes_from_the_access_token_claim() {
    let token = token_with(serde_json::json!({ "exp": 1_800_000_000u64 }));
    assert_eq!(jwt::expiry(&token), Some(1_800_000_000));
}

/// A token with no `exp`, or one that is not a JWT at all, yields nothing —
/// which `needs_refresh` treats as expired. Refreshing needlessly costs one
/// request; assuming a token is live when it is not fails the turn.
#[test]
fn an_unreadable_token_yields_no_expiry() {
    assert_eq!(jwt::expiry("not-a-jwt"), None);
    assert_eq!(jwt::expiry(&token_with(serde_json::json!({}))), None);
    assert_eq!(jwt::expiry(""), None);
}

/// The account id is a claim nested under the auth namespace of the id token.
#[test]
fn the_account_id_is_read_from_the_id_token() {
    let token = token_with(serde_json::json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "acct_from_claim",
            "chatgpt_plan_type": "pro",
        },
    }));

    assert_eq!(
        jwt::account_id(Some(&token)).as_deref(),
        Some("acct_from_claim")
    );
}

/// The plan sits beside the account id, under the same namespace.
///
/// It is worth reading because it is the only local explanation for a whole
/// class of refusal: efforts and models are gated on the subscription, and a
/// free account asking for one gets an error that names the value, never the
/// plan. Reporting it turns "the backend said no" into a checkable fact.
#[test]
fn the_plan_is_read_from_the_id_token() {
    let token = token_with(serde_json::json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "acct_from_claim",
            "chatgpt_plan_type": "plus",
        },
    }));

    assert_eq!(jwt::plan(Some(&token)).as_deref(), Some("plus"));
}

/// A missing plan is absent, not guessed at. Defaulting to "free" would be a
/// fabricated figure, and defaulting to "plus" would explain away a refusal
/// that deserves explaining.
#[test]
fn an_id_token_without_a_plan_claims_nothing() {
    assert_eq!(jwt::plan(None), None);
    assert_eq!(jwt::plan(Some("not-a-jwt")), None);
    assert_eq!(
        jwt::plan(Some(&token_with(serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "a" },
        })))),
        None
    );
}

#[test]
fn an_id_token_without_the_claim_yields_no_account() {
    assert_eq!(jwt::account_id(None), None);
    assert_eq!(jwt::account_id(Some("not-a-jwt")), None);
    assert_eq!(
        jwt::account_id(Some(&token_with(serde_json::json!({ "sub": "u" })))),
        None
    );
}
