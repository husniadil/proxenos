//! `docs/proxy-behavior.md` §3.2 — the session store: what matches, what is
//! kept, and what is never created.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use proxenos::session::Scope;
use proxenos::session::SessionStore;
use proxenos_core::responses::InputItem;
use serde_json::json;
use std::sync::Arc;

fn conversation(texts: &[&str]) -> Vec<InputItem> {
    let values: Vec<_> = texts
        .iter()
        .map(|text| {
            json!({
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": text }],
            })
        })
        .collect();
    serde_json::from_value(json!(values)).unwrap()
}

fn scope(model: &str, account: Option<&str>) -> Scope {
    Scope {
        requested: "claude-sonnet-5".to_owned(),
        model: model.to_owned(),
        account: account.map(str::to_owned),
    }
}

fn main_scope() -> Scope {
    scope("gpt-6-sol", None)
}

/// A turn completing advances its session, as the ingress does.
fn turn(store: &SessionStore, texts: &[&str], scope: &Scope) -> Arc<proxenos::session::Session> {
    let input = conversation(texts);
    let session = store.resolve(&input, scope);
    session.advance(&input, &[]);
    session
}

/// At most 64 conversations are kept, and the one dropped for the 65th is the
/// one used longest ago. A full store degrades to full sends, never errors.
#[test]
fn the_store_keeps_sixty_four_and_evicts_the_least_recently_used() {
    let store = SessionStore::new();
    let first = turn(&store, &["conversation 0"], &main_scope());
    for index in 1..64 {
        turn(&store, &[&format!("conversation {index}")], &main_scope());
    }
    assert_eq!(store.len(), 64);

    // Conversation 0 is used again, so conversation 1 is now the oldest.
    let again = store.resolve(&conversation(&["conversation 0", "more"]), &main_scope());
    assert!(Arc::ptr_eq(&first, &again));

    turn(&store, &["conversation 64"], &main_scope());
    assert_eq!(store.len(), 64);

    assert!(
        store
            .lookup(
                &conversation(&["conversation 0", "more"]),
                "claude-sonnet-5"
            )
            .is_some(),
        "the conversation used most recently was evicted"
    );
    assert!(
        store
            .lookup(
                &conversation(&["conversation 1", "more"]),
                "claude-sonnet-5"
            )
            .is_none(),
        "the conversation used longest ago survived"
    );
}

/// An input that extends two baselines continues the longer one: the shorter
/// is a prefix of it, and continuing that would drop everything between.
#[test]
fn the_longest_matching_baseline_wins() {
    let store = SessionStore::new();
    // The longer first: a shorter input extends nothing longer, so the two are
    // separate, and the shorter is the one used most recently.
    let long = turn(&store, &["one", "two", "three"], &main_scope());
    let short = turn(&store, &["one"], &main_scope());
    assert!(!Arc::ptr_eq(&short, &long));

    let continued = store.resolve(
        &conversation(&["one", "two", "three", "four"]),
        &main_scope(),
    );

    assert!(Arc::ptr_eq(&continued, &long));
}

/// Sizing looks a conversation up and creates nothing. An entry made there
/// would never advance and would match every first turn that followed.
#[test]
fn sizing_never_creates_a_session() {
    let store = SessionStore::new();

    assert!(
        store
            .lookup(&conversation(&["unknown"]), "claude-sonnet-5")
            .is_none()
    );
    assert_eq!(store.len(), 0);

    let known = turn(&store, &["known"], &main_scope());
    let found = store
        .lookup(&conversation(&["known", "next"]), "claude-sonnet-5")
        .expect("a known conversation is found");
    assert!(Arc::ptr_eq(&known, &found));
    assert_eq!(store.len(), 1);
}

/// A request for another model, or as another account, replaying the same
/// conversation is a conversation of its own. Joined to the first, it advanced
/// that session's baseline with its own items, so the main conversation's next
/// turn matched nothing, and the main conversation's retained reasoning, made
/// by one model for one account, was put into a request to another.
#[test]
fn another_model_or_account_is_another_conversation() {
    let store = SessionStore::new();
    let main = turn(&store, &["one", "two"], &main_scope());

    let side = store.resolve(
        &conversation(&["one", "two", "summarize"]),
        &scope("gpt-6-luna", None),
    );
    assert!(
        !Arc::ptr_eq(&main, &side),
        "another model joined the conversation"
    );

    let pinned = store.resolve(
        &conversation(&["one", "two", "summarize"]),
        &scope("gpt-6-sol", Some("spare")),
    );
    assert!(
        !Arc::ptr_eq(&main, &pinned),
        "another account joined the conversation"
    );

    let continued = store.resolve(&conversation(&["one", "two", "three"]), &main_scope());
    assert!(Arc::ptr_eq(&main, &continued));
}
