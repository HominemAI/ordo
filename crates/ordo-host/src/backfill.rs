//! **The backfill worker** (`plan-versions.md` increment 10b;
//! `plan-backfill-dual-writes.md` steps B1, B3, B6).
//!
//! A step that fills a new required field (B2), or makes an optional field
//! required (B5), runs in three parts on one database:
//!
//! 1. **Expand.** The column lands nullable, with the step's other
//!    statements (`schema_apply::triage_counted`).
//! 2. **Backfill.** This worker fills it in chunks, each chunk and its
//!    cursor in one transaction, so a kill between them is impossible.
//! 3. **Contract.** After verify, `SET NOT NULL` (`Triaged::after`).
//!
//! # The cursor: `$backfills`
//!
//! One row per `(step, entity, field)` in the database the rows are in:
//! the last `($space, id)` done, rows done and total, attempts and the last
//! error, the state (`running`, `done`, `dead`). `$backfill_spaces` holds
//! each space's row count and running content hash, so verify does not
//! start over. Both travel with the tenant's database: a restore, an
//! export, a move.
//!
//! # A chunk
//!
//! In one transaction, under `backfill.chunk_timeout` as both the
//! statement and the lock timeout:
//!
//! 1. the database's backfill lock, tried and never waited for (a second
//!    worker answers [`BackfillRefusal::Busy`]);
//! 2. the next `backfill.chunk_rows` rows after the cursor, in
//!    `($space, id)` order;
//! 3. `UPDATE` each whose column is null to the mapping over its own
//!    columns (an Ordo expression lowered by `ordo_schema::check`, never SQL
//!    from a person), or to what the backfill function answered;
//! 4. each row's new value folded into its space's content hash
//!    ([`ordo_commandlog::backfill_row_hash`]);
//! 5. the cursor advanced; commit.
//!
//! A chunk that fails is retried with half the rows. `backfill.max_attempts`
//! (5) failures in a row mark the cursor `dead`, naming the last error. An
//! explicit run again retries a dead cursor from where it stopped.
//!
//! # Verify and the audit record (B3)
//!
//! Verify reads the whole table once: no row is left null, a mapping over a
//! new column (B2) equals the column on every row, and each space's count
//! and content hash recompute to what the chunks recorded. A mismatch names
//! the first row.
//!
//! Then the audit record, by the kind the plan states (ruled 2026-09-08 and
//! 2026-09-20):
//!
//! - **summarised** (the default): one group per space, no commands, trailer
//!   `0x04` with the step, the mapping, the row count and the content hash.
//!   Old values come back only from a backup.
//! - **images**: written with each chunk instead, one group per space per
//!   chunk, each converge carrying the row's full before and after images.
//!
//! Both are attributed `system:backfill`, and `done` is written in the same
//! transaction as the summaries, so a run again never writes them twice.
//!
//! # Backfill functions (B6, D16)
//!
//! A step may fill a field with a backend function of the version it goes
//! to. The worker calls it once per chunk: rows in (each row's fields), one
//! value per row out. It runs with the limits of any function. **Ruled
//! 2026-11-23 (Q14):** it may use secrets and call out only as the plan
//! declares, and the plan shows both ([`BackfillFunctions`] is handed the
//! declared lists and nothing else).
//!
//! # Live writes
//!
//! Not here. This increment runs with no live writes to the filled entity
//! in mind; the dual writes that keep a running tenant's writes in the new
//! shape are increment 11a (B2, B4).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{
    backfill_row_hash, Actor, BackfillAudit, BackfillRecord, Command, CommandGroup, CommandId,
    CommandKind, SpaceId, Timestamp, BACKFILL_COMPONENT,
};
use ordo_exec::{sql_lit, OpClass, Pool, PoolError};
use ordo_schema::ddl::q;
use serde_json::{json, Value};

use crate::schema_apply::{BackfillStep, FillMapping};

/// `$backfills` and `$backfill_spaces`, in the database the rows are in.
pub const BACKFILLS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$backfills\" ( \
  \"step\" text NOT NULL, \
  \"entity\" text NOT NULL, \
  \"field\" text NOT NULL, \
  \"mapping\" text NOT NULL, \
  \"audit\" text NOT NULL, \
  \"cursor_space\" text, \
  \"cursor_id\" text, \
  \"rows_done\" bigint NOT NULL DEFAULT 0, \
  \"rows_total\" bigint NOT NULL DEFAULT 0, \
  \"attempts\" integer NOT NULL DEFAULT 0, \
  \"last_error\" text, \
  \"state\" text NOT NULL DEFAULT 'running', \
  \"started_at\" bigint NOT NULL, \
  \"updated_at\" bigint NOT NULL, \
  PRIMARY KEY (\"step\", \"entity\", \"field\") \
); \
CREATE TABLE IF NOT EXISTS \"$backfill_spaces\" ( \
  \"step\" text NOT NULL, \
  \"entity\" text NOT NULL, \
  \"field\" text NOT NULL, \
  \"space\" text NOT NULL, \
  \"rows\" bigint NOT NULL DEFAULT 0, \
  \"hash\" text NOT NULL, \
  PRIMARY KEY (\"step\", \"entity\", \"field\", \"space\") \
)";

