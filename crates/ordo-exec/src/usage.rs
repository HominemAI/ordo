//! 17n — the metering substrate's durable half: `$usage_replica` and
//! `$usage`, and the arithmetic `usage.flush` and `usage.rollup`
//! (`ordo-host`) drive over them.
//!
//! `plan-production-ops.md` §3.2a splits usage along durability, and this
//! module is that split written as two tables:
//!
//! - **`$usage_replica`** — per-replica quantities (requests by cost class,
//!   rows returned, worker runs) that exist only in one process's memory
//!   ([`ordo_host::metrics::Metrics::usage_take`]) until a flush writes
//!   them. Keyed by `(space, period_start, replica)`, so two replicas in
//!   one period are two rows that sum rather than one row a race can drop.
//! - **`$usage`** — per-space quantities that are already durable or are a
//!   measurement of the database: writes and rows written from the command
//!   log, bytes from `$sealedpayload`, `$blobref` and the live schema. One
//!   row per `(space, period_start)`, written by the leader once a period.
//!
//! Both live in the **control database** — they answer "what did this
//! deployment serve", which is a fact about the deployment and not about a
//! tenant's own database, so they are never routed through
//! [`crate::route::PoolResolver`].
//!
//! # Why usage is not a stream
//!
//! §3.2 gives three disqualifying reasons a third hash-chained stream would
//! be wrong here: it would take the per-space append lock on the exact axis
//! 5f already worries about, a usage total is an accounting quantity that
//! gets recomputed and back-filled rather than attested, and every count
//! this module writes is already visible where it happens. So there is no
//! chain here, no hash, and a write to either table is an ordinary
//! `UPSERT` a correction can simply repeat.
//!
//! # `command_count` reads the chain's own encoding, not `ordo-commandlog`
//!
//! `chain::decode_group` parses a whole stored body — every command, every
//! image, the trailer — to answer a question the rollup only needs the
//! first three fields for: correlation, actor, command count. Decoding the
//! rest of a group the rollup will never look at, over every body in a
//! window, is real work for nothing this table stores. So
//! [`command_count`] reads exactly as far as `chain::encode_group` writes
//! before it starts emitting commands — two length-prefixed strings, then
//! the `u64` count — and stops. `command_count_agrees_with_decode_group_on_
//! every_stored_body` (`ordo-host/tests/usage.rs`) is what keeps this a
//! second reading of one format rather than a second format.

use std::collections::BTreeMap;

use ordo_commandlog::{SpaceId, VERIFY_BATCH};

use crate::{sql_lit, Pool, PoolError};

/// The version this module's counting method is at, stored on every row so
/// a later change to what a column means (a different byte estimate, a
/// different write definition) is visible in the data rather than silently
/// blended into an existing series.
pub const METHOD_VERSION: &str = "ordo.usage/1";

/// `$usage_replica` and `$usage`. Joins `ensure_platform_tables`
/// (`crate::lib::ensure_platform_tables`) the way every other platform
/// table's DDL does.
pub const USAGE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$usage_replica\" ( \
  \"space\" text NOT NULL, \
  \"period_start\" bigint NOT NULL, \
  \"period_end\" bigint NOT NULL, \
  \"replica\" text NOT NULL, \
  \"requests\" jsonb NOT NULL, \
  \"rows_returned\" bigint NOT NULL, \
  \"worker_runs\" bigint NOT NULL, \
  \"state\" text NOT NULL, \
  \"method_version\" text NOT NULL, \
  PRIMARY KEY (\"space\", \"period_start\", \"replica\") \
); \
CREATE TABLE IF NOT EXISTS \"$usage\" ( \
  \"space\" text NOT NULL, \
  \"period_start\" bigint NOT NULL, \
  \"period_end\" bigint NOT NULL, \
  \"chain_from\" bigint NOT NULL, \
  \"chain_to\" bigint NOT NULL, \
  \"writes\" bigint NOT NULL, \
  \"rows_written\" bigint NOT NULL, \
  \"bytes_sealed\" bigint NOT NULL, \
  \"bytes_chain\" bigint NOT NULL, \
  \"bytes_blob\" bigint NOT NULL, \
  \"bytes_rows_estimated\" bigint NOT NULL, \
  \"method_version\" text NOT NULL, \
  PRIMARY KEY (\"space\", \"period_start\") \
); \
CREATE TABLE IF NOT EXISTS \"$usage_gaps\" ( \
  \"space\" text NOT NULL, \
  \"period_start\" bigint NOT NULL, \
  \"period_end\" bigint NOT NULL, \
  \"target\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"recorded_at\" bigint NOT NULL, \
  PRIMARY KEY (\"space\", \"period_start\") \
)";

