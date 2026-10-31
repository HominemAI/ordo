//! A read call, with the door's refinement, built as a pipeline.
//!
//! Ruled 2026-10-05 (`refinement-default` ask 1): **conditions on open and
//! blind-index fields compile into the plan**, so pages come back full. The
//! language already has the shape, written composition:
//! `name($p: v, cond, cond)[projection]` (`definition-language.md`, call-form
//! refinement). The binder inlines the op, and when the op's rows are
//! settled by a `take`, a `group` or `distinct` it wraps it, so the
//! conditions apply after everything the op does (`ordo-sema`'s
//! `composed_source`). A refinement is that call with its conditions taken
//! from the envelope.
//!
//! **Built as an AST, never as text.** The call used to be rendered as
//! source and re-parsed, which strips `"` from a text value instead of
//! escaping it (`plan-refinement.md` section 4). Here every value becomes a
//! literal node by its declared type, so nothing a caller sends is ever
//! spelled as source.
//!
//! The door has already checked every value against its field's type
//! (`ordo_api::refine::validate`), so a value that does not fit here is a
//! door bug and answers `internal`.

use ordo_api::{
    BoundParams, ExecError, FieldTy, OpSpec, ParamTy, RefineCond, RefineOp, Refinement,
};
use ordo_plan::artifact::RowKeyCond;
use ordo_syntax::{
    AggDecl, CmpOp, Condition, Expr, Ident, Pipeline, ProjItem, Source, SourceArg, Span, Step,
};
use serde_json::Value;

/// The anchor id the read path attaches (`ordo_plan::artifact::ROW_KEY`).
/// It rides every row that has one whatever `pick` says, so it is never a
/// projection item: the op's own body cannot name it. For the same reason a
/// condition on it is not a call condition; it travels beside the call as a
/// [`RowKeyCond`] and is applied to the bound query.
const ROW_KEY: &str = ordo_plan::artifact::ROW_KEY;

fn span() -> Span {
    Span::new(0, 0, 1, 1)
}

fn ident(name: &str) -> Ident {
    Ident {
        name: name.to_string(),
        span: span(),
    }
}

fn path(name: &str) -> Expr {
    Expr::Path {
        segments: vec![ident(name)],
        span: span(),
    }
}

fn bad(what: impl std::fmt::Display) -> ExecError {
    ExecError::internal(format!(
        "refinement: the door passed a value it should have refused: {what}"
    ))
}

/// A call ready to bind, and what the host still owes the rows.
pub(crate) struct RefinedCall {
    pub pipeline: Pipeline,
    /// A `pick` that named only `$id`: the projection cannot say that, so
    /// the rows are trimmed to it after the read.
    pub keep_only: Option<Vec<String>>,
    /// The refinement's conditions on `$id`, applied to the bound query
    /// (`ordo_plan::artifact::filter_row_key`). C4e (2026-09-17): *"`$id`
    /// is the attached name, and it is refinable."*
    pub row_key: Vec<RowKeyCond>,
}

