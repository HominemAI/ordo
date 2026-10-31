//! **Recovery R5: the deployment owner restores a person through the door**,
//! and makes an owner of a tenant whose owner is gone. Through the real door,
//! with the streams read afterwards.
//!
//! The claims:
//!
//! - a tenant's sole owner who lost their only passkey is restored by the
//!   deployment owner, and the deployment's stream and the tenant's each say
//!   so, the tenant's without naming the deployment owner;
//! - nobody inside a tenant can do it, a manager and an owner included;
//! - the door's delay holds the ticket back, and a passkey the person still
//!   holds cancels the case in the meantime;
//! - the restored person is told at every sign-in until they acknowledge it
//!   with presence;
//! - owner succession makes an existing member the owner and touches no
//!   credential and no session.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::auth::AuthState;
use ordo_host::credential_ops::ManagedStore;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::identity_ops::{IdentityOps, RecoveryHandle};
use ordo_host::member_ops::{self, MemberOps};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::recovery::{MemoryRecoveryStore, Recovery, RecoveryPolicy};
use ordo_host::recovery_codes::{MemoryCodeStore, RecoveryCodes};
use ordo_host::recovery_hold::{HoldOnRecovery, TenantCodesGate, TenantRecovery};
use ordo_host::runtime::SwappableAccess;
use ordo_host::tickets::{MemoryTicketStore, Tickets};
use ordo_host::FixtureExecutor;
use ordo_identity::directory::{Directory, DirectoryStore, Provisioning};
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::provider::CompleteRequest;
use ordo_identity::session::{SessionClaim, SessionId, SessionRevocations};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    MemoryDirectoryStore, MemorySessionRevocations, PasskeyProvider, ProviderRegistry,
    RelyingParty, TokenKey, TokenSpec,
};
use ordo_secevent::{EventKind, Scope};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const SECRET: [u8; 32] = *b"recovery-ops-secret-32-bytes!!!!";
const OWNER: &str = "mem_o";
const TWIN: &str = "twin_space";
const DAY: u64 = 86_400;

fn real_now() -> u64 {
    ordo_host::runtime::now_secs()
}

struct Harness {
    state: ApiState,
    pool: ordo_exec::Pool,
    home: String,
    members: Arc<MemberOps>,
    recovery: Arc<Recovery>,
    auth: Arc<AuthState>,
    store: Arc<MemoryDirectoryStore>,
    keys: Arc<ordo_identity::MemoryPasskeyStore>,
    revocations: Arc<MemorySessionRevocations>,
    access: Arc<SwappableAccess>,
    /// The clock recovery and the sign-in routes read. It starts two seconds
    /// behind the real time and moves forward, so a day passes without a
    /// sleep. The door
    /// reads the real one, and every token a test mints for it is minted at
    /// the real time.
    clock: Arc<AtomicU64>,
}

impl Harness {
    fn set_now(&self, at: u64) {
        self.clock.store(at, Ordering::SeqCst);
    }
    fn auth_app(&self) -> axum::Router {
        ordo_host::auth::router(Some(self.auth.clone()))
    }
}

fn cfg(db: &str) -> ordo_exec::PoolConfig {
    ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    }
}

async fn harness(db: &str, policy: RecoveryPolicy) -> Harness {
    let f = Arc::new(Fixture::load().expect("fixture"));
    f.reset(&Pg::named(db)).expect("seeded");
    let pool = ordo_exec::Pool::connect(cfg(db)).await.expect("pool");
    ordo_host::credential_ops::ensure_table(&pool)
        .await
        .expect("managed table");
    ordo_host::recovery_hold::ensure_table(&pool)
        .await
        .expect("tenant recovery table");
    for t in ["$managed", "$recoverytenant"] {
        pool.rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("clean");
    }
    let home = ordo_conformance::fixture_space().0;
    // The tenant, with an owner of its own, before the engine is built.
    pool.rows(&format!(
        "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
         \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") VALUES \
         ('{TWIN}', 'mem_twin_seed', 1, 'seed_owner', 'Seed', 'principal', 1, 'owner', \
          'active', CURRENT_DATE)"
    ))
    .await
    .expect("twin seed");

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("grants");
    let delegations = Arc::new(ordo_host::delegation::PgDelegationRegistry::new(
        pool.clone(),
    ));
    delegations.ensure_table().await.expect("delegations");
    ordo_host::consent::Ceremony::ensure_table(&pool)
        .await
        .expect("delegation requests");
    for t in ["$delegation", "$capabilitygrant", "$delegation_request"] {
        pool.rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("clean");
    }
    let revocations = Arc::new(MemorySessionRevocations::new());
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let engine = ordo_host::deployment_engine_pooled(&f, &pool)
        .await
        .expect("engine");
    let door = ordo_host::IdentityAccess::sharing(
        Arc::new(engine),
        TokenKey::from_secret(&SECRET),
        Box::new(real_now),
        metrics.clone(),
    )
    .with_revocations(revocations.clone());
    let access = Arc::new(SwappableAccess::new(door));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "recovery-ops-test",
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
    .expect("compiles");
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

    // Who people are: one directory store, shared by the sign-in routes and
    // by the ops that name a person.
    let store = Arc::new(MemoryDirectoryStore::new());
    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory: Some(store.clone()),
        stream: Some(access.recorder.clone()),
        refresh: Some(refresh),
        now: Arc::new(real_now),
        managed: None,
    });

    // The sign-in routes and recovery, on one clock and one recorder.
    // Two seconds behind the door's clock, and that is the point. A recovery
    // ends every session that started before the second after it ran (a tie
    // goes to the safe side), and this clock stands still while the door's
    // moves. Started at the real time, a session the test minted in the same
    // wall-clock second as the harness was ended by the recovery it had not
    // waited for, so a fast run failed with a 403 and a slow one passed.
    let clock = Arc::new(AtomicU64::new(real_now().saturating_sub(2)));
    let keys = Arc::new(ordo_identity::MemoryPasskeyStore::new());
    let passkey = Arc::new(PasskeyProvider::with_stores(
        RelyingParty::new("example.com", &["https://app.example.com"]).expect("rp"),
        keys.clone(),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
    ));
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("passkey");
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store.clone()),
        TokenKey::from_secret(&SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(access.recorder.clone())
    .with_passkey(passkey);
    let read = clock.clone();
    state.now = Box::new(move || read.load(Ordering::SeqCst));
    let read = clock.clone();
    let tickets = Arc::new(
        Tickets::new(Arc::new(MemoryTicketStore::new()))
            .with_clock(Box::new(move || read.load(Ordering::SeqCst))),
    );
    let read = clock.clone();
    let door_for_tenants = access.clone();
    let door_for_hold = access.clone();
    let recovery = Arc::new(
        Recovery::new(
            Arc::new(MemoryRecoveryStore::new()),
            tickets.clone(),
            policy.clone(),
        )
        .with_clock(Box::new(move || read.load(Ordering::SeqCst)))
        .with_directory(store.clone())
        .with_recorder(access.recorder.clone())
        .with_tenants(Box::new(move |person| {
            door_for_tenants
                .current()
                .engine
                .spaces_of(&PrincipalId(person.to_string()))
                .into_iter()
                .map(|(space, _, _)| space.0)
                .collect()
        }))
        // A case about a tenant's owner reaches its managers.
        .with_roles(ordo_host::recovery::roles_from(access.clone()))
        .with_codes(Arc::new(
            RecoveryCodes::new(Arc::new(MemoryCodeStore::new())).with_clock(Box::new(real_now)),
        ))
        // A tenant may switch recovery codes off for the people it created.
        .with_codes_gate(Arc::new(TenantCodesGate {
            managed: ManagedStore::new(pool.clone()),
            tenants: TenantRecovery::new(pool.clone(), policy.hold_default),
            runtime: tokio::runtime::Handle::current(),
        }))
        // A tenant that holds suspends a recovered person once the recovery
        // completes.
        .with_settlement(Arc::new(HoldOnRecovery {
            members: members.clone(),
            tenants: TenantRecovery::new(pool.clone(), policy.hold_default),
            tenants_of: Box::new(move |person| {
                door_for_hold
                    .current()
                    .engine
                    .spaces_of(&PrincipalId(person.to_string()))
                    .into_iter()
                    .map(|(space, _, _)| space.0)
                    .collect()
            }),
            runtime: tokio::runtime::Handle::current(),
        })),
    );
    let state = Arc::new(state);
    state.wire_enrolment(ordo_host::auth::Enrolment {
        tickets: Some(tickets),
        ..Default::default()
    });
    state.wire_recovery(recovery.clone());
    let handle: RecoveryHandle = Arc::default();
    let _ = handle.set(recovery.clone());

    let live = access.clone();
    let consents = ordo_host::consent::Consents {
        pool: pool.clone(),
        delegations: delegations.clone(),
        grants: grants.clone(),
    };
    let platform = Arc::new(
        PlatformOps::new(access.current().engine.clone(), grants, Arc::new(real_now))
            .with_engine_source(Arc::new(move || live.current().engine.clone()))
            .with_members(members.clone())
            .with_identity(Arc::new(IdentityOps {
                recovery: handle,
                directory: Some(store.clone()),
                members: members.clone(),
                home: home.clone(),
                managed: Some(ManagedStore::new(pool.clone())),
                tenants: Some(TenantRecovery::new(pool.clone(), policy.hold_default)),
                consents: Some(consents),
            })),
    );
    let api = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: Pg::named(db),
                page_size: 50,
            }),
            platform,
        )),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"recovery-ops-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state: api,
        pool,
        home,
        members,
        recovery,
        auth: state,
        store,
        keys,
        revocations,
        access,
        clock,
    }
}

/// A session token for the door, with presence now.
fn session_token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: Some(IStamp(real_now())),
            capability: None,
            delegation: None,
            session: Some(SessionClaim {
                id: SessionId::mint(),
                started_at: IStamp(real_now()),
                second_step: None,
                via: None,
                upstream_presence: None,
            }),
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

