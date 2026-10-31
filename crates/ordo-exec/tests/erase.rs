//! Space deletion by crypto-erasure, against a real Postgres.
//!
//! The claim under test is not "the rows are gone" — a `DELETE` proves
//! that and proves nothing else. It is **"the bytes in every backup are
//! unopenable"**, and the only way to assert that is to hold ciphertext
//! that was readable a moment ago and show it is not readable now.
//! [`erasure_makes_a_surviving_envelope_unopenable`] does exactly that: it
//! keeps a copy of the envelope *outside* the database, which is what a
//! backup is.
//!
//! Databases are prefixed `ordo_exp_`, one per test function.

use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::Pg;
use ordo_crypto::{
    KeyProvider, LocalKeyProvider, MemoryKeyStore, Purpose, SpaceId, WrappedKeyStore,
};
use ordo_exec::cadence::LogPolicy;
use ordo_exec::erase::{disclose, erase_space, EraseRefusal, RETAINED};
use ordo_exec::prune::ConsumerKind;
use ordo_exec::{Pool, PoolConfig};
use ordo_schema::{build, ddl_for, Schema};
use ordo_syntax::parse_program;

const SPACE: &str = "spc_doomed";
const NEIGHBOUR: &str = "spc_bystander";
const NOW: u64 = 10_000_000;

const PROGRAM: &str = "\
entity vendors {
  name:   text
  active: bool = true
}
entity orders (key: ord) {
  vendor -> vendors
  total:  money
  memo:   text sealed
}
";

// ── scaffolding ──────────────────────────────────────────────────────────

fn schema_of(src: &str) -> Schema {
    build(&parse_program(src).expect("parses")).expect("builds")
}

fn scratch(db: &str) -> Pg {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    Pg {
        db: db.to_string(),
        ..Pg::default()
    }
}

async fn pool_for(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots")
}

/// `LocalKeyProvider` rather than `TestProvider`, deliberately.
/// `TestProvider` re-creates custody on demand in `release`, so a test that
/// erased a space and then asked it for a lease would get a fresh SMK and
/// prove nothing. Erasure has to be asserted against a provider that can
/// say no.
fn provider() -> (Arc<LocalKeyProvider>, Arc<MemoryKeyStore>) {
    let store = Arc::new(MemoryKeyStore::new());
    let p = Arc::new(LocalKeyProvider::new(
        b"erase-test-master-key",
        Box::new(SharedStore(store.clone())),
    ));
    for s in [SPACE, NEIGHBOUR] {
        p.create_space(&SpaceId(s.into())).expect("custody");
    }
    (p, store)
}

/// A `WrappedKeyStore` the test can also look into, to assert that the
/// SMK and the DEKs are actually gone rather than merely unreachable.
struct SharedStore(Arc<MemoryKeyStore>);

impl WrappedKeyStore for SharedStore {
    fn smk(&self, s: &SpaceId) -> Result<Option<Vec<u8>>, ordo_crypto::KeyError> {
        self.0.smk(s)
    }
    fn put_smk(&self, s: &SpaceId, w: &[u8]) -> Result<(), ordo_crypto::KeyError> {
        self.0.put_smk(s, w)
    }
    fn deks(&self, s: &SpaceId) -> Result<Vec<(u8, Vec<u8>)>, ordo_crypto::KeyError> {
        self.0.deks(s)
    }
    fn put_dek(&self, s: &SpaceId, e: u8, w: &[u8]) -> Result<(), ordo_crypto::KeyError> {
        self.0.put_dek(s, e, w)
    }
    fn drop_dek(&self, s: &SpaceId, e: u8) -> Result<(), ordo_crypto::KeyError> {
        self.0.drop_dek(s, e)
    }
    fn drop_space(&self, s: &SpaceId) -> Result<(), ordo_crypto::KeyError> {
        self.0.drop_space(s)
    }
}

