//! Increment 2 of `plan-versions.md`: every tenant is pinned to a version,
//! and every pin change is recorded (§1.3, owner 2026-11-02, D14).
//!
//! - An activation pins every tenant to it, with a `$pins` row and an
//!   epoch bump each.
//! - A rollback re-pins them, cause `rollback`.
//! - A new tenant is pinned at creation, cause `new_tenant`, to the active
//!   version or to `versions.new_tenant` when set.
//! - A setting naming no stored version is refused, naming the setting.
//! - A tenant made before pins is backfilled once.
//! - `std.deployment.versions` counts the tenants pinned to each version.
//! - A paused tenant is pinned; a failed or reaped one is not.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, Activation};
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::{VersionRefusal, Versions, NEW_TENANT_SETTING};

const V010: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

/// `V010` with a field added, under the next number.
const V020: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    pool
}

async fn apply(pool: &Pool, source: &str, activate: bool) -> apply_op::Report {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        activate,
    )
    .await
    .expect("applies")
}

/// A tenant row in `state`, made as `tenant_ops` makes one, with no pin.
async fn tenant(pool: &Pool, id: &str, state: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    match state {
        "provisioning" => {}
        "active" => {
            tenants::activate(pool, id, 11).await.expect("active");
        }
        "paused" => {
            tenants::activate(pool, id, 11).await.expect("active");
            tenants::pause(pool, id, tenants::PAUSED_BY_DEPLOYMENT, "billing", 12)
                .await
                .expect("paused");
        }
        "failed" => tenants::fail(pool, id, "test", 11).await.expect("failed"),
        "reaped" => {
            tenants::fail(pool, id, "test", 11).await.expect("failed");
            tenants::reaped(pool, id, 12).await.expect("reaped");
        }
        other => panic!("no state {other}"),
    }
}

