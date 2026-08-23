//! Turning control-socket results into terminal output.
//!
//! Presentation only. The daemon holds the state and decides what is true; this
//! decides how it reads.

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

/// The stored accounts, and which one serves turns.
///
/// The name comes first because it is what selects the account; the id and the
/// address are what tell two of them apart.
pub fn accounts(result: &Value) -> String {
    accounts_at(result, now())
}

/// The same listing against a stated clock, since one field of it counts down.
#[must_use]
pub fn accounts_at(result: &Value, now: u64) -> String {
    let Some(accounts) = field(result, "accounts").and_then(Value::as_array) else {
        return "no accounts".to_owned();
    };
    if accounts.is_empty() {
        return "no accounts — sign in with `claude auth login` or `codex login` and they are \
                found from there, declare a profile under `[profiles]`, or store a key with \
                `proxenos accounts add-key NAME --provider codex|anthropic`"
            .to_owned();
    }

    // Said once, under the rows rather than on them: it is true of the whole
    // set, and what it tells the operator is why accounts they never wrote
    // down are here — and how to stop them moving when they sign in
    // somewhere else.
    let found = if field(result, "discovered").and_then(Value::as_bool) == Some(true) {
        "\nnote: these were found, not declared — the stock profile of each program, read \
         because `[profiles]` is empty. Write them into `[profiles]` to pin them."
    } else {
        ""
    };

    let ignored = match field(result, "ignored_grants").and_then(Value::as_array) {
        Some(names) if !names.is_empty() => {
            let names = names
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\nnote: {names} in credentials.json is a stored grant, which is no longer \
                 read. A subscription is borrowed from the profile that holds it — declare \
                 that profile under `[profiles]`."
            )
        }
        _ => String::new(),
    };

    let rows = accounts
        .iter()
        .map(|account| {
            let name = field(account, "name")
                .and_then(Value::as_str)
                .unwrap_or("unnamed");
            let marker = if field(account, "selected")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "*"
            } else {
                " "
            };
            // The string, then the fallback. Asking for the field first and
            // reading it as a string afterwards makes a present-but-null
            // `email` shadow an account id that is right there.
            //
            // A key has neither an address nor an id — it is one secret — so
            // the column that tells two accounts apart falls through to what
            // the account is.
            // A key is one secret: no address, no id, and nothing else to say
            // about it. A grant missing both has an id somewhere and this
            // daemon simply does not have it, which is a different sentence.
            //
            // The plan comes third because a borrowed Claude grant carries no
            // address and no id at all — its store holds neither — but it does
            // say which subscription it is. Printing `id unknown` there stated
            // the one thing the row could not say while sitting next to
            // something it could.
            let key = field(account, "kind").and_then(Value::as_str) == Some("key");
            let who = field(account, "email")
                .and_then(Value::as_str)
                .or_else(|| field(account, "account_id").and_then(Value::as_str))
                .or_else(|| field(account, "plan").and_then(Value::as_str))
                .unwrap_or(if key { "key" } else { "id unknown" });
            // On every row. With two providers in the store an unnamed one is
            // a guess, and the row that leaves it out is the one an operator
            // has to guess about. Omitted only where the payload genuinely
            // does not carry it — a daemon that predates providers — because
            // filling that in would be inventing the answer.
            let provider = match field(account, "provider").and_then(Value::as_str) {
                Some(provider) => format!("  {provider}"),
                None => String::new(),
            };
            // Where the credential was read from, for an account this daemon
            // does not hold. A name is the operator's own label; this is the
            // directory they can go and look at (§8.4).
            let source = match field(account, "source").and_then(Value::as_str) {
                Some(source) => format!("  {source}"),
                None => String::new(),
            };
            // Said on the row it belongs to, because the consequence is turns
            // billed to an account nobody pointed at them.
            let changed = if field(account, "identity_changed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "  (a different account than when it was chosen)"
            } else {
                ""
            };
            // What the backend made of this credential, where it turned one
            // away. On the row because it belongs to the account rather than
            // to the daemon, and ahead of the renewal count because a
            // credential already refused is past being renewed early.
            let refused = if field(account, "refused").is_some() {
                "  (the backend refused this credential — sign in to that profile again)"
            } else {
                ""
            };
            // The fact on the row, and the remedy on `status`: this line is
            // a listing, and the account it belongs to may not be the one an
            // operator is about to act on.
            let renew = match renewal(
                field(account, "login_expires_at").and_then(Value::as_u64),
                now,
            ) {
                Some(notice) => format!("  ({notice})"),
                None => String::new(),
            };
            // Trimmed so a payload without a provider does not leave the
            // padding hanging off the end of the line.
            format!("{marker} {name:<24} {who:<24}{provider}{source}{changed}{refused}{renew}")
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!("{rows}{found}{ignored}")
}

