//! `docs/api.md` §2 — the account verbs, driven through the shipping binary.
//!
//! The assembly, not the parts. Features in this project have shipped inert
//! while every unit test passed, each time because nothing exercised the wiring
//! between a value handed in at startup and the thing meant to read it. These
//! start the real binary as a daemon and drive the real CLI against it, so a
//! selection that never reaches the store fails here rather than in someone's
//! session.
//!
//! Nothing here reaches the network. The catalog endpoint is pointed at a
//! loopback port with nothing on it, so the startup fetch fails immediately and
//! falls back; the stored grants carry expiries far enough out that no refresh
//! is due.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use pretty_assertions::assert_eq;
use serde_json::json;

/// An unsigned JWT carrying the given claims. Nothing here verifies one.
fn id_token(account: &str) -> String {
    use base64::Engine;
    let encode = |value: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value);
    let claims = json!({
        "email": format!("{account}@example.test"),
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account,
            "chatgpt_plan_type": "plus",
        },
    });
    format!(
        "{}.{}.{}",
        encode(br#"{"alg":"none"}"#),
        encode(claims.to_string().as_bytes()),
        encode(b"signature")
    )
}

/// An access token whose `exp` claim is where a borrowed grant's expiry is
/// read from. The file itself records none.
fn access_token(expires_at: u64) -> String {
    use base64::Engine;
    let encode = |value: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value);
    format!(
        "{}.{}.{}",
        encode(br#"{"alg":"none"}"#),
        encode(json!({ "exp": expires_at }).to_string().as_bytes()),
        encode(b"signature")
    )
}

fn grant(account: &str) -> serde_json::Value {
    json!({
        "access_token": format!("access-{account}"),
        "refresh_token": format!("refresh-{account}"),
        "id_token": id_token(account),
        "account_id": account,
        // Far enough out that nothing is due, so no refresh is attempted and
        // no authorization server is contacted.
        "expires_at": 4_000_000_000_u64,
    })
}

/// A daemon of the shipping binary, on a home of its own.
struct Daemon {
    dir: tempfile::TempDir,
    process: std::process::Child,
}

impl Daemon {
    fn start(credentials: &serde_json::Value) -> Self {
        Self::start_with_config(credentials, "")
    }

    /// The same daemon, with more written into its configuration file.
    ///
    /// A catalog endpoint with nothing behind it: the fetch fails at once and
    /// the daemon falls back, which is the documented behaviour and keeps this
    /// offline.
    fn start_with_config(credentials: &serde_json::Value, extra: &str) -> Self {
        Self::start_with_file(
            credentials,
            &format!("{extra}\n[upstream]\ncatalog = \"http://127.0.0.1:1/models\"\n"),
        )
    }

