//! S7 of `plan-space-routing.md`: a listener per database, so `?live`
//! reaches a space that lives in a database of its own. Since 25g the
//! listener is opened when a feed opens on the space and closed once its
//! last feed has ended, so a tenant nobody watches holds no connection.
//!
//! A write notifies inside its own transaction on the database its space
//! lives in, and Postgres delivers a `NOTIFY` only to sessions connected to
//! that database. The failure this file guards against is silence: a
//! subscriber that stops hearing about writes and reports nothing.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_api::InvalidationBus;
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::SpaceId;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::invalidation::{entity_index, PgInvalidationBus};

const CONTROL: &str = "ordo_inval_route_control";

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    }
}

async fn fresh(db: &str) -> Pool {
    let _ = Pg::named(db);
    Pool::connect(cfg(db)).await.expect("boot")
}

fn on(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.into(),
        credential: "local".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 1,
        attached_by: "system:provisioning".into(),
    }
}

async fn notify(pool: &Pool, entity: &str) {
    pool.rows(&format!("SELECT pg_notify('ordo.invalidate', '{entity}')"))
        .await
        .expect("notify");
}

/// Whether `rx` hears something within `wait`.
async fn heard(
    rx: &mut tokio::sync::broadcast::Receiver<ordo_api::InvalidationEvent>,
    wait: Duration,
) -> Option<String> {
    match tokio::time::timeout(wait, rx.recv()).await {
        Ok(Ok(event)) => Some(event.op),
        _ => None,
    }
}

/// **An invalidation on an attached database reaches a feed on it**, which
/// is S7's done-when, alongside what sits around it: the single-database
/// bus does not hear it, a database attached after the bus connected is
/// heard as soon as a feed opens on it, nothing is listened to for a space
/// nobody is watching, and a database the bus cannot listen to is named
/// rather than silently missing.
#[tokio::test(flavor = "multi_thread")]
async fn an_invalidation_on_an_attached_target_reaches_a_feed() {
    let f = Fixture::load().expect("fixture");
    let (entity, op) = entity_index(&f)
        .into_iter()
        .find_map(|(e, ops)| ops.first().cloned().map(|op| (e, op)))
        .expect("some entity is read by some op");

    let control = fresh(CONTROL).await;
    let target = fresh("ordo_inval_route_t1").await;
    let late = fresh("ordo_inval_route_t2").await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    spacedb::attach(
        &control,
        &SpaceId("spc_alpha".into()),
        &on("ordo_inval_route_t1"),
    )
    .await
    .expect("attached");

    let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        cfg(CONTROL),
        Arc::new(Installed::new().as_configured("local")),
    ));
    let routed = PgInvalidationBus::connect_routed(&f, pools, None)
        .await
        .expect("routed bus")
        .with_linger(Some(Duration::from_millis(300)));
    let single = PgInvalidationBus::connect(&f, &cfg(CONTROL), None)
        .await
        .expect("single-database bus");
    assert_eq!(
        routed.listeners(),
        1,
        "with no feed open, only the deployment's own database is listened to"
    );

    let mut single_rx = single.subscribe(&op);
    let mut alpha = routed.subscribe_in(&op, "spc_alpha").await;
    assert_eq!(
        routed.listeners(),
        2,
        "a feed on the attached space opens one"
    );

    notify(&target, &entity).await;
    assert_eq!(
        heard(&mut alpha.receiver, Duration::from_secs(5)).await,
        Some(op.clone()),
        "a write on the attached database reaches the feed on it"
    );
    assert_eq!(
        heard(&mut single_rx, Duration::from_millis(500)).await,
        None,
        "and a bus on the control database alone hears nothing from it, which is the gap"
    );

    // A second space, on a database attached after the bus connected.
    spacedb::attach(
        &control,
        &SpaceId("spc_late".into()),
        &on("ordo_inval_route_t2"),
    )
    .await
    .expect("attached late");
    notify(&late, &entity).await;
    assert_eq!(
        heard(&mut alpha.receiver, Duration::from_millis(500)).await,
        None,
        "until a feed opens on it, a newly attached database is not listened to"
    );
    let mut late_feed = routed.subscribe_in(&op, "spc_late").await;
    assert_eq!(routed.listeners(), 3);
    notify(&late, &entity).await;
    assert_eq!(
        heard(&mut late_feed.receiver, Duration::from_secs(5)).await,
        Some(op.clone()),
        "and once one has, it is"
    );

    // Two feeds on one database share its listener, and it is released only
    // after the last of them has gone.
    let second = routed.subscribe_in(&op, "spc_late").await;
    assert_eq!(routed.listeners(), 3, "the second feed opened no listener");
    drop(late_feed);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(routed.listeners(), 3, "one feed is still open on it");
    drop(second);
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        routed.listeners(),
        2,
        "the last feed on it ended, so its listener is closed"
    );

    // A database nobody can reach is named, and the rest keep working.
    spacedb::attach(
        &control,
        &SpaceId("spc_gone".into()),
        &DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            ..on("nowhere")
        },
    )
    .await
    .expect("attached");
    let gone = routed.subscribe_in(&op, "spc_gone").await;
    assert!(gone.lease.is_none(), "there is nothing to hold");
    let unheard = routed.unheard();
    assert!(
        unheard.len() == 1 && unheard[0].contains("spc_gone"),
        "{unheard:?}"
    );
    notify(&target, &entity).await;
    assert_eq!(
        heard(&mut alpha.receiver, Duration::from_secs(5)).await,
        Some(op),
        "a dead database does not stop the others"
    );
}
