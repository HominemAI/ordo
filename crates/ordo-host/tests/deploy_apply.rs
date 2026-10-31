//! 18l — `std.deployment.apply` through the real door.
//!
//! `ordo-cli`'s `schema_apply.rs` proves the apply itself: what a first
//! apply creates, what a class-A change sends, what the triage refuses by
//! name. None of that is re-proved here. This file proves the four things
//! the apply cannot know about, because they are the door's:
//!
//! - **who may**: a deployment owner, and nobody else — not a manager of
//!   the deployment's own space, and not the owner of a tenant space;
//! - **what the caller is recorded as**: the principal the door proved,
//!   with no `--as` to assert;
//! - **that two applies serialize** rather than interleave;
//! - **that a refusal over the wire carries the direct path's class**.
//!
//! # Why the refusals get most of the file
//!
//! This op replaces the program every space on the deployment runs. There
//! is no other op in this system where being wrong about *who may* is this
//! expensive — a test that only proved the owner can apply would pass for a
//! build where every tenant owner could too, which is exactly the build
//! check 3 of the gate exists to prevent.
//!
//! # The two databases, and why
//!
//! The access engine's directory and the apply's target are different
//! databases here, and deliberately: an apply's target starts **empty**, so
//! it holds no `members` table for a directory to be read from. Splitting
//! them is what lets the first-apply case be tested through a door that
//! still knows who anybody is. `ApplyOps` takes its own pool, so this is
//! the shape rather than a workaround.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::apply_op::{self, APPLY};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"deploy-apply-secret";

/// The deployment's own space: the one the binary was configured with, and
/// the only one this op answers in.
const HOME: &str = "ordo_home";
/// A tenant. Its owner is a space member, however much authority that role
/// carries inside it.
const TENANT: &str = "tenant_a";

const DEP_OWNER: &str = "dep_owner";
const DEP_MANAGER: &str = "dep_manager";
const DEP_EDITOR: &str = "dep_editor";
const TENANT_OWNER: &str = "ten_owner";

/// A program small enough to read and complete enough to apply: two
/// entities and one op over them.
const V1: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
}

op listA unbounded =
  a[name, n]
"#;

/// V1 with `name` made required. B5: a change that needs the app
/// developer's say, so `ordo apply` does not make it itself.
const V3: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text
  n:     int
}

op listA unbounded =
  a[name, n]
"#;

/// V1 plus an optional field. Additive, so an apply over V1's database
/// lands; the interesting case here is that it is *not* what the live
/// schema holds when only V1 has been created.
const V2: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  tag:   text?
}

op listA unbounded =
  a[name, n, tag]
"#;

struct Harness {
    state: ApiState,
    engine: Arc<AccessEngine>,
    /// The apply's target: a database of its own, emptied per test.
    target: ordo_exec::Pool,
    target_pg: Pg,
    /// The security record the deploy ops write to (G12).
    stream: Arc<ordo_secevent::Recorder>,
}

/// The directory, hand-built and run through the **real** registration
/// loop.
///
/// `engine_from_members` is what a deployment builds its engine with, so
/// `owner_only`, `require_step_up` and the per-role profile are the
/// production ones rather than a test's imitation of them. Only the rows
/// are synthetic, because the two databases here mean there is no one
/// `members` table to read them out of.
fn engine(f: &Fixture) -> AccessEngine {
    let row = |space: &str, id: &str, role: &str| {
        vec![
            Some(space.to_string()),
            Some(id.to_string()),
            Some(role.to_string()),
        ]
    };
    let rows = vec![
        row(HOME, DEP_OWNER, "owner"),
        row(HOME, DEP_MANAGER, "manager"),
        row(HOME, DEP_EDITOR, "editor"),
        row(TENANT, TENANT_OWNER, "owner"),
    ];
    ordo_host::engine_from_members(f, &rows).expect("engine")
}

