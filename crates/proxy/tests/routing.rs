//! `docs/proxy-behavior.md` §7.1 — which account a turn is served as.
//!
//! A tier may pin another account, and the whole point of the feature is which
//! subscription's quota the turn spends. That is invisible in the response: a
//! turn served as the wrong account succeeds and reads exactly like one served
//! as the right one. The only place it shows is the `authorization` header the
//! upstream request carried, so that is what these assert on — a stub upstream
//! capturing one header per request, and two accounts holding tokens that
//! cannot be confused for each other.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use proxenos::auth::store::AccountStore;
use proxenos::auth::store::CredentialStore;
use proxenos::auth::store::Credentials;
use proxenos::auth::store::FileStore;
use proxenos::auth::store::Provider;
use proxenos::config::ResolvedTier;
use proxenos::ingress::AppState;
use proxenos::ingress::router;
use proxenos::upstream::http::HttpTransport;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;

/// Every `authorization` header the upstream was sent, in order.
type Seen = Arc<Mutex<Vec<String>>>;

/// Every model id the upstream was asked for, in order. Which mapping a turn
/// was translated on is invisible anywhere else: a turn translated on the
/// wrong account's tiers succeeds and reads exactly like one translated on the
/// right ones.
type Asked = Arc<Mutex<Vec<String>>>;

fn grant(access: &str, refresh: &str, account_id: &str) -> Credentials {
    Credentials {
        access_token: access.to_owned(),
        refresh_token: refresh.to_owned(),
        id_token: None,
        account_id: Some(account_id.to_owned()),
        // Well clear of the refresh margin, so nothing here refreshes and the
        // token on the wire is the one that was stored.
        expires_at: Some(u64::MAX / 2),
    }
}

fn tier(name: &'static str, model: &str, account: Option<&str>) -> ResolvedTier {
    ResolvedTier {
        defaulted: false,
        missing: None,
        account: account.map(str::to_owned),
        tier: name,
        model: model.to_owned(),
    }
}

/// An upstream that answers every turn identically and records who asked, and
/// for which model.
async fn upstream() -> (String, Seen, Asked) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let models = Arc::clone(&asked);

    let app = axum::Router::new().route(
        "/responses",
        axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
            if let Ok(mut seen) = sink.lock() {
                seen.push(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("<absent>")
                        .to_owned(),
                );
            }
            if let Ok(mut models) = models.lock() {
                models.push(
                    serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|request| {
                            request["model"]
                                .as_str()
                                .map(std::borrow::ToOwned::to_owned)
                        })
                        .unwrap_or_else(|| "<absent>".to_owned()),
                );
            }
            // A quota snapshot at the head of the stream, as the backend
            // sends one — with a percentage that names the account whose
            // token asked, so a figure filed under the wrong account is
            // visible rather than plausible.
            let used_percent = match headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
            {
                Some("Bearer spare-token") => 77,
                _ => 11,
            };
            let body = [
                json!({
                    "type": "codex.rate_limits",
                    "plan_type": "plus",
                    "rate_limits": {
                        "limit_reached": false,
                        "primary": {
                            "used_percent": used_percent,
                            "window_minutes": 300,
                            "reset_at": 1_789_487_264u64,
                        },
                        "secondary": null,
                    },
                }),
                json!({ "type": "response.created", "response": { "id": "resp_1" } }),
                json!({ "type": "response.output_text.delta", "delta": "hi" }),
                // Upstream's own counts on the completed response, which are
                // what a spend tally may state and the only thing it may
                // state (§6.1). Distinct per account for the same reason the
                // percentage is: a tally filed under the wrong account has to
                // be visible rather than plausible.
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_1",
                        "usage": {
                            "input_tokens": if used_percent == 77 { 900 } else { 100 },
                            "output_tokens": if used_percent == 77 { 90 } else { 20 },
                        },
                    },
                }),
            ]
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    body,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/responses"), seen, asked)
}

