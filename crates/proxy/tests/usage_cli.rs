//! `docs/api.md` §CLI — reaching the per-account figure from a terminal.
//!
//! These drive the shipping binary rather than the socket, because the gap
//! these cover is exactly the one a socket-level test cannot see: a method the
//! daemon answers and no argument reaches.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::sync::Mutex;

/// A stand-in daemon that records what it was asked for.
///
/// It answers `usage` the way a daemon with two accounts does: the spare
/// account carries a figure only once `usage.refresh` has been called, which is
/// what makes "the CLI can ask" observable from outside.
fn stand_in(
    socket: &std::path::Path,
    connections: usize,
) -> (Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let listener = UnixListener::bind(socket).unwrap();
    let methods = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&methods);

    let handle = std::thread::spawn(move || {
        let mut asked = false;
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
            recorded.lock().unwrap().push(method.clone());

            let result = match method.as_str() {
                "usage.refresh" => {
                    asked = true;
                    serde_json::json!({ "known": true, "windows": [] })
                }
                _ => usage_answer(asked),
            };
            let _ = writeln!(
                stream,
                "{}",
                serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": result })
            );
            let _ = stream.flush();
        }
    });

    (methods, handle)
}

/// The `usage` answer, before and after the spare account has been asked for.
fn usage_answer(asked: bool) -> serde_json::Value {
    let spare = if asked {
        serde_json::json!({
            "known": true,
            "account": "spare-codex",
            "provider": "codex",
            "serving": false,
            "source": "fetch",
            "windows": [{ "label": "30d", "used_percent": 3.0 }],
        })
    } else {
        serde_json::json!({
            "known": false,
            "account": "spare-codex",
            "provider": "codex",
            "serving": false,
            "detail": "no turn has been served as this account",
        })
    };

    serde_json::json!({
        "known": true,
        "plan": "pro",
        "windows": [{ "label": "5h", "used_percent": 11.0 }],
        "models": ["gpt-5"],
        "accounts": [
            {
                "known": true,
                "account": "serving-codex",
                "provider": "codex",
                "serving": true,
                "source": "stream",
                "windows": [{ "label": "5h", "used_percent": 11.0 }],
            },
            spare,
        ],
    })
}

fn run(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(args)
        .env("PROXENOS_HOME", dir)
        .env("TMPDIR", dir)
        .output()
        .expect("the binary should run")
}

/// One invocation fills in an account that never served.
#[test]
fn asking_fills_in_a_stored_account_that_never_served() {
    let dir = tempfile::tempdir().unwrap();
    let (methods, server) = stand_in(&dir.path().join("proxenos.sock"), 2);

    let output = run(dir.path(), &["usage", "--refresh"]);
    server.join().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "usage --refresh failed: {stdout}");
    assert_eq!(
        methods.lock().unwrap().as_slice(),
        ["usage.refresh", "usage"],
        "asking should ask, then read what asking stored"
    );
    assert!(
        stdout.contains("spare-codex") && stdout.contains("3%"),
        "the spare account's figure never reached the terminal:\n{stdout}"
    );
}

/// A bare `usage` asks for nothing. This fails if it ever does.
#[test]
fn a_bare_usage_asks_for_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (methods, server) = stand_in(&dir.path().join("proxenos.sock"), 1);

    let output = run(dir.path(), &["usage"]);
    server.join().unwrap();

    assert!(output.status.success());
    assert_eq!(
        methods.lock().unwrap().as_slice(),
        ["usage"],
        "a bare usage spent a request it was never asked to spend"
    );
}

/// `--json` emits the same document whether or not a figure was just asked for.
#[test]
fn json_keeps_its_shape_whether_or_not_a_figure_was_asked_for() {
    let keys = |args: &[&str], connections: usize| {
        let dir = tempfile::tempdir().unwrap();
        let (_, server) = stand_in(&dir.path().join("proxenos.sock"), connections);
        let output = run(dir.path(), args);
        server.join().unwrap();
        let parsed: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("--json should emit a document");
        let mut names: Vec<String> = parsed
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    };

    assert_eq!(
        keys(&["usage", "--json"], 1),
        keys(&["usage", "--json", "--refresh"], 2),
        "--refresh changed the shape a status line parses"
    );
}
