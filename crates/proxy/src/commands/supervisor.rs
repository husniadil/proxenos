//! `supervisor` — the part of supervision that touches the machine.

use super::Answering;
use super::STOP_WINDOW;
use super::answering;
use super::watch;
use crate::cli;
use anyhow::Result;
use anyhow::bail;

/// The origin this machine and this shell describe, for the unit.
fn supervisor_origin() -> Result<proxenos::supervisor::Origin> {
    use anyhow::Context;
    Ok(proxenos::supervisor::Origin {
        program: std::env::current_exe().context("could not find this binary's own path")?,
        log: proxenos::config::config_dir().join("daemon.log"),
        proxenos_home: std::env::var_os("PROXENOS_HOME"),
        tmpdir: std::env::var_os("TMPDIR"),
    })
}

/// The launchd domain a per-user LaunchAgent lives in.
///
/// The uid comes from the owner of the home directory being written into,
/// which is the same question asked a different way and needs no dependency:
/// the agent is installed for whoever owns that directory.
#[cfg(unix)]
fn gui_domain(home: &std::path::Path) -> std::io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("gui/{}", std::fs::metadata(home)?.uid()))
}

/// The same question where there is no launchd to ask it of.
///
/// Never reached: `plan` refuses a platform without launchd before anything
/// here runs. It exists because the refusal is a runtime decision while the
/// file still has to compile everywhere this ships, and a uid is a Unix idea.
#[cfg(not(unix))]
fn gui_domain(_home: &std::path::Path) -> std::io::Result<String> {
    Err(std::io::Error::other(
        "launchd is not this platform's supervisor",
    ))
}

fn launchctl(arguments: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("launchctl")
        .args(arguments)
        .output()
}

/// What is still answering once the daemon this verb just booted out has gone.
///
/// `bootout` returns before the process it signalled has finished exiting, so a
/// read taken immediately still sees it and the reinstall reports a daemon that
/// is on its way out. Anything still there when the window closes is one this
/// verb did not end, which is the only kind worth naming.
async fn settled_after_bootout() -> Option<Answering> {
    let mut last = None;
    watch(STOP_WINDOW, |now| {
        last = now.clone();
        now.is_none()
    })
    .await;
    last
}

