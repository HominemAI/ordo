//! Running a routine's body through the write protocol (checklist 12a).
//!
//! A routine is not a caller (DL §8, "Routines are not callers"). It runs
//! the statements its body declares, as one command group attributed
//! `routine:<name>@<version>`, through [`Fixture::run_write_on`]: the same
//! locks, images, checks and chain append as any op. Two things differ, and
//! both are R-G20:
//!
//! - **Guards pass.** The run is planned against a view of the schema whose
//!   entities carry no guards, the way the door's rule-6 view clears acl
//!   guards: by their not being there, so nothing downstream decides.
//! - **A change routine is bounded to the rows its trigger reaches.** Unless
//!   its `run` header says `unguarded`, every statement's condition is
//!   narrowed to `id in (<reachable ids>)` before it is planned, and an
//!   entity with no reachable rows gets `false`. The runtime resolves the
//!   reachable set (`ordo_exec::triggers::reachable`) and hands it in.
//!
//! The op the body runs as lives in a registry made for the run, so a
//! routine is never callable through the door.
//!
//! # `each ->`
//!
//! A routine whose body is `<pipeline> each -> <writes>` first reads: the
//! pipeline is bound under the run's provenance (`ordo_plan::each`), so a
//! change run reads only rows reachable from the changed ones, and at most
//! `each_max + 1` rows are fetched. Then, under one batch, the writes run
//! once per row with that row's values written in where the emission names
//! them. One batch is one command group: a run is one group, whatever the
//! number of rows, and its revert undoes the whole run.
//!
//! A run handles at most `each_max` rows. The summary says when more
//! matched. Nothing is carried to the next run: a body whose writes take
//! a row out of its selection finishes the rest over the next runs, and
//! one whose writes leave rows matching meets the same first rows again.

use ordo_syntax::ast::{Condition, Expr, Ident};
use ordo_syntax::decl::{OpBody, OpDecl, OpFlags, RoutineDecl, ScalarTy, WriteStmt};
use serde_json::{json, Value};

use crate::write::{WriteDb, WriteLog};
use crate::Fixture;

/// The rows a change-woken run may touch, per entity. `None` is "guards
/// pass": a clock run, or a change routine whose header says `unguarded`.
pub type RoutineRows = Option<std::collections::BTreeMap<String, Vec<String>>>;

impl Fixture {
    /// Run `routine`'s body once, as `actor` (`<name>@<version>`), at
    /// `now` (unix seconds, UTC).
    ///
    /// Returns the write summary. A run that changed nothing returns `Ok`
    /// and leaves `log` with no pending group, which the caller reads as
    /// "nothing to append".
    ///
    /// `each_max` is the most rows an `each` body handles in one run; a
    /// plain write body ignores it.
    #[allow(clippy::too_many_arguments)]
    pub fn run_routine_on(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        routine: &RoutineDecl,
        actor: &str,
        rows: &RoutineRows,
        now: u64,
        each_max: usize,
    ) -> Result<Value, String> {
        if routine.each.is_some() {
            return self.run_each_on(db, log, routine, actor, rows, now, each_max);
        }
        let mut decl = routine.clone();
        if let Some(rows) = rows {
            decl.body = routine
                .body
                .iter()
                .map(|s| bound_stmt(s, rows))
                .filter_map(Result::transpose)
                .collect::<Result<_, _>>()?;
            if decl.body.is_empty() {
                return Ok(Value::Array(Vec::new()));
            }
        }
        let op = ordo_schema::routine::routine_op(&decl);
        let mut schema = self.schema.clone();
        for e in &mut schema.entities {
            e.guards = None;
        }
        let mut view = self.with_schema(schema);
        view.registry.extend_from(&ordo_syntax::Program {
            ops: vec![op],
            ..ordo_syntax::Program::default()
        });
        let call = routine_call(&decl, now);
        log.act_as_routine(Some(actor.to_string()));
        let caller = format!("routine:{actor}");
        view.run_write_on(db, log, &call, Some(&caller))
    }
}