/// What a rename says it did. Both halves, because the point of the command is
/// that the name changed and the operator has to know what to type next.
pub fn renamed_account(result: &Value) -> String {
    let from = field(result, "renamed")
        .and_then(Value::as_str)
        .unwrap_or("nothing");
    // Whether the configuration file moved with it. Said only when it did:
    // most accounts have no section, and an operator who has one wrote it by
    // hand and will want to know it was rewritten.
    let moved = field(result, "moved_configuration")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match field(result, "name").and_then(Value::as_str) {
        Some(to) if moved => {
            format!("{from} is now {to}; its section in config.toml moved with it")
        }
        Some(to) => format!("{from} is now {to}"),
        None => format!("renamed {from}"),
    }
}

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

/// What removing an account says it did, and who is left serving turns.
///
/// The second half matters: removing the account that was serving hands over
/// to another one, and an operator who is not told has to go and look.
pub fn removed_account(result: &Value) -> String {
    let removed = field(result, "removed")
        .and_then(Value::as_str)
        .unwrap_or("nothing");
    if let Some(serving) = field(result, "serving").and_then(Value::as_str) {
        return format!("removed {removed}; serving turns as {serving}");
    }
    // Nothing is serving, and the two reasons for that want opposite advice.
    // Seen live: removing a leftover out of a store that still held two
    // accounts answered "no accounts left".
    if field(result, "remaining")
        .and_then(Value::as_u64)
        .is_some_and(|remaining| remaining > 0)
    {
        return format!(
            "removed {removed}; no account is serving turns — choose one with \
             `proxenos accounts use NAME`"
        );
    }
    format!(
        "removed {removed}; no accounts left — declare a profile under `[profiles]`, \
         or store a key with `proxenos accounts add-key NAME --provider codex|anthropic`"
    )
}

/// What a switch says it did. The name, because the caller may have typed a
/// label and the daemon is the side that resolved it — and how far the switch
/// moved, because the same six words cannot stand for both sizes of it. Within
/// one provider it changed whose quota is spent. Across providers it changed
/// which backend answers, which path the turn takes, and which subscription is
/// drawn down, and an operator who is not told has to go and look.
///
/// The providers are named outright: this is operator-facing output, where a
/// role word is this project's vocabulary and not the reader's.
pub fn selected_account(result: &Value) -> String {
    let Some(name) = field(result, "selected").and_then(Value::as_str) else {
        return "no account selected".to_owned();
    };
    let provider = field(result, "provider").and_then(Value::as_str);
    // Absent for the first account stored, where nothing was serving before.
    // Saying a provider changed there would be inventing the half that does
    // not exist, so the line states only what is true: who now serves, on
    // which provider.
    let previous = field(result, "previous_provider").and_then(Value::as_str);
    match (provider, previous) {
        (Some(provider), Some(previous)) if provider != previous => format!(
            "serving turns as {name}; {previous} to {provider}, so a different backend on a \
             different subscription answers every turn"
        ),
        (Some(provider), Some(_)) => format!("serving turns as {name}; still on {provider}"),
        (Some(provider), None) => format!("serving turns as {name} on {provider}"),
        // A daemon that predates the field says what it always said.
        (None, _) => format!("serving turns as {name}"),
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
             it is found from there, or store a key with `proxenos accounts add-key NAME --provider codex|anthropic`"
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

/// `docs/api.md` §3 — the quota snapshot, in one line per window.
///
/// Written to be read by a person and parsed by a status line, which is what a
/// caller wanting this every few seconds is building. `--json` is there for the
/// second case; this is the first.
pub fn usage(result: &Value) -> String {
    usage_at(result, now())
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
