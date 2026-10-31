//! Routines, checked against the shapes (DL §7, §8; checklist 12a).
//!
//! A routine is a write group the platform runs on a clock or on a change,
//! in every tenant, as `routine:<name>@<version>`. The parser has read its
//! trigger with §7's operand grammars. What is checked here is what needs
//! the entities: that the names it writes exist, that a change trigger can
//! fire at all, and that a change routine keeps to R-G20's row bound.
//!
//! | Class | Refused |
//! |---|---|
//! | `invalid_routine` | a duplicate or op-shadowing name, a domain that is not the program's, a wildcard or unknown entity in a change trigger, a parameter other than `$now: datetime` and `$today: date`, `$caller` in a body, an entity or field the body writes that does not exist, a name in an `each` emission that is not a field of the row |
//! | `routine_out_of_bound` | a change routine without `unguarded` writing a row it cannot reach: a `set` with no condition and no `id` creates or addresses by key a row the changed rows do not lead to |
//!
//! An `each` body's pipeline is bound against the program in `ordo_plan`
//! (`ordo_plan::each`), which needs the binder. What is checked here is what
//! the shapes alone can answer.

use std::collections::BTreeMap;

use ordo_syntax::ast::{Condition, Expr, Ident};
use ordo_syntax::decl::{
    MatchKind, OpBody, OpDecl, OpFlags, Pipeline, Program, ProjItem, RoutineDecl, RoutineTrigger,
    ScalarTy, Source, SourceArg, Step, WriteStmt,
};
use ordo_syntax::trigger::Pos;

use crate::{Schema, SchemaError};

/// The parameters a routine may declare, and the type each must have. The
/// runtime supplies both from the run's own instant; nothing else is in
/// scope for a body nobody calls.
pub const ROUTINE_PARAMS: [(&str, &str); 2] = [("now", "datetime"), ("today", "date")];

fn refuse(code: &'static str, span: ordo_syntax::Span, message: String) -> SchemaError {
    SchemaError {
        code,
        message,
        span,
    }
}

/// The op a routine's body runs as.
///
/// Registered nowhere a caller can reach: the runtime builds a registry of
/// its own with this one op in it for the length of a run, so a routine is
/// never callable through the door (DL §8, "routines are not registered
/// ops and are not callable"). `unguarded` and `nopause` are the routine's
/// to read and are not passed on; `unbounded` and `reproducible` mean what
/// they mean on an op and go through the same registration gate.
pub fn routine_op(r: &RoutineDecl) -> OpDecl {
    OpDecl {
        name: r.name.clone(),
        params: r.params.clone(),
        flags: OpFlags {
            unbounded: r.flags.unbounded,
            reproducible: r.flags.reproducible,
            unguarded: false,
            nopause: false,
            public: false,
            download: false,
        },
        body: OpBody::Write(r.body.clone()),
        span: r.span,
    }
}

/// The entity a change routine is woken by, when it has one.
pub fn trigger_entity(r: &RoutineDecl) -> Option<&str> {
    match &r.trigger {
        RoutineTrigger::Change { pattern, .. } => match &pattern.entity {
            Pos::Named(n) => Some(n.as_str()),
            Pos::Any => None,
        },
        RoutineTrigger::Clock(_) => None,
    }
}

/// Check every routine in `program` against the built `schema`.
pub fn check(schema: &Schema, program: &Program) -> Result<(), SchemaError> {
    let mut seen: Vec<&str> = Vec::new();
    for r in &program.routines {
        let name = r.name.name.as_str();
        if seen.contains(&name) {
            return Err(refuse(
                "invalid_routine",
                r.name.span,
                format!("routine `{name}` is declared twice"),
            ));
        }
        seen.push(name);
        if program.ops.iter().any(|o| o.name.name == name) {
            return Err(refuse(
                "invalid_routine",
                r.name.span,
                format!("`{name}` names both an op and a routine; give the routine its own name"),
            ));
        }
        if let (Some(d), Some(prog)) = (&r.domain, &schema.domain) {
            if &d.name != prog {
                return Err(refuse(
                    "invalid_routine",
                    d.span,
                    format!(
                        "routine `{name}` says `in {}`, and this program is domain `{prog}`",
                        d.name
                    ),
                ));
            }
        }
        check_trigger(schema, r)?;
        check_params(r)?;
        check_body(schema, r)?;
        check_notify(schema, r)?;
    }
    Ok(())
}

