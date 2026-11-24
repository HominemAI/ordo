//! `prev` in the plan (`plan-versions.md` increment 15,
//! `plan-syntax-additions.md` §5), against real Postgres:
//!
//! - a declared field rename renders as a rename (B4), never a drop and an
//!   add, with its forward and reverse statements;
//! - the same edit without `prev` renders as a new required field, which is
//!   refused;
//! - a tenant replaying the stored step keeps every value under the new
//!   name, and the old column is kept;
//! - the plan checks: `prev_unresolved`, `prev_type`,
//!   `prev_mapping_required`, `prev_sealed`, and `prev_stale` as a warning;
//! - an entity rename renders as a rename and is refused until the backfill
//!   worker, naming the way round;
//! - the offer to remove a landed mark, only in the chain's last
//!   environment.
//!
//! A regression looks like a rename that renders as D1 plus A2 or B2, a
//! value missing under the new name, a refusal with no fix, or an offer to
//! remove a mark before the last environment has it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::evolve::{self, EvolveRefusal};
use ordo_host::schema_apply::{self as sa, Choices, Program};
use ordo_host::versions::Versions;

const OWNER: &str = "deployment-owner:ada";

fn notes(version: &str, fields: &str) -> String {
    format!(
        "domain notes @ {version}\n\nentity note {{\n{fields}}}\n\n\
         op allNotes unbounded =\n  note[body]\n"
    )
}

const V1: &str = "  title: text\n  body: text?\n";
const V2: &str = "  heading: text prev(title)\n  body: text?\n";

