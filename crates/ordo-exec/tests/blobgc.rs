//! B4: reference GC, floored at the undo horizon.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`) and
//! its own database, for `tests/sealed.rs`'s reason: every test resets the
//! fixture schema, so a shared database makes one test's seed another
//! test's truncation.
//!
//! What has to be true for B4 to count as landed, in the plan's own words:
//!
//! - **deleting the last row referencing an object leaves the bytes
//!   present** — until the horizon passes;
//! - **a revert inside the undo horizon restores a row whose blob still
//!   reads** — which here is the tombstone being *cancelled* rather than
//!   raced;
//! - **after the horizon passes the sweep removes the object and reports
//!   the same `FloorHolder` the log prune reports**, for the same reason.
//!
//! The rows are written with plain SQL rather than through the write
//! protocol, and that is deliberate rather than a shortcut: what is under
//! test is the *sweep*, and driving it through a compiled op would put a
//! whole write pipeline between the assertion and the thing it is about.
//! The one thing SQL cannot stand in for — that a real revert cancels a
//! tombstone — is exercised as the state a revert produces (the row is
//! back before the floor passes), which is precisely the input the sweep
//! sees.

use std::sync::Arc;
use std::time::Duration;

use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore, SingleStore, StoreResolver, Upload};
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::custody::{KeyProvider, SpaceId as CryptoSpace};
use ordo_exec::blobgc::{self, SweepReport};
use ordo_exec::cadence::LogPolicy;
use ordo_exec::prune::FloorHolder;
use ordo_exec::{Pool, PoolConfig};
use ordo_schema::Schema;
use ordo_syntax::parse_program;

const SPACE: &str = "conformance";

/// A one-entity schema with one external blob field. Small on purpose: the
/// sweep's correctness is about the anti-join and the floor, and a wide
/// schema would only add columns nothing here reads.
const SRC: &str = "\
domain gc @ 1.0.0
entity docs (key: doc) {
  title: text
  scan:  blob?
}
";

/// A schema declaring `blob sealed`, for the one test that is about the
/// defect §0.10 of `plan-blob-chain.md` names rather than about the
/// ordinary sweep behaviour every other test in this file exercises: a
/// sealed blob field is admitted by the door's `tier_of` before C3 and
/// modelled by nothing downstream, and `live_reference_clause` compares
/// the column against `encode(b."ref", 'hex')`, `bytea` against `text`,
/// which Postgres has no operator for, so a sweep over a schema like this
/// one errored before C3 excluded sealed fields from that clause.
const SEALED_SRC: &str = "\
domain gc @ 1.0.0
entity docs (key: doc) {
  title:    text
  contract: blob? sealed
}
";

struct Env {
    pool: Pool,
    schema: Schema,
    dir: std::path::PathBuf,
    stores: SingleStore,
    store: Arc<dyn BlobStore>,
    pipe: BlobPipeline,
}

async fn env(db: &str) -> Env {
    env_with(db, SRC).await
}