/// The advisory-lock namespace of a database's backfill lock: `"BKFL"`.
pub const BACKFILL_LOCK_NS: i32 = 0x424B_464C;

// ── settings ─────────────────────────────────────────────────────────────

/// The worker's settings (`plan-versions.md` §7). Increment 10 stores the
/// rollout and backfill settings in Ordo (ruled 2026-11-23, Q15); until
/// then the caller passes these, and [`BackfillSettings::default`] holds the
/// plan's defaults.
#[derive(Debug, Clone)]
pub struct BackfillSettings {
    /// `backfill.chunk_rows`: rows per chunk.
    pub chunk_rows: u32,
    /// `backfill.chunk_timeout`: one chunk's statement and lock timeout.
    pub chunk_timeout: Duration,
    /// Failures in a row before the cursor is `dead`.
    pub max_attempts: u32,
    /// `backfill.budget`: chunks one run may take before it pauses. `None`
    /// runs to the end. A paused backfill resumes from its cursor.
    pub budget: Option<u32>,
}

impl Default for BackfillSettings {
    fn default() -> Self {
        BackfillSettings {
            chunk_rows: 1000,
            chunk_timeout: Duration::from_secs(30),
            max_attempts: 5,
            budget: None,
        }
    }
}

// ── the typed error ──────────────────────────────────────────────────────

/// Why a backfill did not finish. One typed error (rule 9);
/// `crate::evolve` turns it into the step's refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillRefusal {
    /// Another worker holds this database's backfill lock. Try again.
    Busy,
    /// `max_attempts` chunks failed in a row. The cursor is `dead` and
    /// stays where the last good chunk left it.
    Dead {
        entity: String,
        field: String,
        attempts: u32,
        last_error: String,
    },
    /// Verify found a row that is not what the backfill wrote.
    Verify {
        entity: String,
        field: String,
        /// `space/id` of the first such row, when there is one.
        row: Option<String>,
        why: String,
    },
    /// The backfill function refused or failed.
    Function { name: String, why: String },
    /// A database fault outside a chunk.
    Store(String),
}

impl BackfillRefusal {
    pub fn class(&self) -> &'static str {
        match self {
            BackfillRefusal::Busy => "backfill_busy",
            BackfillRefusal::Dead { .. } => "backfill_dead",
            BackfillRefusal::Verify { .. } => "backfill_verify_failed",
            BackfillRefusal::Function { .. } => "backfill_function_failed",
            BackfillRefusal::Store(_) => "store_failed",
        }
    }

    /// Asking again unchanged may succeed.
    pub fn transient(&self) -> bool {
        matches!(self, BackfillRefusal::Busy | BackfillRefusal::Store(_))
    }
}

impl From<PoolError> for BackfillRefusal {
    fn from(e: PoolError) -> Self {
        BackfillRefusal::Store(e.to_string())
    }
}

impl std::fmt::Display for BackfillRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackfillRefusal::Busy => write!(
                f,
                "backfill_busy: another worker is running a backfill on this database. Run \
                 the step again when it ends; it resumes from its cursor"
            ),
            BackfillRefusal::Dead {
                entity,
                field,
                attempts,
                last_error,
            } => write!(
                f,
                "backfill_dead: the backfill of `{entity}.{field}` failed {attempts} chunks in \
                 a row and stopped at its cursor. The last error: {last_error}. Fix the cause \
                 (a mapping the column's type refuses, a chunk too slow for \
                 `backfill.chunk_timeout`), then run the step again; it retries from the cursor"
            ),
            BackfillRefusal::Verify {
                entity,
                field,
                row,
                why,
            } => write!(
                f,
                "backfill_verify_failed: `{entity}.{field}` {why}{}. The column was not made \
                 required and the database still serves the version before the step. Run the \
                 step again to fill it again from the start",
                row.as_deref()
                    .map(|r| format!(" (first row: {r})"))
                    .unwrap_or_default()
            ),
            BackfillRefusal::Function { name, why } => write!(
                f,
                "backfill_function_failed: backfill function `{name}`: {why}"
            ),
            BackfillRefusal::Store(e) => write!(f, "store_failed: {e}"),
        }
    }
}

// ── backfill functions (B6) ──────────────────────────────────────────────

/// What a backfill function is handed, once per chunk.
#[derive(Debug, Clone)]
pub struct FunctionChunk {
    /// The version the step goes to: the function is its.
    pub version: String,
    pub name: String,
    /// The space the chunk's rows are in. A chunk spanning spaces is called
    /// once per space.
    pub space: String,
    /// `{"rows": [{"id": ..., <field>: ...}, ...]}`.
    pub input: Value,
    /// What the plan declares it may use (Q14). Nothing else.
    pub secrets: Vec<String>,
    pub hosts: Vec<String>,
}