/// The daemon's own wiring, minus the parts a turn does not touch.
///
/// The conduit factory is built the way `main.rs` builds it, WebSocket
/// omitted: the socket is covered by its own tests and this is about which
/// credential goes on the request.
async fn daemon(
    store: Arc<FileStore>,
    tiers: Vec<ResolvedTier>,
) -> (String, Seen, Arc<proxenos::usage::UsageStore>) {
    let (base, seen, usage, _asked) =
        daemon_with(store, tiers, proxenos::config::Config::default()).await;
    (base, seen, usage)
}

/// The same, over a stated configuration.
///
/// The mapping handed in is the one in force — the selection's, resolved when
/// it was selected (§7.1). The configuration is what any *other* account's
/// mapping is resolved from, which is the whole subject of a tagged turn
/// (`api.md` §2.3).
async fn daemon_with(
    store: Arc<FileStore>,
    tiers: Vec<ResolvedTier>,
    config: proxenos::config::Config,
) -> (String, Seen, Arc<proxenos::usage::UsageStore>, Asked) {
    let (endpoint, seen, asked) = upstream().await;

    let authorizer: Arc<dyn proxenos::auth::authorize::Authorizer> =
        Arc::new(proxenos::auth::authorize::AccountAuthorizer::new(
            Arc::clone(&store) as Arc<dyn AccountStore>,
            Arc::new(proxenos::auth::grants::Grants::new(
                Arc::clone(&store) as Arc<dyn CredentialStore>,
                Arc::new(proxenos::auth::grants::SystemClock),
            )),
        ));

    let http_endpoint = endpoint.clone();
    let conduit_authorizer = Arc::clone(&authorizer);
    let conduits: proxenos::ingress::ConduitFactory = Arc::new(move |session_id| {
        Arc::new(proxenos::upstream::conduit::Conduit::new(
            Arc::new(
                HttpTransport::new(&http_endpoint)
                    .with_credentials(Arc::clone(&conduit_authorizer)),
            ),
            None,
            session_id,
        ))
    });

    // Bound to the same store the turns authenticate against, which is how
    // the daemon builds it: an unpinned turn's account is whoever it has
    // selected at the moment that turn is served.
    let usage = Arc::new(proxenos::usage::UsageStore::for_accounts(
        Arc::clone(&store) as Arc<dyn AccountStore>,
    ));

    let state = AppState {
        policy: Arc::new(
            proxenos::policy::Policy::new(proxenos::policy::Snapshot::new(
                tiers,
                None,
                proxenos::config::CrossAccountTiers::Permitted,
            ))
            .resolving_accounts_from(
                Arc::new(config),
                Arc::clone(&store) as Arc<dyn AccountStore>,
            ),
        ),
        catalog: Arc::new(proxenos::catalog::CatalogSource::fixed(
            proxenos::catalog::Catalog::fallback(),
        )),
        transport: Arc::new(HttpTransport::new(&endpoint)),
        conduits: Some(conduits),
        recorder: None,
        capture: Arc::new(proxenos::recorder::Switches::new(None)),
        usage: Arc::clone(&usage),
        refusals: Arc::new(proxenos::auth::refusals::Refusals::default()),
        instructions: Arc::new(proxenos::config::InstructionsConfig {
            identity: false,
            append: None,
            working_budget: false,
        }),
        sessions: Arc::new(proxenos::session::SessionStore::new()),
        relay: None,
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    (format!("http://{addr}"), seen, usage, asked)
}

/// One turn on one tier, with a body distinct enough to be its own
/// conversation.
async fn turn(base: &str, model: &str, text: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .json(&json!({
            "model": model,
            "max_tokens": 64,
            "stream": true,
            "messages": [{ "role": "user", "content": text }],
        }))
        .send()
        .await
        .expect("the ingress should answer")
}

/// The same turn, launched with an account tag riding the bearer the client
/// was told to send (`exec --account`, api.md §2.3).
async fn tagged_turn(base: &str, model: &str, text: &str, account: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .header(
            "authorization",
            format!("Bearer {}{account}", proxenos::ingress::ACCOUNT_TAG),
        )
        .json(&json!({
            "model": model,
            "max_tokens": 64,
            "stream": true,
            "messages": [{ "role": "user", "content": text }],
        }))
        .send()
        .await
        .expect("the ingress should answer")
}

fn store_with_two_accounts(dir: &tempfile::TempDir) -> Arc<FileStore> {
    let store = Arc::new(FileStore::new(dir.path().join("credentials.json")));
    store
        .add(
            &grant("serving-token", "serving-refresh", "acct_serving"),
            None,
        )
        .unwrap();
    store
        .add(
            &grant("spare-token", "spare-refresh", "acct_spare"),
            Some("spare"),
        )
        .unwrap();
    store.select("acct_serving").unwrap();
    store
}

/// **The acceptance criterion.** A pinned tier's turn authenticates as the
/// account it names; every other tier is unchanged.
///
/// Both turns succeed either way — that is what makes this the assertion that
/// matters. Served as the serving account, the pinned turn would return the
/// same stream and spend the wrong subscription's quota with nothing said.
#[tokio::test]
async fn a_pinned_tier_spends_the_account_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);

    let (base, seen, _usage) = daemon(
        Arc::clone(&store),
        vec![
            tier("sonnet", "gpt-5.6-terra", None),
            tier("haiku", "gpt-5.6-luna", Some("spare")),
        ],
    )
    .await;

    assert_eq!(turn(&base, "sonnet", "the main turn").await.status(), 200);
    assert_eq!(turn(&base, "haiku", "the cheap turn").await.status(), 200);

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            "Bearer serving-token".to_owned(),
            "Bearer spare-token".to_owned(),
        ]
    );
}