    /// The same daemon, on a configuration file written exactly as given —
    /// for the cases that need the catalog section to say something else.
    ///
    /// `credentials` is written the way an operator's machine holds it now: a
    /// grant goes into a profile directory of the program that owns it, and is
    /// declared under `[profiles]`; a key stays in this daemon's own store. The
    /// callers below still describe accounts in one place, and this is where
    /// that description is put where each half actually lives (§8.4).
    fn start_with_file(credentials: &serde_json::Value, config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let described = match credentials.get("accounts") {
            Some(serde_json::Value::Array(accounts)) => accounts.clone(),
            _ => vec![credentials.clone()],
        };

        let mut profiles = String::new();
        let mut keys = Vec::new();
        for account in &described {
            let name = account
                .get("name")
                .or_else(|| account.get("account_id"))
                .and_then(serde_json::Value::as_str)
                .expect("an account is named")
                .to_owned();

            if account.get("kind").and_then(serde_json::Value::as_str) == Some("key") {
                keys.push(json!({
                    "name": name,
                    "kind": "key",
                    "api_key": account["api_key"],
                    "provider": account.get("provider").cloned().unwrap_or(json!("codex")),
                }));
                continue;
            }

            let profile = home.join("profiles").join(&name);
            std::fs::create_dir_all(&profile).unwrap();
            std::fs::write(
                profile.join("auth.json"),
                serde_json::to_string_pretty(&json!({
                    "auth_mode": "chatgpt",
                    "OPENAI_API_KEY": null,
                    "last_refresh": "2026-08-23T08:00:44.123456Z",
                    "tokens": {
                        // The expiry a borrowed grant has is the one inside its
                        // access token, so the figure the caller asked for goes
                        // there rather than into a field of its own.
                        "access_token": access_token(
                            account["expires_at"].as_u64().unwrap_or(4_000_000_000),
                        ),
                        "refresh_token": account["refresh_token"],
                        "id_token": account["id_token"],
                        "account_id": account["account_id"],
                    },
                }))
                .unwrap(),
            )
            .unwrap();
            profiles.push_str(&format!(
                "\n[profiles.{name}]\nprovider = \"codex\"\npath = \"{}\"\n",
                profile.display()
            ));
        }

        if !keys.is_empty() {
            std::fs::write(
                home.join("credentials.json"),
                serde_json::to_string_pretty(&json!({ "accounts": keys })).unwrap(),
            )
            .unwrap();
        }
        if let Some(selected) = credentials
            .get("selected")
            .and_then(serde_json::Value::as_str)
        {
            std::fs::write(
                home.join("selected.json"),
                serde_json::to_string(&json!({ "selected": selected })).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(home.join("config.toml"), format!("{config}{profiles}")).unwrap();

        let process = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
            .args(["run", "--port", "0"])
            .env("PROXENOS_HOME", &home)
            .env("HOME", dir.path())
            .env("TMPDIR", dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the daemon should start");

        let socket = home.join("proxenos.sock");
        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(socket.exists(), "the daemon never answered its socket");

        Self { dir, process }
    }

    /// One CLI verb, through the socket this daemon is serving.
    fn run(&self, args: &[&str]) -> String {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
            .args(args)
            .env("PROXENOS_HOME", self.dir.path().join("home"))
            .env("HOME", self.dir.path())
            .env("TMPDIR", self.dir.path())
            .output()
            .expect("the binary should run");
        assert!(
            output.status.success(),
            "`{}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// §8.1 — a credential file written before the store held more than one
/// account is read as the one account it describes, by the shipping binary,
/// with no re-login.
#[test]
fn the_binary_reads_a_credential_file_from_before_accounts() {
    let daemon = Daemon::start(&grant("acct_legacy"));

    let listed = daemon.run(&["accounts"]);

    assert!(
        listed.contains("acct_legacy"),
        "the migrated account should be listed: {listed}"
    );
    assert!(
        listed.starts_with('*'),
        "the only account serves turns: {listed}"
    );
    let status = daemon.run(&["status"]);
    assert!(
        status.contains("acct_legacy@example.test"),
        "status should name the account it is connected as: {status}"
    );
}

/// §2 — `accounts` lists, `accounts use` switches, and the switch is what
/// the next turn would authenticate with.
#[test]
fn the_binary_lists_and_switches_accounts() {
    let daemon = Daemon::start(&json!({
        // The name, not the account id: `spare` is what this store calls the
        // account the backend knows as `acct_two`.
        "selected": "spare",
        "accounts": [
            { "name": "acct_one", "access_token": "access-acct_one",
              "refresh_token": "refresh-acct_one", "id_token": id_token("acct_one"),
              "account_id": "acct_one", "expires_at": 4_000_000_000_u64 },
            { "name": "spare", "access_token": "access-acct_two",
              "refresh_token": "refresh-acct_two", "id_token": id_token("acct_two"),
              "account_id": "acct_two", "expires_at": 4_000_000_000_u64 },
        ],
    }));

    let listed = daemon.run(&["accounts"]);
    let marked: Vec<&str> = listed
        .lines()
        .filter(|line| line.starts_with('*'))
        .collect();
    assert_eq!(
        marked.len(),
        1,
        "exactly one account serves turns: {listed}"
    );
    assert!(marked[0].contains("spare"), "{listed}");
    assert!(listed.contains("acct_one@example.test"), "{listed}");

    let switched = daemon.run(&["accounts", "use", "acct_one"]);
    assert!(switched.contains("acct_one"), "{switched}");

    // The store every request authenticates through, as the daemon now reads
    // it: the account named is the one `status` reports being connected as.
    let status = daemon.run(&["status"]);
    assert!(
        status.contains("acct_one@example.test"),
        "the switch did not reach what serves turns: {status}"
    );
    let listed = daemon.run(&["accounts"]);
    let marked: Vec<&str> = listed
        .lines()
        .filter(|line| line.starts_with('*'))
        .collect();
    assert!(marked[0].contains("acct_one"), "{listed}");

    // And a name nobody holds is refused rather than silently ignored.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(["accounts", "use", "nobody"])
        .env("PROXENOS_HOME", daemon.dir.path().join("home"))
        .env("TMPDIR", daemon.dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nobody"), "{stderr}");
    assert!(stderr.contains("acct_one"), "{stderr}");
}

/// An account states its provider, and everything that reports the account
/// says so — the roadmap's first rule for a second provider. The default is
/// the provider this project started with, so every credential file written
/// before the field existed reads unchanged, and the listing names it on every
/// row: with two providers stored, a row that leaves it out is a row the
/// operator has to guess about.
#[test]
fn an_account_states_its_provider_and_the_reports_name_it() {
    let daemon = Daemon::start(&json!({
        "selected": "work",
        "accounts": [
            { "name": "work", "access_token": "access-acct_one",
              "refresh_token": "refresh-acct_one", "id_token": id_token("acct_one"),
              "account_id": "acct_one", "expires_at": 4_000_000_000_u64 },
            { "name": "claude", "kind": "key", "api_key": "sk-test-not-a-real-key",
              "provider": "anthropic" },
        ],
    }));

    let listed = daemon.run(&["accounts"]);
    let line_of = |name: &str| {
        listed
            .lines()
            .find(|line| line.contains(name))
            .unwrap_or_else(|| panic!("{name} should be listed: {listed}"))
            .to_owned()
    };
    assert!(
        line_of("claude").contains("anthropic"),
        "the account of the other provider must say so: {listed}"
    );
    assert!(
        line_of("work").contains("codex"),
        "the default provider is named too, not left implicit: {listed}"
    );

    // The switch rewrites the credential file, so this also holds the field
    // through a round-trip: a write that dropped it would strand the account
    // back on the default provider silently.
    let _ = daemon.run(&["accounts", "use", "claude"]);
    let status = daemon.run(&["status"]);
    assert!(
        status.contains("anthropic"),
        "status reports the provider of the account serving turns: {status}"
    );
    let relisted = daemon.run(&["accounts"]);
    assert!(
        relisted
            .lines()
            .find(|line| line.contains("claude"))
            .is_some_and(|line| line.contains("anthropic")),
        "the provider must survive the file being rewritten: {relisted}"
    );
}

/// §8.4 — a key this daemon holds can be forgotten; a borrowed profile cannot.
///
/// The two are different kinds of thing. A key is ours, and forgetting it is
/// the whole of what that verb ever meant. A profile belongs to another
/// program, so the refusal points at the file that declares it rather than
/// removing something the operator did not mean to lose.
#[test]
fn the_binary_forgets_a_key_and_refuses_to_forget_a_profile() {
    let daemon = Daemon::start(&json!({
        "selected": "work",
        "accounts": [
            { "name": "work", "access_token": "access-acct_one",
              "refresh_token": "refresh-acct_one", "id_token": id_token("acct_one"),
              "account_id": "acct_one", "expires_at": 4_000_000_000_u64 },
            { "name": "billing", "kind": "key", "api_key": "sk-test-not-a-real-key",
              "provider": "anthropic" },
        ],
    }));

    let forgotten = daemon.run(&["accounts", "remove", "billing"]);
    assert!(forgotten.contains("billing"), "{forgotten}");

    let listed = daemon.run(&["accounts"]);
    assert!(!listed.contains("billing"), "{listed}");
    assert!(listed.contains("work"), "{listed}");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(["accounts", "remove", "work"])
        .env("PROXENOS_HOME", daemon.dir.path().join("home"))
        .env("HOME", daemon.dir.path())
        .env("TMPDIR", daemon.dir.path())
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "a profile cannot be forgotten here"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("work"), "{stderr}");
    assert!(stderr.contains("profiles"), "{stderr}");
    assert!(daemon.run(&["accounts"]).contains("work"));
}

/// §8.4 — renaming a borrowed profile is refused, and the refusal says where
/// the name actually lives.
///
/// A profile's name is the key it is declared under, so changing it is an edit
/// to a file the operator already has open. Accepting the verb here would
/// leave the daemon calling an account something the configuration does not.
#[test]
fn the_binary_refuses_to_rename_a_borrowed_profile() {
    let daemon = Daemon::start(&grant("acct_legacy"));

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(["accounts", "rename", "acct_legacy", "work"])
        .env("PROXENOS_HOME", daemon.dir.path().join("home"))
        .env("HOME", daemon.dir.path())
        .env("TMPDIR", daemon.dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "a profile cannot be renamed here");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("acct_legacy"), "{stderr}");
    assert!(stderr.contains("profiles"), "{stderr}");

    // And it is still there, under the name it was declared with.
    let listed = daemon.run(&["accounts"]);
    assert!(listed.contains("acct_legacy"), "{listed}");
}

/// §8 — a key is stored without a browser flow, and never through argv.
///
/// The secret arrives on stdin because non-negotiable #7 puts credentials out
/// of process arguments: an argument is visible to every process on the
/// machine and lands in shell history. The name is required, because a key
/// carries no id to be named by and the name is what `accounts use` takes.
#[test]
fn the_binary_stores_a_key_from_stdin_and_serves_turns_as_it() {
    use std::io::Write;

    let daemon = Daemon::start(&grant("acct_legacy"));
    let home = daemon.dir.path().join("home");

    let mut login = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(["accounts", "add-key", "billing", "--provider", "codex"])
        .env("PROXENOS_HOME", &home)
        .env("TMPDIR", daemon.dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the binary should run");
    login
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"sk-probe-9c21-not-in-argv\n")
        .unwrap();
    let stored = login.wait_with_output().unwrap();
    assert!(
        stored.status.success(),
        "{}",
        String::from_utf8_lossy(&stored.stderr)
    );
    assert!(
        String::from_utf8_lossy(&stored.stdout).contains("billing"),
        "it should say what it stored: {}",
        String::from_utf8_lossy(&stored.stdout)
    );

    // Both accounts are there, and the account that was already serving turns
    // is still the one serving them: a login stores a credential, and
    // `accounts use` is the verb that moves the selection.
    let listed = daemon.run(&["accounts"]);
    assert!(listed.contains("acct_legacy"), "{listed}");
    assert!(
        listed
            .lines()
            .any(|line| line.starts_with('*') && line.contains("acct_legacy")),
        "the login must not have taken over: {listed}"
    );
    assert!(
        !listed.contains("sk-probe"),
        "the key reached a listing: {listed}"
    );

    // Switching is switching accounts, nothing more.
    daemon.run(&["accounts", "use", "billing"]);
    assert!(
        daemon
            .run(&["accounts"])
            .lines()
            .any(|line| { line.starts_with('*') && line.contains("billing") })
    );
    assert!(daemon.run(&["status"]).contains("billing"));

    // And the secret is not in the file's listing of names either.
    let stored = std::fs::read_to_string(home.join("credentials.json")).unwrap();
    assert!(
        stored.contains("sk-probe-9c21-not-in-argv"),
        "it has to be stored somewhere"
    );
    assert!(stored.contains("\"kind\": \"key\""), "{stored}");
}

/// A login while another account serves leaves the running daemon alone, and
/// the switch that follows is what hands over.
///
/// The daemon reads the credential file on every request, so the account moves
/// either way. What does not move on its own is what a switch carries with it:
/// the conversations bound to the previous account keep the endpoint they
/// dialed, and after a change of kind that endpoint refuses every turn. So the
/// hand-over rides on the verb that actually chooses — not on storing a
/// credential, which is the other decision.
#[test]
fn a_cli_login_leaves_the_running_daemon_alone_and_the_switch_hands_over() {
    use std::io::Write;

    let daemon = Daemon::start(&grant("acct_legacy"));
    let home = daemon.dir.path().join("home");

    let mut login = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(["accounts", "add-key", "billing", "--provider", "codex"])
        .env("PROXENOS_HOME", &home)
        .env("TMPDIR", daemon.dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the binary should run");
    login
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"sk-handover-probe\n")
        .unwrap();
    let stored = login.wait_with_output().unwrap();
    let said = String::from_utf8_lossy(&stored.stdout);

    assert!(
        !said.contains("running daemon"),
        "a login that chose nothing must not claim a hand-over: {said}"
    );
    assert!(
        said.contains("accounts use billing"),
        "it should say how to switch: {said}"
    );
    assert!(
        daemon.run(&["status"]).contains("acct_legacy"),
        "the daemon must still serve the account it was serving"
    );

    daemon.run(&["accounts", "use", "billing"]);
    assert!(daemon.run(&["status"]).contains("billing"));
}

/// §2 — a live probe run that cannot authenticate says so, once.
///
/// It used to answer with the whole capability matrix, every row failed and
/// the header claiming the backend answered and was billed. Nothing had been
/// sent and nothing had been billed: the transport never got a token. A matrix
/// that reports seven capabilities as broken when the credential is what is
/// missing sends whoever reads it to the wrong place entirely — and it is the
/// same failure the probes exist to prevent, printed the other way round.
#[test]
fn a_live_probe_run_without_a_credential_refuses_rather_than_reporting_failures() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(["doctor", "--live"])
        .env("PROXENOS_HOME", &home)
        .env("TMPDIR", dir.path())
        .output()
        .expect("the binary should run");

    assert!(!output.status.success(), "it cannot have succeeded");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        said.contains("accounts add-key"),
        "it should say what is missing, and the verb that supplies it: {said}"
    );
    assert!(
        !said.contains("Capability matrix"),
        "nothing was probed, so nothing may be reported about the backend: {said}"
    );
    assert!(
        !said.contains("billed"),
        "nothing was sent, so nothing was billed: {said}"
    );
}

/// §4 — the mapping the daemon serves is the selected account's, through the
/// shipping binary.
///
/// A catalog is one account's menu, so a mapping written for one account can
/// name a model another is not offered. This is the wiring half of that: the
/// override is in the file, the account it names is the one selected, and what
/// `status` reports has to be the override rather than the shared table.
#[test]
fn the_binary_serves_the_selected_accounts_mapping() {
    let daemon = Daemon::start_with_config(
        &json!({
            "selected": "acct_two",
            "accounts": [
                { "name": "acct_one", "access_token": "access-acct_one",
                  "refresh_token": "refresh-acct_one", "id_token": id_token("acct_one"),
                  "account_id": "acct_one", "expires_at": 4_000_000_000_u64 },
                { "name": "acct_two", "access_token": "access-acct_two",
                  "refresh_token": "refresh-acct_two", "id_token": id_token("acct_two"),
                  "account_id": "acct_two", "expires_at": 4_000_000_000_u64 },
            ],
        }),
        "[tiers]\nopus = \"shared-opus\"\n\n[accounts.acct_two.tiers]\nopus = \"only-for-two\"\n",
    );

    let status = daemon.run(&["status"]);

    assert!(
        status.contains("only-for-two"),
        "the selected account's override should be what serves turns: {status}"
    );
    assert!(
        !status.contains("shared-opus"),
        "the shared mapping should have been replaced: {status}"
    );
}

/// §7.1 — a pinned tier names another account's menu, so the catalog fetched
/// for the serving account does not decide whether it is valid.
///
/// The daemon's start and `tiers.set` are two doors onto one rule, and only one
/// of them held it: `tiers.set` excluded a pinned entry and startup measured it
/// against the serving account's list anyway. So a mapping written through the
/// socket and then persisted refused the daemon at the next start — the exact
/// silent-until-restart failure the write-time check exists to prevent.
///
/// Driven against an authoritative catalog, because the fallback list validates
/// nothing and would pass this whatever the rule was.
#[tokio::test(flavor = "multi_thread")]
async fn a_pinned_tier_does_not_have_to_be_on_the_serving_accounts_catalog() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(|| async {
                r#"{"data":[{"id":"gpt-5.6-terra","context_window":272000}]}"#
            }),
        );
        let _ = axum::serve(listener, app).await;
    });

    let daemon = Daemon::start_with_file(
        &json!({
            "selected": "work",
            "accounts": [
                { "name": "work", "access_token": "access-acct_one",
                  "refresh_token": "refresh-acct_one", "id_token": id_token("acct_one"),
                  "account_id": "acct_one", "expires_at": 4_000_000_000_u64 },
                { "name": "spare", "access_token": "access-acct_two",
                  "refresh_token": "refresh-acct_two", "id_token": id_token("acct_two"),
                  "account_id": "acct_two", "expires_at": 4_000_000_000_u64 },
            ],
        }),
        &format!(
            "cross_account_tiers = true\n\
             [tiers]\n\
             opus = \"gpt-5.6-terra\"\n\
             sonnet = \"gpt-5.6-terra\"\n\
             fable = \"gpt-5.6-terra\"\n\
             haiku = {{ account = \"spare\", model = \"gpt-5.5\" }}\n\
             [upstream]\n\
             catalog = \"http://{addr}/models\"\n"
        ),
    );

    // The daemon answered at all, which is the assertion: `gpt-5.5` is not on
    // the list this catalog served, and it is not that list's to refuse.
    let status = daemon.run(&["status"]);
    assert!(
        status.contains("gpt-5.5"),
        "the pinned mapping should be in force: {status}"
    );
}

