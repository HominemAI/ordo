//! The blob door's real implementation, driven through the real router.
//!
//! `ordo_api::router_with_blobs` is exercised in `ordo-api`'s own
//! `tests/blob_door.rs` against a hand-written `FakePort` — proving the
//! half `ordo-api` owns (routing, the door check, `Content-Length`, ranges,
//! response headers) without a database in sight. `ordo_host::blob`'s
//! `PlatformBlobs` — the ~350 lines that actually run the sealer, the
//! store and the row guard — had no test of its own kind: no
//! `#[cfg(test)]` in `blob.rs`, and the fake port never touches it.
//!
//! This file is that test. It mirrors `tests/attach_bucket.rs`'s harness
//! shape (a real fixture, a real pool, a real `AccessEngine`, a real
//! `Deployment`) and adds what a blob needs beyond that: a schema with a
//! blob field in each tier, and a real `FsStore` behind a `BlobPipeline`.
//!
//! # What is proved here, and why each thing is the point
//!
//! `blob.rs`'s own module doc names three things its signature does not
//! show: the row guard runs, the tier comes from the schema and not the
//! request, and the inline cap is checked before the sealer. The row guard
//! is the one this file is built around — "a blob served without it is
//! every byte delivered correctly to the wrong person, and nothing about
//! it looks wrong" is not a property a fake port can be asked to violate,
//! because a fake port has no rows to hide one behind. Proving it needs a
//! real schema, a real guard predicate, and a caller the DOOR would admit
//! (the std op reaches their role) but the ROW does not — which is exactly
//! the gap between "may this principal call this op" and "may this
//! principal see this row" the module doc draws.
//!
//! The transaction-ordering test is the other half of the same doc: store
//! the object, then open the transaction, then check the guard. A refusal
//! there must roll back the reference and the column and must NOT delete
//! the object it already wrote — that asymmetry is deliberate (the sweep
//! collects the leak; a dangling reference has no such mechanism), and it
//! is asserted directly rather than trusted from a reading of the code.
//!
//! # The defect this file found
//!
//! `deployment_engine` never granted `std.$blob.get` — the reserved
//! pseudo-op `blob_gate` checks a `GET` against, before an entity is even
//! known. So under a deployment wired the way `deployment_engine` wires
//! one, **every `GET /blob/{handle}` was `access_denied` for every
//! principal in every role, unconditionally**: the row guard, the tier
//! check and everything else in `PlatformBlobs::read` were unreachable,
//! not because of anything `blob.rs` did but because the door in front of
//! it never opened.
//!
//! It is granted now (`deployment_engine`, to manager and owner, by the
//! std exposure rule rather than as an exception to it), so this suite
//! runs against the **real** profile with no patch — which is the only
//! way it can keep proving the door is reachable.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ordo_api::blob::{BlobLimits, BlobPort, BlobState};
use ordo_api::{router_with_blobs, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore, Upload};
use ordo_commandlog::store::Store;
use ordo_conformance::{Fixture, Pg, WriteLog, FIXTURE, SEED};
use ordo_crypto::custody::SpaceId as CryptoSpace;
use ordo_host::blob::PlatformBlobs;
use ordo_host::deploy::Deployment;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"blob-door-test-secret";

/// The fixture's own seeded members (`tests/attach_bucket.rs`'s consts,
/// unchanged): `mem_o` is Owner, `mem_1` is Manager. Both roles reach
/// `std.<entity>.*` under `deployment_engine`'s template — Viewer and
/// Editor deliberately do not (`std-library.md` §5 rule 3) — so both are
/// usable to prove the row guard is a SECOND check, past a door both of
/// them clear.
const OWNER: &str = "mem_o";
const MANAGER: &str = "mem_1";

/// One entity, one blob field per custody tier, one guard: rows are
/// visible to and writable by their own `owner` alone. Appended to the
/// shipped fixture rather than edited into it — `Fixture::from_source`'s
/// documented door for exactly this, used the same way
/// `tests/guard_groups.rs` uses it for its own `ESTATE` cluster — so
/// nothing here moves a number another suite asserts about `market.ordo`.
const DOCS_SCHEMA: &str = "
entity docs (key: doc) {
  owner -> members
  title:  text
  scan:   blob?
  photo:  blob? inline
  secret: blob? private
  vault:  blob? sealed
  access read   owner == $caller
  access write  owner == $caller
}

entity posters (key: post) {
  title: text
  art:   blob?
  public
}
";

struct Harness {
    app: axum::Router,
    pool: ordo_exec::Pool,
    pipeline: BlobPipeline,
    dir: PathBuf,
    /// The chain an upload's command group joins. Shared with the door,
    /// because an upload is a converge and a converge appends a group.
    log: Arc<Mutex<WriteLog>>,
    /// The fixture the door plans against, for the test that drives a
    /// revert rather than a `PUT`.
    fixture: Arc<Fixture>,
    /// The door's state, the real port and the resolver, for the test that
    /// merges the function route over the same door.
    state: ApiState,
    port: Arc<dyn BlobPort>,
    spaces: Arc<ordo_host::spaces::Spaces>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// No ops in this rig, matching `ordo-api`'s own blob-door test: the blob
/// routes never reach the executor, and a rig that could execute one would
/// be testing the op path by accident.
struct NoExec;

impl ordo_api::spec::OpExecutor for NoExec {
    fn execute<'a>(
        &'a self,
        _op: &'a ordo_api::OpSpec,
        _space: &'a str,
        _bound: ordo_api::BoundParams,
        _refine: Option<ordo_api::Refinement>,
        _page: Option<ordo_api::PageCursor>,
        _lease: Option<ordo_api::ReplayLease>,
    ) -> futures::future::BoxFuture<'a, Result<ordo_api::ExecOutcome, ordo_api::ExecError>> {
        Box::pin(async { Err(ordo_api::ExecError::internal("no ops in this rig")) })
    }
}

async fn harness(db: &str) -> Harness {
    let dir = std::env::temp_dir().join(format!("ordo-blob-door-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    harness_with_store(db, dir, store).await
}

/// The same rig as [`harness`], with the store the door writes through
/// swapped for the caller's own.
///
/// `a_store_error_is_never_returned_to_the_caller` needs a real transport
/// failure driven through the real door — `PlatformBlobs::write`, the real
/// pipeline, the real router — rather than a reading of `blob.rs`'s error
/// maps, and the fixture, the pool and the access engine underneath that
/// are identical to every other test in this file. Factored out rather
/// than duplicated so the two rigs cannot drift apart.
async fn harness_with_store(db: &str, dir: PathBuf, store: Arc<dyn BlobStore>) -> Harness {
    harness_with_resolver(db, dir, Arc::new(ordo_blob::SingleStore::new(store))).await
}

/// The same rig again, with the door's resolver chosen by the caller: the
/// detach and replace tests below hand it `AttachedStores`, which is what
/// a deployment on attached routing runs.
async fn harness_with_resolver(
    db: &str,
    dir: PathBuf,
    stores: Arc<dyn ordo_blob::StoreResolver>,
) -> Harness {
    harness_full(db, dir, stores, "").await
}

/// The rig over the fixture, [`DOCS_SCHEMA`] and `extra` source.
async fn harness_full(
    db: &str,
    dir: PathBuf,
    stores: Arc<dyn ordo_blob::StoreResolver>,
    extra: &str,
) -> Harness {
    let src = format!("{FIXTURE}\n{DOCS_SCHEMA}\n{extra}");
    let f = Arc::new(
        Fixture::from_source(&src, SEED).expect("the fixture plus the docs entity compiles"),
    );
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");

    // `Pool::connect` runs `ensure_platform_tables`, which is what actually
    // creates `$blobref`/`$blobtomb` — `Fixture::reset`'s DDL is schema-only
    // (see its own doc for why `$sealedpayload` needs the same workaround).
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");

    // The real profile, unpatched. That is the point: this suite exists
    // to prove the blob door is reachable under the deployment everybody
    // actually gets, and a patched profile would prove it about one
    // nobody runs.
    let engine = Arc::new(ordo_host::deployment_engine(&f, &pg).expect("engine"));
    let deployment = Deployment::pinned(
        "blob-door-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );

    // The door's pipeline takes the **resolver**, which is what the shipped
    // runtime hands it. `SingleStore` over the one store this rig
    // configured is the resolver a deployment that has attached no bucket
    // has, so the door under test is the door that ships.
    let pipeline = BlobPipeline::new(stores, f.keys.clone(), BlobPolicy::DEFAULT);

    // The resolver the door writes through, and the space's chain read
    // back off it: the log an upload appends to is the one `Spaces` holds
    // for the space the request named, not one this harness owns.
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let log = spaces.log_of(ordo_host::DEFAULT_SPACE);
    let port = Arc::new(PlatformBlobs {
        spaces: spaces.clone(),
        pipeline: pipeline.clone(),
    });

    let access = Arc::new(ordo_host::IdentityAccess::sharing(
        engine.clone(),
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    ));

    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(NoExec),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"blob-door-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };

    let port = port as Arc<dyn BlobPort>;
    let app = router_with_blobs(
        state.clone(),
        BlobState {
            port: port.clone(),
            limits: BlobLimits::DEFAULT,
        },
    );

    Harness {
        app,
        pool,
        pipeline,
        dir,
        log,
        fixture: f.clone(),
        state,
        port,
        spaces,
    }
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn insert_doc(pool: &ordo_exec::Pool, id: &str, owner: &str, title: &str) {
    let title = title.replace('\'', "''");
    pool.rows(&format!(
        "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"owner\", \"title\") \
         VALUES ('{}', '{id}', 1, '{owner}', '{title}')",
        ordo_host::DEFAULT_SPACE
    ))
    .await
    .expect("insert doc");
}

async fn scan_column(pool: &ordo_exec::Pool, id: &str) -> Option<String> {
    pool.rows(&format!(
        "SELECT \"scan\" FROM \"docs\" WHERE \"$space\" = '{}' AND \"id\" = '{id}'",
        ordo_host::DEFAULT_SPACE
    ))
    .await
    .expect("select")
    .into_iter()
    .next()
    .and_then(|r| r.into_iter().next())
    .flatten()
}

async fn blobref_count(pool: &ordo_exec::Pool) -> i64 {
    pool.rows("SELECT count(*) FROM \"$blobref\"")
        .await
        .expect("count")[0][0]
        .clone()
        .expect("count(*) is never null")
        .parse()
        .expect("count(*) is an integer")
}

/// Every regular file under `dir`, recursively — a `FsStore` root has no
/// other kind of entry, so this counts objects without knowing a single
/// handle.
fn count_files(dir: &std::path::Path) -> usize {
    fn walk(dir: &std::path::Path, n: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, n);
            } else {
                *n += 1;
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

async fn put_blob(
    app: &axum::Router,
    entity: &str,
    field: &str,
    id: &str,
    who: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::PUT)
        .uri(format!("/blob/{entity}/{field}/{id}"))
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
        .header("content-length", body.len().to_string())
        .header("content-type", "application/pdf")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// On success, the bytes; on refusal, the error body. Never both, which is
/// what lets every call site assert on exactly the half it asked for.
async fn get_blob(app: &axum::Router, handle_hex: &str, who: &str) -> (StatusCode, Vec<u8>, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blob/{handle_hex}"))
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    if status.is_success() {
        (status, bytes.to_vec(), Value::Null)
    } else {
        (
            status,
            Vec::new(),
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

// ── the round trip ──────────────────────────────────────────────────────

/// **The round trip through the real door.** A `PUT` lands the object and
/// the reference in one transaction, the row's column actually holds the
/// handle afterwards, a `$blobref` row exists naming this entity and
/// field, and a `GET` of the minted handle returns the identical bytes —
/// through the router, the real sealer, and a real `FsStore`, none of
/// which `blob_door.rs`'s `FakePort` exercises.
#[tokio::test(flavor = "multi_thread")]
async fn a_round_trip_through_the_real_door_returns_the_same_bytes() {
    let h = harness("ordo_blobdoor_roundtrip").await;
    insert_doc(&h.pool, "doc_1", OWNER, "quarterly report").await;

    let bytes: Vec<u8> = (0u8..=255).cycle().take(5_000).collect();
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, bytes.clone()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let handle_hex = scan_column(&h.pool, "doc_1")
        .await
        .expect("the column was set");
    assert_eq!(
        handle_hex.len(),
        32,
        "a blob handle is 16 bytes, hex-encoded"
    );

    let refs = h
        .pool
        .rows(&format!(
            "SELECT \"entity\", \"field\" FROM \"$blobref\" \
             WHERE \"$space\" = '{}' AND \"ref\" = decode('{handle_hex}', 'hex')",
            ordo_host::DEFAULT_SPACE
        ))
        .await
        .expect("select blobref");
    assert_eq!(refs.len(), 1, "exactly one reference row for this handle");
    assert_eq!(refs[0][0].as_deref(), Some("docs"));
    assert_eq!(refs[0][1].as_deref(), Some("scan"));

    let (status, got, err) = get_blob(&h.app, &handle_hex, OWNER).await;
    assert_eq!(status, StatusCode::OK, "{err}");
    assert_eq!(got, bytes);
}

/// **U0 · a store error's text never reaches the caller.** `BlobError::
/// Transport` and `BlobError::Refused` carry the provider's own words — a
/// bucket name, a principal, a body an operator needs and a caller must
/// never see — and this property holds today only because every call site
/// in `blob.rs` maps a store failure through `internal("blob storage")`
/// without ever formatting the underlying error into the message. Nothing
/// asserted that before this test: one `format!("{e}")` slipped into one
/// of those maps would put a bucket's internals into an HTTP response and
/// nothing would fail red.
///
/// Driven against [`unreachable`], whose every method answers
/// `BlobError::Transport(DOWN)`, through the real router rather than by
/// calling `blob.rs`'s mapping functions directly — the assertion is on
/// the bytes a caller actually receives, which is what the property is
/// about.
///
/// **Falsified by**: temporarily changing `from_pipeline`'s
/// `PipelineError::Custody(_) | PipelineError::Store(_) => internal("blob
/// storage")` arm to `internal(&e.to_string())` and re-running this test —
/// it turns red with `DOWN`'s exact text in the response body. Reverted
/// before landing; nothing here depends on that change staying.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_error_is_never_returned_to_the_caller() {
    let dir = std::env::temp_dir().join(format!("ordo-blob-door-storeerr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let h = harness_with_store(
        "ordo_blobdoor_storeerr",
        dir,
        Arc::new(unreachable()) as Arc<dyn BlobStore>,
    )
    .await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let (status, body) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        OWNER,
        b"bytes nobody sees".to_vec(),
    )
    .await;
    // A transport failure is transient (audit bug 6): `server_busy`, so
    // every client sends it again, never a 500.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "server_busy", "{body}");
    let text = body.to_string();
    assert!(
        !text.contains(DOWN),
        "the store's own words reached the caller: {text}"
    );
}

/// A store that refuses for good (a 403, a bad credential) is still the
/// opaque 500, and its words still never reach the caller.
#[tokio::test(flavor = "multi_thread")]
async fn a_permanent_store_refusal_stays_internal() {
    let dir = std::env::temp_dir().join(format!("ordo-blob-door-refused-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let h = harness_with_store(
        "ordo_blobdoor_store_refused",
        dir,
        Arc::new(Failing(ordo_blob::BlobError::Refused(DOWN.into()))) as Arc<dyn BlobStore>,
    )
    .await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"x".to_vec()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(!body.to_string().contains(DOWN), "{body}");
}

// ── the row guard ────────────────────────────────────────────────────────

/// **The obligation the signature does not show, enforced.** MANAGER's
/// role reaches `std.docs.get` and `std.docs.update` — `deployment_engine`
/// grants every std op to Manager and Owner alike — so a door that only
/// checked "may this principal call this op" would let MANAGER read and
/// overwrite OWNER's file. The row guard is the second check, and this is
/// the one property a fake port cannot be asked to get wrong, because a
/// fake port has no row to hide behind.
#[tokio::test(flavor = "multi_thread")]
async fn the_row_guard_refuses_a_caller_the_door_would_otherwise_admit() {
    let h = harness("ordo_blobdoor_guard").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let (status, body) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        OWNER,
        b"owner's bytes".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let handle_hex = scan_column(&h.pool, "doc_1").await.expect("set");

    // Read: MANAGER clears the door and is refused the row, with the answer
    // a handle that is not there gets (owner, 2026-10-25).
    let (status, _, err) = get_blob(&h.app, &handle_hex, MANAGER).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], "not_found", "{err}");

    // Write: same caller, same row, refused. **`not_found`, not
    // `access_guard`**, and the change of class is C4's, deliberate. The
    // write is the write protocol's now, and the protocol evaluates the
    // read guard before the write guard because the order IS the
    // disclosure rule (R-G10): a row outside the caller's read view must
    // answer exactly like a row that never existed. The old door checked
    // the write guard alone, so it told a caller who cannot see `doc_1`
    // that `doc_1` is there, which is the enumeration answer this door's
    // own `not_found()` helper exists to refuse everywhere else.
    let (status, err) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        MANAGER,
        b"an intruder's overwrite".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], "not_found", "{err}");

    // OWNER's bytes are exactly as they were.
    let (status, got, err) = get_blob(&h.app, &handle_hex, OWNER).await;
    assert_eq!(status, StatusCode::OK, "{err}");
    assert_eq!(got, b"owner's bytes");
}

/// `GET /blob/{handle}/meta`: the media type and plaintext size, behind the
/// same row guard as the bytes.
#[tokio::test(flavor = "multi_thread")]
async fn the_meta_read_answers_type_and_size_behind_the_row_guard() {
    let h = harness("ordo_blobdoor_meta").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let bytes: Vec<u8> = (0u8..=255).cycle().take(5_000).collect();
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, bytes).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let handle_hex = scan_column(&h.pool, "doc_1").await.expect("set");

    let meta = |who: &'static str| {
        let app = h.app.clone();
        let uri = format!("/blob/{handle_hex}/meta");
        async move {
            let req = Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header("authorization", format!("Bearer {}", token(who)))
                .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };

    let (status, v) = meta(OWNER).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["mime"], "application/pdf");
    assert_eq!(v["size"], 5_000);

    // MANAGER clears the door and is refused the row: no type, no size.
    let (status, v) = meta(MANAGER).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
    assert!(v.get("mime").is_none() && v.get("size").is_none(), "{v}");
}

// ── the two tiers this door cannot serve ────────────────────────────────

/// **`blob private` and `blob inline` each refuse a write by name**, not
/// as an unclassified failure the caller has to reverse-engineer. Both are
/// `invalid_param`: the request named a field this route does not serve,
/// which is a property of the request, not a new refusal class.
#[tokio::test(flavor = "multi_thread")]
async fn private_and_inline_tiers_refuse_a_write_by_name() {
    let h = harness("ordo_blobdoor_tiers").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let (status, body) = put_blob(&h.app, "docs", "secret", "doc_1", OWNER, b"shh".to_vec()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_param");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("blob private"),
        "the refusal did not name the tier: {body}"
    );

    let (status, body) =
        put_blob(&h.app, "docs", "photo", "doc_1", OWNER, b"pixels".to_vec()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_param");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("blob inline"),
        "the refusal did not name the tier: {body}"
    );

    // Neither reached the store or the log: no reference row exists at all,
    // and the columns are still unset.
    assert_eq!(blobref_count(&h.pool).await, 0);
    assert_eq!(scan_column(&h.pool, "doc_1").await, None);
}

// ── the third tier this door cannot serve ───────────────────────────────

/// **`blob sealed` refuses a write by name too, and the refusal states
/// why it is not merely unfinished.** Unlike `private` and `inline`, the
/// declaration reaches the door's tier switch cleanly (`tier_of` reads
/// `f.sealed` like any other flag) and nothing about the request is
/// malformed: the field is exactly what it says it is. The refusal
/// exists anyway because the collector cannot serve it: a sealed handle
/// carries a fresh nonce, so the live-reference query can never compare
/// two envelopes of the same handle and find them equal, and the
/// collector holds no keys on purpose so it cannot open one to check some
/// other way.
#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_blob_field_is_refused_by_name() {
    let h = harness("ordo_blobdoor_sealed").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let (status, body) = put_blob(
        &h.app,
        "docs",
        "vault",
        "doc_1",
        OWNER,
        b"a contract".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_param");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("blob sealed"), "did not name the tier: {body}");
    assert!(
        msg.contains("collector") && msg.contains("nonce"),
        "did not state why the tier is unservable, not merely unfinished: {body}"
    );

    // Nothing reached the store or the log: no reference row, no column.
    assert_eq!(blobref_count(&h.pool).await, 0);
    assert_eq!(scan_column(&h.pool, "doc_1").await, None);
}

/// **The read path refuses `blob sealed` too, and it has to be checked
/// separately from the write refusal above.** A caller cannot produce a
/// legitimate handle for this tier (the write above is refused for every
/// caller), so this simulates the one way a `$blobref` row naming a
/// sealed field could exist: written directly, bypassing the door
/// entirely, the way a bug or a hand migration might. Even then, the read
/// route refuses rather than serving it, because a route that read it
/// would treat the column as a live reference the collector's
/// `live_reference_clause` has been told to ignore, which is exactly the
/// dangling-reference shape this module's tests otherwise guard against.
#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_blob_field_is_also_refused_on_read() {
    let h = harness("ordo_blobdoor_sealed_read").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    // A `$blobref` row naming `docs.vault`, written directly: nothing a
    // caller of this door can produce, since the write above refuses it.
    let handle = [0x42u8; 16];
    let space = ordo_crypto::custody::SpaceId(ordo_host::DEFAULT_SPACE.to_string());
    h.pool
        .rows(&ordo_crypto::blobref::insert_sql(
            &space,
            &handle,
            1,
            "docs",
            "vault",
            b"application/octet-stream",
            b"3",
            b"not-a-real-envelope",
            3,
            0,
            "",
        ))
        .await
        .expect("insert a synthetic blobref row");

    let handle_hex = ordo_crypto::hex(&handle);
    let (status, _, err) = get_blob(&h.app, &handle_hex, OWNER).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "invalid_param");
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("blob sealed"), "did not name the tier: {err}");
}

