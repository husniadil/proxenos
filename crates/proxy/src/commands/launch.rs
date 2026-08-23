//! Starting something else with this proxy's configuration applied.

use crate::cli;
use anyhow::Result;
use proxenos::control;
use proxenos::render;

pub(crate) async fn print_env(args: cli::EnvArgs) -> Result<()> {
    // The JSON form is the `settings` document, so it goes through `settings`
    // rather than alongside it. Rendering it here as well left one of the two
    // names unguarded, and it was the older name — the one a caller reaches for
    // out of habit — that printed a document quietly missing a permission rule.
    if args.json {
        return print_settings().await;
    }

    let result = control::call(&control::default_path(), "env", None).await?;
    println!("{}", render::env_shell(&result));
    Ok(())
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
    let result = control::call(&control::default_path(), "env", None)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "the daemon is not answering ({error}), so there is no configuration to start \
                 this with. Start it with `proxenos run`."
            )
        })?;

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

    // Who pays for this session, said once, at the one moment there is a
    // person deciding whether to start it. A borrowed grant is why it is worth
    // saying: the account is a directory signed into somewhere else, and it
    // can have become somebody else since it was chosen. A read that fails
    // says nothing rather than delaying the launch.
    if let Ok(accounts) = control::call(&control::default_path(), "accounts", None).await
        && let Some(line) = render::serving_line(&accounts)
    {
        eprintln!("{line}");
    }

    let mut child = std::process::Command::new(&plan.program);
    child.args(&plan.arguments);
    for (name, value) in render::variables(&result) {
        child.env(name, value);
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
/// The same bytes `env --json` prints. Two names, one document: a caller that
/// reaches for the obvious one must not get the half that leaves the policy out.
pub(crate) async fn print_settings() -> Result<()> {
    let result = control::call(&control::default_path(), "env", None).await?;
    // A document silently missing a permission rule looks complete and behaves
    // otherwise, so this stops rather than prints.
    control::require_client_policy(&result)?;
    println!("{}", render::settings_json(&result));
    Ok(())
}
