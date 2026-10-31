//! 18d T7 R-S3 — **an apply reaches every database, and the activation
//! waits for all of them.**
//!
//! Before this, `apply` ran against the one pool it was handed, which is
//! the control database in the binary. A deployment with an attached
//! tenant could therefore deploy a second schema version, activate it,
//! and leave that tenant's database without the tables the new program
//! references. Every request in that tenant would then fail per
//! statement, late, and differently each time.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::route::{PoolResolver, Target, Targets};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{apply_everywhere, apply_everywhere_except_default, Program};

/// The apply lock, which R-S5 makes the walk's precondition.
async fn routing_lock() -> ordo_host::apply_op::ApplyLock {
    // One connection, which is all the lock rides on. Sized deliberately:
    // the conformance cluster's `max_connections` is Postgres's default
    // 100, and a whole-workspace run is already close to it.
    let pool = Pool::connect(PoolConfig {
        size: 1,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool for the lock");
    ordo_host::apply_op::ApplyLock::take(&pool)
        .await
        .expect("taking the apply lock")
        .expect("the apply lock is free in this test")
}

const PROGRAM: &str = r#"
entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
"#;

async fn pool_on(db: &str) -> Pool {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("an empty database");
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool")
}

/// A resolver over a fixed list, so a test can state the fleet exactly.
///
/// Hand-built rather than an `AttachedPools`, because what is under test
/// is the **walk**, and `AttachedPools` would make the list a function of
/// `$spacedb` rows and drag attachment into a test about ordering. The
/// contract `apply_everywhere` relies on is `Target`'s own: a target
/// labelled `default` is the control database.
struct Fleet(Vec<Target>);

impl PoolResolver for Fleet {
    fn resolve<'a>(&'a self, _space: &'a ordo_crypto::SpaceId) -> ordo_exec::route::Resolving<'a> {
        Box::pin(async move {
            self.0
                .iter()
                .find_map(|t| t.pool.as_ref().ok().cloned())
                .ok_or(ordo_exec::PoolError::Saturated)
        })
    }

    fn targets<'a>(&'a self) -> Targets<'a> {
        Box::pin(async move {
            Ok(self
                .0
                .iter()
                .map(|t| Target {
                    label: t.label.clone(),
                    pool: t.pool.clone(),
                })
                .collect())
        })
    }
}

async fn table_exists(pool: &Pool, name: &str) -> bool {
    let rows = pool
        .rows(&format!("SELECT to_regclass('\"{name}\"') IS NOT NULL"))
        .await
        .expect("the existence query");
    matches!(
        rows.first().and_then(|r| r[0].as_deref()),
        Some("t") | Some("true")
    )
}

/// **Every target takes the program, and the control database is applied
/// last.**
///
/// The ordering is the half that is easy to get wrong and impossible to
/// notice: `PoolResolver::targets` lists the default **first**, which is
/// right for a reader and wrong for an apply. The control database holds
/// `$artifacts`, `$activations` and the fleet's own record, so applying
/// it first leaves the database that records the fleet's state ahead of
/// the fleet for the length of the walk. A process that died in that
/// window would recover against a control database claiming a version
/// some tenant had never received.
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_reaches_every_target_before_the_activation() {
    let control = pool_on("ordo_t7_everywhere_control").await;
    let tenant = pool_on("ordo_t7_everywhere_tenant").await;
    let fleet = Fleet(vec![
        // Deliberately in the order a real resolver emits: default first.
        Target {
            label: "default".into(),
            pool: Ok(control.clone()),
        },
        Target {
            label: "tenant-a".into(),
            pool: Ok(tenant.clone()),
        },
    ]);
    let program = Program::compile(PROGRAM).expect("the program compiles");

    // R-S5: the walk cannot be called without the routing lock, which is
    // the signature saying so. Nothing here contends for it; the point is
    // that a caller cannot forget it.
    let routing = routing_lock().await;
    let out = apply_everywhere(&fleet, &program, "tester", "test.apply", &routing)
        .await
        .expect("the walk runs");

    assert!(
        out.all_succeeded(),
        "every target should have taken it: {}",
        out.summary()
    );
    assert_eq!(
        out.applied
            .iter()
            .map(|t| t.label.as_str())
            .collect::<Vec<_>>(),
        vec!["tenant-a", "default"],
        "the control database must be applied last, so the record is \
         never ahead of the fleet it records: {}",
        out.summary()
    );
    assert!(
        table_exists(&tenant, "task").await,
        "the attached tenant's database did not receive the schema"
    );
    assert!(
        table_exists(&control, "task").await,
        "the control database did not receive the schema"
    );
}

