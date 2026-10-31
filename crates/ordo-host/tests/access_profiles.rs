//! **A tenant's own access profile** (27ah, owner ruling 2026-10-10): the
//! app developer's template is every tenant's default, and a tenant's
//! manager or owner changes it for that tenant alone, through the real door
//! against real Postgres.
//!
//! The claims:
//!
//! - the template is the default in every tenant, a new one included;
//! - an owner narrows an op to a group, and a member outside it is refused
//!   at the door, before anything runs;
//! - a per-member override allows and denies on top of role and group;
//! - two tenants' profiles are independent;
//! - every change is one command group in the tenant's own chain;
//! - a refusal names what to change;
//! - with no template, the door is as it was; what the deployment closes
//!   (a delete closed to delegated tokens) stays closed whatever a tenant
//!   opens.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::access_template::AccessTemplate;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::member_ops::{self, MemberOps};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::profile_ops::{self, ProfileOps, TenantAccess};
use ordo_host::runtime::SwappableAccess;
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, SpaceId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"access-profiles-test-secret-32b!";

// The conformance seed: mem_o owner, mem_1 manager, mem_2 and mem_3 editors,
// mem_4 viewer. Their principals are their ids.
const OWNER: &str = "mem_o";
const MANAGER: &str = "mem_1";
const EDITOR: &str = "mem_2";
const VIEWER: &str = "mem_4";

/// The second tenant, its owner and its viewer.
const TWIN: &str = "twin";
const TWIN_OWNER: &str = "mem_to";
const TWIN_VIEWER: &str = "mem_tv";

/// The app developer's template for this suite: the one report is for
/// owners and managers. Installed once for the whole test binary, before
/// any door is built, as the binary installs `ORDO_ACCESS_TEMPLATE`.
const TEMPLATE: &str = "activeProducts=owner,manager; productRevenue=owner";

/// A read the template does not name: every role keeps it.
const OPEN_READ: &str = "catInfo";
/// The report the template narrows.
const REPORT: &str = "activeProducts";
/// An op only an owner holds under the template: a manager cannot call it,
/// so a manager cannot hand it out.
const OWNER_ONLY: &str = "productRevenue";

fn install_template() {
    let platform: Vec<String> = platform_ops::specs().into_iter().map(|s| s.name).collect();
    ordo_host::access_template::install(
        AccessTemplate::parse(TEMPLATE, &platform).expect("the template parses"),
    );
}

struct Harness {
    state: ApiState,
    pool: ordo_exec::Pool,
    spaces: Arc<ordo_host::spaces::Spaces>,
}

async fn harness(db: &str) -> Harness {
    install_template();
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

    // The second tenant, before the door is built, with a viewer beside
    // its founding owner.
    ordo_host::provision::provision_space(
        &pool,
        f.keys.as_ref(),
        TWIN,
        &ordo_host::provision::FoundingOwner {
            member_id: TWIN_OWNER,
            principal: TWIN_OWNER,
            display_name: "Tam",
        },
    )
    .await
    .expect("the twin provisions");
    pool.rows(&format!(
        r#"INSERT INTO "members" ("$space", "id", "rev", "principal", "displayName", "kind",
             "memberCount", "role", "status", "joined")
           VALUES ('{TWIN}', '{TWIN_VIEWER}', 1, '{TWIN_VIEWER}', 'Tess', 'principal', 1,
             'viewer', 'active', DATE '2026-01-01')"#
    ))
    .await
    .expect("the twin's viewer");

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
        Box::new(ordo_host::runtime::now_secs),
        metrics.clone(),
    );
    let access = Arc::new(SwappableAccess::new(door));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "access-profiles-test",
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

    // The hook the runtime wires: rebuild the door from the tables.
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

    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory: None,
        stream: Some(access.recorder.clone()),
        refresh: Some(refresh),
        now: Arc::new(|| NOW),
        managed: None,
    });
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let live = access.clone();
    let profiles = Arc::new(ProfileOps {
        spaces: spaces.clone(),
        members: members.clone(),
        engine: Arc::new(move || live.current().engine.clone()),
    });
    let live = access.clone();
    let ops = Arc::new(
        PlatformOps::new(access.current().engine.clone(), grants, Arc::new(|| NOW))
            .with_engine_source(Arc::new(move || live.current().engine.clone()))
            .with_profiles(profiles)
            .with_members(members),
    );

    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: spaces.clone(),
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
        page_key: PageKey::from_secret(b"access-profiles-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        pool,
        spaces,
    }
}

