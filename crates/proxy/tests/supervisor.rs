//! `docs/api.md` §2.6 — the supervisor unit.
//!
//! Everything here is a pure function over data: the rendering of the unit, the
//! path decisions, and the refusal on a platform this cannot supervise. No test
//! writes into the real `~/Library/LaunchAgents`, contacts the network, or
//! touches a running daemon.

use proxenos::supervisor::Origin;
use proxenos::supervisor::Platform;
use proxenos::supervisor::plan;
use proxenos::supervisor::render;
use std::ffi::OsString;
use std::path::PathBuf;

fn origin() -> Origin {
    Origin {
        program: PathBuf::from("/Users/someone/.local/bin/proxenos"),
        log: PathBuf::from("/Users/someone/.config/proxenos/daemon.log"),
        proxenos_home: None,
        tmpdir: Some("/var/folders/j2/abcdef/T/".into()),
    }
}

/// A platform with neither supervisor refuses and says so by name, rather than
/// writing a file that supervises nothing. A half-installed unit that silently
/// never runs is worse than no verb at all, and the operator cannot tell the
/// two apart from the outside.
#[test]
fn a_platform_this_cannot_supervise_is_refused_by_name() {
    let error = plan(&Platform::Other("freebsd"), &origin()).unwrap_err();
    let message = error.to_string();

    assert!(message.contains("freebsd"), "{message}");
    assert!(
        message.contains("launchd") && message.contains("systemd"),
        "the refusal names both supervisors this implements: {message}"
    );
    assert!(
        message.contains("proxenos start"),
        "and names what to do meanwhile: {message}"
    );
}

/// What the supervised daemon's environment actually is.
///
/// **launchd does not hand a job an empty environment.** It supplies its own
/// `TMPDIR` — measured on this machine as byte-identical to the login shell's —
/// and what the unit carries goes on top of that rather than instead of it. A
/// test that models the daemon's environment as exactly what the unit carries
/// cannot express the failure this verb exists to catch, because the case that
/// fails is the one where the unit carries nothing and launchd fills the gap.
fn as_launchd_hands_it_over(
    unit: &proxenos::supervisor::Unit,
    supplied_tmpdir: &str,
) -> (Option<PathBuf>, Option<PathBuf>) {
    let carried = |key: &str| {
        unit.environment
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| PathBuf::from(value))
    };
    (
        carried("PROXENOS_HOME"),
        carried("TMPDIR").or_else(|| Some(PathBuf::from(supplied_tmpdir))),
    )
}

/// The hazard this verb exists to avoid: a supervised daemon binding one socket
/// path while the operator's CLI dials another. The daemon answers turns on the
/// port and every verb reports connection refused, which reads as a dead daemon
/// and is not one.
///
/// Every combination of the two inputs the path is derived from, against a
/// supervisor that supplies a `TMPDIR` of its own — deliberately not the
/// installing shell's, since a value that happened to match would prove nothing
/// about the case where they differ.
#[test]
fn the_socket_the_unit_binds_is_the_one_the_cli_dials() {
    const SUPPLIED_BY_LAUNCHD: &str = "/var/folders/zz/launchd-supplied/T/";

    for home in [None, Some(OsString::from("/Users/someone/px"))] {
        for tmpdir in [None, Some(OsString::from("/var/folders/j2/abcdef/T/"))] {
            let origin = Origin {
                proxenos_home: home.clone(),
                tmpdir: tmpdir.clone(),
                ..origin()
            };
            let unit = plan(&Platform::MacOs, &origin).unwrap();

            // What the supervised daemon derives, from the environment launchd
            // actually gives it.
            let (daemon_home, daemon_tmpdir) = as_launchd_hands_it_over(&unit, SUPPLIED_BY_LAUNCHD);
            let bound =
                proxenos::control::path_for(daemon_home.as_deref(), daemon_tmpdir.as_deref());

            // What the installing shell's own CLI dials.
            let dialed = proxenos::control::path_for(
                home.as_ref().map(PathBuf::from).as_deref(),
                tmpdir.as_ref().map(PathBuf::from).as_deref(),
            );

            assert_eq!(
                bound,
                dialed,
                "home {home:?}, tmpdir {tmpdir:?}: the daemon binds {} and the CLI dials {}",
                bound.display(),
                dialed.display()
            );
            assert_eq!(
                unit.socket, bound,
                "home {home:?}, tmpdir {tmpdir:?}: the unit reports a socket it does not bind"
            );
            assert!(
                unit.environment.iter().any(|(key, _)| key == "TMPDIR"),
                "home {home:?}, tmpdir {tmpdir:?}: the unit must carry the TMPDIR the \
                 derivation used, or launchd supplies one of its own and the two ends drift"
            );
        }
    }
}

