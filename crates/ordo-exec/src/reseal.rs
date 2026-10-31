//! K1: the re-seal pass, and K2: the destruction gate.
//!
//! `KeyProvider::rotate` mints a new epoch and re-seals nothing, so until
//! this module existed a rotation forced by an exposure rotated to a key
//! that still opened everything. This is the pass that moves every
//! ciphertext a space holds from one epoch to the next, and the gate that
//! refuses to destroy an epoch while anything is still under it.
//!
//! # What a pass moves, and what it never touches
//!
//! Four populations, in this order (`plan-production-ops.md` §1.3, §1.6):
//!
//! 1. **`$sealedpayload` rows** — the command images' payloads. Re-sealed
//!    **in place**: the chain hashes the sixteen-byte handle and never the
//!    payload, so a new envelope under the same `("$space","ref")` moves
//!    no hash (`corpus-rulings.md`, 2026-09-08).
//! 2. **Live sealed columns** — every `sealed` field that is not content,
//!    keyset-paged on `("$space","id")`, never `OFFSET`.
//! 3. **`$blobref` envelopes** — `mime`, `size`, `integrity`, re-sealed in
//!    place. The row's `epoch` column is the **object's** epoch and is left
//!    alone by this population.
//! 4. **Blob objects** — counted by `$blobref."epoch"`. An object's key is
//!    its handle and the store refuses to overwrite one, so an object is
//!    moved to a **new handle**: the bytes are opened under `n` and sealed
//!    under the current epoch into a new object, a new `$blobref` row is
//!    inserted, and the entity column is moved by CAS from the old hex
//!    handle to the new. The old row and object are **left alone**: a
//!    retained command image names the old handle, a revert resolves it
//!    through the old row, and `blobgc` collects the pair once no live row
//!    and no retained image names it. So this population clears by the
//!    blobgc floor, and a pass whose only remainder is objects nothing can
//!    move now (unreferenced old pairs, or no store wired) reports
//!    [`PassState::Blocked`] rather than re-opening; the gate still
//!    refuses by name until the count is zero.
//!
//! **Blind-index columns are not a population.** The blind-index key is
//! SMK-derived and epoch-independent, and no statement in this module names
//! a `$bidx` column. A DEK rotation moves no digest, which is what keeps
//! `sealed indexed unique` continuous across the pass.
//!
//! # Why there is no write barrier
//!
//! Writes always seal under the newest epoch, so a row written during the
//! pass is already in the state the pass wanted. The only hazard is the
//! pass clobbering that newer write with a value it re-sealed from a stale
//! read, and every `UPDATE` here is a compare-and-set on the **old envelope
//! bytes**: every seal draws a fresh nonce, so those bytes are unique to
//! one write and a zero-row update means the row moved. It is counted as
//! skipped, never retried.
//!
//! # What is verified, and what is persisted
//!
//! Per row, in memory: open under `n`, seal under the current epoch, open
//! the result, compare, drop both. A mismatch is [`ResealError::Verify`]
//! and the chunk does not commit. What the `$reseal` row keeps is the
//! cursor, the counts, and a SHA-256 over `(ref ‖ new envelope)` for the
//! chunk. A digest over plaintext is never stored: it would be the
//! unsalted permanent equality index §7 rejects, and it would survive
//! crypto-erasure.
//!
//! # Completion is a count
//!
//! A row sealed under `n` inside a transaction that committed after the
//! chunk passed its keyset position is rare and real. When every
//! population's cursor is exhausted the pass re-counts, and a non-zero
//! population re-opens its cursor for a second sweep. [`Step::Completed`]
//! is returned once, when [`Remaining::total`] is zero.

use ordo_crypto::blobref::{BLOB_REF_TABLE, BLOB_SPACE_COLUMN};
use ordo_crypto::sealed::{SEALED_PAYLOAD_TABLE, SEALED_SPACE_COLUMN};
use ordo_crypto::{Epoch, KeyError, KeyLease, KeyProvider, Purpose, SpaceId};
use ordo_schema::Schema;
use sha2::{Digest, Sha256};

use crate::{sql_lit, OpClass, Pool, PoolError, TextRow, TxConn};

/// The pass table. One row per `(space, from_epoch, population)`: it
/// carries the cursor, the attempt count and the state that `$jobs` does
/// not.
pub const RESEAL_TABLE: &str = "$reseal";

pub const RESEAL_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$reseal" (
  "space"        text   NOT NULL,
  "from_epoch"   int    NOT NULL,
  "to_epoch"     int    NOT NULL,
  "population"   text   NOT NULL,
  "cursor"       text,
  "rows_done"    bigint NOT NULL DEFAULT 0,
  "rows_skipped" bigint NOT NULL DEFAULT 0,
  "chunk_digest" text,
  "attempts"     int    NOT NULL DEFAULT 0,
  "state"        text   NOT NULL,
  "reason"       text   NOT NULL,
  "started_at"   bigint NOT NULL,
  "advanced_at"  bigint NOT NULL,
  "finished_at"  bigint,
  PRIMARY KEY ("space", "from_epoch", "population")
)"#;

/// Create `$reseal`. Idempotent, retried once for the reason
/// `Pool::ensure_platform_tables` gives: `CREATE TABLE IF NOT EXISTS` is
/// not race-free between two processes.
pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(RESEAL_DDL).await.is_err() {
        pool.rows(RESEAL_DDL).await?;
    }
    Ok(())
}

/// The four populations, in the order a pass moves them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Population {
    /// `$sealedpayload` rows. First, because it is the population the
    /// horizon gate was written about.
    Payloads,
    /// Live sealed columns on entity tables.
    Columns,
    /// `$blobref`'s `mime`, `size` and `integrity` envelopes.
    BlobRefs,
    /// Stored objects, by `$blobref."epoch"`.
    Objects,
}

impl Population {
    pub const ALL: [Population; 4] = [
        Population::Payloads,
        Population::Columns,
        Population::BlobRefs,
        Population::Objects,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Population::Payloads => "payloads",
            Population::Columns => "columns",
            Population::BlobRefs => "blobrefs",
            Population::Objects => "objects",
        }
    }

    pub fn parse(s: &str) -> Option<Population> {
        Population::ALL.into_iter().find(|p| p.as_str() == s)
    }
}

