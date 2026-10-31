//! Row invariants — DL §3, "Row invariants: `check` [settled]" — lowered to
//! real Postgres CHECK constraints, **row-local half only** (ruling
//! 2026-09-05, `corpus-rulings.md`).
//!
//! # The scope, and why it is a refusal rather than an approximation
//!
//! DL settles three forms. This module builds one and refuses two **by
//! name**:
//!
//! - **Row-local** (`check used <= seats`) — built. A condition over the
//!   row's own columns lowers to a `CHECK` constraint the database enforces
//!   inside every write's transaction; a violation aborts the whole atomic
//!   group, which is DL's seat-never-oversells argument verbatim.
//! - **Ref-walking** (`check assignee.org == customer.org`) — refused,
//!   `check_beyond_row`. Postgres CHECK admits no subqueries, so this form
//!   is trigger work on the declaring write *and* on writes to every entity
//!   the walk passes through. The trigger substrate has only just landed;
//!   wiring an invariant that guards only one of its own edges would be
//!   worse than refusing, because DL itself says an invariant that only
//!   guards one edge is not an invariant.
//! - **Time-dependent** (`check due >= today`) — refused,
//!   `check_time_dependent`. A CHECK expression must be IMMUTABLE; a
//!   time-dependent check would pass at write and silently become false as
//!   the clock moves, which is a lie wearing an invariant's name.
//!
//! # Two-valued logic is the same obligation here as in every query
//!
//! The rendered SQL uses the planner's exact shapes (see
//! `ordo_plan::Planner::cond`): equality is the distinctness family
//! (`IS [NOT] DISTINCT FROM`, total, never UNKNOWN), ordering comparisons
//! and `in` are `COALESCE(…, false)`. This matters doubly for a CHECK,
//! because Postgres treats a NULL check result as *pass* — under naive
//! rendering, `check used <= seats` with a null `used` would be silently
//! unenforced for exactly the rows least likely to be looked at. Under the
//! language's table, an ordering predicate against null is **false**, so
//! the write refuses, in the constraint just as it would in a `where`.
//!
//! # The constraint name is the diagnostic
//!
//! A violation surfaces from Postgres as SQLSTATE 23514 plus a constraint
//! name — the name is the only channel wide enough to say *which* check
//! failed, so it carries the condition: `teams$chk0$used<=seats`. The
//! `{entity}$chk{i}` prefix is the stable identity (renaming-safe, unique
//! per entity); the suffix is the condensed source, truncated to fit
//! Postgres's 63-byte identifier limit.

use ordo_syntax::ast::{CmpOp, Condition, Expr, TextOp};
use ordo_syntax::decl::CheckDecl;

use crate::ddl::q;
use crate::{Field, FieldTy, SchemaError};

/// One compiled row invariant, carried on the [`crate::Entity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRule {
    /// The constraint name the database will report on violation.
    pub name: String,
    /// The condition as the author wrote it (condensed), for manifests and
    /// diagnostics.
    pub source: String,
    /// The rendered CHECK expression — planner-shaped SQL over this row's
    /// own columns.
    pub sql: String,
}

/// Validate and lower an entity's `check` clauses.
///
/// `fields` are the declaring entity's own fields (implicit `id`/`rev` are
/// not checkable — they are server-minted and a rule about them is a rule
/// about mechanics, not data).
pub fn build_checks(
    entity: &str,
    fields: &[Field],
    decls: &[CheckDecl],
) -> Result<Vec<CheckRule>, SchemaError> {
    decls
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let sql = render_cond(entity, fields, &d.cond)?;
            let source = condense(&d.cond);
            Ok(CheckRule {
                name: constraint_name(entity, i, &source),
                source,
                sql,
            })
        })
        .collect()
}

