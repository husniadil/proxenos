//! Command line surface. The verb set is semver-bound — see `docs/api.md` §6.

use clap::Parser;
use clap::Subcommand;

/// Run Claude Code against models served over a ChatGPT subscription.
#[derive(Debug, Parser)]
// The build id rides along: `--version` is what an operator reads to answer
// "is the binary on disk the one I just built", and a version number alone
// cannot answer it.
#[command(name = "proxenos", version = proxenos::version::build(), about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the daemon in the background and return once it answers.
    ///
    /// A verb of its own rather than a flag on `run`, because backgrounding is
    /// what an operator asks for and holding the terminal is what a supervisor
    /// asks for. `stop` is the pair of this one, and was the pair of neither
    /// while the two lived under one name.
    Start(StartArgs),
    /// Start the daemon in the foreground, holding the terminal.
    Run(RunArgs),
    /// Stored accounts: list them, add one, choose one, drop one.
    Accounts(AccountsArgs),
    /// Connection, tier mapping, and whether the catalog was reachable.
    Status(StatusArgs),
    /// Available models.
    Models(ModelsArgs),
    /// Environment for Claude Code, as shell exports.
    Env,
    /// The same configuration as one client settings document.
    ///
    /// A separate verb rather than a flag on `env`, because it produces
    /// something an environment is not: it carries the client policy no export
    /// can hold. It is the only name for that document: `env --json` printed
    /// it too, which made one flag mean "the payload behind this verb" on
    /// three verbs and "a different verb's document" on the fourth.
    Settings,
    /// Re-read config.toml into the running daemon.
    ///
    /// A verb of its own rather than a flag on `run`, because it is asked of a
    /// daemon that is already serving. It says what it applied and what still
    /// needs a restart: a reload that reported only success would leave an
    /// operator believing a key took effect that cannot.
    Reload,
    /// Ask the running daemon to stop.
    ///
    /// Named for what it asks, not for what follows: whether anything starts
    /// the daemon again belongs to whatever supervises it. What happened is
    /// reported from what was observed afterwards.
    Stop,
    /// The tier mapping: read it, or point one tier at a model.
    ///
    /// The socket has carried `tiers` and `tiers.set` since v0.1, and until
    /// this verb the only door onto them was the daemon's own front-end.
    /// Every other program that wants to change a mapping on a running
    /// daemon is a caller of this CLI, not of the socket, and a method with
    /// no verb is a method it cannot reach.
    Tiers(TiersArgs),
    /// The effort ceiling: read it, or set it.
    ///
    /// The pair of `tiers`, for the other setting a running daemon can be
    /// handed without a restart.
    Effort(EffortArgs),
    /// Run a command with this proxy's configuration applied.
    ///
    /// Named for what it does rather than what it launches: a launcher that
    /// only ever starts one program is a launcher that cannot start the next
    /// one.
    Exec(ExecArgs),
    /// Probe backend capabilities.
    Doctor(DoctorArgs),
    /// What quota is left: what the last turn reported, or what asking finds.
    Usage(UsageArgs),
    /// Wrap a status-line script, adding the quota the client cannot supply.
    Statusline(StatuslineArgs),
    /// Capture exchanges as fixtures.
    Record(RecordArgs),
    /// Install, remove, or inspect the supervisor that keeps the daemon alive.
    ///
    /// Named for what supervises rather than for launchd, because the one
    /// platform this implements is not the only one it will be asked about, and
    /// a verb named after an implementation cannot grow a second one.
    Supervisor(SupervisorArgs),
}

#[derive(Debug, clap::Args)]
pub struct TiersArgs {
    #[command(subcommand)]
    pub action: Option<TiersAction>,
    /// Print the socket's own payload instead of the table.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum TiersAction {
    /// Point one tier at a model. The default is to read the mapping.
    ///
    /// One tier per call, because a set is partial: naming one tier changes
    /// that tier and no other. The model is validated against the catalog of
    /// the account being changed, and a model that catalog does not carry is
    /// refused rather than written. `--as ACCOUNT` pins the tier: its turns
    /// are made as that account whatever serves the rest, which needs the
    /// operator's consent (`cross_account_tiers`) and is refused without it.
    Set(SetTierArgs),
    /// Grant or revoke consent for pinned tiers: `on` or `off`.
    ///
    /// Always written to config.toml, unlike a set: consent is the operator
    /// changing what the daemon is. `off` is refused while any tier still
    /// pins an account, since the file would then refuse the next start.
    CrossAccount(CrossAccountArgs),
}

