//! **`$orphans`**: the columns and tables a version left behind
//! (`plan-versions.md` §2.7, increment 14).
//!
//! A version that stops declaring a field or an entity never drops it (ruled
//! 2026-10-01, 27y). The column or table stays, with what was written to it,
//! and nothing reads or writes it. That is an **orphan**. Each database
//! records its own in `$orphans`, one row per orphaned field or entity, so
//! the acts that must reach every byte a person wrote can find them:
//!
//! - **Erasure** deletes a space's rows from every orphaned table as well as
//!   the declared ones ([`crate::erase`]). An orphaned column goes with its
//!   row.
//! - **Export** writes every orphaned column and table beside the declared
//!   entities, under `orphans/` ([`crate::export`]).
//! - **Readers** lose an orphaned column from their grant
//!   ([`crate::peer::grant_readers`]): a reader reads only columns the read
//!   app still writes (owner ruling 2026-11-23, Q1).
//! - **Pruning** drops the column or table, once the deployment owner asks
//!   twice. The row stays as a **tombstone**: `pruned_at` and `pruned_by`
//!   say when and by whom. A tombstone is not an orphan any more.
//!
//! # The drop date
//!
//! Every orphan gets a drop date, `orphans.drop_after` after it was
//! orphaned (`ORDO_ORPHANS_DROP_AFTER`, in days, 30 by default). Ordo has no
//! idea of personal data (owner, 2026-11-25): every orphan is treated alike,
//! and what counts as personal is the app developer's call. The date is
//! recorded and shown as due. Nothing drops by itself: pruning is always the
//! deployment owner's act.
//!
//! # A field declared again
//!
//! A later version that declares an orphaned field again serves it again.
//! Its row is removed ([`revive`]), because the column is written again.

use crate::{sql_lit, Pool, PoolError};

/// The table, by name.
pub const TABLE: &str = "$orphans";

/// One row per orphaned field (`field` the field's name) or entity (`field`
/// empty). `columns` is every physical column the orphan holds, comma
/// separated: a `sealed indexed` field holds 2.
pub const DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$orphans\" ( \
  \"entity\" text NOT NULL, \
  \"field\" text NOT NULL DEFAULT '', \
  \"columns\" text NOT NULL DEFAULT '', \
  \"sealed\" boolean NOT NULL DEFAULT false, \
  \"from_version\" text, \
  \"to_version\" text NOT NULL, \
  \"orphaned_at\" bigint NOT NULL, \
  \"drop_after\" bigint, \
  \"actor\" text NOT NULL, \
  \"pruned_at\" bigint, \
  \"pruned_by\" text, \
  PRIMARY KEY (\"entity\", \"field\") \
)";

/// `orphans.drop_after`, in days.
pub const DROP_AFTER_ENV: &str = "ORDO_ORPHANS_DROP_AFTER";

/// The name this setting had until 2026-11-25. A deployment built on
/// 2026-11-23 or later may have it set, so it is still read when the new
/// name is not set.
pub const DROP_AFTER_OLD_ENV: &str = "ORDO_ORPHANS_PERSONAL_DROP_AFTER";

/// The variable in force: the new name when set, else the old one when
/// that is set, else the new name.
pub fn drop_after_var() -> &'static str {
    if std::env::var(DROP_AFTER_ENV).is_err() && std::env::var(DROP_AFTER_OLD_ENV).is_ok() {
        DROP_AFTER_OLD_ENV
    } else {
        DROP_AFTER_ENV
    }
}

/// Its default: 30 days.
pub const DROP_AFTER_DEFAULT_DAYS: u64 = 30;

/// The drop date's distance, in seconds. A value that is not a whole number
/// of days reads as the default; the boot refuses it ([`drop_after_value`]).
pub fn drop_after_secs() -> u64 {
    drop_after_value(std::env::var(drop_after_var()).ok().as_deref())
        .unwrap_or(DROP_AFTER_DEFAULT_DAYS)
        * 86_400
}

/// Parse the setting: `30`, or `30d`. `None` when it is not a count of days.
pub fn drop_after_value(v: Option<&str>) -> Option<u64> {
    match v.map(str::trim) {
        None | Some("") => Some(DROP_AFTER_DEFAULT_DAYS),
        Some(s) => s.strip_suffix('d').unwrap_or(s).parse::<u64>().ok(),
    }
}

