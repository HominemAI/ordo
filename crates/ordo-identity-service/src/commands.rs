//! The host commands (`plan-identity-service.md` 4.2, 5.2, 5.8, 7), run
//! inside the container with the service's own settings:
//!
//! | Command | What |
//! |---|---|
//! | `invite [--label <text>] [--role admin]` | a new admin's invite link; also recovery when every passkey is lost |
//! | `break-glass-token [--minutes <1..60>]` | one admin token for `ordo --token`, default 15 minutes |
//! | `rotate-keys [--now]` | a new token key; `--now` signs with it at once and retires the old |
//! | `audit verify [--key <kid>:<x>]...` | check the audit chain and its checkpoints |
//!
//! Each is recorded in the audit log under `unix:<user>`. None adds
//! power: whoever can run them already holds the host.

use ordo_commandlog::CheckpointVerifier;
use ordo_identity::webauthn::b64url_decode;

use crate::audit;
use crate::people::Actor;
use crate::server::Service;

/// The first words that name a command, so `main` knows to run one.
pub const COMMANDS: &[&str] = &["invite", "break-glass-token", "rotate-keys", "audit"];

const USAGE: &str = "usage: ordo-identity-service [invite [--label <text>] [--role admin] \
                     | break-glass-token [--minutes <1..60>] | rotate-keys [--now] \
                     | audit verify [--key <kid>:<x>]...]";

/// The value after `--name`, if given.
fn flag<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>, String> {
    match args.iter().position(|a| a == name) {
        None => Ok(None),
        Some(i) => args
            .get(i + 1)
            .map(|v| Some(v.as_str()))
            .ok_or_else(|| format!("{name} needs a value. {USAGE}")),
    }
}

/// Run one command. Returns the exit code and what to print: on stdout
/// for 0, on stderr otherwise.
pub async fn run(svc: &Service, args: &[String], unix_user: &str) -> (i32, String) {
    match run_inner(svc, args, unix_user).await {
        Ok(out) => (0, out),
        Err(e) => (1, format!("ordo-identity-service: {e}\n")),
    }
}

async fn run_inner(svc: &Service, args: &[String], unix_user: &str) -> Result<String, String> {
    let actor_id = format!("unix:{unix_user}");
    let actor = Actor {
        id: &actor_id,
        client: "host",
    };
    match args.first().map(String::as_str) {
        Some("invite") => {
            if let Some(role) = flag(args, "--role")? {
                if role != "admin" {
                    return Err("--role is `admin`, the only role for now".into());
                }
            }
            let label = flag(args, "--label")?.unwrap_or("");
            let m = svc.invite(actor, label).await.map_err(|e| e.to_string())?;
            Ok(format!(
                "{}\nOpen it within {} days to sign up; it works once.\n",
                m.link,
                svc.config().invite_days
            ))
        }
        Some("break-glass-token") => {
            let minutes = match flag(args, "--minutes")? {
                None => 15,
                Some(m) => m
                    .parse::<u64>()
                    .map_err(|_| format!("--minutes is a whole number. {USAGE}"))?,
            };
            let issued = svc
                .break_glass(unix_user, minutes)
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!("{}\n", issued.token))
        }
        Some("rotate-keys") => {
            let now_flag = args.iter().any(|a| a == "--now");
            let r = svc
                .keys()
                .rotate(svc.store().pool(), svc.now(), now_flag, &actor_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(if now_flag {
                format!(
                    "New token key {} signs now. Retired: {}. Every older token dies within {} \
                     seconds.\n",
                    r.kid,
                    if r.retired.is_empty() {
                        "none".to_string()
                    } else {
                        r.retired.join(", ")
                    },
                    svc.config().token_secs
                )
            } else {
                format!(
                    "New token key {} is published and signs from {} (unix seconds). The old \
                     key retires a day after its last token expires.\n",
                    r.kid, r.active_from
                )
            })
        }
        Some("audit") if args.get(1).map(String::as_str) == Some("verify") => {
            let mut given = Vec::new();
            let mut rest = args[2..].iter();
            while let Some(a) = rest.next() {
                if a != "--key" {
                    return Err(format!("unknown argument `{a}`. {USAGE}"));
                }
                let v = rest
                    .next()
                    .ok_or_else(|| format!("--key needs <kid>:<x>. {USAGE}"))?;
                let (kid, x) = v
                    .split_once(':')
                    .ok_or_else(|| "--key is <kid>:<x>, as the audit keys list them".to_string())?;
                let public = b64url_decode(x)
                    .ok()
                    .filter(|b| b.len() == 32)
                    .ok_or_else(|| "--key's x is 32 bytes, base64url".to_string())?;
                given.push(
                    CheckpointVerifier::from_public_hex(kid, &ordo_identity::sealing::hex(&public))
                        .map_err(|e| e.to_string())?,
                );
            }
            let pool = svc.store().pool();
            let verifiers = if given.is_empty() {
                audit::verifiers(pool, svc.handle())
                    .await
                    .map_err(|e| e.to_string())?
            } else {
                given
            };
            let checked = audit::verify(pool, svc.handle(), &verifiers)
                .await
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "The audit log checks out: {} rows, {} signed checkpoints.\n",
                checked.rows, checked.checkpoints
            ))
        }
        _ => Err(USAGE.to_string()),
    }
}

/// The unix user this process runs as, by name when `/etc/passwd` has
/// it, else by number.
#[cfg(unix)]
pub fn unix_user() -> String {
    let uid = rustix::process::getuid().as_raw();
    let named = std::fs::read_to_string("/etc/passwd").ok().and_then(|p| {
        p.lines().find_map(|l| {
            let mut f = l.split(':');
            let name = f.next()?;
            let id = f.nth(1)?;
            (id.parse::<u32>().ok()? == uid).then(|| name.to_string())
        })
    });
    named.unwrap_or_else(|| uid.to_string())
}

#[cfg(not(unix))]
pub fn unix_user() -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "unknown".into())
}
