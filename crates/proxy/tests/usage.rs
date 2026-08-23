//! `docs/api.md` §3 — the quota snapshot the backend opens a stream with.
//!
//! The payloads here are shaped like a real one, because they came from one.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use pretty_assertions::assert_eq;
use proxenos::usage::Snapshot;
use proxenos::usage::Source;
use serde_json::json;
use std::sync::Arc;

/// A real snapshot, as the backend sends it: one long window, no second one.
fn free_plan() -> String {
    json!({
        "type": "codex.rate_limits",
        "plan_type": "free",
        "credits": { "balance": null, "has_credits": false },
        "rate_limits": {
            "allowed": true,
            "limit_reached": false,
            "primary": {
                "used_percent": 6,
                "window_minutes": 43200,
                "reset_at": 1789487264u64,
                "reset_after_seconds": 2554912u64,
            },
            "secondary": null,
        },
    })
    .to_string()
}

#[test]
fn a_snapshot_is_read_from_the_event_the_backend_opens_with() {
    let snapshot = Snapshot::parse(&free_plan()).expect("this is a rate-limit event");

    assert_eq!(snapshot.plan.as_deref(), Some("free"));
    assert!(!snapshot.limit_reached);
    assert_eq!(snapshot.windows.len(), 1);
    assert_eq!(snapshot.windows[0].used_percent, 6.0);
    assert_eq!(snapshot.windows[0].window_minutes, Some(43200));
}

/// Anything else in the stream is not a snapshot.
#[test]
fn other_events_are_not_snapshots() {
    assert!(Snapshot::parse(&json!({ "type": "response.created" }).to_string()).is_none());
    assert!(Snapshot::parse("not json").is_none());
}

/// Windows are matched to header slots by how long they are, never by their
/// position in the payload.
///
/// The backend has changed which windows it reports — a five-hour window
/// existed, was removed, and may return — so `primary` is not a synonym for
/// "the five-hour one". Position-based mapping would put whatever is reported
/// first into the five-hour slot and be wrong the moment the set changes again.
#[test]
fn windows_map_to_slots_by_duration() {
    let payload = json!({
        "type": "codex.rate_limits",
        "rate_limits": {
            "limit_reached": false,
            // Deliberately the wrong way round: the long window first.
            "primary": { "used_percent": 40, "window_minutes": 10080, "reset_at": 200 },
            "secondary": { "used_percent": 10, "window_minutes": 300, "reset_at": 100 },
        },
    })
    .to_string();

    let headers = Snapshot::parse(&payload).unwrap().headers();
    let get = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    };

    assert_eq!(
        get("anthropic-ratelimit-unified-5h-utilization").as_deref(),
        Some("0.1000")
    );
    assert_eq!(
        get("anthropic-ratelimit-unified-5h-reset").as_deref(),
        Some("100")
    );
    assert_eq!(
        get("anthropic-ratelimit-unified-7d-utilization").as_deref(),
        Some("0.4000")
    );
}

/// A window matching no slot produces no header at all.
///
/// The live account's window is thirty days. Announcing that as five hours
/// would show a meter that is wrong in the reassuring direction — it would read
/// as plenty of headroom resetting shortly, when neither is true.
#[test]
fn a_window_that_fits_no_slot_is_not_forced_into_one() {
    let headers = Snapshot::parse(&free_plan()).unwrap().headers();
    assert!(
        headers.is_empty(),
        "a thirty-day window fits neither slot: {headers:?}"
    );
}

/// The control socket still reports it, with its real length — that is the
/// difference between the two surfaces, and why both exist.
#[test]
fn the_socket_reports_a_window_the_headers_cannot() {
    let reported = Snapshot::parse(&free_plan()).unwrap().to_json();

    assert_eq!(reported["known"], json!(true));
    assert_eq!(reported["plan"], json!("free"));
    assert_eq!(reported["windows"][0]["window_minutes"], json!(43200));
    assert_eq!(reported["windows"][0]["used_percent"], json!(6.0));
}

/// A window with no percentage is absent, not zero.
#[test]
fn a_window_without_a_percentage_is_not_reported_as_empty() {
    let payload = json!({
        "type": "codex.rate_limits",
        "rate_limits": {
            "limit_reached": false,
            "primary": { "window_minutes": 300, "reset_at": 100 },
            "secondary": null,
        },
    })
    .to_string();

    let snapshot = Snapshot::parse(&payload).unwrap();
    assert!(snapshot.windows.is_empty());
    assert!(snapshot.headers().is_empty());
}

/// A reached limit says so, in the word the client parses.
#[test]
fn a_reached_limit_is_reported_as_rejected() {
    let payload = json!({
        "type": "codex.rate_limits",
        "rate_limits": {
            "limit_reached": true,
            "primary": { "used_percent": 100, "window_minutes": 300, "reset_at": 100 },
        },
    })
    .to_string();

    let headers = Snapshot::parse(&payload).unwrap().headers();
    assert!(
        headers.contains(&("anthropic-ratelimit-unified-status", "rejected".to_owned())),
        "{headers:?}"
    );
}

/// The quota endpoint's own shape, read from a recorded response.
///
/// It lives under `fixtures/upstream/` rather than in the probe corpus: the
/// corpus is a set of replayable Messages exchanges, and this is a recorded
/// REST response. Putting it there made four corpus tests fail, which is the
/// corpus correctly refusing something that is not one of its own.
///
/// **This fixture was captured, not written.** The shape differs from the
/// stream event's in three ways that a guess would have got wrong: the windows
/// are `primary_window`/`secondary_window` rather than `primary`/`secondary`,
/// the length is stated in **seconds** rather than minutes, and the plan sits at
/// the top level rather than beside the limits. A parser written from the
/// stream shape would have parsed this into nothing and reported "no quota" on
/// an account that has one.
#[test]
fn a_recorded_quota_response_parses_into_the_same_snapshot_a_stream_produces() {
    let payload = std::fs::read_to_string("../../fixtures/upstream/quota-rest.json").unwrap();

    let snapshot = proxenos::usage::Snapshot::parse_rest(&payload)
        .expect("the recorded response should parse");

    assert_eq!(snapshot.plan.as_deref(), Some("free"));
    assert!(!snapshot.limit_reached);
    assert_eq!(snapshot.windows.len(), 1);

    let window = &snapshot.windows[0];
    assert_eq!(window.used_percent, 15.0);
    // Seconds on the wire, minutes in the snapshot — the unit every other
    // reader of a window already uses.
    assert_eq!(window.window_minutes, Some(43_200));
    assert_eq!(window.resets_at, Some(1_789_487_264));
}

