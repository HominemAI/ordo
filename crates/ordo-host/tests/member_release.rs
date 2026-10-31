//! **Leaving a tenant** (checklist 27aq, `plan-member-release.md` §3): the
//! `left` seat (R2), `std.space.leave` under each form of the program's
//! release rule (R3), and a managed identity on leave (R4), through the real
//! door against real Postgres.
//!
//! The claims, in the order they matter:
//!
//! - with no release line, members are locked;
//! - a rule that holds names the condition as written and no row;
//! - the platform's own condition comes first: the last owner cannot leave;
//! - a left seat is no membership at the door on the next request, keeps the
//!   rows that point at it, and keeps its name unless the app developer set
//!   a replacement (owner ruling 2026-10-06);
//! - a deleted account is marked and kept, never signs in again, and its
//!   data is removed only when the app developer's setting says so;
//! - the app may remove or reassign what points at the leaver.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, BoundParams, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::member_ops::{self, MemberOps};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::release_ops::{self, ReleaseOps};
use ordo_host::runtime::SwappableAccess;
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: [u8; 32] = *b"member-release-test-secret-32by!";

// The conformance seed: mem_o owner, mem_1 manager, mem_2 and mem_3 editors,
// mem_4 viewer. Their principals are their ids.
const OWNER: &str = "mem_o";
const VIEWER: &str = "mem_4";

/// An entity the shipped fixture lacks: who manages whom, for `reassign`.
const PEOPLE: &str = "
entity people {
  member  -> members unique
  manager -> members?
}
";

struct Harness {
    access: Arc<SwappableAccess>,
    state: ApiState,
    pool: ordo_exec::Pool,
    ops: Arc<PlatformOps>,
    release: Arc<ReleaseOps>,
    /// The resolver the door and the leave share, so the leave's group is
    /// in the same chain head an op's is.
    spaces: Arc<ordo_host::spaces::Spaces>,
    space: String,
}

/// The market fixture, the `people` entity, and `rule` as its release line
/// (`""` for none).
async fn harness(db: &str, rule: &str) -> Harness {
    harness_named(db, rule, None).await
}

/// The same, with the app developer's replacement for a leaver's name.
async fn harness_named(db: &str, rule: &str, left_name: Option<&str>) -> Harness {
    let source = format!("{FIXTURE}\n{PEOPLE}\n{rule}\n");
    let f = Arc::new(Fixture::from_source(&source, SEED).expect("fixture"));
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
        Box::new(ordo_host::runtime::now_secs),
        metrics.clone(),
    );
    let access = Arc::new(SwappableAccess::new(door));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "member-release-test",
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
    let live = access.clone();
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    let release = Arc::new(ReleaseOps {
        members: members.clone(),
        spaces: spaces.clone(),
        grants: Some(grants.clone()),
        left_name: left_name.map(str::to_string),
    });
    let ops = Arc::new(
        PlatformOps::new(
            access.current().engine.clone(),
            grants.clone(),
            Arc::new(|| NOW),
        )
        .with_engine_source(Arc::new(move || live.current().engine.clone()))
        .with_release(release.clone())
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
            ops.clone(),
        )),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"member-release-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        access,
        state,
        pool,
        ops,
        release,
        spaces,
        space: ordo_conformance::fixture_space().0,
    }
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

async fn leave(h: &Harness, who: &str) -> (StatusCode, Value) {
    command(h, release_ops::LEAVE, who, json!({})).await
}

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

fn message(v: &Value) -> String {
    v["error"]["message"].as_str().unwrap_or("").to_string()
}

async fn rows(h: &Harness, sql: &str) -> Vec<Vec<Option<String>>> {
    h.pool.rows(sql).await.expect("read")
}

async fn one(h: &Harness, sql: &str) -> Option<String> {
    rows(h, sql)
        .await
        .first()
        .and_then(|r| r.first())
        .cloned()
        .flatten()
}

/// The seat: (principal, role, status, displayName).
async fn seat(h: &Harness, member: &str) -> (String, String, String, String) {
    let r = rows(
        h,
        &format!(
            r#"SELECT "principal", coalesce("role", ''), "status", "displayName" FROM "members"
               WHERE "$space" = '{}' AND "id" = '{member}'"#,
            h.space
        ),
    )
    .await;
    let c = |i: usize| r[0][i].clone().unwrap_or_default();
    (c(0), c(1), c(2), c(3))
}

// ── R2: the left seat ────────────────────────────────────────────────────

/// Under `release member open` any member may leave; the very next request
/// is refused, and the seat stays as a former member.
#[tokio::test(flavor = "multi_thread")]
async fn leave_under_open_releases_the_seat() {
    let h = harness("ordo_release_open", "release member open").await;
    assert!(door_is_open_to(&h, VIEWER).await, "the control: open first");
    let before = seat(&h, VIEWER).await;

    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["status"], "left");
    assert_eq!(body["rows"][0]["rule"], "open");
    assert!(
        body["rows"][0]["door"]
            .as_str()
            .unwrap()
            .starts_with("rebuilt here"),
        "{body}"
    );

    let (principal, role, status, name) = seat(&h, VIEWER).await;
    assert_eq!(status, "left");
    assert_eq!(role, "", "a left seat holds no role");
    assert_eq!(
        principal, "left:mem_4:mem_4",
        "the person is unlinked, and their id is kept behind the prefix"
    );
    assert_eq!(name, before.3, "the seat keeps its name by default");

    // On the record, in the tenant's own stream.
    let audit = h.access.current().take_audit().join("\n");
    assert!(audit.contains("member-released"), "{audit}");
}

