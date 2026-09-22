//! `docs/api.md` §3 — `update`: the daemon replacing its own binary with a
//! published release, then stopping so its supervisor starts the new one.
//!
//! The pure half decides whether an update may happen at all and reads what a
//! release publishes; the other half downloads, verifies, and swaps the file.
//! The layout read is the one `install.sh` reads: `v<version>/SHA256SUMS` and
//! `v<version>/proxenos-<target>.tar.gz`, the binary one directory inside.

use crate::error::ProxyError;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::path::Path;
use std::path::PathBuf;

/// Where releases are published.
pub const RELEASES: &str = "https://github.com/husniadil/proxenos/releases/download";

/// Replaces `RELEASES`, for a mirror and for the suite, which must never reach
/// the network.
pub const RELEASES_VAR: &str = "PROXENOS_RELEASES";

/// The install directory `install.sh` takes, read here with the same meaning.
pub const BIN_DIR_VAR: &str = "PROXENOS_BIN_DIR";

/// The version asked for, from the method's parameters.
pub fn requested_version(params: Option<&Value>) -> Result<String, ProxyError> {
    let Some(version) = params
        .and_then(|params| params.get("version"))
        .and_then(Value::as_str)
    else {
        return Err(ProxyError::invalid_request(
            "`update` needs `{\"version\": \"X.Y.Z\"}`, a release version without the leading `v`",
        ));
    };
    if parse_version(version).is_none() {
        return Err(ProxyError::invalid_request(format!(
            "`{version}` is not a release version; expected X.Y.Z, without the leading `v`"
        )));
    }
    Ok(version.to_owned())
}

/// `X.Y.Z`, each part digits and nothing else.
pub fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.');
    let mut parsed = [0_u64; 3];
    for slot in &mut parsed {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some(parsed)
}

/// Whether `requested` is a later release than `running`.
///
/// `running` may carry a build id (`0.30.0+ab12cd3`), which says which commit
/// and not which release, so only the part before it is compared.
pub fn is_newer(requested: &str, running: &str) -> bool {
    let running = running.split(['+', '-']).next().unwrap_or(running);
    match (parse_version(requested), parse_version(running)) {
        (Some(requested), Some(running)) => requested > running,
        _ => false,
    }
}

/// The platform, as the release names it. `None` where no binary is published.
pub fn target() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "aarch64",
        target_env = "gnu"
    )) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else {
        None
    }
}

pub fn archive_name(target: &str) -> String {
    format!("proxenos-{target}.tar.gz")
}

/// The expected digest of `file`, from a `SHA256SUMS` document.
///
/// A line is `<hex>  <name>`, or `<hex> *<name>` from a tool in binary mode.
/// A line that is not exactly that is skipped rather than guessed at, so a
/// malformed document reads as the file not being listed.
pub fn checksum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let digest = fields.next()?;
        let name = fields.next()?;
        let well_formed = fields.next().is_none()
            && digest.len() == 64
            && digest.bytes().all(|byte| byte.is_ascii_hexdigit());
        (well_formed && name.strip_prefix('*').unwrap_or(name) == file)
            .then(|| digest.to_ascii_lowercase())
    })
}

/// The directory an update may write into: `PROXENOS_BIN_DIR` where set, as
/// `install.sh` reads it, else `~/.local/bin`.
pub fn bin_dir(home: Option<&Path>, stated: Option<&Path>) -> Option<PathBuf> {
    match stated.filter(|stated| !stated.as_os_str().is_empty()) {
        Some(stated) => Some(stated.to_path_buf()),
        None => home.map(|home| home.join(".local").join("bin")),
    }
}

/// The version a binary's `--version` states, where it is the one asked for.
///
/// `proxenos 0.31.0` or `proxenos 0.31.0+ab12cd3`. A different release, or
/// `0.31.01`, is not a match: the prefix has to end where the version does.
pub fn reported_version(output: &str, requested: &str) -> Option<String> {
    let line = output.lines().next()?.trim();
    let stated = line.strip_prefix("proxenos ").unwrap_or(line).trim();
    let matches = stated == requested
        || stated
            .strip_prefix(requested)
            .is_some_and(|rest| rest.starts_with('+'));
    matches.then(|| stated.to_owned())
}

/// What the daemon knows about itself when asked to update.
pub struct Situation<'a> {
    pub requested: &'a str,
    /// The build serving, as `status.version` names it.
    pub running: &'a str,
    pub supervised: Option<bool>,
    /// This process's executable, symlinks resolved.
    pub executable: &'a Path,
    /// `bin_dir`, symlinks resolved. `None` where no home is known.
    pub bin_dir: Option<&'a Path>,
    pub target: Option<&'a str>,
}

