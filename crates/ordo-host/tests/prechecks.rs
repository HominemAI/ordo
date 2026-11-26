//! Pre-checks on every tenant (`plan-versions.md` §2.3, increment 9; 27w),
//! against real Postgres:
//!
//! - a plan over 3 tenant databases, 2 of them bad, lists both issues with
//!   their fixes, from every database at once; `--run` refuses while they
//!   are open, and runs once the data is fixed;
//! - a shared database names each bad space in one issue;
//! - a tenant's own step (replay) refuses with the same issues and fixes;
//! - a version that removes a sign-in method some people rely on stops
//!   `ordo evolve`, naming the count and the migration steps; it goes on
//!   only with the deployment owner's approval of that count, recorded on
//!   the stream (ruled 2026-11-23, Q6).
//!
//! A regression looks like a pre-check that stops at the first bad
//! database, an issue with no fix, a run over data the new shape refuses,
//! or a removed sign-in method stored without the owner's approval.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::Pg;
use ordo_crypto::SpaceId;
use ordo_exec::route::{PoolResolver, SinglePool, Target, Targets};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::deploy_ops;
use ordo_host::evolve::{self, EvolveRefusal};
use ordo_host::precheck;
use ordo_host::principal_directory as pd;
use ordo_host::schema_apply::{self as sa, Choices, Program};
use ordo_host::versions::Versions;

const OWNER: &str = "deployment-owner:ada";

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        size: 3,
        ..PoolConfig::from_env()
    }
}

async fn empty(stem: &str) -> (Pg, Pool) {
    let pg = Pg::scoped(stem);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(cfg(&pg.db)).await.expect("a pool");
    (pg, pool)
}

async fn store(pool: &Pool, source: &str) -> Program {
    let p = Program::compile(source).expect("compiles");
    Versions::on(pool).ensure().await.expect("registry");
    ordo_exec::registry::store(
        pool,
        &ordo_exec::registry::Artifact {
            version: p.version.clone(),
            artifact: ordo_plan::artifact::to_bytes(&p.doc),
            source: p.source.clone(),
            ddl: p.ddl.clone(),
            applied_at: ordo_commandlog::Timestamp(0),
            actor: "dev:ada".into(),
        },
    )
    .await
    .expect("stored");
    p
}

async fn create(pool: &Pool, p: &Program) {
    let (_, created) = sa::apply(pool, p, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

fn plans_count(pg: &Pg) -> String {
    if one(pg, "SELECT to_regclass('\"$plans\"') IS NULL") == "t" {
        return "0".into();
    }
    one(pg, "SELECT count(*) FROM \"$plans\"")
}

/// 1.0.0 has notes with a title and a rank. 1.1.0 makes the title unique
/// and bounds the rank: 2 constraints existing data may break.
fn notes(version: &str, rules: &str) -> String {
    format!(
        "domain notes @ {version}\n\nentity note {{\n  title: text{}\n  rank: int\n{}}}\n\n\
         op allNotes unbounded =\n  note[title, rank]\n",
        if rules.is_empty() { "" } else { " unique" },
        rules
    )
}

fn add(pg: &Pg, space: &str, id: &str, title: &str, rank: i64) {
    pg.exec(&format!(
        "INSERT INTO note (\"$space\", id, rev, title, rank) \
         VALUES ('{space}', '{id}', 1, '{title}', {rank})"
    ))
    .expect("a row");
}

/// The deployment's own database, and one database per tenant.
struct PerTenant {
    control: Pool,
    tenants: Vec<(String, Pool)>,
}

impl PoolResolver for PerTenant {
    fn resolve<'a>(&'a self, space: &'a SpaceId) -> ordo_exec::route::Resolving<'a> {
        Box::pin(async move {
            Ok(self
                .tenants
                .iter()
                .find(|(n, _)| *n == space.0)
                .map(|(_, p)| p.clone())
                .unwrap_or_else(|| self.control.clone()))
        })
    }

    fn targets<'a>(&'a self) -> Targets<'a> {
        Box::pin(async move {
            let mut out = vec![Target {
                label: "default".into(),
                pool: Ok(self.control.clone()),
            }];
            out.extend(self.tenants.iter().map(|(n, p)| Target {
                label: n.clone(),
                pool: Ok(p.clone()),
            }));
            Ok(out)
        })
    }
}