/// The row invariants a `text(n)` field implies: one per bounded **open**
/// text field, `char_length(col) <= n`, after the authored checks.
///
/// A length bound is a row-local invariant, so it rides the same machinery
/// as `check`: the DDL renders it, the schema diff adds and drops it, the
/// apply path adds it `NOT VALID` and then validates, and the artifact's
/// `checks` list shows it to a reviewer. The name carries the number
/// (`notes$len$title<=64`), so a changed bound reaches the diff as a drop
/// and an add rather than as a silent redefinition, exactly as a changed
/// `check` condition does.
///
/// `char_length` counts characters, which is what the door and the write
/// path count. A **sealed** field gets no rule: its column holds
/// ciphertext, whose length says nothing about the value's, so the write
/// path's refusal is the only enforcer there. A null passes, because `?`
/// admits null and the bound is about length.
pub fn text_bound_checks(entity: &str, fields: &[Field]) -> Vec<CheckRule> {
    fields
        .iter()
        .filter(|f| !f.sealed && matches!(f.ty, FieldTy::Scalar(ordo_syntax::ScalarTy::Text)))
        .filter_map(|f| {
            let n = f.text_max?;
            let col = q(&f.column());
            let mut name = format!("{entity}$len${}<={n}", f.name);
            if name.len() > 63 {
                let mut cut = 63;
                while !name.is_char_boundary(cut) {
                    cut -= 1;
                }
                name.truncate(cut);
            }
            Some(CheckRule {
                name,
                source: format!("length({}) <= {n}", f.name),
                sql: format!("({col} IS NULL OR char_length({col}) <= {n})"),
            })
        })
        .collect()
}

/// `{entity}$chk{i}$<condensed source>`, truncated to Postgres's 63-byte
/// identifier limit on a UTF-8 boundary. The prefix alone is unique, so
/// truncation can never collide two checks.
fn constraint_name(entity: &str, i: usize, source: &str) -> String {
    let mut name = format!("{entity}$chk{i}${source}");
    if name.len() > 63 {
        let mut cut = 63;
        while !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name.truncate(cut);
    }
    name
}

fn err(code: &'static str, span: ordo_syntax::Span, message: String) -> SchemaError {
    SchemaError {
        code,
        message,
        span,
    }
}