/// Rows in both spaces, with a sealed value in each.
fn seed(pg: &Pg, keys: &dyn KeyProvider, space: &str) -> Vec<u8> {
    let sid = SpaceId(space.to_string());
    let lease = keys.release(&sid, Purpose::Write, "seed").expect("release");
    let memo = keys
        .seal_at(
            &lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("r1"),
            format!("{space} secret").as_bytes(),
        )
        .expect("seal");
    pg.exec(&format!(
        "INSERT INTO \"vendors\" (\"$space\", \"id\", \"rev\", \"name\", \"active\") \
           VALUES ('{space}', 'vend_1', 1, 'Alpha', true); \
         INSERT INTO \"orders\" (\"$space\", \"id\", \"rev\", \"vendor\", \"total\", \"memo\") \
           VALUES ('{space}', 'ord_1', 1, 'vend_1', 500, {}), \
                  ('{space}', 'ord_2', 1, 'vend_1', 700, {});",
        ordo_crypto::sealed::bytea_lit(&memo),
        ordo_crypto::sealed::bytea_lit(&memo),
    ))
    .expect("seed");
    memo
}

async fn log_a_group(pool: &Pool, space: &str, i: u64) {
    let store =
        ordo_exec::commandlog::PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let mut after = ordo_commandlog::Image::new();
    after.insert("id".into(), ordo_commandlog::Value::Id(format!("vend_{i}")));
    store
        .try_append(&ordo_commandlog::CommandGroup {
            correlation: ordo_commandlog::CorrelationId(format!("cg_{space}_{i}")),
            actor: ordo_commandlog::Actor::User("usr_owner".into()),
            commands: vec![ordo_commandlog::Command {
                id: ordo_commandlog::CommandId(format!("cmd_{space}_{i}")),
                space: ordo_commandlog::SpaceId(space.to_string()),
                entity: "vendors".into(),
                row_id: format!("vend_{i}"),
                kind: ordo_commandlog::CommandKind::Create,
                before: None,
                after: Some(after),
                at: ordo_commandlog::Timestamp(NOW - 10),
            }],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        })
        .await
        .expect("append");
}

async fn env(db: &str) -> (Pg, Pool, Schema, Arc<LocalKeyProvider>, Arc<MemoryKeyStore>) {
    let pg = scratch(db);
    let schema = schema_of(PROGRAM);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    pg.exec(&ddl_for(&schema).expect("ddl")).expect("apply");
    let pool = pool_for(db).await;
    pool.ensure_platform_tables().await.expect("platform");
    ordo_exec::prune::ensure_tables(&pool).await.expect("prune");
    let (keys, store) = provider();
    (pg, pool, schema, keys, store)
}

