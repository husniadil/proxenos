//! `docs/api.md` §3 — what quota is left.
//!
//! The backend opens every stream with a snapshot of the account's rate limits,
//! before it says anything about the response. That is free — it rides along
//! with a turn already being made — and it is the only place this figure
//! appears, so it is read there rather than polled.
//!
//! **Nothing here is computed.** A window the backend did not report is absent
//! rather than zero, and a percentage is passed through as given. An invented
//! quota figure is worse than no figure: it reads as headroom that is not there.

use serde_json::Value;
use std::sync::Mutex;

/// One quota window as the backend reports it.
///
/// Serialized because a snapshot outlives the process that took it (§6.1), and
/// nothing in it is any part of a credential.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Window {
    pub used_percent: f64,
    /// How long the window is. The backend has changed which windows it
    /// reports, so this is what identifies one — not its position.
    pub window_minutes: Option<u64>,
    /// Epoch seconds.
    pub resets_at: Option<u64>,
    /// The provider's own name for a window whose length does not identify it.
    ///
    /// An overage window has a figure and a reset and no duration at all, so
    /// duration cannot be what names it. Absent where the duration already
    /// does the naming.
    pub label: Option<String>,
    /// The provider's own word on this window — `allowed`, `allowed_warning`,
    /// `rejected`.
    ///
    /// A turn that went through can still carry a warning the provider
    /// attached to it, and the number alone does not say so.
    pub status: Option<String>,
    /// The fraction at which the provider itself starts warning, where it said
    /// one. A threshold nobody stated is absent rather than assumed.
    pub surpassed_threshold: Option<f64>,
    /// Whether the provider named this window as the one that speaks for the
    /// account.
    ///
    /// With one window near empty and another near full in the same snapshot,
    /// this is the provider's own answer to which decides whether the account
    /// is about to be cut off.
    pub representative: bool,
}

impl Window {
    /// Whether this window's figure describes a window that has since turned
    /// over.
    ///
    /// **A property of one window, never of a snapshot.** One snapshot can
    /// hold a five-hour window whose reset has passed beside a seven-day one
    /// whose has not, and marking the snapshot would be wrong in both
    /// directions at once — hiding a figure that is still true, or passing one
    /// that is not.
    ///
    /// A window the provider stated no reset for is never called stale.
    /// Guessing it has turned over would drop a figure on no evidence, and the
    /// error this exists to prevent is the opposite one: a spent figure shown
    /// against an empty window sends an operator to switch accounts they did
    /// not need to switch.
    #[must_use]
    pub fn is_stale_at(&self, now: u64) -> bool {
        has_reset(self.resets_at, now)
    }
}

/// Whether a window whose reset the provider stated has since turned over.
///
/// **One definition, reached from both sides.** The parse side holds a
/// `Window` and the meter holds the JSON it became, so neither can call the
/// other's shape — which is exactly how a rule like this ends up written twice,
/// with the tested copy and the shipping copy free to drift apart. Both go
/// through here.
///
/// `None` is never stale: a window the provider stated no reset for cannot be
/// said to have turned over, and guessing would drop a figure on no evidence.
#[must_use]
/// Epoch seconds from an RFC 3339 timestamp, for the one provider that states
/// a reset that way.
///
/// Deliberately small: it reads the shape that provider actually sends —
/// `2026-08-23T09:00:00.383476+00:00`, always UTC — and answers `None` for
/// anything else rather than guessing at an offset. A wrong answer here marks
/// a live window stale, or a stale one live.
pub fn epoch_from_rfc3339(text: &str) -> Option<u64> {
    let (date, rest) = text.split_once('T')?;
    let (year, month, day) = {
        let mut parts = date.split('-');
        (
            parts.next()?.parse::<i64>().ok()?,
            parts.next()?.parse::<i64>().ok()?,
            parts.next()?.parse::<i64>().ok()?,
        )
    };

    // Only UTC. Every timestamp seen from this endpoint is UTC, and reading an
    // offset we have not seen would be inventing behaviour.
    let time = rest.strip_suffix('Z').or_else(|| {
        rest.strip_suffix("+00:00")
            .or_else(|| rest.strip_suffix("+0000"))
    })?;
    let time = time.split('.').next()?;
    let mut clock = time.split(':');
    let (hour, minute, second) = (
        clock.next()?.parse::<i64>().ok()?,
        clock.next()?.parse::<i64>().ok()?,
        clock.next()?.parse::<i64>().ok()?,
    );

    // Days from the civil calendar, Howard Hinnant's algorithm: March-based
    // years make the leap day the last of the year, which is what removes the
    // special cases.
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

pub fn has_reset(resets_at: Option<u64>, now: u64) -> bool {
    resets_at.is_some_and(|resets_at| now >= resets_at)
}

/// The account's quota, as of one turn.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub plan: Option<String>,
    pub limit_reached: bool,
    pub windows: Vec<Window>,
}

/// The header names this client reads a quota out of.
///
/// Two fixed windows, five hours and seven days. The backend's windows are not
/// fixed — it has reported a five-hour window in the past, does not now, and may
/// again — so windows are matched to slots by *duration*. A window that matches
/// neither slot is reported through the control socket, where it can state its
/// real length, and is not forced into a slot that would misname it.
const FIVE_HOURS: u64 = 5 * 60;
const SEVEN_DAYS: u64 = 7 * 24 * 60;

/// How far from the nominal duration still counts as that window.
///
/// Generous, because the point is to recognize a window the backend calls five
/// hours even if it reports 299 minutes — and narrow enough that a thirty-day
/// window can never be mistaken for either.
const TOLERANCE: f64 = 0.25;

