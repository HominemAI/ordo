//! U8: the storage sweep's runner (`ordo_host::workers::BlobGc`).
//!
//! `blobgc::sweep` is tested on its own in `ordo-exec/tests/blobgc.rs`. What
//! is under test here is the loop that calls it: that it runs on the one
//! deployment-wide cadence (D-S3), that it walks every database the resolver
//! names and not only the one the process booted with, that it resolves each
//! space's store, and that the census names it as running.
//!
//! Needs a Postgres cluster (`scripts/conformance-pg.sh start`). Two
//! databases: a control database, and a second one attached through
//! `$spacedb` the way `tests/usage.rs`'s rollup test attaches its own.

use std::sync::Arc;
use std::time::Duration;

use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore, SingleStore, StoreResolver, Upload};
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::blobref::{insert_sql, tomb_declare_sql, BLOB_REF_TABLE, BLOB_TOMB_TABLE};
use ordo_crypto::custody::{KeyProvider, SpaceId};
use ordo_exec::cadence::LogPolicy;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::{sql_lit, Pool, PoolConfig};
use ordo_host::metrics::Metrics;
use ordo_host::supervisor::{Plan, Supervisor};
use ordo_host::workers::BlobGc;

/// A pool over its own database with the platform tables and the `members`
/// entity an empty-source deployment still carries (`tests/usage.rs`
/// `raw_pool_with_empty_schema` says why that table is needed).
async fn fresh_pool(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean");
    let empty = Fixture::from_source("", "").expect("an empty source compiles");
    pg.exec(&empty.ddl).expect("the members table");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    pool.ensure_platform_tables()
        .await
        .expect("platform tables");
    pool
}

