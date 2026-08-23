//! A build id for the version this binary reports.
//!
//! One file is both the daemon and the CLI, and two builds of one version
//! number are indistinguishable by that number alone — which is exactly the
//! question `stop` and `status` are asked after a rebuild. The commit and
//! whether the tree was dirty are read here, at build time, because a running
//! binary cannot go and look: it may have been moved, installed, or shipped
//! away from any checkout.
//!
//! Declaring `rerun-if-changed` replaces cargo's default of rerunning on any
//! change under this package, so the package directory is declared back
//! explicitly, alongside the repository's `HEAD` and the ref it points at.
//! Without those two a commit touching nothing under this crate left the sha
//! one commit behind until something here happened to change.

fn main() {
    // Absent git — a tarball build, or a checkout with no history — is a
    // stated `unknown` rather than a version that quietly looks like a release
    // build of unknown provenance.
    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    let dirty = git(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty());

    println!("cargo:rerun-if-changed=.");
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={git_dir}/{reference}");
        }
    }

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
