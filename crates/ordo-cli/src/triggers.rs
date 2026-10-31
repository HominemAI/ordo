//! `ordo triggers` — see the listener cursors a space holds, and retire one.
//!
//! ```text
//! ordo triggers ls          --space <spc> [--host <h> --token <t>] [--db <d>]
//! ordo triggers deregister  --space <spc> --listener <name> [--as <who>] [--force]
//!                           [--host <h> --token <t>] [--db <d>]
//! ```
//!
//! # Why a person runs this
//!
//! Every trigger listener has a cursor row in `$logconsumers`. The cursor
//! pins the command log's pruning floor and is never evicted, because that
//! is what at-least-once delivery means. A deploy that renames or deletes a
//! listener leaves its cursor behind, and the log stops shrinking.
//!
//! The supervisor reports these on `/health/deep` under `orphaned_listeners`
//! and never removes them. During a rolling deploy a replica on the old
//! version may still serve the listener, and a replica on the new version
//! removing its cursor would reset the guarantee for it. So retiring a
//! cursor is a deliberate act by the deployment owner, after the rollout is
//! finished. This command is that act.
//!
//! One case retires itself (audit 2026-10-08 finding 7): a routine kept
//! under its name whose trigger changed. The drain of the version that
//! declares the new trigger starts the new cursor where the old one
//! stopped, and retires the old one through the same path as this command
//! once the new one has passed it. A renamed or deleted routine still
//! needs this command.
//!
//! # What "orphaned" means, and where it is read
//!
//! The same thing the supervisor means: a registered cursor of kind
//! `listener` that the running program does not serve. The compiled program
//! does not yet carry its listeners (checklist 12a), so the database cannot
//! answer that. The running deployment can, and it says so on `/health/deep`.
//! With `--host` and `--token` this command asks it. Without them the answer
//! is `unknown`, and `deregister` refuses unless `--force` is given.
//!
//! # The record
//!
//! One `CustodyAction` in the space's security-event stream, written in the
//! same transaction as the delete, so there is no retirement without a record
//! and no record without a retirement. The action string is
//! `listener_deregistered position=<n> forced=<bool> listener=<name>`, the
//! detail-in-the-action-string form `attach_bucket` uses. The subject is
//! `--as`, or `operator:unattributed`. Attribution, never authentication:
//! this tool holds database credentials, so it says who the caller claims
//! to be.

use std::collections::BTreeSet;

use ordo_commandlog::SpaceId;
use ordo_exec::prune::{self, ConsumerKind};

use crate::operator::{connect, Flags, Outcome};

/// The action name the security stream carries. Alerting keys off it, so it
/// is asserted by a test.
pub const ACTION: &str = ordo_exec::triggers::DEREGISTERED_ACTION;

/// The op name on the record. Dotted rather than the command line's spelling:
/// `ordo_secevent::sanitize_op` admits only `[A-Za-z0-9._-]` and records
/// anything else, spaces included, as `<unprintable>`.
pub const OP: &str = "ordo.triggers.deregister";

const USAGE: &str = "ordo triggers ls --space <spc> [--host <h> --token <t>]\n\
                     ordo triggers deregister --space <spc> --listener <name> [--as <who>] \
                     [--force] [--host <h> --token <t>]\n";

/// What the running deployment says about one listener cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// The deployment reports it as orphaned: registered, and not served.
    Orphaned,
    /// The deployment does not report it, so the running program serves it.
    Live,
    /// Nobody was asked, or the answer could not be used. Carries why.
    Unknown(String),
}

impl Standing {
    fn word(&self) -> &'static str {
        match self {
            Standing::Orphaned => "orphaned",
            Standing::Live => "live",
            Standing::Unknown(_) => "unknown",
        }
    }
}

