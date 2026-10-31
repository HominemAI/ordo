//! G5 (`plan-console.md` 4.5, step 8): `std.space.members`, `grants`,
//! `groupMembers` and `callableBy` through the real door against real
//! Postgres, and the record each People write leaves (G12).
//!
//! The claims:
//!
//! - an owner and a manager read each of the four; an editor and a viewer
//!   are refused at the door with the one `access_denied`;
//! - a person with no role in a tenant reads nothing there;
//! - `callableBy` names only ops the asker may call too, and an id the
//!   tenant does not hold is `not_found`;
//! - invite, change a role, add to a group and remove each leave a record
//!   naming who did it;
//! - so do the rest of the People screen's writes (step 8, session 2):
//!   suspend and reinstate, make and delete a group, take a member out of
//!   one, change the access profile and a member's own access, and revoke
//!   an invitation; and the screen's two reads (`profile`, `invitations`)
//!   answer it.
//!
//! The console session's window is held in `console_key.rs`.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::invite_ops::{self, InviteOps};
use ordo_host::member_ops::{self, MemberOps};
use ordo_host::people_ops;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::profile_ops::{self, ProfileOps};
use ordo_host::runtime::SwappableAccess;
use ordo_host::tickets::{MemoryTicketStore, Tickets};
use ordo_host::FixtureExecutor;
use ordo_identity::grants::{GrantRegistry, NewGrant};
use ordo_identity::principal::{ClientId, GrantId, PrincipalId, SpaceId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"people-reads-test-secret-32byte!";

// The conformance seed: mem_o owner, mem_1 manager, mem_2 and mem_3 editors,
// mem_4 viewer. Their principals are their ids.
const OWNER: &str = "mem_o";
const MANAGER: &str = "mem_1";
const EDITOR: &str = "mem_2";
const EDITOR_2: &str = "mem_3";
const VIEWER: &str = "mem_4";

/// The second tenant, its owner and its viewer. The home tenant's owner
/// holds no role here.
const TWIN: &str = "twin";
const TWIN_OWNER: &str = "mem_to";
const TWIN_VIEWER: &str = "mem_tv";

struct Harness {
    state: ApiState,
    grants: Arc<PgGrantRegistry>,
    recorder: Arc<ordo_secevent::Recorder>,
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
    let grants_handle = grants.clone();
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
        "people-reads-test",
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
            .with_members(members)
            .with_delegations(delegations.clone())
            .with_invite(Arc::new(InviteOps {
                tickets: Arc::new(Tickets::new(Arc::new(MemoryTicketStore::new()))),
                stream: Some(access.recorder.clone()),
                now: Arc::new(|| NOW),
            })),
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
        page_key: PageKey::from_secret(b"people-reads-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        grants: grants_handle,
        recorder: access.recorder.clone(),
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

fn rows_of(body: &Value) -> Vec<Value> {
    body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows in {body}"))
        .clone()
}

fn ids(body: &Value, field: &str) -> BTreeSet<String> {
    rows_of(body)
        .iter()
        .filter_map(|r| r[field].as_str().map(str::to_string))
        .collect()
}

/// Refused by the door, with its one answer.
fn denied(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "access_denied", "{body}");
}

fn not_found(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "not_found", "{body}");
}

/// `std.space.members`: the people with their roles and groups, never a
/// group as a person; paged by member id. Owner and manager read it; an
/// editor, a viewer and the owner of another tenant are refused.
#[tokio::test(flavor = "multi_thread")]
async fn members_answers_the_people_to_owner_and_manager_only() {
    let h = harness("ordo_people_members").await;
    let ops_group = group(&h, "Ops", &[EDITOR]).await;
    for who in [OWNER, MANAGER] {
        let (status, body) = read(&h, &home(), people_ops::MEMBERS, who).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let found = ids(&body, "member");
        for m in [OWNER, MANAGER, EDITOR, EDITOR_2, VIEWER] {
            assert!(found.contains(m), "{m} missing: {body}");
        }
        assert!(!found.contains(&ops_group), "a group is not a person");
        let editor = rows_of(&body)
            .into_iter()
            .find(|r| r["member"] == EDITOR)
            .expect("the editor");
        assert_eq!(editor["role"], "editor");
        assert_eq!(editor["status"], "active");
        assert_eq!(editor["groups"], json!([ops_group]));
    }
    // A page from a member id on.
    let (status, body) = read(
        &h,
        &home(),
        &format!("{}?after={EDITOR_2}", people_ops::MEMBERS),
        OWNER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(ids(&body, "member").iter().all(|m| m.as_str() > EDITOR_2));
    for who in [EDITOR, VIEWER] {
        let (status, body) = read(&h, &home(), people_ops::MEMBERS, who).await;
        denied(status, &body);
    }
    // The home tenant's owner holds no role in the twin.
    let (status, body) = read(&h, TWIN, people_ops::MEMBERS, OWNER).await;
    denied(status, &body);
    // Its own owner reads it, and only its people.
    let (status, body) = read(&h, TWIN, people_ops::MEMBERS, TWIN_OWNER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body, "member"),
        [TWIN_OWNER, TWIN_VIEWER].map(String::from).into()
    );
}

/// `std.space.groupMembers`: who is in one group; a group this tenant does
/// not hold, and a person's member id, are `not_found`.
#[tokio::test(flavor = "multi_thread")]
async fn group_members_lists_one_group_of_this_tenant() {
    let h = harness("ordo_people_groups").await;
    let ops_group = group(&h, "Ops", &[EDITOR, VIEWER]).await;
    let _other = group(&h, "Sales", &[MANAGER]).await;
    let q = |g: &str| format!("{}?group={g}", people_ops::GROUP_MEMBERS);
    for who in [OWNER, MANAGER] {
        let (status, body) = read(&h, &home(), &q(&ops_group), who).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            ids(&body, "member"),
            [EDITOR, VIEWER].map(String::from).into()
        );
    }
    for g in ["mem_nosuch", EDITOR, "bad.id"] {
        let (status, body) = read(&h, &home(), &q(g), OWNER).await;
        not_found(status, &body);
    }
    // The twin's owner cannot reach the home tenant's group from the twin.
    let (status, body) = read(&h, TWIN, &q(&ops_group), TWIN_OWNER).await;
    not_found(status, &body);
    for who in [EDITOR, VIEWER] {
        let (status, body) = read(&h, &home(), &q(&ops_group), who).await;
        denied(status, &body);
    }
}

