//! `ordo people`: the deployment's people and machine keys, at the
//! identity service (`plan-identity-service.md` 5.3, 5.6, 5.7).
//!
//! Every call carries the token [`crate::signin::credential`] picks, and
//! goes through [`IdentityClient`]. A change needs a passkey touch in the
//! last 5 minutes. At a terminal the touch runs by itself and the call is
//! sent once more; in a script the refusal says to run `ordo login --touch`.

use serde_json::{json, Value};

use crate::operator::Outcome;
use crate::signin::{
    credential, touch_if_interactive, when, Args, Ctx, IdentityClient, Origin, SignInError,
    IDENTITY_URL_ENV,
};

pub const USAGE: &str = "\
usage:
  ordo people list                      everyone, and whose access has ended
  ordo people invite [--label <text>]   a sign-up link for a new admin, 7 days
  ordo people end <p_id>                end a person's access now
  ordo people machine-key add --name <name> [--public-key <base64url>]
                                        a key for CI; its secret is shown once
  ordo people machine-key list          every machine key
  ordo people machine-key end <m_id>    end a machine key now
  Each takes [--identity <url>]. Default: ORDO_IDENTITY_URL, or the one you are signed in to.
";

pub async fn run(ctx: &Ctx, args: &[String]) -> Outcome {
    let a = match Args::parse(
        args,
        &["--identity", "--label", "--name", "--public-key", "--token"],
        &[],
        USAGE,
    ) {
        Ok(a) => a,
        Err(e) => return Outcome::usage(e),
    };
    match act(ctx, &a).await {
        Err(e) if e.code() == Some("presence_required") => {
            match touch_if_interactive(ctx, a.get("--token"), a.get("--identity")).await {
                Ok(true) => match act(ctx, &a).await {
                    Ok(o) => o,
                    Err(e) => e.outcome(),
                },
                Ok(false) => e.outcome(),
                Err(t) => t.outcome(),
            }
        }
        Ok(o) => o,
        Err(e) => e.outcome(),
    }
}

async fn act(ctx: &Ctx, a: &Args) -> Result<Outcome, SignInError> {
    let p: Vec<&str> = a.positional.iter().map(String::as_str).collect();
    let call = match p.as_slice() {
        ["list"] => Call::List,
        ["invite"] => Call::Invite,
        ["end", id] => Call::End(id.to_string()),
        ["machine-key", "add" | "create"] => Call::KeyAdd,
        ["machine-key", "list"] => Call::KeyList,
        ["machine-key", "end", id] => Call::KeyEnd(id.to_string()),
        _ => return Ok(Outcome::usage(USAGE.into())),
    };
    let cred = credential(ctx, a.get("--token"), a.get("--identity")).await?;
    let origin = cred.origin.clone().ok_or_else(|| SignInError::Setting {
        setting: IDENTITY_URL_ENV,
        problem: "is unset, and a raw token does not say which identity service issued it".into(),
        fix: format!("set {IDENTITY_URL_ENV}, or pass `--identity <url>`"),
    })?;
    let client = IdentityClient::new(Origin::parse(&origin, true)?);
    let t = Some(cred.token.as_str());
    let now = ctx.clock.now();
    let out = match call {
        Call::List => {
            let v = client.get("/people", t).await?;
            let mut out = String::new();
            for p in v["people"].as_array().into_iter().flatten() {
                let ended = match p["endedAt"].as_u64() {
                    Some(_) => "ended",
                    None => "active",
                };
                out.push_str(&format!(
                    "{:<26} {:<7} {:<6} {}\n",
                    s(p, "id"),
                    s(p, "role"),
                    ended,
                    s(p, "displayName")
                ));
            }
            if out.is_empty() {
                out.push_str("No people yet.\n");
            }
            out
        }
        Call::Invite => {
            let label = a.get("--label").unwrap_or("");
            let v = client
                .post("/invites", json!({ "role": "admin", "label": label }), t)
                .await?;
            format!(
                "Invite {} made. Send this link to the new admin only:\n  {}\nIt works once, \
                 and ends {}.\n",
                s(&v, "id"),
                s(&v, "link"),
                v["expiresAt"]
                    .as_u64()
                    .map(|e| when(e, now))
                    .unwrap_or_else(|| "?".into())
            )
        }
        Call::End(id) => {
            let v = client
                .post(&format!("/people/{}/end", path_part(&id)), json!({}), t)
                .await?;
            format!(
                "Ended {id}. {} session(s) ended, {} recovery code(s) and {} invite(s) voided.\n\
                 Tokens they already hold stop working within 10 minutes.\n",
                n(&v, "sessions"),
                n(&v, "recoveryCodes"),
                n(&v, "invites"),
            )
        }
        Call::KeyAdd => {
            let Some(name) = a.get("--name") else {
                return Ok(Outcome::usage(format!(
                    "`machine-key add` needs `--name <name>`\n{USAGE}"
                )));
            };
            let mut body = json!({ "name": name });
            if let Some(k) = a.get("--public-key") {
                body["publicKey"] = json!(k);
            }
            let v = client.post("/machine-keys", body, t).await?;
            match v["secret"].as_str() {
                Some(secret) => format!(
                    "Machine key {} ({}) made.\nIts secret, shown this once. Store it as the CI \
                     secret ORDO_MACHINE_KEY,\nwith ORDO_IDENTITY_URL={origin}:\n  {secret}\n",
                    s(&v, "id"),
                    s(&v, "name"),
                ),
                None => format!(
                    "Machine key {} ({}) registered with your public key.\n",
                    s(&v, "id"),
                    s(&v, "name")
                ),
            }
        }
        Call::KeyList => {
            let v = client.get("/machine-keys", t).await?;
            let mut out = String::new();
            for k in v["machineKeys"].as_array().into_iter().flatten() {
                let state = if k["endedAt"].is_u64() {
                    "ended"
                } else {
                    "active"
                };
                let used = k["lastUsedAt"]
                    .as_u64()
                    .map(|u| format!("used {} seconds ago", now.saturating_sub(u)))
                    .unwrap_or_else(|| "never used".into());
                out.push_str(&format!(
                    "{:<26} {:<6} {:<20} {used}\n",
                    s(k, "id"),
                    state,
                    s(k, "name")
                ));
            }
            if out.is_empty() {
                out.push_str("No machine keys yet.\n");
            }
            out
        }
        Call::KeyEnd(id) => {
            let v = client
                .post(
                    &format!("/machine-keys/{}/end", path_part(&id)),
                    json!({}),
                    t,
                )
                .await?;
            format!(
                "Ended machine key {id}. {} session(s) ended. Its tokens stop working within \
                 10 minutes.\n",
                n(&v, "sessions")
            )
        }
    };
    Ok(Outcome::ok(out))
}

enum Call {
    List,
    Invite,
    End(String),
    KeyAdd,
    KeyList,
    KeyEnd(String),
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("?")
}

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}

/// An id as one path segment: anything but the id's own characters is
/// dropped, so an id can never reach another route.
fn path_part(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}
