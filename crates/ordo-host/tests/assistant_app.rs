//! The assistant app (`examples/assistant`): a manager lends an assistant
//! part of their rights, through the real door and real Postgres.
//!
//! What it proves:
//!
//! - The assistant lists the manager's pending expenses and approves a small
//!   one. The row shows the manager as the one who decided, and the command
//!   log names the assistant beside the manager
//!   (`agent:<assistant> on behalf of user:<manager>`).
//! - It cannot approve one over the limit: `approveSmallExpense` changes
//!   nothing on it, and the op that would, `approveExpense`, is not in the
//!   grant.
//! - It cannot reject, cannot delete, and cannot change a role. A grant that
//!   does list `std.expenses.delete` still cannot delete, because deletes are
//!   closed to delegated tokens until `ORDO_DELEGABLE_OPS` names them.
//! - It sees the manager's expenses and no other manager's.
//! - After the manager revokes, the assistant's next call is refused.
//!
//! - The manager lends through the consent ceremony (11-4b): the assistant
//!   asks, the manager reads and approves, the assistant redeems with its
//!   binder and works. The other tests record the delegation through the
//!   registry, so each can start from a known one.
//!
//! Databases are prefixed `ordo_asst_`. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`), or any Postgres named by
//! `ORDO_PG_HOST` and `ORDO_PG_PORT`.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::delegable::DelegablePolicy;
use ordo_host::delegation::PgDelegationRegistry;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::delegation::{DelegationRegistry, NewDelegation};
use ordo_identity::principal::{
    DelegationId, GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp,
};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const SOURCE: &str = include_str!("../../../examples/assistant/assistant.ordo");

/// Maya is the manager. Daniel and Priya are staff. Omar is another manager
/// in the same tenant, with his own staff expense that Maya's assistant must
/// never see.
const SEED: &str = r#"
INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
  (@SPACE@, 'mem_maya',   'mem_maya',   'Maya',   'principal', 1, 'manager', 'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_omar',   'mem_omar',   'Omar',   'principal', 1, 'manager', 'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_daniel', 'mem_daniel', 'Daniel', 'principal', 1, 'editor',  'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_priya',  'mem_priya',  'Priya',  'principal', 1, 'editor',  'active', DATE '2026-01-01');
"#;

const MAYA: &str = "mem_maya";
const OMAR: &str = "mem_omar";
const DANIEL: &str = "mem_daniel";
const PRIYA: &str = "mem_priya";

/// The assistant. Not a member of the tenant: its authority is Maya's consent.
const ASSISTANT: &str = "agt_assistant";

/// The door's clock, fixed so an expiry is a property of the row.
const NOW: u64 = 1_000_000;

/// What the grant lists: the pending list and the small approval.
const GRANT: &[&str] = &["pendingExpenses", "approveSmallExpense"];

struct Harness {
    state: ApiState,
    /// The agent's routes: `/auth/delegation/request` and `/redeem`.
    auth: axum::Router,
    delegations: Arc<PgDelegationRegistry>,
    engine: Arc<AccessEngine>,
    pg: Pg,
    pool: ordo_exec::Pool,
}