/// Runs a backfill function. The host's is
/// `crate::functions::HostFunctions`; a test may give its own.
#[async_trait::async_trait]
pub trait BackfillFunctions: Send + Sync {
    /// The function's answer: `{"values": [v0, v1, ...]}`, one per row, in
    /// the order given.
    async fn call(&self, chunk: FunctionChunk) -> Result<Value, String>;
}

// ── the worker ───────────────────────────────────────────────────────────

/// One backfill to run on one database.
pub struct Backfill<'a> {
    pub pool: &'a Pool,
    /// The stored plan's digest: the step this backfill belongs to.
    pub step: &'a str,
    /// The version the step goes to.
    pub version: &'a str,
    pub fill: &'a BackfillStep,
    /// The entity as the version the step goes to declares it. Images and
    /// function inputs are read by it.
    pub entity: &'a ordo_schema::Entity,
    pub settings: &'a BackfillSettings,
    pub functions: Option<Arc<dyn BackfillFunctions>>,
}

/// Where a backfill stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub state: String,
    pub rows_done: i64,
    pub rows_total: i64,
    /// Rows this run filled.
    pub rows_this_run: i64,
    /// Chunks this run committed.
    pub chunks: u32,
    pub cursor: Option<(String, String)>,
    pub attempts: u32,
    pub last_error: Option<String>,
    /// No row is left after the cursor: the backfill is ready to verify.
    /// `false` when a run stopped at its budget.
    pub ended: bool,
}

impl Progress {
    pub fn finished(&self) -> bool {
        self.state == "done"
    }

    pub fn to_json(&self) -> Value {
        json!({
            "state": self.state, "rowsDone": self.rows_done, "rowsTotal": self.rows_total,
            "rowsThisRun": self.rows_this_run, "chunks": self.chunks,
            "attempts": self.attempts, "lastError": self.last_error,
        })
    }
}

/// What one chunk did.
enum Chunk {
    /// Rows after the cursor were filled; it moved.
    Advanced { rows: i64 },
    /// No row is left after the cursor.
    End,
}