/// **A target that cannot be reached holds back the activation, and does
/// not stop the walk.**
///
/// Two properties in one, and they pull in opposite directions. The walk
/// continues past a failure, because an operator repairing this needs to
/// know whether one target is broken or all of them, and a walk that
/// stopped at the first would make them find out one deploy at a time.
/// And `all_succeeded` is false, because `$artifacts` and `$activations`
/// are deployment-wide: a program activated while one target is behind is
/// a program whose tables that tenant's database does not have.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_that_fails_holds_back_the_activation() {
    let control = pool_on("ordo_t7_everywhere_partial").await;
    let fleet = Fleet(vec![
        Target {
            label: "default".into(),
            pool: Ok(control.clone()),
        },
        Target {
            label: "gone".into(),
            pool: Err("nothing listens there".into()),
        },
    ]);
    let program = Program::compile(PROGRAM).expect("the program compiles");

    // R-S5: the walk cannot be called without the routing lock, which is
    // the signature saying so. Nothing here contends for it; the point is
    // that a caller cannot forget it.
    let routing = routing_lock().await;
    let out = apply_everywhere(&fleet, &program, "tester", "test.apply", &routing)
        .await
        .expect("the walk runs");

    assert!(
        !out.all_succeeded(),
        "an unreachable target must hold the activation back: {}",
        out.summary()
    );
    assert_eq!(out.failures().len(), 1, "{}", out.summary());
    assert_eq!(out.failures()[0].label, "gone");
    // The walk kept going: the reachable target still holds the schema.
    // Nothing is rolled back and nothing can be, since no transaction
    // spans two databases. What protects the deployment is the activation
    // not landing, not the applies being undone.
    assert!(
        table_exists(&control, "task").await,
        "the walk stopped at the failure instead of continuing past it"
    );
}

/// **An unreachable target is a refusal, not an absence.**
///
/// `targets()` names a target it cannot reach rather than leaving it out,
/// and this is the same rule one layer up. A target silently missing from
/// an apply is a tenant whose database is a version behind with nothing
/// anywhere saying so, which is the failure the whole increment exists to
/// prevent.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_target_is_named_rather_than_skipped() {
    let control = pool_on("ordo_t7_everywhere_named").await;
    let fleet = Fleet(vec![
        Target {
            label: "default".into(),
            pool: Ok(control),
        },
        Target {
            label: "host:5432/gone".into(),
            pool: Err("connection refused".into()),
        },
    ]);
    let program = Program::compile(PROGRAM).expect("the program compiles");

    // R-S5: the walk cannot be called without the routing lock, which is
    // the signature saying so. Nothing here contends for it; the point is
    // that a caller cannot forget it.
    let routing = routing_lock().await;
    let out = apply_everywhere(&fleet, &program, "tester", "test.apply", &routing)
        .await
        .expect("the walk runs");

    assert_eq!(
        out.applied.len(),
        2,
        "the unreachable target must appear in the outcomes: {}",
        out.summary()
    );
    assert!(
        out.summary().contains("host:5432/gone"),
        "and be named in the summary an operator reads: {}",
        out.summary()
    );
}

// ── the walk, wired into the deploy door (T7 R-S3) ──────────────────────