impl Fixture {
    /// Run an `each` routine once: read, then write per row, as one group.
    #[allow(clippy::too_many_arguments)]
    fn run_each_on(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        routine: &RoutineDecl,
        actor: &str,
        rows: &RoutineRows,
        now: u64,
        each_max: usize,
    ) -> Result<Value, String> {
        let pl = routine.each.as_ref().expect("an each routine");
        let (y, m, d, _, _, _) = civil(now);
        let params = routine_params(routine, now);
        let provenance = match rows {
            None => ordo_sema::Provenance::Clock,
            Some(map) => ordo_sema::Provenance::Listener(ordo_sema::TriggerRows(map.clone())),
        };
        let (mut q, outputs) = ordo_plan::each::bind_each(
            &self.schema,
            &self.registry,
            routine,
            &params,
            provenance,
            ordo_sema::Clock::at(y as i32, m as u8, d as u8),
        )
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
        self.resolve_blind(&mut q, &[])?;
        // One more than the limit, so the run can tell "exactly the limit"
        // from "more matched".
        let fetch = each_max.saturating_add(1) as i64;
        let keep = q.limit.as_ref().and_then(|l| l.at_most());
        if keep.is_none_or(|n| n > fetch) {
            q.limit = Some(ordo_sema::ir::Limit::Lit(fetch));
        }
        let plan = ordo_plan::plan(&q, &self.plan_space()).map_err(|e| format!("plan: {e}"))?;
        if !plan.params.is_empty() {
            return Err(format!(
                "routine `{}`: unbound parameters remain: {:?}",
                routine.name.name, plan.params
            ));
        }
        let mut cells = db.rows(&plan.sql)?;
        self.open_host_values(&mut cells, &plan)?;
        let more = cells.len() > each_max;
        cells.truncate(each_max);

        let names: Vec<String> = outputs.iter().map(|(n, _)| n.clone()).collect();
        let scope = ordo_schema::routine::row_scope(pl, &names);

        let mut schema = self.schema.clone();
        for e in &mut schema.entities {
            e.guards = None;
        }
        let mut view = self.with_schema(schema);
        let call = routine_call(routine, now);
        let caller = format!("routine:{actor}");
        log.act_as_routine(Some(actor.to_string()));
        // One group for the run. Opened here and closed on every path out,
        // as a leave does (`run_release_on`), so a refusal does not leave
        // the attempt's log batching.
        let opened = !log.in_batch();
        if opened {
            let correlation = format!("cg_{}", log.mint());
            log.open_batch(correlation);
        }
        let mut writes = Vec::new();
        let mut out = Ok(());
        for row in &cells {
            let r = self.each_row(
                db, log, &mut view, routine, &call, &caller, rows, &scope, &outputs, row,
            );
            match r {
                Ok(Some(v)) => writes.push(v),
                Ok(None) => {}
                Err(e) => {
                    out = Err(e);
                    break;
                }
            }
        }
        if opened {
            log.close_batch();
        }
        out?;
        Ok(json!({
            "routine": routine.name.name,
            "rows": cells.len(),
            "more": more,
            "writes": writes,
        }))
    }

    /// One row's writes. `None` when the bound left none of them.
    #[allow(clippy::too_many_arguments)]
    fn each_row(
        &self,
        db: &dyn WriteDb,
        log: &mut WriteLog,
        view: &mut Fixture,
        routine: &RoutineDecl,
        call: &str,
        caller: &str,
        rows: &RoutineRows,
        scope: &std::collections::BTreeMap<String, String>,
        outputs: &[(String, ScalarTy)],
        row: &[Option<String>],
    ) -> Result<Option<Value>, String> {
        let names: Vec<String> = outputs.iter().map(|(n, _)| n.clone()).collect();
        let name = routine.name.name.clone();
        let stmts = ordo_schema::routine::rewrite_emission(routine, &mut |rn| {
            let field = scope
                .get(&rn.spelled)
                .ok_or_else(|| ordo_schema::routine::not_in_row(&name, &rn.spelled, &names))?;
            let i = outputs
                .iter()
                .position(|(n, _)| n == field)
                .expect("the scope names outputs only");
            row_value(row.get(i).cloned().flatten(), outputs[i].1, rn.span)
        })
        .map_err(|e| format!("{}: {}", e.code, e.message))?;
        let stmts: Vec<WriteStmt> = match rows {
            None => stmts,
            Some(reach) => stmts
                .iter()
                .map(|s| bound_stmt(s, reach))
                .filter_map(Result::transpose)
                .collect::<Result<_, _>>()?,
        };
        if stmts.is_empty() {
            return Ok(None);
        }
        view.registry.extend_from(&ordo_syntax::Program {
            ops: vec![OpDecl {
                name: routine.name.clone(),
                params: routine.params.clone(),
                flags: OpFlags {
                    unbounded: routine.flags.unbounded,
                    reproducible: routine.flags.reproducible,
                    unguarded: false,
                    nopause: false,
                    public: false,
                    download: false,
                },
                body: OpBody::Write(stmts),
                span: routine.span,
            }],
            ..ordo_syntax::Program::default()
        });
        view.run_write_on(db, log, call, Some(caller)).map(Some)
    }
}

