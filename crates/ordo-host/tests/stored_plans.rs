//! Stored plans (`plan-versions.md` §2.3, increment 8), against real
//! Postgres:
//!
//! - a plan rendered in dev renders to the same digest in another
//!   environment, whose live rows differ; a different option there refuses;
//! - a plan with errors is refused and never stored, naming the fix;
//! - the receipt lists each change, its class, its option, forward and
//!   reverse, and carries no statement on Ordo's own tables;
//! - a tenant 3 behind replays 3 stored steps, each recorded, and never a
//!   diff of its version against the last;
//! - the stale check: a database at another version than a step's from, a
//!   database that lacks part of it, a stored digest that no longer renders;
//! - a missing step refuses naming the command that renders it;
//! - the door: `std.deployment.evolve` stores and records, `plans` lists,
//!   `replay` moves a tenant's database and its pin;
//! - F1: a removed field of a table on a `reader` line lists the reader.
//!
//! A regression looks like a digest that moves between environments, a
//! plan with errors in `$plans`, a replay that skips a step, or a step run
//! on a database at another version.
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
use ordo_host::evolve::{self, EvolveRefusal, Plans, StepRun};
use ordo_host::schema_apply::{self as sa, Choices, Program};
use ordo_host::tenants;
use ordo_host::versions::Versions;

const OWNER: &str = "deployment-owner:ada";

/// The notes program at `version`, with `extra` declared after `body`.
fn notes(version: &str, extra: &str) -> String {
    format!(
        "domain notes @ {version}\n\nentity note {{\n  title: text\n  body: text?\n{extra}}}\n\n\
         op allNotes unbounded =\n  note[title]\n"
    )
}

/// 4 versions, each adding one optional field.
fn chain() -> Vec<String> {
    vec![
        notes("1.0.0", ""),
        notes("1.1.0", "  pinned: bool?\n"),
        notes("1.2.0", "  pinned: bool?\n  colour: text?\n"),
        notes("1.3.0", "  pinned: bool?\n  colour: text?\n  rank: int?\n"),
    ]
}

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

/// Store a version in the registry, as `ordo apply --no-activate` leaves
/// it, without touching the app's tables.
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

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

/// How many plans are stored: none when `$plans` was never made.
fn plans_count(pg: &Pg) -> String {
    if one(pg, "SELECT to_regclass('\"$plans\"') IS NULL") == "t" {
        return "0".into();
    }
    one(pg, "SELECT count(*) FROM \"$plans\"")
}

fn places(p: &str) -> Choices {
    evolve::parse_options(p).expect("options parse")
}

/// **A plan re-rendered in another environment matches dev's digest.**
/// Staging's live tables hold rows dev has never seen, and its versions
/// arrived in another order; only the two stored sources and the options
/// decide the digest.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_re_rendered_in_staging_matches_devs_digest() {
    let (_dev_pg, dev) = empty("ordo_plans_dev").await;
    let (staging_pg, staging) = empty("ordo_plans_staging").await;
    let c = chain();
    for s in &c[..2] {
        store(&dev, s).await;
    }
    // Staging holds the live table, with rows, and stored the versions the
    // other way round.
    let v1 = store(&staging, &c[1]).await;
    store(&staging, &c[0]).await;
    let (_, created) = sa::apply(&staging, &v1, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    staging_pg
        .exec("INSERT INTO note (\"$space\", id, rev, title) VALUES ('s1', 'n1', 1, 'kept')")
        .expect("a row");

    let at = 1_800_000_000;
    let in_dev = evolve::evolve(&dev, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, at)
        .await
        .expect("dev renders and stores");
    assert!(in_dev.stored);
    assert!(in_dev.rendered.digest.starts_with("sha256:"));

    let in_staging = evolve::evolve(
        &staging,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        Some(&in_dev.rendered.digest),
        OWNER,
        at + 3_600,
    )
    .await
    .expect("staging renders the same plan");
    assert_eq!(in_staging.rendered.digest, in_dev.rendered.digest);
    assert_eq!(in_staging.rendered.receipt, in_dev.rendered.receipt);

    // The same step again stores nothing new.
    let again = evolve::evolve(&dev, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, at)
        .await
        .expect("renders again");
    assert!(!again.stored, "the same plan was stored twice");
}

