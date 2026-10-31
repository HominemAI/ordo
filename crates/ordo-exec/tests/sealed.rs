//! 8j: the sealed payload store — what a `SealedRef` resolves through.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`). Every
//! test owns its own database: they run in parallel against one cluster and
//! all of them reset the fixture schema, so a shared database would make
//! one test's seed another test's truncation.
//!
//! What has to be true for 8j to count as landed:
//!
//! - a handle is **random**, not a counter and not a digest of anything the
//!   store holds — proved negatively, against every content hash a
//!   content-addressed scheme would plausibly have used;
//! - a sealed write's image **resolves**, through a key release, to the
//!   plaintext that was written;
//! - the store **never deduplicates**: one value written twice is two
//!   handles over two rows holding two different envelopes;
//! - a revert whose before-image carries a sealed value **succeeds**, where
//!   the whole entity class used to be refused;
//! - and the two ways a payload can genuinely be gone — pruned, or its
//!   epoch key destroyed — still refuse, each naming itself.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp, Value};
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_crypto::{Purpose, SealedHandle, SpaceId as CryptoSpace};
use ordo_exec::sealed::{self, Reader, Resolved};
use ordo_exec::{undo, Executor, LoggedExecutor, Pool, PoolConfig};
use ordo_schema::Schema;

// ── plumbing ─────────────────────────────────────────────────────────────

struct Env {
    pool: Pool,
    fixture: Arc<Fixture>,
    schema: Schema,
    log: Arc<Mutex<WriteLog>>,
}

/// A database of this test's own: the market fixture seeded, the chain
/// empty, and the platform tables (`$sealedpayload` among them) created.
async fn env(db: &str) -> Env {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    pool.rows("DELETE FROM \"$checkpoints\"").await.unwrap();
    pool.rows("DELETE FROM \"$sealedpayload\"").await.unwrap();
    let schema = f.schema.clone();
    Env {
        pool,
        fixture: Arc::new(f),
        schema,
        log: Arc::new(Mutex::new(WriteLog::new())),
    }
}

fn space() -> SpaceId {
    SpaceId("conformance".into())
}

fn crypto_space() -> CryptoSpace {
    CryptoSpace("conformance".into())
}

impl Env {
    fn executor(&self) -> LoggedExecutor {
        LoggedExecutor::new(
            Executor {
                pool: self.pool.clone(),
                schema: self.fixture.schema.clone(),
                registry: self.fixture.registry.clone(),
                keys: self.fixture.keys.clone(),
                space: ordo_conformance::fixture_space(),
            },
            "conformance",
        )
    }

    async fn write(&self, call: &str, caller: &str) {
        self.executor()
            .write_as(
                self.fixture.clone(),
                self.log.clone(),
                call.to_string(),
                caller.to_string(),
            )
            .await
            .unwrap_or_else(|e| panic!("write `{call}`: {e}"));
    }

    /// The group at a chain index, decoded.
    async fn group(&self, idx: usize) -> ordo_commandlog::CommandGroup {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = 'conformance' \
                 AND \"idx\" = {idx}"
            ))
            .await
            .unwrap();
        let hex = rows[0][0].clone().expect("a group at this index");
        let body = ordo_crypto::sealed::unhex(&hex).expect("a body");
        ordo_commandlog::chain::decode_group(&body).expect("decodes")
    }

    async fn correlation(&self, idx: usize) -> CorrelationId {
        CorrelationId(self.group(idx).await.correlation.0)
    }

    /// A write release for opening payloads in the fixture's space.
    fn lease(&self) -> ordo_crypto::KeyLease {
        self.fixture
            .keys
            .release(&crypto_space(), Purpose::Write, "test")
            .expect("release")
    }

    async fn resolve(&self, handle: &SealedHandle) -> Resolved {
        sealed::resolve(
            Reader::Pool(&self.pool),
            self.fixture.keys.as_ref(),
            &self.lease(),
            &crypto_space(),
            handle,
        )
        .await
        .expect("the store answers")
    }

    async fn one(&self, sql: &str) -> Option<String> {
        self.pool.rows(sql).await.unwrap().first()?.first()?.clone()
    }

    /// The `orders.memo` envelope on a row, opened.
    async fn memo_of(&self, id: &str) -> Option<String> {
        let raw = self
            .one(&format!(
                "SELECT encode(\"memo\", 'hex') FROM \"orders\" WHERE \"id\" = '{id}'"
            ))
            .await?;
        let bytes = ordo_crypto::sealed::unhex(&format!("\\x{raw}"))?;
        if bytes.is_empty() {
            return None;
        }
        let plain = self
            .fixture
            .keys
            .open_at(
                &self.lease(),
                "orders",
                "memo",
                ordo_crypto::Anchor::Row(id),
                &bytes,
            )
            .expect("the row's own envelope opens");
        Some(String::from_utf8(plain).unwrap())
    }
}

