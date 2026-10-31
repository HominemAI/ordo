//! Delegation through the real door, against real Postgres.
//!
//! `definition-language.md` §8 [settled] makes delegation *"itself a grant —
//! user → agent, expiring, revocable, command-logged, distinct from the
//! capability token's op allowlist"*, and `bootstrap.md` item 10 requires a
//! **live** one on the agent provisioning path. `ordo-identity`'s unit tests
//! prove the half a bearer string can decide: the delegation's space, its
//! expiry, and the bound that makes the two consents intersect.
//!
//! This file proves the half it cannot. **Revocation is the whole reason
//! this suite exists against a database rather than a fixture**: a token
//! cannot say "withdrawn", so the only honest test of "revocable" is one
//! where a row changes in Postgres and the very next HTTP call is refused.
//!
//! The tests are ordered by that: both directions of revocation first,
//! because the word "revocable" in a settled spec is a promise, and a
//! delegation that could only be waited out would make it false.
//!
//! Databases are prefixed `ordo_del_`. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_host::delegable::DelegablePolicy;
use ordo_host::delegation::PgDelegationRegistry;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps, CALLERS_OF};
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::delegation::{
    DelegationClaim, DelegationRecord, DelegationRegistry, NewDelegation,
};
use ordo_identity::grants::{GrantRegistry, NewGrant};
use ordo_identity::principal::{
    DelegationId, GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp,
};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

/// The door's clock. Fixed, so an expiry under test is a property of the
/// row rather than of how long the suite took to run.
const NOW: u64 = 1_000_000;

/// The agent doing the acting. Not a member of the space — an agent needs
/// no membership, which is exactly why its authority has to come from
/// somewhere, and why "somewhere" being a person's consent is the design.
const AGENT: &str = "agt_ellanor";

/// `mem_1` is the fixture's manager (`seed.sql`), so a delegation from her
/// conveys a manager's reach and no more.
const ADA: &str = "mem_1";

struct Harness {
    state: ApiState,
    delegations: Arc<PgDelegationRegistry>,
    engine: Arc<AccessEngine>,
    /// The deployment surfaces and the catalog, behind the same door.
    platform: ordo_host::serve::PlatformState,
    pg: Pg,
    pool: ordo_exec::Pool,
}

/// A door with the delegation registry wired into it, and the same registry
/// behind `callersOf`'s fifth via.
///
/// `wired` is the one thing a test varies: `false` builds the deployment
/// that records no delegations, which must refuse every token that claims
/// one rather than quietly admitting it.
async fn harness(db: &str, wired: bool) -> Harness {
    harness_behind(db, wired, false).await
}

/// `swappable` puts the door behind `runtime::SwappableAccess`, the wrapper
/// every deployment serves through. The gate's answers must come through
/// it unchanged; a wrapper that left `token_gate` to the trait's default
/// would pass every dead delegation while looking like a door that checks.
async fn harness_behind(db: &str, wired: bool, swappable: bool) -> Harness {
    harness_full(db, wired, swappable, &DelegablePolicy::closed()).await
}

/// A wired door whose app developer opened the deletes `delegable` names
/// (`ORDO_DELEGABLE_OPS`), without installing anything in the process.
async fn harness_opening(db: &str, delegable: &DelegablePolicy) -> Harness {
    harness_full(db, true, false, delegable).await
}

async fn harness_full(
    db: &str,
    wired: bool,
    swappable: bool,
    delegable: &DelegablePolicy,
) -> Harness {
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
    // These databases outlive a run, and a previous run's live delegation is
    // indistinguishable from this one's.
    let _ = pool.rows("DELETE FROM \"$capabilitygrant\"").await;
    let _ = pool.rows("DELETE FROM \"$delegation\"").await;

    let engine =
        Arc::new(ordo_host::deployment_engine_delegating(&f, &pg, delegable).expect("engine"));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "delegation-test",
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

    let mut ops = PlatformOps::new(engine.clone(), grants.clone(), Arc::new(|| NOW));
    let mut access = ordo_host::IdentityAccess::sharing(
        engine.clone(),
        TokenKey::from_secret(b"delegation-secret"),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    );
    if wired {
        // One `Arc` in both, so the audit surface and the door can never
        // come to disagree about which delegations are live.
        ops = ops.with_delegations(delegations.clone());
        access = access.with_delegations(delegations.clone());
    }

    let access: Arc<dyn ordo_api::AccessChecker> = if swappable {
        Arc::new(ordo_host::runtime::SwappableAccess::new(access))
    } else {
        Arc::new(access)
    };
    let platform = ordo_host::serve::PlatformState::new(
        access.clone(),
        deployment.clone(),
        pool.clone(),
        f.keys.clone(),
        DEFAULT_SPACE.to_string(),
        Arc::new(ordo_host::metrics::Metrics::new()),
    );
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
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"delegation-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        delegations,
        engine,
        platform,
        pg,
        pool,
    }
}

fn space() -> ISpace {
    ISpace(DEFAULT_SPACE.to_string())
}