async fn harness(db: &str) -> Harness {
    let f = Arc::new(Fixture::load().expect("fixture"));

    // The directory's database: the fixture's, seeded, because the door
    // needs a deployment to dispatch on and `FixtureExecutor` needs
    // somewhere to be pointed at. Nothing in this file calls a compiled op.
    let dir_db = format!("{db}_dir");
    let dir_pg = Pg::named(&dir_db);
    f.reset(&dir_pg).expect("seeded");
    let dir_pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: dir_db.clone(),
        // Two, explicitly. Eleven tests run in parallel and each stands up
        // two pools; a default sized to the machine's parallelism would
        // exhaust the cluster's connection budget before any of them
        // proved anything.
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("directory pool");

    // The apply's target: empty, which is the state a first apply is for.
    let target_pg = Pg::named(db);
    target_pg
        .exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("empty target");
    let target = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        // Three, not two: an apply holds one connection for the lock while
        // it runs its DDL on another, and the serialization test wants a
        // third free so the waiter is waiting on the LOCK rather than on
        // the pool.
        size: 3,
        db: db.to_string(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("target pool");

    let grants = Arc::new(PgGrantRegistry::new(dir_pool.clone()));
    grants.ensure_table().await.expect("platform table");
    let _ = dir_pool.rows("DELETE FROM \"$capabilitygrant\"").await;

    let engine = Arc::new(engine(&f));
    let engine_for_harness = engine.clone();
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "deploy-apply-test",
        f.clone(),
        ordo_exec::Executor {
            pool: dir_pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("the fixture compiles no platform-op name");

    let ops = Arc::new(
        PlatformOps::new(engine.clone(), grants, Arc::new(|| NOW)).with_apply(
            apply_op::ApplyOps::recording(
                target.clone(),
                std::sync::Arc::new(ordo_exec::route::SinglePool::new(target.clone())),
                HOME,
                Some(stream.clone()),
            ),
        ),
    );
    let access = Arc::new(ordo_host::IdentityAccess::sharing(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    ));
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: dir_pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"deploy-apply-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        engine: engine_for_harness,
        target,
        target_pg,
        stream,
    }
}

/// A human's token: present, so step-up is satisfied.
fn token(principal: &str) -> String {
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

/// The strongest capability token that can exist for this op: it names the
/// op and claims fresh presence. It must still be refused — §5 pins that a
/// capability token can never satisfy step-up, and that is the mechanism
/// that makes applying a schema structurally human-only.
fn capability_token(engine: &AccessEngine, principal: &str) -> String {
    let set: BTreeSet<String> = BTreeSet::from([APPLY.to_string()]);
    let grant = engine
        .grant_for(
            GrantId("gr_apply".into()),
            &ISpace(HOME.to_string()),
            set,
            IStamp(NOW + 3_600),
        )
        .expect("the op is deployed");
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: Some(IStamp(NOW)),
            capability: Some(grant),
            delegation: None,
            session: None,
        },
    )
}

async fn call(
    state: &ApiState,
    space: &str,
    bearer: &str,
    program: &str,
    activate: bool,
) -> (StatusCode, Value) {
    let body = json!({
        "$space": space,
        "program": program,
        "activate": activate,
    });
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/op/{APPLY}"))
        .header("x-ordo-space", space)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = router(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The resolver for a test that applies to one database. T7 gave the
/// apply path a fleet walk; this file is about the gate and the
/// serialisation, so its fleet is one.
fn single(pool: &ordo_exec::Pool) -> ordo_exec::route::SinglePool {
    ordo_exec::route::SinglePool::new(pool.clone())
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

// ── who may ──────────────────────────────────────────────────────────────

/// The owner of the deployment's own space applies, and the whole schema
/// lands on the empty target.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_owner_applies_and_the_schema_lands() {
    let h = harness("ordo_dapply_owner").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let row = &body["rows"][0];
    let report = apply_op::Report::from_json(row).expect("a report this client can read");
    // `Created.tables` counts the program's entities **as compiled**,
    // which includes the platform entities `ordo_schema` injects into every
    // program (`members` and its siblings). The two this program declares
    // are asserted below, in the catalog, where the question is unambiguous.
    assert!(
        matches!(report.schema, apply_op::SchemaOutcome::Created { .. }),
        "{:?}",
        report.schema
    );
    assert_eq!(report.stored, apply_op::Stored::Written);
    assert!(matches!(
        report.activation,
        apply_op::Activation::Switched { from: None, .. }
    ));

    let tables: Vec<String> = h
        .target_pg
        .query(
            "SELECT relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND relkind = 'r' AND relname IN ('a', 'b') ORDER BY 1",
        )
        .expect("catalog")
        .into_iter()
        .filter_map(|r| r.first().cloned().flatten())
        .collect();
    assert_eq!(tables, vec!["a".to_string(), "b".to_string()]);
}

/// A **manager** of the deployment's own space is refused. Administering
/// who may do what inside a space is not replacing the program every space
/// runs, and `owner_only` is where that line is drawn.
#[tokio::test(flavor = "multi_thread")]
async fn a_manager_of_the_deployments_own_space_is_refused() {
    let h = harness("ordo_dapply_mgr").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_MANAGER), V1, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    assert_eq!(applied_tables(&h), 0, "nothing was created");
}

/// An **editor** of the deployment's own space, for the same reason one
/// role further down. Listed separately because a blanket role list is the
/// thing that loosens by accident.
#[tokio::test(flavor = "multi_thread")]
async fn an_editor_of_the_deployments_own_space_is_refused() {
    let h = harness("ordo_dapply_ed").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_EDITOR), V1, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    assert_eq!(applied_tables(&h), 0);
}

