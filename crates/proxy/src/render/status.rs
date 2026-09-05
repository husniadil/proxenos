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
    let mut lines = vec![applied];
    if !restart.is_empty() {
        lines.push(format!("still needs a restart: {restart}"));
    }
    // Only where the payload carries `serving` at all: a daemon that predates
    // it is silent here rather than reported as serving nobody.
    if let Some(serving) = field(result, "serving")
        && serving.is_null()
        && field(result, "remaining")
            .and_then(Value::as_u64)
            .is_some_and(|remaining| remaining > 0)
    {
        lines.push(
            "no account is serving turns — the one that was is no longer declared; choose \
             one with `proxenos accounts use NAME`"
                .to_owned(),
        );
    }
    lines.join("\n")
}

/// The mapping as a table, with a tier the catalog cannot honour marked.
///
/// The same `TIER MODEL` columns `models` prints in the other direction, so
/// the two listings read against each other.
pub fn tiers(result: &Value) -> String {
    let missing: Vec<&str> = field(result, "missing_tiers")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let rows: Vec<(String, String, String)> = field(result, "tiers")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(tier, value)| {
                    let (model, pinned) = tier_value(value);
                    let state = if missing.contains(&tier.as_str()) {
                        "not in this account's catalog".to_owned()
                    } else {
                        pinned
                            .map(|account| format!("as {account}"))
                            .unwrap_or_default()
                    };
                    (tier.clone(), model, state)
                })
                .collect()
        })
        .unwrap_or_default();
    // The STATE column only where a row has one, as `status` prints it.
    let stated = rows.iter().any(|(_, _, state)| !state.is_empty());
    let width = rows
        .iter()
        .map(|(_, model, _)| model.len())
        .max()
        .unwrap_or(5)
        .max(5);
    let mut lines = vec![if stated {
        format!("{:<7} {:<width$}  STATE", "TIER", "MODEL")
    } else {
        "TIER    MODEL".to_owned()
    }];
    for (tier, model, state) in rows {
        lines.push(if stated {
            format!("{tier:<7} {model:<width$}  {state}")
                .trim_end()
                .to_owned()
        } else {
            format!("{tier:<7} {model}")
        });
    }
    if let Some(consent) = field(result, "cross_account_tiers").and_then(Value::as_bool) {
        lines.push(format!(
            "cross-account tiers: {}",
            if consent { "allowed" } else { "off" }
        ));
    }
    lines.join("\n")
}