#[derive(Debug, clap::Args)]
pub struct CrossAccountArgs {
    /// on, or off.
    #[arg(value_parser = ["on", "off"])]
    pub state: String,
}

#[derive(Debug, clap::Args)]
pub struct SetTierArgs {
    /// The tier: opus, sonnet, haiku, or fable.
    pub tier: String,
    /// The model id the tier resolves to.
    pub model: String,
    /// Write that account's own section instead of the shared table. An
    /// account that is not serving turns takes the write and applies nothing,
    /// which needs --persist to mean anything.
    #[arg(long)]
    pub account: Option<String>,
    /// Pin the tier: make its turns as this stored account, whatever account
    /// serves the rest of the session. The model is then that account's to
    /// offer, and its quota is the one spent.
    #[arg(long = "as", value_name = "ACCOUNT")]
    pub as_account: Option<String>,
    /// Grant the consent a pin needs in the same breath, written to
    /// config.toml before the pin is set. A consent already given is left
    /// alone. Without this, a pin on a daemon that lacks it is refused,
    /// naming this flag.
    #[arg(long, requires = "as_account")]
    pub allow_cross_account: bool,
    /// Write the change into config.toml as well. Without it the change lasts
    /// until the daemon stops, and the answer says which it was.
    #[arg(long)]
    pub persist: bool,
}

#[derive(Debug, clap::Args)]
pub struct EffortArgs {
    #[command(subcommand)]
    pub action: Option<EffortAction>,
    /// Print the socket's own payload instead of the line.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum EffortAction {
    /// Set the ceiling, or remove it with `none`. The default is to read it.
    Set(SetEffortArgs),
}

#[derive(Debug, clap::Args)]
pub struct SetEffortArgs {
    /// low, medium, high, or none to remove the ceiling. Under an account,
    /// none removes that account's override and the shared ceiling applies
    /// again; the answer reports the ceiling that results.
    pub level: String,
    /// Write that account's own section instead of the shared line.
    #[arg(long)]
    pub account: Option<String>,
    /// Write the change into config.toml as well.
    #[arg(long)]
    pub persist: bool,
}

#[derive(Debug, clap::Args)]
pub struct SupervisorArgs {
    #[command(subcommand)]
    pub action: SupervisorAction,
    /// Print what `status` found as one JSON document instead of the report.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum SupervisorAction {
    /// Write the unit for this user and hand it to the supervisor.
    Install,
    /// Remove it. The daemon it was supervising stops with it.
    Uninstall,
    /// Whether it is installed, and what the supervisor makes of it.
    Status,
}