/// **A digest that does not match refuses, and stores nothing.** The
/// refusal names both digests and the command that renders the step.
#[tokio::test(flavor = "multi_thread")]
async fn a_digest_that_does_not_match_is_refused() {
    let (pg, pool) = empty("ordo_plans_mismatch").await;
    let c = chain();
    store(&pool, &c[0]).await;
    store(&pool, &c[1]).await;
    let err = evolve::evolve(
        &pool,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        Some("sha256:00"),
        OWNER,
        1,
    )
    .await
    .expect_err("a digest from elsewhere is refused");
    assert!(
        matches!(err, EvolveRefusal::DigestMismatch { .. }),
        "{err:?}"
    );
    let text = err.to_string();
    assert!(text.contains("sha256:00"), "{text}");
    assert!(
        text.contains("ordo evolve --from 1.0.0 --to 1.1.0"),
        "{text}"
    );
    assert_eq!(plans_count(&pg), "0", "a refused plan was stored");
    assert_eq!(
        err.to_exec_error("the plan was not stored").code,
        ErrorCode::ApplyRefused
    );
}

/// **A plan with errors is refused and never stored**: a retype the menu
/// does not offer, and a change of places with no choice. Each names its
/// fix. With the choice stated, the plan is stored and its receipt records
/// the option.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_with_errors_is_never_stored() {
    let (pg, pool) = empty("ordo_plans_errors").await;
    let money = |v: &str, p: &str| {
        format!(
            "domain shop @ {v}\n\nentity invoice {{\n  label: text\n  total: money({p})\n}}\n\n\
             op allInvoices unbounded =\n  invoice[label, total]\n"
        )
    };
    store(&pool, &money("1.0.0", "0")).await;
    store(&pool, &money("1.1.0", "2")).await;
    store(
        &pool,
        "domain shop @ 1.2.0\n\nentity invoice {\n  label: int\n  total: money(2)\n}\n\n\
         op allInvoices unbounded =\n  invoice[label, total]\n",
    )
    .await;

    let unstated = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect_err("a change of places needs a choice");
    let text = unstated.to_string();
    assert!(text.contains("plan_has_errors"), "{text}");
    assert!(text.contains("--places invoice.total=rescale"), "{text}");

    let retype = evolve::evolve(&pool, "1.1.0", "1.2.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect_err("a retype is not on the menu yet");
    assert!(matches!(retype, EvolveRefusal::Errors { .. }), "{retype:?}");
    assert_eq!(plans_count(&pg), "0", "a plan with errors was stored");

    // An option for a change the step lacks is refused too.
    let stray = evolve::evolve(
        &pool,
        "1.0.0",
        "1.1.0",
        &places("invoice.total=rescale,invoice.label=keep"),
        None,
        OWNER,
        1,
    )
    .await
    .expect_err("an option naming no change is refused");
    assert!(stray.to_string().contains("invoice.label"), "{stray}");

    let stated = evolve::evolve(
        &pool,
        "1.0.0",
        "1.1.0",
        &places("invoice.total=rescale"),
        None,
        OWNER,
        1,
    )
    .await
    .expect("stated, it stores");
    let change = &stated.rendered.receipt["changes"][0];
    assert_eq!(change["option"], "rescale", "{change}");
    assert_eq!(change["class"], "B", "{change}");
    // Ordo's own tables are written at run time, never stored.
    let receipt = ordo_plan::artifact::to_bytes(&stated.rendered.receipt);
    assert!(!receipt.contains("$places"), "{receipt}");
    assert!(
        receipt.contains("UPDATE"),
        "the rescale is in the receipt: {receipt}"
    );
    // A different option is a different plan.
    let keep = evolve::render(&pool, "1.0.0", "1.1.0", &places("invoice.total=keep"))
        .await
        .expect("renders");
    assert_ne!(keep.digest, stated.rendered.digest);
}