async fn plant(h: &Harness, id: &str, principal: &str, ops: &[&str], expires: u64) {
    h.grants
        .record(&NewGrant {
            id: GrantId(id.into()),
            space: SpaceId(home()),
            principal: PrincipalId(principal.into()),
            client: Some(ClientId("cli_mail".into())),
            pins: ops
                .iter()
                .map(|op| {
                    (
                        op.to_string(),
                        ordo_identity::OpPin {
                            manifest_hash: ordo_identity::ManifestHash([9; 32]),
                            unguarded: false,
                            entities: Vec::new(),
                        },
                    )
                })
                .collect(),
            granted_at: IStamp(10),
            expires: IStamp(expires),
        })
        .await
        .expect("recorded");
}

/// `std.space.grants`: one row a live grant, its ops gathered, whose it is
/// and which client. An expired grant is gone. Owner and manager only.
#[tokio::test(flavor = "multi_thread")]
async fn grants_lists_each_live_grant_with_its_ops() {
    let h = harness("ordo_people_grants").await;
    plant(
        &h,
        "grt_live",
        EDITOR,
        &["catInfo", "activeProducts"],
        NOW + 3_600,
    )
    .await;
    plant(&h, "grt_gone", VIEWER, &["catInfo"], NOW - 1).await;
    for who in [OWNER, MANAGER] {
        let (status, body) = read(&h, &home(), people_ops::GRANTS, who).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let rows = rows_of(&body);
        assert_eq!(rows.len(), 1, "{body}");
        let g = &rows[0];
        assert_eq!(g["grant"], "grt_live");
        assert_eq!(g["kind"], "token");
        assert_eq!(g["principal"], EDITOR);
        assert_eq!(g["member"], EDITOR);
        assert_eq!(g["client"], "cli_mail");
        let ops: BTreeSet<&str> = g["ops"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(ops, ["activeProducts", "catInfo"].into());
    }
    for who in [EDITOR, VIEWER] {
        let (status, body) = read(&h, &home(), people_ops::GRANTS, who).await;
        denied(status, &body);
    }
    let (status, body) = read(&h, TWIN, people_ops::GRANTS, OWNER).await;
    denied(status, &body);
}

/// `std.space.callableBy`: every op a member may call and how, cut to what
/// the asker may call. A manager asking about the owner never learns an
/// owner-only op; the owner does. An id the tenant does not hold is
/// `not_found`, the twin's member included.
#[tokio::test(flavor = "multi_thread")]
async fn callable_by_answers_within_the_askers_reach() {
    let h = harness("ordo_people_callable").await;
    let ops_group = group(&h, "Ops", &[VIEWER]).await;
    plant(&h, "grt_v", VIEWER, &["catInfo"], NOW + 3_600).await;
    let q = |m: &str| format!("{}?member={m}", people_ops::CALLABLE_BY);

    // The viewer: by role, by the group (which grants nothing yet, so
    // absent), and by the token.
    let (status, body) = read(&h, &home(), &q(VIEWER), OWNER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = rows_of(&body);
    assert!(
        rows.iter()
            .any(|r| r["op"] == "catInfo" && r["via"] == "role" && r["detail"] == "viewer"),
        "{body}"
    );
    assert!(
        rows.iter()
            .any(|r| r["op"] == "catInfo" && r["via"] == "token"),
        "{body}"
    );
    assert!(
        !rows.iter().any(|r| r["via"] == "group"),
        "{ops_group} grants nothing: {body}"
    );
    // A viewer calls no membership command.
    assert!(!ids(&body, "op").contains(member_ops::SET_ROLE));

    // The owner about themselves: owner-only ops are there.
    let (status, body) = read(&h, &home(), &q(OWNER), OWNER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let owners = ids(&body, "op");
    assert!(owners.contains(member_ops::SET_ROLE), "{body}");
    assert!(owners.contains(people_ops::MEMBERS), "{body}");

    // The manager about the owner: only ops the manager holds.
    let (status, body) = read(&h, &home(), &q(OWNER), MANAGER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let seen = ids(&body, "op");
    assert!(!seen.contains(member_ops::SET_ROLE), "{body}");
    assert!(seen.contains(people_ops::MEMBERS), "{body}");
    assert!(seen.is_subset(&owners));

    for m in ["mem_nosuch", TWIN_VIEWER, &ops_group] {
        let (status, body) = read(&h, &home(), &q(m), OWNER).await;
        not_found(status, &body);
    }
    for who in [EDITOR, VIEWER] {
        let (status, body) = read(&h, &home(), &q(VIEWER), who).await;
        denied(status, &body);
    }
    let (status, body) = read(&h, TWIN, &q(TWIN_VIEWER), OWNER).await;
    denied(status, &body);
}

fn acts(h: &Harness) -> Vec<String> {
    h.recorder
        .drain_all(ordo_commandlog::Timestamp(NOW + 1))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Enrolment { act, detail } => Some(format!("{act} {detail}")),
            _ => None,
        })
        .collect()
}

/// G12: each write the People screen makes leaves a record in the
/// tenant's security stream naming who made it.
#[tokio::test(flavor = "multi_thread")]
async fn each_people_write_leaves_a_record() {
    let h = harness("ordo_people_g12").await;
    let _ = acts(&h);
    let ops_group = group(&h, "Ops", &[]).await;
    // A newcomer nothing refers to yet, so the removal can run.
    let (status, body) = command(
        &h,
        &home(),
        member_ops::ADMIT,
        OWNER,
        json!({"principal": "usr_new", "role": "viewer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let newcomer = body["rows"][0]["member"]
        .as_str()
        .expect("member")
        .to_string();
    let steps: [(&str, Value, &str); 4] = [
        (
            invite_ops::INVITE,
            json!({"role": "editor", "label": "Pat"}),
            "invitation-issued",
        ),
        (
            member_ops::SET_ROLE,
            json!({"member": newcomer, "role": "editor"}),
            "member-role-changed",
        ),
        (
            member_ops::REMOVE,
            json!({"member": newcomer}),
            "member-removed",
        ),
        (
            profile_ops::ADD_TO_GROUP,
            json!({"group": ops_group, "member": VIEWER}),
            "group-member-added",
        ),
    ];
    for (op, params, act) in steps {
        let (status, body) = command(&h, &home(), op, OWNER, params).await;
        assert_eq!(status, StatusCode::OK, "{op}: {body}");
        let seen = acts(&h);
        assert!(
            seen.iter()
                .any(|a| a.starts_with(act) && a.contains(&format!("by={OWNER}"))),
            "{op} left no `{act}` record: {seen:?}"
        );
    }
}

/// Every other kind of event a write leaves: `(kind, detail)`.
fn events(h: &Harness) -> Vec<(ordo_secevent::EventKind, String)> {
    h.recorder
        .drain_all(ordo_commandlog::Timestamp(NOW + 1))
        .into_iter()
        .map(|e| (e.kind.clone(), format!("{:?}", e.subject)))
        .collect()
}

fn acts_in(seen: &[(ordo_secevent::EventKind, String)]) -> Vec<String> {
    seen.iter()
        .filter_map(|(k, _)| match k {
            ordo_secevent::EventKind::Enrolment { act, detail } => Some(format!("{act} {detail}")),
            _ => None,
        })
        .collect()
}

/// G12, the rest of the People screen (step 8, session 2): suspend and
/// reinstate, make a group, take a member out of it, delete it, set and
/// reset an op's access, set one member's access, and revoke an
/// invitation. Each leaves a record in the tenant's stream; the
/// `profile` and `invitations` reads answer what the screen shows.
#[tokio::test(flavor = "multi_thread")]
async fn the_rest_of_the_people_writes_leave_records() {
    let h = harness("ordo_people_g12_rest").await;
    let _ = events(&h);

    // Suspend and reinstate are membership events of their own kind.
    for (op, kind) in [
        (
            member_ops::SUSPEND,
            ordo_secevent::EventKind::MembershipSuspended,
        ),
        (
            member_ops::REINSTATE,
            ordo_secevent::EventKind::MembershipReactivated,
        ),
    ] {
        let (status, body) = command(&h, &home(), op, OWNER, json!({"member": VIEWER})).await;
        assert_eq!(status, StatusCode::OK, "{op}: {body}");
        let seen = events(&h);
        assert!(
            seen.iter()
                .any(|(k, who)| *k == kind && who.contains(OWNER)),
            "{op} left no {kind:?} naming {OWNER}: {seen:?}"
        );
    }

    let (status, body) = command(
        &h,
        &home(),
        profile_ops::CREATE_GROUP,
        OWNER,
        json!({"name": "Night shift"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let night = body["rows"][0]["group"].as_str().expect("id").to_string();
    let seen = acts_in(&events(&h));
    assert!(
        seen.iter()
            .any(|a| a.starts_with("group-created") && a.contains("by=")),
        "{seen:?}"
    );
    let (status, body) = command(
        &h,
        &home(),
        profile_ops::ADD_TO_GROUP,
        OWNER,
        json!({"group": night, "member": EDITOR}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let _ = events(&h);

    const REPORT: &str = "activeProducts";
    let steps: [(&str, Value, &str); 5] = [
        (
            profile_ops::REMOVE_FROM_GROUP,
            json!({"group": night, "member": EDITOR}),
            "group-member-removed",
        ),
        (
            profile_ops::DELETE_GROUP,
            json!({"group": night}),
            "group-deleted",
        ),
        (
            profile_ops::SET_OP_ACCESS,
            json!({"op": REPORT, "roles": "owner,manager", "groups": ""}),
            "profile-op-set",
        ),
        (
            profile_ops::RESET_OP_ACCESS,
            json!({"op": REPORT}),
            "profile-op-reset",
        ),
        (
            profile_ops::SET_MEMBER_ACCESS,
            json!({"member": VIEWER, "op": REPORT, "effect": "allow"}),
            "profile-member-set",
        ),
    ];
    for (op, params, act) in steps {
        let (status, body) = command(&h, &home(), op, OWNER, params).await;
        assert_eq!(status, StatusCode::OK, "{op}: {body}");
        let seen = acts_in(&events(&h));
        assert!(
            seen.iter()
                .any(|a| a.starts_with(act) && a.contains(&format!("by={OWNER}"))),
            "{op} left no `{act}` record: {seen:?}"
        );
    }

    // The profile read shows the member's own allowance.
    let (status, body) = read(
        &h,
        &home(),
        &format!("{}?op={REPORT}", profile_ops::PROFILE),
        OWNER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["allow"], json!([VIEWER]), "{body}");

    // An invitation, listed by id and never by its secret, then revoked.
    let (status, body) = command(
        &h,
        &home(),
        invite_ops::INVITE,
        OWNER,
        json!({"role": "viewer", "label": "Quinn"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["rows"][0]["ticket"]
        .as_str()
        .expect("ticket")
        .to_string();
    let (status, body) = read(&h, &home(), invite_ops::INVITATIONS, MANAGER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = rows_of(&body)
        .into_iter()
        .find(|r| r["label"] == "Quinn")
        .expect("listed");
    assert!(
        !body.to_string().contains(&ticket),
        "the secret is never listed"
    );
    let id = row["id"].as_str().expect("id").to_string();
    let _ = events(&h);
    let (status, body) = command(
        &h,
        &home(),
        invite_ops::REVOKE_INVITATION,
        OWNER,
        json!({"id": id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let seen = acts_in(&events(&h));
    assert!(
        seen.iter()
            .any(|a| a.starts_with("invitation-revoked") && a.contains(&format!("by={OWNER}"))),
        "{seen:?}"
    );
}
