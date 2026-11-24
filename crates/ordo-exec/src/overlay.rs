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
    Refused(Refused),
    /// The database refused a statement.
    Pool(PoolError),
}

impl From<PoolError> for OverlayError {
    fn from(e: PoolError) -> Self {
        OverlayError::Pool(e)
    }
}

impl MoveOverlay {
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
                    return Err(OverlayError::Refused(Refused {
                        serving: self.serving.clone(),
                        other: self.other.clone(),
                        entity: e.entity.clone(),
                        field: g.field.clone(),
                        rule: g.rule.clone(),
                        row: found.into_iter().next().flatten().unwrap_or_default(),
                    }));
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