/// **A tenant database that cannot be reached does not hold back the
/// activation, and the report names it** (25i, which changes R-S3's gate).
///
/// With one tenant per person, gating the activation on every database
/// would let one unreachable person stop every deploy for everybody. The
/// deployment's own database still gates it. A database the walk could not
/// reach is closed by the schema-behind quarantine, alone, and the catch-up
/// brings it forward when it answers.
///
/// This drives `apply_op::run_program`, the body `std.deployment.apply` and
/// `ordo apply` both call, against a fleet with one unreachable target.
/// The assertion is on `$activations` and on the report.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_target_does_not_hold_back_the_activation_and_is_reported() {
    let control = pool_on("ordo_t7_wired_partial").await;
    let fleet = Fleet(vec![
        Target {
            label: "default".into(),
            pool: Ok(control.clone()),
        },
        Target {
            label: "gone".into(),
            pool: Err("nothing listens there".into()),
        },
    ]);
    let program = Program::compile(PROGRAM).expect("the program compiles");

    let report = ordo_host::apply_op::run_program(
        &control,
        &fleet,
        &program,
        vec![],
        "tester",
        "test.apply",
        true,
    )
    .await
    .expect("one unreachable tenant database must not fail the apply");

    let active = control
        .rows(r#"SELECT count(*) FROM "$activations""#)
        .await
        .expect("the activations table");
    assert_eq!(
        active.first().and_then(|r| r[0].as_deref()),
        Some("1"),
        "the deployment's own database took the version, so it activates"
    );
    assert_eq!(report.tenants.advanced, 0);
    assert_eq!(report.tenants.held.len(), 1, "{report:?}");
    let (target, why) = &report.tenants.held[0];
    assert_eq!(target, "gone");
    assert!(why.contains("could not be reached"), "{why}");
}

/// **A tenant database that refuses the change is named and does not stop
/// the others.** One database holds rows a new required field cannot be
/// added over; the other is empty and takes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_tenant_is_held_and_the_others_are_brought_forward() {
    let control = pool_on("ordo_t7_wired_refused_control").await;
    let full = pool_on("ordo_t7_wired_refused_full").await;
    let empty = pool_on("ordo_t7_wired_refused_empty").await;
    let fleet = Fleet(vec![
        Target {
            label: "default".into(),
            pool: Ok(control.clone()),
        },
        Target {
            label: "host:5432/full".into(),
            pool: Ok(full.clone()),
        },
        Target {
            label: "host:5432/empty".into(),
            pool: Ok(empty.clone()),
        },
    ]);
    let v1 = Program::compile(PROGRAM).expect("v1 compiles");
    ordo_host::apply_op::run_program(&control, &fleet, &v1, vec![], "t", "t", true)
        .await
        .expect("v1 applies everywhere");
    full.rows(
        r#"INSERT INTO "task" ("$space", "id", "rev", "title", "done") VALUES
           ('s', 't_1', 1, 'a', false), ('s', 't_2', 1, 'a', false)"#,
    )
    .await
    .expect("two rows with one title in the full database");

    // A unique title over duplicate titles is refused by its probe.
    let v2 = Program::compile(
        "entity task {\n  title: text unique\n  done:  bool\n}\n\nop openTasks unbounded =\n  task[title, done]\n",
    )
    .expect("v2 compiles");
    // The deployment's own database is empty, so it takes the change first,
    // as `ordo migrate --apply` does before an `ordo apply`.
    let (_, applied) = ordo_host::schema_apply::apply(&control, &v2, "t", "t").await;
    applied.expect("the empty control database takes the unique title");
    let report = ordo_host::apply_op::run_program(&control, &fleet, &v2, vec![], "t", "t", true)
        .await
        .expect("the apply answers");
    assert_eq!(report.tenants.advanced, 1, "{report:?}");
    assert_eq!(report.tenants.held.len(), 1, "{report:?}");
    assert_eq!(report.tenants.held[0].0, "host:5432/full");
    assert!(
        report.tenants.held[0].1.starts_with("plan_refused")
            || report.tenants.held[0].1.starts_with("not_class_a"),
        "refused by name: {:?}",
        report.tenants.held
    );
    // The refused database is untouched and still records the old version.
    assert_eq!(
        ordo_exec::spacedb::recorded_version(&full)
            .await
            .expect("v"),
        ordo_exec::spacedb::TargetVersion::At(v1.version.clone())
    );
    assert_eq!(
        ordo_exec::spacedb::recorded_version(&empty)
            .await
            .expect("v"),
        ordo_exec::spacedb::TargetVersion::At(v2.version.clone())
    );
}