impl std::fmt::Display for Population {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `pad`, not `write_str`, so a `{:<10}` in a table lines up.
        f.pad(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassState {
    /// Enqueued, not yet touched.
    Pending,
    /// A chunk has committed and the cursor is somewhere.
    Running,
    /// The cursor is exhausted and the re-count agreed.
    Done,
    /// Rows remain and nothing can move them now: old object pairs waiting
    /// on `blobgc`, or objects with no store wired. The gate still counts
    /// them, and `ordo keys reseal` re-opens the row for another sweep.
    Blocked,
}

impl PassState {
    pub fn as_str(self) -> &'static str {
        match self {
            PassState::Pending => "pending",
            PassState::Running => "running",
            PassState::Done => "done",
            PassState::Blocked => "blocked",
        }
    }
    fn parse(s: &str) -> Option<PassState> {
        [
            PassState::Pending,
            PassState::Running,
            PassState::Done,
            PassState::Blocked,
        ]
        .into_iter()
        .find(|p| p.as_str() == s)
    }
}

/// One `$reseal` row, as the operator surface and the worker read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassRow {
    pub space: String,
    pub from_epoch: Epoch,
    pub to_epoch: Epoch,
    pub population: Population,
    pub cursor: Option<String>,
    pub rows_done: i64,
    pub rows_skipped: i64,
    pub chunk_digest: Option<String>,
    pub attempts: i64,
    pub state: PassState,
    pub reason: String,
    pub started_at: u64,
    pub advanced_at: u64,
    pub finished_at: Option<u64>,
}

impl PassRow {
    fn of(r: &TextRow) -> Result<PassRow, PoolError> {
        let cell = |i: usize| r.get(i).and_then(|c| c.as_deref());
        let need = |i: usize| {
            cell(i).ok_or_else(|| PoolError::Db("$reseal: a NOT NULL column came back null".into()))
        };
        let num = |s: &str| {
            s.parse::<i64>()
                .map_err(|_| PoolError::db(format!("$reseal: `{s}` is not a number")))
        };
        let epoch = |s: &str| {
            u8::try_from(num(s)?)
                .map_err(|_| PoolError::db(format!("$reseal: epoch {s} is out of range")))
        };
        Ok(PassRow {
            space: need(0)?.to_string(),
            from_epoch: epoch(need(1)?)?,
            to_epoch: epoch(need(2)?)?,
            population: Population::parse(need(3)?)
                .ok_or_else(|| PoolError::Db("$reseal: unknown population".into()))?,
            cursor: cell(4).map(str::to_string),
            rows_done: num(need(5)?)?,
            rows_skipped: num(need(6)?)?,
            chunk_digest: cell(7).map(str::to_string),
            attempts: num(need(8)?)?,
            state: PassState::parse(need(9)?)
                .ok_or_else(|| PoolError::Db("$reseal: unknown state".into()))?,
            reason: need(10)?.to_string(),
            started_at: num(need(11)?)? as u64,
            advanced_at: num(need(12)?)? as u64,
            finished_at: cell(13).map(num).transpose()?.map(|n| n as u64),
        })
    }
}

const PASS_COLUMNS: &str = "\"space\", \"from_epoch\", \"to_epoch\", \"population\", \"cursor\", \
    \"rows_done\", \"rows_skipped\", \"chunk_digest\", \"attempts\", \"state\", \"reason\", \
    \"started_at\", \"advanced_at\", \"finished_at\"";

#[derive(Debug)]
#[non_exhaustive]
pub enum ResealError {
    Db(PoolError),
    Key(KeyError),
    /// A re-sealed value did not open back to what was read. The chunk
    /// was not committed. Names the row by population and handle, never
    /// by value.
    Verify {
        population: Population,
        at: String,
    },
    /// A pass over the epoch new writes seal under would never terminate.
    EpochIsCurrent(Epoch),
    /// A stored value did not decode as Postgres hex, or an object's
    /// header did not decode as one.
    Malformed {
        at: String,
    },
    /// The object store refused or failed.
    Store(String),
}

impl std::fmt::Display for ResealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResealError::Db(e) => write!(f, "re-seal: {e}"),
            ResealError::Key(e) => write!(f, "re-seal: {e}"),
            ResealError::Verify { population, at } => write!(
                f,
                "re-seal: {population} row {at} did not open back to what was read; the chunk was \
                 not committed"
            ),
            ResealError::EpochIsCurrent(e) => write!(
                f,
                "re-seal: epoch {e} is the one new writes seal under; rotate first"
            ),
            ResealError::Malformed { at } => {
                write!(f, "re-seal: `{at}` does not decode as stored")
            }
            ResealError::Store(m) => write!(f, "re-seal: object store: {m}"),
        }
    }
}

impl std::error::Error for ResealError {}

impl From<PoolError> for ResealError {
    fn from(e: PoolError) -> Self {
        ResealError::Db(e)
    }
}

impl From<KeyError> for ResealError {
    fn from(e: KeyError) -> Self {
        ResealError::Key(e)
    }
}

// ── enqueueing ───────────────────────────────────────────────────────────

/// Mint the next epoch and enqueue the pass that moves everything off the
/// one before it. The one production caller of `KeyProvider::rotate`.
///
/// Returns the new epoch. The `$reseal` rows are written after the
/// rotation commits, so a crash between the two leaves an epoch with no
/// pass, which `ordo keys reseal` repairs.
pub async fn enqueue_rotation(
    pool: &Pool,
    keys: &dyn KeyProvider,
    space: &SpaceId,
    reason: &str,
    now: u64,
) -> Result<Epoch, ResealError> {
    let from = keys.current_epoch(space)?;
    let to = keys.rotate(space, reason)?;
    enqueue_pass(pool, space, from, to, reason, now).await?;
    Ok(to)
}

/// Enqueue a pass from `from` to `to` for an epoch already minted.
///
/// Idempotent over an open pass: rows that are `pending` or `running` are
/// left exactly where they are. A `done` or `blocked` row is re-opened,
/// which is what makes this the forced re-encrypt an operator runs after
/// an exposure when the rotation already happened, and what gives a
/// blocked object population another sweep once `blobgc` has run. Returns
/// whether any row was created or re-opened.
pub async fn enqueue_pass(
    pool: &Pool,
    space: &SpaceId,
    from: Epoch,
    to: Epoch,
    reason: &str,
    now: u64,
) -> Result<bool, ResealError> {
    let mut touched = false;
    for p in Population::ALL {
        let rows = pool
            .rows(&format!(
                "INSERT INTO \"{RESEAL_TABLE}\" (\"space\", \"from_epoch\", \"to_epoch\", \
                 \"population\", \"state\", \"reason\", \"started_at\", \"advanced_at\") \
                 VALUES ({s}, {from}, {to}, {p}, 'pending', {reason}, {now}, {now}) \
                 ON CONFLICT (\"space\", \"from_epoch\", \"population\") DO UPDATE SET \
                   \"state\" = 'pending', \"cursor\" = NULL, \"finished_at\" = NULL, \
                   \"to_epoch\" = EXCLUDED.\"to_epoch\", \"reason\" = EXCLUDED.\"reason\", \
                   \"advanced_at\" = EXCLUDED.\"advanced_at\" \
                 WHERE \"{RESEAL_TABLE}\".\"state\" IN ('done', 'blocked') \
                 RETURNING \"population\"",
                s = sql_lit(&space.0),
                p = sql_lit(p.as_str()),
                reason = sql_lit(reason),
            ))
            .await?;
        touched |= !rows.is_empty();
    }
    Ok(touched)
}

