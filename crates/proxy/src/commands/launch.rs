//! Starting something else with this proxy's configuration applied.

use crate::cli;
use anyhow::Result;
use proxenos::control;
use proxenos::render;

/// Shell exports, and only those.
///
/// There is no `--json` here: the settings document is what that flag used to
/// print, and it has a verb of its own. `--json` means one thing on every verb
/// that takes it — the control socket's payload, unrendered — and a flag that
/// meant "a different verb's document" on one of them is a flag nobody can
/// read off the surface.
pub(crate) async fn print_env() -> Result<()> {
    let endpoint = control::Endpoint::resolve()?;
    let mut result = control::dial(&endpoint, "env", None).await?;
    let carried = point_at(&endpoint, &mut result);

    // §2.2 — the token is not printed, ever. These exports are what an
    // operator pastes into a shell and a shell's history; the base URL is the
    // half a remote session actually needs from here, and the line that would
    // carry the secret is replaced by the command that sets it from the
    // variable this process already read.
    if !carried && endpoint.token().is_some() {
        println!(
            "# ANTHROPIC_AUTH_TOKEN is left out: it would carry this daemon's token. Set it with"
        );
        println!("#   export ANTHROPIC_AUTH_TOKEN=\"proxenos-token:$PROXENOS_TOKEN\"");
        println!("# or start the client with `proxenos exec`, which sets it without printing it.");
    }
    println!("{}", render::env_shell(&result));
    Ok(())
}

/// Point an `env` payload at the daemon that answered it, and take the token
/// out of what is about to be printed.
///
/// The daemon states its own base URL as `http://127.0.0.1:<port>`, which is
/// the truth on its machine and useless on any other. Rewritten here rather
/// than in the daemon because the daemon does not know what address the caller
/// reached it on — only this side knows which URL it dialed.
///
/// Returns whether `ANTHROPIC_AUTH_TOKEN` is still in the payload.
fn point_at(endpoint: &control::Endpoint, result: &mut serde_json::Value) -> bool {
    let Some(url) = endpoint.remote_url() else {
        return true;
    };
    let secret = endpoint.token().is_some();
    let Some(variables) = result
        .get_mut("variables")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return !secret;
    };

    for entry in variables.iter_mut() {
        if entry.get(0).and_then(serde_json::Value::as_str) == Some("ANTHROPIC_BASE_URL")
            && let Some(pair) = entry.as_array_mut()
            && let Some(value) = pair.get_mut(1)
        {
            *value = serde_json::Value::from(url);
        }
    }
    if secret {
        variables.retain(|entry| {
            entry.get(0).and_then(serde_json::Value::as_str) != Some("ANTHROPIC_AUTH_TOKEN")
        });
    }
    !secret
}

/// What a launch says when the socket does not answer.
///
/// Launching regardless would hand the operator a connection refused from a
/// client that cannot explain it, so every call `exec` makes before starting
/// anything fails with this one sentence.
fn daemon_silent(error: proxenos::error::ProxyError) -> anyhow::Error {
    anyhow::anyhow!(
        "the daemon is not answering ({error}), so there is no configuration to start \
         this with. Start it with `proxenos run`."
    )
}