/// Upload one object for `space` and write its `$blobref` row on `pool`:
/// the state a committed upload leaves. Returns the handle.
async fn attach_object(
    pool: &Pool,
    pipe: &BlobPipeline,
    space: &SpaceId,
    bytes: &[u8],
) -> [u8; 16] {
    let stored = pipe
        .put(
            &Upload {
                space,
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
    pool.rows(&insert_sql(
        space,
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

async fn count(pool: &Pool, table: &str, space: &str) -> i64 {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM \"{table}\" WHERE \"$space\" = {}",
            sql_lit(space)
        ))
        .await
        .expect("count");
    rows[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

async fn poll<F, Fut>(deadline: Duration, mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = std::time::Instant::now();
    loop {
        if f().await {
            return true;
        }
        if start.elapsed() > deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// **U8's done-when, one tier over from `blobgc.rs`'s.** The runner walks
/// every target `PoolResolver::targets` names: an orphan whose reference
/// lives in an attached tenant database, and whose tombstone is past the
/// floor, is collected from that tenant's store, reference last. The
/// default database is walked on the same pass, and a reference there that
/// only just lost its row is marked and not collected, because the floor
/// has not passed. The census names the worker, its schedule and its
/// period.
#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_runs_on_its_cadence_and_collects_an_orphan_in_an_attached_database() {
    const CONTROL_DB: &str = "ordo_blob_gc_runner_control";
    const ATTACHED_DB: &str = "ordo_blob_gc_runner_attached";
    const DEFAULT_SPACE: &str = "spc_blob_gc_default_side";
    const ATTACHED_SPACE: &str = "spc_blob_gc_attached_side";

    let control = fresh_pool(CONTROL_DB).await;
    let target = fresh_pool(ATTACHED_DB).await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();

    let env = PoolConfig::from_env();
    spacedb::attach(
        &control,
        &SpaceId(ATTACHED_SPACE.into()),
        &DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: ATTACHED_DB.into(),
            credential: "local".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "mem_owner".into(),
        },
    )
    .await
    .expect("attached");

    // One store for both spaces, on disk, so "the object is gone" is a
    // `head` that fails and nothing else.
    let dir = std::env::temp_dir().join(format!("ordo-blob-gc-runner-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    let stores: Arc<dyn StoreResolver> = Arc::new(SingleStore::new(store.clone()));
    let keys: Arc<dyn KeyProvider> = Arc::new(ordo_crypto::custody::TestProvider::new());
    let pipe = BlobPipeline::new(stores.clone(), keys.clone(), BlobPolicy::DEFAULT);

    let attached_space = SpaceId(ATTACHED_SPACE.into());
    let default_space = SpaceId(DEFAULT_SPACE.into());
    // The orphan: a reference in the ATTACHED database whose tombstone was
    // stamped long ago, as an earlier pass would have left it. The schema
    // this deployment runs declares no blob field, so no live row can name
    // either handle and both are unreferenced; what separates the two is
    // the floor.
    let orphan = attach_object(&target, &pipe, &attached_space, b"attached-side bytes").await;
    target
        .rows(&tomb_declare_sql(&attached_space, &orphan, 1, ""))
        .await
        .expect("an old tombstone");
    // A reference on the DEFAULT side with no tombstone yet: this pass
    // marks it, and the bytes stay because the horizon has not passed.
    let fresh = attach_object(&control, &pipe, &default_space, b"default-side bytes").await;

    let resolver: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        PoolConfig {
            size: 2,
            ..PoolConfig::from_env()
        },
        Arc::new(Installed::new().as_configured("local")),
    ));
    let deployment_space = ordo_crypto::SpaceId("spc_blob_gc_deployment".into());
    let deployment_keys: Arc<dyn KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(&deployment_space.0));
    let deployment = ordo_host::deploy::Deployment::empty(
        deployment_space.clone(),
        control.clone(),
        deployment_keys.clone(),
    )
    .expect("an empty deployment");
    let spaces = ordo_host::spaces::Spaces::routed(
        deployment.clone(),
        resolver,
        ordo_exec::witness::Witness::new(control.clone()),
    );
    let gc = Arc::new(BlobGc {
        spaces,
        stores: stores.clone(),
        policy: LogPolicy::DEFAULT,
        period: Duration::from_secs(1),
        stream: None,
    });
    let sup = Supervisor::start(
        control.clone(),
        deployment_keys,
        deployment,
        Arc::new(Metrics::new()),
        deployment_space,
        PoolConfig {
            db: CONTROL_DB.to_string(),
            ..PoolConfig::from_env()
        },
        Plan {
            workers: vec![gc],
            ..Plan::default()
        },
    )
    .await
    .expect("the supervisor starts");

    // The census: running, on its own named schedule, leader only.
    let census = sup.census();
    let line = census
        .iter()
        .find(|(name, _)| *name == "blob.gc")
        .map(|(_, how)| how.clone())
        .expect("blob.gc is in the running census");
    assert!(
        line.starts_with("schedule blob.gc every 1s") && line.ends_with("leader only"),
        "the census line: {line}"
    );
    assert_eq!(sup.health()["blob.gc"]["enabled"], serde_json::json!(true));

    // The orphan goes from the tenant's store and from the tenant's
    // database, on the schedule rather than on a call this test makes.
    let collected = poll(Duration::from_secs(15), || async {
        store.head(&attached_space, &orphan).await.is_err()
    })
    .await;
    assert!(collected, "the attached-side orphan was never collected");
    // The reference is deleted after the stored object, in a later step of
    // the same sweep, so it is waited for rather than read at once. Each
    // walk now dials the tenant afresh, which widens that gap (CI run
    // 39580353000).
    let reference_gone = poll(Duration::from_secs(15), || async {
        count(&target, BLOB_REF_TABLE, ATTACHED_SPACE).await == 0
    })
    .await;
    assert!(reference_gone, "the reference goes last, and it went");

    // The default side was walked on the same pass: its reference is
    // tombstoned now, and its bytes stay inside the horizon.
    let marked = poll(Duration::from_secs(10), || async {
        count(&control, BLOB_TOMB_TABLE, DEFAULT_SPACE).await == 1
    })
    .await;
    assert!(marked, "the default-side reference was never marked");
    assert!(
        store.head(&default_space, &fresh).await.is_ok(),
        "inside the horizon the bytes stay: a reverted delete must find them"
    );
    assert_eq!(count(&control, BLOB_REF_TABLE, DEFAULT_SPACE).await, 1);

    sup.shutdown(Duration::from_secs(5)).await;
    std::fs::remove_dir_all(&dir).ok();
}

// ── the security stream ─────────────────────────────────────────────────

/// Attach `space` to `db`, through `control`'s `$spacedb`.
async fn attach_db(control: &Pool, space: &str, db: &str) {
    let env = PoolConfig::from_env();
    spacedb::attach(
        control,
        &SpaceId(space.into()),
        &DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: db.into(),
            credential: "local".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "mem_owner".into(),
        },
    )
    .await
    .expect("attached");
}

/// A supervisor running only the sweep, every second, recording into
/// `stream`.
async fn start_gc(
    control: &Pool,
    control_db: &str,
    stores: Arc<dyn StoreResolver>,
    stream: Arc<ordo_secevent::Recorder>,
) -> Supervisor {
    let resolver: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        PoolConfig {
            size: 2,
            ..PoolConfig::from_env()
        },
        Arc::new(Installed::new().as_configured("local")),
    ));
    let deployment_space = ordo_crypto::SpaceId(format!("spc_{control_db}_deployment"));
    let deployment_keys: Arc<dyn KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space(&deployment_space.0));
    let deployment = ordo_host::deploy::Deployment::empty(
        deployment_space.clone(),
        control.clone(),
        deployment_keys.clone(),
    )
    .expect("an empty deployment");
    let spaces = ordo_host::spaces::Spaces::routed(
        deployment.clone(),
        resolver,
        ordo_exec::witness::Witness::new(control.clone()),
    );
    let gc = Arc::new(BlobGc {
        spaces,
        stores,
        policy: LogPolicy::DEFAULT,
        period: Duration::from_secs(1),
        stream: Some(stream),
    });
    Supervisor::start(
        control.clone(),
        deployment_keys,
        deployment,
        Arc::new(Metrics::new()),
        deployment_space,
        PoolConfig {
            db: control_db.to_string(),
            ..PoolConfig::from_env()
        },
        Plan {
            workers: vec![gc],
            ..Plan::default()
        },
    )
    .await
    .expect("the supervisor starts")
}

