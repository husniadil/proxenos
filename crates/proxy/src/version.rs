//! What this binary calls itself.
//!
//! `docs/api.md` §3 — the version reported over the control socket, printed by
//! `--version`, and named by `status`, `stop` and `supervisor install`. It
//! carries a build id, because the question those verbs are asked after a
//! rebuild is "is the thing answering the thing I just built", and a version
//! number cannot tell two builds of itself apart.
//!
//! It is not the version sent upstream: that one names a client the backend
//! filters its catalog by, and a build id there would be a lie about a
//! different program.

use std::sync::LazyLock;

/// The version string, from its three parts.
///
/// A pure function so the shape is a test rather than something read off
/// whichever commit happens to be checked out. `dirty` is a build from a tree
/// with uncommitted changes: the sha alone would name a commit the binary does
/// not actually contain.
pub fn describe(version: &str, sha: &str, dirty: bool) -> String {
    let suffix = if dirty { "-dirty" } else { "" };
    format!("{version}+{sha}{suffix}")
}

static BUILD: LazyLock<String> = LazyLock::new(|| {
    describe(
        env!("CARGO_PKG_VERSION"),
        env!("PROXENOS_BUILD_SHA"),
        env!("PROXENOS_BUILD_DIRTY") == "1",
    )
});

/// This build, as everything operator-facing names it.
pub fn build() -> &'static str {
    &BUILD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_is_its_version_and_its_commit() {
        assert_eq!(describe("0.12.0", "ab12cd3", false), "0.12.0+ab12cd3");
    }

    /// A tree with uncommitted changes is not the commit it sits on, and the
    /// string says so rather than naming a build nobody can check out.
    #[test]
    fn a_dirty_tree_is_said_out_loud() {
        assert_eq!(describe("0.12.0", "ab12cd3", true), "0.12.0+ab12cd3-dirty");
    }

    /// No git is a stated unknown. A version with nothing after it would be
    /// indistinguishable from a build id nobody thought to add.
    #[test]
    fn a_build_with_no_git_says_unknown() {
        assert_eq!(describe("0.12.0", "unknown", false), "0.12.0+unknown");
    }

    /// The one that matters to `status` and `stop`: two builds of one version
    /// number are different strings.
    #[test]
    fn two_builds_of_one_version_do_not_print_alike() {
        assert_ne!(
            describe("0.12.0", "ab12cd3", false),
            describe("0.12.0", "ef45gh6", false)
        );
        assert_ne!(
            describe("0.12.0", "ab12cd3", false),
            describe("0.12.0", "ab12cd3", true)
        );
    }
}
