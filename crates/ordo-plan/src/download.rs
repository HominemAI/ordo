//! The `download` flag: which ops fill a person's own download.
//!
//! Owner ruling 2026-10-22 ("A person's own download"): "Yes only if allowed
//! by the app dev. There could also be filtering to determine what 'all' is."
//! The app developer marks read ops `download`. Each marked op is one entity
//! in the download: its projection is the fields, its conditions are the
//! filter, and the row guard decides the rows, because the door runs the op
//! as the person. A blob field in the projection brings the files.
//!
//! What is checked here, at compile time, so a deploy refuses a mark the
//! door could not honour:
//!
//! - `download_write`: the op is a read.
//! - `download_params`: every parameter has a default. The door calls the op
//!   with none.
//! - `download_public`: the op is not `public`. A public op has no caller, and
//!   a person's download is about the caller.
//! - `download_source`: the op reads one entity directly, so the download can
//!   name the entity its rows are.
//! - `download_entity_twice`: one marked op per entity, so each entity is one
//!   file in the download.

use std::collections::BTreeMap;

use ordo_schema::Schema;
use ordo_sema::{Registry, SemaError};
use ordo_syntax::{OpBody, OpDecl};

/// Every marked op passes, or the first that does not is refused.
pub fn check_download(schema: &Schema, registry: &Registry) -> Result<(), SemaError> {
    let mut names: Vec<&String> = registry.names().collect();
    names.sort();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for name in names {
        let Some(op) = registry.get(name) else {
            continue;
        };
        if !op.flags.download {
            continue;
        }
        let entity = check_op(schema, name, op)?;
        if let Some(first) = seen.get(&entity) {
            return Err(refuse(
                "download_entity_twice",
                op,
                format!(
                    "`{name}` and `{first}` are both marked `download` and both read \
                     `{entity}`. A download holds one op per entity; mark one of them."
                ),
            ));
        }
        seen.insert(entity, name.clone());
    }
    Ok(())
}

/// The entity a marked op reads, for the ops the door can run with no
/// parameters. `None` when the op is not marked or would be refused.
pub fn download_entity(schema: &Schema, op: &OpDecl) -> Option<String> {
    if !op.flags.download {
        return None;
    }
    check_op(schema, &op.name.name, op).ok()
}

fn refuse(code: &'static str, op: &OpDecl, message: String) -> SemaError {
    SemaError {
        code,
        message,
        span: op.name.span,
    }
}

fn check_op(schema: &Schema, name: &str, op: &OpDecl) -> Result<String, SemaError> {
    let pl = match &op.body {
        OpBody::Read(pl) => pl,
        OpBody::Write(_) => {
            return Err(refuse(
                "download_write",
                op,
                format!(
                    "`{name}` is marked `download` and writes. Only a read can fill a \
                     person's download; mark the read op that answers the rows."
                ),
            ))
        }
    };
    if let Some(p) = op.params.iter().find(|p| p.default.is_none()) {
        return Err(refuse(
            "download_params",
            op,
            format!(
                "`{name}` is marked `download` and `${}` has no default. The download \
                 calls the op with no parameters; give `${}` a default or drop it.",
                p.name.name, p.name.name
            ),
        ));
    }
    if op.flags.public {
        return Err(refuse(
            "download_public",
            op,
            format!(
                "`{name}` is marked both `public` and `download`. A public op has no \
                 caller, and a person's download holds what that person may read. \
                 Drop one of the two."
            ),
        ));
    }
    let source = match pl.sources.as_slice() {
        [one] if schema.entity(&one.name.name).is_some() => one.name.name.clone(),
        _ => {
            return Err(refuse(
                "download_source",
                op,
                format!(
                    "`{name}` is marked `download` and does not read one entity directly. \
                     A download names the entity its rows are; read the entity, \
                     `notes[id, title]`, not another op or several sources."
                ),
            ))
        }
    };
    Ok(source)
}