fn new_delegation(id: &str, delegator: &str, agent: &str, expires: u64) -> NewDelegation {
    NewDelegation {
        id: DelegationId(id.into()),
        space: space(),
        delegator: PrincipalId(delegator.into()),
        agent: PrincipalId(agent.into()),
        granted_at: IStamp(NOW),
        expires: IStamp(expires),
        // The consent's own command group. DL says a delegation is
        // command-logged; this is the join that makes the claim checkable
        // rather than aspirational.
        consent_correlation: format!("cg_consent_{id}"),
    }
}

fn claim(id: &str, delegator: &str, expires: u64) -> DelegationClaim {
    DelegationClaim {
        id: DelegationId(id.into()),
        space: space(),
        delegator: PrincipalId(delegator.into()),
        expires: IStamp(expires),
    }
}

/// An agent token: a capability grant listing `ops`, plus — optionally —
/// the delegation it claims to be acting under.
fn agent_token(
    engine: &AccessEngine,
    principal: &str,
    ops: &[&str],
    delegation: Option<DelegationClaim>,
) -> String {
    let set: BTreeSet<String> = ops.iter().map(|s| s.to_string()).collect();
    let grant = engine
        .grant_for(
            GrantId("gr_agent".into()),
            &space(),
            set,
            IStamp(NOW + 3_600),
        )
        .expect("every op named is deployed");
    ordo_identity::mint(
        &TokenKey::from_secret(b"delegation-secret"),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            // Never presence: an agent has no authenticator, and a token
            // claiming one would be refused by the step-up gate anyway.
            user_presence_at: None,
            capability: Some(grant),
            delegation,
            session: None,
        },
    )
}

/// A human token, for the surfaces a human reads.
fn human_token(principal: &str, present: Option<u64>) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(b"delegation-secret"),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: present.map(IStamp),
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

fn code(body: &Value) -> String {
    body["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ───────────── revocation and expiry, in both directions ─────────────

/// The property the word "revocable" in DL §8 is a promise about.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_delegation_is_refused_on_the_very_next_call() {
    let h = harness("ordo_del_revoke", true).await;
    h.delegations
        .record(&new_delegation("dlg_1", ADA, AGENT, NOW + 3_600))
        .await
        .expect("recorded");
    let t = agent_token(
        &h.engine,
        AGENT,
        &["openTasks"],
        Some(claim("dlg_1", ADA, NOW + 3_600)),
    );

    // Live: the agent is Ada, and Ada is a manager who may read.
    let (st, body) = call(&h.state, "openTasks", &t).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // The delegator withdraws. Nothing about the token changes — same
    // bytes, same signature, same unexpired capability grant.
    h.delegations
        .revoke(&space(), &DelegationId("dlg_1".into()), IStamp(NOW))
        .await
        .expect("revoked");

    let (st, body) = call(&h.state, "openTasks", &t).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a withdrawn delegation must stop the NEXT call, not the one after \
         the token expires: {body}"
    );
    assert_eq!(
        code(&body),
        "delegation_required",
        "the refusal has to say what the repair is — a fresh consent from \
         the delegator — because no retry and no wider scope reaches it: {body}"
    );

    // Revocation is idempotent and does not move the timestamp: a second
    // click is not a mistake, and the first stamp is the audit answer to
    // "when did this stop".
    h.delegations
        .revoke(&space(), &DelegationId("dlg_1".into()), IStamp(NOW + 5))
        .await
        .expect("second revoke");
    let all = h
        .delegations
        .granted_by(&space(), &PrincipalId(ADA.into()))
        .await
        .expect("history");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].revoked_at, Some(IStamp(NOW)));
    assert_eq!(
        all[0].consent_correlation, "cg_consent_dlg_1",
        "the record names the command group that recorded the consent"
    );
}

/// The other end of "expiring". Recorded as expired, so the registry — not
/// the token — is what refuses; the token-side expiry is the identity
/// crate's test, and both bounds exist so neither is load-bearing alone.
#[tokio::test(flavor = "multi_thread")]
async fn an_expired_delegation_is_refused_and_a_fresh_one_is_not() {
    let h = harness("ordo_del_expiry", true).await;
    // Expires at exactly `NOW` — the instant itself is already expired, the
    // same boundary convention grants and tokens use.
    h.delegations
        .record(&new_delegation("dlg_old", ADA, AGENT, NOW))
        .await
        .expect("recorded");
    let stale = agent_token(
        &h.engine,
        AGENT,
        &["openTasks"],
        Some(claim("dlg_old", ADA, NOW)),
    );
    let (st, body) = call(&h.state, "openTasks", &stale).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "delegation_required", "{body}");

    // The repair: the delegator consents again. The lapsed row does not
    // block it — a delegation that expired a year ago and was never
    // explicitly withdrawn must not lock the delegator out forever.
    h.delegations
        .record(&new_delegation("dlg_new", ADA, AGENT, NOW + 3_600))
        .await
        .expect("re-consent after expiry is not a conflict");
    let fresh = agent_token(
        &h.engine,
        AGENT,
        &["openTasks"],
        Some(claim("dlg_new", ADA, NOW + 3_600)),
    );
    let (st, body) = call(&h.state, "openTasks", &fresh).await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