/// The account verbs.
///
/// One sub-verb per thing an operator does, each naming its account
/// positionally. The surface before this used flags as actions — `--use`,
/// `--forget` and `--rename` on one struct, and a `login` whose two unrelated
/// halves were told apart by `--key` and `--profile` — so what a command did
/// was decided by which flags were present, and the account it did it to was
/// spelled `--as` in one verb and `--use` in another. Three words for one
/// thing is three things to remember.
#[derive(Debug, clap::Args)]
pub struct AccountsArgs {
    #[command(subcommand)]
    pub action: Option<AccountsAction>,
    /// Print the socket's own payload instead of the table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum AccountsAction {
    /// Stored accounts, and which one serves turns. The default.
    List(ListArgs),
    /// Sign in to a new profile of the owning program, and declare it.
    ///
    /// Runs that program's own login against a directory this daemon then
    /// borrows the grant from. Nothing here sees a token: the client
    /// authenticates and writes, and this side reads the profile afterwards
    /// and writes the `[profiles]` entry naming it. **This daemon obtains no
    /// subscription grant of its own** — there is no authorization flow here
    /// and no callback port.
    Login(AccountLoginArgs),
    /// Store an API key, read from stdin.
    ///
    /// The secret arrives on stdin and never as an argument: an argument is
    /// visible to every process on the machine and lands in shell history.
    AddKey(AddKeyArgs),
    /// Serve every following turn as this account.
    Use(NamedArgs),
    /// Change what an account is called here, leaving its grant alone.
    Rename(RenameArgs),
    /// Remove this account, leaving the rest usable.
    Remove(NamedArgs),
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    /// Print the socket's own payload instead of the table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct AccountLoginArgs {
    /// What to call the account this login produces.
    pub name: String,
    /// Which program's profile this is, and therefore which endpoint the
    /// grant inside it is spent against.
    ///
    /// Required rather than defaulted. A login that silently chose one signs
    /// in to a program the operator may not have meant, and the wrong answer
    /// is only found out later, from an account that cannot serve.
    #[arg(long, value_enum)]
    pub provider: proxenos::auth::store::Provider,
    /// Where the new profile lives.
    ///
    /// Absent, it goes under this daemon's own directory. A path given here is
    /// what gets declared, so it is also how an existing profile directory —
    /// one another tool made — is signed into and adopted.
    #[arg(long, value_name = "DIR")]
    pub path: Option<std::path::PathBuf>,
    /// Have the client print a URL and a code instead of opening a browser.
    ///
    /// For a machine with no browser to open — a container, or a session over
    /// ssh. It is `codex login --device-auth`, and `--provider anthropic`
    /// refuses it: `claude auth login` has no equivalent.
    #[arg(long)]
    pub device_auth: bool,
    /// Sign a profile this file already declares back in.
    ///
    /// For a grant that has lapsed. Without it a declared name is refused,
    /// because declaring it twice leaves a file the daemon cannot start from;
    /// with it the name has to be declared already, the provider has to be the
    /// one it is declared as, and the directory is the declaration's — so
    /// `--path` is refused, and nothing is written afterwards.
    #[arg(long)]
    pub relogin: bool,
}

#[derive(Debug, clap::Args)]
pub struct AddKeyArgs {
    /// What to call the account this key becomes.
    ///
    /// Required, because a key carries no id to be named by.
    pub name: String,
    /// Whose endpoints the stored key is spent against.
    ///
    /// Required rather than defaulted: the two providers refuse each other's
    /// credentials, and a key that silently claimed the wrong one fails as an
    /// authentication error naming the credential rather than the choice.
    #[arg(long, value_enum)]
    pub provider: proxenos::auth::store::Provider,
}

#[derive(Debug, clap::Args)]
pub struct NamedArgs {
    /// The account this is about.
    pub name: String,
}

#[derive(Debug, clap::Args)]
pub struct RenameArgs {
    /// The account as it is called now.
    #[arg(value_name = "OLD")]
    pub from: String,
    /// What to call it instead.
    #[arg(value_name = "NEW")]
    pub to: String,
}

#[derive(Debug, clap::Args)]
pub struct RunArgs {
    /// Port to bind on loopback. Overrides the configured value.
    #[arg(long, env = "PROXENOS_PORT")]
    pub port: Option<u16>,
}

#[derive(Debug, clap::Args)]
pub struct StartArgs {
    /// Port to bind on loopback. Overrides the configured value.
    #[arg(long, env = "PROXENOS_PORT")]
    pub port: Option<u16>,
}