/// A pin naming an account the store does not hold refuses the turn, in the
/// error shape the client understands, naming the account.
///
/// Never a fallback to the serving account: the turn would succeed against a
/// subscription nobody pointed at it, and the mapping would go on being wrong
/// invisibly.
#[tokio::test]
async fn a_pin_the_store_cannot_answer_for_refuses_the_turn_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);

    let (base, seen, _usage) = daemon(
        Arc::clone(&store),
        vec![tier("haiku", "gpt-5.6-luna", Some("retired"))],
    )
    .await;

    let response = turn(&base, "haiku", "the cheap turn").await;
    assert_eq!(response.status(), 400);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("retired"), "{message}");

    assert!(
        seen.lock().unwrap().is_empty(),
        "nothing reached the backend as somebody else"
    );
}

/// A pinned account holding a credential of the wrong kind is refused the way
/// a mismatched selection already is, and the refusal names the account.
///
/// The mismatch message names the account serving turns, which is the wrong
/// half when a tier pinned a different one: it sends whoever reads it to check
/// the selection, and the selection is fine.
#[tokio::test]
async fn a_pinned_credential_of_the_wrong_kind_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);
    store
        .add_key("billing", "key-secret-value", Provider::Codex)
        .unwrap();
    store.select("acct_serving").unwrap();

    let (base, seen, _usage) = daemon(
        Arc::clone(&store),
        vec![tier("haiku", "gpt-5.6-luna", Some("billing"))],
    )
    .await;

    let response = turn(&base, "haiku", "the cheap turn").await;
    assert_eq!(response.status(), 400);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("billing"), "{message}");
    assert!(message.contains("key"), "{message}");

    assert!(seen.lock().unwrap().is_empty(), "nothing was sent");
}

