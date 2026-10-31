//! The migration probes — increment 1 of `plan-schema-evolution.md`.
//!
//! `nfc.rs` is the precedent and this is it generalised. Its rule is the
//! whole design: **report and never repair**, because *"which row survives
//! is the deployment's decision, and a probe that took it would be making it
//! for them."* Everything here reads. Nothing here writes, ever — not a
//! temp table, not a `SET`, not an `ANALYZE`.
//!
//! # What a probe is for
//!
//! §5's rule for every constraint class is that *the declaration is not the
//! question; the live data is*. `unique (a, b)` is free on an empty table,
//! a refusal on a table holding duplicates, and the difference is not
//! visible anywhere in the program. A migration that discovers it by hitting
//! the constraint finds out mid-write with half the table converted; these
//! find out first, at `ordo plan` time, over an indexed scan.
//!
//! # One round trip per question
//!
//! Each function asks the server one question and returns its answer. The
//! alternative — stream the rows into the process and decide in Rust — reads
//! every row of every column of a live tenant database into an auditing
//! tool, which is both slower and a disclosure nobody consented to.
//!
//! # The null rule is Ordo-specific, and [`duplicates`] gets it for free
//!
//! DL's `null == null` is TRUE, so `ddl_for` emits `UNIQUE NULLS NOT
//! DISTINCT` and a nullable unique column admits **at most one null row per
//! space**. A table with 500 nulls in a column cannot take a `unique` that
//! every other SQL system would accept. `GROUP BY` treats nulls as equal, so
//! the duplicate probe reports that case without a special path — and
//! [`Duplicate::value`] is `None` for it, which is how the report knows to
//! say the surprising thing in the words §5's C1 note asks for.

use std::time::Instant;

use ordo_schema::ddl::q;
use ordo_schema::{LiveColumn, LiveShape};

use crate::{Pool, PoolError};

/// The tenancy column, which leads every generated unique index and is
/// therefore part of every question a probe asks.
const SPACE: &str = "$space";

// ── reading the live shape ───────────────────────────────────────────────

/// Read one schema's columns, constraints and indexes into the shape
/// `ordo_schema::diff` classifies against.
///
/// `format_type` rather than `information_schema.columns.data_type` for the
/// same reason `observed_shape` uses it: it is the server's own canonical
/// spelling of the type, including the typmod, so `numeric(10,2)` and
/// `numeric` do not read as one type.
pub async fn live_shape(pool: &Pool, schema: &str) -> Result<LiveShape, PoolError> {
    let lit = crate::sql_lit(schema);
    let mut out = LiveShape::default();

    let rows = pool
        .rows(&format!(
            "SELECT t.relname, a.attname, format_type(a.atttypid, a.atttypmod), \
                    NOT a.attnotnull, a.atthasdef OR a.atthasmissing \
             FROM pg_attribute a \
             JOIN pg_class t ON t.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             WHERE n.nspname = {lit} AND t.relkind = 'r' \
               AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY t.relname, a.attnum"
        ))
        .await?;
    for r in rows {
        let g = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        let (Some(Some(table)), Some(Some(column))) = (r.first().cloned(), r.get(1).cloned())
        else {
            continue;
        };
        out.column(
            &table,
            &column,
            LiveColumn {
                ty: g(2),
                nullable: g(3) == "t",
                has_default: g(4) == "t",
            },
        );
    }

    // Constraints and indexes, by name. A table with no columns cannot
    // exist, so every table these name is already in the map.
    let rows = pool
        .rows(&format!(
            "SELECT t.relname, c.conname, 'c' FROM pg_constraint c \
             JOIN pg_class t ON t.oid = c.conrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             WHERE n.nspname = {lit} AND t.relkind = 'r' \
             UNION ALL \
             SELECT t.relname, i.relname, 'i' FROM pg_index x \
             JOIN pg_class i ON i.oid = x.indexrelid \
             JOIN pg_class t ON t.oid = x.indrelid \
             JOIN pg_namespace n ON n.oid = i.relnamespace \
             WHERE n.nspname = {lit} AND t.relkind = 'r' \
               AND NOT EXISTS (SELECT 1 FROM pg_constraint c WHERE c.conindid = x.indexrelid) \
             ORDER BY 1, 2"
        ))
        .await?;
    for r in rows {
        let (Some(Some(table)), Some(Some(name)), Some(Some(kind))) =
            (r.first().cloned(), r.get(1).cloned(), r.get(2).cloned())
        else {
            continue;
        };
        out.object(&table, &name, kind == "c");
    }
    out.places = held_places_in(pool, schema).await?;
    Ok(out)
}