fn count(pg: &Pg, sql: &str) -> i64 {
    pg.query(sql).expect("count")[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .expect("a number")
}

// ── the claim ────────────────────────────────────────────────────────────

/// **The point of crypto-erasure**, and the only assertion that
/// distinguishes it from a `DELETE`.
///
/// `memo` is a copy of the envelope taken before the erasure and held
/// outside the database — which is what a backup, a WAL segment, a replica
/// and an archive all are. It opens before. It must not open after, and
/// the reason must be that the key is gone rather than that a lease was
/// refused: the assertion below re-creates custody from scratch and shows
/// the envelope still does not open, because the new SMK is not the old
/// one.
#[tokio::test(flavor = "multi_thread")]
async fn erasure_makes_a_surviving_envelope_unopenable() {
    let (pg, pool, schema, keys, store) = env("ordo_exp_erase_keys").await;
    let memo = seed(&pg, keys.as_ref(), SPACE);
    let neighbour_memo = seed(&pg, keys.as_ref(), NEIGHBOUR);
    let space = SpaceId(SPACE.into());

    // Before: it opens.
    let lease = keys
        .release(&space, Purpose::Read, "check")
        .expect("release");
    assert_eq!(
        keys.open_at(
            &lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("r1"),
            &memo
        )
        .expect("opens"),
        b"spc_doomed secret".to_vec()
    );
    assert!(store.smk(&space).expect("store").is_some());
    assert!(!store.deks(&space).expect("store").is_empty());

    let erased = erase_space(
        &pool,
        &space,
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        SPACE,
        // No blob tier in this suite — the object half is proved in
        // `ordo-host`'s `tests/exit_ops.rs`, through the op that owns the
        // ordering, and re-proving it here would be a second opinion about
        // a sequence this function is the authority on.
        None,
    )
    .await
    .expect("erasure");

    // The key material is gone from the store, not merely unreachable.
    assert!(
        store.smk(&space).expect("store").is_none(),
        "the space master key survived, so every $bidx digest in a backup is still \
         confirmable by anyone who can recompute HMAC(k_field, guess)"
    );
    assert!(store.deks(&space).expect("store").is_empty());

    // The envelope that escaped into a backup no longer opens — and it
    // does not open even after custody is re-created, because the new SMK
    // is a different key.
    assert!(keys.release(&space, Purpose::Read, "after").is_err());
    keys.create_space(&space).expect("custody, freshly minted");
    let fresh = keys
        .release(&space, Purpose::Read, "after")
        .expect("release");
    assert!(
        keys.open_at(
            &fresh,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("r1"),
            &memo
        )
        .is_err(),
        "a re-created space must not open the erased one's bytes"
    );

    // The rows are gone too, and the neighbour is untouched — in both
    // halves, keys and rows.
    assert_eq!(erased.total_deleted(), 3, "1 vendor + 2 orders");
    assert_eq!(count(&pg, "SELECT count(*) FROM \"orders\""), 2);
    assert_eq!(count(&pg, "SELECT count(*) FROM \"vendors\""), 1);
    let other = SpaceId(NEIGHBOUR.into());
    let other_lease = keys
        .release(&other, Purpose::Read, "check")
        .expect("release");
    assert_eq!(
        keys.open_at(
            &other_lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("r1"),
            &neighbour_memo
        )
        .expect("the bystander still opens"),
        b"spc_bystander secret".to_vec()
    );
}

// ── the ordering that matters ────────────────────────────────────────────

/// Keys first, rows second. When custody refuses, **nothing is deleted** —
/// because the alternative is a space that looks deleted and whose every
/// backup stays openable.
#[tokio::test(flavor = "multi_thread")]
async fn a_custody_refusal_leaves_every_row_where_it_was() {
    let (pg, pool, schema, keys, _store) = env("ordo_exp_erase_custody").await;
    seed(&pg, keys.as_ref(), SPACE);

    struct Refusing;
    impl KeyProvider for Refusing {
        fn release(
            &self,
            _s: &SpaceId,
            _p: Purpose,
            _c: &str,
        ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn blind_index(
            &self,
            _s: &SpaceId,
            _e: &str,
            _f: &str,
            _v: ordo_crypto::BlindValue<'_>,
        ) -> Result<[u8; 32], ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn seal_at(
            &self,
            _l: &ordo_crypto::KeyLease,
            _e: &str,
            _f: &str,
            _a: ordo_crypto::Anchor<'_>,
            _v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        fn open_at(
            &self,
            _l: &ordo_crypto::KeyLease,
            _e: &str,
            _f: &str,
            _a: ordo_crypto::Anchor<'_>,
            _v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        #[allow(clippy::too_many_arguments)]
        fn seal_chunk(
            &self,
            _l: &ordo_crypto::KeyLease,
            _e: &str,
            _f: &str,
            _a: ordo_crypto::Anchor<'_>,
            _ep: u8,
            _b: &ordo_crypto::NonceBase,
            _c: u32,
            _last: bool,
            _v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        #[allow(clippy::too_many_arguments)]
        fn open_chunk(
            &self,
            _l: &ordo_crypto::KeyLease,
            _e: &str,
            _f: &str,
            _a: ordo_crypto::Anchor<'_>,
            _ep: u8,
            _b: &ordo_crypto::NonceBase,
            _c: u32,
            _last: bool,
            _v: &[u8],
        ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::NotLeased)
        }
        fn current_epoch(&self, _s: &SpaceId) -> Result<u8, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn rotate(&self, _s: &SpaceId, _r: &str) -> Result<u8, ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn destroy_epoch(&self, _s: &SpaceId, _e: u8) -> Result<(), ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::UnknownSpace)
        }
        fn destroy_space(&self, _s: &SpaceId) -> Result<(), ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::Store(
                "the KMS is unreachable".into(),
            ))
        }
        fn create_space(&self, _s: &SpaceId) -> Result<(), ordo_crypto::KeyError> {
            Err(ordo_crypto::KeyError::Store(
                "the KMS is unreachable".into(),
            ))
        }
    }

    let err = erase_space(
        &pool,
        &SpaceId(SPACE.into()),
        &schema,
        &Refusing,
        &LogPolicy::DEFAULT,
        NOW,
        SPACE,
        // No blob tier in this suite — the object half is proved in
        // `ordo-host`'s `tests/exit_ops.rs`, through the op that owns the
        // ordering, and re-proving it here would be a second opinion about
        // a sequence this function is the authority on.
        None,
    )
    .await
    .expect_err("custody refused, so the deletion must not proceed");
    assert!(matches!(err, EraseRefusal::Custody(_)), "{err}");
    assert_eq!(
        count(&pg, "SELECT count(*) FROM \"orders\""),
        2,
        "rows were deleted despite custody refusing — which would leave a space that \
         looks deleted and whose backups still open"
    );
}

