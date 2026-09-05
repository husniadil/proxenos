//! `docs/api.md` §2 — `tiers` and `effort`, the two settings a running daemon
//! can be handed, reached from a terminal.
//!
//! These drive the shipping binary rather than the socket, for the reason
//! `usage_cli.rs` gives: `tiers.set` and `effort.set` were answered by the
//! daemon for seventeen minor versions with no verb that reached them, and a
//! socket-level test cannot see that gap. What is asserted is exactly what
//! the verb sends — a caller of this CLI has to be able to trust that a
//! choice left out is a parameter left out.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::sync::Mutex;

/// A stand-in daemon that records each request whole and answers the way the
/// real handlers do.
fn stand_in(
    socket: &std::path::Path,
    connections: usize,
) -> (
    Arc<Mutex<Vec<serde_json::Value>>>,
    std::thread::JoinHandle<()>,
) {
    let listener = UnixListener::bind(socket).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);

    let handle = std::thread::spawn(move || {
        for stream in listener.incoming().take(connections) {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            let request: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
            let method = request
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            recorded.lock().unwrap().push(request.clone());

            let result = match method.as_str() {
                "tiers" => serde_json::json!({
                    "tiers": {
                        "fable": "gpt-5.6-sol",
                        "haiku": { "account": "spare", "model": "gpt-5.6-luna" },
                        "opus": "gpt-5.6-luna",
                    },
                    "missing_tiers": ["opus"],
                    "cross_account_tiers": true,
                }),
                "cross_account_tiers.set" => serde_json::json!({
                    "cross_account_tiers": request["params"]["enabled"],
                    "persisted": true,
                }),
                "tiers.set" => serde_json::json!({
                    "tiers": {
                        "fable": "gpt-5.6-sol",
                        "opus": request["params"]["tiers"]["opus"].clone(),
                        "haiku": request["params"]["tiers"]["haiku"].clone(),
                    },
                    "persisted": request["params"]["persist"].as_bool().unwrap_or(false),
                    "account": request["params"]["account"],
                    "detail": "in effect until the daemon stops; the configuration file is unchanged",
                }),
                "status" => serde_json::json!({ "version": "test", "effort_ceiling": "low" }),
                "effort.set" => serde_json::json!({
                    "effort": request["params"]["effort"],
                    "persisted": false,
                    "account": null,
                    "detail": "in effect until the daemon stops; the configuration file is unchanged",
                }),
                _ => serde_json::json!({}),
            };
            let _ = writeln!(
                stream,
                "{}",
                serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": result })
            );
            let _ = stream.flush();
        }
    });

    (requests, handle)
}

fn run(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(args)
        .env("PROXENOS_HOME", dir)
        .env("TMPDIR", dir)
        .output()
        .expect("the binary should run")
}

/// A bare `tiers` reads the mapping and marks the tier the catalog lacks.
#[test]
fn tiers_reads_the_mapping_and_marks_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let (requests, server) = stand_in(&dir.path().join("proxenos.sock"), 1);

    let output = run(dir.path(), &["tiers"]);
    server.join().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "tiers failed: {stdout}");
    assert_eq!(requests.lock().unwrap()[0]["method"], "tiers");
    assert!(stdout.contains("TIER    MODEL         STATE"), "{stdout}");
    assert!(stdout.contains("fable   gpt-5.6-sol"), "{stdout}");
    assert!(
        stdout.contains("haiku   gpt-5.6-luna  as spare"),
        "{stdout}"
    );
    assert!(
        stdout.contains("opus    gpt-5.6-luna  not in this account's catalog"),
        "{stdout}"
    );
    assert!(stdout.contains("cross-account tiers: allowed"), "{stdout}");
}

/// A pin is the table form the file takes, and consent given in the same
/// breath goes first — written before the pin that needs it is asked for.
#[test]
fn a_pin_is_the_table_form_and_consent_goes_first() {
    let dir = tempfile::tempdir().unwrap();
    let (requests, server) = stand_in(&dir.path().join("proxenos.sock"), 3);

    let pinned = run(
        dir.path(),
        &["tiers", "set", "haiku", "gpt-5.6-luna", "--as", "spare"],
    );
    let consented = run(
        dir.path(),
        &[
            "tiers",
            "set",
            "haiku",
            "gpt-5.6-luna",
            "--as",
            "spare",
            "--allow-cross-account",
        ],
    );
    server.join().unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests[0]["method"], "tiers.set");
    assert_eq!(
        requests[0]["params"]["tiers"],
        serde_json::json!({ "haiku": { "account": "spare", "model": "gpt-5.6-luna" } })
    );
    assert_eq!(requests[1]["method"], "cross_account_tiers.set");
    assert_eq!(
        requests[1]["params"],
        serde_json::json!({ "enabled": true })
    );
    assert_eq!(requests[2]["method"], "tiers.set");

    let stdout = String::from_utf8_lossy(&pinned.stdout);
    assert!(stdout.contains("haiku → gpt-5.6-luna as spare"), "{stdout}");

    let stdout = String::from_utf8_lossy(&consented.stdout);
    assert!(
        stdout.contains("cross-account tiers allowed; written to config.toml"),
        "{stdout}"
    );
    assert!(stdout.contains("haiku → gpt-5.6-luna as spare"), "{stdout}");
}

