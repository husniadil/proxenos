//! `status`, the model list, and what a reload applied.

use super::field;
use super::now;
use super::renewal;
use serde_json::Value;

/// What a reload applied, and what it could not.
///
/// Two lines rather than one, and the second is never omitted: the keys a
/// running daemon cannot move are the ones an operator is most likely to have
/// edited and least likely to be told about, and a line that appeared only
/// sometimes would be read as "nothing was left out this time".
pub fn reloaded_config(result: &Value) -> String {
    let names = |key: &str| {
        field(result, key)
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let reloaded = names("reloaded");
    let applied = if reloaded.is_empty() {
        "reloaded config.toml; nothing in it was a setting this daemon can change".to_owned()
    } else {
        format!("reloaded config.toml: {reloaded}")
    };
    let restart = names("needs_restart");
    if restart.is_empty() {
        return applied;
    }
    format!("{applied}\nstill needs a restart: {restart}")
}

pub fn status(result: &Value) -> String {
    status_at(result, now())
}

/// The same report against a stated clock, since the renewal notice counts
/// down and a test asserting on it cannot be at the mercy of the wall clock.
#[must_use]
pub fn status_at(result: &Value, now: u64) -> String {
    let mut lines = Vec::new();

    lines.push(format!(
        "base url   {}",
        field(result, "base_url")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    ));

    let auth = field(result, "auth");
    let connected = auth
        .and_then(|auth| field(auth, "connected"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    lines.push(if connected {
        // The address if the grant carried one, then the id the backend knows
        // it by, then what this daemon calls the account — which always
        // exists, and is the string every account verb takes.
        let who = auth
            .and_then(|auth| field(auth, "email"))
            .and_then(Value::as_str)
            .or_else(|| {
                auth.and_then(|auth| field(auth, "account_id"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                auth.and_then(|auth| field(auth, "account"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("account unknown");
        // Named where it is not the kind this proxy started with, because a
        // key is spent against another endpoint and cannot be asked for a
        // quota.
        let kind = match auth
            .and_then(|auth| field(auth, "kind"))
            .and_then(Value::as_str)
        {
            Some("key") => ", key",
            _ => "",
        };
        // And the provider, on every connected row. With two providers in
        // the store an unnamed one is a guess, and this line is where an
        // operator looks to find out whose backend the next turn reaches.
        // Omitted only where the payload does not carry it — a daemon that
        // predates providers — because filling that in would invent it.
        let provider = match auth
            .and_then(|auth| field(auth, "provider"))
            .and_then(Value::as_str)
        {
            Some(provider) => format!(", {provider}"),
            None => String::new(),
        };
        format!("auth       connected ({who}{kind}{provider})")
    } else {
        // Two different states wear the same word. Nothing to serve with is
        // one; several accounts and no choice between them is the other, and
        // telling that operator to declare a profile sends them to add a
        // third. Observed on a first run: two profiles found, `accounts`
        // listing both, and `status` advising the one thing that would not
        // help.
        let held = auth
            .and_then(|auth| field(auth, "accounts"))
            .and_then(Value::as_array)
            .is_some_and(|accounts| !accounts.is_empty());
        if held {
            "auth       no account chosen — more than one is available; choose with \
             `proxenos accounts use NAME`"
                .to_owned()
        } else {
            "auth       not connected — sign in with `claude auth login` or `codex login` and \
             it is found from there, or store a key with `proxenos accounts \
             add-key NAME --provider codex|anthropic`"
                .to_owned()
        }
    });

    // The backend's own words, because the operator is about to search for
    // them. Its own line for the same reason as the renewal below: the
    // credential is there and readable, and what this says is that the other
    // end will not take it.
    if let Some(refused) = auth
        .and_then(|auth| field(auth, "refused"))
        .filter(|refused| !refused.is_null())
    {
        let detail = field(refused, "detail")
            .and_then(Value::as_str)
            .unwrap_or("no reason was given");
        let status = field(refused, "status")
            .and_then(Value::as_u64)
            .map_or_else(String::new, |status| format!("{status}: "));
        lines.push(format!(
            "refused    the backend turned this credential away ({status}{detail}) — sign in \
             to that profile again"
        ));
    }

    // Its own line rather than a suffix on the one above: the account is
    // connected and every turn works, and what this says is that it stops
    // working on a date. Carrying the remedy because this is the report an
    // operator reads when they are about to do something about it.
    if let Some(notice) = renewal(
        auth.and_then(|auth| field(auth, "login_expires_at"))
            .and_then(Value::as_u64),
        now,
    ) {
        lines.push(format!(
            "renew      {notice} — run `claude auth login` in that profile; past that date \
             the client cannot renew it and asking it to try empties what is left"
        ));
    }

    // Reported, never enforced. Models and efforts are gated on it, and a
    // refusal names the value it rejected rather than the entitlement that was
    // missing — so this line is often the only local half of the explanation.
    // Absent where the grant said nothing: a guessed plan misleads in whichever
    // direction it guesses.
    //
    // The grant's copy is a snapshot from the last login and can be arbitrarily
    // old, so it says so. The backend's is current as of the last turn and
    // needs no qualifier.
    if let Some(plan) = auth
        .and_then(|auth| field(auth, "plan"))
        .and_then(Value::as_str)
    {
        let qualifier = match auth
            .and_then(|auth| field(auth, "plan_source"))
            .and_then(Value::as_str)
        {
            Some("grant") => " (as of last login)",
            _ => "",
        };
        lines.push(format!("plan       {plan}{qualifier}"));
    }

    // The model list belongs to whichever account it was fetched for, and
    // nothing refetches it when the daemon starts serving another. Said out
    // loud, because every model and effort answer downstream rests on it.
    if field(result, "catalog_stale")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let fetched_for = field(result, "catalog_account")
            .and_then(Value::as_str)
            .unwrap_or("another account");
        lines.push(format!(
            "models     listed for {fetched_for}, not the account serving turns — restart to refresh"
        ));
    }

    // Whether the account serving turns is on the second provider. Every id
    // it authenticates relays verbatim (`proxy-behavior.md` §9.1), so an
    // unpinned tier row decides nothing at all — and four rows printed with no
    // qualifier read as "your turns go to these models", which is the one
    // thing they do not mean in this state.
    // Named rather than ordinal: the operator has `codex` and `anthropic` in
    // front of them in every accounts listing, and "the second provider" is
    // the spec's word for a role, not anything they can act on.
    let relaying = match auth
        .and_then(|auth| field(auth, "provider"))
        .and_then(Value::as_str)
    {
        Some(provider) if connected && provider != "codex" => Some(provider),
        _ => None,
    };

    if let Some(tiers) = field(result, "tiers").and_then(Value::as_object) {
        for (tier, value) in tiers {
            // A pinned tier arrives as `{ account, model }` — the same two
            // shapes the configuration takes — and is printed with its pin,
            // because which account a tier spends is the whole point of one.
            let rendered = match (value.as_str(), value.as_object()) {
                // An unpinned tier follows the account serving turns, so a
                // relaying one takes that tier with it and the mapped model is
                // never asked for. A pin names its own account and stays live
                // either way — that is what pinning one is for.
                (Some(model), _) if relaying.is_some() => format!("{model} (inert while relaying)"),
                (Some(model), _) => model.to_owned(),
                (None, Some(pinned)) => format!(
                    "{} (as {})",
                    pinned.get("model").and_then(Value::as_str).unwrap_or("?"),
                    pinned.get("account").and_then(Value::as_str).unwrap_or("?")
                ),
                _ => "unmapped".to_owned(),
            };
            lines.push(format!("{tier:<10} {rendered}"));
        }
    }

    // And what the mapping's inert rows are inert in favour of, named rather
    // than left to be inferred from a provider in the auth line.
    if let Some(provider) = relaying {
        lines.push(format!(
            "routing    model ids relay verbatim to {provider} — \
             the account serving turns is on it, so an unpinned tier decides nothing"
        ));
    }

    // Whether the mapping was checked against the backend's own catalog. A
    // reader who cannot tell would take an unvalidated mapping for a validated
    // one. A relay-serving daemon's list is the curated one instead: that
    // catalog was never these models' menu, so "not validated" would report a
    // check that was never owed.
    if field(result, "catalog_curated").and_then(Value::as_bool) == Some(true) {
        let provider = relaying.unwrap_or("another provider");
        lines.push(format!(
            "catalog    built-in list for {provider} — curated, not fetched"
        ));
    } else if field(result, "catalog_authoritative").and_then(Value::as_bool) == Some(false) {
        lines.push("catalog    unavailable — the tier mapping has not been validated".to_owned());
    }

    // A tier pointing at a model the catalog withholds. It passed validation —
    // the catalog knows the id — so this is the only place it is ever
    // mentioned. Not an error: the backend may still serve it, and mapping one
    // deliberately is a reasonable thing to do.
    let withheld: Vec<&str> = field(result, "unlisted_tiers")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !withheld.is_empty() {
        let verb = if withheld.len() == 1 { "is" } else { "are" };
        lines.push(format!(
            "catalog    {} {verb} mapped but not offered by the catalog",
            withheld.join(", ")
        ));
    }

    // One binary is both the daemon and the CLI, and replacing the file does not
    // restart what is already running — so a newer CLI against an older daemon
    // is the ordinary state after an upgrade. Silent when they agree: a line
    // printed on every run is one nobody reads on the run that matters.
    //
    // Two reads, because either can be the only one that fires. A version
    // string catches an upgrade across releases. Within one — two builds of the
    // same version, one of them older than a feature — the string is equal and
    // the missing field is the only evidence there is.
    let older_build = field(result, "client").is_none();
    let daemon_version = field(result, "version").and_then(Value::as_str);
    match (daemon_version, older_build) {
        // Across releases the string is the plain answer, and it names both
        // sides so the reader can tell which way round it is.
        (Some(daemon), _) if daemon != crate::control::VERSION => lines.push(format!(
            "version    daemon {daemon}, this binary {} — restart the daemon",
            crate::control::VERSION
        )),
        // Old enough that it does not report a version at all. Saying which
        // number it did not report would be inventing one.
        (None, true) => lines
            .push("version    the daemon is old enough not to report one — restart it".to_owned()),
        // Same string on both sides, and one of them older than a feature. The
        // string says nothing here, so the missing field is the only evidence
        // there is.
        (Some(daemon), true) => lines.push(format!(
            "version    the daemon reports {daemon}, this binary's own, but is an older \
             build — restart it"
        )),
        _ => {}
    }

    // The one place a denied skill is ever attributed. The client's own refusal
    // names no source, so without this the only way to find out is to guess.
    // Silent when nothing is denied: a line about an empty policy sends the
    // reader looking for a rule that does not exist.
    let denied: Vec<&str> = field(result, "client")
        .and_then(|client| field(client, "deny_skills"))
        .and_then(Value::as_array)
        .map(|skills| skills.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !denied.is_empty() {
        lines.push(format!(
            "client     {} denied — change with `client.deny_skills`",
            denied.join(", ")
        ));
    }

    if field(result, "recording").and_then(Value::as_bool) == Some(true) {
        lines.push("recording  on".to_owned());
    }

    lines.join("\n")
}

pub fn models(result: &Value) -> String {
    let mut lines = Vec::new();

    if field(result, "curated").and_then(Value::as_bool) == Some(true) {
        // Named from the payload, and left out where the payload does not
        // carry one — a daemon older than the field. Naming it anyway would
        // be inventing the answer.
        let whose = match field(result, "provider").and_then(Value::as_str) {
            Some(provider) => format!("{provider}'s list is"),
            None => "this list is".to_owned(),
        };
        lines.push(format!(
            "({whose} built in; windows are curated, not fetched)"
        ));
    } else if field(result, "authoritative").and_then(Value::as_bool) == Some(false) {
        lines.push("(the catalog could not be fetched; this is the fallback list)".to_owned());
    }

    for model in field(result, "models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = field(model, "id").and_then(Value::as_str).unwrap_or("?");
        // "unknown" rather than a number. Printing a figure nobody measured is
        // how an assumption becomes a fact.
        let window = field(model, "context_window")
            .and_then(Value::as_u64)
            .map(|window| format!("{window} tokens"))
            .unwrap_or_else(|| "window unknown".to_owned());
        lines.push(format!("{id:<24} {window}"));
    }

    lines.join("\n")
}
