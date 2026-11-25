//! **The move overlay** (`plan-backfill-dual-writes.md` §3, step B2;
//! `plan-versions.md` increment 11a): dual writes in the write path.
//!
//! While one tenant moves from version X to version Y through a step that
//! rewrites data (a rename, a filled field), both shapes stay current:
//!
//! - **Forward**, while X serves (the backfill, then until the switch):
//!   every row X writes to a changed entity also gets Y's new columns, by
//!   the step's forward mapping.
//! - **Reverse**, while Y serves (the soak): every row Y writes also gets
//!   X's old columns, by the reverse mapping, so a revert is a pin switch.
//!
//! The overlay runs **inside the write's own transaction**, after the op's
//! statements and before the chain append (`crate::Executor`'s attempt), so
//! the effect, the extra columns and the link commit together or not at
//! all. It adds no command to the group: the audit chain records the write
//! once (D10, ruled 2026-11-02).
//!
//! **Why the worker and a live write cannot disagree.** Both compute the
//! new columns from the row's own columns with the same mapping, under the
//! row's lock. Whichever runs second writes the same value.
//!
//! **Both versions' guards.** After the extra columns are set, each written
//! row is checked against the other version's rules the database does not
//! enforce during the move (a required column the move left open to
//! nulls). A row that breaks one refuses the whole write, naming both
//! versions, the field and the row.
//!
//! The overlay is data, rendered by the host from the stored plan
//! (`ordo_host::dual_writes`): SQL over the row's own columns, never SQL
//! from a person. One overlay per (version, step) is shared by every tenant
//! in that move ([`MoveOverlay`] behind an `Arc`); a handle carries it
//! ([`crate::Pool::with_overlay`]).

use crate::{sql_lit, PoolError, TxConn};

/// **`$renames`**: the field renames a database is in the middle of
/// (increment 11a). A rename's step writes its row in the transaction that
/// adds the new column, after `LOCK TABLE "$renames" IN EXCLUSIVE MODE`.
/// Every write locks and reads the table first ([`renames_in`]), and keeps
/// both columns current
/// ([`MoveOverlay::for_renames`]).
///
/// Why a write can never miss a rename: the write's `ROW SHARE` lock is
/// taken before its snapshot, and the rename's `EXCLUSIVE` lock conflicts
/// with it.
/// A write that read the table before the rename holds its lock to its
/// commit, so the rename's new column is added after that row exists, and
/// the rename's copy reaches it. A write after the rename reads its row.
/// No node needs to have read anything: the database says.
///
/// A row leaves when the rename is reversed, or when its old column is
/// orphaned at a soak's close.
pub const RENAMES_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$renames\" ( \
  \"entity\" text NOT NULL, \
  \"from\" text NOT NULL, \
  \"field\" text NOT NULL, \
  \"column\" text NOT NULL, \
  \"version\" text NOT NULL, \
  PRIMARY KEY (\"entity\", \"from\") \
)";

/// One row of `$renames`: `entity.from` renamed to `entity.field`, whose
/// column is `column`. `from` is also the old column.
///
/// **An entity rename** (`(prev: from)`) has an empty `field` and
/// `column`: `entity` is the new table and `from` the old one
/// ([`Rename::is_entity`], [`entity_mirrors`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub entity: String,
    pub from: String,
    pub field: String,
    pub column: String,
}

impl Rename {
    /// Whether the row is an entity rename: `from` is the old table and
    /// `entity` the new one.
    pub fn is_entity(&self) -> bool {
        self.field.is_empty()
    }
}

/// **An entity rename's dual write** (`plan-versions.md` increment 15,
/// built with the move of an entity): the rows this version writes to its
/// table, copied whole into the other name's table, and the rows it
/// deletes, deleted there too, in the write's own transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityMirror {
    /// The table this version writes: its entity's name.
    pub table: String,
    /// The table kept current beside it.
    pub other: String,
    /// Every physical column, the same in both tables: the step refuses an
    /// entity rename that also changes its columns
    /// (`rename_entity_reshaped`).
    pub columns: Vec<String>,
}

/// The entity mirrors a version writes with, for the entity renames a
/// database is in the middle of ([`RENAMES_DDL`]). A version that declares
/// the new name and not the old copies into the old table, so the version
/// before reads every row whole and a revert loses nothing; one that
/// declares the old name and not the new copies forward.
pub fn entity_mirrors(schema: &ordo_schema::Schema, renames: &[Rename]) -> Vec<EntityMirror> {
    renames
        .iter()
        .filter(|r| r.is_entity())
        .filter_map(|r| {
            let (e, other) = match (schema.native(&r.entity), schema.native(&r.from)) {
                (Some(e), None) => (e, &r.from),
                (None, Some(e)) => (e, &r.entity),
                _ => return None,
            };
            Some(EntityMirror {
                table: e.name.clone(),
                other: other.clone(),
                columns: ordo_schema::ddl::physical_columns(e)
                    .into_iter()
                    .map(|c| c.name)
                    .collect(),
            })
        })
        .collect()
}