/// **The receipt lists forward and reverse.** An added field's reverse
/// drops its column; a removed field is orphaned, with no statement.
#[tokio::test(flavor = "multi_thread")]
async fn the_receipt_lists_forward_and_reverse() {
    let (_pg, pool) = empty("ordo_plans_receipt").await;
    let c = chain();
    store(&pool, &c[0]).await;
    store(&pool, &c[1]).await;
    store(&pool, &notes("1.4.0", "")).await;
    let r = evolve::render(&pool, "1.0.0", "1.1.0", &Choices::default())
        .await
        .expect("renders");
    let lines = evolve::receipt_lines(&r.receipt).join("\n");
    assert!(lines.contains("A2"), "{lines}");
    assert!(
        lines.contains("reverse: ALTER TABLE \"note\" DROP COLUMN \"pinned\";"),
        "{lines}"
    );
    assert!(
        lines.contains("forward: ALTER TABLE \"note\" ADD COLUMN \"pinned\""),
        "{lines}"
    );

    let removed = evolve::render(&pool, "1.1.0", "1.4.0", &Choices::default())
        .await
        .expect("a removed field renders");
    let change = &removed.receipt["changes"][0];
    assert_eq!(change["orphaned"], true, "{change}");
    assert_eq!(removed.receipt["forward"], serde_json::json!([]));
}