fn matches(window_minutes: u64, nominal: u64) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let (actual, nominal) = (window_minutes as f64, nominal as f64);
    (actual - nominal).abs() <= nominal * TOLERANCE
}

impl Snapshot {
    /// Read a snapshot out of one upstream event, if that is what it is.
    pub fn parse(payload: &str) -> Option<Self> {
        let event: Value = serde_json::from_str(payload).ok()?;
        if event.get("type").and_then(Value::as_str) != Some("codex.rate_limits") {
            return None;
        }

        let limits = event.get("rate_limits")?;
        let windows = ["primary", "secondary"]
            .into_iter()
            .filter_map(|name| parse_window(limits.get(name)?))
            .collect();

        Some(Self {
            plan: event
                .get("plan_type")
                .and_then(Value::as_str)
                .map(str::to_owned),
            limit_reached: limits
                .get("limit_reached")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            windows,
        })
    }

    /// Read a snapshot out of the quota endpoint's response.
    ///
    /// **The shape is not the stream event's**, and the three differences are
    /// exactly where a guess would have gone wrong: the windows are
    /// `primary_window`/`secondary_window` rather than `primary`/`secondary`,
    /// their length is stated in **seconds**, and the plan sits at the top
    /// level. A parser written from the stream shape parses this into nothing
    /// and reports no quota on an account that has one — which is why the
    /// fixture behind this was captured before any of it was written.
    ///
    /// `None` where the body is not this response at all. An empty snapshot
    /// would read as "quota known, nothing used", which is the reassuring
    /// direction to be wrong in.
    pub fn parse_rest(payload: &str) -> Option<Self> {
        let body: Value = serde_json::from_str(payload).ok()?;
        let limits = body.get("rate_limit")?;

        let windows: Vec<Window> = ["primary_window", "secondary_window"]
            .into_iter()
            .filter_map(|name| parse_rest_window(limits.get(name)?))
            .collect();

        // A response carrying no window at all says nothing about quota, and
        // saying nothing is not the same as saying none is used.
        if windows.is_empty() {
            return None;
        }

        Some(Self {
            plan: body
                .get("plan_type")
                .and_then(Value::as_str)
                .map(str::to_owned),
            limit_reached: limits
                .get("limit_reached")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            windows,
        })
    }

    /// Read a snapshot out of the second provider's quota endpoint.
    ///
    /// A third shape, and it shares nothing with the other two: the windows are
    /// named rather than positional (`five_hour`, `seven_day`), the figure is
    /// already a percentage, and the reset is an RFC 3339 timestamp instead of
    /// an epoch. Everything else in the body — spend, extra usage, and several
    /// windows whose names read as internal flags — is left where it is.
    ///
    /// `limits` carries the provider's own word on each window, and it is read
    /// rather than inferred from the percentage: an account can sit high on a
    /// window the provider is still calling normal.
    ///
    /// `None` where the body carries no window at all. An empty snapshot would
    /// read as "quota known, nothing used", which is the reassuring direction
    /// to be wrong in.
    pub fn parse_anthropic(payload: &str) -> Option<Self> {
        let body: Value = serde_json::from_str(payload).ok()?;

        // By `group`, and only the entry with no `scope`.
        //
        // `kind` is not the field to match on: one group carries several of
        // them — `weekly_all` beside `weekly_scoped` — and matching a name
        // guessed from the group's own reads nothing at all, silently. The
        // scoped entry describes one model rather than the window, and its
        // figure says so: measured on one account, `weekly_all` reported 23%
        // against a `seven_day` utilisation of 23.0 while `weekly_scoped`
        // reported 0.
        let severity_of = |group: &str| -> Option<String> {
            body.get("limits")?
                .as_array()?
                .iter()
                .find(|limit| {
                    limit.get("group").and_then(Value::as_str) == Some(group)
                        && limit.get("scope").is_none_or(Value::is_null)
                })?
                .get("severity")?
                .as_str()
                .map(str::to_owned)
        };

        let mut windows: Vec<Window> = [
            ("five_hour", FIVE_HOURS, "session"),
            ("seven_day", SEVEN_DAYS, "weekly"),
        ]
        .into_iter()
        .filter_map(|(name, minutes, group)| {
            let window = body.get(name)?;
            Some(Window {
                used_percent: window.get("utilization")?.as_f64()?,
                window_minutes: Some(minutes),
                resets_at: window
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .and_then(epoch_from_rfc3339),
                label: None,
                status: severity_of(group),
                // No threshold is published here, and inventing one would put a
                // figure in the provider's mouth.
                surpassed_threshold: None,
                // Nothing in this body names one window as deciding for the
                // account, so none is marked.
                representative: false,
            })
        })
        .collect();

        // A scoped entry is one model's figure. It is kept as its own window,
        // named by the model rather than measured: giving it the group's
        // duration would put a second seven-day window where a duration
        // lookup expects the account's.
        if let Some(limits) = body.get("limits").and_then(Value::as_array) {
            windows.extend(limits.iter().filter_map(|limit| {
                let model = limit
                    .get("scope")?
                    .get("model")?
                    .get("display_name")?
                    .as_str()?;
                Some(Window {
                    used_percent: limit.get("percent")?.as_f64()?,
                    window_minutes: None,
                    resets_at: limit
                        .get("resets_at")
                        .and_then(Value::as_str)
                        .and_then(epoch_from_rfc3339),
                    label: Some(model.to_owned()),
                    status: limit
                        .get("severity")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    surpassed_threshold: None,
                    representative: false,
                })
            }));
        }

        if windows.is_empty() {
            return None;
        }

        Some(Self {
            // The body states no plan. It is knowable from the stored grant
            // instead, and reporting it from here would be a second answer to
            // one question.
            plan: None,
            // Stated by a refusal, not by a percentage. Nothing in this body
            // says a turn would be rejected, so nothing here claims one would.
            limit_reached: false,
            windows,
        })
    }

