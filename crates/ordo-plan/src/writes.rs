//! The write gate's compile half: DL §6 safety rules 2 and 3.
//!
//! Both rules are about what a write statement can be *before* any row
//! exists, which is why they live at registration rather than in the
//! executor. A create missing a required field is not a fact about the
//! data — it is a fact about the statement, and discovering it as a
//! Postgres NOT NULL violation on the first call means the wrong layer
//! answered, in the wrong error class, at the worst possible time.
//!
//! - **Rule 2, insert-completeness**: any statement whose mode can create
//!   must statically cover every required field — every declared field
//!   that is neither optional nor defaulted. Implicit fields (`id`,
//!   `rev`, `depth`) are the platform's and are never caller-supplied, so
//!   they are never "missing".
//! - **Rule 3, bulk braces**: a ranged write may not assign `id` or a
//!   unique field. Assigning `id` blurs addressed and bulk; assigning one
//!   unique value across a range asks every matched row to collide on
//!   that key, which is not a write anyone can mean.
//!
//! The mode discriminator here is the AST's, not the executor's: at
//! registration there are no bound parameters, so `cond: None` is the
//! creation-capable form (plain create, and — once §6 rule 8's unique-key
//! addressing lands — converge-or-create, which creates when the key
//! misses), a bare `id == <literal or $param>` is addressed, and every
//! other condition ranges.

use ordo_schema::Schema;
use ordo_sema::{Registry, SemaError};
use ordo_syntax::decl::{OpBody, ScalarTy, WriteStmt};
use ordo_syntax::{BinOp, CmpOp, Condition, Expr, Ident, Span};

/// Where, if anywhere, an unconverted `decimal` is hiding in a write RHS.
///
/// DL §2 ("No implicit rounding, anywhere") makes a `decimal` landing in a
/// `money`/`int` slot a compile error, class `implicit_rounding`, until an
/// explicit `round(x, n)` / `floor(x)` / `ceil(x)` says how it converts.
/// The canonical case is `traps.md`'s:
///
/// ```text
/// set budgets { id: $b, rev: $r, perHead: $total / $heads }
/// ```
///
/// This is **structural**, over the raw AST, because the write path has no
/// binder and therefore no types: `/` is the only thing in the expression
/// grammar that mints a decimal, and it stays one through `+ - *` and
/// through every function except the three conversions. A structural test
/// is exact for that grammar, and it is available at registration — which
/// is where a compile error belongs, rather than as a surprise on the
/// first call that happens to divide.
///
/// Since 27ak a decimal has two more sources than `/`: a `decimal(p)`
/// field or parameter, and a decimal literal multiplied in (`price *
/// 0.08`). `is_decimal` answers for the first, from the schema and the
/// op's declarations; the second is structural. A decimal literal added to
/// a money is not here: `price + 0.01` is a money, converted by the binder.
fn unconverted_decimal(e: &Expr, is_decimal: &dyn Fn(&Expr) -> bool) -> Option<Span> {
    if is_decimal(e) {
        return Some(e.span());
    }
    match e {
        Expr::Binary { op, lhs, rhs, span } => {
            if matches!(op, BinOp::Div) {
                return Some(*span);
            }
            if matches!(op, BinOp::Mul)
                && (matches!(**lhs, Expr::DecimalLit { .. })
                    || matches!(**rhs, Expr::DecimalLit { .. }))
            {
                return Some(*span);
            }
            unconverted_decimal(lhs, is_decimal).or_else(|| unconverted_decimal(rhs, is_decimal))
        }
        Expr::Neg { expr, .. } => unconverted_decimal(expr, is_decimal),
        // The three conversions stop the walk — that is what writing one
        // means. Their ARGUMENTS are not re-examined: `round(a / b, 2)` is
        // exactly the stated boundary the rule asks for.
        Expr::FnCall { name, args, .. } => {
            if matches!(name.name.as_str(), "round" | "floor" | "ceil") {
                return None;
            }
            args.iter().find_map(|a| unconverted_decimal(a, is_decimal))
        }
        Expr::Cond { arms, else_, .. } => arms
            .iter()
            .find_map(|a| unconverted_decimal(&a.value, is_decimal))
            .or_else(|| unconverted_decimal(else_, is_decimal)),
        _ => None,
    }
}