async fn command(
    h: &Harness,
    space: &str,
    op: &str,
    token: &str,
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
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn auth_send(
    app: &axum::Router,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method("POST").uri(path);
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let res = app
        .clone()
        .oneshot(
            b.body(match body {
                Some(v) => Body::from(v.to_string()),
                None => Body::empty(),
            })
            .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn assertion_body(req: &CompleteRequest) -> Value {
    let CompleteRequest::PasskeyAssertion {
        challenge,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = req
    else {
        panic!("not an assertion");
    };
    json!({
        "method": "passkey", "kind": "passkey_assertion", "challenge": challenge.0,
        "credential_id": b64url_encode(credential_id),
        "client_data_json": b64url_encode(client_data_json),
        "authenticator_data": b64url_encode(authenticator_data),
        "signature": b64url_encode(signature),
    })
}

fn registration_body(req: &CompleteRequest) -> Value {
    let CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    } = req
    else {
        panic!("not a registration");
    };
    json!({
        "method": "passkey", "kind": "passkey_registration", "challenge": challenge.0,
        "client_data_json": b64url_encode(client_data_json),
        "attestation_object": b64url_encode(attestation_object),
        "display": "Ana",
    })
}

/// A person signs up with `device`: their principal and the sign-up token.
async fn sign_up(h: &Harness, device: &TestAuthenticator) -> (String, String) {
    let app = h.auth_app();
    let (_, options) = auth_send(
        &app,
        "/auth/register",
        Some(json!({"method": "passkey"})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = auth_send(
        &app,
        "/auth/complete",
        Some(registration_body(&device.register(ch))),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let token = done["token"].as_str().expect("token").to_string();
    let claims = ordo_identity::verify(
        &TokenKey::from_secret(&SECRET),
        &token,
        IStamp(h.clock.load(Ordering::SeqCst)),
    )
    .expect("token");
    (claims.principal.0, token)
}

async fn sign_in_answer(h: &Harness, device: &TestAuthenticator) -> (StatusCode, Value) {
    let app = h.auth_app();
    let (_, options) = auth_send(
        &app,
        "/auth/begin",
        Some(json!({"method": "passkey"})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    auth_send(
        &app,
        "/auth/complete",
        Some(assertion_body(&device.assert(ch))),
        None,
    )
    .await
}

/// Make `principal` a member of `space` at `role`, and rebuild the door.
async fn admit(h: &Harness, space: &str, id: &str, principal: &str, role: &str) {
    h.pool
        .rows(&format!(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
             \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") VALUES \
             ('{space}', '{id}', 1, '{principal}', '{principal}', 'principal', 1, '{role}', \
              'active', CURRENT_DATE)"
        ))
        .await
        .expect("member");
    let door = h.members.rebuilt(space).await;
    assert!(
        door.starts_with("rebuilt here"),
        "the door was not rebuilt after admitting {principal} to {space}: {door}"
    );
}

async fn role_of(h: &Harness, space: &str, principal: &str) -> String {
    let rows = h
        .pool
        .rows(&format!(
            "SELECT \"role\" FROM \"members\" WHERE \"$space\" = '{space}' AND \"principal\" = '{principal}'"
        ))
        .await
        .expect("read");
    rows.first()
        .and_then(|r| r.first())
        .and_then(|c| c.clone())
        .unwrap_or_default()
}

fn entries(h: &Harness) -> Vec<ordo_secevent::StreamEntry> {
    h.access
        .recorder
        .drain_all(ordo_commandlog::Timestamp(real_now() + 400 * DAY))
}

fn code_of(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

// ═══════════════════════════ the last-owner flow ═════════════════════════

/// **A sole owner who lost their only passkey is restored by the deployment
/// owner, and both streams say so.** The deployment's stream names the
/// deployment owner. The tenant's names the class of authority and never the
/// person. The restored owner is then told at every sign-in until they
/// acknowledge it, and acknowledging needs presence.
#[tokio::test(flavor = "multi_thread")]
async fn a_sole_owner_is_restored_and_both_streams_say_so() {
    let h = harness(
        "ordo_rec_ops_sole_owner",
        RecoveryPolicy {
            operator_delay_secs: 0,
            ..RecoveryPolicy::default()
        },
    )
    .await;
    let (lost, kept) = (TestAuthenticator::es256(), TestAuthenticator::ed25519());
    let (ana, _) = sign_up(&h, &lost).await;
    // She is the tenant's only owner.
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(&h, TWIN, "mem_ana", &ana, "owner").await;
    let _ = entries(&h);

    // The deployment owner recovers her through the door.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &session_token(OWNER),
        json!({"principal": ana, "evidence": "verified-out-of-band", "reference": "INC-42"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    let ticket = row["ticket"].as_str().expect("no delay, so a ticket");
    assert_eq!(row["state"], "matured");

    // She registers a new passkey with it.
    let app = h.auth_app();
    let (_, options) = auth_send(
        &app,
        "/auth/recover/begin",
        Some(json!({"ticket": ticket})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    let mut reg = registration_body(&kept.register(ch));
    reg["ticket"] = json!(ticket);
    let (status, done) = auth_send(&app, "/auth/recover/complete", Some(reg), None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let case = done["case"].as_str().expect("case").to_string();

    // The deployment's stream names the deployment owner, the evidence class
    // and the reference. The tenant's names the class and not the person.
    let all = entries(&h);
    let acts: Vec<(Scope, ordo_secevent::Subject, String, String)> = all
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Recovery { act, detail } => Some((
                e.scope.clone(),
                e.subject.clone(),
                act.clone(),
                detail.clone(),
            )),
            _ => None,
        })
        .collect();
    let tenant = Scope::Space(ordo_commandlog::SpaceId(TWIN.into()));
    for act in ["opened", "completed"] {
        assert!(
            acts.iter()
                .any(|(s, _, a, _)| *s == Scope::Deployment && a == act),
            "the deployment's stream has `{act}`"
        );
        assert!(
            acts.iter().any(|(s, _, a, _)| *s == tenant && a == act),
            "the tenant's stream has `{act}`"
        );
    }
    let (_, _, _, deployment_opened) = acts
        .iter()
        .find(|(s, _, a, _)| *s == Scope::Deployment && a == "opened")
        .expect("opened");
    assert!(
        deployment_opened.contains(&format!("by={OWNER}")),
        "{deployment_opened}"
    );
    assert!(deployment_opened.contains("evidence=verified-out-of-band"));
    assert!(deployment_opened.contains("ref=INC-42"));
    assert!(
        deployment_opened.contains("checked=no")
            || deployment_opened.contains("authority=deployment-owner"),
        "{deployment_opened}"
    );
    let (_, subject, _, tenant_opened) = acts
        .iter()
        .find(|(s, _, a, _)| *s == tenant && a == "opened")
        .expect("opened in the tenant");
    assert!(
        !tenant_opened.contains(OWNER) && !tenant_opened.contains("mem_o"),
        "the tenant is not told who: {tenant_opened}"
    );
    assert!(
        tenant_opened.contains("by=deployment-owner"),
        "{tenant_opened}"
    );
    assert_eq!(
        subject.render(),
        ordo_secevent::Subject::platform("deployment-owner").render()
    );
    assert!(
        acts.iter()
            .all(|(s, _, _, _)| *s != Scope::Space(ordo_commandlog::SpaceId(h.home.clone()))),
        "the deployment owner's own space is not told about somebody else's recovery"
    );

    // Her first sign-in carries the case, without the deployment owner's name.
    h.set_now(h.clock.load(Ordering::SeqCst) + 120);
    let (status, answer) = sign_in_answer(&h, &kept).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let notices = answer["recoveries"]
        .as_array()
        .expect("the case is carried");
    assert_eq!(notices.len(), 1, "{answer}");
    assert_eq!(notices[0]["case"], case);
    assert_eq!(notices[0]["authority"], "deployment-owner");
    assert!(!answer.to_string().contains(OWNER), "{answer}");
    let token = answer["token"].as_str().expect("token").to_string();

    // Acknowledging needs presence, and her passkey is quarantined.
    let (status, refused) = auth_send(
        &app,
        "/auth/recover/ack",
        Some(json!({"case": case})),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (_, again) = sign_in_answer(&h, &kept).await;
    assert!(again["recoveries"].is_array(), "still carried");

    // After the quarantine her passkey carries presence, and she acknowledges.
    h.set_now(h.clock.load(Ordering::SeqCst) + DAY);
    let (_, answer) = sign_in_answer(&h, &kept).await;
    let token = answer["token"].as_str().expect("token").to_string();
    let (status, _) = auth_send(
        &app,
        "/auth/recover/ack",
        Some(json!({"case": case})),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, after) = sign_in_answer(&h, &kept).await;
    assert!(after.get("recoveries").is_none(), "acknowledged: {after}");
    let acked = entries(&h)
        .iter()
        .any(|e| matches!(&e.kind, EventKind::Recovery { act, .. } if act == "acknowledged"));
    assert!(acked, "the acknowledgement is itself recorded");
}

/// The door's delay holds the ticket back, a passkey the person still holds
/// cancels the case, and a case that matures is collected with the ticket op.
#[tokio::test(flavor = "multi_thread")]
async fn the_door_delay_holds_the_ticket_and_a_held_passkey_cancels() {
    let h = harness("ordo_rec_ops_delay", RecoveryPolicy::default()).await;
    let (device, kept) = (TestAuthenticator::es256(), TestAuthenticator::ed25519());
    let (ana, _) = sign_up(&h, &device).await;
    let (bob, _) = sign_up(
        &h,
        &TestAuthenticator {
            credential_id: b"bob".to_vec(),
            ..TestAuthenticator::es256()
        },
    )
    .await;
    let owner = session_token(OWNER);

    // A case for Ana. Nothing comes back but the time it matures.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &owner,
        json!({"principal": ana, "evidence": "contract-holder"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["state"], "pending");
    assert!(row["ticket"].is_null(), "a day's delay: no ticket yet");
    let case = row["case"].as_str().expect("case").to_string();
    let start = h.clock.load(Ordering::SeqCst);
    assert_eq!(row["maturesAt"], start + DAY);

    // Too early.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &owner,
        json!({"case": case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");

    // Ana still holds her passkey and did not ask: it cancels the case.
    h.set_now(start + 60);
    let app = h.auth_app();
    let (_, options) = auth_send(
        &app,
        "/auth/begin",
        Some(json!({"method": "passkey"})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = auth_send(
        &app,
        "/auth/recover/contest",
        Some(assertion_body(&device.assert(ch))),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    h.set_now(start + DAY);
    let (status, _) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &owner,
        json!({"case": case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "a cancelled case yields nothing");

    // Bob's case matures, is collected, and recovers him.
    let (_, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &owner,
        json!({"principal": bob, "evidence": "host-break-glass"}),
    )
    .await;
    let bob_case = body["rows"][0]["case"].as_str().expect("case").to_string();
    h.set_now(start + 2 * DAY + 1);
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &owner,
        json!({"case": bob_case}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["rows"][0]["ticket"]
        .as_str()
        .expect("ticket")
        .to_string();
    let (_, options) = auth_send(
        &app,
        "/auth/recover/begin",
        Some(json!({"ticket": ticket})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    let mut reg = registration_body(&kept.register(ch));
    reg["ticket"] = json!(ticket);
    let (status, done) = auth_send(&app, "/auth/recover/complete", Some(reg), None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    // The ticket comes once.
    let (status, _) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &owner,
        json!({"case": bob_case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}

/// **A manager cannot recover an owner**, and neither can an owner of a
/// tenant: the ops answer to the deployment owner in the deployment's own
/// space and to nobody inside a tenant. No case is opened.
#[tokio::test(flavor = "multi_thread")]
async fn a_manager_cannot_recover_an_owner() {
    let h = harness("ordo_rec_ops_manager", RecoveryPolicy::default()).await;
    let (ana, _) = sign_up(&h, &TestAuthenticator::es256()).await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(&h, TWIN, "mem_ana", &ana, "owner").await;
    admit(&h, TWIN, "mem_mia", "mia", "manager").await;

    for (who, space) in [
        ("mia", TWIN),
        ("mia", h.home.as_str()),
        (ana.as_str(), TWIN),
        (ana.as_str(), h.home.as_str()),
    ] {
        let (status, body) = command(
            &h,
            space,
            ordo_host::identity_ops::RECOVER,
            &session_token(who),
            json!({"principal": ana, "evidence": "host-break-glass"}),
        )
        .await;
        assert!(
            status != StatusCode::OK,
            "{who} in {space}: {status} {body}"
        );
        let (status, body) = command(
            &h,
            space,
            ordo_host::identity_ops::SUCCEED_OWNER,
            &session_token(who),
            json!({"tenant": TWIN, "principal": "mia", "evidence": "host-break-glass"}),
        )
        .await;
        assert!(
            status != StatusCode::OK,
            "succession by {who} in {space}: {status} {body}"
        );
    }
    assert!(
        h.recovery.live_for(&ana).expect("read").is_none(),
        "no case was opened"
    );
    assert_eq!(role_of(&h, TWIN, "mia").await, "manager");

    // The deployment owner may, and the door says who is asking.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &session_token(OWNER),
        json!({"principal": ana, "evidence": "host-break-glass"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // An evidence class off the list, and a reference that is not opaque, are
    // refused before anything is opened.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &session_token(OWNER),
        json!({"principal": "nobody", "evidence": "he-said-so"}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(!code_of(&body).is_empty());
}

// ═══════════════════════════ owner succession ════════════════════════════

/// **Succession makes an existing member the owner and touches no credential
/// and no session.** The first call opens a case, the second carries it out
/// once the delay has passed, and both streams record it.
#[tokio::test(flavor = "multi_thread")]
async fn succession_touches_no_credential() {
    let h = harness("ordo_rec_ops_succession", RecoveryPolicy::default()).await;
    let (gone, bob_device) = (TestAuthenticator::es256(), TestAuthenticator::ed25519());
    let (departed, departed_token) = sign_up(&h, &gone).await;
    let (bob, _) = sign_up(&h, &bob_device).await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(&h, TWIN, "mem_departed", &departed, "owner").await;
    admit(&h, TWIN, "mem_bob", &bob, "editor").await;
    let _ = entries(&h);
    let owner = session_token(OWNER);

    let links_before = (
        h.store
            .authenticators_of(&PrincipalId(departed.clone()))
            .expect("read"),
        h.store
            .authenticators_of(&PrincipalId(bob.clone()))
            .expect("read"),
        h.keys.len(),
    );

    // A person who is not a member is refused: succession admits nobody.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &owner,
        json!({"tenant": TWIN, "principal": "stranger", "evidence": "contract-holder"}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");

    // The first call opens a case.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &owner,
        json!({"tenant": TWIN, "principal": bob, "evidence": "contract-holder", "reference": "MSA-7",
               "departed": [departed]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let case = body["rows"][0]["case"].as_str().expect("case").to_string();
    assert_eq!(
        role_of(&h, TWIN, &bob).await,
        "editor",
        "nothing has happened yet"
    );

    // Too early.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &owner,
        json!({"tenant": TWIN, "case": case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(role_of(&h, TWIN, &bob).await, "editor");

    // After the delay it is carried out, and once.
    h.set_now(h.clock.load(Ordering::SeqCst) + DAY);
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &owner,
        json!({"tenant": TWIN, "case": case}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["outcome"], "succeeded");
    assert_eq!(body["rows"][0]["from"], "editor");
    assert_eq!(role_of(&h, TWIN, &bob).await, "owner");
    assert_eq!(
        role_of(&h, TWIN, &departed).await,
        "owner",
        "the departed owner's membership is the tenant's to remove"
    );
    let (status, _) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &owner,
        json!({"tenant": TWIN, "case": case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "a succession is carried out once");

    // No credential moved and no session ended.
    let links_after = (
        h.store
            .authenticators_of(&PrincipalId(departed.clone()))
            .expect("read"),
        h.store
            .authenticators_of(&PrincipalId(bob.clone()))
            .expect("read"),
        h.keys.len(),
    );
    assert_eq!(links_before, links_after, "no link and no key changed");
    let claims = ordo_identity::verify(
        &TokenKey::from_secret(&SECRET),
        &departed_token,
        IStamp(real_now()),
    )
    .expect("token");
    let session = claims.session.expect("a session");
    assert!(!h.revocations.is_revoked(&session.id));
    assert!(
        !h.revocations
            .principal_ended(&claims.principal, session.started_at),
        "no session was ended"
    );
    assert!(
        h.recovery.live_for(&bob).expect("read").is_none(),
        "the case is closed"
    );

    // Recorded in both streams, the tenant's without the deployment owner.
    let all = entries(&h);
    let succeeded: Vec<(Scope, String)> = all
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Recovery { act, detail } if act == "owner-succeeded" => {
                Some((e.scope.clone(), detail.clone()))
            }
            _ => None,
        })
        .collect();
    let tenant = Scope::Space(ordo_commandlog::SpaceId(TWIN.into()));
    assert!(succeeded
        .iter()
        .any(|(s, d)| *s == Scope::Deployment && d.contains(&format!("by={OWNER}"))));
    let (_, told) = succeeded
        .iter()
        .find(|(s, _)| *s == tenant)
        .expect("the tenant's stream");
    assert!(!told.contains(OWNER), "{told}");
    assert!(told.contains(&format!("to={bob}")), "{told}");
    assert!(told.contains("evidence=contract-holder"), "{told}");
}

/// A recovery ticket cannot carry out a succession and a succession case
/// cannot be redeemed as a recovery: they share a table and nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_and_a_succession_are_not_interchangeable() {
    let h = harness(
        "ordo_rec_ops_apart",
        RecoveryPolicy {
            operator_delay_secs: 0,
            succession_delay_secs: 0,
            ..RecoveryPolicy::default()
        },
    )
    .await;
    let (bob, _) = sign_up(&h, &TestAuthenticator::es256()).await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(&h, TWIN, "mem_bob", &bob, "editor").await;
    admit(&h, TWIN, "mem_gone", "gone_owner", "owner").await;
    let owner = session_token(OWNER);

    // A recovery case is not a succession.
    let (_, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &owner,
        json!({"principal": bob, "evidence": "host-break-glass"}),
    )
    .await;
    let recovery_case = body["rows"][0]["case"].as_str().expect("case").to_string();
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &owner,
        json!({"tenant": TWIN, "case": recovery_case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(role_of(&h, TWIN, &bob).await, "editor");
    let (status, _) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::CANCEL_RECOVERY,
        &owner,
        json!({"case": recovery_case, "reason": "opened-in-error"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A succession case yields no recovery ticket.
    let (_, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &owner,
        json!({"tenant": TWIN, "principal": bob, "evidence": "host-break-glass",
               "departed": ["gone_owner"]}),
    )
    .await;
    let succession_case = body["rows"][0]["case"].as_str().expect("case").to_string();
    assert!(body["rows"][0].get("ticket").is_none());
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &owner,
        json!({"case": succession_case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
}

// ═══════════════ R6 · a tenant's part: managed members, hold ═════════════

/// A member the tenant created: a person who signed up, marked as managed by
/// `space`, and admitted to it. Returns their principal.
async fn managed_member(
    h: &Harness,
    space: &str,
    member_id: &str,
    by: &str,
    device: &TestAuthenticator,
    role: &str,
) -> String {
    let (principal, _) = sign_up(h, device).await;
    ordo_host::credential_ops::ManagedStore::new(h.pool.clone())
        .mark(&principal, space, by, real_now())
        .await
        .expect("marked");
    admit(h, space, member_id, &principal, role).await;
    principal
}

/// A second device with its own credential id.
fn device(name: &str) -> TestAuthenticator {
    TestAuthenticator {
        credential_id: name.as_bytes().to_vec(),
        ..TestAuthenticator::es256()
    }
}

async fn get_op(h: &Harness, space: &str, op: &str, token: &str) -> StatusCode {
    let (status, body) = send(
        &h.state,
        Request::builder()
            .method("GET")
            .uri(format!("/op/{op}"))
            .header("x-ordo-space", space)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // Shown only when a test fails, to say why a request was refused.
    eprintln!("GET {op} in {space}: {status} {body}");
    status
}

/// **An administrator recovers a member the tenant created.** The tenant's
/// owner gets a ticket at once (the tenant's delay is zero), the member
/// registers a new passkey with it, and the act is in the tenant's stream and
/// the deployment's.
#[tokio::test(flavor = "multi_thread")]
async fn an_administrator_recovers_a_managed_member() {
    let h = harness("ordo_rec_ops_managed", RecoveryPolicy::default()).await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(&h, TWIN, "mem_olga", "olga", "owner").await;
    let lost = device("carl-lost");
    let carl = managed_member(&h, TWIN, "mem_carl", "olga", &lost, "editor").await;
    let _ = entries(&h);

    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::RECOVER_MEMBER,
        &session_token("olga"),
        json!({"member": "mem_carl"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["principal"], carl);
    let ticket = row["ticket"].as_str().expect("a tenant's delay is zero");

    // Carl registers a new passkey with it.
    let kept = device("carl-new");
    let app = h.auth_app();
    let (_, options) = auth_send(
        &app,
        "/auth/recover/begin",
        Some(json!({"ticket": ticket})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    let mut reg = registration_body(&kept.register(ch));
    reg["ticket"] = json!(ticket);
    let (status, done) = auth_send(&app, "/auth/recover/complete", Some(reg), None).await;
    assert_eq!(status, StatusCode::OK, "{done}");

    // Recorded in the tenant's stream and the deployment's, naming the
    // administrator and the class of authority.
    let all = entries(&h);
    let opened: Vec<(Scope, String)> = all
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Recovery { act, detail } if act == "opened" => {
                Some((e.scope.clone(), detail.clone()))
            }
            _ => None,
        })
        .collect();
    let tenant = Scope::Space(ordo_commandlog::SpaceId(TWIN.into()));
    for scope in [Scope::Deployment, tenant] {
        let (_, detail) = opened
            .iter()
            .find(|(s, _)| *s == scope)
            .unwrap_or_else(|| panic!("no `opened` in {scope:?}"));
        assert!(detail.contains("path=tenant"), "{detail}");
        assert!(
            detail.contains("authority=tenant-administrator"),
            "{detail}"
        );
        assert!(detail.contains("by=olga"), "{detail}");
    }
    // The quarantine applies to every path but a passkey: his new one cannot
    // pass step-up until it ends.
    h.set_now(h.clock.load(Ordering::SeqCst) + 60);
    let (_, answer) = sign_in_answer(&h, &kept).await;
    assert!(answer["presence_available_at"].is_number(), "{answer}");
}

/// **The same administrator is refused once the member joins a second
/// tenant**, and the refusal is the one answer whatever the reason: a member
/// who belongs elsewhere, one the tenant did not create, and an owner all get
/// the same words, and the tenant's stream keeps which.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_administrator_is_refused_once_the_member_joins_a_second_tenant() {
    let h = harness("ordo_rec_ops_second_tenant", RecoveryPolicy::default()).await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(&h, TWIN, "mem_olga", "olga", "owner").await;
    admit(&h, TWIN, "mem_ed", "ed", "owner").await;
    // Ed was created by the tenant too, and is an owner: the one thing the
    // tenant may not recover.
    ordo_host::credential_ops::ManagedStore::new(h.pool.clone())
        .mark("ed", TWIN, "mem_olga", real_now())
        .await
        .expect("marked");
    let carl = managed_member(&h, TWIN, "mem_carl", "olga", &device("carl"), "editor").await;
    // Dana signed up on her own and was admitted: the tenant did not create her.
    let (dana, _) = sign_up(&h, &device("dana")).await;
    admit(&h, TWIN, "mem_dana", &dana, "editor").await;
    let olga = session_token("olga");

    // Before Carl joins another tenant, Olga may.
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::RECOVER_MEMBER,
        &olga,
        json!({"member": "mem_carl"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let case = body["rows"][0]["case"].as_str().expect("case").to_string();
    h.recovery
        .cancel(
            &case,
            ordo_secevent::Subject::member("olga"),
            "person-withdrew",
        )
        .expect("cancelled");
    let _ = entries(&h);

    // Carl is admitted to the deployment's own space as well.
    admit(&h, &h.home, "mem_carl_home", &carl, "viewer").await;
    let mut answers = Vec::new();
    for member in ["mem_carl", "mem_dana", "mem_ed"] {
        let (status, body) = command(
            &h,
            TWIN,
            ordo_host::identity_ops::RECOVER_MEMBER,
            &olga,
            json!({"member": member}),
        )
        .await;
        assert_ne!(status, StatusCode::OK, "{member}: {body}");
        answers.push((status, body["error"].clone()));
    }
    for (status, error) in &answers[1..] {
        assert_eq!(*status, answers[0].0, "one status");
        assert_eq!(
            error["message"], answers[0].1["message"],
            "one sentence, whatever the reason"
        );
        assert_eq!(error["code"], answers[0].1["code"]);
    }
    assert_eq!(answers[0].1["code"], "tenant_refused");
    assert!(
        answers[0].1["message"]
            .as_str()
            .expect("message")
            .contains("Refer this person to the deployment owner"),
        "{:?}",
        answers[0]
    );
    for who in [&carl, &dana] {
        assert!(h.recovery.live_for(who).expect("read").is_none());
    }

    // The tenant's stream keeps the class, and only the tenant's.
    let refused: Vec<(Scope, String)> = entries(&h)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Recovery { act, detail } if act == "refused" => Some((e.scope, detail)),
            _ => None,
        })
        .collect();
    let tenant = Scope::Space(ordo_commandlog::SpaceId(TWIN.into()));
    let in_tenant: Vec<&String> = refused
        .iter()
        .filter(|(s, _)| *s == tenant)
        .map(|(_, d)| d)
        .collect();
    assert_eq!(in_tenant.len(), 3, "{refused:?}");
    assert_eq!(
        in_tenant
            .iter()
            .filter(|d| d.contains("reason=not-managed-here"))
            .count(),
        2
    );
    assert_eq!(
        in_tenant
            .iter()
            .filter(|d| d.contains("reason=target-is-owner"))
            .count(),
        1
    );
    assert!(
        refused
            .iter()
            .all(|(s, _)| *s != Scope::Space(ordo_commandlog::SpaceId(h.home.clone()))),
        "the other tenant is not told"
    );
}

/// **A held tenant stays closed to a recovered person until its own owner
/// reactivates them.** A tenant that has not chosen does not hold, and a
/// tenant's only active owner is never held.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_tenant_stays_closed_until_its_own_owner_reactivates() {
    let h = harness(
        "ordo_rec_ops_hold",
        RecoveryPolicy {
            operator_delay_secs: 0,
            ..RecoveryPolicy::default()
        },
    )
    .await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    let olga_device = device("olga");
    let (olga, _) = sign_up(&h, &olga_device).await;
    admit(&h, TWIN, "mem_olga", &olga, "owner").await;
    let frank_lost = device("frank-lost");
    let (frank, _) = sign_up(&h, &frank_lost).await;
    admit(&h, TWIN, "mem_frank", &frank, "editor").await;
    admit(&h, &h.home, "mem_frank_home", &frank, "viewer").await;
    let owner_token = session_token(OWNER);
    let olga_token = session_token(&olga);

    // The tenant turns on hold.
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::SET_RECOVERY_POLICY,
        &olga_token,
        json!({"hold": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        get_op(&h, TWIN, "std.person.tenants", &session_token(&frank)).await,
        StatusCode::OK
    );

    // The deployment owner recovers Frank and Olga, who is the tenant's only
    // active owner.
    for (who, device) in [(&frank, device("frank-new")), (&olga, device("olga-new"))] {
        let (status, body) = command(
            &h,
            &h.home,
            ordo_host::identity_ops::RECOVER,
            &owner_token,
            json!({"principal": who, "evidence": "verified-out-of-band"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let ticket = body["rows"][0]["ticket"]
            .as_str()
            .expect("ticket")
            .to_string();
        let app = h.auth_app();
        let (_, options) = auth_send(
            &app,
            "/auth/recover/begin",
            Some(json!({"ticket": ticket})),
            None,
        )
        .await;
        let ch = options["challenge"].as_str().expect("challenge");
        let mut reg = registration_body(&device.register(ch));
        reg["ticket"] = json!(ticket);
        let (status, done) = auth_send(&app, "/auth/recover/complete", Some(reg), None).await;
        assert_eq!(status, StatusCode::OK, "{done}");
        assert_eq!(done["warnings"], json!([]), "{done}");
    }

    // Frank is held in the tenant that holds, and not in the one that does not.
    let status_of = |space: &str, principal: &str| {
        let (space, principal) = (space.to_string(), principal.to_string());
        let pool = h.pool.clone();
        async move {
            pool.rows(&format!(
                "SELECT \"status\" FROM \"members\" WHERE \"$space\" = '{space}' AND \"principal\" = '{principal}'"
            ))
            .await
            .expect("read")
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.clone())
            .unwrap_or_default()
        }
    };
    assert_eq!(status_of(TWIN, &frank).await, "suspended");
    assert_eq!(
        status_of(&h.home, &frank).await,
        "active",
        "a tenant that did not choose"
    );
    assert_eq!(
        status_of(TWIN, &olga).await,
        "active",
        "the only active owner is never held"
    );
    assert_ne!(
        get_op(&h, TWIN, "std.person.tenants", &session_token(&frank)).await,
        StatusCode::OK,
        "the tenant is closed to him"
    );
    assert_eq!(
        get_op(&h, &h.home, "std.person.tenants", &session_token(&frank)).await,
        StatusCode::OK,
        "the other is not"
    );

    // Only the tenant's own owner reopens it.
    let (status, body) = command(
        &h,
        TWIN,
        "std.space.reinstate",
        &olga_token,
        json!({"member": "mem_frank"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(status_of(TWIN, &frank).await, "active");
    assert_eq!(
        get_op(&h, TWIN, "std.person.tenants", &session_token(&frank)).await,
        StatusCode::OK
    );
}

// ═════════════════════ R7 · settings, floors, approvals ══════════════════

/// **A tenant cannot lower a deployment floor**, and can raise it. A delay or
/// an approvals count below the deployment's is refused, one above is taken,
/// and a case for the tenant's own members then waits the longer of the two.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_cannot_lower_a_deployment_floor() {
    let h = harness(
        "ordo_rec_ops_floor",
        RecoveryPolicy {
            tenant_delay_secs: 3_600,
            tenant_approvals: 2,
            ..RecoveryPolicy::default()
        },
    )
    .await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(&h, TWIN, "mem_olga", "olga", "owner").await;
    admit(&h, TWIN, "mem_pia", "pia", "owner").await;
    let carl = managed_member(&h, TWIN, "mem_carl", "mem_olga", &device("carl"), "editor").await;
    let olga = session_token("olga");
    let set = |body: Value| {
        command(
            &h,
            TWIN,
            ordo_host::identity_ops::SET_RECOVERY_POLICY,
            &olga,
            body,
        )
    };

    for below in [json!({"delaySecs": 60}), json!({"approvals": 1})] {
        let (status, body) = set(below.clone()).await;
        assert_ne!(status, StatusCode::OK, "{below}: {body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("floor"),
            "{body}"
        );
    }
    // Nothing named is refused too.
    let (status, _) = set(json!({})).await;
    assert_ne!(status, StatusCode::OK);
    // Raising is taken, and is what the tenant's own members then get.
    let (status, body) = set(json!({"delaySecs": 7_200})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["delaySecs"], 7_200);
    assert_eq!(
        body["rows"][0]["approvals"], 1,
        "the tenant's own choice; the deployment's floor applies at the case"
    );

    let start = h.clock.load(Ordering::SeqCst);
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::RECOVER_MEMBER,
        &olga,
        json!({"member": "mem_carl"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["principal"], carl);
    assert_eq!(row["state"], "pending");
    assert!(row["ticket"].is_null());
    assert_eq!(
        row["maturesAt"],
        start + 7_200,
        "the longer of the two delays"
    );
    assert_eq!(row["approvalsRequired"], 2);
    let case = row["case"].as_str().expect("case").to_string();

    // The tenant's second owner approves (`approveMemberRecovery`), and the
    // approval is in the tenant's own stream, naming who (G12).
    let _ = entries(&h);
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::APPROVE_MEMBER_RECOVERY,
        &session_token("pia"),
        json!({"member": "mem_carl", "case": case}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["approvals"], 2);
    assert_eq!(body["rows"][0]["outcome"], "approved");
    let tenant = Scope::Space(ordo_commandlog::SpaceId(TWIN.into()));
    assert!(
        entries(&h).iter().any(|e| e.scope == tenant
            && matches!(&e.kind, EventKind::Recovery { act, detail }
                if act == "approved" && detail.contains("by=pia"))),
        "no `approved` record in the tenant's stream"
    );
}

/// **Two approvals required means one is not enough.** The opener is the first,
/// the case yields nothing on that alone, a second owner completes it, and
/// neither the same owner twice nor the person the case is about can.
#[tokio::test(flavor = "multi_thread")]
async fn two_approvals_required_means_one_is_not_enough() {
    let h = harness(
        "ordo_rec_ops_approvals",
        RecoveryPolicy {
            operator_delay_secs: 0,
            operator_approvals: 2,
            ..RecoveryPolicy::default()
        },
    )
    .await;
    let (ana, _) = sign_up(&h, &TestAuthenticator::es256()).await;
    admit(&h, &h.home, "mem_o2", "owner2", "owner").await;
    let first = session_token(OWNER);
    let second = session_token("owner2");

    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &first,
        json!({"principal": ana, "evidence": "verified-out-of-band"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["state"], "pending", "no delay, and still not enough");
    assert!(row["ticket"].is_null());
    assert_eq!(row["approvalsRequired"], 2);
    let case = row["case"].as_str().expect("case").to_string();

    // One approval, the opener's, yields nothing.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &first,
        json!({"case": case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // The opener cannot approve twice, and the subject cannot approve.
    let (status, _) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::APPROVE_RECOVERY,
        &first,
        json!({"case": case}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "the opener approved already");
    admit(&h, &h.home, "mem_ana_home", &ana, "owner").await;
    let (status, _) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::APPROVE_RECOVERY,
        &session_token(&ana),
        json!({"case": case}),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "the person it is about cannot approve"
    );

    // A second owner completes it, and only then is the ticket theirs to collect.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::APPROVE_RECOVERY,
        &second,
        json!({"case": case}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["approvals"], 2);
    assert_eq!(body["rows"][0]["outcome"], "approved");
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &first,
        json!({"case": case}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["rows"][0]["ticket"].is_string());

    // Each approval is in the deployment's stream, naming who.
    let approved: Vec<String> = entries(&h)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Recovery { act, detail }
                if act == "approved" && e.scope == Scope::Deployment =>
            {
                Some(detail)
            }
            _ => None,
        })
        .collect();
    assert!(
        approved.iter().any(|d| d.contains("by=owner2")),
        "{approved:?}"
    );
}

/// A tenant may switch its administrators' recovery off, and switch recovery
/// codes off for the people it created. Each refuses as the tenant chose, and
/// leaves every other person and tenant alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_may_switch_off_its_administrators_recovery_and_its_members_codes() {
    let h = harness("ordo_rec_ops_switches", RecoveryPolicy::default()).await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(&h, TWIN, "mem_olga", "olga", "owner").await;
    let carl_device = device("carl");
    let carl = managed_member(&h, TWIN, "mem_carl", "mem_olga", &carl_device, "editor").await;
    let olga = session_token("olga");
    // A person nobody created for a tenant, who holds a set of codes.
    let (dana, dana_token) = sign_up(&h, &device("dana")).await;
    let app = h.auth_app();
    let issue = |token: String| {
        let app = app.clone();
        async move { auth_send(&app, "/auth/recovery-codes", None, Some(&token)).await }
    };
    let (status, dana_codes) = issue(dana_token).await;
    assert_eq!(status, StatusCode::OK, "{dana_codes}");
    let (_, carl_token) = {
        let (_, options) = auth_send(
            &app,
            "/auth/begin",
            Some(json!({"method": "passkey"})),
            None,
        )
        .await;
        let ch = options["challenge"].as_str().expect("challenge");
        auth_send(
            &app,
            "/auth/complete",
            Some(assertion_body(&carl_device.assert(ch))),
            None,
        )
        .await
    };
    let carl_token = carl_token["token"].as_str().expect("token").to_string();
    let (status, carl_codes) = issue(carl_token.clone()).await;
    assert_eq!(status, StatusCode::OK, "before the switch: {carl_codes}");

    // The tenant switches both off.
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::SET_RECOVERY_POLICY,
        &olga,
        json!({"pathEnabled": false, "codesEnabled": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["pathEnabled"], false);
    assert_eq!(body["rows"][0]["codesEnabled"], false);
    // G12: the choice is recorded in the tenant's own stream, naming who.
    let tenant = Scope::Space(ordo_commandlog::SpaceId(TWIN.into()));
    assert!(
        entries(&h).iter().any(|e| e.scope == tenant
            && matches!(&e.kind, EventKind::Recovery { act, detail }
                if act == "policy-set" && detail.contains("by=olga"))),
        "no `policy-set` record in the tenant's stream"
    );

    // Its administrators cannot recover its members.
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::RECOVER_MEMBER,
        &olga,
        json!({"member": "mem_carl"}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(h.recovery.live_for(&carl).expect("read").is_none());
    // Its members cannot issue codes, and a code they hold does not work.
    let (status, _) = issue(carl_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let code = carl_codes["codes"][0].as_str().expect("code").to_string();
    let (status, _) = auth_send(
        &app,
        "/auth/recover/code",
        Some(json!({"code": code})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Somebody the tenant did not create is untouched by the choice.
    let dana_code = dana_codes["codes"][0].as_str().expect("code").to_string();
    let (status, body) = auth_send(
        &app,
        "/auth/recover/code",
        Some(json!({"code": dana_code})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let _ = dana;
}

// ═══════════ a tenant's managers recover members: the app developer's setting ═══════════

async fn clear_twin(h: &Harness) {
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
}

async fn recover_member_as(h: &Harness, who: &str, member: &str) -> (StatusCode, Value) {
    command(
        h,
        TWIN,
        ordo_host::identity_ops::RECOVER_MEMBER,
        &session_token(who),
        json!({"member": member}),
    )
    .await
}

/// **A manager recovers nobody unless the app developer has set it**, and a
/// tenant cannot turn on what the deployment left off.
#[tokio::test(flavor = "multi_thread")]
async fn a_manager_may_not_recover_a_member_unless_the_app_developer_set_it() {
    let h = harness("ordo_rec_ops_mgr_off", RecoveryPolicy::default()).await;
    assert!(!RecoveryPolicy::default().managers_may_recover);
    clear_twin(&h).await;
    admit(&h, TWIN, "mem_olga", "olga", "owner").await;
    admit(&h, TWIN, "mem_mia", "mia", "manager").await;
    let carl = managed_member(&h, TWIN, "mem_carl", "mem_olga", &device("carl"), "editor").await;

    let (status, body) = recover_member_as(&h, "mia", "mem_carl").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(h.recovery.live_for(&carl).expect("read").is_none());

    // A tenant may only turn it off for itself.
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::SET_RECOVERY_POLICY,
        &session_token("olga"),
        json!({"managersMayRecover": true}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(
        body.to_string().contains("never on"),
        "the refusal says why: {body}"
    );
    // Its owner still recovers the member.
    let (status, body) = recover_member_as(&h, "olga", "mem_carl").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// **With the setting on, a manager recovers a member below manager, and
/// never a peer or an owner**, and the tenant can turn it off for itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_manager_recovers_a_member_below_manager_when_the_setting_is_on() {
    let h = harness(
        "ordo_rec_ops_mgr_on",
        RecoveryPolicy {
            managers_may_recover: true,
            ..RecoveryPolicy::default()
        },
    )
    .await;
    clear_twin(&h).await;
    admit(&h, TWIN, "mem_olga", "olga", "owner").await;
    admit(&h, TWIN, "mem_mia", "mia", "manager").await;
    let mick = managed_member(&h, TWIN, "mem_mick", "mem_olga", &device("mick"), "manager").await;
    let carl = managed_member(&h, TWIN, "mem_carl", "mem_olga", &device("carl"), "editor").await;
    let dee = managed_member(&h, TWIN, "mem_dee", "mem_olga", &device("dee"), "viewer").await;
    let _ = entries(&h);

    // A member below manager.
    let (status, body) = recover_member_as(&h, "mia", "mem_carl").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["principal"], carl);
    assert!(body["rows"][0]["ticket"].is_string());
    // A peer, an owner and herself.
    for target in ["mem_mick", "mem_olga", "mem_mia"] {
        let (status, body) = recover_member_as(&h, "mia", target).await;
        assert_ne!(status, StatusCode::OK, "{target}: {body}");
    }
    assert!(h.recovery.live_for(&mick).expect("read").is_none());

    // The refusal is recorded with its reason.
    let refused_for_peer = entries(&h).iter().any(|e| {
        matches!(&e.kind, EventKind::Recovery { act, detail }
            if act == "refused" && detail.contains("reason=target-not-below-manager"))
    });
    assert!(refused_for_peer, "the stream says why a peer was refused");

    // The tenant turns it off for itself, and the manager is refused again.
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::SET_RECOVERY_POLICY,
        &session_token("olga"),
        json!({"managersMayRecover": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["managersMayRecover"], false);
    let (status, body) = recover_member_as(&h, "mia", "mem_dee").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(h.recovery.live_for(&dee).expect("read").is_none());
    // Its owner is not affected by the choice.
    let (status, body) = recover_member_as(&h, "olga", "mem_dee").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// **A single-person tenant has nobody to recover.** The owner cannot recover
/// themselves, the setting can be changed, and no case is opened.
#[tokio::test(flavor = "multi_thread")]
async fn a_single_person_tenant_recovers_nobody() {
    let h = harness(
        "ordo_rec_ops_solo",
        RecoveryPolicy {
            managers_may_recover: true,
            ..RecoveryPolicy::default()
        },
    )
    .await;
    clear_twin(&h).await;
    let (sole, _) = sign_up(&h, &device("sole")).await;
    ordo_host::credential_ops::ManagedStore::new(h.pool.clone())
        .mark(&sole, TWIN, "mem_sole", real_now())
        .await
        .expect("marked");
    admit(&h, TWIN, "mem_sole", &sole, "owner").await;

    let (status, body) = recover_member_as(&h, &sole, "mem_sole").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(h.recovery.live_for(&sole).expect("read").is_none());
    let (status, body) = command(
        &h,
        TWIN,
        ordo_host::identity_ops::SET_RECOVERY_POLICY,
        &session_token(&sole),
        json!({"managersMayRecover": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ═══════════ an outside provider's sign-in is recovered by that provider ═══════════

/// A person whose only sign-in is an outside provider's, created for `space`.
async fn outside_only_member(h: &Harness, space: &str, member_id: &str, link: &str) -> String {
    use ordo_identity::DirectoryStore;
    let principal = h
        .store
        .create_principal(
            ordo_identity::principal::PrincipalKind::Human,
            "Erin",
            IStamp(real_now()),
        )
        .expect("minted");
    let row = h.store.principal(&principal).unwrap().unwrap();
    h.store
        .insert_link(
            &ordo_identity::provider::AuthenticatorId(link.into()),
            &row,
            IStamp(real_now()),
        )
        .expect("linked");
    ordo_host::credential_ops::ManagedStore::new(h.pool.clone())
        .mark(&principal.0, space, "olga", real_now())
        .await
        .expect("marked");
    admit(h, space, member_id, &principal.0, "editor").await;
    principal.0
}

/// **No recovery is opened for a person whose only sign-in belongs to an
/// outside provider**, by a tenant's owner or by the deployment owner, and the
/// refusal says to recover it at the provider.
#[tokio::test(flavor = "multi_thread")]
async fn no_recovery_is_opened_for_a_person_whose_only_sign_in_is_an_outside_providers() {
    let h = harness("ordo_rec_ops_outside", RecoveryPolicy::default()).await;
    clear_twin(&h).await;
    admit(&h, TWIN, "mem_olga", "olga", "owner").await;
    let erin = outside_only_member(&h, TWIN, "mem_erin", "oidc:google:erin-sub").await;

    let (status, body) = recover_member_as(&h, "olga", "mem_erin").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(
        body.to_string().contains("Recover it at the provider"),
        "{body}"
    );
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &session_token(OWNER),
        json!({"principal": erin, "evidence": "host-break-glass"}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(
        body.to_string().contains("Recover it at the provider"),
        "{body}"
    );
    assert!(h.recovery.live_for(&erin).expect("read").is_none());
}

/// **A person who has a passkey and an outside provider's sign-in is
/// recovered, and the provider's sign-in is neither retired nor unlinked.**
#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_leaves_a_persons_outside_provider_sign_in_alone() {
    let h = harness("ordo_rec_ops_mixed", RecoveryPolicy::default()).await;
    clear_twin(&h).await;
    admit(&h, TWIN, "mem_olga", "olga", "owner").await;
    let lost = device("gil-lost");
    let gil = managed_member(&h, TWIN, "mem_gil", "olga", &lost, "editor").await;
    {
        use ordo_identity::DirectoryStore;
        let row = h
            .store
            .principal(&PrincipalId(gil.clone()))
            .unwrap()
            .unwrap();
        h.store
            .insert_link(
                &ordo_identity::provider::AuthenticatorId("oidc:google:gil-sub".into()),
                &row,
                IStamp(real_now()),
            )
            .expect("linked");
    }
    let (status, body) = recover_member_as(&h, "olga", "mem_gil").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["rows"][0]["ticket"].as_str().expect("ticket");
    let kept = device("gil-new");
    let app = h.auth_app();
    let (_, options) = auth_send(
        &app,
        "/auth/recover/begin",
        Some(json!({"ticket": ticket})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    let mut reg = registration_body(&kept.register(ch));
    reg["ticket"] = json!(ticket);
    let (status, done) = auth_send(&app, "/auth/recover/complete", Some(reg), None).await;
    assert_eq!(status, StatusCode::OK, "{done}");

    use ordo_identity::DirectoryStore;
    let google = ordo_identity::provider::AuthenticatorId("oidc:google:gil-sub".into());
    let meta = h.store.link_meta(&google).unwrap().expect("still linked");
    assert!(meta.retired.is_none(), "the provider's link is not retired");
    assert_eq!(
        h.store.principal_for(&google).unwrap(),
        Some(PrincipalId(gil.clone()))
    );
    let passkey =
        ordo_identity::provider::AuthenticatorId(format!("passkey:{}", b64url_encode(b"gil-lost")));
    assert!(
        h.store
            .link_meta(&passkey)
            .unwrap()
            .expect("kept for the window")
            .retired
            .is_some(),
        "the person's own passkey is what was retired"
    );
}

// ═══════ owner succession: managers told and object; grants end (2026-10-24) ═══════

/// The acts with this word, by scope and detail.
fn acts(all: &[ordo_secevent::StreamEntry], word: &str) -> Vec<(Scope, String)> {
    all.iter()
        .filter_map(|e| match &e.kind {
            EventKind::Recovery { act, detail } if act == word => {
                Some((e.scope.clone(), detail.clone()))
            }
            _ => None,
        })
        .collect()
}

fn twin_scope() -> Scope {
    Scope::Space(ordo_commandlog::SpaceId(TWIN.into()))
}

/// A tenant with a departed owner, a successor (an editor), a manager and an
/// editor who is not a manager. Answers their principals and the manager's
/// and the editor's devices.
struct Cast {
    departed: String,
    bob: String,
    mia: String,
    ed: String,
    mia_device: TestAuthenticator,
    ed_device: TestAuthenticator,
    departed_device: TestAuthenticator,
}

async fn cast(h: &Harness) -> Cast {
    let departed_device = device("succ-departed");
    let mia_device = device("succ-mia");
    let ed_device = device("succ-ed");
    let (departed, _) = sign_up(h, &departed_device).await;
    let (bob, _) = sign_up(h, &device("succ-bob")).await;
    let (mia, _) = sign_up(h, &mia_device).await;
    let (ed, _) = sign_up(h, &ed_device).await;
    h.pool
        .rows(&format!(
            "DELETE FROM \"members\" WHERE \"$space\" = '{TWIN}'"
        ))
        .await
        .expect("cleared");
    admit(h, TWIN, "mem_departed", &departed, "owner").await;
    admit(h, TWIN, "mem_bob", &bob, "editor").await;
    admit(h, TWIN, "mem_mia", &mia, "manager").await;
    admit(h, TWIN, "mem_ed", &ed, "editor").await;
    Cast {
        departed,
        bob,
        mia,
        ed,
        mia_device,
        ed_device,
        departed_device,
    }
}

async fn try_open_succession(h: &Harness, principal: &str, departed: Value) -> (StatusCode, Value) {
    command(
        h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &session_token(OWNER),
        json!({"tenant": TWIN, "principal": principal, "evidence": "contract-holder",
               "reference": "MSA-9", "departed": departed}),
    )
    .await
}

async fn open_succession(h: &Harness, principal: &str, departed: &str) -> String {
    let (status, body) = try_open_succession(h, principal, json!([departed])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["rows"][0]["case"].as_str().expect("case").to_string()
}

async fn object(h: &Harness, who: &str, case: &str) -> (StatusCode, Value) {
    command(
        h,
        TWIN,
        ordo_host::identity_ops::OBJECT_TO_CASE,
        &session_token(who),
        json!({"case": case}),
    )
    .await
}

async fn complete_succession(h: &Harness, case: &str, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({"tenant": TWIN, "case": case});
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    command(
        h,
        &h.home,
        ordo_host::identity_ops::SUCCEED_OWNER,
        &session_token(OWNER),
        body,
    )
    .await
}

async fn resolve(h: &Harness, case: &str, decision: &str, reason: &str) -> (StatusCode, Value) {
    command(
        h,
        &h.home,
        ordo_host::identity_ops::RESOLVE_OBJECTION,
        &session_token(OWNER),
        json!({"case": case, "decision": decision, "reason": reason}),
    )
    .await
}

async fn open_cases(h: &Harness) -> Vec<Value> {
    let (status, body) = send(
        &h.state,
        Request::builder()
            .method("GET")
            .uri(format!("/op/{}", ordo_host::identity_ops::OPEN_CASES))
            .header("x-ordo-space", &h.home)
            .header("authorization", format!("Bearer {}", session_token(OWNER)))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["rows"].as_array().cloned().unwrap_or_default()
}

/// **The tenant's managers are told of a succession at their next sign-in,
/// and a manager's objection pauses it until the deployment owner resumes
/// it.** An editor is not told and may not object. Every step is in the
/// deployment's stream and the tenant's.
#[tokio::test(flavor = "multi_thread")]
async fn a_managers_objection_pauses_a_succession_until_the_deployment_owner_resumes_it() {
    let h = harness("ordo_rec_ops_object", RecoveryPolicy::default()).await;
    let c = cast(&h).await;
    let _ = entries(&h);
    let case = open_succession(&h, &c.bob, &c.departed).await;

    // The manager is told at the next sign-in; the editor is not.
    let (status, mia_in) = sign_in_answer(&h, &c.mia_device).await;
    assert_eq!(status, StatusCode::OK, "{mia_in}");
    let told = &mia_in["owner_cases"][0];
    assert_eq!(told["case"], case.as_str(), "{mia_in}");
    assert_eq!(told["tenant"], TWIN);
    assert_eq!(told["path"], "succession");
    assert_eq!(told["paused"], false);
    assert_eq!(
        told["departed"],
        json!([c.departed]),
        "the notice names who has left"
    );
    let (_, ed_in) = sign_in_answer(&h, &c.ed_device).await;
    assert!(ed_in.get("owner_cases").is_none(), "{ed_in}");

    // An editor may not object (the door refuses them), nor may an owner
    // (the op refuses and records it: managers only); the manager may, once.
    let (status, body) = object(&h, &c.ed, &case).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    let (status, body) = object(&h, &c.departed, &case).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    let (status, body) = object(&h, &c.mia, &case).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["state"], "paused");
    let (status, _) = object(&h, &c.mia, &case).await;
    assert_ne!(status, StatusCode::OK, "one objection per manager");

    // Paused: past the delay it still cannot be carried out, and the refusal
    // names the op that resolves it.
    h.set_now(h.clock.load(Ordering::SeqCst) + DAY);
    let (status, body) = complete_succession(&h, &case, json!({})).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(
        body.to_string().contains("std.identity.resolveObjection"),
        "{body}"
    );
    assert_eq!(role_of(&h, TWIN, &c.bob).await, "editor");
    let listed = open_cases(&h).await;
    let row = listed
        .iter()
        .find(|r| r["case"] == case.as_str())
        .expect("listed");
    assert_eq!(row["paused"], true);
    assert_eq!(row["mature"], false);
    assert_eq!(row["objections"][0]["objector"], c.mia.as_str());
    assert!(row["objections"][0]["resolution"].is_null());
    assert_eq!(row["departed"], json!([c.departed]));
    let (_, mia_in) = sign_in_answer(&h, &c.mia_device).await;
    assert_eq!(mia_in["owner_cases"][0]["paused"], true, "{mia_in}");
    assert_eq!(mia_in["owner_cases"][0]["objected"], true, "{mia_in}");

    // A reason off the list is refused; a resume goes on.
    let (status, _) = resolve(&h, &case, "resume", "objection-upheld").await;
    assert_ne!(status, StatusCode::OK, "a cancel reason does not resume");
    let (status, body) = resolve(&h, &case, "resume", "evidence-rechecked").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["objections"], 1);
    let listed = open_cases(&h).await;
    let row = listed
        .iter()
        .find(|r| r["case"] == case.as_str())
        .expect("listed");
    assert_eq!(row["objections"][0]["resolution"], "resume");
    assert_eq!(row["objections"][0]["reason"], "evidence-rechecked");
    assert_eq!(row["mature"], true);
    let (status, body) = complete_succession(&h, &case, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(role_of(&h, TWIN, &c.bob).await, "owner");
    let (_, mia_in) = sign_in_answer(&h, &c.mia_device).await;
    assert!(
        mia_in.get("owner_cases").is_none(),
        "a closed case is not told"
    );

    // Both streams carry each step; the tenant's never names the deployment
    // owner, and the objection is in no other tenant.
    let all = entries(&h);
    for word in [
        "opened",
        "objected",
        "objection-resolved",
        "owner-succeeded",
    ] {
        let found = acts(&all, word);
        assert!(
            found.iter().any(|(s, _)| *s == Scope::Deployment),
            "{word} in the deployment's stream: {found:?}"
        );
        let (_, tenant) = found
            .iter()
            .find(|(s, _)| *s == twin_scope())
            .unwrap_or_else(|| panic!("{word} in the tenant's stream: {found:?}"));
        assert!(!tenant.contains(OWNER), "{word}: {tenant}");
    }
    let objected = acts(&all, "objected");
    assert!(objected
        .iter()
        .all(|(s, d)| (*s == Scope::Deployment || *s == twin_scope())
            && d.contains(&format!("by={}", c.mia))));
    assert!(acts(&all, "objection-resolved")
        .iter()
        .any(|(s, d)| *s == Scope::Deployment
            && d.contains(&format!("by={OWNER}"))
            && d.contains("decision=resume reason=evidence-rechecked")));
    let refused = acts(&all, "refused");
    assert!(
        refused
            .iter()
            .any(|(_, d)| d.contains("reason=not-a-manager") && d.contains(&c.departed)),
        "the owner's attempt is recorded: {refused:?}"
    );
}

/// **A cancel ends a paused succession, and a last-owner recovery through
/// the door shares the rule**: the owner's tenant's manager is told of it,
/// may object, and the objection holds the ticket back until resolved.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_ends_a_paused_case_and_a_last_owner_recovery_shares_the_rule() {
    let h = harness("ordo_rec_ops_object_cancel", RecoveryPolicy::default()).await;
    let c = cast(&h).await;
    let case = open_succession(&h, &c.bob, &c.departed).await;
    let (status, _) = object(&h, &c.mia, &case).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = resolve(&h, &case, "cancel", "evidence-rechecked").await;
    assert_ne!(
        status,
        StatusCode::OK,
        "a resume reason does not cancel: {body}"
    );
    let (status, body) = resolve(&h, &case, "cancel", "objection-upheld").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["state"], "cancelled");
    h.set_now(h.clock.load(Ordering::SeqCst) + DAY);
    let (status, _) = complete_succession(&h, &case, json!({})).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "a cancelled succession is not carried out"
    );
    assert_eq!(role_of(&h, TWIN, &c.bob).await, "editor");
    assert!(open_cases(&h)
        .await
        .iter()
        .all(|r| r["case"] != case.as_str()));
    let (status, _) = resolve(&h, &case, "resume", "evidence-rechecked").await;
    assert_ne!(status, StatusCode::OK, "nothing left to resolve");

    // The last owner's recovery through the door.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &session_token(OWNER),
        json!({"principal": c.departed, "evidence": "verified-out-of-band"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rec = body["rows"][0]["case"].as_str().expect("case").to_string();
    let (_, mia_in) = sign_in_answer(&h, &c.mia_device).await;
    let told = mia_in["owner_cases"]
        .as_array()
        .and_then(|a| a.iter().find(|n| n["case"] == rec.as_str()))
        .cloned()
        .unwrap_or_else(|| panic!("the manager is told of the owner's recovery: {mia_in}"));
    assert_eq!(told["path"], "operator");
    assert_eq!(told["tenant"], TWIN);
    let (status, body) = object(&h, &c.ed, &rec).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    let (status, body) = object(&h, &c.mia, &rec).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    h.set_now(h.clock.load(Ordering::SeqCst) + DAY);
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &session_token(OWNER),
        json!({"case": rec}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "a paused recovery yields no ticket");
    assert!(body.to_string().contains("resolveObjection"), "{body}");
    let (status, _) = resolve(&h, &rec, "resume", "objector-answered").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVERY_TICKET,
        &session_token(OWNER),
        json!({"case": rec}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let _ = c.departed_device;
}

/// One live delegation `delegator` granted in the twin tenant to `agent`,
/// with the grant its consent minted, as the consent ceremony leaves them.
async fn delegated(h: &Harness, n: u32, delegator: &str, agent: &str) -> (String, String) {
    let (d, g) = (format!("dlg_{n}"), format!("cg_{n}"));
    let far = 9_000_000_000u64;
    let hash = "ab".repeat(32);
    for sql in [
        format!(
            "INSERT INTO \"$delegation\" (\"$space\", \"delegation_id\", \"delegator\", \"agent\", \
             \"granted_at\", \"expires\") VALUES ('{TWIN}', '{d}', '{delegator}', '{agent}', 1, {far})"
        ),
        format!(
            "INSERT INTO \"$capabilitygrant\" (\"$space\", \"grant_id\", \"op\", \"principal\", \
             \"manifest_hash\", \"granted_at\", \"expires\") VALUES \
             ('{TWIN}', '{g}', 'notes.list', '{agent}', '{hash}', 1, {far})"
        ),
        format!(
            "INSERT INTO \"$delegation_request\" (\"request_id\", \"$space\", \"agent\", \"person\", \
             \"ops\", \"term_secs\", \"binder\", \"created_at\", \"expires_at\", \"status\", \
             \"delegator\", \"delegation_id\", \"grant_id\") VALUES ('req_{n}', '{TWIN}', \
             '{agent}', '{delegator}', 'notes.list', 3600, 'b', 1, 2, 'approved', '{delegator}', \
             '{d}', '{g}')"
        ),
    ] {
        h.pool.rows(&sql).await.expect("seeded consent");
    }
    (d, g)
}

async fn revoked(h: &Harness, table: &str, column: &str, id: &str) -> bool {
    let rows = h
        .pool
        .rows(&format!(
            "SELECT \"revoked_at\" FROM \"{table}\" WHERE \"{column}\" = '{id}'"
        ))
        .await
        .expect("read");
    rows.iter().any(|r| r.first().cloned().flatten().is_some())
}

/// **Completing a succession ends the named departed owner's delegations and
/// the grants to apps that came with them, by default, and the record names
/// each one.** A living co-owner not named keeps theirs. A completion that
/// says `endGrants: false` keeps them all.
#[tokio::test(flavor = "multi_thread")]
async fn completing_a_succession_ends_the_departed_owners_grants_unless_told_not_to() {
    let h = harness("ordo_rec_ops_end_grants", RecoveryPolicy::default()).await;
    let c = cast(&h).await;
    // A living co-owner, not named as departed.
    let (co, _) = sign_up(&h, &device("succ-co")).await;
    admit(&h, TWIN, "mem_co", &co, "owner").await;
    let (d1, g1) = delegated(&h, 1, &c.departed, "agent_x").await;
    // Somebody else's delegations are not touched: a manager's, and the
    // co-owner's.
    let (d_mia, g_mia) = delegated(&h, 2, &c.mia, "agent_y").await;
    let (d_co, g_co) = delegated(&h, 4, &co, "agent_w").await;
    let _ = entries(&h);

    // `departed` is required, names owners only, and never the successor.
    for bad in [
        json!(null),
        json!([]),
        json!([c.ed]),
        json!([c.bob]),
        json!("x"),
    ] {
        let (status, body) = try_open_succession(&h, &c.bob, bad.clone()).await;
        assert_ne!(status, StatusCode::OK, "{bad}: {body}");
    }

    let case = open_succession(&h, &c.bob, &c.departed).await;
    h.set_now(h.clock.load(Ordering::SeqCst) + DAY);
    let (status, body) = complete_succession(&h, &case, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["endsGrants"], true);
    assert_eq!(row["delegationsEnded"], json!([format!("{d1}@agent_x")]));
    assert_eq!(row["grantsEnded"], json!([format!("{g1}@agent_x")]));
    assert_eq!(row["notEnded"], json!([]));
    assert!(revoked(&h, "$delegation", "delegation_id", &d1).await);
    assert!(revoked(&h, "$capabilitygrant", "grant_id", &g1).await);
    assert!(!revoked(&h, "$delegation", "delegation_id", &d_mia).await);
    assert!(!revoked(&h, "$capabilitygrant", "grant_id", &g_mia).await);
    assert!(
        !revoked(&h, "$delegation", "delegation_id", &d_co).await,
        "a living co-owner keeps their delegations"
    );
    assert!(!revoked(&h, "$capabilitygrant", "grant_id", &g_co).await);
    assert_eq!(row["departed"], json!([c.departed]));
    let all = entries(&h);
    let done = acts(&all, "owner-succeeded");
    for scope in [Scope::Deployment, twin_scope()] {
        let (_, d) = done
            .iter()
            .find(|(s, _)| *s == scope)
            .unwrap_or_else(|| panic!("{scope:?}: {done:?}"));
        assert!(d.contains("ends-grants=on"), "{d}");
        assert!(
            d.contains(&format!("delegations-ended={d1}@agent_x")),
            "{d}"
        );
        assert!(d.contains(&format!("grants-ended={g1}@agent_x")), "{d}");
        assert!(d.contains(&format!("departed={}", c.departed)), "{d}");
    }

    // A second succession, carried out with `endGrants: false`, keeps them.
    let (d3, g3) = delegated(&h, 3, &c.departed, "agent_z").await;
    let case = open_succession(&h, &c.ed, &c.departed).await;
    h.set_now(h.clock.load(Ordering::SeqCst) + DAY);
    let (status, body) = complete_succession(&h, &case, json!({"endGrants": false})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["endsGrants"], false);
    assert_eq!(body["rows"][0]["delegationsEnded"], json!([]));
    assert!(!revoked(&h, "$delegation", "delegation_id", &d3).await);
    assert!(!revoked(&h, "$capabilitygrant", "grant_id", &g3).await);
    let done = acts(&entries(&h), "owner-succeeded");
    assert!(
        done.iter()
            .any(|(s, d)| *s == Scope::Deployment && d.contains("ends-grants=off")),
        "{done:?}"
    );
}

/// **`std.identity.closeContest`** (audit F-3, 2026-10-24): the deployment
/// owner ends a completed recovery's contest window now. Only in the home
/// space, only with presence, only with a reason from the list. Both streams
/// record it, the tenant's without the deployment owner's name. After it the
/// lost passkey cannot contest and a new case opens at once.
#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_owner_closes_a_contest_and_a_new_case_opens_at_once() {
    let h = harness(
        "ordo_rec_ops_close_contest",
        RecoveryPolicy {
            operator_delay_secs: 0,
            ..RecoveryPolicy::default()
        },
    )
    .await;
    let (lost, kept) = (TestAuthenticator::es256(), TestAuthenticator::ed25519());
    let (ana, _) = sign_up(&h, &lost).await;
    admit(&h, TWIN, "mem_ana", &ana, "editor").await;
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &session_token(OWNER),
        json!({"principal": ana, "evidence": "verified-out-of-band", "reference": "INC-7"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["rows"][0]["ticket"].as_str().expect("ticket");
    let app = h.auth_app();
    let (_, options) = auth_send(
        &app,
        "/auth/recover/begin",
        Some(json!({"ticket": ticket})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    let mut reg = registration_body(&kept.register(ch));
    reg["ticket"] = json!(ticket);
    let (status, done) = auth_send(&app, "/auth/recover/complete", Some(reg), None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let case = done["case"].as_str().expect("case").to_string();
    let _ = entries(&h);

    let close = |space: String, token: String, reason: &'static str| {
        let case = case.clone();
        let h = &h;
        async move {
            command(
                h,
                &space,
                ordo_host::identity_ops::CLOSE_CONTEST,
                &token,
                json!({"case": case, "reason": reason}),
            )
            .await
        }
    };
    // Not in a tenant.
    let (status, body) = close(TWIN.into(), session_token(OWNER), "recovery-verified").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    // Not without presence.
    let no_touch = ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(OWNER.into()),
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: Some(SessionClaim {
                id: SessionId::mint(),
                started_at: IStamp(real_now()),
                second_step: None,
                via: None,
                upstream_presence: None,
            }),
        },
    );
    let (status, body) = close(h.home.clone(), no_touch, "recovery-verified").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    // Not with a reason off the list.
    let (status, body) = close(h.home.clone(), session_token(OWNER), "because").await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("recovery-verified"), "{body}");

    let (status, body) = close(h.home.clone(), session_token(OWNER), "recovery-verified").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["state"], "closed");
    assert_eq!(body["rows"][0]["retired"].as_array().map(Vec::len), Some(1));

    let closed = acts(&entries(&h), "contest-closed");
    let deployment = closed
        .iter()
        .find(|(s, _)| *s == Scope::Deployment)
        .expect("the deployment's stream has it");
    assert!(
        deployment.1.contains("reason=recovery-verified"),
        "{deployment:?}"
    );
    assert!(
        !deployment.1.contains("by=deployment-owner"),
        "{deployment:?}"
    );
    let tenant = closed
        .iter()
        .find(|(s, _)| *s == twin_scope())
        .expect("the tenant's stream has it");
    assert!(tenant.1.contains("by=deployment-owner"), "{tenant:?}");
    assert!(!tenant.1.contains(OWNER), "{tenant:?}");

    // The lost passkey can no longer contest.
    h.set_now(h.clock.load(Ordering::SeqCst) + 60);
    let (_, options) = auth_send(
        &app,
        "/auth/begin",
        Some(json!({"method": "passkey"})),
        None,
    )
    .await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, _) = auth_send(
        &app,
        "/auth/recover/contest",
        Some(assertion_body(&lost.assert(ch))),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // And a new case opens at once, where it waited out the window before.
    let (status, body) = command(
        &h,
        &h.home,
        ordo_host::identity_ops::RECOVER,
        &session_token(OWNER),
        json!({"principal": ana, "evidence": "verified-out-of-band", "reference": "INC-8"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
