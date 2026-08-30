//! `docs/proxy-behavior.md` §8.4 — putting a refreshed grant back where it
//! came from, for the one kind of store where that is the right move.
//!
//! **This is not a general write path.** A borrowed grant belongs to another
//! program, and rotating one behind that program's back logs the operator out
//! of it. What decides the difference is where the grant was *read*:
//!
//! - The macOS keychain item is the client's own store, held open across its
//!   run. Writing it is the case the refusal in `store.rs` exists for, and it
//!   still refuses.
//! - A file — `auth.json`, or `.credentials.json` beside a profile — is read
//!   by the owning client when it starts. Writing the rotated grant back into
//!   that same file is what keeps the two sides holding the same token;
//!   refusing means this side's grant goes stale at the first refresh and the
//!   file it was borrowed from moves on without it.
//!
//! The write preserves everything it does not model. The file belongs to the
//! other program, which puts fields in it this crate has never heard of, so
//! the bytes are re-read as a `serde_json::Value`, the credential's own fields
//! are overwritten in place, and the rest is written back exactly as it was.

use super::ClaudeSource;
use super::Source;
use crate::auth::store::Credentials;
use crate::auth::store::Provider;
use crate::error::ProxyError;
use serde_json::Value;
use std::path::Path;

/// The file a grant read from `origin` may be written back to, if any.
///
/// The whole of the file/keychain split, as a pure function over the place the
/// read came from. `None` is a refusal, and the caller words it.
///
/// `KeychainThenFile` is `None` deliberately: it is a *compound* source, and a
/// grant's `origin` is always one of the places it resolved to. Reaching here
/// with it means nobody recorded which place answered, and the only safe
/// reading of that is the keychain.
#[must_use]
pub fn writeback(origin: &Source) -> Option<&Path> {
    match origin {
        Source::Codex { auth_json } => Some(auth_json),
        Source::Claude(ClaudeSource::File { path }) => Some(path),
        Source::Claude(ClaudeSource::Keychain { .. } | ClaudeSource::KeychainThenFile { .. }) => {
            None
        }
    }
}

/// Write `credentials` back into the file the grant was read from.
///
/// Atomic: a temporary file in the same directory, then a rename over the
/// target. The owning client reads this file at start, and a client that
/// started while a plain truncating write was half done would read a
/// half-written grant and treat itself as signed out.
pub fn write_back(
    provider: Provider,
    path: &Path,
    credentials: &Credentials,
) -> Result<(), ProxyError> {
    let existing = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|error| {
            // Refused rather than replaced. A file that is not JSON is not a
            // file this side understands the shape of, and overwriting it
            // would take out whatever the owning program did put there.
            ProxyError::authentication(format!(
                "could not write the refreshed grant: {} is not valid JSON: {error}",
                path.display()
            ))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => {
            return Err(ProxyError::authentication(format!(
                "could not read {} before writing it back: {error}",
                path.display()
            )));
        }
    };

    let body = serde_json::to_string_pretty(&merged(provider, existing, credentials)).map_err(
        |error| {
            ProxyError::authentication(format!("could not render the refreshed grant: {error}"))
        },
    )?;

    write_private_atomically(path, &body)
}