    /// The plan a profile body states, with its multiplier where one exists.
    ///
    /// The quota body states no plan; the profile endpoint beside it does, as
    /// `organization.organization_type` — and for a max org the multiplier
    /// rides separately in `rate_limit_tier` (`default_claude_max_20x`). An
    /// organization type this does not recognize yields no plan rather than a
    /// guessed one.
    pub fn plan_from_anthropic_profile(payload: &str) -> Option<String> {
        let body: Value = serde_json::from_str(payload).ok()?;
        let organization = body.get("organization")?;
        let kind = organization.get("organization_type")?.as_str()?;

        match kind {
            "claude_max" => {
                let multiplier = organization
                    .get("rate_limit_tier")
                    .and_then(Value::as_str)
                    .and_then(|tier| tier.rsplit('_').next())
                    .filter(|last| {
                        last.strip_suffix('x')
                            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    });
                Some(match multiplier {
                    Some(multiplier) => format!("max {multiplier}"),
                    None => "max".to_owned(),
                })
            }
            "claude_pro" => Some("pro".to_owned()),
            "claude_team" | "claude_teams" => Some("team".to_owned()),
            "claude_enterprise" => Some("enterprise".to_owned()),
            "claude_free" | "free" => Some("free".to_owned()),
            _ => None,
        }
    }

    /// Read a snapshot out of a relayed response's own headers.
    ///
    /// The second provider states quota in the response headers of every turn,
    /// and for a subscription token that is the *only* place it states one —
    /// its usage endpoint refuses that credential for want of a scope. So this
    /// is the primary path there, not a fallback, and it costs nothing: the
    /// figure rides a turn already being made.
    ///
    /// This is the inverse of [`Snapshot::headers`], and deliberately reads the
    /// same names: what this proxy hands its own client on one path is what the
    /// provider hands this proxy on the other. Utilization is a fraction on the
    /// wire and a percentage in a snapshot, which is the one conversion here.
    ///
    /// Plan is absent because no header states one. An account's plan name is
    /// not derivable from its headroom, and guessing it would put a word in the
    /// provider's mouth.
    ///
    /// `None` where no window was reported at all. An empty snapshot would read
    /// as "quota known, nothing used" — the reassuring direction to be wrong in.
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Option<Self> {
        let read = |name: &str| headers.get(name)?.to_str().ok()?.parse::<f64>().ok();

        let text = |name: &str| Some(headers.get(name)?.to_str().ok()?.to_owned());

        // Which window the provider says speaks for the account. Its own
        // vocabulary, not a duration, so it is matched to a slot by name.
        let representative = text("anthropic-ratelimit-unified-representative-claim");
        let representative = representative.as_deref();

        // The overage window carries a figure and a reset and no duration at
        // all, so it is named rather than measured. Dropping it silently is
        // not the same as deciding it does not belong.
        let windows: Vec<Window> = [
            (Some(FIVE_HOURS), None, "5h", "five_hour"),
            (Some(SEVEN_DAYS), None, "7d", "seven_day"),
            (None, Some("overage"), "overage", "overage"),
        ]
        .into_iter()
        .filter_map(|(nominal, label, slot, claim)| {
            let utilization = read(&format!("anthropic-ratelimit-unified-{slot}-utilization"))?;
            Some(Window {
                used_percent: (utilization * 100.0).clamp(0.0, 100.0),
                window_minutes: nominal,
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                resets_at: read(&format!("anthropic-ratelimit-unified-{slot}-reset"))
                    .map(|reset| reset as u64),
                label: label.map(str::to_owned),
                status: text(&format!("anthropic-ratelimit-unified-{slot}-status")),
                surpassed_threshold: read(&format!(
                    "anthropic-ratelimit-unified-{slot}-surpassed-threshold"
                )),
                representative: representative == Some(claim),
            })
        })
        .collect();

        if windows.is_empty() {
            return None;
        }

        Some(Self {
            plan: None,
            // Only an outright refusal is the limit being reached. The
            // provider also says `allowed_warning`, which is a turn that went
            // through, and reading that as rejected would show a limit the
            // account has not hit.
            limit_reached: headers
                .get("anthropic-ratelimit-unified-status")
                .and_then(|status| status.to_str().ok())
                == Some("rejected"),
            windows,
        })
    }

    /// The window matching a nominal duration, if the backend reported one.
    fn window_of(&self, nominal: u64) -> Option<&Window> {
        self.windows
            .iter()
            .find(|window| window.window_minutes.is_some_and(|m| matches(m, nominal)))
    }

    /// Response headers carrying this snapshot, in the form the client parses.
    ///
    /// Utilization is a fraction rather than a percentage — that is the form
    /// the header takes. Only windows that genuinely match a slot appear: a
    /// thirty-day window announced as a five-hour one would show a meter that
    /// is wrong in the reassuring direction.
    ///
    /// **This is deliberately narrower than [`Snapshot::from_headers`], and
    /// the asymmetry is the point.** That side reads whatever the provider
    /// chose to state — a per-window status, the threshold behind it, which
    /// window it calls representative, an overage window with no duration at
    /// all — because a figure in hand and dropped is a figure lost. This side
    /// states only what has been observed being read back: the two utilization
    /// slots, their resets, and the one unified status. Emitting the rest would
    /// be this proxy asserting a limit of its own in the provider's vocabulary,
    /// on a contract nobody has verified a client honours. The operator reaches
    /// all of it through `usage`, which is where a figure with no header slot
    /// belongs (`api.md` §3).
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = Vec::new();

