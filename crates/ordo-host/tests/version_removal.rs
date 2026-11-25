//! **Removing a stored version** (`plan-versions.md` §1.6, §2.7; rows 10
//! and 17): `ordo versions remove <version> --move-to <version>`, and the
//! restore window that pruning and removal both read.
//!
//! What these pin, over real Postgres:
//!
//! - a removal moves the tenants pinned to the version to `--move-to`,
//!   cause `remove`, then leaves a tombstone: the version no longer
//!   resolves, by name or `#n`, nor lists; the activation log still names
//!   it; storing the same program again brings it back under its number;
//! - a tenant with a database of its own has it brought to `--move-to`
//!   before its pin moves;
//! - a version a restore may still need is refused by name, naming
//!   `versions.restore_window`: unset holds every version that served, a
//!   window holds what served within it, `0` holds none, and a version
//!   that never served is never held;
//! - the refusals, each naming its fix: the active version, a version a
//!   tenant build serves, a version a setting names, tenants and no
//!   `--move-to`, `--move-to` the version itself, a revoked `--move-to`,
//!   and a tenant that cannot move (no tenant moves then);
//! - the door: deployment owner on the deployment's own space, one act on
//!   the stream.
//!
//! A regression looks like a tenant left pinned to a version that no
//! longer loads, a version removed while a backup at it can still be
//! restored, or a removal that moved half its tenants and stopped.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::Pg;
use ordo_crypto::SpaceId;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver, SinglePool};
use ordo_exec::spacedb::{DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::ApplyOps;
use ordo_host::runtime::now_secs;
use ordo_host::schema_apply::Program;
use ordo_host::tenant_builds::TenantBuilds;
use ordo_host::tenants;
use ordo_host::version_removal::{self as vr, RemoveRefusal};
use ordo_host::versions::{PinCause, VersionSetting, Versions};

const V1: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

const V2: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

const V3: &str = "domain tasks @ 0.3.0

entity task {
  title: text
  done:  bool
  due:   int?
  note:  text?
}

op openTasks unbounded =
  task[title, done]
";

const OWNER: &str = "deployment-owner:ada";

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 8,
        ..PoolConfig::from_env()
    }
}

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(cfg(&pg.db)).await.expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    pool
}

async fn run(pool: &Pool, pools: &dyn PoolResolver, source: &str, activate: bool) {
    let program = Program::compile(source).expect("compiles");
    run_all_at_once(pool, pools, &program, vec![], "dev:ada", "t", activate)
        .await
        .expect("applies");
}

async fn apply(pool: &Pool, source: &str, activate: bool) {
    run(pool, &SinglePool::new(pool.clone()), source, activate).await
}

async fn tenant(pool: &Pool, id: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(pool, id, 11).await.expect("active");
}

async fn on(pool: &Pool, id: &str) -> String {
    Versions::on(pool)
        .pin_of(id)
        .await
        .expect("read")
        .expect("a tenant")
        .version
        .expect("pinned")
}

fn shared(pool: &Pool) -> Arc<ApplyOps> {
    ApplyOps::recording(
        pool.clone(),
        Arc::new(SinglePool::new(pool.clone())),
        "home",
        Some(Arc::new(ordo_secevent::Recorder::default())),
    )
}

async fn window(pool: &Pool, value: Option<&str>) {
    Versions::on(pool)
        .set_setting(
            VersionSetting::RestoreWindow,
            value,
            OWNER,
            "the test keeps no backups",
            now_secs(),
        )
        .await
        .expect("the restore window is set");
}

/// Three tenants. 0.1.0 then 0.2.0 activated, and `spc_b` pinned back to
/// 0.1.0, so 0.1.0 serves one tenant and is not active.
async fn one_behind(stem: &str) -> Pool {
    let pool = fresh(stem).await;
    for t in ["spc_a", "spc_b", "spc_c"] {
        tenant(&pool, t).await;
    }
    apply(&pool, V1, true).await;
    apply(&pool, V2, true).await;
    Versions::on(&pool)
        .pin("spc_b", "0.1.0", OWNER, now_secs())
        .await
        .expect("pinned back");
    assert_eq!(on(&pool, "spc_b").await, "0.1.0");
    pool
}

fn refused(e: RemoveRefusal) -> String {
    e.to_string()
}