// ── the places stored amounts are held at (25aa) ─────────────────────────

/// The log of places changes applied to this database: one row per
/// `money` or `decimal` field whose places an apply changed, written in the
/// same transaction as the change itself, so the row and the amounts can
/// never disagree.
///
/// Every `money(p)` is `bigint` minor units, so the column type cannot say
/// what places its amounts are at. Without this, an apply that rescaled the
/// amounts and then failed to write its `$migrations` row would leave the
/// next plan proposing the same rescale again.
///
/// `choice` is what the developer stated (`rescale` or `keep`); a tenant's
/// database brought to the same version later reads it from the
/// deployment's own database and gets the same answer.
///
/// # What a revert reads here (25ad, ruled 2026-10-06)
///
/// A revert re-applies an image from the history, and an image taken before
/// a money rescale holds the old minor units. Three columns let it convert:
///
/// - `from_places`: the places before this change, so a rescale's factor is
///   `10^(places - from_places)` without walking earlier rows.
/// - `money`: whether the field is `money` (a bigint of minor units, which a
///   rescale rewrites and whose `rev` it moves) or `decimal` (stored as its
///   value, so nothing to convert).
/// - `heads`: each space's chain head (`max(idx)` in `$commandlog`) at the
///   moment of the change, as a JSON object. A group at `idx <= heads[space]`
///   was committed before the change, so its images are at the old places.
///   Taken inside the apply's transaction after the table lock, so no write
///   to the entity can fall between the count and the rewrite. A space with
///   no key had no group yet. `NULL` is a row written before this column
///   existed, which a revert refuses to guess about.
///
/// Added by `ALTER TABLE … ADD COLUMN IF NOT EXISTS`, so a table created
/// before them gains them. Nothing has deployed, so no older row is owed a
/// value.
pub const PLACES_DDL: &str = "CREATE TABLE IF NOT EXISTS \"$places\" (\
     \"seq\" bigserial PRIMARY KEY, \
     \"entity\" text NOT NULL, \
     \"field\" text NOT NULL, \
     \"places\" integer NOT NULL, \
     \"choice\" text NOT NULL, \
     \"version\" text NOT NULL, \
     \"at\" bigint NOT NULL);\n\
     ALTER TABLE \"$places\" ADD COLUMN IF NOT EXISTS \"from_places\" integer, \
     ADD COLUMN IF NOT EXISTS \"money\" boolean, \
     ADD COLUMN IF NOT EXISTS \"heads\" jsonb;\n";

/// One `$places` row, as the apply writes it. Rendered as a statement so it
/// can ride the apply's own transaction.
///
/// A `DO` block so the chain heads are read in the same statement that
/// writes the row, and so a database with no `$commandlog` (nothing written
/// yet) records `{}` rather than failing on a table that is not there.
#[allow(clippy::too_many_arguments)]
pub fn places_insert(
    entity: &str,
    field: &str,
    from: u8,
    places: u8,
    money: bool,
    choice: &str,
    version: &str,
    at: u64,
) -> String {
    format!(
        "DO $ordo$ DECLARE h jsonb := '{{}}'::jsonb; BEGIN \
         IF to_regclass('\"$commandlog\"') IS NOT NULL THEN \
         EXECUTE 'SELECT COALESCE(jsonb_object_agg(\"space\", m), ''{{}}''::jsonb) FROM \
         (SELECT \"space\", max(\"idx\") AS m FROM \"$commandlog\" GROUP BY \"space\") x' \
         INTO h; END IF; \
         INSERT INTO \"$places\" (\"entity\", \"field\", \"places\", \"choice\", \"version\", \
         \"at\", \"from_places\", \"money\", \"heads\") \
         VALUES ({}, {}, {places}, {}, {}, {at}, {from}, {money}, h); END $ordo$;\n",
        crate::sql_lit(entity),
        crate::sql_lit(field),
        crate::sql_lit(choice),
        crate::sql_lit(version),
    )
}

/// One money rescale applied to this database, as a revert needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rescale {
    pub entity: String,
    pub field: String,
    pub from: u8,
    pub to: u8,
    /// `rescale` or `keep`.
    pub choice: String,
    /// This space's chain head when it was applied: `None` when the space
    /// had no group yet. `Err(())` when the row predates the column and
    /// cannot be placed against the chain.
    pub head: Result<Option<i64>, ()>,
}