/// **Build 1.** A turn on a pinned tier records its quota under the pin, and
/// the serving account keeps its own figure: both survive side by side.
///
/// The figures are what separates this from the header assertions above. A
/// daemon holding one latest snapshot answers both turns and reports the cheap
/// tier's account as though it were the one the operator is watching — which
/// reads as headroom, and reads exactly like the right answer.
#[tokio::test]
async fn each_accounts_quota_survives_a_turn_on_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);

    let (base, _seen, usage) = daemon(
        Arc::clone(&store),
        vec![
            tier("sonnet", "gpt-5.6-terra", None),
            tier("haiku", "gpt-5.6-luna", Some("spare")),
        ],
    )
    .await;

    assert_eq!(turn(&base, "sonnet", "the main turn").await.status(), 200);
    assert_eq!(turn(&base, "haiku", "the cheap turn").await.status(), 200);

    let percent = |account: &str| {
        usage
            .latest_for(account)
            .map(|measured| measured.snapshot.windows[0].used_percent)
    };
    assert_eq!(
        percent("acct_serving"),
        Some(11.0),
        "the serving account's figure was displaced by the pinned tier's turn"
    );
    assert_eq!(
        percent("spare"),
        Some(77.0),
        "the pinned account's turn was filed under someone else"
    );

    // And the figure reported where a single daemon-wide one always was is the
    // serving account's, whichever turn happened to be last.
    assert_eq!(
        usage.latest().map(|s| s.windows[0].used_percent),
        Some(11.0)
    );
}

/// A figure that rode a turn says so.
#[tokio::test]
async fn a_turns_figure_states_that_it_rode_a_turn() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);

    let (base, _seen, usage) = daemon(
        Arc::clone(&store),
        vec![tier("sonnet", "gpt-5.6-terra", None)],
    )
    .await;
    assert_eq!(turn(&base, "sonnet", "the main turn").await.status(), 200);

    let measured = usage.latest_for("acct_serving").expect("a turn was served");
    assert_eq!(measured.source, proxenos::usage::Source::Turn);
    assert!(measured.at > 0);
}

/// A metered account's spend is the one thing that can be stated about it
/// without a price list, so what upstream charged a turn is tallied under the
/// account that served it — the pinned account where a tier pinned one.
#[tokio::test]
async fn what_a_turn_cost_is_tallied_under_the_account_that_served_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);

    let (base, _seen, usage) = daemon(
        Arc::clone(&store),
        vec![
            tier("sonnet", "gpt-5.6-terra", None),
            tier("haiku", "gpt-5.6-luna", Some("spare")),
        ],
    )
    .await;

    assert_eq!(turn(&base, "sonnet", "the main turn").await.status(), 200);
    assert_eq!(
        turn(&base, "sonnet", "another main turn").await.status(),
        200
    );
    assert_eq!(turn(&base, "haiku", "the cheap turn").await.status(), 200);

    let serving = usage.spent_for("acct_serving");
    assert_eq!(
        (serving.input, serving.output),
        (200, 40),
        "two turns as the serving account, at upstream's own counts"
    );

    let spare = usage.spent_for("spare");
    assert_eq!(
        (spare.input, spare.output),
        (900, 90),
        "the pinned tier's turn was tallied under someone else"
    );
}

/// §2.3 — a tagged turn is translated on the mapping of the account it is
/// served as.
///
/// The tag already decided who pays. The mapping it was translated on was the
/// selection's, so `[accounts.<name>.tiers]` reached a session that named the
/// account and was ignored, and the turn went upstream asking for a model
/// stated for somebody else — on the tagged account's own credential, where an
/// id its subscription does not serve is refused for a reason the mapping
/// never mentions.
#[tokio::test]
async fn a_tagged_turn_translates_on_the_tagged_accounts_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);
    let config: proxenos::config::Config = toml::from_str(
        r#"
[tiers]
sonnet = "gpt-5.6-terra"

[accounts.spare.tiers]
sonnet = "gpt-5.4-mini"
"#,
    )
    .unwrap();

    let (base, seen, _usage, asked) = daemon_with(
        Arc::clone(&store),
        vec![tier("sonnet", "gpt-5.6-terra", None)],
        config,
    )
    .await;

    assert_eq!(
        tagged_turn(&base, "sonnet", "the tagged turn", "spare")
            .await
            .status(),
        200
    );

    assert_eq!(
        seen.lock().unwrap().clone(),
        vec!["Bearer spare-token".to_owned()],
        "the tag decides who pays"
    );
    assert_eq!(
        asked.lock().unwrap().clone(),
        vec!["gpt-5.4-mini".to_owned()],
        "and the mapping the turn is translated on is that account's"
    );
}