/// A tier's value in either of the two shapes the file takes: the model, and
/// the account it is pinned to where it is.
fn tier_value(value: &Value) -> (String, Option<String>) {
    match (value.as_str(), value.as_object()) {
        (Some(model), _) => (model.to_owned(), None),
        (None, Some(pinned)) => (
            pinned
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned(),
            pinned
                .get("account")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        _ => ("?".to_owned(), None),
    }
}

/// What a set did: the tier's new model, the account it is pinned to where
/// it is, and whether it outlives the daemon.
pub fn tier_set(tier: &str, result: &Value) -> String {
    let (model, pinned) = field(result, "tiers")
        .and_then(|tiers| tiers.get(tier))
        .map(tier_value)
        .unwrap_or_else(|| ("?".to_owned(), None));
    let pin = pinned
        .map(|account| format!(" as {account}"))
        .unwrap_or_default();
    format!("{tier} → {model}{pin}\n{}", scope(result))
}

/// What granting or revoking consent did.
pub fn cross_account_set(result: &Value) -> String {
    let enabled = field(result, "cross_account_tiers")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let persisted = field(result, "persisted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match (enabled, persisted) {
        (true, true) => "cross-account tiers allowed; written to config.toml".to_owned(),
        (true, false) => "cross-account tiers were already allowed".to_owned(),
        (false, true) => "cross-account tiers off; written to config.toml".to_owned(),
        (false, false) => "cross-account tiers were already off".to_owned(),
    }
}

/// The ceiling in force, read from `status`.
pub fn effort(status: &Value) -> String {
    match field(status, "effort_ceiling").and_then(Value::as_str) {
        Some(ceiling) => format!("effort ceiling: {ceiling}"),
        None => "no effort ceiling; each request's own effort stands".to_owned(),
    }
}

/// What an `effort set` did: the ceiling that results, which is not always
/// the one asked for, and whether it outlives the daemon.
pub fn effort_set(result: &Value) -> String {
    let ceiling = match field(result, "effort").and_then(Value::as_str) {
        Some(ceiling) => format!("effort ceiling: {ceiling}"),
        None => "no effort ceiling".to_owned(),
    };
    format!("{ceiling}\n{}", scope(result))
}

/// The daemon's own sentence about where a change landed, and for whom.
fn scope(result: &Value) -> String {
    let detail = field(result, "detail")
        .and_then(Value::as_str)
        .unwrap_or("in effect until the daemon stops");
    match field(result, "account").and_then(Value::as_str) {
        Some(account) => format!("for account {account}: {detail}"),
        None => detail.to_owned(),
    }
}

pub fn status(result: &Value) -> String {
    status_at(result, now())
}

/// The same report against a stated clock, since the renewal notice counts
/// down and a test asserting on it cannot be at the mercy of the wall clock.
#[must_use]
pub fn status_at(result: &Value, now: u64) -> String {
    let mut lines = Vec::new();

    // Where the daemon is, said only where it is not here (`api.md` §2.7).
    // Absent on a local daemon rather than null, so nothing is printed for the
    // ordinary case — and printed first where there is one, because every line
    // below it describes a machine that is not this one.
    if let Some(url) = field(result, "daemon_at").and_then(Value::as_str) {
        lines.push(format!("daemon at  {url}"));
    }

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
        // What this daemon calls the account, which is the string every
        // account verb takes and the name `accounts` lists it under. Leading
        // with it is what makes this line and that listing one surface: the
        // operator reads a name here and types it into `accounts use`.
        let name = auth
            .and_then(|auth| field(auth, "account"))
            .and_then(Value::as_str);
        // The address if the grant carried one, then the id the backend knows
        // it by. Where the account has neither and no name either, the word
        // that was here before it had a name to lead with.
        let who = auth
            .and_then(|auth| field(auth, "email"))
            .and_then(Value::as_str)
            .or_else(|| {
                auth.and_then(|auth| field(auth, "account_id"))
                    .and_then(Value::as_str)
            });
        // Named where it is not the kind this proxy started with, because a
        // key is spent against another endpoint and cannot be asked for a
        // quota.
        let kind = match auth
            .and_then(|auth| field(auth, "kind"))
            .and_then(Value::as_str)
        {
            Some("key") => Some("key"),
            _ => None,
        };
        // And the provider, on every connected row. With two providers in
        // the store an unnamed one is a guess, and this line is where an
        // operator looks to find out whose backend the next turn reaches.
        // Omitted only where the payload does not carry it — a daemon that
        // predates providers — because filling that in would invent it.
        let provider = auth
            .and_then(|auth| field(auth, "provider"))
            .and_then(Value::as_str);
        // The name leads where there is one; `connected` leads where there is
        // not, which is what a daemon predating the field answers with. The
        // parenthesis holds whatever else is known, and is left off entirely
        // rather than printed empty.
        let detail = [who, kind, provider]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");
        let head = name.unwrap_or("connected");
        if detail.is_empty() {
            format!("auth       {head}")
        } else {
            format!("auth       {head} ({detail})")
        }
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

    // The mapping as a table. Four rows with no header were four labels an
    // operator had to already know the meaning of, and the state each row can
    // be in — inert, or pinned to another account — trailed off the end of the
    // model as a parenthesis. A column says the same thing where the eye is
    // already looking for it.
    // Tiers whose stated model the catalog does not carry. Read before the
    // table, because the row is where an operator is already looking and a
    // marked tier is the reason their turns are being refused.
    let missing: Vec<&str> = field(result, "missing_tiers")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if let Some(tiers) = field(result, "tiers").and_then(Value::as_object) {
        // The ladder's own order, not the map's. The payload is an unordered
        // object and arrives sorted by name, so the four rows printed as
        // `fable, haiku, opus, sonnet` — which is only how they sort. The
        // model list already answers this question in ladder order, and one
        // surface ordering these two ways is a surface an operator has to
        // re-read.
        let mut ordered: Vec<(&String, &Value)> = tiers.iter().collect();
        ordered.sort_by_key(|(tier, _)| rung(tier));
        let mapped: Vec<(String, String, String)> = ordered
            .into_iter()
            .map(|(tier, value)| {
                // A pinned tier arrives as `{ account, model }` — the same two
                // shapes the configuration takes — and says so in its state,
                // because which account a tier spends is the whole point of one.
                let (model, state) = match (value.as_str(), value.as_object()) {
                    // An unpinned tier follows the account serving turns, so a
                    // relaying one takes that tier with it and the mapped model
                    // is never asked for. A pin names its own account and stays
                    // live either way — that is what pinning one is for.
                    (Some(model), _) if relaying.is_some() => {
                        (model.to_owned(), "inert while relaying".to_owned())
                    }
                    (Some(model), _) => (model.to_owned(), String::new()),
                    (None, Some(pinned)) => (
                        pinned
                            .get("model")
                            .and_then(Value::as_str)
                            .unwrap_or("?")
                            .to_owned(),
                        format!(
                            "as {}",
                            pinned.get("account").and_then(Value::as_str).unwrap_or("?")
                        ),
                    ),
                    _ => ("unmapped".to_owned(), String::new()),
                };
                // The mark outranks whatever else the row had to say. A tier
                // the catalog cannot serve refuses every turn, and reporting
                // it as pinned or inert would name the least important true
                // thing about it.
                let state = if missing.contains(&tier.as_str()) {
                    "missing from the catalog".to_owned()
                } else {
                    state
                };
                (tier.clone(), model, state)
            })
            .collect();

        // The state column only where a row has one. A header over a column of
        // blanks is a column the reader looks at and learns nothing from, and
        // the ordinary mapping — nothing pinned, nothing relaying — is all
        // blanks.
        let stateful = mapped.iter().any(|(_, _, state)| !state.is_empty());
        let rows: Vec<Vec<String>> = mapped
            .into_iter()
            .map(|(tier, model, state)| {
                if stateful {
                    vec![tier, model, state]
                } else {
                    vec![tier, model]
                }
            })
            .collect();
        let header: &[&str] = if stateful {
            &["TIER", "MODEL", "STATE"]
        } else {
            &["TIER", "MODEL"]
        };
        lines.push(super::table(header, &rows));
    }

    // What a marked row means, said once beneath the table. The cell has room
    // for the state and not for the consequence, and the consequence — turns
    // on that tier are refused, and the way out is the file — is the whole
    // reason the daemon came up rather than exiting.
    if !missing.is_empty() {
        let mapped = field(result, "tiers")
            .and_then(Value::as_object)
            .is_some_and(|tiers| tiers.keys().all(|tier| missing.contains(&tier.as_str())));
        let scope = if mapped {
            "no tier can serve".to_owned()
        } else {
            format!("{} cannot serve", missing.join(", "))
        };
        lines.push(format!(
            "catalog    {scope} — the model each names is absent from this account's \
             catalog, and turns asking for it are refused. Edit `[tiers]` in config.toml, \
             then `proxenos reload`"
        ));
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

    // What is actually serving this socket, as one line: the build, the
    // process, and whether anything brings it back. Without it the only
    // version ever printed was the one on the mismatch notice below, so a
    // daemon that agreed with its CLI reported no version at all — and `stop`,
    // `supervisor` and `start` all describe a process the report never
    // named.
    if let Some(version) = daemon_version {
        // The pid where the payload carries one, and nothing where it does not:
        // a daemon predating the field is the case, and inventing a number for
        // it would be worse than a shorter line.
        let pid = field(result, "pid")
            .and_then(Value::as_u64)
            .map_or_else(String::new, |pid| format!(" (pid {pid})"));
        // Said only where the daemon can tell. `null` is the platform, or the
        // process, that cannot answer the question — reported as silence rather
        // than resolved into "not supervised", which would be a claim.
        let supervised = match field(result, "supervised").and_then(Value::as_bool) {
            Some(true) => ", supervised",
            Some(false) => ", not supervised",
            None => "",
        };
        lines.push(format!("daemon     {version}{pid}{supervised}"));
    }
    match (daemon_version, older_build) {
        // Across releases the string is the plain answer, and it names both
        // sides so the reader can tell which way round it is.
        (Some(daemon), _) if daemon != crate::control::version() => lines.push(format!(
            "version    daemon {daemon}, this binary {} — restart the daemon",
            crate::control::version()
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

/// The models this daemon can serve, and which tiers point at them.
///
/// The tier mapping comes from the `tiers` method rather than this one's
/// payload, because that method already carries it and a second copy is a
/// second thing to keep true. `None` where it could not be read — the column
/// is then left off rather than printed empty, since an empty cell there reads
/// as "no tier maps to this model", which would be a claim.
pub fn models(result: &Value, tiers: Option<&Value>) -> String {
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

    let mapping = tiers.and_then(|tiers| field(tiers, "tiers")).cloned();

    let rows: Vec<Vec<String>> = field(result, "models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|model| {
            let id = field(model, "id").and_then(Value::as_str).unwrap_or("?");
            // "unknown" rather than a number. Printing a figure nobody measured
            // is how an assumption becomes a fact.
            let window = field(model, "context_window")
                .and_then(Value::as_u64)
                .map_or_else(
                    || "window unknown".to_owned(),
                    |window| format!("{window} tokens"),
                );
            match &mapping {
                Some(mapping) => vec![id.to_owned(), window, tiers_mapping_to(mapping, id)],
                None => vec![id.to_owned(), window],
            }
        })
        .collect();

    let header: &[&str] = if mapping.is_some() {
        &["MODEL", "WINDOW", "TIER"]
    } else {
        &["MODEL", "WINDOW"]
    };
    lines.push(super::table(header, &rows));

    // A tier the catalog cannot serve has no row here at all — that is what
    // being absent from the catalog means — so the one place it could be read
    // off this list is a line beneath it. Without this, the model an operator
    // came looking for is simply not there and the list says nothing about why.
    let missing: Vec<String> = tiers
        .and_then(|tiers| field(tiers, "missing_tiers"))
        .and_then(Value::as_array)
        .map(|entries| {
            let mut named: Vec<&str> = entries.iter().filter_map(Value::as_str).collect();
            named.sort_by_key(|tier| rung(tier));
            named
                .into_iter()
                .map(|tier| match tier_model(mapping.as_ref(), tier) {
                    Some(model) => format!("{tier} → {model}"),
                    None => tier.to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    if !missing.is_empty() {
        lines.push(format!(
            "({} mapped but not in this catalog; turns asking for it are refused)",
            missing.join(", ")
        ));
    }

    lines.join("\n")
}

/// The model one tier maps to, as the `tiers` payload states it.
fn tier_model(mapping: Option<&Value>, tier: &str) -> Option<String> {
    let value = mapping?.get(tier)?;
    value
        .as_str()
        .or_else(|| value.get("model").and_then(Value::as_str))
        .map(str::to_owned)
}

/// The ladder, in the order these are spoken about everywhere else. A tier
/// nobody named sorts after every tier that was.
const LADDER: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

/// Where a tier sits on the ladder.
fn rung(tier: &str) -> usize {
    LADDER
        .iter()
        .position(|known| *known == tier)
        .unwrap_or(LADDER.len())
}

/// The tiers a model id answers for, in the order an operator reads them.
///
/// The ladder's own order rather than the mapping's, which arrives sorted by
/// name: `opus, sonnet, haiku` is how these are spoken about everywhere else,
/// and `fable, haiku, opus` is only how they sort.
fn tiers_mapping_to(mapping: &Value, id: &str) -> String {
    let Some(mapping) = mapping.as_object() else {
        return String::new();
    };
    let mut named: Vec<&String> = mapping
        .iter()
        .filter(|(_, value)| {
            // A tier is either a model id or `{ account, model }` pinning it to
            // another account. Both name a model, and both point at it.
            let model = value
                .as_str()
                .or_else(|| value.get("model").and_then(Value::as_str));
            model == Some(id)
        })
        .map(|(tier, _)| tier)
        .collect();
    named.sort_by_key(|tier| rung(tier));
    named
        .into_iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}
