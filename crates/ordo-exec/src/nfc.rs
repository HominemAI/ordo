//! The migration probe for checklist 9u.
//!
//! NFC-on-write is settled and now implemented, which leaves every row
//! written *before* it in whatever form its caller happened to send. Those
//! rows are not merely untidy: normalizing them can **merge two rows that a
//! `unique` column currently keeps apart**, because two spellings of one text
//! are distinct bytes today and one value afterwards.
//!
//! That is why 9u is its own item and not a line in the collation ruling. A
//! migration that normalizes in place discovers the collision by hitting the
//! constraint, mid-write, with half the table converted. This finds it first,
//! reads nothing but the columns it must, and answers three separate
//! questions rather than one:
//!
//! - **Is there anything to do?** [`unnormalized`] counts rows whose text is
//!   not its own NFC form, per column.
//! - **Would converting be destructive?** [`collisions`] reports the values
//!   that would become duplicates *inside a unique column*, before anything
//!   is written. A non-empty answer means the deployment has a decision to
//!   make about which row survives — a decision the platform must not take
//!   on its behalf.
//! - **Is a text column safe to convert?** Both together: unnormalized rows
//!   and no collisions is the case a migration can do unattended.
//!
//! Postgres has `normalize(text, NFC)` in core since 13, so this asks the
//! server the same question the engine would ask in Rust. Doing it in SQL
//! means it costs one round trip over an indexed scan rather than streaming
//! every row of every text column into the process that is auditing it.
//!
//! Nothing here writes. The repair is a deployment's decision, and a probe
//! that fixed what it found would be making it for them.

use crate::{Pool, PoolError};

/// One text column's exposure to the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unnormalized {
    pub table: String,
    pub column: String,
    /// Rows whose stored text differs from its own NFC form.
    pub rows: i64,
}

/// A value two or more rows would share after normalization, inside a column
/// that currently forbids sharing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub table: String,
    pub column: String,
    /// The NFC form the colliding rows would all take.
    pub value: String,
    pub rows: i64,
}

/// Count rows that are not already in their stored form.
///
/// `columns` is the set to examine — the caller names them because only the
/// schema knows which columns hold language-level `text`, and guessing from
/// `information_schema` would sweep in the platform's own columns.
pub async fn unnormalized(
    pool: &Pool,
    columns: &[(&str, &str)],
) -> Result<Vec<Unnormalized>, PoolError> {
    let mut out = Vec::new();
    for (table, column) in columns {
        let sql = format!(
            "SELECT count(*) FROM {t} WHERE {c} IS NOT NULL \
             AND {c} <> normalize({c}, NFC)",
            t = quote_ident(table),
            c = quote_ident(column),
        );
        let rows = pool.rows(&sql).await?;
        let n: i64 = rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if n > 0 {
            out.push(Unnormalized {
                table: table.to_string(),
                column: column.to_string(),
                rows: n,
            });
        }
    }
    Ok(out)
}

/// Values that would collide if `columns` were normalized in place.
///
/// Ask this of columns that carry a uniqueness constraint. A non-empty
/// result is the destructive case: the rows are distinct now and would not
/// be afterwards, so a migration cannot proceed unattended.
pub async fn collisions(
    pool: &Pool,
    columns: &[(&str, &str)],
) -> Result<Vec<Collision>, PoolError> {
    let mut out = Vec::new();
    for (table, column) in columns {
        // Grouping on the normalized form is the whole question: rows that
        // are distinct today and land in one group are exactly the rows a
        // unique constraint would refuse mid-migration.
        let sql = format!(
            "SELECT normalize({c}, NFC) AS v, count(*) AS n FROM {t} \
             WHERE {c} IS NOT NULL GROUP BY 1 HAVING count(*) > 1 ORDER BY 1",
            t = quote_ident(table),
            c = quote_ident(column),
        );
        for row in pool.rows(&sql).await? {
            let value = row.first().cloned().flatten().unwrap_or_default();
            let n: i64 = row
                .get(1)
                .cloned()
                .flatten()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            out.push(Collision {
                table: table.to_string(),
                column: column.to_string(),
                value,
                rows: n,
            });
        }
    }
    Ok(out)
}

/// Identifiers come from the schema, not from a caller — but this path builds
/// SQL by formatting, and an unquoted identifier here would be the one place
/// the platform interpolates a name into a statement without saying so.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
