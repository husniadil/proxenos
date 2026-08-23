//! `docs/api.md` §3 — the quota meter.

use super::field;
use super::now;
use super::table;
use serde_json::Value;

/// `docs/api.md` §3 — the quota snapshot, as a table.
///
/// Written to be read by a person and parsed by a status line, which is what a
/// caller wanting this every few seconds is building. `--json` is there for the
/// second case; this is the first.
pub fn usage(result: &Value) -> String {
    usage_at(result, now())
}

/// The meter, rendered against a stated clock.
///
/// Separate from [`usage`] because two of its columns are not pure functions of
/// the answer: a window's reset counts down and a figure's age counts up, so
/// the same snapshot reads differently an hour later and a test asserting that
/// cannot be at the mercy of the wall clock.
///
/// **One row per window, not one cell per account.** An account can hold a
/// five-hour window beside a seven-day one, and each has its own reset — so a
/// single cell reading `42% of 5h, 18% of 7d` would have nowhere to put the two
/// different answers to "when does that come back". The window rows after the
/// first carry only the figure and its reset, because the name, the provider,
/// and the freshness belong to the account rather than to the window.
#[must_use]
pub fn usage_at(result: &Value, now: u64) -> String {
    let mut lines = Vec::new();

    // The serving account's summary, above the table: what the whole answer is
    // about, and the one state that is not a per-window figure.
    if let Some(plan) = field(result, "plan").and_then(Value::as_str) {
        lines.push(format!("plan       {plan}"));
    }
    if field(result, "limit_reached").and_then(Value::as_bool) == Some(true) {
        lines.push("limit      reached".to_owned());
    }

    let accounts = field(result, "accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // A daemon that names no account still has a figure to show, and the
    // table is where it goes. What it cannot say is whose it is.
    let accounts = if accounts.is_empty() {
        vec![daemon_wide(result)]
    } else {
        accounts
    };

    let mut rows = Vec::new();
    for account in &accounts {
        rows.extend(account_rows(account, now));
    }
    lines.push(table(
        &["  NAME", "PROVIDER", "USED", "RESETS", "SOURCE", "AS OF"],
        &rows,
    ));

    if let Some(note) = note(&accounts) {
        lines.push(format!("note: {note}"));
    }

    lines.join("\n")
}

/// The daemon-wide figure as one nameless row.
///
/// Only where no account is listed at all. The figure is real and reporting
/// nothing would hide it; what is not known is which account earned it, and the
/// row says that by leaving the name blank rather than by borrowing one.
fn daemon_wide(result: &Value) -> Value {
    let mut entry = result.clone();
    if let Some(object) = entry.as_object_mut() {
        object.remove("accounts");
        let serving = field(result, "serving");
        object.insert(
            "account".to_owned(),
            serving
                .and_then(|serving| field(serving, "account"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        object.insert(
            "provider".to_owned(),
            serving
                .and_then(|serving| field(serving, "provider"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        object.insert("serving".to_owned(), Value::Bool(true));
    }
    entry
}

/// One account, as one row per window it holds.
fn account_rows(account: &Value, now: u64) -> Vec<Vec<String>> {
    let name = field(account, "account")
        .and_then(Value::as_str)
        .unwrap_or("-");
    let marker = if field(account, "serving").and_then(Value::as_bool) == Some(true) {
        "*"
    } else {
        " "
    };
    let head = format!("{marker} {name}");
    let provider = field(account, "provider")
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_owned();

    if field(account, "known").and_then(Value::as_bool) != Some(true) {
        return vec![vec![
            head,
            provider,
            used_without_a_figure(account),
            "-".to_owned(),
            "-".to_owned(),
            why(account),
        ]];
    }

    // How the figure was come by. A figure that rode a turn is as old as that
    // turn; one that was asked for is as old as the asking.
    let source = match field(account, "source").and_then(Value::as_str) {
        Some("fetch") => "asked",
        _ => "last turn",
    };
    let age = field(account, "measured_at")
        .and_then(Value::as_u64)
        .map_or_else(|| "-".to_owned(), |at| ago(now.saturating_sub(at)));

    let windows = field(account, "windows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if windows.is_empty() {
        return vec![vec![
            head,
            provider,
            "none reported".to_owned(),
            "-".to_owned(),
            source.to_owned(),
            age,
        ]];
    }

    windows
        .iter()
        .enumerate()
        .map(|(index, window)| {
            // Everything but the figure belongs to the account, so it is said
            // once. A name repeated down a column reads as several accounts.
            let (head, provider, source, age) = if index == 0 {
                (
                    head.clone(),
                    provider.clone(),
                    source.to_owned(),
                    age.clone(),
                )
            } else {
                (String::new(), String::new(), String::new(), String::new())
            };
            vec![
                head,
                provider,
                used(window),
                resets(window, now),
                source,
                age,
            ]
        })
        .collect()
}

/// The figure, and the provider's own words about the window it describes.
///
/// All of it rides the same headers the number does, and a meter printing the
/// number alone drops the provider's own warning and its own answer to which
/// window decides. Each is said only where the provider said it; nothing here
/// is inferred from the percentage.
fn used(window: &Value) -> String {
    let percent = field(window, "used_percent")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let figure = format!("{percent:.0}% of {}", name_window(window));

    let mut notes = Vec::new();
    // The provider names one window as the one that decides whether the
    // account is about to be cut off. With one window near empty and another
    // near full, a reader taking the first row reads the reassuring one.
    if field(window, "representative").and_then(Value::as_bool) == Some(true) {
        notes.push("decides".to_owned());
    }
    // A turn that went through can still carry a warning the provider attached
    // to it, against a threshold the provider itself set.
    if field(window, "status").and_then(Value::as_str) == Some("allowed_warning") {
        notes.push(
            match field(window, "surpassed_threshold").and_then(Value::as_f64) {
                Some(threshold) => format!("past the provider's {:.0}%", threshold * 100.0),
                None => "the provider warns on this window".to_owned(),
            },
        );
    }

    if notes.is_empty() {
        return figure;
    }
    format!("{figure} ({})", notes.join(", "))
}

/// What the USED cell says for a row with no figure at all.
///
/// A metered account is the one absence that is not a figure pending: it has no
/// ceiling because it is billed per token, so the cell carries the one honest
/// quantity there is — what this daemon counted as it.
fn used_without_a_figure(account: &Value) -> String {
    if field(account, "reason").and_then(Value::as_str) != Some("metered") {
        return "-".to_owned();
    }
    match field(account, "served_tokens").and_then(Value::as_u64) {
        Some(served) if served > 0 => format!("{served} tok"),
        // Nothing served yet is a true zero rather than an absence, but a bare
        // `0 tok` in a column of percentages reads as a quota figure. The word
        // says what the row is instead.
        _ => "metered".to_owned(),
    }
}

/// When the window comes back, or that it already has.
///
/// A figure whose window has turned over describes a window that is back to
/// zero. It errs toward overstating — spend shown against an empty window sends
/// an operator to switch accounts they did not need to switch — so the cell
/// says so outright.
///
/// Through the same predicate the parse side uses, never a second copy of the
/// rule: two copies do not stay equal, and the one under test would be the one
/// that stayed right.
fn resets(window: &Value, now: u64) -> String {
    let Some(resets_at) = field(window, "resets_at").and_then(Value::as_u64) else {
        return "-".to_owned();
    };
    if crate::usage::has_reset(Some(resets_at), now) {
        return "already reset".to_owned();
    }
    until(resets_at - now)
}

/// Why a row has no figure, in the width of a cell.
///
/// From the payload's own code rather than from its sentence: the sentence is
/// written for a reader and is free to be reworded, and a renderer matching on
/// prose breaks silently when it is. A daemon that states no code says the one
/// thing that is true of every such row.
fn why(account: &Value) -> String {
    match field(account, "reason").and_then(Value::as_str) {
        Some("no_turn") => "no turn yet",
        Some("no_relayed_turn") => "no relayed turn yet",
        Some("metered") => "per token",
        Some("not_reported") => "not reported",
        Some("unknown_key_kind") => "key kind unknown",
        _ => "no figure",
    }
    .to_owned()
}

/// The long explanations, said once under the table rather than on every row.
///
/// Each clause is included only where a row it describes is present, so the
/// note answers the table in front of the reader rather than every table this
/// daemon could print.
fn note(accounts: &[Value]) -> Option<String> {
    let reason_of = |account: &Value| {
        if field(account, "known").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        Some(
            field(account, "reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        )
    };
    let reasons: Vec<String> = accounts.iter().filter_map(reason_of).collect();

    let mut clauses = Vec::new();
    if reasons
        .iter()
        .any(|reason| reason != "metered" && reason != "not_reported")
    {
        clauses.push(
            "an empty row is this daemon's record rather than the account's — a figure arrives \
             with the next turn served as it, or now with `usage --refresh`"
                .to_owned(),
        );
    }
    if reasons.iter().any(|reason| reason == "metered") {
        clauses.push(
            "a metered row has no ceiling to report, and its count is what this daemon served \
             as it rather than the whole of its spend"
                .to_owned(),
        );
    }
    if reasons.iter().any(|reason| reason == "not_reported") {
        clauses.push("`not reported` is a provider that states no quota to this proxy".to_owned());
    }

    if clauses.is_empty() {
        return None;
    }
    Some(format!("{}.", clauses.join("; ")))
}

/// What to call a window.
///
/// Duration where the provider stated one, and the provider's own name where it
/// did not — an overage window has a figure and a reset and no length at all,
/// and "unknown window" says less than the word the provider used.
fn name_window(window: &Value) -> String {
    if let Some(minutes) = field(window, "window_minutes").and_then(Value::as_u64) {
        return describe_window(minutes);
    }
    field(window, "label")
        .and_then(Value::as_str)
        .map_or_else(|| "unknown window".to_owned(), str::to_owned)
}

/// A window length in the units a person would say it in.
fn describe_window(minutes: u64) -> String {
    match minutes {
        m if m % (24 * 60) == 0 => format!("{}d", m / (24 * 60)),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

/// How long until something, in the two largest units that are not zero.
fn until(seconds: u64) -> String {
    match seconds {
        s if s < 60 => "in under a minute".to_owned(),
        s if s < 3_600 => format!("in {}m", s / 60),
        s if s < 86_400 => match (s / 3_600, (s % 3_600) / 60) {
            (hours, 0) => format!("in {hours}h"),
            (hours, minutes) => format!("in {hours}h {minutes}m"),
        },
        s => match (s / 86_400, (s % 86_400) / 3_600) {
            (days, 0) => format!("in {days}d"),
            (days, hours) => format!("in {days}d {hours}h"),
        },
    }
}

/// How old a figure is, in one unit. A meter is read at a glance, and the
/// second unit on an age never changes what the reader does about it.
fn ago(seconds: u64) -> String {
    match seconds {
        s if s < 60 => "just now".to_owned(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s => format!("{}d ago", s / 86_400),
    }
}
