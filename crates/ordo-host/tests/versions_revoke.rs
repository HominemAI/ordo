//! Increment 2b of `plan-versions.md`: revoking a version (§1.11, D8; owner
//! 2026-11-04, asks 18 to 21).
//!
//! - A version that is not active is only marked. Revoking the active one
//!   is refused unless `--move-to` names where tenants go; with it, the
//!   activation and the revoke are one transaction and the pins move.
//! - A revoked version is refused as an activation, a rollback, an apply's
//!   activation, `versions.new_tenant` (at boot and at pin time) and a
//!   `--move-to`, each naming the revoke and the unrevoke command.
//! - An unrevoke lifts the mark; both acts stay on record.
//! - The report lists the tenants on revoked versions.
//! - The door answers as the client does, deployment owner only.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_api::BoundParams;
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, Activation, ApplyRefused};
use ordo_host::deploy_ops;
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::{RevokeAct, VersionRefusal, Versions, NEW_TENANT_ENV};

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
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    pool
}

async fn apply(pool: &Pool, source: &str) -> Result<apply_op::Report, ApplyRefused> {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
}

async fn tenant(pool: &Pool, id: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(pool, id, 11).await.expect("active");
}

async fn pinned(pool: &Pool, id: &str) -> Option<String> {
    Versions::on(pool)
        .pin_of(id)
        .await
        .expect("read")
        .expect("a tenant")
        .version
}