/// Start a program with this proxy's configuration applied.
///
/// The environment half is set on the child, which is the launcher's whole job.
/// The policy half has no environment variable (`proxy-behavior.md` §7.3), so it
/// rides on the client's own settings flag and nothing is written to disk: no
/// file to go stale, and none to clean up. The document holds no secret — the
/// auth token's value is ignored by design — so argv is a fine place for it.
///
/// **It refuses before starting anything when the daemon is not answering.**
/// Launching regardless would hand the operator a connection refused from a
/// client that cannot explain it.
pub(crate) async fn exec(args: cli::ExecArgs) -> Result<()> {
    // `--account`: this session's turns are made as the named account, and
    // the selection is neither read nor moved. The name is checked against
    // the store before anything starts — a session that refuses its first
    // turn is a worse place to learn about a typo than a launch that refuses
    // to happen — and it travels as the auth token value, which the daemon
    // reads and the backend never sees.
    //
    // Checked first because the environment is asked for *as* this account:
    // the daemon refuses an unknown name there too, and that refusal would
    // arrive wrapped in a sentence about a daemon that is answering fine.
    let endpoint = control::Endpoint::resolve()?;
    if let Some(account) = &args.account {
        let accounts = control::dial(&endpoint, "accounts", None)
            .await
            .map_err(daemon_silent)?;
        let stored: Vec<&str> = accounts
            .get("accounts")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("name").and_then(serde_json::Value::as_str))
            .collect();
        if !stored.contains(&account.as_str()) {
            anyhow::bail!(
                "`{account}` names no stored account; this daemon holds {}",
                stored
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        eprintln!("serving this session as `{account}`");
    }

    // §2.2 — the environment for the account that will serve this session's
    // turns. A launch tagged onto another account and configured from the
    // selection is handed the wrong provider's tier ids, and sends them.
    let params = args
        .account
        .as_ref()
        .map(|account| serde_json::json!({ "account": account }));
    let mut result = control::dial(&endpoint, "env", params.clone())
        .await
        .map_err(daemon_silent)?;
    // §2.3 — a client started from here talks to the daemon this CLI is
    // pointed at, whichever machine that is.
    point_at(&endpoint, &mut result);

    // Same stop as `settings`, and for a sharper reason: a launch that quietly
    // omits the policy produces a working session with a rule missing from it,
    // and nothing about that session would ever say so.
    control::require_client_policy(&result)?;

    // Compact rather than pretty: this goes on a command line, and a document
    // full of newlines is one that a person reading `ps` has to reassemble.
    let policy = result
        .get("settings")
        .filter(|policy| !policy.is_null())
        .map(serde_json::to_string)
        .transpose()?;

    // A plain `--model` id is upgraded to its long-context variant where the
    // serving account offers one. Eligibility comes from the curated list and
    // only from it — `curated` is true only when the account that serves this
    // session relays, so a daemon translating to the first provider launches
    // exactly as before. A failed read skips the upgrade rather than the
    // launch: the session it starts is correct either way, just on the
    // standard window.
    //
    // Asked for the same account the environment was, and for the same reason
    // (§2.2): the list is one account's menu, and the selection's answers
    // whether *its* ids have a long-context variant rather than whether this
    // session's do.
    let mut command = args.command.clone();
    if let Ok(models) = control::dial(&endpoint, "models", params).await
        && models.get("curated").and_then(serde_json::Value::as_bool) == Some(true)
    {
        let eligible: Vec<String> = models
            .get("models")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
            .filter_map(|id| id.strip_suffix("[1m]"))
            .map(str::to_owned)
            .collect();
        if let Some((plain, long)) =
            proxenos::launch::upgrade_model_argument(&mut command, &eligible)
        {
            eprintln!("note: --model {plain} upgraded to {long} — the long-context window");
        }
    }

    let plan = proxenos::launch::plan(&command, policy.as_deref())?;

    // By design, and said out loud: to the person whose deny rule just
    // vanished, a silent by-design is indistinguishable from a bug.
    if policy.is_some() && !plan.carries_policy {
        eprintln!(
            "note: the client policy rides `--settings`, which `{}` does not take. \
             This launch carries the environment only.",
            plan.program
        );
    }

    // Which provider's ids this session was handed, said where there is a
    // choice to have got wrong: the flag decides the mapping as well as the
    // payer, and nothing else on the way past prints it.
    if let Some(account) = &args.account {
        eprintln!("{}", render::tier_mapping_line(&result, account));
    }

    // Who pays for this session, said once, at the one moment there is a
    // person deciding whether to start it. A borrowed grant is why it is worth
    // saying: the account is a directory signed into somewhere else, and it
    // can have become somebody else since it was chosen. A read that fails
    // says nothing rather than delaying the launch.
    if args.account.is_none()
        && let Ok(accounts) = control::dial(&endpoint, "accounts", None).await
        && let Some(line) = render::serving_line(&accounts)
    {
        eprintln!("{line}");
    }

    let mut child = std::process::Command::new(&plan.program);
    child.args(&plan.arguments);
    for (name, value) in render::variables(&result) {
        child.env(name, value);
    }
    // §1 — the token and the account tag share the one header the client
    // offers, built in one place so the launcher and the daemon's parser
    // cannot disagree about the separator. Set only where there is something
    // to say: with neither, the payload's own `unused` stands.
    if endpoint.token().is_some() || args.account.is_some() {
        child.env(
            "ANTHROPIC_AUTH_TOKEN",
            proxenos::ingress::auth_token_value(endpoint.token(), args.account.as_deref()),
        );
    }

    // A real exec on Unix, so signals, job control, the terminal, and the exit
    // status all pass through untouched. A wrapper sitting in the middle would
    // orphan the child on a Ctrl-C.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Only returns if the exec itself failed.
        let error = child.exec();
        Err(anyhow::Error::new(error).context(format!("could not start `{}`", plan.program)))
    }
    #[cfg(not(unix))]
    {
        let status = child.status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// One complete client settings document, for a file or a launcher.
///
/// The only name for this document. `env --json` printed it too, which left
/// one flag meaning two different things across four verbs — and left the
/// document reachable under a verb whose whole subject is the environment.
pub(crate) async fn print_settings() -> Result<()> {
    let endpoint = control::Endpoint::resolve()?;
    // §2.2 — refused rather than printed without the token, and refused rather
    // than printed with it. This document is one blob a client reads whole: an
    // `env` block missing its auth token is a document that does not work, and
    // one carrying the token is a secret on stdout. `exec` is the client-mode
    // launcher, and it sets both without printing either.
    if endpoint.token().is_some() {
        anyhow::bail!(
            "this document would have to carry the daemon's token, and a secret on stdout is \
             not something this verb will print. Start the client with `proxenos exec` \
             instead, which sets the environment on the child without printing it."
        );
    }
    let mut result = control::dial(&endpoint, "env", None).await?;
    point_at(&endpoint, &mut result);
    // A document silently missing a permission rule looks complete and behaves
    // otherwise, so this stops rather than prints.
    control::require_client_policy(&result)?;
    println!("{}", render::settings_json(&result));
    Ok(())
}