#[derive(Debug, clap::Args)]
pub struct StatusArgs {
    /// Print the socket's own payload instead of the report.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct ModelsArgs {
    /// Print the socket's own payload instead of the table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct ExecArgs {
    /// The stored account this session's turns are made as, without changing
    /// which account serves everything else.
    ///
    /// Consumed here and never forwarded: the child's argv gains nothing, and
    /// the name travels as the auth token value the client already sends. An
    /// unknown name is refused before anything starts.
    #[arg(long)]
    pub account: Option<String>,
    /// The program to start, and everything to hand it.
    ///
    /// Opaque from the program name onward, so the client's own flags keep
    /// working unchanged. `--` is accepted for a command whose first argument
    /// would otherwise be read as this verb's.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Run one probe by name instead of the whole suite.
    #[arg(long)]
    pub probe: Option<String>,
    /// Where the fixture corpus lives.
    #[arg(long)]
    pub fixtures: Option<std::path::PathBuf>,
    /// Answer the probes from the real backend instead of the recordings.
    ///
    /// This spends inference quota — one turn per probe — and needs
    /// credentials. Without it nothing is contacted and nothing is billed.
    #[arg(long)]
    pub live: bool,
    /// Which stored account the relay probe (§9) is authorized as.
    ///
    /// Only needed where the store holds more than one account on the second
    /// provider; with exactly one there is nothing to choose. The account
    /// serving turns is never read for this and never changed.
    #[arg(long)]
    pub relay_account: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct UsageArgs {
    /// Emit the raw snapshot, for a status line or a script.
    #[arg(long)]
    pub json: bool,
    /// Ask the backend for a figure per account before reporting.
    ///
    /// Costs one request per account that can be asked, so it is what an
    /// operator opts into. Without it nothing is asked and the daemon reports
    /// what it already holds, which is the cheap default and stays that way.
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Debug, clap::Args)]
pub struct StatuslineArgs {
    /// The status-line command to run, after `--`.
    ///
    /// Its stdin is the client's payload with the quota merged in, and its
    /// output is passed through untouched. Omit it to print the merged payload
    /// instead, which is what a script that would rather pipe it wants.
    #[arg(last = true)]
    pub command: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct RecordArgs {
    #[command(subcommand)]
    pub mode: RecordMode,
}

/// The two capture modes differ in what they cost: ingress needs no credentials
/// and spends nothing, upstream needs both.
///
/// Each mode runs a daemon, so each carries the daemon's port control. Declared
/// here rather than assembled in the handler, because a declared binding is the
/// only kind clap actually runs: the first version built the run arguments by
/// hand and silently dropped `PROXENOS_PORT` on this verb alone.
#[derive(Debug, Subcommand)]
pub enum RecordMode {
    /// Capture what the client sends, before translation.
    Ingress {
        /// Port to bind on loopback. Overrides the configured value.
        #[arg(long, env = "PROXENOS_PORT")]
        port: Option<u16>,
    },
    /// Capture what the backend sends back.
    Upstream {
        /// Port to bind on loopback. Overrides the configured value.
        #[arg(long, env = "PROXENOS_PORT")]
        port: Option<u16>,
    },
    /// Capture the real Messages surface: a short fixed set of exchanges made
    /// against the second provider's endpoint, written as conformance
    /// fixtures.
    ///
    /// No daemon and no client. The other two modes wait for a client to send
    /// something; this one makes the calls, because what is wanted is a
    /// handful of known shapes rather than whatever a session happens to send.
    /// It spends one turn per exchange.
    Surface {
        /// The stored account to spend, which must be on the second provider.
        /// Named rather than defaulted: spending the wrong subscription is not
        /// recoverable, and the selected account is usually the other one.
        #[arg(long)]
        account: String,
        /// Where the fixtures go. Defaults to `fixtures/surface` under the
        /// working directory, which is where the suite reads them from.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Capture one named exchange instead of all of them. A capture on
        /// disk is quota already spent, and adding a shape to the corpus is
        /// not a reason to pay for the ones already there.
        #[arg(long)]
        only: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn reload_parses_as_a_verb_of_its_own() {
        let cli = Cli::try_parse_from(["proxenos", "reload"]).unwrap();
        assert!(matches!(cli.command, Command::Reload));
    }

    #[test]
    fn stop_parses_as_a_verb_of_its_own() {
        let cli = Cli::try_parse_from(["proxenos", "stop"]).unwrap();
        assert!(matches!(cli.command, Command::Stop));
    }

    /// The mapping is read bare and set with `set TIER MODEL`; `--json` is
    /// accepted on either side of the sub-verb, since a caller printing the
    /// payload does not care which verb it is on.
    #[test]
    fn tiers_reads_bare_and_sets_one_tier() {
        let cli = Cli::try_parse_from(["proxenos", "tiers", "--json"]).unwrap();
        let Command::Tiers(args) = cli.command else {
            panic!("tiers should parse");
        };
        assert!(args.action.is_none());
        assert!(args.json);

        let cli = Cli::try_parse_from([
            "proxenos",
            "tiers",
            "set",
            "opus",
            "gpt-5.6-sol",
            "--account",
            "spare",
            "--persist",
            "--json",
        ])
        .unwrap();
        let Command::Tiers(args) = cli.command else {
            panic!("tiers set should parse");
        };
        assert!(args.json);
        let Some(TiersAction::Set(set)) = args.action else {
            panic!("set should parse");
        };
        assert_eq!(
            (set.tier.as_str(), set.model.as_str()),
            ("opus", "gpt-5.6-sol")
        );
        assert_eq!(set.account.as_deref(), Some("spare"));
        assert!(set.persist);

        assert!(Cli::try_parse_from(["proxenos", "tiers", "set", "opus"]).is_err());
    }

    /// A pin is `--as ACCOUNT`; consent rides along as a flag that means
    /// nothing without a pin, and consent on its own is a sub-verb.
    #[test]
    fn a_pin_is_as_and_consent_needs_one() {
        let cli = Cli::try_parse_from([
            "proxenos",
            "tiers",
            "set",
            "haiku",
            "gpt-5.6-luna",
            "--as",
            "spare",
            "--allow-cross-account",
        ])
        .unwrap();
        let Command::Tiers(TiersArgs {
            action: Some(TiersAction::Set(set)),
            ..
        }) = cli.command
        else {
            panic!("tiers set should parse");
        };
        assert_eq!(set.as_account.as_deref(), Some("spare"));
        assert!(set.allow_cross_account);

        assert!(
            Cli::try_parse_from([
                "proxenos",
                "tiers",
                "set",
                "haiku",
                "x",
                "--allow-cross-account"
            ])
            .is_err(),
            "consent without a pin is a flag that does nothing, and is refused"
        );

        let cli = Cli::try_parse_from(["proxenos", "tiers", "cross-account", "off"]).unwrap();
        let Command::Tiers(TiersArgs {
            action: Some(TiersAction::CrossAccount(c)),
            ..
        }) = cli.command
        else {
            panic!("cross-account should parse");
        };
        assert_eq!(c.state, "off");
        assert!(Cli::try_parse_from(["proxenos", "tiers", "cross-account", "maybe"]).is_err());
    }

    #[test]
    fn supervisor_status_takes_json() {
        let cli = Cli::try_parse_from(["proxenos", "supervisor", "status", "--json"]).unwrap();
        let Command::Supervisor(args) = cli.command else {
            panic!("supervisor should parse");
        };
        assert!(args.json);
        assert!(matches!(args.action, SupervisorAction::Status));
    }

    #[test]
    fn effort_reads_bare_and_sets_a_level() {
        let cli = Cli::try_parse_from(["proxenos", "effort"]).unwrap();
        let Command::Effort(args) = cli.command else {
            panic!("effort should parse");
        };
        assert!(args.action.is_none());

        let cli = Cli::try_parse_from(["proxenos", "effort", "set", "none", "--persist"]).unwrap();
        let Command::Effort(args) = cli.command else {
            panic!("effort set should parse");
        };
        let Some(EffortAction::Set(set)) = args.action else {
            panic!("set should parse");
        };
        assert_eq!(set.level, "none");
        assert!(set.persist);
    }

    /// Backgrounding is a verb, and the flag that used to spell it is gone
    /// with no alias. A flag still accepted here would start a daemon the
    /// documentation no longer describes.
    #[test]
    fn start_is_a_verb_and_detach_is_not_a_flag() {
        let cli = Cli::try_parse_from(["proxenos", "start"]).unwrap();
        let Command::Start(args) = cli.command else {
            panic!("start should parse");
        };
        assert_eq!(args.port, None);

        assert!(Cli::try_parse_from(["proxenos", "run", "--detach"]).is_err());
        assert!(Cli::try_parse_from(["proxenos", "start", "--detach"]).is_err());
    }

    /// Both daemon verbs take the same port control, because either one is
    /// how a daemon gets started.
    #[test]
    fn start_and_run_take_the_same_port() {
        let cli = Cli::try_parse_from(["proxenos", "start", "--port", "18799"]).unwrap();
        let Command::Start(args) = cli.command else {
            panic!("start should parse");
        };
        assert_eq!(args.port, Some(18799));

        let cli = Cli::try_parse_from(["proxenos", "run", "--port", "18799"]).unwrap();
        let Command::Run(args) = cli.command else {
            panic!("run should parse");
        };
        assert_eq!(args.port, Some(18799));
    }

    /// The secret is never an argument. A command line is visible to every
    /// process on the machine and lands in shell history, so there is nowhere
    /// on it to put a key.
    #[test]
    fn add_key_takes_a_name_and_never_the_secret() {
        let cli = Cli::try_parse_from([
            "proxenos",
            "accounts",
            "add-key",
            "billing",
            "--provider",
            "codex",
        ])
        .unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::AddKey(args)) = args.action else {
            panic!("add-key should parse");
        };
        assert_eq!(args.name, "billing");
        assert_eq!(args.provider, proxenos::auth::store::Provider::Codex);

        // A second positional is the only place a secret could go, and there
        // is no second positional.
        assert!(
            Cli::try_parse_from([
                "proxenos",
                "accounts",
                "add-key",
                "billing",
                "--provider",
                "codex",
                "sk-secret",
            ])
            .is_err()
        );
    }

