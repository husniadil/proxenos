//! `docs/api.md` §2.6 — the supervisor unit.
//!
//! Everything the unit *is* — the platform decision, the socket path, the
//! rendering — is a pure function over data named here rather than read from
//! the process, so a unit that would supervise nothing is refused in a test
//! rather than discovered by an operator whose daemon never came back.
//!
//! Two supervisors are implemented, and which one a platform has is an input
//! rather than a `cfg`: a per-user LaunchAgent on macOS, a systemd **user**
//! service on Linux. They differ in their file format and in the commands that
//! load them, and in nothing else — the same plan, over the same environment,
//! producing the same socket.
//!
//! Installing and removing it is the only part that touches the machine, and
//! that lives with the rest of the I/O.

use crate::control;
use crate::error::ProxyError;
use std::ffi::OsString;
use std::path::PathBuf;

/// What the supervised job is called, to launchd and to the operator.
pub const LABEL: &str = "proxenos.daemon";

/// What it is called to systemd, where a unit's name carries its type.
///
/// Not `proxenos.daemon`: systemd reads the extension as the unit type, so a
/// file named for the launchd label would be a unit of type `daemon`, which
/// does not exist.
pub const SERVICE: &str = "proxenos.service";

/// How long systemd waits before bringing the daemon back.
///
/// Five seconds rather than systemd's 100ms default, and the reason is the
/// start limit rather than politeness: five restarts inside ten seconds put a
/// unit into `failed` and stop the supervision entirely, which is precisely
/// the state an operator installs this to avoid. The case that hits it is real
/// — `run` refuses a port another daemon holds and exits at once, so a daemon
/// started by hand makes the supervised job a tight crash loop. At five
/// seconds the loop stays under the default burst forever, and the throttle is
/// the same order as launchd's own ten.
pub const RESTART_SEC: u32 = 5;

/// Which supervisor a platform has, as an input rather than a `cfg`.
///
/// A `cfg` would make the refusal untestable on the machine that ships it,
/// which is exactly backwards: the refusal is the branch nobody exercises by
/// accident, so it is the one that most needs a test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Other(&'static str),
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other(std::env::consts::OS)
        }
    }
}

/// Which supervisor a unit is written for, carried by the unit itself.
///
/// The rendering, the file it goes in, and the commands that load it all
/// follow from this one value, so a caller cannot render a plist and hand it
/// to systemd by taking the wrong branch somewhere further down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A per-user LaunchAgent under `~/Library/LaunchAgents`.
    LaunchdAgent,
    /// A systemd **user** service under `$XDG_CONFIG_HOME/systemd/user`.
    ///
    /// User, never system: a system unit needs root, runs as another user, and
    /// would bind a socket in a home directory this daemon's operator does not
    /// own. Nothing here escalates to it.
    SystemdUserService,
}

/// What the installing shell saw, handed in whole.
#[derive(Debug, Clone)]
pub struct Origin {
    /// This binary, as launchd will have to resolve it.
    pub program: PathBuf,
    /// Where the daemon already logs.
    pub log: PathBuf,
    /// `PROXENOS_HOME`, if the operator's environment names one.
    pub proxenos_home: Option<OsString>,
    /// `TMPDIR`, if it names one.
    pub tmpdir: Option<OsString>,
}

/// A unit that would supervise this daemon, before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// Which supervisor this unit is for, and therefore how it renders.
    pub kind: Kind,
    pub label: String,
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub log: PathBuf,
    /// Exactly what the job's environment carries, and a closed set: adding to
    /// it is a deliberate edit, not a filter someone can widen by accident.
    pub environment: Vec<(String, String)>,
    /// The socket the supervised daemon will bind, derived from what the
    /// environment above hands it.
    pub socket: PathBuf,
}

