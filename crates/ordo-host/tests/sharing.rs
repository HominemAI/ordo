//! Guests and sharing between tenants (`docs/plan-sharing.md`), through the
//! real door, against real Postgres.
//!
//! A guest is a seat with no role. What a guest reaches comes from a share
//! grant and from nothing else, so the tests here are ordered by the
//! property the plan turns on: a seat with no role is not a viewer.
//!
//! Databases are prefixed `ordo_shr_`. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, BoundParams, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::member_ops::MemberOps;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::share_ops::{ShareOps, REVOKE_SHARE, SHARE, SHARES};
use ordo_host::tickets::{
    MemoryTicketStore, Purpose, TicketRecord, TicketStore, Tickets, Want, MAX_TTL_SECS,
};
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::grants::{register_share, GrantError, GrantRecord, GrantRegistry, NewGrant};
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

/// The door's clock. Fixed, so an expiry under test is a property of the
/// grant rather than of how long the suite took to run.
const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"sharing-secret";

/// The fixture's guest: `seed.sql` seats `mem_g` with a `NULL` role and one
/// acl row, `read` on `board_1`.
const GUEST: &str = "mem_g";

struct Harness {
    engine: Arc<AccessEngine>,
    pool: ordo_exec::Pool,
    grants: Arc<PgGrantRegistry>,
    deployment: Arc<ordo_host::deploy::Deployment>,
    pg: Pg,
    platform: Arc<PlatformOps>,
    fixture: Arc<Fixture>,
}

impl Harness {
    /// The deployment behind the door as it is wired: share grants are
    /// asked of the real registry.
    fn state(&self) -> ApiState {
        self.state_with(self.grants.clone())
    }

    /// The same deployment behind a door that asks `shares` whether a share
    /// grant is live.
    fn state_with(&self, shares: Arc<dyn GrantRegistry>) -> ApiState {
        self.state_over(self.engine.clone(), shares)
    }

    /// The door over `engine`, which a test rebuilds where the runtime would
    /// after a membership change.
    fn door_over(
        &self,
        engine: Arc<AccessEngine>,
        shares: Arc<dyn GrantRegistry>,
    ) -> Arc<ordo_host::IdentityAccess> {
        Arc::new(
            ordo_host::IdentityAccess::sharing(
                engine,
                TokenKey::from_secret(SECRET),
                Box::new(|| NOW),
                Arc::new(ordo_host::metrics::Metrics::new()),
            )
            .with_shares(shares),
        )
    }

    fn state_over(&self, engine: Arc<AccessEngine>, shares: Arc<dyn GrantRegistry>) -> ApiState {
        self.state_from(&self.door_over(engine, shares))
    }

    fn state_from(&self, access: &Arc<ordo_host::IdentityAccess>) -> ApiState {
        ApiState {
            registry: self.deployment.handle(),
            executor: Arc::new(PlatformExecutor::new(
                Arc::new(FixtureExecutor {
                    spaces: ordo_host::spaces::Spaces::new(self.deployment.clone()),
                    pg: self.pg.clone(),
                    page_size: 50,
                }),
                self.platform.clone(),
            )),
            access: access.clone(),
            quota: Arc::new(ordo_api::NoQuota),
            bus: Arc::new(MemoryBus::new()),
            replay: Arc::new(MemoryReplayCache::new()),
            attest: Arc::new(ordo_api::attest::NoAttestation),
            page_key: PageKey::from_secret(b"sharing-test"),
            sync: Default::default(),
            live_streams: Default::default(),
            refine_limits: Default::default(),
            public: Default::default(),
        }
    }
}

async fn harness(db: &str) -> Harness {
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
    let _ = pool.rows("DELETE FROM \"$capabilitygrant\"").await;

    let engine = Arc::new(ordo_host::deployment_engine(&f, &pg).expect("engine"));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "sharing-test",
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
    .expect("the fixture compiles no `std.space.*` name");

    let platform = Arc::new(PlatformOps::new(
        engine.clone(),
        grants.clone(),
        Arc::new(|| NOW),
    ));
    Harness {
        engine,
        pool,
        grants,
        deployment,
        pg,
        platform,
        fixture: f,
    }
}

fn space() -> ISpace {
    ISpace(DEFAULT_SPACE.to_string())
}

/// A window-shaped token: a capability grant listing `ops`, held by
/// `principal`. No session, no presence.
fn grant_token(engine: &AccessEngine, principal: &str, ops: &[&str], id: &str) -> String {
    let set: BTreeSet<String> = ops.iter().map(|s| s.to_string()).collect();
    let grant = engine
        .grant_for(GrantId(id.into()), &space(), set, IStamp(NOW + 3_600))
        .expect("every op named is deployed");
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: Some(grant),
            delegation: None,
            session: None,
        },
    )
}

