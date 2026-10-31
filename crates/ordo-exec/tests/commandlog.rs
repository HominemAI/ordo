//! The durable command log (8e), its per-space append lock (8g), and
//! Ed25519 checkpoints over a chain that survives a restart (8h).
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{
    checkpoint, Actor, ChainHash, Command, CommandGroup, CommandId, CommandKind, CorrelationId,
    Ed25519Signer, Image, SpaceId, Store, TamperKind, Timestamp, Value,
};
use ordo_exec::commandlog::PgStore;
use ordo_exec::{Pool, PoolConfig};

/// Each test owns its database. The suites run in parallel against one
/// cluster and every one of them writes to `$commandlog`, so sharing a
/// database means one test's chain is another's tamper.
async fn fresh(db: &str, size: usize) -> Pool {
    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    pool.rows("DELETE FROM \"$checkpoints\"").await.unwrap();
    pool
}

fn space() -> SpaceId {
    SpaceId("spc_pg".into())
}

fn img(fields: &[(&str, Value)]) -> Image {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// A create group with one command, in `space`.
fn group(n: usize) -> CommandGroup {
    group_in(&space(), n)
}

fn group_in(space: &SpaceId, n: usize) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(format!("cg_{n}")),
        actor: Actor::AgentOnBehalfOf {
            agent: "ellanor".into(),
            user: format!("usr_{n}"),
        },
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: space.clone(),
            entity: "transactions".into(),
            row_id: format!("txn_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(img(&[
                ("amount", Value::Int(n as i64 * 100)),
                ("id", Value::Id(format!("txn_{n}"))),
                ("memo", Value::SealedRef([n as u8; 16])),
                ("note", Value::Text(format!("group {n} — ünicode"))),
            ])),
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

/// The failure 8e names: the audit spine did not survive a restart.
/// Append, drop every connection, reconnect, and read the same chain back —
/// groups, images, actor attribution, hashes and all.
#[tokio::test(flavor = "multi_thread")]
async fn the_chain_survives_a_restart() {
    let pool = fresh("ordo_log_durable", 4).await;
    for n in 0..3 {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group(n)).await.expect("append");
        tx.commit().await.unwrap();
    }
    // Every connection goes away — the process, as far as the database is
    // concerned, is gone.
    drop(pool);

    let pool = Pool::connect(PoolConfig {
        db: "ordo_log_durable".into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("reboot");
    let store = PgStore::new(pool, tokio::runtime::Handle::current());

    tokio::task::spawn_blocking(move || {
        assert_eq!(store.len(&space()), 3, "the chain outlived the process");
        assert!(!store.is_empty(&space()));

        // The decoded group is the group that was appended — actor form,
        // image values, sealed handle, unicode text, all of it.
        let e1 = store.get(&space(), 1).expect("index 1");
        assert_eq!(e1.group, group(1));
        assert_eq!(
            e1.group.actor.to_string(),
            "agent:ellanor on behalf of user:usr_1"
        );

        // Correlation lookup is scoped to the space and finds the same
        // entry the index does.
        assert_eq!(
            store.find(&space(), &CorrelationId("cg_1".into())),
            Some(e1.clone())
        );
        assert_eq!(store.find(&space(), &CorrelationId("cg_9".into())), None);

        // Links: each entry points at its predecessor's stored hash.
        assert_eq!(store.get(&space(), 0).unwrap().prev, ChainHash::GENESIS);
        assert_eq!(e1.prev, store.get(&space(), 0).unwrap().hash);
        assert_eq!(
            store.head(&space()),
            Some(store.get(&space(), 2).unwrap().hash)
        );

        // And the whole thing recomputes.
        store
            .verify(&space())
            .expect("chain verifies after restart");
    })
    .await
    .unwrap();
}

/// 8g, the property itself: two appenders in ONE space, both transactions
/// genuinely in flight, and the chain they produce still verifies.
///
/// The test is only worth anything if the two really are concurrent, so it
/// asserts that too — while A holds the lock, B's append does not complete,
/// and it completes the moment A commits.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_appenders_in_one_space_serialize() {
    let pool = fresh("ordo_log_lock", 6).await;

    // A opens, appends, and stays open holding the transaction-scoped lock.
    let a = pool.begin().await.unwrap();
    a.append_group(&group(0)).await.expect("A appends");

    // B opens a SECOND transaction — in flight at the same time — and tries
    // to append to the same space.
    let b = pool.begin().await.unwrap();
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let b_task = tokio::spawn(async move {
        let entry = b.append_group(&group(1)).await.expect("B appends");
        let _ = done_tx.send(());
        b.commit().await.unwrap();
        entry
    });

    // B is blocked on `pg_advisory_xact_lock`, which is the whole point:
    // it waits rather than racing A to the same `prev`.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "B must be waiting on the per-space append lock while A holds it"
    );
    // Independent confirmation from the server's own view: somebody is
    // waiting on an advisory lock in this database.
    let waiting = pool
        .rows(
            "SELECT count(*) FROM pg_locks \
             WHERE \"locktype\" = 'advisory' AND NOT \"granted\" \
             AND \"database\" = (SELECT oid FROM pg_database \
                                WHERE datname = current_database())",
        )
        .await
        .unwrap();
    assert_eq!(
        waiting[0][0].as_deref(),
        Some("1"),
        "the server should show exactly one ungranted advisory lock"
    );

    // A commits; its transaction-scoped lock is released BY the commit —
    // nothing has to remember to unlock, which is the other half of why the
    // xact form is the one the spec names.
    a.commit().await.unwrap();
    let b_entry = tokio::time::timeout(Duration::from_secs(5), b_task)
        .await
        .expect("B unblocks once A commits")
        .unwrap();

    let store = PgStore::new(pool, tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        assert_eq!(store.len(&space()), 2);
        let first = store.get(&space(), 0).unwrap();
        assert_eq!(first.group.correlation, CorrelationId("cg_0".into()));
        assert_eq!(b_entry.prev, first.hash, "B chained onto A, not beside it");
        store
            .verify(&space())
            .expect("two concurrent appenders produce one verifiable chain");
    })
    .await
    .unwrap();
}

