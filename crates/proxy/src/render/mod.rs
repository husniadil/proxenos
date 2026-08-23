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

/// The widest cell wins: a table whose columns are sized to what is in them.
///
/// No row carries trailing space — the last column is never padded, and a row
/// whose last cell is empty loses the separator in front of it too. No
/// cell is ever shortened here — a column that has to fit inside a terminal
/// shortens its own cells before handing them over, because only the caller
/// knows which of them can lose characters and still mean something.
fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = header
        .iter()
        .enumerate()
        .map(|(column, title)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .chain(std::iter::once(title.chars().count()))
                .max()
                .unwrap_or_default()
        })
        .collect();

    let line = |cells: &[String]| {
        let last = cells.len().saturating_sub(1);
        cells
            .iter()
            .zip(widths.iter())
            .enumerate()
            .map(|(column, (cell, width))| {
                if column == last {
                    cell.clone()
                } else {
                    let pad = width.saturating_sub(cell.chars().count());
                    format!("{cell}{}", " ".repeat(pad))
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };

    let header: Vec<String> = header.iter().map(|cell| (*cell).to_owned()).collect();
    std::iter::once(line(&header))
        .chain(rows.iter().map(|row| line(row)))
        .collect::<Vec<_>>()
        .join("\n")
}