/// Where the launchd unit is written, given the user's home directory.
pub fn plist_path(home: &std::path::Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// Where the systemd unit is written, given the XDG configuration base.
///
/// The base is `XDG_CONFIG_HOME`, or `~/.config` where nothing names one —
/// resolved the way the rest of this project resolves configuration
/// (`config::xdg_config_home`). `PROXENOS_HOME` does not move it: systemd
/// reads `XDG_CONFIG_HOME` and nothing else, so a unit written anywhere else
/// would be a file no supervisor ever loads.
pub fn unit_path(config_home: &std::path::Path) -> PathBuf {
    config_home.join("systemd/user").join(SERVICE)
}

/// What to read when a systemd-supervised daemon will not start.
///
/// journald holds what the unit itself failed with — an `ExecStart` that could
/// not be spawned never reaches the daemon's own log file, because there is no
/// daemon yet to write it.
pub fn journal_command() -> String {
    format!("journalctl --user -u {SERVICE}")
}

/// Decide what would be installed, or refuse and say why.
pub fn plan(platform: &Platform, origin: &Origin) -> Result<Unit, ProxyError> {
    let kind = match platform {
        Platform::MacOs => Kind::LaunchdAgent,
        Platform::Linux => Kind::SystemdUserService,
        Platform::Other(name) => {
            return Err(ProxyError::invalid_request(format!(
                "there is no supervisor for {name} here: launchd on macOS and a systemd user \
                 service on Linux are the two this implements, and neither is a {name} thing. \
                 Nothing writes a file it cannot hand to a supervisor, because a unit that is \
                 written but never runs is worse than no verb: it reports success and supervises \
                 nothing. Start the daemon with `proxenos start` and supervise it with whatever \
                 this platform already has."
            )));
        }
    };

    if !origin.program.is_absolute() {
        return Err(ProxyError::invalid_request(format!(
            "the supervisor needs an absolute path to this binary and was given {}. Neither \
             supervisor resolves one — launchd has no `PATH` and no working directory, and \
             systemd requires an absolute `ExecStart` — so a relative program is a job that never \
             starts and says so nowhere.",
            origin.program.display()
        )));
    }

    let home = origin.proxenos_home.as_ref().map(PathBuf::from);

    // **Resolved, never left absent.** `TMPDIR` is carried whether or not the
    // installing shell names one, because launchd does not hand a job an empty
    // environment: it supplies a `TMPDIR` of its own. Carrying nothing would
    // therefore not mean "no `TMPDIR`" to the daemon — it would mean launchd's,
    // while the path planned here fell back to `/tmp` and the operator's CLI
    // went on dialing whatever its own shell says. That is the failure this
    // whole verb exists to prevent, arrived at from the inside. Carrying the
    // value the derivation actually used is what closes it.
    let tmpdir = origin
        .tmpdir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| control::FALLBACK_TMPDIR.into());

    let socket = control::path_for(home.as_deref(), Some(&tmpdir));
    control::ensure_addressable(&socket)?;

    // Carried explicitly rather than inherited. A launchd job's environment is
    // not the login shell's, and the socket path is derived from both of these
    // — so naming them is what makes the daemon's bind and the operator's dial
    // the same path by construction rather than by luck.
    let mut environment = Vec::new();
    if let Some(home) = &home {
        environment.push(("PROXENOS_HOME".to_owned(), home.display().to_string()));
    }
    environment.push(("TMPDIR".to_owned(), tmpdir.display().to_string()));

    Ok(Unit {
        kind,
        label: match kind {
            Kind::LaunchdAgent => LABEL.to_owned(),
            Kind::SystemdUserService => SERVICE.to_owned(),
        },
        program: origin.program.clone(),
        // Foreground. A job that forks away leaves the supervisor watching a
        // process that has already exited, and its respawn then fights the
        // daemon it cannot see. `Type=simple` says the same thing to systemd
        // that the absence of a fork says to launchd.
        arguments: vec!["run".to_owned()],
        log: origin.log.clone(),
        environment,
        socket,
    })
}

/// Render the unit in the format its supervisor reads.
pub fn render(unit: &Unit) -> String {
    match unit.kind {
        Kind::LaunchdAgent => render_plist(unit),
        Kind::SystemdUserService => render_service(unit),
    }
}

/// Render the unit as a systemd user service.
///
/// **Logs to the same file the daemon already writes**, rather than to
/// journald alone. journald is where a systemd operator looks first and is not
/// being taken away — the unit's own failures land there and nowhere else, and
/// `install` says so — but the daemon's log is read by `start`, by `stop`, and
/// by an operator following any sentence this CLI prints about it. Splitting it
/// by supervisor would leave two files to read on Linux and one on macOS for
/// the same daemon, and would make `log` in `status --json` a path on one
/// platform and a command on the other. `append:` keeps the file the one thing
/// both starts write to.
fn render_service(unit: &Unit) -> String {
    let mut document = String::from(
        // No `After=`/`Wants=`. The obvious `network-online.target` does not
        // exist in the **user** manager — a user unit naming it gets a
        // dependency systemd cannot load — and the daemon needs nothing from
        // it anyway: it binds loopback, and a reachable second door is refused
        // at startup rather than waited for.
        "[Unit]\nDescription=proxenos daemon\n\n[Service]\nType=simple\n",
    );

    document.push_str("ExecStart=");
    document.push_str(&quote(&unit.program.display().to_string()));
    for argument in &unit.arguments {
        document.push(' ');
        document.push_str(&quote(argument));
    }
    document.push('\n');

    for (key, value) in &unit.environment {
        document.push_str(&format!(
            "Environment={}\n",
            quote(&format!("{key}={value}"))
        ));
    }

    let log = escape_specifiers(&unit.log.display().to_string());
    document.push_str(&format!(
        "Restart=always\nRestartSec={RESTART_SEC}\nStandardOutput=append:{log}\n\
         StandardError=append:{log}\n\n[Install]\nWantedBy=default.target\n"
    ));
    document
}