/// A null window is absent, never a window reporting zero used.
#[test]
fn a_window_the_backend_did_not_report_is_absent_rather_than_zero() {
    let payload = std::fs::read_to_string("../../fixtures/upstream/quota-rest.json").unwrap();
    let snapshot = proxenos::usage::Snapshot::parse_rest(&payload).unwrap();

    assert!(
        snapshot
            .windows
            .iter()
            .all(|window| window.used_percent > 0.0),
        "a null secondary window must not become a zeroed one"
    );
}

/// Something that is not this response is refused rather than parsed into an
/// empty snapshot — which would read as "quota known, nothing used".
#[test]
fn an_unrecognized_body_is_not_read_as_an_empty_quota() {
    assert!(proxenos::usage::Snapshot::parse_rest("{}").is_none());
    assert!(proxenos::usage::Snapshot::parse_rest("not json").is_none());
    assert!(
        proxenos::usage::Snapshot::parse_rest(r#"{"rate_limit":{}}"#).is_none(),
        "a rate_limit with no window at all says nothing about quota"
    );
}

/// A snapshot with a percentage no other snapshot in a test can be confused
/// for.
fn snapshot(used_percent: f64) -> Snapshot {
    Snapshot {
        plan: Some("plus".to_owned()),
        limit_reached: false,
        windows: vec![proxenos::usage::Window {
            used_percent,
            window_minutes: Some(300),
            resets_at: Some(1_789_487_264),
            ..proxenos::usage::Window::default()
        }],
    }
}

/// A store whose serving account is whatever this name says.
fn store_serving(name: &'static str) -> proxenos::usage::UsageStore {
    proxenos::usage::UsageStore::default()
        .serving(Arc::new(move || Some(name.to_owned())) as proxenos::usage::ServingAccount)
}

/// **Build 1, at the store.** Two accounts can serve one session, so a figure
/// is held under the account it belongs to rather than as the daemon's one
/// latest.
#[test]
fn each_accounts_figure_is_held_under_its_own_name() {
    let store = store_serving("main");

    store.record_for(None, &snapshot(11.0), Source::Turn);
    store.record_for(Some("spare"), &snapshot(77.0), Source::Turn);

    let held: Vec<(String, f64)> = store
        .accounts()
        .into_iter()
        .map(|(name, measured)| (name, measured.snapshot.windows[0].used_percent))
        .collect();
    assert_eq!(
        held,
        vec![("main".to_owned(), 11.0), ("spare".to_owned(), 77.0)],
        "a pinned tier's turn must not displace the serving account's figure"
    );
}

/// An unpinned turn is filed under the account that actually served it, by
/// name — which is what lets a later select report the right account's figure
/// rather than whatever the last turn happened to leave behind.
#[test]
fn an_unpinned_turn_is_filed_under_the_serving_accounts_name() {
    let store = store_serving("main");
    store.record_for(None, &snapshot(11.0), Source::Turn);

    assert_eq!(
        store.latest_for("main").map(|m| m.snapshot),
        Some(snapshot(11.0))
    );
}

/// The figure reported where one has always been reported is the serving
/// account's, never a pinned account's.
#[test]
fn the_top_level_figure_follows_the_serving_account() {
    let store = store_serving("main");
    store.record_for(Some("spare"), &snapshot(77.0), Source::Turn);

    assert_eq!(
        store.latest(),
        None,
        "the serving account has made no turn, and the pinned account's \
         headroom is not an answer for it"
    );

    store.record_for(None, &snapshot(11.0), Source::Turn);
    assert_eq!(store.latest(), Some(snapshot(11.0)));
}

/// How a figure was come by, and when, travels with it: one that rode a turn
/// and one that was asked for are both legitimate and differently stale.
#[test]
fn a_figure_carries_how_it_was_come_by_and_when() {
    let store = store_serving("main");
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    store.record_for(None, &snapshot(11.0), Source::Turn);
    store.record_for(Some("spare"), &snapshot(77.0), Source::Fetch);

    let main = store.latest_for("main").unwrap();
    assert_eq!(main.source, Source::Turn);
    assert!(main.at >= before, "the moment it was taken travels with it");
    assert_eq!(store.latest_for("spare").unwrap().source, Source::Fetch);
}

/// **Build 4, at the store.** Removing an account drops its figure and
/// nothing else: the rest stay valid because each is held under its own name.
#[test]
fn removing_an_account_drops_only_its_own_figure() {
    let store = store_serving("main");
    store.record_for(None, &snapshot(11.0), Source::Turn);
    store.record_for(Some("spare"), &snapshot(77.0), Source::Turn);

    store.forget("spare");

    assert!(store.latest_for("spare").is_none());
    assert_eq!(
        store.latest_for("main").map(|m| m.snapshot),
        Some(snapshot(11.0))
    );
}

/// A figure taken where no account could be named is reported at the top and
/// never as some account's, and a select drops it — presenting it as the newly
/// selected account's headroom is the error this whole keying exists to stop.
#[test]
fn a_figure_no_account_can_be_named_for_is_dropped_by_a_select() {
    let store = proxenos::usage::UsageStore::default();
    store.record_for(None, &snapshot(11.0), Source::Turn);

    assert_eq!(store.latest(), Some(snapshot(11.0)));
    assert!(
        store.accounts().is_empty(),
        "nothing can name it, so it names nothing"
    );

    store.forget_unattributed();
    assert_eq!(store.latest(), None);
}

/// The wiring the daemon actually uses: the serving account is whichever one
/// the credential store has selected, read when the figure is recorded.
///
/// Asserted through the store rather than through the switch, because a
/// resolver that was never handed a credential store files every figure as
/// unattributed and reports an empty per-account meter while every unit test
/// still passes.
#[test]
fn the_serving_account_is_the_one_the_credential_store_has_selected() {
    use proxenos::auth::store::AccountStore;

    let dir = tempfile::tempdir().unwrap();
    let credentials = Arc::new(proxenos::auth::store::FileStore::new(
        dir.path().join("credentials.json"),
    ));
    let grant = |account_id: &str| proxenos::auth::store::Credentials {
        access_token: "token".to_owned(),
        refresh_token: "refresh".to_owned(),
        id_token: None,
        account_id: Some(account_id.to_owned()),
        expires_at: Some(u64::MAX / 2),
    };
    credentials.add(&grant("acct_main"), Some("main")).unwrap();
    credentials
        .add(&grant("acct_spare"), Some("spare"))
        .unwrap();
    credentials.select("spare").unwrap();

    let store = proxenos::usage::UsageStore::for_accounts(
        Arc::clone(&credentials) as Arc<dyn AccountStore>
    );
    store.record_for(None, &snapshot(23.0), Source::Turn);

    assert_eq!(
        store.latest_for("spare").map(|m| m.snapshot),
        Some(snapshot(23.0))
    );
    assert!(store.latest_for("main").is_none());

    // And it follows the selection rather than remembering it.
    credentials.select("main").unwrap();
    store.record_for(None, &snapshot(41.0), Source::Turn);
    assert_eq!(
        store.latest_for("main").map(|m| m.snapshot),
        Some(snapshot(41.0))
    );
}

/// The second provider answers quota in response headers, and that is the only
/// place it answers one for this credential kind: its usage endpoint refuses a
/// subscription token for want of a scope. `fixtures/upstream/relay-quota-headers.json`
/// is one live turn's headers, captured rather than written.
#[test]
fn a_snapshot_is_read_from_the_second_providers_response_headers() {
    let captured: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/upstream/relay-quota-headers.json"
    ))
    .expect("the fixture is JSON");

    let mut headers = axum::http::HeaderMap::new();
    for (name, value) in captured["headers"].as_object().expect("a header map") {
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.as_str().unwrap().parse().unwrap(),
        );
    }

    let snapshot = Snapshot::from_headers(&headers).expect("these headers carry a quota");

    assert!(!snapshot.limit_reached, "the turn was allowed");
    // Plan is genuinely unavailable on this path — no header states one — and
    // an invented plan name is worse than none.
    assert_eq!(snapshot.plan, None);

    let window = |minutes: u64| {
        snapshot
            .windows
            .iter()
            .find(|window| window.window_minutes == Some(minutes))
            .unwrap_or_else(|| panic!("no {minutes}-minute window in {:?}", snapshot.windows))
    };
    assert_eq!(window(300).used_percent, 13.0);
    assert_eq!(window(300).resets_at, Some(1_787_338_800));
    assert_eq!(window(10080).used_percent, 93.0);
    assert_eq!(window(10080).resets_at, Some(1_787_371_200));
}