        for (nominal, utilization, reset) in [
            (
                FIVE_HOURS,
                "anthropic-ratelimit-unified-5h-utilization",
                "anthropic-ratelimit-unified-5h-reset",
            ),
            (
                SEVEN_DAYS,
                "anthropic-ratelimit-unified-7d-utilization",
                "anthropic-ratelimit-unified-7d-reset",
            ),
        ] {
            let Some(window) = self.window_of(nominal) else {
                continue;
            };
            headers.push((utilization, format!("{:.4}", window.used_percent / 100.0)));
            if let Some(resets_at) = window.resets_at {
                headers.push((reset, resets_at.to_string()));
            }
        }

        // Said only when a window was reported, because "allowed" asserts
        // something about a limit, and no limit was seen means no assertion.
        if !headers.is_empty() {
            headers.push((
                "anthropic-ratelimit-unified-status",
                if self.limit_reached {
                    "rejected".to_owned()
                } else {
                    "allowed".to_owned()
                },
            ));
        }

        headers
    }

    /// The snapshot as the control socket reports it, windows and all.
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "known": true,
            "plan": self.plan,
            "limit_reached": self.limit_reached,
            "windows": self.windows.iter().map(|window| serde_json::json!({
                "used_percent": window.used_percent,
                "window_minutes": window.window_minutes,
                "resets_at": window.resets_at,
                "label": window.label,
                "status": window.status,
                "surpassed_threshold": window.surpassed_threshold,
                "representative": window.representative,
            })).collect::<Vec<_>>(),
        })
    }
}

fn parse_window(value: &Value) -> Option<Window> {
    // A window with no percentage says nothing, and reporting it as zero used
    // would be a figure the backend never gave.
    let used_percent = value.get("used_percent").and_then(Value::as_f64)?;

    Some(Window {
        used_percent: used_percent.clamp(0.0, 100.0),
        window_minutes: value.get("window_minutes").and_then(Value::as_u64),
        resets_at: value.get("reset_at").and_then(Value::as_u64),
        ..Window::default()
    })
}

/// One window from the quota endpoint.
///
/// Seconds on the wire, minutes in the snapshot — the unit every other reader
/// of a window already uses, and converting here is what lets one `Snapshot`
/// serve both sources.
fn parse_rest_window(value: &Value) -> Option<Window> {
    let used_percent = value.get("used_percent").and_then(Value::as_f64)?;

    Some(Window {
        used_percent: used_percent.clamp(0.0, 100.0),
        window_minutes: value
            .get("limit_window_seconds")
            .and_then(Value::as_u64)
            .map(|seconds| seconds / 60),
        resets_at: value.get("reset_at").and_then(Value::as_u64),
        ..Window::default()
    })
}

/// How a figure was come by.
///
/// A figure that rode a turn and a figure that was asked for are both
/// legitimate and differently stale, and a meter that showed one as the other
/// would be stating an age it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Volunteered at the head of a stream, by a turn that was being made
    /// anyway.
    Turn,
    /// Asked for over the control socket.
    Fetch,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Fetch => "fetch",
        }
    }
}

/// One account's quota, with its age.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Measured {
    pub snapshot: Snapshot,
    pub source: Source,
    /// Epoch seconds, as of when the figure was taken.
    ///
    /// `0` where a stored record did not state one, which is not a moment and
    /// is why such a record is not restored: a figure that cannot be dated
    /// renders with no age, and a figure with no age reads as current.
    #[serde(default)]
    pub at: u64,
}

/// What this daemon has served as one account, in tokens upstream counted.
///
/// Not a quota and not a cost. A key carries no entitlement to report a
/// percentage against and nothing here knows a price list (§6.1), but the
/// counts on a completed response are upstream's own — so the one thing that
/// can honestly be said about a metered account is how much of it has been
/// spent through this daemon, as a quantity.
///
/// **Counted by this daemon, and only through it.** The tally is written to
/// disk and read back at startup, so it survives a restart — nothing upstream
/// can restate it, and a restart that reset it to zero would state a floor
/// that is not true. Turns made anywhere else are still invisible to it, so it
/// remains a floor under the account's real spend rather than the whole of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Spent {
    pub input: u64,
    pub output: u64,
}

impl Spent {
    #[must_use]
    pub fn total(self) -> u64 {
        self.input.saturating_add(self.output)
    }
}

