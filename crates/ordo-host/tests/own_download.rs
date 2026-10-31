//! A person's own download, `GET /download`, through the real router, the
//! real executor, the real blob pipeline and the real row guard (owner
//! ruling 2026-10-22, "A person's own download").
//!
//! What each test holds:
//!
//! - off by default, and the refusal names how the app developer turns it on;
//! - the marked op decides the fields and the rows ("what 'all' is");
//! - another person's rows, and their files, are never in it;
//! - files only for a marked op that answers a blob field;
//! - the answer is streamed: no `Content-Length`, a zip any tool opens;
//! - the per-hour bound refuses by its setting's name;
//! - each download is recorded on the security stream;
//! - a delegated token and a share guest are refused by default, naming the
//!   setting, and the app developer can open it to delegated tokens.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ordo_api::blob::{BlobLimits, BlobPort, BlobState};
use ordo_api::download::{DownloadLimits, DownloadState};
use ordo_api::{router_with_blobs, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_host::blob::PlatformBlobs;
use ordo_host::deploy::Deployment;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"own-download-test-secret";
const OWNER: &str = "mem_o";
const MANAGER: &str = "mem_1";

/// Two entities with a file each, both guarded to their own `owner`.
const ENTITIES: &str = "
entity docs (key: doc) {
  owner -> members
  title:  text
  secret: text = \"\"
  scan:   blob?
  access read   owner == $caller
  access write  owner == $caller
}

entity memos (key: memo) {
  owner -> members
  body: text
  att:  blob?
  access read   owner == $caller
  access write  owner == $caller
}
";

/// The app developer's declaration: `docs` without drafts and without
/// `secret`, with their files; `memos` without its file field.
const MARKED: &str = "
op myDocs unbounded download =
  docs(title != \"draft\")[id, title, scan]

op myMemos unbounded download =
  memos[id, body]
";

struct Harness {
    app: axum::Router,
    pool: ordo_exec::Pool,
    dir: PathBuf,
    recorder: Arc<ordo_secevent::Recorder>,
    engine: Arc<ordo_identity::AccessEngine>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

async fn harness(db: &str, program: &str, limits: DownloadLimits) -> Harness {
    harness_with(db, program, limits, false).await
}

async fn harness_with(
    db: &str,
    program: &str,
    limits: DownloadLimits,
    delegated_open: bool,
) -> Harness {
    let dir = std::env::temp_dir().join(format!("ordo-own-download-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store: Arc<dyn BlobStore> = FsStore::shared(&dir).expect("store");
    let src = format!("{FIXTURE}\n{ENTITIES}\n{program}");
    let f = Arc::new(Fixture::from_source(&src, SEED).expect("the program compiles"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");

    let engine = Arc::new(ordo_host::deployment_engine(&f, &pg).expect("engine"));
    let deployment = Deployment::pinned(
        "own-download-test",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    let pipeline = BlobPipeline::new(
        Arc::new(ordo_blob::SingleStore::new(store)),
        f.keys.clone(),
        BlobPolicy::DEFAULT,
    );
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let port = Arc::new(PlatformBlobs {
        spaces: spaces.clone(),
        pipeline,
    });
    let delegations = Arc::new(ordo_host::delegation::PgDelegationRegistry::new(
        pool.clone(),
    ));
    delegations.ensure_table().await.expect("delegation table");
    let _ = pool.rows("DELETE FROM \"$delegation\"").await;
    let access = Arc::new(
        ordo_host::IdentityAccess::sharing(
            engine.clone(),
            TokenKey::from_secret(SECRET),
            Box::new(|| NOW),
            Arc::new(ordo_host::metrics::Metrics::new()),
        )
        .with_delegations(delegations.clone()),
    );
    {
        use ordo_identity::delegation::{DelegationRegistry, NewDelegation};
        use ordo_identity::DelegationId;
        delegations
            .record(&NewDelegation {
                id: DelegationId("dl_dl".into()),
                space: ordo_identity::principal::SpaceId(ordo_host::DEFAULT_SPACE.into()),
                delegator: PrincipalId(OWNER.into()),
                agent: PrincipalId("agent_dl".into()),
                granted_at: IStamp(NOW),
                expires: IStamp(NOW + 3_600),
                consent_correlation: "cg_consent_dl".into(),
            })
            .await
            .expect("recorded");
    }
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(ordo_host::FixtureExecutor {
            spaces,
            pg: Pg {
                db: db.to_string(),
                ..Pg::default()
            },
            page_size: 2,
        }),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"own-download-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    let blobs = BlobState {
        port: port as Arc<dyn BlobPort>,
        limits: BlobLimits::DEFAULT,
    };
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let download = DownloadState::new(
        Arc::new(ordo_host::own_download::ProgramDownload {
            deployment: deployment.clone(),
            stream: Some(recorder.clone()),
            now: Arc::new(|| NOW),
        }),
        limits,
    )
    .open_to_delegated(delegated_open);
    let app = router_with_blobs(state.clone(), blobs.clone()).merge(ordo_api::download::router(
        state,
        Some(blobs),
        download,
    ));
    Harness {
        app,
        pool,
        dir,
        recorder,
        engine,
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

async fn insert(pool: &ordo_exec::Pool, sql: &str) {
    pool.rows(sql).await.expect("insert");
}

async fn doc(pool: &ordo_exec::Pool, id: &str, owner: &str, title: &str) {
    insert(
        pool,
        &format!(
            "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"owner\", \"title\", \"secret\") \
             VALUES ('{}', '{id}', 1, '{owner}', '{title}', 'hidden-{id}')",
            ordo_host::DEFAULT_SPACE
        ),
    )
    .await;
}

async fn memo(pool: &ordo_exec::Pool, id: &str, owner: &str, body: &str) {
    insert(
        pool,
        &format!(
            "INSERT INTO \"memos\" (\"$space\", \"id\", \"rev\", \"owner\", \"body\") \
             VALUES ('{}', '{id}', 1, '{owner}', '{body}')",
            ordo_host::DEFAULT_SPACE
        ),
    )
    .await;
}

async fn put_blob(app: &axum::Router, path: &str, who: &str, body: &[u8]) {
    let req = Request::builder()
        .method(Method::PUT)
        .uri(path)
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
        .header("content-length", body.len().to_string())
        .header("content-type", "text/plain")
        .body(Body::from(body.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "upload to {path}");
}

struct Answer {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

async fn download(app: &axum::Router, who: &str) -> Answer {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/download")
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 24)
        .await
        .unwrap()
        .to_vec();
    Answer {
        status,
        headers,
        body,
    }
}

/// The entries of a zip, by its local headers, and a check that the end
/// record counts them all.
fn unzip(data: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let u32le = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    let u16le = |at: usize| u16::from_le_bytes(data[at..at + 2].try_into().unwrap()) as usize;
    let mut out = BTreeMap::new();
    let mut at = 0;
    while u32le(at) == 0x0403_4b50 {
        let size = u32le(at + 18) as usize;
        let name_len = u16le(at + 26);
        let extra = u16le(at + 28);
        let name = String::from_utf8(data[at + 30..at + 30 + name_len].to_vec()).unwrap();
        let start = at + 30 + name_len + extra;
        out.insert(name, data[start..start + size].to_vec());
        at = start + size;
    }
    let end = data.len() - 22;
    assert_eq!(
        u32le(end),
        0x0605_4b50,
        "the archive ends with its end record"
    );
    assert_eq!(
        u16le(end + 10),
        out.len(),
        "the end record counts every entry"
    );
    out
}

fn json(entries: &BTreeMap<String, Vec<u8>>, name: &str) -> Value {
    serde_json::from_slice(
        entries
            .get(name)
            .unwrap_or_else(|| panic!("{name} is in the archive")),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn off_by_default_and_the_refusal_names_the_switch() {
    let h = harness("ordo_own_download_off", "", DownloadLimits::DEFAULT).await;
    doc(&h.pool, "doc_1", OWNER, "mine").await;
    let a = download(&h.app, OWNER).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let body: Value = serde_json::from_slice(&a.body).unwrap();
    let message = body.to_string();
    assert!(message.contains("access_denied"), "{message}");
    assert!(
        message.contains("`download`"),
        "the refusal names the mark: {message}"
    );
    assert!(message.contains("app developer"), "{message}");

    // No token: the door's one answer, which names nothing.
    let req = Request::builder()
        .uri("/download")
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
        .body(Body::empty())
        .unwrap();
    let resp = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("download`"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_declaration_decides_fields_rows_and_files_and_the_guard_decides_whose() {
    let h = harness("ordo_own_download_on", MARKED, DownloadLimits::DEFAULT).await;
    doc(&h.pool, "doc_a", OWNER, "report").await;
    doc(&h.pool, "doc_b", OWNER, "draft").await;
    doc(&h.pool, "doc_c", OWNER, "letter").await;
    doc(&h.pool, "doc_m", MANAGER, "theirs").await;
    memo(&h.pool, "memo_a", OWNER, "remember").await;
    memo(&h.pool, "memo_m", MANAGER, "not yours").await;
    put_blob(
        &h.app,
        "/blob/docs/scan/doc_a",
        OWNER,
        b"scan of the report",
    )
    .await;
    put_blob(&h.app, "/blob/docs/scan/doc_m", MANAGER, b"their scan").await;
    put_blob(&h.app, "/blob/memos/att/memo_a", OWNER, b"memo file").await;

    let before = h.recorder.observed();
    let a = download(&h.app, OWNER).await;
    assert_eq!(
        a.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&a.body)
    );
    assert_eq!(a.headers["content-type"], "application/zip");
    assert!(a.headers["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment; filename=\"download-"));
    // An archive of stored bytes is under the same sandbox as one file.
    assert_eq!(
        a.headers["content-security-policy"],
        "sandbox; default-src 'none'"
    );
    assert_eq!(a.headers["x-content-type-options"], "nosniff");
    // Streamed: the length is not known when the answer starts.
    assert!(a.headers.get("content-length").is_none(), "{:?}", a.headers);

    let entries = unzip(&a.body);
    let manifest = json(&entries, "manifest.json");
    assert_eq!(manifest["format"], "ordo.download");
    assert_eq!(manifest["complete"], true);

    // Fields and rows: the op's projection and filter, read in pages of two.
    let docs = json(&entries, "docs.json");
    let docs = docs.as_array().unwrap();
    let titles: Vec<&str> = docs.iter().map(|d| d["title"].as_str().unwrap()).collect();
    assert_eq!(titles.len(), 2, "{docs:?}");
    assert!(titles.contains(&"report") && titles.contains(&"letter"));
    assert!(!titles.contains(&"draft"), "the declared filter holds");
    assert!(
        !titles.contains(&"theirs"),
        "another person's row is never in it"
    );
    assert!(
        docs.iter().all(|d| d.get("secret").is_none()),
        "an undeclared field is not in it"
    );

    // Files: the declared one, under its path, with its bytes.
    let report = docs.iter().find(|d| d["title"] == "report").unwrap();
    let path = report["scan"]["path"].as_str().unwrap();
    assert_eq!(path, "files/docs/doc_a/scan");
    assert_eq!(entries[path], b"scan of the report");
    assert_eq!(report["scan"]["size"], 18);
    let letter = docs.iter().find(|d| d["title"] == "letter").unwrap();
    assert_eq!(letter["scan"], Value::Null, "a row with no file says so");

    // The memo's file field is not declared, so no file of it is in it.
    let memos = json(&entries, "memos.json");
    let memos = memos.as_array().unwrap();
    assert_eq!(memos.len(), 1);
    assert_eq!(memos[0]["body"], "remember");
    assert!(memos[0].get("att").is_none());
    assert!(entries.keys().all(|k| !k.starts_with("files/memos")));
    // Nobody else's file, by bytes.
    assert!(entries.values().all(|v| v != b"their scan"));

    assert!(h.recorder.observed() > before, "the download is recorded");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hour_and_the_size_are_bounded_by_their_settings() {
    let h = harness(
        "ordo_own_download_bounds",
        MARKED,
        DownloadLimits {
            per_hour: 1,
            max_bytes: 4,
        },
    )
    .await;
    doc(&h.pool, "doc_a", OWNER, "report").await;
    put_blob(
        &h.app,
        "/blob/docs/scan/doc_a",
        OWNER,
        b"more than four bytes",
    )
    .await;

    let a = download(&h.app, OWNER).await;
    assert_eq!(a.status, StatusCode::OK);
    let entries = unzip(&a.body);
    assert_eq!(json(&entries, "manifest.json")["complete"], false);
    let docs = json(&entries, "docs.json");
    let missing = docs[0]["scan"]["missing"].as_str().unwrap();
    assert!(missing.contains("ORDO_OWN_DOWNLOAD_MAX_BYTES"), "{missing}");

    let again = download(&h.app, OWNER).await;
    assert_eq!(again.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(String::from_utf8_lossy(&again.body).contains("ORDO_OWN_DOWNLOAD_PER_HOUR"));
    assert!(again.headers.get("retry-after").is_some());
    // Another person has an hour of their own.
    assert_eq!(download(&h.app, MANAGER).await.status, StatusCode::OK);
}

/// A token that is not the person's: a capability grant listing the marked
/// ops, with a live delegation from `OWNER` when `delegated`, else a plain
/// grant held by a guest.
fn other_token(h: &Harness, delegated: bool) -> String {
    use ordo_identity::delegation::DelegationClaim;
    use ordo_identity::DelegationId;
    use ordo_identity::GrantId;
    let ops = ["myDocs", "myMemos"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let grant = h
        .engine
        .grant_for(
            GrantId("gr_dl".into()),
            &ordo_identity::principal::SpaceId(ordo_host::DEFAULT_SPACE.into()),
            ops,
            IStamp(NOW + 3_600),
        )
        .expect("the marked ops are deployed");
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(if delegated { "agent_dl" } else { "guest_dl" }.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: Some(grant),
            delegation: delegated.then(|| DelegationClaim {
                id: DelegationId("dl_dl".into()),
                space: ordo_identity::principal::SpaceId(ordo_host::DEFAULT_SPACE.into()),
                delegator: PrincipalId(OWNER.into()),
                expires: IStamp(NOW + 3_600),
            }),
            session: None,
        },
    )
}

async fn download_with(app: &axum::Router, bearer: &str) -> Answer {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/download")
        .header("authorization", format!("Bearer {bearer}"))
        .header("x-ordo-space", ordo_host::DEFAULT_SPACE)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 24)
        .await
        .unwrap()
        .to_vec();
    Answer {
        status,
        headers,
        body,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delegated_token_and_a_guest_are_refused_by_default_and_the_refusal_names_the_setting() {
    let h = harness("ordo_own_download_closed", MARKED, DownloadLimits::DEFAULT).await;
    doc(&h.pool, "doc_a", OWNER, "report").await;

    let a = download_with(&h.app, &other_token(&h, true)).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let text = String::from_utf8_lossy(&a.body).to_string();
    assert!(text.contains("delegated token"), "{text}");
    assert!(
        text.contains("ORDO_DELEGABLE_OPS"),
        "names the setting: {text}"
    );
    assert!(text.contains("`download`"), "names the entry: {text}");

    let a = download_with(&h.app, &other_token(&h, false)).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let text = String::from_utf8_lossy(&a.body).to_string();
    assert!(text.contains("share guest"), "{text}");

    // The person is let in.
    let a = download(&h.app, OWNER).await;
    assert_eq!(a.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn naming_the_download_opens_it_to_delegated_tokens_but_never_to_a_guest() {
    let h = harness_with(
        "ordo_own_download_open",
        MARKED,
        DownloadLimits::DEFAULT,
        true,
    )
    .await;
    doc(&h.pool, "doc_a", OWNER, "report").await;

    let a = download_with(&h.app, &other_token(&h, true)).await;
    assert_eq!(
        a.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&a.body)
    );
    let files = unzip(&a.body);
    assert!(files.contains_key("docs.json"), "{:?}", files.keys());

    let a = download_with(&h.app, &other_token(&h, false)).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "a guest is never let in");
}
