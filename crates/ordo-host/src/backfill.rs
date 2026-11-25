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
//! # Live writes (increment 11a)
//!
//! A tenant that moves keeps writing while the worker runs. The move
//! overlay (`ordo_exec::overlay`) sets each written row's new column by the
//! same mapping in the write's own transaction, so the worker and a write
//! agree whichever runs second. A rename (B4) copies every row, since its
//! new column may hold its default. Verify under live writes
//! ([`Backfill::verify_live_and_close`]) checks the mapping on every row and
//! records the count and hash it reads, rather than comparing them with the
//! chunks', which live writes change by design.

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

/// The backfill settings (`plan-versions.md` §7), stored in
/// `$version_settings` beside the version and rollout settings (ruled
/// 2026-11-23, Q15) and set with `ordo versions set backfill.<name> <value>
/// --reason "..."` (`std.deployment.setVersionSetting`).
pub const CHUNK_ROWS_SETTING: &str = "backfill.chunk_rows";
pub const CHUNK_TIMEOUT_SETTING: &str = "backfill.chunk_timeout";
pub const MAX_ATTEMPTS_SETTING: &str = "backfill.max_attempts";
pub const BUDGET_SETTING: &str = "backfill.budget";
/// How many tenants' moves a rollout's turn runs at once (increment 11b
/// part 2; `plan-versions.md` §7, default 8).
pub const CONCURRENCY_SETTING: &str = "backfill.concurrency";
/// How many of those share one database at most (proposed in
/// `plan-backfill-dual-writes.md` §2, default 2), so the tenants of one
/// database do not take its whole pool.
pub const PER_DATABASE_SETTING: &str = "backfill.per_database";

pub const SETTINGS: [&str; 6] = [
    CHUNK_ROWS_SETTING,
    CHUNK_TIMEOUT_SETTING,
    MAX_ATTEMPTS_SETTING,
    BUDGET_SETTING,
    CONCURRENCY_SETTING,
    PER_DATABASE_SETTING,
];

/// The most moves a turn runs at once, and the most in one database.
pub const MAX_CONCURRENCY: u32 = 64;

/// The most rows one chunk may take. A chunk is one transaction.
pub const MAX_CHUNK_ROWS: u32 = 100_000;

/// The full name of a backfill setting from what a person typed:
/// `backfill.chunk_rows`, `chunk_rows` or `chunk-rows`.
pub fn setting_name(text: &str) -> Option<&'static str> {
    let t = text.trim();
    let t = t.strip_prefix("backfill.").unwrap_or(t).replace('-', "_");
    SETTINGS
        .into_iter()
        .find(|s| s.strip_prefix("backfill.") == Some(t.as_str()))
}

/// Check a value for `setting` and answer it as stored. `None` clears it.
pub fn check_setting(
    setting: &str,
    value: Option<&str>,
) -> Result<Option<String>, BackfillRefusal> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let bad = |why: &str| BackfillRefusal::Setting {
        setting: setting.to_string(),
        value: v.chars().take(40).collect(),
        why: why.to_string(),
    };
    let count = |lo: u32, hi: u32| match v.parse::<u32>() {
        Ok(n) if (lo..=hi).contains(&n) => Some(n.to_string()),
        _ => None,
    };
    match setting {
        CHUNK_ROWS_SETTING => count(1, MAX_CHUNK_ROWS).map(Some).ok_or_else(|| {
            bad(&format!(
                "it is a whole number of rows, 1 to {MAX_CHUNK_ROWS}"
            ))
        }),
        MAX_ATTEMPTS_SETTING => count(1, 100)
            .map(Some)
            .ok_or_else(|| bad("it is a whole number of chunks, 1 to 100")),
        CONCURRENCY_SETTING | PER_DATABASE_SETTING => {
            count(1, MAX_CONCURRENCY).map(Some).ok_or_else(|| {
                bad(&format!(
                    "it is a whole number of moves at once, 1 to {MAX_CONCURRENCY}"
                ))
            })
        }
        BUDGET_SETTING if v == "none" => Ok(None),
        BUDGET_SETTING => count(1, u32::MAX)
            .map(Some)
            .ok_or_else(|| bad("it is a whole number of chunks, 1 or more, or `none`")),
        CHUNK_TIMEOUT_SETTING => match crate::windows::parse_length(v) {
            Ok(n) if (1..=3_600).contains(&n) => Ok(Some(n.to_string())),
            _ => Err(bad(
                "it is a length from 1 second to 1 hour, like `20s` or `2m`",
            )),
        },
        _ => Err(bad(&format!(
            "there is no such backfill setting; there are {}",
            SETTINGS.join(", ")
        ))),
    }
}