/// A response carrying no quota header says nothing about quota, and saying
/// nothing is not the same as saying none is used.
#[test]
fn headers_without_a_quota_are_not_an_empty_snapshot() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        "text/event-stream".parse().unwrap(),
    );
    assert!(Snapshot::from_headers(&headers).is_none());
}

/// A refused turn says so, and the meter has to show it.
#[test]
fn a_rejected_status_reads_as_the_limit_reached() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "anthropic-ratelimit-unified-5h-utilization",
        "1.0".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-unified-status",
        "rejected".parse().unwrap(),
    );
    let snapshot = Snapshot::from_headers(&headers).expect("a window was reported");
    assert!(snapshot.limit_reached);
    assert_eq!(snapshot.windows[0].used_percent, 100.0);
}

/// Headers captured from one live relayed turn, as a header map.
fn relay_quota_headers() -> axum::http::HeaderMap {
    let captured: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/upstream/relay-quota-headers.json"
    ))
    .expect("the fixture is JSON");

    let mut headers = axum::http::HeaderMap::new();
    for (name, value) in captured["headers"].as_object().expect("a header map") {
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.as_str().unwrap().parse().unwrap(),
        );
    }
    headers
}

/// The provider attaches a status to each window, and `allowed_warning` is an
/// account past the threshold the provider itself set on a turn that still went
/// through. Dropping it at parse leaves the meter printing the number and none
/// of the warning that came with it.
#[test]
fn a_per_window_warning_survives_the_parse() {
    let snapshot = Snapshot::from_headers(&relay_quota_headers()).expect("a quota was reported");

    let window = |minutes: u64| {
        snapshot
            .windows
            .iter()
            .find(|window| window.window_minutes == Some(minutes))
            .unwrap_or_else(|| panic!("no {minutes}-minute window in {:?}", snapshot.windows))
    };

    assert_eq!(window(300).status.as_deref(), Some("allowed"));
    assert_eq!(window(300).surpassed_threshold, None);
    assert_eq!(window(10080).status.as_deref(), Some("allowed_warning"));
    assert_eq!(window(10080).surpassed_threshold, Some(0.75));
    // The turn went through. A warning is not a refusal, and reading it as one
    // would show a limit the account has not hit.
    assert!(!snapshot.limit_reached);
}

/// The provider names which window decides whether the account is about to be
/// cut off. With `5h` at 13% and `7d` at 93%, a reader taking the first line
/// reads the reassuring one.
#[test]
fn the_representative_window_is_the_one_the_provider_names() {
    let snapshot = Snapshot::from_headers(&relay_quota_headers()).expect("a quota was reported");

    let representative: Vec<Option<u64>> = snapshot
        .windows
        .iter()
        .filter(|window| window.representative)
        .map(|window| window.window_minutes)
        .collect();
    assert_eq!(representative, vec![Some(10080)]);
}

