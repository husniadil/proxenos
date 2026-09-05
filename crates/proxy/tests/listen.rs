//! `docs/api.md` §4 — `[listen]`, the address and the token that gates it.
//!
//! The refusal is the feature: a daemon bound past loopback with no token
//! hands every stored account to whoever can reach the port, and nothing about
//! a healthy `status` would say so.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use proxenos::config::Config;
use proxenos::config::ListenConfig;

/// A concrete address that is not loopback. A wildcard is refused (see below),
/// so every non-loopback case here names an address the way an operator has to.
const REACHABLE: &str = "10.11.12.13";

fn with_listen(listen: ListenConfig) -> Config {
    Config {
        listen,
        ..Config::default()
    }
}

/// The shipped posture, unchanged: loopback, and nothing demanded.
#[test]
fn the_default_is_loopback_with_no_token() {
    let listen = Config::default()
        .resolve_listen()
        .expect("the default binds");

    assert!(listen.is_loopback());
    assert_eq!(listen.token, None);
    assert_eq!(listen.address.to_string(), "127.0.0.1");
}

/// Beyond loopback with no token is refused before anything binds, and both
/// keys are named — an operator who set one is one key away from the posture
/// they meant.
#[test]
fn binding_past_loopback_without_a_token_is_refused_by_name() {
    let error = with_listen(ListenConfig {
        address: Some(REACHABLE.to_owned()),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .expect_err("a non-loopback address with no token should refuse");

    assert!(error.message.contains(REACHABLE), "{}", error.message);
    assert!(
        error.message.contains("listen.token_file"),
        "the refusal should name the preferred key: {}",
        error.message
    );
    assert!(
        error.message.contains("listen.token"),
        "the refusal should name the inline key: {}",
        error.message
    );
}

/// The same address with a token is exactly what this feature is for.
#[test]
fn binding_past_loopback_with_a_token_is_allowed() {
    let listen = with_listen(ListenConfig {
        address: Some(REACHABLE.to_owned()),
        token: Some("a-long-random-string".to_owned()),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .expect("an address with a token should bind");

    assert!(!listen.is_loopback());
    assert_eq!(listen.token.as_deref(), Some("a-long-random-string"));
}

/// A token in a file is the preferred form, and it is trimmed: an editor's
/// trailing newline is not part of the secret.
#[test]
fn a_token_file_is_read_and_trimmed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, "  the-secret\n").unwrap();
    restrict(&path);

    let listen = with_listen(ListenConfig {
        address: Some(REACHABLE.to_owned()),
        token_file: Some(path),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .expect("a 0600 token file should be read");

    assert_eq!(listen.token.as_deref(), Some("the-secret"));
}

/// A secret anybody on the machine can read is not a secret, and the failure
/// is otherwise silent: the daemon comes up and the token works.
#[cfg(unix)]
#[test]
fn a_group_readable_token_file_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, "the-secret").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let error = with_listen(ListenConfig {
        token_file: Some(path),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .expect_err("a readable token file should refuse");

    assert!(error.message.contains("0644"), "{}", error.message);
    assert!(error.message.contains("chmod 600"), "{}", error.message);
}

/// Two keys stating the token leaves the operator with two answers and no way
/// to tell which one the daemon took.
#[test]
fn a_token_stated_twice_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, "from-the-file").unwrap();
    restrict(&path);

    let error = with_listen(ListenConfig {
        token_file: Some(path),
        token: Some("inline".to_owned()),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .expect_err("two tokens should refuse");

    assert!(
        error.message.contains("listen.token_file") && error.message.contains("listen.token"),
        "{}",
        error.message
    );
}

/// An empty token is a mistake, not a token — and read as one it would open
/// the daemon while looking configured.
#[test]
fn an_empty_token_is_refused() {
    let error = with_listen(ListenConfig {
        token: Some("   ".to_owned()),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .expect_err("an empty token should refuse");

    assert!(error.message.contains("listen.token"), "{}", error.message);
}

/// An address that does not parse is refused by name rather than defaulted:
/// a typo silently falling back to loopback is a daemon nobody can reach and
/// no reason given.
#[test]
fn an_unparseable_address_is_refused_by_name() {
    let error = with_listen(ListenConfig {
        address: Some("macbook.tailnet".to_owned()),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .expect_err("a hostname is not an address");

    assert!(
        error.message.contains("macbook.tailnet"),
        "{}",
        error.message
    );
    assert!(error.message.contains("127.0.0.1"), "{}", error.message);
}

/// `[listen]` parses out of the file the operator actually writes.
#[test]
fn the_table_parses_from_toml() {
    let config: Config = toml::from_str(
        r#"
port = 8787
[listen]
address = "10.11.12.13"
token   = "a-long-random-string"
"#,
    )
    .expect("the table should parse");

    let listen = config.resolve_listen().expect("it should resolve");
    assert_eq!(listen.address.to_string(), "10.11.12.13");
    assert_eq!(listen.token.as_deref(), Some("a-long-random-string"));
}

/// The example ships the table commented out, so a copied file keeps the
/// loopback posture until somebody means otherwise.
#[test]
fn the_shipped_example_still_binds_loopback() {
    let config: Config = toml::from_str(proxenos::config::EXAMPLE).expect("the example parses");
    assert!(config.resolve_listen().unwrap().is_loopback());
}

fn restrict(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// **The two doors, as a plan.** A reachable address opens a second listener
/// beside the loopback one, and the token belongs to that second door. The
/// pair travels together so no caller can read the address without the token
/// that guards it.
#[test]
fn a_reachable_address_opens_a_remote_door_carrying_the_token() {
    let listen = with_listen(ListenConfig {
        address: Some(REACHABLE.to_owned()),
        token: Some("the-secret".to_owned()),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .unwrap();

    let (address, token) = listen.remote_door().expect("a reachable address opens one");
    assert_eq!(address.to_string(), REACHABLE);
    assert_eq!(token, "the-secret");
}

/// A loopback address opens one door and it is the loopback one. A token
/// written beside it demands nothing, because there is no door for it to
/// guard — the daemon says so at startup rather than leaving it silent.
#[test]
fn a_loopback_address_opens_no_remote_door_even_with_a_token() {
    let listen = with_listen(ListenConfig {
        token: Some("the-secret".to_owned()),
        ..ListenConfig::default()
    })
    .resolve_listen()
    .unwrap();

    assert!(listen.is_loopback());
    assert!(listen.remote_door().is_none());
    assert!(
        listen.token_guards_nothing(),
        "a token with no remote door should be reported, not ignored in silence"
    );
}

/// A wildcard address cannot be split into two doors, and is refused by name.
///
/// Measured on this machine: with `SO_REUSEADDR` the BSDs let `0.0.0.0:P` and
/// `127.0.0.1:P` both bind and hand a loopback connection to the more specific
/// socket, while Linux refuses the second bind outright. A posture that is one
/// thing on macOS and another on Linux is not a posture, so neither is relied
/// on: the operator writes the address they meant.
#[test]
fn a_wildcard_address_is_refused_by_name() {
    for wildcard in ["0.0.0.0", "::"] {
        let error = with_listen(ListenConfig {
            address: Some(wildcard.to_owned()),
            token: Some("the-secret".to_owned()),
            ..ListenConfig::default()
        })
        .resolve_listen()
        .expect_err("a wildcard should refuse");

        assert!(error.message.contains(wildcard), "{}", error.message);
        assert!(
            error.message.contains("127.0.0.1"),
            "it should say the loopback door is opened anyway: {}",
            error.message
        );
    }
}
