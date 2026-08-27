//! `docs/proxy-behavior.md` §8.4 — signing in to a profile this daemon will
//! then borrow from.
//!
//! The verb runs the program that owns the profile and gets out of the way. It
//! is the same rule as everywhere else in §8.4, applied one step earlier: the
//! client authenticates, the client writes, and this side learns the result by
//! reading the profile afterwards. Nothing here sees a token.
//!
//! What this side decides is only which directory the client is pointed at,
//! and what the configuration file says about it afterwards.

use crate::auth::store::Provider;
use std::path::Path;
use std::path::PathBuf;

/// Where a profile goes when the operator did not say.
///
/// Under this daemon's own directory, because a profile it created is its own
/// to keep track of, and scattering them through the home directory makes them
/// something nobody can find later. An operator who wants one somewhere else
/// says so, and that path is what gets declared.
pub fn directory(config_dir: &Path, name: &str) -> PathBuf {
    config_dir.join("profiles").join(name)
}

/// The program that signs a profile in, and what it is given.
///
/// The directory travels as an environment variable rather than a flag,
/// because that is how both clients name a profile — the same variable this
/// daemon later resolves the grant from, so what is signed in and what is read
/// cannot drift apart.
#[derive(Debug)]
pub struct Command {
    pub program: String,
    pub arguments: Vec<String>,
    pub variable: &'static str,
    pub directory: PathBuf,
}

impl Command {
    /// What to run for one provider.
    ///
    /// `program` is the configured path for this provider where there is one —
    /// `claude_program` or `codex_program` (§4). It is the same key the daemon
    /// runs that client by; a login started from a shell would usually resolve
    /// the bare name anyway, but an operator who had to write the path down
    /// once should not have to remember where it applies, and neither client
    /// is more likely than the other to be off `PATH`.
    ///
    /// `device_auth` asks the client to print a URL and a code instead of
    /// opening a browser. Only `codex login` has it; the Anthropic arm never
    /// sees it set, because `plan` refuses the flag for that provider before
    /// there is a command to put it on.
    pub fn new(
        provider: Provider,
        directory: PathBuf,
        program: Option<&Path>,
        device_auth: bool,
    ) -> Self {
        match provider {
            Provider::Anthropic => Self {
                program: program.map_or_else(
                    || crate::auth::borrowed::poke::PROGRAM.to_owned(),
                    |path| path.display().to_string(),
                ),
                arguments: vec!["auth".to_owned(), "login".to_owned()],
                variable: "CLAUDE_CONFIG_DIR",
                directory,
            },
            Provider::Codex => {
                let mut arguments = vec!["login".to_owned()];
                if device_auth {
                    arguments.push("--device-auth".to_owned());
                }
                Self {
                    program: program.map_or_else(
                        || crate::auth::borrowed::poke::CODEX_PROGRAM.to_owned(),
                        |path| path.display().to_string(),
                    ),
                    arguments,
                    variable: "CODEX_HOME",
                    directory,
                }
            }
        }
    }

    /// The same command as a line an operator can paste.
    ///
    /// Printed instead of run wherever running it cannot work: no terminal to
    /// answer its prompts, or a program that is not on this machine. The
    /// alternative — starting a client that wants a browser and a keyboard
    /// from something with neither — hangs with nothing said.
    pub fn line(&self) -> String {
        let mut line = format!(
            "{}={} {}",
            self.variable,
            quote(&self.directory.display().to_string()),
            quote(&self.program)
        );
        for argument in &self.arguments {
            line.push(' ');
            line.push_str(&quote(argument));
        }
        line
    }
}