/// A period `usage.rollup` could not measure, because the target holding
/// this space's chain could not be reached (18d R-O5, D-G).
///
/// **Written instead of a `$usage` row, never alongside one for the same
/// period.** The owner's own words on why a zero will not do: *"Writes,
/// rows written and bytes are a measurement of the tenant's database,
/// taken at rollup. If it cannot be read, the measurement did not happen,
/// and that period records a gap rather than a zero. A zero asserts
/// nothing happened, silently discounts the bill and hides the outage."*
/// A caller reading `$usage` for a gapped period therefore finds nothing —
/// the honest answer — and a caller that also reads this table finds out
/// why, rather than mistaking the silence for "not yet rolled up".
///
/// **The rollup does not need to remember it.** [`upsert_space`]'s window
/// continues from the *previous* `$usage` row's `chain_to`, which a gap
/// period never wrote, so the pass after a target comes back simply scans
/// the whole gap in one window — *"When the target returns the period can
/// be measured again, because the command log it measures is still
/// there."* This table exists for the operator, not for the rollup's own
/// bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub space: SpaceId,
    pub period_start: u64,
    pub period_end: u64,
    /// The target's own label (`route::target_label`), never a credential.
    pub target: String,
    /// The dial's own words. Unlike a caller-facing refusal, this never
    /// reaches anyone but an operator reading `$usage_gaps` directly, so
    /// there is no §9.1 R-C4.4 opaque-reason obligation here — R-C4.4 is
    /// about what a *request* is told, and a gap is diagnosed after the
    /// fact by someone who already holds the database.
    pub reason: String,
    /// When the rollup observed the gap, not when the period started.
    pub recorded_at: u64,
}

/// Record (or refresh) a gap. Last writer wins, [`crate::fleet::confirm`]'s
/// reasoning: a later pass over a still-open period is a fresher
/// observation of the same trouble, not a second event to keep beside the
/// first.
pub async fn record_gap(
    pool: &Pool,
    space: &str,
    period_start: u64,
    period_end: u64,
    target: &str,
    reason: &str,
    at: u64,
) -> Result<(), PoolError> {
    pool.rows(&format!(
        "INSERT INTO \"$usage_gaps\" \
           (\"space\", \"period_start\", \"period_end\", \"target\", \"reason\", \"recorded_at\") \
         VALUES ({space}, {period_start}, {period_end}, {target}, {reason}, {at}) \
         ON CONFLICT (\"space\", \"period_start\") DO UPDATE SET \
           \"period_end\" = EXCLUDED.\"period_end\", \
           \"target\" = EXCLUDED.\"target\", \
           \"reason\" = EXCLUDED.\"reason\", \
           \"recorded_at\" = EXCLUDED.\"recorded_at\"",
        space = sql_lit(space),
        period_start = period_start,
        period_end = period_end,
        target = sql_lit(target),
        reason = sql_lit(reason),
        at = at,
    ))
    .await?;
    Ok(())
}

/// Every gap recorded for a space with `period_start` in `[from, to)`,
/// ascending — the read `ordo usage` and an operator report merge with
/// [`between`]'s own rows.
pub async fn gaps_between(
    pool: &Pool,
    space: &str,
    from: u64,
    to: u64,
) -> Result<Vec<Gap>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"period_start\",\"period_end\",\"target\",\"reason\",\"recorded_at\" \
             FROM \"$usage_gaps\" WHERE \"space\" = {} AND \"period_start\" >= {from} \
             AND \"period_start\" < {to} ORDER BY \"period_start\"",
            sql_lit(space)
        ))
        .await?;
    Ok(rows.iter().filter_map(|r| gap_row_of(space, r)).collect())
}

fn gap_row_of(space: &str, r: &[Option<String>]) -> Option<Gap> {
    Some(Gap {
        space: SpaceId(space.to_string()),
        period_start: r.first()?.clone()?.parse().ok()?,
        period_end: r.get(1)?.clone()?.parse().ok()?,
        target: r.get(2)?.clone()?,
        reason: r.get(3)?.clone()?,
        recorded_at: r.get(4)?.clone()?.parse().ok()?,
    })
}

/// Every `$usage`/`$usage_replica` row plus every gap for a space in
/// `[from, to)`, rendered as one caller reads it. `ordo usage` and `GET
/// /usage` both call this rather than [`between`] and [`render`]
/// separately, so the two surfaces cannot come to disagree about whether a
/// gap is shown — the same one-function argument `route::target_label`
/// makes for the fleet view.
pub async fn render_between(
    pool: &Pool,
    space: &str,
    from: u64,
    to: u64,
) -> Result<String, PoolError> {
    let rows = between(pool, space, from, to).await?;
    let gaps = gaps_between(pool, space, from, to).await?;
    let mut out = render(space, rows);
    for g in &gaps {
        out.push_str(&render_gap(g));
    }
    Ok(out)
}

/// One JSONL record for a gap, in the same shape [`render`]'s records take
/// so a line-oriented reader of `ordo usage` does not need a second parser
/// — `"gap": true` is what tells the two apart, and a gap record carries no
/// `writes`, `bytes_*` or `chain_*` field, because it is not a reading of
/// any of them.
pub fn render_gap(g: &Gap) -> String {
    let record = serde_json::json!({
        "schema": METHOD_VERSION,
        "space": g.space.0,
        "period_start": g.period_start,
        "period_end": g.period_end,
        "gap": true,
        "target": g.target,
        "reason": g.reason,
        "recorded_at": g.recorded_at,
    });
    format!("{record}\n")
}