// ── the non-enumerable denial ────────────────────────────────────────────

/// **A handle in another space and a handle that never existed answer
/// identically.** `PlatformBlobs::read` scopes its very first lookup by
/// the caller's own space, so a real reference minted under a space this
/// caller did not name is invisible before the row guard, the tier check,
/// or anything else runs — the same `not_found` an absent handle gets.
#[tokio::test(flavor = "multi_thread")]
async fn a_handle_in_another_space_is_not_found_like_an_absent_one() {
    let h = harness("ordo_blobdoor_space").await;

    // A real object and a real reference, minted directly through the
    // pipeline under a space this door's caller has no membership in.
    let other = CryptoSpace("a-different-space".into());
    let stored = h
        .pipeline
        .put(
            &Upload {
                space: &other,
                entity: "docs",
                field: "scan",
                mime: "text/plain",
                plain_len: 5,
                correlation: "test",
                intent: None,
            },
            ordo_blob::once(b"hello".to_vec()),
        )
        .await
        .expect("put");
    // The reference row alone, written directly. There is no `attach` to
    // call any more (C4 moved the door's write into the write protocol),
    // and there is no row in the other space to converge onto: what this
    // test needs is a real `$blobref` row under a space the caller did not
    // name, which is what the read path's first lookup is scoped by.
    let tx = h.pool.begin().await.expect("tx");
    tx.rows(&ordo_crypto::blobref::insert_sql(
        &other,
        stored.handle(),
        stored.epoch(),
        "docs",
        "scan",
        stored.mime(),
        stored.size(),
        stored.integrity(),
        stored.bytes(),
        0,
        "",
    ))
    .await
    .expect("the reference row");
    tx.commit().await.expect("commit");

    let handle_hex = ordo_crypto::hex(stored.handle());
    let (status, _, err) = get_blob(&h.app, &handle_hex, OWNER).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    assert_eq!(err["error"]["code"], "not_found", "{err}");

    // An absent handle: the same status and the same class.
    let (status2, _, err2) = get_blob(&h.app, &"a".repeat(32), OWNER).await;
    assert_eq!(status2, status);
    assert_eq!(err2["error"]["code"], err["error"]["code"]);
}

// ── the transaction ordering ─────────────────────────────────────────────

/// **If the guard refuses on write, the transaction rolls back — and the
/// object may still be in the store.** `blob.rs`'s module doc states the
/// order (store the object, open a transaction, check the guard, insert
/// the reference, set the column, commit) and the asymmetry it buys: a
/// failure after the object is stored costs money (the sweep collects an
/// unreferenced object); the other ordering would risk a live column
/// pointing at nothing. Proven directly rather than trusted from the
/// reading: a refusal must leave no `$blobref` row and no column set,
/// while the object file count goes up by exactly one.
#[tokio::test(flavor = "multi_thread")]
async fn a_guard_refusal_on_write_rolls_back_the_row_and_reference_but_not_the_object() {
    let h = harness("ordo_blobdoor_rollback").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let refs_before = blobref_count(&h.pool).await;
    let files_before = count_files(&h.dir);

    let (status, body) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        MANAGER,
        b"an intruder's bytes".to_vec(),
    )
    .await;
    // R-G10 again: MANAGER cannot read `doc_1`, so the refusal names
    // nothing. What this test is about is what happened to the bytes, and
    // that is unchanged.
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found");

    assert_eq!(
        blobref_count(&h.pool).await,
        refs_before,
        "the reference did not commit"
    );
    assert_eq!(
        scan_column(&h.pool, "doc_1").await,
        None,
        "the column was not set"
    );
    assert_eq!(
        count_files(&h.dir),
        files_before + 1,
        "the object is stored BEFORE the guard runs, so a refusal leaks it \
         rather than leaving a dangling reference — the sweep's job, not a \
         delete path racing it"
    );
}

// ── the leak the sweep could not reach (plan-tenant-storage.md §5 B-F3) ──

/// Every **open** tombstone in the fixture space, as hex handles.
///
/// An open `$blobtomb` row with no `$blobref` row beside it is an upload
/// that never committed: phase 1 of the sweep only ever tombstones handles
/// it read out of `$blobref`, so it cannot produce one of these, and the
/// door's declared intent is the only thing that can.
async fn open_intents(pool: &ordo_exec::Pool) -> Vec<String> {
    pool.rows(&format!(
        "SELECT encode(\"ref\", 'hex') FROM \"$blobtomb\" \
         WHERE \"$space\" = '{}' AND \"deleted_at\" IS NULL ORDER BY \"ref\"",
        ordo_host::DEFAULT_SPACE
    ))
    .await
    .expect("tombstones")
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect()
}

/// Every tombstone, open or closed. What proves the table does not only
/// grow.
async fn all_tombstones(pool: &ordo_exec::Pool) -> i64 {
    pool.rows("SELECT count(*) FROM \"$blobtomb\"")
        .await
        .expect("count")[0][0]
        .clone()
        .expect("count(*) is never null")
        .parse()
        .expect("count(*) is an integer")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_millis() as u64
}

const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

/// Stamp every group in this space's chain at `secs`, so the chain's
/// retained tail is contemporaneous with a sweep run at that instant.
///
/// Not decoration, and not a way to make an assertion pass. `Floor` caps
/// `live_bound` at the last stored entry, because the tail is retained
/// whatever the horizon says, and `blobgc` converts that position into a
/// tombstone's coordinate by reading the group's own `at`. So a test that
/// advances `now` by ninety-one days while leaving the chain stamped
/// today is asking the sweep to price today's tombstones against a log
/// whose newest entry is ninety-one days stale, and the sweep correctly
/// answers by pinning the cutoff to that entry and collecting nothing.
/// Every "the object survived" assertion below would then pass against a
/// build carrying the defect, because nothing could have been collected in
/// the first place.
///
/// A deployment that actually reached that instant would have written
/// since, or would have a tail that old and would be right to be
/// conservative about it. This is the first case.
async fn chain_stamped_at(h: &Harness, secs: u64) {
    let mut groups = logged_groups(h);
    for g in &mut groups {
        for c in &mut g.commands {
            c.at = ordo_commandlog::Timestamp(secs);
        }
    }
    replace_chain(h, &groups).await;
}

/// One real sweep at `now`, over the store the door actually wrote
/// through and the schema the door actually planned against.
///
/// Not a reading of `$blobtomb`: what the increment claims is that an
/// interrupted upload is something the **sweep** can reach, and the only
/// way to show that is to run it. The sweep is handed the door's **own**
/// resolver rather than one built beside it, which is the arrangement the
/// runtime wires: one answer to where a space's bytes are, asked by the
/// path that writes them and the path that collects them alike.
async fn sweep(h: &Harness, now: u64) -> ordo_exec::blobgc::SweepReport {
    // `$logconsumers` and `$checkpoints` are the prune's, created on demand
    // rather than at boot, and the blob sweep reads the floor the prune
    // computes. One floor, one set of inputs.
    ordo_exec::prune::ensure_tables(&h.pool)
        .await
        .expect("prune tables");
    ordo_exec::cadence::ensure_tables(&h.pool)
        .await
        .expect("cadence tables");
    ordo_exec::blobgc::sweep(
        &h.pool,
        h.pipeline.stores.as_ref(),
        &h.fixture.schema,
        &CryptoSpace(ordo_host::DEFAULT_SPACE.into()),
        &ordo_exec::cadence::LogPolicy {
            undo_horizon: std::time::Duration::from_secs(90 * 86_400),
            ..ordo_exec::cadence::LogPolicy::DEFAULT
        },
        now,
    )
    .await
    .expect("sweep")
}