/// A systemd unit value, as one quoted word.
///
/// Quoted rather than bare because a path may contain a space, and an
/// `ExecStart` split on one is a program that does not exist with an argument
/// nobody passed.
fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        escape_specifiers(value)
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

/// `%` is a specifier introducer to systemd, not a character.
///
/// A home directory holding one — `%u` is the user name, `%h` the home
/// directory — would otherwise be substituted at load time into some other
/// path, quietly, and the daemon would bind a socket nobody dialed. `%%` is
/// the literal.
fn escape_specifiers(value: &str) -> String {
    value.replace('%', "%%")
}

/// Render the unit as a launchd property list.
fn render_plist(unit: &Unit) -> String {
    let mut document = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST \
         1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist \
         version=\"1.0\">\n<dict>\n",
    );

    document.push_str(&format!(
        "    <key>Label</key>\n    <string>{}</string>\n",
        escape(&unit.label)
    ));

    document.push_str("    <key>ProgramArguments</key>\n    <array>\n");
    document.push_str(&format!(
        "        <string>{}</string>\n",
        escape(&unit.program.display().to_string())
    ));
    for argument in &unit.arguments {
        document.push_str(&format!("        <string>{}</string>\n", escape(argument)));
    }
    document.push_str("    </array>\n");

    if !unit.environment.is_empty() {
        document.push_str("    <key>EnvironmentVariables</key>\n    <dict>\n");
        for (key, value) in &unit.environment {
            document.push_str(&format!(
                "        <key>{}</key>\n        <string>{}</string>\n",
                escape(key),
                escape(value)
            ));
        }
        document.push_str("    </dict>\n");
    }

    let log = escape(&unit.log.display().to_string());
    document.push_str(&format!(
        "    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    \
         <true/>\n    <key>StandardOutPath</key>\n    <string>{log}</string>\n    \
         <key>StandardErrorPath</key>\n    <string>{log}</string>\n"
    ));

    document.push_str("</dict>\n</plist>\n");
    document
}

/// XML text, not markup. A home directory may legally contain `&`.
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Whether this process was started by the supervisor, where that is knowable.
///
/// launchd hands a job it starts an `XPC_SERVICE_NAME` naming that job's label,
/// and a process started from a shell inherits whatever the shell had — `0`
/// under a terminal, or nothing at all. So the label this verb installs is a
/// positive answer, and the absence of any label is a negative one.
///
/// **A third label is `None`, not `false`.** Something else launchd knows about
/// started this process, and "not supervised by `proxenos.daemon`" and "nothing
/// supervises this" are different statements. Reporting the second where only
/// the first is established is the kind of plausible answer that reads as a
/// measurement. `None` on every platform with no supervisor here, for the same
/// reason.
///
/// **`None` on Linux too, and that is a gap rather than a decision about
/// systemd.** systemd puts no unit name into the environment of the process it
/// starts: `INVOCATION_ID` says *a* unit started this and never which one, so
/// answering `false` from its absence would be a claim about every other
/// supervisor on the machine, and answering `true` from its presence would
/// name this unit on the strength of some other unit's evidence. Both are the
/// plausible-answer failure this function exists to refuse. Reading it
/// honestly would take asking the manager — `systemctl --user show -p MainPID`
/// against this pid — which is I/O, and this is not where I/O goes.
#[must_use]
pub fn supervised(platform: &Platform, xpc_service_name: Option<&str>) -> Option<bool> {
    if !matches!(platform, Platform::MacOs) {
        return None;
    }
    match xpc_service_name {
        Some(label) if label == LABEL => Some(true),
        // launchd's own placeholder, and what a login shell passes down.
        None | Some("") | Some("0") => Some(false),
        Some(_) => None,
    }
}

