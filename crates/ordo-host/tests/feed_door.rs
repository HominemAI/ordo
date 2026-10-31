//! A long-lived read asks the whole door again while it is open
//! (security audit 2026-10-24, F-2 and the sync pentest).
//!
//! A feed is admitted once, but what admitted it can end while it runs. Each
//! feed kind (`?live=1&sync=1`, `?live=1`, a read on `/live`) asks the token
//! gate (delegation and share liveness) and the access check (membership,
//! session) before every pass and on an idle tick, and a refusal is its last
//! frame. Before the fix the sync asked the access check only, and the other
//! two asked nothing, so:
//!
//! - an agent whose delegation was withdrawn kept receiving `change` frames
//!   with the rows written after the withdrawal;
//! - a window token whose share was revoked kept its feed when its holder
//!   also held a role;
//! - after a logout every feed stayed open, and `?live=1` and `/live` kept
//!   sending `invalidate` for ever.
//!
//! A regression looks like a `change` or `invalidate` frame after the
//! withdrawal, or a feed still open after one idle tick.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::delegable::DelegablePolicy;
use ordo_host::delegation::PgDelegationRegistry;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::delegation::{DelegationClaim, DelegationRegistry, NewDelegation};
use ordo_identity::grants::{register_share, GrantRegistry};
use ordo_identity::principal::{
    DelegationId, GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp,
};
use ordo_identity::session::{MemorySessionRevocations, SessionClaim, SessionRevocations};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"feed-door-secret";
/// The idle re-check these tests run under: one second, so "within one
/// tick" is measurable.
const RECHECK: Duration = Duration::from_secs(1);
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct H {
    state: ApiState,
    bus: Arc<MemoryBus>,
    engine: Arc<AccessEngine>,
    grants: Arc<PgGrantRegistry>,
    delegations: Arc<PgDelegationRegistry>,
    revocations: Arc<MemorySessionRevocations>,
    pool: ordo_exec::Pool,
}

async fn harness(db: &str) -> H {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("grant table");
    let delegations = Arc::new(PgDelegationRegistry::new(pool.clone()));
    delegations.ensure_table().await.expect("delegation table");
    let _ = pool.rows("DELETE FROM \"$capabilitygrant\"").await;
    let _ = pool.rows("DELETE FROM \"$delegation\"").await;
    let engine = Arc::new(
        ordo_host::deployment_engine_delegating(&f, &pg, &DelegablePolicy::closed())
            .expect("engine"),
    );
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "feed-door",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("deployment");
    let revocations = Arc::new(MemorySessionRevocations::new());
    let access = ordo_host::IdentityAccess::sharing(
        engine.clone(),
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    )
    .with_shares(grants.clone())
    .with_delegations(delegations.clone())
    .with_revocations(revocations.clone());
    let ops = PlatformOps::new(engine.clone(), grants.clone(), Arc::new(|| NOW))
        .with_delegations(delegations.clone());
    let bus = Arc::new(MemoryBus::new());
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
                page_size: 50,
            }),
            Arc::new(ops),
        )),
        access: Arc::new(access),
        quota: Arc::new(ordo_api::NoQuota),
        bus: bus.clone(),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"feed-door"),
        sync: Default::default(),
        live_streams: ordo_api::LiveStreams::new(ordo_api::LiveSettings {
            recheck: RECHECK,
            ..Default::default()
        }),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    H {
        state,
        bus,
        engine,
        grants,
        delegations,
        revocations,
        pool,
    }
}

fn space() -> ISpace {
    ISpace(DEFAULT_SPACE.to_string())
}

fn mint(
    principal: &str,
    cap: Option<(&AccessEngine, &[&str], &str)>,
    delegation: Option<DelegationClaim>,
    session: Option<SessionClaim>,
) -> String {
    let capability = cap.map(|(e, ops, id)| {
        let set: BTreeSet<String> = ops.iter().map(|s| s.to_string()).collect();
        e.grant_for(GrantId(id.into()), &space(), set, IStamp(NOW + 3_600))
            .expect("deployed")
    });
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability,
            delegation,
            session,
        },
    )
}

async fn record_share(h: &H, id: &str, principal: &str, ops: &[&str]) {
    let pins = h
        .engine
        .grant_for(
            GrantId(id.into()),
            &space(),
            ops.iter().map(|s| s.to_string()).collect(),
            IStamp(NOW + 3_600),
        )
        .unwrap()
        .pins()
        .clone();
    let g = register_share(
        GrantId(id.into()),
        space(),
        PrincipalId(principal.into()),
        pins,
        IStamp(NOW),
        IStamp(NOW + 3_600),
    )
    .unwrap();
    h.grants.record(&g).await.unwrap();
}

async fn call(
    h: &H,
    method: &str,
    uri: &str,
    bearer: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", format!("Bearer {bearer}"));
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = router(h.state.clone()).oneshot(req).await.unwrap();
    let st = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// An open feed: what it has sent so far, and whether it has ended.
struct Feed {
    text: Arc<Mutex<String>>,
    ended: Arc<AtomicBool>,
}

impl Feed {
    fn mark(&self) -> usize {
        self.text.lock().unwrap().len()
    }
    fn since(&self, mark: usize) -> String {
        self.text.lock().unwrap()[mark..].to_string()
    }
    fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

/// Open an SSE and pump it into a buffer for at most 30 s.
async fn open(h: &H, uri: &str, bearer: &str) -> Feed {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", format!("Bearer {bearer}"))
        .header("accept", "text/event-stream")
        .body(Body::empty())
        .unwrap();
    let resp = router(h.state.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "feed opens: {uri}");
    let text = Arc::new(Mutex::new(String::new()));
    let ended = Arc::new(AtomicBool::new(false));
    let (b, e) = (text.clone(), ended.clone());
    tokio::spawn(async move {
        let mut s = resp.into_body().into_data_stream();
        let until = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            match tokio::time::timeout_at(until, s.next()).await {
                Ok(Some(Ok(chunk))) => b.lock().unwrap().push_str(&String::from_utf8_lossy(&chunk)),
                Ok(_) => {
                    e.store(true, Ordering::SeqCst);
                    return;
                }
                Err(_) => return,
            }
        }
    });
    Feed { text, ended }
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(600)).await;
}