/// Which account this daemon serves unpinned turns as, asked at the moment a
/// figure is recorded.
///
/// A resolver rather than a name, because the answer moves: the operator can
/// select another account on a running daemon, and a figure belongs to
/// whoever served the turn it came from rather than to whoever is serving when
/// someone asks.
pub type ServingAccount = std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// What a tally file holds: an account name and two token counts, and no place
/// for anything else. Nothing here is a credential (CLAUDE.md #7).
fn read_tally(path: &std::path::Path) -> Option<std::collections::BTreeMap<String, Spent>> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// What a snapshot file holds: one measured figure per account, and nothing
/// that is any part of a credential (CLAUDE.md #7).
fn read_quota(path: &std::path::Path) -> Option<std::collections::BTreeMap<String, Measured>> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// What survives a restart, of one account's figure.
///
/// **The reset time is what makes a stored figure true or false**, and it is
/// the only thing that can decide: a percentage says nothing about whether the
/// window it was measured in still exists. So a window whose reset has passed
/// is dropped — it describes a window that is back to zero, and showing it
/// reads as spend the account has not made — and a window the provider stated
/// no reset for is dropped too, because nothing about it can be shown to still
/// be true after an arbitrary gap.
///
/// An account left with no window at all is not restored: an empty snapshot
/// reads as "quota known, nothing used", which is the reassuring direction to
/// be wrong in. Nor is a record that cannot be dated, since the age is half of
/// what the meter prints.
fn restore(mut measured: Measured, now: u64) -> Option<Measured> {
    if measured.at == 0 {
        return None;
    }
    measured
        .snapshot
        .windows
        .retain(|window| window.resets_at.is_some() && !window.is_stale_at(now));
    if measured.snapshot.windows.is_empty() {
        return None;
    }
    Some(measured)
}

/// Take whichever count is higher per account. A tally only ever grows, so the
/// higher of two counts is the one closer to what was actually served.
fn merge_into(
    merged: &mut std::collections::BTreeMap<String, Spent>,
    held: &std::collections::BTreeMap<String, Spent>,
) {
    for (name, spent) in held {
        let entry = merged.entry(name.clone()).or_default();
        entry.input = entry.input.max(spent.input);
        entry.output = entry.output.max(spent.output);
    }
}

/// Replace the file, never write into it.
///
/// `std::fs::write` truncates and then fills, and a daemon killed between those
/// two leaves a short file that parses into nothing — which `tallying_at` reads
/// as an empty tally and reports as a floor of zero. That is the exact defect
/// this file exists to remove, and `proxenos stop` under the supervisor kills
/// the daemon on every install. So the body goes to a sibling and is renamed
/// over the target, and a reader sees the old file or the new one. The
/// replacement carries the process id because two daemons writing one temporary
/// path would interleave into a file that is neither.
///
/// Written at the process umask rather than `0600`, deliberately: this holds an
/// account name and two token counts and no part of any credential, so the
/// restriction the credential store needs would be a claim about this file that
/// is not true.
fn write_and_flush(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()
}