/// What is on disk, against what this environment would write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    Absent,
    Current,
    /// Installed, and not the unit this environment produces.
    ///
    /// The case worth naming: `TMPDIR` moving between install and now changes
    /// the socket the supervised daemon binds while the operator's CLI goes on
    /// dialing the old one. The daemon answers turns on the port and every verb
    /// reports connection refused, which reads as a dead daemon and is not one.
    /// A text comparison catches it without parsing anything back.
    Divergent,
}

/// Compare what is installed against what would be installed now.
pub fn compare(existing: Option<&str>, wanted: &Unit) -> Installed {
    match existing {
        None => Installed::Absent,
        Some(document) if document == render(wanted) => Installed::Current,
        Some(_) => Installed::Divergent,
    }
}

/// What `install` says about a daemon that will still hold the port.
///
/// The supervised job runs `run`, and `run` refuses a port another daemon
/// holds. So an install performed while a hand-started daemon is up is a real
/// install of a job that cannot start yet: launchd respawns it into the same
/// refusal until the port is free. The install is not undone by that and the
/// daemon is not stopped by this verb — ending a process the operator started
/// by hand is not what was asked for — so what is owed is the observation and
/// the way to hand over. Under a supervisor, `stop` is that way (§2.4).
///
/// **The argument is what is answering once the verb has released what it
/// controls, not what was answering when the operator typed it.** A reinstall
/// boots out the unit it had already installed, and a daemon that this verb is
/// about to end is not one the operator has to hand over: reporting it would
/// name a port that is already theirs, for a job that then starts fine.
///
/// `None` when nothing is holding it, which covers both the plain install and
/// the reinstall, and neither gains output.
pub fn port_notice(still_answering: Option<&str>) -> Option<String> {
    let version = still_answering?;
    Some(format!(
        "  {version} is already answering, so the supervised job cannot take the port yet\n  \
         hand it over with `proxenos stop`; the supervisor starts this build in its place"
    ))
}

/// Which observation the port notice may name, if any.
///
/// The notice's own decision is easy and was never wrong. What went wrong is
/// *when* the caller reads: a reinstall boots out the unit it had already
/// installed, so a daemon answering before that is one this verb ends itself,
/// and naming it tells the operator to hand over a port already theirs for a
/// job that then starts fine.
///
/// So the rule is about which read counts. When this install booted a unit out
/// it freed the port itself, and only what is *still* answering afterwards is a
/// daemon it did not end. When it booted nothing out there was no window and
/// nothing was freed, so the read taken at the start is already the truth and
/// `after` is not taken at all.
pub fn holder<'a>(
    booted_out: bool,
    before: Option<&'a str>,
    after: Option<&'a str>,
) -> Option<&'a str> {
    if booted_out { after } else { before }
}

/// What systemd says about the unit, from `systemctl --user show`.
///
/// Parsed rather than trusted line-for-line: `show` prints `KEY=value` pairs
/// in whatever order it likes, and prints them for a unit it has never heard
/// of just as readily as for one it runs. That second half is the trap. An
/// unknown unit answers `ActiveState=inactive`, `SubState=dead`, `MainPID=0`,
/// which reads exactly like an installed unit that is stopped — a state word
/// for a unit systemd does not have. `LoadState=not-found` is what tells the
/// two apart, and where it says so this reports **no state at all**, the same
/// silence launchd's `print` produces for a label it does not know.
///
/// The state word is `active (running)` — systemd's own two-part phrasing,
/// since either half alone loses a distinction the other carries: `active` does
/// not separate `running` from `exited`, and `running` does not separate a
/// healthy unit from one systemd is restarting.
#[must_use]
pub fn parse_systemd_show(printed: &str) -> (Option<String>, Option<u64>) {
    let field = |key: &str| {
        printed.lines().find_map(|line| {
            line.trim()
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::trim)
        })
    };

    if field("LoadState") == Some("not-found") {
        return (None, None);
    }

    let state = field("ActiveState").map(|active| match field("SubState") {
        Some(sub) if !sub.is_empty() && sub != active => format!("{active} ({sub})"),
        _ => active.to_owned(),
    });
    // `MainPID=0` is systemd's word for "no process", not for pid zero.
    let pid = field("MainPID")
        .and_then(|pid| pid.parse::<u64>().ok())
        .filter(|pid| *pid != 0);
    (state, pid)
}