/// §4 — `accounts use` moves between two accounts on different plans, in
/// both directions, with the configuration file untouched throughout.
///
/// The measured complaint this exists for: one `[tiers]` table, two accounts
/// whose catalogs differ, and a switch refused for a model the target account
/// is not offered. `[accounts.<name>.tiers]` is what holds a mapping right for
/// both, and this drives the shipping binary through the whole round trip to
/// show no edit is needed between the switches.
///
/// The catalog stub answers on loopback and keys its answer on the account
/// header, so each account really is offered a model the other is not.
#[tokio::test(flavor = "multi_thread")]
async fn accounts_use_moves_between_accounts_whose_catalogs_differ() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                let account = headers
                    .get("chatgpt-account-id")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("unknown")
                    .to_owned();
                format!(r#"{{"data":[{{"id":"model-for-{account}","context_window":272000}}]}}"#)
            }),
        );
        let _ = axum::serve(listener, app).await;
    });

    let daemon = Daemon::start_with_file(
        &json!({
            "selected": "work",
            "accounts": [
                { "name": "work", "access_token": "access-acct_one",
                  "refresh_token": "refresh-acct_one", "id_token": id_token("acct_one"),
                  "account_id": "acct_one", "expires_at": 4_000_000_000_u64 },
                { "name": "personal", "access_token": "access-acct_two",
                  "refresh_token": "refresh-acct_two", "id_token": id_token("acct_two"),
                  "account_id": "acct_two", "expires_at": 4_000_000_000_u64 },
            ],
        }),
        &format!(
            "[tiers]\n\
             opus = \"model-for-acct_one\"\n\
             sonnet = \"model-for-acct_one\"\n\
             haiku = \"model-for-acct_one\"\n\
             fable = \"model-for-acct_one\"\n\
             [accounts.personal.tiers]\n\
             opus = \"model-for-acct_two\"\n\
             sonnet = \"model-for-acct_two\"\n\
             haiku = \"model-for-acct_two\"\n\
             fable = \"model-for-acct_two\"\n\
             [upstream]\n\
             catalog = \"http://{addr}/models\"\n"
        ),
    );

    let config = daemon.dir.path().join("home").join("config.toml");
    let written = std::fs::read_to_string(&config).unwrap();

    // `run` fails the test on a non-zero exit, so the switch being accepted at
    // all is the first half of this.
    let moved = daemon.run(&["accounts", "use", "personal"]);
    assert!(
        moved.contains("personal"),
        "the switch should name the account now serving: {moved}"
    );
    let status = daemon.run(&["status"]);
    assert!(
        status.contains("model-for-acct_two") && !status.contains("model-for-acct_one"),
        "the account switched to should be served its own mapping: {status}"
    );

    daemon.run(&["accounts", "use", "work"]);
    let status = daemon.run(&["status"]);
    assert!(
        status.contains("model-for-acct_one") && !status.contains("model-for-acct_two"),
        "switching back should be served the shared mapping again: {status}"
    );

    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        written,
        "neither switch may need the configuration file changed"
    );
}

