//! `docs/proxy-behavior.md` §8.4 — how often one method asks the store.
//!
//! A listing is free against a file and is not against a borrowed profile: on
//! macOS every Claude profile in it is a `security` spawn, and a keychain the
//! operator has to unlock is a prompt per spawn. So a method that reports on
//! the accounts asks for them once and passes what it got down, and this is
//! what says so — nothing else about the answers would change if it went back
//! four more times.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

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
        supervised: None,
        port: 8787,
        policy: Arc::new(proxenos::policy::Policy::new(
            proxenos::policy::Snapshot::new(
                // A real mapping, because an empty one short-circuits the
                // question the accounts are listed to answer.
                vec![proxenos::config::ResolvedTier {
                    defaulted: false,
                    missing: None,
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
        refusals: std::sync::Arc::new(proxenos::auth::refusals::Refusals::default()),
        config: Arc::new(proxenos::config::Config::default()),
        shutdown: Arc::new(proxenos::daemon::Shutdown::default()),
        tokens: None,
        usage_endpoint: String::new(),
        anthropic_usage_endpoint: String::new(),
        anthropic_profile_endpoint: String::new(),
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

// --- the rendered listing --------------------------------------------------
//
// `docs/api.md` §2 — a header table, one row per account. Every rule below is
// about a column: what belongs in it, what it says when the payload has
// nothing to put there, and which of several true things it says when more
// than one applies.

const DAY: u64 = 24 * 60 * 60;
const NOW: u64 = 1_800_000_000;

/// A path under the operator's own home, which the renderer abbreviates.
fn under_home(rest: &str) -> String {
    let home = std::env::var("HOME").expect("a home directory");
    format!("{home}/{rest}")
}

/// The four accounts a real store holds: a declared profile, a found one, a
/// borrowed profile whose grant lives in a keychain, and a key.
fn a_store() -> serde_json::Value {
    serde_json::json!({
        "discovered": false,
        "accounts": [
            {
                "name": "work-codex",
                "kind": "grant",
                "provider": "codex",
                "email": "husni@sayurbox.com",
                "source": under_home(".config/proxenos/profiles/work-codex/auth.json"),
                "declared": true,
                "selected": true,
            },
            {
                "name": "codex",
                "kind": "grant",
                "provider": "codex",
                "email": "husni.adil@gmail.com",
                "source": under_home(".codex/auth.json"),
                "selected": false,
            },
            {
                "name": "claude",
                "kind": "grant",
                "provider": "anthropic",
                "plan": "max",
                "source": "keychain item `Claude Code-credentials`",
                "login_expires_at": NOW + 3 * DAY,
                "selected": false,
            },
            {
                "name": "openai-api",
                "kind": "key",
                "provider": "codex",
                "selected": false,
            },
        ],
    })
}

/// The cells of one row, by the name in its first column.
fn row(rendered: &str, name: &str) -> Vec<String> {
    cells(
        rendered
            .lines()
            .find(|line| {
                line.trim_start_matches(['*', ' '])
                    .split_whitespace()
                    .next()
                    == Some(name)
            })
            .unwrap_or_else(|| panic!("no row for {name}: {rendered}")),
    )
}

/// A row's cells. Columns are separated by two spaces and no cell contains a
/// run of two, so the split is the row's own structure.
fn cells(line: &str) -> Vec<String> {
    line.split("  ")
        .filter(|cell| !cell.is_empty())
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// A listing leads with a header, and the `*` is the only thing in front of a
/// name. A reader who has to work out what a column holds is reading a row at
/// a time.
#[test]
fn the_listing_leads_with_a_header_and_marks_the_serving_account() {
    let rendered = proxenos::render::accounts_at(&a_store(), NOW);
    let lines: Vec<&str> = rendered.lines().collect();

    assert_eq!(
        cells(lines[0]),
        vec!["NAME", "PROVIDER", "KIND", "ACCOUNT", "SOURCE", "STATE"],
        "{rendered}"
    );
    let serving: Vec<&&str> = lines.iter().filter(|line| line.starts_with('*')).collect();
    assert_eq!(serving.len(), 1, "{rendered}");
    assert!(serving[0].contains("work-codex"), "{rendered}");
}

/// What kind of thing the row is, in the operator's words rather than the
/// payload's: `kind` names the credential, and what an operator picked was a
/// profile or a key.
#[test]
fn the_kind_column_says_profile_for_a_grant_and_key_for_a_key() {
    let rendered = proxenos::render::accounts_at(&a_store(), NOW);

    assert_eq!(row(&rendered, "work-codex")[2], "profile", "{rendered}");
    assert_eq!(row(&rendered, "openai-api")[2], "key", "{rendered}");
}

/// The column that tells two accounts apart: the address, else the id, else
/// the subscription, else nothing to say. Never the word `key`, which says
/// what the row is and not who it belongs to — the kind column already has it.
#[test]
fn the_account_column_falls_back_from_an_address_to_a_plan_to_nothing() {
    let rendered = proxenos::render::accounts_at(&a_store(), NOW);

    assert_eq!(
        row(&rendered, "work-codex")[3],
        "husni@sayurbox.com",
        "{rendered}"
    );
    assert_eq!(row(&rendered, "claude")[3], "max plan", "{rendered}");
    assert_eq!(row(&rendered, "openai-api")[3], "-", "{rendered}");

    let by_id = proxenos::render::accounts_at(
        &serde_json::json!({ "accounts": [{
            "name": "thin", "kind": "grant", "provider": "codex",
            "account_id": "acct_one", "declared": true, "selected": true,
        }]}),
        NOW,
    );
    assert_eq!(row(&by_id, "thin")[3], "acct_one", "{by_id}");
}

/// Where the credential was read from, in a form that fits a terminal: the
/// operator's home abbreviated, a keychain named as a keychain rather than by
/// its item, a key as what it is — this daemon's own stored secret — and a
/// profile nobody wrote down marked as found.
#[test]
fn the_source_column_abbreviates_home_and_marks_a_found_profile() {
    let rendered = proxenos::render::accounts_at(&a_store(), NOW);

    assert_eq!(
        row(&rendered, "codex")[4],
        "~/.codex/auth.json (found)",
        "{rendered}"
    );
    assert_eq!(
        row(&rendered, "claude")[4],
        "keychain (found)",
        "{rendered}"
    );
    assert_eq!(row(&rendered, "openai-api")[4], "stored", "{rendered}");
    assert!(
        row(&rendered, "work-codex")[4].starts_with("~/.config/proxenos"),
        "{rendered}"
    );
    assert!(
        !row(&rendered, "work-codex")[4].contains("(found)"),
        "a declared profile is not a found one: {rendered}"
    );
}

/// A path can be arbitrarily long and the row it sits on cannot. Only this
/// column is ever cut, and it says so with an ellipsis rather than stopping.
#[test]
fn only_the_source_column_is_ever_cut() {
    let long = under_home(&format!(
        "{}/auth.json",
        "profiles/somewhere-deep".repeat(6)
    ));
    let rendered = proxenos::render::accounts_at(
        &serde_json::json!({ "accounts": [{
            "name": "deep", "kind": "grant", "provider": "codex",
            "email": "someone@example.test", "source": long,
            "declared": true, "selected": true,
        }]}),
        NOW,
    );

    let cells = row(&rendered, "deep");
    assert_eq!(cells[4].chars().count(), 40, "{rendered}");
    assert!(cells[4].ends_with('…'), "{rendered}");
    assert_eq!(cells[3], "someone@example.test", "{rendered}");
}

/// One phrase, and the most urgent true one. A credential the backend has
/// already turned away is past being renewed early, and an account that is
/// not the one it was chosen as is billing somebody else in the meantime.
#[test]
fn the_state_column_says_the_most_urgent_true_thing() {
    let state = |extra: serde_json::Value| {
        let mut account = serde_json::json!({
            "name": "work", "kind": "grant", "provider": "codex",
            "email": "someone@example.test", "declared": true, "selected": true,
        });
        for (key, value) in extra.as_object().expect("an object") {
            account[key] = value.clone();
        }
        let rendered =
            proxenos::render::accounts_at(&serde_json::json!({ "accounts": [account] }), NOW);
        row(&rendered, "work")[5].clone()
    };

    assert_eq!(state(serde_json::json!({})), "ok");
    assert_eq!(
        state(serde_json::json!({ "login_expires_at": NOW + 3 * DAY })),
        "login expires in 3 days"
    );
    assert_eq!(
        state(serde_json::json!({ "identity_changed": true })),
        "identity changed"
    );
    assert_eq!(
        state(serde_json::json!({
            "identity_changed": true,
            "login_expires_at": NOW + 3 * DAY,
            "refused": { "status": 401, "detail": "invalid access token" },
        })),
        "refused"
    );
    // Far off is silence: a row carrying a date eleven months of the year is
    // one the reader learns to skip.
    assert_eq!(
        state(serde_json::json!({ "login_expires_at": NOW + 30 * DAY })),
        "ok"
    );
}

/// A row whose store could not be read says so, and says why under the table.
///
/// It used to say `ok`, which is the one word certainly untrue of it: every
/// field the other states are computed from is absent on such a row, so
/// nothing else in the listing contradicted it.
#[test]
fn a_row_whose_grant_could_not_be_read_says_so_and_says_why() {
    let reason = "/profiles/empty/auth.json holds no grant. Sign in to that profile first, \
                  in the ChatGPT app or with `codex login`";
    let rendered = proxenos::render::accounts_at(
        &serde_json::json!({
            "accounts": [{
                "name": "empty",
                "kind": "grant",
                "provider": "codex",
                "source": "/profiles/empty/auth.json",
                "declared": true,
                "selected": false,
                "unreadable": reason,
            }],
        }),
        NOW,
    );

    assert_eq!(row(&rendered, "empty")[5], "unreadable", "{rendered}");
    assert!(
        rendered.contains(reason),
        "the reason is nowhere in the listing: {rendered}"
    );
    assert!(
        rendered.contains("empty holds no readable grant"),
        "the note does not say which row it is about: {rendered}"
    );
}

/// A readable store says nothing about readability, on the row or under the
/// table.
#[test]
fn a_readable_row_carries_no_unreadable_note() {
    let rendered = proxenos::render::accounts_at(&a_store(), NOW);

    assert!(!rendered.contains("holds no readable grant"), "{rendered}");
    assert_eq!(row(&rendered, "work-codex")[5], "ok", "{rendered}");
}

/// The notes under the table survive the table. The one saying these were
/// found is about the whole set, so it is said only where the whole set is
/// found — a store holding one declared profile is not describing itself.
#[test]
fn the_notes_under_the_table_are_kept() {
    let all_found = proxenos::render::accounts_at(
        &serde_json::json!({
            "discovered": true,
            "ignored_grants": ["leftover"],
            "accounts": [{
                "name": "claude", "kind": "grant", "provider": "anthropic",
                "plan": "max", "source": "keychain item `Claude Code-credentials`",
                "selected": true,
            }],
        }),
        NOW,
    );
    assert!(all_found.contains("found, not declared"), "{all_found}");
    assert!(
        all_found.contains("leftover in credentials.json"),
        "{all_found}"
    );

    assert!(
        !proxenos::render::accounts_at(&a_store(), NOW).contains("found, not declared"),
        "one declared profile is enough to make the note untrue"
    );
}

// --- what an account verb says it did --------------------------------------
//
// One pattern for all of them: the first line is what happened, and the
// second — where there is one — is the consequence, or the next thing to
// type. Nothing is stated only as a clause hanging off a semicolon, because
// the half an operator has to act on is the half that has to be its own line.

/// A switch names who serves now, and what moved with it. Within one provider
/// it changed whose quota is spent; across providers it changed which backend
/// answers, and both facts are consequences rather than asides.
#[test]
fn a_switch_says_who_serves_and_how_far_it_moved() {
    assert_eq!(
        proxenos::render::selected_account(&serde_json::json!({
            "selected": "spare", "provider": "codex", "previous_provider": "codex",
        })),
        "serving turns as spare on codex\nstill on codex, so the same backend answers and \
         another account's quota is spent"
    );
    assert_eq!(
        proxenos::render::selected_account(&serde_json::json!({
            "selected": "relay", "provider": "anthropic", "previous_provider": "codex",
        })),
        "serving turns as relay on anthropic\ncodex to anthropic, so a different backend on a \
         different subscription answers every turn"
    );
    // The first account stored had nothing serving before it, and a second
    // line claiming a provider changed would invent the half it cannot know.
    assert_eq!(
        proxenos::render::selected_account(&serde_json::json!({
            "selected": "main", "provider": "codex",
        })),
        "serving turns as main on codex"
    );
    assert_eq!(
        proxenos::render::selected_account(&serde_json::json!({ "selected": "main" })),
        "serving turns as main"
    );
    assert_eq!(
        proxenos::render::selected_account(&serde_json::json!({})),
        "no account selected"
    );
}

/// A rename says the new name first, because that is the string every other
/// verb now takes, and says the file moved on a line of its own — an operator
/// who wrote that section by hand is the reader of the second line.
#[test]
fn a_rename_says_the_new_name_and_what_followed_it() {
    assert_eq!(
        proxenos::render::renamed_account(&serde_json::json!({
            "renamed": "acct_one", "name": "work", "moved_configuration": true,
        })),
        "acct_one is now work\nits section in config.toml moved with it"
    );
    assert_eq!(
        proxenos::render::renamed_account(&serde_json::json!({
            "renamed": "acct_one", "name": "work",
        })),
        "acct_one is now work"
    );
    assert_eq!(
        proxenos::render::renamed_account(&serde_json::json!({ "renamed": "acct_one" })),
        "renamed acct_one"
    );
}

/// A removal says what went, then who is serving turns now — or, where
/// nothing is, which of the two things to do about it.
#[test]
fn a_removal_says_what_went_and_who_serves_now() {
    assert_eq!(
        proxenos::render::removed_account(&serde_json::json!({
            "removed": "codex", "serving": "work-codex", "remaining": 2,
        })),
        "removed codex\nserving turns as work-codex"
    );
    assert_eq!(
        proxenos::render::removed_account(&serde_json::json!({
            "removed": "codex", "serving": serde_json::Value::Null, "remaining": 2,
        })),
        "removed codex\nno account is serving turns — choose one with `proxenos accounts use NAME`"
    );
    assert_eq!(
        proxenos::render::removed_account(&serde_json::json!({
            "removed": "codex", "serving": serde_json::Value::Null, "remaining": 0,
        })),
        "removed codex\nno accounts left — declare a profile under `[profiles]`, or store a key \
         with `proxenos accounts add-key NAME --provider codex|anthropic`"
    );
}

/// A reload already reads this way and keeps doing so: what it applied, then
/// the keys a running daemon cannot move. The second line is never omitted —
/// one that appeared only sometimes would read as "nothing was left out this
/// time".
#[test]
fn a_reload_says_what_applied_and_what_is_still_waiting() {
    assert_eq!(
        proxenos::render::reloaded_config(&serde_json::json!({
            "reloaded": ["profiles", "tiers"], "needs_restart": ["port"],
        })),
        "reloaded config.toml: profiles, tiers\nstill needs a restart: port"
    );
    assert_eq!(
        proxenos::render::reloaded_config(&serde_json::json!({
            "reloaded": [], "needs_restart": [],
        })),
        "reloaded config.toml; nothing in it was a setting this daemon can change"
    );
}

// --- the rendered status ---------------------------------------------------
//
// `docs/api.md` §2 — the report a running daemon answers `status` with. The
// rules below are about the three parts an operator acts on: who is paying,
// what is serving, and where each tier goes.

/// A daemon serving turns, mapping four tiers, one of them pinned elsewhere.
fn a_daemon() -> serde_json::Value {
    serde_json::json!({
        "base_url": "http://127.0.0.1:8787",
        "auth": {
            "connected": true,
            "account": "work-codex",
            "email": "husni@sayurbox.com",
            "kind": "grant",
            "provider": "codex",
        },
        "tiers": {
            "opus": "gpt-5.6-terra",
            "sonnet": "gpt-5.6-terra",
            "haiku": "gpt-5.6-luna",
            "fable": { "model": "gpt-5.5", "account": "personal-codex" },
        },
        "version": proxenos::version::build(),
        "pid": 4711,
        "supervised": true,
        "client": { "deny_skills": [] },
    })
}

/// The line naming the account holds the name every account verb takes.
///
/// It used to say `connected`, which is the one thing already established by
/// there being a line at all — and left the operator to work out which of the
/// names in `accounts` this was. The address, the kind and the provider follow
/// it, since those tell two accounts of one operator's apart.
#[test]
fn the_auth_line_leads_with_the_name_accounts_lists() {
    let rendered = proxenos::render::status_at(&a_daemon(), NOW);
    assert!(
        rendered.contains("auth       work-codex (husni@sayurbox.com, codex)"),
        "{rendered}"
    );

    // A key says so, because it is spent against a different endpoint and
    // cannot be asked for a quota.
    let key = proxenos::render::status_at(
        &serde_json::json!({
            "auth": {
                "connected": true,
                "account": "openai-api",
                "kind": "key",
                "provider": "codex",
            },
        }),
        NOW,
    );
    assert!(key.contains("auth       openai-api (key, codex)"), "{key}");
}

/// A daemon that does not name the account renders what it always did.
///
/// That is a daemon older than the field, not a bug to paper over: leading
/// with a name that is not there would mean inventing one.
#[test]
fn the_auth_line_falls_back_to_connected_where_there_is_no_name() {
    let rendered = proxenos::render::status_at(
        &serde_json::json!({
            "auth": { "connected": true, "email": "someone@example.com", "provider": "codex" },
        }),
        NOW,
    );
    assert!(
        rendered.contains("auth       connected (someone@example.com, codex)"),
        "{rendered}"
    );
}

/// What is serving the socket, named once: the build, the process, and whether
/// anything brings it back.
#[test]
fn the_daemon_line_names_the_build_the_process_and_its_supervisor() {
    let rendered = proxenos::render::status_at(&a_daemon(), NOW);
    assert!(
        rendered.contains(&format!(
            "daemon     {} (pid 4711), supervised",
            proxenos::version::build()
        )),
        "{rendered}"
    );

    let unsupervised = proxenos::render::status_at(
        &serde_json::json!({ "version": "0.12.0", "pid": 12, "supervised": false }),
        NOW,
    );
    assert!(
        unsupervised.contains("daemon     0.12.0 (pid 12), not supervised"),
        "{unsupervised}"
    );

    // Silence where the daemon cannot tell — a platform with no supervisor
    // here, or a process launchd started under some other label. "not
    // supervised" would be a claim nothing established.
    let unknowable = proxenos::render::status_at(
        &serde_json::json!({ "version": "0.12.0", "pid": 12, "supervised": null }),
        NOW,
    );
    assert!(
        unknowable.contains("daemon     0.12.0 (pid 12)") && !unknowable.contains("supervised"),
        "{unknowable}"
    );
}

/// The tier mapping is a table under a header, and a pinned tier says where it
/// spends in a column rather than in a parenthesis trailing off the model.
#[test]
fn the_tier_rows_lead_with_a_header_and_carry_state_in_a_column() {
    let rendered = proxenos::render::status_at(&a_daemon(), NOW);
    let header = rendered
        .lines()
        .find(|line| line.starts_with("TIER"))
        .unwrap_or_else(|| panic!("no tier header: {rendered}"));
    assert_eq!(cells(header), vec!["TIER", "MODEL", "STATE"], "{rendered}");

    assert_eq!(
        row(&rendered, "fable"),
        vec!["fable", "gpt-5.5", "as personal-codex"],
        "{rendered}"
    );
    assert_eq!(
        row(&rendered, "opus"),
        vec!["opus", "gpt-5.6-terra"],
        "{rendered}"
    );
}

/// The four rows are the ladder, in the order the ladder is spoken in.
///
/// The payload is an unordered object and arrives sorted by name, so without
/// this the report printed `fable, haiku, opus, sonnet` — which is only how
/// they sort, and is not the order the model list prints the same four in.
#[test]
fn the_tier_rows_are_in_ladder_order() {
    let rendered = proxenos::render::status_at(&a_daemon(), NOW);
    let tiers: Vec<String> = rendered
        .lines()
        .skip_while(|line| !line.starts_with("TIER"))
        .skip(1)
        .take(4)
        .filter_map(|line| cells(line).first().map(|cell| (*cell).to_owned()))
        .collect();
    assert_eq!(
        tiers,
        vec!["opus", "sonnet", "haiku", "fable"],
        "{rendered}"
    );
}

/// An ordinary mapping has no state to report, and gets no column for one.
///
/// A header over four blank cells is a column the reader looks at and learns
/// nothing from.
#[test]
fn the_state_column_appears_only_where_a_tier_has_a_state() {
    let rendered = proxenos::render::status_at(
        &serde_json::json!({
            "auth": { "connected": true, "account": "work-codex", "provider": "codex" },
            "tiers": { "opus": "gpt-5.6-terra", "haiku": "gpt-5.6-luna" },
        }),
        NOW,
    );
    let header = rendered
        .lines()
        .find(|line| line.starts_with("TIER"))
        .unwrap_or_else(|| panic!("no tier header: {rendered}"));
    assert_eq!(cells(header), vec!["TIER", "MODEL"], "{rendered}");
    assert!(
        rendered.lines().all(|line| line == line.trim_end()),
        "no row carries trailing space: {rendered:?}"
    );

    // A relaying account has one on every unpinned row instead.
    let relaying = proxenos::render::status_at(
        &serde_json::json!({
            "auth": { "connected": true, "account": "personal-claude", "provider": "anthropic" },
            "tiers": { "opus": "claude-opus-5" },
        }),
        NOW,
    );
    assert_eq!(
        row(&relaying, "opus"),
        vec!["opus", "claude-opus-5", "inert while relaying"],
        "{relaying}"
    );
}

/// The model list is a table, and says which tiers point at each id.
///
/// A list of ids and windows left the one question an operator actually has —
/// which of these does a turn reach — answerable only by reading `status`
/// beside it. The mapping is read from the `tiers` method, which already
/// carries it.
#[test]
fn the_model_list_names_the_tiers_that_map_to_each_id() {
    let rendered = proxenos::render::models(
        &serde_json::json!({
            "models": [
                { "id": "gpt-5.6-terra", "context_window": 272_000 },
                { "id": "gpt-5.6-luna", "context_window": 272_000 },
                { "id": "gpt-5.5" },
            ],
            "authoritative": true,
        }),
        Some(&serde_json::json!({
            "tiers": {
                "opus": "gpt-5.6-terra",
                "sonnet": "gpt-5.6-terra",
                "haiku": "gpt-5.6-terra",
                "fable": { "model": "gpt-5.6-luna", "account": "personal-codex" },
            },
        })),
    );

    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(
        cells(lines[0]),
        vec!["MODEL", "WINDOW", "TIER"],
        "{rendered}"
    );

    // The ladder's order, not the mapping's — which arrives sorted by name.
    assert_eq!(
        row(&rendered, "gpt-5.6-terra"),
        vec!["gpt-5.6-terra", "272000 tokens", "opus, sonnet, haiku"],
        "{rendered}"
    );
    // A pinned tier points at its model like any other.
    assert_eq!(
        row(&rendered, "gpt-5.6-luna"),
        vec!["gpt-5.6-luna", "272000 tokens", "fable"],
        "{rendered}"
    );
    // A catalog that states no window says so rather than printing a figure
    // nobody measured, and a model no tier names has no tier cell.
    assert_eq!(
        row(&rendered, "gpt-5.5"),
        vec!["gpt-5.5", "window unknown"],
        "{rendered}"
    );
}

/// A mapping that could not be read costs the column, not the table.
///
/// An empty cell there would read as "no tier maps to this model", which is a
/// different statement from "this side does not know".
#[test]
fn the_tier_column_is_absent_where_the_mapping_could_not_be_read() {
    let rendered = proxenos::render::models(
        &serde_json::json!({
            "models": [{ "id": "gpt-5.6-terra", "context_window": 272_000 }],
            "authoritative": true,
        }),
        None,
    );

    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(cells(lines[0]), vec!["MODEL", "WINDOW"], "{rendered}");
    assert_eq!(
        row(&rendered, "gpt-5.6-terra"),
        vec!["gpt-5.6-terra", "272000 tokens"],
        "{rendered}"
    );
}

// ---------------------------------------------------------------------------
// §7.1 — a tier whose stated model the catalog no longer carries. The daemon
// is up, the other tiers serve, and this is the only place an operator can see
// which one is not.
// ---------------------------------------------------------------------------

/// The marked tier says so on its own row, and the row keeps the stated id.
///
/// The state column is where the eye is already looking, and a model that
/// silently vanished from the mapping would hide the operator's own decision.
#[test]
fn a_missing_tier_is_marked_on_its_row_and_keeps_its_model() {
    let rendered = proxenos::render::status_at(
        &serde_json::json!({
            "auth": { "connected": true, "account": "work-codex", "provider": "codex" },
            "tiers": { "opus": "gpt-5.6-terra", "fable": "gpt-5.6-sol" },
            "missing_tiers": ["fable"],
        }),
        NOW,
    );

    assert_eq!(
        row(&rendered, "fable"),
        vec!["fable", "gpt-5.6-sol", "missing from the catalog"],
        "{rendered}"
    );
    assert_eq!(
        row(&rendered, "opus"),
        vec!["opus", "gpt-5.6-terra"],
        "a tier that resolves is unmarked: {rendered}"
    );
    assert!(
        rendered.contains("fable cannot serve"),
        "and the consequence is said once beneath the table: {rendered}"
    );
    assert!(
        rendered.contains("proxenos reload"),
        "with the way out: {rendered}"
    );
}

/// Every tier missing is said as such rather than as a list of four. A daemon
/// answering on its port while refusing every turn is a different situation
/// from one tier being down, and reads as one.
#[test]
fn every_tier_missing_says_no_tier_can_serve() {
    let rendered = proxenos::render::status_at(
        &serde_json::json!({
            "auth": { "connected": true, "account": "work-codex", "provider": "codex" },
            "tiers": { "opus": "gone-a", "fable": "gone-b" },
            "missing_tiers": ["opus", "fable"],
        }),
        NOW,
    );

    assert!(rendered.contains("no tier can serve"), "{rendered}");
}

/// A mapping with nothing missing gains no line and no column.
#[test]
fn a_mapping_with_nothing_missing_says_nothing_about_it() {
    let rendered = proxenos::render::status_at(
        &serde_json::json!({
            "auth": { "connected": true, "account": "work-codex", "provider": "codex" },
            "tiers": { "opus": "gpt-5.6-terra" },
            "missing_tiers": [],
        }),
        NOW,
    );

    assert!(!rendered.contains("missing from the catalog"), "{rendered}");
    assert!(!rendered.contains("cannot serve"), "{rendered}");
}

/// The model list names the missing tier too. Its model has no row here at all
/// — that is what being absent from the catalog means — so an operator looking
/// for it would otherwise find silence.
#[test]
fn the_model_list_names_a_tier_whose_model_is_not_in_the_catalog() {
    let rendered = proxenos::render::models(
        &serde_json::json!({
            "models": [{ "id": "gpt-5.6-terra", "context_window": 272_000 }],
        }),
        Some(&serde_json::json!({
            "tiers": { "opus": "gpt-5.6-terra", "fable": "gpt-5.6-sol" },
            "missing_tiers": ["fable"],
        })),
    );

    assert!(rendered.contains("fable → gpt-5.6-sol"), "{rendered}");
    assert!(rendered.contains("not in this catalog"), "{rendered}");
}