/// Where `[start, end)` of the period containing `now_secs` falls, for a
/// period `period_secs` long, aligned to the epoch so every replica and the
/// leader compute the same boundaries independently — no row anywhere
/// records "when this period started", because nothing has to.
///
/// `period_secs` is assumed to be at least 1; the two workers that call
/// this read their period from a validated, refusal-on-malformed
/// environment variable (`runtime.rs`), never from an unchecked value.
pub fn period_of(now_secs: u64, period_secs: u64) -> (u64, u64) {
    let start = now_secs - (now_secs % period_secs);
    (start, start + period_secs)
}

/// `'open'` while a replica may still add to a period; `'closed'` once it
/// has moved on and will never write that `(space, period_start, replica)`
/// row again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaState {
    Open,
    Closed,
}

impl ReplicaState {
    pub fn as_str(self) -> &'static str {
        match self {
            ReplicaState::Open => "open",
            ReplicaState::Closed => "closed",
        }
    }
}

/// One `(space, period_start, replica)` row — what one replica flushed for
/// one space in one period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaRow {
    pub space: SpaceId,
    pub period_start: u64,
    pub period_end: u64,
    pub replica: String,
    /// By `ordo_api::CostClass::as_str()`, e.g. `{"sql": 12, "bounded-host": 1}`.
    pub requests: BTreeMap<String, u64>,
    pub rows_returned: u64,
    pub worker_runs: u64,
    pub state: ReplicaState,
    pub method_version: String,
}

/// One `(space, period_start)` row — the leader's rollup of a period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceRow {
    pub space: SpaceId,
    pub period_start: u64,
    pub period_end: u64,
    /// The `$commandlog` window this row counted, `(chain_from, chain_to]`.
    pub chain_from: u64,
    pub chain_to: u64,
    pub writes: u64,
    pub rows_written: u64,
    pub bytes_sealed: u64,
    pub bytes_chain: u64,
    pub bytes_blob: u64,
    /// The one estimated column — see the module doc on
    /// `bytes_rows_estimated` at the call site (`ordo-host/src/workers.rs`).
    pub bytes_rows_estimated: u64,
    pub method_version: String,
}

/// Add one replica's counts for one period, merging into whatever is
/// already there.
///
/// `ON CONFLICT ... DO UPDATE` rather than an unconditional `INSERT`,
/// because a flush tick is not guaranteed to see exactly one period once:
/// a restarted worker re-entering a period it already wrote, or a
/// supervisor tick that ran twice before the period rolled over, must add
/// to the row rather than replace it or lose the first write's counts.
/// `requests` merges per cost class in one round trip (`jsonb_each_text`
/// on both sides, summed, re-aggregated) rather than a read-modify-write
/// from Rust, which would be a second statement and a window a concurrent
/// flush from the same replica could race.
pub async fn add_replica(pool: &Pool, row: &ReplicaRow) -> Result<(), PoolError> {
    let requests =
        serde_json::to_string(&row.requests).expect("a BTreeMap<String,u64> always serialises");
    pool.rows(&format!(
        "INSERT INTO \"$usage_replica\" \
           (\"space\", \"period_start\", \"period_end\", \"replica\", \"requests\", \
            \"rows_returned\", \"worker_runs\", \"state\", \"method_version\") \
         VALUES ({space}, {ps}, {pe}, {replica}, {requests}::jsonb, {rows}, {runs}, {state}, {mv}) \
         ON CONFLICT (\"space\", \"period_start\", \"replica\") DO UPDATE SET \
           \"period_end\" = EXCLUDED.\"period_end\", \
           \"requests\" = COALESCE(( \
             SELECT jsonb_object_agg(k, v) FROM ( \
               SELECT coalesce(o.key, n.key) AS k, \
                      coalesce((o.value)::bigint, 0) + coalesce((n.value)::bigint, 0) AS v \
               FROM jsonb_each_text(\"$usage_replica\".\"requests\") AS o \
               FULL JOIN jsonb_each_text(EXCLUDED.\"requests\") AS n ON o.key = n.key \
             ) merged \
           ), '{{}}'::jsonb), \
           \"rows_returned\" = \"$usage_replica\".\"rows_returned\" + EXCLUDED.\"rows_returned\", \
           \"worker_runs\" = \"$usage_replica\".\"worker_runs\" + EXCLUDED.\"worker_runs\", \
           \"state\" = EXCLUDED.\"state\", \
           \"method_version\" = EXCLUDED.\"method_version\"",
        space = sql_lit(&row.space.0),
        ps = row.period_start,
        pe = row.period_end,
        replica = sql_lit(&row.replica),
        requests = sql_lit(&requests),
        rows = row.rows_returned,
        runs = row.worker_runs,
        state = sql_lit(row.state.as_str()),
        mv = sql_lit(&row.method_version),
    ))
    .await?;
    Ok(())
}

