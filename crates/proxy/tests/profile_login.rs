//! `docs/proxy-behavior.md` §8.4 — signing in to a profile this daemon then
//! borrows from.
//!
//! Nothing here runs a client. What is asserted is the two decisions this side
//! actually makes: which program is pointed at which directory, and what the
//! configuration file says about it afterwards.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use pretty_assertions::assert_eq;
use proxenos::auth::profile_login::Command;
use proxenos::auth::store::Provider;
use proxenos::config::Config;
use proxenos::config::edit;
use std::path::Path;
use std::path::PathBuf;

/// Each client is signed in by its own verb, and the directory travels as the
/// same variable this daemon later resolves the grant from — so what was
/// signed in and what is read cannot drift apart.
#[test]
fn each_provider_is_signed_in_by_its_own_program() {
    let claude = Command::new(Provider::Anthropic, PathBuf::from("/profiles/work"), None);
    assert_eq!(claude.program, "claude");
    assert_eq!(claude.arguments, ["auth", "login"]);
    assert_eq!(claude.variable, "CLAUDE_CONFIG_DIR");

    let codex = Command::new(Provider::Codex, PathBuf::from("/profiles/work"), None);
    assert_eq!(codex.program, "codex");
    assert_eq!(codex.arguments, ["login"]);
    assert_eq!(codex.variable, "CODEX_HOME");
}

/// The configured client path is used here too. An operator who had to write
/// it down once should not have to remember which half of the project reads
/// it.
#[test]
fn the_configured_client_is_the_one_that_is_run() {
    let command = Command::new(
        Provider::Anthropic,
        PathBuf::from("/profiles/work"),
        Some(Path::new("/opt/homebrew/bin/claude")),
    );

    assert_eq!(command.program, "/opt/homebrew/bin/claude");
}

/// The printed line is what a person pastes, so a profile under a path with a
/// space in it has to survive being pasted. Both clients' stock locations are
/// under such a path on macOS.
#[test]
fn the_printed_line_survives_a_path_with_a_space() {
    let command = Command::new(
        Provider::Anthropic,
        PathBuf::from("/Users/me/Application Support/px/work"),
        None,
    );

    assert_eq!(
        command.line(),
        "CLAUDE_CONFIG_DIR='/Users/me/Application Support/px/work' claude auth login"
    );
}

/// What the file gains: a declared profile naming the directory that was
/// signed in, appended as a table so its keys cannot land in another one.
#[test]
fn declaring_a_profile_appends_a_table_that_parses() {
    let document = edit::add_profile(
        "port = 8787\n\n[tiers]\nopus = \"gpt-5.6-terra\"\n",
        "work",
        Provider::Codex,
        Some(Path::new("/profiles/work")),
    )
    .expect("appended");

    let parsed: Config = toml::from_str(&document).expect("the document parses");
    let profile = parsed.profiles.get("work").expect("declared");
    assert_eq!(profile.provider, Provider::Codex);
    assert_eq!(profile.path.as_deref(), Some(Path::new("/profiles/work")));
    // Everything that was there is still there, byte for byte.
    assert!(document.starts_with("port = 8787\n\n[tiers]\nopus = \"gpt-5.6-terra\"\n"));
}

/// No path written means the stock profile, which is a different profile from
/// one naming the stock directory (§8.4). That difference is not this
/// function's to resolve.
#[test]
fn a_profile_with_no_path_is_declared_as_the_stock_one() {
    let document =
        edit::add_profile("port = 8787\n", "claude", Provider::Anthropic, None).expect("appended");

    assert!(!document.contains("path"), "{document}");
    let parsed: Config = toml::from_str(&document).expect("the document parses");
    assert_eq!(parsed.profiles["claude"].path, None);
}

/// A name the file already declares is refused rather than written twice —
/// TOML refuses a table defined twice, so appending would leave the operator
/// with a file the daemon cannot start from and a parse error as the only
/// clue.
#[test]
fn declaring_a_name_the_file_already_has_is_refused() {
    let existing = "[profiles.work]\nprovider = \"codex\"\n";

    let refusal = edit::add_profile(existing, "work", Provider::Codex, None)
        .expect_err("already declared")
        .to_string();

    assert!(refusal.contains("already declares"), "{refusal}");

    // The quoted spelling of the same key is the same table.
    let quoted = "[profiles.\"work\"]\nprovider = \"codex\"\n";
    assert!(edit::add_profile(quoted, "work", Provider::Codex, None).is_err());
}

/// A path with a backslash or a quote in it is written as a TOML string that
/// reads back as the same path.
#[test]
fn a_path_that_needs_escaping_reads_back_unchanged() {
    let awkward = Path::new(r#"/profiles/with "quotes" and \slashes"#);

    let document = edit::add_profile("port = 8787\n", "odd", Provider::Codex, Some(awkward))
        .expect("appended");

    let parsed: Config = toml::from_str(&document).expect("the document parses");
    assert_eq!(parsed.profiles["odd"].path.as_deref(), Some(awkward));
}