/// Every change of places on a **money** field this database recorded,
/// oldest first, with `space`'s chain head at each. Empty when no places
/// change was ever applied. Read through `rows`, so a revert can ask inside
/// its own transaction.
pub async fn money_places_history<F, Fut>(rows: F, space: &str) -> Result<Vec<Rescale>, PoolError>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<crate::TextRow>, PoolError>>,
{
    let present = rows("SELECT to_regclass('\"$places\"') IS NOT NULL".to_string()).await?;
    if present
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        != Some("t")
    {
        return Ok(Vec::new());
    }
    let got = rows(format!(
        "SELECT \"entity\", \"field\", \"from_places\", \"places\", \"choice\", \
                \"heads\" IS NULL, \"heads\" ->> {} \
         FROM \"$places\" WHERE \"money\" IS NOT FALSE ORDER BY \"seq\"",
        crate::sql_lit(space)
    ))
    .await?;
    let mut out = Vec::new();
    for r in got {
        let g = |i: usize| r.get(i).cloned().flatten();
        let (Some(entity), Some(field), Some(Ok(to)), Some(choice)) =
            (g(0), g(1), g(3).map(|p| p.parse::<u8>()), g(4))
        else {
            continue;
        };
        // A row with no `from_places` predates the column: its factor is
        // unknown, and so is its place in the chain.
        let from = g(2).and_then(|p| p.parse::<u8>().ok());
        let head = match (g(5).as_deref(), g(6)) {
            (Some("t"), _) => Err(()),
            (_, None) => Ok(None),
            (_, Some(idx)) => idx.parse::<i64>().map(Some).map_err(|_| ()),
        };
        let (from, head) = match from {
            Some(f) => (f, head),
            None => (to, Err(())),
        };
        out.push(Rescale {
            entity,
            field,
            from,
            to,
            choice,
            head,
        });
    }
    Ok(out)
}

/// The places each recorded field's stored amounts are held at now: the
/// latest `$places` row per `(entity, field)`. Empty when no places change
/// was ever applied here.
pub async fn held_places(
    pool: &Pool,
) -> Result<std::collections::BTreeMap<(String, String), u8>, PoolError> {
    held_places_in(pool, "public").await
}

async fn held_places_in(
    pool: &Pool,
    schema: &str,
) -> Result<std::collections::BTreeMap<(String, String), u8>, PoolError> {
    let mut out = std::collections::BTreeMap::new();
    let table = format!("{}.\"$places\"", ordo_schema::ddl::q(schema));
    if !places_table_exists(pool, &table).await? {
        return Ok(out);
    }
    let rows = pool
        .rows(&format!(
            "SELECT DISTINCT ON (\"entity\", \"field\") \"entity\", \"field\", \"places\" \
             FROM {table} ORDER BY \"entity\", \"field\", \"seq\" DESC"
        ))
        .await?;
    for r in rows {
        let g = |i: usize| r.get(i).cloned().flatten();
        if let (Some(e), Some(f), Some(Ok(p))) = (g(0), g(1), g(2).map(|p| p.parse::<u8>())) {
            out.insert((e, f), p);
        }
    }
    Ok(out)
}

/// The choice last stated for each `(entity, field, places)` this database
/// moved a field to. A tenant's database reads the deployment's, so a
/// places change is applied to every tenant the way it was stated once.
pub async fn stated_places(
    pool: &Pool,
) -> Result<std::collections::BTreeMap<(String, String, u8), String>, PoolError> {
    let mut out = std::collections::BTreeMap::new();
    if !places_table_exists(pool, "public.\"$places\"").await? {
        return Ok(out);
    }
    let rows = pool
        .rows(
            "SELECT DISTINCT ON (\"entity\", \"field\", \"places\") \
                    \"entity\", \"field\", \"places\", \"choice\" \
             FROM public.\"$places\" ORDER BY \"entity\", \"field\", \"places\", \"seq\" DESC",
        )
        .await?;
    for r in rows {
        let g = |i: usize| r.get(i).cloned().flatten();
        if let (Some(e), Some(f), Some(Ok(p)), Some(c)) =
            (g(0), g(1), g(2).map(|p| p.parse::<u8>()), g(3))
        {
            out.insert((e, f, p), c);
        }
    }
    Ok(out)
}

async fn places_table_exists(pool: &Pool, qualified: &str) -> Result<bool, PoolError> {
    let present = pool
        .rows(&format!(
            "SELECT to_regclass({}) IS NOT NULL",
            crate::sql_lit(qualified)
        ))
        .await?;
    Ok(present
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        == Some("t"))
}