/// Two versions, `0.2.0` active, a tenant on it, and a beta tenant
/// `versions.new_tenant` started on `0.1.0`.
async fn deployment(stem: &str) -> Pool {
    let pool = fresh(stem).await;
    apply(&pool, V010).await.expect("applies");
    apply(&pool, V020).await.expect("applies");
    tenant(&pool, "spc_main").await;
    Versions::on(&pool)
        .backfill("dev:ada", 12)
        .await
        .expect("pinned");
    tenant(&pool, "spc_beta").await;
    Versions::on(&pool)
        .with_new_tenant(Some("0.1.0"))
        .pin_new_tenant("spc_beta", "dev:ada", 13)
        .await
        .expect("pinned");
    assert_eq!(pinned(&pool, "spc_main").await.as_deref(), Some("0.2.0"));
    assert_eq!(pinned(&pool, "spc_beta").await.as_deref(), Some("0.1.0"));
    pool
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_not_active_is_only_marked_and_then_refused_everywhere() {
    let pool = deployment("ordo_revoke_mark").await;
    let v = Versions::on(&pool);

    let r = v
        .revoke("#1", "  leaks a field  ", None, "dev:ada", 20)
        .await
        .expect("revoked");
    assert_eq!(r.version, "0.1.0");
    assert_eq!(r.moved, None);
    // Nothing moved: the beta tenant is still on it, and the report says so.
    assert_eq!(pinned(&pool, "spc_beta").await.as_deref(), Some("0.1.0"));
    let report = v.pinned_to_revoked().await.expect("report");
    assert_eq!(report.len(), 1);
    assert_eq!(report[0].mark.reason, "leaks a field");
    assert_eq!(report[0].tenant_count, 1);
    assert_eq!(report[0].tenants[0].space, "spc_beta");

    // A rollback to it, through the one body both paths call.
    let e = apply_op::activate(&pool, "0.1.0", "dev:ada")
        .await
        .unwrap_err();
    assert_eq!(e.class(), "version_revoked");
    let text = e.to_string();
    assert!(text.contains("#1 · 0.1.0"), "{text}");
    assert!(text.contains("leaks a field"), "{text}");
    assert!(
        text.contains("ordo versions unrevoke 0.1.0 --reason"),
        "{text}"
    );
    // An apply of the same program is refused activation too.
    let e = apply(&pool, V010).await.unwrap_err();
    assert_eq!(e.class(), "version_revoked");
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.2.0"));

    // As the new-tenant setting: at boot, and when a tenant is pinned.
    let beta = v.with_new_tenant(Some("0.1.0"));
    let e = beta.check_new_tenant().await.unwrap_err();
    assert!(e.to_string().contains(NEW_TENANT_ENV), "{e}");
    assert!(matches!(
        e,
        VersionRefusal::Revoked {
            setting: Some(_),
            ..
        }
    ));
    tenant(&pool, "spc_late").await;
    let e = beta
        .pin_new_tenant("spc_late", "dev:ada", 21)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("versions.new_tenant"), "{e}");
    assert_eq!(pinned(&pool, "spc_late").await, None);

    // `std.deployment.versions` carries the mark.
    let h = apply_op::history(&pool, 5)
        .await
        .expect("history")
        .to_json();
    let row = h["stored"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["version"] == "0.1.0")
        .unwrap()
        .clone();
    assert_eq!(row["revoked"]["reason"], "leaks a field");

    // Twice is refused; `--move-to` on a version not active is refused.
    let e = v.revoke("0.1.0", "again", None, "dev:ada", 22).await;
    assert!(matches!(e, Err(VersionRefusal::AlreadyRevoked { .. })));
    let e = v
        .revoke("0.2.0", "x", Some("0.1.0"), "dev:ada", 22)
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::Revoked { .. }), "{e}");

    // Lifted: activations pass again, and both acts are on record.
    assert_eq!(
        v.unrevoke("#1", "fixed upstream", "dev:bo", 30)
            .await
            .unwrap(),
        "0.1.0"
    );
    assert!(v.revocation_of("0.1.0").await.unwrap().is_none());
    let history = v.revocation_history("0.1.0").await.unwrap();
    assert_eq!(
        history
            .iter()
            .map(|r| (r.act, r.actor.as_str(), r.reason.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (RevokeAct::Revoke, "dev:ada", "leaks a field"),
            (RevokeAct::Unrevoke, "dev:bo", "fixed upstream"),
        ]
    );
    let e = v.unrevoke("0.1.0", "again", "dev:bo", 31).await;
    assert!(matches!(e, Err(VersionRefusal::NotRevoked { .. })));
    assert!(matches!(
        apply_op::activate(&pool, "0.1.0", "dev:ada").await,
        Ok(Activation::Switched { .. })
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_active_version_is_revoked_only_with_somewhere_to_go() {
    let pool = deployment("ordo_revoke_active").await;
    let v = Versions::on(&pool);

    let e = apply_op::revoke(&pool, "0.2.0", "bad migration", None, "dev:ada")
        .await
        .unwrap_err();
    let text = e.to_string();
    assert!(text.contains("--move-to <version>"), "{text}");
    assert!(text.contains("#1 · 0.1.0"), "{text}");
    assert!(v.revocation_of("0.2.0").await.unwrap().is_none());

    let e = apply_op::revoke(&pool, "0.2.0", "x", Some("#2"), "dev:ada")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("--move-to refused"), "{e}");
    let e = apply_op::revoke(&pool, "0.2.0", " ", Some("#1"), "dev:ada")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("1 to 500"), "{e}");

    let r = apply_op::revoke(&pool, "0.2.0", "bad migration", Some("#1"), "dev:ada")
        .await
        .expect("revoked");
    assert_eq!(r.moved, Some(("0.2.0".into(), "0.1.0".into())));
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    assert_eq!(pinned(&pool, "spc_main").await.as_deref(), Some("0.1.0"));
    let last = v.pin_history("spc_main").await.unwrap().pop().unwrap();
    assert_eq!(last.cause, "rollback");
    assert_eq!(
        v.revocation_of("0.2.0").await.unwrap().unwrap().reason,
        "bad migration"
    );
    // Nobody is on it now.
    assert_eq!(v.pinned_to_revoked().await.unwrap()[0].tenant_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_door_revokes_as_the_client_does() {
    let pool = deployment("ordo_revoke_door").await;
    let ops = apply_op::ApplyOps::new(
        pool.clone(),
        std::sync::Arc::new(SinglePool::new(pool.clone())),
        "home",
    );
    let bound = |v: serde_json::Value| BoundParams(v.as_object().unwrap().clone());
    let run = |name: &'static str, space: &'static str, v: serde_json::Value| {
        let ops = &ops;
        let b = bound(v);
        async move { deploy_ops::execute(ops, name, space, &b, "owner").await }
    };

    let e = run(
        deploy_ops::REVOKE,
        "home",
        serde_json::json!({"version": "0.2.0", "reason": "bad"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::ApplyRefused);
    assert!(e.message.contains("moveTo"), "{}", e.message);

    let r = run(
        deploy_ops::REVOKE,
        "home",
        serde_json::json!({"version": "0.2.0", "reason": "bad", "moveTo": "#1"}),
    )
    .await
    .expect("revoked");
    assert_eq!(r.rows[0]["version"], "0.2.0");
    assert_eq!(r.rows[0]["ordinal"], 2);
    assert_eq!(r.rows[0]["moved"], "0.1.0");
    assert_eq!(r.rows[0]["revoked"]["reason"], "bad");
    assert_eq!(r.rows[0]["revoked"]["actor"], "owner");

    let e = run(
        deploy_ops::ACTIVATE,
        "home",
        serde_json::json!({"version": "0.2.0"}),
    )
    .await
    .unwrap_err();
    assert!(e.message.contains("version_revoked"), "{}", e.message);

    let list = run(deploy_ops::REVOKED, "home", serde_json::json!({}))
        .await
        .expect("listed");
    let back = deploy_ops::revoked_from_json(&list.rows[0]["versions"][0]).expect("reads back");
    assert_eq!(back.mark.version, "0.2.0");

    let e = run(
        deploy_ops::UNREVOKE,
        "home",
        serde_json::json!({"version": "0.2.0", "reason": "x".repeat(501)}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::InvalidParam);
    assert!(e.message.contains("at most 500"), "{}", e.message);
    let u = run(
        deploy_ops::UNREVOKE,
        "home",
        serde_json::json!({"version": "0.2.0", "reason": "fixed"}),
    )
    .await
    .expect("lifted");
    assert_eq!(u.rows[0]["revoked"], serde_json::Value::Null);

    // Bound to a tenant's space, none of them is there.
    for op in [
        deploy_ops::REVOKE,
        deploy_ops::UNREVOKE,
        deploy_ops::REVOKED,
    ] {
        let e = run(
            op,
            "spc_main",
            serde_json::json!({"version": "0.2.0", "reason": "x"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, ordo_api::ErrorCode::NotFound, "{op}");
    }
}