async fn run(
    ops: &apply_op::ApplyOps,
    name: &str,
    v: serde_json::Value,
) -> Result<ordo_api::ExecResult, ordo_api::ExecError> {
    let b = BoundParams(v.as_object().unwrap().clone());
    deploy_ops::execute(ops, name, "home", &b, OWNER).await
}

/// **A plan over 3 tenants with 2 bad lists both issues with their
/// fixes.** acme holds 2 notes with one title, under a new `unique`;
/// globex a rank over the new bound; initech is clean. Every database is
/// asked, and the answer names each tenant's space and the fix. The door's
/// `--run` refuses while they are open, and runs once the data is fixed.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_over_three_tenants_with_two_bad_lists_both_issues() {
    let (_reg_pg, registry) = empty("ordo_prechecks_reg").await;
    let v1 = store(&registry, &notes("1.0.0", "")).await;
    store(&registry, &notes("1.1.0", "  check rank <= 10\n")).await;
    create(&registry, &v1).await;
    let mut pgs = Vec::new();
    let mut tenants = Vec::new();
    for name in ["acme", "globex", "initech"] {
        let (pg, pool) = empty(&format!("ordo_prechecks_{name}")).await;
        create(&pool, &v1).await;
        add(&pg, name, "n1", "first", 1);
        pgs.push(pg);
        tenants.push((name.to_string(), pool));
    }
    add(&pgs[0], "acme", "n2", "first", 2);
    add(&pgs[1], "globex", "n2", "second", 50);

    let resolver = PerTenant {
        control: registry.clone(),
        tenants: tenants.clone(),
    };
    let done = evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect("the plan is valid, whatever the data holds");
    let found = precheck::precheck(&registry, &resolver, &done.plan)
        .await
        .expect("checks");
    assert_eq!(
        found.checked,
        vec!["default", "acme", "globex", "initech"],
        "every database is asked"
    );
    assert_eq!(found.issues.len(), 2, "{:#?}", found.issues);
    let acme = &found.issues[0];
    assert_eq!(acme.database, "acme");
    assert_eq!(acme.kind, "data");
    assert!(acme.what.contains("duplicates"), "{acme:?}");
    assert!(
        acme.lines.iter().any(|l| l.contains("acme")),
        "{:?}",
        acme.lines
    );
    assert!(acme.fix.contains("distinct"), "{}", acme.fix);
    assert!(acme.fix.contains("`unique`"), "{}", acme.fix);
    let globex = &found.issues[1];
    assert_eq!(globex.database, "globex");
    assert!(globex.what.contains("check rank<=10"), "{globex:?}");
    assert!(
        globex.lines.iter().any(|l| l.contains("globex")),
        "{:?}",
        globex.lines
    );
    assert!(globex.fix.contains("loosen the check"), "{}", globex.fix);
    assert_eq!(found.not_run, vec![precheck::REQUIRES_NOT_BUILT]);

    // A tenant's own step refuses with the same issue and its fix.
    let err = evolve::run_step(&registry, &tenants[0].1, "acme", &done.plan, OWNER)
        .await
        .expect_err("acme's data breaks the step");
    assert_eq!(err.class(), "precheck_failed");
    let text = err.to_string();
    assert!(text.contains("database `acme`"), "{text}");
    assert!(text.contains("Fix: make"), "{text}");

    // The door: stored, both issues listed, and `--run` refused.
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = apply_op::ApplyOps::recording(
        registry.clone(),
        Arc::new(PerTenant {
            control: registry.clone(),
            tenants: tenants.clone(),
        }),
        "home",
        Some(stream.clone()),
    );
    let listed = run(
        &ops,
        deploy_ops::EVOLVE,
        serde_json::json!({ "from": "1.0.0", "to": "1.1.0" }),
    )
    .await
    .expect("stores and reports");
    let row = &listed.rows[0];
    assert_eq!(row["prechecks"]["ok"], false, "{row}");
    assert_eq!(
        row["prechecks"]["issues"].as_array().unwrap().len(),
        2,
        "{row}"
    );
    let err = run(
        &ops,
        deploy_ops::EVOLVE,
        serde_json::json!({ "from": "1.0.0", "to": "1.1.0", "run": true }),
    )
    .await
    .expect_err("a run over bad data is refused");
    assert_eq!(err.code, ErrorCode::ApplyRefused, "{err:?}");
    assert!(err.message.contains("precheck_failed"), "{}", err.message);
    assert!(err.message.contains("database `acme`"), "{}", err.message);
    assert!(err.message.contains("database `globex`"), "{}", err.message);
    assert_eq!(
        one(
            &_reg_pg,
            "SELECT count(*) FROM pg_constraint WHERE conname LIKE '%rank%'"
        ),
        "0",
        "the refused run changed the default database"
    );

    // Fixed, the same step runs.
    pgs[0]
        .exec("UPDATE note SET title = 'second' WHERE id = 'n2'")
        .expect("acme fixed");
    pgs[1]
        .exec("UPDATE note SET rank = 5 WHERE id = 'n2'")
        .expect("globex fixed");
    let ran = run(
        &ops,
        deploy_ops::EVOLVE,
        serde_json::json!({ "from": "1.0.0", "to": "1.1.0", "run": true }),
    )
    .await
    .expect("runs once the data is fixed");
    let row = &ran.rows[0];
    assert_eq!(row["prechecks"]["ok"], true, "{row}");
    assert!(!row["ran"].is_null(), "{row}");
}

