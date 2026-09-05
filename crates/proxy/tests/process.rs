//! `docs/api.md` §2.8 — `inspect`, and the parsing under it.
//!
//! The parsing is pure over the environment text, so most of this is a table
//! of the shapes the two platforms hand over. The CLI half drives the shipping
//! binary against a child this test started, because the reading of a real
//! process's environment is the one thing no unit test covers — and because
//! the promise that no token is printed is a promise about what the binary
//! writes to stdout, not about what a function returns.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proxenos::process::Launched;
use proxenos::process::read;

/// The value `exec --account` sets, on a daemon with no token.
#[test]
fn an_account_tag_names_the_account_the_turns_go_as() {
    let seen = read(
        "ANTHROPIC_BASE_URL=http://127.0.0.1:8787 ANTHROPIC_AUTH_TOKEN=proxenos-account:work-codex",
    );
    assert_eq!(
        seen,
        Launched {
            through: true,
            account: Some("work-codex".to_owned()),
            daemon: Some("http://127.0.0.1:8787".to_owned()),
        }
    );
}

/// A client-mode launch puts both in the one header the client offers, in
/// either order, separated by a space — which is also the space the
/// space-separated form splits on, so this is the case that shape has to
/// survive.
#[test]
fn a_tag_beside_a_token_is_read_in_either_order_and_the_token_is_dropped() {
    for value in [
        "proxenos-token:not-a-real-secret proxenos-account:spare",
        "proxenos-account:spare proxenos-token:not-a-real-secret",
    ] {
        let seen = read(&format!(
            "/opt/homebrew/bin/node --title agent ANTHROPIC_AUTH_TOKEN={value} \
             ANTHROPIC_BASE_URL=https://macbook.tailnet:8787 PATH=/usr/bin"
        ));
        assert_eq!(
            seen,
            Launched {
                through: true,
                account: Some("spare".to_owned()),
                daemon: Some("https://macbook.tailnet:8787".to_owned()),
            },
            "{value}"
        );
        assert!(
            !format!("{seen:?}").contains("not-a-real-secret"),
            "the token reaches no field of the answer: {seen:?}"
        );
        assert!(!seen.line(7).contains("not-a-real-secret"), "{value}");
    }
}

/// A launch with no `--account` and no token sets no auth token of its own:
/// the `env` payload's `unused` stands, beside the base URL that payload also
/// carries. That pair is the launch, and the sentinel alone is not.
#[test]
fn a_launch_without_an_account_is_through_as_whoever_is_serving() {
    assert_eq!(
        read("ANTHROPIC_BASE_URL=http://127.0.0.1:8787 ANTHROPIC_AUTH_TOKEN=unused"),
        Launched {
            through: true,
            account: None,
            daemon: Some("http://127.0.0.1:8787".to_owned()),
        }
    );
    assert_eq!(
        read("ANTHROPIC_AUTH_TOKEN=unused"),
        Launched::default(),
        "the sentinel with nothing to point at is not a launch through this daemon"
    );
}

#[test]
fn a_session_on_the_real_api_is_not_through_this_daemon() {
    assert_eq!(
        read("HOME=/Users/agent ANTHROPIC_AUTH_TOKEN=sk-ant-notarealkey PATH=/usr/bin"),
        Launched::default()
    );
    assert_eq!(read("HOME=/Users/agent PATH=/usr/bin"), Launched::default());
}

/// Linux hands the environment over NUL-separated, and the NUL form must not
/// be split on spaces: an account name may hold one, and a value that was
/// never multi-part is read whole (`ingress::parse_tags`).
#[test]
fn the_nul_separated_form_reads_the_same_answer() {
    let seen = read(
        "PATH=/usr/bin\0ANTHROPIC_AUTH_TOKEN=proxenos-account:work codex\0\
         ANTHROPIC_BASE_URL=http://127.0.0.1:8787\0",
    );
    assert_eq!(
        seen,
        Launched {
            through: true,
            account: Some("work codex".to_owned()),
            daemon: Some("http://127.0.0.1:8787".to_owned()),
        }
    );

    let with_token = read(
        "ANTHROPIC_AUTH_TOKEN=proxenos-token:not-a-real-secret proxenos-account:spare\0\
         ANTHROPIC_BASE_URL=http://127.0.0.1:8787\0",
    );
    assert_eq!(with_token.account.as_deref(), Some("spare"));
    assert!(!format!("{with_token:?}").contains("not-a-real-secret"));
}

