//! What the daemon can be asked about: `status`, `models`, `usage`, and the
//! status line that wraps another one.

use crate::cli;
use anyhow::Result;
use proxenos::control;
use proxenos::render;

/// Every verb but `run` works through the control socket. The CLI holds no
/// state of its own, so a second front-end needs no new daemon work.
pub(crate) async fn print_status() -> Result<()> {
    let result = control::call(&control::default_path(), "status", None).await?;
    println!("{}", render::status(&result));
    Ok(())
}

pub(crate) async fn print_models() -> Result<()> {
    let result = control::call(&control::default_path(), "models", None).await?;
    println!("{}", render::models(&result));
    Ok(())
}

/// What quota is left. Reported from the snapshot the backend volunteers on
/// each turn, so it costs nothing to ask and is as of the last turn made.
///
/// `--refresh` asks first, which spends a request per askable account and fills
/// in the accounts no turn has ever been served as. What is reported afterwards
/// is still the `usage` document, so the shape a status line parses does not
/// depend on whether a figure was just asked for.
pub(crate) async fn print_usage(args: cli::UsageArgs) -> Result<()> {
    let socket = control::default_path();
    if args.refresh {
        control::call(&socket, "usage.refresh", None).await?;
    }
    let result = control::call(&socket, "usage", None).await?;
    println!(
        "{}",
        if args.json {
            serde_json::to_string_pretty(&result)?
        } else {
            render::usage(&result)
        }
    );
    Ok(())
}

/// Wrap a status-line script, adding what the client cannot supply.
///
/// The client hands a status line a JSON payload with no quota in it, because
/// it tracks a subscription quota for its own account and a proxy is not one.
/// This reads that payload, merges in what the backend reported, and hands it to
/// the user's own command — which keeps working exactly as written.
///
/// **It never fails the status line.** A daemon that is not running, a socket
/// that does not answer, a snapshot that cannot be read: every one of those
/// passes the payload through unchanged. A status line renders constantly, and
/// one that breaks is worse than one missing a figure.
pub(crate) async fn statusline(args: cli::StatuslineArgs) -> Result<()> {
    use std::io::Read;
    use std::io::Write;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    let payload: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);
    let usage = control::call(&control::default_path(), "usage", None)
        .await
        .unwrap_or(serde_json::Value::Null);
    let merged = proxenos::statusline::merge(payload, &usage);

    // Falls back to what arrived: if the payload would not parse, merging
    // produced null, and handing a script `null` is worse than handing it the
    // bytes it was going to get anyway.
    let body = if merged.is_null() {
        input
    } else {
        serde_json::to_string(&merged)?
    };

    let Some((program, arguments)) = args.command.split_first() else {
        println!("{body}");
        return Ok(());
    };

    let mut child = std::process::Command::new(program)
        .args(arguments)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body.as_bytes())?;
    }
    let status = child.wait()?;

    // The child's status is this command's: a wrapper that swallows a failure
    // makes the thing it wraps undebuggable.
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
