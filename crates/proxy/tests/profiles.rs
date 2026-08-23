//! `docs/api.md` §4 and `docs/proxy-behavior.md` §8.4 — declaring the profile
//! directories this daemon borrows grants from.
//!
//! The table holds paths and nothing else. A credential is never written into
//! the configuration file, and none is read out of it either: an entry says
//! where another program keeps one.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use pretty_assertions::assert_eq;
use proxenos::auth::store::Provider;
use proxenos::config::Config;
use proxenos::config::ProfileConfig;
use std::path::PathBuf;

fn parse(document: &str) -> Config {
    toml::from_str(document).expect("the document parses")
}

fn refusal(document: &str) -> String {
    parse(document)
        .validate()
        .expect_err("this document is refused")
        .to_string()
}

/// The shape an operator writes: which program owns the profile, and which
/// directory it is.
#[test]
fn a_profile_names_a_provider_and_a_directory() {
    let config = parse(
        r#"
[profiles.work]
provider = "codex"
path = "/profiles/work"
"#,
    );

    config.validate().expect("a full entry is accepted");
    assert_eq!(
        config.profiles.get("work"),
        Some(&ProfileConfig {
            provider: Provider::Codex,
            path: Some(PathBuf::from("/profiles/work")),
        })
    );
}

/// An entry with no path is the stock profile of that program: the one it uses
/// when no variable designates a directory.
#[test]
fn a_profile_without_a_path_is_accepted() {
    let config = parse(
        r#"
[profiles.personal]
provider = "anthropic"
"#,
    );

    config.validate().expect("a stock profile is accepted");
    assert_eq!(config.profiles["personal"].path, None);
}

/// A configuration that borrows nothing is unaffected.
#[test]
fn no_profiles_table_is_valid() {
    let config = parse("port = 8787\n");

    config.validate().expect("valid");
    assert!(config.profiles.is_empty());
}

/// A tilde is the shell's, and nothing here expands it. Expanding it would
/// make one spelling of a path work and another fail, and for Claude on macOS
/// the spelling is part of the identity (§8.4).
#[test]
fn a_tilde_path_is_refused_by_name() {
    let message = refusal(
        r#"
[profiles.work]
provider = "codex"
path = "~/.codex"
"#,
    );

    assert!(message.contains("profiles.work.path"), "was: {message}");
    assert!(message.contains('~'), "was: {message}");
}

/// A daemon's working directory is not the operator's, so a relative path
/// would resolve somewhere neither of them meant.
#[test]
fn a_relative_path_is_refused() {
    let message = refusal(
        r#"
[profiles.work]
provider = "codex"
path = "profiles/work"
"#,
    );

    assert!(message.contains("relative"), "was: {message}");
}

/// One directory holds one grant, so it is one account. Two names for it would
/// report the same account twice and offer a choice that changes nothing.
#[test]
fn two_names_for_one_directory_are_refused_naming_both() {
    let message = refusal(
        r#"
[profiles.work]
provider = "codex"
path = "/profiles/work"

[profiles.also_work]
provider = "codex"
path = "/profiles/work"
"#,
    );

    assert!(message.contains("also_work"), "was: {message}");
    assert!(message.contains("work"), "was: {message}");
}

/// Two stock profiles of the same program are the same profile too, path or
/// no path.
#[test]
fn two_stock_profiles_of_one_provider_are_refused() {
    let message = refusal(
        r#"
[profiles.first]
provider = "codex"

[profiles.second]
provider = "codex"
"#,
    );

    assert!(message.contains("same profile"), "was: {message}");
}

/// One directory can hold two programs' state — agent-profiles points both
/// `CODEX_HOME` and a Chromium user-data directory at one folder — so the
/// provider is part of what makes a profile distinct.
#[test]
fn one_directory_under_two_providers_is_two_profiles() {
    parse(
        r#"
[profiles.codex_side]
provider = "codex"
path = "/profiles/shared"

[profiles.claude_side]
provider = "anthropic"
path = "/profiles/shared"
"#,
    )
    .validate()
    .expect("two providers, two profiles");
}

/// A misspelled key is refused at parse rather than ignored: an entry that
/// silently does nothing reads as a profile that exists.
#[test]
fn an_unknown_key_is_refused() {
    let error = toml::from_str::<Config>(
        r#"
[profiles.work]
provider = "codex"
directory = "/profiles/work"
"#,
    )
    .expect_err("an unknown key is refused");

    assert!(error.to_string().contains("directory"), "was: {error}");
}

/// An entry whose provider is missing has no endpoint to be spent against, and
/// guessing one would send a credential to the wrong backend.
#[test]
fn a_profile_without_a_provider_is_refused() {
    toml::from_str::<Config>(
        r#"
[profiles.work]
path = "/profiles/work"
"#,
    )
    .expect_err("a provider is required");
}
