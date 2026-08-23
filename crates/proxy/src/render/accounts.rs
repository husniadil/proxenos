//! The account verbs, as an operator reads them.

use super::field;
use super::now;
use super::renewal;
use serde_json::Value;

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
