//! **`each ->` bodies** (DL §8, "Tier 1"; owner ruling 2026-10-08,
//! `triggers` ask 1).
//!
//! A routine whose body is `<pipeline> each -> <writes>` reads rows and runs
//! its writes once for each. This module is the half that needs the binder:
//!
//! - [`bind_each`] binds the pipeline under the run's provenance (R-G20):
//!   guards pass for a clock run, and a change run's sources are narrowed to
//!   the rows reachable from the changed ones. It is the same
//!   `ordo_sema::bind_routine` seam the guard corpus already holds to that
//!   rule, so the bound is decided in one place.
//! - [`check_op`] is what the registration gate checks: the emission with
//!   each row name rewritten to a typed parameter, `$row.<field>`, so the
//!   write rules (insert-completeness, bulk identity, typed slots, places)
//!   run over it exactly as over a write op.
//!
//! The runtime (`ordo_conformance::routine`) runs the emission once per row
//! with that row's values written in, inside one command group.

use ordo_schema::routine::{not_in_row, rewrite_emission, row_scope};
use ordo_schema::Schema;
use ordo_sema::{Clock, Provenance, Query, Registry, SemaError};
use ordo_syntax::ast::{Expr, Ident};
use ordo_syntax::decl::{OpBody, OpDecl, OpFlags, ParamDecl, RoutineDecl, ScalarTy};

/// The prefix the registration gate gives a row field written as a
/// parameter. A `.` cannot appear in a parameter an author writes, so the
/// name collides with nothing a routine declares.
pub const ROW_PARAM: &str = "row.";

/// The fallback for the number of rows one run of an `each` routine
/// handles: the deployment setting `ORDO_ROUTINE_EACH_MAX_ROWS`.
pub const DEFAULT_EACH_MAX_ROWS: usize = 500;

/// The setting's name.
pub const EACH_MAX_ROWS_ENV: &str = "ORDO_ROUTINE_EACH_MAX_ROWS";

/// [`EACH_MAX_ROWS_ENV`], or [`DEFAULT_EACH_MAX_ROWS`] when it is unset,
/// zero or not a number.
pub fn each_max_rows_from_env() -> usize {
    std::env::var(EACH_MAX_ROWS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_EACH_MAX_ROWS)
}

fn from_schema(e: ordo_schema::SchemaError) -> SemaError {
    SemaError {
        code: e.code,
        message: e.message,
        span: e.span,
    }
}

/// Bind `r`'s pipeline. `params` are the run's `$now` / `$today` as
/// literals. Returns the bound query and the row's fields with their types,
/// in output order.
///
/// # Panics
///
/// Never for a routine with an `each`; a routine without one is the
/// caller's mistake and answers `invalid_routine`.
pub fn bind_each(
    schema: &Schema,
    registry: &Registry,
    r: &RoutineDecl,
    params: &[(String, Expr)],
    provenance: Provenance,
    clock: Clock,
) -> Result<(Query, Vec<(String, ScalarTy)>), SemaError> {
    let Some(pl) = &r.each else {
        return Err(SemaError {
            code: "invalid_routine",
            message: format!("routine `{}` has no `each ->` pipeline", r.name.name),
            span: r.span,
        });
    };
    let q = ordo_sema::bind_routine(schema, registry, pl, params, provenance, r.flags, clock)?;
    let outputs = q
        .outputs
        .iter()
        .map(|o| (o.name.clone(), o.expr.ty()))
        .collect();
    Ok((q, outputs))
}

/// Literal stand-ins for `$now` and `$today`, for binding at registration.
pub fn placeholder_params(r: &RoutineDecl) -> Vec<(String, Expr)> {
    let span = r.span;
    r.params
        .iter()
        .filter_map(|p| match p.name.name.as_str() {
            "now" => Some((
                "now".to_string(),
                Expr::DatetimeLit {
                    y: 2026,
                    m: 5,
                    d: 1,
                    hh: 0,
                    mm: 0,
                    ss: 0,
                    span,
                },
            )),
            "today" => Some((
                "today".to_string(),
                Expr::DateLit {
                    y: 2026,
                    m: 5,
                    d: 1,
                    span,
                },
            )),
            _ => None,
        })
        .collect()
}

/// The op an `each` routine's emission is checked as: its row names
/// rewritten to `$row.<field>` parameters typed as the row's fields.
///
/// Registered nowhere a caller can reach, like `routine_op`. A row field
/// that is a `blob` or a `json` document cannot be carried into a write
/// (`invalid_routine`): a blob handle is never a write's right-hand side,
/// and a document is opaque to the language.
pub fn check_op(
    schema: &Schema,
    registry: &Registry,
    r: &RoutineDecl,
) -> Result<OpDecl, SemaError> {
    let params = placeholder_params(r);
    let (_, outputs) = bind_each(
        schema,
        registry,
        r,
        &params,
        Provenance::Clock,
        Clock::CORPUS,
    )?;
    let names: Vec<String> = outputs.iter().map(|(n, _)| n.clone()).collect();
    let pl = r
        .each
        .as_ref()
        .expect("bind_each refused a routine without one");
    let scope = row_scope(pl, &names);
    let mut used: Vec<(String, ScalarTy)> = Vec::new();
    let routine = r.name.name.clone();
    let body = rewrite_emission(r, &mut |rn| {
        let Some(field) = scope.get(&rn.spelled) else {
            return Err(not_in_row(&routine, &rn.spelled, &names));
        };
        let ty = outputs
            .iter()
            .find(|(n, _)| n == field)
            .map(|(_, t)| *t)
            .expect("the scope names outputs only");
        if matches!(ty, ScalarTy::Blob | ScalarTy::Json) {
            return Err(format!(
                "routine `{routine}`: `{}` is a `{}` field, and a row's `{}` value cannot be \
                 carried into a write",
                rn.spelled,
                ty.spelled(),
                ty.spelled()
            ));
        }
        if !used.iter().any(|(n, _)| n == field) {
            used.push((field.clone(), ty));
        }
        Ok(Expr::Param {
            name: format!("{ROW_PARAM}{field}"),
            span: rn.span,
        })
    })
    .map_err(from_schema)?;
    let mut op_params = r.params.clone();
    for (field, ty) in used {
        op_params.push(ParamDecl {
            name: Ident {
                name: format!("{ROW_PARAM}{field}"),
                span: r.span,
            },
            ty,
            max: None,
            text_max: None,
            default: None,
            filled: false,
            span: r.span,
        });
    }
    Ok(OpDecl {
        name: r.name.clone(),
        params: op_params,
        flags: OpFlags {
            unbounded: r.flags.unbounded,
            reproducible: r.flags.reproducible,
            unguarded: false,
            nopause: false,
            public: false,
            download: false,
        },
        body: OpBody::Write(body),
        span: r.span,
    })
}

/// The pipeline as a read op, for the read half of the registration gate:
/// reserved parameters, `reproducible`, and the viral acknowledgment rule
/// (composing an op whose bypass is live needs `unguarded` on the `run`
/// header, DL §8). The row bound is not asked of it: the per-run row limit
/// bounds what one run reads.
pub fn read_op(r: &RoutineDecl) -> Option<OpDecl> {
    let pl = r.each.as_ref()?;
    Some(OpDecl {
        name: r.name.clone(),
        params: r.params.clone(),
        flags: OpFlags {
            unbounded: r.flags.unbounded,
            reproducible: r.flags.reproducible,
            unguarded: r.flags.unguarded,
            nopause: false,
            public: false,
            download: false,
        },
        body: OpBody::Read(pl.clone()),
        span: r.span,
    })
}