/// Every `$reseal` row for a space, oldest epoch first.
pub async fn passes(pool: &Pool, space: &SpaceId) -> Result<Vec<PassRow>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {PASS_COLUMNS} FROM \"{RESEAL_TABLE}\" WHERE \"space\" = {} \
             ORDER BY \"from_epoch\", \"population\"",
            sql_lit(&space.0)
        ))
        .await?;
    let mut out = rows
        .iter()
        .map(PassRow::of)
        .collect::<Result<Vec<_>, _>>()?;
    out.sort_by_key(|r| (r.from_epoch, r.population));
    Ok(out)
}

// ── counting ─────────────────────────────────────────────────────────────

/// How much of a space is still under an epoch, per population.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Remaining {
    pub payloads: i64,
    pub columns: i64,
    pub blobrefs: i64,
    pub objects: i64,
}

impl Remaining {
    pub fn total(&self) -> i64 {
        self.payloads + self.columns + self.blobrefs + self.objects
    }

    pub fn of(&self, p: Population) -> i64 {
        match p {
            Population::Payloads => self.payloads,
            Population::Columns => self.columns,
            Population::BlobRefs => self.blobrefs,
            Population::Objects => self.objects,
        }
    }

    /// The first population still holding rows, in pass order.
    pub fn first_nonzero(&self) -> Option<(Population, i64)> {
        Population::ALL
            .into_iter()
            .map(|p| (p, self.of(p)))
            .find(|(_, n)| *n > 0)
    }
}

/// The sealed columns a schema declares: every `sealed` field that is not
/// content. A content field's column holds a handle, and its bytes are in
/// `$sealedpayload` or in the object store.
pub fn sealed_columns(schema: &Schema) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for e in &schema.entities {
        for f in &e.fields {
            if f.sealed && !f.is_content() {
                out.push((e.name.clone(), f.column()));
            }
        }
    }
    out
}

/// `length(c) > 0 AND get_byte(c, 0) = n`. Not `get_byte` alone: a sealed
/// column can hold zero bytes, and `get_byte` on an empty `bytea` is an
/// error rather than false.
fn under(col: &str, epoch: Epoch) -> String {
    format!("(length({col}) > 0 AND get_byte({col}, 0) = {epoch})")
}

fn q(name: &str) -> String {
    ordo_schema::ddl::q(name)
}

fn bytea(bytes: &[u8]) -> String {
    ordo_crypto::sealed::bytea_lit(bytes)
}

async fn count(pool: &Pool, sql: &str) -> Result<i64, PoolError> {
    let rows = pool.rows(sql).await?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Count all four populations under `epoch`.
pub async fn remaining(
    pool: &Pool,
    schema: &Schema,
    space: &SpaceId,
    epoch: Epoch,
) -> Result<Remaining, PoolError> {
    let s = sql_lit(&space.0);
    let payloads = count(pool, &ordo_crypto::sealed::epoch_count_sql(space, epoch)).await?;
    let mut columns = 0;
    for (entity, field) in sealed_columns(schema) {
        columns += count(
            pool,
            &format!(
                "SELECT count(*) FROM {} WHERE {} = {s} AND {}",
                q(&entity),
                q(ordo_schema::SPACE_COLUMN),
                under(&q(&field), epoch)
            ),
        )
        .await?;
    }
    let blobrefs = count(
        pool,
        &format!(
            "SELECT count(*) FROM {} WHERE {} = {s} AND ({} OR {} OR {})",
            q(BLOB_REF_TABLE),
            q(BLOB_SPACE_COLUMN),
            under("\"mime\"", epoch),
            under("\"size\"", epoch),
            under("\"integrity\"", epoch),
        ),
    )
    .await?;
    let objects = count(pool, &ordo_crypto::blobref::epoch_count_sql(space, epoch)).await?;
    Ok(Remaining {
        payloads,
        columns,
        blobrefs,
        objects,
    })
}

// ── the gate ─────────────────────────────────────────────────────────────

/// K2's answer to "may this epoch be destroyed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Nothing in the space is under the epoch. `resealed_at` is when the
    /// pass that emptied it finished, which is the date backups taken
    /// before it hold ciphertext this destruction will make unopenable.
    Clear { resealed_at: Option<u64> },
    /// Rows remain, and this is the first population that has any.
    Refused { population: Population, count: i64 },
    /// The epoch new writes seal under. Rotate first.
    Current,
}

/// Count all four populations and resolve the gate. The check
/// `KeyProvider::destroy_epoch`'s doc says the caller owes.
///
/// With every population re-sealed, the §7 rule ("no command image within
/// the undo horizon is tagged with the epoch") is discharged by the count:
/// an image is tagged with an epoch through the payload row its handle
/// names, and a zero count means no such row exists, inside the horizon
/// or out of it. The horizon is therefore not consulted separately, and
/// the confirmation prints the one thing the pass cannot reach, which is
/// a backup taken before it finished.
pub async fn destroy_gate(
    pool: &Pool,
    schema: &Schema,
    keys: &dyn KeyProvider,
    space: &SpaceId,
    epoch: Epoch,
) -> Result<Gate, ResealError> {
    if keys.current_epoch(space)? == epoch {
        return Ok(Gate::Current);
    }
    let left = remaining(pool, schema, space, epoch).await?;
    if let Some((population, count)) = left.first_nonzero() {
        return Ok(Gate::Refused { population, count });
    }
    let resealed_at = passes(pool, space)
        .await?
        .into_iter()
        .filter(|r| r.from_epoch == epoch)
        .filter_map(|r| r.finished_at)
        .max();
    Ok(Gate::Clear { resealed_at })
}

// ── the pass ─────────────────────────────────────────────────────────────

/// What one [`Reseal::step`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// No open pass for the space.
    Idle,
    /// One chunk committed.
    Moved {
        from_epoch: Epoch,
        population: Population,
        done: u64,
        skipped: u64,
    },
    /// A population's cursor was exhausted, the re-count found rows, and
    /// the cursor was re-opened for another sweep.
    Reopened {
        from_epoch: Epoch,
        population: Population,
        count: i64,
    },
    /// Rows remain that nothing can move now (see [`PassState::Blocked`]).
    Blocked {
        from_epoch: Epoch,
        population: Population,
        count: i64,
    },
    /// Every population is at zero. Returned once per pass.
    Completed { from_epoch: Epoch },
}