/// **The case the third check exists for.** A tenant space's *owner* holds
/// `Role::Owner`, satisfies the profile and satisfies step-up — checks 1
/// and 2 both pass — and is still a space member. Without the home check
/// this call would replace the deployment's program.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_owner_cannot_apply_to_the_deployment() {
    let h = harness("ordo_dapply_tenant").await;

    // Bound to their own space, which is the call they can actually make:
    // the op is in their catalog and answers in no space but the
    // deployment's own, so it is not there. `not_found` rather than
    // `access_denied` says the true and more final thing — no membership
    // change inside a tenant will ever make this call succeed.
    let (status, body) = call(&h.state, TENANT, &token(TENANT_OWNER), V1, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(code(&body), "not_found");

    // And bound to the deployment's space, where they are not a member at
    // all: the door's own refusal, before the op is reached.
    let (status, body) = call(&h.state, HOME, &token(TENANT_OWNER), V1, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");

    assert_eq!(applied_tables(&h), 0, "nothing was created either way");
}

/// A capability token naming this very op, with fresh presence, is
/// refused. Applying a schema is structurally human-only and this is the
/// mechanism: a capability token can never satisfy step-up, so no
/// third-party client and no delegated agent can deploy.
#[tokio::test(flavor = "multi_thread")]
async fn a_capability_token_cannot_apply_however_its_allowlist_reads() {
    let h = harness("ordo_dapply_cap").await;
    let cap = capability_token(&h.engine, DEP_OWNER);
    let (status, body) = call(&h.state, HOME, &cap, V1, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    assert_eq!(applied_tables(&h), 0);
}

// ── the attested actor ───────────────────────────────────────────────────

/// `$migrations` records the principal the door proved. There is no `--as`
/// over this door, so there is nothing an operator could have asserted.
#[tokio::test(flavor = "multi_thread")]
async fn the_actor_recorded_is_the_principal_the_door_proved() {
    let h = harness("ordo_dapply_actor").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let rows = h
        .target
        .rows("SELECT \"actor\" FROM \"$migrations\" ORDER BY \"seq\"")
        .await
        .expect("migrations");
    assert_eq!(rows.len(), 1, "one apply, one row");
    assert_eq!(
        rows[0][0].as_deref(),
        Some(DEP_OWNER),
        "the principal the door proved, with no `--as` to assert"
    );

    // And the security stream says which door it came through, which is
    // where `Record::op` lands. An auditor reading `$migrations` alone sees
    // who; reading the stream beside it sees who *and* by what route, which
    // is the difference between an attested apply and one made by whoever
    // held the database password.
    let events = h
        .target
        .rows(
            "SELECT \"subject\", \"op\" FROM \"$secevents\" \
             WHERE \"kind\" = 'schema-migration' ORDER BY \"idx\"",
        )
        .await
        .expect("security events");
    assert_eq!(events.len(), 1, "one apply, one event: {events:?}");
    // `Subject::member` prefixes the principal with its kind on the
    // stream; the bare id is what `$migrations` holds.
    assert_eq!(events[0][0].as_deref(), Some("user:dep_owner"));
    assert_eq!(events[0][1].as_deref(), Some(APPLY));
}

// ── serialization ────────────────────────────────────────────────────────

/// Two applies of the same program against the same empty target, started
/// together.
///
/// **The assertion is `$migrations`, not the return values.** Without the
/// lock both calls read an empty target and both call `create_schema`,
/// which is two correct executions of a sequence that raced at its first
/// read: one creates and the other fails on the catalog, or worse, both
/// write a `create` row for a schema that was created once. With the lock
/// the second waits, re-reads a target that now holds the schema, and
/// reports a match.
#[tokio::test(flavor = "multi_thread")]
async fn two_applies_serialize_rather_than_interleave() {
    let h = harness("ordo_dapply_race").await;
    let program = ordo_host::schema_apply::Program::compile(V1).expect("compile");

    let pools = single(&h.target);
    let (a, b) = tokio::join!(
        apply_op::run_program(&h.target, &pools, &program, vec![], "a", "test", true),
        apply_op::run_program(&h.target, &pools, &program, vec![], "b", "test", true),
    );
    let a = a.unwrap_or_else(|e| panic!("first apply: {e}"));
    let b = b.unwrap_or_else(|e| panic!("second apply: {e}"));

    // Exactly one of them created; the other found the schema already
    // there. Which one won is the scheduler's business and is not asserted.
    let created = [&a, &b]
        .iter()
        .filter(|r| matches!(r.schema, apply_op::SchemaOutcome::Created { .. }))
        .count();
    assert_eq!(created, 1, "one create\na: {a:?}\nb: {b:?}");
    let matched = [&a, &b]
        .iter()
        .filter(|r| matches!(r.schema, apply_op::SchemaOutcome::Matches { .. }))
        .count();
    assert_eq!(matched, 1, "and one match\na: {a:?}\nb: {b:?}");

    let rows = h
        .target
        .rows("SELECT count(*) FROM \"$migrations\" WHERE \"kind\" = 'create'")
        .await
        .expect("migrations");
    assert_eq!(
        rows[0][0].as_deref(),
        Some("1"),
        "the schema was created once, and `$migrations` says so once"
    );
    assert_eq!(applied_tables(&h), 2);
}

/// While one apply holds the lock, a second one that is not willing to wait
/// sees it held.
///
/// The bound is `LOCK_WAIT_MS` in the op; here the guard is held across a
/// second `take`, and the assertion is that the second call returns rather
/// than hanging — which is what a bounded wait buys and an unbounded one
/// would not.
#[tokio::test(flavor = "multi_thread")]
async fn the_lock_is_held_while_an_apply_is_running() {
    let h = harness("ordo_dapply_lock").await;
    let held = apply_op::ApplyLock::take(&h.target)
        .await
        .expect("take")
        .expect("free");
    let second = tokio::time::timeout(
        std::time::Duration::from_millis(apply_op::LOCK_WAIT_MS + 5_000),
        apply_op::ApplyLock::take(&h.target),
    )
    .await
    .expect("the wait is bounded, so this never reaches the timeout")
    .expect("no driver failure");
    assert!(second.is_none(), "the lock was held, and the second saw it");

    held.release().await;
    // And released: the next one takes it.
    let third = apply_op::ApplyLock::take(&h.target).await.expect("take");
    assert!(third.is_some(), "a released lock is a free lock");
}

// ── the refusal classes ──────────────────────────────────────────────────

/// A program the load gate would refuse over this database comes back as
/// `not_activated`, with the blocking changes named by their diff codes —
/// which is what the direct path prints, because both render the same
/// `ApplyRefused`.
#[tokio::test(flavor = "multi_thread")]
async fn a_gate_blocking_program_is_stored_and_refused_by_name() {
    let h = harness("ordo_dapply_behind").await;
    // V1 first, so there is a live schema to be behind.
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // V3 makes a column required, so the gate would refuse an artifact of
    // it, and it is not a safe change `ordo apply` makes itself.
    // `ordo migrate --apply` is what moves it.
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), V3, true).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(code(&body), "not_activated");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("not activated") && message.contains("B5"),
        "the refusal names the blocking change by its diff code: {message}"
    );

    // Stored all the same, which is the half `--no-activate` exists for and
    // the half that distinguishes this class from `apply_refused`.
    let stored = h
        .target
        .rows("SELECT count(*) FROM \"$artifacts\"")
        .await
        .expect("artifacts");
    assert_eq!(stored[0][0].as_deref(), Some("2"));
}