/// The handle a command's image carries for one field.
fn handle_in(image: &ordo_commandlog::Image, field: &str) -> SealedHandle {
    match image.get(field) {
        Some(Value::SealedRef(h)) => *h,
        other => panic!("`{field}` is not a sealed handle: {other:?}"),
    }
}

// ── the handle ───────────────────────────────────────────────────────────

/// **The handle is not derived from what it names**, proved through the
/// whole write path rather than at the unit boundary.
///
/// The same plaintext is written twice; the two images carry two different
/// handles, the store holds two rows, and the two stored envelopes differ
/// (a fresh nonce per seal). Then the negative half: neither handle is any
/// digest a content-addressed scheme would have used, over any material a
/// holder of the database actually has.
///
/// That negative half is the whole of §7's objection. A content hash makes
/// the log an unsalted, permanent equality index over every sealed field —
/// in the *plaintext* envelope, readable at every custody position, and
/// surviving crypto-erasure, because destroying a key does not touch the
/// skeleton. The property this asserts is exactly that no such index
/// exists.
#[tokio::test(flavor = "multi_thread")]
async fn one_value_written_twice_is_two_handles_over_two_rows() {
    use sha2::{Digest, Sha256};
    let e = env("ordo_seal_nodedup").await;

    let memo = "leave at the side door";
    e.write(
        &format!(r#"setMemo($order: "ord_1", $memo: "{memo}")"#),
        "mem_1",
    )
    .await;
    e.write(
        &format!(r#"setMemo($order: "ord_2", $memo: "{memo}")"#),
        "mem_1",
    )
    .await;

    let first = handle_in(e.group(0).await.commands[0].after.as_ref().unwrap(), "memo");
    let second = handle_in(e.group(1).await.commands[0].after.as_ref().unwrap(), "memo");
    assert_ne!(
        first, second,
        "two writes of one plaintext must never share a handle"
    );

    // Two rows, not one: the store does not deduplicate, and it has no way
    // to — there is no unique index on the payload and no upsert anywhere.
    assert_eq!(
        e.one("SELECT count(*) FROM \"$sealedpayload\" WHERE \"field\" = 'memo'")
            .await
            .as_deref(),
        Some("2")
    );
    let envelopes = e
        .pool
        .rows("SELECT encode(\"payload\", 'hex') FROM \"$sealedpayload\" WHERE \"field\" = 'memo'")
        .await
        .unwrap();
    assert_ne!(
        envelopes[0][0], envelopes[1][0],
        "a fresh nonce per seal: equal values, unequal bytes at rest"
    );

    // Both resolve to the value that was written.
    for h in [first, second] {
        assert_eq!(
            e.resolve(&h).await,
            Resolved::Payload(memo.as_bytes().to_vec())
        );
    }

    // And neither handle is a function of anything the database holds.
    let sha16 = |bytes: &[u8]| {
        let d = Sha256::digest(bytes);
        let mut out = [0u8; 16];
        out.copy_from_slice(&d[..16]);
        out
    };
    let mut candidates: Vec<(String, [u8; 16])> = vec![
        ("sha256(plaintext)".into(), sha16(memo.as_bytes())),
        ("sha256(entity:field)".into(), sha16(b"orders:memo")),
    ];
    for row in &envelopes {
        let bytes = ordo_crypto::sealed::unhex(&format!("\\x{}", row[0].as_deref().unwrap()))
            .expect("stored envelope");
        candidates.push(("sha256(stored envelope)".into(), sha16(&bytes)));
        candidates.push((
            "the envelope's first 16 bytes".into(),
            <[u8; 16]>::try_from(&bytes[..16]).unwrap(),
        ));
        candidates.push((
            "the envelope's last 16 bytes".into(),
            <[u8; 16]>::try_from(&bytes[bytes.len() - 16..]).unwrap(),
        ));
    }
    for h in [first, second] {
        for (what, candidate) in &candidates {
            assert_ne!(&h, candidate, "the handle is {what}");
        }
        // Not a counter either. That is what it used to be — `h[8..] =
        // log.next()` — and a counter-derived handle has a zero half.
        assert!(
            h[..8].iter().any(|b| *b != 0),
            "a counter-derived handle has a zero half"
        );
    }
}

// ── resolution ───────────────────────────────────────────────────────────

/// **A sealed write's image resolves.** The loop 8j existed to close: a
/// write seals a value into the row, the image carries a handle, and the
/// handle produces the original plaintext through a key release.
///
/// Also asserts the two things the store records beside the payload, both
/// of which are load-bearing rather than decorative: the epoch (so an
/// operator can count what destroying a key would strand) and the
/// entity/field the envelope is AAD-bound to (so a payload moved between
/// columns refuses to open rather than opening into the wrong place).
#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_image_resolves_to_the_plaintext_that_was_written() {
    let e = env("ordo_seal_resolve").await;
    e.write(
        r#"setMemo($order: "ord_1", $memo: "gift for June")"#,
        "mem_1",
    )
    .await;

    let after = e.group(0).await.commands[0].after.clone().unwrap();
    let handle = handle_in(&after, "memo");

    assert_eq!(
        e.resolve(&handle).await,
        Resolved::Payload(b"gift for June".to_vec()),
        "the handle in the image names the value the write sealed"
    );

    let stored = sealed::lookup(Reader::Pool(&e.pool), &crypto_space(), &handle)
        .await
        .unwrap()
        .expect("the payload is stored");
    assert_eq!(stored.entity, "orders");
    assert_eq!(stored.field, "memo");
    assert_eq!(
        stored.epoch,
        e.fixture.keys.current_epoch(&crypto_space()).unwrap(),
        "the payload records the epoch that sealed it"
    );
    assert_eq!(
        stored.bytes[0], stored.epoch,
        "and the stored envelope is the envelope, not a re-seal of it"
    );

    // The image never carried the value. The canonical encoding the chain
    // hashed is the proof: the plaintext is not in it anywhere.
    let body = ordo_commandlog::encode_group(&e.group(0).await);
    assert!(
        !body
            .windows(b"gift for June".len())
            .any(|w| w == b"gift for June"),
        "plaintext in the chain's own bytes"
    );

    // A handle from another space resolves to nothing. The lookup is keyed
    // on `("$space", "ref")`, so a handle is not transferable between
    // tenants even when it is known.
    let elsewhere = sealed::lookup(
        Reader::Pool(&e.pool),
        &CryptoSpace("some-other-tenant".into()),
        &handle,
    )
    .await
    .unwrap();
    assert!(elsewhere.is_none(), "a handle does not cross spaces");
}

// ── the revert this unblocks ─────────────────────────────────────────────

/// **The refusal narrowed.** `orders` has a sealed `memo`, so until 8j the
/// whole entity was unrevertable: `refuse_unreproducible` turned away any
/// revert touching an entity with a sealed field, and
/// `ordo_commandlog::invert_group` refused any image holding a handle. Both
/// were right at the time — nothing persisted the payloads — and both are
/// wrong now.
///
/// This reverts a converge whose before-image is a sealed value, with real
/// custody, against real rows, and checks the row came back. It also checks
/// the same revert without custody, which is what `ordo revert` itself
/// passes: still refused, but for the reason that is actually true.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_restores_a_sealed_value_that_used_to_be_unrevertable() {
    let e = env("ordo_seal_revert").await;
    e.write(r#"setMemo($order: "ord_1", $memo: "original")"#, "mem_1")
        .await;
    e.write(r#"setMemo($order: "ord_1", $memo: "changed")"#, "mem_1")
        .await;
    assert_eq!(e.memo_of("ord_1").await.as_deref(), Some("changed"));

    let second = e.correlation(1).await;

    // Without custody: refused, and the refusal says what is missing.
    // `ordo revert` is this caller — an operator tool with database
    // credentials and no `KeyProvider` — so this is the CLI's own answer.
    let keyless: Option<&dyn undo::KeyProvider> = None;
    let refused = undo::revert(
        &e.pool,
        &e.schema,
        keyless,
        None,
        None,
        &space(),
        &second,
        "mem_1",
        Timestamp(1),
    )
    .await
    .expect_err("no keys, no sealed revert");
    let msg = refused.to_string();
    assert!(msg.starts_with("unrevertable_sealed"), "{msg}");
    assert!(
        msg.contains("no key custody is wired into this caller"),
        "the refusal must name what is missing, not a gap that has been \
         closed: {msg}"
    );

    // With custody: the revert lands.
    let reverted = undo::revert(
        &e.pool,
        &e.schema,
        Some(e.fixture.keys.as_ref()),
        None,
        None,
        &space(),
        &second,
        "mem_1",
        Timestamp(2),
    )
    .await
    .expect("a sealed before-image is reproducible now");

    assert_eq!(
        e.memo_of("ord_1").await.as_deref(),
        Some("original"),
        "the sealed value came back"
    );
    assert_eq!(
        e.one("SELECT \"rev\" FROM \"orders\" WHERE \"id\" = 'ord_1'")
            .await
            .as_deref(),
        Some("4"),
        "`rev` moves FORWARD: a revert is a write, not a rewind — the row \
         was at 3 after two writes and is at 4 after undoing one"
    );

    // A revert is a write, so the value it left behind is sealed the way a
    // write of that value today would seal it — a fresh envelope, not the
    // one the store was holding.
    let restored = e
        .one("SELECT encode(\"memo\", 'hex') FROM \"orders\" WHERE \"id\" = 'ord_1'")
        .await
        .unwrap();
    let originals = e
        .pool
        .rows(&format!(
            "SELECT encode(\"payload\", 'hex') FROM \"$sealedpayload\" \
                 WHERE \"field\" = 'memo' AND \"correlation\" = '{}'",
            second.0
        ))
        .await
        .unwrap();
    for row in &originals {
        assert_ne!(
            row[0].as_deref(),
            Some(restored.as_str()),
            "a revert re-seals with a fresh nonce; it does not replay an envelope"
        );
    }

    // The revert is itself history, and its own images are complete: both
    // sides carry handles that resolve, so reverting the revert would work
    // too. An image that silently omitted the sealed field is exactly what
    // made this class unrevertable in the first place.
    let cmd = &reverted.group.commands[0];
    assert_eq!(
        e.resolve(&handle_in(cmd.before.as_ref().unwrap(), "memo"))
            .await,
        Resolved::Payload(b"changed".to_vec()),
        "the recorded before-image names what the row held"
    );
    assert_eq!(
        e.resolve(&handle_in(cmd.after.as_ref().unwrap(), "memo"))
            .await,
        Resolved::Payload(b"original".to_vec()),
        "and the after-image names what it holds now"
    );

    // The chain still verifies over the whole thing, revert included.
    undo::verify(
        undo::Db::Pool(&e.pool),
        &space(),
        undo::Anchor::Genesis,
        ordo_commandlog::ChainHash::GENESIS,
    )
    .await
    .expect("the chain verifies");
}

/// A sealed CREATE round-trips too, through the indexed path: reverting a
/// create deletes the row, and reverting that delete puts it back with its
/// sealed value **and its blind index** — which the revert recomputes from
/// the resolved plaintext rather than copying from history, because a
/// digest copied out of the log would be an equality index over historical
/// values, which is the thing the whole design refuses.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_row_gets_its_blind_index_back_and_stays_findable() {
    let e = env("ordo_seal_bidx").await;
    e.write(
        r#"addCustomer($name: "Zed", $email: "Zed@Example.com")"#,
        "mem_1",
    )
    .await;
    let created = e.group(0).await;
    let row_id = created.commands[0].row_id.clone();
    let digest_before = e
        .one(&format!(
            "SELECT encode(\"email$bidx\", 'hex') FROM \"customers\" WHERE \"id\" = '{row_id}'"
        ))
        .await
        .expect("a digest");

    // Revert the create: the row goes.
    let first = e.correlation(0).await;
    undo::revert(
        &e.pool,
        &e.schema,
        Some(e.fixture.keys.as_ref()),
        None,
        None,
        &space(),
        &first,
        "mem_1",
        Timestamp(10),
    )
    .await
    .expect("the create inverts to a delete");
    assert_eq!(
        e.one(&format!(
            "SELECT count(*) FROM \"customers\" WHERE \"id\" = '{row_id}'"
        ))
        .await
        .as_deref(),
        Some("0")
    );

    // Revert the revert: the row comes back, sealed value and all.
    let undone = e.correlation(1).await;
    undo::revert(
        &e.pool,
        &e.schema,
        Some(e.fixture.keys.as_ref()),
        None,
        None,
        &space(),
        &undone,
        "mem_1",
        Timestamp(11),
    )
    .await
    .expect("the delete inverts to a create");

    assert_eq!(
        e.one(&format!(
            "SELECT encode(\"email$bidx\", 'hex') FROM \"customers\" WHERE \"id\" = '{row_id}'"
        ))
        .await,
        Some(digest_before),
        "the blind index is recomputed from the plaintext — same value, \
         same normalization, same digest, so the row is findable again"
    );
    // Found by equality, which is the property the digest exists for.
    let found = e
        .one(
            "SELECT \"name\" FROM \"customers\" WHERE \"email$bidx\" = \
              (SELECT \"email$bidx\" FROM \"customers\" WHERE \"name\" = 'Zed')",
        )
        .await;
    assert_eq!(found.as_deref(), Some("Zed"));
}

// ── the two honest refusals ──────────────────────────────────────────────

/// The refusal that remains, in both its forms, each naming itself.
///
/// This is what "narrowed" means: not "sealed fields are unrevertable" but
/// "this payload is gone, and here is which way".
#[tokio::test(flavor = "multi_thread")]
async fn a_pruned_payload_and_a_destroyed_epoch_refuse_differently() {
    let e = env("ordo_seal_gone").await;
    e.write(r#"setMemo($order: "ord_1", $memo: "first")"#, "mem_1")
        .await;
    e.write(r#"setMemo($order: "ord_1", $memo: "second")"#, "mem_1")
        .await;
    let second = e.correlation(1).await;

    // (1) Pruned: the payload row is gone, which is what a prune of the
    //     group that minted it leaves behind.
    // The handle the revert will have to RESOLVE: the second group's
    // before-image, which is the value it would write back. Note this is a
    // *different* handle from the first group's after-image even though
    // both name the same value — the store does not deduplicate, so each
    // image got its own row, and deleting one does not touch the other.
    let target = handle_in(
        e.group(1).await.commands[0].before.as_ref().unwrap(),
        "memo",
    );
    assert!(matches!(e.resolve(&target).await, Resolved::Payload(_)));
    e.pool
        .rows(&format!(
            "DELETE FROM \"$sealedpayload\" WHERE \"ref\" = '\\x{}'::bytea",
            ordo_crypto::hex(&target)
        ))
        .await
        .unwrap();
    assert_eq!(e.resolve(&target).await, Resolved::Pruned);

    let refused = undo::revert(
        &e.pool,
        &e.schema,
        Some(e.fixture.keys.as_ref()),
        None,
        None,
        &space(),
        &second,
        "mem_1",
        Timestamp(20),
    )
    .await
    .expect_err("a revert cannot invent a value it cannot read");
    let msg = refused.to_string();
    assert!(msg.contains("has been pruned"), "{msg}");
    assert!(msg.contains("memo"), "the refusal names the field: {msg}");
    assert_eq!(
        e.memo_of("ord_1").await.as_deref(),
        Some("second"),
        "and nothing was applied — a half-restored row is worse than none"
    );

    // (2) Destroyed: the payload is there, and its epoch key is not.
    //     Crypto-erasure, working exactly as designed; §7 gates it on the
    //     undo horizon precisely so this stays rare rather than impossible.
    let still_there = handle_in(e.group(1).await.commands[0].after.as_ref().unwrap(), "memo");
    let epoch = e.fixture.keys.current_epoch(&crypto_space()).unwrap();
    e.fixture
        .keys
        .rotate(&crypto_space(), "test")
        .expect("rotate");
    e.fixture
        .keys
        .destroy_epoch(&crypto_space(), epoch)
        .expect("destroy the now-retired epoch");
    assert_eq!(e.resolve(&still_there).await, Resolved::Destroyed(epoch));
    assert!(Resolved::Destroyed(epoch)
        .refusal()
        .unwrap()
        .contains("destroyed"));
}

/// A key store that is busy or broken answers with the store's own fault.
/// The payload and its key are both still there; reading that as a
/// destroyed key would report crypto-erasure that never happened.
struct FailingKeys {
    inner: Arc<dyn ordo_crypto::KeyProvider>,
    fault: ordo_crypto::KeyError,
}

impl ordo_crypto::KeyProvider for FailingKeys {
    fn release(
        &self,
        s: &CryptoSpace,
        p: Purpose,
        c: &str,
    ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
        self.inner.release(s, p, c)
    }
    fn seal_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.inner.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        _: &ordo_crypto::KeyLease,
        _: &str,
        _: &str,
        _: ordo_crypto::Anchor<'_>,
        _: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        Err(self.fault.clone())
    }
    fn blind_index(
        &self,
        s: &CryptoSpace,
        e: &str,
        f: &str,
        v: ordo_crypto::BlindValue<'_>,
    ) -> Result<[u8; 32], ordo_crypto::KeyError> {
        self.inner.blind_index(s, e, f, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: ordo_crypto::Anchor<'_>,
        ep: ordo_crypto::Epoch,
        b: &ordo_crypto::NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.inner.seal_chunk(l, e, f, a, ep, b, c, last, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        _: &ordo_crypto::KeyLease,
        _: &str,
        _: &str,
        _: ordo_crypto::Anchor<'_>,
        _: ordo_crypto::Epoch,
        _: &ordo_crypto::NonceBase,
        _: u32,
        _: bool,
        _: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        Err(self.fault.clone())
    }
    fn current_epoch(&self, s: &CryptoSpace) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.inner.current_epoch(s)
    }
    fn rotate(
        &self,
        s: &CryptoSpace,
        r: &str,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.inner.rotate(s, r)
    }
    fn create_space(&self, s: &CryptoSpace) -> Result<(), ordo_crypto::KeyError> {
        self.inner.create_space(s)
    }
    fn destroy_epoch(
        &self,
        s: &CryptoSpace,
        e: ordo_crypto::Epoch,
    ) -> Result<(), ordo_crypto::KeyError> {
        self.inner.destroy_epoch(s, e)
    }
    fn destroy_space(&self, s: &CryptoSpace) -> Result<(), ordo_crypto::KeyError> {
        self.inner.destroy_space(s)
    }
}

/// Audit bug 4: a key store fault, busy included, is never a destroyed key.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_or_broken_key_store_is_never_a_destroyed_key() {
    let e = env("ordo_seal_store_fault").await;
    e.write(r#"setMemo($order: "ord_1", $memo: "first")"#, "mem_1")
        .await;
    let handle = handle_in(e.group(0).await.commands[0].after.as_ref().unwrap(), "memo");
    assert!(matches!(e.resolve(&handle).await, Resolved::Payload(_)));

    let lease = e.lease();
    let resolve_under = |fault: ordo_crypto::KeyError| {
        let keys = FailingKeys {
            inner: e.fixture.keys.clone(),
            fault,
        };
        let (pool, lease) = (e.pool.clone(), lease.clone());
        async move {
            sealed::resolve(Reader::Pool(&pool), &keys, &lease, &crypto_space(), &handle).await
        }
    };

    // Busy: answered as the pool's own busy, so the caller says
    // `server_busy` and the request can be sent again.
    let busy = resolve_under(ordo_crypto::KeyError::of_class(
        ordo_fault::FaultClass::Busy,
        ordo_exec::SATURATED_MESSAGE,
    ))
    .await;
    assert!(
        matches!(busy, Err(ordo_exec::PoolError::Saturated)),
        "{busy:?}"
    );

    // Broken: a store failure, not erasure.
    let broken = resolve_under(ordo_crypto::KeyError::Store("connection reset".into())).await;
    match broken {
        Err(ordo_exec::PoolError::Db(m)) => assert!(m.contains("connection reset"), "{m}"),
        other => panic!("a broken key store is a store failure: {other:?}"),
    }

    // An envelope that fails with its key present is unreadable, possibly
    // tampered, and never "destroyed" (owner, 2026-10-23); only a missing
    // epoch is destroyed.
    let epoch = e.fixture.keys.current_epoch(&crypto_space()).unwrap();
    let unreadable = resolve_under(ordo_crypto::KeyError::Seal).await.unwrap();
    assert_eq!(unreadable, Resolved::Unreadable(epoch));
    let why = unreadable.refusal().unwrap();
    assert!(
        why.contains("unreadable") && !why.contains("destroyed"),
        "{why}"
    );
    assert_eq!(
        resolve_under(ordo_crypto::KeyError::UnknownEpoch(epoch))
            .await
            .unwrap(),
        Resolved::Destroyed(epoch)
    );
}

// ── garbage ──────────────────────────────────────────────────────────────

/// The garbage story, run rather than described.
///
/// A payload is reachable only through the image that names it, so it is
/// garbage exactly when its command group is. Collection is the anti-join
/// against `$commandlog`. This test drives it directly, standing in for a
/// prune; `ordo_exec::prune::prune` is what schedules it for real, calling
/// `collect_pruned_in` in the same transaction as the delete.
#[tokio::test(flavor = "multi_thread")]
async fn collection_takes_exactly_the_payloads_a_prune_orphaned() {
    let e = env("ordo_seal_gc").await;
    e.write(r#"setMemo($order: "ord_1", $memo: "keep me")"#, "mem_1")
        .await;
    e.write(r#"setMemo($order: "ord_2", $memo: "prune me")"#, "mem_1")
        .await;

    let space = crypto_space();
    assert_eq!(
        sealed::space_payload_count(&e.pool, &space).await.unwrap(),
        2
    );

    // Nothing has been pruned, so collection takes nothing. A collector
    // that ate live payloads would be far worse than one that never ran.
    assert_eq!(sealed::collect_pruned(&e.pool, &space).await.unwrap(), 0);
    assert_eq!(
        sealed::space_payload_count(&e.pool, &space).await.unwrap(),
        2
    );

    // Prune the second group the way a checkpoint-anchored prune would:
    // its chain entry goes, and its payloads become unnameable.
    let pruned = e.correlation(1).await;
    e.pool
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = 'conformance' \
             AND \"correlation\" = '{}'",
            pruned.0
        ))
        .await
        .unwrap();
    assert_eq!(sealed::collect_pruned(&e.pool, &space).await.unwrap(), 1);

    // The surviving group's payload is untouched and still resolves.
    let kept = handle_in(e.group(0).await.commands[0].after.as_ref().unwrap(), "memo");
    assert_eq!(
        e.resolve(&kept).await,
        Resolved::Payload(b"keep me".to_vec())
    );
    assert_eq!(
        sealed::space_payload_count(&e.pool, &space).await.unwrap(),
        1
    );

    // Another tenant's payloads are never in scope: collection is per
    // space, like everything else the store does.
    assert_eq!(
        sealed::collect_pruned(&e.pool, &CryptoSpace("other".into()))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sealed::space_payload_count(&e.pool, &space).await.unwrap(),
        1
    );
}