/// The orphaned names in a `/health/deep` document, for `space`.
///
/// A document for a different space, or one with no supervisor (`null`, the
/// dev embedding), cannot say anything about this space, and both are
/// errors rather than an empty set: an empty set would read as "nothing is
/// orphaned".
pub fn orphans_in(doc: &serde_json::Value, space: &str) -> Result<BTreeSet<String>, String> {
    if let Some(theirs) = doc["space"].as_str() {
        if theirs != space {
            return Err(format!(
                "the deployment answered for space `{theirs}`, not `{space}`"
            ));
        }
    }
    let Some(list) = doc["supervisor"]["orphaned_listeners"].as_array() else {
        return Err("the deployment runs no supervisor, so it reports no census".into());
    };
    Ok(list
        .iter()
        .filter_map(|o| o["consumer"].as_str().map(str::to_string))
        .collect())
}

/// Ask the deployment for its orphan census. `None` when `--host` was not
/// given, `Some(Err)` when it was and the answer was unusable.
async fn census(flags: &Flags, space: &str) -> Option<Result<BTreeSet<String>, String>> {
    let host = match crate::wire::requested(flags)? {
        Ok(h) => h,
        Err(e) => return Some(Err(e)),
    };
    let token = match crate::wire::token(flags).await {
        Ok(t) => t,
        Err(e) => return Some(Err(e.trim().to_string())),
    };
    let (status, doc) = match crate::wire::get_json(&host, "/health/deep", &token).await {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("/health/deep: {e}"))),
    };
    if status != 200 {
        return Some(Err(format!("/health/deep answered {status}")));
    }
    Some(orphans_in(&doc, space))
}