/// Re-consent while one is still live is refused, not silently overwritten.
///
/// This is the rule that makes the audit join exact: DL pins the command
/// log's attribution to `agent:<name> on behalf of user:<id>` with no room
/// for a delegation id, so a chain entry has to be joined to the registry on
/// `(space, agent, delegator, at)` — which is ambiguous if two live
/// delegations share the triple.
#[tokio::test(flavor = "multi_thread")]
async fn one_live_delegation_per_pair_and_re_consent_is_revoke_then_record() {
    let h = harness("ordo_del_unique", true).await;
    h.delegations
        .record(&new_delegation("dlg_a", ADA, AGENT, NOW + 3_600))
        .await
        .expect("first");

    let clash = h
        .delegations
        .record(&new_delegation("dlg_b", ADA, AGENT, NOW + 7_200))
        .await;
    assert!(
        matches!(
            clash,
            Err(ordo_identity::delegation::DelegationError::AlreadyLive { ref existing })
                if existing.0 == "dlg_a"
        ),
        "a second live delegation for the same pair must be refused and must \
         name the one in the way: {clash:?}"
    );

    // Recording the SAME consent twice is not a conflict — a retried
    // ceremony must be idempotent, or a dropped response becomes a
    // permanent refusal.
    h.delegations
        .record(&new_delegation("dlg_a", ADA, AGENT, NOW + 3_600))
        .await
        .expect("the same consent, replayed");

    // Revoke, then record: two audited acts rather than one silent
    // overwrite.
    h.delegations
        .revoke(&space(), &DelegationId("dlg_a".into()), IStamp(NOW))
        .await
        .expect("revoked");
    h.delegations
        .record(&new_delegation("dlg_b", ADA, AGENT, NOW + 7_200))
        .await
        .expect("re-consent after revocation");

    let live = h.delegations.live_in(&space(), IStamp(NOW)).await.unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id.0, "dlg_b");
}

// ───────────── the bound: only the delegator's authority ─────────────

/// The escalation DL §8 warns about, executed: *"a manager granting an agent
/// ops never grants it someone else's face"*, and its converse — someone
/// else's face never grants it the manager's ops.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegation_from_a_stranger_conveys_nothing() {
    let h = harness("ordo_del_bound", true).await;
    // A delegation from a principal with no membership in this space. The
    // registry will hold it happily — it records consent, not authority —
    // and the door is what refuses to let it convey anything.
    h.delegations
        .record(&new_delegation("dlg_x", "usr_outsider", AGENT, NOW + 3_600))
        .await
        .expect("recorded");
    let t = agent_token(
        &h.engine,
        AGENT,
        &["openTasks"],
        Some(claim("dlg_x", "usr_outsider", NOW + 3_600)),
    );
    let (st, body) = call(&h.state, "openTasks", &t).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "the delegation is live, so the gate passes; the DOOR is what \
         refuses, because the delegator reaches nothing here: {body}"
    );
    assert_eq!(
        code(&body),
        "access_denied",
        "and it refuses under the door's uniform answer, not the token \
         gate's class — this one depends on the op and the space, so it is \
         exactly the kind of refusal non-enumerability covers: {body}"
    );

    // The same agent, the same allowlist, a delegation from somebody who
    // does reach the op: served.
    h.delegations
        .record(&new_delegation("dlg_ok", ADA, AGENT, NOW + 3_600))
        .await
        .expect("recorded");
    let good = agent_token(
        &h.engine,
        AGENT,
        &["openTasks"],
        Some(claim("dlg_ok", ADA, NOW + 3_600)),
    );
    assert_eq!(call(&h.state, "openTasks", &good).await.0, StatusCode::OK);
}

/// A deployment that records no delegations cannot confirm one, so it
/// admits none — and that failure is confined to the calls that claim one.
#[tokio::test(flavor = "multi_thread")]
async fn an_unwired_deployment_refuses_delegated_tokens_and_serves_the_rest() {
    let h = harness("ordo_del_unwired", false).await;
    let delegated = agent_token(
        &h.engine,
        AGENT,
        &["openTasks"],
        Some(claim("dlg_ghost", ADA, NOW + 3_600)),
    );
    let (st, body) = call(&h.state, "openTasks", &delegated).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        code(&body),
        "delegation_required",
        "a delegation nothing can confirm is one nobody can revoke: {body}"
    );

    // §8: "without a live delegation grant the agent acts only as itself."
    // Acting as itself is an ORDINARY call, and it is untouched — the
    // failure is closed on exactly the calls whose authority cannot be
    // checked and on nothing else.
    let itself = agent_token(&h.engine, AGENT, &["openTasks"], None);
    assert_eq!(call(&h.state, "openTasks", &itself).await.0, StatusCode::OK);
}