fn home() -> String {
    ordo_conformance::fixture_space().0
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
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

/// A write op, as its caller sends it, in `space`.
async fn command(
    h: &Harness,
    space: &str,
    op: &str,
    who: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut body = body;
    body["$space"] = json!(space);
    send(
        &h.state,
        Request::builder()
            .method("POST")
            .uri(format!("/op/{op}"))
            .header("x-ordo-space", space)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", token(who)))
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

/// A read op, with its query string.
async fn read(h: &Harness, space: &str, op_and_query: &str, who: &str) -> (StatusCode, Value) {
    send(
        &h.state,
        Request::builder()
            .method("GET")
            .uri(format!("/op/{op_and_query}"))
            .header("x-ordo-space", space)
            .header("authorization", format!("Bearer {}", token(who)))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// Does the door let `who` call the read `op` in `space`?
async fn opens(h: &Harness, space: &str, op: &str, who: &str) -> bool {
    let (status, body) = read(h, space, op, who).await;
    match status {
        StatusCode::OK => true,
        StatusCode::FORBIDDEN => {
            assert_eq!(
                body["error"]["code"], "access_denied",
                "refused by the door, not by anything after it: {body}"
            );
            false
        }
        other => panic!("{op} as {who} in {space}: {other} {body}"),
    }
}

/// Make a group in the home tenant, as its owner, and answer its id.
async fn group(h: &Harness, name: &str, members: &[&str]) -> String {
    let (status, body) = command(
        h,
        &home(),
        profile_ops::CREATE_GROUP,
        OWNER,
        json!({ "name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["rows"][0]["group"]
        .as_str()
        .expect("the new group's id")
        .to_string();
    for m in members {
        let (status, body) = command(
            h,
            &home(),
            profile_ops::ADD_TO_GROUP,
            OWNER,
            json!({ "group": id, "member": m }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    id
}

/// The template is the default in every tenant: the report reaches owners
/// and managers and refuses the viewer at the door, in the seeded tenant and
/// in one made afterwards. An op the template does not name keeps the
/// built-in default.
#[tokio::test(flavor = "multi_thread")]
async fn the_template_is_the_default_in_every_tenant() {
    let h = harness("ordo_profiles_template").await;
    assert!(opens(&h, &home(), REPORT, OWNER).await);
    assert!(opens(&h, &home(), REPORT, MANAGER).await);
    assert!(!opens(&h, &home(), REPORT, EDITOR).await);
    assert!(!opens(&h, &home(), REPORT, VIEWER).await);
    assert!(opens(&h, TWIN, REPORT, TWIN_OWNER).await);
    assert!(!opens(&h, TWIN, REPORT, TWIN_VIEWER).await);
    assert!(opens(&h, &home(), OPEN_READ, VIEWER).await);
    assert!(opens(&h, TWIN, OPEN_READ, TWIN_VIEWER).await);

    // The profile read says where each line comes from.
    let (status, body) = read(
        &h,
        &home(),
        &format!("{}?op={REPORT}", profile_ops::PROFILE),
        MANAGER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"],
        json!([{
            "op": REPORT,
            "roles": ["owner", "manager"],
            "groups": [],
            "allow": [],
            "deny": [],
            "changed": false,
        }])
    );
}

/// An owner makes a group, gives it the report, and takes the report from
/// every role. The group's member is let in; a manager outside it is
/// refused at the door on the next request.
#[tokio::test(flavor = "multi_thread")]
async fn an_owner_narrows_an_op_to_a_group_and_others_are_refused_at_the_door() {
    let h = harness("ordo_profiles_group").await;
    let finance = group(&h, "Finance", &[VIEWER]).await;
    assert!(!opens(&h, &home(), REPORT, VIEWER).await);

    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        OWNER,
        json!({ "op": REPORT, "roles": "", "groups": finance }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["groups"], json!([finance]));

    assert!(opens(&h, &home(), REPORT, VIEWER).await, "in Finance");
    assert!(!opens(&h, &home(), REPORT, MANAGER).await, "not in Finance");
    assert!(!opens(&h, &home(), REPORT, OWNER).await, "not in Finance");

    // Out of the group, out of the report.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::REMOVE_FROM_GROUP,
        OWNER,
        json!({ "group": finance, "member": VIEWER }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!opens(&h, &home(), REPORT, VIEWER).await);

    // The groups read names the group and who is in it.
    let (status, body) = read(&h, &home(), profile_ops::GROUPS, MANAGER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"],
        json!([{ "group": finance, "name": "Finance", "members": [] }])
    );

    // Back to the template.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::RESET_OP_ACCESS,
        OWNER,
        json!({ "op": REPORT }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), REPORT, OWNER).await);
    assert!(!opens(&h, &home(), REPORT, VIEWER).await);
}

/// A per-member override allows one person what their role does not, and
/// denies one person what their role does. `default` removes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_override_allows_and_denies_one_person() {
    let h = harness("ordo_profiles_override").await;
    let set = |member: &'static str, effect: &'static str| json!({ "member": member, "op": REPORT, "effect": effect });
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        OWNER,
        set(VIEWER, "allow"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        OWNER,
        set(MANAGER, "deny"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), REPORT, VIEWER).await);
    assert!(!opens(&h, &home(), REPORT, MANAGER).await);
    assert!(opens(&h, &home(), REPORT, OWNER).await);

    let (_, body) = read(
        &h,
        &home(),
        &format!("{}?op={REPORT}", profile_ops::PROFILE),
        OWNER,
    )
    .await;
    assert_eq!(body["rows"][0]["allow"], json!([VIEWER]));
    assert_eq!(body["rows"][0]["deny"], json!([MANAGER]));

    for m in [VIEWER, MANAGER] {
        let (status, body) = command(
            &h,
            &home(),
            profile_ops::SET_MEMBER_ACCESS,
            OWNER,
            set(m, "default"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    assert!(!opens(&h, &home(), REPORT, VIEWER).await);
    assert!(opens(&h, &home(), REPORT, MANAGER).await);
}

/// One tenant opening the report to its viewers changes nothing in the
/// other tenant.
#[tokio::test(flavor = "multi_thread")]
async fn two_tenants_profiles_are_independent() {
    let h = harness("ordo_profiles_two").await;
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        OWNER,
        json!({ "op": REPORT, "roles": "owner,manager,editor,viewer" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), REPORT, VIEWER).await);
    assert!(
        !opens(&h, TWIN, REPORT, TWIN_VIEWER).await,
        "the twin still has the template"
    );

    // And the other way round: the twin's owner closes a read the home
    // tenant keeps open.
    let (status, body) = command(
        &h,
        TWIN,
        profile_ops::SET_OP_ACCESS,
        TWIN_OWNER,
        json!({ "op": OPEN_READ, "roles": "owner" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!opens(&h, TWIN, OPEN_READ, TWIN_VIEWER).await);
    assert!(opens(&h, &home(), OPEN_READ, VIEWER).await);

    // A tenant's owner cannot change another tenant's profile: they hold no
    // membership there, so the door refuses them.
    let (status, _) = command(
        &h,
        TWIN,
        profile_ops::SET_OP_ACCESS,
        OWNER,
        json!({ "op": OPEN_READ, "roles": "viewer" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// The tenant's durable chain, oldest first, as `(correlation, actor)`.
async fn chain(h: &Harness, space: &str) -> Vec<String> {
    h.pool
        .rows(&format!(
            r#"SELECT "correlation" FROM "$commandlog" WHERE "space" = '{space}' ORDER BY "idx""#
        ))
        .await
        .expect("chain")
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect()
}

/// Every change is one command group in the tenant's own chain, attributed
/// to the person who made it, with the row imaged before and after. The
/// other tenant's chain does not move.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_is_one_command_in_the_tenant_chain() {
    use ordo_commandlog::{Actor, CommandKind};
    let h = harness("ordo_profiles_chain").await;
    let home_before = chain(&h, &home()).await.len();
    let twin_before = chain(&h, TWIN).await.len();

    let finance = group(&h, "Finance", &[VIEWER]).await;
    assert_eq!(
        chain(&h, &home()).await.len(),
        home_before + 2,
        "a group made and a member added: two commands"
    );

    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        MANAGER,
        json!({ "op": REPORT, "roles": "owner", "groups": finance }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = chain(&h, &home()).await;
    assert_eq!(after.len(), home_before + 3);
    assert_eq!(chain(&h, TWIN).await.len(), twin_before);

    let bound = h.spaces.resolve(&home()).await.expect("the space");
    let group = {
        let log = bound.log.lock().unwrap();
        log.last_in(&ordo_commandlog::SpaceId(home()))
            .expect("a group")
    };
    assert_eq!(Some(&group.correlation.0), after.last());
    assert_eq!(group.actor, Actor::User(MANAGER.into()));
    assert_eq!(group.commands.len(), 1);
    let c = &group.commands[0];
    assert_eq!(c.entity, "$opaccess");
    assert_eq!(c.row_id, REPORT);
    assert_eq!(c.kind, CommandKind::Create);
    assert!(
        format!("{:?}", c.after).contains(&finance),
        "the line is imaged: {c:?}"
    );

    // A second change to the same line is a converge with both images.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        OWNER,
        json!({ "op": REPORT, "roles": "owner,manager" }),
    )
    .await;
    // The manager took their own role off the line, so only an owner can
    // put it back.
    assert_eq!(status, StatusCode::OK, "{body}");
    let group = {
        let log = bound.log.lock().unwrap();
        log.last_in(&ordo_commandlog::SpaceId(home()))
            .expect("a group")
    };
    assert_eq!(group.commands[0].kind, CommandKind::Converge);
    assert!(format!("{:?}", group.commands[0].before).contains(&finance));
}

/// A refusal names what to change. Platform ops are not a tenant's to list,
/// an unknown op points at the profile read, a word that is not a role
/// points at groups, and a person is not a group. An editor is refused at
/// the door.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_names_what_to_change() {
    let h = harness("ordo_profiles_refusals").await;
    let set =
        |body: Value| async { command(&h, &home(), profile_ops::SET_OP_ACCESS, OWNER, body).await };

    let (status, body) = set(json!({ "op": "std.space.delete", "roles": "viewer" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("platform op") && msg.contains("std.space.profile"),
        "{msg}"
    );

    let (status, body) = set(json!({ "op": "noSuchOp", "roles": "viewer" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("std.space.profile"),
        "{body}"
    );

    let (status, body) = set(json!({ "op": REPORT, "roles": "finance" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("std.space.createGroup"),
        "{body}"
    );

    let (status, body) = set(json!({ "op": REPORT, "groups": VIEWER })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("std.space.createGroup"),
        "{body}"
    );

    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        OWNER,
        json!({ "member": VIEWER, "op": REPORT, "effect": "maybe" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Groups may not hold groups.
    let a = group(&h, "A", &[]).await;
    let b = group(&h, "B", &[]).await;
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::ADD_TO_GROUP,
        OWNER,
        json!({ "group": a, "member": b }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("group_nesting"), "{body}");

    // An editor administers nothing here.
    let (status, _) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        EDITOR,
        json!({ "op": REPORT, "roles": "editor" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = read(&h, &home(), profile_ops::PROFILE, EDITOR).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Owner ruling 2026-10-10 (`op-role-limit` ask 5): a manager may change the
/// profile only to hand out ops the manager can call. Allowed: an op they
/// hold, and any narrowing. Refused, naming the op and saying only an owner
/// can grant it: an op they lack, for their own role, for a group's member,
/// or for one person. The owner does all of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_manager_hands_out_only_ops_the_manager_holds() {
    let h = harness("ordo_profiles_manager_limit").await;
    let set = |who: &'static str, body: Value| {
        let h = &h;
        async move { command(h, &home(), profile_ops::SET_OP_ACCESS, who, body).await }
    };
    let refusal = |status: StatusCode, body: &Value| {
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let msg = body["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains(OWNER_ONLY) && msg.contains("only an owner can grant"),
            "{msg}"
        );
    };

    // An op the manager holds: handed to editors, allowed. Narrowed, allowed.
    let (status, body) = set(
        MANAGER,
        json!({ "op": REPORT, "roles": "owner,manager,editor" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), REPORT, EDITOR).await);
    let (status, body) = set(MANAGER, json!({ "op": REPORT, "roles": "owner" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!opens(&h, &home(), REPORT, MANAGER).await);
    // Put the manager back: the owner may.
    let (status, body) = set(OWNER, json!({ "op": REPORT, "roles": "owner,manager" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // An op the manager lacks, to their own role: refused.
    let (status, body) = set(
        MANAGER,
        json!({ "op": OWNER_ONLY, "roles": "owner,manager" }),
    )
    .await;
    refusal(status, &body);
    assert!(!opens(&h, &home(), OWNER_ONLY, MANAGER).await);

    // A person's override for that op: refused for the manager.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        MANAGER,
        json!({ "member": VIEWER, "op": OWNER_ONLY, "effect": "allow" }),
    )
    .await;
    refusal(status, &body);
    // Denying is narrowing, so it is allowed.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        MANAGER,
        json!({ "member": VIEWER, "op": OWNER_ONLY, "effect": "deny" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A group whose ops exceed the manager's: the owner makes it, the
    // manager cannot put anyone in it.
    let ops_group = group(&h, "Revenue", &[]).await;
    let (status, body) = set(
        OWNER,
        json!({ "op": OWNER_ONLY, "roles": "owner", "groups": ops_group }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let add = |who: &'static str| {
        let (h, g) = (&h, ops_group.clone());
        async move {
            command(
                h,
                &home(),
                profile_ops::ADD_TO_GROUP,
                who,
                json!({ "group": g, "member": EDITOR }),
            )
            .await
        }
    };
    let (status, body) = add(MANAGER).await;
    refusal(status, &body);
    assert!(!opens(&h, &home(), OWNER_ONLY, EDITOR).await);
    // The owner may. Removing anyone from any group is narrowing.
    let (status, body) = add(OWNER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), OWNER_ONLY, EDITOR).await);
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::REMOVE_FROM_GROUP,
        MANAGER,
        json!({ "group": ops_group, "member": EDITOR }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The owner is not limited: the role, then the person.
    let (status, body) = set(OWNER, json!({ "op": OWNER_ONLY, "roles": "owner,manager" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), OWNER_ONLY, MANAGER).await);
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        OWNER,
        json!({ "member": VIEWER, "op": OWNER_ONLY, "effect": "allow" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), OWNER_ONLY, VIEWER).await);

    // Now the manager holds it, so handing it out is allowed.
    let (status, body) = set(
        MANAGER,
        json!({ "op": OWNER_ONLY, "roles": "owner,manager,editor" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Owner ruling 2026-10-10: a manager must not be able to give themselves
/// more access. Every route to their own access is shut: joining a group
/// with wider ops, writing a line for a group they are in, an override that
/// allows them an op, and lifting a deny an owner put on them. The owner
/// still may do each.
#[tokio::test(flavor = "multi_thread")]
async fn a_manager_cannot_raise_their_own_access() {
    let h = harness("ordo_profiles_manager_self").await;
    let refused = |status: StatusCode, body: &Value| {
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let msg = body["error"]["message"].as_str().unwrap_or_default();
        assert!(msg.contains("only an owner can grant"), "{msg}");
    };

    // 1. Joining a group with wider ops.
    let wide = group(&h, "Wide", &[]).await;
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        OWNER,
        json!({ "op": OWNER_ONLY, "roles": "owner", "groups": wide }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::ADD_TO_GROUP,
        MANAGER,
        json!({ "group": wide, "member": MANAGER }),
    )
    .await;
    refused(status, &body);
    assert!(!opens(&h, &home(), OWNER_ONLY, MANAGER).await);

    // 2. Editing a group the manager belongs to: a line that gives it an op
    // the manager lacks.
    let mine = group(&h, "Mine", &[MANAGER]).await;
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        MANAGER,
        json!({ "op": OWNER_ONLY, "roles": "owner", "groups": mine }),
    )
    .await;
    refused(status, &body);
    assert!(!opens(&h, &home(), OWNER_ONLY, MANAGER).await);

    // 3. An override that allows themselves the op.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        MANAGER,
        json!({ "member": MANAGER, "op": OWNER_ONLY, "effect": "allow" }),
    )
    .await;
    refused(status, &body);
    assert!(!opens(&h, &home(), OWNER_ONLY, MANAGER).await);

    // 4. Lifting a deny an owner put on them.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        OWNER,
        json!({ "member": MANAGER, "op": REPORT, "effect": "deny" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!opens(&h, &home(), REPORT, MANAGER).await);
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_MEMBER_ACCESS,
        MANAGER,
        json!({ "member": MANAGER, "op": REPORT, "effect": "default" }),
    )
    .await;
    refused(status, &body);
    assert!(!opens(&h, &home(), REPORT, MANAGER).await);

    // The owner may put the manager in the wide group.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::ADD_TO_GROUP,
        OWNER,
        json!({ "group": wide, "member": MANAGER }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), OWNER_ONLY, MANAGER).await);
}

/// `std.space.deleteGroup`: refused while a profile line names the group, and
/// nothing changes; once the line is changed, the group's links and its row go
/// in one command group, and a manager may do it.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_a_profile_line_names_is_refused_until_the_line_is_changed() {
    let h = harness("ordo_profiles_delgroup").await;
    let finance = group(&h, "Finance", &[VIEWER, EDITOR]).await;
    let ops = group(&h, "Ops", &[VIEWER]).await;
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        OWNER,
        json!({ "op": REPORT, "roles": "owner", "groups": format!("{finance},{ops}") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(opens(&h, &home(), REPORT, EDITOR).await, "in Finance");

    // An editor may not.
    let (status, _) = command(
        &h,
        &home(),
        profile_ops::DELETE_GROUP,
        EDITOR,
        json!({ "group": finance }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A manager is refused while a line names the group. Nothing changes.
    let before = chain(&h, &home()).await.len();
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::DELETE_GROUP,
        MANAGER,
        json!({ "group": finance }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("group_in_use") && msg.contains(REPORT) && msg.contains("setOpAccess"),
        "{msg}"
    );
    assert_eq!(chain(&h, &home()).await.len(), before, "nothing recorded");
    assert!(opens(&h, &home(), REPORT, EDITOR).await, "still in Finance");

    // Take Finance out of the line, then the delete goes through.
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::SET_OP_ACCESS,
        OWNER,
        json!({ "op": REPORT, "roles": "owner", "groups": ops }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let before = chain(&h, &home()).await.len();
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::DELETE_GROUP,
        MANAGER,
        json!({ "group": finance }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["outcome"], "deleted", "{body}");
    assert_eq!(body["rows"][0]["links"], 2, "{body}");
    assert_eq!(
        chain(&h, &home()).await.len(),
        before + 1,
        "one command group for the whole delete"
    );

    // Gone from the row, the links and the door.
    let n = |sql: String| {
        let pool = h.pool.clone();
        async move {
            pool.rows(&sql).await.expect("query")[0][0]
                .clone()
                .unwrap_or_default()
        }
    };
    assert_eq!(
        n(format!(
            r#"SELECT count(*) FROM "members" WHERE "id" = '{finance}'"#
        ))
        .await,
        "0"
    );
    assert_eq!(
        n(format!(
            r#"SELECT count(*) FROM "groupMembers" WHERE "group" = '{finance}'"#
        ))
        .await,
        "0"
    );
    let (status, body) = read(
        &h,
        &home(),
        &format!("{}?op={REPORT}", profile_ops::PROFILE),
        OWNER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["groups"], json!([ops]), "{body}");
    let (_, body) = read(&h, &home(), profile_ops::GROUPS, OWNER).await;
    assert_eq!(
        body["rows"],
        json!([{ "group": ops, "name": "Ops", "members": [VIEWER] }])
    );
    assert!(!opens(&h, &home(), REPORT, EDITOR).await, "Finance is gone");
    assert!(opens(&h, &home(), REPORT, VIEWER).await, "still in Ops");

    // A second delete, and a person's id, are not groups.
    for id in [finance.as_str(), VIEWER] {
        let (status, body) = command(
            &h,
            &home(),
            profile_ops::DELETE_GROUP,
            OWNER,
            json!({ "group": id }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
}

/// A group an access grant still names is not deleted, and nothing about it
/// changes: the refusal rolls the whole command back.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_an_access_grant_still_names_is_not_deleted() {
    let h = harness("ordo_profiles_delgroup_used").await;
    let used = group(&h, "Board readers", &[VIEWER]).await;
    h.pool
        .rows(&format!(
            r#"INSERT INTO "boardsAccess" ("$space", "id", "board", "member", "level")
               VALUES ('{}', 'ba_used', 'board_1', '{used}', 'read')"#,
            home()
        ))
        .await
        .expect("the grant seeds");
    let before = chain(&h, &home()).await.len();
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::DELETE_GROUP,
        OWNER,
        json!({ "group": used }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("group_in_use") && msg.contains("grants"),
        "{msg}"
    );
    assert_eq!(chain(&h, &home()).await.len(), before, "nothing recorded");
    let (_, body) = read(&h, &home(), profile_ops::GROUPS, OWNER).await;
    assert_eq!(
        body["rows"],
        json!([{ "group": used, "name": "Board readers", "members": [VIEWER] }]),
        "the group and its link are still there"
    );
}

// ── The engine, without a database ────────────────────────────────────────

fn seat(space: &str, id: &str, role: &str) -> Vec<Option<String>> {
    vec![
        Some(space.into()),
        Some(id.into()),
        Some(role.into()),
        Some("active".into()),
        Some(id.into()),
        Some("principal".into()),
    ]
}

fn engine_with(template: &AccessTemplate, tenant: &TenantAccess) -> ordo_identity::AccessEngine {
    let f = Fixture::load().expect("fixture");
    ordo_host::engine_from_directory(
        &f,
        &[seat("s1", "v1", "viewer"), seat("s1", "o1", "owner")],
        tenant,
        &Default::default(),
        &Default::default(),
        &ordo_host::delegable::DelegablePolicy::closed(),
        template,
    )
    .expect("engine")
}

fn reaches(e: &ordo_identity::AccessEngine, who: &str, op: &str) -> bool {
    e.membership_reaches(&SpaceId("s1".into()), &PrincipalId(who.into()), op)
}

/// With no template the door is what it was: every role reads, editors and
/// above write, std is for managers and owners.
#[test]
fn no_template_keeps_the_built_in_default() {
    let e = engine_with(&AccessTemplate::none(), &TenantAccess::default());
    assert!(reaches(&e, "v1", REPORT));
    assert!(!reaches(&e, "v1", "addTask"));
    assert!(!reaches(&e, "v1", "std.tasks.get"));
    assert!(reaches(&e, "o1", "std.tasks.get"));
    let platform: Vec<String> = platform_ops::specs().into_iter().map(|s| s.name).collect();
    let t = AccessTemplate::parse(TEMPLATE, &platform).unwrap();
    let e = engine_with(&t, &TenantAccess::default());
    assert!(!reaches(&e, "v1", REPORT));
    assert!(reaches(&e, "o1", REPORT));
}

/// A tenant may open a delete to its viewers; a delete closed to delegated
/// tokens stays closed, because that is the deployment's and not the
/// profile's.
#[test]
fn a_delete_closed_to_delegation_stays_closed_whatever_a_tenant_opens() {
    let line = |op: &str, roles: &str| {
        vec![
            Some("s1".into()),
            Some(op.into()),
            Some(roles.into()),
            Some(String::new()),
        ]
    };
    let tenant = TenantAccess {
        lines: vec![
            line("purgeDone", "viewer"),
            line("std.space.delete", "viewer"),
        ],
        ..Default::default()
    };
    let e = engine_with(&AccessTemplate::none(), &tenant);
    assert!(reaches(&e, "v1", "purgeDone"), "the tenant opened it");
    assert!(!e.delegable("purgeDone"), "and it stays closed to agents");
    assert!(
        !reaches(&e, "v1", "std.space.delete"),
        "a platform op in a tenant's rows is never read"
    );
    let program: BTreeSet<String> = ordo_host::build_specs(&Fixture::load().unwrap())
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(program.contains("purgeDone"));
}

/// A tenant's session settings (owner, 2026-10-21): the app developer closes
/// the setter to tenants through the template, and keeps the read open for a
/// page that shows the values. A tenant's own rows cannot open it again.
#[test]
fn the_app_developer_closes_the_session_setter_and_keeps_the_read() {
    use ordo_host::session_policy::{READ_TENANT, SET_TENANT};
    let e = engine_with(&AccessTemplate::none(), &TenantAccess::default());
    assert!(
        reaches(&e, "o1", SET_TENANT),
        "the tenant owner's by default"
    );
    assert!(!reaches(&e, "v1", SET_TENANT));
    assert!(
        !reaches(&e, "v1", READ_TENANT),
        "owners and managers read by default"
    );

    let platform: Vec<String> = platform_ops::specs().into_iter().map(|s| s.name).collect();
    let t = AccessTemplate::parse(
        &format!("{SET_TENANT}=; {READ_TENANT}=owner,manager,editor,viewer"),
        &platform,
    )
    .expect("the two session ops are the template's to name");
    let line = |op: &str, roles: &str| {
        vec![
            Some("s1".into()),
            Some(op.into()),
            Some(roles.into()),
            Some(String::new()),
        ]
    };
    let tenant = TenantAccess {
        lines: vec![line(SET_TENANT, "owner")],
        ..Default::default()
    };
    let e = engine_with(&t, &tenant);
    // The owner passes the door, so the op can say how it is allowed; no
    // other role does.
    assert!(reaches(&e, "o1", SET_TENANT));
    assert!(!reaches(&e, "v1", SET_TENANT));
    let why = ordo_host::session_policy::closed_by_template(
        &t,
        Some(ordo_identity::principal::Role::Owner),
    )
    .expect("closed to the owner");
    assert!(why.contains("ORDO_ACCESS_TEMPLATE"), "{why}");
    assert!(why.contains("std.space.setSessionPolicy=owner"), "{why}");
    assert!(why.contains("app developer"), "{why}");
    let open = AccessTemplate::parse(&format!("{SET_TENANT}=owner"), &platform).unwrap();
    assert!(ordo_host::session_policy::closed_by_template(
        &open,
        Some(ordo_identity::principal::Role::Owner)
    )
    .is_none());
    assert!(
        ordo_host::session_policy::closed_by_template(
            &AccessTemplate::none(),
            Some(ordo_identity::principal::Role::Owner)
        )
        .is_none(),
        "no template line: the platform's default"
    );
    assert!(reaches(&e, "o1", READ_TENANT), "the read still answers");
    assert!(reaches(&e, "v1", READ_TENANT), "opened to every role");

    // Every other platform op stays out of the template's reach.
    let err =
        AccessTemplate::parse("std.space.setShareLinkLifetime=viewer", &platform).unwrap_err();
    assert!(err.contains("platform op"), "{err}");
}