#[test]
fn the_line_says_which_of_the_three_it_is() {
    assert_eq!(
        Launched {
            through: true,
            account: Some("work-codex".to_owned()),
            daemon: Some("http://127.0.0.1:8787".to_owned()),
        }
        .line(4242),
        "pid 4242: through proxenos as work-codex (http://127.0.0.1:8787)"
    );
    assert_eq!(
        Launched {
            through: true,
            account: None,
            daemon: Some("http://127.0.0.1:8787".to_owned()),
        }
        .line(4242),
        "pid 4242: through proxenos as the serving account (http://127.0.0.1:8787)"
    );
    assert_eq!(
        Launched::default().line(4242),
        "pid 4242: not through proxenos"
    );
}

/// A command line with no assignment after it is what `ps` prints for a
/// process that is not the caller's — not an error, and not an environment.
#[test]
fn a_command_line_alone_carries_no_environment() {
    assert!(!proxenos::process::carries_environment("/bin/sleep 30"));
    assert!(!proxenos::process::carries_environment(
        "node -e setTimeout(()=>{},1)"
    ));
    assert!(proxenos::process::carries_environment(
        "/bin/sh PATH=/usr/bin"
    ));
}

// The CLI half. A child of this test, so the environment being read is one
// this test set — and a child that is this binary rather than a platform one,
// because macOS hides a platform binary's environment from `ps` even for its
// owner (`/bin/sleep` shows its command and nothing else, measured).
#[cfg(unix)]
mod cli {
    const TOKEN: &str = "not-a-real-secret";

    /// A child that stays up until this test drops its stdin.
    ///
    /// `statusline` with no command reads stdin to EOF and prints what it read,
    /// and it never fails on a daemon that is not answering — which is exactly
    /// the shape a test wants from a process it only needs to exist.
    fn agent() -> std::process::Child {
        std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
            .arg("statusline")
            .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")
            .env(
                "ANTHROPIC_AUTH_TOKEN",
                format!("proxenos-token:{TOKEN} proxenos-account:spare"),
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the binary should run")
    }

    fn inspect(arguments: &[&str]) -> std::process::Output {
        std::process::Command::new(env!("CARGO_BIN_EXE_proxenos"))
            .arg("inspect")
            .args(arguments)
            .output()
            .expect("the binary should run")
    }

    #[test]
    fn it_reads_a_child_s_account_without_printing_its_token() {
        let mut child = agent();
        // The environment is read from the process, so it has to have got as
        // far as being one: on macOS `ps` answers about a pid the moment it
        // exists, but the spawn itself is what this waits on.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let pid = child.id().to_string();

        let json = inspect(&[&pid, "--json"]);
        let rendered = inspect(&[&pid]);

        let _ = child.stdin.take();
        let _ = child.wait();

        let stdout = String::from_utf8_lossy(&json.stdout).into_owned();
        assert!(json.status.success(), "inspect --json failed: {stdout}");
        let payload: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON document");
        assert_eq!(payload["pid"], child.id());
        assert_eq!(payload["through"], true);
        assert_eq!(payload["account"], "spare");
        assert_eq!(payload["daemon"], "http://127.0.0.1:1");
        assert!(
            !stdout.contains(TOKEN),
            "the token the child carries is never printed: {stdout}"
        );

        let line = String::from_utf8_lossy(&rendered.stdout).into_owned();
        assert!(rendered.status.success(), "inspect failed: {line}");
        assert_eq!(
            line.trim(),
            format!(
                "pid {}: through proxenos as spare (http://127.0.0.1:1)",
                child.id()
            )
        );
        assert!(!line.contains(TOKEN), "{line}");
    }

    #[test]
    fn a_pid_nobody_is_using_is_refused_rather_than_answered() {
        // A child that has already gone, so the pid is one this test knows is
        // free rather than one it guessed.
        let mut child = agent();
        let pid = child.id().to_string();
        let _ = child.stdin.take();
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(300));

        let output = inspect(&[&pid]);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            !output.status.success(),
            "a pid nobody is using is a refusal, not an answer: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            stderr.contains(&format!("no process {pid} is running")),
            "the refusal says which of the two things went wrong: {stderr}"
        );
    }
}
