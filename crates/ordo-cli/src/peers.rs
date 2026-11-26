//! `ordo peers key|rotate-key`: this deployment's consumer key, the one it
//! signs with when it calls another deployment's door
//! (`plan-federation-grants.md` G9).
//!
//! `ordo apply` registers the key by itself at the identity service, the
//! first time a file has a `peer` block ([`ensure_registered`]). The owner's
//! token the command already holds does it, so no step is left for a
//! person. A later apply, and every plan, only asks whether it is
//! registered.
//!
//! `ordo peers rotate-key [--overlap-secs <n>]` moves to the next key
//! version and registers it. The key it replaces stays accepted at the
//! identity service for the overlap (`ORDO_IDENTITY_CONSUMER_KEY_OVERLAP_SECS`,
//! 3600 by default; `--overlap-secs` asks for less), so no node of this
//! deployment is cut off while it takes the new key up. Rotating the
//! blind-index key rotates the consumer key too: run `ordo apply` or
//! `ordo peers rotate-key` afterwards, and the new key is registered.
//!
//! Both go over the door (`--host`), as the owner.

use ordo_host::peer_ops;
use serde_json::{json, Value};

use crate::operator::{Flags, Outcome};
use crate::wire;

const USAGE: &str = "ordo peers key\n\
     ordo peers rotate-key [--overlap-secs <n>]\n\
     each takes --host <h> [--token <t> --space <s>]";

pub async fn run(flags: Flags) -> Outcome {
    let mut flags = flags;
    let Some(sub) = flags.positional.first().cloned() else {
        return Outcome::usage(format!("{USAGE}\n"));
    };
    flags.positional.remove(0);
    if let Err(e) = flags.only(&["--host", "--token", "--space", "--overlap-secs"]) {
        return Outcome::usage(format!("{e}\n{USAGE}\n"));
    }
    match sub.as_str() {
        "key" => status(&flags).await,
        "rotate-key" => rotate(&flags).await,
        other => Outcome::usage(format!(
            "unknown `ordo peers` subcommand `{other}`; there are `key` and `rotate-key`\n\
             {USAGE}\n"
        )),
    }
}

fn host(flags: &Flags) -> Result<wire::Host, Outcome> {
    match wire::requested(flags) {
        Some(Ok(h)) => Ok(h),
        Some(Err(e)) => Err(Outcome::usage(e)),
        None => Err(Outcome::usage(format!(
            "`ordo peers` talks to the deployment's door: `--host <h>` or {} is required\n",
            wire::HOST_ENV
        ))),
    }
}

async fn status(flags: &Flags) -> Outcome {
    let host = match host(flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    match wire::door_op(&host, flags, peer_ops::KEY_STATUS, json!({}), false).await {
        Ok(row) => Outcome::ok(render("consumer key", &row)),
        Err(o) => o,
    }
}

async fn rotate(flags: &Flags) -> Outcome {
    let host = match host(flags) {
        Ok(h) => h,
        Err(o) => return o,
    };
    let overlap = match flags.named_value("--overlap-secs") {
        None => None,
        Some(v) => match v.parse::<u64>() {
            Ok(n) => Some(n),
            Err(_) => {
                return Outcome::usage(
                    "`--overlap-secs` is a whole number of seconds: how long the key this \
                     replaces stays accepted. Leave it out for the identity service's \
                     default\n"
                        .into(),
                )
            }
        },
    };
    let token = match wire::token(flags).await {
        Ok(t) => t,
        Err(e) => return Outcome::usage(e),
    };
    let mut params = json!({ "token": token });
    if let Some(o) = overlap {
        params["overlapSecs"] = json!(o);
    }
    match wire::door_op(&host, flags, peer_ops::ROTATE_KEY, params, true).await {
        Ok(row) => {
            let mut out = render("rotated", &row);
            out.push_str(&format!(
                "old key     {} stays accepted for the overlap, then is refused\n",
                short(row["previousConsumerKey"].as_str().unwrap_or("?"))
            ));
            Outcome::ok(out)
        }
        Err(o) => o,
    }
}

fn short(key: &str) -> &str {
    &key[..key.len().min(8)]
}

fn render(what: &str, row: &Value) -> String {
    format!(
        "{what}\n  handle      {}\n  version     v{}\n  public key  {}\n  registered  {}\n",
        row["handle"].as_str().unwrap_or("?"),
        row["version"].as_u64().unwrap_or(0),
        row["consumerKey"].as_str().unwrap_or("?"),
        if row["registered"].as_bool() == Some(true) {
            "yes"
        } else {
            "no"
        }
    )
}

/// Register the consumer key at the identity service when the file has a
/// `peer` block and the key is not registered, with the token this command
/// already holds. Returns the line to print, or the refusal. Nothing
/// happens when it is registered, so a second apply does nothing.
pub(crate) async fn ensure_registered(
    host: &wire::Host,
    flags: &Flags,
) -> Result<Option<String>, Outcome> {
    let row = wire::door_op(host, flags, peer_ops::KEY_STATUS, json!({}), false).await?;
    if row["registered"].as_bool() == Some(true) {
        return Ok(None);
    }
    let token = wire::token(flags).await.map_err(Outcome::usage)?;
    let row = wire::door_op(
        host,
        flags,
        peer_ops::REGISTER_KEY,
        json!({ "token": token }),
        true,
    )
    .await?;
    Ok(Some(format!(
        "peer key    registered consumer key v{} ({}) at the identity service\n",
        row["version"].as_u64().unwrap_or(0),
        short(row["consumerKey"].as_str().unwrap_or("?"))
    )))
}