/// A tag naming the account that is selected keeps the mapping in force.
///
/// The mapping in force is the selection's, resolved when it was selected and
/// moved since by anything `tiers.set` did — so it is the answer for the
/// selection, and re-resolving it from the file would drop a change that was
/// never persisted. The configuration here states something else for the same
/// tier, and the turn must not take it.
#[tokio::test]
async fn a_tag_naming_the_selected_account_keeps_the_mapping_in_force() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);
    let config: proxenos::config::Config = toml::from_str(
        r#"
[tiers]
sonnet = "gpt-5.4-mini"
"#,
    )
    .unwrap();

    let (base, seen, _usage, asked) = daemon_with(
        Arc::clone(&store),
        vec![tier("sonnet", "gpt-5.6-terra", None)],
        config,
    )
    .await;

    assert_eq!(
        tagged_turn(&base, "sonnet", "the tagged turn", "acct_serving")
            .await
            .status(),
        200
    );

    assert_eq!(
        seen.lock().unwrap().clone(),
        vec!["Bearer serving-token".to_owned()]
    );
    assert_eq!(
        asked.lock().unwrap().clone(),
        vec!["gpt-5.6-terra".to_owned()]
    );
}

// ---------------------------------------------------------------------------
// §7.1 — a tier the catalog cannot serve refuses its own turns and nothing
// else. One retired model used to stop the daemon at startup, taking every
// tier that resolves and every worker on the box with it.
// ---------------------------------------------------------------------------

/// The same tier, marked as the catalog marks one it cannot serve.
fn missing_tier(name: &'static str, model: &str, reason: &str) -> ResolvedTier {
    ResolvedTier {
        missing: Some(reason.to_owned()),
        ..tier(name, model, None)
    }
}

/// **The acceptance criterion.** A marked tier's turn is refused, in the
/// sentence the daemon's start used to refuse with — and every other tier
/// keeps serving, on the same daemon, in the same run.
#[tokio::test]
async fn a_turn_on_a_missing_tier_is_refused_while_the_others_serve() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_with_two_accounts(&dir);
    let reason = "tier `fable` cannot serve: these mapped models are not in the catalog: \
                  gpt-5.6-sol. Available: gpt-5.4-mini, gpt-5.6-terra";
    let (base, _seen, _usage, asked) = daemon_with(
        Arc::clone(&store),
        vec![
            tier("sonnet", "gpt-5.6-terra", None),
            missing_tier("fable", "gpt-5.6-sol", reason),
        ],
        proxenos::config::Config::default(),
    )
    .await;

    let refused = turn(&base, "fable", "the turn nothing can serve").await;
    assert_eq!(refused.status(), 400);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["type"], "error", "{body}");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("fable"), "{body}");
    assert!(message.contains("gpt-5.6-sol"), "{body}");
    assert!(
        message.contains("gpt-5.6-terra"),
        "the refusal names what is available: {body}"
    );

    assert_eq!(
        turn(&base, "sonnet", "the turn that still works")
            .await
            .status(),
        200,
        "a resolving tier keeps serving"
    );

    // And the refused turn never reached the backend. A tier that cannot be
    // served must cost nothing, or the refusal is only a nicer error on a
    // request that was already billed.
    assert_eq!(
        asked.lock().unwrap().clone(),
        vec!["gpt-5.6-terra".to_owned()]
    );
}
