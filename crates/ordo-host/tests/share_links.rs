//! Open share links (`docs/plan-sharing.md` L1 to L4), through the real door,
//! against real Postgres.
//!
//! A link is a bearer credential: whoever holds the secret reads, with no
//! account. It rides on the seat and the share grant a guest has, so what a
//! link reaches is decided where a guest's reach is decided, and these tests
//! are ordered by what a link can never do.
//!
//! Databases are prefixed `ordo_lnk_`. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`). Each test boots a database and a
//! pool, so they take turns.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, BoundParams, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::member_ops::MemberOps;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::share_links::{hash_of, LinkSettings, ShareLinks, SET_LINK_LIFETIME, SHARE_LINK};
use ordo_host::share_ops::ShareOps;
use ordo_host::tickets::{MemoryTicketStore, Tickets};
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::grants::{GrantRecord, GrantRegistry};
use ordo_identity::principal::{PrincipalId, SpaceId as ISpace, Timestamp as IStamp};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use ordo_secevent::EventKind;
use serde_json::{json, Value};
use tower::util::ServiceExt;

/// A test file whose tests each boot a database and a pool must not run at
/// default parallelism, or they exhaust the server's connections.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The door's clock, fixed so an expiry is a property of the grant.
const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"share-links-secret";
const DAY: u64 = 86_400;

/// Ana is a manager who administers `board_1`; Ora is the owner; Dov is a
/// viewer; Gil holds a seat and no role.
const ANA: &str = "mem_1";
const OWNER: &str = "mem_o";
const VIEWER: &str = "mem_4";
const GUEST: &str = "mem_g";

const ROWS: &str = r#"[{"entity":"boards","id":"board_1","level":"read"}]"#;

struct Harness {
    engine: Arc<AccessEngine>,
    pool: ordo_exec::Pool,
    grants: Arc<PgGrantRegistry>,
    deployment: Arc<ordo_host::deploy::Deployment>,
    pg: Pg,
    fixture: Arc<Fixture>,
    share: Arc<ShareOps>,
    links: Arc<ShareLinks>,
    recorder: Arc<ordo_secevent::Recorder>,
    platform: Arc<PlatformOps>,
}

async fn harness(db: &str, settings: LinkSettings) -> Harness {
    harness_src(db, settings, None).await
}

/// The same over the fixture with more program appended, for a test that
/// needs an entity the shipped fixture has not got.
async fn harness_src(db: &str, settings: LinkSettings, source: Option<&str>) -> Harness {
    let f = Arc::new(match source {
        Some(src) => {
            Fixture::from_source(src, ordo_conformance::SEED).expect("the program compiles")
        }
        None => Fixture::load().expect("fixture"),
    });
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
    ordo_host::share_links::ensure_table(&pool)
        .await
        .expect("link tables");
    for clear in [
        r#"DELETE FROM "$capabilitygrant""#,
        r#"DELETE FROM "$sharelink""#,
        r#"DELETE FROM "$admissionpolicy""#,
        r#"DELETE FROM "$deploymentsetting" WHERE "key" = 'share_link_password_limits'"#,
        r#"DELETE FROM "$guesscount""#,
    ] {
        let _ = pool.rows(clear).await;
    }

    let engine = Arc::new(ordo_host::deployment_engine(&f, &pg).expect("engine"));
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "share-links-test",
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

    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let tickets =
        Arc::new(Tickets::new(Arc::new(MemoryTicketStore::new())).with_clock(Box::new(|| NOW)));
    let live = engine.clone();
    let share = Arc::new(ShareOps {
        tickets,
        stream: Some(recorder.clone()),
        now: Arc::new(|| NOW),
        max_secs: None,
        grants: grants.clone(),
        members: Arc::new(MemberOps {
            control: pool.clone(),
            pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
            directory: None,
            stream: None,
            refresh: None,
            now: Arc::new(|| NOW),
            managed: None,
        }),
        accept: Some(ordo_host::share_accept::AcceptWiring {
            engine: Arc::new(move || live.clone()),
            acl: Arc::new(ordo_host::share_accept::ExecutorAcl {
                executor: Arc::new(FixtureExecutor {
                    spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                    pg: pg.clone(),
                    page_size: 50,
                }),
                registry: deployment.clone(),
            }),
        }),
    });
    // The cheapest argon2id, so a test that hashes many times stays fast.
    // The production cost is `Argon2idHasher::new`, which `new` uses.
    let links = Arc::new(
        ShareLinks::new(share.clone(), settings)
            .with_hasher(Arc::new(ordo_identity::Argon2idHasher::weak_for_tests())),
    );
    let platform = Arc::new(
        PlatformOps::new(engine.clone(), grants.clone(), Arc::new(|| NOW))
            .with_share(share.clone())
            .with_links(links.clone()),
    );
    Harness {
        engine,
        pool,
        grants,
        deployment,
        pg,
        fixture: f,
        share,
        links,
        recorder,
        platform,
    }
}

