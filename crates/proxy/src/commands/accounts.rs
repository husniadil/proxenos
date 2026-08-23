//! The `accounts` family: list, login, add-key, use, rename, remove.
//!
//! One sub-verb per thing an operator does. The listing and the three changes
//! that only the daemon can make go through the control socket, because the
//! daemon is what holds the selection: a CLI that wrote the file directly
//! would leave a running daemon serving the account it read at startup. The
//! two that *add* an account do not — they write a credential file or run
//! somebody else's client, and neither needs a daemon to be running.

use super::account_store;
use crate::cli;
use anyhow::Result;
use proxenos::control;
use proxenos::render;
use std::sync::Arc;

pub(crate) async fn accounts(args: cli::AccountsArgs) -> Result<()> {
    match args.action {
        None => list(args.json).await,
        Some(cli::AccountsAction::List(list_args)) => list(args.json || list_args.json).await,
        Some(cli::AccountsAction::Login(login_args)) => sign_in_profile(&login_args).await,
        Some(cli::AccountsAction::AddKey(key_args)) => {
            store_key(&key_args.name, key_args.provider).await
        }
        Some(cli::AccountsAction::Use(named)) => select(&named.name).await,
        Some(cli::AccountsAction::Rename(names)) => rename(&names.from, &names.to).await,
        Some(cli::AccountsAction::Remove(named)) => remove(&named.name).await,
    }
}

/// Stored accounts, and which one serves turns.
async fn list(json: bool) -> Result<()> {
    let result = control::call(&control::default_path(), "accounts", None).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    println!("{}", render::accounts(&result));
    Ok(())
}

async fn select(name: &str) -> Result<()> {
    let result = control::call(
        &control::default_path(),
        "accounts.select",
        Some(serde_json::json!({ "account": name })),
    )
    .await?;
    println!("{}", render::selected_account(&result));
    Ok(())
}

async fn rename(from: &str, to: &str) -> Result<()> {
    let result = control::call(
        &control::default_path(),
        "accounts.rename",
        Some(serde_json::json!({ "account": from, "name": to })),
    )
    .await?;
    println!("{}", render::renamed_account(&result));
    Ok(())
}

async fn remove(name: &str) -> Result<()> {
    let result = control::call(
        &control::default_path(),
        "accounts.remove",
        Some(serde_json::json!({ "account": name })),
    )
    .await?;
    println!("{}", render::removed_account(&result));
    Ok(())
}

/// `accounts login` — run the owning program's own login, then declare what
/// it wrote (§8.4).
///
/// Everything about the credential happens inside that program. This side
/// chooses a directory, points the client at it, and afterwards reads the
/// profile to find out whether there is anything to declare — which is the
/// same read every turn makes, so a profile that passes here is one the daemon
/// can actually serve.
///
/// **The URL the client prints is never opened here.** Opening it would hand
/// the authorization to whichever account the default browser is already
/// signed into, which is a choice this command has no basis for making.
async fn sign_in_profile(args: &cli::AccountLoginArgs) -> Result<()> {
    let config = proxenos::config::Config::load()?;
    // The key store's names, so a profile cannot be declared under one. The
    // store is the only place they are, and asking it is cheaper than a
    // login run for nothing.
    let store: Arc<dyn proxenos::auth::store::AccountStore> = Arc::new(account_store()?);
    let keys: Vec<String> = store
        .accounts()?
        .into_iter()
        .filter(|account| account.kind == "key")
        .map(|account| account.name)
        .collect();
    let plan = proxenos::auth::profile_login::plan(
        &args.name,
        args.provider,
        args.path.clone(),
        &config,
        &proxenos::config::config_dir(),
        &keys,
    )?;
    let mut environment = proxenos::auth::profile_login::Stdio::new()?;
    proxenos::auth::profile_login::run(&plan, &mut environment)?;
    reload_daemon().await;
    Ok(())
}

/// Store a key, read from stdin.
///
/// The reading half lives in `auth::key_login`, behind a `Guide` seam, so the
/// tty and pipe halves are testable without a terminal.
async fn store_key(name: &str, provider: proxenos::auth::store::Provider) -> Result<()> {
    let store: Arc<dyn proxenos::auth::store::AccountStore> = Arc::new(account_store()?);
    let mut guide = proxenos::auth::key_login::Terminal::stdio(provider);
    let stored = proxenos::auth::key_login::run(store.as_ref(), &mut guide, name)?;
    report_serving(&store, &stored).await;
    Ok(())
}

/// Say what the account just stored does to the next turn, and tell a running
/// daemon only where the answer is "serves it".
///
/// A login stores a credential; choosing what serves turns is the other
/// decision, and `accounts use NAME` is the verb for it. So this reports which
/// of the two happened rather than asserting the switch every login used to
/// make.
async fn report_serving(store: &Arc<dyn proxenos::auth::store::AccountStore>, stored: &str) {
    let serving = store
        .accounts()
        .ok()
        .and_then(|accounts| accounts.into_iter().find(|account| account.selected))
        .map(|account| account.name);
    match serving.as_deref() {
        Some(serving) if serving == stored => {
            println!("It serves turns from now on.");
            hand_over_to(stored).await;
        }
        Some(serving) => {
            println!("{serving} still serves turns.\n  switch with: proxenos accounts use {stored}")
        }
        // Nothing selected at all is not a state a login leaves behind, and
        // saying nothing beats guessing which account a turn would reach.
        None => {}
    }
}

/// Tell a running daemon that this account is the one serving turns now.
///
/// The CLI writes the credential file directly, and the daemon reads it on
/// every request — so the account moves either way. What does not move without
/// this is everything a switch carries with it (`api.md` §3): the
/// conversations already bound to the previous account, its quota, and its
/// model list. A live conversation keeps the endpoint it dialed, and after a
/// change of kind that endpoint refuses every turn it is given.
///
/// Best effort by design. No daemon running is the ordinary case for a login,
/// and it is not a failure of one.
async fn hand_over_to(name: &str) {
    let path = control::default_path();
    let told = control::call(
        &path,
        "accounts.select",
        Some(serde_json::json!({ "account": name })),
    )
    .await;
    match told {
        Ok(_) => println!("The running daemon now serves turns as {name}."),
        // A refusal is worth saying. A switch can now be refused for a reason
        // the operator can fix — a mapping this account cannot serve — and
        // staying quiet would leave a daemon serving the previous account with
        // nothing said about why. Only where there is a daemon to have refused:
        // no socket is no daemon, which is an ordinary state after a login and
        // not something to report as a failure.
        Err(error) if path.exists() => {
            eprintln!("The running daemon is still serving another account: {error}");
        }
        Err(_) => {}
    }
}

/// Hand a `[profiles]` edit to a daemon that is already serving.
///
/// Best effort, exactly as `hand_over_to` is: no socket is no daemon, which is
/// an ordinary state for a login and not a failure. What it replaces is a
/// sentence telling the operator to stop the daemon and let it come back —
/// advice that was true only because nothing re-read the file.
pub(crate) async fn reload_daemon() {
    let path = control::default_path();
    match control::call(&path, "config.reload", None).await {
        Ok(_) => println!("The running daemon has re-read config.toml and holds it now."),
        // A refusal is the operator's to see: it means the file this just
        // wrote is one the daemon will not run, and staying quiet would leave
        // them believing the account is there.
        Err(error) if path.exists() => {
            eprintln!("The running daemon did not take the change: {error}");
        }
        Err(_) => {}
    }
}