/// Close this replica's still-`'open'` rows for every period that ended
/// before `before` — called with the current period's start, so "every
/// period this replica has moved past" closes in one statement. Returns how
/// many rows changed.
///
/// A row still `'open'` after its period ended, with no later flush ever
/// closing it, is a replica that died mid-period — recorded rather than
/// smoothed over, `EventKind::EventsDropped { lost }`'s posture.
pub async fn close_replica_periods(
    pool: &Pool,
    replica: &str,
    before: u64,
) -> Result<u64, PoolError> {
    let rows = pool
        .rows(&format!(
            "UPDATE \"$usage_replica\" SET \"state\" = 'closed' \
             WHERE \"replica\" = {replica} AND \"period_start\" < {before} AND \"state\" <> 'closed' \
             RETURNING \"space\"",
            replica = sql_lit(replica),
            before = before,
        ))
        .await?;
    Ok(rows.len() as u64)
}

/// Write (or overwrite) the leader's rollup for one `(space, period_start)`.
///
/// An unconditional replace, unlike [`add_replica`]: a rollup is
/// recomputed from `$commandlog` and the byte tables each time it runs, so
/// a second rollup of the same period is not a delta to merge, it is a
/// corrected reading that must win outright.
pub async fn upsert_space(pool: &Pool, row: &SpaceRow) -> Result<(), PoolError> {
    pool.rows(&format!(
        "INSERT INTO \"$usage\" \
           (\"space\", \"period_start\", \"period_end\", \"chain_from\", \"chain_to\", \
            \"writes\", \"rows_written\", \"bytes_sealed\", \"bytes_chain\", \"bytes_blob\", \
            \"bytes_rows_estimated\", \"method_version\") \
         VALUES ({space}, {ps}, {pe}, {cf}, {ct}, {w}, {rw}, {bs}, {bc}, {bb}, {bre}, {mv}) \
         ON CONFLICT (\"space\", \"period_start\") DO UPDATE SET \
           \"period_end\" = EXCLUDED.\"period_end\", \
           \"chain_from\" = EXCLUDED.\"chain_from\", \
           \"chain_to\" = EXCLUDED.\"chain_to\", \
           \"writes\" = EXCLUDED.\"writes\", \
           \"rows_written\" = EXCLUDED.\"rows_written\", \
           \"bytes_sealed\" = EXCLUDED.\"bytes_sealed\", \
           \"bytes_chain\" = EXCLUDED.\"bytes_chain\", \
           \"bytes_blob\" = EXCLUDED.\"bytes_blob\", \
           \"bytes_rows_estimated\" = EXCLUDED.\"bytes_rows_estimated\", \
           \"method_version\" = EXCLUDED.\"method_version\"",
        space = sql_lit(&row.space.0),
        ps = row.period_start,
        pe = row.period_end,
        cf = row.chain_from,
        ct = row.chain_to,
        w = row.writes,
        rw = row.rows_written,
        bs = row.bytes_sealed,
        bc = row.bytes_chain,
        bb = row.bytes_blob,
        bre = row.bytes_rows_estimated,
        mv = sql_lit(&row.method_version),
    ))
    .await?;
    Ok(())
}

/// The newest rollup for a space, or `None` for a space `usage.rollup` has
/// never rolled up — the rollup's own starting point, so the first pass
/// over a space counts from its earliest stored `idx` rather than from an
/// arbitrary zero.
pub async fn last_space_row(pool: &Pool, space: &str) -> Result<Option<SpaceRow>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {SPACE_COLS} FROM \"$usage\" WHERE \"space\" = {} \
             ORDER BY \"period_start\" DESC LIMIT 1",
            sql_lit(space)
        ))
        .await?;
    Ok(rows.first().and_then(|r| space_row_of(r)))
}

/// Every `$usage` and `$usage_replica` row for a space with
/// `period_start` in `[from, to)`, both ascending by `period_start` — the
/// read a reconciliation test or an operator report sums over.
pub async fn between(
    pool: &Pool,
    space: &str,
    from: u64,
    to: u64,
) -> Result<(Vec<SpaceRow>, Vec<ReplicaRow>), PoolError> {
    let space_lit = sql_lit(space);
    let usage = pool
        .rows(&format!(
            "SELECT {SPACE_COLS} FROM \"$usage\" \
             WHERE \"space\" = {space_lit} AND \"period_start\" >= {from} AND \"period_start\" < {to} \
             ORDER BY \"period_start\""
        ))
        .await?;
    let replica = pool
        .rows(&format!(
            "SELECT {REPLICA_COLS} FROM \"$usage_replica\" \
             WHERE \"space\" = {space_lit} AND \"period_start\" >= {from} AND \"period_start\" < {to} \
             ORDER BY \"period_start\", \"replica\""
        ))
        .await?;
    Ok((
        usage.iter().filter_map(|r| space_row_of(r)).collect(),
        replica.iter().filter_map(|r| replica_row_of(r)).collect(),
    ))
}

