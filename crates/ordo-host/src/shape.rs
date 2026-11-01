//! 8n / F6 — the load path that can fail.
//!
//! `ordo-exec`'s [`ordo_exec::shape`] is the mechanism: build an empty
//! shadow schema from the deployment's DDL, ask Postgres for the row type
//! of a planned statement, compare it to what the compiler inferred. It
//! was correct and it gated nothing — its only callers were its own tests,
//! so a deployment whose database disagreed with its compiler served the
//! disagreement.
//!
//! This is the gate. It runs over the **whole registry** at load, and a
//! single mismatch refuses the boot — not by convention but by type:
//! [`crate::serve::PlatformState::new`] takes a [`ShapeReport`], the only
//! way to obtain one is [`verify_registry`], and it returns one only where
//! verification passed. A deployment that skipped the gate has no
//! [`crate::serve::platform_router`] to mount and therefore no surface.
//!
//! # What "the whole registry" honestly means
//!
//! Not every registered op has a shape to verify, and pretending otherwise
//! would make the gate lie about its own coverage. Each op lands in exactly
//! one bucket and the report names all of them:
//!
//! - **Verified.** The op planned and Postgres described it, and the two
//!   shapes agree. Parameterized reads are *in* this bucket: a parameter's
//!   VALUE cannot change a result's shape (its declared type already fixed
//!   that), so the gate binds one canonical placeholder per declared type
//!   and describes the statement that produces. This is what lets the gate
//!   cover the registry rather than the seven ops that happen to take no
//!   arguments.
//! - **No result shape.** Writes and deletes. A `set` returns an effect
//!   summary, not a projected row type; the write path's own refusals are
//!   its story.
//! - **Undescribable.** An op the gate genuinely cannot pose to the
//!   database as registered — a plan that still carries a free parameter
//!   after placeholder binding, a blind-index digest that will not resolve,
//!   a cost class this executor does not serve. Each is named with its
//!   reason in the report, so "we skipped it" is a statement a reader can
//!   audit rather than a silence.
//!
//! And the gate refuses a registry it verified *nothing* in: a gate whose
//! coverage has quietly collapsed to zero is indistinguishable from no gate,
//! which is precisely the state 8n describes.
//!
//! # What the empty shadow cannot see, and where that is caught
//!
//! The shadow is built from the DDL **the artifact carries**, and the
//! compiler infers its shapes from the same program. So both sides of the
//! comparison descend from one source: what this gate proves is that the
//! *planner* and *Postgres* agree, not that the live database is the one
//! the program was compiled for. A migration that has not been applied, or
//! a restore that landed the database on an older shape, passes here and
//! fails on the first request.
//!
//! That second question is [`ordo_exec::shape::Pool::live_drift`], asked on
//! the same shadow while it exists and carried out on [`ShapeReport::drift`]
//! — *observed* here so `/health/deep` shows it, and *refused* by
//! [`crate::deploy`], which is the only caller in a position to insist,
//! because it is the one about to make an artifact the serving one.
//!
//! It is worth naming the consequence for this module's own tests: a pure
//! shape mismatch with no drift is close to unproducible, because producing
//! one means handing the gate a DDL that is not the live schema — which is
//! drift. The mismatch test below is therefore also a drift case, and that
//! is a fact about the mechanism rather than about the test.
//!
//! # Why verification runs on its own connections
//!
//! `build_shadow` and `observed_shape` both issue `SET search_path TO
//! ordo_shadow`, and `search_path` is **connection state**. A pooled
//! connection handed back to the idle list still carrying it would aim the
//! next caller's ordinary read at an empty schema — silently zero rows, the
//! worst possible failure. So verification takes a short-lived pool of its
//! own, and that pool is dropped (its connections closed) before the
//! deployment serves anything.

use ordo_api::{OpKind, OpSpec, ParamTy};
use ordo_conformance::Fixture;
use ordo_exec::shape::ShapeMismatch;
use ordo_exec::{Pool, PoolConfig};

/// Why an op could not be posed to the database as registered. Never a
/// pass — a reason a reader can weigh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undescribable {
    pub op: String,
    pub reason: String,
}

/// What the load gate saw. Published on `/health/deep` so an operator can
/// read the coverage rather than trust it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShapeReport {
    /// Ops whose inferred shape the database confirmed.
    pub verified: Vec<String>,
    /// Ops with no result shape to verify (writes, deletes).
    pub no_result_shape: Vec<String>,
    /// Ops the gate could not pose, each with the reason why.
    pub undescribable: Vec<Undescribable>,
    /// Ways the **live** schema is not the shape this DDL asserts.
    ///
    /// Observed here and refused by [`crate::deploy`], not by this
    /// function — see the module doc's *"what the empty shadow cannot
    /// see"*. It is carried on the report so `/health/deep` shows an
    /// operator a database that has drifted under a deployment which is
    /// still serving, which is the state a restore or a half-applied
    /// migration leaves behind.
    pub drift: Vec<ordo_exec::shape::Drift>,
}