/// Every **one-segment** path in a write's right-hand side, in source
/// order: the columns of the row being written that this value reads.
///
/// Conditions inside a `cond` arm count, and must. DL: *"Arm conditions use
/// the full condition grammar … but never existence tests — conditions see
/// the row, not other rows."* A `cond(used < seats -> used + 1, used)` reads
/// the row in three places and it is the same row in all three, so the
/// statement is row-reading whichever place did the reading.
///
/// Two-segment paths are [`dotted_paths`]' business (a `let` descriptor, or
/// a refusal); nothing here looks at them.
fn row_paths(e: &Expr) -> Vec<(&Ident, Span)> {
    fn expr<'a>(e: &'a Expr, out: &mut Vec<(&'a Ident, Span)>) {
        match e {
            Expr::Path { segments, span } if segments.len() == 1 => out.push((&segments[0], *span)),
            Expr::Binary { lhs, rhs, .. } => {
                expr(lhs, out);
                expr(rhs, out);
            }
            Expr::Neg { expr: e, .. } => expr(e, out),
            Expr::FnCall { args, .. } => args.iter().for_each(|a| expr(a, out)),
            Expr::Cond { arms, else_, .. } => {
                for a in arms {
                    cond(&a.condition, out);
                    expr(&a.value, out);
                }
                expr(else_, out);
            }
            _ => {}
        }
    }
    fn cond<'a>(c: &'a Condition, out: &mut Vec<(&'a Ident, Span)>) {
        match c {
            Condition::Compare { lhs, rhs, .. } | Condition::Text { lhs, rhs, .. } => {
                expr(lhs, out);
                expr(rhs, out);
            }
            Condition::In { expr: e, list, .. } => {
                expr(e, out);
                list.iter().for_each(|x| expr(x, out));
            }
            Condition::IsNull { expr: e, .. } | Condition::BareBool(e) => expr(e, out),
            Condition::Since {
                expr: e, dur: x, ..
            }
            | Condition::Under { expr: e, id: x, .. }
            | Condition::Above { expr: e, id: x, .. } => {
                expr(e, out);
                expr(x, out);
            }
            Condition::And { items, .. } | Condition::Or { items, .. } => {
                items.iter().for_each(|i| cond(i, out))
            }
            Condition::Not { inner, .. } => cond(inner, out),
        }
    }
    let mut out = Vec::new();
    expr(e, &mut out);
    out
}

/// **A converge-or-create reads each field as its default when it creates**
/// (owner, 2026-10-31). `set counters { key: $k, n: n + 1 }` converges onto
/// the row holding `$k` and adds one to its `n`; when no row holds `$k`, the
/// create branch reads `n` as `n`'s declared default, so the first write
/// stores `0 + 1`. This is SQL's `INSERT … ON CONFLICT DO UPDATE` counter.
///
/// Answers the first field the value reads that has no declared default,
/// with where it was read; the caller turns that into its refusal
/// ([`no_default_on_create`]). An optional field is not read as null:
/// `n + 1` over a null is null, an answer nobody wrote.
///
/// The one place the rule lives: registration ([`check_writes`]) and the
/// write path's plan-time check both ask it.
pub fn create_branch_reads(e: &ordo_schema::Entity, value: &Expr) -> Result<(), (String, Span)> {
    match row_paths(value)
        .into_iter()
        .find(|(p, _)| e.field(&p.name).is_some_and(|f| f.default.is_none()))
    {
        Some((p, span)) => Err((p.name.clone(), span)),
        None => Ok(()),
    }
}

/// The refusal [`create_branch_reads`] earns, worded once for both layers:
/// it names the field that has no default and the two ways to fix it.
pub fn no_default_on_create(entity: &str, written: &str, read: &str) -> String {
    format!(
        "`{entity}.{written}` reads `{read}`, and this statement `creates-or-converges`: \
         when it creates the row it reads `{read}` as its default, and `{read}` has none. \
         Give `{read}` a default in `entity {entity}` (`{read}: <type> = <value>`), or \
         address the row \
         (`set {entity}(id == …) {{ … }}`)"
    )
}

