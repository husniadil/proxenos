//! The account verbs: `login` and `accounts`.

use super::account_store;
use crate::cli;
use anyhow::Result;
use proxenos::control;
use proxenos::render;
use std::sync::Arc;

/// Login runs in the CLI rather than through the socket: it needs a callback
/// port, and the daemon need not be running to authenticate.
///
/// **The URL is printed, never opened.** Opening it hands the authorization to
/// whichever account the default browser is already signed into, which is a
/// choice this command has no basis for making — the grant it produces is the
/// one every later request spends. Printing it leaves the choice where it
/// belongs, and costs one paste.
pub(crate) async fn login(args: cli::LoginArgs) -> Result<()> {
    let store: Arc<dyn proxenos::auth::store::AccountStore> = Arc::new(account_store()?);

    if args.profile {
        return sign_in_profile(&args);
    }
    if !args.key {
        anyhow::bail!(
            "`login` needs to be told which kind. `--profile --as NAME` signs in to a new \
             profile of the owning program and declares it; `--key --as NAME` stores an API \
             key. This daemon obtains no subscription grant of its own either way."
        );
    }

    store_key(&store, args.label.as_deref(), args.provider).await
}

/// `login --profile` — run the owning program's own login, then declare what
/// it wrote (§8.4).
///
/// Everything about the credential happens inside that program. This side
/// chooses a directory, points the client at it, and afterwards reads the
/// profile to find out whether there is anything to declare — which is the
/// same read every turn makes, so a profile that passes here is one the daemon
/// can actually serve.
fn sign_in_profile(args: &cli::LoginArgs) -> Result<()> {
    let config = proxenos::config::Config::load()?;
    let plan = proxenos::auth::profile_login::plan(
        args.label.as_deref(),
        args.provider,
        args.path.clone(),
        &config,
        &proxenos::config::config_dir(),
    )?;
    let mut environment = proxenos::auth::profile_login::Stdio::new()?;
    proxenos::auth::profile_login::run(&plan, &mut environment)
}

/// Store a key, read from stdin.
///
/// The reading half lives in `auth::key_login`, behind a `Guide` seam, so the
/// tty and pipe halves are testable without a terminal.
async fn store_key(
    store: &Arc<dyn proxenos::auth::store::AccountStore>,
    label: Option<&str>,
    provider: proxenos::auth::store::Provider,
) -> Result<()> {
    let mut guide = proxenos::auth::key_login::Terminal::stdio(provider);
    let name = proxenos::auth::key_login::run(store.as_ref(), &mut guide, label)?;
    report_serving(store, &name).await;
    Ok(())
}

/// Say what the account just stored does to the next turn, and tell a running
/// daemon only where the answer is "serves it".
///
/// A login stores a credential; choosing what serves turns is the other
/// decision, and `accounts --use` is the verb for it. So this reports which of
/// the two happened rather than asserting the switch every login used to make.
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
        Some(serving) => println!(
            "{serving} still serves turns.\n  switch with: proxenos accounts --use {stored}"
        ),
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

/// Stored accounts, and which one serves turns.
///
/// Through the socket, because the daemon is what holds the selection: a CLI
/// that wrote the file directly would leave a running daemon serving the
/// account it read at startup.
pub(crate) async fn accounts(args: cli::AccountsArgs) -> Result<()> {
    if let Some(names) = args.rename {
        let [from, to] = names.as_slice() else {
            anyhow::bail!("`--rename` takes the old name and the new one");
        };
        let result = control::call(
            &control::default_path(),
            "accounts.rename",
            Some(serde_json::json!({ "account": from, "name": to })),
        )
        .await?;
        println!("{}", render::renamed_account(&result));
        return Ok(());
    }

    if let Some(name) = args.forget {
        let result = control::call(
            &control::default_path(),
            "accounts.forget",
            Some(serde_json::json!({ "account": name })),
        )
        .await?;
        println!("{}", render::forgotten_account(&result));
        return Ok(());
    }

    if let Some(name) = args.select {
        let result = control::call(
            &control::default_path(),
            "accounts.select",
            Some(serde_json::json!({ "account": name })),
        )
        .await?;
        println!("{}", render::selected_account(&result));
        return Ok(());
    }

    let result = control::call(&control::default_path(), "accounts", None).await?;
    println!("{}", render::accounts(&result));
    Ok(())
}
