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
pub struct Command {
    pub program: String,
    pub arguments: Vec<String>,
    pub variable: &'static str,
    pub directory: PathBuf,
}

impl Command {
    /// What to run for one provider.
    ///
    /// `claude_program` is the configured path where there is one (§4). It is
    /// the same key the daemon runs the client by; a login started from a
    /// shell would usually resolve the bare name anyway, but an operator who
    /// had to write the path down once should not have to remember where it
    /// applies.
    pub fn new(provider: Provider, directory: PathBuf, claude_program: Option<&Path>) -> Self {
        match provider {
            Provider::Anthropic => Self {
                program: claude_program.map_or_else(
                    || crate::auth::borrowed::poke::PROGRAM.to_owned(),
                    |path| path.display().to_string(),
                ),
                arguments: vec!["auth".to_owned(), "login".to_owned()],
                variable: "CLAUDE_CONFIG_DIR",
                directory,
            },
            Provider::Codex => Self {
                program: "codex".to_owned(),
                arguments: vec!["login".to_owned()],
                variable: "CODEX_HOME",
                directory,
            },
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