/// **A right-hand side that reads the row is legal only where there IS a
/// row** — DL §6, "RHS field references": *"`{ n: n + 1 }` — computed
/// server-side inside the row's transaction; atomic increments without
/// read-modify-write races."*
///
/// Checked at registration, structurally over the AST, for the same reason
/// `insert_incomplete` is: whether a statement can read a current row is a
/// fact about the *statement*, decidable from the mode label the compiler
/// already prints, and discovering it on the first call that happens to
/// take the create branch is the wrong layer answering at the worst time.
/// The write path answers it again at plan time (the harness can plan an
/// unregistered statement), and both read the same [`stmt_label`], so they
/// cannot disagree about which statements have a row.
///
/// The second half is the fields whose value the platform must **hold**
/// before the write — sealed, unique, ref, closed-set, content. Each is a
/// real host-side check standing between the statement and the database,
/// and a value the database computes arrives after all of them. Refusing
/// here rather than at runtime means the op does not deploy.
fn check_computed_rhs(schema: &Schema, op: &str, stmt: &WriteStmt) -> Result<(), SemaError> {
    let WriteStmt::Set { entity, fields, .. } = stmt else {
        return Ok(()); // `del` carries no braces.
    };
    let Some(e) = schema.entity(&entity.name) else {
        return Ok(()); // `unknown_source` is another check's answer.
    };
    let label = stmt_label(schema, stmt);
    for (name, value) in fields {
        let Some(f) = e.field(&name.name) else {
            continue;
        };
        let paths = row_paths(value);
        if paths.is_empty() {
            continue;
        }
        // A misspelled column would otherwise reach the database as a
        // statement it refuses to describe, in an op nobody had just edited.
        //
        // **`projection_unknown_field`, not `unknown_field`.** GC088's
        // ruling: the class of a miss is decided by the POSITION that named
        // the field, not by the lookup that failed — and the binder's
        // `unknown_field` "is internal vocabulary that appears in no closed
        // inventory", so it must never be what a caller sees. The position
        // here is a `set`'s braces, whose inventory class for naming a field
        // the entity does not have is already `projection_unknown_field`;
        // the write path answers with it on the LEFT of the colon, and the
        // right of the colon is the same braces and the same mistake.
        if let Some((bad, span)) = paths.iter().find(|(p, _)| e.field(&p.name).is_none()) {
            return Err(SemaError {
                code: "projection_unknown_field",
                message: format!(
                    "`{op}` writes `{}.{}` from `{}`, and `{}` has no field `{}` — a \
                     right-hand side names columns of the row being written",
                    entity.name, f.name, bad.name, entity.name, bad.name,
                ),
                span: *span,
            });
        }
        let span = paths[0].1;
        // Converge-or-create reads each field as its default when it
        // creates (2026-10-31); a field with none is refused by name.
        if label == "creates-or-converges" {
            if let Err((read, at)) = create_branch_reads(e, value) {
                return Err(SemaError {
                    code: "invalid_binding_expr",
                    message: format!(
                        "`{op}`: {}",
                        no_default_on_create(&entity.name, &f.name, &read)
                    ),
                    span: at,
                });
            }
        } else if !matches!(label, "converges" | "bulk") {
            return Err(SemaError {
                code: "invalid_binding_expr",
                message: format!(
                    "`{op}` writes `{}.{}` from a value that reads the row, and this \
                     statement `{label}` — a create has no current row to read. Address \
                     the row (`set {}(id == …) {{ … }}`, or `{{ id: …, … }}`), or write \
                     the value out",
                    entity.name, f.name, entity.name,
                ),
                span,
            });
        }
        let refuse = |why: &str| {
            Err(SemaError {
                code: "invalid_binding_expr",
                message: format!(
                    "`{op}` computes `{}.{}` from the row, and that field {why} — a \
                     computed right-hand side is evaluated by the statement itself, so \
                     the platform never holds the value",
                    entity.name, f.name,
                ),
                span,
            })
        };
        if f.sealed {
            return refuse("is sealed, and sealing needs the plaintext");
        }
        if (f.unique && !f.implicit) || e.unique_keys.iter().any(|k| k.contains(&f.name)) {
            return refuse(
                "carries a unique key, and rule 8 must know which row the value identifies",
            );
        }
        if matches!(f.ty, ordo_schema::FieldTy::Ref { .. }) {
            return refuse("is a ref, and G12 must prove the target is one you can see");
        }
        if f.enum_values.is_some() {
            return refuse("is pinned to a closed set of values, checked before the write");
        }
        if matches!(f.ty.value_ty(), ScalarTy::Blob | ScalarTy::Uri) {
            return refuse(
                "is content: a blob handle is minted by an upload and a uri is validated at \
                 the door, so neither is a thing an expression computes",
            );
        }
    }
    Ok(())
}

/// **`allocate` has one legal home, and a write's braces are not it**
/// (DL §3, Allocation: "inside an `each` emission over a list parameter
/// … Anywhere else it is `invalid_allocate`").
///
/// The binder refuses `allocate` in every read position and the planner
/// refuses it at lowering, but a write's right-hand side is bound by the
/// write protocol's own folder, which
/// `set p(id == $id) { m: allocate($x, 1) }` sailed through: the call
/// folds to a constant and the statement registered. This is the gate for
/// that position. It walks the braces for the call and refuses by the
/// class DL names, so the day `each` parses the only change is the
/// position this check exempts.
fn check_allocate_site(op: &str, stmt: &WriteStmt) -> Result<(), SemaError> {
    fn find(e: &Expr) -> Option<Span> {
        match e {
            Expr::FnCall { name, args, span } => {
                if name.name == "allocate" {
                    return Some(*span);
                }
                args.iter().find_map(find)
            }
            Expr::Binary { lhs, rhs, .. } => find(lhs).or_else(|| find(rhs)),
            Expr::Neg { expr, .. } => find(expr),
            Expr::Cond { arms, else_, .. } => arms
                .iter()
                .find_map(|a| find(&a.value))
                .or_else(|| find(else_)),
            _ => None,
        }
    }
    let WriteStmt::Set { entity, fields, .. } = stmt else {
        return Ok(());
    };
    for (f, v) in fields {
        if let Some(span) = find(v) {
            return Err(SemaError {
                code: "invalid_allocate",
                message: format!(
                    "`{op}` writes `{}.{}` from `allocate(…)` — `allocate(total, weights)` is \
                     legal only inside an `each` emission over a list parameter, where the \
                     weights are the element field; a single write has no vector to split \
                     among (DL §3, Allocation)",
                    entity.name, f.name
                ),
                span,
            });
        }
    }
    Ok(())
}