// ── what refuses, and what merely discloses ──────────────────────────────

/// A consumer that may not be evicted, behind the head, refuses. A stalled
/// mirror does not — the same split `prune` already makes, and for the same
/// reasons.
#[tokio::test(flavor = "multi_thread")]
async fn a_non_evictable_consumer_behind_the_head_refuses_and_a_mirror_does_not() {
    let (pg, pool, schema, keys, _store) = env("ordo_exp_erase_pinned").await;
    seed(&pg, keys.as_ref(), SPACE);
    let space = SpaceId(SPACE.into());
    let sid = ordo_commandlog::SpaceId(SPACE.into());
    for i in 0..3 {
        log_a_group(&pool, SPACE, i).await;
    }

    // A mirror miles behind, and stale. It does not block a prune and it
    // does not block a deletion: it is a cache.
    ordo_exec::prune::register_consumer(
        &pool,
        &sid,
        "cache",
        ConsumerKind::Mirror,
        0,
        NOW - 1_000_000,
    )
    .await
    .expect("register");
    let d = disclose(&pool, &space, &schema, &LogPolicy::DEFAULT, NOW)
        .await
        .expect("disclose");
    assert!(d.pinned.is_empty());
    assert_eq!(d.stranded_mirrors.len(), 1);

    // An archive behind the head does block: it is the copy of record, and
    // the history it has not received dies with the space.
    ordo_exec::prune::register_consumer(&pool, &sid, "coldstore", ConsumerKind::Archive, 0, NOW)
        .await
        .expect("register");
    ordo_exec::prune::advance_consumer(&pool, &sid, "coldstore", 1, NOW)
        .await
        .expect("advance");

    let err = erase_space(
        &pool,
        &space,
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        SPACE,
        // No blob tier in this suite — the object half is proved in
        // `ordo-host`'s `tests/exit_ops.rs`, through the op that owns the
        // ordering, and re-proving it here would be a second opinion about
        // a sequence this function is the authority on.
        None,
    )
    .await
    .expect_err("an archive behind the head pins the deletion");
    match err {
        EraseRefusal::ConsumerPinned { consumers, head } => {
            assert_eq!(head, 3);
            assert_eq!(consumers.len(), 1);
            assert_eq!(consumers[0].name, "coldstore");
            assert_eq!(consumers[0].kind, ConsumerKind::Archive);
        }
        other => panic!("wrong refusal: {other}"),
    }
    assert_eq!(
        count(&pg, "SELECT count(*) FROM \"orders\""),
        2,
        "a refusal deletes nothing"
    );

    // Let it catch up — the remedy the refusal names — and the deletion
    // proceeds, with the stalled mirror still stranded and still not
    // blocking.
    ordo_exec::prune::advance_consumer(&pool, &sid, "coldstore", 3, NOW)
        .await
        .expect("advance");
    let erased = erase_space(
        &pool,
        &space,
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        SPACE,
        // No blob tier in this suite — the object half is proved in
        // `ordo-host`'s `tests/exit_ops.rs`, through the op that owns the
        // ordering, and re-proving it here would be a second opinion about
        // a sequence this function is the authority on.
        None,
    )
    .await
    .expect("a caught-up archive does not pin");
    assert_eq!(erased.disclosure.stranded_mirrors.len(), 1);
}