/// **An upload interrupted between the object write and the transaction
/// leaves a tombstone, not an object nothing can name.**
///
/// The interruption is real and is the door's own: `blob.rs` stores the
/// object, then opens the write protocol's transaction, and the row guard
/// is evaluated **inside** it. `MANAGER` does not own `doc_1`, so the
/// protocol refuses and rolls back, after the bytes are in the store and
/// before anything about them is committed. That is precisely the window
/// B-F3 names, produced by running the real door rather than described by
/// inserting the state it leaves.
///
/// The distinction matters more than it looks. A test that writes a
/// `$blobtomb` row by hand and then sweeps proves the sweep reads a row; it
/// says nothing about whether the door writes one, or writes it before the
/// `put` rather than after it, and "after the `put`" is the same bug in a
/// smaller window.
///
/// What is asserted is the pair: the object exists, and the tombstone names
/// **that** object. A tombstone for some other handle would satisfy a count
/// and leave the bytes exactly as unreachable as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_upload_leaves_a_tombstone_not_an_orphan() {
    let h = harness("ordo_blobdoor_intent").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    assert!(
        open_intents(&h.pool).await.is_empty(),
        "nothing has been uploaded yet"
    );

    let files_before = count_files(&h.dir);
    let (status, body) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        MANAGER,
        b"an interrupted upload".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    assert_eq!(
        blobref_count(&h.pool).await,
        0,
        "the transaction rolled back, so nothing names the object"
    );
    assert_eq!(scan_column(&h.pool, "doc_1").await, None);
    assert_eq!(
        count_files(&h.dir),
        files_before + 1,
        "the bytes were written before the transaction opened, which is the window"
    );

    let intents = open_intents(&h.pool).await;
    assert_eq!(
        intents.len(),
        1,
        "the door declares the handle before the `put`, so an upload that never commits \
         still leaves something the sweep can find. `BlobStore` has no `list`: without \
         this row the object is unreachable by every mechanism in the deployment, \
         permanently, and no count of files or bytes anywhere would ever mention it"
    );
    // Names *that* object. `object_path` panics when no file carries the
    // name, which is the assertion: a tombstone for a handle the store
    // never received leaves the real bytes exactly as lost as no tombstone
    // at all.
    let path = object_path(&h.dir, &intents[0]);
    assert!(path.is_file(), "{}", path.display());
}

/// **A committed upload leaves no tombstone behind, proven by sweeping.**
///
/// This is the dangerous half of B-F3. A tombstone that outlives a
/// successful upload is a live object queued for deletion, and the failure
/// is silent until ninety days later when a file a tenant still has a row
/// for stops resolving.
///
/// It is asserted three ways, because the table being empty is the weakest
/// of them. The sweep runs past the floor, with `cutoff` asserted to be
/// past the upload so that a run which could not have collected anything
/// cannot pass this test by doing nothing, and it reports `deleted: 0` and
/// `cancelled: 0`. The second number is the one that distinguishes the two
/// designs: a build where the reference insert did not cancel the intent
/// would leave the tombstone standing, the sweep would find it due, the
/// re-check would find the reference live, and it would report
/// `cancelled: 1`. The bytes would survive that run, so an assertion on the
/// object alone would pass against a build carrying the defect.
///
/// Then the object is fetched back through the door, which is the only
/// statement a tenant would recognise.
#[tokio::test(flavor = "multi_thread")]
async fn a_committed_upload_cancels_its_own_tombstone() {
    let h = harness("ordo_blobdoor_cancel").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let at = now_ms();
    let (status, body) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        OWNER,
        b"a signed contract".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let handle = scan_column(&h.pool, "doc_1").await.expect("the column");
    assert_eq!(blobref_count(&h.pool).await, 1);

    assert_eq!(
        open_intents(&h.pool).await,
        Vec::<String>::new(),
        "the `$blobref` insert cancels the intent in the same statement, so a committed \
         upload is never a live object with a tombstone queued against it"
    );

    chain_stamped_at(&h, (at + 91 * DAY_MS) / 1_000).await;
    let r = sweep(&h, at + 91 * DAY_MS).await;
    assert!(
        r.cutoff > at,
        "the sweep has to be past the floor for this test to mean anything: a run that \
         could collect nothing would report `deleted: 0` whatever the table held \
         (cutoff {}, upload {at})",
        r.cutoff,
    );
    assert_eq!(r.marked, 0, "a live row names the handle");
    assert_eq!(
        r.cancelled, 0,
        "nothing was left for the sweep to cancel. A `1` here is the defect: the intent \
         survived the commit, and it survived it carrying the instant the upload STARTED \
         as its `unreferenced_since`"
    );
    assert_eq!(r.deleted, 0);
    assert_eq!(r.waiting, 0);
    assert_eq!(all_tombstones(&h.pool).await, 0);

    let (status, bytes, err) = get_blob(&h.app, &handle, OWNER).await;
    assert_eq!(status, StatusCode::OK, "{err}");
    assert_eq!(bytes, b"a signed contract", "the object survived the sweep");
}

/// **The floor is computed from the delete, not from the upload.**
///
/// The regression guard for the way B-F3 goes wrong quietly. Phase 1 will
/// not tombstone a handle that already has an open tombstone, so an intent
/// left standing beside a committed reference does not merely linger: it
/// becomes the row the floor is compared against for every later
/// unreferencing of that handle. Delete the row a year after the upload and
/// the sweep prices the delete against a timestamp from the upload, finds
/// it long past the horizon, and takes the bytes while the delete is still
/// seconds old and fully revertible.
///
/// So: upload, then delete the row, then sweep past the *upload's* horizon.
/// The object must still be there, because the only tombstone that may
/// exist is one phase 1 stamped at the delete.
#[tokio::test(flavor = "multi_thread")]
async fn a_delete_long_after_an_upload_is_floored_from_the_delete() {
    let h = harness("ordo_blobdoor_floor").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let at = now_ms();
    let (status, body) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        OWNER,
        b"a contract".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let handle = scan_column(&h.pool, "doc_1").await.expect("the column");

    // The row goes, long after the upload. Raw SQL rather than a revert or
    // an op, because what is under test is the arithmetic the sweep does
    // over the state a delete leaves, and a compiled delete would put a
    // whole write pipeline between the assertion and the thing it is about.
    let later = at + 91 * DAY_MS;
    h.pool
        .rows(&format!(
            "DELETE FROM \"docs\" WHERE \"$space\" = '{}' AND \"id\" = 'doc_1'",
            ordo_host::DEFAULT_SPACE
        ))
        .await
        .expect("delete");

    chain_stamped_at(&h, later / 1_000).await;
    let r = sweep(&h, later).await;
    assert!(
        r.cutoff > at,
        "the cutoff has to be past the upload, or a surviving intent stamped at the \
         upload would not have been due either and this test would prove nothing \
         (cutoff {}, upload {at})",
        r.cutoff,
    );
    assert_eq!(
        r.deleted, 0,
        "the delete is seconds old and revertible for ninety days. A `1` here means the \
         sweep priced it against the upload's own timestamp, which is what a tombstone \
         surviving a committed upload turns every delete into"
    );
    assert!(object_path(&h.dir, &handle).is_file());
    assert_eq!(
        r.marked, 1,
        "the tombstone the sweep acts on is one phase 1 stamped at the delete. A `0` \
         here is the mechanism of the failure above: phase 1 will not tombstone a \
         handle that already has an open one, so an intent left standing becomes the \
         row every later unreferencing of this handle is priced against"
    );
    assert_eq!(r.waiting, 1);
}

/// **The sweep retires an abandoned upload once the floor passes**, and
/// the tombstone does not outlive the object it was written for.
///
/// The input is a real interrupted upload, produced by the same refusal
/// `a_failed_upload_leaves_a_tombstone_not_an_orphan` produces it with, so
/// this is the second half of one story rather than a sweep run against a
/// hand-built row. Three sweeps: inside the horizon nothing moves, past it
/// the object goes, and past it again the closed tombstone is reaped so
/// `$blobtomb` is not a table that only grows.
#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_retires_an_abandoned_upload_once_the_floor_passes() {
    let h = harness("ordo_blobdoor_retire").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let at = now_ms();
    let (status, _) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        MANAGER,
        b"an interrupted upload".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let handle = open_intents(&h.pool)
        .await
        .into_iter()
        .next()
        .expect("the interrupted upload left an intent");
    assert!(object_path(&h.dir, &handle).is_file());

    // Inside the horizon: found, waiting, untouched. The bytes are not
    // revertible into, because no command group was appended and the group
    // and the reference commit together, so the floor here is
    // conservatism rather than a bound. It is applied all the same,
    // because the sweep has one rule.
    let r = sweep(&h, at).await;
    assert_eq!(
        r.marked, 0,
        "phase 1 tombstones handles `$blobref` names, \
                             and there is no `$blobref` row: the intent is already there"
    );
    assert_eq!(r.deleted, 0);
    assert_eq!(
        r.waiting, 1,
        "the sweep can see it, which is the whole increment"
    );
    assert!(object_path(&h.dir, &handle).is_file());

    // Past the floor: the object goes.
    let r = sweep(&h, at + 91 * DAY_MS).await;
    assert_eq!(
        r.deleted, 1,
        "an upload nothing can enumerate becomes an object that retires on the same \
         floor as every other unreferenced object"
    );
    assert_eq!(r.waiting, 0);
    assert_eq!(
        r.holder,
        ordo_exec::prune::FloorHolder::UndoHorizon,
        "no lagging consumer, so the horizon is the holder and the sweep reports the \
         same name the log prune does"
    );
    assert_eq!(count_files(&h.dir), 0, "the bytes are gone");
    assert_eq!(
        blobref_count(&h.pool).await,
        0,
        "there never was a reference"
    );

    // The closed tombstone is phase 2's crash-recovery record. Once the
    // horizon has passed it again it answers nothing.
    assert_eq!(all_tombstones(&h.pool).await, 1, "closed, not yet reaped");
    sweep(&h, at + 200 * DAY_MS).await;
    assert_eq!(
        all_tombstones(&h.pool).await,
        0,
        "a table that only grows is the other way this leaks"
    );
}

// ── the upload is a converge (docs/plan-blob-chain.md §2, C4) ────────────

/// The `rev` and the `scan` column of one row, read outside any
/// transaction. Both are what an upload is supposed to move.
async fn row_state(pool: &ordo_exec::Pool, id: &str) -> (i64, Option<String>) {
    let rows = pool
        .rows(&format!(
            "SELECT \"rev\", \"scan\" FROM \"docs\" WHERE \"$space\" = '{}' AND \"id\" = '{id}'",
            ordo_host::DEFAULT_SPACE
        ))
        .await
        .expect("select");
    let row = rows.into_iter().next().expect("the row is there");
    (
        row[0]
            .clone()
            .expect("rev is not null")
            .parse()
            .expect("rev"),
        row[1].clone(),
    )
}

/// **An upload appends exactly one command group, and it is a converge on
/// the row that was uploaded to.**
///
/// This is the whole of C4 in one assertion. Before it, `attach` wrote the
/// reference row and the column and appended nothing, so an upload was
/// invisible to the chain: unrevertible, un-triggered, and not a version
/// change. The group here is built by the same code that builds an
/// ordinary converge's group, which is why its actor, its correlation
/// shape and its before/after images need no separate argument.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_appends_one_converge_group() {
    let h = harness("ordo_blobdoor_group").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    assert_eq!(h.log.lock().unwrap().len(), 0, "nothing has been written");

    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"bytes".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (len, group) = {
        let log = h.log.lock().unwrap();
        (log.len(), log.last().expect("the group"))
    };
    assert_eq!(len, 1, "one upload, one group");
    assert_eq!(
        group.actor,
        ordo_commandlog::Actor::User(OWNER.into()),
        "the upload is attributed to the principal who made it"
    );
    assert!(
        !group.owner_administration,
        "rule 6 is about acl link entities; a blob column is never one"
    );
    assert_eq!(group.commands.len(), 1, "one command");
    let cmd = &group.commands[0];
    assert_eq!(cmd.kind, ordo_commandlog::CommandKind::Converge);
    assert_eq!(cmd.entity, "docs");
    // The row the token named, and there is no other argument it could
    // have come from: `run_platform_converge` reads the target out of the
    // value.
    assert_eq!(cmd.row_id, "doc_1");

    let handle_hex = scan_column(&h.pool, "doc_1").await.expect("set");
    let before = cmd.before.as_ref().expect("a converge images before");
    let after = cmd.after.as_ref().expect("a converge images after");
    assert_eq!(
        before.get("scan"),
        Some(&ordo_commandlog::Value::Null),
        "the column held nothing before the upload"
    );
    assert_eq!(
        after.get("scan"),
        Some(&ordo_commandlog::Value::Text(handle_hex)),
        "the after-image names the object that was stored"
    );
    // Both images are full rows, built by the one image builder. But
    // "full" no longer means "the same key set" once a column carries a
    // digest sibling key only when it is non-null (`docs/plan-blob-
    // chain.md` §4.1, C5). `scan` was null before this upload and is not
    // after, so `after` carries exactly one key `before` does not:
    // `scan$digest`, the sealed digest over the object this upload
    // stored. Everything else still matches exactly.
    let before_keys: std::collections::BTreeSet<String> = before.keys().cloned().collect();
    let after_keys: std::collections::BTreeSet<String> = after.keys().cloned().collect();
    let extra: Vec<&str> = after_keys
        .difference(&before_keys)
        .map(String::as_str)
        .collect();
    assert_eq!(
        extra,
        vec!["scan$digest"],
        "the after-image's only extra key over the before-image's must be `scan$digest`"
    );
    assert!(
        before_keys.is_subset(&after_keys),
        "every key `before` has must still be in `after`"
    );
    assert!(
        matches!(
            after.get("scan$digest"),
            Some(ordo_commandlog::Value::SealedRef(_))
        ),
        "the digest rides as a SealedRef, never as the plaintext digest itself: {after:?}"
    );
}

/// **An upload moves `rev`**, because the platform converge emits the same
/// `rev = rev + 1` an addressed set emits.
///
/// The third consequence of the old shape, and the one a caller could be
/// hurt by without anything looking wrong: a client holding a version from
/// before an upload would have gone on believing it was current, and a
/// compare-and-set against it would have succeeded over a row whose file
/// had changed underneath.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_moves_rev() {
    let h = harness("ordo_blobdoor_rev").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let (before, scan) = row_state(&h.pool, "doc_1").await;
    assert_eq!(scan, None);

    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"one".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (after, scan) = row_state(&h.pool, "doc_1").await;
    assert_eq!(after, before + 1, "one upload, one version");
    assert!(scan.is_some());

    // And again, because "it moved once" and "it moves per upload" are
    // different claims.
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"two".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (third, _) = row_state(&h.pool, "doc_1").await;
    assert_eq!(third, before + 2);
}

/// **`converged` and `written` fire, with no new wiring.**
///
/// `ordo_exec::triggers::classify` derives the event set from the command
/// kind, the actor and the correlation alone, so a `Converge` command in a
/// user-actored group with an ordinary correlation is exactly the input an
/// op's converge produces. Asserted through `classify` rather than through
/// a subscription, because what C4 changed is that there is a command to
/// classify at all: before it there was no group, and `classify` was never
/// reached for an upload.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_fires_converged_and_written() {
    let h = harness("ordo_blobdoor_triggers").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"bytes".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let group = h.log.lock().unwrap().last().expect("the group");
    let events =
        ordo_exec::triggers::classify(group.commands[0].kind, &group.actor, &group.correlation.0);
    assert!(
        events.contains(&ordo_syntax::trigger::EventKind::Converged),
        "an upload converges the row: {events:?}"
    );
    assert!(
        events.contains(&ordo_syntax::trigger::EventKind::Written),
        "`written` is on every command: {events:?}"
    );
    assert!(
        !events.contains(&ordo_syntax::trigger::EventKind::Reverted),
        "an upload is not a revert: {events:?}"
    );
}