/// The same two answers through the swappable door the server serves with.
/// Found in the audit of 2026-10-04: `SwappableAccess` forwarded `check`
/// alone, so in production the token gate was the trait's default and a
/// dead delegation was never refused as one.
#[tokio::test(flavor = "multi_thread")]
async fn the_swappable_door_forwards_the_token_gate() {
    let h = harness_behind("ordo_del_swappable", false, true).await;
    let delegated = agent_token(
        &h.engine,
        AGENT,
        &["openTasks"],
        Some(claim("dlg_ghost", ADA, NOW + 3_600)),
    );
    let (st, body) = call(&h.state, "openTasks", &delegated).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        code(&body),
        "delegation_required",
        "the gate's own class must reach the caller through the wrapper: {body}"
    );
    let itself = agent_token(&h.engine, AGENT, &["openTasks"], None);
    assert_eq!(call(&h.state, "openTasks", &itself).await.0, StatusCode::OK);
}

/// The class must not become an op-existence oracle. A dead-delegation token
/// gets the identical answer for a registered op, an op it could never call,
/// and a name nobody has ever registered.
#[tokio::test(flavor = "multi_thread")]
async fn the_gate_answers_identically_for_every_name_a_token_could_send() {
    let h = harness("ordo_del_oracle", true).await;
    h.delegations
        .record(&new_delegation("dlg_1", ADA, AGENT, NOW + 3_600))
        .await
        .expect("recorded");
    h.delegations
        .revoke(&space(), &DelegationId("dlg_1".into()), IStamp(NOW))
        .await
        .expect("revoked");
    let t = agent_token(
        &h.engine,
        AGENT,
        &["openTasks"],
        Some(claim("dlg_1", ADA, NOW + 3_600)),
    );

    let mut seen = BTreeSet::new();
    for name in ["openTasks", "renameTask", CALLERS_OF, "no.such.op.at.all"] {
        let (st, body) = call(&h.state, name, &t).await;
        seen.insert((
            st.as_u16(),
            code(&body),
            body["error"]["message"].to_string(),
        ));
    }
    assert_eq!(
        seen.len(),
        1,
        "the token gate runs BEFORE the op registry precisely so that these \
         are one answer; if they ever differ, `delegation_required` has \
         become a map of the deployment's op surface: {seen:?}"
    );
}

// ───────────── attribution: both principals, in the chain ─────────────

/// The actor an op write names for these claims, built the way the real
/// write path builds it: `WriteLog::actor_for` over the caller, which under
/// a delegation is the delegator's member id and the token's principal is
/// the agent. The delegation id is deliberately not in the rendering: DL §8
/// pins it, and a chain entry joins to the registry on agent, delegator
/// and instant.
fn actor_of(claims: &ordo_identity::TokenClaims) -> ordo_commandlog::Actor {
    let mut log = WriteLog::new();
    match &claims.delegation {
        Some(d) => {
            log.act_for(Some(claims.principal.0.clone()));
            log.actor_for(&d.delegator.0)
        }
        None => log.actor_for(&claims.principal.0),
    }
}