/// The overage window is a real window with a real figure. Dropping it at parse
/// is not the same as deciding it does not belong on the meter.
#[test]
fn the_overage_window_is_kept() {
    let snapshot = Snapshot::from_headers(&relay_quota_headers()).expect("a quota was reported");

    let overage = snapshot
        .windows
        .iter()
        .find(|window| window.label.as_deref() == Some("overage"))
        .unwrap_or_else(|| panic!("no overage window in {:?}", snapshot.windows));
    assert_eq!(overage.used_percent, 0.0);
    assert_eq!(overage.resets_at, Some(1_788_220_800));
    assert_eq!(overage.status.as_deref(), Some("allowed"));
    // It has no duration on the wire, and inventing one would name a window the
    // provider never named.
    assert_eq!(overage.window_minutes, None);
    assert!(!overage.representative);
}

/// A window whose reset has passed is a figure about a window that is over.
/// Whether that is so is a property of one window, never of the snapshot: a
/// stored snapshot can hold a five-hour window that has turned over beside a
/// seven-day one that has not.
#[test]
fn staleness_is_a_property_of_one_window() {
    let snapshot = Snapshot::from_headers(&relay_quota_headers()).expect("a quota was reported");

    let window = |minutes: u64| {
        snapshot
            .windows
            .iter()
            .find(|window| window.window_minutes == Some(minutes))
            .expect("the window is in the fixture")
    };

    // Between the 5h reset and the 7d one, which is exactly the case marking
    // the whole snapshot would get wrong in both directions at once.
    let between = 1_787_350_000;
    assert!(window(300).is_stale_at(between));
    assert!(!window(10080).is_stale_at(between));

    // Before either reset, both figures still describe the window they came
    // from.
    assert!(!window(300).is_stale_at(1_787_338_000));
    assert!(!window(10080).is_stale_at(1_787_338_000));

    // After both, both have turned over.
    assert!(window(300).is_stale_at(1_787_400_000));
    assert!(window(10080).is_stale_at(1_787_400_000));
}

/// A window the provider stated no reset for cannot be said to have turned
/// over. Guessing it has would hide a figure that is still true.
#[test]
fn a_window_with_no_reset_is_never_called_stale() {
    let window = proxenos::usage::Window {
        used_percent: 42.0,
        window_minutes: Some(300),
        ..proxenos::usage::Window::default()
    };
    assert!(!window.is_stale_at(u64::MAX));
}

/// The meter renders a window against a clock, so a figure whose window has
/// since turned over can say so. Rendering it silently reads as spend the
/// account has not made, which is what sends an operator to switch accounts
/// they did not need to switch.
#[test]
fn a_window_that_has_reset_says_so_on_the_meter() {
    let snapshot = Snapshot::from_headers(&relay_quota_headers()).expect("a quota was reported");
    let mut result = snapshot.to_json();
    result["accounts"] = serde_json::json!([]);

    // Between the 5h reset and the 7d one.
    let rendered = proxenos::render::usage_at(&result, 1_787_350_000);
    let five_hour = row_for(&rendered, "of 5h");
    let seven_day = row_for(&rendered, "of 7d");

    assert!(five_hour.contains("already reset"), "{rendered}");
    assert!(
        !seven_day.contains("already reset"),
        "the seven-day figure is still true:\n{rendered}"
    );

    // Before either reset, neither figure has outlived its window, and the
    // cell says when each comes back instead.
    let fresh = proxenos::render::usage_at(&result, 1_787_338_000);
    assert!(!fresh.contains("already reset"), "{fresh}");
    assert!(row_for(&fresh, "of 5h").contains("in 13m"), "{fresh}");
    assert!(row_for(&fresh, "of 7d").contains("in 9h"), "{fresh}");
}

/// The row a figure is rendered on.
fn row_for(rendered: &str, figure: &str) -> String {
    rendered
        .lines()
        .find(|line| line.contains(figure))
        .expect("the figure should be on a row")
        .to_owned()
}

/// The provider's own warning, its own threshold, and its own answer to which
/// window decides — all three are on the wire of every relayed turn, and a
/// meter printing the number alone drops each of them.
#[test]
fn the_meter_carries_the_providers_own_words() {
    let snapshot = Snapshot::from_headers(&relay_quota_headers()).expect("a quota was reported");
    let mut result = snapshot.to_json();
    result["accounts"] = serde_json::json!([]);

    let rendered = proxenos::render::usage_at(&result, 1_787_338_000);

    // The seven-day window is the one the provider named, and the one it
    // warned about.
    assert!(
        row_for(&rendered, "of 7d").contains("decides"),
        "{rendered}"
    );
    assert!(
        row_for(&rendered, "of 7d").contains("past the provider's 75%"),
        "{rendered}"
    );
    // The five-hour window was allowed with no warning, and saying nothing is
    // the accurate thing to say about it.
    assert!(
        !row_for(&rendered, "of 5h").contains("decides"),
        "{rendered}"
    );
    assert!(
        !row_for(&rendered, "of 5h").contains("past the provider's"),
        "{rendered}"
    );
    // The overage window is named by the provider's word for it, because it
    // has no duration to be named by.
    assert!(rendered.contains("0% of overage"), "{rendered}");
}

