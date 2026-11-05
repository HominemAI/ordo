//! `ordo ... --direct`: the record of a command that opened the database
//! itself (owner, 2026-11-05: "the CLI goes through the door").
//!
//! The CLI reaches a deployment through the door. A few commands must work
//! when no server can be up (the key ceremonies, `verify`, `restore`,
//! `identity recover`, the first `apply`, a `rollback` when no host boots),
//! and those open the database directly, only when `--direct` is typed.
//!
//! **Where the record goes, and why a table.** The security stream is a
//! signed chain with one writer, the running host. A second writer from the
//! CLI would fork it, and when the CLI runs there may be no host. So the
//! CLI writes a row to `$direct_uses` in the control database, and the next
//! boot ([`drain`], called from `runtime`) turns each row into a `deploy`
//! event with the act `direct-cli` on the security stream. A row is deleted
//! only once its event is in the stored stream ([`confirm`]): the recorder
//! is an outbox flushed later, so a node that died before the flush would
//! otherwise have lost the record (security audit 2026-11-05, L6).
//! A row is written before the command acts, so a command that fails midway
//! is still on record. A row that cannot be written refuses the command.

use ordo_exec::{sql_lit, Pool, PoolError};

/// The act word the security stream carries.
pub const ACT: &str = "direct-cli";

/// Make the table. The CLI calls it before writing, and boot before reading.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    const DDL: &str = "CREATE TABLE IF NOT EXISTS \"$direct_uses\" (\
           \"id\" bigserial PRIMARY KEY, \
           \"at\" bigint NOT NULL, \
           \"command\" text NOT NULL, \
           \"by\" text NOT NULL, \
           \"machine\" text NOT NULL)";
    // Two callers creating it at the same moment can collide inside
    // Postgres even with `IF NOT EXISTS`; the second try finds it.
    match pool.rows(DDL).await {
        Ok(_) => Ok(()),
        Err(_) => pool.rows(DDL).await.map(|_| ()),
    }
}

/// A word safe for a `key=value` line: printable, no spaces, no `=`.
fn clean(v: &str) -> String {
    let v: String = v
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '=' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    if v.is_empty() {
        "unknown".to_string()
    } else {
        v
    }
}

/// Write one use: which command, which unix user, which machine.
pub async fn record(
    pool: &Pool,
    command: &str,
    by: &str,
    machine: &str,
    at: u64,
) -> Result<(), PoolError> {
    ensure_table(pool).await?;
    pool.rows(&format!(
        "INSERT INTO \"$direct_uses\" (\"at\", \"command\", \"by\", \"machine\") \
         VALUES ({at}, {}, {}, {})",
        sql_lit(&clean(command)),
        sql_lit(&clean(by)),
        sql_lit(&clean(machine)),
    ))
    .await
    .map(|_| ())
}

/// One use, as the boot reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    pub id: i64,
    pub at: u64,
    pub command: String,
    pub by: String,
    pub machine: String,
}

impl Use {
    /// The one line the security stream carries. `use=` is the row's id,
    /// which [`confirm`] reads back.
    pub fn detail(&self) -> String {
        format!(
            "command={} by={} machine={} at={} use={}",
            self.command, self.by, self.machine, self.at, self.id
        )
    }
}

/// The row id a stored `direct-cli` detail names.
fn use_of(detail: &str) -> Option<i64> {
    detail
        .split(' ')
        .find_map(|w| w.strip_prefix("use="))
        .and_then(|n| n.parse().ok())
}

/// The op the stream files a use under.
const OP: &str = "cli.direct";

/// Delete every pending use whose event is already in the stored
/// deployment stream. Returns how many. Run before [`drain`], so a use a
/// previous boot carried is not carried twice, and again after the drainer
/// has had time to write, so the table empties without waiting for a boot.
pub async fn confirm(pool: &Pool) -> Result<usize, PoolError> {
    let uses = pending(pool).await?;
    let Some(since) = uses.iter().map(|u| u.at).min() else {
        return Ok(0);
    };
    let store =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let filter = ordo_exec::secevent::EntryFilter {
        kind: Some("deploy".to_string()),
        since: Some(since),
        ..Default::default()
    };
    let mut stored: Vec<i64> = Vec::new();
    let mut from = 0usize;
    loop {
        let page = match store
            .try_entries_where(
                &ordo_secevent::Scope::Deployment,
                &filter,
                ordo_exec::secevent::Page::Forward { from },
                500,
            )
            .await
        {
            Ok(p) => p,
            // No stream yet: nothing is stored, so nothing is confirmed.
            Err(e) if e.is_missing_table() => return Ok(0),
            Err(e) => return Err(e),
        };
        let Some((last, _)) = page.last() else {
            break;
        };
        from = last + 1;
        stored.extend(page.iter().filter_map(|(_, c)| match &c.entry.kind {
            ordo_secevent::EventKind::Deploy { act, detail } if act == ACT && c.entry.op == OP => {
                use_of(detail)
            }
            _ => None,
        }));
    }
    let carried: Vec<String> = uses
        .iter()
        .filter(|u| stored.contains(&u.id))
        .map(|u| u.id.to_string())
        .collect();
    if carried.is_empty() {
        return Ok(0);
    }
    pool.rows(&format!(
        "DELETE FROM \"$direct_uses\" WHERE \"id\" IN ({})",
        carried.join(", ")
    ))
    .await?;
    Ok(carried.len())
}

/// Every use not yet carried to the security stream, oldest first.
pub async fn pending(pool: &Pool) -> Result<Vec<Use>, PoolError> {
    ensure_table(pool).await?;
    let rows = pool
        .rows(
            "SELECT \"id\", \"at\", \"command\", \"by\", \"machine\" FROM \"$direct_uses\" \
             ORDER BY \"id\"",
        )
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(Use {
                id: r.first()?.clone()?.parse().ok()?,
                at: r.get(1)?.clone()?.parse().ok()?,
                command: r.get(2)?.clone()?,
                by: r.get(3)?.clone()?,
                machine: r.get(4)?.clone()?,
            })
        })
        .collect())
}

/// Carry every pending use not yet in the stored stream to the security
/// stream. Returns how many. The rows stay until [`confirm`] finds their
/// events stored. Called once at boot, after the boot record.
pub async fn drain(pool: &Pool, stream: &ordo_secevent::Recorder) -> Result<usize, PoolError> {
    confirm(pool).await?;
    let uses = pending(pool).await?;
    for u in &uses {
        stream.observe(
            ordo_secevent::Observation {
                space: None,
                subject: ordo_secevent::Subject::platform("cli"),
                op: OP.to_string(),
                kind: ordo_secevent::EventKind::Deploy {
                    act: ACT.to_string(),
                    detail: u.detail(),
                },
                at: ordo_commandlog::Timestamp(u.at),
                correlation: ordo_secevent::correlation::mint(),
            },
            ordo_secevent::Relationship::None,
        );
    }
    Ok(uses.len())
}
