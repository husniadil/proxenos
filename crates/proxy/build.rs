//! A build id for the version this binary reports.
//!
//! One file is both the daemon and the CLI, and two builds of one version
//! number are indistinguishable by that number alone — which is exactly the
//! question `stop` and `status` are asked after a rebuild. The commit and
//! whether the tree was dirty are read here, at build time, because a running
//! binary cannot go and look: it may have been moved, installed, or shipped
//! away from any checkout.
//!
//! No `rerun-if-changed` is declared on purpose. Declaring one replaces the
//! default, which is to rerun whenever a file in this package changes — and
//! that default is what keeps the dirty flag honest for the source this crate
//! is actually built from.

fn main() {
    // Absent git — a tarball build, or a checkout with no history — is a
    // stated `unknown` rather than a version that quietly looks like a release
    // build of unknown provenance.
    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    let dirty = git(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty());

    println!("cargo:rustc-env=PROXENOS_BUILD_SHA={sha}");
    println!("cargo:rustc-env=PROXENOS_BUILD_DIRTY={}", u8::from(dirty));
}

/// What git says, or nothing at all where it cannot be asked.
fn git(arguments: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(arguments)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
