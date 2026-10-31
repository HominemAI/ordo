//! The registration gate for `public` ops (`plan-syntax-additions.md` §1,
//! `design-public-path.md` §2).
//!
//! A public op is served to a visitor with no token, from a cache, with no
//! guard walk and no per-call audit row. Skipping those is safe only because
//! this gate has already proved what the op can disclose: public fields, and
//! nothing else. The runtime relies on that proof and does not re-derive it.
//!
//! The proof reads the op's field-scope manifest ([`crate::artifact::op_scope`]),
//! which is taken from the bound IR. Binding flattens composition and keeps
//! every column the plan touches, so a field read only in a condition, a sort
//! key or a join is in it. A condition over a private field is an oracle (one
//! bit per call), which is why conditions count as reads here.
//!
//! # Classes
//!
//! | Class | Refused |
//! |---|---|
//! | `public_reads_private` | a public read whose scope holds a field that is not public, or an entity with no public field |
//! | `public_composes_private` | a public read composing an op that is not public |
//! | `public_caller` | `$caller` anywhere in a public op |
//! | `public_write_not_create` | a public write with a statement that is not a create: an update, a converge (a unique field without `rev: null`), or a delete |
//! | `public_write_guarded` | a public write into an entity with a guard or an `acl` |
//! | `public_write_ref` | a public write that sets a ref from a visitor's value, to an entity with no public field |
//!
//! `public_sealed`, `public_private` and `public_guarded` are the schema's,
//! because they are about declarations rather than ops.
//!
//! # A public write (`design-public-path.md` §5)
//!
//! A visitor may only add rows. They may never touch a row that exists, and
//! learn nothing from the answer but the new row's id. So every statement
//! must be a create: a plain one ([`crate::writes::SetMode::Create`]) or a
//! create-once keyed on a unique field (`rev: null`). A converge or an update
//! would let a visitor change a row they did not write. A create-once on a
//! unique field is allowed (owner, 2026-10-12, ask 31), and the door gives the
//! visitor the same answer whether the row is new or already there, so the
//! answer never says whether somebody's email is on a list ([`has_keyed_create`]
//! marks such an op). A ref set from a visitor's value probes whether that row
//! exists, so it may only point at an entity that is public anyway. A file
//! field cannot be set at all: a blob is never an op parameter, and the
//! upload route takes a token.

use ordo_schema::Schema;
use ordo_sema::{Registry, SemaError};
use ordo_syntax::decl::MatchKind;
use ordo_syntax::{Expr, OpBody, OpDecl, Pipeline, Step, WriteStmt};

/// Every public op in the registry passes, or the first that does not is
/// refused with its class.
pub fn check_public(schema: &Schema, registry: &Registry) -> Result<(), SemaError> {
    let mut names: Vec<&String> = registry.names().collect();
    names.sort();
    for name in names {
        let Some(op) = registry.get(name) else {
            continue;
        };
        if !op.flags.public {
            continue;
        }
        check_op(schema, registry, name, op)?;
    }
    Ok(())
}

fn refuse(code: &'static str, op: &OpDecl, message: String) -> SemaError {
    SemaError {
        code,
        message,
        span: op.name.span,
    }
}