fn standing(
    name: &str,
    kind: ConsumerKind,
    census: &Option<Result<BTreeSet<String>, String>>,
) -> Option<Standing> {
    if kind != ConsumerKind::Listener {
        return None;
    }
    Some(match census {
        None => Standing::Unknown("no --host, so the running program was not asked".into()),
        Some(Err(e)) => Standing::Unknown(e.clone()),
        Some(Ok(set)) if set.contains(name) => Standing::Orphaned,
        Some(Ok(_)) => Standing::Live,
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `ordo triggers <ls | deregister>`. The subcommand is `flags.positional[0]`.
pub async fn run(flags: Flags) -> Outcome {
    let Some(sub) = flags.positional.first().cloned() else {
        return Outcome::usage(USAGE.into());
    };
    let mut flags = flags;
    flags.positional.remove(0);
    match sub.as_str() {
        "ls" => ls(flags).await,
        "deregister" => deregister(flags).await,
        other => Outcome::usage(format!(
            "unknown `ordo triggers` subcommand `{other}`\n{USAGE}"
        )),
    }
}

async fn ls(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&["--space", "--db", "--host", "--token"]) {
        return Outcome::usage(e);
    }
    let space = match flags.require("--space") {
        Ok(s) => s.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    if let Err(e) = prune::ensure_tables(&pool).await {
        return Outcome::failed(format!("consumer table: {e}\n"));
    }
    let all = match prune::consumers(&pool, &SpaceId(space.clone())).await {
        Ok(c) => c,
        Err(e) => return Outcome::failed(format!("space   {space}\ndatabase: {e}\n")),
    };
    let census = census(&flags, &space).await;
    let now = now_secs();
    let mut out = format!("space   {space}\n");
    if all.is_empty() {
        out.push_str("no consumers are registered\n");
        return Outcome::ok(out);
    }
    out.push_str("consumer  kind  position  idle  standing\n");
    for c in &all {
        let stand = standing(&c.name, c.kind, &census);
        out.push_str(&format!(
            "{}  {}  {}  {}s  {}\n",
            c.name,
            c.kind,
            c.position,
            now.saturating_sub(c.advanced_at),
            stand.as_ref().map(Standing::word).unwrap_or("-"),
        ));
    }
    if let Some(Err(e)) = &census {
        out.push_str(&format!("standing is unknown for every listener: {e}\n"));
    } else if census.is_none() {
        out.push_str(
            "standing is unknown for every listener: pass --host and --token to ask the \
             running deployment which listeners it serves\n",
        );
    }
    Outcome::ok(out)
}

async fn deregister(flags: Flags) -> Outcome {
    if let Err(e) = flags.only(&[
        "--space",
        "--listener",
        "--as",
        "--force",
        "--db",
        "--host",
        "--token",
    ]) {
        return Outcome::usage(e);
    }
    let space = match flags.require("--space") {
        Ok(s) => s.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let name = match flags.require("--listener") {
        Ok(s) => s.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let force = flags.has("--force");
    let actor = flags
        .named_value("--as")
        .map(str::to_string)
        .unwrap_or_else(|| "operator:unattributed".to_string());
    let pool = match connect(&flags).await {
        Ok(p) => p,
        Err(o) => return o,
    };
    if let Err(e) = prune::ensure_tables(&pool).await {
        return Outcome::failed(format!("consumer table: {e}\n"));
    }
    let sp = SpaceId(space.clone());
    let all = match prune::consumers(&pool, &sp).await {
        Ok(c) => c,
        Err(e) => return Outcome::failed(format!("space   {space}\ndatabase: {e}\n")),
    };
    let Some(found) = all.iter().find(|c| c.name == name) else {
        return Outcome::failed(format!(
            "space   {space}\nREFUSED no consumer is registered as `{name}`. Nothing was \
             changed. `ordo triggers ls --space {space}` lists the names.\n"
        ));
    };
    if found.kind != ConsumerKind::Listener {
        return Outcome::failed(format!(
            "space   {space}\nREFUSED `{name}` is a {} cursor, not a trigger listener. This \
             command retires listeners only.\n",
            found.kind
        ));
    }
    let census = census(&flags, &space).await;
    let stand = standing(&name, found.kind, &census).expect("a listener has a standing");
    if stand != Standing::Orphaned && !force {
        let why = match &stand {
            Standing::Live => "the running deployment still serves this listener".to_string(),
            Standing::Unknown(why) => format!("its standing is unknown ({why})"),
            Standing::Orphaned => unreachable!(),
        };
        return Outcome::failed(format!(
            "space   {space}\nREFUSED `{name}` is not confirmed orphaned: {why}.\n\
             A listener cursor holds the log floor so that delivery is at-least-once. Removing \
             the cursor of a listener that is still served resets that guarantee for it, and the \
             events it has not yet read may never reach it.\n\
             Nothing was changed. If a rolling deploy is finished and you are sure, run again \
             with --force.\n"
        ));
    }

    // The delete and its record commit together.
    let held = match retire(
        &pool,
        &sp,
        &name,
        &actor,
        force && stand != Standing::Orphaned,
    )
    .await
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Outcome::failed(format!(
                "space   {space}\nREFUSED `{name}` was removed by someone else first. \
                 Nothing was changed by this run.\n"
            ))
        }
        Err(e) => {
            return Outcome::failed(format!(
                "space   {space}\nFAILED  {e}\nNothing was changed and nothing was recorded.\n"
            ))
        }
    };
    Outcome::ok(format!(
        "space     {space}\nlistener  {name}\nreleased  position {held}, which no longer holds \
         the log floor\nstanding  {}{}\nas        {actor}\naudit     recorded in the space's \
         security stream as `{ACTION}`\n",
        stand.word(),
        if force && stand != Standing::Orphaned {
            " (forced)"
        } else {
            ""
        },
    ))
}

/// Delete the cursor and append the record in one transaction. Returns the
/// position the cursor held, or `None` when there was no such row.
async fn retire(
    pool: &ordo_exec::Pool,
    space: &SpaceId,
    name: &str,
    actor: &str,
    forced: bool,
) -> Result<Option<u64>, String> {
    ordo_exec::triggers::retire_listener(pool, space, name, actor, OP, forced, now_secs()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_census_is_read_from_health_deep() {
        let doc = json!({
            "space": "spc_a",
            "supervisor": { "orphaned_listeners": [
                { "consumer": "digest:*.*.created", "position": 4 }
            ]}
        });
        let set = orphans_in(&doc, "spc_a").unwrap();
        assert!(set.contains("digest:*.*.created"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn a_census_that_cannot_speak_for_the_space_is_an_error_not_an_empty_set() {
        let other = json!({ "space": "spc_b", "supervisor": { "orphaned_listeners": [] } });
        assert!(orphans_in(&other, "spc_a").is_err());
        let none = json!({ "space": "spc_a", "supervisor": null });
        assert!(orphans_in(&none, "spc_a").is_err());
    }
}
