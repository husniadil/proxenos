//! Turning control-socket results into what a launch and a shell need.
//!
//! Presentation only. The daemon holds the state and decides what is true; this
//! decides how it reads.

use super::field;
use super::quote;
use serde_json::Value;

/// `docs/api.md` §2.2 — shell exports, for a shell.
///
/// Routing, plus the one piece of client policy that has an environment
/// variable: the connector opt-out. The rest — the denied skill, the connector
/// notice — lives in the client's settings file and has no environment
/// variable, so this rendering cannot carry it and says so in a comment `eval`
/// steps over.
///
/// It keeps working against a daemon older than this binary, because routing is
/// all it ever carried and an older daemon has all of it. That case gets its own
/// comment: continuing quietly while a permission rule goes missing is the
/// failure this whole area exists to prevent.
pub fn env_shell(result: &Value) -> String {
    let mut lines = Vec::new();

    match result.get("settings").and_then(Value::as_object) {
        None => {
            lines.push(
                "# The running daemon is from an older build, so the client policy is missing"
                    .to_owned(),
            );
            lines.push("# from the exports below. Restart the daemon to pick it up.".to_owned());
        }
        Some(policy) if !policy.is_empty() => {
            lines.push(
                "# Client policy is only partly below (the connector switch). The rest — a \
                 denied skill,"
                    .to_owned(),
            );
            lines.push(
                "# the connector notice — lives in the client's settings file and has no \
                 environment"
                    .to_owned(),
            );
            lines.push(
                "# variable. `proxenos settings` carries it, and `proxenos exec` applies it \
                 for one run."
                    .to_owned(),
            );
        }
        // Present and empty: the daemon knows about this and has nothing to say.
        Some(_) => {}
    }

    lines.extend(
        variables(result)
            .into_iter()
            .map(|(name, value)| format!("export {name}={}", quote(&value))),
    );

    lines.join("\n")
}

/// `docs/api.md` §2.2 — one complete client settings document.
///
/// Complete on its own, which is measured rather than assumed: a client started
/// with no `ANTHROPIC_*` in its environment, reading only a settings file
/// holding this document's `env` block, still reached the proxy. So this is not
/// half a configuration waiting for an `eval`.
pub fn settings_json(result: &Value) -> String {
    let map: serde_json::Map<String, Value> = variables(result)
        .into_iter()
        .map(|(name, value)| (name, Value::from(value)))
        .collect();

    let mut document = serde_json::Map::new();
    document.insert("env".to_owned(), Value::Object(map));

    // The policy half merges in as siblings of `env`, because that is where the
    // client reads them. Absent when the daemon published none: an empty
    // `permissions` block would read as a policy to whoever merges this, and
    // merging an empty deny list over a real one is how a rule disappears.
    if let Some(policy) = result.get("settings").and_then(Value::as_object) {
        for (key, value) in policy {
            document.insert(key.clone(), value.clone());
        }
    }

    serde_json::to_string_pretty(&Value::Object(document)).unwrap_or_default()
}

/// The environment half of the payload, for a caller that sets it rather than
/// prints it.
pub fn variables(result: &Value) -> Vec<(String, String)> {
    result
        .get("variables")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some((
                        entry.get(0)?.as_str()?.to_owned(),
                        entry.get(1)?.as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One line naming who pays for the session about to start.
///
/// `None` where nothing is serving turns: a launch with no account is refused
/// by the daemon with a message of its own, and saying "nobody is paying"
/// first would only get in front of it.
pub fn serving_line(result: &Value) -> Option<String> {
    let serving = field(result, "accounts")?
        .as_array()?
        .iter()
        .find(|account| field(account, "selected").and_then(Value::as_bool) == Some(true))?;

    let name = field(serving, "name").and_then(Value::as_str)?;
    let provider = field(serving, "provider")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    // The identity beside the label, because the label is the operator's own
    // word and the identity is the account that gets billed.
    let who = field(serving, "email")
        .and_then(Value::as_str)
        .or_else(|| field(serving, "account_id").and_then(Value::as_str));
    let plan = field(serving, "plan").and_then(Value::as_str);

    let mut line = format!("serving as {name} ({provider}");
    if let Some(who) = who {
        line.push_str(&format!(", {who}"));
    }
    if let Some(plan) = plan {
        line.push_str(&format!(", {plan}"));
    }
    line.push(')');

    if field(serving, "identity_changed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        line.push_str(" — a different account than when it was chosen");
    }
    Some(line)
}