/// One orphan, or one tombstone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub entity: String,
    /// Empty for a whole entity.
    pub field: String,
    /// The physical columns it holds. Empty for a whole entity.
    pub columns: Vec<String>,
    pub sealed: bool,
    pub from_version: Option<String>,
    pub to_version: String,
    pub orphaned_at: u64,
    pub drop_after: Option<u64>,
    pub actor: String,
    pub pruned_at: Option<u64>,
    pub pruned_by: Option<String>,
}

impl Orphan {
    /// `entity.field`, or `entity` for a whole entity.
    pub fn name(&self) -> String {
        if self.field.is_empty() {
            self.entity.clone()
        } else {
            format!("{}.{}", self.entity, self.field)
        }
    }

    /// A whole entity rather than one field.
    pub fn is_entity(&self) -> bool {
        self.field.is_empty()
    }

    /// Still in the database: not pruned.
    pub fn is_live(&self) -> bool {
        self.pruned_at.is_none()
    }
}

/// What a version left behind on one database, before it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    pub entity: String,
    /// `None` for a whole entity.
    pub field: Option<String>,
    pub columns: Vec<String>,
    pub sealed: bool,
}

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    pool.rows(DDL).await.map(|_| ())
}

/// Record what `to` left behind. An orphan already recorded keeps its first
/// date. A tombstone of the same name becomes an orphan again only when its
/// column is back, which a prune never allows, so it is left alone.
pub async fn record(
    pool: &Pool,
    left: &[Left],
    from: Option<&str>,
    to: &str,
    actor: &str,
    at: u64,
) -> Result<usize, PoolError> {
    if left.is_empty() {
        return Ok(0);
    }
    ensure_table(pool).await?;
    let drop_after = at.saturating_add(drop_after_secs());
    let mut written = 0;
    for l in left {
        let rows = pool
            .rows(&format!(
                "INSERT INTO \"$orphans\" (\"entity\", \"field\", \"columns\", \"sealed\", \
                   \"from_version\", \"to_version\", \"orphaned_at\", \"drop_after\", \"actor\") \
                 VALUES ({}, {}, {}, {}, {}, {}, {at}, {drop_after}, {}) \
                 ON CONFLICT (\"entity\", \"field\") DO NOTHING RETURNING 1",
                sql_lit(&l.entity),
                sql_lit(l.field.as_deref().unwrap_or("")),
                sql_lit(&l.columns.join(",")),
                l.sealed,
                from.map(sql_lit).unwrap_or_else(|| "NULL".into()),
                sql_lit(to),
                sql_lit(actor),
            ))
            .await?;
        written += rows.len();
    }
    Ok(written)
}