// ── the probes ───────────────────────────────────────────────────────────

/// A value two or more rows share, inside one space, in a column that is
/// about to forbid sharing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub space: String,
    /// `None` when the shared value is NULL — see the module doc. Under
    /// `NULLS NOT DISTINCT` this is a violation like any other, and it is
    /// the one operators do not expect.
    pub value: Option<String>,
    pub rows: i64,
}

/// Rows that would violate a `unique` the program declares and the database
/// does not yet hold.
///
/// `columns` is the tuple, in the order the constraint names them; the space
/// is prepended here because it leads every generated unique index without
/// exception (`ddl.rs`) and a probe that forgot it would report cross-tenant
/// collisions that the constraint permits.
///
/// The sample is capped: an operator needs to know a duplicate exists and
/// see enough to recognise it, and a probe that streams ten million
/// offending values into a terminal has answered a different question.
pub async fn duplicates(
    pool: &Pool,
    table: &str,
    columns: &[String],
    limit: usize,
) -> Result<Vec<Duplicate>, PoolError> {
    if columns.is_empty() {
        return Ok(Vec::new());
    }
    let cols: Vec<String> = columns.iter().map(|c| q(c)).collect();
    // The tuple is rendered as text for reporting only; the GROUPING is on
    // the columns themselves, so the server's own equality — including its
    // null handling — decides what collides, not a string rendering of it.
    let render = if cols.len() == 1 {
        cols[0].clone()
    } else {
        format!("({})::text", cols.join(", "))
    };
    let sql = format!(
        "SELECT {space}, {render}, count(*) FROM {t} \
         GROUP BY {space}, {group} HAVING count(*) > 1 \
         ORDER BY count(*) DESC, 1 LIMIT {limit}",
        space = q(SPACE),
        t = q(table),
        group = cols.join(", "),
    );
    Ok(pool
        .rows(&sql)
        .await?
        .into_iter()
        .map(|r| Duplicate {
            space: r.first().cloned().flatten().unwrap_or_default(),
            value: r.get(1).cloned().flatten(),
            rows: num(r.get(2)),
        })
        .collect())
}

/// Rows holding NULL in a column that is about to become `NOT NULL`.
///
/// Per space, because a backfill runs per space and an operator's question
/// is which tenants are affected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NullCount {
    pub space: String,
    pub rows: i64,
}

pub async fn nulls(pool: &Pool, table: &str, column: &str) -> Result<Vec<NullCount>, PoolError> {
    let sql = format!(
        "SELECT {space}, count(*) FROM {t} WHERE {c} IS NULL \
         GROUP BY 1 ORDER BY 2 DESC, 1",
        space = q(SPACE),
        t = q(table),
        c = q(column),
    );
    Ok(pool
        .rows(&sql)
        .await?
        .into_iter()
        .map(|r| NullCount {
            space: r.first().cloned().flatten().unwrap_or_default(),
            rows: num(r.get(1)),
        })
        .collect())
}

/// A stored value outside the closed set a program is about to pin the
/// column to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutOfSet {
    pub space: String,
    pub value: Option<String>,
    pub rows: i64,
}

/// Values a new `enum(…)` pin would reject.
///
/// NULL is deliberately not counted: the generated `CHECK (col IN (…))` is
/// unknown-not-false for a null, so Postgres admits it, and reporting it as
/// a violation would refuse a plan the database would accept.
pub async fn out_of_enum(
    pool: &Pool,
    table: &str,
    column: &str,
    values: &[String],
    limit: usize,
) -> Result<Vec<OutOfSet>, PoolError> {
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let list: Vec<String> = values.iter().map(|v| crate::sql_lit(v)).collect();
    let sql = format!(
        "SELECT {space}, {c}, count(*) FROM {t} \
         WHERE {c} IS NOT NULL AND {c} NOT IN ({list}) \
         GROUP BY 1, 2 ORDER BY 3 DESC, 1, 2 LIMIT {limit}",
        space = q(SPACE),
        t = q(table),
        c = q(column),
        list = list.join(", "),
    );
    Ok(pool
        .rows(&sql)
        .await?
        .into_iter()
        .map(|r| OutOfSet {
            space: r.first().cloned().flatten().unwrap_or_default(),
            value: r.get(1).cloned().flatten(),
            rows: num(r.get(2)),
        })
        .collect())
}