fn render_cond(entity: &str, fields: &[Field], c: &Condition) -> Result<String, SchemaError> {
    Ok(match c {
        Condition::Compare { op, lhs, rhs, span } => {
            let l = render_expr(entity, fields, lhs)?;
            let r = render_expr(entity, fields, rhs)?;
            match op {
                // The distinctness family — total, never UNKNOWN — is the
                // language's `==`/`!=` table (`null == null` true,
                // `null != 5` true), and a CHECK passing on UNKNOWN makes
                // totality load-bearing twice over.
                CmpOp::Eq => format!("({l} IS NOT DISTINCT FROM {r})"),
                CmpOp::Ne => format!("({l} IS DISTINCT FROM {r})"),
                _ => {
                    let sym = match op {
                        CmpOp::Lt => "<",
                        CmpOp::Le => "<=",
                        CmpOp::Gt => ">",
                        CmpOp::Ge => ">=",
                        CmpOp::Eq | CmpOp::Ne => unreachable!(),
                    };
                    let _ = span;
                    format!("COALESCE({l} {sym} {r}, false)")
                }
            }
        }
        Condition::In { expr, list, span } => {
            let e = render_expr(entity, fields, expr)?;
            let items: Result<Vec<String>, SchemaError> = list
                .iter()
                .map(|i| render_expr(entity, fields, i))
                .collect();
            let _ = span;
            format!("COALESCE({e} IN ({}), false)", items?.join(", "))
        }
        Condition::Text { op, lhs, rhs, .. } => {
            // The planner's shapes exactly: the right side is a VALUE, not
            // a pattern, so `contains` is strpos and never LIKE. All three
            // functions are IMMUTABLE, which a CHECK requires.
            let l = render_expr(entity, fields, lhs)?;
            let r = render_expr(entity, fields, rhs)?;
            match op {
                TextOp::Contains => format!("(strpos({l}, {r}) > 0)"),
                TextOp::Starts => format!("(left({l}, length({r})) = {r})"),
                TextOp::Matches => format!("({l} ~ {r})"),
            }
        }
        Condition::IsNull { expr, negated, .. } => format!(
            "({} IS {}NULL)",
            // `doc is not null` asks whether a `json` field holds a
            // document, which is the one question a document answers (28b).
            match json_column(fields, expr) {
                Some(col) => col,
                None => render_expr(entity, fields, expr)?,
            },
            if *negated { "NOT " } else { "" }
        ),
        Condition::And { items, .. } => {
            let parts: Result<Vec<String>, SchemaError> = items
                .iter()
                .map(|i| render_cond(entity, fields, i))
                .collect();
            format!("({})", parts?.join(" AND "))
        }
        Condition::Or { items, .. } => {
            let parts: Result<Vec<String>, SchemaError> = items
                .iter()
                .map(|i| render_cond(entity, fields, i))
                .collect();
            format!("({})", parts?.join(" OR "))
        }
        Condition::Not { inner, .. } => {
            format!("(NOT {})", render_cond(entity, fields, inner)?)
        }
        Condition::BareBool(e) => format!("({} IS TRUE)", render_expr(entity, fields, e)?),
        // `since` is execution-time by definition — the clock is an
        // operand. Same refusal as `today`, stated on the connective.
        Condition::Since { span, .. } => {
            return Err(err(
                "check_time_dependent",
                *span,
                format!(
                    "`check` on `{entity}` uses `since` — a CHECK constraint must be \
                     immutable, and a window anchored to execution time would pass at \
                     write and silently become false later; time-dependent rules stay \
                     rule-tier"
                ),
            ))
        }
        // `under`/`above` test position against OTHER rows of the tree —
        // beyond this row by construction.
        Condition::Under { span, .. } | Condition::Above { span, .. } => {
            return Err(err(
                "check_beyond_row",
                *span,
                format!(
                    "`check` on `{entity}` tests tree position — that is a fact about \
                     other rows, and a CHECK constraint cannot see them; cross-row \
                     invariants await the trigger-backed form"
                ),
            ))
        }
    })
}

/// The column of an open `json` field a bare path names, if it names one.
fn json_column(fields: &[Field], e: &Expr) -> Option<String> {
    let Expr::Path { segments, .. } = e else {
        return None;
    };
    let [seg] = segments.as_slice() else {
        return None;
    };
    fields
        .iter()
        .find(|f| {
            f.name == seg.name && !f.sealed && f.ty == FieldTy::Scalar(ordo_syntax::ScalarTy::Json)
        })
        .map(|f| q(&f.column()))
}