/// The pass engine. Holds references, because the worker builds one per
/// run over what its context holds and the CLI builds one per command.
pub struct Reseal<'a> {
    pub pool: &'a Pool,
    pub keys: &'a dyn KeyProvider,
    pub schema: &'a Schema,
    pub space: &'a SpaceId,
    /// Rows per chunk. The throttle's other half is the worker's tick.
    pub chunk: usize,
    /// Where a space's objects are. `None` leaves the object population
    /// counted and blocked; the worker hands the deployment's resolver in,
    /// the CLI has none.
    pub stores: Option<&'a dyn ordo_blob::StoreResolver>,
}

/// [`KeyProvider::open_at`] at a stored anchor label. A label that names no
/// anchor never opens.
fn open_label(
    keys: &dyn KeyProvider,
    lease: &KeyLease,
    entity: &str,
    field: &str,
    label: &str,
    sealed: &[u8],
) -> Result<Vec<u8>, KeyError> {
    let anchor = ordo_crypto::Anchor::parse(label).ok_or(KeyError::Seal)?;
    keys.open_at(lease, entity, field, anchor, sealed)
}

/// [`open_label`]'s sealing half.
fn seal_label(
    keys: &dyn KeyProvider,
    lease: &KeyLease,
    entity: &str,
    field: &str,
    label: &str,
    plain: &[u8],
) -> Result<Vec<u8>, KeyError> {
    let anchor = ordo_crypto::Anchor::parse(label).ok_or(KeyError::Seal)?;
    keys.seal_at(lease, entity, field, anchor, plain)
}

/// One row read from a population, before the crypto.
struct Item {
    /// The keyset position this row occupies, as the cursor will store it.
    at: String,
    entity: String,
    field: String,
    /// The label of the anchor the envelopes were sealed at: the row for a
    /// column, the row a payload images, the object for a reference.
    anchor: String,
    /// The envelopes to move. One for payloads and columns; up to three
    /// for a `$blobref` row, `None` where the column is already current.
    envelopes: Vec<Option<Vec<u8>>>,
}

/// One chunk's outcome, inside its transaction.
struct Chunk {
    done: u64,
    skipped: u64,
    /// The last keyset position read, or `None` when the read was empty.
    last: Option<String>,
    digest: String,
}