/// The most bytes a `notify` text may have. It is the subject of the mail.
pub const NOTIFY_TEXT_MAX: usize = 200;

/// Where a written row's `acl` lives, for `notify`.
///
/// `path` is the ref fields to follow from the written entity to `root`, in
/// order: empty when the entity declares the `acl` itself, `["note"]` for a
/// child that inherits it from `notes` through `note -> notes owned`.
/// `link` is the derived access entity (`notesAccess`) and `back` its ref
/// to the root (`note`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclHome {
    pub root: String,
    pub path: Vec<String>,
    pub link: String,
    pub back: String,
    pub levels: Vec<String>,
}

/// The row an entity's `acl` lives on: its own, or the ancestor it inherits
/// its guard from down an owned subtree. `None` when it has no acl.
pub fn acl_home(schema: &Schema, entity: &str) -> Option<AclHome> {
    let e = schema.entity(entity)?;
    let (root, path) = match &e.acl_levels {
        Some(_) => (entity.to_string(), Vec::new()),
        None => {
            let g = e.guards.as_ref()?;
            match &g.source {
                crate::GuardSource::Inherited { from } => (from.clone(), g.rebase.clone()),
                _ => return None,
            }
        }
    };
    let r = schema.entity(&root)?;
    let levels = r.acl_levels.clone()?;
    let link = format!("{root}Access");
    let back = schema
        .entity(&link)?
        .fields
        .iter()
        .find_map(|f| match &f.ty {
            crate::FieldTy::Ref { target, .. } if *target == root => Some(f.name.clone()),
            _ => None,
        })?;
    Some(AclHome {
        root,
        path,
        link,
        back,
        levels,
    })
}

