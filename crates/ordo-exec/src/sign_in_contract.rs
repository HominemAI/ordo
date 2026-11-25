//! **The sign-in contract** (owner ruling 2026-11-23, Q16;
//! `plan-evolution-federation.md` §5, step F6).
//!
//! `$caller` on a peer's table reads 4 of the peer's sign-in tables:
//! `members`, `$principal`, `$authenticator`, `$authconnection`. Their shape
//! moves with the Ordo release, not with a program version, and 2
//! deployments in one database can run 2 releases. So the columns a reader
//! is granted in them are a named contract, [`CONTRACT`]:
//!
//! - A column is never removed from the list. A release that changes one
//!   marks it [`Retired`], with its own release date.
//! - A retired column stays granted, and its table keeps it, for
//!   `editions.load_window` after that date
//!   ([`ordo_plan::edition::LOAD_WINDOW_MONTHS`]). A reader on an older
//!   release keeps matching `$caller` through the window.
//! - The window is measured against the running build's release date, as
//!   editions are, so the same build always grants the same columns.
//!
//! Each deployment also records the Ordo release it last started under, in
//! [`RELEASE_TABLE`], and grants its `release` column to every reader. The
//! release check reads it to name each peer on another release
//! (`plan-versions.md` §2.8).
//!
//! A regression looks like a column taken off [`CONTRACT`] instead of
//! retired (the test `the_contract_names_every_column` fails), or a retired
//! column revoked before its window ends.

use std::collections::BTreeMap;

use ordo_plan::edition::{Date, Editions, LOAD_WINDOW_MONTHS};

use crate::{sql_lit, Pool, PoolError};

/// A column a release changed. It stays readable for `editions.load_window`
/// after `on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retired {
    /// The Ordo release that changed it.
    pub release: &'static str,
    /// That release's date, `YYYY-MM-DD`.
    pub on: &'static str,
    /// The column that carries it now, when there is one.
    pub replaced_by: Option<&'static str>,
}

/// One column of the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub table: &'static str,
    pub column: &'static str,
    pub retired: Option<Retired>,
}

const fn live(table: &'static str, column: &'static str) -> Column {
    Column {
        table,
        column,
        retired: None,
    }
}

/// **The contract**: every column of the 4 sign-in tables a reader's role
/// is granted, what a `$caller` guard and the peer link read. Add a column
/// freely. Never remove one: mark it [`Retired`].
pub const CONTRACT: &[Column] = &[
    live("members", "id"),
    live("members", "principal"),
    live("members", "kind"),
    live("members", "status"),
    live("$principal", "id"),
    live("$principal", "deleted_at"),
    live("$authenticator", "authenticator"),
    live("$authenticator", "principal"),
    live("$authenticator", "retired_case"),
    live("$authconnection", "name"),
    live("$authconnection", "kind"),
    live("$authconnection", "fingerprint"),
    live("$authconnection", "subject_claim"),
];

/// Whether `c` is still granted under a build dated `release`.
pub fn readable(c: &Column, release: Date) -> bool {
    match c.retired {
        None => true,
        Some(r) => Date::parse(r.on)
            .map(|on| release < on.plus_months(LOAD_WINDOW_MONTHS))
            // A date that does not read keeps the column: the safe side.
            .unwrap_or(true),
    }
}

/// The first release date that no longer grants a retired column: `on`
/// plus the window.
pub fn kept_until(r: &Retired) -> Option<Date> {
    Date::parse(r.on).map(|on| on.plus_months(LOAD_WINDOW_MONTHS))
}

/// The contract's granted columns, table by table, under a build dated
/// `release`.
pub fn columns_at(contract: &[Column], release: Date) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut out: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for c in contract.iter().filter(|c| readable(c, release)) {
        out.entry(c.table).or_default().push(c.column);
    }
    out.into_iter().collect()
}

/// [`CONTRACT`]'s granted columns under this build's release date.
pub fn columns() -> Vec<(&'static str, Vec<&'static str>)> {
    columns_at(CONTRACT, Editions::this_release().release_date())
}

/// The columns this release has retired, still granted or not.
pub fn retired_by(release: &str) -> Vec<&'static Column> {
    CONTRACT
        .iter()
        .filter(|c| c.retired.is_some_and(|r| r.release == release))
        .collect()
}

// ── the release each deployment runs ────────────────────────────────────

/// The table a deployment records its Ordo release in: one row, written at
/// every start. Its `release` column is granted to every reader.
pub const RELEASE_TABLE: &str = "$ordorelease";

/// The column of [`RELEASE_TABLE`] a reader is granted.
pub const RELEASE_COLUMN: &str = "release";

const RELEASE_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$ordorelease" (
  "one"        boolean PRIMARY KEY DEFAULT true CHECK ("one"),
  "release"    text    NOT NULL,
  "started_at" bigint  NOT NULL
)"#;