/// The same program with `activate: false` is a **report**, not a refusal:
/// the staged deploy.
#[tokio::test(flavor = "multi_thread")]
async fn staging_a_gate_blocking_program_reports_rather_than_refuses() {
    let h = harness("ordo_dapply_staged").await;
    let (status, _) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), V3, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let report = apply_op::Report::from_json(&body["rows"][0]).expect("report");
    match &report.schema {
        apply_op::SchemaOutcome::Behind { changes } => {
            assert!(changes.iter().any(|(code, _)| code == "B5"), "{changes:?}");
        }
        other => panic!("expected behind, got {other:?}"),
    }
    assert_eq!(report.activation, apply_op::Activation::Staged);
}

/// A program that does not compile is refused as `apply_refused`, before
/// anything is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_does_not_compile_is_refused_and_stores_nothing() {
    let h = harness("ordo_dapply_broken").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), "entity {{{", true).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(code(&body), "apply_refused");
    assert_eq!(applied_tables(&h), 0);
}

/// How many of the program's tables the target holds.
fn applied_tables(h: &Harness) -> usize {
    h.target_pg
        .query(
            "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND relkind = 'r' AND relname IN ('a', 'b')",
        )
        .expect("catalog")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// **An apply waits for an attach rather than walking past it** (18d R-S5),
/// the other half of the exclusion.
///
/// `prepare_target` takes the same lock, so a tenant being attached holds
/// off the walk. Without it the walk could pass a target that a
/// half-finished attach is about to add, and then activate a version that
/// database never received — R-S4's quarantine, produced by the deploy
/// rather than found by it.
///
/// The lock is held directly rather than by running a real attach, because
/// what is under test is the exclusion and a concurrent attach would make
/// this a race about which side won.
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_waits_for_an_attach_in_flight() {
    let h = harness("ordo_dapply_rs5").await;

    // Whoever is attaching.
    let held = ordo_host::apply_op::ApplyLock::take(&h.target)
        .await
        .expect("no error")
        .expect("the lock is free to start with");

    let state = h.state.clone();
    let bearer = token(DEP_OWNER);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = tx.send(());
        call(&state, HOME, &bearer, V1, true).await
    });
    rx.await.expect("the apply started");

    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        !handle.is_finished(),
        "the apply did not wait for a routing change in flight"
    );

    drop(held);

    let (status, body) = tokio::time::timeout(std::time::Duration::from_secs(30), handle)
        .await
        .expect("the apply never resumed after the attach finished")
        .expect("the apply task panicked");
    assert_eq!(
        status,
        StatusCode::OK,
        "the apply failed rather than waiting: {body}"
    );
}