async fn empty(stem: &str) -> (Pg, Pool) {
    let pg = Pg::scoped(stem);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
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

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

fn errors(r: Result<evolve::Rendered, EvolveRefusal>) -> Vec<(String, String, String)> {
    match r {
        Err(EvolveRefusal::Errors { errors, .. }) => errors
            .into_iter()
            .map(|e| (e.class, e.why, e.fix))
            .collect(),
        other => panic!("expected plan errors, got {other:?}"),
    }
}

/// **A declared rename renders as a rename**, and a tenant that replays it
/// keeps every value under the new name.
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_rename_renders_as_a_rename_and_keeps_every_value() {
    let (_reg_pg, registry) = empty("ordo_prev_reg").await;
    let (tenant_pg, tenant) = empty("ordo_prev_tenant").await;
    let x = store(&registry, &notes("1.0.0", V1)).await;
    store(&registry, &notes("1.1.0", V2)).await;
    store(
        &registry,
        &notes("1.2.0", "  heading: text\n  body: text?\n"),
    )
    .await;

    let r = evolve::render(&registry, "1.0.0", "1.1.0", &Choices::default(), 1)
        .await
        .expect("a declared rename renders");
    let changes = r.receipt["changes"].as_array().unwrap();
    let codes: Vec<&str> = changes
        .iter()
        .map(|c| c["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["B4"], "{changes:?}");
    let lines = evolve::receipt_lines(&r.receipt).join("\n");
    assert!(
        lines.contains("`note.title` is renamed to `note.heading`"),
        "{lines}"
    );
    assert!(
        lines.contains("forward: ALTER TABLE \"note\" ADD COLUMN \"heading\""),
        "{lines}"
    );
    // A stored plan copies the values with the backfill worker, so a tenant
    // keeps writing while it moves (increment 11a); the new column's
    // `NOT NULL` lands after the copy.
    assert!(lines.contains("backfill: note.heading = title"), "{lines}");
    assert!(
        lines.contains(
            "after backfill: ALTER TABLE \"note\" ALTER COLUMN \"heading\" SET NOT NULL;"
        ),
        "{lines}"
    );
    assert!(
        lines.contains("forward: ALTER TABLE \"note\" ALTER COLUMN \"title\" DROP NOT NULL;"),
        "{lines}"
    );
    assert!(
        lines.contains("reverse: UPDATE \"note\" SET \"title\" = \"heading\";"),
        "{lines}"
    );
    assert!(!lines.contains("D1"), "no drop: {lines}");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    // The same edit without `prev` is a new required field and a removed
    // one: refused, since nothing says where the values come from.
    let without = errors(evolve::render(&registry, "1.0.0", "1.2.0", &Choices::default(), 1).await);
    assert!(
        without.iter().any(|(c, _, _)| c == "not_class_a"),
        "{without:?}"
    );

    // A tenant at 1.0.0 with a row replays the stored step.
    evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect("stores");
    let (_, created) = sa::apply(&tenant, &x, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    tenant_pg
        .exec("INSERT INTO note (\"$space\", id, rev, title) VALUES ('acme', 'n1', 1, 'first')")
        .expect("a row");
    evolve::replay(&registry, &tenant, "acme", "1.1.0", OWNER)
        .await
        .expect("replays the rename");
    assert_eq!(
        one(&tenant_pg, "SELECT heading FROM note WHERE id = 'n1'"),
        "first"
    );
    // The old column is kept, with its value, and no longer required.
    assert_eq!(
        one(&tenant_pg, "SELECT title FROM note WHERE id = 'n1'"),
        "first"
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = 'title'"
        ),
        "YES"
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = 'heading'"
        ),
        "NO"
    );
    // A row written under the new version needs no `title`.
    tenant_pg
        .exec("INSERT INTO note (\"$space\", id, rev, heading) VALUES ('acme', 'n2', 1, 'second')")
        .expect("a row under 1.1.0");
}

/// **Each plan check refuses what the step cannot make, naming the fix.**
#[tokio::test(flavor = "multi_thread")]
async fn the_plan_checks_refuse_with_the_fix_and_a_landed_mark_warns() {
    let (_pg, pool) = empty("ordo_prev_checks").await;
    store(
        &pool,
        &notes(
            "1.0.0",
            "  title: text\n  body: text?\n  secret: text sealed\n",
        ),
    )
    .await;
    let cases = [
        (
            "2.0.0",
            "  heading: text prev(nothing)\n  body: text?\n  secret: text sealed\n",
            "prev_unresolved",
        ),
        (
            "2.1.0",
            "  heading: int prev(title)\n  body: text?\n  secret: text sealed\n",
            "prev_type",
        ),
        (
            "2.2.0",
            "  heading: text prev(title, body)\n  secret: text sealed\n",
            "prev_mapping_required",
        ),
        (
            "2.3.0",
            "  heading: text prev(other.title)\n  body: text?\n  secret: text sealed\n",
            "prev_mapping_required",
        ),
        (
            "2.4.0",
            "  title: text\n  body: text?\n  hidden: text sealed prev(secret)\n",
            "prev_sealed",
        ),
    ];
    for (v, fields, class) in cases {
        store(&pool, &notes(v, fields)).await;
        let errs = errors(evolve::render(&pool, "1.0.0", v, &Choices::default(), 1).await);
        let hit = errs.iter().find(|(c, _, _)| c == class);
        let (_, why, fix) = hit.unwrap_or_else(|| panic!("{v}: no {class} in {errs:?}"));
        assert!(!why.is_empty() && !fix.is_empty(), "{v}: {why} / {fix}");
    }
    // Nothing with an error is stored.
    assert_eq!(
        evolve::Plans::on(&pool).all().await.expect("plans").len(),
        0
    );

    // A mark whose rename the serving version already has: a warning only.
    store(
        &pool,
        &notes(
            "3.0.0",
            "  heading: text prev(title)\n  body: text?\n  secret: text sealed\n",
        ),
    )
    .await;
    store(
        &pool,
        &notes(
            "3.1.0",
            "  heading: text prev(title)\n  body: text\n  secret: text sealed\n",
        ),
    )
    .await;
    let r = evolve::render(&pool, "3.0.0", "3.1.0", &Choices::default(), 1).await;
    // 3.0.0 to 3.1.0 makes `body` required, which is refused on its own;
    // the stale mark is never the reason.
    if let Err(EvolveRefusal::Errors { errors, .. }) = &r {
        assert!(
            errors.iter().all(|e| !e.class.starts_with("prev_")),
            "{errors:?}"
        );
    }
    store(
        &pool,
        &notes(
            "3.2.0",
            "  heading: text prev(title)\n  body: text?\n  secret: text sealed\n  extra: text?\n",
        ),
    )
    .await;
    let r = evolve::render(&pool, "3.0.0", "3.2.0", &Choices::default(), 1)
        .await
        .expect("a stale mark renders");
    assert!(
        r.warnings.iter().any(|w| w.starts_with("prev_stale")),
        "{:?}",
        r.warnings
    );
}

/// **An entity rename renders as a rename**, and is refused until the
/// backfill worker can copy a table, naming the way round.
#[tokio::test(flavor = "multi_thread")]
async fn an_entity_rename_renders_as_one_and_is_refused_with_the_way_round() {
    let (_pg, pool) = empty("ordo_prev_entity").await;
    store(&pool, &notes("1.0.0", V1)).await;
    store(
        &pool,
        "domain notes @ 1.1.0\n\nentity memo (prev: note) {\n  title: text\n  body: text?\n}\n\n\
         op allNotes unbounded =\n  memo[body]\n",
    )
    .await;
    let errs = errors(evolve::render(&pool, "1.0.0", "1.1.0", &Choices::default(), 1).await);
    let (_, why, _) = errs
        .iter()
        .find(|(c, _, _)| c == "rename_entity_not_built")
        .unwrap_or_else(|| panic!("{errs:?}"));
    assert!(why.contains("add `memo` beside it"), "{why}");
    assert!(
        !errs.iter().any(|(c, _, _)| c == "not_class_a"),
        "no drop and add: {errs:?}"
    );
}

/// **The offer to remove a landed mark** names it in the chain's last
/// environment, once every tenant has the new name, and not before.
#[tokio::test(flavor = "multi_thread")]
async fn a_landed_mark_is_offered_only_in_the_last_environment() {
    let (_pg, pool) = empty("ordo_prev_offer").await;
    let y = store(&pool, &notes("1.1.0", V2)).await;
    // Production, no tenant behind: the mark has landed.
    let landed = ordo_host::prev_offer::landed(&pool, &y).await;
    let texts: Vec<String> = landed.iter().map(|m| m.text()).collect();
    assert_eq!(texts, ["note.heading prev(title)"]);

    // The same deployment as dev, with production after it: not yet.
    let envs = ordo_host::environments::Environments::on(&pool);
    envs.register("dev", Some(1), OWNER, "the dev step", 1)
        .await
        .expect("dev registered");
    envs.set_own("dev", "this deployment is dev")
        .await
        .expect("own set");
    assert!(ordo_host::prev_offer::landed(&pool, &y).await.is_empty());
}