/// The undo horizon is **disclosed, not enforced**: it is the owner's
/// guarantee, and deletion is the owner spending it. Prune refuses to cut
/// inside the horizon; deletion tells you the number and proceeds on the
/// confirmation.
#[tokio::test(flavor = "multi_thread")]
async fn the_undo_horizon_is_a_number_on_the_report_and_not_a_refusal() {
    let (pg, pool, schema, keys, _store) = env("ordo_exp_erase_horizon").await;
    seed(&pg, keys.as_ref(), SPACE);
    let space = SpaceId(SPACE.into());
    for i in 0..4 {
        log_a_group(&pool, SPACE, i).await;
    }

    // Every group was written ten seconds ago, so every one of them is
    // inside a ninety-day horizon.
    let d = disclose(&pool, &space, &schema, &LogPolicy::DEFAULT, NOW)
        .await
        .expect("disclose");
    assert_eq!(d.log_groups, 4);
    assert_eq!(
        d.inside_undo_horizon, 4,
        "the owner is told exactly how much revertable history they are ending"
    );
    assert_eq!(d.total_rows(), 3);
    assert_eq!(
        d.sealed_payloads, 0,
        "these rows carry their envelopes inline"
    );
    // Every entity the schema has, including the compiler's own platform
    // entities — the disclosure is derived, so it enumerates what the
    // program holds rather than what this test remembered to list.
    assert_eq!(
        d.rows,
        vec![
            ("groupMembers".to_string(), 0),
            ("members".to_string(), 0),
            ("orders".to_string(), 2),
            ("vendors".to_string(), 1),
        ]
    );

    erase_space(
        &pool,
        &space,
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        SPACE,
        // No blob tier in this suite — the object half is proved in
        // `ordo-host`'s `tests/exit_ops.rs`, through the op that owns the
        // ordering, and re-proving it here would be a second opinion about
        // a sequence this function is the authority on.
        None,
    )
    .await
    .expect("the horizon does not refuse — the owner's guarantee is theirs to spend");
    assert_eq!(count(&pg, "SELECT count(*) FROM \"orders\""), 0);
}

/// The confirmation is the space's own id typed back. Not a boolean, which
/// a retried request supplies for free; not a token the platform hands
/// out, which would make the platform the party that decided.
#[tokio::test(flavor = "multi_thread")]
async fn the_confirmation_is_the_spaces_own_id() {
    let (pg, pool, schema, keys, _store) = env("ordo_exp_erase_confirm").await;
    seed(&pg, keys.as_ref(), SPACE);

    for wrong in ["", "yes", "true", NEIGHBOUR] {
        let err = erase_space(
            &pool,
            &SpaceId(SPACE.into()),
            &schema,
            keys.as_ref(),
            &LogPolicy::DEFAULT,
            NOW,
            wrong,
            None,
        )
        .await
        .expect_err("`{wrong}` is not a confirmation");
        assert!(matches!(err, EraseRefusal::NotConfirmed { .. }), "{err}");
    }
    assert_eq!(count(&pg, "SELECT count(*) FROM \"orders\""), 2);
}

