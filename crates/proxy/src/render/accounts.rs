//! The account verbs, as an operator reads them.

use super::field;
use super::now;
use super::renewal;
use serde_json::Value;

use super::table;

/// How wide a source is allowed to be.
///
/// A terminal's own width is not knowable here — the listing is as likely to
/// be read out of a pipe — so this is a fixed cap rather than a fraction of
/// something unmeasured. Long enough for a profile path with a name on the
/// end, short enough that the state column stays on the same screen.
const SOURCE_WIDTH: usize = 40;

/// The stored accounts, and which one serves turns.
///
/// A header table: the name selects the account, the middle columns tell two
/// of them apart, and the last says whether this one needs anything doing to
/// it. Everything that used to trail off the end of a row as a parenthesised
/// sentence is a cell now, which is what makes a store of four readable at a
/// glance rather than a paragraph at a time.
pub fn accounts(result: &Value) -> String {
    accounts_at(result, now())
}

/// The same listing against a stated clock, since one column counts down.
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

    let rows: Vec<Vec<String>> = accounts.iter().map(|account| row(account, now)).collect();

    // Said once, under the rows rather than on them: it is true of the whole
    // set, and what it tells the operator is why accounts they never wrote
    // down are here — and how to stop them moving when they sign in
    // somewhere else. Only where the whole set is found: a store holding one
    // declared profile is not describing itself.
    let every_row_found = accounts.iter().all(found);
    let found =
        if field(result, "discovered").and_then(Value::as_bool) == Some(true) && every_row_found {
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

    format!(
        "{}{found}{ignored}",
        table(
            &["  NAME", "PROVIDER", "KIND", "ACCOUNT", "SOURCE", "STATE"],
            &rows,
        )
    )
}

/// One account, cell by cell.
fn row(account: &Value, now: u64) -> Vec<String> {
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
    // Named on every row. With two providers in the store an unnamed one is a
    // guess, and the row that leaves it out is the one an operator has to
    // guess about. Empty only where the payload genuinely does not carry it —
    // a daemon that predates providers — because filling that in would be
    // inventing the answer.
    let provider = field(account, "provider")
        .and_then(Value::as_str)
        .unwrap_or("-");
    vec![
        format!("{marker} {name}"),
        provider.to_owned(),
        kind(account).to_owned(),
        who(account),
        source(account),
        state(account, now),
    ]
}

/// What the operator chose, rather than what the credential is.
///
/// The payload's `kind` names the credential — `grant` or `key` — and keeps
/// that meaning. What an operator declared was a profile of another program,
/// or a secret they pasted, and those are the two words the column uses.
fn kind(account: &Value) -> &'static str {
    if is_key(account) { "key" } else { "profile" }
}

fn is_key(account: &Value) -> bool {
    field(account, "kind").and_then(Value::as_str) == Some("key")
}

/// The column that tells two accounts apart.
///
/// The string, then the fallback. Asking for the field first and reading it as
/// a string afterwards makes a present-but-null `email` shadow an account id
/// that is right there.
///
/// The plan comes third because a borrowed Claude grant carries no address and
/// no id at all — its store holds neither — but it does say which subscription
/// it is. Never the word `key`: what the row is belongs to the kind column,
/// and putting it here said the one thing this column is not for.
fn who(account: &Value) -> String {
    if let Some(email) = field(account, "email").and_then(Value::as_str) {
        return email.to_owned();
    }
    if let Some(id) = field(account, "account_id").and_then(Value::as_str) {
        return id.to_owned();
    }
    match field(account, "plan").and_then(Value::as_str) {
        Some(plan) => format!("{plan} plan"),
        None => "-".to_owned(),
    }
}

/// Where the credential was read from (§8.4).
///
/// A name is the operator's own label; this is the thing they can go and look
/// at. A key has no elsewhere — it is this daemon's own secret — and a
/// keychain is named as a keychain, because the item's own name is a string
/// nobody types and it is what pushed the row past a screen.
fn source(account: &Value) -> String {
    if is_key(account) {
        return "stored".to_owned();
    }
    let mark = if found(account) { " (found)" } else { "" };
    let Some(source) = field(account, "source").and_then(Value::as_str) else {
        // A grant this daemon holds itself: no profile behind it to name.
        return format!("stored{mark}");
    };
    if source.starts_with("keychain") {
        return format!("keychain{mark}");
    }
    format!("{}{mark}", shorten(&abbreviate(source)))
}

