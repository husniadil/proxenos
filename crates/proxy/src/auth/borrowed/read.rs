//! `docs/proxy-behavior.md` §8.4 — fetching what the rules above located.
//!
//! Split from the decisions on purpose: where a grant lives and what its
//! contents mean are answered without touching a disk or a keychain, and this
//! is the one part that cannot be. It goes behind a trait so the store above
//! it is tested against grants that were never written anywhere.
//!
//! Nothing here writes, and nothing here logs what it read.

use super::BorrowedError;
use super::ClaudeSource;
use super::Host;
use super::Source;
use super::remedy;
use super::source;
use crate::auth::jwt;
use crate::auth::store::Credentials;
use crate::auth::store::Provider;
use crate::error::ProxyError;
use std::path::Path;
use std::path::PathBuf;

/// One profile as the configuration declares it, resolved against a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// The name the account is filed under, which is what `accounts --use`
    /// takes.
    pub name: String,
    pub provider: Provider,
    /// `None` is the stock profile of that program (§8.4).
    pub config_dir: Option<PathBuf>,
}

impl Profile {
    pub fn source(&self, host: Host, home: &Path) -> Source {
        source(self.provider, host, self.config_dir.as_deref(), home)
    }
}

/// A grant read out of a profile, with what describes it to an operator.
///
/// One shape for both providers. What each of them can say about an account
/// differs — a Codex id token carries an email and a plan, a Claude item
/// carries a subscription type and no email at all — and absent stays absent
/// rather than being filled in with something plausible.
#[derive(Debug)]
pub struct Grant {
    pub credentials: Credentials,
    /// Unix seconds, and only ever present for a Claude grant. It is what says
    /// whether asking the owning client to refresh can work at all: past it,
    /// the attempt fails AND blanks the stored item (§8.4).
    pub refresh_token_expires_at: Option<u64>,
    pub plan: Option<String>,
    pub email: Option<String>,
}

/// Where the bytes come from.
///
/// A trait because the alternative is a test suite that needs a real keychain
/// and a signed-in profile, which is the kind of test that stops running.
pub trait GrantReader: Send + Sync {
    /// The text a source holds, or `None` where the source is not there at
    /// all — a profile directory that was never signed into, a keychain item
    /// that does not exist. Absent is an answer, and it is not an error.
    fn read(&self, source: &Source) -> Result<Option<String>, ProxyError>;
}

/// The real one: a file on disk, or a keychain item read by spawning
/// `security`.
pub struct HostReader;

/// What `security` exits with when the item is simply not there. Anything else
/// is a failure worth reporting, and the two are not the same answer: absent
/// means sign in, and a failure means something is wrong with the keychain.
const SECURITY_NOT_FOUND: i32 = 44;

impl GrantReader for HostReader {
    fn read(&self, source: &Source) -> Result<Option<String>, ProxyError> {
        match source {
            Source::Codex { auth_json } => read_file(auth_json),
            Source::Claude(ClaudeSource::File { path }) => read_file(path),
            Source::Claude(ClaudeSource::Keychain { service }) => read_keychain(service),
        }
    }
}

fn read_file(path: &Path) -> Result<Option<String>, ProxyError> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ProxyError::authentication(format!(
            "could not read {}: {error}",
            path.display()
        ))),
    }
}

/// Spawn `security` rather than calling Security.framework.
///
/// The item's ACL trusts that binary. A process reading through the framework
/// is a different application to the keychain and is prompted for every read,
/// and a daemon that prompts is a daemon nobody can run unattended (§8.4).
fn read_keychain(service: &str) -> Result<Option<String>, ProxyError> {
    let output = std::process::Command::new("security")
        .arg("find-generic-password")
        .arg("-w")
        .arg("-s")
        .arg(service)
        .output()
        .map_err(|error| {
            ProxyError::authentication(format!("could not run `security`: {error}"))
        })?;

    if output.status.success() {
        // `-w` prints the value and a newline. The newline is not part of the
        // credential, and a JSON parser would not mind it, but everything
        // downstream is easier if what came out is exactly what went in.
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_owned(),
        ));
    }
    if output.status.code() == Some(SECURITY_NOT_FOUND) {
        return Ok(None);
    }

    // stderr, never stdout: stdout on any other exit is not a credential, but
    // there is no reading of the exit code that makes printing it safe.
    Err(ProxyError::authentication(format!(
        "could not read keychain item `{service}`: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Read one profile's grant, or say why there is none.
///
/// A source that is not there and a source that holds nothing usable are the
/// same answer to the operator — sign in to that profile — so both arrive as
/// the refusal `BorrowedError` already words, naming the store and the remedy.
pub fn grant(
    reader: &dyn GrantReader,
    profile: &Profile,
    host: Host,
    home: &Path,
) -> Result<Grant, ProxyError> {
    let source = profile.source(host, home);
    let label = source.label();

    let Some(raw) = reader.read(&source)? else {
        return Err(ProxyError::authentication(
            BorrowedError::NotSignedIn(label, remedy(profile.provider)).to_string(),
        ));
    };

    match profile.provider {
        Provider::Codex => {
            let credentials = super::codex(&raw, &label).map_err(as_error)?;
            let id_token = credentials.id_token.as_deref();
            Ok(Grant {
                plan: jwt::plan(id_token),
                email: jwt::email(id_token),
                refresh_token_expires_at: None,
                credentials,
            })
        }
        Provider::Anthropic => {
            let borrowed = super::claude(&raw, &label).map_err(as_error)?;
            Ok(Grant {
                credentials: borrowed.credentials,
                refresh_token_expires_at: borrowed.refresh_token_expires_at,
                plan: borrowed.plan,
                // The item carries no email, and the client's own account file
                // is not this module's to read. Absent is reported as absent.
                email: None,
            })
        }
    }
}

fn as_error(error: BorrowedError) -> ProxyError {
    ProxyError::authentication(error.to_string())
}