/// `id == <literal or $param>` and nothing else — the addressed shape,
/// the compile-time mirror of the executor's `addressed_id`. A condition
/// that merely *mentions* `id` (`id == $t and status == "open"`) ranges,
/// exactly as it does at runtime.
fn is_id_addressed(c: &Condition) -> bool {
    let Condition::Compare {
        op: CmpOp::Eq,
        lhs: Expr::Path { segments, .. },
        rhs,
        ..
    } = c
    else {
        return false;
    };
    segments.len() == 1
        && segments[0].name == "id"
        && matches!(rhs, Expr::Param { .. } | Expr::StrLit { .. })
}

/// Every two-segment path in a write statement's braces and condition, in
/// source order.
///
/// A one-segment path in a write RHS is a **column of the row being
/// written** (`used: used + 1`), so it is not a binding reference and is
/// not collected. A two-segment path is: nothing else in the write grammar
/// dots, because a write's right-hand sides are literals, parameters and
/// `$caller`.
fn dotted_paths(stmt: &WriteStmt) -> Vec<(&Ident, &Ident, Span)> {
    fn expr<'a>(e: &'a Expr, out: &mut Vec<(&'a Ident, &'a Ident, Span)>) {
        match e {
            Expr::Path { segments, span } if segments.len() == 2 => {
                out.push((&segments[0], &segments[1], *span))
            }
            Expr::Binary { lhs, rhs, .. } => {
                expr(lhs, out);
                expr(rhs, out);
            }
            Expr::Neg { expr: e, .. } => expr(e, out),
            Expr::FnCall { args, .. } => args.iter().for_each(|a| expr(a, out)),
            Expr::Cond { arms, else_, .. } => {
                for a in arms {
                    cond(&a.condition, out);
                    expr(&a.value, out);
                }
                expr(else_, out);
            }
            _ => {}
        }
    }
    fn cond<'a>(c: &'a Condition, out: &mut Vec<(&'a Ident, &'a Ident, Span)>) {
        match c {
            Condition::Compare { lhs, rhs, .. } | Condition::Text { lhs, rhs, .. } => {
                expr(lhs, out);
                expr(rhs, out);
            }
            Condition::In { expr: e, list, .. } => {
                expr(e, out);
                list.iter().for_each(|x| expr(x, out));
            }
            Condition::IsNull { expr: e, .. } | Condition::BareBool(e) => expr(e, out),
            Condition::Since {
                expr: e, dur: x, ..
            }
            | Condition::Under { expr: e, id: x, .. }
            | Condition::Above { expr: e, id: x, .. } => {
                expr(e, out);
                expr(x, out);
            }
            Condition::And { items, .. } | Condition::Or { items, .. } => {
                items.iter().for_each(|i| cond(i, out))
            }
            Condition::Not { inner, .. } => cond(inner, out),
        }
    }
    let mut out = Vec::new();
    match stmt {
        WriteStmt::Set {
            cond: c, fields, ..
        } => {
            if let Some(c) = c {
                cond(c, &mut out);
            }
            for (_, v) in fields {
                expr(v, &mut out);
            }
        }
        WriteStmt::Del { cond: c, .. } => cond(c, &mut out),
    }
    out
}