fn now() -> u64 {
    crate::runtime::now_secs()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

const SPACE: &str = ordo_schema::ddl::SPACE_COLUMN;

impl<'a> Backfill<'a> {
    fn key(&self) -> String {
        format!(
            "\"step\" = {} AND \"entity\" = {} AND \"field\" = {}",
            sql_lit(self.step),
            sql_lit(&self.fill.entity),
            sql_lit(&self.fill.field)
        )
    }

    fn table(&self) -> String {
        q(&self.fill.entity)
    }

    fn col(&self) -> String {
        q(&self.fill.column)
    }

    /// `("$space", "id") > (cursor)`, or true with no cursor.
    fn after(cursor: &Option<(String, String)>) -> String {
        match cursor {
            None => "TRUE".into(),
            Some((s, i)) => format!("({}, \"id\") > ({}, {})", q(SPACE), sql_lit(s), sql_lit(i)),
        }
    }

    /// Make the tables and this backfill's row. Answers where it stands.
    pub async fn ensure(&self) -> Result<Progress, BackfillRefusal> {
        self.pool.rows(BACKFILLS_DDL).await?;
        // The chain the audit record goes to. A database that serves
        // writes has it already; this is idempotent.
        self.pool
            .rows(ordo_exec::commandlog::COMMANDLOG_DDL)
            .await?;
        let total = self
            .pool
            .rows(&format!("SELECT count(*) FROM {}", self.table()))
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0);
        let t = now();
        self.pool
            .rows(&format!(
                "INSERT INTO \"$backfills\" (\"step\", \"entity\", \"field\", \"mapping\", \
                 \"audit\", \"rows_total\", \"started_at\", \"updated_at\") \
                 VALUES ({}, {}, {}, {}, {}, {total}, {t}, {t}) \
                 ON CONFLICT (\"step\", \"entity\", \"field\") DO NOTHING",
                sql_lit(self.step),
                sql_lit(&self.fill.entity),
                sql_lit(&self.fill.field),
                sql_lit(&self.fill.mapping.text()),
                sql_lit(self.fill.audit.as_str()),
            ))
            .await?;
        self.progress().await
    }

    /// Where this backfill stands, read from `$backfills`.
    pub async fn progress(&self) -> Result<Progress, BackfillRefusal> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"state\", \"rows_done\", \"rows_total\", \"cursor_space\", \
                 \"cursor_id\", \"attempts\", \"last_error\" FROM \"$backfills\" WHERE {}",
                self.key()
            ))
            .await?;
        let r = rows
            .first()
            .ok_or_else(|| BackfillRefusal::Store("the backfill has no `$backfills` row".into()))?;
        let g = |i: usize| r.get(i).cloned().flatten();
        Ok(Progress {
            state: g(0).unwrap_or_default(),
            rows_done: g(1).and_then(|s| s.parse().ok()).unwrap_or(0),
            rows_total: g(2).and_then(|s| s.parse().ok()).unwrap_or(0),
            rows_this_run: 0,
            chunks: 0,
            cursor: match (g(3), g(4)) {
                (Some(s), Some(i)) => Some((s, i)),
                _ => None,
            },
            attempts: g(5).and_then(|s| s.parse().ok()).unwrap_or(0),
            last_error: g(6),
            ended: g(0).as_deref() == Some("done"),
        })
    }

    /// **Run the chunk loop** to the end of the table, or to the budget.
    /// A dead cursor is retried from where it stopped: a run is asked for.
    /// A backfill already `done` does nothing.
    pub async fn run(&self) -> Result<Progress, BackfillRefusal> {
        let start = self.ensure().await?;
        if start.finished() {
            return Ok(start);
        }
        if start.state == "dead" {
            self.pool
                .rows(&format!(
                    "UPDATE \"$backfills\" SET \"state\" = 'running', \"attempts\" = 0, \
                     \"updated_at\" = {} WHERE {}",
                    now(),
                    self.key()
                ))
                .await?;
        }
        let mut n = self.settings.chunk_rows.max(1);
        let mut chunks = 0u32;
        let mut rows_this_run = 0i64;
        let mut failures = 0u32;
        let mut ended = false;
        loop {
            if self.settings.budget.is_some_and(|b| chunks >= b) {
                break;
            }
            match self.chunk(n).await {
                Ok(Chunk::Advanced { rows }) => {
                    chunks += 1;
                    rows_this_run += rows;
                    failures = 0;
                }
                Ok(Chunk::End) => {
                    ended = true;
                    break;
                }
                Err(BackfillRefusal::Busy) => return Err(BackfillRefusal::Busy),
                Err(e) => {
                    failures += 1;
                    let why = e.to_string();
                    let dead = failures >= self.settings.max_attempts.max(1);
                    self.pool
                        .rows(&format!(
                            "UPDATE \"$backfills\" SET \"attempts\" = {failures}, \
                             \"last_error\" = {}, \"state\" = {}, \"updated_at\" = {} WHERE {}",
                            sql_lit(&why),
                            sql_lit(if dead { "dead" } else { "running" }),
                            now(),
                            self.key()
                        ))
                        .await?;
                    if dead {
                        return Err(BackfillRefusal::Dead {
                            entity: self.fill.entity.clone(),
                            field: self.fill.field.clone(),
                            attempts: failures,
                            last_error: why,
                        });
                    }
                    // Retried with half the rows (§2).
                    n = (n / 2).max(1);
                }
            }
        }
        let mut p = self.progress().await?;
        p.rows_this_run = rows_this_run;
        p.chunks = chunks;
        p.ended = ended;
        Ok(p)
    }

    /// One chunk, in one transaction with its cursor.
    async fn chunk(&self, n: u32) -> Result<Chunk, BackfillRefusal> {
        let tx = self.pool.begin_for(OpClass::AddressedWrite).await?;
        let ms = self.settings.chunk_timeout.as_millis().max(1);
        tx.rows(&format!("SET LOCAL statement_timeout = {ms}"))
            .await?;
        tx.rows(&format!("SET LOCAL lock_timeout = {ms}")).await?;
        let got = tx
            .rows(&format!(
                "SELECT pg_try_advisory_xact_lock({BACKFILL_LOCK_NS}, hashtext({}))",
                sql_lit(&format!("{}/{}", self.fill.entity, self.fill.field))
            ))
            .await?;
        if got
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .as_deref()
            != Some("t")
        {
            let _ = tx.rollback().await;
            return Err(BackfillRefusal::Busy);
        }
        // The cursor, read under the lock: a worker before this one may
        // have moved it.
        let cur = tx
            .rows(&format!(
                "SELECT \"cursor_space\", \"cursor_id\" FROM \"$backfills\" WHERE {} FOR UPDATE",
                self.key()
            ))
            .await?;
        let cursor = match cur.first() {
            Some(r) => match (r.first().cloned().flatten(), r.get(1).cloned().flatten()) {
                (Some(s), Some(i)) => Some((s, i)),
                _ => None,
            },
            None => {
                return Err(BackfillRefusal::Store(
                    "the `$backfills` row is gone".into(),
                ))
            }
        };
        let after = Self::after(&cursor);
        // The chunk's last row: the upper bound of `(cursor, upper]`.
        let last = tx
            .rows(&format!(
                "SELECT {sp}, \"id\" FROM (SELECT {sp}, \"id\" FROM {t} WHERE {after} \
                 ORDER BY {sp}, \"id\" LIMIT {n}) c ORDER BY {sp} DESC, \"id\" DESC LIMIT 1",
                sp = q(SPACE),
                t = self.table(),
            ))
            .await?;
        let Some((us, ui)) = last
            .first()
            .and_then(|r| Some((r.first().cloned().flatten()?, r.get(1).cloned().flatten()?)))
        else {
            let _ = tx.rollback().await;
            return Ok(Chunk::End);
        };
        let range = format!(
            "{after} AND ({}, \"id\") <= ({}, {})",
            q(SPACE),
            sql_lit(&us),
            sql_lit(&ui)
        );
        // Images (B3's option): the rows before, read under the chunk's
        // own row locks.
        let images = self.fill.audit == BackfillAudit::Images;
        let before = if images {
            self.image_rows(&tx, &range).await?
        } else {
            Vec::new()
        };
        match (&self.fill.mapping, &self.fill.sql) {
            (FillMapping::Expr(_), Some(sql)) => {
                tx.rows(&format!(
                    "UPDATE {t} SET {c} = ({sql}) WHERE {range} AND {c} IS NULL",
                    t = self.table(),
                    c = self.col(),
                ))
                .await?;
            }
            (
                FillMapping::Function {
                    name,
                    secrets,
                    hosts,
                },
                _,
            ) => {
                self.function_chunk(&tx, &range, name, secrets, hosts)
                    .await?;
            }
            (FillMapping::Expr(src), None) => {
                return Err(BackfillRefusal::Store(format!(
                    "the mapping `{src}` was stored without its SQL; render the plan again"
                )))
            }
        }
        // Each row's new value, folded into its space's content hash.
        let filled = tx
            .rows(&format!(
                "SELECT {sp}, \"id\", {c}::text FROM {t} WHERE {range} ORDER BY {sp}, \"id\"",
                sp = q(SPACE),
                c = self.col(),
                t = self.table(),
            ))
            .await?;
        let mut spaces: BTreeMap<String, (i64, [u8; 32])> = BTreeMap::new();
        for s in filled
            .iter()
            .filter_map(|r| r.first().cloned().flatten())
            .collect::<std::collections::BTreeSet<_>>()
        {
            let held = tx
                .rows(&format!(
                    "SELECT \"rows\", \"hash\" FROM \"$backfill_spaces\" WHERE {} AND \"space\" = {}",
                    self.key(),
                    sql_lit(&s)
                ))
                .await?;
            let (rows, hash) = held
                .first()
                .and_then(|r| {
                    Some((
                        r.first().cloned().flatten()?.parse::<i64>().ok()?,
                        unhex32(&r.get(1).cloned().flatten()?)?,
                    ))
                })
                .unwrap_or((0, [0u8; 32]));
            spaces.insert(s, (rows, hash));
        }
        for r in &filled {
            let (Some(s), Some(id)) = (r.first().cloned().flatten(), r.get(1).cloned().flatten())
            else {
                continue;
            };
            let v = r.get(2).cloned().flatten();
            let e = spaces.entry(s.clone()).or_insert((0, [0u8; 32]));
            e.0 += 1;
            e.1 = backfill_row_hash(&e.1, &s, &id, v.as_deref());
        }
        for (s, (rows, hash)) in &spaces {
            tx.rows(&format!(
                "INSERT INTO \"$backfill_spaces\" (\"step\", \"entity\", \"field\", \"space\", \
                 \"rows\", \"hash\") VALUES ({}, {}, {}, {}, {rows}, {}) \
                 ON CONFLICT (\"step\", \"entity\", \"field\", \"space\") DO UPDATE \
                 SET \"rows\" = EXCLUDED.\"rows\", \"hash\" = EXCLUDED.\"hash\"",
                sql_lit(self.step),
                sql_lit(&self.fill.entity),
                sql_lit(&self.fill.field),
                sql_lit(s),
                sql_lit(&hex(hash)),
            ))
            .await?;
        }
        if images {
            let after_rows = self.image_rows(&tx, &range).await?;
            self.image_groups(&tx, &before, &after_rows).await?;
        }
        let done = filled.len() as i64;
        tx.rows(&format!(
            "UPDATE \"$backfills\" SET \"cursor_space\" = {}, \"cursor_id\" = {}, \
             \"rows_done\" = \"rows_done\" + {done}, \"attempts\" = 0, \"last_error\" = NULL, \
             \"updated_at\" = {} WHERE {}",
            sql_lit(&us),
            sql_lit(&ui),
            now(),
            self.key()
        ))
        .await?;
        tx.commit().await?;
        Ok(Chunk::Advanced { rows: done })
    }

    /// The rows of `range`, each `(space, id, image)`, by the version the
    /// step goes to.
    async fn image_rows(
        &self,
        tx: &ordo_exec::TxConn,
        range: &str,
    ) -> Result<Vec<(String, String, ordo_commandlog::Image)>, BackfillRefusal> {
        let cols = ordo_schema::image::image_columns(self.entity);
        let col_sql = cols
            .iter()
            .map(|f| q(&f.column()))
            .collect::<Vec<_>>()
            .join(", ");
        let rows = tx
            .rows(&format!(
                "SELECT {sp}, {col_sql} FROM {t} WHERE {range} ORDER BY {sp}, \"id\" FOR UPDATE",
                sp = q(SPACE),
                t = self.table(),
            ))
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let space = r.first().cloned().flatten().unwrap_or_default();
            let cells = &r[1..];
            let img =
                ordo_schema::image::image_of_row(self.entity, cells, &BTreeMap::new(), &mut NoSeal)
                    .map_err(|e| BackfillRefusal::Store(e.to_string()))?;
            let id = match img.get("id") {
                Some(ordo_commandlog::Value::Id(s) | ordo_commandlog::Value::Text(s)) => s.clone(),
                _ => String::new(),
            };
            out.push((space, id, img));
        }
        Ok(out)
    }

    /// B3's option: one group per space for this chunk, a converge per row
    /// with its full before and after images.
    async fn image_groups(
        &self,
        tx: &ordo_exec::TxConn,
        before: &[(String, String, ordo_commandlog::Image)],
        after: &[(String, String, ordo_commandlog::Image)],
    ) -> Result<(), BackfillRefusal> {
        let at = Timestamp(now());
        let befores: BTreeMap<(&str, &str), &ordo_commandlog::Image> = before
            .iter()
            .map(|(s, i, img)| ((s.as_str(), i.as_str()), img))
            .collect();
        let mut by_space: BTreeMap<
            &str,
            Vec<(&str, &ordo_commandlog::Image, &ordo_commandlog::Image)>,
        > = BTreeMap::new();
        for (s, id, img) in after {
            if let Some(b) = befores.get(&(s.as_str(), id.as_str())) {
                by_space
                    .entry(s.as_str())
                    .or_default()
                    .push((id.as_str(), b, img));
            }
        }
        for (space, rows) in by_space {
            let correlation = ordo_secevent::correlation::mint();
            let mut content = [0u8; 32];
            let commands: Vec<Command> = rows
                .iter()
                .enumerate()
                .map(|(i, (id, b, a))| {
                    let v = a.get(&self.fill.field).map(value_text);
                    content = backfill_row_hash(&content, space, id, v.flatten().as_deref());
                    Command {
                        id: CommandId(format!("{}-{i}", correlation.0)),
                        space: SpaceId(space.to_string()),
                        entity: self.fill.entity.clone(),
                        row_id: id.to_string(),
                        kind: CommandKind::Converge,
                        before: Some((*b).clone()),
                        after: Some((*a).clone()),
                        at,
                    }
                })
                .collect();
            let group = CommandGroup {
                correlation,
                actor: Actor::System(BACKFILL_COMPONENT.into()),
                owner_administration: false,
                restored_unverified: false,
                restore: None,
                backfill: Some(BackfillRecord {
                    space: SpaceId(space.to_string()),
                    kind: BackfillAudit::Images,
                    step: self.step.to_string(),
                    entity: self.fill.entity.clone(),
                    field: self.fill.field.clone(),
                    mapping: self.fill.mapping.text(),
                    rows: commands.len() as u64,
                    content,
                }),
                commands,
            };
            tx.append_group(&group)
                .await
                .map_err(|e| BackfillRefusal::Store(format!("the image group: {e}")))?;
        }
        Ok(())
    }

    /// B6: the function computes the chunk's values, one call per space.
    async fn function_chunk(
        &self,
        tx: &ordo_exec::TxConn,
        range: &str,
        name: &str,
        secrets: &[String],
        hosts: &[String],
    ) -> Result<(), BackfillRefusal> {
        let Some(functions) = &self.functions else {
            return Err(BackfillRefusal::Function {
                name: name.to_string(),
                why: "this node runs no backend functions; run the step where the host does".into(),
            });
        };
        let rows = self.image_rows(tx, range).await?;
        let mut by_space: BTreeMap<&str, Vec<&(String, String, ordo_commandlog::Image)>> =
            BTreeMap::new();
        for r in rows.iter() {
            // Only the rows still to fill: a B5 row that holds a value keeps it.
            if matches!(
                r.2.get(&self.fill.field),
                None | Some(ordo_commandlog::Value::Null)
            ) {
                by_space.entry(r.0.as_str()).or_default().push(r);
            }
        }
        for (space, rows) in by_space {
            let input = json!({
                "rows": rows.iter().map(|(_, _, img)| image_json(img)).collect::<Vec<_>>(),
            });
            let answer = functions
                .call(FunctionChunk {
                    version: self.version.to_string(),
                    name: name.to_string(),
                    space: space.to_string(),
                    input,
                    secrets: secrets.to_vec(),
                    hosts: hosts.to_vec(),
                })
                .await
                .map_err(|why| BackfillRefusal::Function {
                    name: name.to_string(),
                    why,
                })?;
            let values = answer["values"].as_array().cloned().unwrap_or_default();
            if values.len() != rows.len() {
                return Err(BackfillRefusal::Function {
                    name: name.to_string(),
                    why: format!(
                        "it answered {} value(s) for {} row(s); answer `{{\"values\": [...]}}`, \
                         one per row in the order given",
                        values.len(),
                        rows.len()
                    ),
                });
            }
            for ((_, id, _), v) in rows.iter().zip(values) {
                let lit = match &v {
                    Value::Null => {
                        return Err(BackfillRefusal::Function {
                            name: name.to_string(),
                            why: format!(
                                "it answered null for row `{id}`, and the field is required"
                            ),
                        })
                    }
                    Value::String(s) => sql_lit(s),
                    other => sql_lit(&other.to_string()),
                };
                tx.rows(&format!(
                    "UPDATE {t} SET {c} = {lit}::{ty} WHERE {sp} = {} AND \"id\" = {} AND {c} IS NULL",
                    sql_lit(space),
                    sql_lit(id),
                    t = self.table(),
                    c = self.col(),
                    ty = self.fill.column_ty,
                    sp = q(SPACE),
                ))
                .await?;
            }
        }
        Ok(())
    }

    /// **Verify** (B3): the whole table, read once. Then the summarised
    /// record, and `done`, in one transaction.
    pub async fn verify_and_close(&self) -> Result<Verified, BackfillRefusal> {
        let p = self.progress().await?;
        if p.finished() {
            return Ok(Verified {
                rows: p.rows_done,
                spaces: Vec::new(),
                already: true,
            });
        }
        let fail = |row: Option<String>, why: String| BackfillRefusal::Verify {
            entity: self.fill.entity.clone(),
            field: self.fill.field.clone(),
            row,
            why,
        };
        let first = |rows: Vec<ordo_exec::TextRow>| {
            rows.first().map(|r| {
                format!(
                    "{}/{}",
                    r.first().cloned().flatten().unwrap_or_default(),
                    r.get(1).cloned().flatten().unwrap_or_default()
                )
            })
        };
        // No row left null.
        let nulls = self
            .pool
            .rows(&format!(
                "SELECT {sp}, \"id\" FROM {t} WHERE {c} IS NULL ORDER BY {sp}, \"id\" LIMIT 1",
                sp = q(SPACE),
                t = self.table(),
                c = self.col()
            ))
            .await?;
        if let Some(row) = first(nulls) {
            return Err(fail(Some(row), "has a row the backfill left null".into()));
        }
        // A new column (B2) filled by an expression holds the mapping on
        // every row. A B5 field kept what its rows already held.
        if let (FillMapping::Expr(_), Some(sql), "B2") =
            (&self.fill.mapping, &self.fill.sql, self.fill.code)
        {
            let off = self
                .pool
                .rows(&format!(
                    "SELECT {sp}, \"id\" FROM {t} WHERE {c} IS DISTINCT FROM ({sql}) \
                     ORDER BY {sp}, \"id\" LIMIT 1",
                    sp = q(SPACE),
                    t = self.table(),
                    c = self.col()
                ))
                .await?;
            if let Some(row) = first(off) {
                return Err(fail(
                    Some(row),
                    format!(
                        "holds a value other than the mapping `{}`",
                        self.fill.mapping.text()
                    ),
                ));
            }
        }
        // Each space's count and content hash, recomputed over the table
        // in `($space, id)` pages.
        let mut got: BTreeMap<String, (i64, [u8; 32])> = BTreeMap::new();
        let mut cursor: Option<(String, String)> = None;
        loop {
            let page = self
                .pool
                .rows(&format!(
                    "SELECT {sp}, \"id\", {c}::text FROM {t} WHERE {a} ORDER BY {sp}, \"id\" \
                     LIMIT 5000",
                    sp = q(SPACE),
                    c = self.col(),
                    t = self.table(),
                    a = Self::after(&cursor),
                ))
                .await?;
            if page.is_empty() {
                break;
            }
            for r in &page {
                let (Some(s), Some(id)) =
                    (r.first().cloned().flatten(), r.get(1).cloned().flatten())
                else {
                    continue;
                };
                let v = r.get(2).cloned().flatten();
                let e = got.entry(s.clone()).or_insert((0, [0u8; 32]));
                e.0 += 1;
                e.1 = backfill_row_hash(&e.1, &s, &id, v.as_deref());
                cursor = Some((s, id));
            }
        }
        let held = self
            .pool
            .rows(&format!(
                "SELECT \"space\", \"rows\", \"hash\" FROM \"$backfill_spaces\" WHERE {} \
                 ORDER BY \"space\"",
                self.key()
            ))
            .await?;
        let mut recorded: BTreeMap<String, (i64, [u8; 32])> = BTreeMap::new();
        for r in held {
            if let (Some(s), Some(n), Some(h)) = (
                r.first().cloned().flatten(),
                r.get(1).cloned().flatten().and_then(|n| n.parse().ok()),
                r.get(2).cloned().flatten().and_then(|h| unhex32(&h)),
            ) {
                recorded.insert(s, (n, h));
            }
        }
        for (s, (n, h)) in &got {
            match recorded.get(s) {
                Some((rn, rh)) if rn == n && rh == h => {}
                Some((rn, _)) if rn != n => {
                    return Err(fail(
                        None,
                        format!(
                            "in space `{s}` holds {n} row(s), and the backfill filled {rn}. \
                             A row was added or removed while it ran"
                        ),
                    ))
                }
                Some(_) => {
                    return Err(fail(
                        None,
                        format!(
                            "in space `{s}` does not hash to what the backfill wrote: a value \
                             changed after its chunk"
                        ),
                    ))
                }
                None => {
                    return Err(fail(
                        None,
                        format!("in space `{s}` has rows the backfill never reached"),
                    ))
                }
            }
        }
        if let Some(s) = recorded.keys().find(|s| !got.contains_key(*s)) {
            return Err(fail(
                None,
                format!("in space `{s}` lost every row the backfill filled"),
            ));
        }
        // The record, and `done`, in one transaction.
        let tx = self.pool.begin_for(OpClass::AddressedWrite).await?;
        let state = tx
            .rows(&format!(
                "SELECT \"state\" FROM \"$backfills\" WHERE {} FOR UPDATE",
                self.key()
            ))
            .await?;
        if state
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .as_deref()
            == Some("done")
        {
            let _ = tx.rollback().await;
            return Ok(Verified {
                rows: p.rows_done,
                spaces: Vec::new(),
                already: true,
            });
        }
        let mut spaces = Vec::new();
        for (s, (n, h)) in &got {
            if self.fill.audit == BackfillAudit::Summarised {
                let group = CommandGroup {
                    correlation: ordo_secevent::correlation::mint(),
                    actor: Actor::System(BACKFILL_COMPONENT.into()),
                    commands: Vec::new(),
                    owner_administration: false,
                    restored_unverified: false,
                    restore: None,
                    backfill: Some(BackfillRecord {
                        space: SpaceId(s.clone()),
                        kind: BackfillAudit::Summarised,
                        step: self.step.to_string(),
                        entity: self.fill.entity.clone(),
                        field: self.fill.field.clone(),
                        mapping: self.fill.mapping.text(),
                        rows: *n as u64,
                        content: *h,
                    }),
                };
                tx.append_group(&group)
                    .await
                    .map_err(|e| BackfillRefusal::Store(format!("the summary group: {e}")))?;
            }
            spaces.push(SpaceVerified {
                space: s.clone(),
                rows: *n,
                content: hex(h),
            });
        }
        let total: i64 = got.values().map(|(n, _)| n).sum();
        tx.rows(&format!(
            "UPDATE \"$backfills\" SET \"state\" = 'done', \"rows_total\" = {total}, \
             \"updated_at\" = {} WHERE {}",
            now(),
            self.key()
        ))
        .await?;
        tx.commit().await?;
        Ok(Verified {
            rows: total,
            spaces,
            already: false,
        })
    }
}

