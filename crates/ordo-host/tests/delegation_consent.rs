//! The consent ceremony (checklist 11-4b), through the real door and real
//! Postgres, over the assistant app (`examples/assistant`).
//!
//! An agent asks; the person reads a computed disclosure and approves with
//! presence; the agent redeems with its binder and gets a token for the
//! approved ops and nothing else. The person lists and revokes. Every step
//! is a security event.
//!
//! Databases are prefixed `ordo_consent_`. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`), or any Postgres named by
//! `ORDO_PG_HOST` and `ORDO_PG_PORT`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::auth::AuthState;
use ordo_host::consent::{Ceremony, RequestRow, Settings};
use ordo_host::delegable::DelegablePolicy;
use ordo_host::delegation::PgDelegationRegistry;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::delegation::DelegationRegistry;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::principal::{PrincipalId, SpaceId as ISpace, Timestamp as IStamp};
use ordo_identity::{AccessEngine, ProviderRegistry, TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const SOURCE: &str = include_str!("../../../examples/assistant/assistant.ordo");

const SEED: &str = r#"
INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
  (@SPACE@, 'mem_maya',   'mem_maya',   'Maya',   'principal', 1, 'manager', 'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_omar',   'mem_omar',   'Omar',   'principal', 1, 'manager', 'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_daniel', 'mem_daniel', 'Daniel', 'principal', 1, 'editor',  'active', DATE '2026-01-01');
"#;

const MAYA: &str = "mem_maya";
const OMAR: &str = "mem_omar";
const DANIEL: &str = "mem_daniel";
const ASSISTANT: &str = "agt_assistant";
const SECRET: &[u8] = b"consent-secret";
const NOW: u64 = 1_000_000;

struct H {
    state: ApiState,
    auth: axum::Router,
    clock: Arc<AtomicU64>,
    events: Arc<ordo_secevent::Recorder>,
    delegations: Arc<PgDelegationRegistry>,
    engine: Arc<AccessEngine>,
    fixture: Arc<Fixture>,
    pg: Pg,
}

impl H {
    fn now(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }

    fn advance(&self, secs: u64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }

    /// Every ceremony event so far as `act detail`, drained.
    fn events(&self) -> Vec<String> {
        self.events
            .drain_all(ordo_commandlog::Timestamp(self.now() + 3_600))
            .into_iter()
            .map(|e| match &e.kind {
                ordo_secevent::EventKind::Enrolment { act, detail } => format!("{act} {detail}"),
                _ => e.render(),
            })
            .collect()
    }
}

fn settings() -> Settings {
    Settings {
        agents: Settings::parse_agents("agt_assistant=Expense assistant").unwrap(),
        ..Settings::default()
    }
}

async fn harness(db: &str) -> H {
    harness_with(db, settings(), None).await
}

async fn harness_with(
    db: &str,
    settings: Settings,
    human: Option<ordo_host::auth::human::HumanCheck>,
) -> H {
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
    Ceremony::ensure_table(&pool).await.expect("request table");
    for t in ["$capabilitygrant", "$delegation", "$delegation_request"] {
        let _ = pool.rows(&format!("DELETE FROM \"{t}\"")).await;
    }

    let engine = Arc::new(
        ordo_host::deployment_engine_delegating(&f, &pg, &DelegablePolicy::closed())
            .expect("engine"),
    );
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "consent-test",
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
    .expect("the program compiles no platform name");
    let spaces = ordo_host::spaces::Spaces::new(deployment.clone());

    let clock = Arc::new(AtomicU64::new(NOW));
    let now: Arc<dyn Fn() -> u64 + Send + Sync> = {
        let c = clock.clone();
        Arc::new(move || c.load(Ordering::SeqCst))
    };
    let events = Arc::new(ordo_secevent::Recorder::default());
    let ceremony = Arc::new(Ceremony {
        pool: pool.clone(),
        delegations: delegations.clone(),
        grants: grants.clone(),
        engine: {
            let e = engine.clone();
            Arc::new(move || e.clone())
        },
        spaces: spaces.clone(),
        stream: Some(events.clone()),
        now: now.clone(),
        settings,
    });

    let ops = PlatformOps::new(engine.clone(), grants.clone(), now.clone())
        .with_delegations(delegations.clone())
        .with_consent(ceremony.clone());
    let access = {
        let c = clock.clone();
        ordo_host::IdentityAccess::sharing(
            engine.clone(),
            TokenKey::from_secret(SECRET),
            Box::new(move || c.load(Ordering::SeqCst)),
            Arc::new(ordo_host::metrics::Metrics::new()),
        )
        .with_delegations(delegations.clone())
    };
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
        page_key: PageKey::from_secret(b"consent-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };

    let mut auth = AuthState::new(
        ProviderRegistry::new(),
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    );
    let c = clock.clone();
    auth.now = Box::new(move || c.load(Ordering::SeqCst));
    if let Some(check) = human {
        auth = auth.with_human_check(check);
    }
    let auth = Arc::new(auth);
    auth.wire_consent(ceremony);

    H {
        state,
        auth: ordo_host::auth::router(Some(auth)),
        clock,
        events,
        delegations,
        engine,
        fixture: f,
        pg,
    }
}

/// A person's session token. `presence` stamps a passkey touch now.
fn person(h: &H, id: &str, presence: bool) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(id.into()),
            expires: IStamp(h.now() + 3_600),
            client: None,
            user_presence_at: presence.then_some(IStamp(h.now())),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn read_body(resp: axum::response::Response) -> (StatusCode, Value) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A call through the op door.
async fn op(h: &H, method: &str, uri: &str, bearer: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", format!("Bearer {bearer}"));
    let req = if body.is_null() {
        req.body(Body::empty()).unwrap()
    } else {
        req.header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    read_body(router(h.state.clone()).oneshot(req).await.unwrap()).await
}

/// A call to the agent's routes, which take no token.
async fn agent(h: &H, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    read_body(h.auth.clone().oneshot(req).await.unwrap()).await
}

fn code(body: &Value) -> String {
    body["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The assistant asks Maya for `ops`. Returns (request, binder).
async fn ask(h: &H, ops: &[&str], hours: Option<u64>) -> (String, String) {
    let mut body = json!({
        "space": DEFAULT_SPACE,
        "agent": ASSISTANT,
        "person": MAYA,
        "ops": ops,
    });
    if let Some(n) = hours {
        body["hours"] = json!(n);
    }
    let (st, b) = agent(h, "/auth/delegation/request", body).await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    (
        b["request"].as_str().unwrap().to_string(),
        b["binder"].as_str().unwrap().to_string(),
    )
}

async fn review(h: &H, who: &str, request: &str) -> (StatusCode, Value) {
    op(
        h,
        "GET",
        &format!("/op/std.delegation.review?request={request}"),
        &person(h, who, false),
        Value::Null,
    )
    .await
}

async fn approve(h: &H, who: &str, presence: bool, request: &str) -> (StatusCode, Value) {
    op(
        h,
        "POST",
        "/op/std.delegation.approve",
        &person(h, who, presence),
        json!({ "request": request }),
    )
    .await
}

async fn redeem(h: &H, request: &str, binder: &str) -> (StatusCode, Value) {
    agent(
        h,
        "/auth/delegation/redeem",
        json!({ "request": request, "binder": binder }),
    )
    .await
}

/// A member submits an expense to a manager. Returns its id.
async fn submit(h: &H, who: &str, manager: &str, amount: &str, title: &str) -> String {
    let (st, body) = op(
        h,
        "POST",
        "/op/submitExpense",
        &person(h, who, false),
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

/// The whole ceremony, end to end: ask, read, approve, redeem, use.
async fn granted(h: &H, ops: &[&str]) -> (String, Value) {
    let (request, binder) = ask(h, ops, None).await;
    let (st, b) = approve(h, MAYA, true, &request).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let (st, b) = redeem(h, &request, &binder).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    (b["token"].as_str().unwrap().to_string(), b)
}

// ───────────── the ceremony ─────────────

/// Ask, review, approve with presence, redeem: the token works, it acts as
/// Maya, and the delegation's consent correlation is the approval's event.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_the_whole_ceremony_gives_a_working_token() {
    let h = harness("ordo_consent_whole").await;
    let small = submit(&h, DANIEL, MAYA, "42.50", "taxi").await;

    let (request, binder) = ask(&h, &["pendingExpenses", "approveSmallExpense"], Some(2)).await;
    let (st, d) = review(&h, MAYA, &request).await;
    assert_eq!(st, StatusCode::OK, "{d}");
    let d = &d["rows"][0];
    assert_eq!(d["agent"]["name"], "Expense assistant");
    assert_eq!(d["term"]["hours"], 2);
    assert_eq!(d["term"]["expiresIfApprovedNow"], NOW + 7_200);

    // Before the person answers, the agent waits.
    let (st, b) = redeem(&h, &request, &binder).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    assert_eq!(b["status"], "waiting");

    let (st, a) = approve(&h, MAYA, true, &request).await;
    assert_eq!(st, StatusCode::OK, "{a}");
    let delegation = a["rows"][0]["delegation"].as_str().unwrap().to_string();
    assert_eq!(a["rows"][0]["expiresAt"], NOW + 7_200);

    let (st, r) = redeem(&h, &request, &binder).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["delegation"], delegation);
    assert_eq!(r["expiresAt"], NOW + 7_200);
    let token = r["token"].as_str().unwrap();

    let (st, body) = op(&h, "GET", "/op/pendingExpenses", token, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 1);
    let (st, body) = op(
        &h,
        "POST",
        "/op/approveSmallExpense",
        token,
        json!({ "expense": small }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // The record joins to the approval's event.
    let record = h
        .delegations
        .granted_by(&ISpace(DEFAULT_SPACE.into()), &PrincipalId(MAYA.into()))
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.id.0 == delegation)
        .expect("recorded");
    assert_eq!(record.agent.0, ASSISTANT);
    let approved = h
        .events
        .drain_all(ordo_commandlog::Timestamp(h.now() + 3_600))
        .into_iter()
        .find(|e| {
            matches!(&e.kind, ordo_secevent::EventKind::Enrolment { act, .. } if act == "delegation-approved")
        })
        .expect("the approval is recorded");
    assert!(
        approved
            .correlations
            .iter()
            .any(|c| c.0 == record.consent_correlation),
        "consent_correlation names the approval's event"
    );
}

/// The disclosure is the manifests in plain words: every field each op
/// reads or writes, the guard-read fields, and nothing the agent wrote.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_the_disclosure_matches_the_ops_manifests() {
    let h = harness("ordo_consent_disclose").await;
    let (request, _) = ask(
        &h,
        &[
            "pendingExpenses",
            "approveSmallExpense",
            "std.expenses.delete",
            "keeps_your_inbox_tidy",
        ],
        None,
    )
    .await;
    let (st, d) = review(&h, MAYA, &request).await;
    assert_eq!(st, StatusCode::OK, "{d}");
    let d = d["rows"][0].clone();

    for o in d["ops"].as_array().unwrap() {
        let name = o["op"].as_str().unwrap();
        let m = ordo_host::manifest(&h.fixture, name).expect("a manifest");
        let mut want_reads = BTreeMap::new();
        let mut want_writes = BTreeMap::new();
        let mut want_examines = BTreeMap::new();
        for s in m["scope"].as_array().unwrap() {
            let entity = s["entity"].as_str().unwrap().to_string();
            let read = s["read"].clone();
            let written = s["written"].clone();
            if s["via"] == "guard" {
                if !read.as_array().unwrap().is_empty() {
                    want_examines.insert(entity, read);
                }
                continue;
            }
            if !read.as_array().unwrap().is_empty() {
                want_reads.insert(entity.clone(), read);
            }
            if !written.as_array().unwrap().is_empty() {
                want_writes.insert(entity, written);
            }
        }
        let got = |k: &str| -> BTreeMap<String, Value> {
            o[k].as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e["entity"].as_str().unwrap().to_string(),
                        e["fields"].clone(),
                    )
                })
                .collect()
        };
        assert_eq!(got("reads"), want_reads, "{name} reads");
        assert_eq!(got("writes"), want_writes, "{name} writes");
        assert_eq!(got("examines"), want_examines, "{name} examines");
        assert!(!o["words"].as_array().unwrap().is_empty());
    }
    let names: Vec<&str> = d["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["op"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["approveSmallExpense", "pendingExpenses"]);
    // The approval writes the expense's decision fields.
    let approve = &d["ops"][0];
    assert!(approve["writes"][0]["fields"]
        .as_array()
        .unwrap()
        .contains(&json!("status")));

    // The delete is shown as not granted, with the setting that would open it.
    assert_eq!(d["notGranted"][0]["op"], "std.expenses.delete");
    assert!(d["notGranted"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("ORDO_DELEGABLE_OPS"));

    // No agent-supplied text: a name that is not an op is counted, never shown.
    assert_eq!(d["unknownNames"], 1);
    assert!(!d.to_string().contains("inbox"), "{d}");
    // The name shown is the app developer's, the term is a date, and the
    // closed doors are named.
    assert_eq!(d["agent"]["namedBy"], "the app developer");
    assert!(d["term"]["expiresText"].as_str().unwrap().ends_with("UTC"));
    assert!(d["cannot"].to_string().contains("passkey"));
    // Maya is a manager, not the owner: the owner-visibility line is there.
    assert!(d["ownerSees"].is_string());

    // The pure function agrees with the op.
    let row = RequestRow {
        id: request.clone(),
        space: DEFAULT_SPACE.into(),
        agent: ASSISTANT.into(),
        person: MAYA.into(),
        ops: vec!["approveSmallExpense".into(), "pendingExpenses".into()],
        term_secs: 3_600,
        created_at: NOW,
        expires_at: NOW + 600,
        status: "waiting".into(),
        decided_at: None,
        delegator: None,
        delegation: None,
        grant: None,
        granted: Vec::new(),
        delegation_expires: None,
    };
    let pure = Ceremony::disclose(&h.fixture, &h.engine, &settings(), &row, NOW);
    assert_eq!(pure["ops"], d["ops"]);
}

/// 25bf: `pendingExpenses` returns `submitter.displayName`, so the person is
/// told it reads `members.displayName`. The ref hop is the op's own read,
/// never a guard's, and the disclosure must not shrink it to "examines".
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_a_field_reached_through_a_ref_hop_is_disclosed_as_read() {
    let h = harness("ordo_consent_hop").await;
    let (request, _) = ask(&h, &["pendingExpenses"], None).await;
    let (st, d) = review(&h, MAYA, &request).await;
    assert_eq!(st, StatusCode::OK, "{d}");
    let o = d["rows"][0]["ops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["op"] == "pendingExpenses")
        .cloned()
        .expect("pendingExpenses is disclosed");
    let members = o["reads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entity"] == "members")
        .unwrap_or_else(|| panic!("members is among the reads: {o}"));
    assert!(
        members["fields"]
            .as_array()
            .unwrap()
            .contains(&json!("displayName")),
        "{o}"
    );
    assert!(
        !o["examines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["entity"] == "members"),
        "members.displayName is the op's read, not a guard's: {o}"
    );
}

/// Approval asks for a fresh passkey touch. Without one the door refuses,
/// and the request stays open for the approval that has it.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_approval_needs_presence() {
    let h = harness("ordo_consent_presence").await;
    let (request, _) = ask(&h, &["pendingExpenses"], None).await;
    let (st, b) = approve(&h, MAYA, false, &request).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    assert_eq!(code(&b), "access_denied");
    // A stale touch is no better.
    let stale = ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(MAYA.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: Some(IStamp(NOW - 3_600)),
            capability: None,
            delegation: None,
            session: None,
        },
    );
    let (st, _) = op(
        &h,
        "POST",
        "/op/std.delegation.approve",
        &stale,
        json!({ "request": request }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, b) = approve(&h, MAYA, true, &request).await;
    assert_eq!(st, StatusCode::OK, "{b}");
}

/// The token reaches the approved ops and nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_the_token_works_only_for_the_approved_ops() {
    let h = harness("ordo_consent_scope").await;
    let big = submit(&h, DANIEL, MAYA, "250.00", "flight").await;
    let (token, r) = granted(&h, &["pendingExpenses", "std.expenses.delete"]).await;
    assert_eq!(
        r["ops"],
        json!(["pendingExpenses"]),
        "the delete is not granted"
    );

    let (st, b) = op(&h, "GET", "/op/pendingExpenses", &token, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    for (method, path, body) in [
        ("POST", "/op/approveExpense", json!({ "expense": big })),
        ("POST", "/op/approveSmallExpense", json!({ "expense": big })),
        ("POST", "/op/rejectExpense", json!({ "expense": big })),
        ("DELETE", "/op/std.expenses.delete", json!({ "id": big })),
    ] {
        let (st, b) = op(&h, method, path, &token, body).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{path}: {b}");
        assert_eq!(code(&b), "access_denied", "{path}");
    }
    // The agent cannot touch the ceremony in Maya's name.
    let (st, _) = op(&h, "GET", "/op/std.delegation.mine", &token, Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (other, _) = ask(&h, &["pendingExpenses"], None).await;
    let (st, _) = op(
        &h,
        "POST",
        "/op/std.delegation.approve",
        &token,
        json!({ "request": other }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "an agent cannot approve, even for itself"
    );
}

/// Only the person the request names may read or answer it.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_only_the_named_person_can_approve() {
    let h = harness("ordo_consent_person").await;
    let (request, binder) = ask(&h, &["pendingExpenses"], None).await;
    let (st, b) = review(&h, OMAR, &request).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{b}");
    let (st, b) = approve(&h, OMAR, true, &request).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{b}");
    let (st, _) = approve(&h, DANIEL, true, &request).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, b) = redeem(&h, &request, &binder).await;
    assert_eq!(st, StatusCode::ACCEPTED, "still waiting for Maya: {b}");
    let events = h.events().join("\n");
    assert!(events.contains("reason=not-the-person"), "{events}");
}

/// A request id alone redeems nothing: the binder is the agent's secret.
/// And a request gives one token.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_a_stolen_request_id_cannot_be_redeemed() {
    let h = harness("ordo_consent_binder").await;
    let (request, binder) = ask(&h, &["pendingExpenses"], None).await;
    let (st, _) = approve(&h, MAYA, true, &request).await;
    assert_eq!(st, StatusCode::OK);

    let (st, b) = redeem(&h, &request, "0".repeat(64).as_str()).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{b}");
    assert_eq!(code(&b), "not_found");
    let (st, b) = redeem(&h, "dr_nosuchrequest", &binder).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "the same answer: {b}");

    let (st, b) = redeem(&h, &request, &binder).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let (st, b) = redeem(&h, &request, &binder).await;
    assert_eq!(st, StatusCode::GONE, "{b}");
    assert_eq!(code(&b), "delegation_redeemed");
    let events = h.events().join("\n");
    assert!(
        events.contains("reason=unknown-or-wrong-binder"),
        "{events}"
    );
    assert!(events.contains("reason=replayed"), "{events}");
}

/// The token stops at the delegation's expiry, and a request that waits
/// too long lapses.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_expiry() {
    let h = harness("ordo_consent_expiry").await;
    let (token, _) = granted(&h, &["pendingExpenses"]).await;
    let (st, _) = op(&h, "GET", "/op/pendingExpenses", &token, Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    h.advance(3_600);
    let (st, _) = op(&h, "GET", "/op/pendingExpenses", &token, Value::Null).await;
    assert_ne!(st, StatusCode::OK, "an hour later the token is refused");

    let (request, binder) = ask(&h, &["pendingExpenses"], None).await;
    h.advance(601);
    let (st, _) = approve(&h, MAYA, true, &request).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "the request lapsed");
    let (st, b) = redeem(&h, &request, &binder).await;
    assert_eq!(st, StatusCode::GONE, "{b}");
    assert_eq!(code(&b), "delegation_expired");
}

/// Revoking stops the agent's next call. Only the person who granted it
/// may revoke, and the list says what happened.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_revoke_stops_the_next_call() {
    let h = harness("ordo_consent_revoke").await;
    let (token, r) = granted(&h, &["pendingExpenses"]).await;
    let delegation = r["delegation"].as_str().unwrap().to_string();

    let (st, b) = op(
        &h,
        "GET",
        "/op/std.delegation.mine",
        &person(&h, MAYA, false),
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["state"], "live");
    assert_eq!(b["rows"][0]["agentName"], "Expense assistant");

    let (st, _) = op(
        &h,
        "POST",
        "/op/std.delegation.revoke",
        &person(&h, OMAR, false),
        json!({ "delegation": delegation }),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "Omar did not grant it");

    let (st, b) = op(
        &h,
        "POST",
        "/op/std.delegation.revoke",
        &person(&h, MAYA, false),
        json!({ "delegation": delegation }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b["rows"][0]["outcome"], "revoked");

    let (st, b) = op(&h, "GET", "/op/pendingExpenses", &token, Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    assert_eq!(code(&b), "delegation_required");

    let (_, b) = op(
        &h,
        "GET",
        "/op/std.delegation.mine",
        &person(&h, MAYA, false),
        Value::Null,
    )
    .await;
    assert_eq!(b["rows"][0]["state"], "revoked");
}

/// A declined request leaves no delegation, and the agent is told.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_a_declined_request_grants_nothing() {
    let h = harness("ordo_consent_decline").await;
    let (request, binder) = ask(&h, &["pendingExpenses"], None).await;
    let (st, b) = op(
        &h,
        "POST",
        "/op/std.delegation.decline",
        &person(&h, MAYA, false),
        json!({ "request": request }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    let (st, b) = redeem(&h, &request, &binder).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    assert_eq!(code(&b), "delegation_declined");
    let rows = h
        .delegations
        .granted_by(&ISpace(DEFAULT_SPACE.into()), &PrincipalId(MAYA.into()))
        .await
        .unwrap();
    assert!(rows.is_empty());
    let (st, _) = approve(&h, MAYA, true, &request).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "a declined request stays declined"
    );
}

/// Refusals a setting can fix name the setting and its value.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_settings_refuse_by_name() {
    let h = harness("ordo_consent_settings").await;
    let (st, b) = agent(
        &h,
        "/auth/delegation/request",
        json!({"space": DEFAULT_SPACE, "agent": "agt_stranger", "person": MAYA, "ops": ["pendingExpenses"]}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    assert_eq!(code(&b), "delegation_refused");
    assert!(b["error"]["message"]
        .as_str()
        .unwrap()
        .contains("ORDO_DELEGATION_AGENTS"));

    let (st, b) = agent(
        &h,
        "/auth/delegation/request",
        json!({"space": DEFAULT_SPACE, "agent": ASSISTANT, "person": MAYA, "ops": ["pendingExpenses"], "hours": 24}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    assert!(b["error"]["message"]
        .as_str()
        .unwrap()
        .contains("ORDO_DELEGATION_MAX_HOURS=8"));

    let many: Vec<String> = (0..11).map(|i| format!("op{i}")).collect();
    let (st, b) = agent(
        &h,
        "/auth/delegation/request",
        json!({"space": DEFAULT_SPACE, "agent": ASSISTANT, "person": MAYA, "ops": many}),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    let m = b["error"]["message"].as_str().unwrap();
    assert!(m.contains("ORDO_DELEGATION_MAX_OPS=10"), "{m}");
    assert!(m.contains("set ORDO_DELEGATION_MAX_OPS=11"), "{m}");
}

/// A refusal names the value to set, not only the one set (audit
/// 2026-10-11, finding 21).
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_refusals_name_the_value_to_set() {
    let h = harness("ordo_consent_values").await;
    let (_, b) = agent(
        &h,
        "/auth/delegation/request",
        json!({"space": DEFAULT_SPACE, "agent": ASSISTANT, "person": MAYA, "ops": ["pendingExpenses"], "hours": 24}),
    )
    .await;
    let m = b["error"]["message"].as_str().unwrap();
    assert!(m.contains("set ORDO_DELEGATION_MAX_HOURS=24"), "{m}");
    let (_, b) = agent(
        &h,
        "/auth/delegation/request",
        json!({"space": DEFAULT_SPACE, "agent": "agt_stranger", "person": MAYA, "ops": ["pendingExpenses"]}),
    )
    .await;
    let m = b["error"]["message"].as_str().unwrap();
    assert!(
        m.contains("`agt_stranger=<the name people see>`"),
        "the id=name form: {m}"
    );
}

/// The request route takes no token, so what one caller can leave waiting
/// is capped per agent and per person, and each refusal names its setting
/// (audit 2026-10-11, finding 28).
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_waiting_requests_are_capped_per_person_and_per_agent() {
    let h = harness_with(
        "ordo_consent_caps",
        Settings {
            open_per_person: 2,
            open_per_agent: 3,
            ..settings()
        },
        None,
    )
    .await;
    let body = |person: &str| json!({"space": DEFAULT_SPACE, "agent": ASSISTANT, "person": person, "ops": ["pendingExpenses"]});
    for _ in 0..2 {
        let (st, b) = agent(&h, "/auth/delegation/request", body(MAYA)).await;
        assert_eq!(st, StatusCode::CREATED, "{b}");
    }
    let (st, b) = agent(&h, "/auth/delegation/request", body(MAYA)).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b}");
    assert_eq!(code(&b), "delegation_limit");
    let m = b["error"]["message"].as_str().unwrap();
    assert!(m.contains("ORDO_DELEGATION_OPEN_PER_PERSON=3"), "{m}");

    // A person who does not exist is counted the same way: the cap says
    // nothing about who is a member.
    let (st, b) = agent(&h, "/auth/delegation/request", body("mem_nobody")).await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    let (st, b) = agent(&h, "/auth/delegation/request", body(OMAR)).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b}");
    let m = b["error"]["message"].as_str().unwrap();
    assert!(m.contains("ORDO_DELEGATION_OPEN_PER_AGENT=4"), "{m}");

    // A lapsed request no longer counts.
    h.advance(Settings::default().request_secs + 1);
    let (st, b) = agent(&h, "/auth/delegation/request", body(MAYA)).await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
}

/// With `ORDO_AUTH_HUMAN_CHECK` on, a request without a `human` token is
/// refused before any row is written (audit 2026-10-11, finding 28).
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_the_human_check_guards_the_request_route() {
    let check = ordo_host::auth::human::HumanCheck::new(
        "s".to_string(),
        "https://challenges.cloudflare.com/turnstile/v0/siteverify",
        ordo_egress::TlsRoots::webpki(),
        ordo_egress::DenyList::categories_only(),
    )
    .unwrap();
    let h = harness_with("ordo_consent_human", settings(), Some(check)).await;
    let (st, b) = agent(
        &h,
        "/auth/delegation/request",
        json!({"space": DEFAULT_SPACE, "agent": ASSISTANT, "person": MAYA, "ops": ["pendingExpenses"]}),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{b}");
    assert_eq!(code(&b), "authentication_failed");
    let rows =
        h.pg.query(r#"SELECT count(*) FROM "$delegation_request""#)
            .expect("count");
    assert_eq!(rows[0][0].as_deref(), Some("0"), "no row was written");
}

/// Every step is a security event: request, approval, refusal, decline,
/// redeem and revoke.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_every_step_is_recorded() {
    let h = harness("ordo_consent_events").await;
    let (_, r) = granted(&h, &["pendingExpenses"]).await;
    let (declined, _) = ask(&h, &["pendingExpenses"], None).await;
    let _ = op(
        &h,
        "POST",
        "/op/std.delegation.decline",
        &person(&h, MAYA, false),
        json!({ "request": declined }),
    )
    .await;
    let _ = approve(&h, OMAR, true, &declined).await;
    let _ = op(
        &h,
        "POST",
        "/op/std.delegation.revoke",
        &person(&h, MAYA, false),
        json!({ "delegation": r["delegation"] }),
    )
    .await;
    let events = h.events().join("\n");
    for act in [
        "delegation-requested",
        "delegation-approved",
        "delegation-redeemed",
        "delegation-declined",
        "delegation-refused",
        "delegation-revoked",
    ] {
        assert!(events.contains(act), "{act} missing from:\n{events}");
    }
}

/// A second approval for the same agent replaces the first: one live
/// delegation per person and agent.
#[tokio::test(flavor = "multi_thread")]
async fn ordo_consent_a_new_approval_replaces_the_old_one() {
    let h = harness("ordo_consent_replace").await;
    let (first, _) = granted(&h, &["pendingExpenses"]).await;
    let (second, r) = granted(&h, &["pendingExpenses"]).await;
    let (st, _) = op(&h, "GET", "/op/pendingExpenses", &first, Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "the first is revoked");
    let (st, _) = op(&h, "GET", "/op/pendingExpenses", &second, Value::Null).await;
    assert_eq!(st, StatusCode::OK);
    let live = h
        .delegations
        .live_in(&ISpace(DEFAULT_SPACE.into()), IStamp(h.now()))
        .await
        .unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id.0, r["delegation"].as_str().unwrap());
}