/// **`let x = set …` binds a create, and only a create** — DL §3, "Naming a
/// freshly created row in the same op" [settled]; R-G19; GC094/GC095.
///
/// Two halves, and both are registration-time facts:
///
/// 1. **What may be bound.** *"`let` binds a `set` whose derived mode is
///    CREATE, because a create produces exactly one row with exactly one
///    fresh server-minted id. The other modes bind nothing and are a
///    compile error (`unbindable_write`)"* — and the mode is read off
///    [`stmt_label`], the single authority §6 rule 1 already routes every
///    renderer through, rather than by a second reading of the braces. That
///    is not tidiness: a `let` that bound one thing while the artifact
///    printed another is precisely the drift `stmt_label`'s doc comment
///    exists to prevent.
///
///    **The one case the spec does not spell, ruled here:** a `rev: null`
///    converge-or-create (`SetMode::Keyed { create_once: true }`) **is**
///    bindable. Its label is already `creates` — this engine ruled that
///    under rule 1, because "the statement asserts the row's absence, so
///    converging is not one of the things it can do" — and DL's exclusion
///    list is given with its reasons, all three of which it fails to
///    match: it addresses no row by an id the caller holds (CONVERGE, CAS)
///    and it affects no unknown number of rows (BULK). It produces exactly
///    one row with exactly one fresh minted id, which is the whole
///    justification the rule gives. Reading it as unbindable would refuse
///    a statement for a property it does not have.
///
/// 2. **Where a name is in scope.** *"usable in later statements of the
///    same op, never in the same statement and never in a condition of the
///    statement that produced it."* So the environment grows one statement
///    at a time and a reference is checked against what was bound
///    *before* it — which makes self-reference and forward reference the
///    same check, and both fall out as "not bound yet".
///
///    The descriptor is *"`id`, `rev`, and the fields that statement
///    wrote"*. `id` and `rev` are the platform's and are always there; the
///    field list is the statement's own braces, so `b.name` is in scope
///    exactly when the create said `name:`. A member outside that set is
///    `invalid_binding_expr` — the class for "a right-hand side whose
///    shape the statement cannot mean" — rather than a new one, because
///    the row descriptor is not a row and reading an unwritten column off
///    it is a shape error, not a missing field on an entity.
fn check_bindings(schema: &Schema, stmts: &[WriteStmt]) -> Result<(), SemaError> {
    // (name, the members it offers) — in binding order, which is statement
    // order, so `in_scope` is exactly "bound by an EARLIER statement".
    let mut in_scope: Vec<(&str, Vec<&str>)> = Vec::new();
    for stmt in stmts {
        // Checked BEFORE this statement's own binding joins the scope: a
        // name is not usable in the statement that produced it.
        for (head, member, span) in dotted_paths(stmt) {
            let Some((_, members)) = in_scope.iter().find(|(n, _)| *n == head.name) else {
                // Not a binding at all. Two-segment paths have no other
                // meaning in a write — there is nothing to dot through —
                // but naming the bindings that DO exist is what turns a
                // typo into a one-line fix.
                let known = if in_scope.is_empty() {
                    "no rows are bound in this op".to_string()
                } else {
                    format!(
                        "bound so far: {}",
                        quoted(&in_scope.iter().map(|(n, _)| *n).collect::<Vec<_>>())
                    )
                };
                return Err(SemaError {
                    code: "invalid_binding_expr",
                    message: format!(
                        "`{}.{}` names no bound row — a `let` name is in scope only for the \
                         statements AFTER the one that bound it ({known})",
                        head.name, member.name
                    ),
                    span,
                });
            };
            if !members.contains(&member.name.as_str()) {
                return Err(SemaError {
                    code: "invalid_binding_expr",
                    message: format!(
                        "`{}` is a row descriptor, not a row: it carries `id`, `rev` and the \
                         fields its statement wrote, and `{}` is not among them ({})",
                        head.name,
                        member.name,
                        quoted(members)
                    ),
                    span,
                });
            }
        }
        let WriteStmt::Set {
            binding: Some(name),
            entity,
            fields,
            span,
            ..
        } = stmt
        else {
            continue;
        };
        if stmt_label(schema, stmt) != "creates" {
            return Err(SemaError {
                code: "unbindable_write",
                message: format!(
                    "`let {} = set {}` binds a write whose derived mode is `{}` — only a \
                     create can be named: a converge or a CAS addresses a row whose id the \
                     caller already holds, and a bulk write touches an unknown number of \
                     rows, so there is no single `.id` to bind (DL §3, R-G19)",
                    name.name,
                    entity.name,
                    stmt_label(schema, stmt),
                ),
                span: *span,
            });
        }
        if in_scope.iter().any(|(n, _)| *n == name.name) {
            return Err(SemaError {
                code: "invalid_binding_expr",
                message: format!(
                    "`{}` is bound twice in one op — a name in a command group refers to one \
                     created row, and a second binding would make every later reference \
                     ambiguous",
                    name.name
                ),
                span: name.span,
            });
        }
        let mut members = vec!["id", "rev"];
        members.extend(fields.iter().map(|(n, _)| n.name.as_str()));
        in_scope.push((&name.name, members));
    }
    Ok(())
}

/// Rules 2 and 3 over every registered write op's statements.
///
/// Statements naming an entity or a field the schema does not have are
/// left alone: `unknown_source` and `projection_unknown_field` are other
/// checks' answers, and shadowing them here would report the wrong
/// problem first.
/// A domain op may not write a platform-provided entity.
///
/// DL §3: a platform entity's fields are *"all read-only to domain code"*
/// and *"there is no domain-authored path to minting one"*. Nothing enforced
/// that, and the consequence was a privilege escalation rather than an
/// untidiness: `set members { id: $id, role: "owner" }` compiled, registered
/// and ran, while `ordo_host::deployment_engine` builds the door's access
/// engine by reading `SELECT "id", "role" FROM "members"` — so **the role a
/// domain op wrote was the role the door read**. The reference profile grants
/// authored write ops to Editor and up; an editor could promote themselves,
/// and every other member, to owner.
///
/// Refused at REGISTRATION, so the op never deploys. The write path refuses
/// it too, at its one dispatch point: an op that cannot execute should not
/// register, and a statement that should not run must not run even if it
/// somehow did.
///
/// The class is `reserved_derived_name`, whose documented meaning was already
/// "the statement writes something the platform owns" — the inventory row
/// named `id` and `rev` because those were the only two such things.
fn check_platform_entities(schema: &Schema, stmts: &[WriteStmt]) -> Result<(), SemaError> {
    for stmt in stmts {
        let (name, span) = match stmt {
            WriteStmt::Set { entity, span, .. } | WriteStmt::Del { entity, span, .. } => {
                (&entity.name, *span)
            }
        };
        if schema.entity(name).is_some_and(|e| e.platform) {
            return Err(SemaError {
                code: "reserved_derived_name",
                message: format!(
                    "`{name}` is provided by the platform and is read-only to domain code — \
                     its rows are minted and changed by the platform's own paths, never by \
                     an op"
                ),
                span,
            });
        }
    }
    Ok(())
}