/// One shell word, quoted only where it has to be.
///
/// A profile directory is very often under a path with a space in it, and a
/// line an operator pastes has to survive that.
fn quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_./:@=+,".contains(character))
    {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// What signing in to one profile comes to, decided without touching anything.
///
/// Everything here is a consequence of the operator's arguments and the
/// configuration file as it already reads. Nothing in it has run a client,
/// created a directory, or read a grant — which is what makes the decisions
/// answerable on their own, and the six ways the run below can end testable
/// without a terminal.
#[derive(Debug)]
pub struct Plan {
    /// What the account is filed under, which is what `accounts use` takes.
    pub name: String,
    pub provider: Provider,
    /// The directory the client is pointed at, and the one that is declared.
    pub directory: PathBuf,
    pub command: Command,
    /// Whether the client was asked to print a URL and a code rather than open
    /// a browser, which the way back has to carry for the same reason it
    /// carries the provider.
    pub device_auth: bool,
    /// How the grant is read back afterwards — the same read every turn
    /// makes, so a profile that passes here is one the daemon can serve.
    pub profile: crate::auth::borrowed::read::Profile,
    /// Whether `[profiles]` is still empty, in which case declaring anything
    /// stops the daemon looking for the stock profiles (§8.4) and what was
    /// being read has to be written down first.
    pub preserve_discovered: bool,
}

/// Decide what an `accounts login` would do.
///
/// The one refusal that has to live here is a name the configuration file
/// already declares: it cannot be found out later, because appending a table
/// TOML already has leaves a file the daemon cannot start from. The name
/// itself is a positional on the verb, so there is nothing to refuse about it.
///
/// `keys` are the names this daemon's own key store already holds. One name is
/// one account: `add-key` refuses a name that is a profile, and this is the
/// same rule from the other side.
///
/// `device_auth` is the other refusal, and it is here for the same reason: a
/// flag only one of the two clients has cannot be found out later. Passing an
/// argument `claude auth login` does not take ends as that client's own usage
/// error, about a spelling rather than about the choice.
pub fn plan(
    name: &str,
    provider: Provider,
    path: Option<PathBuf>,
    device_auth: bool,
    config: &crate::config::Config,
    config_dir: &Path,
    keys: &[String],
) -> anyhow::Result<Plan> {
    if device_auth && provider == Provider::Anthropic {
        anyhow::bail!(
            "`--device-auth` is a `codex login` flag, and `claude auth login` has no equivalent. \
             Sign the anthropic profile in where a browser can open, then declare that directory \
             here with `--path`."
        );
    }
    if config.profiles.contains_key(name) {
        anyhow::bail!(
            "`{name}` is already declared in `[profiles]`. Sign in to it with the command \
             that profile's own client takes, or choose another name."
        );
    }
    if keys.iter().any(|key| key == name) {
        anyhow::bail!(
            "`{name}` is already a stored key. Choose another name for the profile, or \
             remove the key first with `proxenos accounts remove {name}`."
        );
    }

    let directory = path.unwrap_or_else(|| directory(config_dir, name));
    // Each provider's own configured path: the same key the daemon pokes that
    // client by (§8.4), so a login and a refresh cannot run different programs.
    let program = match provider {
        Provider::Anthropic => config.claude_program.as_deref(),
        Provider::Codex => config.codex_program.as_deref(),
    };
    let command = Command::new(provider, directory.clone(), program, device_auth);

    Ok(Plan {
        device_auth,
        name: name.to_owned(),
        provider,
        profile: crate::auth::borrowed::read::Profile {
            name: name.to_owned(),
            provider,
            config_dir: Some(directory.clone()),
        },
        directory,
        command,
        preserve_discovered: config.profiles.is_empty(),
    })
}

/// How the client's run ended.
///
/// Its own shape rather than a `std::process::ExitStatus`, because a status
/// cannot be constructed on every platform this builds for, and what the run
/// step needs of it is two things: whether it succeeded, and how to say what
/// it was otherwise.
pub struct Exit {
    pub success: bool,
    /// The status as the operator is shown it.
    pub description: String,
}

/// Everything the run step cannot decide for itself.
///
/// One seam for the whole of it — the terminal, the client, the profile, and
/// the configuration file — so the outcomes are exercised against values
/// rather than against a machine that has a client installed and a person
/// sitting at it.
pub trait Environment {
    /// Make sure the profile directory exists before the client is pointed at
    /// it.
    fn create_directory(&mut self, directory: &Path) -> std::io::Result<()>;
    /// Whether there is somebody at a keyboard to answer a login's prompts.
    fn is_interactive(&self) -> bool;
    /// Run the client, returning when it has exited.
    fn run(&mut self, command: &Command) -> std::io::Result<Exit>;
    /// Whether a profile holds a grant, and what is wrong with it where it
    /// does not. The message is the operator's, and is carried rather than
    /// summarized.
    fn grant_held(&mut self, profile: &crate::auth::borrowed::read::Profile) -> Result<(), String>;
    /// The profiles being read without being declared (§8.4).
    fn discovered(&mut self) -> Vec<crate::auth::borrowed::read::Profile>;
    /// The configuration file as it reads now, or the stock example where
    /// there is no file yet.
    fn read_document(&mut self) -> anyhow::Result<String>;
    fn write_document(&mut self, document: String) -> anyhow::Result<()>;
    /// The configuration file's path, as the closing line names it.
    fn document_path(&self) -> String;
    /// Where a line of this verb's output goes.
    fn say(&mut self, line: &str);
}

/// Carry out a plan: sign the profile in if it is not already, then declare
/// what the client wrote.
pub fn run(plan: &Plan, environment: &mut dyn Environment) -> anyhow::Result<()> {
    use anyhow::Context as _;

    environment
        .create_directory(&plan.directory)
        .with_context(|| format!("could not create {}", plan.directory.display()))?;

    // A directory that already holds a grant is signed in, whoever signed it
    // in. Running the client over it would ask the operator to authenticate
    // something that already is — and this is also the path that adopts a
    // profile another tool made, and the path a second run takes after the
    // operator ran the printed line themselves.
    if environment.grant_held(&plan.profile).is_ok() {
        let said = format!("{} is already signed in", plan.directory.display());
        environment.say(&said);
    } else {
        // A login wants a browser and a keyboard. Started from something with
        // neither it hangs with nothing said, so where there is no terminal
        // the line is printed and the operator runs it themselves — the same
        // thing this would have done, in a place where it can work. The way
        // back is the whole command, because a re-run that dropped the
        // provider would sign in to the other one — and one that dropped
        // `--device-auth` would, on a machine with a terminal but no browser,
        // start the client the way that hangs.
        if !environment.is_interactive() {
            let said = format!(
                "run this:\n\n  {}\n\nthen declare it with:\n\n  proxenos accounts login \
                 {} --provider {} --path {}{}",
                plan.command.line(),
                plan.name,
                plan.provider.as_str(),
                plan.directory.display(),
                if plan.device_auth {
                    " --device-auth"
                } else {
                    ""
                }
            );
            environment.say(&said);
            return Ok(());
        }

        let said = format!(
            "signing in to {} — {}",
            plan.directory.display(),
            plan.command.line()
        );
        environment.say(&said);
        match environment.run(&plan.command) {
            Ok(exit) if exit.success => {}
            // Its exit status is not the answer, and neither is a program that
            // could not be started: what settles it is whether the profile now
            // holds a grant. Both are said, then the profile is read.
            Ok(exit) => {
                let said = format!("`{}` exited {}", plan.command.program, exit.description);
                environment.say(&said);
            }
            Err(error) => {
                let said = format!(
                    "could not run `{}`: {error}. Run it yourself:\n\n  {}",
                    plan.command.program,
                    plan.command.line()
                );
                environment.say(&said);
            }
        }

        if let Err(message) = environment.grant_held(&plan.profile) {
            anyhow::bail!(
                "{} holds no grant, so nothing was declared: {message}",
                plan.directory.display()
            );
        }
    }

    let mut document = environment.read_document()?;
    // Declaring anything stops the daemon looking for the stock profiles
    // (§8.4), so a first `accounts login` would take away every account the
    // operator already had. They are written down first, exactly as they were
    // being read, and only the ones that hold a grant: an entry for a program
    // that was never signed into is an account that cannot serve.
    if plan.preserve_discovered {
        for found in environment.discovered() {
            if found.name == plan.name || environment.grant_held(&found).is_err() {
                continue;
            }
            document =
                crate::config::edit::add_profile(&document, &found.name, found.provider, None)?;
            let said = format!(
                "declaring `{}`, which was being read without being declared",
                found.name
            );
            environment.say(&said);
        }
    }

    let updated = crate::config::edit::add_profile(
        &document,
        &plan.name,
        plan.provider,
        Some(plan.directory.as_path()),
    )?;
    environment.write_document(updated)?;

    let said = format!(
        "declared `{}` in {}.",
        plan.name,
        environment.document_path()
    );
    environment.say(&said);
    Ok(())
}

/// The real one: this machine's terminal, client, profiles, and configuration
/// file.
pub struct Stdio {
    host: crate::auth::borrowed::Host,
    home: PathBuf,
    path: PathBuf,
    interactive: bool,
}

impl Stdio {
    pub fn new() -> anyhow::Result<Self> {
        use anyhow::Context as _;
        use std::io::IsTerminal as _;

        Ok(Self {
            host: crate::auth::borrowed::host()?,
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .context("`HOME` is not set, and a profile is resolved relative to it")?,
            path: crate::config::config_path(),
            interactive: std::io::stdin().is_terminal(),
        })
    }
}

impl Environment for Stdio {
    fn create_directory(&mut self, directory: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(directory)
    }

    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn run(&mut self, command: &Command) -> std::io::Result<Exit> {
        let status = std::process::Command::new(&command.program)
            .args(&command.arguments)
            .env(command.variable, &command.directory)
            .status()?;
        Ok(Exit {
            success: status.success(),
            description: status.to_string(),
        })
    }

    fn grant_held(&mut self, profile: &crate::auth::borrowed::read::Profile) -> Result<(), String> {
        crate::auth::borrowed::read::grant(
            &crate::auth::borrowed::read::HostReader,
            profile,
            self.host,
            &self.home,
        )
        .map(|_| ())
        .map_err(|error| error.message)
    }

    fn discovered(&mut self) -> Vec<crate::auth::borrowed::read::Profile> {
        crate::auth::borrowed::discovered()
    }

    fn read_document(&mut self) -> anyhow::Result<String> {
        use anyhow::Context as _;

        match std::fs::read_to_string(&self.path) {
            Ok(document) => Ok(document),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(crate::config::EXAMPLE.to_owned())
            }
            Err(error) => {
                Err(error).with_context(|| format!("could not read {}", self.path.display()))
            }
        }
    }

    fn write_document(&mut self, document: String) -> anyhow::Result<()> {
        use anyhow::Context as _;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, document)
            .with_context(|| format!("could not write {}", self.path.display()))
    }

    fn document_path(&self) -> String {
        self.path.display().to_string()
    }

    fn say(&mut self, line: &str) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::borrowed::read;

    fn config(document: &str) -> crate::config::Config {
        toml::from_str(document).expect("the document parses")
    }

    fn plan_for(name: &str, document: &str) -> Plan {
        plan(
            name,
            Provider::Codex,
            None,
            false,
            &config(document),
            Path::new("/config"),
            &[],
        )
        .expect("a plan")
    }

    /// The machine, as far as this verb can tell: which profiles hold a grant,
    /// whether anybody is at a keyboard, what the client's run comes to, and
    /// what the configuration file says.
    struct Fake {
        interactive: bool,
        held: Vec<String>,
        /// What the client's run comes to, and which profiles hold a grant
        /// once it has.
        outcome: std::io::Result<Exit>,
        held_after_run: Vec<String>,
        discovered: Vec<read::Profile>,
        document: String,
        written: Option<String>,
        runs: usize,
        said: Vec<String>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                interactive: true,
                held: Vec::new(),
                outcome: Ok(Exit {
                    success: true,
                    description: "exit status: 0".to_owned(),
                }),
                held_after_run: Vec::new(),
                discovered: Vec::new(),
                document: "port = 8787\n".to_owned(),
                written: None,
                runs: 0,
                said: Vec::new(),
            }
        }
    }

    impl Fake {
        fn said(&self) -> String {
            self.said.join("\n")
        }
    }

    impl Environment for Fake {
        fn create_directory(&mut self, _directory: &Path) -> std::io::Result<()> {
            Ok(())
        }

        fn is_interactive(&self) -> bool {
            self.interactive
        }

        fn run(&mut self, _command: &Command) -> std::io::Result<Exit> {
            self.runs += 1;
            self.held = self.held_after_run.clone();
            match &self.outcome {
                Ok(exit) => Ok(Exit {
                    success: exit.success,
                    description: exit.description.clone(),
                }),
                Err(error) => Err(std::io::Error::new(error.kind(), error.to_string())),
            }
        }

        fn grant_held(&mut self, profile: &read::Profile) -> Result<(), String> {
            if self.held.contains(&profile.name) {
                return Ok(());
            }
            Err("holds no grant. Sign in to that profile first".to_owned())
        }

        fn discovered(&mut self) -> Vec<read::Profile> {
            self.discovered.clone()
        }

        fn read_document(&mut self) -> anyhow::Result<String> {
            Ok(self.document.clone())
        }

        fn write_document(&mut self, document: String) -> anyhow::Result<()> {
            self.written = Some(document);
            Ok(())
        }

        fn document_path(&self) -> String {
            "/config/proxenos.toml".to_owned()
        }

        fn say(&mut self, line: &str) {
            self.said.push(line.to_owned());
        }
    }

    /// Where a profile goes when the operator did not say: under this daemon's
    /// own directory, named after the account.
    #[test]
    fn a_profile_with_no_path_goes_under_this_daemons_directory() {
        let plan = plan_for("work", "port = 8787\n");

        assert_eq!(plan.directory, Path::new("/config/profiles/work"));
        assert_eq!(plan.command.directory, plan.directory);
        assert_eq!(
            plan.profile.config_dir.as_deref(),
            Some(plan.directory.as_path())
        );
    }

    /// A name the file already declares is refused before anything runs.
    /// Appending a table TOML already has leaves a file the daemon cannot
    /// start from, and the client would have been run for nothing first.
    #[test]
    fn a_name_the_file_already_declares_is_refused_before_anything_runs() {
        let refusal = plan(
            "work",
            Provider::Codex,
            None,
            false,
            &config("[profiles.work]\nprovider = \"codex\"\n"),
            Path::new("/config"),
            &[],
        )
        .expect_err("already declared")
        .to_string();

        assert!(
            refusal.contains("already declared in `[profiles]`"),
            "{refusal}"
        );
    }

    /// A name the key store already holds is refused the same way. `add-key`
    /// refuses a name that is a profile; without this half, `login` under a
    /// key's name produced two accounts one `accounts use` could not tell
    /// apart.
    #[test]
    fn a_name_the_key_store_already_holds_is_refused_before_anything_runs() {
        let refusal = plan(
            "billing",
            Provider::Codex,
            None,
            false,
            &config(""),
            Path::new("/config"),
            &["billing".to_owned()],
        )
        .expect_err("already a key")
        .to_string();

        assert!(refusal.contains("already a stored key"), "{refusal}");
        assert!(refusal.contains("accounts remove billing"), "{refusal}");
    }

    /// `--device-auth` is a flag only one of the two clients has. Refused here
    /// rather than passed on, because `claude auth login` would end in its own
    /// usage error about a spelling rather than about the choice.
    #[test]
    fn device_auth_against_the_client_that_has_no_such_flag_is_refused() {
        let refusal = plan(
            "work",
            Provider::Anthropic,
            None,
            true,
            &config(""),
            Path::new("/config"),
            &[],
        )
        .expect_err("no such flag")
        .to_string();

        assert!(refusal.contains("`codex login` flag"), "{refusal}");
        assert!(refusal.contains("claude auth login"), "{refusal}");
    }

    /// A directory that already holds a grant is signed in, whoever signed it
    /// in. Nothing is run over it, and it is declared all the same — this is
    /// the path that adopts a profile another tool made.
    #[test]
    fn a_directory_that_is_already_signed_in_is_declared_without_running_anything() {
        let plan = plan_for("work", "port = 8787\n");
        let mut fake = Fake {
            held: vec!["work".to_owned()],
            ..Fake::default()
        };

        run(&plan, &mut fake).expect("declared");

        assert_eq!(fake.runs, 0);
        assert!(
            fake.said()
                .contains("/config/profiles/work is already signed in"),
            "{}",
            fake.said()
        );
        let written = fake.written.expect("the file was written");
        assert!(written.contains("[profiles.work]"), "{written}");
    }

    /// No terminal means no browser and no keyboard, so the client is not
    /// started — it would hang with nothing said. The line to run and the way
    /// back are printed instead, and the file is left alone.
    #[test]
    fn a_run_with_no_terminal_prints_the_line_and_writes_nothing() {
        let plan = plan_for("work", "port = 8787\n");
        let mut fake = Fake {
            interactive: false,
            ..Fake::default()
        };

        run(&plan, &mut fake).expect("printed");

        let said = fake.said();
        assert!(said.contains("run this:"), "{said}");
        assert!(
            said.contains("CODEX_HOME=/config/profiles/work codex login"),
            "{said}"
        );
        assert!(
            said.contains(
                "proxenos accounts login work --provider codex --path /config/profiles/work"
            ),
            "{said}"
        );
        assert_eq!(fake.runs, 0);
        assert_eq!(fake.written, None);
    }

    /// No terminal is one reason a login cannot run here; no browser is the
    /// other, and it is the one `--device-auth` answers. The flag is on the
    /// line to paste, and on the way back — a re-run that dropped it would,
    /// on a machine with a terminal but no browser, start the client the way
    /// that hangs.
    #[test]
    fn device_auth_is_on_both_lines_the_printed_run_gives_back() {
        let plan = plan(
            "work",
            Provider::Codex,
            None,
            true,
            &config("port = 8787\n"),
            Path::new("/config"),
            &[],
        )
        .expect("a plan");
        let mut fake = Fake {
            interactive: false,
            ..Fake::default()
        };

        run(&plan, &mut fake).expect("printed");

        let said = fake.said();
        assert!(
            said.contains("CODEX_HOME=/config/profiles/work codex login --device-auth"),
            "{said}"
        );
        assert!(
            said.contains(
                "proxenos accounts login work --provider codex --path /config/profiles/work \
                 --device-auth"
            ),
            "{said}"
        );
        assert_eq!(fake.runs, 0);
    }

    /// The client's exit status is not the answer. What settles it is whether
    /// the profile holds a grant afterwards — so a non-zero exit over a
    /// directory that now holds one is said and then declared.
    #[test]
    fn a_client_that_exited_non_zero_still_declares_a_grant_it_left_behind() {
        let plan = plan_for("work", "port = 8787\n");
        let mut fake = Fake {
            outcome: Ok(Exit {
                success: false,
                description: "exit status: 1".to_owned(),
            }),
            held_after_run: vec!["work".to_owned()],
            ..Fake::default()
        };

        run(&plan, &mut fake).expect("declared");

        assert_eq!(fake.runs, 1);
        assert!(
            fake.said().contains("`codex` exited exit status: 1"),
            "{}",
            fake.said()
        );
        let written = fake.written.expect("the file was written");
        assert!(written.contains("[profiles.work]"), "{written}");
    }

    /// A directory holding no grant afterwards is declared nowhere: an entry
    /// for a profile that was never signed into is an account that cannot
    /// serve, and the refusal names the directory the operator has to look at.
    #[test]
    fn a_directory_that_holds_no_grant_afterwards_declares_nothing() {
        let plan = plan_for("work", "port = 8787\n");
        let mut fake = Fake::default();

        let refusal = run(&plan, &mut fake).expect_err("no grant").to_string();

        assert!(
            refusal.contains("/config/profiles/work holds no grant, so nothing was declared"),
            "{refusal}"
        );
        assert_eq!(fake.written, None);
    }

    /// Declaring anything stops the daemon looking for the stock profiles, so
    /// the first declaration writes down what was already being read — and
    /// only the ones that hold a grant.
    #[test]
    fn the_first_declaration_preserves_what_was_being_read() {
        let plan = plan_for("work", "port = 8787\n");
        let mut fake = Fake {
            held: vec!["work".to_owned(), "codex".to_owned()],
            discovered: vec![
                read::Profile {
                    name: "codex".to_owned(),
                    provider: Provider::Codex,
                    config_dir: None,
                },
                read::Profile {
                    name: "claude".to_owned(),
                    provider: Provider::Anthropic,
                    config_dir: None,
                },
            ],
            ..Fake::default()
        };

        run(&plan, &mut fake).expect("declared");

        assert!(
            fake.said()
                .contains("declaring `codex`, which was being read without being declared"),
            "{}",
            fake.said()
        );
        let written = fake.written.clone().expect("the file was written");
        assert!(written.contains("[profiles.codex]"), "{written}");
        // The one holding no grant is an account that cannot serve, so it is
        // not written down as one.
        assert!(!written.contains("[profiles.claude]"), "{written}");
        assert!(written.contains("[profiles.work]"), "{written}");
    }

    /// A file that already declares something is not the first declaration:
    /// what the operator wrote is the whole set, and nothing is added to it.
    #[test]
    fn a_file_that_already_declares_a_profile_preserves_nothing() {
        let plan = plan_for("work", "[profiles.other]\nprovider = \"codex\"\n");
        let mut fake = Fake {
            held: vec!["work".to_owned(), "codex".to_owned()],
            discovered: vec![read::Profile {
                name: "codex".to_owned(),
                provider: Provider::Codex,
                config_dir: None,
            }],
            document: "[profiles.other]\nprovider = \"codex\"\n".to_owned(),
            ..Fake::default()
        };

        run(&plan, &mut fake).expect("declared");

        let written = fake.written.expect("the file was written");
        assert!(!written.contains("[profiles.codex]"), "{written}");
    }
}