/// **An upload is revertible, and the old handle comes back.**
///
/// The property the whole increment exists for. A revert restores the
/// before-image's column, so reverting the second upload puts the first
/// upload's handle back, and the first object is still readable through
/// the door because the column names it again.
///
/// The door appends each upload's group to the durable chain itself, in the
/// upload's own transaction (checklist 11-28), and the revert applier reads
/// it from there. This test used to copy the groups into the chain by hand,
/// because the host once built them in memory only; it now asserts the
/// chain already holds exactly the two uploads' groups before reverting.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_is_revertible_and_the_old_handle_comes_back() {
    let h = harness("ordo_blobdoor_revert").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"first".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let first_handle = scan_column(&h.pool, "doc_1").await.expect("set");
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"second".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let second_handle = scan_column(&h.pool, "doc_1").await.expect("set");
    assert_ne!(first_handle, second_handle, "two uploads, two handles");

    let (groups, correlation) = {
        let log = h.log.lock().unwrap();
        assert_eq!(log.len(), 2, "two uploads, two groups");
        let mut out = Vec::new();
        for i in 0..2 {
            out.push(
                log.store
                    .get(
                        &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into()),
                        i,
                    )
                    .expect("the group")
                    .group,
            );
        }
        let correlation = out[1].correlation.clone();
        (out, correlation)
    };
    assert_eq!(
        chain_correlations(&h.pool).await,
        correlations_of(&groups),
        "the door appended both uploads' groups to the durable chain itself"
    );

    let reverted = ordo_exec::undo::revert(
        &h.pool,
        &h.fixture.schema,
        Some(h.fixture.keys.as_ref()),
        Some(&h.pipeline),
        None,
        &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into()),
        &correlation,
        OWNER,
        ordo_commandlog::Timestamp(1),
    )
    .await
    .expect("an upload's group reverts like any other converge");
    assert_eq!(reverted.group.commands.len(), 1);

    assert_eq!(
        scan_column(&h.pool, "doc_1").await,
        Some(first_handle.clone()),
        "the revert restored the handle the row held before the second upload"
    );
    // And the object that handle names is still servable, which is what
    // makes the restored column a reference rather than a name for
    // nothing.
    let (status, got, err) = get_blob(&h.app, &first_handle, OWNER).await;
    assert_eq!(status, StatusCode::OK, "{err}");
    assert_eq!(got, b"first");
}

/// **A revert of a blob converge still succeeds, and its image carries the
/// digest that made C5's change to `imaged_fields` necessary.**
///
/// Before that change, `imaged_fields` refused any image key the schema
/// does not declare a field for, and `"{field}$digest"` is exactly such a
/// key: `$` cannot appear in a declared identifier, so the schema will
/// never declare `scan$digest`. Reverting the second upload here restores
/// the first upload's before-image, which (once C5 lands) carries
/// `scan` and `scan$digest` together, because the column was non-null when
/// that image was taken. This test asserts the digest key is actually
/// there (so a future change that quietly stopped emitting it would not
/// make this test pass for the wrong reason) and that the revert succeeds
/// anyway.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_of_a_blob_converge_still_succeeds() {
    let h = harness("ordo_blobdoor_revert_digest").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"first".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let first_handle = scan_column(&h.pool, "doc_1").await.expect("set");
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"second".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (groups, correlation) = {
        let log = h.log.lock().unwrap();
        assert_eq!(log.len(), 2, "two uploads, two groups");
        let mut out = Vec::new();
        for i in 0..2 {
            out.push(
                log.store
                    .get(
                        &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into()),
                        i,
                    )
                    .expect("the group")
                    .group,
            );
        }
        let correlation = out[1].correlation.clone();
        (out, correlation)
    };

    // The property this test exists to pin: the second upload's
    // before-image (the row as the first upload left it) names both the
    // handle and its digest, not just the handle.
    let before = groups[1].commands[0]
        .before
        .as_ref()
        .expect("a converge images before");
    assert_eq!(
        before.get("scan"),
        Some(&ordo_commandlog::Value::Text(first_handle.clone())),
        "the before-image names the handle the first upload set"
    );
    assert!(
        matches!(
            before.get("scan$digest"),
            Some(ordo_commandlog::Value::SealedRef(_))
        ),
        "the before-image must also carry the digest sibling key, or this test is not \
         exercising what it claims to: {before:?}"
    );

    assert_eq!(
        chain_correlations(&h.pool).await,
        correlations_of(&groups),
        "the door appended both uploads' groups to the durable chain itself"
    );

    let reverted = ordo_exec::undo::revert(
        &h.pool,
        &h.fixture.schema,
        Some(h.fixture.keys.as_ref()),
        Some(&h.pipeline),
        None,
        &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into()),
        &correlation,
        OWNER,
        ordo_commandlog::Timestamp(1),
    )
    .await
    .expect(
        "a revert whose restored image carries a digest sibling key must still succeed: \
         `imaged_fields` must skip it, the way it skips `id` and `rev`",
    );
    assert_eq!(reverted.group.commands.len(), 1);
    assert_eq!(
        scan_column(&h.pool, "doc_1").await,
        Some(first_handle),
        "the revert restored the handle despite the digest key riding beside it"
    );
}

/// **Pinning the reference-before-column order through its consequence.**
///
/// `run_platform_converge` inserts the `$blobref` row and then updates the
/// column, in one transaction, and C4 found no test could observe the two
/// statements' relative order: both run to completion before either image
/// is read, so nothing between them can tell the difference
/// (`docs/plan-blob-chain.md` §6 C4, finding 4). That is still true here:
/// this test does not reverse the two statements, because there is no way
/// to make that reversal observable without editing the production code
/// under test.
///
/// What C5 makes real is the dependency the ordering exists to satisfy:
/// **imaging a row with a non-null external blob column requires the
/// `$blobref` row to already be there.** A wrong order (or any bug that
/// drops the reference insert) leaves exactly the state this test builds
/// by hand, a column naming a handle with nothing behind it, and the
/// image builder refuses rather than silently imaging without a digest,
/// which is the failure mode that would let a reordering go unnoticed.
/// This is the ordering's consequence, pinned at the boundary the code
/// actually enforces.
#[tokio::test(flavor = "multi_thread")]
async fn a_column_naming_an_absent_blobref_row_refuses_the_image() {
    let h = harness("ordo_blobdoor_missing_ref").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"bytes".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let handle_hex = scan_column(&h.pool, "doc_1").await.expect("set");

    let (group, correlation) = {
        let log = h.log.lock().unwrap();
        assert_eq!(log.len(), 1, "one upload, one group");
        let group = log
            .store
            .get(
                &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into()),
                0,
            )
            .expect("the group")
            .group;
        let correlation = group.correlation.clone();
        (group, correlation)
    };
    assert_eq!(
        chain_correlations(&h.pool).await,
        correlations_of(std::slice::from_ref(&group)),
        "the door appended the upload's group to the durable chain itself"
    );

    // Simulate the failure a reversed (or dropped) reference insert would
    // leave: the column still names the handle, but `$blobref` no longer
    // does. The row is otherwise untouched: this is not a delete, it is
    // exactly "the reference is missing," isolated from everything else
    // that must also be true for a real upload to reach this state.
    h.pool
        .rows(&format!(
            "DELETE FROM \"$blobref\" WHERE \"$space\" = '{}' AND \"ref\" = decode('{handle_hex}', 'hex')",
            ordo_host::DEFAULT_SPACE
        ))
        .await
        .expect("delete the reference");

    // Reverting the upload must re-image the row as it now stands (to
    // check its revision under the row lock), and that image-build is
    // exactly what a wrong statement order would have broken.
    let err = ordo_exec::undo::revert(
        &h.pool,
        &h.fixture.schema,
        Some(h.fixture.keys.as_ref()),
        Some(&h.pipeline),
        None,
        &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into()),
        &correlation,
        OWNER,
        ordo_commandlog::Timestamp(1),
    )
    .await
    .expect_err("a column naming a handle with no `$blobref` row must refuse, not image silently");
    let msg = err.to_string();
    assert!(
        msg.contains("scan") && msg.contains("$blobref"),
        "the refusal should name the field and the missing table: {msg}"
    );
}

/// **A refused upload appends nothing and writes nothing.**
///
/// The guard that refuses is the write protocol's, evaluated under the row
/// lock, and the refusal aborts the transaction that would have carried
/// the reference, the column, the `rev` bump and the group. So the four
/// things an upload does are all-or-nothing together, which is the
/// property the old shape could not have: it had no group to lose.
///
/// The object is not rolled back, deliberately. §2.4: the bytes are stored
/// before the transaction opens, and a leak costs money while a dangling
/// reference costs a guarantee.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_refused_by_the_guard_appends_nothing_and_writes_nothing() {
    let h = harness("ordo_blobdoor_refused").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let (rev_before, _) = row_state(&h.pool, "doc_1").await;
    let refs_before = blobref_count(&h.pool).await;
    let files_before = count_files(&h.dir);

    let (status, body) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        MANAGER,
        b"an intruder's bytes".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    assert_eq!(h.log.lock().unwrap().len(), 0, "no group was appended");
    assert!(
        h.log.lock().unwrap().last().is_none(),
        "and none is held pending either"
    );
    let (rev_after, scan) = row_state(&h.pool, "doc_1").await;
    assert_eq!(rev_after, rev_before, "`rev` did not move");
    assert_eq!(scan, None, "the column was not set");
    assert_eq!(
        blobref_count(&h.pool).await,
        refs_before,
        "the reference row rolled back with the column"
    );
    assert_eq!(
        count_files(&h.dir),
        files_before + 1,
        "the object is stored before the transaction opens, so it survives \
         the refusal and the sweep collects it"
    );
}

// ── C6 · verification at revert ──────────────────────────────────────────
//
// `docs/plan-blob-chain.md` §4.3, and the owner's own definition:
// **verification is comparing the chain's recorded digest against the
// object that exists.** Every test below follows from that sentence, and
// the two that do *not* refuse follow from its consequence: absence is not
// a failed comparison, because there is nothing to compare.
//
// The fast-fail on length lives one layer down, where it belongs, and is
// pinned there against a store that is made to misreport its own length
// (`ordo-blob/tests/verify_streaming.rs`,
// `a_length_the_store_disagrees_with_refuses_before_any_transfer`). What
// this file adds is the revert: which outcome each condition produces,
// what is written, and what reaches the security stream.

/// Every group the door has logged, oldest first.
fn logged_groups(h: &Harness) -> Vec<ordo_commandlog::CommandGroup> {
    let log = h.log.lock().unwrap();
    (0..log.len())
        .map(|i| {
            log.store
                .get(
                    &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into()),
                    i,
                )
                .expect("the group")
                .group
        })
        .collect()
}

/// Replace this space's durable chain with `groups`.
///
/// For the tests that doctor a group before the revert reads it (an image
/// from before the digest existed, or times stamped to land a horizon). The
/// door has already appended the real groups, so appending doctored copies
/// beside them would be refused as duplicate correlations; the chain is
/// cleared and rebuilt from the doctored ones instead.
async fn replace_chain(h: &Harness, groups: &[ordo_commandlog::CommandGroup]) {
    h.pool
        .rows(&format!(
            "DELETE FROM \"$commandlog\" WHERE \"space\" = '{}'",
            ordo_host::DEFAULT_SPACE
        ))
        .await
        .expect("clear the chain");
    let tx = h.pool.begin().await.expect("tx");
    for g in groups {
        tx.append_group(g).await.expect("the chain takes the group");
    }
    tx.commit().await.expect("commit");
}

/// The correlations in this space's durable chain, in chain order.
async fn chain_correlations(pool: &ordo_exec::Pool) -> Vec<String> {
    pool.rows(&format!(
        "SELECT \"correlation\" FROM \"$commandlog\" WHERE \"space\" = '{}' ORDER BY \"idx\"",
        ordo_host::DEFAULT_SPACE
    ))
    .await
    .expect("read the chain")
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect()
}

fn correlations_of(groups: &[ordo_commandlog::CommandGroup]) -> Vec<String> {
    groups.iter().map(|g| g.correlation.0.clone()).collect()
}

/// Where an object lives under the `FsStore` root. Found by walking rather
/// than by rebuilding `object_key`, so a test that tampers with bytes does
/// not also encode a second opinion about the key scheme.
fn object_path(dir: &std::path::Path, handle_hex: &str) -> PathBuf {
    fn walk(dir: &std::path::Path, name: &str, found: &mut Option<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, name, found);
            } else if p.file_name().and_then(|s| s.to_str()) == Some(name) {
                *found = Some(p);
            }
        }
    }
    let mut found = None;
    walk(dir, handle_hex, &mut found);
    found.unwrap_or_else(|| panic!("no object named {handle_hex} under {}", dir.display()))
}

/// Rewrite the object `handle_hex` names as a valid sealed stream over
/// `plaintext`, which must be the same length as what it replaces: the
/// same header, the same nonce base, the same epoch, the same
/// `(entity, field)` AAD, so what comes back opens, is the length the
/// reference row says, and disagrees with the chain only in its content.
/// That is what makes a test built on this exercise the **digest
/// comparison** rather than an AEAD tag or a length — the only threat a
/// forgery this well-formed can still be caught by.
fn tamper(h: &Harness, handle_hex: &str, plaintext: &[u8]) {
    let path = object_path(&h.dir, handle_hex);
    let raw = std::fs::read(&path).expect("the stored object");
    let header = ordo_crypto::stream::StreamHeader::decode(&raw[..ordo_crypto::stream::HEADER])
        .expect("a stored object begins with its header");
    let space = CryptoSpace(ordo_host::DEFAULT_SPACE.into());
    let lease = h
        .fixture
        .keys
        .release(&space, ordo_crypto::custody::Purpose::Write, "tamper")
        .expect("a lease");
    let forged = h
        .fixture
        .keys
        .seal_chunk(
            &lease,
            "docs",
            "scan",
            ordo_crypto::Anchor::Object(handle_hex),
            header.epoch,
            &header.base,
            0,
            true,
            plaintext,
        )
        .expect("seal the forged chunk");
    let mut out = raw[..ordo_crypto::stream::HEADER].to_vec();
    out.extend_from_slice(&forged);
    assert_eq!(out.len(), raw.len(), "the forgery is the same size");
    std::fs::write(&path, &out).expect("tamper");
}

/// The revert `ordo revert` will run once C7 wires it: real custody, a
/// real pipeline, and a recorder to catch what it says.
#[allow(clippy::too_many_arguments)]
async fn revert_with(
    h: &Harness,
    pipeline: &BlobPipeline,
    events: &ordo_secevent::Recorder,
    correlation: &ordo_commandlog::CorrelationId,
) -> Result<ordo_exec::undo::Reverted, ordo_exec::undo::RevertFailure> {
    ordo_exec::undo::revert(
        &h.pool,
        &h.fixture.schema,
        Some(h.fixture.keys.as_ref()),
        Some(pipeline),
        Some(events),
        &ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into()),
        correlation,
        OWNER,
        ordo_commandlog::Timestamp(1),
    )
    .await
}