/// What verify found, per space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub rows: i64,
    pub spaces: Vec<SpaceVerified>,
    /// Verified and recorded by an earlier run.
    pub already: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceVerified {
    pub space: String,
    pub rows: i64,
    /// The content hash, hex.
    pub content: String,
}

/// A row's image as a function's input: field names to JSON values.
fn image_json(img: &ordo_commandlog::Image) -> Value {
    Value::Object(
        img.iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v {
                        ordo_commandlog::Value::Null => Value::Null,
                        ordo_commandlog::Value::Bool(b) => Value::Bool(*b),
                        ordo_commandlog::Value::Int(n) => Value::from(*n),
                        ordo_commandlog::Value::Text(s) | ordo_commandlog::Value::Id(s) => {
                            Value::String(s.clone())
                        }
                        ordo_commandlog::Value::Decimal(d) => Value::String(d.to_string()),
                        ordo_commandlog::Value::SealedRef(_) => Value::Null,
                    },
                )
            })
            .collect(),
    )
}

/// A logged value as the text its column renders, for the content hash.
fn value_text(v: &ordo_commandlog::Value) -> Option<String> {
    match v {
        ordo_commandlog::Value::Null => None,
        ordo_commandlog::Value::Bool(b) => Some(if *b { "true" } else { "false" }.into()),
        ordo_commandlog::Value::Int(n) => Some(n.to_string()),
        ordo_commandlog::Value::Text(s) | ordo_commandlog::Value::Id(s) => Some(s.clone()),
        ordo_commandlog::Value::Decimal(d) => Some(d.to_string()),
        ordo_commandlog::Value::SealedRef(_) => None,
    }
}

/// Images are refused for an entity with sealed or file fields
/// (`schema_apply::backfill_step`), so nothing is ever sealed here.
struct NoSeal;

impl ordo_schema::image::SealSink for NoSeal {
    fn seal_ref(
        &mut self,
        entity: &str,
        field: &str,
        _anchor: &str,
        _envelope: &[u8],
    ) -> Result<ordo_commandlog::Value, ordo_schema::image::ImageError> {
        Err(ordo_schema::image::ImageError::Sink(format!(
            "`{entity}.{field}` is sealed; a backfill with images does not reach one"
        )))
    }
}