/// A `TMPDIR` deep enough to push the socket past the platform's address limit
/// fails at bind, after the HTTP listener is already up — the daemon looks
/// healthy and every CLI verb gets connection refused. It is refused here, where
/// the path is chosen, rather than there.
#[test]
fn a_socket_path_the_platform_cannot_address_is_refused() {
    let deep = format!("/var/folders/{}/T/", "a".repeat(120));
    let origin = Origin {
        tmpdir: Some(deep.into()),
        ..origin()
    };

    let message = plan(&Platform::MacOs, &origin).unwrap_err().to_string();
    assert!(message.contains("unix socket address"), "{message}");
}

/// launchd resolves nothing: a relative program in a plist is a unit that never
/// starts, reported nowhere.
#[test]
fn a_program_launchd_cannot_resolve_is_refused() {
    let origin = Origin {
        program: PathBuf::from("target/release/proxenos"),
        ..origin()
    };

    let message = plan(&Platform::MacOs, &origin).unwrap_err().to_string();
    assert!(message.contains("absolute"), "{message}");
}

/// The unit runs `run` in the foreground and keeps it alive. Not `start`:
/// a process that forks away leaves launchd supervising something that has
/// already exited, and the respawn loop then fights the daemon it cannot see.
#[test]
fn the_unit_runs_the_daemon_in_the_foreground_and_keeps_it_alive() {
    let unit = plan(&Platform::MacOs, &origin()).unwrap();
    let document = render(&unit);

    assert_eq!(unit.arguments, vec!["run".to_owned()]);
    assert!(!document.contains("<string>start</string>"), "{document}");
    assert!(
        document.contains("<key>KeepAlive</key>\n    <true/>"),
        "{document}"
    );
    assert!(
        document.contains("<key>RunAtLoad</key>\n    <true/>"),
        "{document}"
    );
    assert!(
        document.contains("/Users/someone/.local/bin/proxenos"),
        "{document}"
    );
}

/// It logs where the daemon already logs, so a supervised start and a background
/// one leave one file to read rather than two.
#[test]
fn the_unit_logs_where_the_daemon_already_logs() {
    let unit = plan(&Platform::MacOs, &origin()).unwrap();
    let document = render(&unit);

    assert_eq!(
        unit.log,
        PathBuf::from("/Users/someone/.config/proxenos/daemon.log")
    );
    assert!(
        document.contains("<key>StandardOutPath</key>\n    <string>/Users/someone/.config/proxenos/daemon.log</string>"),
        "{document}"
    );
    assert!(
        document.contains("<key>StandardErrorPath</key>\n    <string>/Users/someone/.config/proxenos/daemon.log</string>"),
        "{document}"
    );
}

/// A plist in the user's home is a world-readable file. The store holds
/// credentials; this holds none, and the set it may carry is closed rather than
/// filtered, so a later addition has to be made deliberately.
#[test]
fn the_unit_carries_no_credential() {
    let origin = Origin {
        proxenos_home: Some("/Users/someone/px".into()),
        ..origin()
    };
    let unit = plan(&Platform::MacOs, &origin).unwrap();

    let keys: Vec<&str> = unit
        .environment
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    assert_eq!(keys, vec!["PROXENOS_HOME", "TMPDIR"]);

    let document = render(&unit).to_ascii_lowercase();
    for forbidden in ["token", "secret", "api_key", "apikey", "password", "auth"] {
        assert!(
            !document.contains(forbidden),
            "the unit must carry no credential, found {forbidden}"
        );
    }
}

/// A path with an XML metacharacter in it renders as data rather than as markup.
#[test]
fn a_path_carrying_markup_is_escaped() {
    let origin = Origin {
        log: PathBuf::from("/Users/a&b/<x>/daemon.log"),
        ..origin()
    };
    let document = render(&plan(&Platform::MacOs, &origin).unwrap());

    assert!(
        document.contains("/Users/a&amp;b/&lt;x&gt;/daemon.log"),
        "{document}"
    );
    assert!(!document.contains("/Users/a&b/"), "{document}");
}