/// How many groups this space's chain holds.
async fn chain_len(pool: &ordo_exec::Pool) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"$commandlog\" WHERE \"space\" = '{}'",
        ordo_host::DEFAULT_SPACE
    ))
    .await
    .expect("count")[0][0]
        .clone()
        .expect("count(*) is never null")
        .parse()
        .expect("count(*) is an integer")
}

/// Every `blob_object_absent` the recorder is holding.
fn absences(events: &ordo_secevent::Recorder) -> Vec<ordo_secevent::StreamEntry> {
    events
        .drain_all(ordo_commandlog::Timestamp(2))
        .into_iter()
        .filter(|e| {
            matches!(
                &e.kind,
                ordo_secevent::EventKind::CustodyAction { action }
                    if action == ordo_exec::undo::BLOB_OBJECT_ABSENT
            )
        })
        .collect()
}

/// Two uploads into `docs.scan`, chained, with the second's correlation
/// and both handles: the shape every test below reverts.
async fn two_uploads(h: &Harness) -> (String, String, ordo_commandlog::CorrelationId) {
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"first".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let first = scan_column(&h.pool, "doc_1").await.expect("set");
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"second".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let second = scan_column(&h.pool, "doc_1").await.expect("set");

    let groups = logged_groups(h);
    assert_eq!(groups.len(), 2, "two uploads, two groups");
    let correlation = groups[1].correlation.clone();
    assert_eq!(
        chain_correlations(&h.pool).await,
        correlations_of(&groups),
        "the door appended both uploads' groups to the durable chain itself"
    );
    (first, second, correlation)
}

/// **A tampered object refuses the revert**, class `blob_integrity`, before
/// any write.
///
/// The tampering is done to the stored object directly, through the
/// `FsStore` path, and it is done **well**: the object is rewritten as a
/// valid sealed stream under its own header (the same nonce base, the
/// same epoch, the same `(entity, field)` AAD) carrying different
/// plaintext of the same length. So it opens, it is the length the
/// reference row says, and the only thing wrong with it is that it is not
/// the file the chain recorded.
///
/// That is deliberate on two counts. It makes the test about the **digest
/// comparison** rather than about an AEAD tag or a length, which is the
/// only way to know the comparison is actually running. And it is the
/// threat the chain exists for: `docs/plan-blob-chain.md` §7.3 answers the
/// owner's "could the object carry a digest of itself" with the
/// observation that against a key holder the false body and the false
/// header are written together, so the chain is the only independent
/// witness. This test is that attacker.
#[tokio::test(flavor = "multi_thread")]
async fn a_tampered_object_refuses_the_revert() {
    let h = harness("ordo_blobdoor_tampered").await;
    let (first, second, correlation) = two_uploads(&h).await;

    tamper(&h, &first, b"FIRST");

    let events = ordo_secevent::Recorder::default();
    let err = revert_with(&h, &h.pipeline, &events, &correlation)
        .await
        .expect_err("an object that is not what the chain recorded must refuse the revert");
    let msg = err.to_string();
    assert!(
        msg.starts_with("blob_integrity"),
        "the refusal must carry its own class: {msg}"
    );
    assert!(msg.contains("scan") && msg.contains(&first), "{msg}");
    // The forgery opened, so this refusal came from the **digest
    // comparison** and not from an AEAD tag. Without this the test would
    // pass identically against a spliced object, and would then be pinning
    // the wrong mechanism.
    assert!(
        msg.contains("does not match the digest this history recorded"),
        "the refusal must be the digest comparison's, not the tag's: {msg}"
    );

    // Refused **before any write**: the column still holds the second
    // upload's handle and no inverse group joined the chain.
    assert_eq!(
        scan_column(&h.pool, "doc_1").await,
        Some(second),
        "a refused revert restores nothing"
    );
    assert_eq!(
        chain_len(&h.pool).await,
        2,
        "no inverse group joined the chain either"
    );
    // A mismatch is the refusal. The security stream carries the cases
    // where nothing could be compared, and this is not one of them.
    assert!(
        absences(&events).is_empty(),
        "a mismatch refuses; it is not reported as an absence"
    );
}

/// **An object that is there and will not open refuses too**, and by the
/// same class.
///
/// This is the crude tamper, a flipped byte in the ciphertext, and it
/// fails one layer lower down, in the AEAD tag, never reaching the hash.
/// It is worth its own test because the naive mapping gets it wrong: a
/// custody error looks like "the key did not work", which looks like the
/// absent case, which restores. It must not. Custody has already opened
/// this reference row's own sealed length two statements earlier, so a
/// chunk that will not open is about the bytes and not about the keys, and
/// a tampered object must not restore quietly because the tampering was
/// caught by a tag instead of by a digest.
#[tokio::test(flavor = "multi_thread")]
async fn an_object_that_will_not_open_refuses_the_revert_as_integrity() {
    let h = harness("ordo_blobdoor_spliced").await;
    let (first, second, correlation) = two_uploads(&h).await;

    let path = object_path(&h.dir, &first);
    let mut raw = std::fs::read(&path).expect("the stored object");
    let at = ordo_crypto::stream::HEADER + 1;
    raw[at] ^= 0xff;
    std::fs::write(&path, &raw).expect("tamper");

    let events = ordo_secevent::Recorder::default();
    let err = revert_with(&h, &h.pipeline, &events, &correlation)
        .await
        .expect_err("a spliced object must refuse the revert");
    let msg = err.to_string();
    assert!(msg.starts_with("blob_integrity"), "{msg}");
    assert!(
        msg.contains("did not open"),
        "this one fails at the tag, which is the arm the test is about: {msg}"
    );
    assert_eq!(
        scan_column(&h.pool, "doc_1").await,
        Some(second),
        "a refused revert restores nothing"
    );
    assert!(absences(&events).is_empty());
}

/// **An absent object restores the handle and records
/// `blob_object_absent`.**
///
/// Nothing was compared, so nothing was disproved. Refusing here would
/// make the undo horizon depend on whether a bucket is answering, which is
/// the one thing §4.3 says it must never do. So the revert succeeds, the
/// handle comes back, and the fact that the platform could not check it is
/// evidence that goes to the security stream.
#[tokio::test(flavor = "multi_thread")]
async fn an_absent_object_restores_the_handle_and_records_blob_object_absent() {
    let h = harness("ordo_blobdoor_absent").await;
    let (first, _second, correlation) = two_uploads(&h).await;

    std::fs::remove_file(object_path(&h.dir, &first)).expect("remove the object");

    let events = ordo_secevent::Recorder::default();
    let reverted = revert_with(&h, &h.pipeline, &events, &correlation)
        .await
        .expect("an absent object must not hold the undo horizon hostage");
    assert_eq!(
        scan_column(&h.pool, "doc_1").await,
        Some(first.clone()),
        "the handle came back"
    );

    assert_eq!(reverted.unverified.len(), 1);
    let entry = &reverted.unverified[0];
    assert_eq!(entry.field, "scan");
    assert_eq!(entry.handle, first);
    assert!(
        matches!(
            entry.reason,
            ordo_exec::undo::Unverified::ObjectAbsent { .. }
        ),
        "{:?}",
        entry.reason
    );

    let events = absences(&events);
    assert_eq!(events.len(), 1, "one object, one event");
    assert_eq!(
        events[0].op,
        ordo_exec::undo::REVERT_OP,
        "the event names the act that could not verify"
    );
    assert_eq!(
        events[0].subject,
        ordo_secevent::Subject::member(OWNER),
        "and the principal who ran it"
    );
    assert_eq!(
        events[0].correlations,
        vec![reverted.group.correlation.clone()],
        "and the revert's own correlation, which is what joins it to the chain"
    );
}

/// **An unreachable store restores the handle and records it**, the same
/// way an absent object does and for the same reason: the platform did not
/// compare anything, so it disproved nothing.
///
/// The store here is a real `BlobStore` that reports a transport failure:
/// a bucket behind a broken network, from the pipeline's point of view.
/// Everything else is real, including the `$blobref` row, which is why
/// this reaches the store at all rather than stopping earlier.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_store_restores_the_handle_and_records_it() {
    let h = harness("ordo_blobdoor_unreachable").await;
    let (first, _second, correlation) = two_uploads(&h).await;

    let broken = BlobPipeline::new(
        Arc::new(ordo_blob::SingleStore::new(
            Arc::new(unreachable()) as Arc<dyn BlobStore>
        )),
        h.fixture.keys.clone(),
        BlobPolicy::DEFAULT,
    );
    let events = ordo_secevent::Recorder::default();
    let reverted = revert_with(&h, &broken, &events, &correlation)
        .await
        .expect("a store that is down must not hold the undo horizon hostage");

    assert_eq!(scan_column(&h.pool, "doc_1").await, Some(first.clone()));
    assert_eq!(reverted.unverified.len(), 1);
    match &reverted.unverified[0].reason {
        ordo_exec::undo::Unverified::ObjectAbsent { detail } => assert!(
            detail.contains("transport"),
            "the record carries the store's own words: {detail}"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(absences(&events).len(), 1);
}

/// **An image with no recorded digest restores, raises no security event,
/// and is recorded as a skip.**
///
/// §4.2: images carrying a blob column and no digest already exist in
/// every deployed chain, because a delete's before-image has always
/// carried every column. Absence of a digest in an old image is a fact
/// about the encoding's age and not evidence about an object, and the
/// owner drew the line at a security event himself, on the ground that it
/// would be noise.
///
/// The pre-C5 image is built by taking a real one and removing the key,
/// which is exactly the difference between the two encodings and nothing
/// else. The row, the reference and the object are all untouched and all
/// perfectly verifiable, so a revert that raised nothing *because it
/// could not reach the object* would pass this test for the wrong reason,
/// and the assertion on the skip's own reason is what stops that.
///
/// **What "records the skip" means today, exactly.** The transient record
/// is `Reverted::unverified`, asserted here field by field: which entity,
/// row, field and handle, and the reason told apart from
/// `ObjectAbsent` by name rather than merged. The durable record is
/// `CommandGroup::restored_unverified`, `plan-opslog.md` D7's trailer bit
/// on `reverted.group`, covered by its own test
/// (`a_restore_without_a_digest_flags_its_group`) rather than repeated
/// here. Neither is the ops log 11-22 once might have been: that routing
/// was decided against on 2026-09-10, so the test name says `records`
/// rather than `logs`, and means the trailer.
#[tokio::test(flavor = "multi_thread")]
async fn an_image_without_a_digest_restores_without_an_event_and_records_the_skip() {
    let h = harness("ordo_blobdoor_nodigest").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"first".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let first = scan_column(&h.pool, "doc_1").await.expect("set");
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"second".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let mut groups = logged_groups(&h);
    let correlation = groups[1].correlation.clone();
    // Age the second upload's before-image back to the pre-C5 encoding:
    // the handle, and no digest beside it.
    let before = groups[1].commands[0]
        .before
        .as_mut()
        .expect("a converge images before");
    assert!(
        before.remove("scan$digest").is_some(),
        "the image has to have carried a digest for removing it to mean anything"
    );
    replace_chain(&h, &groups).await;

    let events = ordo_secevent::Recorder::default();
    let reverted = revert_with(&h, &h.pipeline, &events, &correlation)
        .await
        .expect("an image that predates the digest still reverts");
    assert_eq!(scan_column(&h.pool, "doc_1").await, Some(first.clone()));

    assert_eq!(reverted.unverified.len(), 1);
    let skip = &reverted.unverified[0];
    assert_eq!(
        skip.reason,
        ordo_exec::undo::Unverified::NoRecordedDigest,
        "the skip must be recorded as what it is, not as an object it could not reach"
    );
    assert_eq!(skip.correlation, reverted.group.correlation);
    assert_eq!(skip.entity, "docs");
    assert_eq!(skip.field, "scan");
    assert_eq!(skip.row_id, "doc_1");
    assert_eq!(skip.handle, first);
    assert!(
        absences(&events).is_empty(),
        "a missing digest is a fact about the encoding's age, and the security stream is for \
         what means something is wrong"
    );
}

/// **The no-digest restore also flags the revert's own group**,
/// `plan-opslog.md` D7's durable half of what the previous test's
/// `reverted.unverified` records only transiently. Same setup, same aged
/// before-image; the assertion is on `reverted.group.restored_unverified`,
/// because that bit, not `reverted.unverified`, is what a database restart
/// or a second reader still finds.
#[tokio::test(flavor = "multi_thread")]
async fn a_restore_without_a_digest_flags_its_group() {
    let h = harness("ordo_blobdoor_nodigest_flag").await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"first".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"second".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let mut groups = logged_groups(&h);
    let correlation = groups[1].correlation.clone();
    let before = groups[1].commands[0]
        .before
        .as_mut()
        .expect("a converge images before");
    assert!(
        before.remove("scan$digest").is_some(),
        "the image has to have carried a digest for removing it to mean anything"
    );
    replace_chain(&h, &groups).await;

    let events = ordo_secevent::Recorder::default();
    let reverted = revert_with(&h, &h.pipeline, &events, &correlation)
        .await
        .expect("an image that predates the digest still reverts");

    assert!(
        reverted.group.restored_unverified,
        "a restore with no recorded digest to verify against must flag the \
         group it was recorded in"
    );
    assert!(
        !reverted.group.owner_administration,
        "the two trailer bits are independent; flagging one must not set the other"
    );
}

/// **A fully verified restore leaves its group unflagged.** The mirror of
/// the previous test: same shape, no aging, so the digest the chain
/// recorded matches the object that exists and `verify_restored_objects`
/// has something to compare. `reverted.unverified` is empty, and so is the
/// trailer bit that exists for the case where it is not.
#[tokio::test(flavor = "multi_thread")]
async fn a_verified_restore_leaves_the_group_unflagged() {
    let h = harness("ordo_blobdoor_verified_unflagged").await;
    let (_first, _second, correlation) = two_uploads(&h).await;

    let events = ordo_secevent::Recorder::default();
    let reverted = revert_with(&h, &h.pipeline, &events, &correlation)
        .await
        .expect("a fully verifiable revert succeeds");

    assert!(
        reverted.unverified.is_empty(),
        "both handles carry a digest and a reachable object, so nothing is unverified"
    );
    assert!(
        !reverted.group.restored_unverified,
        "a revert that verified everything it restored must not flag its group"
    );
}

/// **A revert past the undo horizon still refuses for the horizon's
/// reason**, unchanged by anything C6 added.
///
/// The horizon is enforced where it always was (the prune's floor, and
/// the sweep's cutoff in `blobgc.rs`), and a group below it is not in the
/// chain at all, so the revert never reaches an image, a digest, or an
/// object. Pinned because the failure mode is silent: a verification step
/// that ran before the group lookup, or a blob refusal that swallowed the
/// horizon's, would turn "this is older than the platform promises to
/// undo" into "something is wrong with your bytes", which is an operator
/// sent to look at a bucket for a policy answer.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_past_the_horizon_still_refuses_for_the_horizon_s_reason() {
    let h = harness("ordo_blobdoor_horizon").await;
    ordo_exec::prune::ensure_tables(&h.pool)
        .await
        .expect("$logconsumers");
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    for body in [b"first".to_vec(), b"second".to_vec(), b"third".to_vec()] {
        let (status, err) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{err}");
    }

    // Three uploads, stamped an interval apart so a cutoff can fall
    // between them. The door stamps every group from one clock, and the
    // horizon is computed from command timestamps, so the spread is set
    // here rather than by sleeping.
    let mut groups = logged_groups(&h);
    assert_eq!(groups.len(), 3);
    for (i, g) in groups.iter_mut().enumerate() {
        for c in &mut g.commands {
            c.at = ordo_commandlog::Timestamp(1_000 * (i as u64 + 1));
        }
    }
    let middle = groups[1].correlation.clone();
    replace_chain(&h, &groups).await;

    // A signed anchor at position 2, and a horizon that also lands at 2:
    // everything below it may go.
    let space = ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into());
    let signer = ordo_commandlog::Ed25519Signer::from_seed("chain-key-2026", &[9u8; 32]);
    let head = head_at(&h.pool, 1).await;
    let cp = ordo_commandlog::Checkpoint {
        space: space.clone(),
        upto: 2,
        head,
        at: ordo_commandlog::Timestamp(2_500),
        key_id: ordo_commandlog::Signer::key_id(&signer).to_string(),
        signature: ordo_commandlog::Signer::sign(
            &signer,
            &ordo_commandlog::Checkpoint::signed_bytes(
                &space,
                2,
                head,
                ordo_commandlog::Timestamp(2_500),
            ),
        ),
    };
    let tx = h.pool.begin().await.expect("tx");
    tx.put_checkpoint(&cp).await.expect("checkpoint");
    tx.commit().await.expect("commit");

    let policy = ordo_exec::cadence::LogPolicy {
        undo_horizon: std::time::Duration::from_secs(100),
        ..ordo_exec::cadence::LogPolicy::DEFAULT
    };
    let pruned = ordo_exec::prune::prune(&h.pool, &space, &signer.verifier(), &policy, 2_600)
        .await
        .expect("a lawful prune");
    match pruned {
        ordo_exec::prune::Pruned::Removed { floor, entries, .. } => {
            assert_eq!(entries, 2, "the two groups below the anchor are gone");
            assert!(
                matches!(floor.holder, ordo_exec::prune::FloorHolder::UndoHorizon),
                "this test is only about the horizon if the horizon is what held the floor: \
                 {:?}",
                floor.holder
            );
        }
        other => panic!("expected a prune: {other:?}"),
    }

    // The middle group restored a real handle, so a revert of it would
    // have had an object to verify, if there were still a group to
    // revert.
    let events = ordo_secevent::Recorder::default();
    let err = revert_with(&h, &h.pipeline, &events, &middle)
        .await
        .expect_err("a group below the horizon is not there to revert");
    let msg = err.to_string();
    assert!(
        msg.contains("no logged group"),
        "the refusal is the horizon's own, unchanged: {msg}"
    );
    assert!(
        !msg.contains("blob_integrity"),
        "the horizon's answer must not be dressed up as an integrity failure: {msg}"
    );
    assert!(absences(&events).is_empty());
}