/// **An apply populates the fleet view from each target's own
/// `$migrations`** (18d R-S2).
///
/// The cache's whole justification is that an operator can ask "what is the
/// fleet at" without dialling N databases, some of which are down. If an
/// apply did not write it, the answer would be blank until something else
/// did, and nothing else would.
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_records_each_target_in_the_fleet_view() {
    let h = harness("ordo_dapply_fleet").await;
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let seen = ordo_exec::fleet::all(&h.target).await.expect("fleet view");
    assert!(!seen.is_empty(), "the apply recorded no target at all");

    // This harness serves one database, so `default` is the fleet.
    let row = seen
        .iter()
        .find(|c| c.label == ordo_exec::route::DEFAULT_LABEL)
        .unwrap_or_else(|| panic!("no row for the default target: {seen:?}"));

    // **The version is the target's own, read back from the target.** Not
    // asserted against a literal: the point of R-S2 is that this row is a
    // copy of `$migrations`, so the assertion compares the two.
    let authority = ordo_exec::spacedb::recorded_version(&h.target)
        .await
        .expect("the target's own row");
    match authority {
        ordo_exec::spacedb::TargetVersion::At(v) => assert_eq!(
            row.version, v,
            "the fleet view disagrees with the database it is a cache of"
        ),
        other => panic!("the target should know its version after an apply: {other:?}"),
    }

    assert!(
        row.confirmed_at > 0,
        "a confirmation with no timestamp is a version with no age, which is the \
         one thing a cache may not report: {row:?}"
    );
}