/// The installed unit is compared against the one this environment would write,
/// as text. A `TMPDIR` that has moved since install renders a different
/// document, and that is precisely the case where the daemon binds one socket
/// path and the operator's CLI dials another — healthy on the port, connection
/// refused on every verb. Saying so is the whole point of the `status` verb.
#[test]
fn a_unit_installed_from_a_different_environment_is_reported_as_divergent() {
    let wanted = plan(&Platform::MacOs, &origin()).unwrap();

    let elsewhere = Origin {
        tmpdir: Some("/var/folders/zz/999999/T/".into()),
        ..origin()
    };
    let stale = render(&plan(&Platform::MacOs, &elsewhere).unwrap());

    assert_eq!(
        proxenos::supervisor::compare(None, &wanted),
        proxenos::supervisor::Installed::Absent
    );
    assert_eq!(
        proxenos::supervisor::compare(Some(&render(&wanted)), &wanted),
        proxenos::supervisor::Installed::Current
    );
    assert_eq!(
        proxenos::supervisor::compare(Some(&stale), &wanted),
        proxenos::supervisor::Installed::Divergent
    );
}

/// `install` never asked what was already answering, so an operator who had
/// started the daemon by hand got "supervising ..." for a job that could not
/// take the port: `run` refuses a busy one, and the supervisor respawns it into
/// the same refusal every ten seconds. The install is real either way — the unit
/// is written and accepted — so what is owed is an accurate report, naming what
/// holds the port and how to hand it over.
///
/// It does not stop that daemon. This verb installs a supervisor; ending a
/// process the operator started by hand was not asked for.
#[test]
fn a_daemon_holding_the_port_is_named_and_nothing_is_said_when_none_is() {
    let notice = proxenos::supervisor::port_notice(Some("0.8.0"))
        .expect("a daemon holding the port must be reported");
    assert!(notice.contains("0.8.0"), "{notice}");
    assert!(notice.contains("proxenos stop"), "{notice}");

    assert_eq!(proxenos::supervisor::port_notice(None), None);
}

/// The regression, and the reason the notice needs a timing rule rather than
/// just a message. A reinstall — the ordinary case, a new build or a moved
/// binary — boots out the unit it had already installed. The daemon answering a
/// moment before that is the one this verb ends, and the first version of this
/// feature named it: the operator was told to hand over a port already theirs,
/// for a job that then started fine.
///
/// The middle two cases are the whole finding. They hand in the same `before`
/// and differ only in whether this install freed the port itself.
#[test]
fn only_a_daemon_this_install_does_not_end_holds_the_port() {
    use proxenos::supervisor::holder;

    // Nothing installed before, nothing answering: the normal path.
    assert_eq!(holder(false, None, None), None);

    // Nothing installed before, something answering: started by hand, and this
    // verb frees nothing, so it is still there when the job starts.
    assert_eq!(holder(false, Some("0.8.0"), None), Some("0.8.0"));

    // A unit was installed, so the bootout ended what was answering. Same
    // `before` as above; naming it is what regressed.
    assert_eq!(holder(true, Some("0.8.0"), None), None);

    // A unit was installed AND something is still answering after the bootout —
    // a hand-started daemon holding the port while the supervised job crash
    // loops behind it. That one this verb did not end.
    assert_eq!(holder(true, Some("0.7.0"), Some("0.8.0")), Some("0.8.0"));
}

/// Whether the daemon can tell it was started by the supervisor.
///
/// launchd names the job in `XPC_SERVICE_NAME`; a shell passes down `0`, or
/// nothing. A third label is neither answer: something else launchd knows
/// about started this process, and "not supervised by proxenos.daemon" is not
/// the same statement as "nothing supervises this". `status` prints the line
/// only where there is an answer.
#[test]
fn supervision_is_read_from_the_job_label_and_never_guessed() {
    use proxenos::supervisor::Platform;
    use proxenos::supervisor::supervised;

    assert_eq!(
        supervised(&Platform::MacOs, Some(proxenos::supervisor::LABEL)),
        Some(true)
    );
    assert_eq!(supervised(&Platform::MacOs, None), Some(false));
    assert_eq!(supervised(&Platform::MacOs, Some("0")), Some(false));
    assert_eq!(supervised(&Platform::MacOs, Some("")), Some(false));
    assert_eq!(
        supervised(&Platform::MacOs, Some("com.example.other")),
        None
    );

    // No supervisor on this platform, so nothing here can answer for it.
    assert_eq!(
        supervised(&Platform::Other("linux"), Some(proxenos::supervisor::LABEL)),
        None
    );
}