/// The chain hash at `idx`, for a hand-written checkpoint.
async fn head_at(pool: &ordo_exec::Pool, idx: u64) -> ordo_commandlog::ChainHash {
    let rows = pool
        .rows(&format!(
            "SELECT encode(\"hash\",'hex') FROM \"$commandlog\" \
             WHERE \"space\" = '{}' AND \"idx\" = {idx}",
            ordo_host::DEFAULT_SPACE
        ))
        .await
        .expect("head");
    let hex = rows[0][0].clone().expect("a chain row has a hash");
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex");
    }
    ordo_commandlog::ChainHash(out)
}

/// A store that answers nothing.
///
/// A real `BlobStore` whose answer is a transport failure, which is what a
/// bucket behind a broken network looks like from inside the pipeline. The
/// pipeline, the sealer, the reference row and the revert are all the real
/// ones, so only the store's availability is under test.
/// A store whose every method answers one error: [`unreachable`] the
/// transient one (a transport failure), `Failing(Refused)` a permanent one.
struct Failing(ordo_blob::BlobError);

/// A bucket behind a broken network.
fn unreachable() -> Failing {
    Failing(ordo_blob::BlobError::Transport(DOWN.into()))
}

impl Failing {
    fn err(&self) -> ordo_blob::BlobError {
        self.0.clone()
    }
}

const DOWN: &str = "the store is not answering";

#[async_trait::async_trait]
impl BlobStore for Failing {
    fn binding(&self) -> &'static str {
        "unreachable"
    }

    fn production_ready(&self) -> bool {
        false
    }

    async fn put(
        &self,
        _space: &CryptoSpace,
        _handle: &ordo_crypto::custody::BlobHandle,
        _len: u64,
        _body: ordo_blob::ByteStream,
    ) -> Result<ordo_blob::ObjectHead, ordo_blob::BlobError> {
        Err(self.err())
    }

    async fn get(
        &self,
        _space: &CryptoSpace,
        _handle: &ordo_crypto::custody::BlobHandle,
        _range: Option<ordo_blob::ByteRange>,
    ) -> Result<ordo_blob::ByteStream, ordo_blob::BlobError> {
        Err(self.err())
    }

    async fn head(
        &self,
        _space: &CryptoSpace,
        _handle: &ordo_crypto::custody::BlobHandle,
    ) -> Result<ordo_blob::ObjectHead, ordo_blob::BlobError> {
        Err(self.err())
    }

    async fn delete(
        &self,
        _space: &CryptoSpace,
        _handle: &ordo_crypto::custody::BlobHandle,
    ) -> Result<(), ordo_blob::BlobError> {
        Err(self.err())
    }
}

// ── C7 · `ordo verify --objects` ─────────────────────────────────────────
//
// `ordo_exec::undo::verify_objects` is the engine `ordo verify --objects`
// runs (`docs/plan-blob-chain.md` §4.5): the offline form of §4.3, walking
// every image the chain has ever stored instead of the one image a revert
// is about to write back. It calls C6's own [`ordo_exec::undo::
// verify_restored_objects`] unchanged, so what these two tests are really
// pinning is the walk — that it reaches every digest-bearing image in
// chain order, and that a disagreement stops it at the first one rather
// than the loudest or the last.

/// **A clean chain's objects all verify**, and the walk reports exactly
/// what it looked at: both uploads' after-images carry a digest (the
/// first upload's before-image does not, since the row had no object
/// yet), so three images carry one and none of them is unverified.
#[tokio::test(flavor = "multi_thread")]
async fn verify_objects_reports_a_clean_chain_with_nothing_unverified() {
    let h = harness("ordo_blobdoor_verify_objects_clean").await;
    two_uploads(&h).await;

    let space = ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into());
    let db = ordo_exec::undo::Db::Pool(&h.pool);
    let verified = ordo_exec::undo::verify_objects(
        db,
        &h.fixture.schema,
        &h.pipeline,
        h.fixture.keys.as_ref(),
        &space,
    )
    .await
    .expect("both uploads' objects match what the chain recorded");

    assert!(
        verified.unverified.is_empty(),
        "every digest-bearing image reached a real object: {:?}",
        verified.unverified
    );
    assert_eq!(
        verified.images, 3,
        "two converges, four images total, one of them (the first \
         upload's before-image) names no blob column at all — three carry \
         `docs.scan`"
    );
}

/// **`verify_objects` names the first mismatching handle**, in chain
/// order, and stops there rather than reporting every disagreement the
/// walk would otherwise find.
///
/// Both uploaded objects are tampered the same well-formed way
/// [`a_tampered_object_refuses_the_revert`] uses, so either alone would
/// refuse — the point of tampering both is to prove which one the walk
/// reports. The first upload's after-image is the first image in the
/// chain that carries a digest, so that is the handle the walk must name.
/// The second upload's before-image also names the first handle (a
/// converge's before-image is the row as the previous write left it) and
/// would refuse there too, and the second upload's own after-image is
/// tampered as well — neither is reached, because the walk stops at the
/// first `Err` it produces, and that is this one.
#[tokio::test(flavor = "multi_thread")]
async fn verify_objects_names_the_first_mismatching_handle() {
    let h = harness("ordo_blobdoor_verify_objects_first").await;
    let (first, second, _correlation) = two_uploads(&h).await;

    tamper(&h, &first, b"FIRST");
    tamper(&h, &second, b"SECOND");

    let space = ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into());
    let db = ordo_exec::undo::Db::Pool(&h.pool);
    let err = ordo_exec::undo::verify_objects(
        db,
        &h.fixture.schema,
        &h.pipeline,
        h.fixture.keys.as_ref(),
        &space,
    )
    .await
    .expect_err("two tampered objects must still refuse the walk");
    let msg = err.to_string();
    assert!(
        msg.starts_with("blob_integrity"),
        "the refusal must carry its own class: {msg}"
    );
    assert!(
        msg.contains(&first),
        "the walk must name the FIRST mismatching handle, the one the \
         chain recorded first: {msg}"
    );
    assert!(
        !msg.contains(&second),
        "and not the second upload's, even though its object is tampered \
         too and the walk would have reached it eventually: {msg}"
    );
}

// ── where an upload lands during a detach or a replace (25ac) ────────────
//
// Ruled 2026-10-05 (`detach-upload-race` ask 2): nothing is checked or
// refused at upload. Each file's `$blobref` row records the bucket it was
// written to, and reads and the sweep follow that name. These tests drive
// the real door through `AttachedStores`, with a node whose cached route is
// stale on purpose: that staleness, up to `AttachedStores::TTL` on every
// node, is the window the finding named.

/// An in-memory bucket gateway on loopback. It stores what is `PUT`,
/// answers `GET` (ranges included), `HEAD` and `DELETE`, and remembers
/// every `DELETE` path, so a test can say which bucket an object is in and
/// which bucket the sweep deleted it from.
///
/// The attachment still travels the production path to reach it: a
/// `$spacebucket` row, a credential label, `S3Config::new`, `S3Store`,
/// SigV4 and a socket (`ordo-exec/tests/bucket_routing.rs` argues why this
/// is the right double). Path style, so the bucket is the first segment of
/// every request path.
struct Gateway {
    endpoint: String,
    objects: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    deleted: Arc<Mutex<Vec<String>>>,
}

impl Gateway {
    fn start() -> Gateway {
        use std::net::ToSocketAddrs;
        let addr = "localhost:0"
            .to_socket_addrs()
            .expect("localhost resolves")
            .next()
            .expect("localhost resolves to something");
        let listener = std::net::TcpListener::bind(addr).expect("a loopback port");
        let endpoint = format!("http://localhost:{}", listener.local_addr().unwrap().port());
        let objects = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let deleted = Arc::new(Mutex::new(Vec::new()));
        let (o, d) = (Arc::clone(&objects), Arc::clone(&deleted));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (o, d) = (Arc::clone(&o), Arc::clone(&d));
                std::thread::spawn(move || Gateway::serve(stream, &o, &d));
            }
        });
        Gateway {
            endpoint,
            objects,
            deleted,
        }
    }

    fn serve(
        stream: std::net::TcpStream,
        objects: &Mutex<std::collections::HashMap<String, Vec<u8>>>,
        deleted: &Mutex<Vec<String>>,
    ) {
        use std::io::{BufRead, Read, Write};
        let mut reader = std::io::BufReader::new(stream);
        let mut request = String::new();
        if reader.read_line(&mut request).is_err() {
            return;
        }
        let mut parts = request.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let (mut len, mut range) = (0usize, None::<String>);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if line == "\r\n" => break,
                Ok(_) => {
                    let Some((k, v)) = line.split_once(':') else {
                        continue;
                    };
                    match k.trim().to_ascii_lowercase().as_str() {
                        "content-length" => len = v.trim().parse().unwrap_or(0),
                        "range" => range = Some(v.trim().to_string()),
                        _ => {}
                    }
                }
            }
        }
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let answer = |status: &str, bytes: &[u8], head_only: bool| {
            let mut out = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                bytes.len()
            )
            .into_bytes();
            if !head_only {
                out.extend_from_slice(bytes);
            }
            out
        };
        let reply = match method.as_str() {
            "PUT" => {
                objects.lock().unwrap().insert(path, body);
                answer("200 OK", &[], false)
            }
            "DELETE" => {
                objects.lock().unwrap().remove(&path);
                deleted.lock().unwrap().push(path);
                answer("204 No Content", &[], true)
            }
            "GET" | "HEAD" => match objects.lock().unwrap().get(&path).cloned() {
                None => answer("404 Not Found", &[], false),
                Some(all) => match range.as_deref().and_then(|r| r.strip_prefix("bytes=")) {
                    None => answer("200 OK", &all, method == "HEAD"),
                    Some(r) => {
                        let (a, b) = r.split_once('-').unwrap_or((r, ""));
                        let a: usize = a.parse().unwrap_or(0);
                        let b: usize = b.parse().unwrap_or(all.len().saturating_sub(1));
                        if a >= all.len() {
                            answer("416 Range Not Satisfiable", &[], false)
                        } else {
                            let b = b.min(all.len() - 1);
                            answer("206 Partial Content", &all[a..=b], false)
                        }
                    }
                },
            },
            _ => answer("400 Bad Request", &[], false),
        };
        let mut stream = reader.into_inner();
        let _ = stream.write_all(&reply);
        let _ = stream.flush();
    }

    /// Whether `bucket` holds the object named by `handle_hex`.
    fn holds(&self, bucket: &str, handle_hex: &str) -> bool {
        let prefix = format!("/{bucket}/");
        self.objects
            .lock()
            .unwrap()
            .keys()
            .any(|k| k.starts_with(&prefix) && k.ends_with(handle_hex))
    }

    /// Whether a `DELETE` for that object reached `bucket`.
    fn was_deleted_from(&self, bucket: &str, handle_hex: &str) -> bool {
        let prefix = format!("/{bucket}/");
        self.deleted
            .lock()
            .unwrap()
            .iter()
            .any(|k| k.starts_with(&prefix) && k.ends_with(handle_hex))
    }
}

