//! Peers: other Ordo deployments in the same database (owner rulings
//! 2026-11-16 and 2026-11-17, `example-apps-notes-v2-plus-note_viewer` asks
//! 1, 2, 5 and 10).
//!
//! A program writes `peer journal`, and an external `from journal.notes`
//! reads the peer's `notes`. The file never names the peer's schema: the
//! setting `ORDO_PEER_JOURNAL_SCHEMA` does, read here through the same
//! reader and the same check as the deployment's own `ORDO_PG_SCHEMA`
//! ([`crate::schema_setting`], [`crate::check_schema_setting`]).
//!
//! A plain external may swap the schema its file writes with
//! `ORDO_EXTERNAL_<SCHEMA>_SCHEMA` (owner, 2026-11-17, ask 12):
//! `ORDO_EXTERNAL_CRM_SCHEMA=crm_dev` reads `crm_dev.customers` where the file
//! says `crm.customers`. Unset, the schema is as written. It is read here too,
//! by the same reader and check. A name that is both a `peer` and the schema
//! of a plain external cannot occur: with a `peer` line, every external
//! `from <name>.` is the peer's, and its `ORDO_EXTERNAL_..._SCHEMA` is not
//! read. A schema written in quotes that is not lowercase letters, digits and
//! underscores (`"Legacy"`) has no clean variable name and is not swapped.
//!
//! Two steps, both before a program serves:
//!
//! - [`resolve`] reads each peer's setting, and each plain external's
//!   optional one, and writes the schema into the program's externals. No
//!   I/O. A missing peer setting or a bad value is a [`SchemaRefusal`] naming
//!   the variable and what to set.
//! - [`check`] asks the database whether each peer schema holds an Ordo
//!   deployment: the tables `$caller` is resolved through must be there.

use ordo_schema::Schema;

use crate::{check_schema_setting, schema_setting, sql_lit, Pool, PoolError, DEFAULT_SCHEMA};

/// The setting that names a peer's schema: `ORDO_PEER_<NAME>_SCHEMA`, the
/// nickname upper-cased (owner, 2026-11-17, ask 10). The parser admits only
/// lowercase letters and digits in a nickname, so no two share a variable.
pub fn schema_env(peer: &str) -> String {
    format!("ORDO_PEER_{}_SCHEMA", peer.to_ascii_uppercase())
}

/// The optional setting that swaps a plain external's schema:
/// `ORDO_EXTERNAL_<SCHEMA>_SCHEMA`, the schema as the file writes it,
/// upper-cased (owner, 2026-11-17, ask 12). `None` when the written name has
/// no clean variable name: anything but lowercase letters, digits and
/// underscores, which only a quoted name can hold.
pub fn external_schema_env(written: &str) -> Option<String> {
    let clean = written
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && written
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    clean.then(|| format!("ORDO_EXTERNAL_{}_SCHEMA", written.to_ascii_uppercase()))
}

/// The tables a peer schema must hold for Ordo to resolve `$caller` there:
/// the command log every deployment keeps, its members, and the directory
/// that links a sign-in subject to a person.
pub const PEER_TABLES: [&str; 4] = ["$commandlog", "members", "$authenticator", "$principal"];

/// Why a program's external schemas cannot be resolved: a peer's setting, or
/// a plain external's swap. Each names the setting.
#[derive(Debug)]
pub enum SchemaRefusal {
    /// The setting is not set.
    Unset { peer: String, var: String },
    /// The setting (a peer's, or a plain external's swap) is not a schema
    /// name Ordo accepts.
    Invalid { var: String, detail: String },
    /// The setting names this deployment's own schema.
    Own { var: String, schema: String },
    /// The schema holds no Ordo deployment: one of [`PEER_TABLES`] is
    /// missing.
    NotOrdo {
        var: String,
        schema: String,
        missing: Vec<String>,
    },
    /// The database could not be asked.
    Database(PoolError),
}

impl SchemaRefusal {
    /// The setting the refusal is about, when it is about one.
    pub fn var(&self) -> Option<&str> {
        match self {
            SchemaRefusal::Unset { var, .. }
            | SchemaRefusal::Invalid { var, .. }
            | SchemaRefusal::Own { var, .. }
            | SchemaRefusal::NotOrdo { var, .. } => Some(var),
            SchemaRefusal::Database(_) => None,
        }
    }
}

