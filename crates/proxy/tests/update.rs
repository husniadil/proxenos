//! `docs/api.md` §3 `update` — a daemon replacing its own binary with a
//! published release.
//!
//! Every release here is built by the test and served from a local server:
//! nothing reaches the network. The archive has the layout the release
//! workflow produces (`proxenos-<target>/proxenos`, one directory inside), and
//! the binary in it is a script that states a version, which is all the
//! daemon asks of it before putting it in place.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use sha2::Digest;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

const VERSION: &str = "999.0.0";

fn target() -> &'static str {
    proxenos::update::target().expect("the suite runs on a platform with a release")
}

/// A script that answers `--version` the way a release binary does.
fn fake_binary(path: &Path, reports: &str) {
    std::fs::write(path, format!("#!/bin/sh\necho 'proxenos {reports}'\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `v<VERSION>/proxenos-<target>.tar.gz` and its `SHA256SUMS` under a fresh
/// directory. `sums` rewrites the checksum document, to tamper with it.
fn release(reports: &str, sums: impl FnOnce(String) -> String) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let release = root.path().join(format!("v{VERSION}"));
    let inner = release.join(format!("proxenos-{}", target()));
    std::fs::create_dir_all(&inner).unwrap();
    fake_binary(&inner.join("proxenos"), reports);

    let archive = proxenos::update::archive_name(target());
    let packed = std::process::Command::new("tar")
        .arg("-czf")
        .arg(release.join(&archive))
        .arg("-C")
        .arg(&release)
        .arg(format!("proxenos-{}", target()))
        .status()
        .unwrap();
    assert!(packed.success());
    std::fs::remove_dir_all(&inner).unwrap();

    let digest = hex(&sha2::Sha256::digest(
        std::fs::read(release.join(&archive)).unwrap(),
    ));
    let document = format!(
        "{}  proxenos-some-other-target.tar.gz\n{digest}  {archive}\n",
        "0".repeat(64)
    );
    std::fs::write(release.join("SHA256SUMS"), sums(document)).unwrap();
    root
}

/// Serve a directory as a release host does: a file by its path, 404 otherwise.
async fn serve(root: PathBuf) -> String {
    let router = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let root = root.clone();
        async move {
            let path = root.join(uri.path().trim_start_matches('/'));
            match std::fs::read(path) {
                Ok(bytes) => (axum::http::StatusCode::OK, bytes),
                Err(_) => (axum::http::StatusCode::NOT_FOUND, Vec::new()),
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_verified_archive_is_unpacked_to_its_binary() {
    let host = release(&format!("{VERSION}+abc1234"), |sums| sums);
    let base = serve(host.path().to_path_buf()).await;
    let work = tempfile::tempdir().unwrap();

    let binary = proxenos::update::fetch(
        &reqwest::Client::new(),
        &base,
        VERSION,
        target(),
        work.path(),
    )
    .await
    .unwrap();

    assert_eq!(
        binary,
        work.path()
            .join(format!("proxenos-{}", target()))
            .join("proxenos")
    );
    let output = std::process::Command::new(&binary)
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("proxenos {VERSION}+abc1234")
    );
}

/// A tampered archive is refused before it is unpacked: nothing from it
/// reaches the disk beyond the download itself.
#[tokio::test]
async fn an_archive_that_does_not_match_its_checksum_is_refused() {
    let host = release(VERSION, |sums| {
        let archive = proxenos::update::archive_name(target());
        sums.lines()
            .map(|line| {
                if line.ends_with(&archive) {
                    format!("{}  {archive}", "f".repeat(64))
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    let base = serve(host.path().to_path_buf()).await;
    let work = tempfile::tempdir().unwrap();

    let refused = proxenos::update::fetch(
        &reqwest::Client::new(),
        &base,
        VERSION,
        target(),
        work.path(),
    )
    .await
    .unwrap_err();

    assert!(
        refused.message.contains("checksum mismatch"),
        "{}",
        refused.message
    );
    assert!(
        !work.path().join(format!("proxenos-{}", target())).exists(),
        "a refused archive is never unpacked"
    );
}

#[tokio::test]
async fn a_release_that_lists_no_binary_for_this_platform_is_refused() {
    let host = release(VERSION, |sums| {
        let archive = proxenos::update::archive_name(target());
        sums.lines()
            .filter(|line| !line.ends_with(&archive))
            .collect::<Vec<_>>()
            .join("\n")
    });
    let base = serve(host.path().to_path_buf()).await;
    let work = tempfile::tempdir().unwrap();

    let refused = proxenos::update::fetch(
        &reqwest::Client::new(),
        &base,
        VERSION,
        target(),
        work.path(),
    )
    .await
    .unwrap_err();

    assert!(
        refused
            .message
            .contains("publishes no binary for this platform"),
        "{}",
        refused.message
    );
}

#[tokio::test]
async fn a_release_that_does_not_exist_is_refused_with_the_answer() {
    let host = release(VERSION, |sums| sums);
    let base = serve(host.path().to_path_buf()).await;
    let work = tempfile::tempdir().unwrap();

    let refused = proxenos::update::fetch(
        &reqwest::Client::new(),
        &base,
        "998.0.0",
        target(),
        work.path(),
    )
    .await
    .unwrap_err();

    assert!(
        refused.message.contains("v998.0.0/SHA256SUMS"),
        "{}",
        refused.message
    );
    assert!(refused.message.contains("404"), "{}", refused.message);
}

/// The new file is renamed over the old one, never written into it: the
/// running binary keeps its inode, now under the `.previous` name.
#[test]
fn the_new_binary_replaces_the_old_by_rename_and_the_old_is_kept() {
    let bin = tempfile::tempdir().unwrap();
    let installed = bin.path().join("proxenos");
    fake_binary(&installed, "0.30.0");
    let original = std::fs::metadata(&installed).unwrap().ino();
    let downloads = tempfile::tempdir().unwrap();
    let downloaded = downloads.path().join("proxenos");
    fake_binary(&downloaded, &format!("{VERSION}+abc1234"));

    let reported = proxenos::update::install(&downloaded, &installed, VERSION).unwrap();

    assert_eq!(reported, format!("{VERSION}+abc1234"));
    assert_eq!(
        std::fs::read(&installed).unwrap(),
        std::fs::read(&downloaded).unwrap()
    );
    let previous = bin.path().join("proxenos.previous");
    assert_eq!(std::fs::metadata(&previous).unwrap().ino(), original);
    assert_ne!(std::fs::metadata(&installed).unwrap().ino(), original);
    let left: Vec<_> = std::fs::read_dir(bin.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left.len(), 2, "nothing staged is left behind: {left:?}");
}

#[test]
fn a_binary_reporting_another_version_is_not_installed() {
    let bin = tempfile::tempdir().unwrap();
    let installed = bin.path().join("proxenos");
    fake_binary(&installed, "0.30.0");
    let before = std::fs::read(&installed).unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let downloaded = downloads.path().join("proxenos");
    fake_binary(&downloaded, "0.31.0");

    let refused = proxenos::update::install(&downloaded, &installed, VERSION).unwrap_err();

    assert!(refused.message.contains("reports"), "{}", refused.message);
    assert!(
        refused.message.contains("Nothing was installed"),
        "{}",
        refused.message
    );
    assert_eq!(std::fs::read(&installed).unwrap(), before);
    let left: Vec<_> = std::fs::read_dir(bin.path()).unwrap().collect();
    assert_eq!(left.len(), 1, "no previous and no staged file");
}

/// The whole path through the shipping binary: a daemon launchd would call
/// supervised, run from `~/.local/bin`, asked by the CLI to update. It answers
/// with what it did, the file on disk is the new release, and the process goes.
///
/// macOS only, because supervision there is read from the environment launchd
/// sets, which a test can set; on Linux it is asked of systemd.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn a_supervised_daemon_updates_itself_and_stops() {
    let host = release(&format!("{VERSION}+abc1234"), |sums| sums);
    let base = serve(host.path().to_path_buf()).await;

    // A short TMPDIR: the control socket's path has a ~104-byte ceiling.
    let dir = tempfile::Builder::new()
        .prefix("pxu")
        .tempdir_in("/tmp")
        .unwrap();
    let home = dir.path().to_path_buf();
    let bin = home.join(".local").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let installed = bin.join("proxenos");
    std::fs::copy(env!("CARGO_BIN_EXE_proxenos"), &installed).unwrap();

    let command = |program: &Path| {
        let mut command = std::process::Command::new(program);
        command
            .env_remove("PROXENOS_DAEMON")
            .env_remove("PROXENOS_TOKEN_FILE")
            .env_remove("PROXENOS_TOKEN")
            .env_remove(proxenos::update::BIN_DIR_VAR)
            .env("PROXENOS_HOME", home.join("config"))
            .env("HOME", &home)
            .env("TMPDIR", &home);
        command
    };
    std::fs::create_dir_all(home.join("config")).unwrap();

    let mut daemon = command(&installed)
        .args(["run", "--port", "0"])
        .env("XPC_SERVICE_NAME", proxenos::supervisor::LABEL)
        .env(proxenos::update::RELEASES_VAR, &base)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let socket = home.join("config").join("proxenos.sock");
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(socket.exists(), "the daemon never answered its socket");

    let mut cli = command(Path::new(env!("CARGO_BIN_EXE_proxenos")));
    cli.args(["update", "--version", VERSION, "--json"]);
    let output = tokio::task::spawn_blocking(move || cli.output().unwrap())
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "update failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let canonical = std::fs::canonicalize(&installed).unwrap();
    assert_eq!(
        answer["from"],
        serde_json::json!(proxenos::version::build())
    );
    assert_eq!(
        answer["to"],
        serde_json::json!(format!("{VERSION}+abc1234"))
    );
    assert_eq!(
        answer["path"],
        serde_json::json!(canonical.display().to_string())
    );
    assert_eq!(answer["restarting"], serde_json::json!(true));

    let mut exited = None;
    for _ in 0..100 {
        exited = daemon.try_wait().unwrap();
        if exited.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    if exited.is_none() {
        let _ = daemon.kill();
        panic!("the daemon kept running after it answered the update");
    }

    let reported = std::process::Command::new(&installed)
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&reported.stdout).trim(),
        format!("proxenos {VERSION}+abc1234")
    );
    assert_eq!(
        std::fs::read(bin.join("proxenos.previous")).unwrap(),
        std::fs::read(env!("CARGO_BIN_EXE_proxenos")).unwrap()
    );
}