/// **A fleet that is all reachable does activate**, so the guard above is
/// not passing for the trivial reason that nothing ever activates.
#[tokio::test(flavor = "multi_thread")]
async fn a_reachable_fleet_activates() {
    let control = pool_on("ordo_t7_wired_ok_control").await;
    let tenant = pool_on("ordo_t7_wired_ok_tenant").await;
    let fleet = Fleet(vec![
        Target {
            label: "default".into(),
            pool: Ok(control.clone()),
        },
        Target {
            label: "tenant-a".into(),
            pool: Ok(tenant.clone()),
        },
    ]);
    let program = Program::compile(PROGRAM).expect("the program compiles");

    ordo_host::apply_op::run_program(
        &control,
        &fleet,
        &program,
        vec![],
        "tester",
        "test.apply",
        true,
    )
    .await
    .expect("a reachable fleet applies");

    let active = control
        .rows(r#"SELECT count(*) FROM "$activations""#)
        .await
        .expect("the activations table");
    assert_eq!(
        active.first().and_then(|r| r[0].as_deref()),
        Some("1"),
        "a fleet that all took the program must activate"
    );
    assert!(
        table_exists(&tenant, "task").await,
        "and the attached target must hold the schema"
    );
}

/// **Each attached target's confirmed version comes from that target's own
/// `$migrations`** (18d R-S2).
///
/// The property the fleet view rests on: it is *"a cache of the targets' own
/// rows and is never the authority."* Asserted per target against the target
/// itself rather than against the program's version, because those two
/// agreeing is what is being claimed, not assumed.
///
/// This exists because the first falsification of the fleet view missed:
/// planting a fake version in the walk's observation failed no test, since
/// the only test then covering it had one target and that target's row was
/// read by a different code path. A cache with no multi-target test is a
/// cache tested where it cannot be wrong.
#[tokio::test(flavor = "multi_thread")]
async fn each_target_reports_its_own_version_to_the_walk() {
    let control = pool_on("ordo_t7_confirm_control").await;
    let a = pool_on("ordo_t7_confirm_a").await;
    let b = pool_on("ordo_t7_confirm_b").await;
    let fleet = Fleet(vec![
        Target {
            label: "default".into(),
            pool: Ok(control.clone()),
        },
        Target {
            label: "host:5432/a".into(),
            pool: Ok(a.clone()),
        },
        Target {
            label: "host:5432/b".into(),
            pool: Ok(b.clone()),
        },
    ]);
    let program = Program::compile(PROGRAM).expect("the program compiles");
    let routing = routing_lock().await;

    let out = apply_everywhere(&fleet, &program, "tester", "test.apply", &routing)
        .await
        .expect("the walk");
    assert!(out.all_succeeded(), "{out:?}");

    for target in &out.applied {
        let pool = match target.label.as_str() {
            "default" => &control,
            "host:5432/a" => &a,
            "host:5432/b" => &b,
            other => panic!("unexpected target {other}"),
        };
        let authority = ordo_exec::spacedb::recorded_version(pool)
            .await
            .expect("the target's own row");
        let expected = match authority {
            ordo_exec::spacedb::TargetVersion::At(v) => v,
            other => panic!("`{}` should know its version: {other:?}", target.label),
        };
        assert_eq!(
            target.confirmed.as_deref(),
            Some(expected.as_str()),
            "`{}` reported a version that is not the one in its own `$migrations`",
            target.label
        );
    }

    // And the three are not all the same string by accident of every
    // database being empty: they each recorded something.
    assert_eq!(out.applied.len(), 3, "{out:?}");
    assert!(
        out.applied.iter().all(|t| t.confirmed.is_some()),
        "a reached target reported nothing: {out:?}"
    );

    // **The same claim for `apply_everywhere_except_default`**, which is the
    // one `ApplyOps::sequence` actually calls. The two walks are near
    // duplicates, so a property asserted only on this one is a property the
    // production path does not have — and the first attempt to falsify this
    // test patched that copy and failed to fail, which is how the gap was
    // found.
    let skipping = apply_everywhere_except_default(
        &fleet,
        &control,
        &program,
        "tester",
        "test.apply",
        &routing,
    )
    .await
    .expect("the walk that skips the default");
    assert!(
        skipping.applied.iter().all(|t| t.label != "default"),
        "the default target was walked: {skipping:?}"
    );
    for target in &skipping.applied {
        let pool = match target.label.as_str() {
            "host:5432/a" => &a,
            "host:5432/b" => &b,
            other => panic!("unexpected target {other}"),
        };
        let expected = match ordo_exec::spacedb::recorded_version(pool)
            .await
            .expect("row")
        {
            ordo_exec::spacedb::TargetVersion::At(v) => v,
            other => panic!("`{}` should know its version: {other:?}", target.label),
        };
        assert_eq!(
            target.confirmed.as_deref(),
            Some(expected.as_str()),
            "`{}` reported a version that is not the one in its own `$migrations`",
            target.label
        );
    }
}