// ── the sweep, and the exception ─────────────────────────────────────────

/// The platform's own space-scoped tables are erased **without being
/// named** — they are discovered from `information_schema` — and the
/// security-event stream is the one thing deliberately left, for the
/// reasons on [`RETAINED`].
#[tokio::test(flavor = "multi_thread")]
async fn platform_rows_are_swept_by_discovery_and_the_security_stream_survives() {
    let (pg, pool, schema, keys, _store) = env("ordo_exp_erase_sweep").await;
    seed(&pg, keys.as_ref(), SPACE);
    seed(&pg, keys.as_ref(), NEIGHBOUR);
    let space = SpaceId(SPACE.into());
    let sid = ordo_commandlog::SpaceId(SPACE.into());
    log_a_group(&pool, SPACE, 0).await;
    log_a_group(&pool, NEIGHBOUR, 0).await;
    ordo_exec::prune::register_consumer(&pool, &sid, "cache", ConsumerKind::Mirror, 0, NOW)
        .await
        .expect("register");

    // A security event for this space, and one for the deployment. Neither
    // may be swept.
    for scope in [SPACE, "deployment"] {
        pg.exec(&format!(
            "INSERT INTO \"$secevents\" \
             (\"scope\", \"idx\", \"subject\", \"subject_kind\", \"op\", \"kind\", \
              \"first_at\", \"last_at\", \"count\", \"prev\", \"hash\", \"body\") \
             VALUES ('{scope}', 0, 'usr_owner', 'user', 'std.space.delete', 'CustodyAction', \
                     1, 1, 1, '\\x00'::bytea, '\\x01'::bytea, '\\x02'::bytea)"
        ))
        .expect("security event");
    }

    // A platform table nobody wrote this module to know about. It must be
    // swept anyway, because the sweep asks the catalog rather than a list.
    pg.exec("CREATE TABLE \"$madeuplater\" (\"space\" text NOT NULL, \"n\" bigint NOT NULL)")
        .expect("a future platform table");
    pg.exec(&format!(
        "INSERT INTO \"$madeuplater\" VALUES ('{SPACE}', 1), ('{NEIGHBOUR}', 2)"
    ))
    .expect("rows");

    let erased = erase_space(
        &pool,
        &space,
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        SPACE,
        // No blob tier in this suite — the object half is proved in
        // `ordo-host`'s `tests/exit_ops.rs`, through the op that owns the
        // ordering, and re-proving it here would be a second opinion about
        // a sequence this function is the authority on.
        None,
    )
    .await
    .expect("erasure");

    let swept: Vec<&str> = erased.deleted.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        swept.contains(&"$madeuplater"),
        "a space-scoped platform table this module never heard of was left behind: {swept:?}"
    );
    assert!(swept.contains(&"$commandlog") && swept.contains(&"$logconsumers"));
    assert!(
        RETAINED.iter().all(|r| !swept.contains(r)),
        "the security stream must not be swept: {swept:?}"
    );

    assert_eq!(
        count(&pg, "SELECT count(*) FROM \"$madeuplater\""),
        1,
        "the neighbour's row stayed"
    );
    assert_eq!(
        count(&pg, "SELECT count(*) FROM \"$commandlog\""),
        1,
        "the neighbour's chain stayed"
    );
    assert_eq!(
        count(&pg, "SELECT count(*) FROM \"$logconsumers\""),
        0,
        "the deleted space's cursors went with it"
    );
    assert_eq!(
        count(&pg, "SELECT count(*) FROM \"$secevents\""),
        2,
        "the security stream is a hash chain whose prefix would include this very \
         deletion, and it is the operator's record rather than the tenant's"
    );
    assert_eq!(erased.retained, RETAINED.to_vec());
}

