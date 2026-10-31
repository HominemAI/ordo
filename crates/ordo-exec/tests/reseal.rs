//! K1 and K2: the re-seal pass and the destruction gate, run against the
//! market fixture.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`). Every
//! test owns its own database, as `tests/sealed.rs` does.
//!
//! What has to be true for K1 and K2 to count as landed:
//!
//! - a pass moves **every** payload row and every live sealed column off
//!   the old epoch, and every blind-index digest is byte-identical after
//!   it;
//! - every value read through the ordinary path is byte-identical before
//!   and after, while every envelope has changed;
//! - a pass killed mid-way resumes from its cursor and counts every row
//!   exactly once;
//! - a row written during the pass is skipped, never clobbered, and the
//!   skip is a real race and not a mock;
//! - a re-sealed value that does not open back to what was read aborts the
//!   chunk with nothing committed;
//! - the gate refuses by population name with the exact count, and once
//!   it clears and the epoch is destroyed, every envelope captured before
//!   the pass refuses to open while every live value still reads.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_commandlog::{SpaceId, Timestamp, Value};
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_crypto::{
    Anchor, BlindValue, Epoch, KeyError, KeyLease, KeyProvider, NonceBase, Purpose, SealedHandle,
    SpaceId as CryptoSpace,
};
use ordo_exec::reseal::{self, Gate, PassState, Population, Reseal, ResealError, Step};
use ordo_exec::sealed::{self, Reader, Resolved};
use ordo_exec::{undo, Executor, LoggedExecutor, Pool, PoolConfig};
use ordo_schema::Schema;

// ── plumbing ─────────────────────────────────────────────────────────────

struct Env {
    pool: Pool,
    fixture: Arc<Fixture>,
    schema: Schema,
    log: Arc<Mutex<WriteLog>>,
    pg: Pg,
    space: CryptoSpace,
}

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
    pool.ensure_platform_tables()
        .await
        .expect("platform tables");
    reseal::ensure_tables(&pool).await.expect("$reseal");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    pool.rows("DELETE FROM \"$checkpoints\"").await.unwrap();
    pool.rows("DELETE FROM \"$sealedpayload\"").await.unwrap();
    pool.rows("DELETE FROM \"$reseal\"").await.unwrap();
    let schema = f.schema.clone();
    Env {
        pool,
        fixture: Arc::new(f),
        schema,
        log: Arc::new(Mutex::new(WriteLog::new())),
        pg,
        space: space(),
    }
}