impl Reseal<'_> {
    /// Run one chunk. Never loops: the worker's tick is the inter-chunk
    /// delay, and a caller that wants the whole pass calls this until
    /// [`Step::Idle`].
    pub async fn step(&self, now: u64) -> Result<Step, ResealError> {
        let open = passes(self.pool, self.space)
            .await?
            .into_iter()
            .filter(|r| matches!(r.state, PassState::Pending | PassState::Running))
            .min_by_key(|r| (r.from_epoch, r.population));
        let Some(row) = open else {
            return Ok(Step::Idle);
        };
        if self.keys.current_epoch(self.space)? == row.from_epoch {
            return Err(ResealError::EpochIsCurrent(row.from_epoch));
        }
        match self.chunk_of(&row, now).await {
            Ok(step) => Ok(step),
            Err(e) => {
                // Counted outside the chunk's transaction, which rolled
                // back: the count is the operator's evidence of a pass
                // that is trying and failing, which the stuck gauge
                // cannot tell from one that is not running.
                let _ = self
                    .pool
                    .rows(&format!(
                        "UPDATE \"{RESEAL_TABLE}\" SET \"attempts\" = \"attempts\" + 1 \
                         WHERE \"space\" = {} AND \"from_epoch\" = {} AND \"population\" = {}",
                        sql_lit(&self.space.0),
                        row.from_epoch,
                        sql_lit(row.population.as_str())
                    ))
                    .await;
                Err(e)
            }
        }
    }

    async fn chunk_of(&self, row: &PassRow, now: u64) -> Result<Step, ResealError> {
        let from = row.from_epoch;
        let correlation = format!("reseal:{}:{}:{}", self.space.0, from, row.population);
        let tx = self.pool.begin_for(OpClass::AddressedWrite).await?;

        let chunk = match row.population {
            Population::Payloads => {
                let items = self.read_payloads(&tx, from, row.cursor.as_deref()).await?;
                let lease = self.lease(&correlation)?;
                self.move_payloads(&tx, &lease, from, items).await?
            }
            Population::Columns => {
                let (pair, items) = self.read_columns(&tx, from, row.cursor.as_deref()).await?;
                match pair {
                    None => Chunk {
                        done: 0,
                        skipped: 0,
                        last: None,
                        digest: String::new(),
                    },
                    Some((entity, field)) => {
                        let lease = self.lease(&correlation)?;
                        let mut c = self
                            .move_columns(&tx, &lease, from, &entity, &field, items)
                            .await?;
                        // An exhausted pair advances to the next pair rather
                        // than ending the population.
                        if c.last.is_none() {
                            if let Some(next) = self.next_pair(&entity, &field) {
                                c.last = Some(column_cursor(&next.0, &next.1, ""));
                            }
                        }
                        c
                    }
                }
            }
            Population::BlobRefs => {
                let items = self.read_blobrefs(&tx, from, row.cursor.as_deref()).await?;
                let lease = self.lease(&correlation)?;
                self.move_blobrefs(&tx, &lease, from, items).await?
            }
            Population::Objects => match self.stores {
                // No store wired: the population is closed and the re-count
                // below reports it blocked.
                None => Chunk {
                    done: 0,
                    skipped: 0,
                    last: None,
                    digest: String::new(),
                },
                Some(stores) => {
                    let items = self.read_objects(&tx, from, row.cursor.as_deref()).await?;
                    let lease = self.lease(&correlation)?;
                    self.move_objects(&tx, stores, &lease, from, items, now)
                        .await?
                }
            },
        };

        let s = sql_lit(&self.space.0);
        let p = sql_lit(row.population.as_str());
        match chunk.last {
            Some(cursor) => {
                tx.rows(&format!(
                    "UPDATE \"{RESEAL_TABLE}\" SET \"state\" = 'running', \"cursor\" = {}, \
                     \"rows_done\" = \"rows_done\" + {}, \"rows_skipped\" = \"rows_skipped\" + {}, \
                     \"chunk_digest\" = {}, \"advanced_at\" = {now} \
                     WHERE \"space\" = {s} AND \"from_epoch\" = {from} AND \"population\" = {p}",
                    sql_lit(&cursor),
                    chunk.done,
                    chunk.skipped,
                    sql_lit(&chunk.digest),
                ))
                .await?;
                tx.commit().await?;
                Ok(Step::Moved {
                    from_epoch: from,
                    population: row.population,
                    done: chunk.done,
                    skipped: chunk.skipped,
                })
            }
            None => {
                tx.rows(&format!(
                    "UPDATE \"{RESEAL_TABLE}\" SET \"state\" = 'done', \"finished_at\" = {now}, \
                     \"advanced_at\" = {now} \
                     WHERE \"space\" = {s} AND \"from_epoch\" = {from} AND \"population\" = {p}",
                ))
                .await?;
                tx.commit().await?;
                self.recount(row, now).await
            }
        }
    }

    /// After a population closes: if any population of this epoch is still
    /// open, carry on; otherwise re-count and either re-open, block, or
    /// complete.
    async fn recount(&self, row: &PassRow, now: u64) -> Result<Step, ResealError> {
        let from = row.from_epoch;
        let rows: Vec<PassRow> = passes(self.pool, self.space)
            .await?
            .into_iter()
            .filter(|r| r.from_epoch == from)
            .collect();
        if rows
            .iter()
            .any(|r| matches!(r.state, PassState::Pending | PassState::Running))
        {
            return Ok(Step::Moved {
                from_epoch: from,
                population: row.population,
                done: 0,
                skipped: 0,
            });
        }
        let left = remaining(self.pool, self.schema, self.space, from).await?;
        let Some((population, count)) = left.first_nonzero() else {
            return Ok(Step::Completed { from_epoch: from });
        };
        let s = sql_lit(&self.space.0);
        let p = sql_lit(population.as_str());
        // Objects are never re-opened by the pass itself: what an objects
        // sweep leaves behind is old pairs only `blobgc` can retire, and
        // re-opening would scan them once per tick forever. `ordo keys
        // reseal` re-opens the row when an operator wants another sweep.
        if population == Population::Objects {
            self.pool
                .rows(&format!(
                    "UPDATE \"{RESEAL_TABLE}\" SET \"state\" = 'blocked', \"finished_at\" = NULL \
                     WHERE \"space\" = {s} AND \"from_epoch\" = {from} AND \"population\" = {p}",
                ))
                .await?;
            return Ok(Step::Blocked {
                from_epoch: from,
                population,
                count,
            });
        }
        self.pool
            .rows(&format!(
                "UPDATE \"{RESEAL_TABLE}\" SET \"state\" = 'pending', \"cursor\" = NULL, \
                 \"finished_at\" = NULL, \"advanced_at\" = {now} \
                 WHERE \"space\" = {s} AND \"from_epoch\" = {from} AND \"population\" = {p}",
            ))
            .await?;
        Ok(Step::Reopened {
            from_epoch: from,
            population,
            count,
        })
    }

    fn lease(&self, correlation: &str) -> Result<KeyLease, KeyError> {
        self.keys.release(self.space, Purpose::Reseal, correlation)
    }

    /// Open under `from`, seal under the current epoch, open the result,
    /// compare, and hand back only the new envelope. The plaintext is
    /// dropped here and never leaves this function.
    #[allow(clippy::too_many_arguments)]
    fn reseal_one(
        &self,
        lease: &KeyLease,
        population: Population,
        at: &str,
        entity: &str,
        field: &str,
        anchor: &str,
        old: &[u8],
    ) -> Result<Vec<u8>, ResealError> {
        // The value moves to a new epoch and stays at the anchor it was
        // sealed at: the label is read off the row (or the payload), never
        // made up here, so a reseal cannot quietly re-home a value.
        let plain = open_label(self.keys, lease, entity, field, anchor, old)?;
        let new = seal_label(self.keys, lease, entity, field, anchor, &plain)?;
        let back = open_label(self.keys, lease, entity, field, anchor, &new)?;
        if back != plain || ordo_crypto::envelope_epoch(&new) == ordo_crypto::envelope_epoch(old) {
            return Err(ResealError::Verify {
                population,
                at: at.to_string(),
            });
        }
        Ok(new)
    }

    // ── payloads ──

    async fn read_payloads(
        &self,
        tx: &TxConn,
        from: Epoch,
        cursor: Option<&str>,
    ) -> Result<Vec<Item>, ResealError> {
        let after = match cursor {
            Some(c) => format!("AND \"ref\" > {}", bytea(&unhex(c)?)),
            None => String::new(),
        };
        let rows = tx
            .rows(&format!(
                "SELECT encode(\"ref\", 'hex'), \"entity\", \"field\", encode(\"payload\", 'hex'), \
                 \"anchor\" \
                 FROM {} WHERE {} = {} AND \"epoch\" = {from} {after} \
                 ORDER BY \"ref\" LIMIT {}",
                q(SEALED_PAYLOAD_TABLE),
                q(SEALED_SPACE_COLUMN),
                sql_lit(&self.space.0),
                self.chunk.max(1)
            ))
            .await?;
        rows.iter()
            .map(|r| {
                let cell = |i: usize| r.get(i).and_then(|c| c.as_deref()).unwrap_or("");
                Ok(Item {
                    at: cell(0).to_string(),
                    entity: cell(1).to_string(),
                    field: cell(2).to_string(),
                    anchor: cell(4).to_string(),
                    envelopes: vec![Some(unhex(cell(3))?)],
                })
            })
            .collect()
    }

    async fn move_payloads(
        &self,
        tx: &TxConn,
        lease: &KeyLease,
        from: Epoch,
        items: Vec<Item>,
    ) -> Result<Chunk, ResealError> {
        let mut chunk = Chunk {
            done: 0,
            skipped: 0,
            last: None,
            digest: String::new(),
        };
        let mut hasher = Sha256::new();
        for item in items {
            let old = item.envelopes[0].as_deref().unwrap_or_default();
            let new = self.reseal_one(
                lease,
                Population::Payloads,
                &item.at,
                &item.entity,
                &item.field,
                &item.anchor,
                old,
            )?;
            let epoch = ordo_crypto::envelope_epoch(&new).unwrap_or(from);
            let updated = tx
                .rows(&format!(
                    "UPDATE {} SET \"payload\" = {}, \"epoch\" = {epoch} \
                     WHERE {} = {} AND \"ref\" = {} AND \"payload\" = {} RETURNING \"ref\"",
                    q(SEALED_PAYLOAD_TABLE),
                    bytea(&new),
                    q(SEALED_SPACE_COLUMN),
                    sql_lit(&self.space.0),
                    bytea(&unhex(&item.at)?),
                    bytea(old),
                ))
                .await?;
            tally(&mut chunk, &mut hasher, updated.len(), &item.at, &new);
            chunk.last = Some(item.at);
        }
        chunk.digest = ordo_crypto::hex(&hasher.finalize());
        Ok(chunk)
    }

    // ── columns ──

    fn pairs(&self) -> Vec<(String, String)> {
        sealed_columns(self.schema)
    }

    fn next_pair(&self, entity: &str, field: &str) -> Option<(String, String)> {
        let pairs = self.pairs();
        let i = pairs.iter().position(|(e, f)| e == entity && f == field)?;
        pairs.get(i + 1).cloned()
    }

    /// Resolve the cursor to a `(entity, field)` pair and the id to read
    /// after. A cursor naming a pair the schema no longer has restarts
    /// from the first pair: rows already moved do not match the predicate
    /// again, so a restart costs a scan and never a clobber.
    fn column_position(&self, cursor: Option<&str>) -> Option<((String, String), String)> {
        let pairs = self.pairs();
        match cursor.and_then(parse_column_cursor) {
            Some((e, f, id)) if pairs.iter().any(|(pe, pf)| *pe == e && *pf == f) => {
                Some(((e, f), id))
            }
            _ => pairs.first().cloned().map(|p| (p, String::new())),
        }
    }

    #[allow(clippy::type_complexity)]
    async fn read_columns(
        &self,
        tx: &TxConn,
        from: Epoch,
        cursor: Option<&str>,
    ) -> Result<(Option<(String, String)>, Vec<Item>), ResealError> {
        let Some(((entity, field), after)) = self.column_position(cursor) else {
            return Ok((None, Vec::new()));
        };
        let rows = tx
            .rows(&format!(
                "SELECT \"id\", encode({f}, 'hex') FROM {e} WHERE {sp} = {s} AND {f} IS NOT NULL \
                 AND {pred} AND \"id\" > {after} ORDER BY \"id\" LIMIT {n}",
                e = q(&entity),
                f = q(&field),
                sp = q(ordo_schema::SPACE_COLUMN),
                s = sql_lit(&self.space.0),
                pred = under(&q(&field), from),
                after = sql_lit(&after),
                n = self.chunk.max(1)
            ))
            .await?;
        let items = rows
            .iter()
            .map(|r| {
                let cell = |i: usize| r.get(i).and_then(|c| c.as_deref()).unwrap_or("");
                Ok(Item {
                    at: cell(0).to_string(),
                    entity: entity.clone(),
                    field: field.clone(),
                    anchor: ordo_crypto::Anchor::Row(cell(0)).label(),
                    envelopes: vec![Some(unhex(cell(1))?)],
                })
            })
            .collect::<Result<Vec<_>, ResealError>>()?;
        Ok((Some((entity, field)), items))
    }

    async fn move_columns(
        &self,
        tx: &TxConn,
        lease: &KeyLease,
        _from: Epoch,
        entity: &str,
        field: &str,
        items: Vec<Item>,
    ) -> Result<Chunk, ResealError> {
        let mut chunk = Chunk {
            done: 0,
            skipped: 0,
            last: None,
            digest: String::new(),
        };
        let mut hasher = Sha256::new();
        for item in items {
            let old = item.envelopes[0].as_deref().unwrap_or_default();
            let at = format!("{entity}.{field}#{}", item.at);
            let new = self.reseal_one(
                lease,
                Population::Columns,
                &at,
                entity,
                field,
                &item.anchor,
                old,
            )?;
            let updated = tx
                .rows(&format!(
                    "UPDATE {e} SET {f} = {new} WHERE {sp} = {s} AND \"id\" = {id} AND {f} = {old} \
                     RETURNING \"id\"",
                    e = q(entity),
                    f = q(field),
                    new = bytea(&new),
                    sp = q(ordo_schema::SPACE_COLUMN),
                    s = sql_lit(&self.space.0),
                    id = sql_lit(&item.at),
                    old = bytea(old),
                ))
                .await?;
            tally(&mut chunk, &mut hasher, updated.len(), &item.at, &new);
            chunk.last = Some(column_cursor(entity, field, &item.at));
        }
        chunk.digest = ordo_crypto::hex(&hasher.finalize());
        Ok(chunk)
    }

    // ── blob references ──

    async fn read_blobrefs(
        &self,
        tx: &TxConn,
        from: Epoch,
        cursor: Option<&str>,
    ) -> Result<Vec<Item>, ResealError> {
        let after = match cursor {
            Some(c) => format!("AND \"ref\" > {}", bytea(&unhex(c)?)),
            None => String::new(),
        };
        let rows = tx
            .rows(&format!(
                "SELECT encode(\"ref\", 'hex'), \"entity\", \"field\", encode(\"mime\", 'hex'), \
                 encode(\"size\", 'hex'), encode(\"integrity\", 'hex') \
                 FROM {} WHERE {} = {} AND ({} OR {} OR {}) {after} \
                 ORDER BY \"ref\" LIMIT {}",
                q(BLOB_REF_TABLE),
                q(BLOB_SPACE_COLUMN),
                sql_lit(&self.space.0),
                under("\"mime\"", from),
                under("\"size\"", from),
                under("\"integrity\"", from),
                self.chunk.max(1)
            ))
            .await?;
        rows.iter()
            .map(|r| {
                let cell = |i: usize| r.get(i).and_then(|c| c.as_deref()).unwrap_or("");
                let env = |i: usize| -> Result<Option<Vec<u8>>, ResealError> {
                    let b = unhex(cell(i))?;
                    Ok((ordo_crypto::envelope_epoch(&b) == Some(from)).then_some(b))
                };
                Ok(Item {
                    at: cell(0).to_string(),
                    entity: cell(1).to_string(),
                    field: cell(2).to_string(),
                    // A reference's attributes are bound to the object they
                    // describe: the handle, which is the row's own key.
                    anchor: ordo_crypto::Anchor::Object(cell(0)).label(),
                    envelopes: vec![env(3)?, env(4)?, env(5)?],
                })
            })
            .collect()
    }

    async fn move_blobrefs(
        &self,
        tx: &TxConn,
        lease: &KeyLease,
        _from: Epoch,
        items: Vec<Item>,
    ) -> Result<Chunk, ResealError> {
        let mut chunk = Chunk {
            done: 0,
            skipped: 0,
            last: None,
            digest: String::new(),
        };
        let mut hasher = Sha256::new();
        for item in items {
            let mut sets = Vec::new();
            let mut cas = Vec::new();
            let mut digest_input = Vec::new();
            for (col, old) in ["mime", "size", "integrity"].iter().zip(&item.envelopes) {
                let Some(old) = old else { continue };
                let new = self.reseal_one(
                    lease,
                    Population::BlobRefs,
                    &format!("{}:{col}", item.at),
                    &item.entity,
                    &item.field,
                    &item.anchor,
                    old,
                )?;
                sets.push(format!("{} = {}", q(col), bytea(&new)));
                cas.push(format!("{} = {}", q(col), bytea(old)));
                digest_input.extend_from_slice(&new);
            }
            if sets.is_empty() {
                chunk.last = Some(item.at);
                continue;
            }
            let updated = tx
                .rows(&format!(
                    "UPDATE {} SET {} WHERE {} = {} AND \"ref\" = {} AND {} RETURNING \"ref\"",
                    q(BLOB_REF_TABLE),
                    sets.join(", "),
                    q(BLOB_SPACE_COLUMN),
                    sql_lit(&self.space.0),
                    bytea(&unhex(&item.at)?),
                    cas.join(" AND "),
                ))
                .await?;
            tally(
                &mut chunk,
                &mut hasher,
                updated.len(),
                &item.at,
                &digest_input,
            );
            chunk.last = Some(item.at);
        }
        chunk.digest = ordo_crypto::hex(&hasher.finalize());
        Ok(chunk)
    }
}