/// The next request from a seat that left is refused at the door, and the
/// rebuilt engine knows no such membership.
#[tokio::test(flavor = "multi_thread")]
async fn a_left_seat_is_refused_at_the_door() {
    let h = harness("ordo_release_door", "release member open").await;
    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !door_is_open_to(&h, VIEWER).await,
        "the very next request must be refused"
    );
    let engine = h.access.current().engine.clone();
    assert!(
        engine
            .spaces_of(&PrincipalId(VIEWER.into()))
            .into_iter()
            .all(|(s, _, _)| s.0 != h.space),
        "no membership for the person who left"
    );
}

/// Rows that point at the leaver stay and point at the seat; the seat's acl
/// rows go; its name is the placeholder.
#[tokio::test(flavor = "multi_thread")]
async fn leaving_deletes_access_rows_and_keeps_referring_rows() {
    let h = harness("ordo_release_keeps_rows", "release member open").await;
    let (status, body) = leave(&h, "mem_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["effects"]["accessRows"], 2, "{body}");

    let s = &h.space;
    assert_eq!(
        one(&h, &format!(r#"SELECT count(*) FROM "boardsAccess" WHERE "$space" = '{s}' AND "member" = 'mem_2'"#)).await.as_deref(),
        Some("0")
    );
    assert_eq!(
        one(
            &h,
            &format!(
                r#"SELECT "owner" FROM "projects" WHERE "$space" = '{s}' AND "id" = 'proj_2'"#
            )
        )
        .await
        .as_deref(),
        Some("mem_2"),
        "a project the leaver owns still names the seat"
    );
    assert_eq!(
        one(
            &h,
            &format!(
                r#"SELECT "assignee" FROM "tasks" WHERE "$space" = '{s}' AND "id" = 'task_5'"#
            )
        )
        .await
        .as_deref(),
        Some("mem_2")
    );
}

/// The name a row reads through its ref to the leaver, task_5's assignee.
async fn assignee_name(h: &Harness) -> Option<String> {
    let s = &h.space;
    one(
        h,
        &format!(
            r#"SELECT m."displayName" FROM "tasks" t JOIN "members" m
               ON m."id" = t."assignee" AND m."$space" = t."$space"
               WHERE t."$space" = '{s}' AND t."id" = 'task_5'"#
        ),
    )
    .await
}

/// A leave keeps the seat's name by default (owner ruling 2026-10-06:
/// *"Default should be to leave it"*). A row reading the leaver through its
/// ref still reads their name, and the seat says `left`.
#[tokio::test(flavor = "multi_thread")]
async fn a_leave_keeps_the_seat_name_by_default() {
    let h = harness("ordo_release_name", "release member open").await;
    let before = assignee_name(&h).await;
    assert_eq!(before.as_deref(), Some("Bram"), "the seed names mem_2 Bram");
    let (status, body) = leave(&h, "mem_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(assignee_name(&h).await, before, "the name stays");
    assert_eq!(seat(&h, "mem_2").await.2, "left");
}

/// The app developer's setting replaces the name on leave, with the text it
/// gives, and the old name is nowhere in the tenant's rows.
#[tokio::test(flavor = "multi_thread")]
async fn the_left_name_setting_replaces_the_name_on_leave() {
    let h = harness_named(
        "ordo_release_renamed",
        "release member open",
        Some("former member"),
    )
    .await;
    let (status, body) = leave(&h, "mem_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(assignee_name(&h).await.as_deref(), Some("former member"));
    let s = &h.space;
    assert!(
        rows(
            &h,
            &format!(
                r#"SELECT 1 FROM "members" WHERE "$space" = '{s}' AND "displayName" = 'Bram'"#
            )
        )
        .await
        .is_empty(),
        "the old name is nowhere in the tenant's rows"
    );
}

/// The setting reads unset and blank as "keep the name", and refuses a
/// replacement too long to be a name.
#[test]
fn the_left_name_setting_defaults_to_keeping_the_name() {
    assert_eq!(release_ops::left_name_from(None), Ok(None));
    assert_eq!(release_ops::left_name_from(Some("  ")), Ok(None));
    assert_eq!(
        release_ops::left_name_from(Some(" former member ")),
        Ok(Some("former member".to_string()))
    );
    assert!(release_ops::left_name_from(Some(&"x".repeat(257))).is_err());
}

/// An app reads a seat's status from its own program, so it can render a
/// left member its own way, such as "Bram (Deactivated)". The op is
/// ordinary program code over the platform's `members` entity.
#[tokio::test(flavor = "multi_thread")]
async fn the_app_reads_a_left_seat_status_from_program_code() {
    let h = harness(
        "ordo_release_status_read",
        "release member open\n\nop rosterWithStatus unbounded =\n  \
         members(kind == \"principal\")[displayName, status]\n  sort displayName\n",
    )
    .await;
    let (status, body) = leave(&h, "mem_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, out) = send(
        &h.state,
        Request::builder()
            .method("GET")
            .uri("/op/rosterWithStatus")
            .header("x-ordo-space", &h.space)
            .header("authorization", format!("Bearer {}", token(OWNER)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let text = out.to_string();
    let rows = out["rows"].as_array().expect("rows");
    let bram = rows
        .iter()
        .find(|r| r["displayName"] == "Bram")
        .unwrap_or_else(|| panic!("the left seat is readable: {text}"));
    assert_eq!(bram["status"], "left", "{text}");
}

/// `std.space.remove` keeps its meaning on a left seat: refused while rows
/// still refer to it.
#[tokio::test(flavor = "multi_thread")]
async fn remove_on_a_left_seat_with_referrers_is_has_referrers() {
    let h = harness("ordo_release_remove_left", "release member open").await;
    let (status, body) = leave(&h, "mem_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, out) = command(&h, member_ops::REMOVE, OWNER, json!({"member": "mem_2"})).await;
    assert_ne!(status, StatusCode::OK, "{out}");
    assert_eq!(code(&out), "has_referrers", "{out}");

    // A left seat nothing points at is removed like any other.
    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, out) = command(&h, member_ops::REMOVE, OWNER, json!({"member": VIEWER})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

// ── R3: the rule ─────────────────────────────────────────────────────────

/// With no release line, members are locked (owner, 2026-10-05). `locked`
/// says the same.
#[tokio::test(flavor = "multi_thread")]
async fn leave_under_locked_is_release_locked() {
    for (db, rule) in [
        ("ordo_release_none", ""),
        ("ordo_release_locked", "release member locked"),
    ] {
        let h = harness(db, rule).await;
        let (status, out) = leave(&h, VIEWER).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rule:?}: {out}");
        assert_eq!(code(&out), "tenant_refused");
        assert!(message(&out).starts_with("release_locked:"), "{out}");
        assert_eq!(seat(&h, VIEWER).await.2, "active");
        assert!(door_is_open_to(&h, VIEWER).await);
        let audit = h.access.current().take_audit().join("\n");
        assert!(audit.contains("member-leave-refused"), "{audit}");
    }
}

/// A rule that holds names the condition as the program wrote it, and never
/// the row that held it. A member the rule does not hold leaves.
#[tokio::test(flavor = "multi_thread")]
async fn leave_held_names_the_atom_and_no_rows() {
    let h = harness(
        "ordo_release_held",
        r#"release member when not tasks(assignee == $member and status == "open")"#,
    )
    .await;
    let (status, out) = leave(&h, "mem_3").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    let m = message(&out);
    assert!(m.starts_with("release_held:"), "{out}");
    assert!(
        m.contains(r#"not tasks(assignee == $member and status == "open")"#),
        "{m}"
    );
    assert!(
        !m.contains("task_3") && !m.contains("build site"),
        "no row: {m}"
    );
    assert_eq!(seat(&h, "mem_3").await.2, "active");

    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["rule"], "rule");
}

/// The platform's own condition comes before the rule, even an open one.
#[tokio::test(flavor = "multi_thread")]
async fn the_last_owner_cannot_leave() {
    let h = harness("ordo_release_last_owner", "release member open").await;
    let (status, out) = leave(&h, OWNER).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert!(message(&out).starts_with("last_owner:"), "{out}");
    assert_eq!(seat(&h, OWNER).await.2, "active");

    // With a second owner, the first may go.
    let (status, body) = command(
        &h,
        member_ops::SET_ROLE,
        OWNER,
        json!({"member": "mem_1", "role": "owner"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = leave(&h, OWNER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A second leave from the seat, asked of the op itself, answers the same
/// and changes nothing. Through the door it is refused: there is no
/// membership left to call anything with.
#[tokio::test(flavor = "multi_thread")]
async fn leave_twice_answers_the_same() {
    let h = harness("ordo_release_twice", "release member open").await;
    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let spec = platform_ops::specs()
        .into_iter()
        .find(|s| s.name == release_ops::LEAVE)
        .expect("registered");
    let again = h
        .ops
        .execute(&spec, &h.space, &BoundParams(Default::default()), VIEWER)
        .await
        .expect("answers");
    assert_eq!(again.rows[0]["status"], "left");
    assert_eq!(again.rows[0]["rule"], "already left");

    let (status, out) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(code(&out), "access_denied");
}

/// The door is rebuilt here and announced, so every replica refuses the
/// seat within a notification, as a suspension is.
#[tokio::test(flavor = "multi_thread")]
async fn leaving_ends_the_tenant_sessions_on_every_replica() {
    let h = harness("ordo_release_announce", "release member open").await;
    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"][0]["door"], "rebuilt here, announced to every replica",
        "{body}"
    );
}

/// The app's `remove` and `reassign`: a looked-up manager first, the
/// tenant's owner as the fallback.
#[tokio::test(flavor = "multi_thread")]
async fn the_app_may_remove_or_reassign_what_points_at_the_leaver() {
    let h = harness(
        "ordo_release_effects",
        "release member open {\n  remove projectMembers.member\n  \
         reassign tasks.assignee to people(member == $member).manager or owner\n  \
         reassign projects.owner to owner\n}",
    )
    .await;
    let s = h.space.clone();
    h.pool
        .rows(&format!(
            r#"INSERT INTO "people" ("$space", "id", "rev", "member", "manager")
               VALUES ('{s}', 'peop_1', 1, 'mem_3', 'mem_1')"#
        ))
        .await
        .expect("a manager for mem_3");

    let (status, body) = leave(&h, "mem_3").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let effects = &body["rows"][0]["effects"];
    assert_eq!(effects["removed projectMembers.member"], 1, "{body}");
    assert_eq!(
        effects["reassigned tasks.assignee"]["to"], "mem_1",
        "{body}"
    );
    assert_eq!(effects["reassigned projects.owner"]["to"], OWNER, "{body}");

    assert_eq!(
        one(
            &h,
            &format!(
                r#"SELECT "assignee" FROM "tasks" WHERE "$space" = '{s}' AND "id" = 'task_3'"#
            )
        )
        .await
        .as_deref(),
        Some("mem_1"),
        "to the person's manager"
    );
    assert_eq!(
        one(
            &h,
            &format!(
                r#"SELECT "owner" FROM "projects" WHERE "$space" = '{s}' AND "id" = 'proj_3'"#
            )
        )
        .await
        .as_deref(),
        Some(OWNER),
        "to the tenant's owner"
    );
    assert_eq!(
        one(&h, &format!(r#"SELECT count(*) FROM "projectMembers" WHERE "$space" = '{s}' AND "member" = 'mem_3'"#)).await.as_deref(),
        Some("0")
    );

    // With no manager the fallback answers.
    let (status, body) = leave(&h, "mem_2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["rows"][0]["effects"]["reassigned tasks.assignee"]["to"],
        OWNER
    );
}

/// A reassignment that names nobody holds the seat, and nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn a_reassignment_with_nobody_to_name_holds_the_seat() {
    let h = harness(
        "ordo_release_nobody",
        "release member open {\n  remove projectMembers.member\n  \
         reassign tasks.assignee to people(member == $member).manager\n}",
    )
    .await;
    let (status, out) = leave(&h, "mem_3").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert!(message(&out).starts_with("release_held:"), "{out}");
    let s = &h.space;
    assert_eq!(
        one(&h, &format!(r#"SELECT count(*) FROM "projectMembers" WHERE "$space" = '{s}' AND "member" = 'mem_3'"#)).await.as_deref(),
        Some("1"),
        "the remove before it rolled back with it"
    );
    assert_eq!(seat(&h, "mem_3").await.2, "active");

    // A member nothing points at leaves under the same rule.
    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ── R4: a managed identity ───────────────────────────────────────────────

/// A person this tenant made, leaving it, becomes independent (E7's rule).
#[tokio::test(flavor = "multi_thread")]
async fn leaving_releases_a_managed_identity_that_signed_in() {
    let h = harness("ordo_release_managed", "release member open").await;
    ordo_host::credential_ops::ensure_table(&h.pool)
        .await
        .expect("$managed");
    h.pool
        .rows(ordo_host::principal_directory::SIGNEDIN_DDL)
        .await
        .expect("$signedin");
    let s = &h.space;
    h.pool
        .rows(&format!(
            r#"INSERT INTO "$managed" ("principal", "tenant") VALUES ('{VIEWER}', '{s}')"#
        ))
        .await
        .expect("managed");
    h.pool
        .rows(&format!(
            r#"INSERT INTO "$signedin" ("principal", "first_at") VALUES ('{VIEWER}', 1)"#
        ))
        .await
        .expect("signed in");

    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["effects"]["identity"], "released", "{body}");
    assert!(
        one(
            &h,
            &format!(r#"SELECT "released_at" FROM "$managed" WHERE "principal" = '{VIEWER}'"#)
        )
        .await
        .is_some(),
        "independent from now on"
    );
}

// ── R6: deleting one's own account ──────────────────────────────────────

use ordo_host::account_removal::{self, Removal};
use ordo_host::auth::account::Accounts;
use ordo_host::auth::AuthState;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::session::SessionRevocations;
use ordo_identity::{MemorySessionRevocations, ProviderRegistry, SessionClaim};

/// The auth routes, with account deletion wired over the harness's door.
fn auth_app(h: &Harness, revocations: Arc<MemorySessionRevocations>) -> axum::Router {
    auth_app_with(h, revocations, Removal::Now, None).0
}

/// The auth routes with the app developer's removal setting, over `store`
/// when given (the deployment's Postgres directory) or a memory one. The
/// state is handed back so a test can sign in through its directory.
fn auth_app_with(
    h: &Harness,
    revocations: Arc<MemorySessionRevocations>,
    removal: Removal,
    store: Option<Arc<dyn ordo_identity::directory::DirectoryStore>>,
) -> (axum::Router, Arc<AuthState>) {
    let directory = match store {
        Some(store) => Directory::with_store(Provisioning::AutoOnboard, store),
        None => Directory::new(Provisioning::AutoOnboard),
    };
    let mut state = AuthState::new(
        ProviderRegistry::new(),
        directory,
        TokenKey::from_secret(&SECRET),
    )
    .with_revocations(revocations)
    .with_recorder(h.access.recorder.clone());
    state.now = Box::new(|| NOW);
    let live = h.access.clone();
    state.wire_account(Arc::new(Accounts {
        release: h.release.clone(),
        engine: Arc::new(move || live.current().engine.clone()),
        removal,
    }));
    let state = Arc::new(state);
    (ordo_host::auth::router(Some(state.clone())), state)
}

/// A session's token, with fresh presence or without.
fn session(principal: &str, presence: bool) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: presence.then_some(IStamp(NOW)),
            capability: None,
            delegation: None,
            session: Some(SessionClaim::starting_at(IStamp(NOW - 10))),
        },
    )
}

async fn delete_account(
    app: &axum::Router,
    who: &str,
    presence: bool,
    confirm: &str,
) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/account/delete")
                .header("content-type", "application/json")
                .header(
                    "authorization",
                    format!("Bearer {}", session(who, presence)),
                )
                .body(Body::from(json!({ "confirm": confirm }).to_string()))
                .unwrap(),
        )
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

/// The rows for one person's identity in the control database.
async fn identity_rows(h: &Harness, who: &str) -> (usize, usize) {
    let p = rows(
        h,
        &format!(r#"SELECT 1 FROM "$principal" WHERE "id" = '{who}'"#),
    )
    .await;
    let a = rows(
        h,
        &format!(r#"SELECT 1 FROM "$authenticator" WHERE "principal" = '{who}'"#),
    )
    .await;
    (p.len(), a.len())
}

async fn an_identity(h: &Harness, who: &str) {
    ordo_host::principal_directory::ensure_table(&h.pool)
        .await
        .expect("directory tables");
    h.pool
        .rows(&format!(
            r#"INSERT INTO "$principal" ("id", "kind", "display") VALUES ('{who}', 'user', 'Dov')
               ON CONFLICT DO NOTHING"#
        ))
        .await
        .expect("principal");
    h.pool
        .rows(&format!(
            r#"INSERT INTO "$authenticator" ("authenticator", "principal")
               VALUES ('passkey:{who}', '{who}') ON CONFLICT DO NOTHING"#
        ))
        .await
        .expect("link");
}

/// Deleting an account while tenants hold the person is refused, and the
/// body lists each tenant with what stands in the way (owner, ask 8).
#[tokio::test(flavor = "multi_thread")]
async fn delete_with_memberships_lists_each_tenant_and_its_bar() {
    let h = harness(
        "ordo_release_delete_bars",
        r#"release member when not tasks(assignee == $member and status == "open")"#,
    )
    .await;
    let app = auth_app(&h, Arc::new(MemorySessionRevocations::new()));
    for (who, bar) in [
        ("mem_3", "held"),
        (OWNER, "last_owner"),
        ("mem_g", "guest"),
        (VIEWER, "open"),
    ] {
        let (status, out) = delete_account(&app, who, true, who).await;
        assert_eq!(status, StatusCode::CONFLICT, "{who}: {out}");
        assert_eq!(code(&out), "memberships_remain");
        let tenants = out["tenants"].as_array().expect("a list");
        assert_eq!(tenants.len(), 1, "{out}");
        assert_eq!(tenants[0]["space"], h.space);
        assert_eq!(tenants[0]["bar"], bar, "{who}: {out}");
        if bar == "held" {
            assert_eq!(
                tenants[0]["held"][0],
                r#"not tasks(assignee == $member and status == "open")"#
            );
        }
    }

    // With no release line, the bar is the lock.
    let h = harness("ordo_release_delete_locked", "").await;
    let app = auth_app(&h, Arc::new(MemorySessionRevocations::new()));
    let (_, out) = delete_account(&app, VIEWER, true, VIEWER).await;
    assert_eq!(out["tenants"][0]["bar"], "locked", "{out}");
}

/// Presence and the confirmation are asked for before anything is read.
#[tokio::test(flavor = "multi_thread")]
async fn delete_needs_presence_and_the_principal_typed_back() {
    let h = harness("ordo_release_delete_gate", "release member open").await;
    let app = auth_app(&h, Arc::new(MemorySessionRevocations::new()));
    let (status, out) = delete_account(&app, VIEWER, false, VIEWER).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(code(&out), "presence_required");
    let (status, out) = delete_account(&app, VIEWER, true, "mem_3").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert_eq!(code(&out), "not_confirmed");
}

/// Removal at once (`ORDO_ACCOUNT_REMOVAL=now`): once every tenant has let
/// the person go, the account is marked deleted and its data goes in the
/// same request: links, the principal, every session. The answer is a
/// receipt naming the `identity-deleted` record.
#[tokio::test(flavor = "multi_thread")]
async fn removal_now_erases_links_passkeys_credential_and_principal() {
    let h = harness("ordo_release_delete_erases", "release member open").await;
    an_identity(&h, VIEWER).await;
    let revocations = Arc::new(MemorySessionRevocations::new());
    let app = auth_app(&h, revocations.clone());

    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(identity_rows(&h, VIEWER).await, (1, 1));

    let (status, out) = delete_account(&app, VIEWER, true, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["deleted"], VIEWER);
    assert_eq!(out["receipt"]["act"], "identity-deleted");
    assert_eq!(out["removal"]["setting"], "now");
    assert_eq!(out["removal"]["removed"], true);
    assert_eq!(out["receipt"]["reason"], "self");
    assert!(out["receipt"]["correlation"]
        .as_str()
        .is_some_and(|c| !c.is_empty()));
    assert_eq!(
        identity_rows(&h, VIEWER).await,
        (0, 0),
        "nothing names them"
    );

    // Every session ended.
    assert!(
        revocations.principal_ended(&PrincipalId(VIEWER.into()), IStamp(NOW - 10)),
        "a session started before the deletion is ended"
    );

    // The seat stays, marked, with its name: how it reads is the app's.
    let (principal, _, status, _) = seat(&h, VIEWER).await;
    assert_eq!(
        (principal.as_str(), status.as_str()),
        ("left:mem_4:mem_4", "deleted")
    );
    assert_eq!(out["seats"]["marked"], 1, "{out}");
}

/// By default (`never`) a deleted account is marked and kept: its sessions
/// end, it cannot sign in again with an identifier it held, its data stays,
/// and its left seat reads `deleted` with its name kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_account_cannot_sign_in_and_its_data_stays_by_default() {
    let h = harness("ordo_release_delete_kept", "release member open").await;
    an_identity(&h, VIEWER).await;
    let store: Arc<dyn ordo_identity::directory::DirectoryStore> = Arc::new(
        ordo_host::principal_directory::PgDirectoryStore::open_on(h.pool.clone())
            .expect("the directory store"),
    );
    let revocations = Arc::new(MemorySessionRevocations::new());
    let (app, state) = auth_app_with(&h, revocations.clone(), Removal::Never, Some(store));
    let name = seat(&h, VIEWER).await.3;
    let subject = ordo_identity::AuthenticatedSubject {
        authenticator: ordo_identity::AuthenticatorId(format!("passkey:{VIEWER}")),
        method: ordo_identity::AuthMethodKind::Passkey,
        attributes: Default::default(),
        presence: None,
        upstream_presence: None,
    };
    {
        let mut dir = state.directory.lock().unwrap();
        assert!(
            ordo_identity::sign_in(&mut dir, &subject, "Dov", IStamp(NOW)).is_ok(),
            "the control: the identity signs in before it is deleted"
        );
    }

    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, out) = delete_account(&app, VIEWER, true, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["receipt"]["act"], "identity-deleted");
    assert_eq!(out["removal"]["setting"], "never");
    assert_eq!(out["removal"]["removed"], false);
    assert!(out["removal"]["removeAt"].is_null(), "{out}");

    // Never signs in again, by the identifier it held.
    {
        let mut dir = state.directory.lock().unwrap();
        assert!(matches!(
            ordo_identity::sign_in(&mut dir, &subject, "Dov", IStamp(NOW + 1)),
            Err(ordo_identity::DirectoryError::Deleted(_))
        ));
    }
    assert!(
        revocations.principal_ended(&PrincipalId(VIEWER.into()), IStamp(NOW - 10)),
        "every session ended"
    );
    // The data stays, marked.
    assert_eq!(identity_rows(&h, VIEWER).await, (1, 1), "nothing removed");
    assert_ne!(
        one(
            &h,
            &format!(r#"SELECT "deleted_at" FROM "$principal" WHERE "id" = '{VIEWER}'"#)
        )
        .await
        .as_deref(),
        Some("0"),
        "the identity is marked deleted"
    );
    let (_, _, status, kept) = seat(&h, VIEWER).await;
    assert_eq!(status, "deleted", "the left seat carries the mark");
    assert_eq!(kept, name, "and keeps its name");
    // A deleted seat is no membership, and deleting again finds nothing new.
    assert!(!door_is_open_to(&h, VIEWER).await);
}

/// `ORDO_ACCOUNT_REMOVAL=<n>d`: the request schedules the removal, and the
/// `accounts.removal` worker's pass removes the data once the days have
/// passed, and not before. A second pass finds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn removal_after_days_is_run_by_the_worker() {
    let h = harness("ordo_release_delete_later", "release member open").await;
    an_identity(&h, VIEWER).await;
    let (app, _) = auth_app_with(
        &h,
        Arc::new(MemorySessionRevocations::new()),
        Removal::AfterDays(2),
        Some(pg_store(&h)),
    );
    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, out) = delete_account(&app, VIEWER, true, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let due = NOW + 2 * 86_400;
    assert_eq!(out["removal"]["setting"], "after");
    assert_eq!(out["removal"]["removeAt"], due);
    assert_eq!(out["removal"]["removed"], false);

    let pass = account_removal::sweep(&h.pool, None, Removal::AfterDays(2), due - 1)
        .await
        .expect("a pass");
    assert!(pass.removed.is_empty(), "not before the day");
    assert_eq!(identity_rows(&h, VIEWER).await, (1, 1));

    let pass = account_removal::sweep(&h.pool, None, Removal::AfterDays(2), due)
        .await
        .expect("a pass");
    assert_eq!(pass.removed, vec![VIEWER.to_string()]);
    assert_eq!(
        identity_rows(&h, VIEWER).await,
        (0, 0),
        "removed on the day"
    );
    let again = account_removal::sweep(&h.pool, None, Removal::AfterDays(2), due + 1)
        .await
        .expect("a pass");
    assert!(again.removed.is_empty(), "a second pass finds nothing");
}

/// The deployment's Postgres directory, so a delete marks `$principal`.
fn pg_store(h: &Harness) -> Arc<dyn ordo_identity::directory::DirectoryStore> {
    Arc::new(
        ordo_host::principal_directory::PgDirectoryStore::open_on(h.pool.clone())
            .expect("the directory store"),
    )
}

/// Delete `who` (who has left every tenant) under `setting`, so the account
/// is marked deleted at `NOW` and nothing more.
async fn deleted_under(h: &Harness, who: &str, setting: Removal) {
    an_identity(h, who).await;
    let (app, _) = auth_app_with(
        h,
        Arc::new(MemorySessionRevocations::new()),
        setting,
        Some(pg_store(h)),
    );
    let (status, body) = leave(h, who).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, out) = delete_account(&app, who, true, who).await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

/// Ruled 2026-10-06 (ask 12): the due time is worked out at each pass from
/// `deleted_at` and the setting in force.
#[tokio::test(flavor = "multi_thread")]
async fn shortening_the_days_removes_an_account_that_is_now_past_due() {
    let h = harness("ordo_release_removal_shorter", "release member open").await;
    deleted_under(&h, VIEWER, Removal::AfterDays(30)).await;
    let two_days = NOW + 2 * 86_400;
    let pass = account_removal::sweep(&h.pool, None, Removal::AfterDays(30), two_days)
        .await
        .expect("a pass");
    assert!(pass.removed.is_empty(), "not due under 30 days");
    assert_eq!(identity_rows(&h, VIEWER).await, (1, 1));

    let pass = account_removal::sweep(&h.pool, None, Removal::AfterDays(1), two_days)
        .await
        .expect("a pass");
    assert_eq!(pass.removed, vec![VIEWER.to_string()]);
    assert_eq!(identity_rows(&h, VIEWER).await, (0, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_to_now_removes_everyone_waiting_at_the_next_boot() {
    let h = harness("ordo_release_removal_now", "release member open").await;
    deleted_under(&h, VIEWER, Removal::AfterDays(30)).await;
    // A second deleted account, marked directly.
    an_identity(&h, "mem_9").await;
    h.pool
        .rows(&format!(
            r#"UPDATE "$principal" SET "deleted_at" = {} WHERE "id" = 'mem_9'"#,
            NOW + 5
        ))
        .await
        .expect("marked");
    // A live account is never waiting.
    an_identity(&h, "mem_8").await;

    let pass = account_removal::sweep(&h.pool, None, Removal::Now, NOW + 10)
        .await
        .expect("the boot pass");
    let mut removed = pass.removed.clone();
    removed.sort();
    assert_eq!(removed, vec!["mem_4".to_string(), "mem_9".to_string()]);
    assert_eq!(identity_rows(&h, VIEWER).await, (0, 0));
    assert_eq!(identity_rows(&h, "mem_9").await, (0, 0));
    assert_eq!(
        identity_rows(&h, "mem_8").await,
        (1, 1),
        "not deleted, kept"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_to_never_removes_no_one() {
    let h = harness("ordo_release_removal_never", "release member open").await;
    deleted_under(&h, VIEWER, Removal::AfterDays(1)).await;
    let pass = account_removal::sweep(&h.pool, None, Removal::Never, NOW + 365 * 86_400)
        .await
        .expect("a pass");
    assert!(pass.removed.is_empty());
    assert_eq!(identity_rows(&h, VIEWER).await, (1, 1), "kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_deleted_under_never_is_removed_once_the_setting_is_days_and_the_time_has_passed(
) {
    let h = harness("ordo_release_removal_later", "release member open").await;
    deleted_under(&h, VIEWER, Removal::Never).await;
    let pass = account_removal::sweep(&h.pool, None, Removal::AfterDays(3), NOW + 3 * 86_400 - 1)
        .await
        .expect("a pass");
    assert!(pass.removed.is_empty(), "the days have not passed");
    let pass = account_removal::sweep(&h.pool, None, Removal::AfterDays(3), NOW + 3 * 86_400)
        .await
        .expect("a pass");
    assert_eq!(pass.removed, vec![VIEWER.to_string()]);
    assert_eq!(identity_rows(&h, VIEWER).await, (0, 0));
}

/// A tenant of the person's own holds them even with no seat in it.
#[tokio::test(flavor = "multi_thread")]
async fn delete_with_an_own_tenant_is_refused() {
    let h = harness("ordo_release_delete_own", "release member open").await;
    ordo_host::tenants::ensure_table(&h.pool)
        .await
        .expect("$spaces");
    h.pool
        .rows(&format!(
            r#"INSERT INTO "$spaces" ("$space", "name", "created_at", "created_by", "state", "own_of")
               VALUES ('spc_dovs', 'Dov''s', 1, '{VIEWER}', 'active', '{VIEWER}')"#
        ))
        .await
        .expect("own tenant");
    let app = auth_app(&h, Arc::new(MemorySessionRevocations::new()));
    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, out) = delete_account(&app, VIEWER, true, VIEWER).await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    let tenants = out["tenants"].as_array().expect("a list");
    assert_eq!(tenants.len(), 1, "{out}");
    assert_eq!(tenants[0]["space"], "spc_dovs");
    assert_eq!(tenants[0]["name"], "Dov's");
    assert_eq!(tenants[0]["bar"], "own");
}

// ── The leave is the write protocol's ───────────────────────────────────

use ordo_commandlog::{Actor, CommandGroup, CommandKind, SpaceId};

const CHAINED: &str = "release member open {\n  remove projectMembers.member\n  \
                       reassign tasks.assignee to owner\n}";

/// The tenant's durable chain, oldest first, as correlations.
async fn chain(h: &Harness) -> Vec<String> {
    rows(
        h,
        &format!(
            r#"SELECT "correlation" FROM "$commandlog" WHERE "space" = '{}' ORDER BY "idx""#,
            h.space
        ),
    )
    .await
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect()
}

/// The last group the tenant's chain head took, from the resolver the door
/// and the leave share.
async fn last_group(h: &Harness) -> CommandGroup {
    let bound = h.spaces.resolve(&h.space).await.expect("the space");
    let log = bound.log.lock().unwrap();
    log.last_in(&SpaceId(h.space.clone())).expect("a group")
}

fn commands_on<'a>(
    g: &'a CommandGroup,
    entity: &str,
    kind: CommandKind,
) -> Vec<&'a ordo_commandlog::Command> {
    g.commands
        .iter()
        .filter(|c| c.entity == entity && c.kind == kind)
        .collect()
}

/// A removed row and a reassigned row are each a command in one group in
/// the leaver's tenant chain, with the seat's converge and the seat's acl
/// rows beside them. The group is in the durable chain, attributed to the
/// leaver, and `rev` moved on the reassigned row.
#[tokio::test(flavor = "multi_thread")]
async fn a_leave_is_one_command_group_in_the_tenant_chain() {
    let h = harness("ordo_release_chain", CHAINED).await;
    let s = h.space.clone();
    let removed: Vec<String> = rows(
        &h,
        &format!(
            r#"SELECT "id" FROM "projectMembers" WHERE "$space" = '{s}' AND "member" = 'mem_3'
               ORDER BY "id""#
        ),
    )
    .await
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect();
    assert!(!removed.is_empty(), "the seed gives mem_3 a project seat");
    let rev_before = one(
        &h,
        &format!(r#"SELECT "rev" FROM "tasks" WHERE "$space" = '{s}' AND "id" = 'task_3'"#),
    )
    .await
    .expect("task_3");
    let chained_before = chain(&h).await;

    let (status, body) = leave(&h, "mem_3").await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let chained = chain(&h).await;
    assert_eq!(
        chained.len(),
        chained_before.len() + 1,
        "one leave, one entry in the durable chain"
    );
    let group = last_group(&h).await;
    assert_eq!(
        Some(&group.correlation.0),
        chained.last(),
        "the in-memory head and the durable chain hold the same group"
    );
    assert_eq!(group.actor, Actor::User("mem_3".into()));
    assert!(!group.owner_administration);

    let deleted: Vec<&str> = commands_on(&group, "projectMembers", CommandKind::Delete)
        .iter()
        .map(|c| c.row_id.as_str())
        .collect();
    assert_eq!(
        deleted, removed,
        "each removed row is a delete with its image"
    );
    assert!(commands_on(&group, "projectMembers", CommandKind::Delete)
        .iter()
        .all(|c| c.before.is_some()));

    let moved = commands_on(&group, "tasks", CommandKind::Converge);
    let task = moved
        .iter()
        .find(|c| c.row_id == "task_3")
        .expect("the reassigned task is a converge");
    assert!(
        format!("{:?}", task.before).contains("mem_3")
            && format!("{:?}", task.after).contains(OWNER),
        "imaged before and after: {task:?}"
    );
    let rev_after = one(
        &h,
        &format!(r#"SELECT "rev" FROM "tasks" WHERE "$space" = '{s}' AND "id" = 'task_3'"#),
    )
    .await
    .expect("task_3");
    assert_eq!(
        rev_after.parse::<i64>().unwrap(),
        rev_before.parse::<i64>().unwrap() + 1,
        "rev moved the normal way"
    );

    let seat_cmd = commands_on(&group, "members", CommandKind::Converge);
    assert!(
        seat_cmd.iter().any(|c| c.row_id == "mem_3"),
        "the seat's release is in the same group: {:?}",
        group.commands
    );
}

/// Reverting the leave's group puts back the removed rows, the reassigned
/// row's assignee, and the seat as it was.
///
/// A revert is the reverting principal's own write, guarded like any other,
/// so it needs someone who may write every row the group touched. Here that
/// is mem_2, who owns `proj_2` (mem_3's project seat and `task_3` are both
/// in it). mem_3's read row on `board_1` is taken out first, because only
/// that board's admin may write it, and nobody here is both.
#[tokio::test(flavor = "multi_thread")]
async fn reverting_the_leave_restores_what_it_removed_and_reassigned() {
    let h = harness("ordo_release_revert", CHAINED).await;
    let s = h.space.clone();
    h.pool
        .rows(&format!(
            r#"DELETE FROM "boardsAccess" WHERE "$space" = '{s}' AND "member" = 'mem_3'"#
        ))
        .await
        .expect("no board row for mem_3");
    let count_seats = format!(
        r#"SELECT count(*) FROM "projectMembers" WHERE "$space" = '{s}' AND "member" = 'mem_3'"#
    );
    let assignee =
        format!(r#"SELECT "assignee" FROM "tasks" WHERE "$space" = '{s}' AND "id" = 'task_3'"#);
    let seats_before = one(&h, &count_seats).await;
    let seat_before = seat(&h, "mem_3").await;

    let (status, body) = leave(&h, "mem_3").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(one(&h, &count_seats).await.as_deref(), Some("0"));
    assert_eq!(one(&h, &assignee).await.as_deref(), Some(OWNER));

    let group = last_group(&h).await;
    let bound = h.spaces.resolve(&h.space).await.expect("the space");
    let f = bound.view.fixture.clone();
    let reverted = ordo_exec::undo::revert(
        &h.pool,
        &f.schema,
        Some(f.keys.as_ref()),
        None,
        None,
        &SpaceId(h.space.clone()),
        &group.correlation,
        "mem_2",
        ordo_commandlog::Timestamp(ordo_host::runtime::now_secs()),
    )
    .await
    .expect("a leave's group reverts like any other");
    assert_eq!(reverted.group.commands.len(), group.commands.len());

    assert_eq!(
        one(&h, &count_seats).await,
        seats_before,
        "the removed rows are back"
    );
    assert_eq!(
        one(&h, &assignee).await.as_deref(),
        Some("mem_3"),
        "the reassigned row points at the seat again"
    );
    assert_eq!(
        seat(&h, "mem_3").await,
        seat_before,
        "the seat is as it was"
    );
}

/// Everything in the control database that names the person goes with the
/// account (`plan-member-release.md` §5.3): a delegation they granted, one
/// they hold as the agent, and a recovery approval they gave. These tables
/// name a person in columns other than `principal`, which is how they were
/// left behind.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_account_removes_its_delegations_and_recovery_approvals() {
    let h = harness("ordo_release_delete_personal", "release member open").await;
    an_identity(&h, VIEWER).await;
    ordo_host::recovery::ensure_table(&h.pool)
        .await
        .expect("recovery tables");
    let s = h.space.clone();
    h.pool
        .rows(&format!(
            r#"INSERT INTO "$delegation"
               ("$space", "delegation_id", "delegator", "agent", "granted_at", "expires")
               VALUES ('{s}', 'dlg_given', '{VIEWER}', 'mem_2', {NOW}, {later}),
                      ('{s}', 'dlg_held', 'mem_2', '{VIEWER}', {NOW}, {later})"#,
            later = NOW + 3_600,
        ))
        .await
        .expect("delegations");
    h.pool
        .rows(&format!(
            r#"INSERT INTO "$recoveryapproval" ("case", "approver", "at")
               VALUES ('rc_other', '{VIEWER}', {NOW})"#
        ))
        .await
        .expect("approval");
    assert_eq!(names_person(&h).await, (2, 1), "the rows are there to go");

    let app = auth_app(&h, Arc::new(MemorySessionRevocations::new()));
    let (status, body) = leave(&h, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, out) = delete_account(&app, VIEWER, true, VIEWER).await;
    assert_eq!(status, StatusCode::OK, "{out}");

    assert_eq!(
        names_person(&h).await,
        (0, 0),
        "no delegation and no approval names the deleted person"
    );
}

/// How many delegation rows and recovery approvals name the viewer.
async fn names_person(h: &Harness) -> (usize, usize) {
    let s = h.space.clone();
    let d = rows(
        h,
        &format!(
            r#"SELECT 1 FROM "$delegation" WHERE "$space" = '{s}'
               AND '{VIEWER}' IN ("delegator", "agent")"#
        ),
    )
    .await;
    let a = rows(
        h,
        &format!(r#"SELECT 1 FROM "$recoveryapproval" WHERE "approver" = '{VIEWER}'"#),
    )
    .await;
    (d.len(), a.len())
}