// ── the rollup's own reads (17n M1c) ─────────────────────────────────────
//
// Everything below reads `$commandlog`, `$sealedpayload`, `$blobref` and
// `pg_class` on a TARGET database — never the control database — because a
// space's rows live wherever `PoolResolver` routed it
// (`crate::route::PoolResolver`). `usage.rollup` (`ordo-host/src/workers.rs`)
// calls these once per target per pass.

/// Every space this database holds chain entries for, with its lowest and
/// highest stored `idx`. What `usage.rollup` starts its per-target pass
/// from — a space with no row here has never written, so it never appears.
pub async fn chain_heads(pool: &Pool) -> Result<Vec<(String, u64, u64)>, PoolError> {
    let rows = pool
        .rows(
            "SELECT \"space\", MIN(\"idx\"), MAX(\"idx\") FROM \"$commandlog\" GROUP BY \"space\"",
        )
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone()?,
                r.get(1)?.clone()?.parse().ok()?,
                r.get(2)?.clone()?.parse().ok()?,
            ))
        })
        .collect())
}

/// `writes` (the number of stored command groups) and `rows_written` (the
/// sum of [`command_count`] over their bodies) for one space, over
/// `["scan_from", "chain_to"]` inclusive on both ends — the caller decides
/// what `scan_from` means: the smallest stored `idx` on a space's first
/// rollup (nothing has been counted, so the lower bound is inclusive), or
/// the previous row's `chain_to + 1` on every rollup after (that position
/// was already counted, so the new window starts one past it). Both callers
/// land here as an inclusive `>=`, which is what keeps this one query
/// rather than two.
///
/// Read in [`ordo_commandlog::VERIFY_BATCH`] windows, the same chunk size
/// `prune::horizon_position` scans the chain in, for the same reason: a
/// space's whole history must not be one round trip.
pub async fn chain_window(
    pool: &Pool,
    space: &str,
    scan_from: u64,
    chain_to: u64,
) -> Result<(u64, u64), PoolError> {
    let mut writes = 0u64;
    let mut rows_written = 0u64;
    let mut from = scan_from;
    loop {
        if from > chain_to {
            return Ok((writes, rows_written));
        }
        let rows = pool
            .rows(&format!(
                "SELECT \"idx\", \"body\" FROM \"$commandlog\" WHERE \"space\" = {} \
                 AND \"idx\" >= {from} AND \"idx\" <= {chain_to} ORDER BY \"idx\" \
                 LIMIT {VERIFY_BATCH}",
                sql_lit(space)
            ))
            .await?;
        if rows.is_empty() {
            return Ok((writes, rows_written));
        }
        for r in &rows {
            writes += 1;
            let body = r.get(1).and_then(|c| c.as_ref()).and_then(|s| bytea_of(s));
            // A body that will not decode is not silently skipped: `writes`
            // still counts it (it is a real stored write), but a row count
            // this module cannot read is worth more than an undercount
            // nobody can see, so the pass fails loudly instead.
            let count = body.as_deref().and_then(command_count).ok_or_else(|| {
                PoolError::db(format!("{space}: a chain entry has no readable body"))
            })?;
            rows_written += count;
        }
        let last: u64 = rows
            .last()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| PoolError::Db("chain entry has no index".into()))?;
        if rows.len() < VERIFY_BATCH {
            return Ok((writes, rows_written));
        }
        from = last + 1;
    }
}

