//! R2c — `PgKeyStore`: `LocalKeyProvider` over Postgres.
//!
//! The properties `plan-runtime.md` §2.4's done-when names: custody boots
//! over Postgres, seals and opens **across a process restart**, and
//! `drop_space` removes an SMK and every DEK under it in one act.
//!
//! "Across a process restart" is simulated the only way a test can and the
//! only way that means anything: the store, the provider, and every byte of
//! in-memory cache are **dropped**, and a second store is opened against
//! the same database with the same master key. Nothing survives except the
//! rows, which is exactly the claim.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_crypto::{KeyError, KeyProvider, LocalKeyProvider, Purpose, SpaceId, WrappedKeyStore};
use ordo_exec::keystore::{self, PgKeyStore};
use ordo_exec::{Pool, PoolConfig};

const MASTER: &[u8] = b"an-operator-master-key-for-r2c-tests";

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    }
}

/// A database of this test's own, with `$keycustody` in it.
async fn prepared(db: &str) -> Pool {
    let admin = Pool::connect(cfg("postgres")).await.expect("admin pool");
    // `CREATE DATABASE` fails if it exists, which is the ordinary case on a
    // rerun. Nothing else in this suite depends on it having been new.
    let _ = admin.rows(&format!("CREATE DATABASE {db}")).await;
    let pool = Pool::connect(cfg(db)).await.expect("pool boots");
    pool.rows(r#"DROP TABLE IF EXISTS "$keycustody""#)
        .await
        .expect("clean");
    keystore::ensure_tables(&pool).await.expect("custody table");
    pool
}

fn store(db: &str) -> PgKeyStore {
    PgKeyStore::open(cfg(db)).expect("the store opens")
}

/// How many wrapped keys the table holds for a space.
async fn rows_for(pool: &Pool, space: &str) -> i64 {
    pool.rows(&format!(
        r#"SELECT count(*) FROM "$keycustody" WHERE "space" = '{space}'"#
    ))
    .await
    .expect("count")[0][0]
        .as_deref()
        .expect("a count is never null")
        .parse()
        .expect("a count is a number")
}

/// Custody survives the process. Seal in one, open in the next.
#[tokio::test(flavor = "multi_thread")]
async fn custody_seals_in_one_process_and_opens_in_the_next() {
    const DB: &str = "ordo_rt_keystore_restart";
    let pool = prepared(DB).await;
    let space = SpaceId("acme".into());

    // Process one: provision, then seal.
    let sealed = {
        let p = LocalKeyProvider::new(MASTER, Box::new(store(DB)));
        p.create_space(&space).expect("custody minted");
        let lease = p.release(&space, Purpose::Write, "r2c").expect("lease");
        p.seal_at(
            &lease,
            "customers",
            "email",
            ordo_crypto::Anchor::Row("r1"),
            b"ada@example.com",
        )
        .expect("sealed")
    };
    // Everything the first process held is gone — the store, its cache, its
    // connection, and the unwrapped master.

    // Process two: the same master over the same database, nothing else
    // shared.
    let p = LocalKeyProvider::new(MASTER, Box::new(store(DB)));
    let lease = p
        .release(&space, Purpose::Read, "r2c")
        .expect("custody was found, not re-minted");
    assert_eq!(
        p.open_at(
            &lease,
            "customers",
            "email",
            ordo_crypto::Anchor::Row("r1"),
            &sealed
        )
        .expect("the envelope opens under the restored custody"),
        b"ada@example.com",
    );

    // And the blind index — SMK-derived, so it is the sharper test of the
    // same fact: a digest computed in the second process must address the
    // row the first one wrote.
    let first = LocalKeyProvider::new(MASTER, Box::new(store(DB)))
        .blind_index(
            &space,
            "customers",
            "email",
            ordo_crypto::BlindValue::Text("ada@example.com"),
        )
        .expect("digest");
    let second = p
        .blind_index(
            &space,
            "customers",
            "email",
            ordo_crypto::BlindValue::Text("ada@example.com"),
        )
        .expect("digest");
    assert_eq!(first, second, "the SMK did not survive the restart");

    // Only *wrapped* material reached the table, and the shape says so: a
    // key is 32 bytes, an envelope is longer — nonce, ciphertext and tag.
    // A row that was exactly 32 bytes would be a key somebody stored raw
    // in a table that goes into the backup.
    let rows = pool
        .rows(r#"SELECT min(octet_length("wrapped")) FROM "$keycustody""#)
        .await
        .expect("scan");
    let shortest: usize = rows[0][0].as_deref().expect("rows").parse().expect("len");
    assert!(
        shortest > 32,
        "a {shortest}-byte row is unwrapped key material"
    );
}

/// A different master opens nothing, and says so indistinctly.
#[tokio::test(flavor = "multi_thread")]
async fn a_different_master_key_unwraps_nothing() {
    const DB: &str = "ordo_rt_keystore_master";
    prepared(DB).await;
    let space = SpaceId("acme".into());

    let p = LocalKeyProvider::new(MASTER, Box::new(store(DB)));
    p.create_space(&space).expect("custody minted");
    let lease = p.release(&space, Purpose::Write, "r2c").expect("lease");
    let sealed = p
        .seal_at(
            &lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("r1"),
            b"secret",
        )
        .expect("sealed");

    // The wrapped rows are there and are readable; what is not there is the
    // key that unwraps them. This is the whole point of storing only
    // wrapped material.
    let wrong = LocalKeyProvider::new(b"a-different-operator-master-key", Box::new(store(DB)));
    let lease = wrong
        .release(&space, Purpose::Read, "r2c")
        .expect("the SPACE exists — that is not the secret");
    let refused = wrong.open_at(
        &lease,
        "orders",
        "memo",
        ordo_crypto::Anchor::Row("r1"),
        &sealed,
    );
    assert!(
        matches!(refused, Err(KeyError::Seal)),
        "the wrong master opened a sealed value: {refused:?}"
    );
}

/// `drop_space` takes the SMK and every DEK under it, in one statement.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_space_takes_its_smk_and_every_epoch_at_once() {
    const DB: &str = "ordo_rt_keystore_drop";
    let pool = prepared(DB).await;
    let (a, b) = (SpaceId("acme".into()), SpaceId("beta".into()));

    let p = LocalKeyProvider::new(MASTER, Box::new(store(DB)));
    for s in [&a, &b] {
        p.create_space(s).expect("custody minted");
    }
    // Three epochs, so "every DEK under it" is a claim with more than one
    // row behind it.
    assert_eq!(p.rotate(&a, "test").expect("rotate"), 2);
    assert_eq!(p.rotate(&a, "test").expect("rotate"), 3);

    assert_eq!(rows_for(&pool, "acme").await, 4, "one SMK and three DEKs");

    p.destroy_space(&a).expect("erased");

    assert_eq!(
        rows_for(&pool, "acme").await,
        0,
        "erasure left rows behind — partial crypto-erasure is not erasure"
    );
    // The neighbouring space is untouched: `drop_space` is scoped by the
    // space, and a `DELETE` that took the wrong rows would be the worst
    // possible bug in this file.
    assert_eq!(rows_for(&pool, "beta").await, 2, "its SMK and epoch 1");

    // And it is gone for the next process too, not only for this cache.
    let next = LocalKeyProvider::new(MASTER, Box::new(store(DB)));
    assert!(
        matches!(
            next.release(&a, Purpose::Read, "r2c"),
            Err(KeyError::UnknownSpace)
        ),
        "an erased space came back on restart"
    );
    next.release(&b, Purpose::Read, "r2c")
        .expect("the neighbour still has custody");
}

/// `ordo keys init` has not been run: the store says which command is
/// missing rather than reporting an empty custody table.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_custody_table_names_the_command_that_creates_it() {
    const DB: &str = "ordo_rt_keystore_uninit";
    let pool = prepared(DB).await;
    pool.rows(r#"DROP TABLE "$keycustody""#)
        .await
        .expect("dropped");

    let refused = PgKeyStore::open(cfg(DB));
    let Err(KeyError::Store(text)) = refused else {
        panic!("a store with no table must refuse");
    };
    assert!(
        text.reason().contains("ordo keys init"),
        "the refusal does not name the remedy: {text}"
    );
}

/// A second process's `ordo keys init` is visible without a restart.
///
/// The cache is loaded at `open`, so a space provisioned afterwards is a
/// miss — and a miss for an SMK is looked up rather than answered from a
/// snapshot, because "no custody for that space" is a refusal an operator
/// would otherwise have to restart their way out of.
#[tokio::test(flavor = "multi_thread")]
async fn custody_provisioned_by_another_process_is_found_without_a_restart() {
    const DB: &str = "ordo_rt_keystore_late";
    prepared(DB).await;
    let space = SpaceId("acme".into());

    // The serving process, opened before the space exists.
    let serving = LocalKeyProvider::new(MASTER, Box::new(store(DB)));
    assert!(matches!(
        serving.release(&space, Purpose::Read, "r2c"),
        Err(KeyError::UnknownSpace)
    ));

    // The operator, in another process.
    LocalKeyProvider::new(MASTER, Box::new(store(DB)))
        .create_space(&space)
        .expect("custody minted");

    // And the serving process finds it, including the epoch — an SMK
    // without its DEKs would be custody that cannot seal.
    let lease = serving
        .release(&space, Purpose::Write, "r2c")
        .expect("the late provision is visible");
    assert_eq!(serving.current_epoch(&space).expect("epoch"), 1);
    let sealed = serving
        .seal_at(
            &lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("r1"),
            b"late",
        )
        .expect("seal");
    let read = serving
        .release(&space, Purpose::Read, "r2c")
        .expect("lease");
    assert_eq!(
        serving
            .open_at(
                &read,
                "orders",
                "memo",
                ordo_crypto::Anchor::Row("r1"),
                &sealed
            )
            .expect("open"),
        b"late"
    );
}

/// The epoch of a space another process made is found when it is the
/// FIRST question asked. `Spaces::resolve` asks `current_epoch` before
/// anything calls `release`, and it reads only the DEKs. Answered from the
/// cache, a replica that served no request for a tenant made on another
/// replica refused every routine run for it with "no key custody"
/// (notes v2's two-server reminder test, 2026-10-31).
#[tokio::test(flavor = "multi_thread")]
async fn the_epoch_of_a_space_made_by_another_process_is_found_first() {
    const DB: &str = "ordo_rt_keystore_late_epoch";
    prepared(DB).await;
    let space = SpaceId("acme".into());

    let serving = LocalKeyProvider::new(MASTER, Box::new(store(DB)));
    assert!(matches!(
        serving.current_epoch(&space),
        Err(KeyError::UnknownSpace)
    ));

    LocalKeyProvider::new(MASTER, Box::new(store(DB)))
        .create_space(&space)
        .expect("custody minted");

    assert_eq!(
        serving
            .current_epoch(&space)
            .expect("the late provision's epoch is visible"),
        1
    );
    serving
        .release(&space, Purpose::Write, "r2c")
        .expect("and its keys");
}

/// The store is a `WrappedKeyStore` like the in-memory one, and the two are
/// substitutable — including the one method whose contract is about
/// atomicity rather than about values.
#[tokio::test(flavor = "multi_thread")]
async fn the_store_answers_the_trait_the_way_the_memory_one_does() {
    const DB: &str = "ordo_rt_keystore_trait";
    prepared(DB).await;
    let space = SpaceId("acme".into());

    let pg = store(DB);
    let mem = ordo_crypto::MemoryKeyStore::new();
    for s in [&pg as &dyn WrappedKeyStore, &mem as &dyn WrappedKeyStore] {
        assert_eq!(s.smk(&space).expect("smk"), None);
        s.put_smk(&space, b"wrapped-smk").expect("put");
        assert_eq!(s.smk(&space).expect("smk"), Some(b"wrapped-smk".to_vec()));
        s.put_dek(&space, 1, b"wrapped-dek-1").expect("put");
        s.put_dek(&space, 2, b"wrapped-dek-2").expect("put");
        assert_eq!(
            s.deks(&space).expect("deks"),
            vec![
                (1u8, b"wrapped-dek-1".to_vec()),
                (2u8, b"wrapped-dek-2".to_vec())
            ],
            "DEKs come back by ascending epoch"
        );
        s.drop_dek(&space, 1).expect("drop");
        assert_eq!(
            s.deks(&space).expect("deks"),
            vec![(2u8, b"wrapped-dek-2".to_vec())]
        );
        s.drop_space(&space).expect("drop");
        assert_eq!(s.smk(&space).expect("smk"), None);
        assert!(s.deks(&space).expect("deks").is_empty());
    }
}