/// Consent on its own is a sub-verb, `on` or `off`, and nothing else.
#[test]
fn cross_account_grants_and_revokes_by_word() {
    let dir = tempfile::tempdir().unwrap();
    let (requests, server) = stand_in(&dir.path().join("proxenos.sock"), 2);

    let on = run(dir.path(), &["tiers", "cross-account", "on"]);
    let off = run(dir.path(), &["tiers", "cross-account", "off", "--json"]);
    server.join().unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests[0]["method"], "cross_account_tiers.set");
    assert_eq!(
        requests[0]["params"],
        serde_json::json!({ "enabled": true })
    );
    assert_eq!(
        requests[1]["params"],
        serde_json::json!({ "enabled": false })
    );

    let stdout = String::from_utf8_lossy(&on.stdout);
    assert!(
        stdout.contains("allowed; written to config.toml"),
        "{stdout}"
    );
    let payload: serde_json::Value = serde_json::from_slice(&off.stdout).unwrap();
    assert_eq!(payload["cross_account_tiers"], false);
}

/// `tiers set` sends one tier, and only the flags that were given.
#[test]
fn tiers_set_sends_exactly_what_was_typed() {
    let dir = tempfile::tempdir().unwrap();
    let (requests, server) = stand_in(&dir.path().join("proxenos.sock"), 2);

    let plain = run(dir.path(), &["tiers", "set", "opus", "gpt-5.6-sol"]);
    let written = run(
        dir.path(),
        &[
            "tiers",
            "set",
            "opus",
            "gpt-5.6-sol",
            "--account",
            "spare",
            "--persist",
            "--json",
        ],
    );
    server.join().unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests[0]["method"], "tiers.set");
    assert_eq!(
        requests[0]["params"],
        serde_json::json!({ "tiers": { "opus": "gpt-5.6-sol" } }),
        "a choice left out must be a parameter left out"
    );
    assert_eq!(
        requests[1]["params"],
        serde_json::json!({ "tiers": { "opus": "gpt-5.6-sol" }, "account": "spare", "persist": true })
    );

    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert!(plain.status.success(), "{stdout}");
    assert!(stdout.contains("opus → gpt-5.6-sol"), "{stdout}");
    assert!(stdout.contains("until the daemon stops"), "{stdout}");

    let payload: serde_json::Value =
        serde_json::from_slice(&written.stdout).expect("--json prints the payload");
    assert_eq!(payload["persisted"], true);
    assert_eq!(payload["account"], "spare");
}

/// A bare `effort` reads the ceiling from `status`; `set none` sends null.
#[test]
fn effort_reads_from_status_and_none_sends_null() {
    let dir = tempfile::tempdir().unwrap();
    let (requests, server) = stand_in(&dir.path().join("proxenos.sock"), 3);

    let read = run(dir.path(), &["effort"]);
    let low = run(dir.path(), &["effort", "set", "high"]);
    let none = run(dir.path(), &["effort", "set", "none", "--json"]);
    server.join().unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests[0]["method"], "status");
    assert_eq!(requests[1]["method"], "effort.set");
    assert_eq!(
        requests[1]["params"],
        serde_json::json!({ "effort": "high" })
    );
    assert_eq!(requests[2]["params"], serde_json::json!({ "effort": null }));

    let stdout = String::from_utf8_lossy(&read.stdout);
    assert!(read.status.success(), "{stdout}");
    assert!(stdout.contains("effort ceiling: low"), "{stdout}");

    let stdout = String::from_utf8_lossy(&low.stdout);
    assert!(stdout.contains("effort ceiling: high"), "{stdout}");

    let payload: serde_json::Value = serde_json::from_slice(&none.stdout).unwrap();
    assert!(payload["effort"].is_null());
}
