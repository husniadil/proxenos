//! `docs/proxy-behavior.md` §8 — reading a grant out of another program's
//! profile directory.
//!
//! The shapes here are the ones a real `auth.json` has: checked against three
//! signed-in `CODEX_HOME` directories on one machine, which is where the
//! field names and the account-id equality below come from.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use base64::Engine;
use pretty_assertions::assert_eq;
use proxenos::auth::borrowed;
use proxenos::auth::borrowed::BorrowedError;

const SOURCE: &str = "/profiles/work/auth.json";

fn token_with(payload: serde_json::Value) -> String {
    let encode = |value: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value);
    format!(
        "{}.{}.{}",
        encode(br#"{"alg":"none"}"#),
        encode(payload.to_string().as_bytes()),
        encode(b"signature")
    )
}

/// An access token carrying an expiry, as the owning program writes one.
fn access_token(exp: u64) -> String {
    token_with(serde_json::json!({ "exp": exp }))
}

/// An id token carrying the claims the account is described by.
fn id_token(account_id: &str, plan: &str) -> String {
    token_with(serde_json::json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "chatgpt_plan_type": plan,
        },
    }))
}

fn auth_json(tokens: serde_json::Value) -> String {
    serde_json::json!({
        "OPENAI_API_KEY": null,
        "tokens": tokens,
        "last_refresh": "2026-08-23T08:00:44.123456Z",
        "auth_mode": "chatgpt",
    })
    .to_string()
}

/// What a refusal was, for a case that is expected to be one.
///
/// `Credentials` deliberately implements no `PartialEq` — comparing two
/// secrets is not an operation this codebase wants to make easy — so a refusal
/// is asserted on rather than the whole `Result`.
fn refusal(raw: &str) -> BorrowedError {
    borrowed::codex(raw, SOURCE).expect_err("this case is a refusal")
}

