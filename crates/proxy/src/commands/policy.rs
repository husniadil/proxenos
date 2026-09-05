//! `tiers` and `effort`: the two settings a running daemon can be handed.
//!
//! Each verb reads bare and sets under `set`. A set goes to the socket method
//! of the same name with exactly what was typed — the daemon validates the
//! model against the catalog, decides where a persisted line is written, and
//! says whether the change is in effect or only on disk. This side prints
//! that answer and invents nothing about it.

use crate::cli;
use anyhow::Result;
use proxenos::control;
use proxenos::render;
use serde_json::Value;
use serde_json::json;

pub(crate) async fn tiers(args: cli::TiersArgs) -> Result<()> {
    let socket = control::default_path();
    let set = match args.action {
        None => {
            let result = control::call(&socket, "tiers", None).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
                return Ok(());
            }
            println!("{}", render::tiers(&result));
            return Ok(());
        }
        Some(cli::TiersAction::CrossAccount(consent)) => {
            let enabled = consent.state == "on";
            let result = control::call(
                &socket,
                "cross_account_tiers.set",
                Some(json!({ "enabled": enabled })),
            )
            .await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
                return Ok(());
            }
            println!("{}", render::cross_account_set(&result));
            return Ok(());
        }
        Some(cli::TiersAction::Set(set)) => set,
    };

    // Consent first, in the same breath, where the pin asks for it. The
    // daemon persists consent always and answers `persisted: false` where it
    // already stood, so granting again is not a second decision.
    let mut consented = None;
    if set.allow_cross_account {
        let answer = control::call(
            &socket,
            "cross_account_tiers.set",
            Some(json!({ "enabled": true })),
        )
        .await?;
        consented = Some(answer);
    }

    // The same two shapes the file takes: a model id, or the pinned table.
    let value = match &set.as_account {
        Some(account) => json!({ "account": account, "model": set.model }),
        None => Value::String(set.model.clone()),
    };
    let params = with_scope(
        json!({ "tiers": { set.tier.clone(): value } }),
        set.account,
        set.persist,
    );
    let result = control::call(&socket, "tiers.set", Some(params)).await?;
    if args.json {
        let mut document = result;
        if let (Some(consent), Some(fields)) = (consented, document.as_object_mut()) {
            fields.insert("consent".to_owned(), consent);
        }
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }
    if let Some(consent) = consented {
        println!("{}", render::cross_account_set(&consent));
    }
    println!("{}", render::tier_set(&set.tier, &result));
    Ok(())
}

pub(crate) async fn effort(args: cli::EffortArgs) -> Result<()> {
    let socket = control::default_path();
    let Some(cli::EffortAction::Set(set)) = args.action else {
        // The ceiling has no read method of its own: `status` carries it,
        // beside the mapping it caps.
        let result = control::call(&socket, "status", None).await?;
        if args.json {
            let ceiling = result.get("effort_ceiling").cloned().unwrap_or(Value::Null);
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({ "effort": ceiling }))?
            );
            return Ok(());
        }
        println!("{}", render::effort(&result));
        return Ok(());
    };

    // `none` is the word for removing a ceiling; the socket spells it null.
    let level = if set.level == "none" {
        Value::Null
    } else {
        Value::String(set.level)
    };
    let params = with_scope(json!({ "effort": level }), set.account, set.persist);
    let result = control::call(&socket, "effort.set", Some(params)).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    println!("{}", render::effort_set(&result));
    Ok(())
}

/// The two flags both setters share, added only where they were given: a
/// caller of this CLI has to be able to trust that a choice left out is a
/// parameter left out, since the daemon reads an absent `persist` as "until
/// it stops" and an absent `account` as the shared table.
fn with_scope(mut params: Value, account: Option<String>, persist: bool) -> Value {
    if let Some(fields) = params.as_object_mut() {
        if let Some(account) = account {
            fields.insert("account".to_owned(), Value::String(account));
        }
        if persist {
            fields.insert("persist".to_owned(), Value::Bool(true));
        }
    }
    params
}