/// Whether the operator wrote this row down.
///
/// `declared` is absent rather than false on a row nobody declared, and a
/// daemon that predates the field says nothing at all — where its listing is
/// `discovered`, every profile in it was found. Both read the same way here.
/// A key is never found: it is stored, and there is nothing to have found it
/// in.
fn found(account: &Value) -> bool {
    !is_key(account) && field(account, "declared").and_then(Value::as_bool) != Some(true)
}

/// The operator's home, as they write it themselves.
fn abbreviate(source: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return source.to_owned();
    };
    let home = home.to_string_lossy();
    match source.strip_prefix(home.trim_end_matches('/')) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => source.to_owned(),
    }
}

/// The one cell that is ever cut. A path can be arbitrarily long and the row
/// it sits on cannot; the ellipsis says the rest is there rather than letting
/// a truncated path read as a real one.
fn shorten(source: &str) -> String {
    if source.chars().count() <= SOURCE_WIDTH {
        return source.to_owned();
    }
    let kept: String = source.chars().take(SOURCE_WIDTH - 1).collect();
    format!("{kept}…")
}

/// One phrase, and the most urgent true one.
///
/// A credential the backend has already turned away is past being renewed
/// early, and an account that is no longer the one it was chosen as is billing
/// somebody else in the meantime — so those two come first, in that order.
/// `status` carries the remedy for each; this is a listing, and the account it
/// names may not be the one the operator is about to act on.
fn state(account: &Value, now: u64) -> String {
    if field(account, "refused").is_some_and(|refused| !refused.is_null()) {
        return "refused".to_owned();
    }
    if field(account, "identity_changed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return "identity changed".to_owned();
    }
    renewal(
        field(account, "login_expires_at").and_then(Value::as_u64),
        now,
    )
    .unwrap_or_else(|| "ok".to_owned())
}

/// What a rename says it did.
///
/// The new name first, because it is the string every other account verb now
/// takes. Whether the configuration file moved with it goes on its own line,
/// and only when it did: most accounts have no section, and an operator who
/// has one wrote it by hand and will want to know it was rewritten.
pub fn renamed_account(result: &Value) -> String {
    let from = field(result, "renamed")
        .and_then(Value::as_str)
        .unwrap_or("nothing");
    let moved = field(result, "moved_configuration")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match field(result, "name").and_then(Value::as_str) {
        Some(to) if moved => {
            format!("{from} is now {to}\nits section in config.toml moved with it")
        }
        Some(to) => format!("{from} is now {to}"),
        None => format!("renamed {from}"),
    }
}

/// What removing an account says it did, and who is left serving turns.
///
/// The second line matters: removing the account that was serving hands over
/// to another one, and an operator who is not told has to go and look.
pub fn removed_account(result: &Value) -> String {
    let removed = field(result, "removed")
        .and_then(Value::as_str)
        .unwrap_or("nothing");
    if let Some(serving) = field(result, "serving").and_then(Value::as_str) {
        return format!("removed {removed}\nserving turns as {serving}");
    }
    // Nothing is serving, and the two reasons for that want opposite advice.
    // Seen live: removing a leftover out of a store that still held two
    // accounts answered "no accounts left".
    if field(result, "remaining")
        .and_then(Value::as_u64)
        .is_some_and(|remaining| remaining > 0)
    {
        return format!(
            "removed {removed}\nno account is serving turns — choose one with \
             `proxenos accounts use NAME`"
        );
    }
    format!(
        "removed {removed}\nno accounts left — declare a profile under `[profiles]`, \
         or store a key with `proxenos accounts add-key NAME --provider codex|anthropic`"
    )
}

/// What a switch says it did.
///
/// The first line names who serves now — the caller may have typed a label and
/// the daemon is the side that resolved it — and the second says how far the
/// switch moved, because the same six words cannot stand for both sizes of it.
/// Within one provider it changed whose quota is spent. Across providers it
/// changed which backend answers, which path the turn takes, and which
/// subscription is drawn down, and an operator who is not told has to go and
/// look.
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
            "serving turns as {name} on {provider}\n{previous} to {provider}, so a different \
             backend on a different subscription answers every turn"
        ),
        (Some(provider), Some(_)) => format!(
            "serving turns as {name} on {provider}\nstill on {provider}, so the same backend \
             answers and another account's quota is spent"
        ),
        (Some(provider), None) => format!("serving turns as {name} on {provider}"),
        // A daemon that predates the field says what it always said.
        (None, _) => format!("serving turns as {name}"),
    }
}