/// The registry, holding the 4 versions of [`chain`] and the 3 stored
/// steps between them; and a tenant's database created at the first.
async fn registry_and_tenant(stem: &str) -> (Pg, Pool, Pg, Pool) {
    let (reg_pg, registry) = empty(&format!("{stem}_reg")).await;
    let (tenant_pg, tenant) = empty(&format!("{stem}_tenant")).await;
    let c = chain();
    let mut programs = Vec::new();
    for s in &c {
        programs.push(store(&registry, s).await);
    }
    for w in ["1.0.0", "1.1.0", "1.2.0", "1.3.0"].windows(2) {
        evolve::evolve(&registry, w[0], w[1], &Choices::default(), None, OWNER, 1)
            .await
            .expect("each step stores");
    }
    let (_, created) = sa::apply(&tenant, &programs[0], "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    tenant_pg
        .exec("INSERT INTO note (\"$space\", id, rev, title) VALUES ('acme', 'n1', 1, 'first')")
        .expect("a row");
    (reg_pg, registry, tenant_pg, tenant)
}

/// **A tenant 3 behind replays 3 steps**, each its own stored plan, each
/// recorded in the tenant's `$migrations` with its from and to. The row
/// written at 1.0.0 is still there.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_three_behind_replays_three_steps() {
    let (_reg_pg, registry, tenant_pg, tenant) = registry_and_tenant("ordo_plans_replay").await;
    let done = evolve::replay(&registry, &tenant, "acme", "1.3.0", OWNER)
        .await
        .expect("replays");
    assert_eq!(done.from, "1.0.0");
    let walked: Vec<(String, String)> = done
        .steps
        .iter()
        .map(|(p, _)| (p.from.clone(), p.to.clone()))
        .collect();
    assert_eq!(
        walked,
        vec![
            ("1.0.0".into(), "1.1.0".into()),
            ("1.1.0".into(), "1.2.0".into()),
            ("1.2.0".into(), "1.3.0".into()),
        ]
    );
    assert!(done
        .steps
        .iter()
        .all(|(_, r)| matches!(r, StepRun::Ran { statements, .. } if *statements > 0)));
    let recorded = tenant_pg
        .query(
            "SELECT from_version, to_version FROM \"$migrations\" \
             WHERE phase <> 'failed' ORDER BY seq",
        )
        .expect("the record");
    let pairs: Vec<(String, String)> = recorded
        .iter()
        .map(|r| {
            (
                r[0].clone().unwrap_or_default(),
                r[1].clone().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        &pairs[pairs.len() - 3..],
        &[
            ("1.0.0".to_string(), "1.1.0".to_string()),
            ("1.1.0".to_string(), "1.2.0".to_string()),
            ("1.2.0".to_string(), "1.3.0".to_string()),
        ],
        "{pairs:?}"
    );
    assert_eq!(
        one(&tenant_pg, "SELECT title FROM note WHERE id = 'n1'"),
        "first"
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name IN ('pinned', 'colour', 'rank')"
        ),
        "3"
    );
    // Replaying again finds nothing to do.
    let again = evolve::replay(&registry, &tenant, "acme", "1.3.0", OWNER)
        .await
        .expect("replays again");
    assert!(again.steps.is_empty(), "{:?}", again.steps);
}

/// **The stale check runs per database, at run time.** A step whose
/// from-version the database does not record is refused naming both; a
/// database that lacks part of the from-version is refused as drifted;
/// nothing runs in either.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_or_drifted_database_is_refused() {
    let (_reg_pg, registry, tenant_pg, tenant) = registry_and_tenant("ordo_plans_stale").await;
    let second = Plans::on(&registry)
        .latest("1.1.0", "1.2.0")
        .await
        .expect("reads")
        .expect("stored");
    let err = evolve::run_step(&registry, &tenant, "acme", &second, OWNER)
        .await
        .expect_err("the tenant is at 1.0.0");
    assert!(matches!(err, EvolveRefusal::Stale { .. }), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("records version `1.0.0`"), "{text}");
    assert!(text.contains("starts from `1.1.0`"), "{text}");
    assert!(text.contains("ordo evolve --tenant"), "{text}");

    // Drift: the tenant records 1.0.0 and lacks its `body` column.
    tenant_pg
        .exec("ALTER TABLE note DROP COLUMN body")
        .expect("drift");
    let first = Plans::on(&registry)
        .latest("1.0.0", "1.1.0")
        .await
        .expect("reads")
        .expect("stored");
    let err = evolve::run_step(&registry, &tenant, "acme", &first, OWNER)
        .await
        .expect_err("a drifted database is refused");
    assert!(matches!(err, EvolveRefusal::Drifted { .. }), "{err:?}");
    assert!(err.to_string().contains("body"), "{err}");
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = 'pinned'"
        ),
        "0",
        "a refused step ran"
    );
}

/// **A stored digest this release does not render is refused at run
/// time**, and a missing step refuses the replay naming the command that
/// renders it.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_digest_or_a_missing_step_is_refused() {
    let (reg_pg, registry, _tenant_pg, tenant) = registry_and_tenant("ordo_plans_digest").await;
    reg_pg
        .exec(
            "UPDATE \"$plans\" SET digest = 'sha256:feed' \
             WHERE from_version = '1.0.0' AND to_version = '1.1.0'",
        )
        .expect("tampered");
    let err = evolve::replay(&registry, &tenant, "acme", "1.3.0", OWNER)
        .await
        .expect_err("the stored digest does not render");
    assert!(
        matches!(err, EvolveRefusal::DigestMismatch { .. }),
        "{err:?}"
    );

    reg_pg
        .exec("DELETE FROM \"$plans\" WHERE from_version = '1.1.0'")
        .expect("a step removed");
    let err = evolve::path(&registry, "1.0.0", "1.3.0")
        .await
        .expect_err("no path");
    assert!(matches!(err, EvolveRefusal::NoPath { .. }), "{err:?}");
    assert!(err.to_string().contains("ordo evolve --from"), "{err}");
}

/// A resolver that routes the tenant `acme` to its own database and
/// everything else to the control database.
struct Routed {
    control: Pool,
    acme: Pool,
}

