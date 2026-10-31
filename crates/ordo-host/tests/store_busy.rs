//! Audit bug 5 (`docs/audit-service-interfaces.md`): the tenant-setting
//! stores (managed persons, admission policy, recovery hold; the claim code was removed)
//! dropped a full pool's class with `map_err(|_| ...)`, so a busy pool
//! answered `space_unavailable`, which no client retries. Each now fails with
//! a typed `StoreFault` whose class comes from `PoolError::is_busy`, and an
//! op answers it `server_busy`.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::admission::PolicyStore;
use ordo_host::credential_ops::ManagedStore;
use ordo_host::recovery_hold::TenantRecovery;

async fn one_connection(stem: &str) -> Pool {
    let pg = ordo_conformance::Pg::scoped(stem);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 1,
        acquire_timeout: Duration::from_millis(200),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool")
}

fn assert_busy(what: &str, f: ordo_identity::StoreFault) {
    assert!(f.is_busy(), "{what}: {f}");
    assert!(
        std::error::Error::source(&f).is_some(),
        "{what}: the pool error is the cause"
    );
    let e = ExecError::from(f);
    assert_eq!(e.code, ErrorCode::ServerBusy, "{what}: {e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_pool_is_server_busy_from_every_tenant_setting_store() {
    let pool = one_connection("ordo_store_busy").await;
    let held = pool.begin().await.expect("the one connection");

    let managed = ManagedStore::new(pool.clone());
    assert_busy("managed_by", managed.managed_by("prn_1").await.unwrap_err());
    assert_busy(
        "mark",
        managed.mark("prn_1", "spc_1", "me", 1).await.unwrap_err(),
    );
    assert_busy(
        "set_temporary",
        managed.set_temporary("prn_1", "me", 1).await.unwrap_err(),
    );

    let policy = PolicyStore::new(pool.clone());
    assert_busy("admission policy", policy.get("spc_1").await.unwrap_err());

    let recovery = TenantRecovery::new(pool.clone(), false);
    assert_busy("recovery hold", recovery.hold("spc_1").await.unwrap_err());

    // Free again: a broken store (no such table here) is not busy, and an
    // op still answers it `space_unavailable`.
    drop(held);
    let broken = managed.managed_by("prn_1").await.unwrap_err();
    assert!(!broken.is_busy(), "{broken}");
    assert_eq!(ExecError::from(broken).code, ErrorCode::SpaceUnavailable);
}