/// `notify <level> "<text>"`: a short one-line text, and every entity the
/// body sets has an `acl` (its own or inherited) that declares `<level>`.
fn check_notify(schema: &Schema, r: &RoutineDecl) -> Result<(), SchemaError> {
    let Some(n) = &r.notify else {
        return Ok(());
    };
    let name = &r.name.name;
    let text = n.text.trim();
    if text.is_empty() || text.len() > NOTIFY_TEXT_MAX || text.chars().any(char::is_control) {
        return Err(refuse(
            "invalid_routine",
            n.span,
            format!(
                "routine `{name}`: the `notify` text is the subject of an email. Write one line \
                 of 1 to {NOTIFY_TEXT_MAX} bytes"
            ),
        ));
    }
    let mut sets = 0;
    for stmt in &r.body {
        let WriteStmt::Set { entity, .. } = stmt else {
            continue;
        };
        sets += 1;
        let Some(home) = acl_home(schema, &entity.name) else {
            return Err(refuse(
                "invalid_routine",
                n.span,
                format!(
                    "routine `{name}` notifies the holders of `{}` on the rows it writes, and \
                     `{}` has no `acl`, of its own or from an owner it inherits from. Give it \
                     one, or write rows that have one",
                    n.level.name, entity.name
                ),
            ));
        };
        if !home.levels.contains(&n.level.name) {
            return Err(refuse(
                "invalid_routine",
                n.level.span,
                format!(
                    "routine `{name}` notifies the holders of `{}`, and `{}`'s acl declares {}",
                    n.level.name,
                    home.root,
                    home.levels
                        .iter()
                        .map(|l| format!("`{l}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    if sets == 0 {
        return Err(refuse(
            "invalid_routine",
            n.span,
            format!(
                "routine `{name}` notifies the people on the rows it creates or changes, and it \
                 only deletes. A deleted row has nobody left to tell"
            ),
        ));
    }
    Ok(())
}

fn check_trigger(schema: &Schema, r: &RoutineDecl) -> Result<(), SchemaError> {
    let RoutineTrigger::Change { pattern, .. } = &r.trigger else {
        return Ok(());
    };
    let name = &r.name.name;
    if let (Pos::Named(d), None) = (&pattern.domain, &schema.domain) {
        return Err(refuse(
            "invalid_routine",
            r.trigger_span,
            format!(
                "routine `{name}` listens on domain `{d}`, and this program declares no \
                 domain, so it would never fire. Write `*` in the first position"
            ),
        ));
    }
    if let (Pos::Named(d), Some(prog)) = (&pattern.domain, &schema.domain) {
        if d != prog {
            // A pattern on another domain's name never matches a row of this
            // program: refused, so it cannot compile and silently never fire.
            return Err(refuse(
                "invalid_routine",
                r.trigger_span,
                format!(
                    "routine `{name}` listens on domain `{d}`, and this program is domain \
                     `{prog}`, so it would never fire"
                ),
            ));
        }
    }
    match &pattern.entity {
        Pos::Any => Err(refuse(
            "invalid_routine",
            r.trigger_span,
            format!(
                "routine `{name}`: name the entity the change lands on \
                 (`when <domain>.<entity>.<kind>`). A wildcard entity is not built yet"
            ),
        )),
        Pos::Named(e) if schema.entity(e).is_none() => Err(refuse(
            "invalid_routine",
            r.trigger_span,
            format!("routine `{name}` listens on `{e}`, which this program does not declare"),
        )),
        Pos::Named(_) => Ok(()),
    }
}

fn check_params(r: &RoutineDecl) -> Result<(), SchemaError> {
    for p in &r.params {
        let want = ROUTINE_PARAMS
            .iter()
            .find(|(n, _)| *n == p.name.name)
            .map(|(_, t)| *t);
        let ty_ok = matches!(
            (want, &p.ty),
            (Some("datetime"), ScalarTy::Datetime) | (Some("date"), ScalarTy::Date)
        );
        if !ty_ok || p.default.is_some() || p.max.is_some() || p.filled {
            return Err(refuse(
                "invalid_routine",
                p.span,
                format!(
                    "routine `{}`: a routine takes `$now: datetime` and `$today: date`, which \
                     the platform fills from the run's own instant, and nothing else (`${}`)",
                    r.name.name, p.name.name
                ),
            ));
        }
    }
    Ok(())
}

fn check_body(schema: &Schema, r: &RoutineDecl) -> Result<(), SchemaError> {
    let name = &r.name.name;
    let declared: Vec<&str> = r.params.iter().map(|p| p.name.name.as_str()).collect();
    let bounded = matches!(r.trigger, RoutineTrigger::Change { .. }) && !r.flags.unguarded;
    if let Some(pl) = &r.each {
        let mut params = Vec::new();
        pipeline_params(pl, &mut params);
        check_params_used(r, &declared, params)?;
    }
    for stmt in &r.body {
        let (entity, span) = match stmt {
            WriteStmt::Set { entity, span, .. } | WriteStmt::Del { entity, span, .. } => {
                (entity, *span)
            }
        };
        let Some(e) = schema.entity(&entity.name) else {
            return Err(refuse(
                "invalid_routine",
                entity.span,
                format!(
                    "routine `{name}` writes `{}`, which this program does not declare",
                    entity.name
                ),
            ));
        };
        let mut params = Vec::new();
        match stmt {
            WriteStmt::Set { cond, fields, .. } => {
                for (f, v) in fields {
                    if f.name != "id" && f.name != "rev" && e.field(&f.name).is_none() {
                        return Err(refuse(
                            "invalid_routine",
                            f.span,
                            format!(
                                "routine `{name}` sets `{}.{}`, which is not a field",
                                entity.name, f.name
                            ),
                        ));
                    }
                    expr_params(v, &mut params);
                }
                if let Some(c) = cond {
                    cond_params(c, &mut params);
                } else if bounded && !fields.iter().any(|(f, _)| f.name == "id") {
                    return Err(refuse(
                        "routine_out_of_bound",
                        span,
                        format!(
                            "routine `{name}` is woken by a change, so it may write only rows \
                             reachable from the changed rows, and `set {} {{ … }}` with no \
                             condition and no `id` creates or addresses by key a row they do \
                             not lead to. Write a condition (`set {}(…) {{ … }}`) or address \
                             the row by `id`, or write `unguarded` on the `run` header to range \
                             wider on purpose",
                            entity.name, entity.name
                        ),
                    ));
                }
            }
            WriteStmt::Del { cond, .. } => cond_params(cond, &mut params),
        }
        check_params_used(r, &declared, params)?;
    }
    Ok(())
}

/// Every `$name` a routine's body uses must be one its `run` header
/// declares, and none may be `$caller`.
fn check_params_used(r: &RoutineDecl, declared: &[&str], params: Found) -> Result<(), SchemaError> {
    let name = &r.name.name;
    for (p, sp) in params {
        if p == "caller" {
            return Err(refuse(
                "invalid_routine",
                sp,
                format!(
                    "routine `{name}` reads `$caller`, and a routine has no caller: it runs \
                     as `routine:{name}`"
                ),
            ));
        }
        if !declared.contains(&p.as_str()) {
            return Err(refuse(
                "invalid_routine",
                sp,
                format!(
                    "routine `{name}` uses `${p}`, which its `run` header does not declare. \
                     A routine takes `$now: datetime` and `$today: date`"
                ),
            ));
        }
    }
    Ok(())
}

type Found = Vec<(String, ordo_syntax::Span)>;

fn expr_params(e: &Expr, out: &mut Found) {
    match e {
        Expr::Param { name, span } => out.push((name.clone(), *span)),
        Expr::Neg { expr, .. } => expr_params(expr, out),
        Expr::Binary { lhs, rhs, .. } => {
            expr_params(lhs, out);
            expr_params(rhs, out);
        }
        Expr::FnCall { args, .. } => args.iter().for_each(|a| expr_params(a, out)),
        Expr::Cond { arms, else_, .. } => {
            for a in arms {
                cond_params(&a.condition, out);
                expr_params(&a.value, out);
            }
            expr_params(else_, out);
        }
        Expr::IntLit { .. }
        | Expr::DecimalLit { .. }
        | Expr::StrLit { .. }
        | Expr::DateLit { .. }
        | Expr::DatetimeLit { .. }
        | Expr::DurationLit { .. }
        | Expr::BoolLit { .. }
        | Expr::NullLit { .. }
        | Expr::Path { .. } => {}
    }
}

fn cond_params(c: &Condition, out: &mut Found) {
    match c {
        Condition::Compare { lhs, rhs, .. } | Condition::Text { lhs, rhs, .. } => {
            expr_params(lhs, out);
            expr_params(rhs, out);
        }
        Condition::In { expr, list, .. } => {
            expr_params(expr, out);
            list.iter().for_each(|x| expr_params(x, out));
        }
        Condition::IsNull { expr, .. } => expr_params(expr, out),
        Condition::Since { expr, dur, .. } => {
            expr_params(expr, out);
            expr_params(dur, out);
        }
        Condition::Under { expr, id, .. } | Condition::Above { expr, id, .. } => {
            expr_params(expr, out);
            expr_params(id, out);
        }
        Condition::BareBool(e) => expr_params(e, out),
        Condition::And { items, .. } | Condition::Or { items, .. } => {
            items.iter().for_each(|i| cond_params(i, out))
        }
        Condition::Not { inner, .. } => cond_params(inner, out),
    }
}

fn pipeline_params(pl: &Pipeline, out: &mut Found) {
    for l in &pl.lets {
        pipeline_params(&l.pipeline, out);
    }
    for s in &pl.sources {
        source_params(s, out);
    }
    for st in &pl.steps {
        match st {
            Step::Filter { cond, .. } => cond_params(cond, out),
            Step::Sort { keys, .. } => keys.iter().for_each(|k| expr_params(&k.expr, out)),
            Step::Take { keys, n, .. } => {
                keys.iter().for_each(|k| expr_params(k, out));
                if let ordo_syntax::decl::TakeCount::Param { name } = n {
                    out.push((name.name.clone(), name.span));
                }
            }
            Step::Group { keys, aggs, .. } => {
                keys.iter().for_each(|k| expr_params(k, out));
                for a in aggs {
                    a.arg.iter().for_each(|x| expr_params(x, out));
                    a.cond.iter().for_each(|c| cond_params(c, out));
                }
            }
            Step::Reduce { aggs, .. } => {
                for a in aggs {
                    a.arg.iter().for_each(|x| expr_params(x, out));
                    a.cond.iter().for_each(|c| cond_params(c, out));
                }
            }
            Step::Match { kind, .. } => match kind {
                MatchKind::Any(s) | MatchKind::None(s) => source_params(s, out),
                MatchKind::Ref { .. } | MatchKind::On { .. } => {}
            },
            Step::Append { source, .. } => source_params(source, out),
            Step::Distinct { .. } => {}
        }
    }
}

fn source_params(s: &Source, out: &mut Found) {
    for a in &s.args {
        match a {
            SourceArg::Named { value, .. } => expr_params(value, out),
            SourceArg::Cond(c) => cond_params(c, out),
        }
    }
    for item in s.projection.iter().flatten() {
        if let ProjItem::Named { value, .. } = item {
            expr_params(value, out);
        }
    }
}

/// The entities a routine's run reads or writes by name: the targets of
/// its writes, and every entity its `each` pipeline lists as a source. A
/// change run resolves the reachable rows of exactly these.
pub fn touched_entities(schema: &Schema, r: &RoutineDecl) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |n: &str| {
        if schema.entity(n).is_some() && !out.iter().any(|o| o == n) {
            out.push(n.to_string());
        }
    };
    for stmt in &r.body {
        match stmt {
            WriteStmt::Set { entity, .. } | WriteStmt::Del { entity, .. } => add(&entity.name),
        }
    }
    fn sources<'a>(pl: &'a Pipeline, into: &mut Vec<&'a str>) {
        for l in &pl.lets {
            sources(&l.pipeline, into);
        }
        for s in &pl.sources {
            into.push(&s.name.name);
        }
        for st in &pl.steps {
            match st {
                Step::Match {
                    kind: MatchKind::Any(s) | MatchKind::None(s),
                    ..
                }
                | Step::Append { source: s, .. } => into.push(&s.name.name),
                _ => {}
            }
        }
    }
    if let Some(pl) = &r.each {
        let mut names = Vec::new();
        sources(pl, &mut names);
        for n in names {
            add(n);
        }
    }
    out
}

// ── `each ->`: the row's names, and the emission written against them ────

/// The names an `each` emission may use, each mapped to the row field it
/// means (DL §8, "`each ->` scope").
///
/// The row is what the pipeline produced, and its fields are named as the
/// binder names its outputs: by the last segment of a projected path, or
/// by the name a projection item gives. A reader writes the field the way
/// the pipeline wrote it, so each output is reachable by the spellings the
/// pipeline used for it as well: `unit.name` for `[unit.name]`, and
/// `plants.id` for the `id` of the source `plants`.
///
/// After a `group` or a `reduce` the row is the bucket: its group keys and
/// aggregate names, written as the step wrote them, and nothing else.
///
/// A spelling that would name two different fields is left out, so it is
/// refused as not in scope rather than read as either.
pub fn row_scope(pl: &Pipeline, outputs: &[String]) -> BTreeMap<String, String> {
    let mut map: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut add = |spelling: String, field: &str| {
        if !outputs.iter().any(|o| o == field) {
            return;
        }
        map.entry(spelling)
            .and_modify(|v| {
                if v.as_deref() != Some(field) {
                    *v = None;
                }
            })
            .or_insert_with(|| Some(field.to_string()));
    };
    for o in outputs {
        add(o.clone(), o);
    }
    let grouped = pl
        .steps
        .iter()
        .any(|s| matches!(s, Step::Group { .. } | Step::Reduce { .. }));
    if grouped {
        for st in &pl.steps {
            if let Step::Group { keys, .. } = st {
                for k in keys {
                    if let Expr::Path { segments, .. } = k {
                        if let Some(last) = segments.last() {
                            add(dotted(segments), &last.name);
                        }
                    }
                }
            }
        }
    } else {
        for src in &pl.sources {
            let s = &src.name.name;
            match &src.projection {
                Some(items) if !items.iter().any(|i| matches!(i, ProjItem::Star { .. })) => {
                    for item in items {
                        match item {
                            ProjItem::Field { path, .. } => {
                                if let Some(last) = path.last() {
                                    add(dotted(path), &last.name);
                                    add(format!("{s}.{}", dotted(path)), &last.name);
                                }
                            }
                            ProjItem::Named { out, .. } => {
                                add(format!("{s}.{}", out.name), &out.name)
                            }
                            ProjItem::Star { .. } | ProjItem::Exclude { .. } => {}
                        }
                    }
                }
                _ => {
                    for o in outputs {
                        add(format!("{s}.{o}"), o);
                    }
                }
            }
        }
    }
    map.into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v)))
        .collect()
}

fn dotted(path: &[Ident]) -> String {
    path.iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// A name an emission used for a row field, and where.
pub struct RowName<'a> {
    pub spelled: String,
    pub path: &'a [Ident],
    pub span: ordo_syntax::Span,
}

/// Rewrite an `each` emission's row names with `resolve`.
///
/// The row's scope is the emission braces (DL §8) and the `{ id: … }` of an
/// addressed `del`, which the parser has already lowered to `id == …`: its
/// right-hand side is read from the row. A statement's condition otherwise
/// keeps the scope of the entity it writes, as every condition does (DL §4).
/// A `let` name bound by an earlier statement of the emission and dotted
/// (`x.id`) is that row descriptor and is left as written.
///
/// `resolve` answers a row name with the expression that replaces it, or
/// with why the name is not in scope.
pub fn rewrite_emission(
    r: &RoutineDecl,
    resolve: &mut dyn FnMut(RowName<'_>) -> Result<Expr, String>,
) -> Result<Vec<WriteStmt>, SchemaError> {
    let mut lets: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for stmt in &r.body {
        let mut stmt = stmt.clone();
        match &mut stmt {
            WriteStmt::Set {
                binding, fields, ..
            } => {
                for (_, v) in fields.iter_mut() {
                    rw_expr(v, &lets, resolve)?;
                }
                if let Some(b) = binding {
                    lets.push(b.name.clone());
                }
            }
            WriteStmt::Del { cond, .. } => {
                if let Some(rhs) = addressed_rhs(cond) {
                    rw_expr(rhs, &lets, resolve)?;
                }
            }
        }
        out.push(stmt);
    }
    Ok(out)
}

/// The `…` of `id == …`, the form `del e { id: … }` lowers to.
fn addressed_rhs(c: &mut Condition) -> Option<&mut Expr> {
    match c {
        Condition::Compare {
            op: ordo_syntax::ast::CmpOp::Eq,
            lhs: Expr::Path { segments, .. },
            rhs,
            ..
        } if segments.len() == 1 && segments[0].name == "id" => Some(rhs),
        _ => None,
    }
}

fn rw_expr(
    e: &mut Expr,
    lets: &[String],
    resolve: &mut dyn FnMut(RowName<'_>) -> Result<Expr, String>,
) -> Result<(), SchemaError> {
    match e {
        Expr::Path { segments, span } => {
            if segments.len() >= 2 && lets.contains(&segments[0].name) {
                return Ok(());
            }
            let span = *span;
            let got = resolve(RowName {
                spelled: dotted(segments),
                path: segments,
                span,
            })
            .map_err(|m| refuse("invalid_routine", span, m))?;
            *e = got;
        }
        Expr::Neg { expr, .. } => rw_expr(expr, lets, resolve)?,
        Expr::Binary { lhs, rhs, .. } => {
            rw_expr(lhs, lets, resolve)?;
            rw_expr(rhs, lets, resolve)?;
        }
        Expr::FnCall { args, .. } => {
            for a in args {
                rw_expr(a, lets, resolve)?;
            }
        }
        Expr::Cond { arms, else_, .. } => {
            for a in arms {
                rw_cond(&mut a.condition, lets, resolve)?;
                rw_expr(&mut a.value, lets, resolve)?;
            }
            rw_expr(else_, lets, resolve)?;
        }
        Expr::IntLit { .. }
        | Expr::DecimalLit { .. }
        | Expr::StrLit { .. }
        | Expr::DateLit { .. }
        | Expr::DatetimeLit { .. }
        | Expr::DurationLit { .. }
        | Expr::BoolLit { .. }
        | Expr::NullLit { .. }
        | Expr::Param { .. } => {}
    }
    Ok(())
}

fn rw_cond(
    c: &mut Condition,
    lets: &[String],
    resolve: &mut dyn FnMut(RowName<'_>) -> Result<Expr, String>,
) -> Result<(), SchemaError> {
    match c {
        Condition::Compare { lhs, rhs, .. } | Condition::Text { lhs, rhs, .. } => {
            rw_expr(lhs, lets, resolve)?;
            rw_expr(rhs, lets, resolve)?;
        }
        Condition::In { expr, list, .. } => {
            rw_expr(expr, lets, resolve)?;
            for x in list {
                rw_expr(x, lets, resolve)?;
            }
        }
        Condition::IsNull { expr, .. } => rw_expr(expr, lets, resolve)?,
        Condition::Since { expr, dur, .. } => {
            rw_expr(expr, lets, resolve)?;
            rw_expr(dur, lets, resolve)?;
        }
        Condition::Under { expr, id, .. } | Condition::Above { expr, id, .. } => {
            rw_expr(expr, lets, resolve)?;
            rw_expr(id, lets, resolve)?;
        }
        Condition::BareBool(e) => rw_expr(e, lets, resolve)?,
        Condition::And { items, .. } | Condition::Or { items, .. } => {
            for i in items {
                rw_cond(i, lets, resolve)?;
            }
        }
        Condition::Not { inner, .. } => rw_cond(inner, lets, resolve)?,
    }
    Ok(())
}

/// "`x` is not a field of the row …", naming the fields that are.
pub fn not_in_row(routine: &str, spelled: &str, outputs: &[String]) -> String {
    format!(
        "routine `{routine}`: `{spelled}` is not a field of the row the pipeline produces. \
         Inside `each ->` the names in scope are that row's fields ({}), and the run's own \
         `$` parameters",
        outputs.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use crate::{build_with_keys, KeyTable};

    const SHAPES: &str = "domain shop\nentity orders {\n  status: text\n}\n";

    fn check(routine: &str) -> Result<(), crate::SchemaError> {
        let program = ordo_syntax::parse_program(&format!("{SHAPES}{routine}")).expect("parses");
        build_with_keys(&program, &KeyTable::default()).map(|_| ())
    }

    #[test]
    fn a_routine_over_declared_shapes_checks() {
        check("routine r {\n  trigger every 5m\n  run:\n    set orders(status == \"a\") { status: \"b\" }\n}\n")
            .expect("checks");
    }

    #[test]
    fn a_change_routine_may_not_create_without_unguarded() {
        let e = check(
            "routine r {\n  trigger when shop.orders.created\n  run:\n    set orders { status: \"b\" }\n}\n",
        )
        .expect_err("out of bound");
        assert_eq!(e.code, "routine_out_of_bound");
        check(
            "routine r {\n  trigger when shop.orders.created\n  run unguarded:\n    set orders { status: \"b\" }\n}\n",
        )
        .expect("unguarded ranges wider on purpose");
    }

    /// Notes with an acl, and reminders that inherit it through an owned ref.
    const ACL: &str =
        "domain notes\nentity notes {\n  title: text\n  acl (read, write, admin)\n}\n\
        entity reminders {\n  note -> notes owned\n  due: datetime\n  fired: datetime?\n}\n\
        entity loose {\n  due: datetime\n  fired: datetime?\n}\n";

    fn check_acl(routine: &str) -> Result<crate::Schema, crate::SchemaError> {
        let program = ordo_syntax::parse_program(&format!("{ACL}{routine}")).expect("parses");
        build_with_keys(&program, &KeyTable::default())
    }

    const FIRE: &str =
        "run($now: datetime):\n    reminders(due <= $now and fired == null)[id]\n    \
        each ->\n      set reminders { id: id, fired: $now }\n}\n";

    #[test]
    fn notify_on_rows_that_inherit_an_acl_checks_and_finds_the_acl() {
        let schema = check_acl(&format!(
            "routine remind {{\n  trigger every 30s\n  notify admin \"A reminder is due\"\n  {FIRE}"
        ))
        .expect("checks");
        let home = super::acl_home(&schema, "reminders").expect("an acl home");
        assert_eq!(home.root, "notes");
        assert_eq!(home.path, vec!["note".to_string()]);
        assert_eq!(home.link, "notesAccess");
        assert_eq!(home.back, "note");
        assert_eq!(
            super::acl_home(&schema, "notes")
                .expect("its own")
                .path
                .len(),
            0
        );
        assert!(super::acl_home(&schema, "loose").is_none());
    }

    #[test]
    fn notify_is_refused_for_an_undeclared_level_rows_with_no_acl_and_a_bad_text() {
        let level = check_acl(&format!(
            "routine remind {{\n  trigger every 30s\n  notify owner \"Due\"\n  {FIRE}"
        ))
        .expect_err("no level `owner`");
        assert_eq!(level.code, "invalid_routine");
        assert!(
            level.message.contains("`read`, `write`, `admin`"),
            "{}",
            level.message
        );
        let loose = check_acl(
            "routine remind {\n  trigger every 30s\n  notify admin \"Due\"\n  run($now: datetime):\n    \
             set loose(due <= $now and fired == null) { fired: $now }\n}\n",
        )
        .expect_err("no acl");
        assert!(loose.message.contains("has no `acl`"), "{}", loose.message);
        let long = "x".repeat(super::NOTIFY_TEXT_MAX + 1);
        for text in ["", "   ", long.as_str()] {
            let e = check_acl(&format!(
                "routine remind {{\n  trigger every 30s\n  notify admin \"{text}\"\n  {FIRE}"
            ))
            .expect_err("a bad text");
            assert!(e.message.contains("subject of an email"), "{}", e.message);
        }
        let deletes = check_acl(
            "routine remind {\n  trigger every 30s\n  notify admin \"Due\"\n  run:\n    \
             del reminders(fired != null)\n}\n",
        )
        .expect_err("a delete tells nobody");
        assert!(
            deletes.message.contains("only deletes"),
            "{}",
            deletes.message
        );
    }

    #[test]
    fn a_trigger_that_could_never_fire_is_refused() {
        for bad in [
            "routine r {\n  trigger when other.orders.created\n  run:\n    set orders(true) { status: \"b\" }\n}\n",
            "routine r {\n  trigger when shop.*.created\n  run:\n    set orders(true) { status: \"b\" }\n}\n",
            "routine r {\n  trigger when shop.nothing.created\n  run:\n    set orders(true) { status: \"b\" }\n}\n",
        ] {
            assert_eq!(check(bad).expect_err(bad).code, "invalid_routine", "{bad}");
        }
    }

    #[test]
    fn a_routine_takes_only_now_and_today() {
        let e = check(
            "routine r {\n  trigger every 5m\n  run($x: int):\n    set orders(true) { status: \"b\" }\n}\n",
        )
        .expect_err("a parameter nobody supplies");
        assert_eq!(e.code, "invalid_routine");
        let e = check(
            "routine r {\n  trigger every 5m\n  run:\n    set orders(true) { status: $caller }\n}\n",
        )
        .expect_err("no caller");
        assert_eq!(e.code, "invalid_routine");
    }
}