/// Run each mirror over the rows this write wrote (`written`) and deleted
/// (`deleted`), `(entity, row id)`, in `space`, inside `tx`.
///
/// A written row is copied whole (`INSERT … ON CONFLICT DO UPDATE`), so the
/// worker's copy and a live write cannot disagree: whichever runs second
/// writes the row as it stands. A deleted row leaves the other table too.
/// A key a unique there already holds is refused by name
/// ([`OverlayError::Duplicate`]).
pub async fn apply_mirrors(
    tx: &TxConn,
    space: &str,
    mirrors: &[EntityMirror],
    written: &[(String, String)],
    deleted: &[(String, String)],
) -> Result<(), OverlayError> {
    for m in mirrors {
        let ids = |list: &[(String, String)]| {
            let mut ids: Vec<String> = list
                .iter()
                .filter(|(entity, _)| *entity == m.table)
                .map(|(_, id)| id.clone())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        let cols = m
            .columns
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        let sets = m
            .columns
            .iter()
            .filter(|c| c.as_str() != "$space" && c.as_str() != "id")
            .map(|c| format!("{} = EXCLUDED.{}", quote(c), quote(c)))
            .collect::<Vec<_>>()
            .join(", ");
        let written = ids(written);
        for chunk in written.chunks(IDS_PER_STATEMENT) {
            let list = chunk
                .iter()
                .map(|i| sql_lit(i))
                .collect::<Vec<_>>()
                .join(", ");
            let conflict = if sets.is_empty() {
                "DO NOTHING".to_string()
            } else {
                format!("DO UPDATE SET {sets}")
            };
            let sql = format!(
                "INSERT INTO {other} ({cols}) SELECT {cols} FROM {t} \
                 WHERE \"$space\" = {} AND \"id\" IN ({list}) \
                 ON CONFLICT (\"$space\", \"id\") {conflict}",
                sql_lit(space),
                other = quote(&m.other),
                t = quote(&m.table),
            );
            match tx.rows(&sql).await {
                Ok(_) => {}
                Err(err) if err.is_unique_violation() => {
                    return Err(OverlayError::Duplicate(Box::new(Duplicate {
                        entity: m.other.clone(),
                        columns: Vec::new(),
                        rows: chunk.to_vec(),
                    })));
                }
                Err(err) => return Err(err.into()),
            }
        }
        let deleted = ids(deleted);
        for chunk in deleted.chunks(IDS_PER_STATEMENT) {
            let list = chunk
                .iter()
                .map(|i| sql_lit(i))
                .collect::<Vec<_>>()
                .join(", ");
            tx.rows(&format!(
                "DELETE FROM {} WHERE \"$space\" = {} AND \"id\" IN ({list})",
                quote(&m.other),
                sql_lit(space),
            ))
            .await?;
        }
    }
    Ok(())
}

/// Read `$renames` under a share lock, as a write's first statements.
///
/// The lock is its own statement, `LOCK TABLE`, which takes no snapshot.
/// A `SELECT … FOR SHARE` would take the transaction's snapshot before it
/// waited on a rename's lock, and under `SERIALIZABLE` the write would then
/// not see the rename that committed while it waited, though its insert
/// would land beside the new column.
pub async fn renames_in(tx: &TxConn) -> Result<Vec<Rename>, PoolError> {
    tx.rows("LOCK TABLE \"$renames\" IN ROW SHARE MODE").await?;
    let rows = tx
        .rows(
            "SELECT \"entity\", \"from\", \"field\", \"column\" FROM \"$renames\" \
             ORDER BY \"entity\", \"from\"",
        )
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            Some(Rename {
                entity: it.next()??,
                from: it.next()??,
                field: it.next()??,
                column: it.next()??,
            })
        })
        .collect())
}

/// **A write's turn alone** (Reported fix, 2026-11-24): take `"$renames"`
/// in `EXCLUSIVE` mode as the transaction's first statement, before
/// [`renames_in`].
///
/// Every write holds `"$renames"` in `ROW SHARE` mode from its first
/// statement, so this waits for the writes in flight on this database to
/// end, and holds new ones at their first statement until it commits.
/// Postgres queues lock requests in order, so a write waiting here is not
/// overtaken. Its snapshot is taken after the writes before it committed,
/// and no write runs beside it, so no other write can make it the loser of
/// a serialization conflict.
///
/// The executor takes this turn only for a write that already lost to
/// other writes ([`crate::Executor`]'s retry loop). Without it, a stream of
/// writes could cancel the same write on every try. Writes during a seal
/// did: the seal overlay ([`apply_seals`]) reads back each row it wrote
/// through the primary key, Postgres locks that read at the index page, and
/// every other write to the table touches the same page, so two concurrent
/// writers each depend on the other and one is cancelled.
pub async fn take_turn(tx: &TxConn) -> Result<(), PoolError> {
    tx.rows("LOCK TABLE \"$renames\" IN EXCLUSIVE MODE").await?;
    Ok(())
}

/// **`$fills`**: the fills a shared database is in the middle of
/// (increment 11b part 4). A move on the deployment's own database that
/// fills a field (B2, a new required field; B5, an optional field made
/// required) records each fill here, under `"$renames"` in `EXCLUSIVE`
/// mode, before its copy starts. Every write already holds `"$renames"` in
/// `ROW SHARE` mode from its first statement, reads this table under the
/// same fence ([`fills_in`]), and, while its version does not require the
/// field, sets the column by the fill's mapping on every row it wrote
/// ([`MoveOverlay::for_database`]). So every space there, whatever version
/// it serves, keeps the new column current, as `$renames` keeps a rename's.
///
/// - `sql`: what the column is set to, over the row's own columns (a B5
///   fill keeps what the row holds: `coalesce`).
/// - `from`, `to`: the step's versions, named when a write breaks `to`'s
///   `required`.
///
/// The row leaves when the step's soaks there close together, after its
/// `NOT NULL` lands.
pub const FILLS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$fills\" ( \
  \"entity\" text NOT NULL, \
  \"field\" text NOT NULL, \
  \"column\" text NOT NULL, \
  \"sql\" text NOT NULL, \
  \"from\" text NOT NULL, \
  \"to\" text NOT NULL, \
  PRIMARY KEY (\"entity\", \"field\") \
)";

/// The columns a split or a merge (increment 15b) adds to `$fills`, on a
/// table made before them: `required`, whether a write must leave the
/// column set (null reads as yes), and `absent`, whether only a version
/// that does not declare the field sets it (null reads as no: a B5 fill is
/// set by the version that declares the field optional).
///
/// A split or merge records each new field (set by the version before,
/// which does not declare it) and each old field it keeps current (set by
/// the new version, which does not declare it, by the reverse).
pub const FILLS_MAP_DDL: &str = "ALTER TABLE \"$fills\" ADD COLUMN IF NOT EXISTS \"required\" \
     boolean NULL, ADD COLUMN IF NOT EXISTS \"absent\" boolean NULL";

/// One row of `$fills`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fill {
    pub entity: String,
    pub field: String,
    pub column: String,
    pub sql: String,
    pub from: String,
    pub to: String,
    /// A write must leave the column set ([`FILLS_MAP_DDL`]).
    pub required: bool,
    /// Only a version that does not declare the field sets it.
    pub absent: bool,
}

/// Read `$fills`. Run after [`renames_in`], whose lock fences this table
/// too.
pub async fn fills_in(tx: &TxConn) -> Result<Vec<Fill>, PoolError> {
    let rows = tx
        .rows(
            "SELECT f.\"entity\", f.\"field\", f.\"column\", f.\"sql\", f.\"from\", f.\"to\", \
             to_jsonb(f) ->> 'required', to_jsonb(f) ->> 'absent' FROM \"$fills\" f \
             ORDER BY f.\"entity\", f.\"field\"",
        )
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            Some(Fill {
                entity: it.next()??,
                field: it.next()??,
                column: it.next()??,
                sql: it.next()??,
                from: it.next()??,
                to: it.next()??,
                required: it.next().flatten().is_none_or(|v| v != "false"),
                absent: it.next().flatten().is_some_and(|v| v == "true"),
            })
        })
        .collect())
}