/// A ref that names no row a new foreign key would accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dangling {
    pub space: String,
    pub rows: i64,
}

/// Rows whose ref column names a target that does not exist **in the same
/// space**.
///
/// The same-space half is the point rather than an optimisation: every
/// generated foreign key is `(space, ref) REFERENCES target (space, id)`,
/// so a ref naming a row of another tenant is dangling as far as the
/// constraint is concerned, and a probe that joined on id alone would call
/// a cross-space edge valid and then watch `VALIDATE CONSTRAINT` refuse it.
pub async fn dangling_refs(
    pool: &Pool,
    table: &str,
    column: &str,
    target: &str,
) -> Result<Vec<Dangling>, PoolError> {
    let sql = format!(
        "SELECT s.{space}, count(*) FROM {t} s \
         WHERE s.{c} IS NOT NULL AND NOT EXISTS ( \
           SELECT 1 FROM {target} p \
           WHERE p.{space} = s.{space} AND p.{id} = s.{c}) \
         GROUP BY 1 ORDER BY 2 DESC, 1",
        space = q(SPACE),
        t = q(table),
        c = q(column),
        target = q(target),
        id = q("id"),
    );
    Ok(pool
        .rows(&sql)
        .await?
        .into_iter()
        .map(|r| Dangling {
            space: r.first().cloned().flatten().unwrap_or_default(),
            rows: num(r.get(1)),
        })
        .collect())
}

/// Values that two distinct rows would share **after** a declared mapping
/// is applied to a column that carries a `unique`.
///
/// This is `nfc::collisions` with the mapping lifted out of the function.
/// `mapping` is a SQL expression in which `{}` stands for the quoted column
/// — `normalize({}, NFC)`, `lower({})` — and it comes from the *schema*
/// (`Field::normalize`) or from a declared migration mapping, never from a
/// caller. §5 C4: a non-empty answer is a refusal, because merging two rows
/// under a unique constraint is a data-loss path with a plausible-looking
/// plan.
pub async fn collisions(
    pool: &Pool,
    table: &str,
    column: &str,
    mapping: &str,
    limit: usize,
) -> Result<Vec<Duplicate>, PoolError> {
    let expr = mapping.replace("{}", &q(column));
    let sql = format!(
        "SELECT {space}, {expr}, count(*) FROM {t} WHERE {c} IS NOT NULL \
         GROUP BY 1, 2 HAVING count(*) > 1 ORDER BY 3 DESC, 1, 2 LIMIT {limit}",
        space = q(SPACE),
        t = q(table),
        c = q(column),
    );
    Ok(pool
        .rows(&sql)
        .await?
        .into_iter()
        .map(|r| Duplicate {
            space: r.first().cloned().flatten().unwrap_or_default(),
            value: r.get(1).cloned().flatten(),
            rows: num(r.get(2)),
        })
        .collect())
}

/// How many rows of one sealed column sit under each epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochCount {
    pub space: String,
    pub epoch: u8,
    pub rows: i64,
}

/// The epoch histogram of a sealed column, read **without a key**.
///
/// The envelope is `[epoch:1][nonce:24][ct‖tag]` (`ordo-crypto`), so the
/// epoch is byte zero and the server can group by it. That is the whole
/// reason this probe is cheap: asking *which rows will not open* normally
/// means opening them, and this asks the question the envelope answers in
/// clear.
///
/// It is a histogram rather than an "undecryptable" count because liveness
/// is the custody provider's answer, not the database's. §7.3: a row whose
/// envelope will not open under a destroyed epoch **cannot be transformed**,
/// and writing null in its place would be the platform deciding to lose
/// data — so the count must exist before the plan is offered, and the caller
/// that holds the provider is the one that can classify it.
pub async fn sealed_epochs(
    pool: &Pool,
    table: &str,
    column: &str,
) -> Result<Vec<EpochCount>, PoolError> {
    let sql = format!(
        "SELECT {space}, get_byte({c}, 0), count(*) FROM {t} \
         WHERE {c} IS NOT NULL AND octet_length({c}) > 0 \
         GROUP BY 1, 2 ORDER BY 1, 2",
        space = q(SPACE),
        t = q(table),
        c = q(column),
    );
    Ok(pool
        .rows(&sql)
        .await?
        .into_iter()
        .map(|r| EpochCount {
            space: r.first().cloned().flatten().unwrap_or_default(),
            epoch: r
                .get(1)
                .cloned()
                .flatten()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            rows: num(r.get(2)),
        })
        .collect())
}