    /// Both account-adding verbs state their provider. Neither defaults: the
    /// two providers refuse each other's credentials, and a silent default is
    /// found out from an account that cannot serve.
    #[test]
    fn adding_an_account_states_its_provider() {
        assert!(Cli::try_parse_from(["proxenos", "accounts", "add-key", "relay"]).is_err());
        assert!(Cli::try_parse_from(["proxenos", "accounts", "login", "work"]).is_err());

        let cli = Cli::try_parse_from([
            "proxenos",
            "accounts",
            "login",
            "work",
            "--provider",
            "anthropic",
        ])
        .unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::Login(args)) = args.action else {
            panic!("login should parse");
        };
        assert_eq!(args.name, "work");
        assert_eq!(args.provider, proxenos::auth::store::Provider::Anthropic);
        assert_eq!(args.path, None);

        // A provider this proxy has no path for is refused at the boundary
        // rather than stored and discovered on the first turn.
        assert!(
            Cli::try_parse_from([
                "proxenos",
                "accounts",
                "add-key",
                "x",
                "--provider",
                "gemini",
            ])
            .is_err()
        );
    }

    /// A profile login names where the profile goes, which is also how a
    /// directory another tool made is adopted.
    #[test]
    fn a_profile_login_can_name_its_directory() {
        let cli = Cli::try_parse_from([
            "proxenos",
            "accounts",
            "login",
            "work",
            "--provider",
            "codex",
            "--path",
            "/profiles/work",
        ])
        .unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::Login(args)) = args.action else {
            panic!("login should parse");
        };
        assert_eq!(
            args.path.as_deref(),
            Some(std::path::Path::new("/profiles/work"))
        );
        assert!(!args.device_auth);
    }

    /// `--device-auth` is how a login finishes where there is no browser to
    /// open. It is off unless it is asked for, because the browser flow is
    /// fewer steps everywhere it works.
    #[test]
    fn a_profile_login_can_ask_for_the_flow_that_needs_no_browser() {
        let cli = Cli::try_parse_from([
            "proxenos",
            "accounts",
            "login",
            "work",
            "--provider",
            "codex",
            "--device-auth",
        ])
        .unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::Login(args)) = args.action else {
            panic!("login should parse");
        };
        assert!(args.device_auth);
    }

    /// `--relogin` says the profile is one the file already declares, whose
    /// grant has lapsed. It is off unless it is asked for: a login that
    /// silently re-signed a declared name would hide the misspelling that is
    /// the other reason to have typed one.
    #[test]
    fn a_profile_login_can_ask_to_sign_a_declared_profile_in_again() {
        let cli = Cli::try_parse_from([
            "proxenos",
            "accounts",
            "login",
            "work",
            "--provider",
            "codex",
            "--relogin",
        ])
        .unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::Login(args)) = args.action else {
            panic!("login should parse");
        };
        assert!(args.relogin);
        assert_eq!(args.path, None);

        let cli = Cli::try_parse_from([
            "proxenos",
            "accounts",
            "login",
            "work",
            "--provider",
            "codex",
        ])
        .unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::Login(args)) = args.action else {
            panic!("login should parse");
        };
        assert!(!args.relogin);
    }

    /// Top-level `login` is gone with the flag pair that told its two halves
    /// apart, and the parser is where that has to be true: a verb still
    /// accepted here would take an operator as far as a refusal from somewhere
    /// else, about something else. `--setup-token` went the same way, with the
    /// flow behind it.
    #[test]
    fn the_top_level_login_verb_is_gone() {
        assert!(Cli::try_parse_from(["proxenos", "login", "--key", "--as", "billing"]).is_err());
        assert!(Cli::try_parse_from(["proxenos", "login", "--profile", "--as", "work"]).is_err());
        assert!(Cli::try_parse_from(["proxenos", "login", "--setup-token"]).is_err());
    }

    /// Renaming takes both halves. One of them missing would leave the
    /// command guessing which account it was about.
    #[test]
    fn accounts_renames_with_both_halves_or_not_at_all() {
        let cli = Cli::try_parse_from(["proxenos", "accounts", "rename", "old", "new"]).unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::Rename(args)) = args.action else {
            panic!("rename should parse");
        };
        assert_eq!(args.from, "old");
        assert_eq!(args.to, "new");

        assert!(Cli::try_parse_from(["proxenos", "accounts", "rename", "old"]).is_err());
        assert!(Cli::try_parse_from(["proxenos", "accounts", "rename"]).is_err());
    }

    /// Removing names its account. An account is gone once this returns, and
    /// the operator naming which one is the whole safeguard.
    #[test]
    fn accounts_removes_only_the_account_it_is_given() {
        let cli = Cli::try_parse_from(["proxenos", "accounts", "remove", "spare"]).unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::Remove(args)) = args.action else {
            panic!("remove should parse");
        };
        assert_eq!(args.name, "spare");

        // A bare `remove` has no default target.
        assert!(Cli::try_parse_from(["proxenos", "accounts", "remove"]).is_err());
    }

    /// Listing is the default, and switching is a verb of its own: a bare
    /// `accounts` that switched on a stray argument would bill a turn to the
    /// wrong account.
    #[test]
    fn accounts_lists_by_default_and_switches_only_when_asked() {
        let cli = Cli::try_parse_from(["proxenos", "accounts"]).unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        assert!(args.action.is_none());
        assert!(!args.json);

        let cli = Cli::try_parse_from(["proxenos", "accounts", "use", "work"]).unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::Use(args)) = args.action else {
            panic!("use should parse");
        };
        assert_eq!(args.name, "work");

        // A bare name is not a switch, and never was.
        assert!(Cli::try_parse_from(["proxenos", "accounts", "work"]).is_err());
    }

    /// The listing is reachable by name as well as by default, and `--json`
    /// belongs to both spellings — a flag that worked on only one of them
    /// would be a flag an operator has to guess about.
    #[test]
    fn the_listing_takes_json_under_either_spelling() {
        let cli = Cli::try_parse_from(["proxenos", "accounts", "--json"]).unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        assert!(args.json);
        assert!(args.action.is_none());

        let cli = Cli::try_parse_from(["proxenos", "accounts", "list", "--json"]).unwrap();
        let Command::Accounts(args) = cli.command else {
            panic!("accounts should parse");
        };
        let Some(AccountsAction::List(args)) = args.action else {
            panic!("list should parse");
        };
        assert!(args.json);
    }

    /// The settings document has one name. `env` renders shell exports and
    /// nothing else, so `--json` means the same thing on every verb that takes
    /// it: the socket's own payload for that verb.
    #[test]
    fn settings_parses_as_a_verb_of_its_own_and_env_takes_no_json() {
        let cli = Cli::try_parse_from(["proxenos", "settings"]).unwrap();
        assert!(matches!(cli.command, Command::Settings));

        let cli = Cli::try_parse_from(["proxenos", "env"]).unwrap();
        assert!(matches!(cli.command, Command::Env));

        assert!(Cli::try_parse_from(["proxenos", "env", "--json"]).is_err());
    }

    /// The two read-only verbs that rendered a payload and could not hand it
    /// over. `--json` is the same flag it is on `accounts` and `usage`.
    #[test]
    fn status_and_models_take_json() {
        let cli = Cli::try_parse_from(["proxenos", "status", "--json"]).unwrap();
        let Command::Status(args) = cli.command else {
            panic!("status should parse");
        };
        assert!(args.json);

        let cli = Cli::try_parse_from(["proxenos", "status"]).unwrap();
        let Command::Status(args) = cli.command else {
            panic!("status should parse");
        };
        assert!(!args.json);

        let cli = Cli::try_parse_from(["proxenos", "models", "--json"]).unwrap();
        let Command::Models(args) = cli.command else {
            panic!("models should parse");
        };
        assert!(args.json);

        let cli = Cli::try_parse_from(["proxenos", "models"]).unwrap();
        let Command::Models(args) = cli.command else {
            panic!("models should parse");
        };
        assert!(!args.json);
    }

    /// Everything after the program name belongs to the child, hyphens and
    /// all. A launcher that swallowed one would make the thing it wraps
    /// undebuggable.
    #[test]
    fn exec_forwards_everything_after_the_program() {
        let cli =
            Cli::try_parse_from(["proxenos", "exec", "claude", "--resume", "abc", "-p"]).unwrap();
        let Command::Exec(args) = cli.command else {
            panic!("expected exec");
        };
        assert_eq!(args.command, ["claude", "--resume", "abc", "-p"]);
    }

    /// `--account` is this verb's, consumed before the program name and never
    /// forwarded: the child's argv gains nothing, and the same spelling after
    /// the program name still belongs to the child.
    #[test]
    fn exec_consumes_its_own_account_flag() {
        let cli = Cli::try_parse_from([
            "proxenos",
            "exec",
            "--account",
            "personal",
            "claude",
            "--model",
            "haiku",
        ])
        .unwrap();
        let Command::Exec(args) = cli.command else {
            panic!("expected exec");
        };
        assert_eq!(args.account.as_deref(), Some("personal"));
        assert_eq!(args.command, ["claude", "--model", "haiku"]);

        let cli =
            Cli::try_parse_from(["proxenos", "exec", "some-tool", "--account", "theirs"]).unwrap();
        let Command::Exec(args) = cli.command else {
            panic!("expected exec");
        };
        assert_eq!(args.account, None);
        assert_eq!(args.command, ["some-tool", "--account", "theirs"]);
    }

    /// `--` for the command whose own first argument would otherwise be read
    /// here. The separator itself is not passed on.
    #[test]
    fn exec_accepts_a_double_dash_boundary() {
        let cli = Cli::try_parse_from(["proxenos", "exec", "--", "claude", "--help"]).unwrap();
        let Command::Exec(args) = cli.command else {
            panic!("expected exec");
        };
        assert_eq!(args.command, ["claude", "--help"]);
    }

    #[test]
    fn record_requires_an_explicit_mode() {
        // Defaulting the mode would let `record` spend quota without being asked
        // to. The mode is always stated.
        let parsed = Cli::try_parse_from(["proxenos", "record"]);
        assert!(parsed.is_err());
    }

    #[test]
    fn record_ingress_parses() {
        let cli = Cli::try_parse_from(["proxenos", "record", "ingress"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Record(RecordArgs {
                mode: RecordMode::Ingress { .. }
            })
        ));
    }

    /// The account is required rather than defaulted. Spending the wrong
    /// subscription is not recoverable, and the selected account is usually
    /// the other provider's.
    #[test]
    fn surface_capture_will_not_run_without_being_told_which_account_pays() {
        assert!(Cli::try_parse_from(["proxenos", "record", "surface"]).is_err());

        let cli = Cli::try_parse_from(["proxenos", "record", "surface", "--account", "personal"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Command::Record(RecordArgs {
                mode: RecordMode::Surface { only: None, .. }
            })
        ));
    }

    #[test]
    fn record_takes_the_port_the_daemon_verbs_take() {
        let cli =
            Cli::try_parse_from(["proxenos", "record", "ingress", "--port", "18799"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Record(RecordArgs {
                mode: RecordMode::Ingress { port: Some(18799) }
            })
        ));
    }
}
