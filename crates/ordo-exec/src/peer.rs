//! The settings an external reads from: a peer's door, or a plain
//! external's schema swap.
//!
//! A program writes `peer notes { std.notes.list }`, and an external `from
//! notes.notes` reads the holder's `notes` through the holder's door
//! (`plan-federation-grants.md` §4, [`crate::peer_reads`]). The file never
//! names the door: the setting `ORDO_PEER_NOTES_URL` does. One Ordo
//! deployment never reads another's tables (owner, board
//! `federation-ordo-deployments-share-data-through`, asks 1 and 2: the
//! shared-database path was removed in G8).
//!
//! A plain external, of a system that is not Ordo, may swap the schema its
//! file writes with `ORDO_EXTERNAL_<SCHEMA>_SCHEMA` (owner, 2026-11-17, ask
//! 12): `ORDO_EXTERNAL_CRM_SCHEMA=crm_dev` reads `crm_dev.customers` where
//! the file says `crm.customers`. Unset, the schema is as written. A schema
//! written in quotes that is not lowercase letters, digits and underscores
//! (`"Legacy"`) has no clean variable name and is not swapped.
//!
//! [`resolve`] reads both, before a program serves. No I/O. A peer with no
//! door, or a bad swap, is a [`SchemaRefusal`] naming the variable and what
//! to set.

use ordo_schema::Schema;

use crate::{check_schema_setting, schema_setting};

/// `ORDO_PEER_<NAME>_URL`: where the peer's door answers. The host's
/// `peer_client::url_env` is the same name.
pub fn door_url_env(peer: &str) -> String {
    format!("ORDO_PEER_{}_URL", peer.to_ascii_uppercase())
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

/// Why a program's externals cannot be read here. Each names the setting.
#[derive(Debug)]
pub enum SchemaRefusal {
    /// A peer's `ORDO_PEER_<NAME>_URL` is not set.
    Unset { peer: String, var: String },
    /// A plain external's swap is not a schema name Ordo accepts.
    Invalid { var: String, detail: String },
}

impl SchemaRefusal {
    /// The setting the refusal is about.
    pub fn var(&self) -> Option<&str> {
        match self {
            SchemaRefusal::Unset { var, .. } | SchemaRefusal::Invalid { var, .. } => Some(var),
        }
    }
}

impl std::fmt::Display for SchemaRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaRefusal::Unset { peer, var } => write!(
                f,
                "{var} is not set. The program declares `peer {peer}`, which is read through \
                 its door, and the door's address comes from this setting. Set {var} to \
                 {peer}'s door, such as https://notes.example.com."
            ),
            SchemaRefusal::Invalid { detail, .. } => f.write_str(detail),
        }
    }
}

impl std::error::Error for SchemaRefusal {}

/// [`resolve_with`] over this process's environment.
pub fn resolve(schema: &mut Schema) -> Result<(), SchemaRefusal> {
    resolve_with(schema, schema_setting)
}

/// Check that each peer has a door, and write each plain external's swapped
/// schema into it. `lookup` reads a setting by name; a test hands in its
/// own.
pub fn resolve_with(
    schema: &mut Schema,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(), SchemaRefusal> {
    for peer in &schema.peer_uses {
        let var = door_url_env(&peer.name);
        if lookup(&var).is_none_or(|v| v.trim().is_empty()) {
            return Err(SchemaRefusal::Unset {
                peer: peer.name.clone(),
                var,
            });
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