/// `subtree` cascades down a tree, so it is spellable on a tree entity only.
/// On any other it compiled, deployed, and failed with `internal` when a
/// caller reached it (checklist 25b). The executor still refuses it, as a
/// second line; this is the first.
fn check_subtree_targets(schema: &Schema, stmts: &[WriteStmt]) -> Result<(), SemaError> {
    for stmt in stmts {
        let WriteStmt::Del {
            entity,
            subtree: true,
            span,
            ..
        } = stmt
        else {
            continue;
        };
        if schema.entity(&entity.name).is_some_and(|e| !e.is_tree) {
            return Err(SemaError {
                code: "subtree_not_tree",
                message: format!(
                    "`subtree` cascades down a tree, and `{0}` has no `tree` ref — delete a \
                     `{0}` row with `del {0} {{ id: $id }}`, which refuses while another row \
                     still points at it",
                    entity.name
                ),
                span: *span,
            });
        }
    }
    Ok(())
}

/// A `json` field takes a document, and a document lands in no other field
/// (28b). Decided at registration, so a `$t: text` written into a `json`
/// field is the author's compile error rather than a caller's refusal.
///
/// A document is a `json` parameter, a `json` field of the same row, or a
/// string literal holding JSON text; `null` where the field admits it.
/// Anything else in a `json` slot, or any of the first two in another
/// slot, is `param_type_mismatch`.
fn check_json_slot(
    name: &str,
    params: &[ordo_syntax::ParamDecl],
    e: &ordo_schema::Entity,
    f: &ordo_schema::Field,
    v: &Expr,
) -> Result<(), SemaError> {
    let json_valued = |x: &Expr| match x {
        Expr::Param { name: p, .. } => params
            .iter()
            .any(|d| d.name.name == *p && d.ty == ScalarTy::Json),
        Expr::Path { segments, .. } if segments.len() == 1 => e
            .field(&segments[0].name)
            .is_some_and(|g| g.ty.value_ty() == ScalarTy::Json),
        _ => false,
    };
    let into_json = f.ty.value_ty() == ScalarTy::Json;
    let ok = if into_json {
        match v {
            Expr::NullLit { .. } => f.optional,
            Expr::StrLit { value, .. } => serde_json::from_str::<serde_json::Value>(value).is_ok(),
            Expr::Param { name: p, .. } if p == "caller" => false,
            other => json_valued(other),
        }
    } else {
        !json_valued(v)
    };
    if ok {
        return Ok(());
    }
    Err(SemaError {
        code: "param_type_mismatch",
        message: format!(
            "`{name}` writes `{}.{}`, which is `{}`, and a `json` document and other values \
             never convert into each other. A `json` field takes a `json` parameter, a `json` \
             field, a string literal holding JSON, or `null` when it admits null",
            e.name,
            f.name,
            f.ty.value_ty().spelled()
        ),
        span: v.span(),
    })
}