/// **`$held`**: the checks a new version adds that a shared database holds
/// to its close (increment 11b, `plan-backfill-dual-writes.md` §4). A move
/// on a database several spaces share records each one here, under
/// `"$renames"` in `EXCLUSIVE` mode, before its expand, as a fill records
/// itself ([`FILLS_DDL`]). Every write there reads it under the same fence
/// ([`held_in`]), so a space still on the version before is held to the
/// check on each row it writes, and refused by name, rather than leaving a
/// row the close would refuse. The row leaves when the check lands.
///
/// `holds` is SQL over the row's own columns in `"<entity>"`; `rule` and
/// `field` are the words a refusal uses.
pub const HELD_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$held\" ( \
  \"entity\" text NOT NULL, \
  \"name\" text NOT NULL, \
  \"holds\" text NOT NULL, \
  \"field\" text NOT NULL, \
  \"rule\" text NOT NULL, \
  \"from\" text NOT NULL, \
  \"to\" text NOT NULL, \
  PRIMARY KEY (\"entity\", \"name\") \
)";

/// One row of `$held`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldCheck {
    pub entity: String,
    pub name: String,
    pub holds: String,
    pub field: String,
    pub rule: String,
    pub from: String,
    pub to: String,
}

/// Read `$held`. Run after [`renames_in`], whose lock fences this table
/// too.
pub async fn held_in(tx: &TxConn) -> Result<Vec<HeldCheck>, PoolError> {
    let rows = tx
        .rows(
            "SELECT \"entity\", \"name\", \"holds\", \"field\", \"rule\", \"from\", \"to\" \
             FROM \"$held\" ORDER BY \"entity\", \"name\"",
        )
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            Some(HeldCheck {
                entity: it.next()??,
                name: it.next()??,
                holds: it.next()??,
                field: it.next()??,
                rule: it.next()??,
                from: it.next()??,
                to: it.next()??,
            })
        })
        .collect())
}

/// **`$seals`**: the sealed backfills a database is in the middle of
/// (increment 13): a field being sealed (B7), or a sealed field gaining its
/// blind index (B8). Recorded the way a rename is ([`RENAMES_DDL`]): the
/// step's expand takes `"$renames"` in `EXCLUSIVE` mode, adds the column
/// the worker fills, and writes the row here, in one transaction. Every
/// write already holds `"$renames"` in `ROW SHARE` mode from its first
/// statement, so it reads this table under the same fence
/// ([`seals_in`]) and fills the column for every row it wrote
/// ([`apply_seals`]), in its own transaction, with its own key custody.
///
/// - `kind` `seal`: `target` (a `bytea` beside the plaintext) holds the
///   row's `source` (the plaintext column) sealed at its row. Written by a
///   version that declares the field plaintext.
/// - `kind` `index`: `target` (`<field>$bidx`) holds the blind index of the
///   value sealed in `source`. Written by a version that declares the field
///   sealed and not indexed.
/// - `kind` `reindex` (B10): `target` (`<field>$bidx$next`) holds the blind
///   index of the value sealed in `source` under the coming normalization.
///   Written by every version that declares the field sealed: the one
///   before computes its own index under the normalization it declares.
///
/// `normalize` is the normalization an `index` or `reindex` target is
/// computed under (`lower`, or empty for none). Null on a row recorded
/// before the column existed: the writer's own declaration is used then.
/// [`SEALS_NORMALIZE_DDL`] adds it to a table made before it.
///
/// - `kind` `kept` (B10 after its end): `target` (`<field>$bidx$prev`)
///   holds the index under the normalization the row records, the one the
///   step replaced; `live` the one the field's own index is under. Written
///   by every sealed writer until the column is pruned; a rollback trades
///   it back in.
///
/// The row leaves in the transaction that lands the step's end (the
/// plaintext dropped, or the index made `NOT NULL`), under the same
/// exclusive lock.
pub const SEALS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$seals\" ( \
  \"entity\" text NOT NULL, \
  \"field\" text NOT NULL, \
  \"kind\" text NOT NULL, \
  \"source\" text NOT NULL, \
  \"target\" text NOT NULL, \
  \"version\" text NOT NULL, \
  \"normalize\" text NULL, \
  PRIMARY KEY (\"entity\", \"field\", \"kind\") \
)";

/// The `normalize` column of [`SEALS_DDL`], on a table made before it.
pub const SEALS_NORMALIZE_DDL: &str =
    "ALTER TABLE \"$seals\" ADD COLUMN IF NOT EXISTS \"normalize\" text NULL";

/// The `live` column of [`SEALS_DDL`]: on a `kept` row, the normalization
/// the field's own blind index is computed under now, so a trade of the
/// two indexes knows what the kept one becomes without being told which
/// version served before. Null on a row recorded before the column. Added
/// under the `"$renames"` lock, as every change to `$seals` is.
pub const SEALS_LIVE_DDL: &str =
    "ALTER TABLE \"$seals\" ADD COLUMN IF NOT EXISTS \"live\" text NULL";

/// What a sealed backfill in progress fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealKind {
    /// B7: the plaintext sealed at its row.
    Seal,
    /// B8: the sealed value's blind index.
    Index,
    /// B10: the sealed value's blind index under a new normalization.
    Reindex,
    /// B10, after its end: the old blind index, kept under the old
    /// normalization until its orphan is pruned, so a rollback reads a
    /// whole index.
    Kept,
}

impl SealKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SealKind::Seal => "seal",
            SealKind::Index => "index",
            SealKind::Reindex => "reindex",
            SealKind::Kept => "kept",
        }
    }

    pub fn parse(s: &str) -> Option<SealKind> {
        match s {
            "seal" => Some(SealKind::Seal),
            "index" => Some(SealKind::Index),
            "reindex" => Some(SealKind::Reindex),
            "kept" => Some(SealKind::Kept),
            _ => None,
        }
    }
}

/// One row of `$seals`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seal {
    pub entity: String,
    pub field: String,
    pub kind: SealKind,
    /// The column read: the plaintext (B7) or the sealed value (B8).
    pub source: String,
    /// The column filled.
    pub target: String,
    /// The normalization an index is computed under: `Some("")` for none,
    /// `None` when the row does not say (recorded before the column).
    pub normalize: Option<String>,
}