async fn harness(db: &str) -> Harness {
    let f = Arc::new(Fixture::from_source(SOURCE, SEED).expect("assistant.ordo compiles"));
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

    // The default policy: no delete is open to delegation, which is what
    // `ORDO_DELEGABLE_OPS` unset means.
    let engine = Arc::new(
        ordo_host::deployment_engine_delegating(&f, &pg, &DelegablePolicy::closed())
            .expect("engine"),
    );

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "assistant-test",
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
    .expect("the program compiles no `std.space.*` name");

    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());
    ordo_host::consent::Ceremony::ensure_table(&pool)
        .await
        .expect("request table");
    let _ = pool.rows("DELETE FROM \"$delegation_request\"").await;
    let ceremony = Arc::new(ordo_host::consent::Ceremony {
        pool: pool.clone(),
        delegations: delegations.clone(),
        grants: grants.clone(),
        engine: {
            let e = engine.clone();
            Arc::new(move || e.clone())
        },
        spaces: spaces.clone(),
        stream: None,
        now: Arc::new(|| NOW),
        settings: ordo_host::consent::Settings {
            agents: ordo_host::consent::Settings::parse_agents("agt_assistant=Expense assistant")
                .unwrap(),
            ..Default::default()
        },
    });
    let auth = {
        let mut a = ordo_host::auth::AuthState::new(
            ordo_identity::ProviderRegistry::new(),
            ordo_identity::directory::Directory::new(
                ordo_identity::directory::Provisioning::AutoOnboard,
            ),
            TokenKey::from_secret(b"assistant-secret"),
        );
        a.now = Box::new(|| NOW);
        let a = Arc::new(a);
        a.wire_consent(ceremony.clone());
        ordo_host::auth::router(Some(a))
    };
    let ops = PlatformOps::new(engine.clone(), grants.clone(), Arc::new(|| NOW))
        .with_delegations(delegations.clone())
        .with_consent(ceremony);
    let access = ordo_host::IdentityAccess::sharing(
        engine.clone(),
        TokenKey::from_secret(b"assistant-secret"),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    )
    .with_delegations(delegations.clone());
    let access: Arc<dyn ordo_api::AccessChecker> = Arc::new(access);

    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces,
                pg: pg.clone(),
                page_size: 50,
            }),
            Arc::new(ops),
        )),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"assistant-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        auth,
        delegations,
        engine,
        pg,
        pool,
    }
}

fn space() -> ISpace {
    ISpace(DEFAULT_SPACE.to_string())
}

/// Maya consents: the assistant may act in her name until `NOW + 3600`.
async fn lend(h: &Harness, id: &str) {
    h.delegations
        .record(&NewDelegation {
            id: DelegationId(id.into()),
            space: space(),
            delegator: PrincipalId(MAYA.into()),
            agent: PrincipalId(ASSISTANT.into()),
            granted_at: IStamp(NOW),
            expires: IStamp(NOW + 3_600),
            consent_correlation: format!("cg_consent_{id}"),
        })
        .await
        .expect("recorded");
}