// ---------------------------------------------------------------------------
// The systemd user service. Same plan, same environment, same socket — a
// different file format and a different tool to hand it to.
// ---------------------------------------------------------------------------

/// The unit systemd is handed, in full.
///
/// A rendering asserted key by key passes while the file is unloadable, because
/// what makes a unit work is as much its sections as its settings: a
/// `Restart=always` outside `[Service]` is a parse error, and an `[Install]`
/// section that is missing takes `enable` with it. So the whole document is the
/// assertion.
#[test]
fn the_systemd_unit_is_a_user_service_that_always_comes_back() {
    let document = render(&plan(&Platform::Linux, &origin()).unwrap());

    assert_eq!(
        document,
        "[Unit]\n\
         Description=proxenos daemon\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart=\"/Users/someone/.local/bin/proxenos\" \"run\"\n\
         Environment=\"TMPDIR=/var/folders/j2/abcdef/T/\"\n\
         Restart=always\n\
         RestartSec=5\n\
         StandardOutput=append:/Users/someone/.config/proxenos/daemon.log\n\
         StandardError=append:/Users/someone/.config/proxenos/daemon.log\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );
}

/// It runs `run` in the foreground, for the reason the launchd unit does: a
/// process that forks away leaves the supervisor watching something that has
/// already exited, and the respawn then fights the daemon it cannot see.
#[test]
fn the_systemd_unit_runs_the_daemon_in_the_foreground() {
    let unit = plan(&Platform::Linux, &origin()).unwrap();
    let document = render(&unit);

    assert_eq!(unit.arguments, vec!["run".to_owned()]);
    assert!(!document.contains("\"start\""), "{document}");
    assert!(document.contains("Type=simple"), "{document}");
    assert!(!document.contains("Type=forking"), "{document}");
}

/// `%` introduces a specifier to systemd, not a character. A home directory
/// holding one would be substituted at load time — `%u` is the user name, `%h`
/// the home directory — and the daemon would bind a socket nobody dials, which
/// is this verb's whole failure mode arrived at through the file format.
///
/// A space is the other one: an unquoted `ExecStart` splits on it into a
/// program that does not exist and an argument nobody passed.
#[test]
fn a_systemd_path_carrying_a_specifier_or_a_space_survives_the_rendering() {
    let origin = Origin {
        program: PathBuf::from("/Users/some one/bin/100% proxenos"),
        log: PathBuf::from("/Users/some one/log/daemon.log"),
        ..origin()
    };
    let document = render(&plan(&Platform::Linux, &origin).unwrap());

    assert!(
        document.contains("ExecStart=\"/Users/some one/bin/100%% proxenos\" \"run\""),
        "{document}"
    );
    assert!(
        document.contains("StandardOutput=append:/Users/some one/log/daemon.log"),
        "{document}"
    );
}

/// The unit file is a world-readable file in the user's home, the same as the
/// plist, and the environment it may carry is the same closed set of two.
#[test]
fn the_systemd_unit_carries_no_credential() {
    let origin = Origin {
        proxenos_home: Some("/Users/someone/px".into()),
        ..origin()
    };
    let unit = plan(&Platform::Linux, &origin).unwrap();
    let document = render(&unit);

    let keys: Vec<&str> = unit
        .environment
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    assert_eq!(keys, vec!["PROXENOS_HOME", "TMPDIR"]);
    assert!(
        document.contains("Environment=\"PROXENOS_HOME=/Users/someone/px\"")
            && document.contains("Environment=\"TMPDIR=/var/folders/j2/abcdef/T/\""),
        "{document}"
    );

    let lowered = document.to_ascii_lowercase();
    for forbidden in ["token", "secret", "api_key", "apikey", "password", "auth"] {
        assert!(
            !lowered.contains(forbidden),
            "the unit must carry no credential, found {forbidden}"
        );
    }
    // `EnvironmentFile=` would let a later edit point the unit at a file of
    // secrets and have systemd read it in. Nothing here writes one.
    assert!(!document.contains("EnvironmentFile"), "{document}");
}

/// The socket hazard, on the other platform. `systemd --user` supplies no
/// `TMPDIR` at all where the unit names none, so the daemon would fall back to
/// `/tmp` while the operator's shell went on dialing its own — the same drift
/// the launchd unit carries `TMPDIR` to close, and closed the same way.
#[test]
fn the_systemd_unit_binds_the_socket_the_cli_dials() {
    for home in [None, Some(OsString::from("/home/someone/px"))] {
        for tmpdir in [None, Some(OsString::from("/tmp/mine"))] {
            let origin = Origin {
                proxenos_home: home.clone(),
                tmpdir: tmpdir.clone(),
                ..origin()
            };
            let unit = plan(&Platform::Linux, &origin).unwrap();

            let carried = |key: &str| {
                unit.environment
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| PathBuf::from(value))
            };
            let bound = proxenos::control::path_for(
                carried("PROXENOS_HOME").as_deref(),
                carried("TMPDIR").as_deref(),
            );
            let dialed = proxenos::control::path_for(
                home.as_ref().map(PathBuf::from).as_deref(),
                tmpdir.as_ref().map(PathBuf::from).as_deref(),
            );

            assert_eq!(bound, dialed, "home {home:?}, tmpdir {tmpdir:?}");
            assert_eq!(unit.socket, bound, "home {home:?}, tmpdir {tmpdir:?}");
        }
    }
}