impl std::fmt::Display for SchemaRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaRefusal::Unset { peer, var } => write!(
                f,
                "{var} is not set. The program declares `peer {peer}`, and the peer's \
                 schema comes from this setting. Set {var} to the schema the other \
                 deployment's ORDO_PG_SCHEMA names (`{DEFAULT_SCHEMA}` when it sets none)."
            ),
            SchemaRefusal::Invalid { detail, .. } => f.write_str(detail),
            SchemaRefusal::Own { var, schema } => write!(
                f,
                "{var} is `{schema}`, this deployment's own schema. A peer is another \
                 deployment: set {var} to the schema that deployment's ORDO_PG_SCHEMA names."
            ),
            SchemaRefusal::NotOrdo {
                var,
                schema,
                missing,
            } => {
                let list = missing
                    .iter()
                    .map(|t| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let verb = if missing.len() == 1 { "is" } else { "are" };
                write!(
                    f,
                    "{var} is `{schema}`, which holds no Ordo deployment: {list} {verb} not \
                     there. Set {var} to the schema an Ordo deployment's ORDO_PG_SCHEMA \
                     names, and start that deployment once so its tables exist."
                )
            }
            SchemaRefusal::Database(e) => write!(f, "the peer check could not read: {e}"),
        }
    }
}

impl std::error::Error for SchemaRefusal {}

/// [`resolve_with`] over this process's environment.
pub fn resolve(schema: &mut Schema, own: &str) -> Result<(), SchemaRefusal> {
    resolve_with(schema, own, schema_setting)
}

/// Write each peer's schema into the externals that read it. `own` is this
/// deployment's schema, which a peer may not name. `lookup` reads a setting
/// by name; a test hands in its own.
pub fn resolve_with(
    schema: &mut Schema,
    own: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(), SchemaRefusal> {
    for peer in schema.peers.clone() {
        let var = schema_env(&peer);
        let Some(value) = lookup(&var) else {
            return Err(SchemaRefusal::Unset { peer, var });
        };
        check_schema_setting(
            &var,
            &value,
            &format!("Set {var} to the schema the other deployment's ORDO_PG_SCHEMA names."),
        )
        .map_err(|detail| SchemaRefusal::Invalid {
            var: var.clone(),
            detail,
        })?;
        if value == own {
            return Err(SchemaRefusal::Own { var, schema: value });
        }
        for e in &mut schema.externals {
            if let Some(src) = e.external.as_mut() {
                if let Some(p) = src.peer.as_mut().filter(|p| p.name == peer) {
                    p.schema = Some(value.clone());
                }
            }
        }
    }
    // Plain externals: the optional swap. Each written schema is read once.
    for e in &mut schema.externals {
        let Some(src) = e.external.as_mut() else {
            continue;
        };
        if src.peer.is_some() {
            continue;
        }
        let Some(var) = external_schema_env(&src.schema) else {
            continue;
        };
        let Some(value) = lookup(&var) else {
            src.swapped = None;
            continue;
        };
        check_schema_setting(
            &var,
            &value,
            &format!(
                "Set {var} to the schema that holds `{}` in this environment, or unset it \
                 to read `{}` as the file writes it.",
                src.table, src.schema
            ),
        )
        .map_err(|detail| SchemaRefusal::Invalid {
            var: var.clone(),
            detail,
        })?;
        src.swapped = Some(value);
    }
    Ok(())
}

/// Whether each resolved peer schema holds an Ordo deployment.
pub async fn check(pool: &Pool, schema: &Schema) -> Result<(), SchemaRefusal> {
    for peer in &schema.peers {
        let Some(pg) = schema
            .externals
            .iter()
            .filter_map(|e| e.external.as_ref()?.peer.as_ref())
            .find(|p| &p.name == peer)
            .and_then(|p| p.schema.clone())
        else {
            continue;
        };
        let probes = PEER_TABLES
            .iter()
            .map(|t| {
                format!(
                    "to_regclass({}) IS NULL",
                    sql_lit(&format!("{}.{}", quote(&pg), quote(t)))
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let rows = pool
            .rows(&format!("SELECT {probes}"))
            .await
            .map_err(SchemaRefusal::Database)?;
        let missing: Vec<String> = rows
            .first()
            .map(|r| {
                r.iter()
                    .zip(PEER_TABLES)
                    .filter(|(gone, _)| gone.as_deref() == Some("t"))
                    .map(|(_, t)| t.to_string())
                    .collect()
            })
            .unwrap_or_default();
        if !missing.is_empty() {
            return Err(SchemaRefusal::NotOrdo {
                var: schema_env(peer),
                schema: pg,
                missing,
            });
        }
    }
    Ok(())
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