/// Record that this deployment started under `release`, at `at` (seconds).
/// Called at every start, before the program loads, so the reader grants
/// the load makes cover the table.
pub async fn record_release(pool: &Pool, release: &str, at: u64) -> Result<(), PoolError> {
    // Two replicas making it at once can collide inside Postgres even with
    // `IF NOT EXISTS`; the second try finds it.
    if pool.rows(RELEASE_DDL).await.is_err() {
        pool.rows(RELEASE_DDL).await?;
    }
    pool.rows(&format!(
        r#"INSERT INTO "$ordorelease" ("one", "release", "started_at") VALUES (true, {}, {at})
           ON CONFLICT ("one") DO UPDATE SET "release" = EXCLUDED."release",
             "started_at" = EXCLUDED."started_at""#,
        sql_lit(release)
    ))
    .await?;
    Ok(())
}

/// What a peer's [`RELEASE_TABLE`] says, read from schema `pg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerRelease {
    /// The release it last started under.
    Known(String),
    /// It records none: a release from before this table.
    Unrecorded,
    /// The table is there and this role is not granted it. The peer grants
    /// it with its reader grants, at its next start under a release that
    /// has the table.
    NotGranted,
}

/// The schemas whose `$ordorelease` is owned by `role`: where a reader
/// that connects as `role` keeps its release. The read app finds its
/// readers' releases there (owner ruling 2026-11-25).
pub async fn schemas_owned_by(pool: &Pool, role: &str) -> Result<Vec<String>, PoolError> {
    Ok(pool
        .rows(&format!(
            "SELECT n.nspname::text FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relname = {} AND c.relkind = 'r' \
               AND pg_get_userbyid(c.relowner) = {} \
             ORDER BY 1",
            sql_lit(RELEASE_TABLE),
            sql_lit(role)
        ))
        .await?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect())
}

/// Read the release a deployment in schema `pg` last started under.
pub async fn release_of(pool: &Pool, pg: &str) -> Result<PeerRelease, PoolError> {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    match pool
        .rows(&format!(
            "SELECT {} FROM {}.{} LIMIT 1",
            q(RELEASE_COLUMN),
            q(pg),
            q(RELEASE_TABLE)
        ))
        .await
    {
        Ok(rows) => Ok(rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .map(PeerRelease::Known)
            .unwrap_or(PeerRelease::Unrecorded)),
        Err(e) if e.is_missing_table() || e.is_missing_column() => Ok(PeerRelease::Unrecorded),
        // 42501: insufficient privilege, on the schema or the table.
        Err(e) if e.sqlstate() == Some("42501") => Ok(PeerRelease::NotGranted),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    /// The contract as written. A column taken off it fails here: mark it
    /// `Retired` instead, so readers on older releases keep it for the
    /// window (owner ruling 2026-11-23, Q16).
    #[test]
    fn the_contract_names_every_column() {
        let all: Vec<(&str, &str)> = CONTRACT.iter().map(|c| (c.table, c.column)).collect();
        for want in [
            ("members", "id"),
            ("members", "principal"),
            ("members", "kind"),
            ("members", "status"),
            ("$principal", "id"),
            ("$principal", "deleted_at"),
            ("$authenticator", "authenticator"),
            ("$authenticator", "principal"),
            ("$authenticator", "retired_case"),
            ("$authconnection", "name"),
            ("$authconnection", "kind"),
            ("$authconnection", "fingerprint"),
            ("$authconnection", "subject_claim"),
        ] {
            assert!(all.contains(&want), "{want:?} left the contract");
        }
        let tables: std::collections::BTreeSet<&str> = all.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            tables.into_iter().collect::<Vec<_>>(),
            ["$authconnection", "$authenticator", "$principal", "members"]
        );
    }

    #[test]
    fn a_retired_column_is_granted_for_the_load_window_then_not() {
        let contract = [
            live("members", "id"),
            Column {
                table: "members",
                column: "kind",
                retired: Some(Retired {
                    release: "0.6.0",
                    on: "2027-01-15",
                    replaced_by: Some("role"),
                }),
            },
            live("members", "role"),
        ];
        // The release that changed it, and one a day before the window ends.
        for at in ["2027-01-15", "2028-01-14"] {
            assert_eq!(
                columns_at(&contract, d(at)),
                [("members", vec!["id", "kind", "role"])],
                "{at}"
            );
        }
        // A release dated at the window's end no longer grants it.
        assert_eq!(
            columns_at(&contract, d("2028-01-15")),
            [("members", vec!["id", "role"])]
        );
        assert_eq!(
            kept_until(&contract[1].retired.unwrap()),
            Some(d("2028-01-15"))
        );
    }

    #[test]
    fn this_build_grants_every_live_column() {
        let n: usize = columns().iter().map(|(_, c)| c.len()).sum();
        assert_eq!(n, CONTRACT.iter().filter(|c| c.retired.is_none()).count());
    }
}