async fn pin(pool: &Pool, id: &str) -> (Option<String>, i64) {
    let p = Versions::on(pool)
        .pin_of(id)
        .await
        .expect("read")
        .expect("a tenant");
    (p.version, p.epoch)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_activation_pins_every_tenant_with_history() {
    let pool = fresh("ordo_pins_activate").await;
    tenant(&pool, "spc_a", "active").await;
    tenant(&pool, "spc_b", "provisioning").await;

    apply(&pool, V010, true).await;
    assert_eq!(pin(&pool, "spc_a").await, (Some("0.1.0".into()), 1));
    assert_eq!(pin(&pool, "spc_b").await, (Some("0.1.0".into()), 1));

    apply(&pool, V020, true).await;
    assert_eq!(pin(&pool, "spc_a").await, (Some("0.2.0".into()), 2));
    let history = Versions::on(&pool)
        .pin_history("spc_a")
        .await
        .expect("history");
    let rows: Vec<_> = history
        .iter()
        .map(|c| {
            (
                c.from.clone(),
                c.to.as_str(),
                c.epoch,
                c.cause.as_str(),
                c.actor.as_str(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (None, "0.1.0", 1, "activate", "dev:ada"),
            (Some("0.1.0".into()), "0.2.0", 2, "activate", "dev:ada"),
        ]
    );

    // The same version again re-points nothing and moves no pin.
    apply(&pool, V020, true).await;
    assert_eq!(pin(&pool, "spc_a").await, (Some("0.2.0".into()), 2));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rollback_re_pins_with_its_cause() {
    let pool = fresh("ordo_pins_rollback").await;
    tenant(&pool, "spc_a", "active").await;
    apply(&pool, V010, true).await;
    apply(&pool, V020, true).await;

    let back = apply_op::activate(&pool, "0.1.0", "dev:bo")
        .await
        .expect("rolls back");
    assert!(matches!(back, Activation::Switched { .. }), "{back:?}");
    assert_eq!(pin(&pool, "spc_a").await, (Some("0.1.0".into()), 3));
    let last = Versions::on(&pool)
        .pin_history("spc_a")
        .await
        .expect("history")
        .pop()
        .expect("a row");
    assert_eq!(last.cause, "rollback");
    assert_eq!(last.from.as_deref(), Some("0.2.0"));
    assert_eq!(last.actor, "dev:bo");

    // Forward again is an activation, not a rollback.
    apply_op::activate(&pool, "0.2.0", "dev:bo")
        .await
        .expect("forward");
    let last = Versions::on(&pool)
        .pin_history("spc_a")
        .await
        .expect("history")
        .pop()
        .expect("a row");
    assert_eq!(last.cause, "activate");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_tenant_is_pinned_to_the_active_version() {
    let pool = fresh("ordo_pins_new").await;
    apply(&pool, V010, true).await;
    apply(&pool, V020, true).await;
    tenant(&pool, "spc_new", "provisioning").await;

    let to = Versions::on(&pool)
        .pin_new_tenant("spc_new", "dev:cy", 20)
        .await
        .expect("pinned");
    assert_eq!(to.as_deref(), Some("0.2.0"));
    assert_eq!(pin(&pool, "spc_new").await, (Some("0.2.0".into()), 1));
    let h = Versions::on(&pool)
        .pin_history("spc_new")
        .await
        .expect("history");
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].cause, "new_tenant");
    assert_eq!(h[0].from, None);
    assert_eq!(h[0].actor, "dev:cy");

    // A second call leaves a pinned tenant as it is.
    Versions::on(&pool)
        .pin_new_tenant("spc_new", "dev:cy", 21)
        .await
        .expect("no-op");
    assert_eq!(pin(&pool, "spc_new").await, (Some("0.2.0".into()), 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_setting_starts_a_new_tenant_on_another_version() {
    let pool = fresh("ordo_pins_setting").await;
    apply(&pool, V010, true).await;
    apply(&pool, V020, true).await;
    tenant(&pool, "spc_beta", "provisioning").await;

    let versions = Versions::on(&pool).with_new_tenant(Some("0.1.0"));
    versions.check_new_tenant().await.expect("a stored version");
    versions
        .pin_new_tenant("spc_beta", "dev:cy", 20)
        .await
        .expect("pinned");
    assert_eq!(pin(&pool, "spc_beta").await, (Some("0.1.0".into()), 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_setting_naming_no_stored_version_is_refused_naming_it() {
    let pool = fresh("ordo_pins_bad_setting").await;
    apply(&pool, V010, true).await;
    let refused = Versions::on(&pool)
        .with_new_tenant(Some("9.9.9"))
        .check_new_tenant()
        .await
        .expect_err("refused");
    assert!(
        matches!(refused, VersionRefusal::Setting { .. }),
        "{refused:?}"
    );
    let text = refused.to_string();
    for want in [
        NEW_TENANT_SETTING,
        "ordo versions set new-tenant",
        "9.9.9",
        "0.1.0",
    ] {
        assert!(text.contains(want), "`{want}` missing from: {text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_from_before_pins_is_backfilled_once() {
    let pool = fresh("ordo_pins_backfill").await;
    apply(&pool, V010, true).await;
    // Made after the activation and never pinned: what a tenant created
    // before this increment looks like.
    tenant(&pool, "spc_old", "active").await;
    assert_eq!(pin(&pool, "spc_old").await, (None, 0));

    let v = Versions::on(&pool);
    assert_eq!(v.backfill("ordo:versions", 30).await.expect("backfill"), 1);
    assert_eq!(v.backfill("ordo:versions", 31).await.expect("again"), 0);
    assert_eq!(pin(&pool, "spc_old").await, (Some("0.1.0".into()), 1));
    let h = v.pin_history("spc_old").await.expect("history");
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].cause, "backfill");
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_count_the_tenants_pinned_to_each() {
    let pool = fresh("ordo_pins_count").await;
    apply(&pool, V010, true).await;
    tenant(&pool, "spc_a", "active").await;
    tenant(&pool, "spc_b", "active").await;
    let v = Versions::on(&pool);
    v.backfill("ordo:versions", 30).await.expect("backfill");
    // Stored, not activated: no tenant moves.
    apply(&pool, V020, false).await;
    tenant(&pool, "spc_c", "provisioning").await;
    v.with_new_tenant(Some("0.2.0"))
        .pin_new_tenant("spc_c", "dev:cy", 40)
        .await
        .expect("pinned");

    let json = apply_op::history(&pool, 10)
        .await
        .expect("history")
        .to_json();
    let count = |version: &str| {
        json["stored"]
            .as_array()
            .expect("stored")
            .iter()
            .find(|s| s["version"] == version)
            .map(|s| s["tenants"].clone())
    };
    assert_eq!(count("0.1.0"), Some(serde_json::json!(2)));
    assert_eq!(count("0.2.0"), Some(serde_json::json!(1)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_tenant_is_pinned_and_a_failed_or_reaped_one_is_not() {
    let pool = fresh("ordo_pins_states").await;
    tenant(&pool, "spc_paused", "paused").await;
    tenant(&pool, "spc_failed", "failed").await;
    tenant(&pool, "spc_reaped", "reaped").await;
    apply(&pool, V010, true).await;

    assert_eq!(pin(&pool, "spc_paused").await, (Some("0.1.0".into()), 1));
    assert_eq!(pin(&pool, "spc_failed").await, (None, 0));
    assert_eq!(pin(&pool, "spc_reaped").await, (None, 0));
    let v = Versions::on(&pool);
    assert_eq!(v.backfill("ordo:versions", 30).await.expect("backfill"), 0);
    assert!(v
        .pin_history("spc_failed")
        .await
        .expect("history")
        .is_empty());
}

/// A tenant whose row commits while an activation is running is pinned to
/// the version that activation makes active, not the one it replaces
/// (review, 2026-11-04). The activation is held open by hand here: its turn
/// taken and its `$activations` row written, not yet committed.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_made_during_an_activation_gets_the_new_version() {
    let pool = fresh("ordo_pins_race").await;
    apply(&pool, V010, true).await;
    apply(&pool, V020, false).await;
    tenant(&pool, "spc_mid", "provisioning").await;

    let activation = pool.begin().await.expect("begin");
    activation
        .rows(&format!(
            "SELECT pg_advisory_xact_lock({}, 0)",
            ordo_host::versions::PIN_LOCK_NS
        ))
        .await
        .expect("the turn");
    ordo_exec::registry::record_activation(
        &activation,
        "0.2.0",
        "dev:ada",
        ordo_commandlog::Timestamp(50),
    )
    .await
    .expect("activation row");

    let p = pool.clone();
    let pinning = tokio::spawn(async move {
        Versions::on(&p)
            .pin_new_tenant("spc_mid", "dev:cy", 51)
            .await
            .expect("pinned")
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!pinning.is_finished(), "the pin waits its turn");
    activation.commit().await.expect("commit");

    assert_eq!(pinning.await.expect("joined").as_deref(), Some("0.2.0"));
    assert_eq!(pin(&pool, "spc_mid").await, (Some("0.2.0".into()), 1));
}