/// The worker's settings (`plan-versions.md` §7). A stored step reads them
/// from `$version_settings` ([`stored_settings`]); unset ones are
/// [`BackfillSettings::default`], the plan's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// `backfill.concurrency`: tenants a rollout's turn moves at once.
    pub concurrency: u32,
    /// `backfill.per_database`: of those, the most in one database.
    pub per_database: u32,
}

impl Default for BackfillSettings {
    fn default() -> Self {
        BackfillSettings {
            chunk_rows: 1000,
            chunk_timeout: Duration::from_secs(20),
            max_attempts: 5,
            budget: None,
            concurrency: 8,
            per_database: 2,
        }
    }
}

impl BackfillSettings {
    /// As the settings op answers them.
    pub fn to_json(&self) -> Value {
        json!({
            "chunkRows": self.chunk_rows,
            "chunkTimeout": self.chunk_timeout.as_secs(),
            "maxAttempts": self.max_attempts,
            "budget": self.budget,
            "concurrency": self.concurrency,
            "perDatabase": self.per_database,
        })
    }

    /// One stored value over the default. A value the checks would refuse
    /// (written by hand) reads as the default.
    fn with(&mut self, setting: &str, value: Option<&str>) {
        let d = BackfillSettings::default();
        let n = |v: Option<&str>| {
            v.and_then(|v| v.trim().parse::<u32>().ok())
                .filter(|n| *n >= 1)
        };
        match setting {
            CHUNK_ROWS_SETTING => {
                self.chunk_rows = n(value)
                    .map(|n| n.min(MAX_CHUNK_ROWS))
                    .unwrap_or(d.chunk_rows)
            }
            CHUNK_TIMEOUT_SETTING => {
                self.chunk_timeout = n(value)
                    .map(|s| Duration::from_secs(u64::from(s)))
                    .unwrap_or(d.chunk_timeout)
            }
            MAX_ATTEMPTS_SETTING => self.max_attempts = n(value).unwrap_or(d.max_attempts),
            BUDGET_SETTING => self.budget = n(value),
            CONCURRENCY_SETTING => {
                self.concurrency = n(value)
                    .map(|n| n.min(MAX_CONCURRENCY))
                    .unwrap_or(d.concurrency)
            }
            PER_DATABASE_SETTING => {
                self.per_database = n(value)
                    .map(|n| n.min(MAX_CONCURRENCY))
                    .unwrap_or(d.per_database)
            }
            _ => {}
        }
    }
}

/// The backfill settings as stored in `$version_settings`. Unset ones read
/// as their defaults; so does a database with no settings table yet.
pub async fn stored_settings(pool: &Pool) -> Result<BackfillSettings, BackfillRefusal> {
    let rows = match pool
        .rows(
            "SELECT DISTINCT ON (\"setting\") \"setting\", \"value\" \
             FROM \"$version_settings\" WHERE \"setting\" LIKE 'backfill.%' \
             ORDER BY \"setting\", \"seq\" DESC",
        )
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_missing_table() => return Ok(BackfillSettings::default()),
        Err(e) => return Err(e.into()),
    };
    let mut out = BackfillSettings::default();
    for r in rows {
        let (Some(Some(name)), value) = (r.first().cloned(), r.get(1).cloned().flatten()) else {
            continue;
        };
        out.with(&name, value.as_deref());
    }
    Ok(out)
}

