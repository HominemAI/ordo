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
//! event with the act `direct-cli` on the security stream, then deletes it.
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
    /// The one line the security stream carries.
    pub fn detail(&self) -> String {
        format!(
            "command={} by={} machine={} at={}",
            self.command, self.by, self.machine, self.at
        )
    }
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

/// Carry every pending use to the security stream, then delete what was
/// carried. Returns how many. Called once at boot, after the boot record.
pub async fn drain(pool: &Pool, stream: &ordo_secevent::Recorder) -> Result<usize, PoolError> {
    let uses = pending(pool).await?;
    for u in &uses {
        stream.observe(
            ordo_secevent::Observation {
                space: None,
                subject: ordo_secevent::Subject::platform("cli"),
                op: "cli.direct".to_string(),
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
    if let Some(last) = uses.last() {
        pool.rows(&format!(
            "DELETE FROM \"$direct_uses\" WHERE \"id\" <= {}",
            last.id
        ))
        .await?;
    }
    Ok(uses.len())
}