/// The same property at width: eight writers in one space, all racing, one
/// chain. Without the lock they read the same head and fork; with it the
/// indices are exactly 0..8 and `verify` accepts.
#[tokio::test(flavor = "multi_thread")]
async fn eight_racing_writers_produce_one_verifiable_chain() {
    let pool = fresh("ordo_log_race", 8).await;
    let mut tasks = Vec::new();
    for n in 0..8 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            let tx = pool.begin().await.unwrap();
            let entry = tx.append_group(&group(n)).await.expect("append");
            tx.commit().await.unwrap();
            entry.hash
        }));
    }
    let mut hashes = Vec::new();
    for t in tasks {
        hashes.push(t.await.unwrap());
    }
    // Eight distinct links, no two writers landing on the same hash.
    hashes.sort_by_key(|h| h.0);
    hashes.dedup_by_key(|h| h.0);
    assert_eq!(hashes.len(), 8, "every append produced its own link");

    let idx = pool
        .rows("SELECT string_agg(\"idx\"::text, ',' ORDER BY \"idx\") FROM \"$commandlog\"")
        .await
        .unwrap();
    assert_eq!(
        idx[0][0].as_deref(),
        Some("0,1,2,3,4,5,6,7"),
        "positions are a dense total order, not a fork"
    );

    let store = PgStore::new(pool, tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || store.verify(&space()).expect("one chain"))
        .await
        .unwrap();
}

