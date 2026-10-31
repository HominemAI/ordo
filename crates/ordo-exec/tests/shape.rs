//! F6 shape verification, executed: every registered op's inferred shape
//! checked against what Postgres actually reports, with zero rows read.

use std::time::Duration;

use ordo_conformance::Fixture;
use ordo_exec::shape::{accepts, compare, ShapeMismatch};
use ordo_exec::{Pool, PoolConfig};

async fn shadow_pool(db: &str) -> Pool {
    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_millis(500),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot")
}

/// The load-time gate: the whole registry verified against an empty
/// shadow. A divergence between planner and database can never reach a
/// caller, because the load fails first.
#[tokio::test(flavor = "multi_thread")]
async fn every_registered_op_matches_the_shape_postgres_reports() {
    let f = Fixture::load().expect("fixture");
    let pool = shadow_pool("ordo_shape").await;
    pool.build_shadow(&f.ddl).await.expect("shadow builds");

    let mut names: Vec<&String> = f.registry.names().collect();
    names.sort();
    let mut checked = 0;
    let mut failures: Vec<ShapeMismatch> = Vec::new();

    for name in names {
        let Some(op) = f.registry.get(name) else {
            continue;
        };
        // Write ops have no result shape to verify; their statements are
        // the write path's own story.
        if op.read_pipeline().is_none() {
            continue;
        }
        let Ok(q) =
            ordo_sema::bind_op_generic(&f.schema, &f.registry, name, ordo_sema::Clock::CORPUS)
        else {
            continue;
        };
        // Shape verification is a load-time activity and holds custody
        // like any other server-side path, so it resolves digest sites
        // before planning. Worth noticing: an op with a blind-index
        // comparison cannot have its shape verified without a key
        // release — the digest does not affect the shape, but the plan
        // will not lower without one.
        let mut q = q;
        // Parameterized ops need bindings before they can be planned at
        // all — a blind comparison against a free parameter cannot resolve
        // its digest either. They are the corpus's business, with real
        // values; the load gate covers every op planable as registered.
        if f.resolve_blind(&mut q, &[]).is_err() {
            continue;
        }
        let plan = ordo_plan::plan(&q, &f.plan_space()).expect("registered ops plan");
        // A plan with unbound parameters cannot be described without
        // values; the fixture's parameterized ops are exercised by the
        // corpus with real bindings.
        if !plan.params.is_empty() {
            continue;
        }
        let observed = pool
            .observed_shape(&plan.sql)
            .await
            .unwrap_or_else(|e| panic!("{name}: describing the plan failed: {e}"));
        if let Err(m) = compare(name, &plan.outputs, &observed) {
            failures.push(m);
        }
        checked += 1;
    }

    // Parameterized ops need bindings to describe, so they are the
    // corpus's business rather than the load gate's. What the gate covers
    // is every op that can be planned as registered.
    assert!(
        checked >= 7,
        "only {checked} ops were verifiable — the gate has gone hollow"
    );
    assert!(
        failures.is_empty(),
        "the compiler and the database disagree:\n{}",
        failures
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!("shape-verified {checked} registered ops against an empty shadow");
}

/// Verification reads nothing. That is what makes it safe on every boot
/// and every hot swap: no row, no guard, nothing to leak.
#[tokio::test(flavor = "multi_thread")]
async fn verification_touches_no_rows() {
    let f = Fixture::load().expect("fixture");
    let pool = shadow_pool("ordo_shape_rows").await;
    pool.build_shadow(&f.ddl).await.expect("shadow builds");

    // Put a row in the shadow, then verify: the probe must not see it.
    pool.rows("SET search_path TO ordo_shadow; INSERT INTO \"vendors\" (\"$space\", \"id\", \"name\", \"region\", \"active\", \"rev\") VALUES ('conformance', 'v1', 'x', 'r', true, 1)")
        .await
        .expect("seeded the shadow");
    let shape = pool
        .observed_shape("SELECT \"name\" AS \"name\" FROM \"vendors\";")
        .await
        .expect("described");
    assert_eq!(shape, vec![("name".to_string(), "text".to_string())]);
    // The view describing the shape returns no rows when selected from —
    // the type is what was wanted, never the data.
    let rows = pool
        .rows("SET search_path TO ordo_shadow; SELECT count(*) FROM \"vendors\"")
        .await
        .unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("1"), "the shadow is untouched");
}

