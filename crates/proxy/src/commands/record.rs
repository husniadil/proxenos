//! `record` — capture exchanges as fixtures.

use super::account_store;
use super::daemon::Capture;
use super::daemon::run_with;
use crate::cli;
use crate::cli::RunArgs;
use anyhow::Context;
use anyhow::Result;
use proxenos::config::Config;
use std::sync::Arc;

/// Capture exchanges as fixtures.
///
/// Ingress capture runs the daemon normally and records what the client sends
/// before translation. It needs no credentials, because nothing upstream is
/// involved at the point of capture.
pub(crate) async fn record(args: cli::RecordArgs) -> Result<()> {
    match args.mode {
        cli::RecordMode::Ingress { port } => run_with(RunArgs { port }, Capture::Ingress).await,
        cli::RecordMode::Upstream { port } => {
            // Says so before it starts, because the cost is the difference
            // between the two modes and it is not recoverable afterwards.
            tracing::warn!(
                "recording upstream: every turn through this daemon spends quota \
                 and is written to disk with its content"
            );
            run_with(RunArgs { port }, Capture::Upstream).await
        }
        cli::RecordMode::Surface { account, out, only } => {
            surface(&account, out, only.as_deref()).await
        }
    }
}

/// Capture the real Messages surface.
///
/// Out through `Relay`, the same code a relayed turn takes, so what lands in a
/// fixture is what the shipping path would receive rather than what a
/// purpose-built client would.
async fn surface(account: &str, out: Option<std::path::PathBuf>, only: Option<&str>) -> Result<()> {
    let store: Arc<dyn proxenos::auth::store::AccountStore> = Arc::new(account_store()?);

    // Refused here rather than at the endpoint. A credential stored for one
    // provider spent against the other's host is a key leaking somewhere it
    // was never stored for, and the endpoint's own refusal would arrive after
    // it had already been sent.
    let named = store
        .accounts()?
        .into_iter()
        .find(|stored| stored.name == account)
        .with_context(|| format!("no account named `{account}` is stored"))?;
    if named.provider != proxenos::auth::store::Provider::Anthropic.as_str() {
        anyhow::bail!(
            "`{account}` is stored for {}, and this captures the Messages surface of              anthropic; name an anthropic account (`proxenos accounts` lists them)",
            named.provider
        );
    }

    let tokens = Arc::new(proxenos::auth::grants::Grants::new(
        Arc::clone(&store) as Arc<dyn proxenos::auth::store::CredentialStore>,
        Arc::new(proxenos::auth::grants::SystemClock),
    ));
    let authorizer: Arc<dyn proxenos::auth::authorize::Authorizer> = Arc::new(
        proxenos::auth::authorize::AccountAuthorizer::new(Arc::clone(&store), tokens),
    );

    let endpoint = Config::load()?.upstream.anthropic.endpoint;
    let messages = proxenos::upstream::relay::Relay::new(
        endpoint.clone(),
        Arc::clone(&store),
        Arc::clone(&authorizer),
    );
    let sizing = proxenos::upstream::relay::Relay::new(
        format!("{endpoint}/count_tokens"),
        Arc::clone(&store),
        Arc::clone(&authorizer),
    );

    let directory = out.unwrap_or_else(|| std::path::PathBuf::from("fixtures/surface"));

    // Says so before it starts, because the cost is not recoverable afterwards.
    let exchanges = match only {
        Some(_) => 1,
        None => proxenos::surface::PLANS.len(),
    };
    tracing::warn!(
        exchanges,
        account,
        "capturing the Messages surface: one live turn per exchange, spent on this account"
    );

    let written =
        proxenos::surface::capture_some(&messages, &sizing, account, &directory, only).await?;
    for path in &written {
        println!("{}", path.display());
    }
    Ok(())
}