/// The whole table, verbatim: one account per state a row can be in, and one
/// account holding two windows at once.
///
/// A column layout is exactly the kind of thing that reads well in the diff
/// and wrong in the terminal, so this asserts the characters rather than a
/// substring of them.
#[test]
fn the_meter_is_a_table_one_row_per_window() {
    let result = json!({
        "known": true,
        "plan": "team",
        "limit_reached": false,
        "accounts": [
            {
                "known": true,
                "account": "work-codex",
                "provider": "codex",
                "serving": true,
                "source": "turn",
                "measured_at": 1_787_337_760u64,
                "served_tokens": 12_000,
                "windows": [
                    { "used_percent": 42.0, "window_minutes": 300, "resets_at": 1_787_349_520u64 },
                    { "used_percent": 18.0, "window_minutes": 10080, "resets_at": 1_787_856_400u64 },
                ],
            },
            {
                "known": false,
                "account": "spare-codex",
                "provider": "codex",
                "serving": false,
                "reason": "no_turn",
                "served_tokens": 0,
                "detail": "codex reports quota when a turn is made; this daemon has recorded no turn as this account yet",
            },
            {
                "known": false,
                "account": "personal-claude",
                "provider": "anthropic",
                "serving": false,
                "reason": "no_relayed_turn",
                "served_tokens": 0,
                "detail": "anthropic states quota on every turn; this daemon has recorded no relayed turn as this account yet",
            },
            {
                "known": false,
                "account": "openai-api",
                "provider": "codex",
                "serving": false,
                "reason": "metered",
                "served_tokens": 1_540,
                "detail": "a key has no quota ceiling",
            },
        ],
    });

    let rendered = proxenos::render::usage_at(&result, 1_787_338_000);
    assert_eq!(
        rendered,
        "\
plan       team
  NAME             PROVIDER   USED       RESETS     SOURCE     AS OF
* work-codex       codex      42% of 5h  in 3h 12m  last turn  4m ago
                              18% of 7d  in 6d
  spare-codex      codex      -          -          -          no turn yet
  personal-claude  anthropic  -          -          -          no relayed turn yet
  openai-api       codex      1540 tok   -          -          per token
note: an empty row is this daemon's record rather than the account's — a figure arrives with the next turn served as it, or now with `usage --refresh`; a metered row has no ceiling to report, and its count is what this daemon served as it rather than the whole of its spend."
    );
}

/// One `accounts use` moves every unpinned turn onto that account's provider
/// and its subscription. The operator asked for it; the command reads smaller
/// than what it does, and the provider is the half a name does not state.
#[test]
fn selecting_an_account_says_which_provider_now_serves() {
    let rendered = proxenos::render::selected_account(&serde_json::json!({
        "selected": "personal-relay",
        "provider": "anthropic",
    }));
    assert!(rendered.contains("personal-relay"), "{rendered}");
    assert!(rendered.contains("anthropic"), "{rendered}");

    // And where it is a move rather than a first selection, the line says what
    // the move costs: a different backend, on a different subscription.
    let moved = proxenos::render::selected_account(&serde_json::json!({
        "selected": "personal-relay",
        "provider": "anthropic",
        "previous_provider": "codex",
    }));
    assert!(moved.contains("subscription"), "{moved}");

    // A daemon that could not name the provider says the part it knows and
    // invents nothing.
    let quiet = proxenos::render::selected_account(&serde_json::json!({ "selected": "spare" }));
    assert_eq!(quiet, "serving turns as spare");
}

/// The meter and the parse side answer "has this window turned over" the same
/// way, because they ask the same predicate.
///
/// A rule written twice does not stay written twice the same. The copy under
/// test stays right and the copy that ships drifts, and the suite says nothing
/// — so this asserts the two agree over the clocks where they could differ,
/// against what `usage` actually prints rather than against the predicate
/// alone.
#[test]
fn the_meter_and_the_predicate_agree_on_what_has_reset() {
    let snapshot = Snapshot::from_headers(&relay_quota_headers()).expect("a quota was reported");
    let mut result = snapshot.to_json();
    result["accounts"] = serde_json::json!([]);

    // Straddling both resets in the capture: before either, between them, and
    // after both.
    for now in [1_787_338_000, 1_787_350_000, 1_787_400_000] {
        let rendered = proxenos::render::usage_at(&result, now);
        for window in &snapshot.windows {
            let name = window.window_minutes.map_or_else(
                || {
                    window
                        .label
                        .clone()
                        .expect("a window is named one way or the other")
                },
                |minutes| {
                    if minutes % (24 * 60) == 0 {
                        format!("{}d", minutes / (24 * 60))
                    } else {
                        format!("{}h", minutes / 60)
                    }
                },
            );
            let line = row_for(&rendered, &format!("of {name}"));

            assert_eq!(
                line.contains("already reset"),
                window.is_stale_at(now),
                "at {now}, the meter and the predicate disagree about {name}:\n{rendered}"
            );
            // The figure itself is kept either way. A window that has turned
            // over is not a zero — that would be a number the provider never
            // gave, and it reads as headroom.
            assert!(
                line.contains(&format!("{:.0}% of {name}", window.used_percent)),
                "the figure must survive the marking:\n{rendered}"
            );
        }
    }
}

/// Tokens served are tallied under the account that served them, the same way
/// a quota figure is — the point of the tally is a metered account's own
/// spend, and one account's tokens under another's name would misstate both.
#[test]
fn tokens_served_are_tallied_under_the_account_that_served_them() {
    let store = store_serving("main");

    store.record_spend(None, 100, 20);
    store.record_spend(None, 5, 1);
    store.record_spend(Some("spare"), 900, 90);

    let main = store.spent_for("main");
    assert_eq!((main.input, main.output), (105, 21));
    assert_eq!(main.total(), 126);

    let spare = store.spent_for("spare");
    assert_eq!((spare.input, spare.output), (900, 90));
}

/// An account nothing has been served as reports zero rather than nothing:
/// "this daemon has served no tokens as it" is a true statement about spend,
/// where a quota figure of zero would be an invented entitlement.
#[test]
fn an_account_nothing_was_served_as_has_served_nothing() {
    let store = store_serving("main");
    assert_eq!(store.spent_for("main"), proxenos::usage::Spent::default());
    assert_eq!(store.spent_for("main").total(), 0);
}

