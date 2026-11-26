//! Orphans and pruning (`plan-versions.md` §2.7, increment 14), against
//! real Postgres:
//!
//! - a version that stops declaring a field and an entity orphans them:
//!   `$orphans` records each, with its columns and a drop date, and the
//!   column and the table keep what was written;
//! - erasing a space reaches the orphans: its rows in the orphaned table go,
//!   and its orphaned values go with its rows; another space keeps its own;
//! - exporting a space carries the orphans under `orphans/`;
//! - pruning: off in production until `prune.enabled` is on; the first ask
//!   prunes nothing and says what would go; a wrong confirmation, and in
//!   production a missing deployment name, are refused; a field a pinned
//!   version still declares is refused naming the version; asked twice, the
//!   column is gone from every database and a tombstone is kept;
//! - a field declared again is no longer an orphan.
//!
//! A regression looks like an orphan missing from `$orphans`, an erasure
//! that leaves a space's rows in an orphaned table, an export without the
//! orphaned values, or a prune that runs on one ask.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::Pg;
use ordo_crypto::{KeyProvider, SpaceId, TestProvider};
use ordo_exec::route::{PoolResolver, SinglePool, Target, Targets};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::deploy_ops;
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::Versions;

const OWNER: &str = "deployment-owner:ada";

/// v1: notes with a size, and drafts.
const V1: &str = "domain notes @ 1.0.0\n\n\
    entity note {\n  title: text\n  size: int?\n}\n\n\
    entity draft {\n  text: text\n}\n\n\
    op allNotes unbounded =\n  note[title]\n";

/// v2: no size, no drafts.
const V2: &str = "domain notes @ 2.0.0\n\n\
    entity note {\n  title: text\n}\n\n\
    op allNotes unbounded =\n  note[title]\n";

/// v3: size again.
const V3: &str = "domain notes @ 3.0.0\n\n\
    entity note {\n  title: text\n  size: int?\n}\n\n\
    op allNotes unbounded =\n  note[title]\n";

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
    pool.ensure_platform_tables().await.expect("platform");
    ordo_exec::prune::ensure_tables(&pool).await.expect("prune");
    (pg, pool)
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

async fn apply(pool: &Pool, source: &str) -> apply_op::Report {
    let p = Program::compile(source).expect("compiles");
    run_all_at_once(
        pool,
        &SinglePool::new(pool.clone()),
        &p,
        vec![],
        "dev:ada",
        "test",
        true,
    )
    .await
    .expect("applies")
}

/// v1 applied with 2 spaces' rows, then v2.
async fn orphaned(stem: &str) -> (Pg, Pool) {
    let (pg, pool) = empty(stem).await;
    apply(&pool, V1).await;
    pg.exec(
        "INSERT INTO note (\"$space\", id, rev, title, size) VALUES \
           ('s1', 'n1', 1, 'mine', 41), ('s2', 'n2', 1, 'theirs', 42); \
         INSERT INTO draft (\"$space\", id, rev, text) VALUES \
           ('s1', 'd1', 1, 'my draft'), ('s2', 'd2', 1, 'their draft');",
    )
    .expect("rows under v1");
    let report = apply(&pool, V2).await;
    let notes = format!("{:?}", report.notes);
    assert!(notes.contains("orphaned    `note.size`"), "{notes}");
    assert!(notes.contains("orphaned    `draft`"), "{notes}");
    (pg, pool)
}

