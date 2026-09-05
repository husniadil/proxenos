//! `docs/api.md` §2.8 — reading another process's environment to say how it
//! was started.
//!
//! The knowledge this settles used to live in whatever program wanted it: a
//! consumer read an agent's environment itself and matched `proxenos-account:`
//! by hand, which is this project's own spelling parsed somewhere it cannot be
//! kept in step. Here it is parsed with `ingress::parse_tags`, the same
//! function the daemon reads a request's header with, so a change to the
//! spelling moves both at once.
//!
//! **Nothing here carries a token.** The value being parsed may hold
//! `proxenos-token:<secret>` beside the account tag — that is what a
//! client-mode launch sets — and the parsed token is dropped where it is read.
//! `Launched` has no field it could reach.

use crate::ingress;

/// Where a launch through this daemon points the client.
pub const BASE_URL: &str = "ANTHROPIC_BASE_URL";

/// The one header the client offers, and so the one place a launch can say
/// which account it is tagged for (§1).
pub const AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";

/// What one process's environment says about how it was started.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Launched {
    /// Whether this process was started through this daemon.
    pub through: bool,
    /// The account its turns are tagged as. `None` where it was launched
    /// without `--account` — its turns go as whichever account is serving —
    /// and `None` where it was not launched through this daemon at all.
    pub account: Option<String>,
    /// The daemon it was pointed at, as its `ANTHROPIC_BASE_URL` says.
    pub daemon: Option<String>,
}

impl Launched {
    /// The one line the verb prints.
    #[must_use]
    pub fn line(&self, pid: u32) -> String {
        if !self.through {
            return format!("pid {pid}: not through proxenos");
        }
        let as_who = match &self.account {
            Some(account) => format!("as {account}"),
            None => "as the serving account".to_owned(),
        };
        match &self.daemon {
            Some(daemon) => format!("pid {pid}: through proxenos {as_who} ({daemon})"),
            None => format!("pid {pid}: through proxenos {as_who}"),
        }
    }
}

/// Read one process's environment text.
///
/// Pure over the text, because the two platforms hand it over in two shapes
/// and neither of them is worth a second parser: Linux's `/proc/<pid>/environ`
/// is NUL-separated, and macOS's `ps -Eww -o command=` prints the command and
/// then the environment separated by spaces.
///
/// **`through` is what `exec` actually sets, not a guess at it.** A launch with
/// `--account` tags the auth token, and a client-mode launch puts the daemon's
/// token in the same value; either is this daemon's own spelling and could come
/// from nowhere else. A launch with neither sets no token of its own at all —
/// the `env` payload's `unused` stands — so that sentinel counts only beside an
/// `ANTHROPIC_BASE_URL`, which is the other half of what a launch applies.
#[must_use]
pub fn read(environment: &str) -> Launched {
    let variables = variables(environment);
    let daemon = value(&variables, BASE_URL);
    let Some(auth) = value(&variables, AUTH_TOKEN) else {
        return Launched::default();
    };

    // The token part is read here and goes no further: `Tags` is dropped at the
    // end of this expression and `Launched` has no field for it.
    let tags = ingress::parse_tags(&auth);
    let through = tags.account.is_some()
        || tags.token.is_some()
        || (auth == ingress::auth_token_value(None, None) && daemon.is_some());
    if !through {
        return Launched::default();
    }
    Launched {
        through: true,
        account: tags.account,
        daemon,
    }
}

/// Whether this text carries an environment at all.
///
/// The macOS reader needs it: `ps` prints the command alone for a process that
/// is not the caller's, which is not an error and would otherwise be read as
/// an environment that says nothing.
#[must_use]
pub fn carries_environment(environment: &str) -> bool {
    !variables(environment).is_empty()
}

/// What one variable holds, or nothing where it is unset.
fn value(variables: &[(String, String)], name: &str) -> Option<String> {
    variables
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// The assignments in one environment text, in the order they appear.
fn variables(environment: &str) -> Vec<(String, String)> {
    if environment.contains('\0') {
        return environment.split('\0').filter_map(assignment).collect();
    }

    // The space-separated form has no framing at all: the command comes first,
    // and **a value may itself hold a space** — the auth token carrying both
    // tags is exactly that, `proxenos-token:<secret> proxenos-account:<name>`.
    // So a word that does not start a new assignment continues the value
    // before it, and words before the first assignment are the command.
    let mut found: Vec<(String, String)> = Vec::new();
    for word in environment.split_whitespace() {
        match assignment(word) {
            Some(pair) => found.push(pair),
            None => {
                if let Some((_, value)) = found.last_mut() {
                    value.push(' ');
                    value.push_str(word);
                }
            }
        }
    }
    found
}

/// One `NAME=VALUE`, where the name is shaped like an environment variable's.
///
/// The shape check is what keeps a command argument out of the answer in the
/// space-separated form, where nothing else tells argv and the environment
/// apart.
fn assignment(entry: &str) -> Option<(String, String)> {
    let (name, value) = entry.split_once('=')?;
    let mut characters = name.chars();
    let first = characters.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if !characters.all(|character| character.is_ascii_alphanumeric() || character == '_') {
        return None;
    }
    Some((name.to_owned(), value.to_owned()))
}