/// `$now` and `$today` as the literals the run's instant gives them.
pub fn routine_params(r: &RoutineDecl, now: u64) -> Vec<(String, Expr)> {
    let (y, m, d, hh, mm, ss) = civil(now);
    r.params
        .iter()
        .filter_map(|p| match p.name.name.as_str() {
            "now" => Some((
                "now".to_string(),
                Expr::DatetimeLit {
                    y: y as i32,
                    m: m as u8,
                    d: d as u8,
                    hh: hh as u8,
                    mm: mm as u8,
                    ss: ss as u8,
                    span: p.span,
                },
            )),
            "today" => Some((
                "today".to_string(),
                Expr::DateLit {
                    y: y as i32,
                    m: m as u8,
                    d: d as u8,
                    span: p.span,
                },
            )),
            _ => None,
        })
        .collect()
}

/// A row's value, as read from the database, as the literal a write takes.
///
/// Money arrives in minor units and is written back in major units, which
/// is what a money literal means (27ak), so it lands as the same number of
/// minor units. A datetime with no fraction becomes a datetime literal; one
/// with a fraction or an offset other than UTC is passed as the text the
/// database wrote, which it reads back exactly.
pub fn row_value(
    cell: Option<String>,
    ty: ScalarTy,
    span: ordo_syntax::Span,
) -> Result<Expr, String> {
    let Some(s) = cell else {
        return Ok(Expr::NullLit { span });
    };
    let neg = |e: Expr| Expr::Neg {
        expr: Box::new(e),
        span,
    };
    Ok(match ty {
        ScalarTy::Text | ScalarTy::Uri | ScalarTy::Id => Expr::StrLit { value: s, span },
        ScalarTy::Int => Expr::IntLit {
            value: s.parse().map_err(|_| format!("`{s}` is not an int"))?,
            span,
        },
        ScalarTy::Bool => Expr::BoolLit {
            value: s == "t" || s == "true",
            span,
        },
        ScalarTy::Money(places) => {
            let minor: i128 = s.parse().map_err(|_| format!("`{s}` is not money"))?;
            let abs = minor.unsigned_abs();
            let lit = if places == 0 {
                Expr::IntLit {
                    value: abs as i128,
                    span,
                }
            } else {
                let scale = 10u128.pow(u32::from(places));
                Expr::DecimalLit {
                    digits: format!(
                        "{}.{:0width$}",
                        abs / scale,
                        abs % scale,
                        width = usize::from(places)
                    ),
                    span,
                }
            };
            if minor < 0 {
                neg(lit)
            } else {
                lit
            }
        }
        ScalarTy::Decimal(_) => {
            let (minus, digits) = match s.strip_prefix('-') {
                Some(rest) => (true, rest.to_string()),
                None => (false, s.clone()),
            };
            let lit = if digits.contains('.') {
                Expr::DecimalLit { digits, span }
            } else {
                Expr::IntLit {
                    value: digits
                        .parse()
                        .map_err(|_| format!("`{s}` is not a decimal"))?,
                    span,
                }
            };
            if minus {
                neg(lit)
            } else {
                lit
            }
        }
        ScalarTy::Date => {
            let p: Vec<&str> = s.split('-').collect();
            match p.as_slice() {
                [y, m, d] => Expr::DateLit {
                    y: y.parse().map_err(|_| format!("`{s}` is not a date"))?,
                    m: m.parse().map_err(|_| format!("`{s}` is not a date"))?,
                    d: d.parse().map_err(|_| format!("`{s}` is not a date"))?,
                    span,
                },
                _ => return Err(format!("`{s}` is not a date")),
            }
        }
        ScalarTy::Datetime => match utc_datetime(&s) {
            Some((y, m, d, hh, mm, ss)) => Expr::DatetimeLit {
                y,
                m,
                d,
                hh,
                mm,
                ss,
                span,
            },
            None => Expr::StrLit { value: s, span },
        },
        ScalarTy::Duration(unit) => {
            let n: i128 = s.parse().map_err(|_| format!("`{s}` is not a duration"))?;
            let lit = Expr::DurationLit {
                n: n.unsigned_abs() as u64,
                unit,
                span,
            };
            if n < 0 {
                neg(lit)
            } else {
                lit
            }
        }
        ScalarTy::Blob | ScalarTy::Json => {
            return Err(format!(
                "a `{}` value cannot be carried into a write",
                ty.spelled()
            ))
        }
    })
}