/// §8.4 — a grant left in this daemon's own store is not read any more, and
/// not listed as an account either.
///
/// It cannot be spent and cannot be refreshed: nothing here obtains one. But
/// skipping it silently reads as a credential that vanished, so the listing
/// says where a subscription comes from now.
#[test]
fn a_grant_left_in_the_key_store_is_reported_rather_than_offered() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // A credentials.json as a version that obtained its own grants wrote it.
    std::fs::write(
        home.join("credentials.json"),
        serde_json::to_string_pretty(&json!({
            "selected": "acct_old",
            "accounts": [{
                "name": "acct_old",
                "access_token": "access-acct_old",
                "refresh_token": "refresh-acct_old",
                "id_token": id_token("acct_old"),
                "account_id": "acct_old",
                "expires_at": 4_000_000_000_u64,
            }],
        }))
        .unwrap(),
    )
    .unwrap();

    let profile = home.join("profiles").join("work");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(
        profile.join("auth.json"),
        serde_json::to_string_pretty(&json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "last_refresh": "2026-08-23T08:00:44.123456Z",
            "tokens": {
                "access_token": access_token(4_000_000_000),
                "refresh_token": "rt.1.borrowed",
                "id_token": id_token("acct_new"),
                "account_id": "acct_new",
            },
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[upstream]\ncatalog = \"http://127.0.0.1:1/models\"\n\n\
             [profiles.work]\nprovider = \"codex\"\npath = \"{}\"\n",
            profile.display()
        ),
    )
    .unwrap();

    let process = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(["run", "--port", "0"])
        .env("PROXENOS_HOME", &home)
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the daemon should start");
    let socket = home.join("proxenos.sock");
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(socket.exists(), "the daemon never answered its socket");
    let daemon = Daemon { dir, process };

    let listed = daemon.run(&["accounts"]);

    assert!(
        listed.contains("work"),
        "the borrowed profile serves: {listed}"
    );
    assert!(
        !listed
            .lines()
            .any(|line| line.starts_with("* acct_old") || line.starts_with("  acct_old")),
        "the stored grant is not an account any more: {listed}"
    );
    assert!(listed.contains("acct_old"), "but it is named: {listed}");
    assert!(
        listed.contains("no longer"),
        "and said to be unread: {listed}"
    );
    assert!(
        listed.contains("[profiles]"),
        "with where one comes from: {listed}"
    );
}
