//! `docs/api.md` §2.6 and §3 on Linux — install, what the manager then says,
//! and what the supervised daemon reports about itself.
//!
//! The one test here that touches a machine, and the only one in the suite
//! that does: everything else about the unit is a pure function over data.
//! What cannot be settled that way is whether systemd accepts the file, starts
//! it, and hands the daemon an `INVOCATION_ID` its `MainPID` reading then
//! confirms — three claims about another program's behaviour, and the project
//! rule against tests that need a live backend is about the *network*, not
//! about the machine the suite runs on.
//!
//! **It refuses rather than improvises.** The unit name is fixed —
//! `proxenos.service`, in the manager's own search path, since a user manager
//! reads the `XDG_CONFIG_HOME` it was started with and not the one this
//! process holds — so there is no throwaway name to install under. Where a
//! real unit is already at that path, or a daemon already holds the port, or
//! there is no user manager to ask, this prints why and passes without
//! touching anything. Everything it does install, it removes on every path,
//! including a panic.

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

/// The default port a daemon binds, which the supervised job will want.
const PORT: u16 = 8787;

fn systemctl(arguments: &[&str]) -> std::io::Result<Output> {
    Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .output()
}

/// Where this user's manager reads units from, as systemd resolves it.
///
/// `XDG_CONFIG_HOME` where it names one, `~/.config` otherwise — the same
/// resolution `proxenos::config::xdg_config_home` performs, spelled out here
/// because the point of reading it is to check the *real* location before
/// deciding whether this test may run at all.
fn unit_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(base) if !base.is_empty() => PathBuf::from(base),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(
        base.join("systemd/user")
            .join(proxenos::supervisor::SERVICE),
    )
}

/// Why this test is not running, or `None` where it may.
fn refusal() -> Option<String> {
    let Some(path) = unit_path() else {
        return Some("neither XDG_CONFIG_HOME nor HOME names a directory".to_owned());
    };
    match systemctl(&["show", "-p", "Version"]) {
        Err(error) => {
            return Some(format!("`systemctl --user` could not be run: {error}"));
        }
        Ok(output) if !output.status.success() => {
            return Some(format!(
                "no reachable per-user systemd: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(_) => {}
    }
    if path.exists() {
        return Some(format!(
            "{} already exists — this machine has a real install, and this test refuses to \
             stop, replace, or remove somebody's daemon to make room for itself",
            path.display()
        ));
    }
    if std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok() {
        return Some(format!(
            "something already answers on 127.0.0.1:{PORT}, so the supervised job could not \
             take the port and would crash-loop instead of running"
        ));
    }
    None
}

/// Removes whatever the test installed, whether it finished or panicked.
///
/// `systemctl` directly rather than the verb under test: a cleanup that runs
/// the thing that may have just failed can leave a unit behind for exactly the
/// reason the test was reporting.
struct Cleanup {
    path: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = systemctl(&["disable", "--now", proxenos::supervisor::SERVICE]);
        let _ = std::fs::remove_file(&self.path);
        let _ = systemctl(&["daemon-reload"]);
    }
}

/// The binary under test, in an environment of its own.
///
/// `PROXENOS_HOME` and `TMPDIR` are the throwaway directory, so the control
/// socket, the configuration and the log are all inside it — and the unit
/// carries both, which is what makes the daemon's bind and this CLI's dial the
/// same path. `HOME` is deliberately the real one: the manager reads its own
/// `XDG_CONFIG_HOME`, so a unit written anywhere else is a file nothing loads.
fn proxenos(home: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_proxenos"))
        .args(arguments)
        .env("PROXENOS_HOME", home)
        .env("TMPDIR", home)
        .output()
        .expect("the binary should run")
}

fn json(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "expected one JSON document, got {error}\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn install_supervises_this_daemon_and_uninstall_takes_it_away() {
    if let Some(reason) = refusal() {
        println!("skipped: {reason}");
        return;
    }
    let path = unit_path().expect("a unit path, since the refusal above found one");
    let home = tempfile::tempdir().expect("a throwaway home");
    let _cleanup = Cleanup { path: path.clone() };

    let install = proxenos(home.path(), &["supervisor", "install"]);
    assert!(
        install.status.success(),
        "install failed: {}{}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );
    assert!(
        String::from_utf8_lossy(&install.stdout).contains("supervising proxenos.service"),
        "{}",
        String::from_utf8_lossy(&install.stdout)
    );

    let installed = json(&proxenos(home.path(), &["supervisor", "status", "--json"]));
    assert_eq!(installed["installed"], "current", "{installed}");
    assert_eq!(
        installed["unit"],
        serde_json::Value::from(path.display().to_string()),
        "{installed}"
    );
    let state = installed["state"].as_str().unwrap_or_default().to_owned();
    assert!(
        state.starts_with("active"),
        "the manager should be running the unit it just enabled, and says: {state}"
    );
    assert!(installed["pid"].is_u64(), "{installed}");

    // The daemon behind that pid, once it answers its socket. `enable --now`
    // returns when the process has been spawned, not when it has bound
    // anything, so this waits rather than asserting on the first read.
    let daemon = answering(home.path());
    assert_eq!(
        daemon["supervised"],
        serde_json::Value::Bool(true),
        "a daemon systemd started should say so: {daemon}"
    );
    assert_eq!(
        daemon["pid"], installed["pid"],
        "the process the manager names and the one answering are the same: {daemon}"
    );

    let uninstall = proxenos(home.path(), &["supervisor", "uninstall"]);
    assert!(
        uninstall.status.success(),
        "uninstall failed: {}{}",
        String::from_utf8_lossy(&uninstall.stdout),
        String::from_utf8_lossy(&uninstall.stderr)
    );
    assert!(!path.exists(), "{} survived uninstall", path.display());

    let after = json(&proxenos(home.path(), &["supervisor", "status", "--json"]));
    assert_eq!(after["installed"], "absent", "{after}");
    assert!(
        after["state"].is_null(),
        "a removed unit has no state to report: {after}"
    );
}

/// The daemon's own `status` payload, once it is answering.
///
/// Ten seconds, in the same order `start` uses: a supervised daemon reads a
/// configuration, resolves its tiers and binds two listeners before the socket
/// exists, and none of that is instant on a cold machine.
fn answering(home: &Path) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let output = proxenos(home, &["status", "--json"]);
        if output.status.success() {
            return json(&output);
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "the supervised daemon never answered its control socket. Last error: {}\nJournal: {}",
                String::from_utf8_lossy(&output.stderr).trim(),
                String::from_utf8_lossy(
                    &systemctl(&["status", "--no-pager", proxenos::supervisor::SERVICE])
                        .map(|output| output.stdout)
                        .unwrap_or_default()
                )
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