/// A session-shaped token: what a person holds after signing in.
fn session_token(principal: &str) -> String {
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

async fn call(state: &ApiState, op: &str, bearer: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/op/{op}"))
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The rows an answer carries, without the row ids the wire adds.
fn rows(body: &Value) -> Value {
    let mut rows = body["rows"].clone();
    for r in rows.as_array_mut().into_iter().flatten() {
        if let Some(o) = r.as_object_mut() {
            o.remove("$id");
        }
    }
    rows
}

fn code(body: &Value) -> String {
    body["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ───────────── G1: the seat ─────────────

/// The seat the fixture gives Gil has no role. A session token for it
/// reaches no op by role: before G1 the engine read the `NULL` as a viewer,
/// and the viewer profile lists every read op, so the seat read every board
/// and every task the program serves. The same person through a grant reads
/// exactly the rows the acl row names, and `$caller` is the seat.
#[tokio::test(flavor = "multi_thread")]
async fn a_null_role_seat_binds_caller_and_reaches_no_op_by_role() {
    let h = harness("ordo_shr_g1").await;

    // The engine's own account of the seat.
    assert_eq!(
        h.engine.member_id_of(&space(), &PrincipalId(GUEST.into())),
        Some(GUEST),
        "the seat binds `$caller`"
    );
    let held = h.engine.spaces_of(&PrincipalId(GUEST.into()));
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].1, None, "a guest seat is listed with no role");
    assert!(
        !h.engine
            .membership_reaches(&space(), &PrincipalId(GUEST.into()), "boardList"),
        "a guest reaches nothing by membership"
    );

    // First half: a session token is refused at the door, for a read op the
    // viewer profile lists.
    let (st, body) = call(&h.state(), "boardList", &session_token(GUEST)).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied", "{body}");

    // Second half: the capability token listing the op reads the rows the
    // acl row admits, and only those.
    let t = grant_token(&h.engine, GUEST, &["boardList", "cardList"], "grn_g1");
    let (st, body) = call(&h.state(), "boardList", &t).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(rows(&body), serde_json::json!([{"name": "roadmap"}]));
    let (st, body) = call(&h.state(), "cardList", &t).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        serde_json::json!([
            {"title": "q3 goals", "done": false, "board": "board_1"},
            {"title": "q4 goals", "done": true,  "board": "board_1"},
        ])
    );

    // An op the grant does not list stays refused, however guarded.
    let (st, body) = call(&h.state(), "openTasks", &t).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
}

// ───────────── G2: `std.space.share` ─────────────

/// A ticket store that counts what is put in it, so a refusal can be shown
/// to have left nothing behind.
struct Counting {
    inner: MemoryTicketStore,
    inserts: AtomicUsize,
}

impl TicketStore for Counting {
    fn insert(&self, rec: &TicketRecord) -> Result<(), ordo_identity::StoreFault> {
        self.inserts.fetch_add(1, Ordering::SeqCst);
        self.inner.insert(rec)
    }
    fn take(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, ordo_identity::StoreFault> {
        self.inner.take(hash, now, want)
    }
    fn peek(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, ordo_identity::StoreFault> {
        self.inner.peek(hash, now, want)
    }
    fn decline(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, ordo_identity::StoreFault> {
        self.inner.decline(hash, now, want)
    }
    fn sweep(&self, now: u64) -> Result<u64, ordo_identity::StoreFault> {
        self.inner.sweep(now)
    }
}

const ANA: &str = "mem_1";
const CEILING: u64 = 365 * 86_400;

struct Sharing {
    ops: Arc<ShareOps>,
    store: Arc<Counting>,
}

impl Sharing {
    fn new(h: &Harness) -> Sharing {
        Sharing::build(h, None)
    }

    /// The same, with a recorder and what accepting needs: the door's engine
    /// and the fixture's executor to write acl rows through, as the sharer.
    fn accepting(h: &Harness, recorder: Arc<ordo_secevent::Recorder>) -> Sharing {
        Sharing::build(h, Some(recorder))
    }

    fn build(h: &Harness, recorder: Option<Arc<ordo_secevent::Recorder>>) -> Sharing {
        let store = Arc::new(Counting {
            inner: MemoryTicketStore::new(),
            inserts: AtomicUsize::new(0),
        });
        let tickets = Arc::new(Tickets::new(store.clone()).with_clock(Box::new(|| NOW)));
        let accept = recorder.as_ref().map(|_| {
            let engine = h.engine.clone();
            ordo_host::share_accept::AcceptWiring {
                engine: Arc::new(move || engine.clone()),
                acl: Arc::new(ordo_host::share_accept::ExecutorAcl {
                    executor: Arc::new(FixtureExecutor {
                        spaces: ordo_host::spaces::Spaces::new(h.deployment.clone()),
                        pg: h.pg.clone(),
                        page_size: 50,
                    }),
                    registry: h.deployment.clone(),
                }),
            }
        });
        Sharing {
            ops: Arc::new(ShareOps {
                tickets,
                stream: recorder,
                now: Arc::new(|| NOW),
                max_secs: Some(CEILING),
                grants: h.grants.clone(),
                members: Arc::new(MemberOps {
                    control: h.pool.clone(),
                    pools: Arc::new(ordo_exec::route::SinglePool::new(h.pool.clone())),
                    directory: None,
                    stream: None,
                    refresh: None,
                    now: Arc::new(|| NOW),
                    managed: None,
                }),
                accept,
            }),
            store,
        }
    }

    fn tickets_made(&self) -> usize {
        self.store.inserts.load(Ordering::SeqCst)
    }

    async fn share(
        &self,
        engine: &AccessEngine,
        caller: &str,
        params: &[(&str, &str)],
    ) -> Result<ordo_api::ExecResult, ordo_api::ExecError> {
        let map = params
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect();
        self.ops
            .execute(engine, SHARE, DEFAULT_SPACE, &BoundParams(map), caller)
            .await
    }
}

const ROWS: &str = r#"[{"entity":"boards","id":"board_1","level":"read"}]"#;

/// A share by a manager over two guarded reads answers a ticket that carries
/// the intent, and the ticket stops working when its life ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_share_ticket_carries_the_intent_and_expires() {
    let h = harness("ordo_shr_g2a").await;
    let s = Sharing::new(&h);

    let out = s
        .share(
            &h.engine,
            ANA,
            &[
                ("ops", "boardList, cardList"),
                ("rows", ROWS),
                ("expires", "7d"),
                ("label", "the roadmap"),
            ],
        )
        .await
        .expect("a manager may share guarded reads");
    let row = &out.rows[0];
    let ticket = row["ticket"].as_str().expect("the ticket");
    assert_eq!(row["expiresAt"], NOW + 7 * 86_400);
    assert_eq!(row["windowEndsAt"], NOW + 7 * 86_400);
    assert_eq!(s.tickets_made(), 1);

    let seen = s
        .ops
        .tickets
        .peek(ticket, &Want::default())
        .expect("a live ticket peeks");
    assert_eq!(seen.purpose, Purpose::Share);
    assert_eq!(seen.space.as_deref(), Some(DEFAULT_SPACE));
    assert_eq!(seen.display, "the roadmap");
    assert_eq!(seen.principal, None, "no recipient was named");
    let intent: Value = serde_json::from_str(seen.payload.as_deref().expect("payload")).unwrap();
    assert_eq!(intent["ops"], serde_json::json!(["boardList", "cardList"]));
    assert_eq!(
        intent["rows"],
        serde_json::json!([{"entity": "boards", "id": "board_1", "level": "read"}])
    );
    assert_eq!(intent["expiresAt"], NOW + 7 * 86_400);

    // The ticket never outlives the thirty-day cap, whatever the window.
    let long = s
        .share(
            &h.engine,
            ANA,
            &[("ops", "boardList"), ("rows", ROWS), ("expires", "300d")],
        )
        .await
        .expect("300 days is under the ceiling");
    assert_eq!(long.rows[0]["expiresAt"], NOW + MAX_TTL_SECS);
    assert_eq!(long.rows[0]["windowEndsAt"], NOW + 300 * 86_400);

    // At the ticket's expiry it is one refusal, like every ticket.
    let later = Tickets::new(s.store.clone()).with_clock(Box::new(|| NOW + 7 * 86_400));
    assert!(later.peek(ticket, &Want::default()).is_err());
}

/// The refusals, each before a ticket exists: a platform op, an unguarded op,
/// an op with no reachable guard, a write, an op beyond the sharer, an op the
/// program does not have, a window past the ceiling, a write level, a
/// recipient nobody has seated.
#[tokio::test(flavor = "multi_thread")]
async fn sharing_an_unguarded_op_a_platform_op_or_an_op_beyond_the_sharer_is_refused_before_a_ticket_exists(
) {
    let h = harness("ordo_shr_g2b").await;
    let s = Sharing::new(&h);

    let cases: [(&str, &str, &str, &str, &str); 8] = [
        (ANA, "std.space.callersOf", "7d", ROWS, "share_platform_op"),
        (ANA, "allTasks", "7d", ROWS, "guest_exposure_unguarded"),
        (
            ANA,
            "activeProducts",
            "7d",
            ROWS,
            "guest_exposure_unguarded",
        ),
        (ANA, "addCard", "7d", ROWS, "share_read_only"),
        // Gil holds a seat and no role, so reaches no op by membership.
        (GUEST, "boardList", "7d", ROWS, "share_beyond_sharer"),
        (ANA, "noSuchOp", "7d", ROWS, "share_unknown_op"),
        (ANA, "boardList", "400d", ROWS, "share_bad_expiry"),
        (
            ANA,
            "boardList",
            "7d",
            r#"[{"entity":"boards","id":"board_1","level":"write"}]"#,
            "share_bad_rows",
        ),
    ];
    for (who, op, expires, rows, want) in cases {
        let e = s
            .share(
                &h.engine,
                who,
                &[("ops", op), ("rows", rows), ("expires", expires)],
            )
            .await
            .expect_err(&format!("`{op}` must be refused"));
        assert!(
            e.to_string().contains(want),
            "`{op}` as {who}: wanted {want}, got {e}"
        );
        assert_eq!(s.tickets_made(), 0, "`{op}` left a ticket behind");
        if want == "share_bad_expiry" {
            // Audit 31: the refusal names the setting that holds the ceiling.
            assert!(e.to_string().contains("ORDO_SHARE_MAX_SECS"), "{e}");
        }
    }

    // A recipient must already hold a seat in this tenant.
    let e = s
        .share(
            &h.engine,
            ANA,
            &[
                ("ops", "boardList"),
                ("rows", ROWS),
                ("expires", "7d"),
                ("to", "usr_nobody"),
            ],
        )
        .await
        .expect_err("nobody by that name has a seat");
    assert!(e.to_string().contains("share_unknown_recipient"), "{e}");
    assert_eq!(s.tickets_made(), 0);

    // Naming Gil, who holds a seat, is allowed and binds the ticket to him.
    let out = s
        .share(
            &h.engine,
            ANA,
            &[
                ("ops", "boardList"),
                ("rows", ROWS),
                ("expires", "7d"),
                ("to", GUEST),
            ],
        )
        .await
        .expect("a seated recipient");
    assert_eq!(s.tickets_made(), 1);
    let t = out.rows[0]["ticket"].as_str().unwrap();
    let by = |who| Want {
        holder: Some(who),
        ..Default::default()
    };
    assert!(s.ops.tickets.peek(t, &by("usr_other")).is_err());
    assert!(s.ops.tickets.peek(t, &by(GUEST)).is_ok());

    // Who may share: a manager and an owner do; a viewer and a guest do not.
    let reaches = |who: &str| {
        h.engine
            .membership_reaches(&space(), &PrincipalId(who.into()), SHARE)
    };
    assert!(reaches(ANA), "a manager reaches it");
    assert!(reaches("mem_o"), "an owner reaches it");
    assert!(!reaches("mem_4"), "a viewer does not");
    assert!(!reaches(GUEST), "a guest does not");
}

// ───────────── G5: revocation ─────────────

/// Record a share grant for `principal` over `ops`, as `register_share`
/// builds it, and answer the window token that carries it.
async fn open_window(h: &Harness, id: &str, principal: &str, ops: &[&str]) -> String {
    let token = grant_token(&h.engine, principal, ops, id);
    let pins = h
        .engine
        .grant_for(
            GrantId(id.into()),
            &space(),
            ops.iter().map(|s| s.to_string()).collect(),
            IStamp(NOW + 3_600),
        )
        .expect("deployed")
        .pins()
        .clone();
    let grant = register_share(
        GrantId(id.into()),
        space(),
        PrincipalId(principal.into()),
        pins,
        IStamp(NOW),
        IStamp(NOW + 3_600),
    )
    .expect("guarded reads are shareable");
    h.grants.record(&grant).await.expect("recorded");
    token
}

async fn count(h: &Harness, sql: &str) -> i64 {
    h.pool.rows(sql).await.expect("rows")[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

async fn seat_status(h: &Harness, seat: &str) -> String {
    h.pool
        .rows(&format!(
            r#"SELECT "status" FROM "members" WHERE "id" = '{seat}'"#
        ))
        .await
        .expect("rows")[0][0]
        .clone()
        .unwrap()
}

fn params(pairs: &[(&str, &str)]) -> BoundParams {
    BoundParams(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect(),
    )
}

/// The sharer closes the window: the very next call with the same token,
/// unexpired and correctly signed, is refused; the acl rows the shared ops'
/// entities held for the seat are gone; the seat is suspended. A second
/// revocation finds nothing to do.
///
/// Left to G4: that no window can then be reminted (`/auth/window` is in
/// `auth.rs`). What it will rely on is asserted here, that the registry no
/// longer holds the grant live.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_share_is_refused_at_the_next_call() {
    let h = harness("ordo_shr_g5a").await;
    let s = Sharing::new(&h);
    let t = open_window(&h, "shr_g5a", GUEST, &["boardList", "cardList"]).await;

    let (st, body) = call(&h.state(), "boardList", &t).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(rows(&body), serde_json::json!([{"name": "roadmap"}]));
    assert_eq!(
        count(
            &h,
            r#"SELECT count(*) FROM "boardsAccess" WHERE "member" = 'mem_g'"#
        )
        .await,
        1
    );

    let out = s
        .ops
        .execute(
            &h.engine,
            REVOKE_SHARE,
            DEFAULT_SPACE,
            &params(&[("seat", GUEST)]),
            ANA,
        )
        .await
        .expect("a manager closes the window");
    let row = &out.rows[0];
    assert_eq!(row["grants"], 1);
    assert_eq!(row["aclRows"], 1);
    assert_eq!(row["suspended"], true);

    // The same bytes, still inside their expiry, refused at the next call.
    for op in ["boardList", "cardList"] {
        let (st, body) = call(&h.state(), op, &t).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{op}: {body}");
        assert_eq!(code(&body), "access_denied", "{op}: {body}");
    }
    assert_eq!(
        count(
            &h,
            r#"SELECT count(*) FROM "boardsAccess" WHERE "member" = 'mem_g'"#
        )
        .await,
        0
    );
    assert_eq!(seat_status(&h, GUEST).await, "suspended");
    assert!(
        !h.grants
            .is_live(&space(), &GrantId("shr_g5a".into()), IStamp(NOW))
            .await
            .unwrap(),
        "the registry no longer holds the grant live"
    );

    // Revoking again is not an error and changes nothing.
    let again = s
        .ops
        .execute(
            &h.engine,
            REVOKE_SHARE,
            DEFAULT_SPACE,
            &params(&[("seat", GUEST)]),
            ANA,
        )
        .await
        .expect("idempotent");
    assert_eq!(again.rows[0]["grants"], 0);

    // A member who holds a role keeps the membership and their own rows:
    // only the share grant is closed.
    let t2 = open_window(&h, "shr_g5b", "mem_2", &["boardList"]).await;
    let (st, body) = call(&h.state(), "boardList", &t2).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let out = s
        .ops
        .execute(
            &h.engine,
            REVOKE_SHARE,
            DEFAULT_SPACE,
            &params(&[("seat", "mem_2")]),
            ANA,
        )
        .await
        .expect("a member's share can be closed too");
    assert_eq!(out.rows[0]["grants"], 1);
    assert_eq!(out.rows[0]["suspended"], false);
    assert_eq!(seat_status(&h, "mem_2").await, "active");
    assert_eq!(
        count(
            &h,
            r#"SELECT count(*) FROM "boardsAccess" WHERE "member" = 'mem_2'"#
        )
        .await,
        2,
        "the member's own acl rows stay"
    );
    let (st, _) = call(&h.state(), "boardList", &t2).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // An unknown seat is one answer.
    let e = s
        .ops
        .execute(
            &h.engine,
            REVOKE_SHARE,
            DEFAULT_SPACE,
            &params(&[("seat", "mem_nobody")]),
            ANA,
        )
        .await
        .expect_err("no such seat");
    assert_eq!(e.to_string(), "not_found: this tenant has no such seat");
}

/// The guest closes their own window. The route that carries it is G3's and
/// G5's `POST /auth/share/leave` in `auth.rs`; what it calls is `leave`,
/// which is tested here.
#[tokio::test(flavor = "multi_thread")]
async fn a_guest_who_leaves_is_refused_at_the_next_call() {
    let h = harness("ordo_shr_g5c").await;
    let s = Sharing::new(&h);
    let t = open_window(&h, "shr_g5c", GUEST, &["boardList", "cardList"]).await;
    let (st, body) = call(&h.state(), "boardList", &t).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    let out = s
        .ops
        .leave(DEFAULT_SPACE, GUEST)
        .await
        .expect("a guest leaves");
    assert_eq!(out.rows[0]["outcome"], "share left");
    assert_eq!(out.rows[0]["grants"], 1);

    let (st, body) = call(&h.state(), "boardList", &t).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        count(
            &h,
            r#"SELECT count(*) FROM "boardsAccess" WHERE "member" = 'mem_g'"#
        )
        .await,
        0
    );
    assert_eq!(seat_status(&h, GUEST).await, "suspended");

    // A person with no seat here has nothing to leave.
    let e = s
        .ops
        .leave(DEFAULT_SPACE, "usr_stranger")
        .await
        .expect_err("no seat");
    assert_eq!(e.to_string(), "not_found: this tenant has no such seat");
}

/// A registry nothing can reach.
struct Down;

#[async_trait::async_trait]
impl GrantRegistry for Down {
    async fn record(&self, _: &NewGrant) -> Result<(), GrantError> {
        Err(GrantError::Unavailable("down".into()))
    }
    async fn revoke(&self, _: &ISpace, _: &GrantId, _: IStamp) -> Result<(), GrantError> {
        Err(GrantError::Unavailable("down".into()))
    }
    async fn callers_of(
        &self,
        _: &ISpace,
        _: &str,
        _: IStamp,
    ) -> Result<Vec<GrantRecord>, GrantError> {
        Err(GrantError::Unavailable("down".into()))
    }
    async fn grants_of(&self, _: &ISpace, _: &PrincipalId) -> Result<Vec<GrantRecord>, GrantError> {
        Err(GrantError::Unavailable("down".into()))
    }
    async fn is_live(&self, _: &ISpace, _: &GrantId, _: IStamp) -> Result<bool, GrantError> {
        Err(GrantError::Unavailable("down".into()))
    }
}

/// With the grant registry out of reach, window calls are refused and
/// nothing else is: members with a session, and an ordinary capability
/// grant, are served as before. The refusal is a retryable 503, never a
/// denial: a lookup that could not run is not an answer (77e6e3de).
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_grant_registry_refuses_window_calls_and_serves_members() {
    let h = harness("ordo_shr_g5d").await;
    let window = open_window(&h, "shr_g5d", GUEST, &["boardList"]).await;
    let ordinary = grant_token(&h.engine, GUEST, &["boardList"], "grn_g5d");
    let down = h.state_with(Arc::new(Down));

    let (st, body) = call(&down, "boardList", &window).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(code(&body), "server_busy", "{body}");

    let (st, body) = call(&down, "boardList", &session_token(ANA)).await;
    assert_eq!(st, StatusCode::OK, "members are served: {body}");
    let (st, body) = call(&down, "boardList", &ordinary).await;
    assert_eq!(
        st,
        StatusCode::OK,
        "an ordinary grant is not asked about: {body}"
    );

    // The same window, with the registry back, is served.
    let (st, body) = call(&h.state(), "boardList", &window).await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

// ───────────── G8: the guest-exposure corpus, GC063 to GC065 ─────────────

/// GC063 to GC065, which `guards_corpus.rs` could not run while sharing was
/// a `share to guest:` line in source that does not exist. They run here,
/// over the registration the host does: `share_ops::validate`, which calls
/// `register_share` and adds the reachable-guard rule. No database is
/// needed: the engine is built from the deployed program and one manager.
///
/// - **GC063** an op with no reachable guard (no read guard and no `$caller`
///   condition on any entity it touches) is refused `guest_exposure_unguarded`.
///   The corpus names `vendorNames`; the fixture has no such op, and
///   `activeProducts` is the same shape (an unguarded entity, no `$caller`).
/// - **GC064** an `unguarded`-marked op is refused with the same code.
/// - **GC065** guard-bearing ops are accepted and the basis is recorded per
///   op from the pins: an entity guard, an inherited acl guard, and a
///   `$caller` condition beside an entity guard.
#[test]
fn gc063_gc064_gc065_registration() {
    use ordo_host::share_ops::{validate, ShareRefusal, Timing};

    let f = Fixture::load().expect("fixture");
    let rows = vec![[DEFAULT_SPACE, "mem_1", "manager", "active", "mem_1"]
        .into_iter()
        .map(|s| Some(s.to_string()))
        .collect::<Vec<_>>()];
    let engine = ordo_host::engine_from_members(&f, &rows).expect("engine");
    let sharer = PrincipalId("mem_1".into());
    let timing = Timing {
        now: NOW,
        lifetime_secs: 86_400,
        max_secs: Some(CEILING),
    };
    let share = |ops: &[&str]| {
        validate(
            &engine,
            &space(),
            &sharer,
            &ops.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &[],
            timing,
        )
    };

    // GC063: no reachable guard.
    let e = share(&["activeProducts"]).expect_err("GC063");
    assert_eq!(e, ShareRefusal::NoReachableGuard("activeProducts".into()));
    assert_eq!(e.code(), "guest_exposure_unguarded");

    // GC064: an unguarded-marked op.
    let e = share(&["allTasks"]).expect_err("GC064");
    assert_eq!(e, ShareRefusal::Unguarded("allTasks".into()));
    assert_eq!(e.code(), "guest_exposure_unguarded");

    // One bad op among good ones refuses the whole share.
    let e = share(&["openTasks", "allTasks"]).expect_err("the share is one act");
    assert_eq!(e.code(), "guest_exposure_unguarded");

    // GC065: accepted, with the basis per op.
    let ok = share(&["openTasks", "cardList", "myAssignedTasks"]).expect("GC065");
    assert_eq!(ok.ops, ["openTasks", "cardList", "myAssignedTasks"]);
    let basis = &ok.basis;
    let has = |v: &[String], e: &str| v.iter().any(|x| x == e);
    // An entity guard: `tasks` reads through its own walk.
    assert!(has(&basis["openTasks"].guarded, "tasks"));
    assert!(basis["openTasks"].caller_restricted.is_empty());
    // An inherited acl guard: the cards' read walk goes through boardsAccess.
    assert!(has(&basis["cardList"].guarded, "cards"));
    let pin = &ok.pins["cardList"];
    let cards = pin
        .entities
        .iter()
        .find(|e| e.entity == "cards")
        .expect("cardList reads cards");
    let walk = match &cards.read_guard {
        ordo_identity::GuardPin::Walk(w) => w.clone(),
        other => panic!("cardList's read guard is a walk, not {other:?}"),
    };
    assert!(walk.contains("boardsAccess"), "the acl guard: {walk}");
    // A `$caller` condition, beside the entity guard.
    assert!(has(&basis["myAssignedTasks"].caller_restricted, "tasks"));
    assert!(has(&basis["myAssignedTasks"].guarded, "tasks"));
    // The grant's pins are the ones the deployed program has.
    for (op, pin) in &ok.pins {
        assert_eq!(Some(pin), engine.deployed().get(op));
    }
}

/// A share that reads a row by `std.<entity>.get` carries the file door's
/// own check, `std.$blob.get`, as a share link always did. Before
/// 2026-10-22 only a link did, so a window guest was refused the file of a
/// note they could read. Validating the stored ops again, which an accept
/// does, answers the same ops: the carried check is never refused as an op
/// with no reachable guard. A share with no `std.<entity>.get` carries
/// nothing.
#[test]
fn a_share_over_a_std_get_carries_the_file_check_and_accepts_again() {
    use ordo_host::share_ops::{validate, Timing};

    let f = Fixture::load().expect("fixture");
    let rows = vec![[DEFAULT_SPACE, "mem_1", "manager", "active", "mem_1"]
        .into_iter()
        .map(|s| Some(s.to_string()))
        .collect::<Vec<_>>()];
    let engine = ordo_host::engine_from_members(&f, &rows).expect("engine");
    let sharer = PrincipalId("mem_1".into());
    let timing = Timing {
        now: NOW,
        lifetime_secs: 86_400,
        max_secs: Some(CEILING),
    };
    let share = |ops: &[String]| validate(&engine, &space(), &sharer, ops, &[], timing);
    let blob = ordo_api::blob::BLOB_READ_OP.to_string();

    // Any guarded `std.<entity>.get` the fixture deploys.
    let get = engine
        .deployed()
        .keys()
        .filter(|o| o.starts_with("std.") && o.ends_with(".get") && **o != blob)
        .find(|o| share(&[(*o).clone(), "openTasks".to_string()]).is_ok())
        .cloned()
        .expect("the fixture deploys a shareable std get");

    let ok = share(&[get.clone(), "openTasks".to_string()]).expect("a share over a get");
    assert!(
        ok.ops.contains(&blob),
        "the file check is carried: {:?}",
        ok.ops
    );
    assert_eq!(ok.pins.get(&blob), engine.deployed().get(&blob));

    // The accept validates the ops the share stored, the carried one among them.
    let again = share(&ok.ops).expect("the stored ops validate again");
    assert_eq!(again.ops, ok.ops);

    // Listing it by hand without a get opens nothing.
    let plain = share(&["openTasks".to_string(), blob.clone()]).expect("a plain share");
    assert_eq!(plain.ops, ["openTasks"]);
}

// ───────────── G3: accept ─────────────

use ordo_host::auth::AuthState;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{MemorySessionRevocations, ProviderRegistry, SessionClaim};
use ordo_secevent::{EventKind, Scope};

/// What a person holds after signing in: a token with a session and no
/// presence. Accepting a share needs no presence.
fn session_of(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: Some(SessionClaim::starting_at(IStamp(NOW))),
        },
    )
}

/// The auth routes over a deployment that accepts shares.
fn guest_app(s: &Sharing, recorder: &Arc<ordo_secevent::Recorder>) -> axum::Router {
    let mut state = AuthState::new(
        ProviderRegistry::new(),
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(recorder.clone());
    state.now = Box::new(|| NOW);
    state.wire_sharing(s.ops.clone());
    ordo_host::auth::router(Some(Arc::new(state)))
}

async fn post_json(
    app: &axum::Router,
    path: &str,
    body: Value,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let res = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Ana, a manager who administers `board_1`, shares it, and answers the
/// ticket.
async fn issue(s: &Sharing, h: &Harness, params: &[(&str, &str)]) -> String {
    let out = s
        .share(&h.engine, ANA, params)
        .await
        .expect("a manager who administers the row may share it");
    out.rows[0]["ticket"].as_str().expect("ticket").to_string()
}

async fn table(h: &Harness, sql: &str) -> Vec<Vec<Option<String>>> {
    h.pool.rows(sql).await.expect("query")
}

fn consent_scopes(recorder: &ordo_secevent::Recorder) -> Vec<String> {
    recorder
        .drain_all(ordo_commandlog::Timestamp(NOW + 10))
        .into_iter()
        .filter(|e| matches!(&e.kind, EventKind::Enrolment { act, .. } if act == "share-accepted"))
        .map(|e| match e.scope {
            Scope::Deployment => "deployment".to_string(),
            Scope::Space(s) => format!("space:{}", s.0),
        })
        .collect()
}

/// A guest accepts a share. The ticket is looked at without being spent; on
/// accept the guest holds a seat with no role under the name they typed, the
/// acl row is written as the sharer and lands in the sharer's chain, the
/// grant is recorded and live, and the consent is in the deployment's
/// security log and in the sharer's.
#[tokio::test(flavor = "multi_thread")]
async fn an_accepted_share_binds_a_seat_records_the_grant_and_writes_the_acl_rows_as_the_sharer() {
    let h = harness("ordo_shr_g3a").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let ticket = issue(
        &s,
        &h,
        &[
            ("ops", "boardList, cardList"),
            ("rows", ROWS),
            ("expires", "7d"),
            ("label", "the roadmap"),
        ],
    )
    .await;
    let app = guest_app(&s, &recorder);

    // What the page shows, with no token, and without spending the ticket.
    for _ in 0..2 {
        let (st, peek) = post_json(&app, "/auth/share/peek", json!({"ticket": ticket}), None).await;
        assert_eq!(st, StatusCode::OK, "{peek}");
        assert_eq!(peek["space"], DEFAULT_SPACE);
        assert_eq!(peek["label"], "the roadmap");
        assert_eq!(peek["ops"], json!(["boardList", "cardList"]));
        assert_eq!(
            peek["rows"],
            json!([{"entity": "boards", "id": "board_1", "level": "read"}])
        );
        assert_eq!(peek["window_ends_at"], NOW + 7 * 86_400);
        assert_eq!(peek["named"], false);
        assert!(peek["notice"].as_str().unwrap().contains("owner"), "{peek}");
        assert!(peek.get("issued_by").is_none(), "no member ids: {peek}");
    }

    // Accepting needs a person's session.
    let (st, _) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Gil B."}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    let (st, out) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Gil B."}),
        Some(&session_of("usr_gil")),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let seat = out["seat"].as_str().expect("seat").to_string();
    let grant = out["grant"].as_str().expect("grant").to_string();
    assert!(grant.starts_with("shr_"), "{grant}");
    assert_eq!(out["space"], DEFAULT_SPACE);
    assert_eq!(out["expires"], NOW + 7 * 86_400);

    // The seat: no role, active, the name the guest typed.
    let seats = table(
        &h,
        &format!(
            "SELECT \"role\", \"status\", \"displayName\", \"principal\" FROM \"members\" \
             WHERE \"id\" = '{seat}'"
        ),
    )
    .await;
    assert_eq!(
        seats,
        vec![vec![
            None,
            Some("active".into()),
            Some("Gil B.".into()),
            Some("usr_gil".into())
        ]]
    );

    // The acl row, at the level the share names, for the seat.
    let acl = table(
        &h,
        &format!("SELECT \"level\" FROM \"boardsAccess\" WHERE \"board\" = 'board_1' AND \"member\" = '{seat}'"),
    )
    .await;
    assert_eq!(acl, vec![vec![Some("read".to_string())]]);

    // Written as the sharer: the newest group in the tenant's chain names
    // Ana's member id as its actor.
    let actor = table(
        &h,
        &format!(
            "SELECT \"actor\" FROM \"$commandlog\" WHERE \"space\" = '{DEFAULT_SPACE}' \
             ORDER BY \"idx\" DESC LIMIT 1"
        ),
    )
    .await;
    let actor = actor[0][0].clone().unwrap_or_default();
    assert!(
        actor.contains(ANA),
        "the acl row is the sharer's act: {actor}"
    );

    // The grant is in the registry, live, one per guest and tenant.
    assert!(h
        .grants
        .is_live(&space(), &GrantId(grant.clone()), IStamp(NOW))
        .await
        .expect("registry"));
    let held = h
        .grants
        .grants_of(&space(), &PrincipalId("usr_gil".into()))
        .await
        .expect("grants_of");
    let ops: BTreeSet<String> = held.iter().map(|r| r.op.clone()).collect();
    assert_eq!(
        ops,
        BTreeSet::from(["boardList".to_string(), "cardList".to_string()])
    );

    // The consent: once in the deployment's stream and once in the sharer's.
    let mut scopes = consent_scopes(&recorder);
    scopes.sort();
    assert_eq!(
        scopes,
        vec!["deployment".to_string(), format!("space:{DEFAULT_SPACE}")]
    );

    // Single use: the same ticket is refused, and says nothing more.
    let (st, again) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Gil B."}),
        Some(&session_of("usr_hal")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{again}");
    assert_eq!(again["error"]["code"], "share_refused");
}

/// A second share to the same person reuses the seat, and the grant is
/// replaced by one for both shares' ops and the later end. The first grant
/// is revoked, so the person holds exactly one live grant in the tenant.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_share_to_the_same_person_reuses_the_seat_and_replaces_the_grant() {
    let h = harness("ordo_shr_g3b").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    let gil = session_of("usr_gil");

    let first = issue(
        &s,
        &h,
        &[("ops", "boardList"), ("rows", ROWS), ("expires", "7d")],
    )
    .await;
    let (st, one) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": first, "display_name": "Gil"}),
        Some(&gil),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{one}");

    let second = issue(
        &s,
        &h,
        &[("ops", "cardList"), ("rows", ROWS), ("expires", "30d")],
    )
    .await;
    let (st, two) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": second, "display_name": "Gilbert"}),
        Some(&gil),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{two}");

    assert_eq!(one["seat"], two["seat"], "one seat per person per tenant");
    assert_ne!(one["grant"], two["grant"]);
    assert_eq!(two["expires"], NOW + 30 * 86_400, "the later end");
    assert_eq!(
        two["ops"],
        json!(["boardList", "cardList"]),
        "the union of the two shares' ops"
    );
    let seats = table(
        &h,
        "SELECT count(*) FROM \"members\" WHERE \"principal\" = 'usr_gil'",
    )
    .await;
    assert_eq!(seats[0][0].as_deref(), Some("1"));
    let name = table(
        &h,
        "SELECT \"displayName\" FROM \"members\" WHERE \"principal\" = 'usr_gil'",
    )
    .await;
    assert_eq!(
        name[0][0].as_deref(),
        Some("Gilbert"),
        "the name they consent to now"
    );

    // Exactly one live share grant, and it is the second.
    let held = h
        .grants
        .grants_of(&space(), &PrincipalId("usr_gil".into()))
        .await
        .expect("grants_of");
    let live: BTreeSet<String> = held
        .iter()
        .filter(|r| r.is_live(IStamp(NOW)))
        .map(|r| r.id.0.clone())
        .collect();
    assert_eq!(
        live,
        BTreeSet::from([two["grant"].as_str().unwrap().to_string()])
    );
    assert!(!h
        .grants
        .is_live(
            &space(),
            &GrantId(one["grant"].as_str().unwrap().into()),
            IStamp(NOW)
        )
        .await
        .expect("registry"));
}

/// A ticket that names a person is that person's. Somebody else presenting
/// it is refused and does not spend it, so the person it was for can still
/// accept.
#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_naming_another_person_is_not_spent_by_the_wrong_holder() {
    let h = harness("ordo_shr_g3c").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    // Gil already holds a seat in this tenant (`mem_g`), so a share may name
    // him.
    let ticket = issue(
        &s,
        &h,
        &[
            ("ops", "boardList"),
            ("rows", ROWS),
            ("expires", "7d"),
            ("to", GUEST),
        ],
    )
    .await;

    let (st, out) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Mallory"}),
        Some(&session_of("usr_mallory")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
    let nobody = table(
        &h,
        "SELECT count(*) FROM \"members\" WHERE \"principal\" = 'usr_mallory'",
    )
    .await;
    assert_eq!(
        nobody[0][0].as_deref(),
        Some("0"),
        "no seat for the wrong holder"
    );

    // The ticket is whole: the person it names accepts it.
    let (st, out) = post_json(&app, "/auth/share/peek", json!({"ticket": ticket}), None).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["named"], true);
    let (st, out) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Gil"}),
        Some(&session_of(GUEST)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["seat"], GUEST, "his existing seat, reused");
}

/// A person who is already a member is not given a seat. The share adds a
/// grant to the row they hold, and an acl row they already hold is left at
/// the level they have: Bram writes `board_1`, and a `read` share does not
/// take that away.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_who_is_shared_with_gets_no_second_seat() {
    let h = harness("ordo_shr_g3d").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    let ticket = issue(
        &s,
        &h,
        &[("ops", "boardList"), ("rows", ROWS), ("expires", "7d")],
    )
    .await;

    // `mem_2` is Bram, an editor whose principal is his member id.
    let (st, out) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Bram"}),
        Some(&session_of("mem_2")),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["seat"], "mem_2", "the row he already holds");
    let rows = table(
        &h,
        "SELECT \"id\", \"role\" FROM \"members\" WHERE \"principal\" = 'mem_2'",
    )
    .await;
    assert_eq!(
        rows,
        vec![vec![Some("mem_2".to_string()), Some("editor".to_string())]],
        "one row, and his role is untouched"
    );
    let level = table(
        &h,
        "SELECT \"level\" FROM \"boardsAccess\" WHERE \"board\" = 'board_1' AND \"member\" = 'mem_2'",
    )
    .await;
    assert_eq!(level, vec![vec![Some("write".to_string())]], "not lowered");
    assert!(h
        .grants
        .is_live(
            &space(),
            &GrantId(out["grant"].as_str().unwrap().into()),
            IStamp(NOW)
        )
        .await
        .expect("registry"));
}