// ── the numbers the disclosure obligation needs ──────────────────────────

/// Live row count and space count for one entity — the two numbers §9's
/// disclosure obligation and §6 respectively ask `ordo plan` to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub rows: i64,
    pub spaces: i64,
}

/// One round trip for both, because they are read from the same scan and an
/// operator asks them together: *how long is the freeze, and how many
/// tenants does it freeze?*
pub async fn extent(pool: &Pool, table: &str) -> Result<Extent, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT count(*), count(DISTINCT {space}) FROM {t}",
            space = q(SPACE),
            t = q(table),
        ))
        .await?;
    let r = rows.first().cloned().unwrap_or_default();
    Ok(Extent {
        rows: num(r.first()),
        spaces: num(r.get(1)),
    })
}

/// A measured scan rate, in rows per second.
///
/// §9 requires the freeze estimate to come from *"live row count and
/// **measured** chunk throughput"*, and a constant compiled into the tool
/// would be a number about the machine it was written on. This measures one
/// chunk on the index the backfill will use — the primary key is
/// `($space, id)`, so `ORDER BY ($space, id)` is an index-ordered scan and
/// the chunk is the backfill's own unit of work.
///
/// It is honestly a **lower bound on the cost and therefore a lower bound
/// on the estimate**: a read-only chunk scan does not pay for the write,
/// the WAL or the command-log append a real backfill chunk pays for. The
/// report says so rather than presenting the number as a prediction, and
/// increment 3 replaces it with the backfill's own measured rate once there
/// is one.
pub async fn chunk_rate(pool: &Pool, table: &str, chunk: usize) -> Result<Option<f64>, PoolError> {
    let sql = format!(
        "SELECT count(*) FROM (SELECT {space}, {id} FROM {t} \
         ORDER BY {space}, {id} LIMIT {chunk}) c",
        space = q(SPACE),
        id = q("id"),
        t = q(table),
    );
    // Once to warm whatever the buffer cache is going to warm, then once to
    // measure. A cold first read of a small table measures the disk and
    // then divides a million rows by it.
    pool.rows(&sql).await?;
    let started = Instant::now();
    let rows = pool.rows(&sql).await?;
    let elapsed = started.elapsed().as_secs_f64();
    let scanned = num(rows.first().and_then(|r| r.first()));
    if scanned == 0 || elapsed <= 0.0 {
        return Ok(None);
    }
    Ok(Some(scanned as f64 / elapsed))
}