/// Read `$seals`. Run after [`renames_in`], whose lock fences this table
/// too. `normalize` is read through the row's JSON, so a table made before
/// that column reads as not saying.
pub async fn seals_in(tx: &TxConn) -> Result<Vec<Seal>, PoolError> {
    let rows = tx
        .rows(
            "SELECT s.\"entity\", s.\"field\", s.\"kind\", s.\"source\", s.\"target\", \
             to_jsonb(s) ->> 'normalize' FROM \"$seals\" s \
             ORDER BY s.\"entity\", s.\"field\", s.\"kind\"",
        )
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let mut it = r.into_iter();
            Some(Seal {
                entity: it.next()??,
                field: it.next()??,
                kind: SealKind::parse(&it.next()??)?,
                source: it.next()??,
                target: it.next()??,
                normalize: it.next().flatten(),
            })
        })
        .collect())
}

/// Why the seal overlay did not run.
#[derive(Debug)]
pub enum SealOverlayError {
    /// Custody refused: nothing was written.
    Keys(ordo_crypto::KeyError),
    Pool(PoolError),
}

impl From<PoolError> for SealOverlayError {
    fn from(e: PoolError) -> Self {
        SealOverlayError::Pool(e)
    }
}

impl From<ordo_crypto::KeyError> for SealOverlayError {
    fn from(e: ordo_crypto::KeyError) -> Self {
        SealOverlayError::Keys(e)
    }
}

impl std::fmt::Display for SealOverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SealOverlayError::Keys(e) => write!(
                f,
                "seal_overlay: a field of this write is being sealed, and its key could not be \
                 released ({e}). Nothing was written; try again"
            ),
            SealOverlayError::Pool(e) => write!(f, "{e}"),
        }
    }
}

/// The seals in progress a version writes into: a `seal` while it declares
/// the field plaintext, an `index` while it declares it sealed and not
/// indexed, a `reindex` while it declares it sealed. A version that already
/// declares the end shape of a `seal` or an `index` writes nothing more:
/// the step's end lands before it serves. `normalize` is no column, so no
/// version's shape shows a `reindex` done: every sealed writer fills its
/// target, under the recorded normalization.
pub fn seals_for<'a>(schema: &ordo_schema::Schema, seals: &'a [Seal]) -> Vec<&'a Seal> {
    seals
        .iter()
        .filter(|s| {
            let Some(f) = schema.native(&s.entity).and_then(|e| e.field(&s.field)) else {
                return false;
            };
            match s.kind {
                SealKind::Seal => !f.sealed,
                SealKind::Index => f.sealed && !f.indexed,
                SealKind::Reindex | SealKind::Kept => f.sealed,
            }
        })
        .collect()
}

/// **What a write's own statements returned, for the seal overlay**
/// (Reported fix, 2026-11-24, part 2).
///
/// While a seal runs, each `INSERT` or `UPDATE` the write sends to an
/// entity being sealed also returns `ctid`, `$space`, `id` and the seal's
/// source columns ([`SealCapture::rewrite`]). The overlay then seals from
/// those values and writes the target by `ctid` ([`apply_seals`]).
///
/// Why: reading the row back by primary key under SERIALIZABLE locks the
/// index page, and every other write to the table touches that page. Two
/// concurrent writers then each depend on the other, and one is cancelled.
/// A fetch by `ctid` of a row this transaction wrote takes no predicate
/// lock. A row not captured (a statement shape this does not rewrite)
/// falls back to the read by key.
#[derive(Debug, Default)]
pub struct SealCapture {
    /// Entity to the source columns returned for it.
    entities: std::collections::BTreeMap<String, Vec<String>>,
    /// `(entity, id)` to the row as last returned.
    rows: std::collections::BTreeMap<(String, String), CapturedRow>,
}

#[derive(Debug, Clone)]
struct CapturedRow {
    space: String,
    ctid: String,
    /// Source column to its value as text.
    values: std::collections::BTreeMap<String, Option<String>>,
}