/// A credential directory holding a key pair under `label`, in the layout
/// `creds::Dir` requires (`ordo-exec/tests/bucket_routing.rs` builds the
/// same). The values are test strings, not keys.
fn bucket_credentials(
    dir: &std::path::Path,
    label: &str,
) -> Arc<dyn ordo_exec::creds::BucketCredentials> {
    use std::os::unix::fs::PermissionsExt;
    let root = dir.join("creds");
    let d = root.join(label);
    std::fs::create_dir_all(&d).unwrap();
    for (name, value) in [
        ("access_key_id", "AKIATEST".to_string()),
        ("secret_access_key", format!("test-value-for-{label}")),
    ] {
        let f = d.join(name);
        std::fs::write(&f, value).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    Arc::new(ordo_exec::creds::Dir::open(&root).expect("the credential directory reads"))
}

fn bucket_named(gw: &Gateway, name: &str) -> ordo_exec::bucket::Attachment {
    ordo_exec::bucket::Attachment {
        endpoint: gw.endpoint.clone(),
        region: "eu-west-1".into(),
        bucket: name.into(),
        credential: "tenant-a".into(),
        custody: ordo_exec::bucket::BucketCustody::PlatformSealed,
        path_style: Some(true),
        key_prefix: None,
        attached_at: 1,
        attached_by: OWNER.into(),
    }
}

/// The door over `AttachedStores`, with a route cache that does not expire
/// by itself, so the test decides when this node notices a change. The
/// deployment's own store is the harness's directory.
async fn attached_harness(db: &str) -> (Harness, Arc<ordo_exec::bucket::AttachedStores>, Gateway) {
    let dir = std::env::temp_dir().join(format!("ordo-blob-door-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let default: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    // The resolver needs its pool before the harness resets the database,
    // so the database has to exist first. It already may; that is fine.
    let _ = Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
    .exec(&format!("CREATE DATABASE {db}"));
    let control = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("control pool");
    let stores = Arc::new(
        ordo_exec::bucket::AttachedStores::new(
            control,
            default,
            bucket_credentials(&dir, "tenant-a"),
        )
        .with_ttl(std::time::Duration::from_secs(3_600)),
    );
    let h = harness_with_resolver(db, dir, stores.clone()).await;
    ordo_exec::bucket::ensure_table(&h.pool)
        .await
        .expect("$spacebucket");
    (h, stores, Gateway::start())
}

/// Whether the deployment's own store (the harness directory) holds an
/// object of that name. [`object_path`] panics on absence, which is the
/// answer some tests here want.
fn in_deployment_store(h: &Harness, handle_hex: &str) -> bool {
    fn walk(dir: &std::path::Path, name: &str) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|e| {
            let p = e.path();
            if p.is_dir() {
                walk(&p, name)
            } else {
                p.file_name().and_then(|s| s.to_str()) == Some(name)
            }
        })
    }
    walk(&h.dir, handle_hex)
}

fn door_space() -> CryptoSpace {
    CryptoSpace(ordo_host::DEFAULT_SPACE.into())
}

/// The store name a table records for one handle.
async fn recorded_store(pool: &ordo_exec::Pool, table: &str, handle_hex: &str) -> String {
    let rows = pool
        .rows(&format!(
            "SELECT \"store\" FROM \"{table}\" \
             WHERE \"$space\" = '{}' AND \"ref\" = decode('{handle_hex}', 'hex')",
            ordo_host::DEFAULT_SPACE
        ))
        .await
        .expect("select store");
    rows.first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_else(|| panic!("no `{table}` row for {handle_hex}"))
}

/// The bucket a recorded store name names, or `None` for the deployment's
/// own store.
fn bucket_of(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    Some(
        ordo_exec::bucket::Attachment::from_store_name(name)
            .expect("a recorded name parses")
            .bucket,
    )
}

/// This node routes the space to `tenant-old`; then another node detaches
/// it. Nothing was in the bucket at the detach, so the op's refusal did not
/// apply. This node has not noticed.
async fn detached_behind_this_nodes_back(
    h: &Harness,
    stores: &ordo_exec::bucket::AttachedStores,
    gw: &Gateway,
) {
    ordo_exec::bucket::attach(&h.pool, &door_space(), &bucket_named(gw, "tenant-old"))
        .await
        .expect("attached");
    ordo_blob::StoreResolver::resolve(stores, &door_space())
        .await
        .expect("this node caches the route");
    ordo_exec::bucket::detach(&h.pool, &door_space())
        .await
        .expect("detached")
        .expect("there was an attachment to detach");
}

/// **An upload that lands during a detach stays readable** (25ac).
///
/// The upload reaches the bucket being detached, because this node's route
/// is stale. Its row records that bucket. Once the node notices the detach,
/// the space's uploads go to the deployment's store, and the file is still
/// served, from where it landed. A regression reads it from the
/// deployment's store and answers `internal` or `not_found`.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_that_lands_during_a_detach_stays_readable() {
    let (h, stores, gw) = attached_harness("ordo_blobdoor_detach_read").await;
    detached_behind_this_nodes_back(&h, &stores, &gw).await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let bytes: Vec<u8> = (0u8..=255).cycle().take(70_000).collect();
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, bytes.clone()).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "nothing is refused at upload: {body}"
    );
    let handle = scan_column(&h.pool, "doc_1").await.expect("the column");

    assert!(
        gw.holds("tenant-old", &handle),
        "the bytes went to the old bucket"
    );
    assert!(
        !in_deployment_store(&h, &handle),
        "and not in the deployment's store"
    );
    assert_eq!(
        bucket_of(&recorded_store(&h.pool, "$blobref", &handle).await).as_deref(),
        Some("tenant-old"),
        "the row names the bucket the bytes reached"
    );

    // The node notices the detach: the space now writes to the deployment.
    stores.forget(&door_space());
    let (status, got, err) = get_blob(&h.app, &handle, OWNER).await;
    assert_eq!(status, StatusCode::OK, "{err}");
    assert_eq!(got, bytes, "served from the bucket it landed in");
}

/// **The sweep collects it from the bucket it landed in** (25ac).
///
/// The file written during the detach is replaced by a later upload, which
/// goes to the deployment's store. Past the floor the first object is
/// unreferenced and due, and the sweep deletes it from `tenant-old`, the
/// bucket its row names. A regression deletes it against the deployment's
/// store, succeeds because an absent key deletes cleanly, retires the
/// reference, and leaves the bytes in the old bucket for good.
#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_collects_an_upload_from_the_bucket_it_landed_in() {
    let (h, stores, gw) = attached_harness("ordo_blobdoor_detach_sweep").await;
    detached_behind_this_nodes_back(&h, &stores, &gw).await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let at = now_ms();
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"first".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let first = scan_column(&h.pool, "doc_1").await.expect("the column");
    assert!(gw.holds("tenant-old", &first));

    stores.forget(&door_space());
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"second".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let second = scan_column(&h.pool, "doc_1").await.expect("the column");
    assert!(
        object_path(&h.dir, &second).is_file(),
        "the deployment's store now"
    );
    assert_eq!(recorded_store(&h.pool, "$blobref", &second).await, "");

    // Inside the horizon: marked, and the tombstone carries the bucket.
    let r = sweep(&h, at).await;
    assert_eq!(r.marked, 1, "the first object is unreferenced");
    assert_eq!(r.deleted, 0);
    assert_eq!(
        bucket_of(&recorded_store(&h.pool, "$blobtomb", &first).await).as_deref(),
        Some("tenant-old")
    );

    let later = at + 91 * DAY_MS;
    chain_stamped_at(&h, later / 1_000).await;
    let r = sweep(&h, later).await;
    assert_eq!(r.deleted, 1, "{r:?}");
    assert!(
        gw.was_deleted_from("tenant-old", &first),
        "deleted where it landed"
    );
    assert!(!gw.holds("tenant-old", &first), "and it is gone");
    assert!(
        object_path(&h.dir, &second).is_file(),
        "the live object stays"
    );
}

/// **An interrupted upload during a detach is collected from the bucket it
/// landed in** (25ac). The guard refuses the upload after its bytes are
/// stored, so there is no `$blobref` row, only the declared intent; the
/// intent names the bucket, and the sweep deletes from there.
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_upload_during_a_detach_is_collected_from_the_bucket_it_landed_in() {
    let (h, stores, gw) = attached_harness("ordo_blobdoor_detach_intent").await;
    detached_behind_this_nodes_back(&h, &stores, &gw).await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;

    let at = now_ms();
    let (status, _) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        MANAGER,
        b"refused".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let handle = open_intents(&h.pool)
        .await
        .into_iter()
        .next()
        .expect("the interrupted upload left an intent");
    assert!(gw.holds("tenant-old", &handle));
    assert_eq!(
        bucket_of(&recorded_store(&h.pool, "$blobtomb", &handle).await).as_deref(),
        Some("tenant-old"),
        "the intent names the bucket the bytes are about to reach"
    );

    stores.forget(&door_space());
    let r = sweep(&h, at + 91 * DAY_MS).await;
    assert_eq!(r.deleted, 1, "{r:?}");
    assert!(gw.was_deleted_from("tenant-old", &handle));
    assert!(!gw.holds("tenant-old", &handle));
}

/// **A replace moves the tenant's uploads, and the file written during it
/// stays where it landed** (25ac).
///
/// This node routes to `tenant-old` when another node replaces it with
/// `tenant-new`. The next upload here still lands in `tenant-old` and its
/// row says so. Once the node notices, uploads move to `tenant-new` and
/// record it. Both files are served, each from its own bucket. No object
/// is copied between buckets: the D-S2 refusal still governs moving a
/// tenant's existing files.
#[tokio::test(flavor = "multi_thread")]
async fn a_replace_moves_new_uploads_and_the_file_written_during_it_stays_where_it_landed() {
    let (h, stores, gw) = attached_harness("ordo_blobdoor_replace").await;
    ordo_exec::bucket::attach(&h.pool, &door_space(), &bucket_named(&gw, "tenant-old"))
        .await
        .expect("attached");
    ordo_blob::StoreResolver::resolve(stores.as_ref(), &door_space())
        .await
        .expect("this node caches the route");
    let outcome =
        ordo_exec::bucket::attach(&h.pool, &door_space(), &bucket_named(&gw, "tenant-new"))
            .await
            .expect("replaced");
    assert_eq!(outcome, ordo_exec::bucket::AttachOutcome::Replaced);

    insert_doc(&h.pool, "doc_1", OWNER, "during").await;
    insert_doc(&h.pool, "doc_2", OWNER, "after").await;
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"during".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let during = scan_column(&h.pool, "doc_1").await.expect("the column");

    stores.forget(&door_space());
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_2", OWNER, b"after".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let after = scan_column(&h.pool, "doc_2").await.expect("the column");

    assert!(gw.holds("tenant-old", &during) && !gw.holds("tenant-new", &during));
    assert!(gw.holds("tenant-new", &after) && !gw.holds("tenant-old", &after));
    assert_eq!(
        bucket_of(&recorded_store(&h.pool, "$blobref", &during).await).as_deref(),
        Some("tenant-old")
    );
    assert_eq!(
        bucket_of(&recorded_store(&h.pool, "$blobref", &after).await).as_deref(),
        Some("tenant-new")
    );

    for (handle, want) in [(&during, b"during".to_vec()), (&after, b"after".to_vec())] {
        let (status, got, err) = get_blob(&h.app, handle, OWNER).await;
        assert_eq!(status, StatusCode::OK, "{err}");
        assert_eq!(got, want);
    }
}

// ── changing a bucket that holds files (ruled 2026-10-06) ───────────────
//
// `st-attach-existing` ask 2 lifted the refusal on changing or detaching a
// tenant's bucket while it holds files. The tests below drive the real op
// (`BucketOps`) rather than writing `$spacebucket` directly, so a refusal
// creeping back in fails them at the op.

/// A probe that always passes: the gateway double answers `head`, but the
/// refusal under test is not the probe's.
struct Passing;

#[async_trait::async_trait]
impl ordo_host::bucket_ops::BucketProbe for Passing {
    async fn probe(
        &self,
        _cfg: &ordo_blob::S3Config,
        _space: &CryptoSpace,
    ) -> Result<(), ordo_blob::BlobError> {
        Ok(())
    }
}

/// The attach and detach op over the harness's database, allowed to dial
/// the loopback gateway.
fn bucket_ops(h: &Harness) -> ordo_host::bucket_ops::BucketOps {
    ordo_host::bucket_ops::BucketOps {
        pool: h.pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(h.pool.clone())),
        stream: None,
        credentials: bucket_credentials(&h.dir.join("ops"), "tenant-a"),
        hosts: ordo_host::provision::HostAllowlist::of("localhost"),
        own_store: None,
        allow_plaintext: true,
        probe: Arc::new(Passing),
        now: Arc::new(|| NOW),
    }
}

/// Attach `bucket` at the gateway through the op, as the owner.
async fn attach_by_op(ops: &ordo_host::bucket_ops::BucketOps, gw: &Gateway, bucket: &str) -> Value {
    let mut p = serde_json::Map::new();
    p.insert("endpoint".into(), Value::from(gw.endpoint.clone()));
    p.insert("region".into(), Value::from("eu-west-1"));
    p.insert("bucket".into(), Value::from(bucket));
    p.insert("credential".into(), Value::from("tenant-a"));
    p.insert("platformSealed".into(), Value::from(true));
    p.insert("pathStyle".into(), Value::from(true));
    let r = ops
        .attach(ordo_host::DEFAULT_SPACE, &ordo_api::BoundParams(p), OWNER)
        .await
        .unwrap_or_else(|e| panic!("the attach of `{bucket}` was refused: {e:?}"));
    r.rows[0].clone()
}

/// **A change while the tenant holds files is allowed, and the old file is
/// still read** (`st-attach-existing` ask 2). The file uploaded to
/// `tenant-old` stays there; new uploads go to `tenant-new`; both are
/// served. A regression is the op answering `bucket_refused`, or the old
/// file answering `internal` or `not_found` because it was looked up in the
/// new bucket.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_with_files_held_is_allowed_and_old_files_still_read() {
    let (h, stores, gw) = attached_harness("ordo_blobdoor_change_held").await;
    let ops = bucket_ops(&h);
    attach_by_op(&ops, &gw, "tenant-old").await;
    stores.forget(&door_space());
    insert_doc(&h.pool, "doc_1", OWNER, "old").await;
    insert_doc(&h.pool, "doc_2", OWNER, "new").await;
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"old".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let old = scan_column(&h.pool, "doc_1").await.expect("the column");
    assert!(gw.holds("tenant-old", &old));

    let answer = attach_by_op(&ops, &gw, "tenant-new").await;
    assert_eq!(answer["outcome"], "replaced", "{answer}");
    assert_eq!(answer["filesStayingPut"], 1, "{answer}");
    stores.forget(&door_space());

    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_2", OWNER, b"new".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let new = scan_column(&h.pool, "doc_2").await.expect("the column");
    assert!(gw.holds("tenant-new", &new) && !gw.holds("tenant-old", &new));
    assert!(
        gw.holds("tenant-old", &old) && !gw.holds("tenant-new", &old),
        "nothing was copied"
    );

    for (handle, want) in [(&old, b"old".to_vec()), (&new, b"new".to_vec())] {
        let (status, got, err) = get_blob(&h.app, handle, OWNER).await;
        assert_eq!(status, StatusCode::OK, "{err}");
        assert_eq!(got, want);
    }
}