/// **A partial apply still records the targets that moved.**
///
/// A deployment where one target failed is exactly when an operator needs
/// the fleet listing, so the write happens before the activation gate. If it
/// happened after, the surface would be blank for the one failure it exists
/// to explain.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_activation_still_leaves_the_fleet_view_written() {
    let h = harness("ordo_dapply_fleet_partial").await;

    // An apply that lands the schema but is not activated. `activate: false`
    // is the staging path, which walks every target and records, and then
    // does not write `$activations`.
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), V1, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let seen = ordo_exec::fleet::all(&h.target).await.expect("fleet view");
    assert!(
        seen.iter()
            .any(|c| c.label == ordo_exec::route::DEFAULT_LABEL),
        "a walk that reached the target recorded nothing: {seen:?}"
    );
}

/// A safe additive change (V2: an optional field) is made by the apply
/// itself and activated (owner ruling 2026-10-22).
#[tokio::test(flavor = "multi_thread")]
async fn a_safe_additive_program_migrates_itself_through_the_door() {
    let h = harness("ordo_dapply_safe").await;
    let (status, _) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&h.state, HOME, &token(DEP_OWNER), V2, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let report = apply_op::Report::from_json(&body["rows"][0]).expect("report");
    match &report.schema {
        apply_op::SchemaOutcome::Migrated { changes, .. } => {
            assert!(changes.iter().any(|(code, _)| code == "A2"), "{changes:?}");
        }
        other => panic!("expected migrated, got {other:?}"),
    }
    assert!(matches!(
        report.activation,
        apply_op::Activation::Switched { .. }
    ));
}

// ── G1 to G3: versions, activate, plan ───────────────────────────────────

use ordo_host::deploy_ops::{ACTIVATE, PLAN, VERSIONS};

/// A token that proves who, with no touch.
fn untouched(principal: &str) -> String {
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

/// Any deploy op: a read by `GET` with its query, a change by `POST`.
async fn deploy_op(
    state: &ApiState,
    name: &str,
    space: &str,
    bearer: &str,
    params: Value,
) -> (StatusCode, Value) {
    let read = name == VERSIONS;
    let mut req = Request::builder()
        .header("x-ordo-space", space)
        .header("authorization", format!("Bearer {bearer}"));
    let body = if read {
        let query: Vec<String> = params
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| format!("{k}={}", v.to_string().trim_matches('"')))
            .collect();
        let uri = if query.is_empty() {
            format!("/op/{name}")
        } else {
            format!("/op/{name}?{}", query.join("&"))
        };
        req = req.method(Method::GET).uri(uri);
        Body::empty()
    } else {
        let mut p = params;
        p["$space"] = json!(space);
        req = req
            .method(Method::POST)
            .uri(format!("/op/{name}"))
            .header("content-type", "application/json");
        Body::from(p.to_string())
    };
    let resp = router(state.clone())
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Apply V1 and then V2, so there are two stored versions and V2 serves.
async fn two_versions(h: &Harness) -> (String, String) {
    let (s1, b1) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(s1, StatusCode::OK, "{b1}");
    let (s2, b2) = call(&h.state, HOME, &token(DEP_OWNER), V2, true).await;
    assert_eq!(s2, StatusCode::OK, "{b2}");
    (
        b1["rows"][0]["version"].as_str().unwrap().to_string(),
        b2["rows"][0]["version"].as_str().unwrap().to_string(),
    )
}

fn deploy_events(h: &Harness) -> Vec<(String, String)> {
    h.stream
        .drain_all(ordo_commandlog::Timestamp(NOW))
        .into_iter()
        .filter_map(|e| match &e.kind {
            ordo_secevent::EventKind::Deploy { act, detail } => Some((act.clone(), detail.clone())),
            _ => None,
        })
        .collect()
}

/// G1. The owner reads every stored version, the serving one, and the
/// activation log, with no touch: it is a read.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_reads_the_versions_and_the_activation_log_without_a_touch() {
    let h = harness("ordo_dops_versions").await;

    // Before any apply: an empty history, not a failure.
    let (status, body) =
        deploy_op(&h.state, VERSIONS, HOME, &untouched(DEP_OWNER), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["active"], Value::Null);
    assert_eq!(body["rows"][0]["stored"], json!([]));

    let (v1, v2) = two_versions(&h).await;
    let (status, body) =
        deploy_op(&h.state, VERSIONS, HOME, &untouched(DEP_OWNER), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["active"], json!(v2));
    let stored: Vec<&str> = row["stored"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["version"].as_str().unwrap())
        .collect();
    assert!(
        stored.contains(&v1.as_str()) && stored.contains(&v2.as_str()),
        "{row}"
    );
    let log = row["activations"].as_array().unwrap();
    assert_eq!(log.len(), 2, "{row}");
    assert_eq!(log[0]["version"], json!(v2), "newest first");
    assert_eq!(log[0]["actor"], json!(DEP_OWNER));

    // `limit` bounds the log.
    let (_, body) = deploy_op(
        &h.state,
        VERSIONS,
        HOME,
        &untouched(DEP_OWNER),
        json!({"limit": 1}),
    )
    .await;
    assert_eq!(body["rows"][0]["activations"].as_array().unwrap().len(), 1);
}