/// A turn no account can be named for is not counted at all. Attributing it to
/// whoever happens to be serving would put one account's spend under another's
/// name, which is the error the whole keying exists to stop.
#[test]
fn a_turn_no_account_can_be_named_for_is_not_tallied() {
    let store = proxenos::usage::UsageStore::default();
    store.record_spend(None, 100, 20);
    assert_eq!(store.spent_for("main").total(), 0);
}

/// Removing an account drops what was served as it, along with its figure:
/// the tally answers "how much has this account spent through this daemon",
/// and there is no such account.
#[test]
fn removing_an_account_drops_what_was_served_as_it() {
    let store = store_serving("main");
    store.record_spend(None, 100, 20);
    store.record_spend(Some("spare"), 900, 90);

    store.forget("spare");

    assert_eq!(store.spent_for("spare").total(), 0);
    assert_eq!(store.spent_for("main").total(), 120);
}

/// A store bound to a tally file, serving `name`.
fn store_tallying(name: &'static str, path: &std::path::Path) -> proxenos::usage::UsageStore {
    proxenos::usage::UsageStore::default()
        .serving(Arc::new(move || Some(name.to_owned())) as proxenos::usage::ServingAccount)
        .tallying_at(path.to_path_buf())
}

/// **The half nothing upstream can restate.** A quota figure is recoverable by
/// asking for it again; the tokens this daemon served are not, and a restart
/// that resets them to zero states a floor of zero that is not true.
#[test]
fn a_tally_survives_a_restart() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("spend.json");

    let first = store_tallying("main", &path);
    first.record_spend(None, 100, 20);
    first.record_spend(Some("spare"), 900, 90);
    drop(first);

    let restarted = store_tallying("main", &path);
    assert_eq!(restarted.spent_for("main").total(), 120);
    assert_eq!(restarted.spent_for("spare").total(), 990);

    // And the restored figure keeps counting rather than starting over.
    restarted.record_spend(None, 1, 1);
    assert_eq!(restarted.spent_for("main").total(), 122);
}

/// **The half upstream can restate is not persisted.** A snapshot read from
/// disk describes a window that may have reset since, and a percentage with a
/// stale age reads as headroom that may not exist. Asking recovers it exactly,
/// so the empty row is the honest one.
#[test]
fn a_quota_figure_does_not_survive_a_restart() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("spend.json");

    let first = store_tallying("main", &path);
    first.record_for(None, &snapshot(11.0), Source::Turn);
    assert!(first.latest_for("main").is_some());
    drop(first);

    let restarted = store_tallying("main", &path);
    assert_eq!(restarted.latest_for("main"), None);
    assert_eq!(restarted.accounts(), Vec::new());
}

/// A file this daemon cannot read is treated as an empty tally rather than as
/// a failure: nothing here is worth refusing to serve a turn over, and a
/// tally that starts at zero says so everywhere it is reported.
#[test]
fn an_unreadable_tally_reads_as_an_empty_one() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("spend.json");
    std::fs::write(&path, "{ this is not json").expect("write");

    let store = store_tallying("main", &path);
    assert_eq!(store.spent_for("main").total(), 0);

    // And it writes over the unreadable file rather than being stuck behind it.
    store.record_spend(None, 100, 20);
    assert_eq!(store_tallying("main", &path).spent_for("main").total(), 120);
}

/// `PROXENOS_HOME` can point two daemons at one directory, and neither sees the
/// other's turns. The merge takes whichever count is higher per account, so a
/// daemon that has been running longer does not have its total replaced by a
/// younger one's.
///
/// This is the sequential case, and it is all this test claims: the second
/// store reads the first one's file before it writes. The interleaved case —
/// a write landing between another write's read and its replacement — is
/// `a_write_that_lost_a_race_starts_over_against_the_newer_file`.
#[test]
fn a_second_daemons_write_keeps_the_higher_count() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("spend.json");

    let one = store_tallying("main", &path);
    let two = store_tallying("main", &path);

    one.record_spend(None, 500, 0);
    two.record_spend(None, 10, 0);

    assert_eq!(store_tallying("main", &path).spent_for("main").total(), 500);
}

/// Nothing in the tally is a credential (CLAUDE.md #7): an account name and
/// two token counts, and no place for anything else to be written.
#[test]
fn a_tally_holds_nothing_but_names_and_counts() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("spend.json");

    let store = store_tallying("main", &path);
    store.record_spend(None, 100, 20);

    let raw = std::fs::read_to_string(&path).expect("read");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("parse");
    assert_eq!(
        parsed,
        serde_json::json!({ "main": { "input": 100, "output": 20 } })
    );
}

/// **A write that does not finish must not destroy the tally.** `std::fs::write`
/// truncates the target and then fills it, so a daemon killed between those two
/// leaves a short file — which parses into nothing and comes back as a floor of
/// zero, the exact figure this whole change exists to stop reporting.
///
/// The kernel will not be raced into a torn write on demand, so what is
/// asserted is the property that makes one impossible: the target is replaced,
/// never written into. A file replaced by rename is a different file; a file
/// truncated in place is the same one.
#[cfg(unix)]
#[test]
fn a_tally_is_replaced_whole_rather_than_written_over_in_place() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("spend.json");

    let store = store_tallying("main", &path);
    store.record_spend(None, 1_000_000, 250_000);
    let first = std::fs::metadata(&path).expect("metadata").ino();

    store.record_spend(None, 5_000, 500);
    let second = std::fs::metadata(&path).expect("metadata").ino();

    assert_ne!(
        first, second,
        "the tally must be replaced by rename, never truncated in place"
    );
    assert_eq!(
        store_tallying("main", &path).spent_for("main").total(),
        1_255_500
    );
}