/// As `env`, but over a caller-supplied schema: the sealed-tier defect
/// test needs a different entity shape than every other test here, and
/// duplicating the whole harness for one schema swap would be the actual
/// shortcut.
async fn env_with(db: &str, src: &str) -> Env {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    // Reset through the fixture so the platform tables exist, then replace
    // the entity surface with this test's own.
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

    let schema = ordo_schema::build(&parse_program(src).expect("parses")).expect("builds");
    pool.rows("DROP TABLE IF EXISTS \"docs\"").await.unwrap();
    // Only this schema's own entity. `ddl_for` also emits the platform
    // entities (`members` and friends), which the fixture reset has
    // already created — running them again is a duplicate-relation error,
    // not a second opinion about their shape.
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
    // `$logconsumers` and `$checkpoints` are the prune's, created on
    // demand rather than at boot — and the blob sweep reads the same floor
    // the prune does, so it needs the same tables. That dependency is the
    // point rather than an inconvenience: one floor, one set of inputs.
    ordo_exec::prune::ensure_tables(&pool)
        .await
        .expect("prune tables");
    ordo_exec::cadence::ensure_tables(&pool)
        .await
        .expect("cadence tables");
    for t in ["$blobref", "$blobtomb", "$commandlog", "$checkpoints"] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }

    let dir = std::env::temp_dir().join(format!("ordo-blobgc-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = FsStore::shared(&dir).expect("store");
    let keys: Arc<dyn KeyProvider> =
        Arc::new(ordo_crypto::custody::TestProvider::with_space(SPACE));
    Env {
        pool,
        schema,
        dir,
        stores: SingleStore::new(Arc::clone(&store)),
        store: Arc::clone(&store),
        pipe: BlobPipeline::new(Arc::new(SingleStore::new(store)), keys, BlobPolicy::DEFAULT),
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn space() -> CryptoSpace {
    CryptoSpace(SPACE.into())
}

impl Env {
    /// Upload an object, insert the row that references it, and write the
    /// `$blobref` row — the state a successful `PUT /blob/...` leaves.
    ///
    /// The three writes are made here rather than through the door because
    /// C4 moved the door's write into the write protocol: there is no
    /// `ordo_exec::blob::attach` any more, and reaching the real path needs
    /// a fixture, a write log and a router, none of which a collector suite
    /// wants. What the sweep reads is the `$blobref` row and the column,
    /// and those are what this leaves.
    async fn attach(&self, row_id: &str, bytes: &[u8], at: i64) -> [u8; 16] {
        self.attach_in(&self.pipe, row_id, bytes, at).await
    }

    /// As [`Env::attach`], through a pipeline of the caller's, and the
    /// `$blobref` row records the store the pipeline wrote to.
    async fn attach_in(
        &self,
        pipe: &BlobPipeline,
        row_id: &str,
        bytes: &[u8],
        at: i64,
    ) -> [u8; 16] {
        let stored = pipe
            .put(
                &Upload {
                    space: &space(),
                    entity: "docs",
                    field: "scan",
                    mime: "application/pdf",
                    plain_len: bytes.len() as u64,
                    correlation: "test",
                    // This helper writes the `$blobref` row itself, two
                    // lines down, so the handle is named in the database
                    // either way. What an intent buys is the case where
                    // that never happens, and that case is driven through
                    // the real door in `ordo-host/tests/blob_door.rs`,
                    // where there is a transaction to interrupt.
                    intent: None,
                },
                ordo_blob::once(bytes.to_vec()),
            )
            .await
            .expect("put");
        self.pool
            .rows(&format!(
                "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"title\", \"scan\") \
                 VALUES ('{SPACE}', '{row_id}', 1, 'a doc', '{}')",
                ordo_crypto::hex(stored.handle())
            ))
            .await
            .expect("row");
        let tx = self.pool.begin().await.expect("tx");
        tx.rows(&ordo_crypto::blobref::insert_sql(
            &space(),
            stored.handle(),
            stored.epoch(),
            "docs",
            "scan",
            stored.mime(),
            stored.size(),
            stored.integrity(),
            stored.bytes(),
            at,
            stored.store(),
        ))
        .await
        .expect("the reference row");
        tx.commit().await.expect("commit");
        *stored.handle()
    }

    async fn delete_row(&self, row_id: &str) {
        self.pool
            .rows(&format!(
                "DELETE FROM \"docs\" WHERE \"$space\" = '{SPACE}' AND \"id\" = '{row_id}'"
            ))
            .await
            .expect("delete");
    }

    /// What a revert of that delete produces: the row is back, pointing at
    /// the same handle, before the floor passed.
    async fn restore_row(&self, row_id: &str, handle: &[u8; 16]) {
        self.pool
            .rows(&format!(
                "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"title\", \"scan\") \
                 VALUES ('{SPACE}', '{row_id}', 2, 'a doc', '{}')",
                ordo_crypto::hex(handle)
            ))
            .await
            .expect("restore");
    }

    async fn sweep(&self, now: u64, horizon: Duration) -> SweepReport {
        blobgc::sweep(
            &self.pool,
            &self.stores,
            &self.schema,
            &space(),
            &LogPolicy {
                undo_horizon: horizon,
                ..LogPolicy::DEFAULT
            },
            now,
        )
        .await
        .expect("sweep")
    }

    async fn object_exists(&self, handle: &[u8; 16]) -> bool {
        self.store.head(&space(), handle).await.is_ok()
    }

    async fn reference_exists(&self, handle: &[u8; 16]) -> bool {
        ordo_exec::blob::lookup(&self.pool, &space(), handle)
            .await
            .expect("lookup")
            .is_some()
    }
}

const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

/// **The headline.** Deleting the last row referencing an object leaves
/// the bytes present until the horizon passes; then the sweep removes
/// them, and reports the same `FloorHolder` the log prune reports.
#[tokio::test]
async fn bytes_outlive_the_last_reference_until_the_horizon_passes() {
    let e = env("ordo_blob_horizon").await;
    let t0 = 1_000 * DAY;
    let handle = e.attach("doc_1", b"a signed contract", t0 as i64).await;

    // A live reference: nothing is marked.
    let r = e.sweep(t0, Duration::from_secs(90 * 86_400)).await;
    assert_eq!(r.marked, 0);
    assert!(e.object_exists(&handle).await);

    // The row goes. The handle is tombstoned; the bytes stay.
    e.delete_row("doc_1").await;
    let r = e.sweep(t0 + HOUR, Duration::from_secs(90 * 86_400)).await;
    assert_eq!(r.marked, 1, "the handle is unreferenced and must be marked");
    assert_eq!(r.deleted, 0, "nothing is deleted inside the horizon");
    assert_eq!(r.waiting, 1);
    assert!(
        e.object_exists(&handle).await,
        "a reverted delete must find its bytes — deleting them here is the \
         one failure this whole module exists to prevent"
    );
    assert!(e.reference_exists(&handle).await);

    // A sweep the next day still finds it waiting, and does not re-mark.
    let r = e.sweep(t0 + DAY, Duration::from_secs(90 * 86_400)).await;
    assert_eq!(r.marked, 0, "phase 1 is idempotent");
    assert_eq!(r.deleted, 0);
    assert!(e.object_exists(&handle).await);

    // Past the horizon: collected, reference last.
    let r = e
        .sweep(t0 + 91 * DAY, Duration::from_secs(90 * 86_400))
        .await;
    assert_eq!(r.deleted, 1);
    assert_eq!(r.waiting, 0);
    assert_eq!(
        r.holder,
        FloorHolder::UndoHorizon,
        "with no lagging consumer the horizon is the holder, and the sweep \
         reports the same name the log prune does"
    );
    assert!(!e.object_exists(&handle).await, "the object is gone");
    assert!(
        !e.reference_exists(&handle).await,
        "the reference went last"
    );
}

/// **A revert cancels a tombstone rather than racing it.** The re-check in
/// phase 2 is what makes that true without a lock across two systems.
#[tokio::test]
async fn a_revert_inside_the_horizon_cancels_the_tombstone() {
    let e = env("ordo_blob_revert").await;
    let t0 = 1_000 * DAY;
    let handle = e.attach("doc_1", b"the only copy", t0 as i64).await;

    e.delete_row("doc_1").await;
    let r = e.sweep(t0 + HOUR, Duration::from_secs(90 * 86_400)).await;
    assert_eq!(r.marked, 1);

    // The revert lands: the row is back, pointing at the same handle.
    e.restore_row("doc_1", &handle).await;

    // Now the horizon passes. The tombstone is CANCELLED, not executed —
    // the re-check finds a live reference.
    let r = e
        .sweep(t0 + 91 * DAY, Duration::from_secs(90 * 86_400))
        .await;
    assert_eq!(r.cancelled, 1, "a revert cancels the tombstone");
    assert_eq!(r.deleted, 0);
    assert!(
        e.object_exists(&handle).await,
        "the restored row's blob must still read"
    );

    // And the object is readable through the pipeline, not merely present.
    let bytes = e
        .pipe
        .get(
            &space(),
            "docs",
            "scan",
            &handle,
            b"the only copy".len() as u64,
            0,
            None,
            "test",
        )
        .await
        .expect("the restored row's blob reads");
    assert_eq!(bytes, b"the only copy");

    // The tombstone is gone, so a later delete starts the clock again
    // rather than inheriting the old one.
    e.delete_row("doc_1").await;
    let r = e
        .sweep(t0 + 92 * DAY, Duration::from_secs(90 * 86_400))
        .await;
    assert_eq!(r.marked, 1, "the clock restarts, it is not resumed");
    assert_eq!(r.deleted, 0);
}

/// Two uploads of the same file are two objects, and collecting one leaves
/// the other alone. This is the cost the no-deduplication ruling states,
/// asserted rather than assumed — and the reason it is safe: the handles
/// are independent, so the anti-join over them is too.
#[tokio::test]
async fn collecting_one_object_leaves_an_identical_one_alone() {
    let e = env("ordo_blob_dedup").await;
    let t0 = 1_000 * DAY;
    let a = e.attach("doc_1", b"identical bytes", t0 as i64).await;
    let b = e.attach("doc_2", b"identical bytes", t0 as i64).await;
    assert_ne!(a, b, "two uploads of one file are two objects");

    e.delete_row("doc_1").await;
    e.sweep(t0 + HOUR, Duration::from_secs(90 * 86_400)).await;
    let r = e
        .sweep(t0 + 91 * DAY, Duration::from_secs(90 * 86_400))
        .await;
    assert_eq!(r.deleted, 1);
    assert!(!e.object_exists(&a).await);
    assert!(
        e.object_exists(&b).await,
        "the surviving row's object must be untouched — a deduplicating \
         store would have collected the shared one here"
    );
}

/// Phase 2 is idempotent across a crash: a tombstone whose object is
/// already gone completes on the retry rather than sticking. Simulated by
/// deleting the object out from under the sweep, which is exactly the
/// state a crash between the store delete and the stamp leaves.
#[tokio::test]
async fn a_crash_between_the_object_delete_and_the_stamp_retries_cleanly() {
    let e = env("ordo_blob_retry").await;
    let t0 = 1_000 * DAY;
    let handle = e.attach("doc_1", b"bytes", t0 as i64).await;
    e.delete_row("doc_1").await;
    e.sweep(t0 + HOUR, Duration::from_secs(90 * 86_400)).await;

    // The crash: the object is gone, the tombstone is still open.
    e.store.delete(&space(), &handle).await.expect("delete");
    assert!(!e.object_exists(&handle).await);

    let r = e
        .sweep(t0 + 91 * DAY, Duration::from_secs(90 * 86_400))
        .await;
    assert_eq!(
        r.deleted, 1,
        "an idempotent store delete lets the retry finish rather than stick"
    );
    assert!(!e.reference_exists(&handle).await);
    assert_eq!(r.waiting, 0);
}

/// **Space erasure is not garbage collection**, and must not be floored at
/// the undo horizon: there is nothing left to undo into. Every object goes
/// immediately, beside the keys.
#[tokio::test]
async fn erasing_a_space_removes_every_object_now() {
    let e = env("ordo_blob_erase").await;
    let t0 = 1_000 * DAY;
    let a = e.attach("doc_1", b"one", t0 as i64).await;
    let b = e.attach("doc_2", b"two", t0 as i64).await;

    let removed = blobgc::erase_space(&e.pool, &e.stores, &space())
        .await
        .expect("erase");
    assert_eq!(removed.deleted, 2);
    assert!(removed.left.is_empty());
    assert!(!e.object_exists(&a).await);
    assert!(!e.object_exists(&b).await);
    assert_eq!(
        blobgc::reference_count(&e.pool, &space()).await.unwrap(),
        0,
        "the references go with the bytes"
    );
}

/// An object whose upload succeeded and whose transaction failed — a
/// `$blobref` row no column ever named. It is collected on the ordinary
/// schedule, which is why the write path does not have to delete it
/// itself: a delete path that could run without the floor is exactly what
/// the floor exists to forbid.
#[tokio::test]
async fn an_object_no_row_ever_named_is_collected_on_the_ordinary_schedule() {
    let e = env("ordo_blob_orphan").await;
    let t0 = 1_000 * DAY;
    let handle = e.attach("doc_1", b"orphan", t0 as i64).await;
    // Remove the row without ever having committed it, leaving the
    // reference and the object.
    e.delete_row("doc_1").await;

    let r = e.sweep(t0, Duration::from_secs(90 * 86_400)).await;
    assert_eq!(r.marked, 1);
    assert!(e.object_exists(&handle).await, "not before the floor");

    let r = e
        .sweep(t0 + 91 * DAY, Duration::from_secs(90 * 86_400))
        .await;
    assert_eq!(r.deleted, 1);
    assert!(!e.object_exists(&handle).await);
}

/// The resolver is asked which store a space's bytes are in, on every
/// sweep — so a space with a BYO bucket is swept against its own bucket
/// rather than the deployment's.
#[tokio::test]
async fn the_sweep_deletes_through_the_spaces_resolved_store() {
    let e = env("ordo_blob_resolver").await;
    let resolved = e.stores.resolve(&space()).await.expect("resolve");
    assert_eq!(resolved.binding(), "filesystem");
}

/// **The defect `plan-blob-chain.md` §0.10 names, on the record.** Before
/// C3, `live_reference_clause` compared every non-inline blob column
/// against `encode(b."ref", 'hex')`, including a `blob sealed` one, whose
/// column is `bytea` (`ddl::column_ty` short-circuits on `f.sealed` before
/// it ever reaches the blob arm). `bytea = text` has no operator in
/// Postgres, and the comparison sits inside a `NOT EXISTS` subquery that
/// Postgres has to type-check to plan the statement at all, so `mark`,
/// and therefore every `sweep`, errored the instant a schema declared the
/// field, with zero rows anywhere near the table. C3 excludes sealed
/// fields from the clause, for the same reason the door refuses to let one
/// ever be written: a sealed handle carries a fresh nonce, so the column
/// could never hold a value the collector could compare against a
/// `$blobref` row and call equal, keyless or not.
#[tokio::test]
async fn a_sweep_over_a_sealed_blob_schema_does_not_error() {
    let e = env_with("ordo_blob_sealed_sweep", SEALED_SRC).await;
    let r = blobgc::sweep(
        &e.pool,
        &e.stores,
        &e.schema,
        &space(),
        &LogPolicy {
            undo_horizon: Duration::from_secs(90 * 86_400),
            ..LogPolicy::DEFAULT
        },
        1_000 * DAY,
    )
    .await;
    match r {
        Ok(report) => assert_eq!(report.marked, 0, "no rows, nothing to mark"),
        Err(e) => {
            panic!("a sweep over a schema declaring `blob sealed` must not error, and it did: {e}")
        }
    }
}

/// **A consumer holding the floor pins the cutoff to its group's time, in
/// the sweep's own unit.**
///
/// A command's `at` is seconds (`Timestamp`'s doc, and the unit the prune
/// compares in); the sweep's `now` and every tombstone are milliseconds.
/// `cutoff_of` used to take the minimum of the two raw numbers, which put the
/// cutoff in January 1970 whenever a consumer held the floor, so the sweep
/// collected nothing at all. No test here held the floor, and the host
/// stamped commands from a counter, so nothing showed it (checklist 11-28).
#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_holding_the_floor_pins_the_cutoff_in_milliseconds() {
    const T: u64 = 1_700_000_000;
    const DAY: u64 = 86_400;
    let e = env("ordo_blobgc_consumer_floor").await;
    let log = ordo_commandlog::SpaceId(SPACE.into());
    append_stamped(&e, &[T, T + 1]).await;
    ordo_exec::prune::register_consumer(
        &e.pool,
        &log,
        "mirror_1",
        ordo_exec::prune::ConsumerKind::Mirror,
        0,
        // Advanced a day ago, so it is lagging (position 0) without being
        // stalled: a mirror idle past `consumerStall` is evicted from the
        // floor, and then it would hold nothing.
        T + 99 * DAY,
    )
    .await
    .expect("register the lagging consumer");

    let report = e
        .sweep((T + 100 * DAY) * 1_000, Duration::from_secs(90 * DAY))
        .await;
    assert!(
        matches!(report.holder, FloorHolder::Consumer { .. }),
        "the lagging consumer should hold the floor, not {}",
        report.holder
    );
    assert_eq!(
        report.cutoff,
        T * 1_000,
        "the cutoff is the held group's time, in milliseconds"
    );
}

/// Append one single-command group per entry of `times`, each stamped at
/// that many seconds.
async fn append_stamped(e: &Env, times: &[u64]) {
    let log = ordo_commandlog::SpaceId(SPACE.into());
    for (i, at) in times.iter().enumerate() {
        let row = format!("doc_floor_{i}");
        let tx = e.pool.begin().await.expect("tx");
        tx.append_group(&ordo_commandlog::CommandGroup {
            correlation: ordo_commandlog::CorrelationId(format!("cg_floor_{i}")),
            actor: ordo_commandlog::Actor::User("usr_1".into()),
            commands: vec![ordo_commandlog::Command {
                id: ordo_commandlog::CommandId(format!("cmd_floor_{i}")),
                space: log.clone(),
                entity: "docs".into(),
                row_id: row.clone(),
                kind: ordo_commandlog::CommandKind::Create,
                before: None,
                after: Some(
                    [("id".to_string(), ordo_commandlog::Value::Id(row))]
                        .into_iter()
                        .collect(),
                ),
                at: ordo_commandlog::Timestamp(*at),
            }],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        })
        .await
        .expect("append");
        tx.commit().await.expect("commit");
    }
}

/// **With no consumer behind it, the undo horizon holds the sweep's floor,
/// and the report says so.**
///
/// One group older than the horizon and one inside it. The prune's floor
/// sits at the first group inside the horizon and names the horizon as the
/// holder. The sweep used to hand the floor its own millisecond `now`,
/// against command times in seconds, so every group looked older than the
/// horizon, the floor fell to the chain's last entry, and the report named
/// the chain end as the reason storage was held (checklist 11-28).
#[tokio::test(flavor = "multi_thread")]
async fn with_no_consumer_the_undo_horizon_holds_the_sweeps_floor() {
    const T: u64 = 1_700_000_000;
    const DAY: u64 = 86_400;
    let e = env("ordo_blobgc_horizon_floor").await;
    append_stamped(&e, &[T, T + 99 * DAY]).await;

    let report = e
        .sweep((T + 100 * DAY) * 1_000, Duration::from_secs(90 * DAY))
        .await;
    assert!(
        matches!(report.holder, FloorHolder::UndoHorizon),
        "the undo horizon should hold the floor, not {}",
        report.holder
    );
    assert_eq!(
        report.cutoff,
        (T + 10 * DAY) * 1_000,
        "the cutoff is the horizon, in milliseconds"
    );
}

/// A store that deletes `allow` objects and then refuses every delete.
struct RefusesAfter {
    inner: Arc<dyn BlobStore>,
    allow: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl BlobStore for RefusesAfter {
    fn binding(&self) -> &'static str {
        "refuses-after"
    }
    fn production_ready(&self) -> bool {
        false
    }
    async fn put(
        &self,
        space: &CryptoSpace,
        handle: &ordo_crypto::custody::BlobHandle,
        len: u64,
        body: ordo_blob::ByteStream,
    ) -> Result<ordo_blob::ObjectHead, ordo_blob::BlobError> {
        self.inner.put(space, handle, len, body).await
    }
    async fn get(
        &self,
        space: &CryptoSpace,
        handle: &ordo_crypto::custody::BlobHandle,
        range: Option<ordo_blob::ByteRange>,
    ) -> Result<ordo_blob::ByteStream, ordo_blob::BlobError> {
        self.inner.get(space, handle, range).await
    }
    async fn head(
        &self,
        space: &CryptoSpace,
        handle: &ordo_crypto::custody::BlobHandle,
    ) -> Result<ordo_blob::ObjectHead, ordo_blob::BlobError> {
        self.inner.head(space, handle).await
    }
    async fn delete(
        &self,
        space: &CryptoSpace,
        handle: &ordo_crypto::custody::BlobHandle,
    ) -> Result<(), ordo_blob::BlobError> {
        use std::sync::atomic::Ordering;
        let left = self.allow.load(Ordering::SeqCst);
        if left == 0 {
            return Err(ordo_blob::BlobError::Transport("refused".into()));
        }
        self.allow.store(left - 1, Ordering::SeqCst);
        self.inner.delete(space, handle).await
    }
}

/// **A store that fails part way still has its deletions counted.** The
/// sweep used to return the store's error and drop the count of what it had
/// already deleted, so a record of the pass could say neither number. Now
/// the deletions stand in `deleted`, and the rest are counted under the
/// cause that stopped them.
#[tokio::test]
async fn a_store_that_fails_part_way_reports_what_it_deleted_and_what_it_left() {
    let e = env("ordo_blobgc_part_way").await;
    let t0 = 1_000 * DAY;
    let mut handles = Vec::new();
    for id in ["doc_1", "doc_2", "doc_3"] {
        handles.push(e.attach(id, id.as_bytes(), t0 as i64).await);
        e.delete_row(id).await;
    }
    let horizon = Duration::from_secs(90 * 86_400);
    assert_eq!(e.sweep(t0 + HOUR, horizon).await.marked, 3);

    let refusing = SingleStore::new(Arc::new(RefusesAfter {
        inner: Arc::clone(&e.store),
        allow: std::sync::atomic::AtomicUsize::new(1),
    }));
    let r = blobgc::sweep(
        &e.pool,
        &refusing,
        &e.schema,
        &space(),
        &LogPolicy {
            undo_horizon: horizon,
            ..LogPolicy::DEFAULT
        },
        t0 + 91 * DAY,
    )
    .await
    .expect("a store failure is an outcome of the pass, not an error");
    assert_eq!(r.deleted, 1, "the one delete the store allowed is counted");
    let stopped = r.stopped.expect("the pass says it stopped");
    assert_eq!(stopped.cause, blobgc::Uncollected::Store);
    assert_eq!(
        stopped.not_deleted, 2,
        "the failed one and the one after it"
    );
    let mut left = 0;
    for h in &handles {
        if e.object_exists(h).await {
            left += 1;
        }
    }
    assert_eq!(left, 2, "what was not deleted is still there");

    // The next pass, with the store answering, collects the rest.
    let r = e.sweep(t0 + 92 * DAY, horizon).await;
    assert_eq!(r.deleted, 2);
    assert_eq!(r.stopped, None);
}

// ── D-S5: erasing a space decides per file from the recorded store ───────

/// A resolver with the deployment's store and one tenant bucket, both
/// filesystem stores. `current` answers the bucket while `in_bucket` is set,
/// which is what a tenant attached at upload time looks like.
struct TwoStores {
    own: Arc<dyn BlobStore>,
    bucket: Arc<dyn BlobStore>,
    bucket_name: String,
    in_bucket: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl StoreResolver for TwoStores {
    async fn resolve(
        &self,
        _space: &CryptoSpace,
    ) -> Result<Arc<dyn BlobStore>, ordo_blob::BlobError> {
        Ok(Arc::clone(&self.own))
    }

    async fn current(
        &self,
        _space: &CryptoSpace,
    ) -> Result<(Arc<dyn BlobStore>, String), ordo_blob::BlobError> {
        if self.in_bucket.load(std::sync::atomic::Ordering::SeqCst) {
            Ok((Arc::clone(&self.bucket), self.bucket_name.clone()))
        } else {
            Ok((Arc::clone(&self.own), String::new()))
        }
    }

    async fn named(
        &self,
        _space: &CryptoSpace,
        name: &str,
    ) -> Result<Arc<dyn BlobStore>, ordo_blob::BlobError> {
        if name.is_empty() {
            Ok(Arc::clone(&self.own))
        } else if name == self.bucket_name {
            Ok(Arc::clone(&self.bucket))
        } else {
            Err(ordo_blob::BlobError::Config("unknown store".into()))
        }
    }
}

fn bucket_name(custody: ordo_exec::bucket::BucketCustody) -> String {
    ordo_exec::bucket::Attachment {
        endpoint: "https://s3.tenant.example".into(),
        region: "eu-west-1".into(),
        bucket: "acme-files".into(),
        credential: "acme-cred-label".into(),
        custody,
        path_style: None,
        key_prefix: Some("incoming".into()),
        attached_at: 0,
        attached_by: String::new(),
    }
    .store_name()
}

struct Twin {
    e: Env,
    stores: Arc<TwoStores>,
    pipe: BlobPipeline,
    bucket_dir: std::path::PathBuf,
}

impl Drop for Twin {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.bucket_dir).ok();
    }
}

async fn twin(db: &str, custody: ordo_exec::bucket::BucketCustody) -> Twin {
    let e = env(db).await;
    let bucket_dir =
        std::env::temp_dir().join(format!("ordo-blobgc-{db}-bucket-{}", std::process::id()));
    std::fs::create_dir_all(&bucket_dir).unwrap();
    let stores = Arc::new(TwoStores {
        own: Arc::clone(&e.store),
        bucket: FsStore::shared(&bucket_dir).expect("bucket store"),
        bucket_name: bucket_name(custody),
        in_bucket: std::sync::atomic::AtomicBool::new(false),
    });
    let keys: Arc<dyn KeyProvider> =
        Arc::new(ordo_crypto::custody::TestProvider::with_space(SPACE));
    let pipe = BlobPipeline::new(stores.clone(), keys, BlobPolicy::DEFAULT);
    Twin {
        e,
        stores,
        pipe,
        bucket_dir,
    }
}

impl Twin {
    fn in_bucket(&self, on: bool) {
        self.stores
            .in_bucket
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    async fn in_own(&self, handle: &[u8; 16]) -> bool {
        self.stores.own.head(&space(), handle).await.is_ok()
    }

    async fn in_bucket_store(&self, handle: &[u8; 16]) -> bool {
        self.stores.bucket.head(&space(), handle).await.is_ok()
    }
}

/// **A file in the deployment's own store is deleted**, as it always was.
#[tokio::test]
async fn an_erase_deletes_files_in_the_deployments_store() {
    let t = twin(
        "ordo_erase_own",
        ordo_exec::bucket::BucketCustody::OwnPerimeter,
    )
    .await;
    let a = t.e.attach_in(&t.pipe, "doc_1", b"one", 1).await;

    let done = blobgc::erase_space(&t.e.pool, t.stores.as_ref(), &space())
        .await
        .expect("erase");
    assert_eq!(done.deleted, 1);
    assert!(done.left.is_empty(), "nothing was left: {done:?}");
    assert!(!t.in_own(&a).await);
}

/// **A file in a tenant-owned bucket is left**, and what was left comes back
/// by store, count, epoch and a digest of the handles. The reference rows
/// still go: the list of objects is the platform's record, and the keys are
/// already destroyed.
#[tokio::test]
async fn an_erase_leaves_files_in_a_tenant_owned_bucket_and_reports_them() {
    let t = twin(
        "ordo_erase_left",
        ordo_exec::bucket::BucketCustody::OwnPerimeter,
    )
    .await;
    t.in_bucket(true);
    let a = t.e.attach_in(&t.pipe, "doc_1", b"one", 1).await;
    let b = t.e.attach_in(&t.pipe, "doc_2", b"two", 1).await;

    let done = blobgc::erase_space(&t.e.pool, t.stores.as_ref(), &space())
        .await
        .expect("erase");
    assert_eq!(done.deleted, 0);
    assert!(t.in_bucket_store(&a).await, "the tenant's object stays");
    assert!(t.in_bucket_store(&b).await, "the tenant's object stays");
    assert_eq!(done.left.len(), 1, "{done:?}");
    let left = &done.left[0];
    assert_eq!(left.count, 2);
    assert_eq!(left.store, "https://s3.tenant.example|acme-files|incoming");
    assert!(left.digest.starts_with("sha256:"), "{left:?}");
    assert!(!left.epochs.is_empty());
    let action = left.action();
    assert!(action.starts_with("objects_left "), "{action}");
    assert!(
        !action.contains("acme-cred-label"),
        "the credential label is not part of the record: {action}"
    );
    assert_eq!(
        blobgc::reference_count(&t.e.pool, &space()).await.unwrap(),
        0,
        "the platform's references go"
    );
}

/// **A tenant with files in both gets both treatments**: the file in the
/// deployment's store is deleted and the one in the bucket is left, in one
/// erase.
#[tokio::test]
async fn an_erase_of_a_tenant_with_files_in_both_stores_does_both() {
    let t = twin(
        "ordo_erase_both",
        ordo_exec::bucket::BucketCustody::OwnPerimeter,
    )
    .await;
    let before =
        t.e.attach_in(&t.pipe, "doc_1", b"before the attach", 1)
            .await;
    t.in_bucket(true);
    let after =
        t.e.attach_in(&t.pipe, "doc_2", b"after the attach", 1)
            .await;

    let done = blobgc::erase_space(&t.e.pool, t.stores.as_ref(), &space())
        .await
        .expect("erase");
    assert_eq!(done.deleted, 1);
    assert_eq!(done.left.len(), 1);
    assert_eq!(done.left[0].count, 1);
    assert!(!t.in_own(&before).await, "deployment store: deleted");
    assert!(t.in_bucket_store(&after).await, "tenant bucket: left");
}

/// A bucket attached under `PlatformSealed` is not the tenant's perimeter:
/// erasure treats it as it always did and deletes the object.
#[tokio::test]
async fn an_erase_still_deletes_from_a_platform_sealed_bucket() {
    let t = twin(
        "ordo_erase_sealed",
        ordo_exec::bucket::BucketCustody::PlatformSealed,
    )
    .await;
    t.in_bucket(true);
    let a = t.e.attach_in(&t.pipe, "doc_1", b"one", 1).await;

    let done = blobgc::erase_space(&t.e.pool, t.stores.as_ref(), &space())
        .await
        .expect("erase");
    assert_eq!(done.deleted, 1);
    assert!(done.left.is_empty());
    assert!(!t.in_bucket_store(&a).await);
}