/// Every ref is a foreign key on `($space, id)`, so a deletion that ran a
/// sequence of `DELETE`s would have to order children before parents. It
/// runs one statement instead, and this is the assertion that it does:
/// `orders` references `vendors`, and both go in the same breath.
#[tokio::test(flavor = "multi_thread")]
async fn referenced_and_referencing_rows_go_in_one_statement() {
    let (pg, pool, schema, keys, _store) = env("ordo_exp_erase_refs").await;
    seed(&pg, keys.as_ref(), SPACE);

    // Sanity: the FK really is there and really would refuse a
    // parents-first delete, so the test below is about the mechanism and
    // not about an absent constraint.
    assert!(
        pg.exec(&format!(
            "DELETE FROM \"vendors\" WHERE \"$space\" = '{SPACE}'"
        ))
        .is_err(),
        "the fixture's foreign key is missing, so this test proves nothing"
    );

    let erased = erase_space(
        &pool,
        &SpaceId(SPACE.into()),
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        SPACE,
        // No blob tier in this suite — the object half is proved in
        // `ordo-host`'s `tests/exit_ops.rs`, through the op that owns the
        // ordering, and re-proving it here would be a second opinion about
        // a sequence this function is the authority on.
        None,
    )
    .await
    .expect("one statement, no ordering problem");
    assert_eq!(erased.total_deleted(), 3);
    assert_eq!(count(&pg, "SELECT count(*) FROM \"vendors\""), 0);
    assert_eq!(count(&pg, "SELECT count(*) FROM \"orders\""), 0);
}

// ---- the system space has no exit (2026-09-12) -------------------------

/// **A platform-owned space cannot be erased, and the refusal comes before
/// the confirmation.**
///
/// Every other space carries the opposite guarantee: exit ops always
/// exist, they are owner-only, and typing the id back is the whole gate.
/// That made "nobody erases the system space" a hope rather than a
/// mechanism, because a system space has an owner like any other and the
/// catalogue every tenant reads would go with it, chain included.
///
/// The confirmation is checked *after* this, deliberately. Typing the id
/// back proves intent, and this is not a question of intent: a correctly
/// confirmed request is refused exactly as an unconfirmed one is, so the
/// refusal can never be read as "you asked wrong".
#[tokio::test(flavor = "multi_thread")]
async fn the_system_space_has_no_exit() {
    let (_pg, pool, schema, keys, _store) = env("ordo_erase_system").await;
    let sys = SpaceId("$catalogue".into());

    // Correctly confirmed, and still refused.
    let refused = erase_space(
        &pool,
        &sys,
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        "$catalogue",
        None,
    )
    .await
    .expect_err("a system space has no exit");
    assert!(
        matches!(refused, EraseRefusal::SystemSpace { .. }),
        "expected the system-space refusal, got: {refused}"
    );
    assert!(refused.to_string().contains("not a tenant"), "{refused}");

    // **The space the boot creates is covered by the same gate** (17v).
    // It carries no `$`, because the door has to be able to bind it, so
    // the prefix alone would have left the one system space every
    // deployment actually has erasable by its own owner.
    let booted = SpaceId(ordo_exec::erase::SYSTEM_SPACE.to_string());
    let refused = erase_space(
        &pool,
        &booted,
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        ordo_exec::erase::SYSTEM_SPACE,
        None,
    )
    .await
    .expect_err("the deployment's own system space has no exit either");
    assert!(
        matches!(refused, EraseRefusal::SystemSpace { .. }),
        "expected the system-space refusal, got: {refused}"
    );

    // And an ordinary space is untouched by the new gate: it still reaches
    // the confirmation check, which is what proves the gate is narrow.
    let wrong = erase_space(
        &pool,
        &SpaceId(SPACE.into()),
        &schema,
        keys.as_ref(),
        &LogPolicy::DEFAULT,
        NOW,
        "not-the-id",
        None,
    )
    .await
    .expect_err("a wrong confirmation is still a refusal");
    assert!(
        matches!(wrong, EraseRefusal::NotConfirmed { .. }),
        "the gate must not swallow ordinary spaces: {wrong}"
    );
}