impl Reseal<'_> {
    // ── objects ──

    async fn read_objects(
        &self,
        tx: &TxConn,
        from: Epoch,
        cursor: Option<&str>,
    ) -> Result<Vec<ObjectItem>, ResealError> {
        let after = match cursor {
            Some(c) => format!("AND \"ref\" > {}", bytea(&unhex(c)?)),
            None => String::new(),
        };
        let rows = tx
            .rows(&format!(
                "SELECT encode(\"ref\", 'hex'), \"entity\", \"field\", encode(\"mime\", 'hex'), \
                 encode(\"size\", 'hex'), encode(\"integrity\", 'hex'), \"store\" \
                 FROM {} WHERE {} = {} AND \"epoch\" = {from} {after} \
                 ORDER BY \"ref\" LIMIT {}",
                q(BLOB_REF_TABLE),
                q(BLOB_SPACE_COLUMN),
                sql_lit(&self.space.0),
                self.chunk.max(1)
            ))
            .await?;
        rows.iter()
            .map(|r| {
                let cell = |i: usize| r.get(i).and_then(|c| c.as_deref()).unwrap_or("");
                Ok(ObjectItem {
                    at: cell(0).to_string(),
                    entity: cell(1).to_string(),
                    field: cell(2).to_string(),
                    mime: unhex(cell(3))?,
                    size: unhex(cell(4))?,
                    integrity: unhex(cell(5))?,
                    store: cell(6).to_string(),
                })
            })
            .collect()
    }

    /// Move each object to a new handle, or skip it when no live row names
    /// it (it retires through `blobgc`).
    ///
    /// The whole object is held in memory for the read-open-seal-write,
    /// which is the shape §1.3 names; a streaming re-seal would need the
    /// store to accept a body whose length it already knows, which it
    /// does, but at the cost of holding two leases across a network write.
    /// Objects are bounded by the blob policy's cap, so the memory is the
    /// cap, once, per chunk row.
    async fn move_objects(
        &self,
        tx: &TxConn,
        stores: &dyn ordo_blob::StoreResolver,
        lease: &KeyLease,
        from: Epoch,
        items: Vec<ObjectItem>,
        now: u64,
    ) -> Result<Chunk, ResealError> {
        use ordo_crypto::stream::{self, StreamHeader, HEADER, SEALED_CHUNK};

        let mut chunk = Chunk {
            done: 0,
            skipped: 0,
            last: None,
            digest: String::new(),
        };
        let mut hasher = Sha256::new();
        // The new object goes where the space writes now, and its row
        // records that name. The old one is read from where its own row
        // says it was written (ruled 2026-10-05, `detach-upload-race` ask
        // 2), which after a detach or a replace is not the same bucket.
        let (store, store_name) = stores
            .current(self.space)
            .await
            .map_err(|e| ResealError::Store(e.to_string()))?;
        for item in items {
            let from_store = stores
                .named(self.space, &item.store)
                .await
                .map_err(|e| ResealError::Store(e.to_string()))?;
            let old_handle = parse_handle(&item.at)?;
            let old_hex = ordo_crypto::hex(&old_handle);
            // Only a live reference is moved. An old pair nobody names is
            // `blobgc`'s to retire, and moving it would mint a second
            // orphan.
            let live = tx
                .rows(&format!(
                    "SELECT 1 FROM {e} WHERE {sp} = {s} AND {f} = {h} LIMIT 1",
                    e = q(&item.entity),
                    sp = q(ordo_schema::SPACE_COLUMN),
                    s = sql_lit(&self.space.0),
                    f = q(&item.field),
                    h = sql_lit(&old_hex),
                ))
                .await?;
            if live.is_empty() {
                chunk.skipped += 1;
                chunk.last = Some(item.at);
                continue;
            }

            let at = format!("{}.{}#{}", item.entity, item.field, item.at);
            let plain_len: u64 = String::from_utf8(self.keys.open_at(
                lease,
                &item.entity,
                &item.field,
                ordo_crypto::Anchor::Object(&old_hex),
                &item.size,
            )?)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| ResealError::Malformed { at: at.clone() })?;

            // Read and open under `n`.
            let stored = ordo_blob::collect(
                from_store
                    .get(self.space, &old_handle, None)
                    .await
                    .map_err(|e| ResealError::Store(e.to_string()))?,
            )
            .await
            .map_err(|e| ResealError::Store(e.to_string()))?;
            let header = StreamHeader::decode(stored.get(..HEADER).unwrap_or_default())
                .map_err(|_| ResealError::Malformed { at: at.clone() })?;
            if header.epoch != from {
                return Err(ResealError::Malformed { at });
            }
            let chunks = stream::chunk_count(plain_len);
            let mut plain = Vec::with_capacity(plain_len as usize);
            let mut pos = HEADER;
            let mut counter = 0u32;
            while pos < stored.len() {
                let take = SEALED_CHUNK.min(stored.len() - pos);
                let last = counter as u64 + 1 == chunks;
                plain.extend_from_slice(&self.keys.open_chunk(
                    lease,
                    &item.entity,
                    &item.field,
                    ordo_crypto::Anchor::Object(&old_hex),
                    from,
                    &header.base,
                    counter,
                    last,
                    &stored[pos..pos + take],
                )?);
                pos += take;
                counter += 1;
            }
            if plain.len() as u64 != plain_len {
                return Err(ResealError::Verify {
                    population: Population::Objects,
                    at,
                });
            }

            // Seal under the current epoch into a new object.
            let to = self.keys.current_epoch(self.space)?;
            let new_header = StreamHeader::new(to);
            let new_handle = self.keys.mint_blob_ref(lease, to)?;
            // The object moves to a new handle, so what is sealed for it
            // is sealed again at that handle: the chunks below and the
            // three attributes further down.
            let new_hex = ordo_crypto::hex(&new_handle);
            let mut out = Vec::with_capacity(stream::sealed_len(plain_len) as usize);
            out.extend_from_slice(&new_header.encode());
            let mut sealed_chunks = Vec::new();
            for (i, piece) in chunked(&plain, stream::CHUNK, chunks)
                .into_iter()
                .enumerate()
            {
                let last = i as u64 + 1 == chunks;
                let sealed = self.keys.seal_chunk(
                    lease,
                    &item.entity,
                    &item.field,
                    ordo_crypto::Anchor::Object(&new_hex),
                    to,
                    &new_header.base,
                    i as u32,
                    last,
                    piece,
                )?;
                sealed_chunks.push(sealed);
            }
            // Verify in memory before anything is written: every chunk
            // opens back to what was sealed.
            let mut back = Vec::with_capacity(plain.len());
            for (i, sealed) in sealed_chunks.iter().enumerate() {
                let last = i as u64 + 1 == chunks;
                back.extend_from_slice(&self.keys.open_chunk(
                    lease,
                    &item.entity,
                    &item.field,
                    ordo_crypto::Anchor::Object(&new_hex),
                    to,
                    &new_header.base,
                    i as u32,
                    last,
                    sealed,
                )?);
                out.extend_from_slice(sealed);
            }
            if back != plain {
                return Err(ResealError::Verify {
                    population: Population::Objects,
                    at,
                });
            }
            drop(plain);
            drop(back);

            let bytes = out.len() as i64;
            store
                .put(
                    self.space,
                    &new_handle,
                    out.len() as u64,
                    ordo_blob::once(out),
                )
                .await
                .map_err(|e| ResealError::Store(e.to_string()))?;

            // The three envelopes move with the object. Each is opened at
            // the old handle and sealed at the new one, whichever epoch it
            // is under: the binding names the handle, so an envelope
            // carried over as it was would no longer open. The seal is
            // under the current epoch, which is where the new row sits.
            let old_label = ordo_crypto::Anchor::Object(&old_hex).label();
            let new_label = ordo_crypto::Anchor::Object(&new_hex).label();
            let mut envs = Vec::with_capacity(3);
            for (col, old) in [
                ("mime", &item.mime),
                ("size", &item.size),
                ("integrity", &item.integrity),
            ] {
                let plain =
                    open_label(self.keys, lease, &item.entity, &item.field, &old_label, old)?;
                let new = seal_label(
                    self.keys,
                    lease,
                    &item.entity,
                    &item.field,
                    &new_label,
                    &plain,
                )?;
                let back = open_label(
                    self.keys,
                    lease,
                    &item.entity,
                    &item.field,
                    &new_label,
                    &new,
                )?;
                if back != plain {
                    return Err(ResealError::Verify {
                        population: Population::Objects,
                        at: format!("{at}:{col}"),
                    });
                }
                envs.push(new);
            }
            tx.rows(&ordo_crypto::blobref::insert_sql(
                self.space,
                &new_handle,
                to,
                &item.entity,
                &item.field,
                &envs[0],
                &envs[1],
                &envs[2],
                bytes,
                now as i64,
                &store_name,
            ))
            .await?;
            let updated = tx
                .rows(&format!(
                    "UPDATE {e} SET {f} = {new} WHERE {sp} = {s} AND {f} = {old} RETURNING \"id\"",
                    e = q(&item.entity),
                    f = q(&item.field),
                    new = sql_lit(&ordo_crypto::hex(&new_handle)),
                    sp = q(ordo_schema::SPACE_COLUMN),
                    s = sql_lit(&self.space.0),
                    old = sql_lit(&old_hex),
                ))
                .await?;
            tally(
                &mut chunk,
                &mut hasher,
                updated.len(),
                &item.at,
                &new_handle,
            );
            chunk.last = Some(item.at);
        }
        chunk.digest = ordo_crypto::hex(&hasher.finalize());
        Ok(chunk)
    }
}