/// The freeze §9 obliges `ordo plan` to state before anything runs.
///
/// Rendered as a duration string rather than a number, because *"1847
/// seconds"* is a number an operator has to convert before they can decline
/// it, and declining it is the decision the disclosure exists to enable.
pub fn freeze_estimate(rows: i64, rate: Option<f64>) -> String {
    let Some(rate) = rate.filter(|r| *r > 0.0) else {
        return "unknown (the scan rate could not be measured)".into();
    };
    let secs = rows as f64 / rate;
    if secs < 1.0 {
        return "under a second".into();
    }
    let secs = secs.round() as i64;
    match secs {
        0..=90 => format!("~{secs}s"),
        91..=5400 => format!("~{}m {}s", secs / 60, secs % 60),
        _ => format!("~{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

// ── the disclosure a drop owes (§7.2) ────────────────────────────────────

/// Retained command groups a drop would make unrevertable, per space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoHorizonLoss {
    pub space: String,
    /// Groups inside the undo horizon whose logged image names one of the
    /// dropped `entity.field` pairs.
    pub groups: i64,
    /// Groups inside the undo horizon that were examined, so the number
    /// above is readable as a proportion rather than as an absolute nobody
    /// can size.
    pub examined: i64,
}

/// How much of the undo horizon a `D1`/`D2` drop destroys.
///
/// `undo.rs` **already** refuses a revert whose logged image names a field
/// the loaded schema no longer declares (`unrevertable_image`), and it
/// already does it for the right reason. What is missing is the
/// *disclosure*: today a drop truncates the undo horizon for that entity at
/// the instant the column goes, silently, and the operator finds out the
/// next time somebody needs a revert. §7.2 makes the count part of the
/// destructive confirmation, so this is the query that produces it.
///
/// It scans forward from each space's horizon position rather than over the
/// whole chain: everything below the horizon is already beyond revert, so
/// counting it would inflate the number with groups the drop does not cost
/// anybody. `cutoff` is `now − undoHorizon` in the log's own millisecond
/// clock.
pub async fn undo_horizon_loss(
    pool: &Pool,
    entity: &str,
    fields: &[String],
    cutoff: u64,
) -> Result<Vec<UndoHorizonLoss>, PoolError> {
    let spaces = pool
        .rows("SELECT DISTINCT \"space\" FROM \"$commandlog\" ORDER BY 1")
        .await?;
    let mut out = Vec::new();
    for row in spaces {
        let Some(Some(space)) = row.first().cloned() else {
            continue;
        };
        let sid = ordo_commandlog::SpaceId(space.clone());
        let from = crate::prune::horizon_position(pool, &sid, cutoff).await?;
        let rows = pool
            .rows(&format!(
                "SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = {} AND \"idx\" >= {from} \
                 ORDER BY \"idx\"",
                crate::sql_lit(&space)
            ))
            .await?;
        let mut groups = 0i64;
        let mut examined = 0i64;
        for r in &rows {
            let Some(body) = crate::commandlog::bytea_of(r.first().and_then(|c| c.as_ref())) else {
                continue;
            };
            // A body that will not decode is `verify`'s business, not this
            // command's. Skipping it here keeps a tamper report from
            // arriving as a migration refusal, which would page the wrong
            // person.
            let Ok(group) = ordo_commandlog::decode_group(&body) else {
                continue;
            };
            examined += 1;
            let hit = group.commands.iter().any(|c| {
                c.entity == entity
                    && [c.before.as_ref(), c.after.as_ref()]
                        .into_iter()
                        .flatten()
                        .any(|img| fields.iter().any(|f| img.contains_key(f)))
            });
            if hit {
                groups += 1;
            }
        }
        if groups > 0 {
            out.push(UndoHorizonLoss {
                space,
                groups,
                examined,
            });
        }
    }
    Ok(out)
}

/// Rows are counted as text by the pool, and a count that will not parse is
/// zero rather than a panic: a probe that crashes on an unexpected answer
/// tells the operator nothing, and the caller already treats a non-empty
/// probe as a refusal.
fn num(v: Option<&Option<String>>) -> i64 {
    v.and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

// ── row invariants (C9) ─────────────────────────────────────────────────

/// Rows that violate a `check` the program is about to declare, per space.
///
/// `sql` is the rendered CHECK expression (`ordo_schema::check::CheckRule::sql`),
/// which `check.rs` renders total: equality through the distinctness
/// family and ordering through `COALESCE(…, false)`. So `NOT (sql)` is
/// exactly the set of rows the constraint would refuse, nulls included.
/// Postgres treats a NULL check result as a pass; the rendering is what
/// stops a null from passing here and failing there.
pub async fn check_violations(
    pool: &Pool,
    table: &str,
    sql: &str,
) -> Result<Vec<NullCount>, PoolError> {
    let stmt = format!(
        "SELECT {space}, count(*) FROM {t} WHERE NOT ({sql}) \
         GROUP BY 1 ORDER BY 2 DESC, 1",
        space = q(SPACE),
        t = q(table),
    );
    Ok(pool
        .rows(&stmt)
        .await?
        .into_iter()
        .map(|r| NullCount {
            space: r.first().cloned().flatten().unwrap_or_default(),
            rows: num(r.get(1)),
        })
        .collect())
}

// ── the migration record ────────────────────────────────────────────────

/// `$migrations`: one row per schema apply, on the control database beside
/// `$artifacts`.
///
/// Increment 2's shape (`plan-schema-evolution.md` §8): what was applied,
/// from which serving version to which, by whom, as which statements, and
/// whether it landed. Increment 3 grows it into the state machine
/// (`planned → applying → backfilling → verified → swapped → contracted`)
/// with a chunk cursor; the columns here are the ones every phase of that
/// machine also needs, so they are minted once. `IF NOT EXISTS` for the
/// same reason `REGISTRY_DDL` has it.
pub const MIGRATIONS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$migrations\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"kind\" text NOT NULL, \
  \"from_version\" text, \
  \"to_version\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"at\" bigint NOT NULL, \
  \"changes\" text NOT NULL, \
  \"ddl\" text NOT NULL, \
  \"phase\" text NOT NULL, \
  \"note\" text NOT NULL DEFAULT '' \
)";

/// The phases increment 2 writes. Strings rather than an enum on the row
/// so increment 3's phases extend the set without a column migration of
/// the migration table.
pub mod phase {
    /// The DDL committed.
    pub const APPLIED: &str = "applied";
    /// The DDL transaction rolled back; `note` carries the database's
    /// error. Nothing changed.
    pub const FAILED: &str = "failed";
    /// The transactional DDL committed and a `CREATE INDEX CONCURRENTLY`
    /// after it left an INVALID index behind; `note` names it. The
    /// columns and constraints are live, the index is not usable.
    pub const INDEX_INVALID: &str = "index_invalid";
    /// The transactional DDL committed with a constraint added `NOT VALID`,
    /// and `VALIDATE CONSTRAINT` after it failed: existing rows break it,
    /// or its lock never came. The constraint was dropped again; `note`
    /// names it. The columns are live, the constraint is not there, and
    /// the next apply plans it afresh.
    pub const VALIDATE_FAILED: &str = "validate_failed";
}

/// The kinds increment 2 writes.
pub mod kind {
    /// A first apply against an empty target: the whole schema, created.
    pub const CREATE: &str = "create";
    /// Class-A additive DDL over a deployed schema.
    pub const ADDITIVE: &str = "additive";
    /// No DDL ran: the database already held everything the version
    /// declares (a field the program stopped declaring is left in place),
    /// and this row records that it is at that version.
    pub const VERSION: &str = "version";
}

/// One `$migrations` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationRecord {
    pub seq: i64,
    pub kind: String,
    pub from_version: Option<String>,
    pub to_version: String,
    pub actor: String,
    pub at: u64,
    /// One change per line, `code — description`, as the plan printed it.
    pub changes: String,
    /// Every statement that was sent, in order, whether or not it landed.
    pub ddl: String,
    pub phase: String,
    pub note: String,
}

pub async fn ensure_migrations_table(pool: &Pool) -> Result<(), PoolError> {
    pool.rows(MIGRATIONS_DDL).await.map(|_| ())
}

/// Append one record and return its `seq`. Written on its own connection,
/// never inside the DDL transaction: a failed apply must still leave a
/// row saying it failed.
pub async fn record_migration(pool: &Pool, r: &MigrationRecord) -> Result<i64, PoolError> {
    let rows = pool
        .rows(&format!(
            "INSERT INTO \"$migrations\" \
               (\"kind\", \"from_version\", \"to_version\", \"actor\", \"at\", \
                \"changes\", \"ddl\", \"phase\", \"note\") \
             VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}) RETURNING \"seq\"",
            crate::sql_lit(&r.kind),
            r.from_version
                .as_deref()
                .map(crate::sql_lit)
                .unwrap_or_else(|| "NULL".into()),
            crate::sql_lit(&r.to_version),
            crate::sql_lit(&r.actor),
            r.at,
            crate::sql_lit(&r.changes),
            crate::sql_lit(&r.ddl),
            crate::sql_lit(&r.phase),
            crate::sql_lit(&r.note),
        ))
        .await?;
    Ok(num(rows.first().and_then(|r| r.first())))
}

/// The most recent records, newest first.
pub async fn migrations(pool: &Pool, limit: usize) -> Result<Vec<MigrationRecord>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"seq\", \"kind\", \"from_version\", \"to_version\", \"actor\", \"at\", \
                    \"changes\", \"ddl\", \"phase\", \"note\" \
             FROM \"$migrations\" ORDER BY \"seq\" DESC LIMIT {limit}"
        ))
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let g = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
            MigrationRecord {
                seq: num(r.first()),
                kind: g(1),
                from_version: r.get(2).cloned().flatten(),
                to_version: g(3),
                actor: g(4),
                at: num(r.get(5)) as u64,
                changes: g(6),
                ddl: g(7),
                phase: g(8),
                note: g(9),
            }
        })
        .collect())
}

/// Is the named index valid? `None` when no such index exists.
///
/// `CREATE INDEX CONCURRENTLY` that fails part-way leaves an INVALID
/// index in the catalog: present by name, used by no query, and blocking
/// a second `CREATE` under the same name. The apply reads this back
/// rather than inferring it from the statement's exit, because the
/// statement can fail after the catalog row exists.
pub async fn index_valid(pool: &Pool, schema: &str, name: &str) -> Result<Option<bool>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT x.indisvalid FROM pg_index x \
             JOIN pg_class i ON i.oid = x.indexrelid \
             JOIN pg_namespace n ON n.oid = i.relnamespace \
             WHERE n.nspname = {} AND i.relname = {}",
            crate::sql_lit(schema),
            crate::sql_lit(name)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .map(|v| v == "t"))
}