impl SealCapture {
    /// The capture for the seals this version writes into. Empty when none.
    pub fn for_seals(seals: &[&Seal]) -> SealCapture {
        let mut entities: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for s in seals {
            let cols = entities.entry(s.entity.clone()).or_default();
            if !cols.contains(&s.source) {
                cols.push(s.source.clone());
            }
        }
        SealCapture {
            entities,
            rows: Default::default(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// How many rows were captured.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// `(entity, sql with a RETURNING clause)` when `sql` is one
    /// `INSERT INTO` or `UPDATE` of an entity being sealed and returns
    /// nothing yet. `None` leaves the statement as it is.
    pub fn rewrite(&self, sql: &str) -> Option<(String, String)> {
        if self.entities.is_empty() {
            return None;
        }
        let body = single_statement(sql)?;
        let (entity, cols) = self.entities.iter().find(|(e, _)| {
            let t = quote(e);
            body.starts_with(&format!("INSERT INTO {t} "))
                || body.starts_with(&format!("UPDATE {t} "))
        })?;
        let mut ret = format!(
            "{body} RETURNING ctid::text, {}::text, \"id\"::text",
            quote(ordo_schema::SPACE_COLUMN)
        );
        for c in cols {
            ret.push_str(&format!(", {}::text", quote(c)));
        }
        Some((entity.clone(), ret))
    }

    /// Record what a rewritten statement returned.
    pub fn record(&mut self, entity: &str, rows: &[Vec<Option<String>>]) {
        let Some(cols) = self.entities.get(entity) else {
            return;
        };
        for r in rows {
            let (Some(ctid), Some(space), Some(id)) = (
                r.first().cloned().flatten(),
                r.get(1).cloned().flatten(),
                r.get(2).cloned().flatten(),
            ) else {
                continue;
            };
            let values = cols
                .iter()
                .enumerate()
                .map(|(i, c)| (c.clone(), r.get(3 + i).cloned().flatten()))
                .collect();
            self.rows.insert(
                (entity.to_string(), id),
                CapturedRow {
                    space,
                    ctid,
                    values,
                },
            );
        }
    }
}

/// The statement without its final `;`, when `sql` is one statement with
/// no `RETURNING` and no escape-string literal. Quotes are tracked, so a
/// `;` inside a literal or an identifier is not taken for a statement end.
fn single_statement(sql: &str) -> Option<&str> {
    let trimmed = sql.trim();
    let body = trimmed.strip_suffix(';').unwrap_or(trimmed).trim_end();
    let (mut single, mut double) = (false, false);
    let mut prev = ' ';
    let mut top = String::with_capacity(body.len());
    for c in body.chars() {
        match c {
            '\'' if !double => {
                if !single && (prev == 'E' || prev == 'e') {
                    return None;
                }
                single = !single;
            }
            '"' if !single => double = !double,
            ';' if !single && !double => return None,
            _ => {}
        }
        if !single && !double {
            top.push(c.to_ascii_uppercase());
        }
        prev = c;
    }
    if single || double || top.contains("RETURNING") {
        return None;
    }
    Some(body)
}

/// One sealed target's value for one row, from its source as text.
struct SealValue<'a> {
    keys: &'a dyn ordo_crypto::KeyProvider,
    sid: ordo_crypto::SpaceId,
    lease: Option<ordo_crypto::KeyLease>,
}

impl SealValue<'_> {
    fn lease(&mut self) -> Result<&ordo_crypto::KeyLease, SealOverlayError> {
        if self.lease.is_none() {
            self.lease = Some(
                self.keys
                    .release(&self.sid, ordo_crypto::Purpose::Write, "seal")?,
            );
        }
        Ok(self.lease.as_ref().expect("taken just above"))
    }

    fn of(
        &mut self,
        s: &Seal,
        normalize: Option<&str>,
        id: &str,
        source: Option<String>,
    ) -> Result<String, SealOverlayError> {
        let keys = self.keys;
        let value: Option<Vec<u8>> = match (s.kind, source) {
            (SealKind::Seal, None) => None,
            (SealKind::Seal, Some(text)) => {
                let l = self.lease()?;
                Some(keys.seal_at(
                    l,
                    &s.entity,
                    &s.field,
                    ordo_crypto::Anchor::Row(id),
                    text.as_bytes(),
                )?)
            }
            (SealKind::Index | SealKind::Reindex | SealKind::Kept, sealed) => {
                let plain = match sealed {
                    None => None,
                    Some(hexed) => {
                        let l = self.lease()?;
                        let bytes = unhex(hexed.strip_prefix("\\x").unwrap_or(&hexed));
                        let open = keys.open_at(
                            l,
                            &s.entity,
                            &s.field,
                            ordo_crypto::Anchor::Row(id),
                            &bytes,
                        )?;
                        Some(String::from_utf8_lossy(&open).into_owned())
                    }
                };
                let normalized = plain.map(|v| match normalize {
                    Some("lower") => v.to_lowercase(),
                    _ => v,
                });
                Some(
                    keys.blind_index(&self.sid, &s.entity, &s.field, normalized.as_deref().into())?
                        .to_vec(),
                )
            }
        };
        Ok(match value {
            None => "NULL".to_string(),
            Some(b) => format!("'\\x{}'::bytea", ordo_crypto::hex(&b)),
        })
    }
}

/// **The seal overlay** (increment 13 under live writes): every row this
/// write wrote, `(entity, id)`, gets the column a sealed backfill in
/// progress fills, computed from the row as it now stands, in this
/// transaction. One `Purpose::Write` lease per write. A column the
/// database does not hold yet is skipped under a savepoint.
///
/// A row the write's own statements returned ([`SealCapture`]) is sealed
/// from those values and written by `ctid`, with no read by key. Any other
/// row is read by key.
///
/// Why the worker and a write cannot disagree: both compute the target
/// from the row's own source column under the row's lock, and verify
/// opens every value against its row before the step ends.
pub async fn apply_seals(
    tx: &TxConn,
    schema: &ordo_schema::Schema,
    keys: &dyn ordo_crypto::KeyProvider,
    space: &str,
    seals: &[&Seal],
    written: &[(String, String)],
    captured: &mut SealCapture,
) -> Result<(), SealOverlayError> {
    let mut sealer = SealValue {
        keys,
        sid: ordo_crypto::SpaceId(space.to_string()),
        lease: None,
    };
    for s in seals {
        let mut ids: Vec<&str> = written
            .iter()
            .filter(|(entity, _)| *entity == s.entity)
            .map(|(_, id)| id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            continue;
        }
        // The target is named so a database without it skips here.
        let probe = format!(
            "SELECT {} FROM {} WHERE FALSE",
            quote(&s.target),
            quote(&s.entity)
        );
        if !guarded(tx, &probe).await? {
            continue;
        }
        // The normalization the step recorded, which is the coming
        // version's; the writer's own when the row does not say.
        let normalize = match &s.normalize {
            Some(n) => Some(n.clone()).filter(|n| !n.is_empty()),
            None => schema
                .native(&s.entity)
                .and_then(|e| e.field(&s.field))
                .and_then(|f| f.normalize.clone()),
        };
        let (t, c) = (quote(&s.entity), quote(&s.target));
        // The rows this write's own statements returned: sealed from those
        // values and written by `ctid`. A row whose `ctid` moved since
        // (another overlay statement wrote it) is read by key below.
        let mut by_key: Vec<&str> = Vec::new();
        for id in ids {
            let key = (s.entity.clone(), id.to_string());
            let Some(source) = captured
                .rows
                .get(&key)
                .filter(|r| r.space == space)
                .and_then(|r| Some((r.ctid.clone(), r.values.get(&s.source)?.clone())))
            else {
                by_key.push(id);
                continue;
            };
            let (ctid, value) = source;
            let lit = sealer.of(s, normalize.as_deref(), id, value)?;
            let moved = tx
                .rows(&format!(
                    "UPDATE {t} SET {c} = {lit} WHERE ctid = {}::tid AND \"id\" = {} \
                     RETURNING ctid::text",
                    sql_lit(&ctid),
                    sql_lit(id),
                ))
                .await?;
            match moved.first().and_then(|r| r.first().cloned().flatten()) {
                Some(now) => {
                    if let Some(r) = captured.rows.get_mut(&key) {
                        r.ctid = now;
                    }
                }
                None => by_key.push(id),
            }
        }
        for chunk in by_key.chunks(IDS_PER_STATEMENT) {
            let list = chunk
                .iter()
                .map(|i| sql_lit(i))
                .collect::<Vec<_>>()
                .join(", ");
            let rows = tx
                .rows(&format!(
                    "SELECT \"id\"::text, {src}::text FROM {t} WHERE \"$space\" = {} \
                     AND \"id\" IN ({list})",
                    sql_lit(space),
                    src = quote(&s.source),
                ))
                .await?;
            for r in rows {
                let mut it = r.into_iter();
                let Some(id) = it.next().flatten() else {
                    continue;
                };
                let lit = sealer.of(s, normalize.as_deref(), &id, it.next().flatten())?;
                tx.rows(&format!(
                    "UPDATE {t} SET {c} = {lit} WHERE \"$space\" = {} AND \"id\" = {}",
                    sql_lit(space),
                    sql_lit(&id),
                ))
                .await?;
            }
        }
    }
    Ok(())
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// How many row ids one overlay statement names at most.
const IDS_PER_STATEMENT: usize = 500;

/// The savepoint a statement of the overlay runs under.
const SAVEPOINT: &str = "ordo_move_overlay";

/// The extra writes and checks one version's writes take during a move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOverlay {
    /// The stored plan's digest: the step this overlay belongs to.
    pub step: String,
    /// The version whose writes this overlays.
    pub serving: String,
    /// The version kept current beside it.
    pub other: String,
    /// The entities it touches.
    pub entities: Vec<OverlayEntity>,
}

/// One entity's extra columns and guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayEntity {
    pub entity: String,
    /// `(column, SQL expression over the row's own columns)`.
    pub sets: Vec<(String, String)>,
    /// Each must hold on every row written.
    pub guards: Vec<Guard>,
}