/// G1 is the owner's alone, and only in the deployment's own space.
#[tokio::test(flavor = "multi_thread")]
async fn versions_refuse_a_manager_and_a_tenant_owner() {
    let h = harness("ordo_dops_versions_who").await;
    let (status, body) = deploy_op(&h.state, VERSIONS, HOME, &token(DEP_MANAGER), json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    let (status, body) =
        deploy_op(&h.state, VERSIONS, TENANT, &token(TENANT_OWNER), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(code(&body), "not_found");
}

/// G2. The owner rolls the app back to a stored version: one more
/// activation row, by the principal the door proved, and a `deploy` event
/// on the security record.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_rolls_the_app_back_to_a_stored_version() {
    let h = harness("ordo_dops_rollback").await;
    let (v1, v2) = two_versions(&h).await;
    let _ = deploy_events(&h);

    let (status, body) = deploy_op(
        &h.state,
        ACTIVATE,
        HOME,
        &token(DEP_OWNER),
        json!({"version": v1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["state"], json!("switched"));
    assert_eq!(row["version"], json!(v1));
    assert_eq!(row["from"], json!(v2));
    assert_eq!(
        ordo_exec::registry::active_version(&h.target)
            .await
            .unwrap(),
        Some(v1.clone())
    );
    let log = ordo_exec::registry::activations(&h.target, 1)
        .await
        .unwrap();
    assert_eq!(log[0].actor, DEP_OWNER, "the attested caller is recorded");

    let events = deploy_events(&h);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "activated");
    assert!(
        events[0].1.contains(&format!("version={v1} was={v2}")),
        "{events:?}"
    );

    // Again: already active, nothing appended, nothing recorded.
    let (status, body) = deploy_op(
        &h.state,
        ACTIVATE,
        HOME,
        &token(DEP_OWNER),
        json!({"version": v1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["state"], json!("alreadyActive"));
    assert!(deploy_events(&h).is_empty());
}

/// G2 refusals: no touch, the wrong role, a tenant's owner, a capability
/// token, and a version that is not stored. None of them moves the pointer.
#[tokio::test(flavor = "multi_thread")]
async fn activate_refuses_without_a_touch_the_wrong_role_and_an_unknown_version() {
    let h = harness("ordo_dops_activate_no").await;
    let (v1, v2) = two_versions(&h).await;
    let still = || async {
        assert_eq!(
            ordo_exec::registry::active_version(&h.target)
                .await
                .unwrap(),
            Some(v2.clone()),
            "the pointer did not move"
        );
    };

    let (status, body) = deploy_op(
        &h.state,
        ACTIVATE,
        HOME,
        &untouched(DEP_OWNER),
        json!({"version": v1}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no touch: {body}");
    still().await;

    let (status, body) = deploy_op(
        &h.state,
        ACTIVATE,
        HOME,
        &token(DEP_MANAGER),
        json!({"version": v1}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied");
    still().await;

    let (status, body) = deploy_op(
        &h.state,
        ACTIVATE,
        TENANT,
        &token(TENANT_OWNER),
        json!({"version": v1}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    still().await;

    let (status, body) = deploy_op(
        &h.state,
        ACTIVATE,
        HOME,
        &token(DEP_OWNER),
        json!({"version": "nope0000"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(code(&body), "not_found");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("std.deployment.versions"),
        "the refusal names where the versions are: {body}"
    );
    still().await;
}

/// G2 refuses a version no node could load over this database: V3 makes
/// a field required, staged without its migration.
#[tokio::test(flavor = "multi_thread")]
async fn activate_refuses_a_version_the_database_is_behind() {
    let h = harness("ordo_dops_activate_behind").await;
    let (s, b) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let (s, b) = call(&h.state, HOME, &token(DEP_OWNER), V3, false).await;
    assert_eq!(s, StatusCode::OK, "staged: {b}");
    let v3 = b["rows"][0]["version"].as_str().unwrap().to_string();

    let (status, body) = deploy_op(
        &h.state,
        ACTIVATE,
        HOME,
        &token(DEP_OWNER),
        json!({"version": v3}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(code(&body), "not_activated");
}

/// G3. A preview stores nothing and says what an apply would do: create
/// on an empty database, migrate safe changes itself, hold one that needs
/// the app developer's say.
#[tokio::test(flavor = "multi_thread")]
async fn plan_previews_an_apply_and_stores_nothing() {
    let h = harness("ordo_dops_plan").await;

    let (status, body) = deploy_op(
        &h.state,
        PLAN,
        HOME,
        &token(DEP_OWNER),
        json!({"program": V1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["would"]["state"], json!("create"), "{row}");
    assert_eq!(row["alreadyStored"], json!(false));
    assert_eq!(row["serving"], Value::Null);
    assert_eq!(applied_tables(&h), 0, "a preview creates nothing");
    assert!(
        ordo_exec::registry::active_version(&h.target)
            .await
            .map(|v| v.is_none())
            .unwrap_or(true),
        "a preview activates nothing"
    );

    let (s, b) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let v1 = b["rows"][0]["version"].as_str().unwrap().to_string();

    let (_, body) = deploy_op(
        &h.state,
        PLAN,
        HOME,
        &token(DEP_OWNER),
        json!({"program": V1}),
    )
    .await;
    assert_eq!(
        body["rows"][0]["would"]["state"],
        json!("matches"),
        "{body}"
    );
    assert_eq!(body["rows"][0]["alreadyStored"], json!(true));
    assert_eq!(body["rows"][0]["serving"], json!(v1));

    let (_, body) = deploy_op(
        &h.state,
        PLAN,
        HOME,
        &token(DEP_OWNER),
        json!({"program": V2}),
    )
    .await;
    let row = &body["rows"][0];
    assert_eq!(row["would"]["state"], json!("migrate"), "{row}");
    assert!(!row["changes"].as_array().unwrap().is_empty(), "{row}");

    let (_, body) = deploy_op(
        &h.state,
        PLAN,
        HOME,
        &token(DEP_OWNER),
        json!({"program": V3}),
    )
    .await;
    let row = &body["rows"][0];
    assert_eq!(row["would"]["state"], json!("held"), "{row}");

    let stored = ordo_exec::registry::stored(&h.target).await.unwrap();
    assert_eq!(
        stored.len(),
        1,
        "only the apply stored anything: {stored:?}"
    );
    assert!(deploy_events(&h).iter().all(|(act, _)| act != "planned"));
}

/// G3 refusals: no touch, the wrong role, a tenant's owner, and a program
/// that does not compile (its own class, not a 500).
#[tokio::test(flavor = "multi_thread")]
async fn plan_refuses_without_a_touch_the_wrong_role_and_a_broken_program() {
    let h = harness("ordo_dops_plan_no").await;
    let (status, _) = deploy_op(
        &h.state,
        PLAN,
        HOME,
        &untouched(DEP_OWNER),
        json!({"program": V1}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = deploy_op(
        &h.state,
        PLAN,
        HOME,
        &token(DEP_EDITOR),
        json!({"program": V1}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = deploy_op(
        &h.state,
        PLAN,
        TENANT,
        &token(TENANT_OWNER),
        json!({"program": V1}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = deploy_op(
        &h.state,
        PLAN,
        HOME,
        &token(DEP_OWNER),
        json!({"program": "entity {"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(code(&body), "apply_refused");
}

/// G12. An apply through the door is on the security record too.
#[tokio::test(flavor = "multi_thread")]
async fn an_apply_through_the_door_leaves_a_deploy_event() {
    let h = harness("ordo_dops_apply_event").await;
    let (s, b) = call(&h.state, HOME, &token(DEP_OWNER), V1, true).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let events = deploy_events(&h);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "applied");
    assert!(events[0].1.contains("activation=switched"), "{events:?}");
    assert!(
        events[0].1.contains(&format!("by={DEP_OWNER}")),
        "{events:?}"
    );
}