/// The append is a statement of the caller's transaction, so an abort takes
/// the link with it. This is 8f's guarantee made structural rather than
/// procedural: nothing has to *remember* to drop the group.
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_transaction_leaves_no_link() {
    let pool = fresh("ordo_log_abort", 4).await;

    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(0)).await.expect("append");
    drop(tx); // the crash-shaped ending

    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let s = space();
    let len = tokio::task::spawn_blocking(move || store.len(&s))
        .await
        .unwrap();
    assert_eq!(len, 0, "a rolled-back write asserts no effect in the chain");

    // And the next real append starts at genesis, not at a phantom index.
    let tx = pool.begin().await.unwrap();
    let entry = tx.append_group(&group(0)).await.expect("append");
    tx.commit().await.unwrap();
    assert_eq!(entry.prev, ChainHash::GENESIS);
}

/// Correlation uniqueness is `(space, correlation)` — per space, never
/// global (SH2-14). Enforced twice: by the read-then-check inside the lock,
/// and by the unique index that would catch a race the check straddled.
#[tokio::test(flavor = "multi_thread")]
async fn correlation_uniqueness_is_per_space() {
    let pool = fresh("ordo_log_corr", 4).await;
    let other = SpaceId("spc_other".into());

    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(1)).await.unwrap();
    tx.commit().await.unwrap();

    // Same correlation, same space: refused.
    let tx = pool.begin().await.unwrap();
    let err = tx.append_group(&group(1)).await.unwrap_err();
    assert!(
        matches!(&err, ordo_commandlog::AppendError::DuplicateCorrelation(c) if c.0 == "cg_1"),
        "{err:?}"
    );
    drop(tx);

    // Same correlation, another tenant's space: allowed. A global namespace
    // would let one tenant deny an id to every other.
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group_in(&other, 1))
        .await
        .expect("another space may use the same correlation id");
    tx.commit().await.unwrap();

    let store = PgStore::new(pool, tokio::runtime::Handle::current());
    let other2 = other.clone();
    tokio::task::spawn_blocking(move || {
        assert_eq!(store.len(&space()), 1);
        assert_eq!(store.len(&other2), 1);
        // Chains are independent: both start at genesis.
        assert_eq!(store.get(&other2, 0).unwrap().prev, ChainHash::GENESIS);
        // The cross-space probe answers a boolean and nothing else.
        assert!(store.is_logged_elsewhere(&space(), &CorrelationId("cg_1".into())));
        assert!(!store.is_logged_elsewhere(&space(), &CorrelationId("cg_7".into())));
        store.verify(&space()).unwrap();
        store.verify(&other2).unwrap();
    })
    .await
    .unwrap();
}

/// Durability is only worth having if it is checkable. An operator with
/// write access to the table edits a stored image; `verify` names the index.
#[tokio::test(flavor = "multi_thread")]
async fn tampering_with_a_stored_group_is_caught_at_its_index() {
    let pool = fresh("ordo_log_tamper", 4).await;
    for n in 0..4 {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group(n)).await.unwrap();
        tx.commit().await.unwrap();
    }

    // Re-encode group 2 with a different amount and write it back, leaving
    // the stored hash alone — the shape a quiet edit actually takes.
    let mut doctored = group(2);
    doctored.commands[0].after = Some(img(&[
        ("amount", Value::Int(999_999)),
        ("id", Value::Id("txn_2".into())),
        ("memo", Value::SealedRef([2u8; 16])),
        ("note", Value::Text("group 2 — ünicode".into())),
    ]));
    let body = ordo_commandlog::encode_group(&doctored);
    let hex: String = body.iter().map(|b| format!("{b:02x}")).collect();
    pool.rows(&format!(
        "UPDATE \"$commandlog\" SET \"body\" = '\\x{hex}'::bytea \
         WHERE \"space\" = 'spc_pg' AND \"idx\" = 2"
    ))
    .await
    .unwrap();

    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let err = tokio::task::spawn_blocking(move || store.verify(&space()).unwrap_err())
        .await
        .unwrap();
    assert_eq!(err.index, 2);
    assert_eq!(err.kind, TamperKind::HashMismatch);

    // A body that is not a decodable group at all is not a pass either.
    pool.rows(
        "UPDATE \"$commandlog\" SET \"body\" = '\\xdeadbeef'::bytea \
         WHERE \"space\" = 'spc_pg' AND \"idx\" = 1",
    )
    .await
    .unwrap();
    let store = PgStore::new(pool, tokio::runtime::Handle::current());
    let err = tokio::task::spawn_blocking(move || store.verify(&space()).unwrap_err())
        .await
        .unwrap();
    assert_eq!(
        err.kind,
        TamperKind::Unreadable,
        "a chain that cannot be read must not verify green"
    );
}

