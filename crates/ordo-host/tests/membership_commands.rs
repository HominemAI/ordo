//! **Enrolment E3: the membership commands** (checklist 22h): admit, change
//! role, suspend, reinstate, remove, through the real door against real
//! Postgres.
//!
//! The claims, in the order they matter:
//!
//! - a suspension is refused on the **next request**, not after the refresh
//!   period. The door here is a `SwappableAccess` and the command's own
//!   refresh hook is `refresh_access`, exactly as the runtime wires it, and
//!   nothing in this file waits for a timer;
//! - the last owner cannot be removed, suspended or given a lower role;
//! - only an owner may run any of them.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::member_ops::{self, MemberOps};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::runtime::SwappableAccess;
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"membership-commands-secret-32by!";

// The conformance seed: mem_o owner, mem_1 manager, mem_2 and mem_3 editors,
// mem_4 viewer. Their principals are their ids.
const OWNER: &str = "mem_o";
const MANAGER: &str = "mem_1";
const VIEWER: &str = "mem_4";

struct Harness {
    access: Arc<SwappableAccess>,
    state: ApiState,
    pool: ordo_exec::Pool,
    space: String,
}

async fn harness(db: &str) -> Harness {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("platform table");
    let delegations = Arc::new(ordo_host::delegation::PgDelegationRegistry::new(
        pool.clone(),
    ));
    delegations.ensure_table().await.expect("delegations");

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let engine = ordo_host::deployment_engine_pooled(&f, &pool)
        .await
        .expect("engine");
    let door = ordo_host::IdentityAccess::sharing(
        Arc::new(engine),
        TokenKey::from_secret(&SECRET),
        // The wall clock, because the door a refresh builds reads it and
        // the two must agree about when a token was fresh.
        Box::new(ordo_host::runtime::now_secs),
        metrics.clone(),
    );
    let access = Arc::new(SwappableAccess::new(door));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "membership-commands-test",
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

    // The hook the runtime wires: rebuild the door from the table.
    let credentials: Arc<dyn ordo_exec::route::DbCredentials> =
        Arc::new(ordo_exec::route::Installed::new().as_configured("local"));
    let refresh: member_ops::Refresh = {
        let (deployment, pool, metrics) = (deployment.clone(), pool.clone(), metrics.clone());
        let (access, delegations, credentials) =
            (access.clone(), delegations.clone(), credentials.clone());
        Arc::new(move |_space: &str| {
            let (deployment, pool, metrics) = (deployment.clone(), pool.clone(), metrics.clone());
            let (access, delegations, credentials) =
                (access.clone(), delegations.clone(), credentials.clone());
            Box::pin(async move {
                ordo_host::runtime::refresh_access(
                    &deployment,
                    &pool,
                    &SECRET,
                    &metrics,
                    &access,
                    delegations,
                    &credentials,
                )
                .await
                .map_err(|e| e.to_string())
            })
        })
    };

    let live = access.clone();
    let ops = Arc::new(
        PlatformOps::new(access.current().engine.clone(), grants, Arc::new(|| NOW))
            .with_engine_source(Arc::new(move || live.current().engine.clone()))
            .with_members(Arc::new(MemberOps {
                control: pool.clone(),
                pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
                directory: None,
                stream: Some(access.recorder.clone()),
                refresh: Some(refresh),
                now: Arc::new(|| NOW),
                managed: None,
            })),
    );

    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"membership-commands-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        access,
        state,
        pool,
        space: ordo_conformance::fixture_space().0,
    }
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            // Far ahead of the real clock as well as the pinned one: the door
            // a refresh builds reads the wall clock, as the runtime's does.
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: Some(IStamp(ordo_host::runtime::now_secs())),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn send(state: &ApiState, req: Request<Body>) -> (StatusCode, Value) {
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

/// A command, as its caller sends it.
async fn command(h: &Harness, op: &str, who: &str, body: Value) -> (StatusCode, Value) {
    let mut body = body;
    body["$space"] = json!(h.space);
    send(
        &h.state,
        Request::builder()
            .method("POST")
            .uri(format!("/op/{op}"))
            .header("x-ordo-space", &h.space)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", token(who)))
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

/// A read a viewer may make, to ask whether the door is open to somebody.
async fn door_is_open_to(h: &Harness, who: &str) -> bool {
    let (status, body) = send(
        &h.state,
        Request::builder()
            .method("GET")
            .uri("/op/activeProducts")
            .header("x-ordo-space", &h.space)
            .header("authorization", format!("Bearer {}", token(who)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    match status {
        StatusCode::OK => true,
        StatusCode::FORBIDDEN => false,
        other => panic!("unexpected {other}: {body}"),
    }
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

async fn row(h: &Harness, member: &str) -> Option<(String, String)> {
    let rows = h
        .pool
        .rows(&format!(
            "SELECT \"role\", \"status\" FROM \"members\" WHERE \"$space\" = '{}' AND \"id\" = '{member}'",
            h.space
        ))
        .await
        .expect("read");
    rows.first().map(|r| {
        (
            r[0].clone().unwrap_or_default(),
            r[1].clone().unwrap_or_default(),
        )
    })
}

/// **A suspension through the command is refused on the next request.**
/// Nothing between the two calls waits for the refresh timer.
#[tokio::test(flavor = "multi_thread")]
async fn a_suspension_through_the_command_is_refused_on_the_next_request() {
    let h = harness("ordo_members_suspend").await;
    assert!(door_is_open_to(&h, VIEWER).await, "the control: open first");

    let (status, body) = command(&h, member_ops::SUSPEND, OWNER, json!({"member": VIEWER})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["outcome"], "suspended");
    assert!(
        body["rows"][0]["door"]
            .as_str()
            .unwrap()
            .starts_with("rebuilt here"),
        "{body}"
    );

    assert!(
        !door_is_open_to(&h, VIEWER).await,
        "the very next request must be refused"
    );

    let (status, body) = command(&h, member_ops::REINSTATE, OWNER, json!({"member": VIEWER})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        door_is_open_to(&h, VIEWER).await,
        "and reinstating opens it on the next request too"
    );

    // Both acts are on the record, in the space's own stream.
    let audit = h.access.current().take_audit().join("\n");
    assert!(audit.contains("membership-suspended"), "{audit}");
    assert!(audit.contains("membership-reactivated"), "{audit}");
}

/// The whole life of a membership: admitted through the command, reached
/// through the person it names and not through the member id, re-roled,
/// removed.
#[tokio::test(flavor = "multi_thread")]
async fn an_admitted_person_is_reached_by_principal_and_can_be_re_roled_and_removed() {
    let h = harness("ordo_members_life").await;
    assert!(
        !door_is_open_to(&h, "usr_newcomer").await,
        "a stranger first"
    );

    let (status, body) = command(
        &h,
        member_ops::ADMIT,
        OWNER,
        json!({"principal": "usr_newcomer", "role": "viewer", "display": "New"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let member = body["rows"][0]["member"].as_str().unwrap().to_string();
    assert!(member.starts_with("mem_"));
    assert_ne!(
        member, "usr_newcomer",
        "the member id is minted, not the principal"
    );
    assert_eq!(
        row(&h, &member).await,
        Some(("viewer".into(), "active".into()))
    );

    assert!(
        door_is_open_to(&h, "usr_newcomer").await,
        "reached by the person"
    );
    assert!(
        !door_is_open_to(&h, &member).await,
        "and not by the member id, which names nobody the door knows"
    );

    let (status, body) = command(
        &h,
        member_ops::SET_ROLE,
        OWNER,
        json!({"member": member, "role": "editor"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        row(&h, &member).await,
        Some(("editor".into(), "active".into()))
    );

    let (status, body) = command(&h, member_ops::REMOVE, OWNER, json!({"member": member})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(row(&h, &member).await, None);
    assert!(
        !door_is_open_to(&h, "usr_newcomer").await,
        "a removed member is refused on the next request"
    );
}

/// **The last owner cannot be removed**, nor suspended, nor demoted. With a
/// second owner in place the same commands succeed.
#[tokio::test(flavor = "multi_thread")]
async fn the_last_owner_cannot_be_removed_suspended_or_demoted() {
    let h = harness("ordo_members_last_owner").await;
    for (op, body) in [
        (member_ops::REMOVE, json!({"member": OWNER})),
        (member_ops::SUSPEND, json!({"member": OWNER})),
        (
            member_ops::SET_ROLE,
            json!({"member": OWNER, "role": "editor"}),
        ),
    ] {
        let (status, out) = command(&h, op, OWNER, body).await;
        assert_ne!(status, StatusCode::OK, "{op} must be refused: {out}");
        assert_eq!(code(&out), "tenant_refused", "{op}: {out}");
    }
    assert_eq!(
        row(&h, OWNER).await,
        Some(("owner".into(), "active".into()))
    );

    // A second owner makes the first removable.
    let (status, body) = command(
        &h,
        member_ops::ADMIT,
        OWNER,
        json!({"principal": "usr_second", "role": "owner"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let second = body["rows"][0]["member"].as_str().unwrap().to_string();

    let (status, body) = command(
        &h,
        member_ops::REMOVE,
        "usr_second",
        json!({"member": OWNER}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(row(&h, OWNER).await, None);

    // And now the second is the last.
    let (status, out) = command(
        &h,
        member_ops::REMOVE,
        "usr_second",
        json!({"member": second}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{out}");
    assert_eq!(code(&out), "tenant_refused");
}

/// Only an owner runs any of them. A manager and a viewer are refused with
/// the door's one answer, for every command.
#[tokio::test(flavor = "multi_thread")]
async fn only_an_owner_may_administer_membership() {
    let h = harness("ordo_members_owner_only").await;
    for who in [MANAGER, VIEWER] {
        for (op, body) in [
            (
                member_ops::ADMIT,
                json!({"principal": "usr_x", "role": "viewer"}),
            ),
            (
                member_ops::SET_ROLE,
                json!({"member": VIEWER, "role": "owner"}),
            ),
            (member_ops::SUSPEND, json!({"member": "mem_2"})),
            (member_ops::REINSTATE, json!({"member": "mem_2"})),
            (member_ops::REMOVE, json!({"member": "mem_2"})),
        ] {
            let (status, out) = command(&h, op, who, body).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{who} {op}: {out}");
            assert_eq!(code(&out), "access_denied");
        }
    }
    assert_eq!(
        row(&h, "mem_2").await,
        Some(("editor".into(), "active".into()))
    );
}

/// Refusals with a reason the owner can act on: a second admission of one
/// person, a role that is not one, a member the space does not have.
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_a_bad_role_and_an_unknown_member_are_refused_by_name() {
    let h = harness("ordo_members_refusals").await;

    let (_, first) = command(
        &h,
        member_ops::ADMIT,
        OWNER,
        json!({"principal": "usr_once", "role": "viewer"}),
    )
    .await;
    assert_eq!(first["rows"][0]["outcome"], "admitted");
    let (status, out) = command(
        &h,
        member_ops::ADMIT,
        OWNER,
        json!({"principal": "usr_once", "role": "editor"}),
    )
    .await;
    assert_eq!(code(&out), "duplicate", "{status} {out}");

    let (_, out) = command(
        &h,
        member_ops::ADMIT,
        OWNER,
        json!({"principal": "usr_role", "role": "emperor"}),
    )
    .await;
    assert_eq!(code(&out), "tenant_refused", "{out}");

    let (status, out) = command(
        &h,
        member_ops::SUSPEND,
        OWNER,
        json!({"member": "mem_nobody"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    assert_eq!(code(&out), "not_found");
}

/// Membership in one space is not membership in another: the same commands
/// run against the twin space touch only its rows.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_in_one_space_does_not_reach_another() {
    let h = harness("ordo_members_scope").await;
    h.pool
        .rows(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
             VALUES ('twin', 'mem_o', 1, 'mem_o', 'Twin owner', 'principal', 1, 'owner', 'active', CURRENT_DATE), \
                    ('twin', 'mem_4', 1, 'mem_4', 'Twin viewer', 'principal', 1, 'viewer', 'active', CURRENT_DATE)",
        )
        .await
        .expect("twin rows");
    let (status, body) = command(&h, member_ops::SUSPEND, OWNER, json!({"member": VIEWER})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let twin: Vec<_> = h
        .pool
        .rows("SELECT \"status\" FROM \"members\" WHERE \"$space\" = 'twin' AND \"id\" = 'mem_4'")
        .await
        .unwrap();
    assert_eq!(twin[0][0].as_deref(), Some("active"));
}