/// **Change one backfill setting**, or clear it with `None`, with who and
/// why (Q15). Checked first ([`check_setting`]); written beside the version
/// settings under the pin lock. `true` when it changed.
pub async fn set_setting(
    pool: &Pool,
    setting: &str,
    value: Option<&str>,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<(bool, BackfillSettings), BackfillRefusal> {
    let reason = crate::versions::check_reason(reason)
        .map_err(|e| BackfillRefusal::Reason(e.to_string()))?;
    let Some(name) = setting_name(setting) else {
        return Err(BackfillRefusal::Setting {
            setting: setting.chars().take(60).collect(),
            value: value.unwrap_or_default().chars().take(40).collect(),
            why: format!(
                "there is no such backfill setting; there are {}",
                SETTINGS.join(", ")
            ),
        });
    };
    let value = check_setting(name, value)?;
    pool.rows(crate::versions::SETTINGS_DDL).await?;
    let tx = pool.begin().await?;
    tx.rows(&format!(
        "SELECT pg_advisory_xact_lock({}, 0)",
        crate::versions::PIN_LOCK_NS
    ))
    .await?;
    let now = tx
        .rows(&format!(
            "SELECT \"value\" FROM \"$version_settings\" WHERE \"setting\" = {} \
             ORDER BY \"seq\" DESC LIMIT 1",
            sql_lit(name)
        ))
        .await?;
    let current = now.first().and_then(|r| r.first().cloned().flatten());
    // Unchanged: the same value, or clearing what was never set.
    if current == value {
        tx.rollback().await?;
        return Ok((false, stored_settings(pool).await?));
    }
    tx.rows(&format!(
        "INSERT INTO \"$version_settings\" (\"setting\", \"value\", \"actor\", \"reason\", \"at\") \
         VALUES ({}, {}, {}, {}, {at})",
        sql_lit(name),
        value.as_deref().map(sql_lit).unwrap_or_else(|| "NULL".into()),
        sql_lit(actor),
        sql_lit(&reason),
    ))
    .await?;
    tx.commit().await?;
    Ok((true, stored_settings(pool).await?))
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
    /// A sealed backfill (increment 13) could not take a key lease, seal
    /// or open a value. Names the space, never a value.
    Seal { space: String, why: String },
    /// A database fault outside a chunk.
    Store(String),
    /// A backfill setting's value is not one it takes.
    Setting {
        setting: String,
        value: String,
        why: String,
    },
    /// A setting change without a usable reason.
    Reason(String),
}

impl BackfillRefusal {
    pub fn class(&self) -> &'static str {
        match self {
            BackfillRefusal::Busy => "backfill_busy",
            BackfillRefusal::Dead { .. } => "backfill_dead",
            BackfillRefusal::Verify { .. } => "backfill_verify_failed",
            BackfillRefusal::Function { .. } => "backfill_function_failed",
            BackfillRefusal::Seal { .. } => "backfill_seal_failed",
            BackfillRefusal::Store(_) => "store_failed",
            BackfillRefusal::Setting { .. } | BackfillRefusal::Reason(_) => "invalid_param",
        }
    }

    /// As the door answers it, `what` saying what did not happen.
    pub fn to_exec_error(&self, what: &str) -> ordo_api::ExecError {
        use ordo_api::ErrorCode;
        let message = format!("{what}: {self}");
        match self {
            BackfillRefusal::Setting { .. } | BackfillRefusal::Reason(_) => {
                ordo_api::ExecError::new(ErrorCode::InvalidParam, message)
            }
            _ => ordo_api::ExecError::new(ErrorCode::SpaceUnavailable, message),
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
            BackfillRefusal::Seal { space, why } => write!(
                f,
                "backfill_seal_failed: space `{space}`: {why}. Nothing in the chunk was \
                 sealed, and the plaintext column is kept. Run the step again on a node with \
                 the deployment's key custody; it resumes from its cursor"
            ),
            BackfillRefusal::Store(e) => write!(f, "store_failed: {e}"),
            BackfillRefusal::Setting {
                setting,
                value,
                why,
            } => write!(
                f,
                "{setting} `{value}` is not one: {why}. `ordo versions set {setting} <value> \
                 --reason \"...\"` sets it"
            ),
            BackfillRefusal::Reason(why) => write!(f, "{why}"),
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
    /// Key custody, for a sealed backfill (increment 13): one lease per
    /// space, `Purpose::Migration`. `None` refuses a seal.
    pub keys: Option<Arc<dyn ordo_crypto::KeyProvider>>,
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

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
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

    /// The table the cursor walks: the old table of an entity rename's
    /// copy, or the table filled.
    fn source(&self) -> String {
        match self.fill.copy_from() {
            Some(from) => q(from),
            None => self.table(),
        }
    }

    /// What one row's content hash reads: the column filled, or a copied
    /// row whole.
    fn value(&self) -> String {
        match self.fill.copy_from() {
            Some(_) => format!("row_to_json({})::text", self.table()),
            None => format!("{}::text", self.col()),
        }
    }

    /// The copied columns of an entity rename: every physical column.
    fn copied_columns(&self) -> String {
        ordo_schema::ddl::physical_columns(self.entity)
            .iter()
            .map(|c| q(&c.name))
            .collect::<Vec<_>>()
            .join(", ")
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
            .rows(&format!("SELECT count(*) FROM {}", self.source()))
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
                t = self.source(),
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
                // A rename (B4) copies every row: its new column may hold
                // the default it was added with. A new or newly required
                // field fills only what is empty.
                let only_empty = if matches!(self.fill.code, "B4" | "B12") {
                    String::new()
                } else {
                    format!(" AND {} IS NULL", self.col())
                };
                tx.rows(&format!(
                    "UPDATE {t} SET {c} = ({sql}) WHERE {range}{only_empty}",
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
            (FillMapping::Seal, _) => {
                self.seal_chunk(&tx, &range).await?;
            }
            (FillMapping::BlindIndex, _) => {
                self.index_chunk(&tx, &range).await?;
            }
            // An entity rename: the chunk's rows of the old table, copied
            // whole, each locked while it is read so a write to it waits
            // and its dual write lands after. A row a write already put in
            // the new table is newer, and kept.
            (FillMapping::Copy(from), _) => {
                let cols = self.copied_columns();
                tx.rows(&format!(
                    "INSERT INTO {t} ({cols}) SELECT {cols} FROM {o} WHERE {range} \
                     ORDER BY {sp}, \"id\" FOR SHARE ON CONFLICT ({sp}, \"id\") DO NOTHING",
                    t = self.table(),
                    o = q(from),
                    sp = q(SPACE),
                ))
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
                "SELECT {sp}, \"id\", {v} FROM {t} WHERE {range} ORDER BY {sp}, \"id\"",
                sp = q(SPACE),
                v = self.value(),
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

    /// The plaintext column a sealed backfill reads.
    fn plain_col(&self) -> Option<String> {
        self.fill.sealing_from.as_deref().map(q)
    }

    /// One key lease for `space`, for this step (increment 13).
    fn lease(&self, space: &str) -> Result<ordo_crypto::KeyLease, BackfillRefusal> {
        let Some(keys) = &self.keys else {
            return Err(BackfillRefusal::Seal {
                space: space.to_string(),
                why: "this node holds no key custody".into(),
            });
        };
        keys.release(
            &ordo_crypto::SpaceId(space.to_string()),
            ordo_crypto::Purpose::Migration,
            &format!("seal:{}", self.step),
        )
        .map_err(|e| BackfillRefusal::Seal {
            space: space.to_string(),
            why: format!("key release: {e}"),
        })
    }

    /// Increment 13: each row's plaintext sealed under its space's key, at
    /// its row, into the column beside it. One lease per space per chunk.
    /// The plaintext column is never written: no plaintext dual write (D9).
    async fn seal_chunk(&self, tx: &ordo_exec::TxConn, range: &str) -> Result<(), BackfillRefusal> {
        let Some(plain) = self.plain_col() else {
            return Err(BackfillRefusal::Store(
                "a sealed backfill was stored without its plaintext column; render the plan again"
                    .into(),
            ));
        };
        let rows = tx
            .rows(&format!(
                "SELECT {sp}, \"id\", {plain} FROM {t} WHERE {range} AND {c} IS NULL \
                 AND {plain} IS NOT NULL ORDER BY {sp}, \"id\" FOR UPDATE",
                sp = q(SPACE),
                t = self.table(),
                c = self.col(),
            ))
            .await?;
        let mut lease: Option<(String, ordo_crypto::KeyLease)> = None;
        for r in rows {
            let (Some(space), Some(id), Some(text)) = (
                r.first().cloned().flatten(),
                r.get(1).cloned().flatten(),
                r.get(2).cloned().flatten(),
            ) else {
                continue;
            };
            if lease.as_ref().is_none_or(|(s, _)| *s != space) {
                lease = Some((space.clone(), self.lease(&space)?));
            }
            let (_, l) = lease.as_ref().expect("taken just above");
            let keys = self.keys.as_ref().expect("a lease was taken");
            let sealed = keys
                .seal_at(
                    l,
                    &self.fill.entity,
                    &self.fill.field,
                    ordo_crypto::Anchor::Row(&id),
                    text.as_bytes(),
                )
                .map_err(|e| BackfillRefusal::Seal {
                    space: space.clone(),
                    why: format!("seal: {e}"),
                })?;
            tx.rows(&format!(
                "UPDATE {t} SET {c} = '\\x{}'::bytea WHERE {sp} = {} AND \"id\" = {} AND {c} IS NULL",
                hex(&sealed),
                sql_lit(&space),
                sql_lit(&id),
                t = self.table(),
                c = self.col(),
                sp = q(SPACE),
            ))
            .await?;
        }
        Ok(())
    }

    /// The sealed column a blind-index backfill (B8) reads.
    fn sealed_col(&self) -> Option<String> {
        self.fill.indexing_from.as_deref().map(q)
    }

    /// B8: one row's blind index, from its sealed value as stored, under
    /// `lease` for its space. A null value indexes the null sentinel, as
    /// the write path does.
    fn index_of(
        &self,
        lease: &ordo_crypto::KeyLease,
        space: &str,
        id: &str,
        sealed: Option<&str>,
    ) -> Result<[u8; 32], BackfillRefusal> {
        let keys = self.keys.as_ref().ok_or_else(|| BackfillRefusal::Seal {
            space: space.to_string(),
            why: "this node holds no key custody".into(),
        })?;
        let plain = match sealed {
            None => None,
            Some(hexed) => {
                let bytes = unhex(hexed.strip_prefix("\\x").unwrap_or(hexed));
                let open = keys
                    .open_at(
                        lease,
                        &self.fill.entity,
                        &self.fill.field,
                        ordo_crypto::Anchor::Row(id),
                        &bytes,
                    )
                    .map_err(|e| BackfillRefusal::Seal {
                        space: space.to_string(),
                        why: format!("row `{id}` does not open: {e}"),
                    })?;
                Some(String::from_utf8_lossy(&open).into_owned())
            }
        };
        let normalize = self
            .entity
            .field(&self.fill.field)
            .and_then(|f| f.normalize.clone());
        let normalized = plain.map(|v| match normalize.as_deref() {
            Some("lower") => v.to_lowercase(),
            _ => v,
        });
        keys.blind_index(
            &ordo_crypto::SpaceId(space.to_string()),
            &self.fill.entity,
            &self.fill.field,
            normalized.as_deref().into(),
        )
        .map_err(|e| BackfillRefusal::Seal {
            space: space.to_string(),
            why: format!("blind index: {e}"),
        })
    }

    /// Increment 13, B8: each row's sealed value opened under its space's
    /// key and its blind index written. One lease per space per chunk.
    async fn index_chunk(
        &self,
        tx: &ordo_exec::TxConn,
        range: &str,
    ) -> Result<(), BackfillRefusal> {
        let Some(sealed) = self.sealed_col() else {
            return Err(BackfillRefusal::Store(
                "a blind-index backfill was stored without its sealed column; render the plan \
                 again"
                    .into(),
            ));
        };
        let rows = tx
            .rows(&format!(
                "SELECT {sp}, \"id\", {sealed}::text FROM {t} WHERE {range} AND {c} IS NULL \
                 ORDER BY {sp}, \"id\" FOR UPDATE",
                sp = q(SPACE),
                t = self.table(),
                c = self.col(),
            ))
            .await?;
        let mut lease: Option<(String, ordo_crypto::KeyLease)> = None;
        for r in rows {
            let (Some(space), Some(id)) =
                (r.first().cloned().flatten(), r.get(1).cloned().flatten())
            else {
                continue;
            };
            let value = r.get(2).cloned().flatten();
            if lease.as_ref().is_none_or(|(s, _)| *s != space) {
                lease = Some((space.clone(), self.lease(&space)?));
            }
            let (_, l) = lease.as_ref().expect("taken just above");
            let digest = self.index_of(l, &space, &id, value.as_deref())?;
            tx.rows(&format!(
                "UPDATE {t} SET {c} = '\\x{}'::bytea WHERE {sp} = {} AND \"id\" = {} AND {c} IS NULL",
                hex(&digest),
                sql_lit(&space),
                sql_lit(&id),
                t = self.table(),
                c = self.col(),
                sp = q(SPACE),
            ))
            .await?;
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
    ///
    /// A sealed backfill that fails verify starts over: every sealed value
    /// beside the plaintext is cleared with the cursor and the space
    /// hashes, so a run again seals each row's plaintext as it is then.
    pub async fn verify_and_close(&self) -> Result<Verified, BackfillRefusal> {
        self.verify(false).await
    }

    /// **Verify under live writes** (increment 11a): the tenant kept
    /// writing while the worker ran, and the move overlay kept each row's
    /// new column at the mapping. So a row added, or a value changed behind
    /// the cursor, is expected, and the per-chunk counts and hashes are not
    /// compared. What must hold is the plan's verify (§5): no row left null,
    /// and every row's new column equal to the mapping over its own
    /// columns. The record then carries the count and hash read now.
    pub async fn verify_live_and_close(&self) -> Result<Verified, BackfillRefusal> {
        self.verify(true).await
    }

    async fn verify(&self, live: bool) -> Result<Verified, BackfillRefusal> {
        let out = self.verify_inner(live).await;
        let sealed_kind = self.fill.sealing_from.is_some() || self.fill.indexing_from.is_some();
        if let (Err(BackfillRefusal::Verify { .. }), true) = (&out, sealed_kind) {
            let tx = self.pool.begin_for(OpClass::AddressedWrite).await?;
            tx.rows(&format!(
                "UPDATE {t} SET {c} = NULL WHERE {c} IS NOT NULL",
                t = self.table(),
                c = self.col()
            ))
            .await?;
            tx.rows(&format!(
                "DELETE FROM \"$backfill_spaces\" WHERE {}",
                self.key()
            ))
            .await?;
            tx.rows(&format!(
                "UPDATE \"$backfills\" SET \"cursor_space\" = NULL, \"cursor_id\" = NULL, \
                 \"rows_done\" = 0, \"state\" = 'running', \"updated_at\" = {} WHERE {}",
                now(),
                self.key()
            ))
            .await?;
            tx.commit().await?;
        }
        out
    }

    async fn verify_inner(&self, live: bool) -> Result<Verified, BackfillRefusal> {
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
        // An entity rename's copy: both tables hold the same rows, each
        // with the same columns. Writes during the copy kept both current.
        if let Some(from) = self.fill.copy_from() {
            let on = ordo_schema::ddl::physical_columns(self.entity)
                .iter()
                .map(|c| format!("o.{c} IS NOT DISTINCT FROM n.{c}", c = q(&c.name)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let off = self
                .pool
                .rows(&format!(
                    "SELECT coalesce(o.{sp}, n.{sp}), coalesce(o.\"id\", n.\"id\") \
                     FROM {o} o FULL JOIN {t} n ON o.{sp} = n.{sp} AND o.\"id\" = n.\"id\" \
                     WHERE o.\"id\" IS NULL OR n.\"id\" IS NULL OR NOT ({on}) LIMIT 1",
                    sp = q(SPACE),
                    o = q(from),
                    t = self.table(),
                ))
                .await?;
            if let Some(row) = first(off) {
                return Err(fail(
                    Some(row),
                    format!("is not the same in `{from}` and `{}`", self.fill.entity),
                ));
            }
        }
        // No row left null. A sealed backfill leaves a null plaintext null.
        let plain = self.plain_col();
        let sealed_src = self.sealed_col();
        let unfilled = match (&plain, self.fill.code, &self.fill.sql) {
            (Some(p), _, _) => format!("{} IS NULL AND {p} IS NOT NULL", self.col()),
            // A split's or a merge's new field (B12) may be optional, and a
            // row its mapping does not parse stays null: only a row the
            // mapping gives a value to must hold one. A required one's
            // unparsed rows are refused before the step (the pre-check).
            (None, "B12", Some(sql)) => format!("{} IS NULL AND ({sql}) IS NOT NULL", self.col()),
            (None, _, _) => format!("{} IS NULL", self.col()),
        };
        let nulls = if self.fill.copy_from().is_some() {
            Vec::new()
        } else {
            self.pool
                .rows(&format!(
                    "SELECT {sp}, \"id\" FROM {t} WHERE {unfilled} ORDER BY {sp}, \"id\" LIMIT 1",
                    sp = q(SPACE),
                    t = self.table(),
                ))
                .await?
        };
        if let Some(row) = first(nulls) {
            return Err(fail(Some(row), "has a row the backfill left null".into()));
        }
        // A new column (B2) filled by an expression holds the mapping on
        // every row. A B5 field kept what its rows already held.
        if let (FillMapping::Expr(_), Some(sql), "B2" | "B4" | "B12") =
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
        // A sealed backfill also opens every value, in memory, and compares
        // it with the row's plaintext: a value written after its chunk, or a
        // seal at the wrong row, fails here.
        let mut got: BTreeMap<String, (i64, [u8; 32])> = BTreeMap::new();
        let mut cursor: Option<(String, String)> = None;
        let mut lease: Option<(String, ordo_crypto::KeyLease)> = None;
        loop {
            let page = self
                .pool
                .rows(&format!(
                    "SELECT {sp}, \"id\", {v}{p} FROM {t} WHERE {a} ORDER BY {sp}, \"id\" \
                     LIMIT 5000",
                    sp = q(SPACE),
                    v = self.value(),
                    p = plain
                        .as_deref()
                        .map(|p| format!(", {p}"))
                        .or_else(|| sealed_src.as_deref().map(|p| format!(", {p}::text")))
                        .unwrap_or_default(),
                    t = self.table(),
                    a = Self::after(&cursor),
                ))
                .await?;
            if plain.is_some() {
                for r in &page {
                    let (Some(s), Some(id)) =
                        (r.first().cloned().flatten(), r.get(1).cloned().flatten())
                    else {
                        continue;
                    };
                    let sealed = r.get(2).cloned().flatten();
                    let text = r.get(3).cloned().flatten();
                    let opened = match &sealed {
                        None => None,
                        Some(hexed) => {
                            if lease.as_ref().is_none_or(|(x, _)| *x != s) {
                                lease = Some((s.clone(), self.lease(&s)?));
                            }
                            let (_, l) = lease.as_ref().expect("taken just above");
                            let bytes = unhex(hexed.strip_prefix("\\x").unwrap_or(hexed));
                            let keys = self.keys.as_ref().expect("a lease was taken");
                            let open = keys
                                .open_at(
                                    l,
                                    &self.fill.entity,
                                    &self.fill.field,
                                    ordo_crypto::Anchor::Row(&id),
                                    &bytes,
                                )
                                .map_err(|e| {
                                    fail(Some(format!("{s}/{id}")), format!("does not open: {e}"))
                                })?;
                            Some(String::from_utf8_lossy(&open).into_owned())
                        }
                    };
                    if opened != text {
                        return Err(fail(
                            Some(format!("{s}/{id}")),
                            "holds a sealed value other than the row's plaintext: it changed \
                             after its chunk"
                                .into(),
                        ));
                    }
                }
            }
            // A blind-index backfill (B8) computes every row's index again
            // from its sealed value and compares.
            if sealed_src.is_some() {
                for r in &page {
                    let (Some(s), Some(id)) =
                        (r.first().cloned().flatten(), r.get(1).cloned().flatten())
                    else {
                        continue;
                    };
                    if lease.as_ref().is_none_or(|(x, _)| *x != s) {
                        lease = Some((s.clone(), self.lease(&s)?));
                    }
                    let (_, l) = lease.as_ref().expect("taken just above");
                    let want = self.index_of(l, &s, &id, r.get(3).cloned().flatten().as_deref())?;
                    let held = r.get(2).cloned().flatten().unwrap_or_default();
                    if held.strip_prefix("\\x").unwrap_or(&held) != hex(&want) {
                        return Err(fail(
                            Some(format!("{s}/{id}")),
                            "holds a blind index other than its sealed value's: the value \
                             changed after its chunk"
                                .into(),
                        ));
                    }
                }
            }
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
        for (s, (n, h)) in got.iter().filter(|_| !live) {
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
        if let Some(s) = recorded.keys().find(|s| !live && !got.contains_key(*s)) {
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
