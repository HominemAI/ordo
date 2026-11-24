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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub entity: String,
    pub from: String,
    pub field: String,
    pub column: String,
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
  PRIMARY KEY (\"entity\", \"field\", \"kind\") \
)";

/// What a sealed backfill in progress fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealKind {
    /// B7: the plaintext sealed at its row.
    Seal,
    /// B8: the sealed value's blind index.
    Index,
}

impl SealKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SealKind::Seal => "seal",
            SealKind::Index => "index",
        }
    }

    pub fn parse(s: &str) -> Option<SealKind> {
        match s {
            "seal" => Some(SealKind::Seal),
            "index" => Some(SealKind::Index),
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
}

/// Read `$seals`. Run after [`renames_in`], whose lock fences this table
/// too.
pub async fn seals_in(tx: &TxConn) -> Result<Vec<Seal>, PoolError> {
    let rows = tx
        .rows(
            "SELECT \"entity\", \"field\", \"kind\", \"source\", \"target\" FROM \"$seals\" \
             ORDER BY \"entity\", \"field\", \"kind\"",
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
/// indexed. A version that already declares the end shape writes nothing
/// more: the step's end lands before it serves.
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
            }
        })
        .collect()
}

/// **The seal overlay** (increment 13 under live writes): every row this
/// write wrote, `(entity, id)`, gets the column a sealed backfill in
/// progress fills, computed from the row as it now stands, in this
/// transaction. One `Purpose::Write` lease per write. A column the
/// database does not hold yet is skipped under a savepoint.
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
) -> Result<(), SealOverlayError> {
    let sid = ordo_crypto::SpaceId(space.to_string());
    let mut lease: Option<ordo_crypto::KeyLease> = None;
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
        let normalize = schema
            .native(&s.entity)
            .and_then(|e| e.field(&s.field))
            .and_then(|f| f.normalize.clone());
        for chunk in ids.chunks(IDS_PER_STATEMENT) {
            let list = chunk
                .iter()
                .map(|i| sql_lit(i))
                .collect::<Vec<_>>()
                .join(", ");
            let read = format!(
                "SELECT \"id\"::text, {src}::text FROM {t} WHERE \"$space\" = {} \
                 AND \"id\" IN ({list})",
                sql_lit(space),
                src = quote(&s.source),
                t = quote(&s.entity),
            );
            // The target is named so a database without it skips here.
            let probe = format!(
                "SELECT {} FROM {} WHERE FALSE",
                quote(&s.target),
                quote(&s.entity)
            );
            if !guarded(tx, &probe).await? {
                continue;
            }
            let rows = tx.rows(&read).await?;
            for r in rows {
                let mut it = r.into_iter();
                let Some(id) = it.next().flatten() else {
                    continue;
                };
                let source = it.next().flatten();
                let value: Option<Vec<u8>> = match (s.kind, source) {
                    (SealKind::Seal, None) => None,
                    (SealKind::Seal, Some(text)) => {
                        if lease.is_none() {
                            lease =
                                Some(keys.release(&sid, ordo_crypto::Purpose::Write, "seal")?);
                        }
                        let l = lease.as_ref().expect("taken just above");
                        Some(keys.seal_at(
                            l,
                            &s.entity,
                            &s.field,
                            ordo_crypto::Anchor::Row(&id),
                            text.as_bytes(),
                        )?)
                    }
                    (SealKind::Index, sealed) => {
                        let plain = match sealed {
                            None => None,
                            Some(hexed) => {
                                if lease.is_none() {
                                    lease = Some(keys.release(
                                        &sid,
                                        ordo_crypto::Purpose::Write,
                                        "seal",
                                    )?);
                                }
                                let l = lease.as_ref().expect("taken just above");
                                let bytes = unhex(hexed.strip_prefix("\\x").unwrap_or(&hexed));
                                let open = keys.open_at(
                                    l,
                                    &s.entity,
                                    &s.field,
                                    ordo_crypto::Anchor::Row(&id),
                                    &bytes,
                                )?;
                                Some(String::from_utf8_lossy(&open).into_owned())
                            }
                        };
                        let normalized = plain.map(|v| match normalize.as_deref() {
                            Some("lower") => v.to_lowercase(),
                            _ => v,
                        });
                        Some(
                            keys.blind_index(
                                &sid,
                                &s.entity,
                                &s.field,
                                normalized.as_deref().into(),
                            )?
                            .to_vec(),
                        )
                    }
                };
                let lit = match value {
                    None => "NULL".to_string(),
                    Some(b) => format!("'\\x{}'::bytea", ordo_crypto::hex(&b)),
                };
                tx.rows(&format!(
                    "UPDATE {t} SET {c} = {lit} WHERE \"$space\" = {} AND \"id\" = {}",
                    sql_lit(space),
                    sql_lit(&id),
                    t = quote(&s.entity),
                    c = quote(&s.target),
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
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    /// The database refused a statement.
    Pool(PoolError),
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
        for r in renames {
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
                    if !guarded(tx, &sql).await? {
                        // The expand has not landed: nothing to keep current.
                        continue;
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

    #[test]
    fn a_refusal_names_both_versions_the_field_and_the_row() {
        let r = Refused {
            serving: "0.3.0".into(),
            other: "0.3.2".into(),
            entity: "notes".into(),
            field: "text".into(),
            rule: "is required".into(),
            row: "n1".into(),
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
}