// ───────────── G4: the window token ─────────────

/// A call in a named tenant, as the door sees the `x-ordo-space` header.
async fn call_in(state: &ApiState, space: &str, op: &str, bearer: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/op/{op}"))
        .header("x-ordo-space", space)
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The engine the door would rebuild after the accept: read from the
/// tenant's `members`, so it holds the seat the accept wrote.
fn engine_now(h: &Harness) -> ordo_identity::AccessEngine {
    ordo_host::deployment_engine(&h.fixture, &h.pg).expect("engine")
}

/// Ana shares `board_1`; `guest` accepts; the guest opens the window.
/// Answers the window token and what the window route said.
async fn window_for(h: &Harness, s: &Sharing, app: &axum::Router, guest: &str) -> (String, Value) {
    let ticket = issue(
        s,
        h,
        &[
            ("ops", "boardList, cardList"),
            ("rows", ROWS),
            ("expires", "7d"),
        ],
    )
    .await;
    let (st, out) = post_json(
        app,
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Guest"}),
        Some(&session_of(guest)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let (st, w) = post_json(
        app,
        "/auth/window",
        json!({"space": DEFAULT_SPACE}),
        Some(&session_of(guest)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{w}");
    (w["token"].as_str().expect("token").to_string(), w)
}

/// The window a guest opens after accepting reads exactly the rows the acl
/// row names, through the ordinary door, with `$caller` the seat. It is
/// bound to the one tenant, and no other tenant has a window for them.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_token_reads_exactly_the_shared_rows_and_is_refused_in_any_other_space() {
    let h = harness("ordo_shr_g4a").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    let (token, w) = window_for(&h, &s, &app, "usr_gil").await;

    // The token ends at one session life, though the grant has a week.
    assert_eq!(w["expires_at"], NOW + 3_600, "{w}");
    assert_eq!(w["space"], DEFAULT_SPACE);
    assert_eq!(w["ops"], json!(["boardList", "cardList"]));

    let state = h.state_over(Arc::new(engine_now(&h)), h.grants.clone());
    let (st, body) = call(&state, "boardList", &token).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        json!([{"name": "roadmap"}]),
        "board_2 is not theirs"
    );
    let (st, body) = call(&state, "cardList", &token).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        json!([
            {"title": "q3 goals", "done": false, "board": "board_1"},
            {"title": "q4 goals", "done": true,  "board": "board_1"},
        ]),
        "board_2's card is not theirs"
    );

    // An op the share does not list is refused, however guarded.
    let (st, body) = call(&state, "openTasks", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");

    // Another tenant: the token is bound to one, and the route has no grant
    // there to answer.
    let (st, body) = call_in(&state, "spc_elsewhere", "boardList", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    let (st, none) = post_json(
        &app,
        "/auth/window",
        json!({"space": "spc_elsewhere"}),
        Some(&session_of("usr_gil")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{none}");
    assert_eq!(none["error"]["code"], "share_refused");

    // A person nobody shared with has no window to open.
    let (st, none) = post_json(
        &app,
        "/auth/window",
        json!({"space": DEFAULT_SPACE}),
        Some(&session_of("usr_stranger")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{none}");
}

/// The window carries what the guest consented to, not what the deployment
/// says today. The op is widened after the accept (its guards bypassed); a
/// token minted from the widened deployment would pass, and the window
/// refuses, with `ManifestChanged` in the security stream.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_token_carries_the_consented_pins_so_a_widened_op_refuses_the_window() {
    let h = harness("ordo_shr_g4b").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    let (token, _) = window_for(&h, &s, &app, "usr_gil").await;

    // The deployment as it is at the accept: the window works.
    let now = Arc::new(engine_now(&h));
    let door = h.door_over(now.clone(), h.grants.clone());
    let (st, body) = call(&h.state_from(&door), "boardList", &token).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Redeployed with the op's guards bypassed.
    let mut widened = engine_now(&h);
    let mut pin = widened.deployed()["boardList"].clone();
    pin.unguarded = true;
    widened.deploy_op("boardList", pin);
    let widened = Arc::new(widened);
    let door = h.door_over(widened.clone(), h.grants.clone());
    let (st, body) = call(&h.state_from(&door), "boardList", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    let seen = format!(
        "{:?}",
        door.stream.drain_all(ordo_commandlog::Timestamp(NOW))
    );
    assert!(seen.contains("ManifestChanged"), "{seen}");

    // What re-pinning from the widened deployment would have done: the
    // guest's consent would have been silently widened with it.
    let repinned = grant_token(&widened, "usr_gil", &["boardList"], "grn_repinned");
    let (st, body) = call(&h.state_from(&door), "boardList", &repinned).await;
    assert_eq!(
        st,
        StatusCode::OK,
        "the failure the registry pins prevent: {body}"
    );
}

/// A window is not administration and cannot become a session. No platform
/// op is reachable through it, step-up is out of its reach, and it opens no
/// window of its own and accepts no share.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_token_cannot_step_up_or_call_a_platform_op() {
    let h = harness("ordo_shr_g4c").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    let (token, _) = window_for(&h, &s, &app, "usr_gil").await;
    let state = h.state_over(Arc::new(engine_now(&h)), h.grants.clone());

    for op in ["std.space.callersOf", "std.person.tenants"] {
        let (st, body) = call(&state, op, &token).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{op}: {body}");
        assert_eq!(code(&body), "access_denied", "{op}: {body}");
    }

    // The routes that want a person's session refuse a window.
    for (path, body) in [
        ("/auth/window", json!({"space": DEFAULT_SPACE})),
        (
            "/auth/share/accept",
            json!({"ticket": "tkt_x", "display_name": "x"}),
        ),
        ("/auth/step-up/begin", json!({})),
    ] {
        let (st, out) = post_json(&app, path, body, Some(&token)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{path}: {out}");
    }
}

/// A window cannot be re-minted once the sharer closes it, and the token the
/// guest already holds is refused at the next call.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_share_cannot_be_reminted_as_a_window() {
    let h = harness("ordo_shr_g4d").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    let (token, _) = window_for(&h, &s, &app, "usr_gil").await;
    let seat = h
        .pool
        .rows("SELECT \"id\" FROM \"members\" WHERE \"principal\" = 'usr_gil'")
        .await
        .expect("seat")[0][0]
        .clone()
        .unwrap();

    s.ops
        .execute(
            &h.engine,
            REVOKE_SHARE,
            DEFAULT_SPACE,
            &params(&[("seat", &seat)]),
            ANA,
        )
        .await
        .expect("the sharer closes the window");
    // G12: the revocation is on the record, in the deployment's stream and
    // the tenant's (console step 9's guard reads this test).
    assert_eq!(
        act_scopes(&recorder, "share-revoked"),
        vec!["deployment".to_string(), format!("space:{DEFAULT_SPACE}")]
    );

    let (st, out) = post_json(
        &app,
        "/auth/window",
        json!({"space": DEFAULT_SPACE}),
        Some(&session_of("usr_gil")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["error"]["code"], "share_refused");

    let state = h.state_over(Arc::new(engine_now(&h)), h.grants.clone());
    let (st, body) = call(&state, "boardList", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
}

// ───────────── G5: leave; G6: the guest on the person's own list ─────────────

/// The security events of one act, by the scope they landed in.
fn act_scopes(recorder: &ordo_secevent::Recorder, wanted: &str) -> Vec<String> {
    let mut scopes: Vec<String> = recorder
        .drain_all(ordo_commandlog::Timestamp(NOW + 10))
        .into_iter()
        .filter(|e| matches!(&e.kind, EventKind::Enrolment { act, .. } if act == wanted))
        .map(|e| match e.scope {
            Scope::Deployment => "deployment".to_string(),
            Scope::Space(s) => format!("space:{}", s.0),
        })
        .collect();
    scopes.sort();
    scopes
}

/// The auth routes with the tenant list wired over an engine rebuilt from
/// the tenant's `members` on every read, as the runtime's live engine is.
fn listing_app(h: &Harness, s: &Sharing, recorder: &Arc<ordo_secevent::Recorder>) -> axum::Router {
    let mut state = AuthState::new(
        ProviderRegistry::new(),
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(recorder.clone());
    state.now = Box::new(|| NOW);
    state.wire_sharing(s.ops.clone());
    let (f, pg) = (h.fixture.clone(), h.pg.clone());
    state.wire_spaces(Arc::new(ordo_host::auth::spaces::TenantLister {
        engine: Arc::new(move || Arc::new(ordo_host::deployment_engine(&f, &pg).expect("engine"))),
        pool: Some(h.pool.clone()),
    }));
    ordo_host::auth::router(Some(Arc::new(state)))
}

async fn get_json(app: &axum::Router, path: &str, bearer: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The guest leaves by the route. The token they hold is refused at the next
/// call, no window can be re-minted, the acl row is gone, the seat is
/// suspended, and the act is in both logs. A person with no seat there is
/// refused, and leaving twice is refused the second time.
#[tokio::test(flavor = "multi_thread")]
async fn a_guest_who_leaves_by_the_route_is_refused_at_the_next_call() {
    let h = harness("ordo_shr_g5b").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    let (token, _) = window_for(&h, &s, &app, "usr_gil").await;
    let state = h.state_over(Arc::new(engine_now(&h)), h.grants.clone());
    let (st, body) = call(&state, "boardList", &token).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Somebody with no seat here has nothing to leave.
    let (st, out) = post_json(
        &app,
        "/auth/share/leave",
        json!({"space": DEFAULT_SPACE}),
        Some(&session_of("usr_stranger")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["error"]["code"], "share_refused");
    // And a window token is not the person.
    let (st, _) = post_json(
        &app,
        "/auth/share/leave",
        json!({"space": DEFAULT_SPACE}),
        Some(&token),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    let (st, out) = post_json(
        &app,
        "/auth/share/leave",
        json!({"space": DEFAULT_SPACE}),
        Some(&session_of("usr_gil")),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["outcome"], "left");
    assert_eq!(out["suspended"], true);

    let (st, body) = call(&state, "boardList", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    let (st, out) = post_json(
        &app,
        "/auth/window",
        json!({"space": DEFAULT_SPACE}),
        Some(&session_of("usr_gil")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM \"boardsAccess\" WHERE \"member\" IN \
             (SELECT \"id\" FROM \"members\" WHERE \"principal\" = 'usr_gil')"
        )
        .await,
        0,
        "the acl rows the share wrote are taken back"
    );
    let status = h
        .pool
        .rows("SELECT \"status\" FROM \"members\" WHERE \"principal\" = 'usr_gil'")
        .await
        .expect("seat")[0][0]
        .clone();
    assert_eq!(status.as_deref(), Some("suspended"));

    // The act is in the deployment's stream and the sharer's.
    assert_eq!(
        act_scopes(&recorder, "share-left"),
        vec!["deployment".to_string(), format!("space:{DEFAULT_SPACE}")]
    );
}

/// `GET /auth/spaces` lists a guest seat with `via: guest` and no role, a
/// member with `via: member` and their role, and stops listing the guest
/// when they leave.
#[tokio::test(flavor = "multi_thread")]
async fn a_guest_seat_is_listed_as_a_guest_and_a_member_is_not() {
    let h = harness("ordo_shr_g6b").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = listing_app(&h, &s, &recorder);
    window_for(&h, &s, &app, "usr_gil").await;

    let (st, list) = get_json(&app, "/auth/spaces", &session_of("usr_gil")).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(
        list,
        json!([{"space": DEFAULT_SPACE, "name": null, "role": null, "via": "guest", "state": "active"}])
    );

    // Bram is an editor here, and is listed as a member with his role.
    let (_, list) = get_json(&app, "/auth/spaces", &session_of("mem_2")).await;
    assert_eq!(
        list,
        json!([{"space": DEFAULT_SPACE, "name": null, "role": "editor", "via": "member", "state": "active"}])
    );

    // Leaving takes the seat off the list.
    let (st, _) = post_json(
        &app,
        "/auth/share/leave",
        json!({"space": DEFAULT_SPACE}),
        Some(&session_of("usr_gil")),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, list) = get_json(&app, "/auth/spaces", &session_of("usr_gil")).await;
    assert_eq!(list, json!([]));
}

// ───────────── Decline (owner ruling 2026-10-05) ─────────────

/// **A guest may decline a share ticket before it expires.** The decline
/// spends the ticket, so nobody can accept it afterwards and no seat is
/// made; it is in the sharer's tenant stream and the deployment's. A named
/// ticket is declined by the person it names alone, and a window token is
/// not that person.
#[tokio::test(flavor = "multi_thread")]
async fn a_guest_declines_a_share_and_the_ticket_is_spent_and_recorded() {
    let h = harness("ordo_shr_decline").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);

    // An unnamed ticket: whoever holds it may decline it, with no token.
    let ticket = issue(
        &s,
        &h,
        &[("ops", "boardList"), ("rows", ROWS), ("expires", "7d")],
    )
    .await;
    let (st, out) = post_json(&app, "/auth/share/decline", json!({"ticket": ticket}), None).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["outcome"], "declined");
    assert_eq!(out["space"], DEFAULT_SPACE);
    let (st, out) = post_json(&app, "/auth/share/peek", json!({"ticket": ticket}), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "spent: {out}");
    let (st, out) = post_json(
        &app,
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Gil"}),
        Some(&session_of("usr_gil")),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM \"members\" WHERE \"principal\" = 'usr_gil'"
        )
        .await,
        0,
        "a declined share makes no seat"
    );
    assert_eq!(
        act_scopes(&recorder, "share-declined"),
        vec!["deployment".to_string(), format!("space:{DEFAULT_SPACE}")]
    );

    // A named ticket: refused to a holder with no session and to another
    // person, and left whole; declined by the person it names.
    let named = issue(
        &s,
        &h,
        &[
            ("ops", "boardList"),
            ("rows", ROWS),
            ("expires", "7d"),
            ("to", GUEST),
        ],
    )
    .await;
    for bearer in [None, Some(session_of("usr_mallory"))] {
        let (st, out) = post_json(
            &app,
            "/auth/share/decline",
            json!({"ticket": named}),
            bearer.as_deref(),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{out}");
        assert_eq!(out["error"]["code"], "share_refused");
    }
    let (st, _) = post_json(&app, "/auth/share/peek", json!({"ticket": named}), None).await;
    assert_eq!(st, StatusCode::OK, "the named ticket is whole");
    let (st, out) = post_json(
        &app,
        "/auth/share/decline",
        json!({"ticket": named}),
        Some(&session_of(GUEST)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let (st, _) = post_json(&app, "/auth/share/peek", json!({"ticket": named}), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

// ───────────── `std.space.shares` (owner ruling 2026-10-05) ─────────────

/// **A tenant lists its outgoing guest shares**, one row per live seat, with
/// the seat id `revokeShare` takes, the name the guest consented to show,
/// the ops and the expiry, and never the principal. A revoked share leaves
/// the list. The manager and the owner reach it, as they reach
/// `revokeShare`, and an editor does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_lists_its_live_shares_by_seat_and_a_revoked_one_leaves_the_list() {
    let h = harness("ordo_shr_list").await;
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let s = Sharing::accepting(&h, recorder.clone());
    let app = guest_app(&s, &recorder);
    let list = || async {
        s.ops
            .execute(
                &h.engine,
                SHARES,
                DEFAULT_SPACE,
                &BoundParams(Default::default()),
                ANA,
            )
            .await
            .expect("listed")
            .rows
    };

    assert!(list().await.is_empty(), "nothing shared yet");
    window_for(&h, &s, &app, "usr_gil").await;

    let rows = list().await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let seat = h
        .pool
        .rows("SELECT \"id\" FROM \"members\" WHERE \"principal\" = 'usr_gil'")
        .await
        .expect("seat")[0][0]
        .clone()
        .expect("seat id");
    assert_eq!(rows[0]["seat"], json!(seat));
    assert_eq!(rows[0]["displayName"], "Guest");
    assert_eq!(rows[0]["guest"], true);
    assert_eq!(rows[0]["ops"], json!(["boardList", "cardList"]));
    assert_eq!(rows[0]["expiresAt"], json!(NOW + 7 * 86_400));
    assert!(
        !serde_json::to_string(&rows).unwrap().contains("usr_gil"),
        "the seat is the handle, and the principal is not in the answer"
    );

    // The seat the list shows is the one revocation takes.
    s.ops
        .execute(
            &h.engine,
            REVOKE_SHARE,
            DEFAULT_SPACE,
            &params(&[("seat", &seat)]),
            ANA,
        )
        .await
        .expect("revoked");
    assert!(list().await.is_empty(), "a revoked share is not live");

    let reaches = |who: &str| {
        h.engine
            .membership_reaches(&space(), &PrincipalId(who.into()), SHARES)
    };
    assert!(reaches(ANA), "the manager lists shares");
    assert!(!reaches("mem_2"), "an editor does not");
}