/// systemd reads `XDG_CONFIG_HOME` and nothing else, so the unit goes under the
/// same base the rest of this project resolves its configuration from — and
/// under `systemd/user`, which is the only directory a *user* manager loads.
#[test]
fn the_systemd_unit_is_written_where_a_user_manager_looks() {
    assert_eq!(
        proxenos::supervisor::unit_path(std::path::Path::new("/home/someone/.config")),
        PathBuf::from("/home/someone/.config/systemd/user/proxenos.service")
    );
    // Named for the unit type, not for the launchd label: systemd reads the
    // extension, and `proxenos.daemon` would be a unit of a type that does not
    // exist.
    assert!(proxenos::supervisor::SERVICE.ends_with(".service"));
}

/// A relative program is refused on both platforms: systemd requires an
/// absolute `ExecStart` exactly as launchd resolves nothing.
#[test]
fn a_program_systemd_cannot_resolve_is_refused() {
    let origin = Origin {
        program: PathBuf::from("target/release/proxenos"),
        ..origin()
    };

    let message = plan(&Platform::Linux, &origin).unwrap_err().to_string();
    assert!(message.contains("absolute"), "{message}");
}

/// Divergence is the same comparison on both platforms, and it is the one that
/// catches an environment that has moved since install.
#[test]
fn a_systemd_unit_installed_from_a_different_environment_is_divergent() {
    use proxenos::supervisor::Installed;
    use proxenos::supervisor::compare;

    let wanted = plan(&Platform::Linux, &origin()).unwrap();
    let elsewhere = Origin {
        tmpdir: Some("/tmp/elsewhere".into()),
        ..origin()
    };
    let stale = render(&plan(&Platform::Linux, &elsewhere).unwrap());

    assert_eq!(compare(None, &wanted), Installed::Absent);
    assert_eq!(compare(Some(&render(&wanted)), &wanted), Installed::Current);
    assert_eq!(compare(Some(&stale), &wanted), Installed::Divergent);

    // And a plist is not a service. The two platforms render the same plan into
    // documents that must never compare equal, or a machine that changed
    // supervisors would report `current` for a file the supervisor cannot read.
    let plist = render(&plan(&Platform::MacOs, &origin()).unwrap());
    assert_eq!(compare(Some(&plist), &wanted), Installed::Divergent);
}