/// Install, remove, or report the supervisor.
///
/// The decisions all happened in `proxenos::supervisor`; what is left here is
/// the part that touches the machine.
pub(crate) async fn supervisor(args: cli::SupervisorArgs) -> Result<()> {
    use anyhow::Context;
    use proxenos::supervisor;

    // §2 — every action here writes or reads a launchd unit on THIS machine.
    // Pointed at a daemon elsewhere, `install` would supervise a second daemon
    // on the client's own port and `status` would report about it, both
    // looking exactly like success.
    proxenos::control::Endpoint::resolve()?.refuse_remote("supervisor")?;

    let origin = supervisor_origin()?;
    let unit = supervisor::plan(&supervisor::Platform::current(), &origin)?;
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .context("HOME names nothing, so there is no per-user agent directory to write into")?;
    let plist = supervisor::plist_path(&home);
    let domain = gui_domain(&home)
        .with_context(|| format!("could not read the owner of {}", home.display()))?;
    let existing = std::fs::read_to_string(&plist).ok();
    let target = format!("{domain}/{}", unit.label);

    match args.action {
        cli::SupervisorAction::Install => {
            if let Some(parent) = plist.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("could not create {}", parent.display()))?;
            }
            if let Some(parent) = unit.log.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("could not create {}", parent.display()))?;
            }
            // Replacing rather than adding: launchd refuses to bootstrap a
            // label already loaded, and an operator reinstalling after moving
            // the binary means the new one.
            let before = answering().await;
            let booted_out = existing.is_some();
            if booted_out {
                let _ = launchctl(&["bootout", &target]);
            }
            // The second read is worth taking only where a bootout freed
            // something, and it happens here rather than after the bootstrap
            // below: once the job is loaded, what answers is the job itself.
            let after = if booted_out {
                settled_after_bootout().await
            } else {
                None
            };
            let held = supervisor::holder(
                booted_out,
                before.as_ref().map(|a| a.version.as_str()),
                after.as_ref().map(|a| a.version.as_str()),
            );

            std::fs::write(&plist, supervisor::render(&unit))
                .with_context(|| format!("could not write {}", plist.display()))?;

            let output = launchctl(&["bootstrap", &domain, &plist.to_string_lossy()])
                .context("could not run launchctl")?;
            if !output.status.success() {
                // A unit on disk that the supervisor never accepted is the
                // half-installed state this verb exists to avoid, so it does
                // not survive the failure that produced it.
                let _ = std::fs::remove_file(&plist);
                bail!(
                    "launchctl refused the unit, so nothing was installed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }

            println!("supervising {}, from {}", unit.label, plist.display());
            println!("  runs {} run", unit.program.display());
            println!("  logs to {}", unit.log.display());
            println!("  control socket {}", unit.socket.display());
            if let Some(notice) = supervisor::port_notice(held) {
                println!("{notice}");
            }
            println!("stop it for good with `proxenos supervisor uninstall`");
        }
        cli::SupervisorAction::Uninstall => {
            if existing.is_none() {
                println!("nothing installed at {}", plist.display());
                return Ok(());
            }
            let output = launchctl(&["bootout", &target]).context("could not run launchctl")?;
            std::fs::remove_file(&plist)
                .with_context(|| format!("could not remove {}", plist.display()))?;
            if !output.status.success() {
                println!(
                    "removed {}; launchctl said: {}",
                    plist.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                return Ok(());
            }
            println!(
                "removed {}; the daemon it supervised is stopped",
                plist.display()
            );
        }
        cli::SupervisorAction::Status => {
            let installed = supervisor::compare(existing.as_deref(), &unit);
            if args.json {
                let (state, pid) = launchctl_state(&target);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "installed": match installed {
                            supervisor::Installed::Absent => "absent",
                            supervisor::Installed::Current => "current",
                            supervisor::Installed::Divergent => "divergent",
                        },
                        "plist": plist,
                        "program": unit.program,
                        "log": unit.log,
                        "socket": unit.socket,
                        "state": state,
                        "pid": pid,
                    }))?
                );
                return Ok(());
            }
            match installed {
                supervisor::Installed::Absent => {
                    println!("not supervised; install it with `proxenos supervisor install`");
                    return Ok(());
                }
                supervisor::Installed::Current => {
                    println!("supervised, from {}", plist.display());
                }
                // The failure this verb is here to make visible rather than
                // let an operator discover as a daemon that answers turns while
                // every CLI verb reports connection refused.
                supervisor::Installed::Divergent => {
                    println!(
                        "supervised from {}, but by a unit this environment would not write.",
                        plist.display()
                    );
                    println!(
                        "  the daemon it starts may bind a control socket other than the {} \
                         this shell dials, in which case it serves turns on the port while every \
                         verb here reports connection refused.",
                        unit.socket.display()
                    );
                    println!("  reinstall it with `proxenos supervisor install`");
                }
            }
            println!("  control socket {}", unit.socket.display());

            let (state, pid) = launchctl_state(&target);
            println!(
                "  the supervisor says: state {}, pid {}",
                state.as_deref().unwrap_or("unknown to the supervisor"),
                pid.map_or_else(|| "none".to_owned(), |pid| pid.to_string()),
            );
        }
    }
    Ok(())
}

/// What the supervisor itself says about the job: its state word and the pid
/// it holds, each absent where launchd did not say.
fn launchctl_state(target: &str) -> (Option<String>, Option<u64>) {
    let Ok(output) = launchctl(&["print", target]) else {
        return (None, None);
    };
    let printed = String::from_utf8_lossy(&output.stdout);
    let state = printed
        .lines()
        .find_map(|line| line.trim().strip_prefix("state = "))
        .map(|state| state.trim().to_owned());
    let pid = printed
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid = "))
        .and_then(|pid| pid.trim().parse().ok());
    (state, pid)
}