/// Why this daemon will not update itself, or `None` where it may.
///
/// Every refusal is decided before anything is downloaded or written, so a
/// refused update has touched nothing.
pub fn refusal(situation: &Situation<'_>) -> Option<String> {
    let Situation {
        requested,
        running,
        supervised,
        executable,
        bin_dir,
        target,
    } = situation;

    if *supervised != Some(true) {
        return Some(
            "this daemon is not running under its supervisor, so nothing would start it again \
             after the update. Install one with `proxenos supervisor install`, or update the \
             binary by hand and restart the daemon."
                .to_owned(),
        );
    }
    let installed = bin_dir.is_some_and(|dir| executable.parent() == Some(dir));
    if !installed {
        let expected = bin_dir.map_or_else(
            || "~/.local/bin".to_owned(),
            |dir| dir.display().to_string(),
        );
        return Some(format!(
            "this daemon runs from {}, not from {expected}, where `install.sh` puts it. Update it \
             the way it was installed.",
            executable.display()
        ));
    }
    if !is_newer(requested, running) {
        return Some(format!(
            "this daemon runs {running}; {requested} is not newer"
        ));
    }
    if target.is_none() {
        return Some("no release binary is published for this platform".to_owned());
    }
    None
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, ProxyError> {
    let response = client.get(url).send().await.map_err(|error| {
        ProxyError::invalid_request(format!("could not download {url}: {error}"))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProxyError::invalid_request(format!(
            "could not download {url}: it answered {status}"
        )));
    }
    let body = response.bytes().await.map_err(|error| {
        ProxyError::invalid_request(format!("could not download {url}: {error}"))
    })?;
    Ok(body.to_vec())
}

/// Download a release's archive, verify it against the release's
/// `SHA256SUMS`, and extract it into `work`. Returns the extracted binary.
///
/// Nothing outside `work` is written. A checksum that is missing or does not
/// match refuses before the archive is unpacked.
pub async fn fetch(
    client: &reqwest::Client,
    base: &str,
    version: &str,
    target: &str,
    work: &Path,
) -> Result<PathBuf, ProxyError> {
    let release = format!("{}/v{version}", base.trim_end_matches('/'));
    let archive = archive_name(target);

    let sums = download(client, &format!("{release}/SHA256SUMS")).await?;
    let sums = String::from_utf8_lossy(&sums);
    let Some(expected) = checksum_for(&sums, &archive) else {
        return Err(ProxyError::invalid_request(format!(
            "v{version} publishes no binary for this platform: {archive} is not listed in its \
             SHA256SUMS"
        )));
    };

    let bytes = download(client, &format!("{release}/{archive}")).await?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(ProxyError::invalid_request(format!(
            "checksum mismatch for {archive}: expected {expected}, got {actual}. Nothing was \
             installed."
        )));
    }

    let saved = work.join(&archive);
    std::fs::write(&saved, &bytes).map_err(|error| {
        ProxyError::invalid_request(format!("could not write {}: {error}", saved.display()))
    })?;
    let unpacked = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&saved)
        .arg("-C")
        .arg(work)
        .output()
        .map_err(|error| ProxyError::invalid_request(format!("could not run tar: {error}")))?;
    if !unpacked.status.success() {
        return Err(ProxyError::invalid_request(format!(
            "could not unpack {archive}: {}",
            String::from_utf8_lossy(&unpacked.stderr).trim()
        )));
    }

    let binary = work.join(format!("proxenos-{target}")).join("proxenos");
    if !binary.is_file() {
        return Err(ProxyError::invalid_request(format!(
            "{archive} does not contain proxenos-{target}/proxenos"
        )));
    }
    Ok(binary)
}