/// **A removal moves the tenants on it, then leaves a tombstone.** The
/// version no longer resolves or lists; the activation log still names
/// it; the same program stored again comes back under its number.
#[tokio::test(flavor = "multi_thread")]
async fn a_removal_moves_its_tenants_and_leaves_a_tombstone() {
    let pool = one_behind("ordo_vr_moves").await;
    window(&pool, Some("0")).await;
    let ops = shared(&pool);

    // Tenants on it and no `--move-to`: refused, naming them and the flag.
    let e = refused(
        vr::remove(&ops, "0.1.0", None, "old", OWNER, now_secs())
            .await
            .unwrap_err(),
    );
    assert!(e.starts_with("remove_has_tenants"), "{e}");
    assert!(e.contains("spc_b") && e.contains("--move-to"), "{e}");
    assert_eq!(on(&pool, "spc_b").await, "0.1.0");

    let done = vr::remove(&ops, "#1", Some("0.2.0"), "old", OWNER, now_secs())
        .await
        .expect("removed");
    assert_eq!(done.version, "0.1.0");
    assert_eq!(done.ordinal, Some(1));
    assert_eq!(done.moved_to.as_deref(), Some("0.2.0"));
    assert_eq!(done.moved, ["spc_b"]);
    assert_eq!(on(&pool, "spc_b").await, "0.2.0");
    let v = Versions::on(&pool);
    let last = v.pin_history("spc_b").await.unwrap();
    let last = last.last().unwrap();
    assert_eq!(
        (last.cause.as_str(), last.actor.as_str()),
        (PinCause::Remove.as_str(), OWNER)
    );

    // Gone by name and by number, and from the list; the log keeps it.
    for name in ["0.1.0", "#1"] {
        let e = v.resolve(name).await.unwrap_err().to_string();
        assert!(e.contains("0.2.0"), "{name}: {e}");
    }
    assert!(v.get("0.1.0").await.unwrap().is_none());
    let stored: Vec<String> = v.stored().await.unwrap().into_iter().map(|s| s.0).collect();
    assert_eq!(stored, ["0.2.0"]);
    let log: Vec<String> = v
        .activations(10)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.version)
        .collect();
    assert!(log.contains(&"0.1.0".to_string()), "{log:?}");
    let removals = v.removals().await.unwrap();
    assert_eq!(removals.len(), 1);
    assert_eq!(
        (
            removals[0].version.as_str(),
            removals[0].moved_to.as_deref(),
            removals[0].moved
        ),
        ("0.1.0", Some("0.2.0"), 1)
    );
    // Removed twice: it is not there to remove.
    let e = refused(
        vr::remove(&ops, "0.1.0", None, "again", OWNER, now_secs())
            .await
            .unwrap_err(),
    );
    assert!(e.contains("0.1.0") && e.contains("0.2.0"), "{e}");

    // The same program stored again comes back as #1.
    apply(&pool, V1, false).await;
    assert_eq!(v.resolve("#1").await.unwrap(), "0.1.0");
    assert!(v.get("0.1.0").await.unwrap().is_some());
}

/// **A version a restore may still need is not removed** (increment 17).
/// Unset, the window holds every version that served; a window holds what
/// served within it; `0` holds none. A version that never served is
/// never held.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_a_restore_may_need_is_refused_naming_the_window() {
    let pool = fresh("ordo_vr_restore").await;
    tenant(&pool, "spc_a").await;
    apply(&pool, V1, true).await;
    apply(&pool, V2, true).await;
    let ops = shared(&pool);

    // Unset: 0.1.0 served, so it is held, naming the setting.
    let e = refused(
        vr::remove(&ops, "0.1.0", None, "old", OWNER, now_secs())
            .await
            .unwrap_err(),
    );
    assert!(e.starts_with("remove_restorable"), "{e}");
    assert!(e.contains("versions.restore_window"), "{e}");
    assert!(e.contains("ordo versions set restore-window"), "{e}");

    // A window of 30 days: it served a moment ago, so it is held until
    // then, and the answer says until when.
    window(&pool, Some("30d")).await;
    let e = refused(
        vr::remove(&ops, "0.1.0", None, "old", OWNER, now_secs())
            .await
            .unwrap_err(),
    );
    assert!(e.starts_with("remove_restorable"), "{e}");
    assert!(e.contains("is 30d") && e.contains("until"), "{e}");
    // 31 days on, nothing holds it.
    vr::remove(&ops, "0.1.0", None, "old", OWNER, now_secs() + 31 * 86_400)
        .await
        .expect("past the window");

    // A version stored and never served holds no backup, window or not.
    window(&pool, None).await;
    apply(&pool, V3, false).await;
    vr::remove(&ops, "0.3.0", None, "never used", OWNER, now_secs())
        .await
        .expect("never served");
}