/// A rule of the other version, checked on each row written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    /// A SQL predicate over the row's own columns.
    pub holds: String,
    /// The field, as the other version names it.
    pub field: String,
    /// What it asks, in words: "is required".
    pub rule: String,
}

/// A write the other version's guard refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub serving: String,
    pub other: String,
    pub entity: String,
    pub field: String,
    pub rule: String,
    pub row: String,
    /// The writing space is in a move of its own. `false`: it serves the
    /// version before, in a database another space's move holds to a rule
    /// of the version after ([`HELD_DDL`]).
    pub moving: bool,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.rule != "is required" && !self.moving {
            // A space on the version before, in a database a move holds to
            // the version after's check (11b).
            return write!(
                f,
                "check_violation: this space serves `{}`, in a database moving to `{}`, which \
                 adds a rule every space there now writes under: `{}` {}. This write breaks it \
                 on row `{}` ({}). Nothing was written. Change the value, or move this space to \
                 `{}` (`ordo evolve --tenant <id>`, or a rollout)",
                self.serving, self.other, self.entity, self.rule, self.row, self.field, self.other,
            );
        }
        if self.rule != "is required" {
            // A unique or a check the new version adds, held by the host
            // until the soak's close (increment 11b part 4).
            return write!(
                f,
                "check_violation: this tenant is moving between versions `{}` and `{}`, and a \
                 write must hold under both. `{}` {}, and this write breaks it on row `{}` \
                 ({}). Nothing was written. Change the value, or finish or revert the move \
                 (`ordo evolve --tenant <id>`)",
                self.serving, self.other, self.entity, self.rule, self.row, self.field,
            );
        }
        write!(
            f,
            "check_violation: this tenant is moving between versions `{}` and `{}`, and a \
             write must hold under both. `{}.{}` {} in `{}`, and this write leaves row `{}` \
             without it. Nothing was written. Give `{}` a value, or finish or revert the move \
             (`ordo evolve --tenant <id>`)",
            self.serving,
            self.other,
            self.entity,
            self.field,
            self.rule,
            self.other,
            self.row,
            self.field,
        )
    }
}

/// Why the overlay did not run.
#[derive(Debug)]
pub enum OverlayError {
    /// A row broke the other version's rule.
    Refused(Box<Refused>),
    /// A value this write keeps current for the other version is taken by
    /// another row under a unique that version added (11b part 5).
    Duplicate(Box<Duplicate>),
    /// The database refused a statement.
    Pool(PoolError),
}

/// **A unique the other version added refused this write.** A space still
/// on the version before writes the new columns only through the overlay
/// (a rename's copy, a fill's mapping); when such a column is under a
/// unique the new version added, a value another row of the space holds
/// is refused here by name, never as the database's own duplicate error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Duplicate {
    pub entity: String,
    /// The columns the overlay set on the entity.
    pub columns: Vec<String>,
    /// The rows this write wrote there.
    pub rows: Vec<String>,
}