pub fn check_writes(schema: &Schema, registry: &Registry) -> Result<(), SemaError> {
    let mut names: Vec<&String> = registry.names().collect();
    names.sort();
    for name in names {
        let Some(op) = registry.get(name) else {
            continue;
        };
        let OpBody::Write(stmts) = &op.body else {
            continue;
        };
        check_bindings(schema, stmts)?;
        check_platform_entities(schema, stmts)?;
        check_subtree_targets(schema, stmts)?;
        for stmt in stmts {
            check_computed_rhs(schema, name, stmt)?;
            check_allocate_site(name, stmt)?;
        }
        for stmt in stmts {
            let WriteStmt::Set {
                entity,
                cond,
                fields,
                span,
                ..
            } = stmt
            else {
                continue; // `del` carries no braces.
            };
            let Some(e) = schema.entity(&entity.name) else {
                continue;
            };
            // Typed slots, every mode: a create, a converge and a bulk
            // write all land values in the same declared columns.
            for (n, v) in fields {
                let Some(f) = e.field(&n.name) else { continue };
                // A literal written in major units must fit the field's
                // places (27ak): `price: 29.999` on a `money(2)` would have
                // to round, and that is decided here rather than on the
                // first call.
                if let Err(m) = ordo_schema::stored_numeric(f.ty.value_ty(), v) {
                    return Err(SemaError {
                        code: "implicit_rounding",
                        message: format!("`{name}` writes `{}.{}`: {m}", entity.name, f.name),
                        span: v.span(),
                    });
                }
                check_json_slot(name, &op.params, e, f, v)?;
                if !matches!(
                    f.ty.value_ty(),
                    ScalarTy::Money(_) | ScalarTy::Int | ScalarTy::Duration(_)
                ) {
                    continue;
                }
                let is_decimal = |x: &Expr| match x {
                    Expr::Path { segments, .. } if segments.len() == 1 => e
                        .field(&segments[0].name)
                        .is_some_and(|f| matches!(f.ty.value_ty(), ScalarTy::Decimal(_))),
                    Expr::Param { name: p, .. } => op
                        .params
                        .iter()
                        .any(|d| d.name.name == *p && matches!(d.ty, ScalarTy::Decimal(_))),
                    _ => false,
                };
                if let Some(sp) = unconverted_decimal(v, &is_decimal) {
                    return Err(SemaError {
                        code: "implicit_rounding",
                        message: format!(
                            "`{name}` writes a decimal into `{}.{}`, which is `{}` — `/`, `avg`, a \
                             `decimal` field and a decimal rate all produce `decimal`, and nothing \
                             rounds implicitly (DL §2). Say how it converts: `round(x, {})`, \
                             `floor(x)` or `ceil(x)`. If the parts must sum back to a total, that is \
                             `allocate`, not division.",
                            entity.name,
                            f.name,
                            f.ty.value_ty().spelled(),
                            f.ty.value_ty().places().unwrap_or(0),
                        ),
                        span: sp,
                    });
                }
            }

            match cond {
                // Creation-capable: no condition addresses the row, so
                // making the described state true may mint one.
                None => {
                    // Rule 8's precedence, honoured early: `id` in the
                    // braces addresses a row, so that statement converges
                    // and never creates.
                    if fields.iter().any(|(n, _)| n.name == "id") {
                        continue;
                    }
                    let missing: Vec<&str> = e
                        .fields
                        .iter()
                        .filter(|f| !f.implicit && !f.optional && f.default.is_none())
                        .filter(|f| !fields.iter().any(|(n, _)| n.name == f.name))
                        .map(|f| f.name.as_str())
                        .collect();
                    if !missing.is_empty() {
                        return Err(SemaError {
                            code: "insert_incomplete",
                            message: format!(
                                "`{name}` can create `{}` but supplies no value for {} — a creation-capable statement must cover every required field (DL §6 rule 2); give {} a value, a default, or `?`",
                                entity.name,
                                quoted(&missing),
                                if missing.len() == 1 { "it" } else { "them" },
                            ),
                            span: *span,
                        });
                    }
                }
                Some(c) if is_id_addressed(c) => {}
                // Ranged: one set of values over every matched row.
                Some(_) => {
                    for (n, _) in fields {
                        let Some(f) = e.field(&n.name) else { continue };
                        if f.implicit && f.name == "id" {
                            return Err(SemaError {
                                code: "bulk_identity_field",
                                message: format!(
                                    "`{name}` is a ranged write on `{}` and assigns `id` — addressed and bulk cannot blur (DL §6 rule 3); address the row instead: `set {}(id == …) {{ … }}`",
                                    entity.name, entity.name,
                                ),
                                span: n.span,
                            });
                        }
                        if f.unique && !f.implicit {
                            return Err(SemaError {
                                code: "bulk_identity_field",
                                message: format!(
                                    "`{name}` is a ranged write on `{}` and assigns the unique field `{}` — one value across a range makes every matched row collide on that key; unique fields may not appear in bulk braces (DL §6 rule 3)",
                                    entity.name, f.name,
                                ),
                                span: n.span,
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// `` `a`, `b` `` — the missing-field list, in declaration order.
fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// **Identity precedence** (§6 rule 8) for a `set` with no condition,
/// decided from the braces alone — statically, so the mode is a compiler
/// output a reviewer can read and never a runtime surprise.
///
/// The order is total and has no tie-break: `id` wins over every unique
/// key, one covered unique key addresses, and two or more is a compile
/// error rather than a guess about which key the author meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetMode {
    /// No identity in the braces — a new member is the only way to make
    /// the described state true.
    Create,
    /// `id` in the braces: id-addressing, always. Every other unique field
    /// present is an ordinary value, not an address.
    Addressed,
    /// Exactly one covered unique key: converge onto the row holding it,
    /// else create. `create_once` is the literal `rev: null` form, which
    /// can only create.
    ///
    /// `key` is a **column list**, because a unique key can be composite
    /// (`unique (board, member)` — the shape `acl` derives). One column is
    /// the ordinary case and reads as one; the addressing rules do not
    /// change with the arity, only the number of equalities that locate
    /// the row.
    Keyed { key: Vec<String>, create_once: bool },
    /// Two or more covered unique keys. The platform must not pick one.
    Ambiguous(Vec<String>),
}

impl SetMode {
    /// The word the compiler prints in the registration artifact and the
    /// deploy diff (§6 rule 1).
    ///
    /// `rev: null` prints `creates`, not `creates-or-converges`: the
    /// statement asserts the row's absence, so converging is not one of
    /// the things it can do, and the reviewer reads that off the label
    /// instead of re-deriving it from the braces. Note the label follows
    /// the STATIC form — `rev: $r` prints `creates-or-converges` because a
    /// binding that arrives null makes the create branch reachable, which
    /// is exactly what that label claims.
    pub fn label(&self) -> &'static str {
        match self {
            SetMode::Create => "creates",
            SetMode::Addressed => "converges",
            SetMode::Keyed {
                create_once: true, ..
            } => "creates",
            SetMode::Keyed { .. } => "creates-or-converges",
            // An ambiguous statement never compiles, so this label names
            // a program that cannot be deployed rather than a mode. It
            // exists so a catalog built from a broken source prints the
            // refusal instead of a plausible-looking mode.
            SetMode::Ambiguous(_) => "ambiguous-identity",
        }
    }
}

/// Classify an unconditioned `set` by rule 8. Unknown field names are left
/// to the binder to reject; they carry no identity either way.
pub fn set_mode(schema: &Schema, entity: &str, fields: &[(Ident, Expr)]) -> SetMode {
    if fields.iter().any(|(n, _)| n.name == "id") {
        return SetMode::Addressed;
    }
    let Some(e) = schema.entity(entity) else {
        return SetMode::Create;
    };
    let mut keys: Vec<Vec<String>> = fields
        .iter()
        .filter_map(|(n, _)| e.field(&n.name))
        // `id` is unique and implicit; it was handled above, and no other
        // implicit column is addressable by an author.
        .filter(|f| f.unique && !f.implicit)
        .map(|f| vec![f.name.clone()])
        .collect();
    // A **composite** unique key addresses when the braces cover EVERY one
    // of its columns. Partial coverage is not an address — `{ board: $b,
    // level: "read" }` names no row, and treating it as one would converge
    // onto whichever row happened to share the board.
    keys.extend(
        e.unique_keys
            .iter()
            .filter(|k| !k.is_empty() && k.iter().all(|c| fields.iter().any(|(n, _)| &n.name == c)))
            .cloned(),
    );
    match keys.len() {
        0 => SetMode::Create,
        1 => SetMode::Keyed {
            key: keys.remove(0),
            create_once: fields
                .iter()
                .any(|(n, v)| n.name == "rev" && matches!(v, Expr::NullLit { .. })),
        },
        // Named by their columns, so `board+member` reads as the one key
        // it is rather than as two.
        _ => SetMode::Ambiguous(keys.iter().map(|k| k.join("+")).collect()),
    }
}

/// **§6 rule 1's label for ONE statement** — the single authority every
/// renderer reads.
///
/// Rule 1 makes the label a per-STATEMENT fact and the op's label their
/// union, so this is the only function that turns a statement into a word:
/// `ordo_plan::artifact` prints it in the artifact, `ordo_host` puts the
/// union on the wire and the list in the catalog. A second reading of the
/// braces anywhere else is how the label and the behaviour drift, which is
/// the bug this exists to prevent — and it was live: a heuristic elsewhere
/// answered `converges` for every conditioned `set`, so a ranged write over
/// a thousand rows and an addressed write over one printed the same word.
///
/// Rule 1 names four labels for `set`, and all four are here: `creates`,
/// `converges`, `creates-or-converges` (rule 8's identity precedence, via
/// [`SetMode::label`]) and **`bulk`** for a condition that ranges. `del` is
/// the language's second verb and rule 1's list is written for `set`, so
/// its labels extend that vocabulary along the same distinction the rule
/// draws — `deletes` for one addressed row, `bulk deletes` for a range,
/// and `subtree` spelled out where the cascade reaches descendants the
/// caller never named.
pub fn stmt_label(schema: &Schema, stmt: &WriteStmt) -> &'static str {
    match stmt {
        WriteStmt::Set {
            entity,
            cond: None,
            fields,
            ..
        } => set_mode(schema, &entity.name, fields).label(),
        // The compile-time mirror of the executor's `addressed_id`, reused
        // rather than re-guessed: exactly the conditions the write path
        // treats as one row are the ones labelled `converges`.
        WriteStmt::Set { cond: Some(c), .. } => {
            if is_id_addressed(c) {
                "converges"
            } else {
                "bulk"
            }
        }
        WriteStmt::Del { cond, subtree, .. } => match (is_id_addressed(cond), subtree) {
            (true, false) => "deletes",
            (true, true) => "deletes subtree",
            (false, false) => "bulk deletes",
            (false, true) => "bulk deletes subtree",
        },
    }
}

/// The op-level label: rule 1's **union** of its statements', deduped in
/// statement order and joined with ` + `. A single word would have to lie
/// about at least one statement of a transfer that creates one row and
/// converges two others.
pub fn op_label(schema: &Schema, stmts: &[WriteStmt]) -> String {
    let mut labels: Vec<&str> = Vec::new();
    for s in stmts {
        let l = stmt_label(schema, s);
        if !labels.contains(&l) {
            labels.push(l);
        }
    }
    labels.join(" + ")
}