/// Put `downloaded` in place of `installed`, keeping the old file as
/// `<name>.previous` beside it. Returns the version the new file reports.
///
/// **Never written over in place.** The new file is staged beside the target
/// and renamed over it, so the running daemon keeps its own image and macOS,
/// which caches code-signing state per vnode, never sees a signed file change
/// under it. The staged copy is what is asked for its version, so the check
/// is of the file that is installed, not of the one it was copied from.
pub fn install(downloaded: &Path, installed: &Path, requested: &str) -> Result<String, ProxyError> {
    let failed =
        |what: &str, error: std::io::Error| ProxyError::invalid_request(format!("{what}: {error}"));
    let (Some(dir), Some(name)) = (installed.parent(), installed.file_name()) else {
        return Err(ProxyError::invalid_request(format!(
            "{} does not name a file in a directory",
            installed.display()
        )));
    };
    let name = name.to_string_lossy();
    let staged = dir.join(format!(".{name}.update"));
    let previous = dir.join(format!("{name}.previous"));

    let _ = std::fs::remove_file(&staged);
    std::fs::copy(downloaded, &staged)
        .map_err(|error| failed(&format!("could not write {}", staged.display()), error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(
            |error| {
                failed(
                    &format!("could not mark {} executable", staged.display()),
                    error,
                )
            },
        )?;
    }

    let reported = std::process::Command::new(&staged)
        .arg("--version")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    let reported = match reported {
        Ok(output) => match reported_version(&output, requested) {
            Some(version) => version,
            None => {
                let _ = std::fs::remove_file(&staged);
                return Err(ProxyError::invalid_request(format!(
                    "the downloaded binary reports `{}`, not {requested}. Nothing was installed.",
                    output.trim()
                )));
            }
        },
        Err(error) => {
            let _ = std::fs::remove_file(&staged);
            return Err(failed("the downloaded binary did not run", error));
        }
    };

    match std::fs::remove_file(&previous) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            let _ = std::fs::remove_file(&staged);
            return Err(failed(
                &format!("could not replace {}", previous.display()),
                error,
            ));
        }
    }
    // A second name for the running file rather than a copy: the same bytes,
    // and nothing is written that the kernel has already assessed.
    if let Err(error) = std::fs::hard_link(installed, &previous) {
        let _ = std::fs::remove_file(&staged);
        return Err(failed(
            &format!("could not keep {}", previous.display()),
            error,
        ));
    }
    if let Err(error) = std::fs::rename(&staged, installed) {
        let _ = std::fs::remove_file(&staged);
        return Err(failed(
            &format!("could not replace {}", installed.display()),
            error,
        ));
    }
    Ok(reported)
}

/// One update at a time. A second would download beside the first and race it
/// to the rename.
static UPDATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Held while an update runs; letting go of it lets the next one start.
pub struct Running(());

impl Running {
    pub fn claim() -> Option<Self> {
        UPDATING
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
            .then_some(Self(()))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        UPDATING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_version_is_three_numbers() {
        assert_eq!(parse_version("0.30.0"), Some([0, 30, 0]));
        assert_eq!(parse_version("1.2.10"), Some([1, 2, 10]));
        for refused in [
            "v0.30.0",
            "0.30",
            "0.30.0.1",
            "0.30.0+abc",
            "0.x.0",
            "",
            "0..0",
        ] {
            assert_eq!(parse_version(refused), None, "{refused}");
        }
    }

    #[test]
    fn the_parameters_name_the_version_without_its_v() {
        let asked = serde_json::json!({ "version": "0.31.0" });
        assert_eq!(requested_version(Some(&asked)).unwrap(), "0.31.0");

        let with_v = serde_json::json!({ "version": "v0.31.0" });
        let refused = requested_version(Some(&with_v)).unwrap_err();
        assert!(
            refused.message.contains("without the leading `v`"),
            "{}",
            refused.message
        );

        let refused = requested_version(None).unwrap_err();
        assert!(refused.message.contains("needs"), "{}", refused.message);
    }

    /// Numbers compare as numbers, and a build id says nothing about order.
    #[test]
    fn only_a_later_release_is_newer() {
        assert!(is_newer("0.31.0", "0.30.0+6511232"));
        assert!(is_newer("0.30.10", "0.30.9"));
        assert!(is_newer("1.0.0", "0.99.99-dirty"));
        assert!(!is_newer("0.30.0", "0.30.0+6511232"));
        assert!(!is_newer("0.29.1", "0.30.0"));
        assert!(!is_newer("0.31.0", "unparseable"));
    }

    #[test]
    fn a_checksum_line_is_found_by_its_exact_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!(
            "{a}  proxenos-x86_64-unknown-linux-gnu.tar.gz\n\
             {b} *proxenos-aarch64-apple-darwin.tar.gz\n\
             nonsense line\n"
        );
        assert_eq!(
            checksum_for(&sums, "proxenos-x86_64-unknown-linux-gnu.tar.gz"),
            Some(a)
        );
        assert_eq!(
            checksum_for(&sums, "proxenos-aarch64-apple-darwin.tar.gz"),
            Some("b".repeat(64)),
            "a binary-mode line counts, and the digest compares lowercase"
        );
        assert_eq!(
            checksum_for(&sums, "proxenos-x86_64-apple-darwin.tar.gz"),
            None
        );
        assert_eq!(
            checksum_for(&sums, "linux-gnu.tar.gz"),
            None,
            "a suffix is not a name"
        );
    }

    #[test]
    fn a_digest_that_is_not_sha256_is_not_a_listing() {
        let sums = "abc123  proxenos-aarch64-apple-darwin.tar.gz\n";
        assert_eq!(
            checksum_for(sums, "proxenos-aarch64-apple-darwin.tar.gz"),
            None
        );
    }

