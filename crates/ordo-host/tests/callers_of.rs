//! 8r — `std.space.callersOf` through the real door, against real Postgres.
//!
//! `ordo-identity`'s unit tests prove the walk over the access model.
//! `tests/grants.rs` proves the registry. This file proves the two things
//! neither can: that the **platform-op path** exists — a spec no program
//! compiled, dispatched, access-checked and bound by the one pipeline every
//! other op uses — and that the op's **refusal rule** holds in both
//! directions.
//!
//! # Why the refusals get more tests than the answer
//!
//! It returns principals. Two enumeration oracles have already been found in
//! this codebase and both were high-call-count read patterns, so the risk
//! here is not that the answer is wrong — it is that the answer is available
//! to someone it should not be, or that a refusal *tells* a prober which
//! wall they hit. A test that only checks what a manager sees would pass for
//! an op every viewer could also call.
//!
//! The rule, argued in `ordo_host::platform_ops`'s module doc:
//!
//! - **manager and owner**, listed explicitly (roles have no hierarchy);
//! - **step-up**, which is also what makes it structurally undelegable —
//!   §5 pins that capability tokens can never satisfy step-up;
//! - the **argument** is scoped to the asker's own catalog, and an op they
//!   may not call answers exactly as one that does not exist.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps, CALLERS_OF};
use ordo_host::{FixtureExecutor, DEFAULT_SPACE};
use ordo_identity::grants::{GrantRegistry, NewGrant};
use ordo_identity::principal::{
    ClientId, GrantId, PrincipalId, SpaceId as ISpace, Timestamp as IStamp,
};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

/// The door's clock. Fixed, so step-up freshness is a property of the token
/// under test rather than of how long the suite took to run.
const NOW: u64 = 1_000_000;

struct Harness {
    state: ApiState,
    /// The registry the token column reads, so a test can plant and revoke
    /// grants and watch the op's answer follow.
    grants: Arc<PgGrantRegistry>,
    engine: Arc<AccessEngine>,
    deployment: Arc<ordo_host::deploy::Deployment>,
}

/// A door with the platform ops installed.
///
/// `tweak` runs on the access engine **before** it is shared, which is the
/// only place a suspension can be applied: the engine is immutable once the
/// door and the platform ops hold the same `Arc`, and that immutability is
/// the property that keeps the audit surface and the door answering about
/// one deployment.
async fn harness(db: &str, tweak: impl FnOnce(&mut AccessEngine)) -> Harness {
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
    grants.ensure_table().await.expect("platform table");
    // These databases outlive a run, and a previous run's live grant is
    // indistinguishable from this one's.
    let _ = pool.rows("DELETE FROM \"$capabilitygrant\"").await;

    let mut engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    tweak(&mut engine);
    let engine = Arc::new(engine);

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "callers-of-test",
        f.clone(),
        ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("the fixture compiles no `std.space.*` name");

    let ops = Arc::new(PlatformOps::new(
        engine.clone(),
        grants.clone(),
        Arc::new(|| NOW),
    ));

    let access = Arc::new(ordo_host::IdentityAccess::sharing(
        engine.clone(),
        TokenKey::from_secret(b"callers-of-secret"),
        Box::new(|| NOW),
        Arc::new(ordo_host::metrics::Metrics::new()),
    ));

    let state = ApiState {
        registry: deployment.handle(),
        // The platform executor wraps the program's, so a platform op and a
        // compiled op reach the door through one `OpExecutor`.
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access,
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"callers-of-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        grants,
        engine,
        deployment,
    }
}

fn space() -> ISpace {
    ISpace(DEFAULT_SPACE.to_string())
}

/// A human token. `present` is what satisfies the step-up gate; `None` is a
/// perfectly valid session that simply has not touched a passkey recently.
fn token(principal: &str, present: Option<u64>) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(b"callers-of-secret"),
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