/// **A detach while the tenant holds files is allowed, and the old file is
/// still read** (`st-attach-existing` ask 2). After the detach, new uploads
/// go to the deployment's own store and the old file is served from the
/// detached bucket. A regression is `bucket_refused` from the op, or the old
/// file looked up in the deployment's store.
#[tokio::test(flavor = "multi_thread")]
async fn a_detach_with_files_held_is_allowed_and_old_files_still_read() {
    let (h, stores, gw) = attached_harness("ordo_blobdoor_detach_held").await;
    let ops = bucket_ops(&h);
    attach_by_op(&ops, &gw, "tenant-old").await;
    stores.forget(&door_space());
    insert_doc(&h.pool, "doc_1", OWNER, "old").await;
    insert_doc(&h.pool, "doc_2", OWNER, "new").await;
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"old".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let old = scan_column(&h.pool, "doc_1").await.expect("the column");

    let r = ops
        .detach(ordo_host::DEFAULT_SPACE, OWNER)
        .await
        .expect("a detach with files held is allowed");
    assert_eq!(r.rows[0]["outcome"], "detached", "{}", r.rows[0]);
    assert_eq!(r.rows[0]["filesStayingPut"], 1, "{}", r.rows[0]);
    assert!(ordo_exec::bucket::lookup(&h.pool, &door_space())
        .await
        .unwrap()
        .is_none());
    stores.forget(&door_space());

    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_2", OWNER, b"new".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let new = scan_column(&h.pool, "doc_2").await.expect("the column");
    assert!(
        in_deployment_store(&h, &new),
        "new uploads go to the deployment"
    );
    assert!(gw.holds("tenant-old", &old) && !in_deployment_store(&h, &old));

    let (status, got, err) = get_blob(&h.app, &old, OWNER).await;
    assert_eq!(status, StatusCode::OK, "{err}");
    assert_eq!(got, b"old".to_vec(), "served from the detached bucket");
}

/// **The sweep deletes an old file from its own bucket after a change**
/// (`st-attach-existing` ask 2). The file written to `tenant-old` is
/// replaced after the tenant moved to `tenant-new`; past the floor the sweep
/// deletes it from `tenant-old`. A regression deletes it against
/// `tenant-new`, succeeds because an absent key deletes cleanly, retires
/// the reference, and leaves the bytes in the old bucket for good.
#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_deletes_an_old_file_from_its_own_bucket_after_a_change() {
    let (h, stores, gw) = attached_harness("ordo_blobdoor_change_sweep").await;
    let ops = bucket_ops(&h);
    attach_by_op(&ops, &gw, "tenant-old").await;
    stores.forget(&door_space());
    insert_doc(&h.pool, "doc_1", OWNER, "file").await;

    let at = now_ms();
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"first".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let first = scan_column(&h.pool, "doc_1").await.expect("the column");
    assert!(gw.holds("tenant-old", &first));

    attach_by_op(&ops, &gw, "tenant-new").await;
    stores.forget(&door_space());
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, b"second".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let second = scan_column(&h.pool, "doc_1").await.expect("the column");
    assert!(gw.holds("tenant-new", &second));

    let r = sweep(&h, at).await;
    assert_eq!(r.marked, 1, "the first file is unreferenced");
    let later = at + 91 * DAY_MS;
    chain_stamped_at(&h, later / 1_000).await;
    let r = sweep(&h, later).await;
    assert_eq!(r.deleted, 1, "{r:?}");
    assert!(
        gw.was_deleted_from("tenant-old", &first),
        "deleted from the bucket it was written to"
    );
    assert!(!gw.was_deleted_from("tenant-new", &first));
    assert!(!gw.holds("tenant-old", &first));
    assert!(gw.holds("tenant-new", &second), "the live file stays");
}

// ── `ordo verify --objects` follows each file's recorded store ───────────

/// **A file in a tenant's bucket verifies**, through the resolver the
/// command builds (`bucket::recorded_stores`).
///
/// The file is uploaded to an attached bucket and the attachment is then
/// detached, so only the row's recorded name says where the bytes are. The
/// walk over the resolver finds them and verifies the digest. The same walk
/// over a `SingleStore`, which is what the command built before, looks in the
/// deployment's own store and reports the object absent. A node with no
/// credential provider reports it unverified rather than answering from the
/// wrong store.
#[tokio::test(flavor = "multi_thread")]
async fn verify_objects_follows_the_store_each_file_records() {
    let (h, stores, gw) = attached_harness("ordo_blobdoor_verify_recorded").await;
    ordo_exec::bucket::attach(&h.pool, &door_space(), &bucket_named(&gw, "tenant-a"))
        .await
        .expect("attached");
    stores.forget(&door_space());
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let (status, body) = put_blob(
        &h.app,
        "docs",
        "scan",
        "doc_1",
        OWNER,
        b"in a bucket".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let handle = scan_column(&h.pool, "doc_1").await.expect("the column");
    assert!(gw.holds("tenant-a", &handle), "the bytes are in the bucket");
    assert!(!in_deployment_store(&h, &handle));
    ordo_exec::bucket::detach(&h.pool, &door_space())
        .await
        .expect("detached");

    let space = ordo_commandlog::SpaceId(ordo_host::DEFAULT_SPACE.into());
    let default: Arc<dyn BlobStore> = FsStore::shared(&h.dir).expect("store");
    let walk = |resolver: Arc<dyn ordo_blob::StoreResolver>| {
        let pipe = BlobPipeline::new(
            resolver,
            h.fixture.keys.clone(),
            ordo_blob::BlobPolicy::DEFAULT,
        );
        let pool = h.pool.clone();
        let schema = h.fixture.schema.clone();
        let keys = h.fixture.keys.clone();
        let space = space.clone();
        async move {
            ordo_exec::undo::verify_objects(
                ordo_exec::undo::Db::Pool(&pool),
                &schema,
                &pipe,
                keys.as_ref(),
                &space,
            )
            .await
            .expect("the walk completes")
        }
    };

    let followed = walk(ordo_exec::bucket::recorded_stores(
        h.pool.clone(),
        default.clone(),
        Some(bucket_credentials(&h.dir, "tenant-a")),
    ))
    .await;
    assert!(followed.images >= 1, "the walk saw the upload's image");
    assert!(
        followed.unverified.is_empty(),
        "the file is in the bucket its row names, and it verifies: {:?}",
        followed.unverified
    );

    let single = walk(Arc::new(ordo_blob::SingleStore::new(default.clone()))).await;
    assert!(
        !single.unverified.is_empty(),
        "a single-store resolver looks in the deployment's own store and finds nothing"
    );

    let no_creds = walk(ordo_exec::bucket::recorded_stores(
        h.pool.clone(),
        default,
        None,
    ))
    .await;
    assert!(
        !no_creds.unverified.is_empty(),
        "with no credential provider the file is reported unverified, not read from the \
         wrong store"
    );
}

// ── the public path: files follow their row ─────────────────────────────

/// A real object and its reference row, minted straight through the
/// pipeline in `space`, and a row of `entity` whose `field` holds it.
async fn mint_file(h: &Harness, space: &str, entity: &str, field: &str, id: &str) -> String {
    let crypto = CryptoSpace(space.into());
    let stored = h
        .pipeline
        .put(
            &Upload {
                space: &crypto,
                entity,
                field,
                mime: "image/png",
                plain_len: 5,
                correlation: "test",
                intent: None,
            },
            ordo_blob::once(b"hello".to_vec()),
        )
        .await
        .expect("put");
    let tx = h.pool.begin().await.expect("tx");
    tx.rows(&ordo_crypto::blobref::insert_sql(
        &crypto,
        stored.handle(),
        stored.epoch(),
        entity,
        field,
        stored.mime(),
        stored.size(),
        stored.integrity(),
        stored.bytes(),
        0,
        "",
    ))
    .await
    .expect("the reference row");
    let hex = ordo_crypto::hex(stored.handle());
    let row = if entity == "docs" {
        tx.rows(&format!(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"principal\", \"displayName\", \"kind\", \
             \"memberCount\", \"role\", \"status\", \"joined\") VALUES ('{space}', '{OWNER}', \
             '{OWNER}', 'Ora', 'principal', 1, 'owner', 'active', DATE '2026-01-01') \
             ON CONFLICT DO NOTHING"
        ))
        .await
        .expect("the owner's row");
        format!(
            "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"owner\", \"title\", \"{field}\") \
             VALUES ('{space}', '{id}', 1, '{OWNER}', 't', '{hex}')"
        )
    } else {
        format!(
            "INSERT INTO \"{entity}\" (\"$space\", \"id\", \"rev\", \"title\", \"{field}\") \
             VALUES ('{space}', '{id}', 1, 't', '{hex}')"
        )
    };
    tx.rows(&row).await.expect("the row");
    tx.commit().await.expect("commit");
    hex
}

/// A `GET /blob/{handle}` with no token, and the space header only when
/// asked for.
async fn get_anonymous(
    app: &axum::Router,
    handle_hex: &str,
    space: Option<&str>,
) -> (StatusCode, Vec<u8>, String) {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(format!("/blob/{handle_hex}"));
    if let Some(s) = space {
        req = req.header("x-ordo-space", s);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let cache = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, bytes.to_vec(), cache)
}

/// `design-public-path.md` §11 row 1: a public op's file is served and a
/// private row's file is not. A visitor with no token reads a file of a
/// public row in the deployment's space; a file of a private, guarded row in
/// the same space answers what a tokenless request always got.
#[tokio::test(flavor = "multi_thread")]
async fn a_public_rows_file_is_served_and_a_private_rows_is_not() {
    let h = harness("ordo_blobdoor_public").await;
    let public = mint_file(&h, "system", "posters", "art", "post_1").await;
    let (status, bytes, cache) = get_anonymous(&h.app, &public, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(bytes, b"hello");
    assert!(cache.starts_with("public, max-age=0, s-maxage="), "{cache}");

    // What a tokenless request to the file door answered before the public
    // path: the door's denial, naming the pseudo-op.
    let (_, before, _) =
        get_anonymous(&h.app, &"b".repeat(32), Some(ordo_host::DEFAULT_SPACE)).await;
    let private = mint_file(&h, "system", "docs", "scan", "doc_9").await;
    let (status, body, _) = get_anonymous(&h.app, &private, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, before, "byte for byte the door's one answer");

    // A visitor naming a tenant's space is refused the same way.
    let (status, body, _) = get_anonymous(&h.app, &public, Some(ordo_host::DEFAULT_SPACE)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, before);
}

// ── a backend function reads a file (functions increment 2) ─────────────

/// A function that takes a file, reads it as the caller, and sends its
/// bytes out as they are.
const READER: &str = "
function readScan {
  call($file: blob) -> json by member
  code \"fn/read-scan.js\"
  egress api.ocr.example
}
";

const READER_CODE: &str = r#"
export default {
  async call(input, ctx) {
    const file = await ctx.blob(input.file);
    await fetch("https://api.ocr.example/v1/scan", { method: "POST", body: file });
    return { type: file.type, size: file.size, text: await file.text() };
  },
};
"#;

/// The OCR service: keeps every body it was sent.
struct Ocr {
    bodies: Mutex<Vec<Vec<u8>>>,
}

#[async_trait::async_trait]
impl ordo_fn::FnFetch for Ocr {
    async fn fetch(
        &self,
        _space: &str,
        req: ordo_fn::FnFetchRequest,
    ) -> Result<ordo_fn::FnFetchResponse, ordo_fn::FetchError> {
        self.bodies.lock().unwrap().push(req.body);
        Ok(ordo_fn::FnFetchResponse {
            status: 200,
            headers: Vec::new(),
            body: b"{}".to_vec(),
        })
    }

    async fn mac(
        &self,
        _space: &str,
        _req: ordo_fn::FnMacRequest,
    ) -> Result<ordo_fn::FnMacAnswer, ordo_fn::FetchError> {
        panic!("no secret here")
    }
}

async fn call_reader(app: &axum::Router, who: &str, handle: &str) -> (StatusCode, Value) {
    let body = serde_json::json!({ "file": handle }).to_string();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/fn/readScan")
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// **A function reads a file as its caller, through the real store and the
/// real row guard.** OWNER uploads through the real door; the function,
/// called by OWNER, reads the bytes and sends them out. MANAGER clears the
/// door for both the function and `std.$blob.get`, names OWNER's handle,
/// and is refused `not_found` by the row guard: the code never gets the
/// bytes, and nothing goes out.
#[tokio::test(flavor = "multi_thread")]
async fn a_function_reads_a_file_as_its_caller_and_the_row_guard_holds() {
    let db = "ordo_blobdoor_fn_read";
    let dir = std::env::temp_dir().join(format!("ordo-blob-door-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    let h = harness_full(
        db,
        dir,
        Arc::new(ordo_blob::SingleStore::new(store)),
        READER,
    )
    .await;
    insert_doc(&h.pool, "doc_1", OWNER, "owner's file").await;
    let bytes = b"%PDF-1.7 owner's scan".to_vec();
    let (status, body) = put_blob(&h.app, "docs", "scan", "doc_1", OWNER, bytes.clone()).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let handle = scan_column(&h.pool, "doc_1").await.expect("set");

    // What `ordo apply` does: the code by hash into the manifest.
    let mut doc =
        ordo_plan::artifact::program_artifact(&h.fixture.schema, &h.fixture.registry).unwrap();
    let hash = ordo_plan::artifact::code_hash(READER_CODE.as_bytes());
    let mut hashes = std::collections::BTreeMap::new();
    hashes.insert("fn/read-scan.js".to_string(), hash.clone());
    ordo_plan::artifact::bind_function_code(&mut doc, &hashes).unwrap();
    let mut code = std::collections::BTreeMap::new();
    code.insert(hash, READER_CODE.as_bytes().to_vec());
    let table = ordo_host::functions::FnTable::build(&h.fixture.schema, &doc, "v1", &code)
        .expect("the function loads");
    ordo_host::functions::ensure_tables(&h.pool).await.unwrap();
    let ocr = Arc::new(Ocr {
        bodies: Mutex::new(Vec::new()),
    });
    let runner = Arc::new(
        ordo_host::functions::HostFunctions::new(
            ordo_fn::HostConfig::default(),
            table,
            h.spaces.clone(),
            h.pool.clone(),
            ocr.clone(),
            Arc::new(ordo_host::functions::NoCallScrub),
        )
        .with_blobs(Some(h.port.clone())),
    );
    let app = h
        .app
        .clone()
        .merge(ordo_api::function_routes(h.state.clone(), runner));

    // OWNER: the file is read, answered and sent out unchanged.
    let (status, out) = call_reader(&app, OWNER, &handle).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        out["result"],
        serde_json::json!({ "type": "application/pdf", "size": bytes.len(), "text": "%PDF-1.7 owner's scan" })
    );
    assert_eq!(*ocr.bodies.lock().unwrap(), vec![bytes.clone()]);

    // MANAGER, with OWNER's handle: the row guard hides the row.
    let (status, out) = call_reader(&app, MANAGER, &handle).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    assert_eq!(out["error"]["code"], "not_found", "{out}");
    assert!(!out.to_string().contains("owner's scan"), "{out}");
    assert_eq!(ocr.bodies.lock().unwrap().len(), 1, "nothing more went out");

    let runs = ordo_host::functions::runs(&h.pool, ordo_host::DEFAULT_SPACE)
        .await
        .unwrap();
    let outcomes: Vec<&str> = runs.iter().map(|r| r.outcome.as_str()).collect();
    assert_eq!(outcomes, ["ok", "blob_not_found"]);
    assert!(runs[1]
        .principal
        .ends_with(&format!("on behalf of {MANAGER}")));
}