/// DL §8 pins the attribution string verbatim, and the chain hashes that
/// rendering. So the taxonomy is not extended — a producer is supplied, and
/// what it produces has to survive a real chain round-trip.
#[tokio::test(flavor = "multi_thread")]
async fn the_command_log_names_the_agent_and_the_user_and_the_chain_keeps_both() {
    use ordo_commandlog::{
        Actor, Command, CommandGroup, CommandId, CommandKind, Store, Value as CV,
    };

    let db = "ordo_del_attribution";
    let h = harness(db, true).await;
    h.delegations
        .record(&new_delegation("dlg_1", ADA, AGENT, NOW + 3_600))
        .await
        .expect("recorded");

    // The claims the door proved, read back out of the token exactly as the
    // door reads them — not a hand-built struct, so this cannot pass while
    // the token layer drops the delegation on the floor.
    let token = agent_token(
        &h.engine,
        AGENT,
        &["renameTask"],
        Some(claim("dlg_1", ADA, NOW + 3_600)),
    );
    let claims = ordo_identity::verify(
        &TokenKey::from_secret(b"delegation-secret"),
        &token,
        IStamp(NOW),
    )
    .expect("the door's own verification");

    let actor = actor_of(&claims);
    assert_eq!(
        actor.to_string(),
        format!("agent:{AGENT} on behalf of user:{ADA}"),
        "DL §8's exact form — both principals named, and the delegation id \
         deliberately absent because the spec pins this rendering and the \
         chain hashes it"
    );

    // …and the same claims with no delegation attribute as the caller
    // itself, which is what makes the delegated form mean something.
    let plain = ordo_identity::verify(
        &TokenKey::from_secret(b"delegation-secret"),
        &agent_token(&h.engine, AGENT, &["renameTask"], None),
        IStamp(NOW),
    )
    .unwrap();
    assert_eq!(actor_of(&plain).to_string(), format!("user:{AGENT}"));

    // Into a real chain, and back out.
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    let cspace = ordo_commandlog::SpaceId(DEFAULT_SPACE.to_string());
    let group = CommandGroup {
        correlation: ordo_commandlog::CorrelationId("cg_delegated_1".into()),
        actor: actor.clone(),
        commands: vec![Command {
            id: CommandId("cmd_1".into()),
            space: cspace.clone(),
            entity: "tasks".into(),
            row_id: "task_1".into(),
            kind: CommandKind::Converge,
            before: Some([("title".to_string(), CV::Text("draft spec".into()))].into()),
            after: Some([("title".to_string(), CV::Text("renamed by ellanor".into()))].into()),
            at: ordo_commandlog::Timestamp(NOW),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    };
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group).await.expect("append");
    tx.commit().await.unwrap();

    let store = ordo_exec::commandlog::PgStore::new(pool, tokio::runtime::Handle::current());
    let space_for_read = cspace.clone();
    tokio::task::spawn_blocking(move || {
        let entry = store.get(&space_for_read, 0).expect("index 0");
        assert_eq!(
            entry.group.actor,
            Actor::AgentOnBehalfOf {
                agent: AGENT.into(),
                user: ADA.into(),
            },
            "the delegated attribution survives the encode/decode the chain \
             performs on the RENDERING, not on the enum"
        );
        store.verify(&space_for_read).expect("the chain verifies");
    })
    .await
    .unwrap();

    // The delegation id is not in the actor, so the join has to work: agent
    // + delegator + the group's instant land on exactly one registry row,
    // and it is exactly one because a second live delegation for the pair is
    // unrepresentable.
    let candidates: Vec<DelegationRecord> = h
        .delegations
        .live_in(&space(), IStamp(NOW))
        .await
        .expect("live")
        .into_iter()
        .filter(|d| d.agent.0 == AGENT && d.delegator.0 == ADA)
        .collect();
    assert_eq!(
        candidates.len(),
        1,
        "a chain entry naming only the agent and the user must resolve to \
         one delegation, or 'which consent authorised this write' has no \
         answer: {candidates:?}"
    );
    assert_eq!(candidates[0].id.0, "dlg_1");
}

// ───────────── callersOf: the fifth via, through the door ─────────────

/// An agent reaches the op *via delegation from Ada* — and the surface says
/// so, alongside the token row, because stopping it is two different edits
/// by two different people.
#[tokio::test(flavor = "multi_thread")]
async fn callers_of_names_the_delegation_and_drops_it_when_it_is_withdrawn() {
    let h = harness("ordo_del_callers", true).await;
    // The agent's own capability grant over the op, recorded so the token
    // column can see it — the delegation via is conjunctive with it.
    let grants = PgGrantRegistry::new(
        ordo_exec::Pool::connect(ordo_exec::PoolConfig {
            db: "ordo_del_callers".into(),
            ..ordo_exec::PoolConfig::from_env()
        })
        .await
        .expect("pool"),
    );
    let pins = ordo_host::grant_pins(
        &Fixture::load().unwrap(),
        &BTreeSet::from(["openTasks".to_string()]),
    )
    .expect("deployed");
    grants
        .record(&NewGrant {
            id: GrantId("gr_agent".into()),
            space: space(),
            principal: PrincipalId(AGENT.into()),
            client: None,
            pins,
            granted_at: IStamp(NOW - 10),
            expires: IStamp(NOW + 3_600),
        })
        .await
        .expect("grant recorded");
    h.delegations
        .record(&new_delegation("dlg_1", ADA, AGENT, NOW + 3_600))
        .await
        .expect("recorded");

    // The asker is a manager with fresh presence — `callersOf` is
    // step-up gated and manager-floored, and neither of those moves.
    let asker = human_token(ADA, Some(NOW));
    async fn ask(state: &ApiState, t: &str) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("GET")
            .uri(format!("/op/{CALLERS_OF}?op=openTasks"))
            .header("x-ordo-space", DEFAULT_SPACE)
            .header("authorization", format!("Bearer {t}"))
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

    let (st, body) = ask(&h.state, &asker).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let triples: Vec<(String, String, String)> = body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows in {body}"))
        .iter()
        .map(|r| {
            (
                r["principal"].as_str().unwrap_or_default().to_string(),
                r["via"].as_str().unwrap_or_default().to_string(),
                r["detail"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        triples.contains(&(
            AGENT.into(),
            "delegation".into(),
            format!("on behalf of {ADA} (delegation dlg_1)"),
        )),
        "the delegation is a via of its own, and it names both the person \
         to ask and the id to revoke: {triples:?}"
    );
    assert!(
        triples.iter().any(|(p, k, _)| p == AGENT && k == "token"),
        "…and the token row is still there: revoking one does not stop the \
         other, so an answer with a single row would send a manager to do \
         half the job: {triples:?}"
    );

    h.delegations
        .revoke(&space(), &DelegationId("dlg_1".into()), IStamp(NOW))
        .await
        .expect("revoked");
    let (_, after) = ask(&h.state, &asker).await;
    let vias: Vec<String> = after["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["via"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !vias.iter().any(|v| v == "delegation"),
        "a withdrawn delegation is not a way to call anything, and the audit \
         surface must not go on listing it: {after}"
    );
}

// ───────────── increment 2: `$caller` is the delegator ─────────────
//
// `plan-delegation-surface.md` §7, option 2 (owner ruling). Under a live
// delegation the row guards answer for the delegator, the agent rides
// beside for the command log, and nothing the agent holds widens either.
// Fixture (`seed.sql`): Ana (`mem_1`, manager) is in proj_1 with write and
// proj_3 without; Bram (`mem_2`, editor) is in proj_1 without write and
// proj_2 with; Dov (`mem_4`) is a viewer; Ora (`mem_o`) owns the space.

const BRAM: &str = "mem_2";
const DOV: &str = "mem_4";
const ORA: &str = "mem_o";

/// A request with a JSON body, for the write ops.
async fn post(state: &ApiState, op: &str, bearer: &str, body: Value) -> (StatusCode, Value) {
    send(state, "POST", op, bearer, body).await
}

/// The same, for the delete ops, which take their parameters in the query.
async fn delete(state: &ApiState, op: &str, bearer: &str, params: Value) -> (StatusCode, Value) {
    let query: Vec<String> = params
        .as_object()
        .expect("an object")
        .iter()
        .map(|(k, v)| format!("{k}={}", v.as_str().expect("a string")))
        .collect();
    send(
        state,
        "DELETE",
        &format!("{op}?{}", query.join("&")),
        bearer,
        Value::Null,
    )
    .await
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

/// A GET on the deployment surfaces and the catalog.
async fn surface(
    platform: &ordo_host::serve::PlatformState,
    path: &str,
    bearer: &str,
) -> (StatusCode, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("x-ordo-space", DEFAULT_SPACE)
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap();
    let resp = ordo_host::serve::platform_router(platform.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A live delegation from `delegator` to [`AGENT`], and a token for the
/// agent that lists `ops` and claims it.
async fn delegated(h: &Harness, id: &str, delegator: &str, ops: &[&str]) -> String {
    h.delegations
        .record(&new_delegation(id, delegator, AGENT, NOW + 3_600))
        .await
        .expect("recorded");
    agent_token(
        &h.engine,
        AGENT,
        ops,
        Some(claim(id, delegator, NOW + 3_600)),
    )
}

fn sorted_rows(body: &Value) -> Vec<String> {
    let mut rows: Vec<String> = body["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows in {body}"))
        .iter()
        .map(Value::to_string)
        .collect();
    rows.sort();
    rows
}

/// The row half of "a delegation carries only the delegator's authority".
/// Before increment 2 `$caller` was the agent, which holds no seat, so a
/// delegated read saw nothing; now it sees what the delegator sees, row for
/// row, and a second delegator's view is a different set.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegated_read_sees_exactly_the_delegators_rows_and_not_one_more() {
    let h = harness("ordo_del_rows", true).await;
    let ops = ["myProjects", "taskBoard"];
    let for_ana = delegated(&h, "dlg_ana", ADA, &ops).await;

    for op in ops {
        let (st, own) = call(&h.state, op, &human_token(ADA, None)).await;
        assert_eq!(st, StatusCode::OK, "{own}");
        let (st, agent) = call(&h.state, op, &for_ana).await;
        assert_eq!(st, StatusCode::OK, "{agent}");
        assert!(
            !sorted_rows(&own).is_empty(),
            "Ana sees rows in {op}: {own}"
        );
        assert_eq!(
            sorted_rows(&agent),
            sorted_rows(&own),
            "the agent acting for Ana sees exactly Ana's rows of {op}"
        );
    }

    // Bram sees proj_2, which Ana does not: the same agent, delegated by
    // Bram, sees Bram's rows and not Ana's.
    h.delegations
        .revoke(&space(), &DelegationId("dlg_ana".into()), IStamp(NOW))
        .await
        .expect("revoked");
    let for_bram = delegated(&h, "dlg_bram", BRAM, &ops).await;
    let (_, bram_own) = call(&h.state, "myProjects", &human_token(BRAM, None)).await;
    let (st, bram_agent) = call(&h.state, "myProjects", &for_bram).await;
    assert_eq!(st, StatusCode::OK, "{bram_agent}");
    assert_eq!(sorted_rows(&bram_agent), sorted_rows(&bram_own));
    let (_, ana_own) = call(&h.state, "myProjects", &human_token(ADA, None)).await;
    assert_ne!(
        sorted_rows(&bram_agent),
        sorted_rows(&ana_own),
        "the fixture gives the two different views, so the check above can fail"
    );

    // Acting as itself, the agent has no seat and sees no row.
    let itself = agent_token(&h.engine, AGENT, &ops, None);
    let (st, body) = call(&h.state, "myProjects", &itself).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(sorted_rows(&body).is_empty(), "{body}");
}

/// The write guard answers for the delegator: where Ana may write, her
/// agent may; where she may only read, her agent may only read.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegated_write_is_refused_where_the_delegator_cannot_write() {
    let h = harness("ordo_del_write_guard", true).await;
    let t = delegated(&h, "dlg_1", ADA, &["renameTask"]).await;

    // task_1 is in proj_1, where Ana has write.
    let (st, body) = post(
        &h.state,
        "renameTask",
        &t,
        serde_json::json!({ "task": "task_1", "title": "by the agent" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // task_5 is in proj_3, where Ana is a member without write.
    let (st, body) = post(
        &h.state,
        "renameTask",
        &t,
        serde_json::json!({ "task": "task_5", "title": "not allowed" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        code(&body),
        "access_guard",
        "the same answer Ana herself would get: {body}"
    );
    let title =
        h.pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_5'"#)
            .unwrap();
    assert_eq!(
        title[0][0].as_deref(),
        Some("audit books"),
        "nothing written"
    );
}

/// The other consent: the agent's own allowlist. Ana may rename tasks; an
/// agent she delegated to whose token lists only reads may not.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegated_call_outside_the_allowlist_is_refused() {
    let h = harness("ordo_del_allowlist", true).await;
    let t = delegated(&h, "dlg_1", ADA, &["myProjects"]).await;

    assert_eq!(call(&h.state, "myProjects", &t).await.0, StatusCode::OK);
    let (st, body) = post(
        &h.state,
        "renameTask",
        &t,
        serde_json::json!({ "task": "task_1", "title": "outside the list" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied", "{body}");

    // Ana herself may, so the refusal is the allowlist's.
    let (st, body) = post(
        &h.state,
        "renameTask",
        &human_token(ADA, None),
        serde_json::json!({ "task": "task_1", "title": "by Ana" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

/// SH-20: deletes are closed to delegated tokens by default and opened one
/// op at a time by the app developer (`ORDO_DELEGABLE_OPS`). Platform
/// writes stay closed whatever is set.
#[tokio::test(flavor = "multi_thread")]
async fn a_delete_is_closed_to_a_delegated_token_until_the_app_developer_opens_it() {
    let purge = serde_json::json!({ "project": "proj_1" });

    // The default.
    let h = harness("ordo_del_delete_closed", true).await;
    assert!(!h.engine.delegable("purgeDone"));
    assert!(
        h.engine.delegable("renameTask"),
        "ordinary writes stay open"
    );
    assert!(h.engine.delegable("myProjects"));
    for op in platform_ops::specs() {
        if op.kind != ordo_api::OpKind::Read {
            assert!(
                !h.engine.delegable(&op.name),
                "{} is a platform write",
                op.name
            );
        }
    }
    let t = delegated(&h, "dlg_1", ADA, &["purgeDone"]).await;
    let (st, body) = delete(&h.state, "purgeDone", &t, purge.clone()).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        code(&body),
        "access_denied",
        "the door's uniform answer; the record says OpNotDelegable: {body}"
    );
    // Ana herself may purge.
    let (st, body) = delete(
        &h.state,
        "purgeDone",
        &human_token(ADA, None),
        purge.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Opened by name.
    let opened = DelegablePolicy::parse("purgeDone", &[]).unwrap();
    let h = harness_opening("ordo_del_delete_open", &opened).await;
    assert!(h.engine.delegable("purgeDone"));
    assert!(!h.engine.delegable("dropFolder"), "only the op named");
    let t = delegated(&h, "dlg_1", ADA, &["purgeDone"]).await;
    let (st, body) = delete(&h.state, "purgeDone", &t, purge).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // A platform op cannot be named.
    let platform: Vec<String> = platform_ops::specs().into_iter().map(|s| s.name).collect();
    assert!(DelegablePolicy::parse("std.space.leave", &platform).is_err());
}

/// "Revocable" for writes, the act it matters most for.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_delegation_stops_the_next_write() {
    let h = harness("ordo_del_revoke_write", true).await;
    let t = delegated(&h, "dlg_1", ADA, &["renameTask"]).await;
    let first = serde_json::json!({ "task": "task_1", "title": "first" });
    assert_eq!(
        post(&h.state, "renameTask", &t, first).await.0,
        StatusCode::OK
    );

    h.delegations
        .revoke(&space(), &DelegationId("dlg_1".into()), IStamp(NOW))
        .await
        .expect("revoked");
    let (st, body) = post(
        &h.state,
        "renameTask",
        &t,
        serde_json::json!({ "task": "task_1", "title": "second" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "delegation_required", "{body}");
    let title =
        h.pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap();
    assert_eq!(title[0][0].as_deref(), Some("first"));
}

/// DL §8: a delegated write is recorded as `agent:<agent> on behalf of
/// user:<delegator>`, by the real write path and into the real chain.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegated_write_is_recorded_with_the_agent_and_the_delegator() {
    use ordo_commandlog::{Actor, Store, Value as CV};

    let h = harness("ordo_del_recorded", true).await;
    let t = delegated(&h, "dlg_1", ADA, &["renameTask"]).await;
    let (st, body) = post(
        &h.state,
        "renameTask",
        &human_token(ADA, None),
        serde_json::json!({ "task": "task_1", "title": "by Ana" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, body) = post(
        &h.state,
        "renameTask",
        &t,
        serde_json::json!({ "task": "task_1", "title": "by the agent" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    let store =
        ordo_exec::commandlog::PgStore::new(h.pool.clone(), tokio::runtime::Handle::current());
    let cspace = ordo_commandlog::SpaceId(DEFAULT_SPACE.to_string());
    tokio::task::spawn_blocking(move || {
        let all = store.entries(&cspace, 0, store.len(&cspace));
        let actor_of = |title: &str| {
            all.iter()
                .find(|e| {
                    e.group.commands.iter().any(|c| {
                        c.after
                            .as_ref()
                            .and_then(|a| a.get("title"))
                            .is_some_and(|v| *v == CV::Text(title.into()))
                    })
                })
                .unwrap_or_else(|| panic!("no group wrote {title:?}"))
                .group
                .actor
                .clone()
        };
        assert_eq!(actor_of("by Ana"), Actor::User(ADA.into()));
        assert_eq!(
            actor_of("by the agent"),
            Actor::AgentOnBehalfOf {
                agent: AGENT.into(),
                user: ADA.into(),
            },
            "both named, the delegator as the user"
        );
        store.verify(&cspace).expect("the chain verifies");
    })
    .await
    .unwrap();
}

/// A worker credential is an agent token under a delegation. Nothing it
/// holds or sends reaches past the smaller of its allowlist and its
/// delegator's reach.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_credential_cannot_exceed_what_it_was_granted() {
    let h = harness("ordo_del_worker", true).await;
    let t = delegated(&h, "dlg_1", ADA, &["taskBoard", "renameTask"]).await;

    // An op Ana may call that the credential does not list.
    let (st, body) = post(
        &h.state,
        "addTask",
        &t,
        serde_json::json!({ "project": "proj_1", "title": "unlisted" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied", "{body}");

    // It cannot name another caller, or another agent: both are the
    // door's to bind, and a client that sends one is refused by name.
    for (key, refusal) in [("$caller", "caller_supplied"), ("$agent", "unknown_param")] {
        let mut b = serde_json::json!({ "task": "task_1", "title": "spoofed" });
        b[key] = Value::String(BRAM.into());
        let (st, body) = post(&h.state, "renameTask", &t, b).await;
        assert!(st.is_client_error(), "{key}: {body}");
        assert_eq!(code(&body), refusal, "{key}: {body}");
    }
    let title =
        h.pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap();
    assert_ne!(title[0][0].as_deref(), Some("spoofed"));

    // A delegator who holds less conveys less: Dov is a viewer and may not
    // rename, so a credential he delegated may not either, whatever it lists.
    let from_dov = delegated(&h, "dlg_dov", DOV, &["taskBoard", "renameTask"]).await;
    let (st, body) = post(
        &h.state,
        "renameTask",
        &from_dov,
        serde_json::json!({ "task": "task_1", "title": "a viewer's agent" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(code(&body), "access_denied", "{body}");
}

/// The surfaces outside the op door meet the same token gate. Found while
/// building increment 2: `/metrics`, `/security/events`, the other
/// deployment surfaces and the catalog asked `check` alone, which cannot see
/// a revocation, so a withdrawn delegation went on reaching them.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_delegation_reaches_no_deployment_surface_and_no_catalog() {
    use ordo_host::serve::{METRICS_OP, SECEVENTS_OP};

    let h = harness("ordo_del_surfaces", true).await;
    let t = delegated(&h, "dlg_1", ORA, &[METRICS_OP, SECEVENTS_OP, "myProjects"]).await;
    let events = format!("/security/events?space={DEFAULT_SPACE}");

    // Live: the owner's agent reaches what the owner and the list allow.
    let (st, body) = surface(&h.platform, "/metrics", &t).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, body) = surface(&h.platform, &events, &t).await;
    assert_ne!(
        st,
        StatusCode::FORBIDDEN,
        "past the door (this harness has no resolver to read the stream with): {body}"
    );
    let (st, catalog) = surface(&h.platform, "/catalog", &t).await;
    assert_eq!(st, StatusCode::OK);
    assert!(catalog.contains("myProjects"), "{catalog}");
    let (_, interface) = surface(&h.platform, "/interface", &t).await;
    assert!(interface.contains("myProjects"), "{interface}");

    h.delegations
        .revoke(&space(), &DelegationId("dlg_1".into()), IStamp(NOW))
        .await
        .expect("revoked");

    for path in [
        "/metrics",
        "/security/alerts",
        "/health/deep",
        "/usage",
        events.as_str(),
    ] {
        let (st, body) = surface(&h.platform, path, &t).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{path}: {body}");
    }
    let (st, catalog) = surface(&h.platform, "/catalog", &t).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(catalog, "[]", "a dead token lists nothing");
    let (_, interface) = surface(&h.platform, "/interface", &t).await;
    assert!(!interface.contains("myProjects"), "{interface}");
}