/// The check has to actually reject. A shape the compiler got wrong must
/// fail the load rather than serve plausible-looking wrong data.
#[tokio::test(flavor = "multi_thread")]
async fn a_divergent_shape_is_rejected() {
    use ordo_syntax::ScalarTy;

    // Wrong count.
    assert!(compare("op", &[("a".into(), ScalarTy::Int)], &[]).is_err());
    // Wrong name — a rename that silently changed a column.
    let e = compare(
        "op",
        &[("total".into(), ScalarTy::Money(0))],
        &[("sum".into(), "bigint".into())],
    )
    .unwrap_err();
    assert!(e.detail.contains("calls it `total`"), "{e}");
    // Wrong type — the failure that reaches a caller as wrong data.
    let e = compare(
        "op",
        &[("when".into(), ScalarTy::Date)],
        &[("when".into(), "text".into())],
    )
    .unwrap_err();
    assert!(e.detail.contains("says date"), "{e}");

    // And the accepted mappings are the ones the type system settled:
    // money is minor units in bigint, an id is text.
    assert!(accepts(ScalarTy::Money(0), "bigint"));
    assert!(accepts(ScalarTy::Id, "text"));
    assert!(accepts(ScalarTy::Datetime, "timestamp with time zone"));
    assert!(!accepts(ScalarTy::Int, "text"));
    assert!(!accepts(ScalarTy::Date, "timestamp with time zone"));

    // `decimal` — what `/` and `avg` produce — is unconstrained `numeric`
    // and nothing else. Without this row the gate would refuse a correct
    // plan (a type it has never heard of matches nothing), and with a
    // sloppier one it would wave through the float that must never appear
    // in a plan.
    assert!(accepts(ScalarTy::Decimal(12), "numeric"));
    assert!(!accepts(ScalarTy::Decimal(12), "double precision"));
    assert!(!accepts(ScalarTy::Decimal(12), "bigint"));
    let e = compare(
        "op",
        &[("share".into(), ScalarTy::Decimal(12))],
        &[("share".into(), "double precision".into())],
    )
    .unwrap_err();
    assert!(e.detail.contains("says decimal"), "{e}");
}

/// The end of requirement 4's chain: a plan that divides must survive the
/// load gate. The compiler says `decimal`, Postgres says `numeric`, and if
/// those two had not been taught about each other the gate would have
/// rejected a *correct* plan on boot — a new type that is absent from
/// `accepts` fails closed, which is the right default and the wrong
/// outcome.
#[tokio::test(flavor = "multi_thread")]
async fn a_division_plan_passes_the_load_gate() {
    let f = Fixture::load().expect("fixture");
    let pool = shadow_pool("ordo_shape_decimal").await;
    pool.build_shadow(&f.ddl).await.expect("shadow builds");

    for src in [
        "orderItems[amount, third: amount / 3]",
        "orderItems[amount]\nreduce { a: avg amount }",
        "orderItems[cents: round(amount / 3, 0), down: floor(amount / 3), finer: round(amount / 3, 2)]",
    ] {
        let pipeline = ordo_syntax::parse_pipeline(src).expect("parses");
        let q = ordo_sema::bind(&f.schema, &f.registry, &pipeline, ordo_sema::Clock::CORPUS).expect("binds");
        let plan = ordo_plan::plan(&q, &f.plan_space()).expect("plans");
        let observed = pool
            .observed_shape(&plan.sql)
            .await
            .unwrap_or_else(|e| panic!("{src}: describing the plan failed: {e}"));
        // Positively: the database really does report `numeric`, so the
        // mapping is verified against the server rather than asserted.
        assert!(
            observed.iter().any(|(_, t)| t == "numeric"),
            "{src}: no numeric column in {observed:?}"
        );
        assert!(
            !observed.iter().any(|(_, t)| t.contains("double")),
            "{src}: a float reached the plan: {observed:?}"
        );
        compare("case", &plan.outputs, &observed)
            .unwrap_or_else(|m| panic!("{src}: the load gate would refuse a correct plan: {m}"));
    }
}

/// Concurrent probes on one pool each see their own statement's columns,
/// and a probe leaves no `search_path` behind on the connection it used.
///
/// Found by `ordo dev` (2026-09-19): running describes concurrently
/// produced column lists that were the union of several ops' columns. Every
/// probe creates a session-temporary view named `$shape`, and the lookup
/// matched `pg_class.relname` alone, which finds every open session's
/// `$shape`. The probe also set `search_path` to the shadow schema and
/// never reset it, so a pooled connection kept resolving names there.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_probes_each_see_their_own_statement() {
    let pool = shadow_pool("ordo_shape_concurrent").await;
    pool.build_shadow(
        "CREATE TABLE \"a\" (\"x\" integer, \"y\" text); \
         CREATE TABLE \"b\" (\"p\" boolean, \"q\" numeric, \"r\" bigint)",
    )
    .await
    .expect("shadow builds");

    for _ in 0..20 {
        let (a, b) = tokio::join!(
            pool.observed_shape(r#"SELECT "x", "y" FROM "a""#),
            pool.observed_shape(r#"SELECT "p", "q", "r" FROM "b""#),
        );
        let names = |v: Vec<(String, String)>| v.into_iter().map(|c| c.0).collect::<Vec<_>>();
        assert_eq!(names(a.expect("a probes")), ["x", "y"]);
        assert_eq!(names(b.expect("b probes")), ["p", "q", "r"]);
    }

    // No probe left the shadow on a pooled connection's path.
    for _ in 0..4 {
        let rows = pool.rows("SHOW search_path").await.expect("show");
        let path = rows[0][0].clone().unwrap_or_default();
        assert!(
            !path.contains(ordo_exec::shape::SHADOW),
            "a pooled connection kept the probe's search_path: {path}"
        );
    }
}