fn render_expr(entity: &str, fields: &[Field], e: &Expr) -> Result<String, SchemaError> {
    Ok(match e {
        Expr::Path { segments, span } => {
            if segments.len() > 1 {
                // DL settles the ref-walking form; this build scopes it out
                // (ruling 2026-09-05): CHECK admits no subqueries, so the
                // walk is trigger work — on the declaring write AND on
                // writes to every entity the walk passes through, which is
                // exactly the machinery that does not exist yet.
                return Err(err(
                    "check_beyond_row",
                    *span,
                    format!(
                        "`check` on `{entity}` walks `{}` — a CHECK constraint sees one \
                         row and no other table; the ref-walking form is owed to the \
                         trigger substrate, not approximated here",
                        segments
                            .iter()
                            .map(|s| s.name.as_str())
                            .collect::<Vec<_>>()
                            .join(".")
                    ),
                ));
            }
            let name = &segments[0].name;
            let Some(f) = fields.iter().find(|f| &f.name == name) else {
                // `today`/`now` parse as bare paths; when no field shadows
                // them, the author meant the clock — name the real problem
                // rather than "unknown field".
                if name == "today" || name == "now" {
                    return Err(err(
                        "check_time_dependent",
                        *span,
                        format!(
                            "`check` on `{entity}` reads `{name}` — a CHECK constraint \
                             must be immutable, and a rule that consults the clock would \
                             pass at write and silently become false later"
                        ),
                    ));
                }
                // Not `unknown_field` — ruling 10f made that class
                // internal, and this one is authored and case-visible: the
                // author misspelled a field in their own declaration.
                return Err(err(
                    "check_unknown_field",
                    *span,
                    format!("`check` on `{entity}` names `{name}`, which is not a field"),
                ));
            };
            if f.sealed {
                // The column stores ciphertext; the database cannot
                // evaluate a predicate over a value it cannot see. A blind
                // index answers equality-to-a-known-digest only, and a
                // constraint expression cannot compute the keyed digest.
                return Err(err(
                    "check_sealed_field",
                    *span,
                    format!(
                        "`check` on `{entity}` reads sealed field `{name}` — the \
                         database holds ciphertext and cannot evaluate a predicate \
                         over it"
                    ),
                ));
            }
            // A document is read and written whole (28b); only `is null`
            // may name one, and that arm never reaches here.
            if f.ty == FieldTy::Scalar(ordo_syntax::ScalarTy::Json) {
                return Err(err(
                    "json_opaque",
                    *span,
                    format!(
                        "`check` on `{entity}` reads `json` field `{name}`: a document is \
                         read and written whole, so a rule may ask only `{name} is null`"
                    ),
                ));
            }
            if let FieldTy::Ref { .. } = f.ty {
                // The ref's own column is this row's value (the target's
                // id) — comparing it is row-local. Walking it is not, and
                // that case is the multi-segment path above.
            }
            // A `money(2)` column holds cents and the rule is written in
            // dollars (27ak): `check price <= 100.00` reads the column in
            // major units, which is exact — a multiplication by a power of
            // ten rounds nothing. At places 0 the column is read as it is.
            if let FieldTy::Scalar(ordo_syntax::ScalarTy::Money(p)) = f.ty {
                if p > 0 {
                    return Ok(format!(
                        "({} * {})",
                        q(&f.column()),
                        ordo_syntax::numeric::major_spelling(1, p)
                    ));
                }
            }
            q(&f.column())
        }
        Expr::IntLit { value, .. } => value.to_string(),
        Expr::StrLit { value, .. } => format!("'{}'", value.replace('\'', "''")),
        Expr::BoolLit { value, .. } => value.to_string(),
        Expr::NullLit { .. } => "NULL".into(),
        Expr::DateLit { y, m, d, .. } => format!("DATE '{y:04}-{m:02}-{d:02}'"),
        Expr::DatetimeLit {
            y,
            m,
            d,
            hh,
            mm,
            ss,
            ..
        } => format!("TIMESTAMP '{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}'"),
        Expr::Neg { expr, .. } => format!("(-{})", render_expr(entity, fields, expr)?),
        Expr::Binary { op, lhs, rhs, span } => {
            use ordo_syntax::ast::BinOp;
            if matches!(op, BinOp::Div) {
                // Division produces `decimal` at scale 12 (DL §2), and a
                // decimal landing in an integral comparison is the implicit
                // rounding DL forbids everywhere else. Refused rather than
                // approximated — write the rule over scaled integers.
                return Err(err(
                    "check_unsupported",
                    *span,
                    format!(
                        "`check` on `{entity}` divides — division produces `decimal`, \
                         and no implicit rounding happens anywhere in this language; \
                         state the rule over scaled integers instead \
                         (`used * 100 <= seats * threshold`)"
                    ),
                ));
            }
            let l = render_expr(entity, fields, lhs)?;
            let r = render_expr(entity, fields, rhs)?;
            let sym = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => unreachable!(),
            };
            format!("({l} {sym} {r})")
        }
        // An exact decimal, compared exactly. Money columns are read in
        // major units above, so `check price <= 29.99` means dollars on a
        // `money(2)` (27ak). This used to be refused ("write 2999, not
        // 29.99") while money had no places to convert by.
        Expr::DecimalLit { digits, .. } => digits.clone(),
        Expr::DurationLit { span, .. } => {
            return Err(err(
                "check_unsupported",
                *span,
                format!(
                    "`check` on `{entity}` uses a duration literal — duration \
                     arithmetic carries unit classes this row-local slice does not \
                     type; state the rule in the stored unit's integers"
                ),
            ))
        }
        Expr::Param { name, span } => {
            return Err(err(
                "check_unsupported",
                *span,
                format!(
                    "`check` on `{entity}` reads `${name}` — an invariant is a fact \
                     about the row, and there is no caller at constraint time"
                ),
            ))
        }
        Expr::FnCall { name, span, .. } => {
            return Err(err(
                "check_unsupported",
                *span,
                format!(
                    "`check` on `{entity}` calls `{}()` — expression functions are not \
                     in the row-local check slice yet; state the rule over the columns \
                     directly",
                    name.name
                ),
            ))
        }
        Expr::Cond { span, .. } => {
            return Err(err(
                "check_unsupported",
                *span,
                format!(
                    "`check` on `{entity}` uses `cond(…)` — write the rule as `and`/`or` \
                     of plain comparisons; a check is a predicate, not a computation"
                ),
            ))
        }
    })
}