impl ShapeReport {
    pub fn total(&self) -> usize {
        self.verified.len() + self.no_result_shape.len() + self.undescribable.len()
    }

    /// One line an operator can read at a glance.
    pub fn summary(&self) -> String {
        format!(
            "{} of {} registered ops shape-verified against an empty shadow \
             ({} have no result shape, {} were undescribable, {} live-schema drift)",
            self.verified.len(),
            self.total(),
            self.no_result_shape.len(),
            self.undescribable.len(),
            self.drift.len(),
        )
    }
}

/// The load refused. Every variant is fatal: the deployment does not serve.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShapeRefusal {
    /// The compiler and the database disagree about at least one op's
    /// result shape. Both shapes are named, per mismatch.
    Mismatch(Vec<ShapeMismatch>),
    /// Every registered read op turned out to be undescribable, so the gate
    /// verified nothing. A gate with zero coverage is not a gate.
    NoCoverage(ShapeReport),
    /// The shadow schema could not be built, or the database was
    /// unreachable. Verification could not run, so it did not pass.
    Unavailable(String),
    /// A constraint the compiled program's equality relies on is gone from
    /// the live database: a column it treats as never null allows nulls, or
    /// a required ref's foreign key is missing. Each line names one.
    ///
    /// The planner compiles `==` to a plain `=` only where a side is proven
    /// never null (`ordo_plan::never_null`, board
    /// `guard-equality-uses-no-index`), and the proof is the database's own
    /// `NOT NULL`. Serving on a database without it could return different
    /// rows than the language promises, so the load is refused.
    NeverNullGone(Vec<String>),
}

