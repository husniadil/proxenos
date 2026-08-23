//! Turning control-socket results into terminal output.
//!
//! Presentation only. The daemon holds the state and decides what is true; this
//! decides how it reads.

use serde_json::Value;

mod accounts;
mod launch;
mod status;
mod usage;

pub use accounts::*;
pub use launch::*;
pub use status::*;
pub use usage::*;

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name)
}

/// Quote only when the value needs it, so the common case stays readable.
fn quote(value: &str) -> String {
    let safe = value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':')
    });
    if safe {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// How close a login has to be to expiring before it is mentioned.
///
/// Silent while it is far off. A row carrying a date eleven months of the year
/// is a row the reader learns to skip, and this one has to land on the week it
/// appears. Wider than the client's own notice, which starts at three days —
/// long enough that a weekend does not swallow it.
const RENEWAL_NOTICE_SECONDS: u64 = 7 * 24 * 60 * 60;

/// What to say about a login that has to be renewed, if anything.
///
/// Known only for a borrowed Claude profile: its stored item records the date,
/// and a Codex profile records nothing equivalent (§8.4). Absent is silence,
/// never a guess.
///
/// It is worth saying ahead of time because of what happens after: past this
/// date the client cannot refresh the profile, and asking it to try blanks
/// what is left of the stored grant. This is the notice that turns that into
/// something an operator does on purpose.
fn renewal(login_expires_at: Option<u64>, now: u64) -> Option<String> {
    let expiry = login_expires_at?;
    if expiry <= now {
        return Some("login expired".to_owned());
    }
    let left = expiry - now;
    if left > RENEWAL_NOTICE_SECONDS {
        return None;
    }
    Some(match left / 86_400 {
        0 => "login expires today".to_owned(),
        1 => "login expires tomorrow".to_owned(),
        days => format!("login expires in {days} days"),
    })
}

/// Epoch seconds. A meter that says a window has turned over needs a clock to
/// say it against.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