impl Harness {
    /// The door over `engine`, which a test rebuilds where the runtime would
    /// after a membership change.
    fn state_over(&self, engine: Arc<AccessEngine>) -> ApiState {
        let access = Arc::new(
            ordo_host::IdentityAccess::sharing(
                engine,
                TokenKey::from_secret(SECRET),
                Box::new(|| NOW),
                Arc::new(ordo_host::metrics::Metrics::new()),
            )
            .with_shares(self.grants.clone()),
        );
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
            access,
            quota: Arc::new(ordo_api::NoQuota),
            bus: Arc::new(MemoryBus::new()),
            replay: Arc::new(MemoryReplayCache::new()),
            attest: Arc::new(ordo_api::attest::NoAttestation),
            page_key: PageKey::from_secret(b"share-links-test"),
            sync: Default::default(),
            live_streams: Default::default(),
            refine_limits: Default::default(),
            public: Default::default(),
        }
    }

    /// The door as it stands after the seats a mint wrote.
    fn door_now(&self) -> ApiState {
        self.state_over(Arc::new(engine_of(self)))
    }

    async fn mint(&self, who: &str, params: &[(&str, &str)]) -> Result<Value, ordo_api::ExecError> {
        self.run(SHARE_LINK, who, params)
            .await
            .map(|r| r.rows[0].clone())
    }

    async fn run(
        &self,
        op: &str,
        who: &str,
        params: &[(&str, &str)],
    ) -> Result<ordo_api::ExecResult, ordo_api::ExecError> {
        let map = params
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect();
        self.links
            .execute(&self.engine, op, DEFAULT_SPACE, &BoundParams(map), who)
            .await
    }

    async fn table(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        self.pool.rows(sql).await.expect("query")
    }

    async fn count(&self, sql: &str) -> i64 {
        self.table(sql).await[0][0]
            .as_deref()
            .expect("a count")
            .parse()
            .expect("an integer")
    }

    /// What a mint may have written, counted, so a refusal can be shown to
    /// have left nothing.
    async fn footprint(&self) -> (i64, i64, i64, i64) {
        (
            self.count(r#"SELECT count(*) FROM "members""#).await,
            self.count(r#"SELECT count(*) FROM "boardsAccess""#).await,
            self.count(r#"SELECT count(*) FROM "$capabilitygrant""#)
                .await,
            self.count(r#"SELECT count(*) FROM "$sharelink""#).await,
        )
    }

    async fn live_grants(&self, principal: &str) -> Vec<GrantRecord> {
        self.grants
            .grants_of(&space(), &PrincipalId(principal.into()))
            .await
            .expect("grants_of")
            .into_iter()
            .filter(|r| r.is_live(IStamp(NOW)))
            .collect()
    }
}

/// The engine the door would rebuild after a membership change: read from
/// the tenant's `members`.
fn engine_of(h: &Harness) -> AccessEngine {
    ordo_host::deployment_engine(&h.fixture, &h.pg).expect("engine")
}

fn space() -> ISpace {
    ISpace(DEFAULT_SPACE.to_string())
}

fn on() -> LinkSettings {
    LinkSettings::default()
}

/// A session-shaped token with fresh presence, as a person who has just
/// touched their passkey holds. Platform ops are gated on it.
fn person(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: Some(IStamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn door_post(state: &ApiState, op: &str, who: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/op/{op}"))
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", person(who)))
        .body(Body::from(body.to_string()))
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

// ───────────── L1: the mint ─────────────

/// A manager mints a link. The secret comes back once. It is in no chain,
/// no stream and no table: the row holds its hash. The link seat has no role
/// and is named by the label, the acl row is the sharer's act, the grant is a
/// live `shr_` grant for the link's principal, and the mint is in the
/// tenant's log.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_is_minted_once_logged_and_stored_only_as_a_hash() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_mint", on()).await;

    let out = h
        .mint(
            ANA,
            &[
                ("ops", "boardList, cardList"),
                ("rows", ROWS),
                ("expires", "7d"),
                ("label", "for the auditors"),
            ],
        )
        .await
        .expect("a manager may mint a link over guarded reads");
    let secret = out["link"].as_str().expect("the secret").to_string();
    let id = out["id"].as_str().expect("the id").to_string();
    let seat = out["seat"].as_str().expect("the seat").to_string();
    assert!(secret.starts_with("lks_") && secret.len() == 68, "{secret}");
    assert!(id.starts_with("lnk_"), "{id}");
    assert_eq!(out["expiresAt"], NOW + 7 * DAY);
    assert_eq!(out["outcome"], "minted");

    // The seat: no role, active, named by the label, principal `lnk_...`.
    let seats = h
        .table(&format!(
            r#"SELECT "role", "status", "displayName", "principal" FROM "members"
               WHERE "id" = '{seat}'"#
        ))
        .await;
    assert_eq!(
        seats,
        vec![vec![
            None,
            Some("active".into()),
            Some("for the auditors".into()),
            Some(id.clone())
        ]]
    );

    // The acl row, read, for the seat, written as the sharer.
    let acl = h
        .table(&format!(
            r#"SELECT "level" FROM "boardsAccess" WHERE "board" = 'board_1' AND "member" = '{seat}'"#
        ))
        .await;
    assert_eq!(acl, vec![vec![Some("read".to_string())]]);
    let actor = h
        .table(&format!(
            r#"SELECT "actor" FROM "$commandlog" WHERE "space" = '{DEFAULT_SPACE}'
               ORDER BY "idx" DESC LIMIT 1"#
        ))
        .await;
    assert!(
        actor[0][0].clone().unwrap_or_default().contains(ANA),
        "the acl row is the sharer's act"
    );

    // One live `shr_` grant for the link's principal, over the ops asked for.
    let held = h.live_grants(&id).await;
    assert!(!held.is_empty());
    assert!(held.iter().all(|r| r.id.0.starts_with("shr_")));
    let ops: BTreeSet<String> = held.iter().map(|r| r.op.clone()).collect();
    assert_eq!(
        ops,
        BTreeSet::from(["boardList".to_string(), "cardList".to_string()])
    );

    // The table holds the hash and never the secret.
    let stored = h.table(r#"SELECT t::text FROM "$sharelink" t"#).await;
    assert_eq!(stored.len(), 1);
    let row_text = stored[0][0].clone().unwrap();
    assert!(!row_text.contains(&secret), "the secret is not stored");
    assert!(row_text.contains(&hash_of(&secret)), "its hash is");
    assert_eq!(
        h.count(&format!(
            r#"SELECT count(*) FROM "$sharelink" WHERE "hash" = '{}'"#,
            hash_of(&secret)
        ))
        .await,
        1
    );

    // And it is in no chain and no stream.
    assert_eq!(
        h.count(&format!(
            r#"SELECT count(*) FROM "$commandlog" c WHERE c::text LIKE '%{secret}%'"#
        ))
        .await,
        0,
        "the secret is not in the chain"
    );
    let events = h.recorder.drain_all(ordo_commandlog::Timestamp(NOW + 10));
    let minted: Vec<_> = events
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::Enrolment { act, .. } if act == "link-minted"))
        .collect();
    assert_eq!(minted.len(), 1, "the mint is in the tenant's log");
    assert!(
        events
            .iter()
            .all(|e| !format!("{:?}", e.kind).contains(&secret)),
        "the secret is in no stream"
    );

    // A second mint is a different link.
    let again = h
        .mint(ANA, &[("ops", "boardList"), ("rows", ROWS)])
        .await
        .expect("another");
    assert_ne!(again["link"], out["link"]);
    assert_ne!(again["id"], out["id"]);
    assert_eq!(
        again["expiresAt"],
        NOW + 7 * DAY,
        "no `$expires` takes the deployment's default, seven days"
    );
}

/// What a link may list. Every refusal leaves nothing: no seat, no acl row,
/// no grant, no link.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_over_a_write_op_or_a_platform_op_is_refused() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_refuse", on()).await;
    let before = h.footprint().await;

    let cases: [(&str, &str, &str, &str); 8] = [
        (ANA, "addBoard", ROWS, "share_read_only"),
        (ANA, "addCard", ROWS, "share_read_only"),
        (ANA, "std.space.callersOf", ROWS, "share_platform_op"),
        (ANA, "std.space.shareLink", ROWS, "share_platform_op"),
        (ANA, "allTasks", ROWS, "guest_exposure_unguarded"),
        (ANA, "noSuchOp", ROWS, "share_unknown_op"),
        // A seat with no role reaches nothing by membership, so it cannot
        // mint: a guest cannot share onward, and neither can a link.
        (GUEST, "boardList", ROWS, "share_beyond_sharer"),
        (
            ANA,
            "boardList",
            r#"[{"entity":"boards","id":"board_1","level":"write"}]"#,
            "share_bad_rows",
        ),
    ];
    for (who, op, rows, want) in cases {
        let e = h
            .mint(who, &[("ops", op), ("rows", rows)])
            .await
            .expect_err(&format!("`{op}` as {who} must be refused"));
        assert!(
            e.to_string().contains(want),
            "`{op}` as {who}: wanted {want}, got {e}"
        );
        assert_eq!(h.footprint().await, before, "`{op}` left something behind");
    }

    // A row the sharer does not administer refuses the mint and takes back
    // what it wrote: Ana holds no acl row on `board_2`.
    let e = h
        .mint(
            ANA,
            &[
                ("ops", "boardList"),
                (
                    "rows",
                    r#"[{"entity":"boards","id":"board_2","level":"read"}]"#,
                ),
            ],
        )
        .await
        .expect_err("Ana does not administer board_2");
    assert!(e.to_string().contains("share_row_refused"), "{e}");
    assert_eq!(h.footprint().await, before, "the seat was taken back");

    // The same with the refused row second: the row written first is taken
    // back as well as the seat.
    let e = h
        .mint(
            ANA,
            &[
                ("ops", "boardList"),
                (
                    "rows",
                    r#"[{"entity":"boards","id":"board_1","level":"read"},
                        {"entity":"boards","id":"board_2","level":"read"}]"#,
                ),
            ],
        )
        .await
        .expect_err("Ana does not administer board_2");
    assert!(e.to_string().contains("share_row_refused"), "{e}");
    assert_eq!(
        h.footprint().await,
        before,
        "the acl row and the seat were taken back"
    );

    // Who may mint: a manager and an owner do; a viewer and a guest do not.
    let reaches = |who: &str| {
        h.engine
            .membership_reaches(&space(), &PrincipalId(who.into()), SHARE_LINK)
    };
    assert!(reaches(ANA) && reaches(OWNER));
    assert!(!reaches(VIEWER) && !reaches(GUEST));
}

/// With links off the mint is refused, and nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn links_off_refuses_the_mint() {
    let _serial = SERIAL.lock().await;
    let h = harness(
        "ordo_lnk_off",
        LinkSettings {
            enabled: false,
            ..on()
        },
    )
    .await;
    let before = h.footprint().await;
    let e = h
        .mint(ANA, &[("ops", "boardList"), ("rows", ROWS)])
        .await
        .expect_err("links are off");
    assert!(e.to_string().contains("share_links_off"), "{e}");
    assert!(e.to_string().contains("ORDO_SHARE_LINKS=on"), "{e}");
    assert_eq!(h.footprint().await, before);
}

/// The lifetime has layers and the platform adds no cap of its own. The
/// deployment's default is what a link gets when it says nothing, and it is
/// not a ceiling: a link may ask for longer.
#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_default_is_a_default_and_never_a_cap() {
    let _serial = SERIAL.lock().await;
    let h = harness(
        "ordo_lnk_default",
        LinkSettings {
            default_secs: 3 * DAY,
            ..on()
        },
    )
    .await;
    let plain = h
        .mint(ANA, &[("ops", "boardList"), ("rows", ROWS)])
        .await
        .expect("mint");
    assert_eq!(
        plain["expiresAt"],
        NOW + 3 * DAY,
        "the deployment's default"
    );

    let long = h
        .mint(
            ANA,
            &[("ops", "boardList"), ("rows", ROWS), ("expires", "400d")],
        )
        .await
        .expect("nothing caps a link");
    assert_eq!(long["expiresAt"], NOW + 400 * DAY);
    let years = h
        .mint(
            ANA,
            &[("ops", "boardList"), ("rows", ROWS), ("expires", "5y")],
        )
        .await
        .expect("an app developer who wants years gets years");
    assert_eq!(years["expiresAt"], NOW + 5 * 365 * DAY);

    // The stock default is seven days, from the blob policy.
    assert_eq!(
        LinkSettings::default().default_secs,
        7 * DAY,
        "seven days when nothing sets it"
    );
}