/// What `status` reports comes from `systemctl --user show`, and the trap is
/// that `show` answers for a unit it has never heard of in exactly the shape of
/// one that is installed and stopped. `LoadState=not-found` is what separates
/// them, and where it says so nothing is reported rather than `inactive` —
/// which would be a state word for a unit systemd does not have.
#[test]
fn systemd_status_is_read_from_show_and_never_invented() {
    use proxenos::supervisor::parse_systemd_show;

    assert_eq!(
        parse_systemd_show(
            "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=4711\n"
        ),
        (Some("active (running)".to_owned()), Some(4711))
    );

    // Stopped: a real unit with no process. `MainPID=0` is systemd's word for
    // "none", not a pid.
    assert_eq!(
        parse_systemd_show("LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\n"),
        (Some("inactive (dead)".to_owned()), None)
    );

    // Crash looping, which is what an install over a held port looks like. The
    // sub-state is the half that says so, and it is why both are reported.
    assert_eq!(
        parse_systemd_show(
            "LoadState=loaded\nActiveState=activating\nSubState=auto-restart\nMainPID=0\n"
        ),
        (Some("activating (auto-restart)".to_owned()), None)
    );

    // Not installed. Same fields, same shape, and no state is the honest read.
    assert_eq!(
        parse_systemd_show("LoadState=not-found\nActiveState=inactive\nSubState=dead\nMainPID=0\n"),
        (None, None)
    );

    // `show` prints its properties in whatever order it likes, and a version
    // that prints fewer of them says nothing rather than guessing.
    assert_eq!(
        parse_systemd_show("MainPID=12\nSubState=running\nLoadState=loaded\nActiveState=active\n"),
        (Some("active (running)".to_owned()), Some(12))
    );
    assert_eq!(parse_systemd_show(""), (None, None));
}

/// The Linux half of the same question, which no environment variable answers.
///
/// systemd names no unit in a started process's environment, so the reading is
/// two facts: `INVOCATION_ID` says a unit started this at all, and the
/// manager's `MainPID` for `proxenos.service` says whether that unit is this
/// one. Absent, `false` — nothing this manager runs started the process, and
/// systemd sets the id for everything it executes. Present but belonging to
/// some other unit's main process, `false` as well: a terminal emulator is
/// itself a user unit and every shell under it inherits the id, so presence
/// alone would report the operator's own shell-started daemon as supervised.
/// Unreachable manager, `null` — the one case nothing established.
#[test]
fn systemd_supervision_takes_the_manager_at_its_word_or_says_nothing() {
    use proxenos::supervisor::MainPid;
    use proxenos::supervisor::Platform;
    use proxenos::supervisor::supervised_by_systemd;

    // The supervised daemon: a unit started it, and it is this unit's main
    // process.
    assert_eq!(
        supervised_by_systemd(
            &Platform::Linux,
            Some("6b6a1a5f"),
            MainPid::Reported(Some(4711)),
            4711
        ),
        Some(true)
    );

    // Started from a shell: no unit executed this process at all, so nothing
    // is asked of the manager and the answer is a real negative.
    assert_eq!(
        supervised_by_systemd(&Platform::Linux, None, MainPid::Unreachable, 4711),
        Some(false)
    );
    assert_eq!(
        supervised_by_systemd(&Platform::Linux, Some(""), MainPid::Unreachable, 4711),
        Some(false)
    );

    // Started from a terminal that is itself a user unit: the id is inherited
    // and says nothing about this unit, and the pid it names is somebody
    // else's.
    assert_eq!(
        supervised_by_systemd(
            &Platform::Linux,
            Some("6b6a1a5f"),
            MainPid::Reported(Some(22)),
            4711
        ),
        Some(false)
    );

    // The unit systemd has never heard of, or has heard of and is not running:
    // either way it did not start this process.
    assert_eq!(
        supervised_by_systemd(
            &Platform::Linux,
            Some("6b6a1a5f"),
            MainPid::Reported(None),
            4711
        ),
        Some(false)
    );

    // A unit started this and the manager could not be asked which. The one
    // unanswerable case, and answering `false` from it would report an
    // unsupervised daemon on the strength of an unreachable bus.
    assert_eq!(
        supervised_by_systemd(
            &Platform::Linux,
            Some("6b6a1a5f"),
            MainPid::Unreachable,
            4711
        ),
        None
    );

    // No systemd on this platform, so this reading has no standing at all —
    // the mirror of `supervised` refusing to answer for a platform without
    // launchd.
    assert_eq!(
        supervised_by_systemd(
            &Platform::MacOs,
            Some("6b6a1a5f"),
            MainPid::Reported(Some(4711)),
            4711
        ),
        None
    );
}
