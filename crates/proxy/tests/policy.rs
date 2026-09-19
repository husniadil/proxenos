//! `docs/api.md` §3 — the policy a running daemon routes turns from.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use pretty_assertions::assert_eq;
use proxenos::config::ResolvedTier;
use proxenos::policy::Policy;
use proxenos::policy::Snapshot;
use proxenos_core::responses::Effort;
use std::sync::Arc;

fn tier(name: &'static str, model: &str) -> ResolvedTier {
    ResolvedTier {
        effort: None,
        defaulted: false,
        missing: None,
        account: None,
        tier: name,
        model: model.to_owned(),
    }
}

/// The routing table is derived from the tiers, never stored beside them.
///
/// `status` reports the tiers and the ingress routes on the models. If those
/// two can be set independently, a daemon can report one mapping and serve
/// another — working turns against the wrong model, which is the failure this
/// project refuses everywhere else.
#[test]
fn the_routing_table_always_matches_the_tiers() {
    let policy = Policy::new(Snapshot::new(
        vec![tier("opus", "a")],
        None,
        proxenos::config::CrossAccountTiers::Refused,
    ));

    policy.set_tiers(vec![tier("opus", "b"), tier("sonnet", "c")]);

    let snapshot = policy.get();
    let routed: Vec<(&str, &str)> = snapshot
        .models()
        .iter()
        .map(|mapping| (mapping.requested.as_str(), mapping.upstream.as_str()))
        .collect();
    assert_eq!(routed, vec![("opus", "b"), ("sonnet", "c")]);
}

/// Setting one leaves the other alone — under real contention.
///
/// Each setter has to carry across the field it is not changing. Read through
/// one call and written through another, a mapping write that read the ceiling
/// *before* another caller changed it puts the old value back, and the change
/// is gone for good. The control socket spawns a task per connection, so two
/// front-ends really can arrive at once.
///
/// The detector is a ceiling that changes exactly ONCE while a mapping thread
/// spins: a stale carry-across reverts it to `None` permanently, so the single
/// check after both threads join is unambiguous. An earlier version sampled the
/// ceiling inside the loop and counted every `None` as a revert — including the
/// ones before the other thread had set it at all, which made it fail under
/// suite load for a reason that had nothing to do with the race.
#[test]
fn a_concurrent_set_never_reverts_the_field_it_did_not_touch() {
    for attempt in 0..40 {
        let policy = Arc::new(Policy::new(Snapshot::new(
            vec![tier("opus", "a")],
            None,
            proxenos::config::CrossAccountTiers::Refused,
        )));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let mapping = {
            let policy = Arc::clone(&policy);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    policy.set_tiers(vec![tier("opus", "a")]);
                }
            })
        };

        // Long enough for the mapping thread to be mid-flight, short enough
        // that forty attempts stay quick.
        std::thread::sleep(std::time::Duration::from_micros(200 + attempt * 20));
        policy.set_effort_ceiling(Some(Effort::High));

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        mapping.join().unwrap();

        assert_eq!(
            policy.get().effort_ceiling(),
            Some(Effort::High),
            "attempt {attempt}: a mapping write carried a stale ceiling over the change"
        );
    }
}

/// §7.1 — the routing table carries the pinned account beside the ids.
///
/// A turn resolves its model against this table and nothing else, so an
/// account named by a tier entry has to arrive with the pair. Dropped here, a
/// pinned tier reaches the transport indistinguishable from an unpinned one
/// and is served as the wrong account, spending quota nobody asked it to
/// spend.
#[test]
fn the_routing_table_carries_the_pinned_account() {
    let pinned = ResolvedTier {
        effort: None,
        defaulted: false,
        missing: None,
        account: Some("spare".to_owned()),
        tier: "haiku",
        model: "cheap".to_owned(),
    };
    let policy = Policy::new(Snapshot::new(
        vec![tier("opus", "a"), pinned],
        None,
        proxenos::config::CrossAccountTiers::Permitted,
    ));

    let snapshot = policy.get();
    let routed: Vec<(&str, Option<&str>)> = snapshot
        .models()
        .iter()
        .map(|mapping| (mapping.requested.as_str(), mapping.account.as_deref()))
        .collect();
    assert_eq!(routed, vec![("opus", None), ("haiku", Some("spare"))]);
}

/// api.md §2.3 — a turn tagged with another account is translated on the
/// mapping `env --account` prints, which is the configuration on disk. The one
/// the daemon started with goes stale at the first persisted `tiers set
/// --account` or reload, and a pane launched after it would run on ids its own
/// environment does not name.
#[test]
fn a_tagged_turn_reads_the_mapping_on_disk() {
    use proxenos::auth::store::AccountStore;

    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(proxenos::auth::store::FileStore::new(
        dir.path().join("credentials.json"),
    ));
    let grant = |account_id: &str| proxenos::auth::store::Credentials {
        access_token: "token".to_owned(),
        refresh_token: "refresh".to_owned(),
        id_token: None,
        account_id: Some(account_id.to_owned()),
        expires_at: Some(u64::MAX / 2),
    };
    store.add(&grant("acct_serving"), None).unwrap();
    store.add(&grant("acct_spare"), Some("spare")).unwrap();
    store.select("acct_serving").unwrap();

    let config_file = dir.path().join("config.toml");
    let started: proxenos::config::Config =
        toml::from_str("[accounts.spare.tiers]\nsonnet = \"gpt-5.6-terra\"\n").unwrap();
    std::fs::write(
        &config_file,
        "[accounts.spare.tiers]\nsonnet = \"gpt-5.4-mini\"\n",
    )
    .unwrap();

    let policy = Policy::new(Snapshot::new(
        vec![tier("sonnet", "gpt-5.6-terra")],
        None,
        proxenos::config::CrossAccountTiers::Refused,
    ))
    .resolving_accounts_from(
        Arc::new(started),
        Arc::clone(&store) as Arc<dyn AccountStore>,
    )
    .reading_configuration_at(config_file);

    let tagged = policy.snapshot_for(&policy.get(), Some("spare")).unwrap();
    let sonnet = tagged
        .tiers()
        .iter()
        .find(|tier| tier.tier == "sonnet")
        .unwrap();
    assert_eq!(sonnet.model, "gpt-5.4-mini");
}