impl std::fmt::Display for ShapeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShapeRefusal::Mismatch(ms) => {
                write!(
                    f,
                    "shape verification failed — the compiler and the database disagree, \
                     so this registry will not be served:\n{}",
                    ms.iter()
                        .map(|m| format!("  {m}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
            ShapeRefusal::NoCoverage(r) => write!(
                f,
                "shape verification covered no op at all ({}) — a gate that verifies \
                 nothing is not a gate:\n{}",
                r.summary(),
                r.undescribable
                    .iter()
                    .map(|u| format!("  `{}`: {}", u.op, u.reason))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            ShapeRefusal::Unavailable(m) => {
                write!(f, "shape verification could not run: {m}")
            }
            ShapeRefusal::NeverNullGone(lines) => write!(
                f,
                "the compiled program relies on constraints the live database no longer \
                 has, so this registry will not be served. Restore each one (the DDL the \
                 program was compiled with states it):\n{}",
                lines
                    .iter()
                    .map(|l| format!("  {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        }
    }
}

impl std::error::Error for ShapeRefusal {}

/// A caller the gate uses for guard inlining. It is a real string so the
/// guard lowers to real SQL (and therefore a real row type); it matches no
/// member, and the statement is described rather than run, so it can select
/// nothing from nowhere.
const PROBE_CALLER: &str = "$shape-probe";

/// One canonical literal per declared parameter type. A parameter's value
/// cannot change a result's *shape* — its declared type already did — so
/// any well-typed literal describes the same statement the caller's will.
fn placeholder(ty: &ParamTy) -> Option<&'static str> {
    match ty {
        ParamTy::Int => Some("0"),
        ParamTy::Money { .. } | ParamTy::Decimal { .. } => Some("0"),
        ParamTy::Text { .. } => Some("\"\""),
        ParamTy::Date => Some("2026-01-01"),
        // The literal grammar's datetime form, and it is the type's own
        // canonical spelling rather than a second one invented here.
        ParamTy::Datetime => Some("2026-01-01T00:00:00Z"),
        ParamTy::Bool => Some("false"),
        ParamTy::Id => Some("\"shape_probe\""),
        // An address is a string literal, and any well-typed one describes
        // the same statement — the probe only needs the SQL to have a row
        // type, and `uri` lowers to `text`.
        ParamTy::Uri => Some("\"https://example.invalid/probe\""),
        // The empty document, as the string literal a call carries it in.
        ParamTy::Json => Some("\"{}\""),
        // Duration literals carry a unit that belongs to the declaration,
        // not to the type inventory the door sees. Rather than guess one,
        // the op is named undescribable and stays visible in the report.
        ParamTy::Duration => None,
        // A function's parameter only; no op statement takes one.
        ParamTy::Blob => None,
    }
}

/// The call source the gate poses — the same composition form the executor
/// builds at request time ([`crate::FixtureExecutor`]), with placeholders
/// where a caller's arguments would be.
fn probe_call(spec: &OpSpec) -> Option<String> {
    if spec.params.is_empty() {
        return Some(spec.name.clone());
    }
    let mut args = Vec::new();
    for p in &spec.params {
        args.push(format!("${}: {}", p.name, placeholder(&p.ty)?));
    }
    Some(format!("{}({})", spec.name, args.join(", ")))
}

/// Every constraint in the live `public` schema that the never-null rule
/// relies on and that is gone, one line each, naming the table and column.
///
/// - Each column [`ordo_sema::not_null_columns`] lists (the evidence the
///   planner proves `==` with) must be `NOT NULL` live.
/// - Each required ref's foreign key (`entity$field$fk`) must exist and be
///   validated: a hop through a required ref is proven present because that
///   key names a row. A key added `NOT VALID` has not checked the rows
///   already there.
///
/// A table or column missing outright is drift, reported and refused by the
/// drift gate, and is not repeated here.
pub async fn never_null_gone(
    pool: &Pool,
    schema: &ordo_schema::Schema,
) -> Result<Vec<String>, ordo_exec::PoolError> {
    let nullable: std::collections::BTreeSet<(String, String)> = pool
        .rows(
            "SELECT table_name, column_name FROM information_schema.columns \
             WHERE table_schema = 'public' AND is_nullable = 'YES'",
        )
        .await?
        .into_iter()
        .filter_map(|r| match r.as_slice() {
            [Some(t), Some(c)] => Some((t.clone(), c.clone())),
            _ => None,
        })
        .collect();
    let tables: std::collections::BTreeSet<String> = pool
        .rows("SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'")
        .await?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    let keys: std::collections::BTreeSet<String> = pool
        .rows(
            "SELECT c.conname FROM pg_constraint c \
             JOIN pg_namespace n ON n.oid = c.connamespace \
             WHERE n.nspname = 'public' AND c.contype = 'f' AND c.convalidated",
        )
        .await?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();

    let mut gone = Vec::new();
    for (table, columns) in ordo_sema::not_null_columns(schema).iter() {
        for column in columns {
            if nullable.contains(&(table.clone(), column.clone())) {
                gone.push(format!(
                    "`{table}`.`{column}` allows nulls; the program compares it as never \
                     null. Set it NOT NULL."
                ));
            }
        }
    }
    for e in &schema.entities {
        if !tables.contains(&e.name) {
            continue;
        }
        for field in &e.fields {
            if field.optional || !matches!(field.ty, ordo_schema::FieldTy::Ref { .. }) {
                continue;
            }
            let key = format!("{}${}$fk", e.name, field.name);
            if !keys.contains(&key) {
                gone.push(format!(
                    "`{}`.`{}` has no foreign key `{key}`; the program reads through it \
                     as a ref that always names a row. Restore the key.",
                    e.name,
                    field.column()
                ));
            }
        }
    }
    Ok(gone)
}

/// Verify every registered op's inferred shape against what Postgres
/// reports for it, with zero rows read.
///
/// `ddl` is the schema the deployment asserts its database holds — the DDL
/// its migration applied. The shadow is built from that, so a database that
/// has drifted from the DDL the compiler compiled against is exactly what
/// this catches.
pub async fn verify_registry(
    f: &Fixture,
    cfg: &PoolConfig,
    ddl: &str,
) -> Result<ShapeReport, ShapeRefusal> {
    // Its own connections, its own lifetime — see the module doc on why a
    // pooled connection must never carry `search_path` back to the pool.
    let pool = Pool::connect(PoolConfig {
        size: 2,
        ..cfg.clone()
    })
    .await
    .map_err(|e| ShapeRefusal::Unavailable(e.to_string()))?;
    let report = verify_on(f, &pool, ddl).await;
    // Best effort: the shadow is rebuilt from scratch on every verification
    // anyway, so a failure to tidy it is not a reason to refuse a boot that
    // otherwise passed.
    let _ = pool.rows("DROP SCHEMA IF EXISTS ordo_shadow CASCADE").await;
    drop(pool);
    report
}

async fn verify_on(f: &Fixture, pool: &Pool, ddl: &str) -> Result<ShapeReport, ShapeRefusal> {
    pool.build_shadow(ddl)
        .await
        .map_err(|e| ShapeRefusal::Unavailable(format!("the shadow schema did not build: {e}")))?;

    // Asked while the shadow still exists, and asked here rather than by a
    // second caller building a second shadow. It does not refuse — see
    // `ShapeReport::drift`.
    let drift = pool
        .live_drift(ordo_exec::shape::SHADOW, "public")
        .await
        .map_err(|e| {
            ShapeRefusal::Unavailable(format!("the live schema could not be read: {e}"))
        })?;
    let gone = never_null_gone(pool, &f.schema).await.map_err(|e| {
        ShapeRefusal::Unavailable(format!("the live schema could not be read: {e}"))
    })?;
    if !gone.is_empty() {
        return Err(ShapeRefusal::NeverNullGone(gone));
    }
    let mut report = ShapeReport {
        drift,
        ..ShapeReport::default()
    };
    let mut mismatches: Vec<ShapeMismatch> = Vec::new();
    let mut specs = crate::build_specs(f);
    specs.sort_by(|a, b| a.name.cmp(&b.name));

    for spec in &specs {
        if spec.kind != OpKind::Read {
            report.no_result_shape.push(spec.name.clone());
            continue;
        }
        let mut undescribable = |reason: String| {
            report.undescribable.push(Undescribable {
                op: spec.name.clone(),
                reason,
            })
        };
        let Some(call) = probe_call(spec) else {
            undescribable("a parameter type the gate has no canonical literal for".into());
            continue;
        };
        let pl = match ordo_syntax::parse_pipeline(&call) {
            Ok(pl) => pl,
            Err(e) => {
                undescribable(format!(
                    "the probe call did not parse: {} at {}",
                    e.message, e.span
                ));
                continue;
            }
        };
        let mut q = match ordo_sema::bind_as(
            &f.schema,
            &f.registry,
            &pl,
            &[],
            Some(PROBE_CALLER),
            ordo_sema::Clock::CORPUS,
        ) {
            Ok(q) => q,
            Err(e) => {
                undescribable(format!("does not bind as registered: {e}"));
                continue;
            }
        };
        // Shape verification holds custody like any other server-side path:
        // a blind comparison resolves its digest before planning. A site
        // that will not resolve is undescribable, not a pass.
        if let Err(e) = f.resolve_blind(&mut q, &[]) {
            undescribable(format!("a blind-index site did not resolve: {e}"));
            continue;
        }
        // Shape verification plans against the *shadow* schema, which is
        // built from the same DDL and holds no rows. The space is folded
        // in like any other statement's — the shadow has a `$space` column
        // because the real tables do, and a describe over a statement
        // missing its predicate would be describing a different query
        // from the one that will run.
        //
        // **Which** space is folded in does not matter and the fixture's
        // own is used because it is the one in hand: the gate asks Postgres
        // for the described *types* of a statement over an empty shadow, and
        // a `$space` literal changes no column's type. That is what makes
        // one shape report cover a deployment serving many spaces: the
        // program is deployment-wide, and only the rows are per tenant.
        // The same attach the artifact and both read paths make (C4e). The
        // gate compares `plan.outputs` against what the database reports, so
        // planning WITHOUT the row key here would verify a different column
        // set than the descriptor declares, and the gate would go on passing
        // while the two disagreed.
        let _ = ordo_plan::artifact::project_row_key(&mut q);
        let plan = match ordo_plan::plan(&q, &ordo_plan::Space::bound(&f.space().0)) {
            Ok(p) => p,
            Err(e) => {
                undescribable(format!("does not plan as registered: {e}"));
                continue;
            }
        };
        if !plan.params.is_empty() {
            undescribable(format!(
                "the plan still carries free parameters {:?} after placeholder binding",
                plan.params
            ));
            continue;
        }
        let observed = match pool.observed_shape(&plan.sql).await {
            Ok(o) => o,
            Err(e) => {
                // A database that REFUSES the plan is divergence, not an
                // op the gate cannot pose. The distinction matters: the
                // `undescribable` bucket is tolerated, and putting a
                // refusal in it means a schema wrong enough that the
                // statement will not even describe passes the gate that
                // exists to catch exactly that.
                //
                // Found the day a collation pin landed: the divergent
                // fixture stopped producing a shape MISMATCH and started
                // producing "collations are not supported by type date",
                // and the gate shrugged.
                mismatches.push(ShapeMismatch {
                    op: spec.name.clone(),
                    detail: format!(
                        "the database refused to describe this op's plan, which means the \
                         schema it holds cannot run it: {e}"
                    ),
                });
                continue;
            }
        };
        match ordo_exec::shape::compare(&spec.name, &plan.outputs, &observed) {
            Ok(()) => report.verified.push(spec.name.clone()),
            Err(m) => mismatches.push(m),
        }
    }

    if !mismatches.is_empty() {
        return Err(ShapeRefusal::Mismatch(mismatches));
    }
    if report.verified.is_empty() {
        return Err(ShapeRefusal::NoCoverage(report));
    }
    Ok(report)
}
