//! `doctor` — probe the capabilities.

use super::account_store;
use super::serving_account;
use crate::cli;
use anyhow::Result;
use anyhow::bail;
use proxenos::config::Config;
use proxenos::ingress::ModelMapping;
use proxenos::upstream::http::HttpTransport;
use std::sync::Arc;

/// The transport a live probe run answers through: HTTP, with the stored
/// credentials.
///
/// HTTP rather than the conduit. A probe is one turn with no continuation, so
/// nothing here exercises the incremental path, and the WebSocket transport's
/// value is entirely in that path. The simpler transport is also the one whose
/// failures are legible when a probe does fail.
async fn live_transport() -> Result<Arc<dyn proxenos::upstream::Transport>> {
    let credentials: Arc<dyn proxenos::auth::store::AccountStore> = Arc::new(account_store()?);
    let tokens = Arc::new(proxenos::auth::grants::Grants::new(
        Arc::clone(&credentials) as Arc<dyn proxenos::auth::store::CredentialStore>,
        Arc::new(proxenos::auth::grants::SystemClock),
    ));
    let authorizer: Arc<dyn proxenos::auth::authorize::Authorizer> =
        Arc::new(proxenos::auth::authorize::AccountAuthorizer::new(
            Arc::clone(&credentials),
            Arc::clone(&tokens),
        ));

    // Resolved before anything is probed. A run that cannot authenticate used
    // to answer with the whole matrix, every row failed and the header saying
    // the backend answered and was billed — when nothing had been sent at all.
    // A capability reported as broken because the credential is missing sends
    // whoever reads it somewhere else entirely.
    let authorization = authorizer.authorize(None).await?;

    // The operator's endpoint, not a compiled-in one: a probe run that contacts
    // somewhere other than the daemon would answer about the wrong backend. Of
    // the kind the account holds, because the two are not interchangeable
    // (`proxy-behavior.md` §8.2).
    let config = Config::load()?;
    let endpoint = match authorization.kind {
        proxenos::auth::authorize::Kind::Key => config.upstream.key.endpoint,
        proxenos::auth::authorize::Kind::Subscription => config.upstream.endpoint,
    };

    Ok(Arc::new(
        HttpTransport::new(endpoint)
            .for_endpoint(authorization.kind)
            .with_credentials(authorizer),
    ))
}

/// The mapping a live probe run uses: the operator's own.
///
/// The corpus asks for tier ids, and mapping them through the configuration is
/// the point — `web-fetch` asks on the haiku id specifically, so a live run
/// answers whether the tier the client's secondary conversations land on is
/// mapped to something that works.
fn live_models() -> Result<Arc<Vec<ModelMapping>>> {
    let config = Config::load()?;
    let tiers = config.tiers.resolve(config.cross_account_policy())?;
    let by_tier = |name: &str| {
        tiers
            .iter()
            .find(|tier| tier.tier == name)
            .map(|tier| tier.model.clone())
    };

    let mut models: Vec<ModelMapping> = tiers
        .iter()
        .map(|tier| ModelMapping {
            requested: tier.tier.to_owned(),
            upstream: tier.model.clone(),
            account: None,
        })
        .collect();

    // The corpus names concrete model ids rather than tier words, because that
    // is what the client sends.
    for (requested, tier) in [
        ("claude-sonnet-5", "sonnet"),
        ("claude-haiku-4-5-20251001", "haiku"),
    ] {
        if let Some(upstream) = by_tier(tier) {
            models.push(ModelMapping {
                requested: requested.to_owned(),
                upstream,
                account: None,
            });
        }
    }

    Ok(Arc::new(models))
}

/// The authorizer a relayed probe turn is signed with.
///
/// The same one the daemon relays with, so the probe measures the shipping
/// path. It resolves an account by the name it is given and refuses by that
/// name, which is why naming one here cannot disturb the account serving turns.
fn relay_authorizer(
    store: &Arc<dyn proxenos::auth::store::AccountStore>,
) -> Arc<dyn proxenos::auth::authorize::Authorizer> {
    Arc::new(proxenos::auth::authorize::AccountAuthorizer::new(
        Arc::clone(store),
        Arc::new(proxenos::auth::grants::Grants::new(
            Arc::clone(store) as Arc<dyn proxenos::auth::store::CredentialStore>,
            Arc::new(proxenos::auth::grants::SystemClock),
        )),
    ))
}

/// Probe the capabilities, and say what the answer rests on.
pub(crate) async fn doctor(args: cli::DoctorArgs) -> Result<()> {
    let fixtures = proxenos::doctor::Corpus::resolve(args.fixtures);

    let (outcomes, evidence) = if args.live {
        // Read once, and read by name from here on. Choosing which account a
        // relayed turn is authorized as is a decision about whose quota is
        // spent, so it is resolved here rather than inside the probe.
        let store: Arc<dyn proxenos::auth::store::AccountStore> = Arc::new(account_store()?);
        let accounts = store.accounts().unwrap_or_default();
        let relay = proxenos::doctor::relay_account(&accounts, args.relay_account.as_deref()).map(
            |account| proxenos::doctor::LiveRelay {
                endpoint: Config::load()
                    .map(|config| config.upstream.anthropic.endpoint)
                    .unwrap_or_else(|_| proxenos::config::AnthropicEndpoints::default().endpoint),
                store: Arc::clone(&store),
                authorizer: relay_authorizer(&store),
                account,
            },
        );
        let relayed_as = relay.as_ref().ok().map(|relay| relay.account.clone());

        (
            proxenos::doctor::run_live(
                &fixtures,
                args.probe.as_deref(),
                live_transport().await?,
                live_models()?,
                Config::load()?.effort_ceiling()?,
                relay,
            )
            .await?,
            // Named, because the coverage line has to say whose quota this
            // spent. A run reported without it reads as a statement about the
            // proxy when it is a statement about one subscription.
            proxenos::probe::Evidence::Live {
                // Absent rather than guessed where the store cannot be read:
                // a coverage line naming the wrong account is worse than one
                // naming none.
                account: serving_account(&store),
                // The §9 arm spends a different account by construction, so it
                // is named separately or not at all.
                relay: relayed_as,
            },
        )
    } else {
        (
            proxenos::doctor::run(&fixtures, args.probe.as_deref()).await?,
            // Which corpus answered is part of what the run establishes: a
            // directory can hold a recording made minutes ago, the embedded
            // copy is whatever this binary was built from.
            proxenos::probe::Evidence::Replay {
                corpus: fixtures.describe(),
            },
        )
    };

    println!(
        "{}",
        proxenos::probe::matrix(&outcomes, &proxenos::probe::Run { evidence })
    );

    let failed = outcomes
        .iter()
        .filter(|outcome| matches!(outcome.status, proxenos::probe::Status::Failed(_)))
        .count();
    if failed > 0 {
        bail!("{failed} probe(s) failed");
    }
    Ok(())
}
