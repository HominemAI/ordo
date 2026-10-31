//! S6 of `plan-space-routing.md`: the idempotency cache keeps each space's
//! records in the database that space lives in.
//!
//! An idempotency record holds a sealed copy of a write's response, which is
//! tenant data, so it belongs under the tenant's custody with the rest of
//! the space's rows (`plan-space-routing.md` §3).
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_api::idempotency::{
    KeyBinding, ReplayCache, ReplayClaim, ReplayKey, RequestFingerprint, StoredResponse,
};
use ordo_conformance::Pg;
use ordo_crypto::custody::TestProvider;
use ordo_crypto::SpaceId;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::replay::PgReplayCache;
use serde_json::json;

const ALPHA: &str = "spc_alpha";
const BETA: &str = "spc_beta";

async fn fresh(db: &str) -> Pool {
    let _ = Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    let _ = pool.rows("DROP TABLE IF EXISTS \"$idempotency\"").await;
    pool
}

fn key(space: &str, k: &str) -> ReplayKey {
    ReplayKey {
        space: space.into(),
        principal: "usr_a".into(),
        op: "placeOrder".into(),
        key: k.into(),
    }
}

async fn keys_in(pool: &Pool) -> Vec<(String, String, String)> {
    pool.rows(
        "SELECT \"$space\", \"key\", \"state\" FROM \"$idempotency\" ORDER BY \"$space\", \"key\"",
    )
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        let c = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
        (c(0), c(1), c(2))
    })
    .collect()
}

/// **A space's records are in its own database, and another space's are
/// not.** Alpha is attached to a target and beta is not; each claim lands
/// where its space lives, the table is created in each database on first
/// use, and a stored response is written to the same place.
#[tokio::test(flavor = "multi_thread")]
async fn an_idempotency_record_lives_in_its_spaces_database() {
    let control = fresh("ordo_replay_route_control").await;
    let target = fresh("ordo_replay_route_target").await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    let env = PoolConfig::from_env();
    spacedb::attach(
        &control,
        &SpaceId(ALPHA.into()),
        &DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: "ordo_replay_route_target".into(),
            credential: "local".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "system:provisioning".into(),
        },
    )
    .await
    .expect("attached");
    let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        PoolConfig {
            size: 2,
            ..PoolConfig::from_env()
        },
        Arc::new(Installed::new().as_configured("local")),
    ));
    let cache = PgReplayCache::routed(pools, Arc::new(TestProvider::with_space(ALPHA)))
        .with_bounds(Duration::from_secs(3600), Duration::from_secs(1));

    let fp = RequestFingerprint([7; 32]);
    assert_eq!(
        cache
            .claim(&key(ALPHA, "k_alpha"), &fp, KeyBinding::UntilExpiry)
            .await,
        ReplayClaim::Reserved
    );
    assert_eq!(
        cache
            .claim(&key(BETA, "k_beta"), &fp, KeyBinding::UntilExpiry)
            .await,
        ReplayClaim::Reserved
    );
    assert_eq!(
        keys_in(&target).await,
        vec![(ALPHA.into(), "k_alpha".into(), "pending".into())],
        "alpha's record is in alpha's database, and only alpha's"
    );
    assert_eq!(
        keys_in(&control).await,
        vec![(BETA.into(), "k_beta".into(), "pending".into())],
        "beta's record is on the default, and alpha's is not"
    );

    cache
        .put(
            key(ALPHA, "k_alpha"),
            fp,
            StoredResponse {
                status: 201,
                body: json!({"id": "ord_1"}),
            },
            KeyBinding::UntilExpiry,
        )
        .await;
    assert_eq!(
        keys_in(&target).await,
        vec![(ALPHA.into(), "k_alpha".into(), "done".into())],
        "the stored response is written where the reservation is"
    );
}