impl PoolResolver for Routed {
    fn resolve<'a>(&'a self, space: &'a SpaceId) -> ordo_exec::route::Resolving<'a> {
        Box::pin(async move {
            Ok(if space.0 == "acme" {
                self.acme.clone()
            } else {
                self.control.clone()
            })
        })
    }

    fn targets<'a>(&'a self) -> Targets<'a> {
        Box::pin(async move {
            Ok(vec![
                Target {
                    label: "default".into(),
                    pool: Ok(self.control.clone()),
                },
                Target {
                    label: "acme".into(),
                    pool: Ok(self.acme.clone()),
                },
            ])
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

fn row(r: &ordo_api::ExecResult) -> serde_json::Value {
    r.rows[0].clone()
}

/// **Through the door**: `evolve` stores and records the act, `plans`
/// lists it, `replay` brings the tenant's own database through 3 steps and
/// pins the tenant.
#[tokio::test(flavor = "multi_thread")]
async fn the_door_stores_lists_and_replays() {
    let (_reg_pg, registry) = empty("ordo_plans_door_reg").await;
    let (tenant_pg, tenant_db) = empty("ordo_plans_door_tenant").await;
    tenants::ensure_table(&registry).await.expect("$spaces");
    let c = chain();
    let v1 = Program::compile(&c[0]).expect("compiles");
    // The deployment applies and activates 1.0.0, so its tenant is pinned
    // there; the rest are stored.
    apply_op::run_program(
        &registry,
        &SinglePool::new(registry.clone()),
        &v1,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
    .expect("applies");
    for s in &c[1..] {
        store(&registry, s).await;
    }
    assert!(
        tenants::reserve(&registry, "acme", "acme", "dev:ada", 10, None)
            .await
            .expect("reserved")
    );
    tenants::activate(&registry, "acme", 11)
        .await
        .expect("active");
    Versions::on(&registry)
        .pin("acme", "1.0.0", "dev:ada", 12)
        .await
        .expect("pinned");
    let (_, created) = sa::apply(&tenant_db, &v1, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );

    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = apply_op::ApplyOps::recording(
        registry.clone(),
        Arc::new(Routed {
            control: registry.clone(),
            acme: tenant_db.clone(),
        }),
        "home",
        Some(stream.clone()),
    );
    let mut digests = Vec::new();
    for w in ["1.0.0", "1.1.0", "1.2.0", "1.3.0"].windows(2) {
        let r = run(
            &ops,
            deploy_ops::EVOLVE,
            serde_json::json!({ "from": w[0], "to": w[1] }),
        )
        .await
        .expect("stores");
        let r = row(&r);
        assert_eq!(r["stored"], true, "{r}");
        assert!(r["ran"].is_null(), "{r}");
        digests.push(r["digest"].as_str().unwrap().to_string());
    }
    let listed = run(&ops, deploy_ops::PLANS, serde_json::json!({}))
        .await
        .expect("lists");
    assert_eq!(row(&listed)["plans"].as_array().unwrap().len(), 3);
    let one_edge = run(
        &ops,
        deploy_ops::PLANS,
        serde_json::json!({ "from": "1.1.0" }),
    )
    .await
    .expect("lists one");
    assert_eq!(row(&one_edge)["plans"][0]["digest"], digests[1].as_str());

    let replayed = run(
        &ops,
        deploy_ops::REPLAY,
        serde_json::json!({ "tenant": "acme", "to": "1.3.0" }),
    )
    .await
    .expect("replays");
    let r = row(&replayed);
    assert_eq!(r["steps"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(r["pinned"]["to"], "1.3.0", "{r}");
    assert_eq!(
        Versions::on(&registry)
            .pin_of("acme")
            .await
            .expect("reads")
            .and_then(|p| p.version)
            .as_deref(),
        Some("1.3.0")
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT to_version FROM \"$migrations\" WHERE phase <> 'failed' ORDER BY seq DESC LIMIT 1"
        ),
        "1.3.0"
    );

    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(
        acts.iter().filter(|a| *a == "plan-stored").count(),
        3,
        "{acts:?}"
    );
    assert!(acts.iter().any(|a| a == "replayed"), "{acts:?}");

    // Refusals at the door: an unknown tenant, and a step from a version
    // that is not stored.
    let err = run(
        &ops,
        deploy_ops::REPLAY,
        serde_json::json!({ "tenant": "nobody" }),
    )
    .await
    .expect_err("no such tenant");
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
    let err = run(
        &ops,
        deploy_ops::EVOLVE,
        serde_json::json!({ "from": "9.9.9", "to": "1.3.0" }),
    )
    .await
    .expect_err("not stored");
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
}

/// **F1: a grant that opens a changed table is listed**, outside the digest:
/// another deployment reads it through this one's door
/// (`consumer_grants::tables_read`).
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_field_lists_the_grants_that_open_its_table() {
    let (_pg, pool) = empty("ordo_plans_readers").await;
    let with = |v: &str, fields: &str| {
        format!(
            "domain notes @ {v}\n\ngrant viewer {{\n  std.note.list\n}}\n\n\
             entity note {{\n{fields}}}\n\n\
             op allNotes unbounded =\n  note[title]\n"
        )
    };
    store(&pool, &with("1.0.0", "  title: text\n  body: text?\n")).await;
    store(&pool, &with("1.1.0", "  title: text\n")).await;
    let a = evolve::render(&pool, "1.0.0", "1.1.0", &Choices::default())
        .await
        .expect("renders");
    assert_eq!(a.readers.len(), 1, "{:?}", a.readers);
    let n = &a.readers[0];
    assert_eq!(n.grant, "grant viewer");
    assert_eq!(n.table, "note");
    assert_eq!(n.field.as_deref(), Some("body"));
    assert!(
        n.line()
            .contains("`grant viewer` opens `note` to another deployment"),
        "{}",
        n.line()
    );
    assert!(
        n.line()
            .contains("the consumer changes its `external` first"),
        "{}",
        n.line()
    );
}

/// **F4: `ordo rollback --dry-run` checks the version it would return to**
/// against the external tables as they are now. A pin does not cover a
/// foreign table, so a column renamed since is named, and nothing changes.
#[tokio::test(flavor = "multi_thread")]
async fn a_rollback_dry_run_names_an_external_the_version_can_no_longer_read() {
    let (pg, pool) = empty("ordo_plans_rollback_check").await;
    pg.exec(
        "DROP SCHEMA IF EXISTS crm CASCADE; CREATE SCHEMA crm; \
         CREATE TABLE crm.customers (customer_id bigint PRIMARY KEY, full_name text NOT NULL)",
    )
    .expect("the foreign table");
    let reads = "domain desk @ 1.0.0\n\n\
         external patrons (key: pat) from crm.customers shared {\n  id <- \"customer_id\"\n  \
         name: text <- \"full_name\"\n}\n\n\
         entity ticket {\n  subject: text\n}\n\n\
         op patronNames unbounded =\n  patrons[name]\n";
    store(&pool, reads).await;
    let ok = evolve::rollback_check(&pool, "1.0.0")
        .await
        .expect("checks");
    assert!(ok.refusals.is_empty(), "{:?}", ok.refusals);

    pg.exec("ALTER TABLE crm.customers RENAME COLUMN full_name TO display_name")
        .expect("renamed outside Ordo");
    let found = evolve::rollback_check(&pool, "1.0.0")
        .await
        .expect("checks");
    assert_eq!(found.refusals.len(), 1, "{:?}", found.refusals);
    assert!(
        found.refusals[0].contains("full_name"),
        "{:?}",
        found.refusals
    );
    assert_eq!(found.active, None, "the dry run activated something");
}
