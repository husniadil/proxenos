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
    /// The name the account is filed under, which is what `accounts use`
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
    /// The single place that answered, never the compound source it was
    /// reached through.
    ///
    /// A macOS Claude profile is the keychain item *and* the file beside it,
    /// and which of the two held the grant is not a detail: it decides whether
    /// a refreshed grant may be written back (§8.4). Recorded where the read
    /// happened, because that is the only point at which it is known.
    pub origin: Source,
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
            // `grant` splits a compound source into its places before reading,
            // so this arm answers only a caller that read the source whole. It
            // walks the same order, and the keychain's failure goes no further
            // than the log — the refusal that carries it is `grant`'s to word.
            Source::Claude(ClaudeSource::KeychainThenFile { service, path }) => {
                match read_keychain(service) {
                    Ok(Some(raw)) => Ok(Some(raw)),
                    Ok(None) => read_file(path),
                    Err(error) => {
                        tracing::debug!(
                            %error,
                            "the keychain could not be read; trying the file beside it"
                        );
                        read_file(path)
                    }
                }
            }
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

    let (raw, origin) = match locate(reader, &source)? {
        Located::Found { raw, origin } => (raw, origin),
        Located::Absent { carried } => {
            let mut message =
                BorrowedError::NotSignedIn(label, remedy(profile.provider)).to_string();
            // The keychain's failure is not the answer — the file beside it
            // could have held the grant — but where nothing held it, it is the
            // difference between a profile nobody signed into and a keychain
            // this process cannot reach. Silently dropping it would send an
            // operator to sign in again against a keychain that will refuse
            // the next read exactly as it refused this one.
            if let Some(carried) = carried {
                message.push_str(&format!(
                    " The keychain could not be read either: {carried}"
                ));
            }
            return Err(ProxyError::authentication(message));
        }
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
                origin,
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
                origin,
            })
        }
    }
}

/// What the walk over a source's places came back with.
enum Located {
    /// The bytes, and the place they came from.
    Found { raw: String, origin: Source },
    /// Nothing was there. `carried` is the failure of an earlier place that
    /// was tried and could not answer — the macOS keychain, in the only case
    /// that has one.
    Absent { carried: Option<String> },
}

/// Read the first place that answers, in the order the source names them.
///
/// A single-place source is the whole of the old rule: absent is an answer,
/// and a failure is reported. What the macOS Claude source adds is that a
/// keychain which cannot be read is not the end of the attempt — a daemon
/// with no security session, or with a locked login keychain, fails there
/// while the file beside it holds the same JSON — so the failure is set aside
/// and the file is asked. It is set aside, not discarded: logged where the
/// file answered, carried into the refusal where nothing did.
///
/// The **last** place's failure is still reported. There is nothing left to
/// try, and a read that failed is not a profile that was never signed into.
fn locate(reader: &dyn GrantReader, source: &Source) -> Result<Located, ProxyError> {
    let places = source.places();
    let last = places.len().saturating_sub(1);
    let mut carried: Option<String> = None;

    for (index, place) in places.iter().enumerate() {
        match reader.read(place) {
            Ok(Some(raw)) => {
                if let Some(carried) = &carried {
                    tracing::debug!(
                        error = carried,
                        place = %place.label(),
                        "the keychain could not be read; the grant came from the file beside it"
                    );
                }
                return Ok(Located::Found {
                    raw,
                    origin: place.clone(),
                });
            }
            Ok(None) => {}
            Err(error) if index == last => return Err(error),
            Err(error) => carried = Some(error.message),
        }
    }

    Ok(Located::Absent { carried })
}

fn as_error(error: BorrowedError) -> ProxyError {
    ProxyError::authentication(error.to_string())
}