/// The file's own JSON with the credential's fields overwritten, and nothing
/// else touched.
///
/// Every field this crate does not model survives: the Codex `last_refresh`
/// stamp, the Claude item's `subscriptionType` and `refreshTokenExpiresAt`,
/// and anything either program adds later. A field the credential does not
/// carry is left as it was rather than written null — `None` here means this
/// side could not determine it, which is not the same as the owning program
/// having no value for it.
#[must_use]
pub fn merged(provider: Provider, existing: Value, credentials: &Credentials) -> Value {
    let mut out = match existing {
        Value::Object(map) => map,
        // Anything else is replaced by the shape the grant belongs in. A file
        // holding a JSON scalar has no fields to preserve.
        _ => serde_json::Map::new(),
    };

    // Where each program keeps the grant inside its own file, and under which
    // spelling. The two differ in both, which is why the merge is per
    // provider rather than one field list.
    let key = match provider {
        Provider::Codex => "tokens",
        Provider::Anthropic => "claudeAiOauth",
    };
    let mut inner = nested(&out, key);

    match provider {
        Provider::Codex => {
            inner.insert(
                "access_token".to_owned(),
                Value::String(credentials.access_token.clone()),
            );
            inner.insert(
                "refresh_token".to_owned(),
                Value::String(credentials.refresh_token.clone()),
            );
            if let Some(id_token) = &credentials.id_token {
                inner.insert("id_token".to_owned(), Value::String(id_token.clone()));
            }
            if let Some(account_id) = &credentials.account_id {
                inner.insert("account_id".to_owned(), Value::String(account_id.clone()));
            }
            // No expiry is written. `auth.json` records none — this side reads
            // it from the access token's own claim (§8.4) — so writing one
            // would be inventing a field the owning program does not keep.
        }
        Provider::Anthropic => {
            inner.insert(
                "accessToken".to_owned(),
                Value::String(credentials.access_token.clone()),
            );
            inner.insert(
                "refreshToken".to_owned(),
                Value::String(credentials.refresh_token.clone()),
            );
            if let Some(expires_at) = credentials.expires_at {
                // Back into the milliseconds the item stores, which is what
                // the client counts down from.
                inner.insert(
                    "expiresAt".to_owned(),
                    Value::from(expires_at.saturating_mul(1_000)),
                );
            }
            // `refreshTokenExpiresAt` and `subscriptionType` are the item's,
            // not the credential's, and are left exactly as they were.
        }
    }

    out.insert(key.to_owned(), Value::Object(inner));
    Value::Object(out)
}

/// The object already under `key`, or an empty one.
///
/// Taken by value rather than borrowed: what comes back is edited and put
/// back, which keeps every sibling key beside it untouched without needing a
/// borrow that has to be proven to be an object. A `key` holding something
/// that is not an object is replaced — the grant is what the caller came to
/// write, and there is nothing in a scalar to merge with.
fn nested(out: &serde_json::Map<String, Value>, key: &str) -> serde_json::Map<String, Value> {
    match out.get(key) {
        Some(Value::Object(map)) => map.clone(),
        _ => serde_json::Map::new(),
    }
}

/// Write `body` where `path` is, without any reader ever seeing a partial one.
///
/// The temporary file is in the same directory, because a rename across
/// filesystems is not a rename. It is created `0600` before anything is
/// written into it, so the grant is never briefly readable by anyone else.
fn write_private_atomically(path: &Path, body: &str) -> Result<(), ProxyError> {
    let directory = path.parent().ok_or_else(|| {
        ProxyError::authentication(format!(
            "could not write the refreshed grant: {} names no directory",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(directory).map_err(|error| {
        ProxyError::authentication(format!(
            "could not write the refreshed grant: {} is not usable: {error}",
            directory.display()
        ))
    })?;

    let temporary = temporary_beside(path);
    write_private(&temporary, body)?;

    std::fs::rename(&temporary, path).map_err(|error| {
        // The temporary is removed on failure, or a profile directory would
        // fill with half-written grants nobody reads.
        let _ = std::fs::remove_file(&temporary);
        ProxyError::authentication(format!(
            "could not write the refreshed grant to {}: {error}",
            path.display()
        ))
    })
}

/// A name nothing else is using, beside the file being replaced.
///
/// The process id and a counter, because two refreshes of two profiles can be
/// in flight at once and a fixed suffix would have them writing one file.
fn temporary_beside(path: &Path) -> std::path::PathBuf {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let name = path.file_name().map_or_else(
        || "grant".to_owned(),
        |it| it.to_string_lossy().into_owned(),
    );
    let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        ".{name}.proxenos-{}-{suffix}.tmp",
        std::process::id()
    ))
}

#[cfg(unix)]
fn write_private(path: &Path, body: &str) -> Result<(), ProxyError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            ProxyError::authentication(format!(
                "could not write the refreshed grant to {}: {error}",
                path.display()
            ))
        })?;

    file.write_all(body.as_bytes()).map_err(|error| {
        ProxyError::authentication(format!(
            "could not write the refreshed grant to {}: {error}",
            path.display()
        ))
    })
}

#[cfg(not(unix))]
fn write_private(path: &Path, body: &str) -> Result<(), ProxyError> {
    // Windows has no mode bits, and §8.4 does not locate a profile there
    // anyway; this arm exists so the module compiles rather than because a
    // grant is expected here.
    std::fs::write(path, body).map_err(|error| {
        ProxyError::authentication(format!(
            "could not write the refreshed grant to {}: {error}",
            path.display()
        ))
    })
}