/// The condition as one condensed line — the author's spelling with
/// whitespace collapsed, for constraint names and manifests.
fn condense(c: &Condition) -> String {
    fn expr(e: &Expr) -> String {
        match e {
            Expr::Path { segments, .. } => segments
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
                .join("."),
            Expr::IntLit { value, .. } => value.to_string(),
            Expr::DecimalLit { digits, .. } => digits.clone(),
            Expr::StrLit { value, .. } => format!("\"{value}\""),
            Expr::BoolLit { value, .. } => value.to_string(),
            Expr::NullLit { .. } => "null".into(),
            Expr::DateLit { y, m, d, .. } => format!("{y:04}-{m:02}-{d:02}"),
            Expr::DatetimeLit {
                y, m, d, hh, mm, ..
            } => format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}"),
            Expr::DurationLit { n, unit, .. } => format!("{n}{}", unit.as_str()),
            Expr::Param { name, .. } => format!("${name}"),
            Expr::Neg { expr: inner, .. } => format!("-{}", expr(inner)),
            Expr::Binary { op, lhs, rhs, .. } => {
                format!("{}{}{}", expr(lhs), op.as_str(), expr(rhs))
            }
            Expr::FnCall { name, args, .. } => format!(
                "{}({})",
                name.name,
                args.iter().map(expr).collect::<Vec<_>>().join(",")
            ),
            Expr::Cond { .. } => "cond(…)".into(),
        }
    }
    match c {
        Condition::Compare { op, lhs, rhs, .. } => {
            format!("{}{}{}", expr(lhs), op.as_str(), expr(rhs))
        }
        Condition::In { expr: e, list, .. } => format!(
            "{} in ({})",
            expr(e),
            list.iter().map(expr).collect::<Vec<_>>().join(",")
        ),
        Condition::Text { op, lhs, rhs, .. } => {
            format!("{} {} {}", expr(lhs), op.as_str(), expr(rhs))
        }
        Condition::IsNull {
            expr: e, negated, ..
        } => format!("{} is {}null", expr(e), if *negated { "not " } else { "" }),
        Condition::Since { expr: e, .. } => format!("{} since …", expr(e)),
        Condition::Under { expr: e, .. } => format!("{} under …", expr(e)),
        Condition::Above { expr: e, .. } => format!("{} above …", expr(e)),
        Condition::BareBool(e) => expr(e),
        Condition::And { items, .. } => {
            items.iter().map(condense).collect::<Vec<_>>().join(" and ")
        }
        Condition::Or { items, .. } => items.iter().map(condense).collect::<Vec<_>>().join(" or "),
        Condition::Not { inner, .. } => format!("not ({})", condense(inner)),
    }
}