/// The tenant layer. When the app developer exposes it, a tenant owner
/// chooses how long its links live: that replaces the default for its links
/// and bounds them. `default` takes the choice back. A manager may not
/// choose, and where the app developer has not exposed it nobody may, and a
/// value already stored is not applied.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_choice_sets_and_bounds_its_links_only_when_the_app_developer_exposes_it() {
    let _serial = SERIAL.lock().await;
    let h = harness(
        "ordo_lnk_tenant",
        LinkSettings {
            tenant_choice: true,
            ..on()
        },
    )
    .await;
    let state = h.state_over(h.engine.clone());

    // A manager may mint links and may not choose the tenant's lifetime.
    let (st, body) = door_post(&state, SET_LINK_LIFETIME, ANA, json!({"lifetime": "2d"})).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    let (st, body) = door_post(&state, SET_LINK_LIFETIME, OWNER, json!({"lifetime": "2d"})).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["source"], "tenant");
    assert_eq!(body["rows"][0]["lifetimeSecs"], 2 * DAY);

    // It is the default for this tenant's links, and the longest one.
    let plain = h
        .mint(ANA, &[("ops", "boardList"), ("rows", ROWS)])
        .await
        .expect("mint");
    assert_eq!(plain["expiresAt"], NOW + 2 * DAY);
    let shorter = h
        .mint(
            ANA,
            &[("ops", "boardList"), ("rows", ROWS), ("expires", "1d")],
        )
        .await
        .expect("within the tenant's choice");
    assert_eq!(shorter["expiresAt"], NOW + DAY);
    let e = h
        .mint(
            ANA,
            &[("ops", "boardList"), ("rows", ROWS), ("expires", "3d")],
        )
        .await
        .expect_err("past what the tenant chose");
    assert!(e.to_string().contains("share_bad_expiry"), "{e}");

    // `default` takes the choice back: the deployment's default applies and
    // the tenant's bound is gone.
    let (st, body) = door_post(
        &state,
        SET_LINK_LIFETIME,
        OWNER,
        json!({"lifetime": "default"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["source"], "deployment");
    let back = h
        .mint(ANA, &[("ops", "boardList"), ("rows", ROWS)])
        .await
        .expect("mint");
    assert_eq!(back["expiresAt"], NOW + 7 * DAY);
    let (st, body) = door_post(
        &state,
        SET_LINK_LIFETIME,
        OWNER,
        json!({"lifetime": "soon"}),
    )
    .await;
    assert_ne!(st, StatusCode::OK, "{body}");

    // Not exposed: a tenant cannot choose, and a stored choice is not
    // applied.
    let closed = harness("ordo_lnk_tenant_closed", on()).await;
    let e = closed
        .run(SET_LINK_LIFETIME, OWNER, &[("lifetime", "2d")])
        .await
        .expect_err("the app developer has not exposed it");
    assert!(
        e.to_string().contains("share_link_lifetime_not_exposed"),
        "{e}"
    );
    assert!(
        e.to_string().contains("ORDO_SHARE_LINK_TENANT_LIFETIME=on"),
        "{e}"
    );
    closed
        .pool
        .rows(&format!(
            r#"INSERT INTO "$admissionpolicy" ("space", "share_link_secs")
               VALUES ('{DEFAULT_SPACE}', {})"#,
            2 * DAY
        ))
        .await
        .expect("a stored choice");
    let ignored = closed
        .mint(ANA, &[("ops", "boardList"), ("rows", ROWS)])
        .await
        .expect("mint");
    assert_eq!(
        ignored["expiresAt"],
        NOW + 7 * DAY,
        "a choice nobody exposed is not applied"
    );
}

// ───────────── L2: open ─────────────

use axum::extract::ConnectInfo;
use ordo_host::auth::AuthState;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{MemorySessionRevocations, ProviderRegistry};
use ordo_secevent::Scope;

/// The auth routes over a deployment that opens links, with the route's
/// clock at `now`.
fn link_app(h: &Harness, now: u64) -> axum::Router {
    let mut state = AuthState::new(
        ProviderRegistry::new(),
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(h.recorder.clone());
    state.now = Box::new(move || now);
    state.wire_sharing(h.share.clone());
    state.wire_links(h.links.clone());
    ordo_host::auth::router(Some(Arc::new(state)))
}

fn from_address(ip: [u8; 4]) -> ConnectInfo<std::net::SocketAddr> {
    ConnectInfo(std::net::SocketAddr::from((ip, 4000)))
}

/// `POST /auth/link` with no token, from `addr` when one is given.
async fn open_link(app: &axum::Router, body: Value, addr: Option<[u8; 4]>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/auth/link")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    if let Some(ip) = addr {
        req.extensions_mut().insert(from_address(ip));
    }
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

/// A GET at the door in the named tenant, as the link token presents it.
async fn read_in(state: &ApiState, space: &str, op: &str, bearer: &str) -> (StatusCode, Value) {
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

/// The rows an answer carries, without the row ids the wire adds.
fn rows_of(body: &Value) -> Value {
    let mut rows = body["rows"].clone();
    for r in rows.as_array_mut().into_iter().flatten() {
        if let Some(o) = r.as_object_mut() {
            o.remove("$id");
        }
    }
    rows
}

async fn mint_board_link(h: &Harness) -> Value {
    h.mint(
        ANA,
        &[
            ("ops", "boardList, cardList"),
            ("rows", ROWS),
            ("expires", "7d"),
            ("label", "for the auditors"),
        ],
    )
    .await
    .expect("a manager may mint a link")
}

fn events_of(h: &Harness, wanted: &str) -> Vec<(String, String)> {
    h.recorder
        .drain_all(ordo_commandlog::Timestamp(NOW + 10))
        .into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::Enrolment { act, detail } if act == wanted => Some((
                match e.scope {
                    Scope::Deployment => "deployment".to_string(),
                    Scope::Space(s) => format!("space:{}", s.0),
                },
                detail.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// Anyone holding the secret opens the link, with no account and no token.
/// The token it answers reads exactly the rows the sharer opened, through
/// the ordinary door, as the link's own seat. It is bound to the one
/// tenant, and an op the link does not list is refused however guarded.
#[tokio::test(flavor = "multi_thread")]
async fn an_opened_link_reads_exactly_the_shared_rows_and_nothing_else() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_open", on()).await;
    let minted = mint_board_link(&h).await;
    let secret = minted["link"].as_str().unwrap().to_string();
    let id = minted["id"].as_str().unwrap().to_string();
    let app = link_app(&h, NOW);

    let (st, out) = open_link(&app, json!({"link": secret}), Some([203, 0, 113, 7])).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["space"], DEFAULT_SPACE);
    assert_eq!(out["link"], id);
    assert_eq!(out["ops"], json!(["boardList", "cardList"]));
    assert_eq!(
        out["expires_at"],
        NOW + 3_600,
        "one session life, not a week"
    );
    let token = out["token"].as_str().expect("token").to_string();

    let state = h.door_now();
    let (st, body) = read_in(&state, DEFAULT_SPACE, "boardList", &token).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(rows_of(&body), json!([{"name": "roadmap"}]), "not board_2");
    let (st, body) = read_in(&state, DEFAULT_SPACE, "cardList", &token).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        rows_of(&body),
        json!([
            {"title": "q3 goals", "done": false, "board": "board_1"},
            {"title": "q4 goals", "done": true,  "board": "board_1"},
        ]),
        "board_2's card is not the link's"
    );

    // An op the link does not list, and another tenant.
    let (st, _) = read_in(&state, DEFAULT_SPACE, "openTasks", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, body) = read_in(&state, "spc_elsewhere", "boardList", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");

    // Opens are counted, and the open is on the record, in
    // the deployment's stream and the sharer's tenant's.
    let opens = h
        .table(&format!(
            r#"SELECT "opens" FROM "$sharelink" WHERE "id" = '{id}'"#
        ))
        .await;
    assert_eq!(opens[0][0].as_deref(), Some("1"));
    let mut opened = events_of(&h, "link-opened");
    opened.sort();
    assert_eq!(opened.len(), 2, "{opened:?}");
    assert_eq!(opened[0].0, "deployment");
    assert!(opened[0].1.contains(&format!("link={id}")), "{opened:?}");
    assert!(
        !opened[0].1.contains(&secret),
        "the secret is not in the record"
    );
}

/// Unknown, malformed, expired, revoked, a grant no longer live and links
/// switched off are one answer, byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_expired_or_revoked_link_is_the_one_answer() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_one", on()).await;
    let app = link_app(&h, NOW);
    let addr = |n: u8| Some([198, 51, 100, n]);

    let unknown = format!("lks_{}", "ab".repeat(32));
    let (want_status, want_body) = open_link(&app, json!({"link": unknown}), addr(1)).await;
    assert_eq!(want_status, StatusCode::FORBIDDEN, "{want_body}");
    assert_eq!(want_body["error"]["code"], "link_refused");

    // Malformed, and no secret at all.
    for (n, body) in [
        (2u8, json!({"link": "not-a-link"})),
        (3, json!({"link": "tkt_".to_string() + &"ab".repeat(32)})),
        (4, json!({})),
    ] {
        let (st, b) = open_link(&app, body, addr(n)).await;
        assert_eq!((st, &b), (want_status, &want_body));
    }

    // A live link opens; then each way of ending it is the same answer.
    let live = mint_board_link(&h).await;
    let secret = live["link"].as_str().unwrap().to_string();
    let (st, ok) = open_link(&app, json!({"link": secret}), addr(5)).await;
    assert_eq!(st, StatusCode::OK, "{ok}");

    // Expired: the route's clock is past the link's end.
    let late = link_app(&h, NOW + 8 * DAY);
    let (st, b) = open_link(&late, json!({"link": secret}), addr(6)).await;
    assert_eq!((st, &b), (want_status, &want_body), "expired");

    // Revoked in the table.
    h.pool
        .rows(&format!(
            r#"UPDATE "$sharelink" SET "revoked_at" = {NOW} WHERE "hash" = '{}'"#,
            hash_of(&secret)
        ))
        .await
        .expect("revoke");
    let (st, b) = open_link(&app, json!({"link": secret}), addr(7)).await;
    assert_eq!((st, &b), (want_status, &want_body), "revoked");
    h.pool
        .rows(r#"UPDATE "$sharelink" SET "revoked_at" = NULL"#)
        .await
        .expect("un-revoke");
    let (st, _) = open_link(&app, json!({"link": secret}), addr(8)).await;
    assert_eq!(st, StatusCode::OK, "the row is what decided");

    // The grant is no longer live in the registry.
    for g in h.live_grants(live["id"].as_str().unwrap()).await {
        h.grants
            .revoke(&space(), &g.id, IStamp(NOW))
            .await
            .expect("revoke grant");
    }
    let (st, b) = open_link(&app, json!({"link": secret}), addr(9)).await;
    assert_eq!((st, &b), (want_status, &want_body), "grant not live");

    // Links switched off.
    let off = harness(
        "ordo_lnk_one_off",
        LinkSettings {
            enabled: false,
            ..on()
        },
    )
    .await;
    let off_app = link_app(&off, NOW);
    let (st, b) = open_link(&off_app, json!({"link": secret}), addr(10)).await;
    assert_eq!((st, &b), (want_status, &want_body), "links off");
}

/// What the token is: the link's own principal, one tenant, no session, no
/// client, no presence, and a grant that lists no platform op. So it writes
/// nothing, steps up to nothing and opens no window of its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_token_cannot_write_step_up_or_call_a_platform_op() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_token", on()).await;
    let minted = mint_board_link(&h).await;
    let id = minted["id"].as_str().unwrap().to_string();
    let app = link_app(&h, NOW);
    let (st, out) = open_link(&app, json!({"link": minted["link"]}), Some([192, 0, 2, 1])).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let token = out["token"].as_str().unwrap().to_string();

    // The claims.
    let claims = ordo_identity::verify(&TokenKey::from_secret(SECRET), &token, IStamp(NOW))
        .expect("the token verifies");
    assert_eq!(claims.principal.0, id, "the link's own principal");
    assert!(claims.session.is_none() && claims.client.is_none());
    assert!(claims.user_presence_at.is_none(), "no presence");
    let grant = claims.capability.as_ref().expect("a capability token");
    assert!(grant.id().0.starts_with("shr_"));
    assert_eq!(grant.space().0, DEFAULT_SPACE);
    let listed: Vec<&String> = grant.pins().keys().collect();
    assert!(
        listed.iter().all(|o| !o.starts_with("std.")),
        "no platform op is listed: {listed:?}"
    );

    // At the door: a write, platform ops, and minting onward are refused.
    let state = h.door_now();
    for (method, op, body) in [
        ("POST", "addBoard", json!({"name": "mine"})),
        ("GET", "std.space.callersOf?op=boardList", json!(null)),
        (
            "POST",
            "std.space.shareLink",
            json!({"ops": "boardList", "rows": ROWS}),
        ),
        ("GET", "std.space.links", json!(null)),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(format!("/op/{op}"))
            .header("x-ordo-space", DEFAULT_SPACE)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(if method == "GET" {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .unwrap();
        let resp = router(state.clone()).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "`{op}`");
    }
    assert_eq!(
        h.count(r#"SELECT count(*) FROM "boards""#).await,
        2,
        "nothing was written"
    );

    // It is not a session: it opens no window and accepts no share.
    for path in ["/auth/window", "/auth/share/accept", "/auth/share/leave"] {
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(json!({"space": DEFAULT_SPACE}).to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN,
            "{path}: {}",
            resp.status()
        );
    }
}

/// Wrong link opens are never refused for their count: any number from one
/// address is answered `403 link_refused`, never `429`, and the right secret
/// still opens. Ordo does not decide on a client's address.
#[tokio::test(flavor = "multi_thread")]
async fn wrong_link_opens_are_never_refused_for_the_count() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_nolimit", on()).await;
    let minted = mint_board_link(&h).await;
    let secret = minted["link"].as_str().unwrap().to_string();
    let app = link_app(&h, NOW);
    let guesser = Some([203, 0, 113, 50]);
    let wrong = format!("lks_{}", "cd".repeat(32));

    for i in 0..25 {
        let (st, body) = open_link(&app, json!({"link": wrong}), guesser).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "miss {i}: {body}");
    }
    let (st, body) = open_link(&app, json!({"link": secret}), guesser).await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

/// `ORDO_SHARE_LINK_MISS_LIMIT` is proxy mode's. The link settings do not
/// read it, so a number or nonsense neither limits nor refuses the boot, and
/// without proxy mode the boot says what turns it on. Its slow-down is tested
/// in `ordo_host::proxy` and `marketing_site_app.rs`.
#[tokio::test(flavor = "multi_thread")]
async fn the_share_link_miss_limit_waits_for_proxy_mode() {
    let _serial = SERIAL.lock().await;
    let var = "ORDO_SHARE_LINK_MISS_LIMIT";
    for value in ["5", "lots"] {
        std::env::set_var(var, value);
        let settings = LinkSettings::from_env();
        std::env::remove_var(var);
        assert_eq!(settings.expect("not refused"), LinkSettings::default());
    }
    let lines = ordo_host::proxy::inert_limits(|v| v == var);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains(var) && lines[0].contains("ORDO_PROXY_HEADER"));
    assert!(ordo_host::proxy::inert_limits(|_| false).is_empty());
}

// ───────────── L3: revoke and list ─────────────

use ordo_host::share_links::{LINKS, REVOKE_LINK};

/// The sharer revokes a link. The very next call with the same token,
/// unexpired and correctly signed, is refused; the link no longer opens; the
/// acl row is gone, the seat is suspended, and the act is on the record.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_link_is_refused_at_the_next_call() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_revoke", on()).await;
    let minted = mint_board_link(&h).await;
    let (id, seat) = (
        minted["id"].as_str().unwrap().to_string(),
        minted["seat"].as_str().unwrap().to_string(),
    );
    let app = link_app(&h, NOW);
    let (st, out) = open_link(&app, json!({"link": minted["link"]}), Some([192, 0, 2, 9])).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let token = out["token"].as_str().unwrap().to_string();
    let (st, _) = read_in(&h.door_now(), DEFAULT_SPACE, "boardList", &token).await;
    assert_eq!(st, StatusCode::OK);

    // A viewer may not revoke; the manager may.
    let state = h.door_now();
    let (st, _) = door_post(&state, REVOKE_LINK, VIEWER, json!({"link": id})).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, body) = door_post(&state, REVOKE_LINK, ANA, json!({"link": id})).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["aclRows"], 1);
    assert!(body["rows"][0]["grants"].as_u64().unwrap() >= 1);

    // The next call: same token, still inside its life, refused.
    let (st, body) = read_in(&h.door_now(), DEFAULT_SPACE, "boardList", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "access_denied");
    // And no token can be minted again.
    let (st, body) = open_link(&app, json!({"link": minted["link"]}), Some([192, 0, 2, 9])).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");

    // The row, the seat and the acl row.
    let row = h
        .table(&format!(
            r#"SELECT "revoked_at" FROM "$sharelink" WHERE "id" = '{id}'"#
        ))
        .await;
    assert_eq!(row[0][0].as_deref(), Some(NOW.to_string().as_str()));
    let status = h
        .table(&format!(
            r#"SELECT "status" FROM "members" WHERE "id" = '{seat}'"#
        ))
        .await;
    assert_eq!(status[0][0].as_deref(), Some("suspended"));
    assert_eq!(
        h.count(&format!(
            r#"SELECT count(*) FROM "boardsAccess" WHERE "member" = '{seat}'"#
        ))
        .await,
        0
    );
    assert!(h.live_grants(&id).await.is_empty());
    let revoked = events_of(&h, "link-revoked");
    assert_eq!(
        revoked.len(),
        1,
        "one act, in the tenant's log: {revoked:?}"
    );
    assert_eq!(revoked[0].0, format!("space:{DEFAULT_SPACE}"));

    // Another tenant's id, and nothing at all, are not found.
    for who in ["lnk_unknown", "boardList"] {
        let e = h
            .run(REVOKE_LINK, ANA, &[("link", who)])
            .await
            .expect_err("no such link");
        assert!(e.to_string().contains("no such link"), "{e}");
    }
}