/// **A shared database names each bad space.** 3 tenants share one
/// database; 2 hold duplicates. One issue, and its lines name both.
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_database_names_each_bad_space() {
    let (pg, pool) = empty("ordo_prechecks_shared").await;
    let v1 = store(&pool, &notes("1.0.0", "")).await;
    store(&pool, &notes("1.1.0", "  check rank <= 10\n")).await;
    create(&pool, &v1).await;
    for space in ["acme", "globex", "initech"] {
        add(&pg, space, &format!("{space}-1"), "first", 1);
    }
    add(&pg, "acme", "acme-2", "first", 1);
    add(&pg, "globex", "globex-2", "first", 1);
    let done = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect("stores");
    let found = precheck::precheck(&pool, &SinglePool::new(pool.clone()), &done.plan)
        .await
        .expect("checks");
    assert_eq!(found.issues.len(), 1, "{:#?}", found.issues);
    let lines = found.issues[0].lines.join("\n");
    assert!(lines.contains("acme"), "{lines}");
    assert!(lines.contains("globex"), "{lines}");
    assert!(!lines.contains("initech"), "{lines}");
}

/// The deployment's sign-in tables, with 4 people: one with only a
/// password, one with a password and a passkey, one with only a password
/// who deleted the account, one whose only password recovery retired.
fn people(pg: &Pg) {
    pg.exec(pd::PRINCIPAL_DDL).expect("$principal");
    pg.exec(pd::PRINCIPAL_DELETED_DDL).expect("deleted_at");
    pg.exec(pd::AUTHENTICATOR_DDL).expect("$authenticator");
    for ddl in pd::AUTHENTICATOR_RECOVERY_DDL {
        pg.exec(ddl).expect("recovery columns");
    }
    pg.exec(
        "INSERT INTO \"$principal\" (id, kind, display, deleted_at) VALUES \
         ('p1', 'person', 'one', 0), ('p2', 'person', 'two', 0), \
         ('p3', 'person', 'three', 99), ('p4', 'person', 'four', 0); \
         INSERT INTO \"$authenticator\" (authenticator, principal, retired_case) VALUES \
         ('password:one', 'p1', ''), ('password:two', 'p2', ''), ('passkey:AAA', 'p2', ''), \
         ('password:three', 'p3', ''), ('password:four', 'p4', 'case-1'), \
         ('passkey:BBB', 'p4', '')",
    )
    .expect("people");
}