/// A capability token for `principal`, whose allowlist **contains** the op
/// under test and which claims fresh presence anyway.
///
/// Both halves are deliberate. A token that did not list the op would be
/// refused for the ordinary reason and would prove nothing about
/// delegability; a token with no presence claim would be refused by the
/// same gate a human is. This one is the strongest capability token that
/// can exist, and it must still be refused.
fn capability_token(engine: &AccessEngine, principal: &str, ops: &[&str]) -> String {
    let set: BTreeSet<String> = ops.iter().map(|s| s.to_string()).collect();
    let grant = engine
        .grant_for(GrantId("gr_cap".into()), &space(), set, IStamp(NOW + 3_600))
        .expect("every op named is deployed");
    ordo_identity::mint(
        &TokenKey::from_secret(b"callers-of-secret"),
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

async fn get(state: &ApiState, path: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("GET")
        .uri(path)
        .header("x-ordo-space", DEFAULT_SPACE);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = router(state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn ask(state: &ApiState, op: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    get(state, &format!("/op/{CALLERS_OF}?op={op}"), bearer).await
}

/// `(principal, via, detail)` triples out of a 200 body.
fn rows(body: &Value) -> Vec<(String, String, String)> {
    body["rows"]
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
        .collect()
}

async fn plant(h: &Harness, id: &str, principal: &str, client: Option<&str>, op: &str) {
    h.grants
        .record(&NewGrant {
            id: GrantId(id.into()),
            space: space(),
            principal: PrincipalId(principal.into()),
            client: client.map(|c| ClientId(c.into())),
            pins: [(
                op.to_string(),
                ordo_identity::OpPin {
                    manifest_hash: ordo_identity::ManifestHash([9; 32]),
                    unguarded: false,
                    entities: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
            granted_at: IStamp(0),
            expires: IStamp(NOW + 86_400),
        })
        .await
        .expect("recorded");
}

// ── what it answers ──────────────────────────────────────────────────────

/// The whole of §6 rule 4 through the door: role, group, override and
/// **token**, the last of which is the column 8r was blocked on and the only
/// one that requires a database.
#[tokio::test(flavor = "multi_thread")]
async fn a_manager_gets_every_via_including_the_token_column() {
    let h = harness("ordo_m_callers_answer", |_| {}).await;
    // A connected app, consented by an editor, over an op every role can
    // already call by role — so the token row is the one thing in the
    // answer that could only have come from the registry.
    plant(&h, "gr_dash", "mem_2", Some("cli_dash"), "openTasks").await;

    let (st, body) = ask(&h.state, "openTasks", Some(&token("mem_1", Some(NOW)))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let found = rows(&body);

    assert!(
        found.contains(&("mem_1".into(), "role".into(), "manager".into())),
        "{found:?}"
    );
    assert!(
        found.contains(&("mem_o".into(), "role".into(), "owner".into())),
        "{found:?}"
    );
    assert!(
        found.contains(&("mem_4".into(), "role".into(), "viewer".into())),
        "reads reach every role in the reference profile: {found:?}"
    );
    assert!(
        found.contains(&(
            "mem_2".into(),
            "token".into(),
            "gr_dash (client cli_dash)".into()
        )),
        "the token column is empty — this is the omission that kept 8r \
         closed: {found:?}"
    );
    // The grant holder is ALSO a member, and both paths are reported. An
    // implementation that stopped at the first via would show only the role
    // and a manager revoking the app would find it still working.
    assert!(
        found.contains(&("mem_2".into(), "role".into(), "editor".into())),
        "{found:?}"
    );

    // A write op narrows, so the answer is about the op rather than about
    // the space: viewers are not listed for a write.
    let (_, body) = ask(&h.state, "renameTask", Some(&token("mem_1", Some(NOW)))).await;
    let writers = rows(&body);
    assert!(writers.iter().any(|(p, _, _)| p == "mem_2"));
    assert!(
        !writers.iter().any(|(p, _, _)| p == "mem_4"),
        "a viewer cannot write and must not be listed as able to: {writers:?}"
    );
}

/// The registry round trip: revoking the grant removes the token caller and
/// leaves every other via exactly where it was.
///
/// Both halves matter. "The row disappeared" would also be satisfied by an
/// op that broke; the answer has to stay correct about everything else.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_grant_removes_its_caller_and_nothing_else() {
    let h = harness("ordo_m_callers_revoke", |_| {}).await;
    plant(&h, "gr_rev", "mem_3", Some("cli_x"), "openTasks").await;
    let t = token("mem_1", Some(NOW));

    let before = rows(&ask(&h.state, "openTasks", Some(&t)).await.1);
    assert!(
        before.iter().any(|(_, via, _)| via == "token"),
        "{before:?}"
    );

    h.grants
        .revoke(&space(), &GrantId("gr_rev".into()), IStamp(NOW))
        .await
        .expect("revoked");

    let after = rows(&ask(&h.state, "openTasks", Some(&t)).await.1);
    assert!(
        !after.iter().any(|(_, via, _)| via == "token"),
        "a revoked grant is still listed as a caller: {after:?}"
    );
    let expected: Vec<_> = before
        .into_iter()
        .filter(|(_, v, _)| v != "token")
        .collect();
    assert_eq!(
        after, expected,
        "revocation changed more than the grant it named"
    );
}

// ── what it refuses, and that no refusal is an oracle ────────────────────

/// **Every** way of being refused produces the identical answer.
///
/// Asserted as one equality over six callers rather than six assertions,
/// because the failure this catches is a *difference*: a prober who can tell
/// "not a manager" from "no fresh presence" from "no such op" learns the
/// space's role structure one request at a time, which is precisely the
/// shape of the two oracles already found here.
#[tokio::test(flavor = "multi_thread")]
async fn every_refusal_is_the_same_refusal() {
    let h = harness("ordo_m_callers_refuse", |e| {
        // A suspended OWNER: the highest role in the space, refused on
        // standing rather than on role. Suspension is checked before every
        // grant source, so this is the case that proves the floor is not the
        // only gate. The owner rather than the manager, so `mem_1` stays
        // available as this test's positive control.
        e.suspend(&space(), &PrincipalId("mem_o".into()));
        // A strict deployment (`ORDO_PRESENCE_ON_READS=on`), so the presence
        // refusals below are reached. By default a read asks for none.
        e.require_step_up(CALLERS_OF);
    })
    .await;

    let cap = capability_token(&h.engine, "mem_1", &[CALLERS_OF]);
    let refusals = [
        ("a viewer", Some(token("mem_4", Some(NOW)))),
        ("an editor", Some(token("mem_2", Some(NOW)))),
        (
            "a member with no role at all",
            Some(token("mem_g", Some(NOW))),
        ),
        ("a non-member", Some(token("usr_stranger", Some(NOW)))),
        ("a suspended owner", Some(token("mem_o", Some(NOW)))),
        (
            "a manager with no fresh presence",
            Some(token("mem_1", None)),
        ),
        (
            "a manager whose presence went stale",
            Some(token("mem_1", Some(NOW - 100_000))),
        ),
        // The one that matters most: a capability token for a MANAGER,
        // holding this exact op in its allowlist, claiming fresh presence.
        // The strongest token that can exist, refused because presence is
        // not delegable — Alice may delegate Alice's authority, and the
        // answer contains Bob's consent records.
        ("a capability token that lists the op", Some(cap)),
        ("an unverifiable token", Some("not-a-token".to_string())),
    ];

    // Everything about a door refusal except the op name the caller
    // themselves put in the path. That field is their own input echoed
    // back, so it discloses nothing — and it is asserted separately below
    // rather than waved past.
    let shape = |(st, body): &(StatusCode, Value)| {
        (
            *st,
            body["error"]["code"].clone(),
            body["error"]["message"].clone(),
        )
    };

    // The baseline every refusal must be indistinguishable from: a name
    // nobody registered, in the same reserved namespace, asked by a caller
    // who is otherwise entitled. `api-engine-design.md` §4.1 — an op you
    // may not call and an op that does not exist are one answer.
    let baseline = get(
        &h.state,
        "/op/std.space.noSuchPlatformOp?op=openTasks",
        Some(&token("mem_1", Some(NOW))),
    )
    .await;
    assert_eq!(baseline.0, StatusCode::FORBIDDEN, "{:?}", baseline.1);
    assert_eq!(baseline.1["error"]["code"], "access_denied");

    for (who, bearer) in refusals {
        let got = ask(&h.state, "openTasks", bearer.as_deref()).await;
        assert_eq!(
            shape(&got),
            shape(&baseline),
            "{who} got an answer distinguishable from an unregistered op"
        );
        // The only field that differs is the echo, and it echoes the path
        // rather than anything the door learned.
        assert_eq!(got.1["error"]["op"], CALLERS_OF);
    }

    // A caller who sent no credential at all is told so, in its own words —
    // and that is not an oracle, because it is a fact about their *request*
    // rather than about the op. Asserted rather than assumed: it must be
    // the same words for a name nobody registered.
    let anon = ask(&h.state, "openTasks", None).await;
    let anon_baseline = get(&h.state, "/op/std.space.noSuchPlatformOp?op=x", None).await;
    assert_eq!(shape(&anon), shape(&anon_baseline));
    assert_eq!(anon.0, StatusCode::FORBIDDEN);

    // And the positive control, in the same test: with everything else held
    // constant a manager with presence is served. Without this the file
    // would pass if the op simply never worked.
    let (st, body) = ask(&h.state, "openTasks", Some(&token("mem_1", Some(NOW)))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(!rows(&body).is_empty());
}

/// The **argument** enumerates nothing either.
///
/// A manager may call `callersOf`; that does not make every op name a
/// legitimate question. An op outside the asker's own scoped catalog and an
/// op that does not exist answer identically — the door's non-enumerability
/// rule applied to a parameter — and the class is `not_found`, whose
/// documented meaning is already exactly this: *"does not exist, or exists
/// outside the caller's read view, which answers the same way"*.
#[tokio::test(flavor = "multi_thread")]
async fn an_op_outside_the_askers_catalog_answers_as_one_that_does_not_exist() {
    let h = harness("ordo_m_callers_scope", |_| {}).await;
    let manager = token("mem_1", Some(NOW));

    // `platform.metrics` exists and is owner-only.
    let hidden = ask(&h.state, "platform.metrics", Some(&manager)).await;
    let absent = ask(&h.state, "noSuchOpAnywhereAtAll", Some(&manager)).await;
    assert_eq!(hidden.0, StatusCode::NOT_FOUND, "{:?}", hidden.1);
    assert_eq!(
        hidden, absent,
        "a manager can tell an owner-only op from a name nobody registered, \
         which enumerates the deployment's surface one guess at a time"
    );
    assert_eq!(hidden.1["error"]["code"], "not_found");

    // It is refused for the ASKER's sake, not the op's: the owner, who may
    // call `platform.metrics`, gets the answer.
    let (st, body) = ask(
        &h.state,
        "platform.metrics",
        Some(&token("mem_o", Some(NOW))),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        rows(&body),
        vec![("mem_o".to_string(), "role".to_string(), "owner".to_string())],
        "owner-only means one caller, and the surface should say so"
    );

    // And an empty answer is never the refusal: an in-catalog op always has
    // at least the asker in it, so "no rows" cannot be confused with "not
    // for you".
    let (_, body) = ask(&h.state, CALLERS_OF, Some(&manager)).await;
    assert!(
        rows(&body).iter().any(|(p, _, _)| p == "mem_1"),
        "the op does not describe itself: {body}"
    );
}

// ── the mechanism itself ─────────────────────────────────────────────────

/// A program that compiles an op the platform serves is **refused at load**,
/// not shadowed.
///
/// The shadow is the dangerous outcome: a domain able to register a spec the
/// door dispatches under `std.space.callersOf` would answer *"who can call
/// this"* with whatever it liked — the audit surface answering to the thing
/// it audits.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_may_not_register_a_platform_op_name() {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named("ordo_m_callers_collide");
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: "ordo_m_callers_collide".into(),
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");

    // The collision, from the platform's side — a platform spec claiming a
    // name the fixture already compiled. It is the same check either way
    // round, and this is the one a test can set up without inventing a
    // program that spells a reserved name.
    let mut colliding = platform_ops::specs();
    colliding[0].name = "openTasks".to_string();

    let refused = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "collide",
        f.clone(),
        ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        colliding,
    );
    let Err(refusal) = refused else {
        panic!("a collision must refuse the load, not pick a winner");
    };
    let text = refusal.to_string();
    assert!(text.contains("openTasks"), "{text}");
    assert!(text.contains("platform op"), "{text}");
}

/// The platform op is in the catalog it belongs in and out of the ones it
/// does not — the same scoping every compiled op gets, because it went
/// through the same registry rather than around it.
#[tokio::test(flavor = "multi_thread")]
async fn the_platform_op_is_catalogued_and_scoped_like_any_other() {
    let h = harness("ordo_m_callers_catalog", |_| {}).await;

    // A compiled manifest and a platform one must carry the same keys, or a
    // catalog consumer meets a manifest missing a field it reads. Asserted
    // against a real one rather than a written-down list, so the two cannot
    // drift.
    let f = Fixture::load().expect("fixture");
    let compiled = ordo_host::manifest(&f, "openTasks").expect("a compiled manifest");
    let platform = platform_ops::manifest(&platform_ops::specs()[0]);
    let keys = |v: &Value| -> BTreeSet<String> {
        v.as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    let missing: Vec<String> = keys(&compiled)
        .difference(&keys(&platform))
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "a platform manifest is missing keys a compiled one has: {missing:?}"
    );
    assert_eq!(platform["platform"], Value::Bool(true));
    assert_eq!(
        platform["unguarded"],
        Value::Bool(false),
        "there is no declared guard here to bypass, and marking a bypass \
         that did not happen would put a false warning on a consent page"
    );

    // The op is in the deployment's published manifests, which is what the
    // catalog route renders and filters — so it is discoverable rather than
    // a name a caller has to already know.
    assert!(
        h.deployment
            .manifests()
            .iter()
            .any(|(n, _)| n == CALLERS_OF),
        "a platform op absent from the catalog is undiscoverable"
    );
    // And the door dispatches on it through the same registry.
    assert!(
        ordo_api::OpRegistry::get(h.deployment.as_ref(), CALLERS_OF).is_some(),
        "the registry the door reads does not know the platform op"
    );

    // Scoped exactly as every compiled op is — the route filters the same
    // manifests through the same checker, so a viewer's catalog omits it
    // while a manager's carries it. Omitting, not marking: a listed-but-
    // forbidden op would tell the asker it exists (5e).
    let visible = |bearer: &str| -> Vec<String> {
        h.deployment
            .manifests()
            .into_iter()
            .filter(|(name, _)| {
                matches!(
                    ordo_api::AccessChecker::check(
                        h.state.access.as_ref(),
                        bearer,
                        DEFAULT_SPACE,
                        name
                    ),
                    ordo_api::AccessDecision::Allow { .. }
                )
            })
            .map(|(name, _)| name)
            .collect()
    };
    assert!(visible(&token("mem_1", Some(NOW)))
        .iter()
        .any(|n| n == CALLERS_OF));
    assert!(
        !visible(&token("mem_4", Some(NOW)))
            .iter()
            .any(|n| n == CALLERS_OF),
        "a viewer's catalog names the audit surface"
    );
    // A read asks for no touch (owner, 2026-10-22: signed in means
    // authenticated), so a manager with no fresh presence sees it too.
    assert!(
        visible(&token("mem_1", None))
            .iter()
            .any(|n| n == CALLERS_OF),
        "a signed-in manager's catalog omits a read that needs no touch"
    );
    // A change still asks, and the catalog shows what you may call now: what
    // a manager with no fresh presence loses is platform changes, and only
    // those.
    let with = visible(&token("mem_1", Some(NOW)));
    let without = visible(&token("mem_1", None));
    let lost: Vec<&String> = with.iter().filter(|n| !without.contains(n)).collect();
    assert!(
        !lost.is_empty(),
        "the catalog shows what you may call NOW, and step-up is part of \
         that: a manager with stale presence is told to re-assert it by \
         the door, not by a catalog entry that lies"
    );
    for name in lost {
        let spec = platform_ops::specs()
            .into_iter()
            .find(|s| &s.name == name)
            .expect("only platform ops ask for presence here");
        assert_ne!(
            spec.kind,
            ordo_api::OpKind::Read,
            "{name} is a read and asked"
        );
    }
}