/// The list shows each link of the tenant with its label, ops, rows, opens,
/// ends and state, newest first, and never the secret or its hash. Another
/// tenant's links are not in it.
#[tokio::test(flavor = "multi_thread")]
async fn the_list_shows_opens_and_state_and_never_the_secret() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_list", on()).await;
    let one = mint_board_link(&h).await;
    let two = h
        .mint(
            ANA,
            &[
                ("ops", "boardList"),
                ("rows", ROWS),
                ("expires", "1d"),
                ("label", "second"),
            ],
        )
        .await
        .expect("second");
    let three = h
        .mint(
            ANA,
            &[("ops", "boardList"), ("rows", ROWS), ("label", "third")],
        )
        .await
        .expect("third");
    let app = link_app(&h, NOW);
    for _ in 0..2 {
        let (st, _) = open_link(&app, json!({"link": one["link"]}), Some([192, 0, 2, 20])).await;
        assert_eq!(st, StatusCode::OK);
    }
    h.run(REVOKE_LINK, ANA, &[("link", two["id"].as_str().unwrap())])
        .await
        .expect("revoke");
    // A link that has lapsed, and one that is another tenant's.
    h.pool
        .rows(&format!(
            r#"UPDATE "$sharelink" SET "expires_at" = {} WHERE "id" = '{}'"#,
            NOW - 1,
            three["id"].as_str().unwrap()
        ))
        .await
        .expect("lapse");
    h.pool
        .rows(&format!(
            r#"INSERT INTO "$sharelink" ("hash","id","space","grant_id","seat","created_at","expires_at")
               VALUES ('elsewhere', 'lnk_elsewhere', 'spc_elsewhere', 'shr_x', 'mem_x', {NOW}, {})"#,
            NOW + DAY
        ))
        .await
        .expect("another tenant's row");

    let (st, body) = read_in(&h.door_now(), DEFAULT_SPACE, LINKS, &person(ANA)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let by_label: std::collections::BTreeMap<String, Value> = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["label"].as_str().unwrap().to_string(), r.clone()))
        .collect();
    assert_eq!(by_label.len(), 3, "not the other tenant's: {body}");
    assert_eq!(by_label["for the auditors"]["state"], "live");
    assert_eq!(by_label["for the auditors"]["opens"], 2);
    assert_eq!(
        by_label["for the auditors"]["ops"],
        json!(["boardList", "cardList"])
    );
    assert_eq!(
        by_label["for the auditors"]["rows"],
        json!([{"entity": "boards", "id": "board_1", "level": "read"}])
    );
    assert_eq!(by_label["second"]["state"], "revoked");
    assert_eq!(by_label["third"]["state"], "expired");

    let text = body.to_string();
    for link in [&one, &two, &three] {
        let secret = link["link"].as_str().unwrap();
        assert!(!text.contains(secret), "the secret is not listed");
        assert!(!text.contains(&hash_of(secret)), "nor its hash");
    }
}

// ───────────── L4: files ─────────────

use ordo_api::blob::{BlobLimits, BlobPort, BlobState};
use ordo_api::router_with_blobs;
use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore};
use ordo_conformance::FIXTURE;
use ordo_host::blob::PlatformBlobs;

/// One entity with a file and an acl, so the rows a link's seat may open are
/// the acl rows the sharer wrote, and the file of a row is the row.
const PAPERS: &str = "
entity papers (key: paper) {
  title: text
  scan:  blob?
  acl (read, write, admin)
}
";

async fn blob_harness(db: &str) -> Harness {
    harness_src(db, on(), Some(&format!("{FIXTURE}\n{PAPERS}"))).await
}

fn blob_app(h: &Harness, engine: Arc<AccessEngine>, dir: &std::path::Path) -> axum::Router {
    let store: Arc<dyn BlobStore> = FsStore::shared(dir).expect("store");
    let pipeline = BlobPipeline::new(
        Arc::new(ordo_blob::SingleStore::new(store)),
        h.fixture.keys.clone(),
        BlobPolicy::DEFAULT,
    );
    let port = Arc::new(PlatformBlobs {
        spaces: ordo_host::spaces::Spaces::new(h.deployment.clone()),
        pipeline,
    });
    router_with_blobs(
        h.state_over(engine),
        BlobState {
            port: port as Arc<dyn BlobPort>,
            limits: BlobLimits::DEFAULT,
        },
    )
}