fn check_op(
    schema: &Schema,
    registry: &Registry,
    name: &str,
    op: &OpDecl,
) -> Result<(), SemaError> {
    // `$caller` first: it is the plainest mistake and the clearest message.
    // The derived `Debug` form covers every expression position the tree
    // has, so no hand-written walk can forget one.
    if format!("{op:?}").contains("Param { name: \"caller\"") {
        return Err(refuse(
            "public_caller",
            op,
            format!(
                "`{name}` is public and names `$caller`. A public op has no caller: every \
                 visitor gets the same answer, which is what lets it be cached. Drop the \
                 condition, or remove `public`."
            ),
        ));
    }
    let pl = match &op.body {
        OpBody::Read(pl) => pl,
        OpBody::Write(stmts) => return check_write(schema, name, op, stmts),
    };
    let mut callees = Vec::new();
    sources_of(pl, &mut callees);
    for callee in callees {
        if let Some(c) = registry.get(&callee) {
            if !c.flags.public {
                return Err(refuse(
                    "public_composes_private",
                    op,
                    format!(
                        "`{name}` is public and sources `{callee}`, which is not. A public op \
                         composes public ops only; mark `{callee}` public, or read the \
                         entity directly."
                    ),
                ));
            }
        } else if callee.starts_with("std.") {
            return Err(refuse(
                "public_composes_private",
                op,
                format!(
                    "`{name}` is public and sources `{callee}`, a platform op. Platform ops \
                     are never public; read the entity directly."
                ),
            ));
        }
    }
    for scope in crate::artifact::op_scope(schema, registry, name) {
        let Some(entity) = schema.entity(&scope.entity) else {
            continue;
        };
        if !entity.has_public() {
            return Err(refuse(
                "public_reads_private",
                op,
                format!(
                    "`{name}` is public and reads `{}`, which has no public field. A public \
                     op reads public fields only.",
                    scope.entity
                ),
            ));
        }
        for field in &scope.read {
            let public = entity.field(field).is_none_or(|f| f.public || f.implicit);
            if !public {
                return Err(refuse(
                    "public_reads_private",
                    op,
                    format!(
                        "`{name}` is public and reads `{}.{field}`, which is not. A public op \
                         reads public fields only, in its output and in every condition, \
                         sort and join, because a condition on a private field discloses it \
                         a bit at a time. Mark `{field}` public, or stop reading it.",
                        scope.entity
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The names a pipeline reads from: its sources, its `let`s' sources, and
/// the sources of `match any/none` and `append`. A name that is an op is a
/// callee.
fn sources_of(pl: &Pipeline, out: &mut Vec<String>) {
    for l in &pl.lets {
        sources_of(&l.pipeline, out);
    }
    for s in &pl.sources {
        out.push(s.name.name.clone());
    }
    for st in &pl.steps {
        match st {
            Step::Match {
                kind: MatchKind::Any(s) | MatchKind::None(s),
                ..
            }
            | Step::Append { source: s, .. } => out.push(s.name.name.clone()),
            _ => {}
        }
    }
}

/// A public write: plain creates into unguarded entities, with no ref a
/// visitor can aim at a private row and no file.
fn check_write(
    schema: &Schema,
    name: &str,
    op: &OpDecl,
    stmts: &[WriteStmt],
) -> Result<(), SemaError> {
    for stmt in stmts {
        let (entity, fields) = match stmt {
            WriteStmt::Set {
                entity,
                cond: None,
                fields,
                ..
            } if matches!(
                crate::writes::set_mode(schema, &entity.name, fields),
                crate::writes::SetMode::Create
                    | crate::writes::SetMode::Keyed {
                        create_once: true,
                        ..
                    }
            ) =>
            {
                (entity, fields)
            }
            other => {
                let label = crate::writes::stmt_label(schema, other);
                return Err(refuse(
                    "public_write_not_create",
                    op,
                    format!(
                        "`{name}` is a public write, and one of its statements {label}. A \
                         public write only adds rows. A visitor may not change, converge or \
                         delete a row. Write a plain `set <entity> {{ … }}` with no `id` and \
                         no condition, add `rev: null` when it is keyed on a unique field, \
                         or remove `public`."
                    ),
                ));
            }
        };
        let Some(e) = schema.entity(&entity.name) else {
            continue; // `unknown_source` is another check's answer.
        };
        if e.guards.is_some() || e.acl_levels.is_some() {
            return Err(refuse(
                "public_write_guarded",
                op,
                format!(
                    "`{name}` is a public write into `{}`, which is guarded. A visitor has \
                     no caller for a guard to test. Write into an unguarded entity, or \
                     remove `public`.",
                    entity.name
                ),
            ));
        }
        for (field, value) in fields {
            let Some(f) = e.field(&field.name) else {
                continue;
            };
            if let ordo_schema::FieldTy::Ref { target, .. } = &f.ty {
                let target_public = schema.entity(target).is_some_and(|t| t.has_public());
                if names_param(value) && !target_public {
                    return Err(refuse(
                        "public_write_ref",
                        op,
                        format!(
                            "`{name}` is a public write and sets `{}.{}` from a visitor's \
                             value. `{target}` has no public field, so the answer would tell \
                             a visitor whether a row of it exists. Set the ref from a row \
                             this op creates or from a literal, or remove `public`.",
                            entity.name, field.name
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Whether an expression carries a value the caller sent.
fn names_param(e: &Expr) -> bool {
    match e {
        Expr::Param { .. } => true,
        Expr::Binary { lhs, rhs, .. } => names_param(lhs) || names_param(rhs),
        Expr::Neg { expr, .. } => names_param(expr),
        Expr::FnCall { args, .. } => args.iter().any(names_param),
        Expr::Cond { arms, else_, .. } => {
            arms.iter().any(|a| names_param(&a.value)) || names_param(else_)
        }
        _ => false,
    }
}

/// Whether a public write has a create-once keyed on a unique field. The door
/// gives such an op one answer for a new row and an existing one, so a visitor
/// cannot learn whether a value is already taken.
pub fn has_keyed_create(schema: &Schema, stmts: &[WriteStmt]) -> bool {
    stmts.iter().any(|s| {
        matches!(
            s,
            WriteStmt::Set { entity, cond: None, fields, .. }
                if matches!(
                    crate::writes::set_mode(schema, &entity.name, fields),
                    crate::writes::SetMode::Keyed { .. }
                )
        )
    })
}