impl std::fmt::Display for Duplicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let named = self
            .columns
            .iter()
            .map(|c| format!("`{}.{c}`", self.entity))
            .collect::<Vec<_>>()
            .join(", ");
        if self.columns.is_empty() {
            // An entity rename's copy (`apply_mirrors`): the row is copied
            // whole into the other name's table, where a unique took it.
            return write!(
                f,
                "duplicate: this tenant's table is being renamed, and `{}`, the other name, \
                 already holds a row with a unique value row {} gives. Both names are written \
                 until the rename lands. Nothing was written. Change the value, or finish or \
                 revert the move (`ordo evolve --tenant <id>`)",
                self.entity,
                self.rows
                    .iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        write!(
            f,
            "duplicate: {named} is unique in each space in the version after the one this \
             space serves, and another row of this space already holds the value this write \
             gives row {}. This space's writes keep that field current until it moves. Nothing \
             was written. Change the value, or move this space to the new version (`ordo evolve \
             --tenant <id>`, or a rollout)",
            self.rows
                .iter()
                .map(|r| format!("`{r}`"))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

impl From<PoolError> for OverlayError {
    fn from(e: PoolError) -> Self {
        OverlayError::Pool(e)
    }
}

impl MoveOverlay {
    /// **The overlay a version writes with** for the renames a database is
    /// in the middle of ([`RENAMES_DDL`]), worked out from the version's own
    /// schema. `None` when no rename reaches it.
    ///
    /// - A version that declares `field` and not `from` (the renaming one)
    ///   sets the old column from the new one, so the version before reads
    ///   every row it writes whole, and a rollback loses nothing.
    /// - A version that declares `from` and not `field` (the one before)
    ///   sets the new column from the old one.
    /// - A value this version may leave empty keeps what the other column
    ///   holds, so the other version's `required` is never broken by a
    ///   write that is legal here.
    pub fn for_renames(schema: &ordo_schema::Schema, renames: &[Rename]) -> Option<MoveOverlay> {
        let mut entities = Vec::new();
        for r in renames.iter().filter(|r| !r.is_entity()) {
            let Some(e) = schema.native(&r.entity) else {
                continue;
            };
            let copy = |to: &str, src: &str, optional: bool| {
                let sql = if optional {
                    format!("coalesce({}, {})", quote(src), quote(to))
                } else {
                    quote(src)
                };
                (to.to_string(), sql)
            };
            let set = match (e.field(&r.from), e.field(&r.field)) {
                (None, Some(f)) => copy(&r.from, &r.column, f.optional),
                (Some(old), None) => copy(&r.column, &old.column(), old.optional),
                _ => continue,
            };
            entities.push(OverlayEntity {
                entity: r.entity.clone(),
                sets: vec![set],
                guards: Vec::new(),
            });
        }
        (!entities.is_empty()).then(|| MoveOverlay {
            step: "prev".into(),
            serving: String::new(),
            other: String::new(),
            entities,
        })
    }

    /// **The overlay a version writes with on a database in the middle of
    /// renames and fills**: [`MoveOverlay::for_renames`], and each fill
    /// ([`FILLS_DDL`]) while this version does not require its field. Such
    /// a version sets the column by the fill's mapping, and the write must
    /// leave it set: the version after requires it (D10).
    pub fn for_database(
        schema: &ordo_schema::Schema,
        renames: &[Rename],
        fills: &[Fill],
        held: &[HeldCheck],
    ) -> Option<MoveOverlay> {
        let mut out = MoveOverlay::for_renames(schema, renames).unwrap_or(MoveOverlay {
            step: "prev".into(),
            serving: String::new(),
            other: String::new(),
            entities: Vec::new(),
        });
        for f in fills {
            let Some(e) = schema.native(&f.entity) else {
                continue;
            };
            let declared = e.field(&f.field);
            if declared.is_some_and(|x| !x.optional) || (f.absent && declared.is_some()) {
                continue;
            }
            out.serving.clone_from(&f.from);
            out.other.clone_from(&f.to);
            let guard = f.required.then(|| Guard {
                holds: format!("{} IS NOT NULL", quote(&f.column)),
                field: f.field.clone(),
                rule: "is required".into(),
            });
            match out.entities.iter_mut().find(|x| x.entity == f.entity) {
                Some(x) => {
                    x.sets.push((f.column.clone(), f.sql.clone()));
                    x.guards.extend(guard);
                }
                None => out.entities.push(OverlayEntity {
                    entity: f.entity.clone(),
                    sets: vec![(f.column.clone(), f.sql.clone())],
                    guards: guard.into_iter().collect(),
                }),
            }
        }
        // A check the version after adds, held to the close: every space
        // here writes under it, whatever version it serves (11b).
        for h in held {
            if schema.native(&h.entity).is_none() {
                continue;
            }
            if out.serving.is_empty() {
                out.serving.clone_from(&h.from);
                out.other.clone_from(&h.to);
            }
            let guard = Guard {
                holds: h.holds.clone(),
                field: h.field.clone(),
                rule: h.rule.clone(),
            };
            match out.entities.iter_mut().find(|x| x.entity == h.entity) {
                Some(x) => x.guards.push(guard),
                None => out.entities.push(OverlayEntity {
                    entity: h.entity.clone(),
                    sets: Vec::new(),
                    guards: vec![guard],
                }),
            }
        }
        (!out.entities.is_empty()).then_some(out)
    }

    /// The entity's part, when the overlay touches it.
    pub fn entity(&self, name: &str) -> Option<&OverlayEntity> {
        self.entities.iter().find(|e| e.entity == name)
    }

    /// Run the overlay over the rows `written` names, `(entity, row id)`,
    /// in `space`, inside `tx`. Rows deleted are not named by the caller.
    ///
    /// A column the database does not hold yet (a node that read the move
    /// before the step's expand landed) is skipped under a savepoint: the
    /// backfill fills the row later.
    pub async fn apply(
        &self,
        tx: &TxConn,
        space: &str,
        written: &[(String, String)],
    ) -> Result<(), OverlayError> {
        for e in &self.entities {
            let mut ids: Vec<&str> = written
                .iter()
                .filter(|(entity, _)| *entity == e.entity)
                .map(|(_, id)| id.as_str())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            for chunk in ids.chunks(IDS_PER_STATEMENT) {
                let list = chunk
                    .iter()
                    .map(|i| sql_lit(i))
                    .collect::<Vec<_>>()
                    .join(", ");
                let rows = format!("\"$space\" = {} AND \"id\" IN ({list})", sql_lit(space));
                if !e.sets.is_empty() {
                    let sets = e
                        .sets
                        .iter()
                        .map(|(c, sql)| format!("{} = ({sql})", quote(c)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let sql = format!("UPDATE {} SET {sets} WHERE {rows}", quote(&e.entity));
                    match guarded(tx, &sql).await {
                        Ok(true) => {}
                        // The expand has not landed: nothing to keep current.
                        Ok(false) => continue,
                        Err(err) if err.is_unique_violation() => {
                            return Err(OverlayError::Duplicate(Box::new(Duplicate {
                                entity: e.entity.clone(),
                                columns: e.sets.iter().map(|(c, _)| c.clone()).collect(),
                                rows: chunk.iter().map(|i| i.to_string()).collect(),
                            })));
                        }
                        Err(err) => return Err(err.into()),
                    }
                }
                for g in &e.guards {
                    let sql = format!(
                        "SELECT \"id\"::text FROM {} WHERE {rows} AND NOT coalesce(({}), false) \
                         ORDER BY \"id\" LIMIT 1",
                        quote(&e.entity),
                        g.holds
                    );
                    let Some(found) = tx.rows(&sql).await?.into_iter().next() else {
                        continue;
                    };
                    return Err(OverlayError::Refused(Box::new(Refused {
                        serving: self.serving.clone(),
                        other: self.other.clone(),
                        entity: e.entity.clone(),
                        field: g.field.clone(),
                        rule: g.rule.clone(),
                        row: found.into_iter().next().flatten().unwrap_or_default(),
                        // The database's own overlays are named `prev`.
                        moving: self.step != "prev",
                    })));
                }
            }
        }
        Ok(())
    }
}

/// Run `sql` under a savepoint. `false` when the database lacks a column
/// it names, which leaves the transaction as it was.
async fn guarded(tx: &TxConn, sql: &str) -> Result<bool, PoolError> {
    tx.rows(&format!("SAVEPOINT {SAVEPOINT}")).await?;
    match tx.rows(sql).await {
        Ok(_) => {
            tx.rows(&format!("RELEASE SAVEPOINT {SAVEPOINT}")).await?;
            Ok(true)
        }
        Err(e) if e.is_missing_column() => {
            tx.rows(&format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}"))
                .await?;
            tx.rows(&format!("RELEASE SAVEPOINT {SAVEPOINT}")).await?;
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(fields: &str) -> ordo_schema::Schema {
        ordo_conformance::Fixture::from_source(
            &format!("entity note {{\n{fields}}}\n\nop allNotes unbounded =\n  note[body]\n"),
            "",
        )
        .expect("compiles")
        .schema
    }

    /// The renaming version copies the new column back; a version still on
    /// the old name copies forward; a version with neither name, or both,
    /// writes nothing more. An optional source keeps what the other column
    /// holds.
    #[test]
    fn each_version_keeps_the_other_column_of_a_rename_current() {
        let r = [Rename {
            entity: "note".into(),
            from: "title".into(),
            field: "heading".into(),
            column: "heading".into(),
        }];
        let sets = |fields: &str| {
            MoveOverlay::for_renames(&schema(fields), &r).map(|o| o.entities[0].sets.clone())
        };
        assert_eq!(
            sets("  heading: text = \"\" prev(title)\n  body: text?\n"),
            Some(vec![("title".to_string(), "\"heading\"".to_string())])
        );
        assert_eq!(
            sets("  title: text\n  body: text?\n"),
            Some(vec![("heading".to_string(), "\"title\"".to_string())])
        );
        assert_eq!(
            sets("  title: text?\n  body: text?\n"),
            Some(vec![(
                "heading".to_string(),
                "coalesce(\"title\", \"heading\")".to_string()
            )])
        );
        assert_eq!(sets("  name: text\n  body: text?\n"), None);
    }

    /// An entity rename: the version with the new name copies into the
    /// old table, the version with the old name copies forward, and a
    /// version with neither or both copies nothing. A field rename's
    /// overlay never reads an entity rename's row.
    #[test]
    fn each_version_keeps_the_other_table_of_an_entity_rename_current() {
        let r = [Rename {
            entity: "memo".into(),
            from: "note".into(),
            field: String::new(),
            column: String::new(),
        }];
        let entity = |name: &str| {
            ordo_conformance::Fixture::from_source(
                &format!(
                    "entity {name} {{\n  title: text\n}}\n\nop all unbounded =\n  {name}[title]\n"
                ),
                "",
            )
            .expect("compiles")
            .schema
        };
        let m = entity_mirrors(&entity("memo"), &r);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].table.as_str(), m[0].other.as_str()), ("memo", "note"));
        assert!(m[0].columns.contains(&"$space".to_string()));
        assert!(m[0].columns.contains(&"title".to_string()));
        let m = entity_mirrors(&entity("note"), &r);
        assert_eq!((m[0].table.as_str(), m[0].other.as_str()), ("note", "memo"));
        assert!(entity_mirrors(&entity("other"), &r).is_empty());
        assert!(MoveOverlay::for_renames(&entity("memo"), &r).is_none());
    }

    /// While a seal runs, one `INSERT` or `UPDATE` of the sealed entity
    /// returns what the overlay needs; any other statement is left alone.
    #[test]
    fn a_write_to_a_sealed_entity_returns_its_source_and_nothing_else_changes() {
        let seal = Seal {
            entity: "note".into(),
            field: "body".into(),
            kind: SealKind::Seal,
            source: "body".into(),
            target: "body$seal".into(),
            normalize: None,
        };
        let c = SealCapture::for_seals(&[&seal]);
        let (e, sql) = c
            .rewrite("INSERT INTO \"note\" (\"id\", \"body\") VALUES ('n1', 'a;b');")
            .expect("rewritten");
        assert_eq!(e, "note");
        assert_eq!(
            sql,
            "INSERT INTO \"note\" (\"id\", \"body\") VALUES ('n1', 'a;b') RETURNING \
             ctid::text, \"$space\"::text, \"id\"::text, \"body\"::text"
        );
        assert!(c
            .rewrite("UPDATE \"note\" SET \"body\" = 'x' WHERE \"id\" = 'n1';")
            .is_some());
        for left in [
            "UPDATE \"other\" SET \"body\" = 'x';",
            "SELECT 1 FROM \"note\"",
            "UPDATE \"note\" SET \"body\" = 'x'; UPDATE \"note\" SET \"body\" = 'y';",
            "INSERT INTO \"note\" (\"id\") VALUES ('n1') RETURNING \"id\";",
            "UPDATE \"note\" SET \"body\" = E'it\\'s';",
            "UPDATE \"note\" SET \"body\" = 'open",
        ] {
            assert_eq!(c.rewrite(left), None, "{left}");
        }
        assert_eq!(
            SealCapture::default().rewrite("UPDATE \"note\" SET x = 1;"),
            None
        );
    }

    #[test]
    fn a_refusal_names_both_versions_the_field_and_the_row() {
        let r = Refused {
            serving: "0.3.0".into(),
            other: "0.3.2".into(),
            entity: "notes".into(),
            field: "text".into(),
            rule: "is required".into(),
            row: "n1".into(),
            moving: true,
        };
        let s = r.to_string();
        assert!(s.starts_with("check_violation: "), "{s}");
        for want in [
            "0.3.0",
            "0.3.2",
            "`notes.text`",
            "`n1`",
            "ordo evolve --tenant",
        ] {
            assert!(s.contains(want), "{want}: {s}");
        }
    }

    /// A space on the version before, in a database a move holds to a
    /// check of the version after: every space there writes under it.
    #[test]
    fn a_held_check_guards_every_writer_and_names_the_version_after() {
        let held = [HeldCheck {
            entity: "note".into(),
            name: "note_body_short".into(),
            holds: "coalesce((length(\"body\") < 10), true)".into(),
            field: "check length(body) < 10".into(),
            rule: "holds the check `note_body_short` that `0.3.2` adds".into(),
            from: "0.3.0".into(),
            to: "0.3.2".into(),
        }];
        let o = MoveOverlay::for_database(&schema("  body: text?\n"), &[], &[], &held)
            .expect("an overlay");
        assert_eq!(o.entities[0].guards[0].holds, held[0].holds);
        assert!(o.entities[0].sets.is_empty());
        assert_eq!((o.serving.as_str(), o.other.as_str()), ("0.3.0", "0.3.2"));
        // An entity this version does not declare writes nothing there.
        assert!(MoveOverlay::for_database(
            &schema("  title: text?\n"),
            &[],
            &[],
            &[HeldCheck {
                entity: "other".into(),
                ..held[0].clone()
            }]
        )
        .is_none());
        let s = Refused {
            serving: "0.3.0".into(),
            other: "0.3.2".into(),
            entity: "note".into(),
            field: held[0].field.clone(),
            rule: held[0].rule.clone(),
            row: "n1".into(),
            moving: false,
        }
        .to_string();
        assert!(
            s.starts_with("check_violation: this space serves `0.3.0`"),
            "{s}"
        );
        for want in ["note_body_short", "`n1`", "move this space to `0.3.2`"] {
            assert!(s.contains(want), "{want}: {s}");
        }
    }
}