/// 8h over durable storage: the checkpoint is stored, reloaded, and checked
/// by a receiver who holds only the published public key — and a checkpoint
/// signed by a different key does not pass.
#[tokio::test(flavor = "multi_thread")]
async fn durable_checkpoints_verify_from_the_public_key_and_forgeries_do_not() {
    let pool = fresh("ordo_log_checkpoint", 4).await;
    for n in 0..3 {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group(n)).await.unwrap();
        tx.commit().await.unwrap();
    }

    let signer = Ed25519Signer::from_seed("chk-live", &[11u8; 32]);
    let published = signer.verifier().public_hex();
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());

    let cp = tokio::task::spawn_blocking(move || {
        checkpoint(&store, &space(), &signer, Timestamp(9_000)).expect("checkpoint")
    })
    .await
    .unwrap();
    assert_eq!(cp.upto, 3);
    let tx = pool.begin().await.unwrap();
    tx.put_checkpoint(&cp).await.unwrap();
    tx.commit().await.unwrap();

    // The receiver: a key id and 64 hex characters, nothing that can sign.
    let receiver =
        ordo_commandlog::CheckpointVerifier::from_public_hex("chk-live", &published).unwrap();

    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let reloaded = store
        .try_latest_checkpoint(&space())
        .await
        .unwrap()
        .expect("stored checkpoint");
    assert_eq!(reloaded, cp);
    assert!(
        receiver.verify_checkpoint(&reloaded),
        "a receiver with only the public key verifies the stored checkpoint"
    );

    // And it anchors a suffix: verification can start from the checkpoint's
    // head rather than from genesis, which is what makes pruning legal.
    let head = reloaded.head;
    let upto = reloaded.upto as usize;
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(3)).await.unwrap();
    tx.commit().await.unwrap();
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        let suffix = store.entries(&space(), upto, 100);
        assert_eq!(suffix.len(), 1);
        ordo_commandlog::verify_chain_from(&space(), &suffix, head, upto)
            .expect("the suffix verifies against the checkpoint's head");
    })
    .await
    .unwrap();

    // The forgery: a different key, same claim. It is stored perfectly
    // happily — the database is not the control — and it does not verify.
    let attacker = Ed25519Signer::from_seed("chk-live", &[12u8; 32]);
    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let forged = tokio::task::spawn_blocking(move || {
        checkpoint(&store, &space(), &attacker, Timestamp(9_000)).unwrap()
    })
    .await
    .unwrap();
    assert!(
        !receiver.verify_checkpoint(&forged),
        "a checkpoint signed by another key must not verify"
    );
}

/// The lock key is computed here, not by `hashtext`, so that every node
/// agrees on it across server versions. Pin the property (stable, and
/// different per space) rather than the number.
#[test]
fn space_lock_keys_are_stable_and_distinct() {
    use ordo_exec::commandlog::space_lock_key;
    let a = SpaceId("spc_a".into());
    let b = SpaceId("spc_b".into());
    assert_eq!(space_lock_key(&a), space_lock_key(&SpaceId("spc_a".into())));
    assert_ne!(space_lock_key(&a), space_lock_key(&b));
}

