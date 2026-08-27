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
    let result = control::call(&control::default_path(), "env", None).await?;
    println!("{}", render::env_shell(&result));
    Ok(())
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
    if let Some(account) = &args.account {
        let accounts = control::call(&control::default_path(), "accounts", None)
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
    let result = control::call(&control::default_path(), "env", params)
        .await
        .map_err(daemon_silent)?;

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
    // only from it — `curated` is true only when the serving account relays,
    // so a daemon translating to the first provider launches exactly as
    // before. A failed read skips the upgrade rather than the launch: the
    // session it starts is correct either way, just on the standard window.
    let mut command = args.command.clone();
    if let Ok(models) = control::call(&control::default_path(), "models", None).await
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
        && let Ok(accounts) = control::call(&control::default_path(), "accounts", None).await
        && let Some(line) = render::serving_line(&accounts)
    {
        eprintln!("{line}");
    }

    let mut child = std::process::Command::new(&plan.program);
    child.args(&plan.arguments);
    for (name, value) in render::variables(&result) {
        child.env(name, value);
    }
    if let Some(account) = &args.account {
        child.env(
            "ANTHROPIC_AUTH_TOKEN",
            format!("{}{account}", proxenos::ingress::ACCOUNT_TAG),
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
    let result = control::call(&control::default_path(), "env", None).await?;
    // A document silently missing a permission rule looks complete and behaves
    // otherwise, so this stops rather than prints.
    control::require_client_policy(&result)?;
    println!("{}", render::settings_json(&result));
    Ok(())
}