/// Remove the orphan rows of fields and entities `declared` names again: a
/// version that declares them serves them again. Tombstones stay.
pub async fn revive(
    pool: &Pool,
    declared: &(dyn Fn(&str, &str) -> bool + Sync),
) -> Result<Vec<String>, PoolError> {
    let all = match list(pool).await {
        Ok(all) => all,
        Err(e) if e.is_missing_table() => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for o in all.iter().filter(|o| o.is_live()) {
        if declared(&o.entity, &o.field) {
            pool.rows(&format!(
                "DELETE FROM \"$orphans\" WHERE \"entity\" = {} AND \"field\" = {} \
                 AND \"pruned_at\" IS NULL",
                sql_lit(&o.entity),
                sql_lit(&o.field)
            ))
            .await?;
            out.push(o.name());
        }
    }
    Ok(out)
}

/// Every row, orphans and tombstones, by name. A database with no table has
/// none.
pub async fn list(pool: &Pool) -> Result<Vec<Orphan>, PoolError> {
    let rows = match pool
        .rows(
            "SELECT \"entity\", \"field\", \"columns\", \"sealed\"::text, \"from_version\", \
                    \"to_version\", \"orphaned_at\", \"drop_after\", \"actor\", \"pruned_at\", \
                    \"pruned_by\" \
             FROM \"$orphans\" ORDER BY \"entity\", \"field\"",
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(rows
        .into_iter()
        .map(|r| {
            let g = |i: usize| r.get(i).cloned().flatten();
            let n = |i: usize| g(i).and_then(|s| s.parse::<u64>().ok());
            Orphan {
                entity: g(0).unwrap_or_default(),
                field: g(1).unwrap_or_default(),
                columns: g(2)
                    .unwrap_or_default()
                    .split(',')
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .collect(),
                sealed: g(3).as_deref() == Some("true"),
                from_version: g(4),
                to_version: g(5).unwrap_or_default(),
                orphaned_at: n(6).unwrap_or(0),
                drop_after: n(7),
                actor: g(8).unwrap_or_default(),
                pruned_at: n(9),
                pruned_by: g(10),
            }
        })
        .collect())
}

/// The orphans still in the database.
pub async fn live(pool: &Pool) -> Result<Vec<Orphan>, PoolError> {
    Ok(list(pool)
        .await?
        .into_iter()
        .filter(Orphan::is_live)
        .collect())
}

/// Turn an orphan into a tombstone, in the same statement batch that drops
/// what it held. `drop_sql` is the `DROP` the caller rendered. One implicit
/// transaction: either both land or neither does.
pub async fn prune(
    pool: &Pool,
    orphan: &Orphan,
    drop_sql: &str,
    by: &str,
    at: u64,
) -> Result<(), PoolError> {
    pool.rows(&format!(
        "{drop_sql} UPDATE \"$orphans\" SET \"pruned_at\" = {at}, \"pruned_by\" = {} \
         WHERE \"entity\" = {} AND \"field\" = {} AND \"pruned_at\" IS NULL;",
        sql_lit(by),
        sql_lit(&orphan.entity),
        sql_lit(&orphan.field)
    ))
    .await
    .map(|_| ())
}

/// The statements that drop what `orphan` holds in schema `pg`: the table,
/// or each of its columns. `IF EXISTS`, so a database that never held it
/// still gets its tombstone.
pub fn drop_sql(pg: &str, orphan: &Orphan) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    if orphan.is_entity() {
        return format!("DROP TABLE IF EXISTS {}.{};", q(pg), q(&orphan.entity));
    }
    let cols = orphan
        .columns
        .iter()
        .map(|c| format!("DROP COLUMN IF EXISTS {}", q(c)))
        .collect::<Vec<_>>()
        .join(", ");
    if cols.is_empty() {
        return String::new();
    }
    format!(
        "ALTER TABLE IF EXISTS {}.{} {cols};",
        q(pg),
        q(&orphan.entity)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `orphans.drop_after` is `ORDO_ORPHANS_DROP_AFTER` (owner,
    /// 2026-11-25). The old name is read only while the new one is unset.
    /// The one test that sets these variables.
    #[test]
    fn the_new_name_sets_the_drop_date_and_the_old_name_is_still_read() {
        std::env::remove_var(DROP_AFTER_ENV);
        std::env::remove_var(DROP_AFTER_OLD_ENV);
        assert_eq!(drop_after_secs(), 30 * 86_400);
        std::env::set_var(DROP_AFTER_OLD_ENV, "9");
        assert_eq!(drop_after_secs(), 9 * 86_400);
        assert_eq!(drop_after_var(), DROP_AFTER_OLD_ENV);
        std::env::set_var(DROP_AFTER_ENV, "7d");
        assert_eq!(drop_after_secs(), 7 * 86_400, "the new name wins");
        assert_eq!(drop_after_var(), DROP_AFTER_ENV);
        std::env::remove_var(DROP_AFTER_ENV);
        std::env::remove_var(DROP_AFTER_OLD_ENV);
    }

    #[test]
    fn the_drop_date_setting_reads_days_and_refuses_anything_else() {
        assert_eq!(drop_after_value(None), Some(30));
        assert_eq!(drop_after_value(Some("7")), Some(7));
        assert_eq!(drop_after_value(Some("7d")), Some(7));
        assert_eq!(drop_after_value(Some("a week")), None);
    }

    #[test]
    fn a_pruned_field_drops_each_of_its_columns_and_an_entity_its_table() {
        let mut o = Orphan {
            entity: "attachments".into(),
            field: "size".into(),
            columns: vec!["size".into()],
            sealed: false,
            from_version: None,
            to_version: "0.3.0".into(),
            orphaned_at: 0,
            drop_after: None,
            actor: "a".into(),
            pruned_at: None,
            pruned_by: None,
        };
        assert_eq!(
            drop_sql("app", &o),
            "ALTER TABLE IF EXISTS \"app\".\"attachments\" DROP COLUMN IF EXISTS \"size\";"
        );
        o.field.clear();
        o.columns.clear();
        assert_eq!(
            drop_sql("app", &o),
            "DROP TABLE IF EXISTS \"app\".\"attachments\";"
        );
    }
}