async fn rename_board(h: &H, name: &str) {
    h.pool
        .rows(&format!(
            r#"UPDATE "boards" SET "name" = '{name}' WHERE "id" = 'board_1'"#
        ))
        .await
        .unwrap();
    h.bus.publish("boardList");
}

/// The feed ended on a refusal naming `code`, and sent nothing else after
/// `mark`: no change, no invalidate, none of the rows written since.
fn ended_refused(feed: &Feed, mark: usize, code: &str, secret: &str, what: &str) {
    let after = feed.since(mark);
    assert!(
        after.contains("event: error") && after.contains(&format!("\"code\":\"{code}\"")),
        "{what}: ends with an error frame naming {code}; sent:\n{after}"
    );
    assert!(
        !after.contains("event: change") && !after.contains("event: invalidate"),
        "{what}: nothing follows the refusal; sent:\n{after}"
    );
    assert!(
        secret.is_empty() || !after.contains(secret),
        "{what}: no row written after the refusal; sent:\n{after}"
    );
    assert!(feed.ended(), "{what}: the feed is closed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawn_delegation_ends_the_agents_feeds() {
    let _s = SERIAL.lock().await;
    let h = harness("ordo_feed_door_dlg").await;
    h.delegations
        .record(&NewDelegation {
            id: DelegationId("dlg_f".into()),
            space: space(),
            delegator: PrincipalId("mem_1".into()),
            agent: PrincipalId("agt_f".into()),
            granted_at: IStamp(NOW),
            expires: IStamp(NOW + 3_600),
            consent_correlation: "cg_f".into(),
        })
        .await
        .unwrap();
    let claim = DelegationClaim {
        id: DelegationId("dlg_f".into()),
        space: space(),
        delegator: PrincipalId("mem_1".into()),
        expires: IStamp(NOW + 3_600),
    };
    let t = mint(
        "agt_f",
        Some((&h.engine, &["boardList"], "gr_agent")),
        Some(claim),
        None,
    );
    let (st, b) = call(&h, "GET", "/op/boardList", &t, None).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let sync = open(&h, "/op/boardList?live=1&sync=1", &t).await;
    let live = open(&h, "/op/boardList?live=1", &t).await;
    settle().await;
    let (m_sync, m_live) = (sync.mark(), live.mark());

    h.delegations
        .revoke(&space(), &DelegationId("dlg_f".into()), IStamp(NOW))
        .await
        .unwrap();
    // Written at once, before any idle tick: the pass itself must refuse.
    rename_board(&h, "after-withdraw-secret").await;
    settle().await;
    rename_board(&h, "after-withdraw-secret-2").await;
    settle().await;

    ended_refused(
        &sync,
        m_sync,
        "delegation_required",
        "after-withdraw-secret",
        "sync",
    );
    ended_refused(&live, m_live, "delegation_required", "", "?live=1");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_share_ends_a_members_window_feed() {
    let _s = SERIAL.lock().await;
    let h = harness("ordo_feed_door_mem").await;
    // mem_2 holds a role, so the access check alone still lets the window
    // token read; only the share gate knows the window was closed.
    record_share(&h, "shr_fmem", "mem_2", &["boardList"]).await;
    let t = mint(
        "mem_2",
        Some((&h.engine, &["boardList"], "shr_fmem")),
        None,
        None,
    );
    let sync = open(&h, "/op/boardList?live=1&sync=1", &t).await;
    settle().await;
    let m = sync.mark();

    h.grants
        .revoke(&space(), &GrantId("shr_fmem".into()), IStamp(NOW))
        .await
        .unwrap();
    rename_board(&h, "after-revoke-member").await;
    settle().await;

    ended_refused(
        &sync,
        m,
        "access_denied",
        "after-revoke-member",
        "window sync",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_ends_every_feed_kind_within_one_idle_tick_with_no_write() {
    let _s = SERIAL.lock().await;
    let h = harness("ordo_feed_door_logout").await;
    let claim = SessionClaim::starting_at(IStamp(NOW - 10));
    let sid = claim.id.clone();
    let t = mint("mem_1", None, None, Some(claim));
    let sync = open(&h, "/op/boardList?live=1&sync=1", &t).await;
    let live = open(&h, "/op/boardList?live=1", &t).await;
    let shared = open(&h, "/live", &t).await;
    settle().await;
    let text = shared.since(0);
    let id = text
        .split("\"stream\":\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("ready frame")
        .to_string();
    let (st, b) = call(
        &h,
        "POST",
        &format!("/live/{id}"),
        &t,
        Some(json!({"key": "r1", "op": "boardList", "params": {}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    settle().await;
    let (m1, m2, m3) = (sync.mark(), live.mark(), shared.mark());

    h.revocations.revoke(&sid, IStamp(NOW + 3_600)).unwrap();
    // No write. One tick, and room for the frame to arrive.
    tokio::time::sleep(RECHECK * 2 + Duration::from_millis(500)).await;

    ended_refused(&sync, m1, "access_denied", "", "sync");
    ended_refused(&live, m2, "access_denied", "", "?live=1");
    ended_refused(&shared, m3, "access_denied", "", "/live");
    assert!(
        shared.since(m3).contains("\"key\":\"r1\""),
        "/live names the read that was refused"
    );
}
