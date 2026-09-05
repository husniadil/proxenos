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
    let Some(cli::TiersAction::Set(set)) = args.action else {
        let result = control::call(&socket, "tiers", None).await?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
        println!("{}", render::tiers(&result));
        return Ok(());
    };

    let params = with_scope(
        json!({ "tiers": { set.tier.clone(): set.model } }),
        set.account,
        set.persist,
    );
    let result = control::call(&socket, "tiers.set", Some(params)).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
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