/// Wait until the sweep has finished at least `n` passes.
async fn passes(sup: &Supervisor, n: u64) -> bool {
    poll(Duration::from_secs(20), || async {
        sup.health()["blob.gc"]["runs"].as_u64().unwrap_or(0) >= n
    })
    .await
}

/// The sweep's entries the recorder holds, as (space, action, pass).
fn gc_entries(stream: &ordo_secevent::Recorder) -> Vec<(String, String, String)> {
    stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX))
        .into_iter()
        .filter(|e| e.op == "blob.gc")
        .map(|e| {
            let space = match &e.scope {
                ordo_secevent::Scope::Space(s) => s.0.clone(),
                ordo_secevent::Scope::Deployment => "$deployment".to_string(),
            };
            assert_eq!(e.subject, ordo_secevent::Subject::platform("blob.gc"));
            let action = match &e.kind {
                ordo_secevent::EventKind::CustodyAction { action } => action.clone(),
                other => panic!("the sweep records a custody action, not {other:?}"),
            };
            let pass = e
                .correlations
                .first()
                .map(|c| c.0.clone())
                .unwrap_or_default();
            (space, action, pass)
        })
        .collect()
}

/// An object for `space` on `pool` whose tombstone is long past the floor.
async fn orphan(pool: &Pool, pipe: &BlobPipeline, space: &SpaceId, bytes: &[u8]) -> [u8; 16] {
    let h = attach_object(pool, pipe, space, bytes).await;
    pool.rows(&tomb_declare_sql(space, &h, 1, ""))
        .await
        .expect("an old tombstone");
    h
}