/// **A dropped field and entity are orphaned**, kept with their values,
/// and recorded with their columns and a drop date.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_field_and_entity_are_recorded_as_orphans() {
    let (pg, pool) = orphaned("ordo_orphans_record").await;
    let all = ordo_exec::orphans::list(&pool).await.expect("lists");
    let names: Vec<String> = all.iter().map(|o| o.name()).collect();
    assert_eq!(names, vec!["draft", "note.size"], "{all:?}");
    let size = &all[1];
    assert_eq!(size.columns, vec!["size"]);
    assert_eq!(size.from_version.as_deref(), Some("1.0.0"));
    assert_eq!(size.to_version, "2.0.0");
    assert_eq!(
        size.drop_after,
        Some(size.orphaned_at + 30 * 86_400),
        "the drop date is 30 days on"
    );
    assert!(size.is_live());
    // Kept, with what was written.
    assert_eq!(one(&pg, "SELECT size FROM note WHERE id = 'n1'"), "41");
    assert_eq!(one(&pg, "SELECT count(*) FROM draft"), "2");

    // A version that declares the field again serves it again: no longer an
    // orphan. The entity it does not declare stays one.
    apply(&pool, V3).await;
    let names: Vec<String> = ordo_exec::orphans::list(&pool)
        .await
        .expect("lists")
        .iter()
        .map(|o| o.name())
        .collect();
    assert_eq!(names, vec!["draft"]);
}

/// **Erasure reaches the orphans.** Space `s1` is erased: its row in the
/// orphaned table is gone, and so is its orphaned value with its row.
/// Space `s2` keeps both. The answer names the orphans it reached.
#[tokio::test(flavor = "multi_thread")]
async fn erasing_a_space_removes_its_orphaned_values() {
    let (pg, pool) = orphaned("ordo_orphans_erase").await;
    let schema = Program::compile(V2).expect("compiles").schema;
    let keys = TestProvider::new();
    let s1 = SpaceId("s1".into());
    keys.create_space(&s1).expect("custody");
    let erased = ordo_exec::erase::erase_space(
        &pool,
        &s1,
        &schema,
        &keys,
        &ordo_exec::cadence::LogPolicy::DEFAULT,
        1_900_000_000,
        "s1",
        None,
    )
    .await
    .expect("erases");
    assert_eq!(erased.orphans, vec!["draft", "note.size"], "{erased:?}");
    assert!(
        erased.deleted.iter().any(|(t, n)| t == "draft" && *n == 1),
        "the orphaned table was swept: {:?}",
        erased.deleted
    );
    assert_eq!(
        one(&pg, "SELECT count(*) FROM draft WHERE \"$space\" = 's1'"),
        "0"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM note WHERE size IS NOT NULL AND \"$space\" = 's1'"
        ),
        "0"
    );
    // The other space keeps what it wrote.
    assert_eq!(one(&pg, "SELECT size FROM note WHERE id = 'n2'"), "42");
    assert_eq!(
        one(&pg, "SELECT count(*) FROM draft WHERE \"$space\" = 's2'"),
        "1"
    );
}

/// **Export carries the orphans**, under `orphans/`, with each row's id.
#[tokio::test(flavor = "multi_thread")]
async fn exporting_a_space_carries_its_orphans() {
    let (_pg, pool) = orphaned("ordo_orphans_export").await;
    let schema = Program::compile(V2).expect("compiles").schema;
    let artifact = Versions::on(&pool)
        .get("2.0.0")
        .await
        .expect("reads")
        .expect("stored");
    let keys = TestProvider::new();
    let s2 = SpaceId("s2".into());
    let mut bundle = ordo_exec::export::MemoryBundle::new();
    ordo_exec::export::export_space(
        &pool,
        &ordo_exec::export::ExportRequest {
            space: &s2,
            schema: &schema,
            artifact: &artifact,
            keys: &keys,
            blobs: None,
            sealed: ordo_exec::export::SealedMode::Ciphertext,
            correlation: "exp_test",
        },
        &mut bundle,
    )
    .await
    .expect("exports");
    let size = bundle.get("orphans/note.size.jsonl").expect("note.size");
    assert_eq!(size, "{\"id\": \"n2\", \"size\": 42}\n");
    let draft = bundle.get("orphans/draft.jsonl").expect("draft");
    assert!(draft.contains("their draft"), "{draft}");
    assert!(!draft.contains("my draft"), "another space's row: {draft}");
    let manifest = bundle.get("manifest.json").expect("manifest");
    assert!(manifest.contains("orphans/note.size.jsonl"), "{manifest}");
}