/// The refusals, each naming its fix. No tenant moves on any of them.
#[tokio::test(flavor = "multi_thread")]
async fn each_refusal_names_its_fix_and_moves_no_tenant() {
    let pool = one_behind("ordo_vr_refusals").await;
    window(&pool, Some("0")).await;
    let ops = shared(&pool);
    let now = now_secs();
    let rm = |name: &'static str, to: Option<&'static str>| {
        let ops = ops.clone();
        async move {
            vr::remove(&ops, name, to, "tidy", OWNER, now)
                .await
                .unwrap_err()
                .to_string()
        }
    };

    let e = rm("0.2.0", None).await;
    assert!(e.starts_with("remove_active"), "{e}");
    assert!(e.contains("ordo rollback"), "{e}");

    let e = rm("0.1.0", Some("0.1.0")).await;
    assert!(e.contains("being removed"), "{e}");

    // A version a setting names.
    let v = Versions::on(&pool);
    v.set_setting(VersionSetting::NewTenant, Some("0.1.0"), OWNER, "x", now)
        .await
        .unwrap();
    let e = rm("0.1.0", Some("0.2.0")).await;
    assert!(e.starts_with("remove_named_by_setting"), "{e}");
    assert!(e.contains("ordo versions set new-tenant"), "{e}");
    v.set_setting(VersionSetting::NewTenant, None, OWNER, "x", now)
        .await
        .unwrap();

    // A revoked `--move-to`.
    apply(&pool, V3, false).await;
    v.revoke("0.3.0", "a flaw", None, OWNER, now).await.unwrap();
    let e = rm("0.1.0", Some("0.3.0")).await;
    assert!(e.contains("revoked"), "{e}");

    // A version a tenant build serves.
    let b = TenantBuilds::on(&ops);
    b.set("beta", "0.1.0", "beta", OWNER, now).await.unwrap();
    let e = rm("0.1.0", Some("0.2.0")).await;
    assert!(e.starts_with("remove_in_build"), "{e}");
    assert!(e.contains("ordo tenant-builds set beta"), "{e}");
    b.remove("beta").await.unwrap();

    // A tenant that cannot move: the deployment's own database does not
    // hold `--move-to` (its column gone, as after a restore from before
    // it). No pin moves, and the version stays.
    v.unrevoke("0.3.0", "fixed", OWNER, now).await.unwrap();
    pool.rows(r#"ALTER TABLE "task" DROP COLUMN "note""#)
        .await
        .expect("behind");
    let e = rm("0.1.0", Some("0.3.0")).await;
    assert!(e.starts_with("remove_cannot_move"), "{e}");
    assert!(e.contains("spc_b") && e.contains("--no-activate"), "{e}");
    assert!(e.contains("No tenant moved"), "{e}");
    assert_eq!(on(&pool, "spc_b").await, "0.1.0");
    assert!(v.get("0.1.0").await.unwrap().is_some());
}

// ── a tenant with a database of its own ─────────────────────────────────

async fn empty_db(db: &str) -> Pool {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("an empty database");
    Pool::connect(cfg(db)).await.expect("a pool")
}

fn attach_row(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.to_string(),
        credential: "vr".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 1,
        attached_by: "test".into(),
    }
}

/// **A tenant with a database of its own** has it brought to `--move-to`
/// before its pin moves, and the door records one act.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_database_is_brought_to_the_named_version_through_the_door() {
    let control = empty_db("ordo_vr_own_control").await;
    tenants::ensure_table(&control).await.unwrap();
    ordo_exec::spacedb::ensure_table(&control).await.unwrap();
    let own = empty_db("ordo_vr_own_tenant").await;
    let v1 = Program::compile(V1).unwrap();
    ordo_exec::spacedb::apply_schema_at_version(
        &own,
        &v1.ddl,
        &v1.version,
        "test",
        "A1 — new entity `task`",
        1_000,
    )
    .await
    .unwrap();
    tenant(&control, "spc_own").await;
    ordo_exec::spacedb::attach(
        &control,
        &SpaceId("spc_own".into()),
        &attach_row("ordo_vr_own_tenant"),
    )
    .await
    .unwrap();
    let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        cfg("unused"),
        Arc::new(Installed::new().as_configured("vr")),
    ));
    run(&control, pools.as_ref(), V1, true).await;
    // 0.2.0 active on the deployment's own database; the tenant stays on
    // 0.1.0, its database not brought along.
    apply(&control, V2, true).await;
    Versions::on(&control)
        .pin("spc_own", "0.1.0", OWNER, now_secs())
        .await
        .unwrap();
    assert_eq!(on(&control, "spc_own").await, "0.1.0");
    window(&control, Some("0")).await;

    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = ApplyOps::recording(control.clone(), pools, "home", Some(stream.clone()));
    let call = |space: &'static str, v: serde_json::Value| {
        let ops = ops.clone();
        async move {
            ordo_host::deploy_ops::execute(
                &ops,
                ordo_host::deploy_ops::REMOVE_VERSION,
                space,
                &BoundParams(v.as_object().unwrap().clone()),
                OWNER,
            )
            .await
        }
    };
    // Bound to the deployment's own space only.
    let e = call(
        "spc_own",
        serde_json::json!({"version": "0.1.0", "moveTo": "0.2.0", "reason": "old"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let e = call(
        "home",
        serde_json::json!({"version": "0.1.0", "reason": "old"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::ApplyRefused);
    assert!(e.message.contains("remove_has_tenants"), "{}", e.message);

    let done = call(
        "home",
        serde_json::json!({"version": "0.1.0", "moveTo": "#2", "reason": "old"}),
    )
    .await
    .expect("removed");
    assert_eq!(done.rows[0]["version"], "0.1.0");
    assert_eq!(done.rows[0]["movedTo"], "0.2.0");
    assert_eq!(done.rows[0]["moved"], serde_json::json!(["spc_own"]));
    assert_eq!(on(&control, "spc_own").await, "0.2.0");
    let due = own
        .rows(
            "SELECT 1 FROM information_schema.columns WHERE table_name = 'task' \
             AND column_name = 'due'",
        )
        .await
        .unwrap();
    assert_eq!(due.len(), 1, "the tenant's database took 0.2.0 first");
    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(acts, ["version-removed"]);
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