fn space() -> CryptoSpace {
    CryptoSpace("conformance".into())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Env {
    fn keys(&self) -> &dyn KeyProvider {
        self.fixture.keys.as_ref()
    }

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

    async fn write(&self, call: &str) {
        self.executor()
            .write_as(
                self.fixture.clone(),
                self.log.clone(),
                call.to_string(),
                "mem_1".to_string(),
            )
            .await
            .unwrap_or_else(|e| panic!("write `{call}`: {e}"));
    }

    fn engine<'a>(&'a self, keys: &'a dyn KeyProvider, chunk: usize) -> Reseal<'a> {
        Reseal {
            pool: &self.pool,
            keys,
            schema: &self.schema,
            space: &self.space,
            chunk,
            stores: None,
        }
    }

    /// Step until idle, returning every step taken.
    async fn drain(&self, keys: &dyn KeyProvider, chunk: usize) -> Vec<Step> {
        let engine = self.engine(keys, chunk);
        let mut steps = Vec::new();
        for _ in 0..10_000 {
            let s = engine.step(now()).await.expect("a step");
            let idle = s == Step::Idle;
            steps.push(s);
            if idle {
                return steps;
            }
        }
        panic!("the pass did not terminate: {steps:?}");
    }

    fn lease(&self) -> KeyLease {
        self.keys()
            .release(&space(), Purpose::Write, "test")
            .expect("release")
    }

    async fn rows(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        self.pool.rows(sql).await.unwrap()
    }

    async fn one(&self, sql: &str) -> Option<String> {
        self.rows(sql).await.first()?.first()?.clone()
    }

    /// The `orders.memo` envelope on a row, raw.
    async fn memo_envelope(&self, id: &str) -> Vec<u8> {
        let raw = self
            .one(&format!(
                "SELECT encode(\"memo\", 'hex') FROM \"orders\" WHERE \"id\" = '{id}'"
            ))
            .await
            .expect("a row");
        ordo_crypto::sealed::unhex(&format!("\\x{raw}")).unwrap()
    }

    /// The `orders.memo` envelope on a row, opened.
    async fn memo_of(&self, id: &str) -> Option<String> {
        let bytes = self.memo_envelope(id).await;
        if bytes.is_empty() {
            return None;
        }
        let plain = self
            .keys()
            .open_at(&self.lease(), "orders", "memo", Anchor::Row(id), &bytes)
            .expect("the row's own envelope opens");
        Some(String::from_utf8(plain).unwrap())
    }

    /// Every `email$bidx`, by customer.
    async fn digests(&self) -> BTreeMap<String, String> {
        self.rows("SELECT \"id\", encode(\"email$bidx\", 'hex') FROM \"customers\" ORDER BY \"id\"")
            .await
            .into_iter()
            .map(|r| (r[0].clone().unwrap(), r[1].clone().unwrap()))
            .collect()
    }

    /// Every payload row: handle to `(epoch, envelope hex)`.
    async fn payloads(&self) -> BTreeMap<String, (u8, String)> {
        self.rows(
            "SELECT encode(\"ref\", 'hex'), \"epoch\", encode(\"payload\", 'hex') \
             FROM \"$sealedpayload\" ORDER BY \"ref\"",
        )
        .await
        .into_iter()
        .map(|r| {
            (
                r[0].clone().unwrap(),
                (
                    r[1].clone().unwrap().parse().unwrap(),
                    r[2].clone().unwrap(),
                ),
            )
        })
        .collect()
    }

    async fn resolve(&self, handle: &SealedHandle) -> Resolved {
        sealed::resolve(
            Reader::Pool(&self.pool),
            self.keys(),
            &self.lease(),
            &space(),
            handle,
        )
        .await
        .expect("the store answers")
    }

    /// Every handle any image in the chain carries for `orders.memo`.
    async fn handles(&self) -> Vec<SealedHandle> {
        let rows = self
            .rows("SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = 'conformance' ORDER BY \"idx\"")
            .await;
        let mut out = Vec::new();
        for r in rows {
            let body = ordo_crypto::sealed::unhex(r[0].as_deref().unwrap()).unwrap();
            let group = ordo_commandlog::chain::decode_group(&body).unwrap();
            for c in &group.commands {
                for image in [c.before.as_ref(), c.after.as_ref()].into_iter().flatten() {
                    if let Some(Value::SealedRef(h)) = image.get("memo") {
                        out.push(*h);
                    }
                }
            }
        }
        out
    }

    async fn remaining(&self, epoch: Epoch) -> reseal::Remaining {
        reseal::remaining(&self.pool, &self.schema, &space(), epoch)
            .await
            .unwrap()
    }
}

// ── an interposing provider ──────────────────────────────────────────────

/// A population name and the act to run once, on the first `seal` under a
/// `Reseal` lease for that population.
type Hook = (String, Box<dyn FnOnce() + Send>);

/// A provider that forwards everything and lets a test act **between** the
/// pass reading a row and the pass writing it back. The hook runs on the
/// first `seal` under a `Reseal` lease whose correlation names the given
/// population, which is the moment the pass holds the old bytes in hand.
struct Interposed {
    inner: Arc<dyn KeyProvider>,
    on_seal: Mutex<Option<Hook>>,
    /// Corrupt what `open` returns for envelopes under this epoch, under a
    /// `Reseal` lease only: the verification's second `open` sees a
    /// different plaintext and the chunk must abort.
    corrupt_open_under: Option<Epoch>,
}

impl Interposed {
    fn new(inner: Arc<dyn KeyProvider>) -> Interposed {
        Interposed {
            inner,
            on_seal: Mutex::new(None),
            corrupt_open_under: None,
        }
    }
}

impl KeyProvider for Interposed {
    fn release(&self, s: &CryptoSpace, p: Purpose, c: &str) -> Result<KeyLease, KeyError> {
        self.inner.release(s, p, c)
    }
    fn seal_at(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        if l.purpose == Purpose::Reseal {
            let hook = {
                let mut g = self.on_seal.lock().unwrap();
                match g.take() {
                    Some((pop, hook)) if l.correlation.ends_with(&format!(":{pop}")) => Some(hook),
                    other => {
                        *g = other;
                        None
                    }
                }
            };
            if let Some(hook) = hook {
                hook();
            }
        }
        self.inner.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        let mut plain = self.inner.open_at(l, e, f, a, v)?;
        if l.purpose == Purpose::Reseal
            && self.corrupt_open_under.is_some()
            && ordo_crypto::envelope_epoch(v) == self.corrupt_open_under
        {
            if let Some(b) = plain.first_mut() {
                *b ^= 1;
            } else {
                plain.push(1);
            }
        }
        Ok(plain)
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        ep: Epoch,
        b: &NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.inner.seal_chunk(l, e, f, a, ep, b, c, last, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        l: &KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        ep: Epoch,
        b: &NonceBase,
        c: u32,
        last: bool,
        v: &[u8],
    ) -> Result<Vec<u8>, KeyError> {
        self.inner.open_chunk(l, e, f, a, ep, b, c, last, v)
    }
    fn blind_index(
        &self,
        s: &CryptoSpace,
        e: &str,
        f: &str,
        v: BlindValue<'_>,
    ) -> Result<[u8; 32], KeyError> {
        self.inner.blind_index(s, e, f, v)
    }
    fn current_epoch(&self, s: &CryptoSpace) -> Result<Epoch, KeyError> {
        self.inner.current_epoch(s)
    }
    fn rotate(&self, s: &CryptoSpace, r: &str) -> Result<Epoch, KeyError> {
        self.inner.rotate(s, r)
    }
    fn create_space(&self, s: &CryptoSpace) -> Result<(), KeyError> {
        self.inner.create_space(s)
    }
    fn destroy_epoch(&self, s: &CryptoSpace, e: Epoch) -> Result<(), KeyError> {
        self.inner.destroy_epoch(s, e)
    }
    fn destroy_space(&self, s: &CryptoSpace) -> Result<(), KeyError> {
        self.inner.destroy_space(s)
    }
}

// ── K1 ───────────────────────────────────────────────────────────────────

/// **K1's done-when, first half.** A pass moves every payload row and
/// every live sealed column, the digests do not move, and every value
/// reads back byte-identical through both the column and the handle.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_moves_every_population_and_no_digest() {
    let e = env("ordo_reseal_pass").await;
    e.write(r#"setMemo($order: "ord_1", $memo: "first")"#).await;
    e.write(r#"setMemo($order: "ord_2", $memo: "second")"#)
        .await;
    e.write(r#"setMemo($order: "ord_1", $memo: "third")"#).await;

    let old: Epoch = e.keys().current_epoch(&space()).unwrap();
    let digests_before = e.digests().await;
    let payloads_before = e.payloads().await;
    let memos_before: Vec<(String, Option<String>)> = {
        let mut v = Vec::new();
        for id in ["ord_1", "ord_2", "ord_3"] {
            v.push((id.into(), e.memo_of(id).await));
        }
        v
    };
    let envelopes_before: Vec<Vec<u8>> = {
        let mut v = Vec::new();
        for id in ["ord_1", "ord_2"] {
            v.push(e.memo_envelope(id).await);
        }
        v
    };
    let handles = e.handles().await;
    assert!(
        handles.len() >= 4,
        "three converges image at least four payloads"
    );
    let mut resolved_before = Vec::new();
    for h in &handles {
        resolved_before.push(e.resolve(h).await);
    }
    assert!(payloads_before.values().all(|(ep, _)| *ep == old));
    let before = e.remaining(old).await;
    assert_eq!(before.payloads, payloads_before.len() as i64);
    assert_eq!(
        before.columns, 2,
        "ord_1 and ord_2 carry envelopes; ord_3 is empty"
    );

    let new = reseal::enqueue_rotation(&e.pool, e.keys(), &space(), "test", now())
        .await
        .expect("rotation");
    assert_eq!(new, old + 1);
    assert_eq!(e.keys().current_epoch(&space()).unwrap(), new);
    // Rotation alone moved nothing and broke nothing.
    assert_eq!(e.remaining(old).await, before);
    assert_eq!(e.memo_of("ord_1").await.as_deref(), Some("third"));

    let steps = e.drain(e.keys(), 2).await;
    let completed = steps
        .iter()
        .filter(|s| matches!(s, Step::Completed { .. }))
        .count();
    assert_eq!(completed, 1, "completed exactly once: {steps:?}");
    assert!(
        !steps.iter().any(|s| matches!(s, Step::Blocked { .. })),
        "nothing is blocked in a space with no objects: {steps:?}"
    );

    // The done-when's numbers.
    assert_eq!(e.remaining(old).await.total(), 0);
    assert_eq!(
        sealed::epoch_payload_count(&e.pool, &space(), old)
            .await
            .unwrap(),
        0
    );
    let no_old_column = e
        .one(&format!(
            "SELECT count(*) FROM \"orders\" WHERE length(\"memo\") > 0 AND get_byte(\"memo\", 0) = {old}"
        ))
        .await;
    assert_eq!(no_old_column.as_deref(), Some("0"));

    // Digests did not move.
    assert_eq!(e.digests().await, digests_before);

    // Every value reads back identical, through the column and through the
    // handle, and every envelope changed.
    for (id, memo) in &memos_before {
        assert_eq!(&e.memo_of(id).await, memo, "{id}");
    }
    for (i, id) in ["ord_1", "ord_2"].iter().enumerate() {
        let after = e.memo_envelope(id).await;
        assert_ne!(after, envelopes_before[i], "{id}'s envelope did not change");
        assert_eq!(ordo_crypto::envelope_epoch(&after), Some(new));
    }
    for (h, before) in handles.iter().zip(&resolved_before) {
        assert_eq!(&e.resolve(h).await, before);
    }
    let payloads_after = e.payloads().await;
    assert_eq!(
        payloads_after.len(),
        payloads_before.len(),
        "no row added or lost"
    );
    for (r, (ep, bytes)) in &payloads_after {
        assert_eq!(*ep, new, "{r} is not under the new epoch");
        assert_ne!(
            bytes, &payloads_before[r].1,
            "{r}'s envelope did not change"
        );
    }

    // The pass rows say so.
    let rows = reseal::passes(&e.pool, &space()).await.unwrap();
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.state, PassState::Done, "{r:?}");
        assert!(r.finished_at.is_some());
        assert_eq!(r.from_epoch, old);
        assert_eq!(r.to_epoch, new);
    }
    let done: i64 = rows.iter().map(|r| r.rows_done).sum();
    assert_eq!(done, before.payloads + before.columns);
    assert_eq!(rows.iter().map(|r| r.rows_skipped).sum::<i64>(), 0);
}

/// A pass stopped after three chunks resumes from its cursor under a new
/// engine, and the counts show every row processed exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn a_killed_pass_resumes_from_its_cursor() {
    let e = env("ordo_reseal_resume").await;
    for (id, memo) in [
        ("ord_1", "a"),
        ("ord_2", "b"),
        ("ord_3", "c"),
        ("ord_1", "d"),
    ] {
        e.write(&format!(r#"setMemo($order: "{id}", $memo: "{memo}")"#))
            .await;
    }
    let old = e.keys().current_epoch(&space()).unwrap();
    let before = e.remaining(old).await;
    let total = before.payloads + before.columns;
    assert!(total >= 8, "{before:?}");
    reseal::enqueue_rotation(&e.pool, e.keys(), &space(), "test", now())
        .await
        .unwrap();

    {
        let engine = e.engine(e.keys(), 1);
        for _ in 0..3 {
            let s = engine.step(now()).await.unwrap();
            assert!(matches!(s, Step::Moved { done: 1, .. }), "{s:?}");
        }
        // The engine is dropped here: the state is in `$reseal`, not in it.
    }
    let rows = reseal::passes(&e.pool, &space()).await.unwrap();
    let payloads = rows
        .iter()
        .find(|r| r.population == Population::Payloads)
        .unwrap();
    assert_eq!(payloads.state, PassState::Running);
    assert_eq!(payloads.rows_done, 3);
    assert!(payloads.cursor.is_some(), "the cursor is persisted");
    assert_eq!(e.remaining(old).await.total(), total - 3);

    let steps = e.drain(e.keys(), 1).await;
    assert_eq!(
        steps
            .iter()
            .filter(|s| matches!(s, Step::Completed { .. }))
            .count(),
        1
    );
    assert_eq!(e.remaining(old).await.total(), 0);
    let rows = reseal::passes(&e.pool, &space()).await.unwrap();
    assert_eq!(rows.iter().map(|r| r.rows_done).sum::<i64>(), total);
    assert_eq!(rows.iter().map(|r| r.rows_skipped).sum::<i64>(), 0);
    for id in ["ord_2", "ord_3"] {
        assert!(e.memo_of(id).await.is_some(), "{id}");
    }
    assert_eq!(e.memo_of("ord_1").await.as_deref(), Some("d"));
}

/// **The race, real.** Between the pass reading `ord_1`'s envelope and
/// writing it back, another writer commits a new value under the new
/// epoch. The CAS on the old bytes finds no row, the pass counts a skip,
/// and the row holds the newer value.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_written_during_the_pass_is_skipped_not_clobbered() {
    let e = env("ordo_reseal_race").await;
    e.write(r#"setMemo($order: "ord_1", $memo: "stale")"#).await;
    let old = e.keys().current_epoch(&space()).unwrap();
    reseal::enqueue_rotation(&e.pool, e.keys(), &space(), "test", now())
        .await
        .unwrap();

    let keys = Interposed::new(e.fixture.keys.clone());
    let pg = e.pg.clone();
    let inner = e.fixture.keys.clone();
    *keys.on_seal.lock().unwrap() = Some((
        "columns".into(),
        Box::new(move || {
            // A concurrent writer: seals "fresh" under the current (new)
            // epoch and commits it on its own connection while the pass
            // holds the stale bytes.
            let lease = inner.release(&space(), Purpose::Write, "writer").unwrap();
            let env = inner
                .seal_at(&lease, "orders", "memo", Anchor::Row("ord_1"), b"fresh")
                .unwrap();
            pg.exec(&format!(
                "UPDATE \"orders\" SET \"memo\" = '\\x{}'::bytea WHERE \"id\" = 'ord_1'",
                ordo_crypto::hex(&env)
            ))
            .expect("the concurrent write commits");
        }),
    ));

    let steps = e.drain(&keys, 8).await;
    assert!(steps.iter().any(|s| matches!(s, Step::Completed { .. })));
    assert!(
        keys.on_seal.lock().unwrap().is_none(),
        "the concurrent write ran"
    );
    assert_eq!(e.memo_of("ord_1").await.as_deref(), Some("fresh"));
    assert_eq!(e.remaining(old).await.total(), 0);
    let rows = reseal::passes(&e.pool, &space()).await.unwrap();
    let columns = rows
        .iter()
        .find(|r| r.population == Population::Columns)
        .unwrap();
    assert_eq!(columns.rows_skipped, 1, "{columns:?}");
    assert_eq!(columns.rows_done, 0, "{columns:?}");
}

/// A re-sealed value that does not open back to what was read aborts the
/// chunk: nothing committed, the cursor where it was, and the attempt
/// counted.
#[tokio::test(flavor = "multi_thread")]
async fn a_verification_mismatch_aborts_the_chunk() {
    let e = env("ordo_reseal_verify").await;
    e.write(r#"setMemo($order: "ord_1", $memo: "kept")"#).await;
    let old = e.keys().current_epoch(&space()).unwrap();
    let new = reseal::enqueue_rotation(&e.pool, e.keys(), &space(), "test", now())
        .await
        .unwrap();
    let before = e.payloads().await;

    let mut keys = Interposed::new(e.fixture.keys.clone());
    keys.corrupt_open_under = Some(new);
    let engine = e.engine(&keys, 8);
    let err = engine.step(now()).await.expect_err("the chunk aborts");
    assert!(
        matches!(
            err,
            ResealError::Verify {
                population: Population::Payloads,
                ..
            }
        ),
        "{err}"
    );
    let text = err.to_string();
    assert!(text.contains("not committed"), "{text}");
    assert!(
        !text.contains("kept"),
        "the refusal never names a value: {text}"
    );

    assert_eq!(e.payloads().await, before, "nothing committed");
    assert_eq!(e.remaining(old).await.payloads, before.len() as i64);
    let rows = reseal::passes(&e.pool, &space()).await.unwrap();
    let payloads = rows
        .iter()
        .find(|r| r.population == Population::Payloads)
        .unwrap();
    assert_eq!(payloads.rows_done, 0);
    assert!(payloads.cursor.is_none(), "{payloads:?}");
    assert_eq!(payloads.attempts, 1, "{payloads:?}");

    // The honest provider finishes what the corrupting one could not.
    let steps = e.drain(e.keys(), 8).await;
    assert!(steps.iter().any(|s| matches!(s, Step::Completed { .. })));
    assert_eq!(e.memo_of("ord_1").await.as_deref(), Some("kept"));
}

// ── K2 ───────────────────────────────────────────────────────────────────

/// **K2's done-when.** The gate refuses by name with the exact count while
/// anything is under the epoch, and clears once the pass has run. After
/// `destroy_epoch`, every envelope captured before the pass refuses to
/// open, every live value still reads, and a payload re-tagged under the
/// destroyed epoch resolves `Destroyed` and refuses the revert with the
/// typed rejection.
#[tokio::test(flavor = "multi_thread")]
async fn the_gate_refuses_by_name_with_the_count() {
    let e = env("ordo_reseal_gate").await;
    e.write(r#"setMemo($order: "ord_1", $memo: "one")"#).await;
    e.write(r#"setMemo($order: "ord_2", $memo: "two")"#).await;
    let old = e.keys().current_epoch(&space()).unwrap();

    // The write epoch is named as such, before anything is counted.
    assert_eq!(
        reseal::destroy_gate(&e.pool, &e.schema, e.keys(), &space(), old)
            .await
            .unwrap(),
        Gate::Current
    );

    let captured: Vec<(String, Vec<u8>)> = {
        let mut v = Vec::new();
        for id in ["ord_1", "ord_2"] {
            v.push((id.to_string(), e.memo_envelope(id).await));
        }
        v
    };
    let captured_payloads = e.payloads().await;
    let plain_before: Vec<Option<String>> = {
        let mut v = Vec::new();
        for id in ["ord_1", "ord_2"] {
            v.push(e.memo_of(id).await);
        }
        v
    };

    reseal::enqueue_rotation(&e.pool, e.keys(), &space(), "test", now())
        .await
        .unwrap();

    // Refused by name, with the number.
    let gate = reseal::destroy_gate(&e.pool, &e.schema, e.keys(), &space(), old)
        .await
        .unwrap();
    assert_eq!(
        gate,
        Gate::Refused {
            population: Population::Payloads,
            count: captured_payloads.len() as i64
        }
    );

    let steps = e.drain(e.keys(), 8).await;
    assert!(steps.iter().any(|s| matches!(s, Step::Completed { .. })));
    let gate = reseal::destroy_gate(&e.pool, &e.schema, e.keys(), &space(), old)
        .await
        .unwrap();
    let Gate::Clear { resealed_at } = gate else {
        panic!("{gate:?}");
    };
    assert!(
        resealed_at.is_some(),
        "the pass's finish is the date printed"
    );

    e.keys().destroy_epoch(&space(), old).expect("destroy");

    // Every envelope captured before the pass refuses, per row.
    let lease = e.lease();
    for (id, env) in &captured {
        let refused = e
            .keys()
            .open_at(&lease, "orders", "memo", Anchor::Row(id), env);
        assert_eq!(refused, Err(KeyError::UnknownEpoch(old)), "{id}");
    }
    for (r, (_, hex)) in &captured_payloads {
        let bytes = ordo_crypto::sealed::unhex(&format!("\\x{hex}")).unwrap();
        let refused = e
            .keys()
            .open_at(&lease, "orders", "memo", Anchor::Row("ord_1"), &bytes);
        assert_eq!(refused, Err(KeyError::UnknownEpoch(old)), "{r}");
    }
    // Every live value still reads, byte-identical.
    for (i, id) in ["ord_1", "ord_2"].iter().enumerate() {
        assert_eq!(e.memo_of(id).await, plain_before[i], "{id}");
    }
    for h in e.handles().await {
        assert!(matches!(e.resolve(&h).await, Resolved::Payload(_)));
    }

    // A payload left under the destroyed epoch (put back by the test, as a
    // backup restored over this deployment would) resolves `Destroyed`
    // and the revert refuses with the typed rejection, never an empty
    // value.
    let handles = e.handles().await;
    let target = handles[0];
    let (_, hex) = &captured_payloads[&ordo_crypto::hex(&target)];
    e.rows(&format!(
        "UPDATE \"$sealedpayload\" SET \"payload\" = '\\x{hex}'::bytea, \"epoch\" = {old} \
         WHERE \"ref\" = '\\x{}'::bytea",
        ordo_crypto::hex(&target)
    ))
    .await;
    assert_eq!(e.resolve(&target).await, Resolved::Destroyed(old));
    let first = e
        .one("SELECT \"correlation\" FROM \"$commandlog\" WHERE \"idx\" = 0")
        .await
        .unwrap();
    let refused = undo::revert(
        &e.pool,
        &e.schema,
        Some(e.keys()),
        None,
        None,
        &SpaceId("conformance".into()),
        &ordo_commandlog::CorrelationId(first),
        "mem_1",
        Timestamp(20),
    )
    .await
    .expect_err("a revert cannot invent a value whose key is gone");
    let msg = refused.to_string();
    assert!(msg.contains("destroyed"), "{msg}");
}

// ── K1d: objects ─────────────────────────────────────────────────────────

/// A one-entity schema with one external blob field, the shape
/// `tests/blobgc.rs` uses for the same reason: what is under test is the
/// object move, and a wide schema would only add columns nothing reads.
const BLOB_SRC: &str = "\
domain k @ 1.0.0
entity docs (key: doc) {
  title: text
  scan:  blob?
}
";

struct BlobEnv {
    pool: Pool,
    schema: Schema,
    dir: std::path::PathBuf,
    stores: ordo_blob::SingleStore,
    pipe: ordo_blob::BlobPipeline,
    keys: Arc<dyn KeyProvider>,
}

impl Drop for BlobEnv {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

async fn blob_env(db: &str) -> BlobEnv {
    use ordo_syntax::parse_program;
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    Fixture::load()
        .expect("fixture")
        .reset(&pg)
        .expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.ensure_platform_tables()
        .await
        .expect("platform tables");
    reseal::ensure_tables(&pool).await.expect("$reseal");
    let schema = ordo_schema::build(&parse_program(BLOB_SRC).expect("parses")).expect("builds");
    pool.rows("DROP TABLE IF EXISTS \"docs\"").await.unwrap();
    for stmt in ordo_schema::ddl_for(&schema)
        .expect("ddl")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains("\"docs\""))
    {
        pool.rows(stmt)
            .await
            .unwrap_or_else(|e| panic!("{stmt}: {e}"));
    }
    for t in ["$blobref", "$blobtomb", "$sealedpayload", "$reseal"] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }
    let dir = std::env::temp_dir().join(format!("ordo-reseal-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = ordo_blob::FsStore::shared(&dir).expect("store");
    let keys: Arc<dyn KeyProvider> = Arc::new(ordo_crypto::TestProvider::with_space("conformance"));
    BlobEnv {
        pool,
        schema,
        dir,
        stores: ordo_blob::SingleStore::new(Arc::clone(&store)),
        pipe: ordo_blob::BlobPipeline::new(
            Arc::new(ordo_blob::SingleStore::new(store)),
            keys.clone(),
            ordo_blob::BlobPolicy::DEFAULT,
        ),
        keys,
    }
}

impl BlobEnv {
    /// Upload an object and leave the state a successful upload leaves:
    /// the row naming it and the `$blobref` row.
    async fn attach(&self, row_id: &str, bytes: &[u8]) -> ordo_crypto::BlobHandle {
        let stored = self
            .pipe
            .put(
                &ordo_blob::Upload {
                    space: &space(),
                    entity: "docs",
                    field: "scan",
                    mime: "application/pdf",
                    plain_len: bytes.len() as u64,
                    correlation: "test",
                    intent: None,
                },
                ordo_blob::once(bytes.to_vec()),
            )
            .await
            .expect("put");
        self.pool
            .rows(&format!(
                "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"title\", \"scan\") \
                 VALUES ('conformance', '{row_id}', 1, 'a doc', '{}')",
                ordo_crypto::hex(stored.handle())
            ))
            .await
            .expect("row");
        self.pool
            .rows(&ordo_crypto::blobref::insert_sql(
                &space(),
                stored.handle(),
                stored.epoch(),
                "docs",
                "scan",
                stored.mime(),
                stored.size(),
                stored.integrity(),
                stored.bytes(),
                1,
                "",
            ))
            .await
            .expect("the reference row");
        *stored.handle()
    }

    async fn column(&self, row_id: &str) -> String {
        self.pool
            .rows(&format!(
                "SELECT \"scan\" FROM \"docs\" WHERE \"id\" = '{row_id}'"
            ))
            .await
            .unwrap()[0][0]
            .clone()
            .unwrap()
    }

    async fn blob_ref(&self, handle: &ordo_crypto::BlobHandle) -> Option<ordo_crypto::BlobRef> {
        let rows = self
            .pool
            .rows(&ordo_crypto::blobref::lookup_sql(&space(), handle))
            .await
            .unwrap();
        rows.first()
            .and_then(|r| ordo_crypto::blobref::blob_ref_of(r))
    }

    async fn read(&self, handle: &ordo_crypto::BlobHandle, len: u64) -> Vec<u8> {
        self.pipe
            .get(&space(), "docs", "scan", handle, len, 0, None, "test")
            .await
            .expect("the object reads")
    }

    fn engine(&self, chunk: usize) -> Reseal<'_> {
        Reseal {
            pool: &self.pool,
            keys: self.keys.as_ref(),
            schema: &self.schema,
            space: self.space_ref(),
            chunk,
            stores: Some(&self.stores),
        }
    }

    fn space_ref(&self) -> &'static CryptoSpace {
        use std::sync::OnceLock;
        static S: OnceLock<CryptoSpace> = OnceLock::new();
        S.get_or_init(space)
    }
}

/// **K1d's done-when.** An external object under the old epoch is moved
/// to a new handle under the new one; the row points at the new handle;
/// the new object reads and verifies; the old row and object stay for the
/// image that names them; the object population stays counted and blocked
/// until the old pair is collected; and then the gate clears.
#[tokio::test(flavor = "multi_thread")]
async fn an_external_object_is_resealed_to_a_new_handle() {
    let e = blob_env("ordo_reseal_objects").await;
    // Two chunks and a bit, so the chunk loop and the `last` flag are both
    // exercised.
    let bytes: Vec<u8> = (0..(ordo_crypto::CHUNK * 2 + 777))
        .map(|i| (i % 251) as u8)
        .collect();
    let old_handle = e.attach("doc_1", &bytes).await;
    let unreferenced = e.attach("doc_2", b"orphan").await;
    e.pool
        .rows("DELETE FROM \"docs\" WHERE \"id\" = 'doc_2'")
        .await
        .unwrap();

    let old = e.keys.current_epoch(&space()).unwrap();
    let before = reseal::remaining(&e.pool, &e.schema, &space(), old)
        .await
        .unwrap();
    assert_eq!(before.objects, 2, "{before:?}");
    assert_eq!(before.blobrefs, 2, "{before:?}");

    let new = reseal::enqueue_rotation(&e.pool, e.keys.as_ref(), &space(), "test", now())
        .await
        .unwrap();
    let engine = e.engine(8);
    let mut steps = Vec::new();
    for _ in 0..64 {
        let s = engine.step(now()).await.expect("a step");
        let idle = s == Step::Idle;
        steps.push(s);
        if idle {
            break;
        }
    }
    // The live object moved, the orphan was skipped, and the population
    // is blocked on what only blobgc can retire.
    assert!(
        steps.iter().any(|s| matches!(
            s,
            Step::Blocked {
                population: Population::Objects,
                count: 2,
                ..
            }
        )),
        "{steps:?}"
    );
    assert!(
        !steps.iter().any(|s| matches!(s, Step::Completed { .. })),
        "not complete while old pairs remain: {steps:?}"
    );

    let new_hex = e.column("doc_1").await;
    let new_handle = ordo_crypto::blobref::parse_handle(&new_hex).expect("a handle");
    assert_ne!(new_handle, old_handle, "the column moved to a new handle");

    // The new row and object: under the new epoch, and the bytes read
    // back identical through the pipeline, which also verifies the sealed
    // digest.
    let new_ref = e.blob_ref(&new_handle).await.expect("the new row");
    assert_eq!(new_ref.epoch, new);
    assert_eq!(e.read(&new_handle, bytes.len() as u64).await, bytes);
    assert!(e
        .pipe
        .verify(
            &space(),
            "docs",
            "scan",
            &new_handle,
            bytes.len() as u64,
            &new_ref.integrity,
            "test"
        )
        .await
        .expect("verifies"));
    for env in [&new_ref.mime, &new_ref.size, &new_ref.integrity] {
        assert_eq!(ordo_crypto::envelope_epoch(env), Some(new));
    }

    // The old row and object are untouched: the image that names the old
    // handle still resolves through them.
    let old_ref = e.blob_ref(&old_handle).await.expect("the old row stays");
    assert_eq!(
        old_ref.epoch, old,
        "the old row's epoch is the old object's"
    );
    assert_eq!(e.read(&old_handle, bytes.len() as u64).await, bytes);
    // The orphan was neither moved nor touched.
    assert!(e.blob_ref(&unreferenced).await.is_some());
    assert_eq!(
        e.pool
            .rows("SELECT count(*) FROM \"$blobref\"")
            .await
            .unwrap()[0][0]
            .as_deref(),
        Some("3"),
        "one new row, two old ones"
    );

    // The gate refuses by name until blobgc has retired the old pairs.
    let left = reseal::remaining(&e.pool, &e.schema, &space(), old)
        .await
        .unwrap();
    assert_eq!(left.objects, 2, "{left:?}");
    assert_eq!(left.blobrefs, 0, "the envelopes were re-sealed in place");
    assert_eq!(
        reseal::destroy_gate(&e.pool, &e.schema, e.keys.as_ref(), &space(), old)
            .await
            .unwrap(),
        Gate::Refused {
            population: Population::Objects,
            count: 2
        }
    );
    let rows = reseal::passes(&e.pool, &space()).await.unwrap();
    let objects = rows
        .iter()
        .find(|r| r.population == Population::Objects)
        .unwrap();
    assert_eq!(objects.state, PassState::Blocked);
    assert_eq!((objects.rows_done, objects.rows_skipped), (1, 1));

    // What blobgc does once no live row and no retained image names them:
    // the old rows go. Then the gate clears and a forced re-seal has
    // nothing left to do.
    for h in [&old_handle, &unreferenced] {
        e.pool
            .rows(&format!(
                "DELETE FROM \"$blobref\" WHERE \"ref\" = '\\x{}'::bytea",
                ordo_crypto::hex(h)
            ))
            .await
            .unwrap();
    }
    let gate = reseal::destroy_gate(&e.pool, &e.schema, e.keys.as_ref(), &space(), old)
        .await
        .unwrap();
    assert!(matches!(gate, Gate::Clear { .. }), "{gate:?}");
    assert!(
        reseal::enqueue_pass(&e.pool, &space(), old, new, "again", now())
            .await
            .unwrap(),
        "a blocked row re-opens"
    );
    let mut completed = false;
    for _ in 0..16 {
        match engine.step(now()).await.expect("a step") {
            Step::Idle => break,
            Step::Completed { .. } => completed = true,
            _ => {}
        }
    }
    assert!(completed, "with the old pairs gone the pass completes");
}