/// **The sweep's deletions are on the record, one entry per tenant per
/// pass** (owner, 2026-10-05, `audit-2026-10-04` ask 4). Two orphans in one
/// tenant and one in another, attached to a second database, are collected
/// on one pass. Each tenant gets one entry in its own stream with its own
/// count. Both carry the same pass, and the passes after it, which find
/// nothing, add none.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_records_one_entry_per_tenant_in_its_own_stream() {
    const CONTROL_DB: &str = "ordo_blob_gc_stream_control";
    const ATTACHED_DB: &str = "ordo_blob_gc_stream_attached";
    const HOME: &str = "spc_blob_gc_stream_home";
    const AWAY: &str = "spc_blob_gc_stream_away";

    let control = fresh_pool(CONTROL_DB).await;
    let target = fresh_pool(ATTACHED_DB).await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    attach_db(&control, AWAY, ATTACHED_DB).await;

    let dir = std::env::temp_dir().join(format!("ordo-blob-gc-stream-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    let stores: Arc<dyn StoreResolver> = Arc::new(SingleStore::new(store.clone()));
    let keys: Arc<dyn KeyProvider> = Arc::new(ordo_crypto::custody::TestProvider::new());
    let pipe = BlobPipeline::new(stores.clone(), keys, BlobPolicy::DEFAULT);

    let home = SpaceId(HOME.into());
    let away = SpaceId(AWAY.into());
    let orphans = vec![
        (
            home.clone(),
            orphan(&control, &pipe, &home, b"home one").await,
        ),
        (
            home.clone(),
            orphan(&control, &pipe, &home, b"home two").await,
        ),
        (
            away.clone(),
            orphan(&target, &pipe, &away, b"away one").await,
        ),
    ];

    let stream = Arc::new(ordo_secevent::Recorder::default());
    let sup = start_gc(&control, CONTROL_DB, stores, stream.clone()).await;
    let collected = poll(Duration::from_secs(15), || async {
        for (space, h) in &orphans {
            if store.head(space, h).await.is_ok() {
                return false;
            }
        }
        true
    })
    .await;
    assert!(collected, "the orphans were never collected");
    // Two more passes after the one that collected: they find nothing.
    let done = sup.health()["blob.gc"]["runs"].as_u64().unwrap_or(0);
    assert!(passes(&sup, done + 2).await, "the sweep stopped running");
    sup.shutdown(Duration::from_secs(5)).await;

    let mut entries = gc_entries(&stream);
    entries.sort();
    assert_eq!(
        entries.len(),
        2,
        "one entry per tenant, not per object: {entries:?}"
    );
    let (away_space, away_action, away_pass) = &entries[0];
    let (home_space, home_action, home_pass) = &entries[1];
    assert_eq!(away_space, AWAY, "the attached tenant's entry is its own");
    assert_eq!(home_space, HOME, "the default tenant's entry is its own");
    assert_eq!(
        home_action,
        &format!(
            "blob_collected deleted=2 not_deleted_store=0 not_deleted_database=0 pass={home_pass}"
        )
    );
    assert_eq!(
        away_action,
        &format!(
            "blob_collected deleted=1 not_deleted_store=0 not_deleted_database=0 pass={away_pass}"
        )
    );
    assert_eq!(home_pass, away_pass, "both tenants were swept on one pass");
    std::fs::remove_dir_all(&dir).ok();
}

/// **A pass with nothing due records nothing.** A reference that only just
/// lost its row is marked, and its bytes stay inside the horizon. The
/// passes that do that delete nothing and are not stopped, so they write no
/// entry.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_with_nothing_to_delete_records_nothing() {
    const CONTROL_DB: &str = "ordo_blob_gc_stream_quiet";
    const SPACE: &str = "spc_blob_gc_stream_quiet";

    let control = fresh_pool(CONTROL_DB).await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();

    let dir = std::env::temp_dir().join(format!("ordo-blob-gc-quiet-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    let stores: Arc<dyn StoreResolver> = Arc::new(SingleStore::new(store.clone()));
    let keys: Arc<dyn KeyProvider> = Arc::new(ordo_crypto::custody::TestProvider::new());
    let pipe = BlobPipeline::new(stores.clone(), keys, BlobPolicy::DEFAULT);
    let space = SpaceId(SPACE.into());
    let fresh = attach_object(&control, &pipe, &space, b"inside the horizon").await;

    let stream = Arc::new(ordo_secevent::Recorder::default());
    let sup = start_gc(&control, CONTROL_DB, stores, stream.clone()).await;
    assert!(passes(&sup, 3).await, "the sweep never ran");
    sup.shutdown(Duration::from_secs(5)).await;

    assert_eq!(
        count(&control, BLOB_TOMB_TABLE, SPACE).await,
        1,
        "it was marked"
    );
    assert!(store.head(&space, &fresh).await.is_ok(), "and kept");
    assert_eq!(
        gc_entries(&stream),
        Vec::<(String, String, String)>::new(),
        "nothing deleted, nothing recorded"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A store that refuses every delete and serves everything else.
struct RefusesDeletes {
    inner: Arc<dyn BlobStore>,
}

#[async_trait::async_trait]
impl BlobStore for RefusesDeletes {
    fn binding(&self) -> &'static str {
        "refuses-deletes"
    }
    fn production_ready(&self) -> bool {
        false
    }
    async fn put(
        &self,
        space: &SpaceId,
        handle: &ordo_crypto::custody::BlobHandle,
        len: u64,
        body: ordo_blob::ByteStream,
    ) -> Result<ordo_blob::ObjectHead, ordo_blob::BlobError> {
        self.inner.put(space, handle, len, body).await
    }
    async fn get(
        &self,
        space: &SpaceId,
        handle: &ordo_crypto::custody::BlobHandle,
        range: Option<ordo_blob::ByteRange>,
    ) -> Result<ordo_blob::ByteStream, ordo_blob::BlobError> {
        self.inner.get(space, handle, range).await
    }
    async fn head(
        &self,
        space: &SpaceId,
        handle: &ordo_crypto::custody::BlobHandle,
    ) -> Result<ordo_blob::ObjectHead, ordo_blob::BlobError> {
        self.inner.head(space, handle).await
    }
    async fn delete(
        &self,
        _space: &SpaceId,
        _handle: &ordo_crypto::custody::BlobHandle,
    ) -> Result<(), ordo_blob::BlobError> {
        Err(ordo_blob::BlobError::Transport("refused".into()))
    }
}

/// **A pass that storage refused entirely is on the record** (owner,
/// 2026-10-05, `audit-2026-10-05` ask 2). The orphan is past the floor, the
/// store refuses its delete, and the pass deletes nothing. It still writes
/// an entry with `deleted=0` and the object it left under the store, so the
/// tenant's stream shows the sweep tried and was stopped.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_the_store_refused_entirely_records_deleted_zero() {
    const CONTROL_DB: &str = "ordo_blob_gc_stream_refused";
    const SPACE: &str = "spc_blob_gc_stream_refused";

    let control = fresh_pool(CONTROL_DB).await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();

    let dir = std::env::temp_dir().join(format!("ordo-blob-gc-refused-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    let stores: Arc<dyn StoreResolver> = Arc::new(SingleStore::new(store.clone()));
    let keys: Arc<dyn KeyProvider> = Arc::new(ordo_crypto::custody::TestProvider::new());
    let pipe = BlobPipeline::new(stores, keys, BlobPolicy::DEFAULT);
    let space = SpaceId(SPACE.into());
    let h = orphan(&control, &pipe, &space, b"the store keeps this").await;

    let refusing: Arc<dyn StoreResolver> = Arc::new(SingleStore::new(Arc::new(RefusesDeletes {
        inner: store.clone(),
    })));
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let sup = start_gc(&control, CONTROL_DB, refusing, stream.clone()).await;
    assert!(passes(&sup, 2).await, "the sweep never ran");
    sup.shutdown(Duration::from_secs(5)).await;

    assert!(store.head(&space, &h).await.is_ok(), "the store kept it");
    let entries = gc_entries(&stream);
    assert!(!entries.is_empty(), "a refused pass records an entry");
    for (s, action, pass) in &entries {
        assert_eq!(s, SPACE, "the entry is the tenant's own");
        assert_eq!(
            action,
            &format!(
                "blob_collected deleted=0 not_deleted_store=1 not_deleted_database=0 pass={pass}"
            )
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// The cadence is one setting with a shipped default: unset is an hour,
/// a value is read, and zero falls back rather than disabling the sweep.
#[test]
fn the_cadence_setting_is_read_with_its_default() {
    std::env::remove_var("ORDO_BLOB_GC_SECS");
    assert_eq!(
        BlobGc::period_from_env(),
        Duration::from_secs(BlobGc::DEFAULT_SECS)
    );
    assert_eq!(BlobGc::DEFAULT_SECS, 3600);
    std::env::set_var("ORDO_BLOB_GC_SECS", "90");
    assert_eq!(BlobGc::period_from_env(), Duration::from_secs(90));
    std::env::set_var("ORDO_BLOB_GC_SECS", "0");
    assert_eq!(
        BlobGc::period_from_env(),
        Duration::from_secs(BlobGc::DEFAULT_SECS),
        "zero is the default, never a sweep that spins or one that never runs"
    );
    std::env::remove_var("ORDO_BLOB_GC_SECS");
}