/// Decode Postgres's text rendering of `bytea` (`\x0a1b…`). A private copy
/// of `crate::commandlog::bytea_of` — that one is public for callers
/// reading `$commandlog` bodies directly, and this module already imports
/// nothing else from `commandlog` to avoid pulling in the append path for
/// a read-only rollup.
fn bytea_of(cell: &str) -> Option<Vec<u8>> {
    let text = cell.strip_prefix("\\x")?;
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// `SUM(octet_length("payload"))` over `$sealedpayload` for one space.
pub async fn bytes_sealed(pool: &Pool, space: &str) -> Result<u64, PoolError> {
    sum_u64(
        pool,
        &format!(
            "SELECT COALESCE(SUM(octet_length(\"payload\")), 0) FROM \"$sealedpayload\" \
             WHERE \"$space\" = {}",
            sql_lit(space)
        ),
    )
    .await
}

/// `SUM(octet_length("body"))` over `$commandlog` for one space.
pub async fn bytes_chain(pool: &Pool, space: &str) -> Result<u64, PoolError> {
    sum_u64(
        pool,
        &format!(
            "SELECT COALESCE(SUM(octet_length(\"body\")), 0) FROM \"$commandlog\" \
             WHERE \"space\" = {}",
            sql_lit(space)
        ),
    )
    .await
}

/// `SUM("bytes")` over `$blobref` for one space — exact, not estimated:
/// `$blobref.bytes` is a plaintext `bigint` column (§3.1's re-reading), so
/// this is a free `SUM`, unlike [`table_bytes_estimate`] below.
pub async fn bytes_blob(pool: &Pool, space: &str) -> Result<u64, PoolError> {
    sum_u64(
        pool,
        &format!(
            "SELECT COALESCE(SUM(\"bytes\"), 0) FROM \"$blobref\" WHERE \"$space\" = {}",
            sql_lit(space)
        ),
    )
    .await
}

async fn sum_u64(pool: &Pool, sql: &str) -> Result<u64, PoolError> {
    let rows = pool.rows(sql).await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// One entity table's estimated byte footprint for one space: its row
/// count times the table's mean row width
/// (`pg_relation_size / greatest(reltuples, 1)`, from `pg_class`). The one
/// estimated column in `$usage` — every other byte figure here is an exact
/// `SUM`.
///
/// `table` is a live schema's entity name (`ordo_schema::Entity::name`),
/// never caller input: it is interpolated as an identifier, the way every
/// other per-entity statement in this crate reaches its own table
/// (`sealed.rs`, `triggers.rs`).
pub async fn table_bytes_estimate(pool: &Pool, table: &str, space: &str) -> Result<u64, PoolError> {
    let table_lit = sql_lit(table);
    let rows = pool
        .rows(&format!(
            "SELECT count(*)::bigint, \
                    (SELECT COALESCE(pg_relation_size(oid), 0) FROM pg_class \
                     WHERE relname = {table_lit}), \
                    GREATEST((SELECT COALESCE(reltuples, 1) FROM pg_class \
                     WHERE relname = {table_lit}), 1) \
             FROM \"{table}\" WHERE \"$space\" = {}",
            sql_lit(space)
        ))
        .await?;
    let Some(r) = rows.first() else {
        return Ok(0);
    };
    let count: f64 = r
        .first()
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let relsize: f64 = r
        .get(1)
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let reltuples: f64 = r
        .get(2)
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(1.0)
        .max(1.0);
    Ok((count * (relsize / reltuples)).round() as u64)
}

const SPACE_COLS: &str = "\"space\",\"period_start\",\"period_end\",\"chain_from\",\"chain_to\",\
\"writes\",\"rows_written\",\"bytes_sealed\",\"bytes_chain\",\"bytes_blob\",\
\"bytes_rows_estimated\",\"method_version\"";

const REPLICA_COLS: &str = "\"space\",\"period_start\",\"period_end\",\"replica\",\"requests\",\
\"rows_returned\",\"worker_runs\",\"state\",\"method_version\"";

fn space_row_of(r: &[Option<String>]) -> Option<SpaceRow> {
    Some(SpaceRow {
        space: SpaceId(r.first()?.clone()?),
        period_start: r.get(1)?.clone()?.parse().ok()?,
        period_end: r.get(2)?.clone()?.parse().ok()?,
        chain_from: r.get(3)?.clone()?.parse().ok()?,
        chain_to: r.get(4)?.clone()?.parse().ok()?,
        writes: r.get(5)?.clone()?.parse().ok()?,
        rows_written: r.get(6)?.clone()?.parse().ok()?,
        bytes_sealed: r.get(7)?.clone()?.parse().ok()?,
        bytes_chain: r.get(8)?.clone()?.parse().ok()?,
        bytes_blob: r.get(9)?.clone()?.parse().ok()?,
        bytes_rows_estimated: r.get(10)?.clone()?.parse().ok()?,
        method_version: r.get(11)?.clone()?,
    })
}

fn replica_row_of(r: &[Option<String>]) -> Option<ReplicaRow> {
    let requests: BTreeMap<String, u64> = serde_json::from_str(r.get(4)?.as_deref()?).ok()?;
    Some(ReplicaRow {
        space: SpaceId(r.first()?.clone()?),
        period_start: r.get(1)?.clone()?.parse().ok()?,
        period_end: r.get(2)?.clone()?.parse().ok()?,
        replica: r.get(3)?.clone()?,
        requests,
        rows_returned: r.get(5)?.clone()?.parse().ok()?,
        worker_runs: r.get(6)?.clone()?.parse().ok()?,
        state: match r.get(7)?.as_deref()? {
            "open" => ReplicaState::Open,
            _ => ReplicaState::Closed,
        },
        method_version: r.get(8)?.clone()?,
    })
}

// ── rendering (17n M3) ───────────────────────────────────────────────────

/// One JSONL record per `(space, period_start)` present in `rows`, in
/// ascending `period_start` order — `ordo usage` and the `/usage` route
/// both render through this, so the CLI and the wire say the same thing
/// for the same period. `rows` is [`between`]'s own return shape, so the
/// caller never re-derives it.
///
/// **Keys are alphabetical, for free.** `serde_json::Map` is a `BTreeMap`
/// under this workspace's default features (`preserve_order` is enabled
/// nowhere in the tree), so `Value::to_string` already sorts, and two runs
/// over the same closed period produce byte-identical output without this
/// function hand-sorting anything — a record is a fact about a period, not
/// a live reading, and two facts about the same period must print the
/// same bytes.
///
/// **A period with replica rows and no space row yet still gets a
/// record.** `usage.rollup` is leader-only and on its own cadence, so a
/// caller polling `[from, to)` can legitimately see a period this
/// replica's `usage.flush` has already written and the rollup has not
/// reached: `replicas` and `totals` are populated, and the space-row
/// columns (`chain_from`, `writes`, the byte counts, …) are simply absent
/// rather than the period being dropped or padded with zeros that would
/// read as "nothing happened here".
pub fn render(space: &str, rows: (Vec<SpaceRow>, Vec<ReplicaRow>)) -> String {
    #[derive(Default)]
    struct Period {
        space_row: Option<SpaceRow>,
        replicas: Vec<ReplicaRow>,
    }
    let (space_rows, replica_rows) = rows;
    let mut periods: BTreeMap<u64, Period> = BTreeMap::new();
    for r in space_rows {
        let period_start = r.period_start;
        periods.entry(period_start).or_default().space_row = Some(r);
    }
    for r in replica_rows {
        let period_start = r.period_start;
        periods.entry(period_start).or_default().replicas.push(r);
    }

    let mut out = String::new();
    for (period_start, mut p) in periods {
        p.replicas.sort_by(|a, b| a.replica.cmp(&b.replica));

        let mut totals_requests: BTreeMap<String, u64> = BTreeMap::new();
        let mut totals_rows_returned = 0u64;
        let mut totals_worker_runs = 0u64;
        let replicas: Vec<serde_json::Value> = p
            .replicas
            .iter()
            .map(|r| {
                for (k, v) in &r.requests {
                    *totals_requests.entry(k.clone()).or_insert(0) += v;
                }
                totals_rows_returned += r.rows_returned;
                totals_worker_runs += r.worker_runs;
                serde_json::json!({
                    "replica": r.replica,
                    "requests": r.requests,
                    "rows_returned": r.rows_returned,
                    "worker_runs": r.worker_runs,
                    "state": r.state.as_str(),
                })
            })
            .collect();

        // The counting method's own version, from the stored row rather
        // than this build's constant: a row written before a method change
        // must keep saying what actually counted it.
        let method_version = p
            .space_row
            .as_ref()
            .map(|r| r.method_version.clone())
            .or_else(|| p.replicas.first().map(|r| r.method_version.clone()))
            .unwrap_or_else(|| METHOD_VERSION.to_string());
        let period_end = p
            .space_row
            .as_ref()
            .map(|r| r.period_end)
            .or_else(|| p.replicas.first().map(|r| r.period_end))
            .unwrap_or(period_start);

        let mut record = serde_json::json!({
            "schema": METHOD_VERSION,
            "space": space,
            "period_start": period_start,
            "period_end": period_end,
            "method_version": method_version,
            "replicas": replicas,
            "totals": {
                "requests": totals_requests,
                "rows_returned": totals_rows_returned,
                "worker_runs": totals_worker_runs,
            },
        });
        if let Some(sr) = &p.space_row {
            let obj = record
                .as_object_mut()
                .expect("json! always builds an object here");
            obj.insert("chain_from".into(), serde_json::json!(sr.chain_from));
            obj.insert("chain_to".into(), serde_json::json!(sr.chain_to));
            obj.insert("writes".into(), serde_json::json!(sr.writes));
            obj.insert("rows_written".into(), serde_json::json!(sr.rows_written));
            obj.insert("bytes_sealed".into(), serde_json::json!(sr.bytes_sealed));
            obj.insert("bytes_chain".into(), serde_json::json!(sr.bytes_chain));
            obj.insert("bytes_blob".into(), serde_json::json!(sr.bytes_blob));
            obj.insert(
                "bytes_rows_estimated".into(),
                serde_json::json!(sr.bytes_rows_estimated),
            );
        }
        out.push_str(&record.to_string());
        out.push('\n');
    }
    out
}

/// How many commands a stored `$commandlog.body` encodes, read without
/// decoding the rest of the group — see the module doc.
///
/// `encode_group` (`ordo-commandlog/src/chain.rs`) writes, in order: the
/// correlation id as an 8-byte big-endian length then that many bytes; the
/// actor string the same way; then the command count as a bare 8-byte
/// big-endian `u64`. This walks exactly that prefix and stops before the
/// first command. `None` for anything truncated or overflowing — never a
/// panic on a corrupt or foreign body, because a rollup pass must not be
/// crashable by one bad row.
pub fn command_count(body: &[u8]) -> Option<u64> {
    let mut at = 0usize;
    for _ in 0..2 {
        let len = read_u64(body, at)? as usize;
        at = at.checked_add(8)?.checked_add(len)?;
        if at > body.len() {
            return None;
        }
    }
    read_u64(body, at)
}

fn read_u64(body: &[u8], at: usize) -> Option<u64> {
    let end = at.checked_add(8)?;
    let slice = body.get(at..end)?;
    Some(u64::from_be_bytes(slice.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_of_aligns_to_the_epoch_regardless_of_where_now_falls_in_it() {
        assert_eq!(period_of(0, 60), (0, 60));
        assert_eq!(period_of(59, 60), (0, 60));
        assert_eq!(period_of(60, 60), (60, 120));
        assert_eq!(period_of(125, 60), (120, 180));
        assert_eq!(period_of(3_599, 3_600), (0, 3_600));
        assert_eq!(period_of(3_600, 3_600), (3_600, 7_200));
    }

    /// The exact shape `encode_group` writes: two length-prefixed strings,
    /// then a bare `u64` count, before the first command's own bytes.
    #[test]
    fn command_count_reads_the_count_after_two_strings() {
        let mut body = Vec::new();
        body.extend_from_slice(&5u64.to_be_bytes());
        body.extend_from_slice(b"cg_01");
        body.extend_from_slice(&(4u64).to_be_bytes());
        body.extend_from_slice(b"user");
        body.extend_from_slice(&3u64.to_be_bytes());
        body.extend_from_slice(b"whatever commands would go here");
        assert_eq!(command_count(&body), Some(3));
    }

    #[test]
    fn command_count_refuses_a_truncated_body_rather_than_panicking() {
        assert_eq!(command_count(&[]), None);
        assert_eq!(command_count(&0u64.to_be_bytes()), None);
        // A length prefix past the end of the buffer.
        let mut body = Vec::new();
        body.extend_from_slice(&1_000u64.to_be_bytes());
        body.extend_from_slice(b"x");
        assert_eq!(command_count(&body), None);
    }

    fn space_row(period_start: u64) -> SpaceRow {
        SpaceRow {
            space: SpaceId("spc_render_test".into()),
            period_start,
            period_end: period_start + 3_600,
            chain_from: 1,
            chain_to: 4,
            writes: 3,
            rows_written: 7,
            bytes_sealed: 100,
            bytes_chain: 200,
            bytes_blob: 300,
            bytes_rows_estimated: 400,
            method_version: METHOD_VERSION.to_string(),
        }
    }

    fn replica_row(period_start: u64, replica: &str) -> ReplicaRow {
        ReplicaRow {
            space: SpaceId("spc_render_test".into()),
            period_start,
            period_end: period_start + 60,
            replica: replica.to_string(),
            requests: [("sql".to_string(), 5u64)].into_iter().collect(),
            rows_returned: 12,
            worker_runs: 1,
            state: ReplicaState::Closed,
            method_version: METHOD_VERSION.to_string(),
        }
    }

    /// A record is a fact about a period: two calls over the same rows must
    /// print the identical bytes, which is the whole property `ordo usage`
    /// and `/usage` need for a caller to diff two runs.
    #[test]
    fn render_is_byte_identical_across_two_calls() {
        let rows = (
            vec![space_row(0)],
            vec![replica_row(0, "r2"), replica_row(0, "r1")],
        );
        let a = render("spc_render_test", rows.clone());
        let b = render("spc_render_test", rows);
        assert_eq!(a, b);
        assert_eq!(a.lines().count(), 1);
        assert!(a.contains(r#""replica":"r1""#));
        // r1 sorts before r2 in the rendered array, not the input order.
        assert!(a.find(r#""replica":"r1""#) < a.find(r#""replica":"r2""#));
    }

    /// A period `usage.flush` wrote and `usage.rollup` has not reached yet
    /// still gets a record, with the space-row columns simply absent.
    #[test]
    fn render_includes_a_replica_only_period() {
        let out = render("spc_render_test", (vec![], vec![replica_row(60, "r1")]));
        let v: serde_json::Value = serde_json::from_str(out.trim()).expect("one JSON record");
        assert_eq!(v["period_start"], 60);
        assert!(v.get("chain_from").is_none(), "{v}");
        assert_eq!(v["totals"]["rows_returned"], 12);
    }

    /// Keys sorted, `schema` and `method_version` present, `totals` summed
    /// across two replicas.
    #[test]
    fn render_shape_matches_the_plan() {
        let rows = (
            vec![space_row(0)],
            vec![replica_row(0, "r1"), replica_row(0, "r2")],
        );
        let out = render("spc_render_test", rows);
        let v: serde_json::Value = serde_json::from_str(out.trim()).expect("one JSON record");
        assert_eq!(v["schema"], METHOD_VERSION);
        assert_eq!(v["space"], "spc_render_test");
        assert_eq!(v["method_version"], METHOD_VERSION);
        assert_eq!(v["totals"]["rows_returned"], 24);
        assert_eq!(v["totals"]["worker_runs"], 2);
        assert_eq!(v["totals"]["requests"]["sql"], 10);
        assert_eq!(v["replicas"].as_array().unwrap().len(), 2);
        assert_eq!(v["writes"], 3);
    }
}