async fn blob_put(app: &axum::Router, id: &str, bearer: &str, body: Vec<u8>) -> StatusCode {
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/blob/papers/scan/{id}"))
        .header("authorization", format!("Bearer {bearer}"))
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("content-length", body.len().to_string())
        .header("content-type", "application/pdf")
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn blob_get(app: &axum::Router, handle: &str, bearer: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/blob/{handle}"))
        .header("authorization", format!("Bearer {bearer}"))
        .header("x-ordo-space", DEFAULT_SPACE)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

/// A link that lists `std.papers.get` serves the file of the row it shares
/// and refuses the file of another row. It cannot upload. When the sharer
/// revokes it, the very next file request with the same token is refused:
/// the blob door runs the token gate.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_serves_the_file_of_a_shared_row_and_not_another() {
    let _serial = SERIAL.lock().await;
    let h = blob_harness("ordo_lnk_files").await;
    let dir = std::env::temp_dir().join(format!("ordo-lnk-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Two papers, each with a file, both administered by Ana.
    for (id, title) in [("paper_1", "shared"), ("paper_2", "private")] {
        h.pool
            .rows(&format!(
                r#"INSERT INTO "papers" ("$space","id","rev","title")
                   VALUES ('{DEFAULT_SPACE}', '{id}', 1, '{title}')"#
            ))
            .await
            .expect("paper");
        h.pool
            .rows(&format!(
                r#"INSERT INTO "papersAccess" ("$space","id","paper","member","level")
                   VALUES ('{DEFAULT_SPACE}', 'pa_{id}', '{id}', '{ANA}', 'admin')"#
            ))
            .await
            .expect("acl");
    }
    let uploader = blob_app(&h, h.engine.clone(), &dir);
    let (one, two): (Vec<u8>, Vec<u8>) = (
        (0u8..=255).cycle().take(3_000).collect(),
        (0u8..=255).rev().cycle().take(2_000).collect(),
    );
    assert_eq!(
        blob_put(&uploader, "paper_1", &person(ANA), one.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        blob_put(&uploader, "paper_2", &person(ANA), two).await,
        StatusCode::NO_CONTENT
    );
    let handle_of = |id: &'static str| {
        let pool = h.pool.clone();
        async move {
            pool.rows(&format!(
                r#"SELECT "scan" FROM "papers" WHERE "$space" = '{DEFAULT_SPACE}' AND "id" = '{id}'"#
            ))
            .await
            .expect("select")[0][0]
                .clone()
                .expect("the column was set")
        }
    };
    let (h1, h2) = (handle_of("paper_1").await, handle_of("paper_2").await);

    // The link, over the row's own `get`.
    let minted = h
        .mint(
            ANA,
            &[
                ("ops", "std.papers.get"),
                (
                    "rows",
                    r#"[{"entity":"papers","id":"paper_1","level":"read"}]"#,
                ),
                ("expires", "7d"),
                ("label", "the scan"),
            ],
        )
        .await
        .expect("a link over a guarded std read");
    let ops: Vec<String> = serde_json::from_value(minted["ops"].clone()).unwrap();
    assert!(
        ops.contains(&"std.$blob.get".to_string()),
        "the file door's own check is in the grant: {ops:?}"
    );
    let app = link_app(&h, NOW);
    let (st, out) = open_link(&app, json!({"link": minted["link"]}), Some([192, 0, 2, 40])).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let token = out["token"].as_str().unwrap().to_string();

    // The engine as a replica holds it once the seat exists. It is kept: a replica
    // that has not yet heard the revocation announced still holds it, and the
    // token gate is what closes the file door there.
    let stale = Arc::new(engine_of(&h));
    let files = blob_app(&h, stale.clone(), &dir);
    let (st, bytes) = blob_get(&files, &h1, &token).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(bytes, one, "the shared row's file, byte for byte");
    let (st, bytes) = blob_get(&files, &h2, &token).await;
    assert_ne!(st, StatusCode::OK, "another row's file");
    assert!(bytes.is_empty() || !bytes.starts_with(&[255, 254]));
    // The link writes nothing, files included.
    assert_eq!(
        blob_put(&files, "paper_1", &token, vec![1, 2, 3]).await,
        StatusCode::FORBIDDEN
    );

    // Revoked: the next file request with the same token is refused.
    h.run(
        REVOKE_LINK,
        ANA,
        &[("link", minted["id"].as_str().unwrap())],
    )
    .await
    .expect("revoke");
    let closed = blob_app(&h, stale, &dir);
    let (st, bytes) = blob_get(&closed, &h1, &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "a revoked link serves no file");
    assert!(bytes.is_empty() || bytes.len() < one.len());

    // The grant is what closed it, not only the acl row the revoke took back.
    // Put the row back, as if the revoke had stopped after the grant and
    // before the rows (it revokes the grant first for that reason): the row
    // guard would admit the seat, and the token gate still refuses.
    let seat = minted["seat"].as_str().unwrap();
    h.pool
        .rows(&format!(
            r#"INSERT INTO "papersAccess" ("$space","id","paper","member","level")
               VALUES ('{DEFAULT_SPACE}', 'pa_back', 'paper_1', '{seat}', 'read')"#
        ))
        .await
        .expect("the row is back");
    let (st, bytes) = blob_get(&closed, &h1, &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "the gate refuses the file door");
    assert!(bytes.is_empty() || bytes.len() < one.len());
    std::fs::remove_dir_all(&dir).ok();
}

// ───────────── link passwords (owner, 2026-10-22) ─────────────

use ordo_host::link_password::{Deployment as PwDeployment, Field};
use ordo_host::share_links::{SET_APP_PASSWORD_LIMITS, SET_LINK_PASSWORD, SET_PASSWORD_LIMITS};

const PASSWORD: &str = "correct horse";

async fn mint_with_password(h: &Harness, password: &str) -> Value {
    h.mint(
        ANA,
        &[
            ("ops", "boardList"),
            ("rows", ROWS),
            ("label", "locked door"),
            ("password", password),
        ],
    )
    .await
    .expect("a manager may mint a link with a password")
}

async fn opens_of(h: &Harness, id: &str) -> String {
    h.table(&format!(
        r#"SELECT "opens" FROM "$sharelink" WHERE "id" = '{id}'"#
    ))
    .await[0][0]
        .clone()
        .unwrap_or_default()
}

/// A link minted with no password opens as before, and a password sent to
/// it changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_without_a_password_still_opens() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_pw_none", on()).await;
    let minted = mint_board_link(&h).await;
    assert_eq!(minted["hasPassword"], false);
    let app = link_app(&h, NOW);
    let (st, out) = open_link(&app, json!({"link": minted["link"]}), None).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let (st, out) = open_link(
        &app,
        json!({"link": minted["link"], "password": "anything"}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let listed = h.run(LINKS, ANA, &[]).await.expect("links").rows;
    assert_eq!(listed[0]["hasPassword"], false);
}

/// A password link is refused with no password and with a wrong one, each
/// with its own retryable code, and no grant is issued. The right password
/// opens it. A wrong secret is still the one answer, password or not.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_link_wants_its_password_before_any_grant() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_pw_gate", on()).await;
    let minted = mint_with_password(&h, PASSWORD).await;
    assert_eq!(minted["hasPassword"], true);
    let id = minted["id"].as_str().unwrap().to_string();
    let app = link_app(&h, NOW);

    let (st, b) = open_link(&app, json!({"link": minted["link"]}), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{b}");
    assert_eq!(b["error"]["code"], "link_password_required");
    assert!(b.get("token").is_none());

    let (st, b) = open_link(
        &app,
        json!({"link": minted["link"], "password": "wrong horse"}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{b}");
    assert_eq!(b["error"]["code"], "link_password_wrong");
    assert!(b.get("token").is_none());
    assert_eq!(opens_of(&h, &id).await, "0", "no grant was issued");

    let (st, out) = open_link(
        &app,
        json!({"link": minted["link"], "password": PASSWORD}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert!(out["token"].as_str().is_some());
    assert_eq!(opens_of(&h, &id).await, "1");

    // An unknown secret, with or without a password, is the one answer.
    let unknown = format!("lks_{}", "ef".repeat(32));
    let (st, b) = open_link(&app, json!({"link": unknown, "password": PASSWORD}), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(b["error"]["code"], "link_refused");

    // A password that is too short is refused at the mint, naming the rule.
    let e = h
        .mint(
            ANA,
            &[("ops", "boardList"), ("rows", ROWS), ("password", "abc")],
        )
        .await
        .expect_err("too short");
    assert!(e.to_string().contains("link_bad_password"), "{e}");
}

/// Wrong tries are counted per link. At the limit the link locks, even for
/// the right password, the lock is on the tenant's stream, and the refusal
/// says when and names the setting. After the lock ends the right password
/// opens it, and a right password resets the count.
#[tokio::test(flavor = "multi_thread")]
async fn wrong_tries_lock_the_link_until_the_lock_ends() {
    let _serial = SERIAL.lock().await;
    let h = harness(
        "ordo_lnk_pw_lock",
        LinkSettings {
            password: PwDeployment::default()
                .with_value(Field::Tries, 3)
                .with_value(Field::LockSecs, 600),
            ..on()
        },
    )
    .await;
    let minted = mint_with_password(&h, PASSWORD).await;
    let link = minted["link"].clone();
    let id = minted["id"].as_str().unwrap().to_string();
    let app = link_app(&h, NOW);
    let wrong = json!({"link": link, "password": "not it"});
    let right = json!({"link": link, "password": PASSWORD});

    // Two wrong, then the right one resets the count.
    for _ in 0..2 {
        let (st, b) = open_link(&app, wrong.clone(), None).await;
        assert_eq!(b["error"]["code"], "link_password_wrong", "{st} {b}");
    }
    let (st, _) = open_link(&app, right.clone(), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(events_of(&h, "link-locked").is_empty());

    // Three wrong in a row lock it.
    for _ in 0..2 {
        let (_, b) = open_link(&app, wrong.clone(), None).await;
        assert_eq!(b["error"]["code"], "link_password_wrong");
    }
    let (st, b) = open_link(&app, wrong.clone(), None).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b}");
    assert_eq!(b["error"]["code"], "link_locked");
    assert_eq!(b["error"]["retry_after"], 600);
    assert_eq!(b["error"]["setting"], SET_PASSWORD_LIMITS);
    let locked = events_of(&h, "link-locked");
    assert!(
        locked
            .iter()
            .any(|(scope, d)| scope == &format!("space:{DEFAULT_SPACE}")
                && d.contains(&format!("link={id}"))),
        "the lock is on the tenant's stream: {locked:?}"
    );

    // Locked: the right password is refused too, with the time left.
    let later = link_app(&h, NOW + 100);
    let (st, b) = open_link(&later, right.clone(), None).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b}");
    assert_eq!(b["error"]["retry_after"], 500);

    // The lock is read back on the list.
    let listed = h.run(LINKS, ANA, &[]).await.expect("links").rows;
    assert_eq!(listed[0]["lockedUntil"], NOW + 600);

    // After the lock ends the right password opens it.
    let after = link_app(&h, NOW + 601);
    let (st, out) = open_link(&after, right.clone(), None).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let cleared = h
        .table(&format!(
            r#"SELECT "locked_until" FROM "$sharelink" WHERE "id" = '{id}'"#
        ))
        .await;
    assert_eq!(cleared, vec![vec![None]], "the right password clears it");
}

/// The sharer sets and clears a password after the mint. The list says
/// whether a link has one and never shows the hash. The stored value is an
/// argon2id hash, and neither the password nor the hash is in any answer,
/// the chain or the stream, even when the mint goes through the door.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_is_set_cleared_and_read_back_never_as_its_hash() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_pw_set", on()).await;
    let state = h.state_over(h.engine.clone());

    // Minted through the door, with a password.
    let (st, minted) = door_post(
        &state,
        SHARE_LINK,
        ANA,
        json!({"ops": "boardList", "rows": ROWS, "password": PASSWORD}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{minted}");
    let minted = minted["rows"][0].clone();
    assert_eq!(minted["hasPassword"], true);
    let id = minted["id"].as_str().unwrap().to_string();
    let hash_now = |h: &Harness| {
        let id = id.clone();
        let pool = h.pool.clone();
        async move {
            pool.rows(&format!(
                r#"SELECT "password_hash" FROM "$sharelink" WHERE "id" = '{id}'"#
            ))
            .await
            .expect("query")[0][0]
                .clone()
                .expect("a hash")
        }
    };
    let stored = hash_now(&h).await;
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    assert!(!stored.contains(PASSWORD));

    let (st, listed) = read_in(&h.door_now(), DEFAULT_SPACE, LINKS, &person(ANA)).await;
    assert_eq!(st, StatusCode::OK, "{listed}");
    assert_eq!(listed["rows"][0]["hasPassword"], true);

    // Cleared, then set again, through the door.
    let state = h.door_now();
    let (st, b) = door_post(&state, SET_LINK_PASSWORD, ANA, json!({"link": id})).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["hasPassword"], false);
    let app = link_app(&h, NOW);
    let (st, _) = open_link(&app, json!({"link": minted["link"]}), None).await;
    assert_eq!(st, StatusCode::OK, "cleared: opens with none");
    let (st, set_answer) = door_post(
        &state,
        SET_LINK_PASSWORD,
        ANA,
        json!({"link": id, "password": "second password"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{set_answer}");
    let (_, b) = open_link(
        &app,
        json!({"link": minted["link"], "password": PASSWORD}),
        None,
    )
    .await;
    assert_eq!(
        b["error"]["code"], "link_password_wrong",
        "the old one is gone"
    );
    let new_hash = hash_now(&h).await;
    let (_, listed) = read_in(&h.door_now(), DEFAULT_SPACE, LINKS, &person(ANA)).await;

    // Nowhere: answers, the list, the chain, the stream.
    let chain = h
        .table(r#"SELECT coalesce(string_agg(c::text, ' '), '') FROM "$commandlog" c"#)
        .await[0][0]
        .clone()
        .unwrap_or_default();
    let stream: String = h
        .recorder
        .drain_all(ordo_commandlog::Timestamp(NOW + 10))
        .iter()
        .map(|e| format!("{e:?}"))
        .collect();
    assert!(!stream.is_empty(), "the acts are on the record");
    for (what, text) in [
        ("the mint's answer", minted.to_string()),
        ("the list", listed.to_string()),
        ("the setter's answer", set_answer.to_string()),
        ("the chain", chain),
        ("the stream", stream),
    ] {
        for secret in [
            PASSWORD,
            "second password",
            stored.as_str(),
            new_hash.as_str(),
            "$argon2",
        ] {
            assert!(!text.contains(secret), "{what} carries {secret}");
        }
    }

    // A link this tenant has not got is not found.
    let e = h
        .run(SET_LINK_PASSWORD, ANA, &[("link", "lnk_nosuch")])
        .await
        .expect_err("no such link");
    assert!(e.to_string().contains("no such link"), "{e}");
}

/// The limits are layered like session settings (owner: "It is their
/// space"). A tenant sets its own, looser or tighter, inside the app
/// developer's range, or inside the deployment's limits when there is none.
/// A value outside is refused naming the range and who sets it. The owner
/// sets a tenant's; a manager may not.
#[tokio::test(flavor = "multi_thread")]
async fn password_limits_are_layered_like_session_settings() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_pw_layers", on()).await;
    let state = h.state_over(h.engine.clone());

    // A manager may not set the tenant's limits.
    let (st, b) = door_post(&state, SET_PASSWORD_LIMITS, ANA, json!({"tries": "2"})).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");

    // Looser than the deployment's default is the tenant's choice.
    let (st, b) = door_post(&state, SET_PASSWORD_LIMITS, OWNER, json!({"tries": "9"})).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["tries"], 9);
    assert_eq!(b["rows"][0]["triesFrom"], "tenant");
    assert_eq!(b["rows"][0]["triesRange"], json!([0, 100]));
    assert_eq!(b["rows"][0]["triesRangeFrom"], "deployment");

    // Past the deployment's absolute limits names them.
    for (field, value, env) in [
        ("tries", "101", "ORDO_SHARE_LINK_PASSWORD_TRIES_LIMIT"),
        ("lockSecs", "30", "ORDO_SHARE_LINK_PASSWORD_LOCK_SECS_LIMIT"),
        (
            "minLength",
            "0",
            "ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH_LIMIT",
        ),
    ] {
        let e = h
            .run(SET_PASSWORD_LIMITS, OWNER, &[(field, value)])
            .await
            .expect_err("outside the deployment's limits");
        assert!(
            e.to_string().contains("link_password_limit_out_of_range"),
            "{e}"
        );
        assert!(e.to_string().contains(env), "{e}");
    }

    // The app developer sets a range; the app developer's own range must sit
    // inside the deployment's limits.
    let e = h
        .run(SET_APP_PASSWORD_LIMITS, OWNER, &[("triesRange", "1-200")])
        .await
        .expect_err("past the limit");
    assert!(
        e.to_string()
            .contains("ORDO_SHARE_LINK_PASSWORD_TRIES_LIMIT"),
        "{e}"
    );
    let out = h
        .run(
            SET_APP_PASSWORD_LIMITS,
            OWNER,
            &[("tries", "4"), ("triesRange", "2-6"), ("lockSecs", "120")],
        )
        .await
        .expect("the app developer's layer")
        .rows;
    assert_eq!(out[0]["tries"], 4);
    assert_eq!(out[0]["triesFrom"], "app");
    assert_eq!(out[0]["triesRange"], json!([2, 6]));

    // The tenant's stored 9 is held to the new range, and a new 7 is refused
    // naming the app developer's range.
    let e = h
        .run(SET_PASSWORD_LIMITS, OWNER, &[("tries", "7")])
        .await
        .expect_err("outside the app developer's range");
    assert!(e.to_string().contains("triesRange"), "{e}");
    assert!(e.to_string().contains(SET_APP_PASSWORD_LIMITS), "{e}");

    // The tenant chooses 2 tries and a lock shorter than the app
    // developer's 120 s, inside the deployment's limits.
    let (st, b) = door_post(
        &state,
        SET_PASSWORD_LIMITS,
        OWNER,
        json!({"tries": "2", "lockSecs": "90"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["tries"], 2);
    assert_eq!(b["rows"][0]["lockSecs"], 90);
    assert_eq!(b["rows"][0]["lockSecsFrom"], "tenant");

    // And it is what the open applies.
    let minted = mint_with_password(&h, PASSWORD).await;
    let app = link_app(&h, NOW);
    let wrong = json!({"link": minted["link"], "password": "nope nope"});
    let (_, b) = open_link(&app, wrong.clone(), None).await;
    assert_eq!(b["error"]["code"], "link_password_wrong");
    let (st, b) = open_link(&app, wrong, None).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b}");
    assert_eq!(b["error"]["retry_after"], 90);

    // The minimum length is layered too: 8 is the deployment's floor
    // (owner, 2026-10-25), so a tenant may not go below it, and the
    // refusal names the setting that lowers it. It may raise it.
    let e = h
        .run(SET_PASSWORD_LIMITS, OWNER, &[("minLength", "1")])
        .await
        .expect_err("below the floor");
    assert!(
        e.to_string()
            .contains(ordo_host::link_password::MIN_LENGTH_LIMIT_ENV),
        "{e}"
    );
    let out = h
        .run(SET_PASSWORD_LIMITS, OWNER, &[("minLength", "9")])
        .await
        .expect("or raise it")
        .rows;
    assert_eq!(out[0]["minLength"], 9);
    let e = h
        .mint(
            ANA,
            &[
                ("ops", "boardList"),
                ("rows", ROWS),
                ("password", "eight888"),
            ],
        )
        .await
        .expect_err("shorter than the tenant's minimum");
    assert!(e.to_string().contains("link_bad_password"), "{e}");
    assert!(e.to_string().contains(SET_PASSWORD_LIMITS), "{e}");

    // `default` takes the tenant's values back.
    let out = h
        .run(
            SET_PASSWORD_LIMITS,
            OWNER,
            &[
                ("tries", "default"),
                ("lockSecs", "default"),
                ("minLength", "default"),
            ],
        )
        .await
        .expect("default")
        .rows;
    assert_eq!(out[0]["tries"], 4);
    assert_eq!(out[0]["lockSecsFrom"], "app");
    assert_eq!(out[0]["minLength"], 8);
    assert_eq!(out[0]["minLengthFrom"], "deployment");
}

/// The app developer may close a tenant's limits through the access
/// template, as with `std.space.setSessionPolicy`, and the refusal names
/// the template line that opens it.
#[test]
fn the_access_template_may_close_the_tenant_limits() {
    use ordo_host::access_template::AccessTemplate;
    let platform: Vec<String> = platform_ops::specs().into_iter().map(|s| s.name).collect();
    let closed = AccessTemplate::parse(&format!("{SET_PASSWORD_LIMITS}="), &platform)
        .expect("the template may name it");
    let why = ordo_host::session_policy::closed_by_template_for(
        SET_PASSWORD_LIMITS,
        &closed,
        Some(ordo_identity::principal::Role::Owner),
    )
    .expect("closed to the owner");
    assert!(
        why.contains(&format!(
            "ORDO_ACCESS_TEMPLATE=\"{SET_PASSWORD_LIMITS}=owner\""
        )),
        "{why}"
    );
    assert!(ordo_host::session_policy::closed_by_template_for(
        SET_PASSWORD_LIMITS,
        &AccessTemplate::none(),
        Some(ordo_identity::principal::Role::Owner),
    )
    .is_none());
}

/// The deployment's defaults and absolute limits come from the
/// environment. A limit outside what it may be, or a default outside its
/// limit, refuses the boot naming the variable.
#[tokio::test(flavor = "multi_thread")]
async fn password_limit_settings_are_read_and_checked_at_boot() {
    let _serial = SERIAL.lock().await;
    let all = [
        "ORDO_SHARE_LINK_PASSWORD_TRIES",
        "ORDO_SHARE_LINK_PASSWORD_LOCK_SECS",
        "ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH",
        "ORDO_SHARE_LINK_PASSWORD_TRIES_LIMIT",
        "ORDO_SHARE_LINK_PASSWORD_LOCK_SECS_LIMIT",
        "ORDO_SHARE_LINK_PASSWORD_MIN_LENGTH_LIMIT",
    ];
    let with = |vars: &[(&str, &str)]| {
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        let read = LinkSettings::from_env();
        for k in all {
            std::env::remove_var(k);
        }
        read
    };
    let read = with(&[
        (all[0], "7"),
        (all[1], "3600"),
        (all[2], "6"),
        (all[3], "20"),
        (all[5], "6"),
    ])
    .expect("in range");
    assert_eq!(read.password.value(Field::Tries), 7);
    assert_eq!(read.password.value(Field::LockSecs), 3600);
    assert_eq!(read.password.value(Field::MinLength), 6);
    assert_eq!(read.password.bounds(Field::Tries), (0, 20));
    // `off` is a value of tries, and the default.
    let off = with(&[(all[0], "off")]).expect("off");
    assert_eq!(off.password.value(Field::Tries), 0);
    for (vars, named) in [
        (vec![(all[1], "off")], all[1]),
        (vec![(all[0], "many")], all[0]),
        (vec![(all[1], "5")], all[1]),
        (vec![(all[0], "30"), (all[3], "20")], all[0]),
        (vec![(all[3], "0")], all[3]),
        (vec![(all[5], "0")], all[5]),
        (vec![(all[2], "2"), (all[5], "3")], all[2]),
    ] {
        let refused = with(&vars);
        assert_eq!(refused.expect_err("refused").0, named, "{vars:?}");
    }
    let d = LinkSettings::default().password;
    assert_eq!(
        [Field::Tries, Field::LockSecs, Field::MinLength].map(|f| d.value(f)),
        [0, 900, 8]
    );
    assert_eq!(d.bounds(Field::MinLength), (8, 256));
    // The slowdown is on by default: 5 wrong tries, then up to 60 s.
    assert_eq!(
        [Field::SlowAfter, Field::SlowCapSecs].map(|f| d.value(f)),
        [5, 60]
    );
}

// ── lock off by default, maximum opens (owner, 2026-10-22) ──────────

/// Off by default: with no setting anywhere, any number of wrong tries is
/// refused one by one and the link never locks. The right password opens it.
#[tokio::test(flavor = "multi_thread")]
async fn the_wrong_password_lock_is_off_by_default() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_pw_off", on()).await;
    let minted = mint_with_password(&h, PASSWORD).await;
    // Each try a little past the slowdown's longest wait (60 s), so only
    // the lock could refuse one, and it never does.
    let mut at = NOW;
    for _ in 0..12 {
        at += 61;
        let (st, b) = open_link(
            &link_app(&h, at),
            json!({"link": minted["link"], "password": "not it"}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{b}");
        assert_eq!(b["error"]["code"], "link_password_wrong");
    }
    assert!(events_of(&h, "link-locked").is_empty());
    let app = link_app(&h, at + 61);
    let (st, out) = open_link(
        &app,
        json!({"link": minted["link"], "password": PASSWORD}),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let listed = h.run(LINKS, ANA, &[]).await.expect("links").rows;
    assert_eq!(listed[0]["lockedUntil"], Value::Null);
    // The answer of the limits op says off (0) and where it came from.
    let row = h
        .run(ordo_host::share_links::SET_PASSWORD_LIMITS, ANA, &[])
        .await
        .expect("limits")
        .rows[0]
        .clone();
    assert_eq!(row["tries"], 0);
    assert_eq!(row["triesFrom"], "deployment");
}

/// A tenant turns the lock on for itself (`tries` a number) and back off
/// (`off`). While on, that many wrong tries lock the link.
#[tokio::test(flavor = "multi_thread")]
async fn the_lock_is_on_when_a_tenant_sets_tries_and_off_again_with_off() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_pw_on", on()).await;
    let minted = mint_with_password(&h, PASSWORD).await;
    let app = link_app(&h, NOW);
    let wrong = json!({"link": minted["link"], "password": "not it"});
    h.run(
        ordo_host::share_links::SET_PASSWORD_LIMITS,
        ANA,
        &[("tries", "2")],
    )
    .await
    .expect("a tenant may turn the lock on");
    let (_, b) = open_link(&app, wrong.clone(), None).await;
    assert_eq!(b["error"]["code"], "link_password_wrong");
    let (st, b) = open_link(&app, wrong.clone(), None).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b}");
    assert_eq!(b["error"]["code"], "link_locked");

    let row = h
        .run(
            ordo_host::share_links::SET_PASSWORD_LIMITS,
            ANA,
            &[("tries", "off")],
        )
        .await
        .expect("and off again")
        .rows[0]
        .clone();
    assert_eq!(row["tries"], 0);
    // A new password clears the old lock; with the lock off, many wrongs pass.
    h.run(
        ordo_host::share_links::SET_LINK_PASSWORD,
        ANA,
        &[
            ("link", minted["id"].as_str().unwrap()),
            ("password", PASSWORD),
        ],
    )
    .await
    .expect("reset");
    for _ in 0..5 {
        let (_, b) = open_link(&app, wrong.clone(), None).await;
        assert_eq!(b["error"]["code"], "link_password_wrong");
    }
}

async fn mint_limited(h: &Harness, max: &str) -> Value {
    h.mint(
        ANA,
        &[
            ("ops", "boardList"),
            ("rows", ROWS),
            ("label", "few opens"),
            ("maxOpens", max),
        ],
    )
    .await
    .expect("a manager may mint a limited link")
}

/// Once the maximum is reached the link is refused like an expired one,
/// the stream keeps the reason, and the list shows used and the limit. A
/// device that has opened renews without being counted, even at the limit.
/// The sharer changes the limit later, and `none` removes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_at_its_maximum_opens_is_used_up_and_read_back() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_max_opens", on()).await;
    let minted = mint_limited(&h, "2").await;
    assert_eq!(minted["maxOpens"], 2);
    let id = minted["id"].as_str().unwrap().to_string();
    let app = link_app(&h, NOW);
    let open = |device: &str| json!({"link": minted["link"], "device": device});

    let (st, out) = open_link(&app, open("phone"), None).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    // A renewal by the same device is not a second open.
    let (st, _) = open_link(&app, open("phone"), None).await;
    assert_eq!(st, StatusCode::OK);
    let listed = h.run(LINKS, ANA, &[]).await.expect("links").rows;
    assert_eq!(listed[0]["opensUsed"], 1);
    assert_eq!(listed[0]["maxOpens"], 2);
    assert_eq!(listed[0]["state"], "live");

    let (st, _) = open_link(&app, open("laptop"), None).await;
    assert_eq!(st, StatusCode::OK);
    // A third device is refused as the one answer.
    let (st, b) = open_link(&app, open("tablet"), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    assert_eq!(b["error"]["code"], "link_refused");
    assert!(!events_of(&h, "link-used-up").is_empty());
    // A counted device still renews.
    let (st, _) = open_link(&app, open("phone"), None).await;
    assert_eq!(st, StatusCode::OK);
    let listed = h.run(LINKS, ANA, &[]).await.expect("links").rows;
    assert_eq!(listed[0]["opensUsed"], 2);
    assert_eq!(listed[0]["state"], "used-up");

    // Raised, the tablet gets in. Lowered under the use, it is used up.
    let set = |v: &'static str| async {
        h.run(
            ordo_host::share_links::SET_LINK_MAX_OPENS,
            ANA,
            &[("link", id.as_str()), ("maxOpens", v)],
        )
        .await
    };
    let r = set("3").await.expect("raise").rows[0].clone();
    assert_eq!(r["state"], "live");
    assert_eq!(r["opensUsed"], 2);
    let (st, _) = open_link(&app, open("tablet"), None).await;
    assert_eq!(st, StatusCode::OK, "the refused device left no row behind");
    assert_eq!(set("1").await.expect("lower").rows[0]["state"], "used-up");
    let (st, _) = open_link(&app, open("desk"), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    set("none").await.expect("remove");
    let (st, _) = open_link(&app, open("desk"), None).await;
    assert_eq!(st, StatusCode::OK);
    let listed = h.run(LINKS, ANA, &[]).await.expect("links").rows;
    assert_eq!(listed[0]["maxOpens"], Value::Null);

    // A bad limit is refused naming the rule.
    for bad in ["0", "-1", "lots"] {
        let e = set(bad).await.expect_err("bad");
        assert!(e.to_string().contains("link_bad_max_opens"), "{e}");
    }
    // An unknown link id is not found.
    let e = h
        .run(
            ordo_host::share_links::SET_LINK_MAX_OPENS,
            ANA,
            &[("link", "lnk_nothere"), ("maxOpens", "2")],
        )
        .await
        .expect_err("unknown");
    assert!(e.to_string().contains("no such link"), "{e}");
}

/// A client that sends no device id cannot be told apart, so each open
/// counts, and wrong passwords never take a place.
#[tokio::test(flavor = "multi_thread")]
async fn opens_without_a_device_each_count_and_wrong_passwords_do_not() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_max_nodevice", on()).await;
    let minted = h
        .mint(
            ANA,
            &[
                ("ops", "boardList"),
                ("rows", ROWS),
                ("password", PASSWORD),
                ("maxOpens", "1"),
            ],
        )
        .await
        .expect("mint");
    let app = link_app(&h, NOW);
    for _ in 0..3 {
        let (st, _) = open_link(
            &app,
            json!({"link": minted["link"], "password": "not it"}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    let right = json!({"link": minted["link"], "password": PASSWORD});
    let (st, _) = open_link(&app, right.clone(), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = open_link(&app, right, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

/// Many opens at once from different devices cannot pass the limit.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_opens_at_the_limit_cannot_pass_it() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_max_race", on()).await;
    let minted = mint_limited(&h, "3").await;
    let id = minted["id"].as_str().unwrap().to_string();
    let app = link_app(&h, NOW);
    let mut tasks = Vec::new();
    for n in 0..12 {
        let (app, link) = (app.clone(), minted["link"].clone());
        tasks.push(tokio::spawn(async move {
            open_link(
                &app,
                json!({"link": link, "device": format!("dev-{n}")}),
                None,
            )
            .await
            .0
        }));
    }
    let mut ok = 0;
    for t in tasks {
        match t.await.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::FORBIDDEN => {}
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(ok, 3, "exactly the limit got in");
    let used = h
        .table(&format!(
            r#"SELECT "uses" FROM "$sharelink" WHERE "id" = '{id}'"#
        ))
        .await;
    assert_eq!(used[0][0].as_deref(), Some("3"));
    let devices = h
        .count(&format!(
            r#"SELECT count(*) FROM "$sharelinkdevice" WHERE "link" = '{id}'"#
        ))
        .await;
    assert_eq!(devices, 3, "refused devices left no row");
}

// ── the slowdown and the shortest password (owner, 2026-10-25) ─────

/// `POST /auth/link`, with the `Retry-After` header.
async fn open_link_waits(app: &axum::Router, body: Value) -> (StatusCode, Option<u64>, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/auth/link")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let wait = res
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        wait,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The door over another `ShareLinks` on the same database: a second node.
fn link_app_over(h: &Harness, links: Arc<ShareLinks>, now: u64) -> axum::Router {
    let mut state = AuthState::new(
        ProviderRegistry::new(),
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(h.recorder.clone());
    state.now = Box::new(move || now);
    state.wire_sharing(h.share.clone());
    state.wire_links(links);
    ordo_host::auth::router(Some(Arc::new(state)))
}

/// The `Throttled` entries on the stream, as (scope, count).
fn throttled_entries(h: &Harness) -> Vec<(String, u64)> {
    h.recorder
        .drain_all(ordo_commandlog::Timestamp(NOW + 10))
        .into_iter()
        .filter_map(|e| match &e.kind {
            EventKind::Denied(d) if d.contains(&ordo_secevent::RecordedDecision::Throttled) => {
                Some((
                    match e.scope {
                        Scope::Deployment => "deployment".to_string(),
                        Scope::Space(s) => format!("space:{}", s.0),
                    },
                    e.count,
                ))
            }
            _ => None,
        })
        .collect()
}

fn another_door(h: &Harness, settings: LinkSettings) -> Arc<ShareLinks> {
    Arc::new(
        ShareLinks::new(h.share.clone(), settings)
            .with_hasher(Arc::new(ordo_identity::Argon2idHasher::weak_for_tests())),
    )
}

/// 30 wrong passwords: the first 5 are refused as wrong, every one after
/// waits (`429 too_many_attempts`, `Retry-After`), and the burst is one
/// collapsed `Throttled` entry per stream. Even the right password waits
/// its turn; once the wait has passed it opens the link, and the count
/// starts again. Before 2026-10-25, 60 wrong tries went through unslowed.
#[tokio::test(flavor = "multi_thread")]
async fn thirty_wrong_link_passwords_are_slowed_and_the_right_one_opens_after_the_wait() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_slow", on()).await;
    let minted = mint_with_password(&h, PASSWORD).await;
    let app = link_app(&h, NOW);
    let wrong = json!({"link": minted["link"], "password": "not it"});
    let right = json!({"link": minted["link"], "password": PASSWORD});
    for i in 0..30 {
        let (st, wait, b) = open_link_waits(&app, wrong.clone()).await;
        if i < 5 {
            assert_eq!(st, StatusCode::UNAUTHORIZED, "try {i}: {b}");
            assert_eq!(b["error"]["code"], "link_password_wrong");
        } else {
            assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "try {i}: {b}");
            assert_eq!(b["error"]["code"], "too_many_attempts");
            assert_eq!(wait, Some(1), "try {i}");
            assert_eq!(b["error"]["retry_after"], 1);
            assert_eq!(b["error"]["setting"], SET_PASSWORD_LIMITS);
        }
    }
    let (st, _, b) = open_link_waits(&app, right.clone()).await;
    assert_eq!(
        st,
        StatusCode::TOO_MANY_REQUESTS,
        "the right one waits too: {b}"
    );
    let mut records = throttled_entries(&h);
    records.sort();
    assert_eq!(
        records,
        [
            ("deployment".to_string(), 26),
            (format!("space:{DEFAULT_SPACE}"), 26),
        ],
        "one collapsed entry per stream for the burst"
    );
    // No lock: the slowdown never locks.
    assert!(events_of(&h, "link-locked").is_empty());

    let later = link_app(&h, NOW + 1);
    let (st, _, b) = open_link_waits(&later, right).await;
    assert_eq!(st, StatusCode::OK, "after the wait: {b}");
    for _ in 0..5 {
        let (st, _, b) = open_link_waits(&later, wrong.clone()).await;
        assert_eq!(b["error"]["code"], "link_password_wrong", "{st} {b}");
    }
}

/// Two doors over one database are two nodes: the count is one.
#[tokio::test(flavor = "multi_thread")]
async fn the_link_slowdown_is_shared_between_two_doors_on_one_database() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_slow_two", on()).await;
    let minted = mint_with_password(&h, PASSWORD).await;
    let a = link_app(&h, NOW);
    let b = link_app_over(&h, another_door(&h, on()), NOW);
    let wrong = json!({"link": minted["link"], "password": "not it"});
    for i in 0..5 {
        let door = if i % 2 == 0 { &a } else { &b };
        let (_, _, out) = open_link_waits(door, wrong.clone()).await;
        assert_eq!(out["error"]["code"], "link_password_wrong");
    }
    for door in [&a, &b] {
        let (st, wait, out) = open_link_waits(door, wrong.clone()).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{out}");
        assert_eq!(wait, Some(1));
    }
}

/// Wrong tries on a fresh clock, past any wait: the codes answered.
async fn wrong_round(h: &Harness, link: &Value, at: u64, n: usize) -> Vec<String> {
    let app = link_app(h, at);
    let mut codes = Vec::new();
    for _ in 0..n {
        let (_, _, b) = open_link_waits(&app, json!({"link": link, "password": "not it"})).await;
        codes.push(b["error"]["code"].as_str().unwrap_or("").to_string());
    }
    codes
}

/// The app developer moves the threshold or turns the slowdown off; a
/// tenant moves it inside the app developer's range, and turns it off only
/// where that range starts at 0. Refusals name the setting.
#[tokio::test(flavor = "multi_thread")]
async fn the_link_slowdown_is_the_app_developers_and_the_tenants_to_change() {
    let _serial = SERIAL.lock().await;
    let h = harness("ordo_lnk_slow_layers", on()).await;
    let minted = mint_with_password(&h, PASSWORD).await;
    let link = minted["link"].clone();
    let id = minted["id"].as_str().unwrap().to_string();

    // A tenant slows after 2.
    let out = h
        .run(
            SET_PASSWORD_LIMITS,
            OWNER,
            &[("slowAfter", "2"), ("slowCapSecs", "30")],
        )
        .await
        .expect("a tenant's own")
        .rows;
    assert_eq!(out[0]["slowAfter"], 2);
    assert_eq!(out[0]["slowAfterFrom"], "tenant");
    assert_eq!(out[0]["slowCapSecs"], 30);
    assert_eq!(
        wrong_round(&h, &link, NOW + 1_000, 3).await,
        [
            "link_password_wrong",
            "link_password_wrong",
            "too_many_attempts"
        ]
    );

    // Off is refused to a tenant while the app developer set no range from 0.
    let e = h
        .run(SET_PASSWORD_LIMITS, OWNER, &[("slowAfter", "off")])
        .await
        .expect_err("off needs the app developer");
    assert!(e.to_string().contains("slowAfterRange"), "{e}");
    assert!(e.to_string().contains(SET_APP_PASSWORD_LIMITS), "{e}");

    // The app developer opens 0-10 to tenants; the tenant turns it off.
    h.run(
        SET_APP_PASSWORD_LIMITS,
        OWNER,
        &[("slowAfterRange", "0-10")],
    )
    .await
    .expect("the app developer's range");
    h.run(SET_PASSWORD_LIMITS, OWNER, &[("slowAfter", "off")])
        .await
        .expect("now a tenant may");
    h.run(
        ordo_host::share_links::SET_LINK_PASSWORD,
        ANA,
        &[("link", id.as_str()), ("password", PASSWORD)],
    )
    .await
    .expect("a new password starts the count again");
    assert!(wrong_round(&h, &link, NOW + 2_000, 12)
        .await
        .iter()
        .all(|c| c == "link_password_wrong"));

    // The tenant takes its value back, and the app developer's off holds.
    h.run(SET_PASSWORD_LIMITS, OWNER, &[("slowAfter", "default")])
        .await
        .expect("default");
    let out = h
        .run(SET_APP_PASSWORD_LIMITS, OWNER, &[("slowAfter", "off")])
        .await
        .expect("the app developer turns it off")
        .rows;
    assert_eq!(out[0]["slowAfter"], 0);
    assert_eq!(out[0]["slowAfterFrom"], "app");
    assert!(wrong_round(&h, &link, NOW + 3_000, 12)
        .await
        .iter()
        .all(|c| c == "link_password_wrong"));
}

/// A new link password is at least 8 characters, and the refusal names the
/// rule and the setting. A link given a shorter password before the rule
/// keeps it, and it still opens the link.
#[tokio::test(flavor = "multi_thread")]
async fn a_seven_character_link_password_is_refused_and_an_older_short_one_still_opens() {
    let _serial = SERIAL.lock().await;
    // The old deployment: a minimum of 4.
    let old = LinkSettings {
        password: PwDeployment::default()
            .with_limit(Field::MinLength, 1)
            .with_value(Field::MinLength, 4),
        ..on()
    };
    let h = harness("ordo_lnk_pw_eight", old).await;
    let short = mint_with_password(&h, "abcd").await;

    // Today's: the default, 8.
    let today = another_door(&h, on());
    let app = link_app_over(&h, today.clone(), NOW);
    let (st, _, b) =
        open_link_waits(&app, json!({"link": short["link"], "password": "abcd"})).await;
    assert_eq!(
        st,
        StatusCode::OK,
        "an older short password still opens: {b}"
    );

    let params = [
        ("link".to_string(), short["id"].clone()),
        ("password".to_string(), json!("seven77")),
    ]
    .into_iter()
    .collect();
    let e = today
        .execute(
            &h.engine,
            ordo_host::share_links::SET_LINK_PASSWORD,
            DEFAULT_SPACE,
            &BoundParams(params),
            ANA,
        )
        .await
        .expect_err("7 characters");
    let words = e.to_string();
    assert!(words.contains("link_bad_password"), "{words}");
    assert!(words.contains("8 to 256"), "{words}");
    assert!(
        words.contains(ordo_host::link_password::MIN_LENGTH_ENV),
        "{words}"
    );
}