/// What a killed write leaves behind is a half-written *sibling*, and the tally
/// itself is whatever the last finished write left. A stray sibling is not read
/// and does not stop the next write.
#[test]
fn a_sibling_left_by_a_killed_write_does_not_disturb_the_tally() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("spend.json");

    let store = store_tallying("main", &path);
    store.record_spend(None, 1_000_000, 250_000);

    let mut stray = path.clone().into_os_string();
    stray.push(".999999.pending");
    std::fs::write(&stray, "{ \"main\": { \"input\": 1").expect("write");

    let restarted = store_tallying("main", &path);
    assert_eq!(restarted.spent_for("main").total(), 1_250_000);
    restarted.record_spend(None, 1, 0);
    assert_eq!(
        store_tallying("main", &path).spent_for("main").total(),
        1_250_001
    );
}

/// The merge reads the file once. A write that lands after that read is not in
/// what this one is about to replace it with, so replacing it would move the
/// file backwards — the one thing the merge is there to prevent. Re-reading
/// before the replacement catches it, and the attempt starts over against the
/// newer file.
#[test]
fn a_write_that_lost_a_race_starts_over_against_the_newer_file() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("spend.json");

    let store = store_tallying("main", &path);
    store.record_spend(None, 500, 0);

    // Another daemon, landing in the window between this write's read and its
    // replacement, with a count this one has never seen.
    let raced = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let once = std::sync::Arc::clone(&raced);
    let elsewhere = path.clone();
    store.on_tally_write_for_test(move || {
        if once.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        std::fs::write(
            &elsewhere,
            serde_json::json!({ "main": { "input": 9_000, "output": 0 } }).to_string(),
        )
        .expect("write");
    });

    store.record_spend(None, 1, 0);

    let landed = store_tallying("main", &path);
    assert_eq!(
        landed.spent_for("main").total(),
        9_000,
        "the other daemon's count must survive this write"
    );
}

// --- the second provider's quota endpoint ---------------------------------
//
// The body below is the shape a real response has, trimmed to the fields this
// reads. Captured live: `GET /api/oauth/usage` under a borrowed Claude grant.

/// A response as that endpoint sends one.
fn anthropic_usage() -> String {
    serde_json::json!({
        "five_hour": {
            "utilization": 18.0,
            "resets_at": "2026-08-23T09:00:00.383476+00:00",
            "limit_dollars": null,
        },
        "seven_day": {
            "utilization": 20.0,
            "resets_at": "2026-08-29T04:00:00.383497+00:00",
        },
        "seven_day_opus": null,
        // As captured: one group holds several kinds, and the scoped entry
        // describes one model rather than the window — its figure differs
        // from the window's for that reason.
        "limits": [
            { "kind": "session", "group": "session", "percent": 18,
              "severity": "normal", "resets_at": "2026-08-23T09:00:00.383476+00:00",
              "scope": null, "is_active": false },
            { "kind": "weekly_all", "group": "weekly", "percent": 20,
              "severity": "warning", "resets_at": "2026-08-29T04:00:00.383497+00:00",
              "scope": null, "is_active": true },
            { "kind": "weekly_scoped", "group": "weekly", "percent": 0,
              "severity": "critical", "resets_at": "2026-08-29T04:00:00.383497+00:00",
              "scope": { "model": { "id": null, "display_name": "Fable" } },
              "is_active": false },
        ],
        "spend": { "percent": 98, "severity": "critical" },
    })
    .to_string()
}

/// Both windows, with the durations that identify them.
#[test]
fn the_second_providers_endpoint_yields_both_windows() {
    let snapshot = Snapshot::parse_anthropic(&anthropic_usage()).expect("parses");

    assert_eq!(snapshot.windows.len(), 2);
    let five = &snapshot.windows[0];
    assert_eq!(five.window_minutes, Some(300));
    assert!((five.used_percent - 18.0).abs() < f64::EPSILON);
    let seven = &snapshot.windows[1];
    assert_eq!(seven.window_minutes, Some(7 * 24 * 60));
    assert!((seven.used_percent - 20.0).abs() < f64::EPSILON);
}

/// The reset is a timestamp here rather than an epoch, and it is converted
/// rather than dropped: a window with no reset is never marked stale, so
/// dropping it would hide a figure that has turned over.
#[test]
fn a_timestamp_reset_is_converted_to_an_epoch() {
    let snapshot = Snapshot::parse_anthropic(&anthropic_usage()).expect("parses");

    assert_eq!(snapshot.windows[0].resets_at, Some(1_787_475_600));
    assert_eq!(snapshot.windows[1].resets_at, Some(1_787_976_000));
}

/// The provider's own word on each window is read, not inferred from the
/// percentage: an account can sit high on a window still called normal.
///
/// Matched by `group` and by the absence of a `scope`. One group carries
/// several kinds, and the scoped one describes a single model rather than the
/// window — taking it would report that model's severity as the account's.
#[test]
fn each_window_carries_the_providers_own_severity() {
    let snapshot = Snapshot::parse_anthropic(&anthropic_usage()).expect("parses");

    assert_eq!(snapshot.windows[0].status.as_deref(), Some("normal"));
    assert_eq!(snapshot.windows[1].status.as_deref(), Some("warning"));
    assert!(
        snapshot
            .windows
            .iter()
            .all(|window| window.status.as_deref() != Some("critical")),
        "the scoped entry is one model's, not the window's"
    );
}

/// Nothing in that body says a turn would be refused, so nothing here claims
/// one would — a spend at 98% is not a limit reached.
#[test]
fn a_high_figure_is_not_reported_as_a_limit_reached() {
    let snapshot = Snapshot::parse_anthropic(&anthropic_usage()).expect("parses");

    assert!(!snapshot.limit_reached);
}