fn replace_file(path: &std::path::Path, body: &str) {
    let mut pending = path.to_path_buf().into_os_string();
    pending.push(format!(".{}.pending", std::process::id()));
    let pending = std::path::PathBuf::from(pending);

    // Flushed before the rename, not after: a rename that lands ahead of the
    // data it names would expose an empty file on a crash, which is the same
    // floor-of-zero by a different route.
    if write_and_flush(&pending, body).is_err() {
        let _ = std::fs::remove_file(&pending);
        return;
    }
    if std::fs::rename(&pending, path).is_err() {
        let _ = std::fs::remove_file(&pending);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The latest quota figure per account, for whoever asks between turns.
///
/// **One snapshot per account, not one per daemon.** Two accounts can serve
/// one session — a pinned tier's turns spend the account it names while the
/// rest spend the serving one — so a single latest snapshot reports whichever
/// account made the most recent turn as though it were the account being
/// asked about.
#[derive(Default)]
pub struct UsageStore {
    by_account: Mutex<std::collections::BTreeMap<String, Measured>>,
    /// A figure no account could be named for.
    ///
    /// Only where this daemon has no way to answer "who is serving" — a test
    /// harness driving the ingress with no credential store behind it. It is
    /// reported where the daemon-wide figure has always been reported and is
    /// never attributed to an account, because nothing here knows which one it
    /// belongs to.
    unattributed: Mutex<Option<Measured>>,
    serving: Option<ServingAccount>,
    /// Every model id a turn has actually been made against.
    ///
    /// The configured tiers are the ids this daemon is *set up* to serve; an id
    /// the client sent itself passes straight through and is never one of them.
    /// Both are needed to answer "is this session mine" for a status line, and
    /// only a turn can report the second kind.
    served: Mutex<std::collections::BTreeSet<String>>,
    /// Tokens served per account, as upstream counted them.
    spent: Mutex<std::collections::BTreeMap<String, Spent>>,
    /// The plan the profile endpoint last stated per account, and when.
    ///
    /// The profile is asked for at most hourly: a plan changes on the scale of
    /// billing, and asking beside every quota refresh would spend a request to
    /// be told the same word.
    profile_plans: Mutex<std::collections::BTreeMap<String, (u64, String)>>,
    /// Where the tally is written, if it is written anywhere.
    ///
    /// `None` in a test harness and in `doctor`, which have no daemon state
    /// directory to write into and nothing to carry across a restart.
    tally: Option<std::path::PathBuf>,
    /// Where the quota snapshots are written, if they are written anywhere.
    quota: Option<std::path::PathBuf>,
    /// Fired in the window a write can be lost in, so a test can make it
    /// happen. Nothing outside a test sets it.
    #[allow(clippy::type_complexity)]
    on_tally_write: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// How many times a write starts over when it finds the file changed.
///
/// Each attempt is a read, a merge and a replacement with nothing slow in
/// between, so losing several in a row is not contention. The last attempt
/// writes what it has: this is a floor and a lost update leaves a smaller
/// floor, which is the failure this whole file is careful about — but spinning
/// on the ingress path to chase it would be worse than the count it saves.
const TALLY_WRITE_ATTEMPTS: usize = 5;

impl std::fmt::Debug for UsageStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("UsageStore").finish_non_exhaustive()
    }
}

impl UsageStore {
    /// Bound to the accounts this daemon holds.
    ///
    /// The serving account is whichever one the credential store has selected,
    /// asked each time a figure is recorded rather than read once at startup:
    /// the selection moves on a running daemon, and a figure belongs to the
    /// account that served the turn it rode in on.
    #[must_use]
    pub fn for_accounts(store: std::sync::Arc<dyn crate::auth::store::AccountStore>) -> Self {
        Self::default().serving(std::sync::Arc::new(move || {
            store
                .accounts()
                .ok()?
                .into_iter()
                .find(|account| account.selected)
                .map(|account| account.name)
        }))
    }

    /// Bind the store to whoever is serving turns.
    #[must_use]
    pub fn serving(mut self, serving: ServingAccount) -> Self {
        self.serving = Some(serving);
        self
    }

    /// Bind the token tally to a file, and read back what is already in it.
    ///
    /// A file that cannot be read is treated as an empty tally. Nothing here
    /// is worth refusing to serve a turn over, and a tally that starts at zero
    /// says so everywhere it is reported.
    #[must_use]
    pub fn tallying_at(mut self, path: std::path::PathBuf) -> Self {
        if let Some(loaded) = read_tally(&path)
            && let Ok(mut spent) = self.spent.lock()
        {
            *spent = loaded;
        }
        self.tally = Some(path);
        self
    }

    /// Bind the quota snapshots to a file, and read back what is still true in
    /// it (§6.1).
    ///
    /// **What is restored is decided by the reset time, never by the age of
    /// the file.** A window whose reset has passed describes a window that is
    /// back to zero and is dropped; one that cannot be dated at all is dropped
    /// for the same reason, since nothing about it can be shown to still hold.
    /// What survives is restored with the moment it was taken, so the meter
    /// prints an age rather than an empty row.
    ///
    /// A file that cannot be read is treated as no snapshot. Nothing here is
    /// worth refusing to serve a turn over, and an empty meter says so.
    #[must_use]
    pub fn remembering_at(mut self, path: std::path::PathBuf) -> Self {
        if let Some(loaded) = read_quota(&path) {
            let now = now();
            let restored: std::collections::BTreeMap<String, Measured> = loaded
                .into_iter()
                .filter_map(|(name, measured)| restore(measured, now).map(|kept| (name, kept)))
                .collect();
            if let Ok(mut by_account) = self.by_account.lock() {
                *by_account = restored;
            }
        }
        self.quota = Some(path);
        self
    }

    /// A snapshot that rode a turn made as whoever is serving.
    pub fn record(&self, snapshot: &Snapshot) {
        self.record_for(None, snapshot, Source::Turn);
    }

    /// File a figure under the account it belongs to.
    ///
    /// `None` is the serving account — what an unpinned turn is served as —
    /// and it is resolved to that account's name here, at the moment the turn
    /// was served, rather than left to be resolved by whoever asks later.
    pub fn record_for(&self, account: Option<&str>, snapshot: &Snapshot, source: Source) {
        let measured = Measured {
            snapshot: snapshot.clone(),
            source,
            at: now(),
        };

        match account
            .map(str::to_owned)
            .or_else(|| self.serving.as_ref().and_then(|serving| serving()))
        {
            Some(name) => {
                if let Ok(mut by_account) = self.by_account.lock() {
                    by_account.insert(name, measured);
                }
                self.write_quota(None);
            }
            None => {
                if let Ok(mut unattributed) = self.unattributed.lock() {
                    *unattributed = Some(measured);
                }
            }
        }
    }

    /// Forget one account's figure.
    ///
    /// What a removal invalidates, and all it invalidates: a figure is an
    /// account's entitlement, and the account is gone.
    pub fn forget(&self, account: &str) {
        if let Ok(mut by_account) = self.by_account.lock() {
            by_account.remove(account);
        }
        // And what was served as it. The tally answers "how much has this
        // account spent through this daemon", and there is no such account.
        if let Ok(mut spent) = self.spent.lock() {
            spent.remove(account);
        }
        self.write_tally(Some(account));
        self.write_quota(Some(account));
    }

    /// Forget the figure no account could be named for.
    ///
    /// What a select invalidates. Every named figure survives a select — it
    /// still describes the account it was taken from — but an unattributed one
    /// would be reported as the newly selected account's headroom, which is
    /// wrong in the direction that reads as room to spend.
    pub fn forget_unattributed(&self) {
        if let Ok(mut unattributed) = self.unattributed.lock() {
            *unattributed = None;
        }
    }

    /// The serving account's figure, which is the one reported where a single
    /// daemon-wide figure has always been reported.
    pub fn latest(&self) -> Option<Snapshot> {
        self.latest_measured().map(|measured| measured.snapshot)
    }

    pub fn latest_measured(&self) -> Option<Measured> {
        let serving = self
            .serving
            .as_ref()
            .and_then(|serving| serving())
            .and_then(|name| self.latest_for(&name));
        serving.or_else(|| {
            self.unattributed
                .lock()
                .ok()
                .and_then(|unattributed| unattributed.clone())
        })
    }

    /// One account's figure, if it has one.
    pub fn latest_for(&self, account: &str) -> Option<Measured> {
        self.by_account
            .lock()
            .ok()
            .and_then(|by_account| by_account.get(account).cloned())
    }

    /// Every account this daemon holds a figure for, by name.
    pub fn accounts(&self) -> Vec<(String, Measured)> {
        self.by_account
            .lock()
            .map(|by_account| {
                by_account
                    .iter()
                    .map(|(name, measured)| (name.clone(), measured.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Add one completed turn's counts to the account that served it.
    ///
    /// The counts are upstream's, taken from the completed response and never
    /// recomputed (§6.1). `None` is the serving account, resolved here rather
    /// than by whoever asks later, for the same reason a figure is (§8.3).
    /// A turn no account can be named for is not counted at all: attributing
    /// it to whoever happens to be serving would put one account's spend under
    /// another's name.
    pub fn record_spend(&self, account: Option<&str>, input: u64, output: u64) {
        let Some(name) = account
            .map(str::to_owned)
            .or_else(|| self.serving.as_ref().and_then(|serving| serving()))
        else {
            return;
        };
        if let Ok(mut spent) = self.spent.lock() {
            let entry = spent.entry(name).or_default();
            entry.input = entry.input.saturating_add(input);
            entry.output = entry.output.saturating_add(output);
        }
        self.write_tally(None);
    }

    /// Test seam: run this in the window between what a write reads and what it
    /// replaces, standing in for another writer getting there first.
    pub fn on_tally_write_for_test(&self, hook: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut on_write) = self.on_tally_write.lock() {
            *on_write = Some(Box::new(hook));
        }
    }

    /// Write what is held, merged with what is on disk, and start over if the
    /// file moved while this write was preparing.
    ///
    /// `PROXENOS_HOME` can point two daemons at one directory, and neither can
    /// see the other's turns. Two things keep that from costing a count.
    ///
    /// The **merge** takes whichever count is higher per account, so a daemon
    /// that has been running longer never has its total replaced by a younger
    /// one's. The **comparison** covers what the merge cannot: the merge reads
    /// the file once, and a write that landed after that read is not in what
    /// this one is about to replace it with. Re-reading before the replacement
    /// catches it and the attempt starts over against the newer file.
    ///
    /// It does not close the window, and does not claim to — the comparison
    /// and the rename are two operations, and a write landing between them is
    /// still lost. What it turns into is a smaller floor rather than a
    /// corrupted file, which is the tradeoff `auth/store.rs` makes for the same
    /// reason and with a lock this file deliberately does not take: a
    /// credential write is a whole account, a tally write is one turn's count.
    ///
    /// `dropped` is the one case that is not a merge — a forgotten account is
    /// gone, and must not come back off the disk.
    ///
    /// **Blocking I/O, on the async worker that served the turn.** This runs
    /// once per completed turn rather than per event, over a file of a few
    /// hundred bytes. It is not moved off the runtime because the write is what
    /// makes the count survive, and a spawned write is a write that a shutdown
    /// can outrun.
    ///
    /// Every failure here is silent on purpose. Serving turns does not depend
    /// on this file, and a daemon that refused a turn because a tally could
    /// not be written would trade the product for its bookkeeping.
    fn write_tally(&self, dropped: Option<&str>) {
        let Some(path) = self.tally.as_ref() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        for attempt in 1..=TALLY_WRITE_ATTEMPTS {
            let read = std::fs::read_to_string(path).ok();
            let mut merged = read
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok())
                .unwrap_or_default();

            let Ok(held) = self.spent.lock() else {
                return;
            };
            merge_into(&mut merged, &held);
            drop(held);
            if let Some(dropped) = dropped {
                merged.remove(dropped);
            }

            let Ok(body) = serde_json::to_string_pretty(&merged) else {
                return;
            };

            self.fire_tally_write();

            // Someone else got there between the read above and now, and what
            // they wrote is not in `body`. Start over against their file.
            if attempt < TALLY_WRITE_ATTEMPTS && std::fs::read_to_string(path).ok() != read {
                continue;
            }

            replace_file(path, &body);
            return;
        }
    }

    /// Write the figures this daemon holds, keeping whichever record of an
    /// account is the later one.
    ///
    /// **Later, not higher.** A tally accumulates and a snapshot replaces, so
    /// the merge that keeps a tally honest would keep a quota figure that has
    /// since been superseded. Where two daemons share a `PROXENOS_HOME`,
    /// neither sees the other's turns, and the newer measurement is the one
    /// that describes the account now.
    ///
    /// `dropped` is the one case that is not a merge — a forgotten account is
    /// gone, and must not come back off the disk.
    ///
    /// Every failure here is silent, for the reason `write_tally` is: serving
    /// turns does not depend on this file.
    fn write_quota(&self, dropped: Option<&str>) {
        let Some(path) = self.quota.as_ref() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut merged = read_quota(path).unwrap_or_default();
        let Ok(held) = self.by_account.lock() else {
            return;
        };
        for (name, measured) in held.iter() {
            let superseded = merged
                .get(name)
                .is_none_or(|existing| existing.at <= measured.at);
            if superseded {
                merged.insert(name.clone(), measured.clone());
            }
        }
        drop(held);
        if let Some(dropped) = dropped {
            merged.remove(dropped);
        }

        let Ok(body) = serde_json::to_string_pretty(&merged) else {
            return;
        };
        replace_file(path, &body);
    }

    fn fire_tally_write(&self) {
        if let Ok(hook) = self.on_tally_write.lock()
            && let Some(hook) = hook.as_ref()
        {
            hook();
        }
    }

    /// What this daemon has counted as one account, across restarts.
    #[must_use]
    pub fn spent_for(&self, account: &str) -> Spent {
        self.spent
            .lock()
            .ok()
            .and_then(|spent| spent.get(account).copied())
            .unwrap_or_default()
    }

    /// The plan the profile endpoint stated within the last hour, if it did.
    #[must_use]
    pub fn cached_profile_plan(&self, account: &str, now: u64) -> Option<String> {
        const HOUR: u64 = 3_600;
        self.profile_plans
            .lock()
            .ok()?
            .get(account)
            .filter(|(asked, _)| now.saturating_sub(*asked) <= HOUR)
            .map(|(_, plan)| plan.clone())
    }

    /// Remember what the profile endpoint stated, and when it was asked.
    pub fn record_profile_plan(&self, account: &str, plan: &str, now: u64) {
        if let Ok(mut plans) = self.profile_plans.lock() {
            plans.insert(account.to_owned(), (now, plan.to_owned()));
        }
    }

    pub fn record_model(&self, model: &str) {
        if let Ok(mut served) = self.served.lock()
            && !served.contains(model)
        {
            served.insert(model.to_owned());
        }
    }

    pub fn served(&self) -> Vec<String> {
        self.served
            .lock()
            .map(|served| served.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Ask the backend for a quota figure, rather than waiting for one.
///
/// **This is not the primary path and does not replace it.** The backend
/// volunteers a snapshot at the head of every stream; that one is free, rides a
/// turn already being made, and is what `usage` reports. This exists for the
/// case that one cannot cover: a front-end showing a figure on a daemon that
/// has served no turn yet, where the alternative is showing nothing at all.
///
/// Nothing here is computed. The response is projected into the same `Snapshot`
/// the stream path produces, and a window the backend did not report is absent
/// rather than zero.
pub async fn fetch(
    client: &reqwest::Client,
    endpoint: &str,
    authorization: &crate::auth::authorize::Authorization,
    claude_program: &std::path::Path,
) -> Result<Snapshot, crate::error::ProxyError> {
    // Quota belongs to a subscription. There is no such figure behind a key,
    // and asking for one with a key would spend a request to be told so in
    // words that name neither half.
    let authorization = authorization
        .clone()
        .for_endpoint(crate::auth::authorize::Kind::Subscription)?;

    // Each provider's endpoint wants to be addressed as the client it belongs
    // to. The second provider's answers a request carrying the borrowed
    // grant's own client string; the first provider's has always been asked as
    // this proxy.
    let agent = match authorization.provider {
        crate::auth::store::Provider::Codex => crate::upstream::http::USER_AGENT.to_owned(),
        crate::auth::store::Provider::Anthropic => claude_user_agent(claude_program),
    };

    let request = authorization.apply(
        client
            .get(endpoint)
            .header(axum::http::header::USER_AGENT, agent)
            .header(axum::http::header::ACCEPT, "application/json"),
    );

    let response = request.send().await.map_err(|error| {
        crate::error::ProxyError::upstream(
            axum::http::StatusCode::BAD_GATEWAY,
            format!("could not ask for a quota figure: {error}"),
        )
    })?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(crate::error::ProxyError::upstream(
            status,
            format!("the quota endpoint answered {status}"),
        ));
    }

    let parsed = match authorization.provider {
        crate::auth::store::Provider::Codex => Snapshot::parse_rest(&body),
        crate::auth::store::Provider::Anthropic => Snapshot::parse_anthropic(&body),
    };

    parsed.ok_or_else(|| {
        crate::error::ProxyError::upstream(
            axum::http::StatusCode::BAD_GATEWAY,
            "the quota endpoint answered with a shape this proxy does not recognize",
        )
    })
}

/// Ask the second provider's profile endpoint what plan an account is on.
///
/// The quota body states no plan, and the stored grant's word is as old as the
/// last login; this endpoint beside the quota one is where the provider states
/// it now, multiplier included. `None` for anything short of a recognized
/// answer — a plan is decoration on a figure, not the figure, and no request
/// fails for want of one.
pub async fn fetch_anthropic_plan(
    client: &reqwest::Client,
    endpoint: &str,
    authorization: &crate::auth::authorize::Authorization,
    claude_program: &std::path::Path,
) -> Option<String> {
    let authorization = authorization
        .clone()
        .for_endpoint(crate::auth::authorize::Kind::Subscription)
        .ok()?;

    let request = authorization.apply(
        client
            .get(endpoint)
            .header(
                axum::http::header::USER_AGENT,
                claude_user_agent(claude_program),
            )
            .header(axum::http::header::ACCEPT, "application/json"),
    );

    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    Snapshot::plan_from_anthropic_profile(&body)
}

/// What the second provider's quota endpoint is asked as.
///
/// The version comes from the client that owns the grant, because that is
/// whose credential this is. Where it cannot be run, a version-less string is
/// sent rather than a guessed number: a wrong version is a claim, and an
/// absent one is not.
///
/// Cached: the client's version changes on an upgrade, and spawning a process
/// per quota request to learn it would be silly. The program is fixed for the
/// life of the daemon, so the first caller's is the one that is read.
fn claude_user_agent(program: &std::path::Path) -> String {
    static AGENT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            let version = std::process::Command::new(program)
                .arg("--version")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| {
                    let text = String::from_utf8_lossy(&output.stdout).into_owned();
                    text.split_whitespace()
                        .find(|word| {
                            word.split('.').count() == 3
                                && word.split('.').all(|part| {
                                    !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())
                                })
                        })
                        .map(str::to_owned)
                });
            match version {
                Some(version) => format!("claude-cli/{version} (external, cli)"),
                None => "claude-cli (external, cli)".to_owned(),
            }
        })
        .clone()
}
