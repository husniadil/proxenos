//! `inspect` — what another process's environment says about how it was
//! started.
//!
//! Needs no daemon: the answer is in the process being asked about, so this
//! works in client mode and works with nothing running at all. Reading that
//! environment is the only I/O here; everything the answer is made of is
//! decided in `proxenos::process` as a pure function over the text.

use crate::cli;
use anyhow::Result;

/// Which account another agent's turns go as.
///
/// The refusals name which of the two things went wrong — a pid nobody is
/// using, or an environment that cannot be read — because they call for
/// different next steps, and an answer of `not through proxenos` for either
/// would be a wrong answer rather than a missing one.
pub(crate) fn inspect(args: cli::InspectArgs) -> Result<()> {
    let environment = environment_of(args.pid)?;
    let launched = proxenos::process::read(&environment);

    if args.json {
        // The token the environment may have carried is not here to omit: it
        // was dropped where it was parsed, and `Launched` has no field for it.
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pid": args.pid,
                "through": launched.through,
                "account": launched.account,
                "daemon": launched.daemon,
            }))?
        );
        return Ok(());
    }
    println!("{}", launched.line(args.pid));
    Ok(())
}

/// One process's environment, as its platform hands it over.
#[cfg(target_os = "linux")]
fn environment_of(pid: u32) -> Result<String> {
    let directory = std::path::PathBuf::from(format!("/proc/{pid}"));
    if !directory.exists() {
        anyhow::bail!("no process {pid} is running");
    }
    let text = std::fs::read(directory.join("environ"))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|error| {
            anyhow::anyhow!("the environment of process {pid} could not be read: {error}")
        })?;
    // Readable and empty is what a zombie or a kernel thread hands over: an
    // environment that could not be read, not one that says nothing (§2.8).
    if !proxenos::process::carries_environment(&text) {
        anyhow::bail!("the environment of process {pid} could not be read: it is empty");
    }
    Ok(text)
}

/// The same, where the environment is only readable through `ps`.
///
/// `ps -Eww` prints the environment of the caller's **own** processes and the
/// command alone for everyone else's — which is not an error, so a command
/// line with no assignment after it is read as an environment that could not be
/// read rather than as one that says nothing. An agent in a pane is the
/// caller's own process, which is the case this verb exists for.
#[cfg(not(target_os = "linux"))]
fn environment_of(pid: u32) -> Result<String> {
    let output = std::process::Command::new("ps")
        .args(["-Eww", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .map_err(|error| anyhow::anyhow!("could not run `ps` to read process {pid}: {error}"))?;
    let command = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || command.is_empty() {
        anyhow::bail!("no process {pid} is running");
    }
    if !proxenos::process::carries_environment(&command) {
        anyhow::bail!(
            "the environment of process {pid} could not be read; `ps` shows it only for \
             processes running as you"
        );
    }
    Ok(command)
}
