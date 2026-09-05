//! `docs/api.md` §2.6 — `supervisor status --json`, the one document a
//! front-end reads to offer install or uninstall.
//!
//! Drives the shipping binary, since the JSON is assembled in the command
//! and nowhere else. `HOME` is a fresh directory, so nothing this user has
//! installed is read as the answer; what launchd says about the label is
//! whatever it says on the machine running the test, so those two fields
//! are asserted by shape and not by value.

#![cfg(target_os = "macos")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

#[test]
fn status_json_reports_an_absent_unit_with_its_paths() {
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(["supervisor", "status", "--json"])
        .env("HOME", dir.path())
        .env("PROXENOS_HOME", dir.path())
        .env("TMPDIR", dir.path())
        .output()
        .expect("the binary should run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "supervisor status --json failed: {stdout}"
    );
    let payload: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");

    assert_eq!(payload["installed"], "absent");
    let plist = payload["plist"].as_str().expect("plist is a path");
    assert!(
        plist.starts_with(dir.path().to_str().unwrap()),
        "the plist is looked for under this HOME, not the real one: {plist}"
    );
    for key in ["program", "log", "socket"] {
        assert!(payload[key].is_string(), "{key} is a path: {payload}");
    }
    assert!(
        payload["state"].is_string() || payload["state"].is_null(),
        "{payload}"
    );
    assert!(
        payload["pid"].is_u64() || payload["pid"].is_null(),
        "{payload}"
    );
}