fn with_auth(version: &str, methods: &str) -> String {
    format!(
        "domain notes @ {version}\n\nauth {{\n  methods {methods}\n}}\n\n\
         entity note {{\n  title: text\n}}\n\nop allNotes unbounded =\n  note[title]\n"
    )
}

/// **A removed sign-in method stops `ordo evolve` until approved.** 1.1.0
/// drops `password`; 1 person (`p1`) is left with no method. The refusal
/// gives the count and the migration steps and stores nothing. An approval
/// naming another count is refused; one naming 1 stores the plan and the
/// approval is on the stream.
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_sign_in_method_stops_until_approved() {
    let (pg, pool) = empty("ordo_prechecks_signin").await;
    people(&pg);
    store(&pool, &with_auth("1.0.0", "passkey, password")).await;
    store(&pool, &with_auth("1.1.0", "passkey")).await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = apply_op::ApplyOps::recording(
        pool.clone(),
        Arc::new(SinglePool::new(pool.clone())),
        "home",
        Some(stream.clone()),
    );

    let err = run(
        &ops,
        deploy_ops::EVOLVE,
        serde_json::json!({ "from": "1.0.0", "to": "1.1.0" }),
    )
    .await
    .expect_err("stops on the warning");
    assert_eq!(err.code, ErrorCode::ApplyRefused, "{err:?}");
    let m = &err.message;
    assert!(m.contains("sign_in_removal_unapproved"), "{m}");
    assert!(m.contains("`password`"), "{m}");
    assert!(m.contains("1 person(s)"), "{m}");
    assert!(m.contains("add a method"), "{m}");
    assert!(m.contains("one more version"), "{m}");
    assert!(m.contains("--approve-sign-in-removal 1"), "{m}");
    assert_eq!(plans_count(&pg), "0", "stored without approval");

    // The library call refuses the same way.
    let lib = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect_err("refused");
    assert!(
        matches!(lib, EvolveRefusal::SignInRemoval { ref removal, .. } if removal.people == 1),
        "{lib:?}"
    );

    let err = run(
        &ops,
        deploy_ops::EVOLVE,
        serde_json::json!({ "from": "1.0.0", "to": "1.1.0", "approveSignInRemoval": "2" }),
    )
    .await
    .expect_err("an approval of another count");
    assert!(
        err.message.contains("The approval named 2"),
        "{}",
        err.message
    );
    let err = run(
        &ops,
        deploy_ops::EVOLVE,
        serde_json::json!({ "from": "1.0.0", "to": "1.1.0", "approveSignInRemoval": "some" }),
    )
    .await
    .expect_err("not a count");
    assert_eq!(err.code, ErrorCode::InvalidParam, "{err:?}");
    assert_eq!(plans_count(&pg), "0");

    let ok = run(
        &ops,
        deploy_ops::EVOLVE,
        serde_json::json!({ "from": "1.0.0", "to": "1.1.0", "approveSignInRemoval": "1" }),
    )
    .await
    .expect("approved");
    let row = &ok.rows[0];
    assert_eq!(row["stored"], true, "{row}");
    assert_eq!(row["signIn"]["people"], 1, "{row}");
    assert_eq!(plans_count(&pg), "1");
    let approvals: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .filter(|a| a == "sign-in-removal-approved")
        .collect();
    assert_eq!(approvals.len(), 1, "the approval is not on the stream");

    // Once p1 adds a passkey, nobody is left without a method, and the
    // step goes on with no approval.
    pg.exec(
        "INSERT INTO \"$authenticator\" (authenticator, principal) VALUES ('passkey:CCC', 'p1')",
    )
    .expect("p1 adds a passkey");
    let clean = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect("nobody is stranded");
    assert_eq!(clean.sign_in.as_ref().map(|s| s.people), Some(0));
}