/// One `$blobref` row read for the object population.
struct ObjectItem {
    at: String,
    entity: String,
    field: String,
    mime: Vec<u8>,
    size: Vec<u8>,
    integrity: Vec<u8>,
    /// The store the old object was written to.
    store: String,
}

/// Plaintext in `size`-byte pieces, always at least one piece so an empty
/// object still seals its one (empty, last) chunk.
fn chunked(plain: &[u8], size: usize, chunks: u64) -> Vec<&[u8]> {
    let mut out: Vec<&[u8]> = plain.chunks(size).collect();
    while (out.len() as u64) < chunks {
        out.push(&[]);
    }
    out
}

fn parse_handle(hex: &str) -> Result<ordo_crypto::BlobHandle, ResealError> {
    ordo_crypto::blobref::parse_handle(hex).ok_or_else(|| ResealError::Malformed {
        at: hex.to_string(),
    })
}

/// Count one CAS outcome and feed the chunk digest. The digest is over
/// `(ref ‖ new envelope)`: fresh nonces make equal plaintexts contribute
/// unrelated bytes, which is what makes it storable.
fn tally(chunk: &mut Chunk, hasher: &mut Sha256, updated: usize, at: &str, new: &[u8]) {
    if updated == 0 {
        chunk.skipped += 1;
    } else {
        chunk.done += 1;
    }
    hasher.update(at.as_bytes());
    hasher.update(new);
}