/// The deployment's own database and one tenant's.
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
) -> Result<serde_json::Value, ordo_api::ExecError> {
    let b = BoundParams(v.as_object().unwrap().clone());
    deploy_ops::execute(ops, name, "home", &b, OWNER)
        .await
        .map(|r| r.rows[0].clone())
}

/// **Pruning asks twice**, and refuses everything short of that.
#[tokio::test(flavor = "multi_thread")]
async fn pruning_asks_twice_then_removes_the_column_and_keeps_a_tombstone() {
    let (pg, pool) = orphaned("ordo_orphans_prune").await;
    let (acme_pg, acme) = empty("ordo_orphans_prune_acme").await;
    // The tenant's own database: v1 then v2, as the walk brings it.
    for s in [V1, V2] {
        let p = Program::compile(s).expect("compiles");
        let (_, applied) = ordo_host::schema_apply::apply(&acme, &p, "t", "t").await;
        applied.expect("applies");
    }
    ordo_host::schema_apply::record_orphans(
        &acme,
        &Program::compile(V1).unwrap().schema,
        &Program::compile(V2).unwrap(),
        Some("1.0.0"),
        "t",
    )
    .await
    .expect("records");
    acme_pg
        .exec(
            "INSERT INTO note (\"$space\", id, rev, title, size) VALUES ('acme', 'a1', 1, 't', 7)",
        )
        .expect("a row");
    tenants::ensure_table(&pool).await.expect("$spaces");
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = apply_op::ApplyOps::recording(
        pool.clone(),
        Arc::new(Routed {
            control: pool.clone(),
            acme: acme.clone(),
        }),
        "home",
        Some(stream.clone()),
    );

    // Off in production, naming the command that turns it on.
    let off = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({ "name": "note.size" }),
    )
    .await
    .expect_err("off in production");
    assert_eq!(off.code, ErrorCode::ApplyRefused);
    assert!(off.message.contains("prune_disabled"), "{}", off.message);
    assert!(
        off.message.contains("ordo environments set prune on"),
        "{}",
        off.message
    );
    run(
        &ops,
        deploy_ops::SET_PROMOTION_SETTING,
        serde_json::json!({ "setting": "prune", "value": "on", "reason": "the test prunes" }),
    )
    .await
    .expect("turned on");

    // A name that is no orphan, listing the ones there are.
    let none = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({ "name": "note.title" }),
    )
    .await
    .expect_err("not an orphan");
    assert_eq!(none.code, ErrorCode::NotFound);
    assert!(none.message.contains("note.size"), "{}", none.message);

    // A tenant pinned to a version that still declares it.
    assert!(tenants::reserve(&pool, "acme", "acme", "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(&pool, "acme", 11).await.expect("active");
    Versions::on(&pool)
        .pin("acme", "1.0.0", "dev:ada", 12)
        .await
        .expect("pinned");
    let served = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({ "name": "note.size", "confirm": "note.size" }),
    )
    .await
    .expect_err("still served");
    assert!(
        served.message.contains("prune_still_served"),
        "{}",
        served.message
    );
    assert!(served.message.contains("`1.0.0`"), "{}", served.message);
    Versions::on(&pool)
        .pin("acme", "2.0.0", "dev:ada", 13)
        .await
        .expect("moved");

    // A version a restore may still need declares it (increment 17): with
    // `versions.restore_window` unset, a version that served within the 30-day
    // default is held, and the refusal names the setting. A window of `0` holds none.
    let held = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({ "name": "note.size" }),
    )
    .await
    .expect_err("a restore may need 1.0.0");
    assert_eq!(held.code, ErrorCode::ApplyRefused);
    assert!(
        held.message.contains("prune_restorable"),
        "{}",
        held.message
    );
    assert!(held.message.contains("`1.0.0`"), "{}", held.message);
    assert!(
        held.message.contains("ordo versions set restore-window"),
        "{}",
        held.message
    );
    run(
        &ops,
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({
            "setting": "versions.restore_window", "value": "0", "reason": "no backups here"
        }),
    )
    .await
    .expect("the window is set");

    // The first ask prunes nothing, and says what would go.
    let first = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({ "name": "note.size" }),
    )
    .await
    .expect("answers");
    assert_eq!(first["pruned"], false, "{first}");
    let dbs = first["databases"].as_array().unwrap();
    assert_eq!(dbs.len(), 2, "{first}");
    assert_eq!(dbs[0]["database"], "default");
    assert_eq!(dbs[0]["rows"], 2, "{first}");
    assert_eq!(dbs[1]["rows"], 1, "{first}");
    assert_eq!(
        first["confirm"],
        "ordo evolve --prune note.size --confirm note.size --confirm-deployment notes"
    );
    assert_eq!(one(&pg, "SELECT size FROM note WHERE id = 'n1'"), "41");

    // A wrong confirmation, then no deployment name in production.
    let wrong = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({ "name": "note.size", "confirm": "note.title" }),
    )
    .await
    .expect_err("wrong");
    assert_eq!(wrong.code, ErrorCode::InvalidParam);
    assert!(
        wrong.message.contains("prune_unconfirmed"),
        "{}",
        wrong.message
    );
    let unnamed = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({ "name": "note.size", "confirm": "note.size" }),
    )
    .await
    .expect_err("production asks for the name");
    assert!(
        unnamed.message.contains("--confirm-deployment notes"),
        "{}",
        unnamed.message
    );
    assert_eq!(one(&pg, "SELECT size FROM note WHERE id = 'n1'"), "41");

    // Asked twice: gone from both databases, a tombstone in each.
    let done = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({
            "name": "note.size", "confirm": "note.size", "confirmDeployment": "notes",
        }),
    )
    .await
    .expect("prunes");
    assert_eq!(done["pruned"], true, "{done}");
    assert_eq!(done["prunedIn"], serde_json::json!(["default", "acme"]));
    for p in [&pg, &acme_pg] {
        assert_eq!(
            one(
                p,
                "SELECT count(*) FROM information_schema.columns \
                 WHERE table_name = 'note' AND column_name = 'size'"
            ),
            "0"
        );
        assert_eq!(
            one(
                p,
                "SELECT pruned_by FROM \"$orphans\" WHERE entity = 'note' AND field = 'size'"
            ),
            OWNER
        );
    }
    let listed = run(&ops, deploy_ops::ORPHANS, serde_json::json!({}))
        .await
        .expect("lists");
    let default = &listed["databases"][0]["orphans"];
    let size = default
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == "note.size")
        .expect("the tombstone is listed");
    assert!(size["prunedAt"].is_u64(), "{size}");

    // A tombstone is not an orphan: a second prune finds nothing.
    let again = run(
        &ops,
        deploy_ops::PRUNE,
        serde_json::json!({ "name": "note.size", "confirm": "note.size" }),
    )
    .await
    .expect_err("already pruned");
    assert_eq!(again.code, ErrorCode::NotFound);

    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert!(acts.iter().any(|a| a == "orphan-pruned"), "{acts:?}");
}

/// `apply_op::run_program` with `rollout.strategy` set to `all` first. With
/// it unset, a deployment that is not dev rolls an apply out a batch at a
/// time (ruled 2026-11-25); these tests apply and expect every tenant on
/// the new version at once.
async fn run_all_at_once(
    pool: &ordo_exec::Pool,
    pools: &dyn ordo_exec::route::PoolResolver,
    program: &ordo_host::schema_apply::Program,
    notes: Vec<String>,
    actor: &str,
    op: &str,
    activate: bool,
) -> Result<ordo_host::apply_op::Report, ordo_host::apply_op::ApplyRefused> {
    ordo_host::rollouts::Rollouts::on(pool)
        .set_setting(
            "rollout.strategy",
            Some("all"),
            "test",
            "every tenant at once",
            0,
        )
        .await
        .expect("rollout.strategy all");
    ordo_host::apply_op::run_program(pool, pools, program, notes, actor, op, activate).await
}