/// The whole file, as a signed-in profile holds it.
#[test]
fn a_signed_in_profile_yields_its_grant() {
    let raw = auth_json(serde_json::json!({
        "id_token": id_token("acct_123", "team"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("a signed-in profile parses");

    assert_eq!(grant.refresh_token, "rt.1.borrowed");
    assert_eq!(grant.account_id.as_deref(), Some("acct_123"));
    assert_eq!(grant.expires_at, Some(1_800_000_000));
}

/// The file records no expiry of its own, so it comes from the access token's
/// own claim. Without this every borrowed grant would look due for refresh —
/// and on this path there is nothing that may refresh it.
#[test]
fn the_expiry_comes_from_the_access_token_claim() {
    let raw = auth_json(serde_json::json!({
        "access_token": access_token(1_777_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.expires_at, Some(1_777_000_000));
}

/// An access token with no readable claim yields no expiry, which
/// `needs_refresh` treats as expired. That is the safe direction here: it asks
/// the owning program for a fresh one instead of spending a token that may be
/// dead.
#[test]
fn an_unreadable_access_token_yields_no_expiry() {
    let raw = auth_json(serde_json::json!({
        "access_token": "not-a-jwt",
        "refresh_token": "rt.1.borrowed",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.expires_at, None);
    assert!(grant.needs_refresh(1_800_000_000, 0));
}

/// `tokens.account_id` and the id token's claim hold the same value on every
/// profile checked, so either answers. The field is preferred because the
/// owning program writes it deliberately.
#[test]
fn the_account_id_prefers_the_field_over_the_claim() {
    let raw = auth_json(serde_json::json!({
        "id_token": id_token("acct_from_claim", "pro"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_from_field",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.account_id.as_deref(), Some("acct_from_field"));
}

/// A file written before the field existed still names its account, because
/// the claim carries it too.
#[test]
fn the_account_id_falls_back_to_the_claim() {
    let raw = auth_json(serde_json::json!({
        "id_token": id_token("acct_from_claim", "pro"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.account_id.as_deref(), Some("acct_from_claim"));
}

/// A blank field is not an answer, and falls through to the claim rather than
/// being reported as an account named "".
#[test]
fn a_blank_account_id_field_falls_back_to_the_claim() {
    let raw = auth_json(serde_json::json!({
        "id_token": id_token("acct_from_claim", "pro"),
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "   ",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.account_id.as_deref(), Some("acct_from_claim"));
}

/// No id token at all is not a failure: the grant still authenticates, and
/// only the description of it is poorer.
#[test]
fn a_grant_without_an_id_token_still_parses() {
    let raw = auth_json(serde_json::json!({
        "access_token": access_token(1_800_000_000),
        "refresh_token": "rt.1.borrowed",
        "account_id": "acct_123",
    }));

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.id_token, None);
    assert_eq!(grant.account_id.as_deref(), Some("acct_123"));
}

/// A profile nobody has signed into holds the file but no grant.
#[test]
fn a_profile_that_was_never_signed_into_is_refused() {
    let raw = serde_json::json!({ "OPENAI_API_KEY": null, "auth_mode": "chatgpt" }).to_string();

    assert_eq!(refusal(&raw), BorrowedError::NotSignedIn(SOURCE.to_owned()));
}

/// An API-key profile is refused rather than borrowed: a key is spent against
/// a different endpoint with different billing, and this proxy already stores
/// one without reading anyone else's file.
///
/// The check runs before the tokens are read, because such a profile can still
/// carry a stale `tokens` block from a sign-in the operator has replaced.
#[test]
fn an_api_key_profile_is_refused_even_with_tokens_present() {
    let raw = serde_json::json!({
        "OPENAI_API_KEY": "sk-not-borrowed",
        "auth_mode": "apikey",
        "tokens": {
            "access_token": access_token(1_800_000_000),
            "refresh_token": "rt.1.stale",
            "account_id": "acct_stale",
        },
    })
    .to_string();

    assert_eq!(
        refusal(&raw),
        BorrowedError::NotASubscription(SOURCE.to_owned(), "apikey".to_owned())
    );
}

/// A file written before `auth_mode` existed is read as the subscription it
/// is, rather than refused for a field that was never there.
#[test]
fn a_file_without_an_auth_mode_is_read_as_a_subscription() {
    let raw = serde_json::json!({
        "tokens": {
            "access_token": access_token(1_800_000_000),
            "refresh_token": "rt.1.borrowed",
            "account_id": "acct_123",
        },
    })
    .to_string();

    let grant = borrowed::codex(&raw, SOURCE).expect("parses");

    assert_eq!(grant.account_id.as_deref(), Some("acct_123"));
}

/// An empty half is a broken file, and it names which half rather than
/// failing later against the backend.
#[test]
fn an_empty_token_is_refused_by_name() {
    let missing_access = auth_json(serde_json::json!({
        "access_token": "",
        "refresh_token": "rt.1.borrowed",
    }));
    assert_eq!(
        refusal(&missing_access),
        BorrowedError::EmptyToken(SOURCE.to_owned(), "access_token")
    );

    let missing_refresh = auth_json(serde_json::json!({
        "access_token": access_token(1_800_000_000),
        "refresh_token": "  ",
    }));
    assert_eq!(
        refusal(&missing_refresh),
        BorrowedError::EmptyToken(SOURCE.to_owned(), "refresh_token")
    );
}

/// The path is in every refusal, because the operator's next move is to sign
/// in to one particular profile and the message is what names it.
#[test]
fn every_refusal_names_the_file() {
    let cases = [
        serde_json::json!({ "auth_mode": "chatgpt" }).to_string(),
        serde_json::json!({ "auth_mode": "apikey" }).to_string(),
        auth_json(serde_json::json!({ "access_token": "", "refresh_token": "" })),
        "{ not json".to_owned(),
    ];

    for raw in cases {
        let message = borrowed::codex(&raw, SOURCE)
            .expect_err("every case here is a refusal")
            .to_string();
        assert!(message.contains(SOURCE), "message was: {message}");
    }
}

/// Reading a grant never renders one. The tokens are secrets and the file is
/// somebody else's, so a message about it carries neither.
#[test]
fn a_refusal_never_carries_the_tokens() {
    let raw = auth_json(serde_json::json!({
        "access_token": "",
        "refresh_token": "rt.1.SECRET-VALUE",
    }));

    let message = borrowed::codex(&raw, SOURCE)
        .expect_err("an empty access token is refused")
        .to_string();

    assert!(!message.contains("SECRET-VALUE"), "message was: {message}");
}