/// Two spaces are independent: a writer holding one space's append lock
/// does not delay the other. This is the "what is not contended" claim in
/// `implementation.md`, and it is what makes horizontal scale across spaces
/// unaffected by a per-space serialization point.
#[tokio::test(flavor = "multi_thread")]
async fn a_lock_in_one_space_does_not_block_another() {
    let pool = fresh("ordo_log_indep", 6).await;
    let other = SpaceId("spc_other".into());

    let a = pool.begin().await.unwrap();
    a.append_group(&group(0)).await.unwrap();

    // A different space, while A still holds its lock: no waiting at all.
    let b = pool.begin().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), b.append_group(&group_in(&other, 0)))
        .await
        .expect("a different space must not queue behind this one")
        .expect("append");
    b.commit().await.unwrap();
    a.commit().await.unwrap();

    let store = PgStore::new(pool, tokio::runtime::Handle::current());
    let other2 = other.clone();
    tokio::task::spawn_blocking(move || {
        store.verify(&space()).unwrap();
        store.verify(&other2).unwrap();
    })
    .await
    .unwrap();
}

/// The write path end to end: a real op, a real transaction, and the group
/// entering the durable chain *inside* it. A refused write leaves the chain
/// exactly as it found it.
#[tokio::test(flavor = "multi_thread")]
async fn writes_append_to_the_durable_chain_inside_their_transaction() {
    use ordo_conformance::{Fixture, Pg, WriteLog};
    use ordo_exec::{Executor, LoggedExecutor};

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_log_write");
    let pg = Pg {
        db: "ordo_log_write".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");

    let pool = Pool::connect(PoolConfig {
        db: "ordo_log_write".into(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();

    let schema = f.schema.clone();
    let registry = f.registry.clone();
    let fixture = Arc::new(f);
    let log = Arc::new(std::sync::Mutex::new(WriteLog::new()));
    let x = LoggedExecutor::new(
        Executor {
            pool: pool.clone(),
            schema,
            registry,
            keys: fixture.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        "conformance",
    );
    // The command log's space is the fixture's, which is where the write
    // protocol stamps its commands.
    let logged = SpaceId("conformance".into());

    x.write_as(
        fixture.clone(),
        log.clone(),
        r#"renameTask($task: "task_1", $title: "logged")"#.into(),
        "mem_1".into(),
    )
    .await
    .expect("Ana may write");

    let store = PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let s = logged.clone();
    let after_one = tokio::task::spawn_blocking(move || {
        let n = store.len(&s);
        let entry = store.get(&s, 0).expect("the write's group");
        store.verify(&s).expect("verifies");
        (n, entry)
    })
    .await
    .unwrap();
    assert_eq!(
        after_one.0, 1,
        "the committed write is in the durable chain"
    );
    let cmd = &after_one.1.group.commands[0];
    assert_eq!(cmd.kind, CommandKind::Converge);
    assert_eq!(cmd.row_id, "task_1");
    assert_eq!(
        cmd.after.as_ref().unwrap().get("title"),
        Some(&Value::Text("logged".into())),
        "the durable image is the write's own after-image"
    );
    assert_eq!(after_one.1.group.actor, Actor::User("mem_1".into()));

    // A refusal: no effect, and no link asserting one.
    let e = x
        .write_as(
            fixture.clone(),
            log.clone(),
            r#"renameTask($task: "task_1", $title: "refused")"#.into(),
            "mem_2".into(),
        )
        .await
        .expect_err("Bram may not");
    assert!(format!("{e}").contains("access_guard"), "{e}");
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("logged".into())
    );

    let store = PgStore::new(pool, tokio::runtime::Handle::current());
    let n = tokio::task::spawn_blocking(move || store.len(&logged))
        .await
        .unwrap();
    assert_eq!(n, 1, "a refused write appends no link");
}

/// **25bd: a bulk write that describes no row is a success that changes
/// nothing.** `definition-language.md` G11: "an empty selection is the
/// ordinary vacuous no-op ... describing zero rows has always been a no-op".
/// The protocol builds no command for it, so there is no group to append,
/// and `CommandGroup::validate` refuses an empty one. The executor answers
/// the write and leaves the chain exactly as it found it. It used to answer
/// `log_append_failed`, which the door served as a 500.
#[tokio::test(flavor = "multi_thread")]
async fn a_bulk_write_that_matches_no_row_succeeds_and_appends_nothing() {
    use ordo_conformance::{Fixture, Pg, WriteLog};
    use ordo_exec::{Executor, LoggedExecutor};

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_log_empty_bulk");
    let pg = Pg {
        db: "ordo_log_empty_bulk".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: "ordo_log_empty_bulk".into(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();

    let schema = f.schema.clone();
    let registry = f.registry.clone();
    let fixture = Arc::new(f);
    let log = Arc::new(std::sync::Mutex::new(WriteLog::new()));
    let x = LoggedExecutor::new(
        Executor {
            pool: pool.clone(),
            schema,
            registry,
            keys: fixture.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        "conformance",
    );
    let tasks_before = pg.query(r#"SELECT count(*) FROM "tasks""#).unwrap();

    // `purgeDone` is a bulk `del` over `project == $project and status ==
    // "done"`; no project of this name exists, so it describes zero rows.
    x.write_as(
        fixture.clone(),
        log.clone(),
        r#"purgeDone($project: "proj_none")"#.into(),
        "mem_1".into(),
    )
    .await
    .expect("an empty match is an ordinary outcome");

    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "tasks""#).unwrap(),
        tasks_before,
        "nothing changed"
    );
    let store = PgStore::new(pool, tokio::runtime::Handle::current());
    let n = tokio::task::spawn_blocking(move || store.len(&SpaceId("conformance".into())))
        .await
        .unwrap();
    assert_eq!(
        n, 0,
        "no command group is appended for a write that did nothing"
    );
}

/// **The flag survives the durable store, and no column was added for it.**
///
/// The `$commandlog` table's `body` is the canonical encoding — the exact
/// bytes the chain hashed — so a field inside that encoding needs no DDL at
/// all. That is not a saving, it is the only workable answer: the platform
/// table is created with `CREATE TABLE IF NOT EXISTS`, which does nothing to
/// a table that already exists, so a new column would be absent on every
/// deployment that has ever booted and every insert against one would fail.
/// A denormalized column would also be a second, unhashed answer to a
/// question the body already answers under the hash.
///
/// Asserted in three places at once: the decoded group, the recomputed chain
/// hash, and the table's own column list.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_administration_flag_survives_the_durable_store() {
    // Size 1: every step here is sequential, and the workspace suite runs
    // every one of these binaries against one 100-connection cluster.
    let pool = fresh("ordo_flag_durable", 1).await;

    let mut flagged = group(0);
    flagged.owner_administration = true;
    let plain = group(1);
    assert!(!plain.owner_administration);

    for g in [&flagged, &plain] {
        let tx = pool.begin().await.unwrap();
        tx.append_group(g).await.expect("append");
        tx.commit().await.unwrap();
    }

    // No column was added, and nothing needs one.
    let rows = pool
        .rows(
            "SELECT \"column_name\" FROM information_schema.columns \
             WHERE \"table_name\" = '$commandlog' ORDER BY \"ordinal_position\"",
        )
        .await
        .unwrap();
    let cols: Vec<String> = rows
        .into_iter()
        .filter_map(|r| r.first().cloned().flatten())
        .collect();
    assert_eq!(
        cols,
        [
            "space",
            "idx",
            "correlation",
            "actor",
            "prev",
            "hash",
            "body"
        ],
        "the flag rides `body`; a column for it would be unhashed and, on \
         an existing deployment, absent"
    );

    let store = PgStore::new(pool, tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        let e0 = store.get(&space(), 0).expect("index 0");
        let e1 = store.get(&space(), 1).expect("index 1");
        assert_eq!(e0.group, flagged, "decoded back exactly as appended");
        assert!(e0.group.owner_administration);
        assert!(!e1.group.owner_administration);
        // The stored hash recomputes from the stored content, flag included
        // — which is what `verify` does for every entry.
        assert_eq!(
            ordo_commandlog::hash_group(e0.prev, &e0.group),
            e0.hash,
            "the flag is part of what the stored hash covers"
        );
        store
            .verify(&space())
            .expect("a chain with a flagged group in it");
    })
    .await
    .unwrap();
}