/// The assistant's token: the delegation it claims, and a grant over `ops`.
async fn assistant_token(h: &Harness, id: &str, ops: &[&str]) -> String {
    let record = h
        .delegations
        .live_in(&space(), IStamp(NOW))
        .await
        .expect("live")
        .into_iter()
        .find(|d| d.id.0 == id)
        .expect("the delegation is recorded");
    let set: BTreeSet<String> = ops.iter().map(|s| s.to_string()).collect();
    let grant = h
        .engine
        .grant_for(
            GrantId("gr_assistant".into()),
            &space(),
            set,
            IStamp(NOW + 3_600),
        )
        .expect("every op named is deployed");
    ordo_identity::mint(
        &TokenKey::from_secret(b"assistant-secret"),
        &TokenSpec {
            principal: PrincipalId(ASSISTANT.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: Some(grant),
            delegation: Some(record.claim()),
            session: None,
        },
    )
}

/// A person's own token, after a passkey touch now.
fn present(id: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(b"assistant-secret"),
        &TokenSpec {
            principal: PrincipalId(id.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: Some(IStamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// A call to the agent's routes, which take no token.
async fn agent_route(h: &Harness, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = h.auth.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A person's own token.
fn person(id: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(b"assistant-secret"),
        &TokenSpec {
            principal: PrincipalId(id.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn send(
    state: &ApiState,
    method: &str,
    op: &str,
    bearer: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(format!("/op/{op}"))
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", format!("Bearer {bearer}"));
    let req = if body.is_null() {
        req.body(Body::empty()).unwrap()
    } else {
        req.header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
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

async fn get(h: &Harness, op: &str, bearer: &str) -> (StatusCode, Value) {
    send(&h.state, "GET", op, bearer, Value::Null).await
}

async fn post(h: &Harness, op: &str, bearer: &str, body: Value) -> (StatusCode, Value) {
    send(&h.state, "POST", op, bearer, body).await
}

fn code(body: &Value) -> String {
    body["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A member submits an expense to a manager. Returns its id.
async fn submit(h: &Harness, who: &str, manager: &str, amount: &str, title: &str) -> String {
    let (st, body) = post(
        h,
        "submitExpense",
        &person(who),
        json!({
            "manager": manager,
            "amount": amount,
            "title": title,
            "added": "2026-10-01T09:00:00Z",
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    h.pg.query(&format!(
        r#"SELECT "id" FROM "expenses" WHERE "title" = '{title}'"#
    ))
    .expect("row")[0][0]
        .clone()
        .expect("id")
}

fn status_of(h: &Harness, id: &str) -> (String, String) {
    let r =
        h.pg.query(&format!(
            r#"SELECT "status", COALESCE("decidedBy", '') FROM "expenses" WHERE "id" = '{id}'"#
        ))
        .expect("row");
    (
        r[0][0].clone().unwrap_or_default(),
        r[0][1].clone().unwrap_or_default(),
    )
}

fn titles(body: &Value) -> Vec<String> {
    let mut t: Vec<String> = body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows in {body}"))
        .iter()
        .map(|r| r["title"].as_str().unwrap_or_default().to_string())
        .collect();
    t.sort();
    t
}

// ───────────── the consent ceremony ─────────────

/// The whole ceremony, as the example runs it: the assistant asks for its two
/// ops, Maya reads what they do and approves, the assistant redeems with its
/// binder and approves a small expense in her name. Maya revokes, and the
/// assistant's next call is refused.
#[tokio::test(flavor = "multi_thread")]
async fn the_manager_lends_through_the_consent_ceremony() {
    let h = harness("ordo_asst_ceremony").await;
    let small = submit(&h, DANIEL, MAYA, "42.50", "taxi").await;

    let (st, asked) = agent_route(
        &h,
        "/auth/delegation/request",
        json!({"space": DEFAULT_SPACE, "agent": ASSISTANT, "person": MAYA, "ops": GRANT, "hours": 8}),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{asked}");
    let request = asked["request"].as_str().unwrap().to_string();
    let binder = asked["binder"].as_str().unwrap().to_string();

    let (st, d) = get(
        &h,
        &format!("std.delegation.review?request={request}"),
        &person(MAYA),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{d}");
    let d = &d["rows"][0];
    assert_eq!(d["agent"]["name"], "Expense assistant");
    let shown: Vec<&str> = d["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["op"].as_str().unwrap())
        .collect();
    assert_eq!(shown, vec!["approveSmallExpense", "pendingExpenses"]);
    assert!(d["ops"][0]["words"]
        .to_string()
        .contains("Changes decidedBy, decidedVia, note, status in expenses."));

    let (st, a) = post(
        &h,
        "std.delegation.approve",
        &present(MAYA),
        json!({ "request": request }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{a}");
    let delegation = a["rows"][0]["delegation"].as_str().unwrap().to_string();

    let (st, r) = agent_route(
        &h,
        "/auth/delegation/redeem",
        json!({ "request": request, "binder": binder }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let token = r["token"].as_str().unwrap().to_string();

    let (st, body) = post(
        &h,
        "approveSmallExpense",
        &token,
        json!({ "expense": small }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(status_of(&h, &small), ("approved".into(), MAYA.into()));
    let (st, body) = post(&h, "rejectExpense", &token, json!({ "expense": small })).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");

    let (st, body) = post(
        &h,
        "std.delegation.revoke",
        &person(MAYA),
        json!({ "delegation": delegation }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, body) = get(&h, "pendingExpenses", &token).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "delegation_required");
}

// ───────────── the assistant at work ─────────────

/// The assistant lists what waits for Maya and approves a small expense. The
/// row says Maya decided. The command log says the assistant acted for her.
#[tokio::test(flavor = "multi_thread")]
async fn the_assistant_approves_a_small_expense_and_the_record_names_both() {
    use ordo_commandlog::{Actor, Store, Value as CV};

    let h = harness("ordo_asst_approve").await;
    let small = submit(&h, DANIEL, MAYA, "42.50", "taxi").await;
    lend(&h, "dlg_1").await;
    let t = assistant_token(&h, "dlg_1", GRANT).await;

    let (st, body) = get(&h, "pendingExpenses", &t).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(titles(&body), vec!["taxi"]);

    let (st, body) = post(
        &h,
        "approveSmallExpense",
        &t,
        json!({ "expense": small, "note": "by the assistant" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // `$caller` is Maya: the row says she decided.
    assert_eq!(status_of(&h, &small), ("approved".into(), MAYA.into()));

    // The history, read as Maya, shows the approval in her name.
    let (st, body) = get(&h, "history", &person(MAYA)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["status"], "approved");
    assert_eq!(row["decidedBy"], "Maya");
    assert_eq!(row["decidedVia"], "approveSmallExpense");

    // The command log names the assistant beside Maya, and the chain holds.
    let store =
        ordo_exec::commandlog::PgStore::new(h.pool.clone(), tokio::runtime::Handle::current());
    let cspace = ordo_commandlog::SpaceId(DEFAULT_SPACE.to_string());
    tokio::task::spawn_blocking(move || {
        let all = store.entries(&cspace, 0, store.len(&cspace));
        let decided = all
            .iter()
            .find(|e| {
                e.group.commands.iter().any(|c| {
                    c.after
                        .as_ref()
                        .and_then(|a| a.get("status"))
                        .is_some_and(|v| *v == CV::Text("approved".into()))
                })
            })
            .expect("a group wrote the approval");
        assert_eq!(
            decided.group.actor,
            Actor::AgentOnBehalfOf {
                agent: ASSISTANT.into(),
                user: MAYA.into(),
            }
        );
        assert_eq!(
            decided.group.actor.to_string(),
            format!("agent:{ASSISTANT} on behalf of user:{MAYA}")
        );
        // Daniel's own submission is recorded as Daniel alone.
        let submitted = all
            .iter()
            .find(|e| {
                e.group.commands.iter().any(|c| {
                    c.after
                        .as_ref()
                        .and_then(|a| a.get("title"))
                        .is_some_and(|v| *v == CV::Text("taxi".into()))
                })
            })
            .expect("a group wrote the submission");
        assert_eq!(submitted.group.actor, Actor::User(DANIEL.into()));
        store.verify(&cspace).expect("the chain verifies");
    })
    .await
    .unwrap();
}

/// The assistant sees Maya's expenses and not Omar's: the guard answers for
/// the delegator.
#[tokio::test(flavor = "multi_thread")]
async fn the_assistant_sees_only_the_managers_expenses() {
    let h = harness("ordo_asst_sees").await;
    let _ = submit(&h, DANIEL, MAYA, "10.00", "stamps").await;
    let omars = submit(&h, PRIYA, OMAR, "11.00", "tape").await;
    lend(&h, "dlg_1").await;
    let t = assistant_token(&h, "dlg_1", GRANT).await;

    let (st, body) = get(&h, "pendingExpenses", &t).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(titles(&body), vec!["stamps"], "Omar's tape is not Maya's");

    // Omar's expense is not hers to approve, so it is not found.
    let (_, _) = post(&h, "approveSmallExpense", &t, json!({ "expense": omars })).await;
    assert_eq!(status_of(&h, &omars).0, "pending", "untouched");
}

// ───────────── what the assistant cannot do ─────────────

/// The limit lives in `approveSmallExpense`'s body. An expense over 100.00 is
/// left pending by it, and the op that would approve it is not in the grant.
#[tokio::test(flavor = "multi_thread")]
async fn the_assistant_cannot_approve_over_the_limit() {
    let h = harness("ordo_asst_limit").await;
    let big = submit(&h, DANIEL, MAYA, "250.00", "monitor").await;
    let edge = submit(&h, DANIEL, MAYA, "100.00", "dinner").await;
    let over = submit(&h, DANIEL, MAYA, "100.01", "gift").await;
    lend(&h, "dlg_1").await;
    let t = assistant_token(&h, "dlg_1", GRANT).await;

    // The condition matches no row, which is the ordinary no-op of a bulk
    // write (G11): the door answers success and nothing changes.
    let (st, body) = post(&h, "approveSmallExpense", &t, json!({ "expense": big })).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(status_of(&h, &big).0, "pending", "{body}");

    // The limit is inclusive: exactly 100.00 passes, one cent more does not.
    let (st, body) = post(&h, "approveSmallExpense", &t, json!({ "expense": edge })).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(status_of(&h, &edge).0, "approved");
    let _ = post(&h, "approveSmallExpense", &t, json!({ "expense": over })).await;
    assert_eq!(status_of(&h, &over).0, "pending");

    // The op with no limit is not in the grant.
    let (st, body) = post(&h, "approveExpense", &t, json!({ "expense": big })).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    assert_eq!(status_of(&h, &big).0, "pending");

    // Maya herself may approve it.
    let (st, body) = post(
        &h,
        "approveExpense",
        &person(MAYA),
        json!({ "expense": big }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(status_of(&h, &big).0, "approved");
}

/// 25bd: a bulk write whose condition matches no row used to answer 500
/// `log_append_failed`. It is a success that changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_approval_that_matches_no_expense_is_an_empty_success_not_a_server_error() {
    let h = harness("ordo_asst_empty").await;
    let big = submit(&h, DANIEL, MAYA, "250.00", "monitor").await;
    lend(&h, "dlg_1").await;
    let t = assistant_token(&h, "dlg_1", GRANT).await;

    let (st, body) = post(&h, "approveSmallExpense", &t, json!({ "expense": big })).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_ne!(code(&body), "log_append_failed", "{body}");
    assert_eq!(status_of(&h, &big).0, "pending");

    // An expense that does not exist at all answers the same way.
    let (st, body) = post(
        &h,
        "approveSmallExpense",
        &t,
        json!({ "expense": "exp_none" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

/// 25be: a person admitted to the tenant reads back by the name the
/// directory holds, and by the id only when the directory holds none.
/// Admission by sign-in is covered in `admission.rs`; this is the
/// administrator's `std.space.admit`, which fell back to the id whenever the
/// call named no display. `submitter.displayName` reads this column.
#[tokio::test(flavor = "multi_thread")]
async fn a_staff_member_admitted_without_a_display_reads_back_by_the_name_the_directory_holds() {
    use ordo_identity::directory::{DirectoryStore, MemoryDirectoryStore};
    use ordo_identity::principal::{PrincipalKind, Timestamp};

    let h = harness("ordo_asst_names").await;
    let dir = Arc::new(MemoryDirectoryStore::new());
    let named = dir
        .create_principal(PrincipalKind::Human, "Nadia Okafor", Timestamp(1))
        .expect("minted")
        .0;
    let nameless = dir
        .create_principal(PrincipalKind::Human, "", Timestamp(1))
        .expect("minted")
        .0;
    let members = ordo_host::member_ops::MemberOps {
        control: h.pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(h.pool.clone())),
        directory: Some(dir),
        stream: Some(Arc::default()),
        refresh: None,
        now: Arc::new(|| NOW),
        managed: None,
    };
    let space = ordo_conformance::fixture_space().0;
    for who in [&named, &nameless] {
        let mut m = serde_json::Map::new();
        m.insert("principal".into(), json!(who));
        m.insert("role".into(), json!("editor"));
        members
            .execute(
                ordo_host::member_ops::ADMIT,
                &space,
                &ordo_api::BoundParams(m),
                MAYA,
            )
            .await
            .expect("admitted");
    }
    let shown = |who: &str| {
        h.pg.query(&format!(
            r#"SELECT "displayName" FROM "members" WHERE "principal" = '{who}'"#
        ))
        .expect("row")[0][0]
            .clone()
            .unwrap_or_default()
    };
    assert_eq!(shown(&named), "Nadia Okafor");
    assert_eq!(
        shown(&nameless),
        nameless,
        "the id only when no name exists"
    );
}

/// Reject, delete and change a role are all refused.
#[tokio::test(flavor = "multi_thread")]
async fn the_assistant_cannot_reject_delete_or_change_a_role() {
    let h = harness("ordo_asst_closed").await;
    let id = submit(&h, DANIEL, MAYA, "20.00", "lunch").await;
    lend(&h, "dlg_1").await;
    let t = assistant_token(&h, "dlg_1", GRANT).await;

    // Reject: not in the grant.
    let (st, body) = post(&h, "rejectExpense", &t, json!({ "expense": id })).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");

    // Delete: not in the grant either.
    let (st, body) = send(
        &h.state,
        "DELETE",
        &format!("std.expenses.delete?id={id}"),
        &t,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");

    // A role change is a platform write. Not in the grant, and closed to
    // every delegated token whatever the grant lists.
    let (st, body) = post(
        &h,
        "std.space.setRole",
        &t,
        json!({ "member": DANIEL, "role": "owner" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied", "{body}");

    assert_eq!(status_of(&h, &id).0, "pending", "nothing changed");
    let kept =
        h.pg.query(r#"SELECT "role" FROM "members" WHERE "id" = 'mem_daniel'"#)
            .unwrap();
    assert_eq!(kept[0][0].as_deref(), Some("editor"));
}

/// Even a grant that lists the derived delete and a platform write does not
/// open them: deletes are closed to delegated tokens until the deployment
/// owner names them in `ORDO_DELEGABLE_OPS`, and platform writes never open.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_that_lists_a_delete_still_cannot_delete() {
    let h = harness("ordo_asst_delete_closed").await;
    let id = submit(&h, DANIEL, MAYA, "20.00", "lunch").await;
    lend(&h, "dlg_1").await;
    assert!(!h.engine.delegable("std.expenses.delete"));
    assert!(h.engine.delegable("approveSmallExpense"));
    assert!(h.engine.delegable("pendingExpenses"));
    let t = assistant_token(&h, "dlg_1", &["approveSmallExpense", "std.expenses.delete"]).await;

    let (st, body) = send(
        &h.state,
        "DELETE",
        &format!("std.expenses.delete?id={id}"),
        &t,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied", "{body}");
    let rows =
        h.pg.query(&format!(
            r#"SELECT count(*) FROM "expenses" WHERE "id" = '{id}'"#
        ))
        .unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("1"), "the row is still there");

    // Maya may delete it herself.
    let (st, body) = send(
        &h.state,
        "DELETE",
        &format!("std.expenses.delete?id={id}"),
        &person(MAYA),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

// ───────────── taking it back ─────────────

/// The manager revokes. The same token, unexpired, is refused on its next call.
#[tokio::test(flavor = "multi_thread")]
async fn after_the_manager_revokes_the_assistants_next_call_is_refused() {
    let h = harness("ordo_asst_revoke").await;
    let first = submit(&h, DANIEL, MAYA, "15.00", "pens").await;
    let second = submit(&h, DANIEL, MAYA, "16.00", "paper").await;
    lend(&h, "dlg_1").await;
    let t = assistant_token(&h, "dlg_1", GRANT).await;

    let (st, body) = post(&h, "approveSmallExpense", &t, json!({ "expense": first })).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    h.delegations
        .revoke(&space(), &DelegationId("dlg_1".into()), IStamp(NOW))
        .await
        .expect("revoked");

    let (st, body) = post(&h, "approveSmallExpense", &t, json!({ "expense": second })).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "delegation_required", "{body}");
    assert_eq!(status_of(&h, &second).0, "pending", "nothing was approved");

    let (st, body) = get(&h, "pendingExpenses", &t).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "delegation_required", "{body}");

    // The manager is unaffected.
    let (st, body) = get(&h, "pendingExpenses", &person(MAYA)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(titles(&body), vec!["paper"]);
}