/// The column cursor: `entity`, `field` and the last id, joined by a unit
/// separator no identifier can contain.
const SEP: char = '\u{1f}';

fn column_cursor(entity: &str, field: &str, id: &str) -> String {
    format!("{entity}{SEP}{field}{SEP}{id}")
}

fn parse_column_cursor(c: &str) -> Option<(String, String, String)> {
    let mut it = c.splitn(3, SEP);
    Some((it.next()?.into(), it.next()?.into(), it.next()?.into()))
}

/// Postgres's hex `bytea` rendering under `encode(…, 'hex')`, which has
/// no `\x` prefix, unlike the bare-column rendering `sealed::unhex` reads.
fn unhex(hex: &str) -> Result<Vec<u8>, ResealError> {
    ordo_crypto::sealed::unhex(&format!("\\x{hex}")).ok_or_else(|| ResealError::Malformed {
        at: hex.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_cursor_round_trips() {
        let c = column_cursor("orders", "memo", "ord_10");
        assert_eq!(
            parse_column_cursor(&c),
            Some(("orders".into(), "memo".into(), "ord_10".into()))
        );
        assert_eq!(
            parse_column_cursor(&column_cursor("a", "b", "")),
            Some(("a".into(), "b".into(), String::new()))
        );
    }

    /// The predicate never calls `get_byte` on an empty value, and no
    /// statement in this module names a blind-index column.
    #[test]
    fn the_predicate_guards_length_and_names_no_bidx() {
        let p = under("\"memo\"", 3);
        assert!(p.starts_with("(length(\"memo\") > 0 AND"));
        let src = include_str!("reseal.rs");
        assert!(
            !src.contains("$bidx\""),
            "a re-seal must not touch a blind-index column"
        );
    }

    /// The pass table's DDL is per space and keyed per population.
    #[test]
    fn the_pass_table_is_keyed_per_population() {
        assert!(RESEAL_DDL.contains("PRIMARY KEY (\"space\", \"from_epoch\", \"population\")"));
        for p in Population::ALL {
            assert_eq!(Population::parse(p.as_str()), Some(p));
        }
    }
}
