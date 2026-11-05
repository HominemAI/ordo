//! Which way a command reaches the deployment (owner, 2026-11-05: "the CLI
//! goes through the door").
//!
//! Every command that changes or reads a deployment goes through its door
//! (`--host`, or `ORDO_HOST`). The database is opened from this machine only
//! by a command that must work when no server can be up, and only when
//! `--direct` is typed. Each such use is written to `$direct_uses` before
//! the command acts, and the next boot carries it to the security log
//! (`ordo_host::direct_uses`).
//!
//! The policy is one table, [`policy`], so a reader sees every command's
//! route in one place and a refusal always names the flag to use.
//!
//! | Route | Commands | Without a flag |
//! |---|---|---|
//! | [`Route::Door`] | `versions`, `windows`, `fleet`, `quota`, `touches`, `plan` | refused; names `--host` |
//! | [`Route::Either`] | `apply`, `rollback` | refused; names `--host` and `--direct` |
//! | [`Route::Direct`] | `keys`, `verify`, `restore`, `identity recover`, `migrate --apply` | refused; names `--direct` and `--host` |
//! | [`Route::Open`] | the rest | unchanged (session 2 moves them) |

use crate::operator::{Flags, Outcome};

/// The flag that allows a direct database path.
pub const FLAG: &str = "--direct";

/// How one invocation is allowed to reach the deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The door only. There is no direct path.
    Door,
    /// The door, or the database behind `--direct` when no host can boot.
    Either,
    /// The database behind `--direct`. The door has no action for it yet.
    Direct,
    /// Not decided by this ruling; the command keeps its own behaviour.
    Open,
}

/// The route for `cmd` and its first positional argument (`sub`).
pub fn policy(cmd: &str, sub: Option<&str>, flags: &Flags) -> Route {
    match cmd {
        // No subcommand prints the usage and touches nothing.
        "versions" | "windows" | "quota" | "keys" | "identity" if sub.is_none() => Route::Open,
        "versions" | "windows" | "fleet" | "quota" | "touches" => Route::Door,
        // `plan <dir>` alone only compiles. With `--against` or `--host` it
        // is the deployment's plan, which is the door's.
        "plan" => Route::Door,
        "apply" | "rollback" => Route::Either,
        "migrate" => Route::Direct,
        "restore" => Route::Direct,
        // `verify --proof <file>` reads a file and no database.
        "verify" if flags.named_value("--proof").is_some() => Route::Open,
        "verify" => Route::Direct,
        "identity" => Route::Direct,
        // `seal-master` is a file and a key derivation.
        "keys" if sub == Some("seal-master") => Route::Open,
        "keys" => Route::Direct,
        _ => Route::Open,
    }
}

/// What the gate decided.
pub enum Via {
    /// Go through the door.
    Door,
    /// Open the database; the use is already recorded.
    Direct,
    /// Nothing to decide.
    Open,
}

fn display(cmd: &str, sub: Option<&str>) -> String {
    match sub {
        Some(s) if matches!(cmd, "versions" | "windows" | "quota" | "keys" | "identity") => {
            format!("ordo {cmd} {s}")
        }
        _ => format!("ordo {cmd}"),
    }
}

/// Decide the route and strip `--direct` from `flags` so the command's own
/// flag check never sees it. Writes the record for a direct use.
pub fn gate(cmd: &str, flags: &mut Flags) -> Result<Via, Outcome> {
    let sub = flags.positional.first().cloned();
    let route = policy(cmd, sub.as_deref(), flags);
    let direct = flags.has(FLAG);
    flags.switches.retain(|s| s != FLAG);
    if route == Route::Open {
        if direct {
            return Err(Outcome::usage(format!(
                "`{FLAG}` is not offered for `{}`: it has no direct path to allow\n",
                display(cmd, sub.as_deref())
            )));
        }
        return Ok(Via::Open);
    }
    let what = display(cmd, sub.as_deref());
    let host = crate::wire::requested(flags).is_some();
    let named_host = flags.named_value("--host").is_some();
    if direct && named_host {
        return Err(Outcome::usage(format!(
            "`{what}` was given both `{FLAG}` and `--host`: name one. `--host <url>` goes \
             through the door; `{FLAG}` opens the database from this machine\n"
        )));
    }
    match route {
        Route::Door => {
            if direct {
                return Err(Outcome::usage(format!(
                    "`{what}` goes through the door and has no direct path: drop `{FLAG}` and \
                     name the deployment with `--host <url>` (or ORDO_HOST)\n"
                )));
            }
            if flags.named_value("--db").is_some() || flags.named_value("--against").is_some() {
                return Err(Outcome::usage(format!(
                    "`{what}` no longer opens a database, so `--db` and `--against` do not \
                     apply: name the deployment with `--host <url>` (or ORDO_HOST)\n"
                )));
            }
            if !host {
                return Err(Outcome::usage(format!(
                    "`{what}` goes through the door: name the deployment with `--host <url>` \
                     (or ORDO_HOST), and `--token` or a signed-in session (`ordo login`)\n"
                )));
            }
            Ok(Via::Door)
        }
        Route::Either => {
            if direct {
                record(&what)?;
                return Ok(Via::Direct);
            }
            if host {
                return Ok(Via::Door);
            }
            Err(Outcome::usage(format!(
                "`{what}` needs a way to reach the deployment. Name its door with \
                 `--host <url>` (or ORDO_HOST), or pass `{FLAG}` to open the database from \
                 this machine when no host can boot. `{FLAG}` is recorded on the security \
                 log at the next boot\n"
            )))
        }
        Route::Direct => {
            if direct {
                record(&what)?;
                return Ok(Via::Direct);
            }
            Err(Outcome::usage(format!(
                "`{what}` opens the database from this machine, which must be named: pass \
                 `{FLAG}` (recorded on the security log at the next boot). `--host <url>` \
                 does not apply: the door has no action for this command\n"
            )))
        }
        Route::Open => unreachable!("handled above"),
    }
}

/// Write the use to `$direct_uses` in the control database
/// (`PoolConfig::from_env`, not `--db`: the boot reads that one).
///
/// Sync, with a runtime of its own, because `keys` and `identity` run on
/// stores that build theirs. A use that cannot be recorded is refused:
/// an unrecorded direct use is what the flag exists to prevent.
fn record(what: &str) -> Result<(), Outcome> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Outcome::failed(format!("runtime: {e}\n")))?;
    let by = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".into());
    let machine = std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut cfg = ordo_exec::PoolConfig::from_env();
    cfg.size = 1;
    rt.block_on(async {
        let pool = ordo_exec::Pool::connect(cfg).await.map_err(|e| {
            Outcome::failed(format!(
                "REFUSED  `{FLAG}` could not be recorded: the database did not answer ({e}). \
                 Check ORDO_PG_HOST, ORDO_PG_PORT, ORDO_PG_USER and ORDO_PG_DB\n"
            ))
        })?;
        ordo_host::direct_uses::record(&pool, what, &by, &machine, at)
            .await
            .map_err(|e| {
                Outcome::failed(format!(
                    "REFUSED  `{FLAG}` could not be recorded ({e}); nothing was run\n"
                ))
            })
    })
}
