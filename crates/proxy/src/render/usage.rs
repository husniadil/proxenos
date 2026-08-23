//! `docs/api.md` §3 — the quota meter.

use super::field;
use super::now;
use serde_json::Value;

/// `docs/api.md` §3 — the quota snapshot, in one line per window.
///
/// Written to be read by a person and parsed by a status line, which is what a
/// caller wanting this every few seconds is building. `--json` is there for the
/// second case; this is the first.
pub fn usage(result: &Value) -> String {
    usage_at(result, now())
}

/// The meter, rendered against a stated clock.
///
/// Separate from [`usage`] because window staleness is the one thing here that
/// is not a pure function of the answer: the same snapshot reads differently an
/// hour later, and a test asserting that cannot be at the mercy of the wall
/// clock.
#[must_use]
pub fn usage_at(result: &Value, now: u64) -> String {
    let mut lines = Vec::new();

    if field(result, "known").and_then(Value::as_bool) != Some(true) {
        lines.push(
            field(result, "detail")
                .and_then(Value::as_str)
                .unwrap_or("no quota has been reported")
                .to_owned(),
        );
        // Not a return. Another account may hold a figure, and a daemon that
        // printed nothing but "none yet" would hide the pinned tier's quota
        // from the one place a person looks for it.
        lines.extend(per_account(result, now));
        return lines.join("\n");
    }

    if let Some(plan) = field(result, "plan").and_then(Value::as_str) {
        lines.push(format!("plan       {plan}"));
    }
    if field(result, "limit_reached").and_then(Value::as_bool) == Some(true) {
        lines.push("limit      reached".to_owned());
    }

    let windows = field(result, "windows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if windows.is_empty() {
        lines.push("windows    none reported".to_owned());
    }

    for window in &windows {
        let used = field(window, "used_percent")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        lines.push(format!(
            "{:<10} {used:.0}% used{}",
            name_window(window),
            notes(window, now)
        ));
    }

    lines.extend(per_account(result, now));
    lines.join("\n")
}

/// One line per account, where there is more than one.
///
/// A single account's figure is the block above, and repeating it under its own
/// name says nothing. Two accounts can serve one session, and then whose figure
/// is whose is the whole question.
fn per_account(result: &Value, now: u64) -> Vec<String> {
    let accounts = field(result, "accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if accounts.len() < 2 {
        return Vec::new();
    }

    let mut lines = vec![String::new(), "accounts".to_owned()];
    lines.extend(accounts.iter().map(|account| {
        let name = field(account, "account")
            .and_then(Value::as_str)
            .unwrap_or("unnamed");
        let marker = if field(account, "serving").and_then(Value::as_bool) == Some(true) {
            "*"
        } else {
            " "
        };

        if field(account, "known").and_then(Value::as_bool) != Some(true) {
            let detail = field(account, "detail")
                .and_then(Value::as_str)
                .unwrap_or("no figure");
            return format!("{marker} {name:<24} no figure — {detail}");
        }

        let windows = field(account, "windows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let figures = if windows.is_empty() {
            "none reported".to_owned()
        } else {
            windows
                .iter()
                .map(|window| {
                    let used = field(window, "used_percent")
                        .and_then(Value::as_f64)
                        .unwrap_or_default();
                    format!(
                        "{} {used:.0}% used{}",
                        name_window(window),
                        notes(window, now)
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ")
        };

        // How the figure was come by. A figure that rode a turn is as old as
        // that turn; one that was asked for is as old as the asking.
        let source = match field(account, "source").and_then(Value::as_str) {
            Some("fetch") => "asked for",
            _ => "rode a turn",
        };
        format!("{marker} {name:<24} {figures}   {source}")
    }));
    lines
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

/// Everything the provider said about a window beyond its number.
///
/// All of it rides the same headers the figure does, and a meter printing the
/// number alone drops the provider's own warning, its own threshold, and its
/// own answer to which window decides. Each is said only where the provider
/// said it; nothing here is inferred from the percentage.
fn notes(window: &Value, now: u64) -> String {
    let mut notes = Vec::new();

    // A figure whose window has turned over describes a window that is back to
    // zero. It errs toward overstating — spend shown against an empty window
    // sends an operator to switch accounts they did not need to switch — so it
    // is said first.
    //
    // Through the same predicate the parse side uses, never a second copy of
    // the rule: two copies do not stay equal, and the one under test would be
    // the one that stayed right.
    if crate::usage::has_reset(field(window, "resets_at").and_then(Value::as_u64), now) {
        notes.push("window has since reset".to_owned());
    }

    // The provider names one window as the one that decides whether the
    // account is about to be cut off. With one window near empty and another
    // near full, a reader taking the first line reads the reassuring one.
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
        return String::new();
    }
    format!("   ({})", notes.join(", "))
}

/// A window length in the units a person would say it in.
fn describe_window(minutes: u64) -> String {
    match minutes {
        m if m % (24 * 60) == 0 => format!("{}d", m / (24 * 60)),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}