/// The call for `op` with its bound parameters, and the refinement if one
/// came.
///
/// `count_of` is set for `std.E.count` under a refinement with conditions:
/// the name of `std.E.list`. Its conditions were validated against the
/// list's manifest (`std-library.md` §2.1), so they apply to the list, and
/// the count is taken over what is left: `std.E.list(conds) reduce { n:
/// count }`. Same entity, same guards, and the two answer one set.
pub(crate) fn build(
    op: &OpSpec,
    bound: &BoundParams,
    refine: Option<&Refinement>,
    count_of: Option<&str>,
) -> Result<RefinedCall, ExecError> {
    let row_key: Vec<RowKeyCond> = refine
        .map(|r| {
            r.where_
                .iter()
                .filter(|c| c.field == ROW_KEY)
                .map(row_key_condition)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let conds: Vec<SourceArg> = refine
        .map(|r| {
            r.where_
                .iter()
                .filter(|c| c.field != ROW_KEY)
                .map(|c| condition(op, c))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .map(SourceArg::Cond)
        .collect();

    if let (Some(list), false) = (count_of, conds.is_empty() && row_key.is_empty()) {
        return Ok(RefinedCall {
            pipeline: Pipeline {
                lets: Vec::new(),
                sources: vec![Source {
                    name: ident(list),
                    args: conds,
                    projection: None,
                    span: span(),
                }],
                steps: vec![Step::Reduce {
                    aggs: vec![AggDecl {
                        out: ident("n"),
                        func: ident("count"),
                        arg: None,
                        arg2: None,
                        cond: None,
                        span: span(),
                    }],
                    span: span(),
                }],
                span: span(),
            },
            keep_only: None,
            row_key,
        });
    }

    let mut args = params(op, bound)?;
    args.extend(conds);

    let (projection, keep_only) = match refine.and_then(|r| r.pick.as_ref()) {
        None => (None, None),
        Some(pick) => {
            let named: Vec<ProjItem> = pick
                .iter()
                .filter(|f| f.as_str() != ROW_KEY)
                .map(|f| ProjItem::Field {
                    path: vec![ident(f)],
                    span: span(),
                })
                .collect();
            if named.is_empty() {
                (None, Some(vec![ROW_KEY.to_string()]))
            } else {
                (Some(named), None)
            }
        }
    };

    Ok(RefinedCall {
        pipeline: Pipeline {
            lets: Vec::new(),
            sources: vec![Source {
                name: ident(&op.name),
                args,
                projection,
                span: span(),
            }],
            steps: Vec::new(),
            span: span(),
        },
        keep_only,
        row_key,
    })
}

/// The bound parameters as named arguments, each a literal of its declared
/// type.
fn params(op: &OpSpec, bound: &BoundParams) -> Result<Vec<SourceArg>, ExecError> {
    let mut args = Vec::new();
    for p in &op.params {
        let Some(v) = bound.0.get(&p.name) else {
            continue;
        };
        let value = match (&p.ty, v) {
            // Money and decimal arrive as the wire's decimal string in major
            // units; the binder converts it at the parameter's places (27ak).
            (ParamTy::Int | ParamTy::Money { .. } | ParamTy::Decimal { .. }, v) => {
                let digits = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                numeric(&digits).ok_or_else(|| bad(format!("parameter `{}`", p.name)))?
            }
            (ParamTy::Bool, Value::Bool(b)) => Expr::BoolLit {
                value: *b,
                span: span(),
            },
            (ParamTy::Date, Value::String(s)) => {
                date(s).ok_or_else(|| bad(format!("parameter `{}`", p.name)))?
            }
            // Text, ids, uris, datetimes and durations ride as a string
            // literal, as they did in source, and the binder fits each to its
            // slot. Unlike the source path, the string is the caller's own:
            // a `"` in it stays a `"`.
            (_, Value::String(s)) => Expr::StrLit {
                value: s.clone(),
                span: span(),
            },
            (_, other) => {
                numeric(&other.to_string()).ok_or_else(|| bad(format!("parameter `{}`", p.name)))?
            }
        };
        args.push(SourceArg::Named {
            name: ident(&p.name),
            value,
            span: span(),
        });
    }
    Ok(args)
}

/// A condition on `$id` as the bound query takes it. The door checked each
/// value is a string (`FieldTy::Id`).
fn row_key_condition(c: &RefineCond) -> Result<RowKeyCond, ExecError> {
    let id = |v: &Value| v.as_str().map(str::to_string).ok_or_else(|| bad(ROW_KEY));
    let cmp = |op: CmpOp| Ok(RowKeyCond::Cmp(op, id(&c.value)?));
    match c.op {
        RefineOp::Eq => cmp(CmpOp::Eq),
        RefineOp::Ne => cmp(CmpOp::Ne),
        RefineOp::Lt => cmp(CmpOp::Lt),
        RefineOp::Le => cmp(CmpOp::Le),
        RefineOp::Gt => cmp(CmpOp::Gt),
        RefineOp::Ge => cmp(CmpOp::Ge),
        RefineOp::IsNull => Ok(RowKeyCond::IsNull),
        RefineOp::In => c
            .value
            .as_array()
            .ok_or_else(|| bad(ROW_KEY))?
            .iter()
            .map(|v| match v {
                Value::Null => Ok(None),
                v => id(v).map(Some),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(RowKeyCond::In),
    }
}

fn condition(op: &OpSpec, c: &RefineCond) -> Result<Condition, ExecError> {
    let spec = op
        .output_fields
        .iter()
        .find(|f| f.filter && f.name == c.field)
        .ok_or_else(|| bad(format!("unknown field `{}`", c.field)))?;
    let ty = spec.ty.clone();
    let lhs = path(&c.field);
    let cmp = |op: CmpOp| -> Result<Condition, ExecError> {
        Ok(Condition::Compare {
            op,
            lhs: lhs.clone(),
            rhs: literal(&ty, &c.value).ok_or_else(|| bad(&c.field))?,
            span: span(),
        })
    };
    match c.op {
        RefineOp::Eq => cmp(CmpOp::Eq),
        RefineOp::Ne => cmp(CmpOp::Ne),
        RefineOp::Lt => cmp(CmpOp::Lt),
        RefineOp::Le => cmp(CmpOp::Le),
        RefineOp::Gt => cmp(CmpOp::Gt),
        RefineOp::Ge => cmp(CmpOp::Ge),
        RefineOp::IsNull => Ok(Condition::IsNull {
            expr: lhs,
            negated: false,
            span: span(),
        }),
        RefineOp::In => {
            let items = c.value.as_array().ok_or_else(|| bad(&c.field))?;
            if items.is_empty() {
                // `in []` matches nothing, as a chain of no equalities does
                // on the device. SQL has no empty `IN ()`, so it is spelled
                // as the one condition no value meets.
                return Ok(Condition::And {
                    items: vec![
                        Condition::IsNull {
                            expr: lhs.clone(),
                            negated: false,
                            span: span(),
                        },
                        Condition::IsNull {
                            expr: lhs,
                            negated: true,
                            span: span(),
                        },
                    ],
                    span: span(),
                });
            }
            let list = items
                .iter()
                .map(|v| match v {
                    Value::Null => Some(Expr::NullLit { span: span() }),
                    v => literal(&ty, v),
                })
                .collect::<Option<Vec<Expr>>>()
                .ok_or_else(|| bad(&c.field))?;
            if spec.sealed {
                // The blind index answers one equality at a time, so a
                // sealed `in` is the chain of `==` it means: each member is
                // digested and compared, a null member through the reserved
                // null sentinel.
                let mut items: Vec<Condition> = list
                    .into_iter()
                    .map(|rhs| Condition::Compare {
                        op: CmpOp::Eq,
                        lhs: lhs.clone(),
                        rhs,
                        span: span(),
                    })
                    .collect();
                return Ok(if items.len() == 1 {
                    items.remove(0)
                } else {
                    Condition::Or {
                        items,
                        span: span(),
                    }
                });
            }
            Ok(Condition::In {
                expr: lhs,
                list,
                span: span(),
            })
        }
    }
}

/// One condition value as the literal the source would write for it.
fn literal(ty: &FieldTy, v: &Value) -> Option<Expr> {
    let s = span();
    match (ty, v) {
        (FieldTy::Blob | FieldTy::Json, _) => None,
        (FieldTy::Bool | FieldTy::Any, Value::Bool(b)) => {
            Some(Expr::BoolLit { value: *b, span: s })
        }
        (FieldTy::Int | FieldTy::Money { .. } | FieldTy::Decimal { .. }, Value::Number(n)) => {
            numeric(&n.as_i64()?.to_string())
        }
        (FieldTy::Money { .. } | FieldTy::Decimal { .. }, Value::String(d)) => numeric(d),
        (FieldTy::Any, Value::Number(n)) => numeric(&n.to_string()),
        (FieldTy::Date, Value::String(d)) => date(d),
        (FieldTy::Datetime, Value::String(d)) => datetime(d),
        (FieldTy::Duration { unit }, Value::Number(n)) => Some(Expr::DurationLit {
            n: n.as_u64()?,
            unit: ordo_syntax::token::DurationUnit::parse(unit)?,
            span: s,
        }),
        (FieldTy::Text | FieldTy::Id | FieldTy::Uri | FieldTy::Any, Value::String(t)) => {
            Some(Expr::StrLit {
                value: t.clone(),
                span: s,
            })
        }
        _ => None,
    }
}

/// `-?digits(.digits)?` as the parser reads it: an integer or exact-decimal
/// literal, under a unary minus when negative.
fn numeric(text: &str) -> Option<Expr> {
    let (neg, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || !frac.is_none_or(digits) {
        return None;
    }
    let lit = match frac {
        None => Expr::IntLit {
            value: int.parse().ok()?,
            span: span(),
        },
        Some(_) => Expr::DecimalLit {
            digits: body.to_string(),
            span: span(),
        },
    };
    Some(if neg {
        Expr::Neg {
            expr: Box::new(lit),
            span: span(),
        }
    } else {
        lit
    })
}

fn date(s: &str) -> Option<Expr> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    Some(Expr::DateLit {
        y: s[0..4].parse().ok()?,
        m: s[5..7].parse().ok()?,
        d: s[8..10].parse().ok()?,
        span: span(),
    })
}

fn datetime(s: &str) -> Option<Expr> {
    let b = s.as_bytes();
    if b.len() != 20 || b[10] != b'T' || b[19] != b'Z' {
        return None;
    }
    let Expr::DateLit { y, m, d, .. } = date(&s[..10])? else {
        return None;
    };
    Some(Expr::DatetimeLit {
        y,
        m,
        d,
        hh: s[11..13].parse().ok()?,
        mm: s[14..16].parse().ok()?,
        ss: s[17..19].parse().ok()?,
        span: span(),
    })
}

/// The language's spelling of a field's type, as the door sees it.
pub(crate) fn field_ty(ty: ordo_syntax::ScalarTy) -> FieldTy {
    use ordo_syntax::ScalarTy;
    match ty {
        ScalarTy::Text => FieldTy::Text,
        ScalarTy::Int => FieldTy::Int,
        ScalarTy::Money(places) => FieldTy::Money { places },
        ScalarTy::Decimal(places) => FieldTy::Decimal { places },
        ScalarTy::Bool => FieldTy::Bool,
        ScalarTy::Date => FieldTy::Date,
        ScalarTy::Datetime => FieldTy::Datetime,
        ScalarTy::Duration(unit) => FieldTy::Duration {
            unit: unit.as_str().to_string(),
        },
        ScalarTy::Id => FieldTy::Id,
        ScalarTy::Blob => FieldTy::Blob,
        ScalarTy::Uri => FieldTy::Uri,
        ScalarTy::Json => FieldTy::Json,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The literal built for a numeric string has the parser's shape: an
    /// integer or an exact decimal, under a unary minus when negative.
    #[test]
    fn numeric_literals_take_the_parsers_shape() {
        assert!(matches!(numeric("5"), Some(Expr::IntLit { value: 5, .. })));
        assert!(matches!(
            numeric("2.50"),
            Some(Expr::DecimalLit { ref digits, .. }) if digits == "2.50"
        ));
        let Some(Expr::Neg { expr, .. }) = numeric("-0.05") else {
            panic!("a negative decimal is a negation");
        };
        assert!(matches!(*expr, Expr::DecimalLit { ref digits, .. } if digits == "0.05"));
        for not_a_number in ["1e5", "", "-", "1.", ".5", "0x10"] {
            assert!(numeric(not_a_number).is_none(), "{not_a_number}");
        }
    }

    #[test]
    fn dates_and_datetimes_become_their_literals() {
        assert!(matches!(
            date("2026-10-05"),
            Some(Expr::DateLit {
                y: 2026,
                m: 10,
                d: 5,
                ..
            })
        ));
        assert!(matches!(
            datetime("2026-10-05T09:30:00Z"),
            Some(Expr::DatetimeLit {
                hh: 9,
                mm: 30,
                ss: 0,
                ..
            })
        ));
        assert!(datetime("2026-10-05T09:30:00+02:00").is_none());
    }
}