    #[test]
    fn the_install_directory_is_install_shs() {
        assert_eq!(
            bin_dir(Some(Path::new("/home/me")), None),
            Some(PathBuf::from("/home/me/.local/bin"))
        );
        assert_eq!(
            bin_dir(Some(Path::new("/home/me")), Some(Path::new("/opt/bin"))),
            Some(PathBuf::from("/opt/bin"))
        );
        assert_eq!(
            bin_dir(Some(Path::new("/home/me")), Some(Path::new(""))),
            Some(PathBuf::from("/home/me/.local/bin")),
            "an empty override is no override, as in install.sh"
        );
        assert_eq!(bin_dir(None, None), None);
    }

    #[test]
    fn the_new_binary_has_to_say_it_is_the_version_asked_for() {
        assert_eq!(
            reported_version("proxenos 0.31.0\n", "0.31.0").as_deref(),
            Some("0.31.0")
        );
        assert_eq!(
            reported_version("proxenos 0.31.0+ab12cd3\n", "0.31.0").as_deref(),
            Some("0.31.0+ab12cd3")
        );
        assert_eq!(reported_version("proxenos 0.31.01\n", "0.31.0"), None);
        assert_eq!(
            reported_version("proxenos 0.30.0+ab12cd3\n", "0.31.0"),
            None
        );
        assert_eq!(reported_version("", "0.31.0"), None);
    }

    fn situation<'a>(executable: &'a Path, bin_dir: Option<&'a Path>) -> Situation<'a> {
        Situation {
            requested: "0.31.0",
            running: "0.30.0+6511232",
            supervised: Some(true),
            executable,
            bin_dir,
            target: Some("aarch64-apple-darwin"),
        }
    }

    #[test]
    fn a_supervised_install_in_the_bin_dir_may_update() {
        let dir = Path::new("/home/me/.local/bin");
        let exe = dir.join("proxenos");
        assert_eq!(refusal(&situation(&exe, Some(dir))), None);
    }

    /// Exiting is how the new binary starts, so a daemon nothing restarts
    /// would be left down.
    #[test]
    fn an_unsupervised_daemon_is_refused() {
        let dir = Path::new("/home/me/.local/bin");
        let exe = dir.join("proxenos");
        for supervised in [Some(false), None] {
            let refused = refusal(&Situation {
                supervised,
                ..situation(&exe, Some(dir))
            })
            .unwrap();
            assert!(
                refused.contains("not running under its supervisor"),
                "{refused}"
            );
        }
    }

    /// Directly inside, not anywhere under: a build in a checkout below the
    /// directory is not the file `install.sh` wrote.
    #[test]
    fn only_a_binary_directly_in_the_bin_dir_is_replaced() {
        let dir = Path::new("/home/me/.local/bin");
        for exe in [
            PathBuf::from("/home/me/src/proxenos/target/release/proxenos"),
            PathBuf::from("/home/me/.local/bin/nested/proxenos"),
            PathBuf::from("/opt/homebrew/bin/proxenos"),
        ] {
            let refused = refusal(&situation(&exe, Some(dir))).unwrap();
            assert!(refused.contains(&exe.display().to_string()), "{refused}");
            assert!(refused.contains("/home/me/.local/bin"), "{refused}");
            assert!(refused.contains("the way it was installed"), "{refused}");
        }
        let exe = dir.join("proxenos");
        assert!(
            refusal(&situation(&exe, None)).is_some(),
            "no home, no install dir"
        );
    }

    #[test]
    fn a_release_that_is_not_newer_is_refused_naming_the_running_one() {
        let dir = Path::new("/home/me/.local/bin");
        let exe = dir.join("proxenos");
        for requested in ["0.30.0", "0.29.1"] {
            let refused = refusal(&Situation {
                requested,
                ..situation(&exe, Some(dir))
            })
            .unwrap();
            assert_eq!(
                refused,
                format!("this daemon runs 0.30.0+6511232; {requested} is not newer")
            );
        }
    }

    #[test]
    fn a_platform_with_no_release_binary_is_refused() {
        let dir = Path::new("/home/me/.local/bin");
        let exe = dir.join("proxenos");
        let refused = refusal(&Situation {
            target: None,
            ..situation(&exe, Some(dir))
        })
        .unwrap();
        assert!(refused.contains("no release binary"), "{refused}");
    }

    #[test]
    fn one_update_at_a_time() {
        let first = Running::claim().unwrap();
        assert!(Running::claim().is_none());
        drop(first);
        assert!(Running::claim().is_some());
    }
}