/// `2026-10-08 09:00:00+00` (or with `T` / `Z`) as its parts, when it is
/// whole seconds in UTC.
fn utc_datetime(s: &str) -> Option<(i32, u8, u8, u8, u8, u8)> {
    let s = s
        .strip_suffix("+00")
        .or_else(|| s.strip_suffix("+00:00"))
        .or_else(|| s.strip_suffix('Z'))?;
    let (date, time) = s.split_once([' ', 'T'])?;
    let d: Vec<&str> = date.split('-').collect();
    let t: Vec<&str> = time.split(':').collect();
    match (d.as_slice(), t.as_slice()) {
        ([y, m, d], [hh, mm, ss]) if !ss.contains('.') => Some((
            y.parse().ok()?,
            m.parse().ok()?,
            d.parse().ok()?,
            hh.parse().ok()?,
            mm.parse().ok()?,
            ss.parse().ok()?,
        )),
        _ => None,
    }
}

/// The call that runs a routine's op: its name, and the parameters the
/// platform fills from the run's instant.
pub fn routine_call(r: &RoutineDecl, now: u64) -> String {
    let (y, m, d, hh, mm, ss) = civil(now);
    let args: Vec<String> = r
        .params
        .iter()
        .filter_map(|p| match p.name.name.as_str() {
            "now" => Some(format!(
                "$now: \"{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z\""
            )),
            "today" => Some(format!("$today: {y:04}-{m:02}-{d:02}")),
            _ => None,
        })
        .collect();
    if args.is_empty() {
        return r.name.name.clone();
    }
    format!("{}({})", r.name.name, args.join(", "))
}

/// Narrow one statement to the rows a change-woken run may touch. `None`
/// is a statement the bound leaves nothing to do: an addressed converge
/// (`set e { id: "…", … }`) of a row the changed rows do not reach, which
/// is invisible to the run as a guard would make it.
fn bound_stmt(
    stmt: &WriteStmt,
    rows: &std::collections::BTreeMap<String, Vec<String>>,
) -> Result<Option<WriteStmt>, String> {
    let (entity, span) = match stmt {
        WriteStmt::Set { entity, span, .. } | WriteStmt::Del { entity, span, .. } => {
            (entity, *span)
        }
    };
    let ids = rows.get(&entity.name).cloned().unwrap_or_default();
    let reach = if ids.is_empty() {
        Condition::BareBool(Expr::BoolLit { value: false, span })
    } else {
        Condition::In {
            expr: Expr::Path {
                segments: vec![Ident {
                    name: "id".into(),
                    span,
                }],
                span,
            },
            list: ids
                .into_iter()
                .map(|value| Expr::StrLit { value, span })
                .collect(),
            span,
        }
    };
    let and = |c: &Condition| Condition::And {
        items: vec![c.clone(), reach.clone()],
        span,
    };
    Ok(Some(match stmt {
        WriteStmt::Set {
            binding,
            entity,
            cond: Some(c),
            fields,
            span,
        } => WriteStmt::Set {
            binding: binding.clone(),
            entity: entity.clone(),
            cond: Some(and(c)),
            fields: fields.clone(),
            span: *span,
        },
        // An addressed converge: the row is named, so it is written when
        // the changed rows reach it and left alone when they do not.
        WriteStmt::Set {
            cond: None, fields, ..
        } if fields.iter().any(|(f, _)| f.name == "id") => {
            let id = fields.iter().find(|(f, _)| f.name == "id").map(|(_, v)| v);
            return match id {
                Some(Expr::StrLit { value, .. }) => {
                    let reached = rows
                        .get(&entity.name)
                        .is_some_and(|ids| ids.iter().any(|i| i == value));
                    Ok(reached.then(|| stmt.clone()))
                }
                Some(Expr::NullLit { .. }) => Ok(None),
                _ => Err(format!(
                    "routine_out_of_bound: `set {} {{ id: … }}` in a routine woken by a \
                     change addresses a row by a value the run cannot check against the \
                     changed rows",
                    entity.name
                )),
            };
        }
        // `ordo check` refuses this (`routine_out_of_bound`); refused here
        // too, so a program compiled by an older checker cannot reach it.
        WriteStmt::Set { cond: None, .. } => {
            return Err(format!(
                "routine_out_of_bound: `set {} {{ … }}` with no condition, in a routine \
                 woken by a change",
                entity.name
            ))
        }
        WriteStmt::Del {
            entity,
            cond,
            subtree,
            span,
        } => WriteStmt::Del {
            entity: entity.clone(),
            cond: and(cond),
            subtree: *subtree,
            span: *span,
        },
    }))
}

/// Unix seconds to a UTC civil date and time (Howard Hinnant's algorithm).
pub fn civil(secs: u64) -> (i64, u32, u32, u64, u64, u64) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_time_is_utc() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        // 2026-10-08T09:00:00Z
        assert_eq!(civil(1_791_450_000), (2026, 10, 8, 9, 0, 0));
    }
}