/// A body with no window says nothing about quota, and saying nothing is not
/// the same as saying none is used.
#[test]
fn a_body_with_no_window_yields_nothing() {
    assert!(Snapshot::parse_anthropic("{}").is_none());
    assert!(Snapshot::parse_anthropic(r#"{"seven_day_opus": null}"#).is_none());
    assert!(Snapshot::parse_anthropic("not json").is_none());
}

/// The stream shape and this one are different bodies. Reading either with the
/// other's parser must yield nothing rather than an empty snapshot.
#[test]
fn the_two_shapes_do_not_parse_each_other() {
    assert!(Snapshot::parse(&anthropic_usage()).is_none());
    assert!(Snapshot::parse_rest(&anthropic_usage()).is_none());
}

/// The conversion itself, including the forms this endpoint does not send.
#[test]
fn only_utc_timestamps_are_converted() {
    use proxenos::usage::epoch_from_rfc3339;

    assert_eq!(epoch_from_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        epoch_from_rfc3339("2026-08-23T09:00:00.383476+00:00"),
        Some(1_787_475_600)
    );
    assert_eq!(
        epoch_from_rfc3339("2024-02-29T12:00:00Z"),
        Some(1_709_208_000)
    );

    // An offset this endpoint has never sent is refused rather than read as
    // UTC: a wrong answer marks a live window stale, or a stale one live.
    assert_eq!(epoch_from_rfc3339("2026-08-23T09:00:00+07:00"), None);
    assert_eq!(epoch_from_rfc3339("2026-08-23"), None);
    assert_eq!(epoch_from_rfc3339(""), None);
}

// --- asking for a figure, per provider ------------------------------------

/// The headers one request arrived with, in the order they arrived.
type SeenHeaders = Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// A loopback endpoint that records what it was asked and answers `body`.
struct QuotaEndpoint {
    url: String,
    seen: SeenHeaders,
}

impl QuotaEndpoint {
    async fn start(body: &'static str) -> Self {
        use axum::extract::State;
        use axum::http::HeaderMap;

        let seen: SeenHeaders = Arc::default();
        let router = axum::Router::new()
            .route(
                "/usage",
                axum::routing::get(
                    move |State(seen): State<SeenHeaders>, headers: HeaderMap| async move {
                        let mut recorded = seen.lock().expect("not poisoned");
                        for (name, value) in &headers {
                            recorded.push((
                                name.as_str().to_owned(),
                                value.to_str().unwrap_or_default().to_owned(),
                            ));
                        }
                        body
                    },
                ),
            )
            .with_state(Arc::clone(&seen));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/usage", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self { url, seen }
    }

    fn header(&self, name: &str) -> Option<String> {
        self.seen
            .lock()
            .expect("not poisoned")
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }
}

fn authorization(
    provider: proxenos::auth::store::Provider,
) -> proxenos::auth::authorize::Authorization {
    proxenos::auth::authorize::Authorization {
        kind: proxenos::auth::authorize::Kind::Subscription,
        provider,
        account: None,
        headers: vec![("authorization".to_owned(), "Bearer borrowed".to_owned())],
    }
}

const ANTHROPIC_BODY: &str = r#"{
    "five_hour": { "utilization": 18.0, "resets_at": "2026-08-23T09:00:00.383476+00:00" },
    "seven_day": { "utilization": 20.0, "resets_at": "2026-08-29T04:00:00.383497+00:00" },
    "limits": [ { "kind": "session", "severity": "normal" } ]
}"#;

/// The second provider's endpoint is read with the second provider's parser.
/// Before this the one parser there was read that body into nothing, and an
/// account with quota reported none.
#[tokio::test]
async fn a_figure_is_read_from_the_second_providers_own_shape() {
    let endpoint = QuotaEndpoint::start(ANTHROPIC_BODY).await;

    let snapshot = proxenos::usage::fetch(
        &reqwest::Client::new(),
        &endpoint.url,
        &authorization(proxenos::auth::store::Provider::Anthropic),
        std::path::Path::new("claude"),
    )
    .await
    .expect("a figure");

    assert_eq!(snapshot.windows.len(), 2);
    assert_eq!(snapshot.windows[0].window_minutes, Some(300));
}

/// It is asked as the client whose credential this is. The endpoint answers a
/// request carrying that client's own string, and this proxy's own would be a
/// claim about a client it is not.
#[tokio::test]
async fn the_second_providers_endpoint_is_asked_as_the_owning_client() {
    let endpoint = QuotaEndpoint::start(ANTHROPIC_BODY).await;

    proxenos::usage::fetch(
        &reqwest::Client::new(),
        &endpoint.url,
        &authorization(proxenos::auth::store::Provider::Anthropic),
        std::path::Path::new("claude"),
    )
    .await
    .expect("a figure");

    let agent = endpoint
        .header("user-agent")
        .expect("a user agent was sent");
    assert!(agent.starts_with("claude-cli"), "was: {agent}");
    assert!(agent.contains("external, cli"), "was: {agent}");
}

/// The first provider keeps its own shape and its own identification. One
/// parser reading both bodies is what this split exists to prevent.
#[tokio::test]
async fn the_first_provider_is_unchanged() {
    let endpoint = QuotaEndpoint::start(
        r#"{"plan_type":"pro","rate_limit":{"primary_window":{"used_percent":10,"window_seconds":18000,"resets_in_seconds":60}}}"#,
    )
    .await;

    let snapshot = proxenos::usage::fetch(
        &reqwest::Client::new(),
        &endpoint.url,
        &authorization(proxenos::auth::store::Provider::Codex),
        std::path::Path::new("claude"),
    )
    .await
    .expect("a figure");

    assert_eq!(snapshot.plan.as_deref(), Some("pro"));
    let agent = endpoint
        .header("user-agent")
        .expect("a user agent was sent");
    assert!(!agent.starts_with("claude-cli"), "was: {agent}");
}

/// One provider's body through the other's parser yields nothing rather than
/// an empty snapshot, and the refusal says the shape was not recognized.
#[tokio::test]
async fn a_body_in_the_other_providers_shape_is_refused() {
    let endpoint = QuotaEndpoint::start(ANTHROPIC_BODY).await;

    let refusal = proxenos::usage::fetch(
        &reqwest::Client::new(),
        &endpoint.url,
        &authorization(proxenos::auth::store::Provider::Codex),
        std::path::Path::new("claude"),
    )
    .await
    .expect_err("the wrong parser reads nothing");

    assert!(refusal.message.contains("shape"), "{}", refusal.message);
}
