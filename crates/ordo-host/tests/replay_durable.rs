//! Checklist 8q — the replay cache survives a restart and holds across
//! connections.
//!
//! `MemoryReplayCache` passes every semantic test in `ordo-api` and is still
//! wrong for a real deployment, because what it gets wrong is not visible to
//! one process: two replicas hold two maps, so a retry landing on the other
//! one misses and executes. The tests here are the ones that can only fail
//! for the in-memory implementation — a second, independent connection, and
//! a pool dropped and rebuilt.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_api::idempotency::{
    KeyBinding, ReplayCache, ReplayClaim, ReplayKey, RequestFingerprint, StoredResponse,
};
use ordo_conformance::Pg;
use ordo_crypto::custody::{KeyProvider, TestProvider};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::replay::PgReplayCache;
use serde_json::json;

async fn pool(db: &str) -> Pool {
    let _ = Pg::named(db);
    Pool::connect(PoolConfig {
        db: db.into(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot")
}

fn keys() -> Arc<dyn KeyProvider> {
    Arc::new(TestProvider::default())
}

async fn cache(db: &str) -> PgReplayCache {
    cache_with(db, keys()).await
}

/// Two replicas of one deployment share key custody; two *deployments* do
/// not. Which is why the durability test threads one provider through both
/// instances rather than building a second — a replica that cannot open a
/// stored response is not a replica, and the cache answers a miss rather
/// than a wrong response, which is the safe direction but not the property
/// under test here.
async fn cache_with(db: &str, keys: Arc<dyn KeyProvider>) -> PgReplayCache {
    let c = PgReplayCache::new(pool(db).await, keys)
        .with_bounds(Duration::from_secs(3600), Duration::from_secs(1));
    c.ensure_table().await.expect("platform table");
    c
}

/// These databases outlive a test RUN, so a previous run's finished entry
/// would be found by the next one's first claim — and, sealed under a
/// provider that no longer exists, would read as an unopenable hit. Clearing
/// the row unconditionally is setup, where `abandon` is behaviour under test
/// (it deliberately releases only a *pending* reservation).
async fn reset(db: &str, k: &str) {
    let p = pool(db).await;
    let _ = p
        .rows(&format!(
            "DELETE FROM \"$idempotency\" WHERE \"key\" = '{k}'"
        ))
        .await;
}

fn key(k: &str) -> ReplayKey {
    ReplayKey {
        space: "conformance".into(),
        principal: "usr_a".into(),
        op: "placeOrder".into(),
        key: k.into(),
    }
}

fn fp(n: u8) -> RequestFingerprint {
    RequestFingerprint([n; 32])
}

fn resp() -> StoredResponse {
    StoredResponse {
        status: 201,
        body: json!({"id": "ord_1", "total": "95.00"}),
    }
}

/// The property the in-memory cache cannot have. A response stored through
/// one pool is replayed through a **different** pool built afterwards — which
/// is what a retry landing on a second replica, or on the same replica after
/// a deploy, actually looks like.
#[tokio::test(flavor = "multi_thread")]
async fn a_stored_response_survives_the_process_that_stored_it() {
    let db = "ordo_idem_durable";
    let custody = keys();
    {
        let c = cache_with(db, custody.clone()).await;
        reset(db, "k1").await;
        assert_eq!(
            c.claim(&key("k1"), &fp(1), KeyBinding::UntilExpiry).await,
            ReplayClaim::Reserved
        );
        c.put(key("k1"), fp(1), resp(), KeyBinding::UntilExpiry)
            .await;
        // Drop everything: pool, connections, in-process state.
    }

    let fresh = cache_with(db, custody).await;
    match fresh
        .claim(&key("k1"), &fp(1), KeyBinding::UntilExpiry)
        .await
    {
        ReplayClaim::Hit(s) => {
            assert_eq!(s.status, 201);
            assert_eq!(s.body, resp().body, "replayed verbatim");
        }
        other => panic!("a durable cache must replay across processes: {other:?}"),
    }
}

/// Two independent connections racing one key. Exactly one reserves — the
/// unique constraint decides, in the statement that acts on it, so there is
/// no window between looking and taking.
#[tokio::test(flavor = "multi_thread")]
async fn exactly_one_of_two_connections_reserves() {
    let db = "ordo_idem_race";
    let a = cache(db).await;
    let b = cache(db).await;
    reset(db, "k2").await;

    let (k, f) = (key("k2"), fp(1));
    let (ra, rb) = tokio::join!(
        a.claim(&k, &f, KeyBinding::UntilExpiry),
        b.claim(&k, &f, KeyBinding::UntilExpiry)
    );
    let reserved = [&ra, &rb]
        .into_iter()
        .filter(|r| **r == ReplayClaim::Reserved)
        .count();
    assert_eq!(reserved, 1, "exactly one executes: {ra:?} / {rb:?}");
    assert!(
        [ra, rb].contains(&ReplayClaim::InFlight),
        "and the loser waits rather than executing"
    );
}

/// Same key, different payload, across connections — a mismatch, not a
/// second reservation and not the first payload's response.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_payload_is_a_mismatch_not_a_replay() {
    let db = "ordo_idem_mismatch";
    let a = cache(db).await;
    reset(db, "k3").await;
    assert_eq!(
        a.claim(&key("k3"), &fp(1), KeyBinding::UntilExpiry).await,
        ReplayClaim::Reserved
    );
    a.put(key("k3"), fp(1), resp(), KeyBinding::UntilExpiry)
        .await;

    let b = cache(db).await;
    assert_eq!(
        b.claim(&key("k3"), &fp(2), KeyBinding::UntilExpiry).await,
        ReplayClaim::Mismatch,
        "a different payload under a used key can never be answered"
    );
}

/// A winner that never completes must not park the key forever — that turns
/// one crash into a permanent denial of service against one caller's retry.
/// After the reservation timeout the next caller takes it over.
#[tokio::test(flavor = "multi_thread")]
async fn a_crashed_winner_does_not_park_the_key_forever() {
    let db = "ordo_idem_stale";
    let c = cache(db).await; // 1s reservation timeout
    reset(db, "k4").await;
    assert_eq!(
        c.claim(&key("k4"), &fp(1), KeyBinding::UntilExpiry).await,
        ReplayClaim::Reserved
    );
    // …and now the winner "crashes": it never puts and never abandons.
    assert_eq!(
        c.claim(&key("k4"), &fp(1), KeyBinding::UntilExpiry).await,
        ReplayClaim::InFlight,
        "while the holder is live, nobody else may execute"
    );

    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        c.claim(&key("k4"), &fp(1), KeyBinding::UntilExpiry).await,
        ReplayClaim::Reserved,
        "past the timeout the key is takeable — at most twice, and only \
         after a crash, which is the honest bound"
    );
}

/// The stored body is **sealed at rest**. A replay cache is a plaintext
/// mirror of every write response; keeping it in cleartext beside the data it
/// was sealed away from would undo the sealing for anything a caller wrote
/// and retried.
#[tokio::test(flavor = "multi_thread")]
async fn the_stored_response_is_not_readable_from_the_table() {
    let db = "ordo_idem_sealed";
    let c = cache(db).await;
    reset(db, "k5").await;
    c.claim(&key("k5"), &fp(1), KeyBinding::UntilExpiry).await;
    c.put(key("k5"), fp(1), resp(), KeyBinding::UntilExpiry)
        .await;

    let p = pool(db).await;
    let rows = p
        .rows("SELECT encode(\"response\", 'escape') FROM \"$idempotency\" WHERE \"key\" = 'k5'")
        .await
        .unwrap();
    let stored = rows[0][0].clone().unwrap_or_default();
    assert!(
        !stored.contains("ord_1") && !stored.contains("95.00"),
        "the response body must not be legible in the table: {stored}"
    );

    // …and it is genuinely the same response coming back, so the sealing is
    // not achieved by storing nothing useful.
    match c.claim(&key("k5"), &fp(1), KeyBinding::UntilExpiry).await {
        ReplayClaim::Hit(s) => assert_eq!(s.body, resp().body),
        other => panic!("{other:?}"),
    }
}

/// `abandon` frees the key immediately rather than parking it until the
/// timeout — a retry after a `not_found` must re-run against current state.
#[tokio::test(flavor = "multi_thread")]
async fn abandon_frees_the_key_at_once() {
    let db = "ordo_idem_abandon";
    let c = cache(db).await;
    reset(db, "k6").await;
    assert_eq!(
        c.claim(&key("k6"), &fp(1), KeyBinding::UntilExpiry).await,
        ReplayClaim::Reserved
    );
    c.abandon(&key("k6")).await;
    assert_eq!(
        c.claim(&key("k6"), &fp(1), KeyBinding::UntilExpiry).await,
        ReplayClaim::Reserved,
        "an abandoned key is free now, not in a minute"
    );
}

/// A stored response is sealed at the record it belongs to (S6-003). Copy
/// the sealed body of one finished record onto another finished record, in
/// the table, and the second replays nothing: the envelope was sealed at the
/// first record's key, so it does not open at the second's. The cache
/// answers as it does for any unopenable entry, which is a miss.
#[tokio::test(flavor = "multi_thread")]
async fn a_response_moved_to_another_record_does_not_replay() {
    let db = "ordo_idem_moved";
    let c = cache(db).await;
    for k in ["k7a", "k7b"] {
        reset(db, k).await;
        assert_eq!(
            c.claim(&key(k), &fp(1), KeyBinding::UntilExpiry).await,
            ReplayClaim::Reserved
        );
        c.put(key(k), fp(1), resp(), KeyBinding::UntilExpiry).await;
    }
    for k in ["k7a", "k7b"] {
        assert!(
            matches!(
                c.claim(&key(k), &fp(1), KeyBinding::UntilExpiry).await,
                ReplayClaim::Hit(_)
            ),
            "{k} replays its own response"
        );
    }

    let p = pool(db).await;
    p.rows(
        "UPDATE \"$idempotency\" SET \"response\" = \
           (SELECT \"response\" FROM \"$idempotency\" WHERE \"key\" = 'k7a') \
         WHERE \"key\" = 'k7b'",
    )
    .await
    .expect("the operator's UPDATE");

    assert!(
        !matches!(
            c.claim(&key("k7b"), &fp(1), KeyBinding::UntilExpiry).await,
            ReplayClaim::Hit(_)
        ),
        "a response sealed for one record must not replay from another"
    );
    assert!(matches!(
        c.claim(&key("k7a"), &fp(1), KeyBinding::UntilExpiry).await,
        ReplayClaim::Hit(_)
    ));
}
