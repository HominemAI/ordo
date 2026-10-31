//! **Enrolment E6: the password give and the reset**, with managed and
//! independent identities, the typed temporary password as a recorded option
//! off by default, and ending every session. Through the real door against
//! real Postgres.
//!
//! The claims:
//!
//! - an owner resets a person it created, and every session that person had
//!   stops on the next request;
//! - the same owner is refused for a person it did not create;
//! - a person admitted to a second tenant becomes independent, and the first
//!   tenant is then refused;
//! - a temporary password is refused unless the deployment allows it, and is
//!   recorded when it does;
//! - the give: the administrator receives a ticket, never a password.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_host::credential_ops::{CredentialOps, ManagedStore, RedeemError};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::member_ops::{self, MemberOps};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::runtime::SwappableAccess;
use ordo_host::sessions::PgSessionRevocations;
use ordo_host::tickets::{PgTicketStore, Tickets};
use ordo_host::FixtureExecutor;
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::provider::{Argon2idHasher, PasswordHasher};
use ordo_identity::session::{SessionClaim, SessionId, SessionRevocations};
use ordo_identity::{CredentialStore, DirectoryStore, TokenKey, TokenSpec};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const SECRET: [u8; 32] = *b"credential-reset-secret-32bytes!";
const OWNER: &str = "mem_o";
const TWIN: &str = "twin_space";
const TWIN_OWNER: &str = "mem_twin_o";
const SESSION_MAX: u64 = 12 * 3600;

fn now() -> u64 {
    ordo_host::runtime::now_secs()
}

struct Harness {
    state: ApiState,
    pool: ordo_exec::Pool,
    ops: Arc<CredentialOps>,
    revocations: Arc<PgSessionRevocations>,
    access: Arc<SwappableAccess>,
    space: String,
}

fn cfg(db: &str) -> ordo_exec::PoolConfig {
    ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    }
}

async fn harness(db: &str, allow_temporary: bool) -> Harness {
    let f = Arc::new(Fixture::load().expect("fixture"));
    f.reset(&Pg::named(db)).expect("seeded");
    let pool = ordo_exec::Pool::connect(cfg(db)).await.expect("pool");
    ordo_host::principal_directory::ensure_table(&pool)
        .await
        .unwrap();
    ordo_host::credentials::ensure_table(&pool).await.unwrap();
    ordo_host::sessions::ensure_table(&pool).await.unwrap();
    ordo_host::tickets::ensure_table(&pool).await.unwrap();
    ordo_host::credential_ops::ensure_table(&pool)
        .await
        .unwrap();
    for t in [
        "$authenticator",
        "$principal",
        "$credential",
        "$ticket",
        "$managed",
        "$temporarypassword",
        "$principalepoch",
        "$sessionrevocation",
    ] {
        pool.rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("clean");
    }
    let space = ordo_conformance::fixture_space().0;
    // A second tenant with an owner of its own, before the engine is built.
    pool.rows(&format!(
        "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
         \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") VALUES \
         ('{TWIN}', '{TWIN_OWNER}', 1, '{TWIN_OWNER}', 'Twin owner', 'principal', 1, 'owner', \
          'active', CURRENT_DATE)"
    ))
    .await
    .expect("twin owner");

    let grants = Arc::new(PgGrantRegistry::new(pool.clone()));
    grants.ensure_table().await.expect("grants");
    let delegations = Arc::new(ordo_host::delegation::PgDelegationRegistry::new(
        pool.clone(),
    ));
    delegations.ensure_table().await.expect("delegations");
    let revocations = Arc::new(
        PgSessionRevocations::open(pool.clone())
            .await
            .expect("revocations"),
    );

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let engine = ordo_host::deployment_engine_pooled(&f, &pool)
        .await
        .expect("engine");
    let door = ordo_host::IdentityAccess::sharing(
        Arc::new(engine),
        TokenKey::from_secret(&SECRET),
        Box::new(now),
        metrics.clone(),
    )
    .with_revocations(revocations.clone());
    let access = Arc::new(SwappableAccess::new(door));

    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "credential-reset-test",
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

    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory: None,
        stream: Some(access.recorder.clone()),
        refresh: Some(refresh),
        now: Arc::new(now),
        managed: Some(ManagedStore::new(pool.clone())),
    });
    let store: Arc<dyn CredentialStore> =
        Arc::new(ordo_host::credentials::PgCredentialStore::open(cfg(db)).expect("credentials"));
    let directory: Arc<dyn DirectoryStore> =
        Arc::new(PgDirectoryStore::open(cfg(db)).expect("directory"));
    let tickets = Arc::new(
        Tickets::new(Arc::new(PgTicketStore::open(cfg(db)).expect("tickets")))
            .with_recorder(access.recorder.clone()),
    );
    let ops = Arc::new(CredentialOps {
        members: members.clone(),
        directory,
        credentials: store,
        hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        tickets,
        managed: ManagedStore::new(pool.clone()),
        revocations: revocations.clone(),
        session_max_secs: SESSION_MAX,
        allow_temporary,
        control: pool.clone(),
        after_redeem: Default::default(),
    });

    let live = access.clone();
    let platform = Arc::new(
        PlatformOps::new(access.current().engine.clone(), grants, Arc::new(now))
            .with_engine_source(Arc::new(move || live.current().engine.clone()))
            .with_members(members)
            .with_credentials(ops.clone()),
    );
    let state = ApiState {
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
        page_key: PageKey::from_secret(b"credential-reset-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    Harness {
        state,
        pool,
        ops,
        revocations,
        access,
        space,
    }
}

/// A session token: it names its session and when it started.
fn session_token(principal: &str, started_at: u64) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(&SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: Some(IStamp(now())),
            capability: None,
            delegation: None,
            session: Some(SessionClaim {
                id: SessionId::mint(),
                started_at: IStamp(started_at),
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

fn owner_token() -> String {
    session_token(OWNER, now())
}

async fn door_open(h: &Harness, token: &str) -> bool {
    let (status, body) = send(
        &h.state,
        Request::builder()
            .method("GET")
            .uri("/op/activeProducts")
            .header("x-ordo-space", &h.space)
            .header("authorization", format!("Bearer {token}"))
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

/// Create a person through the op, and let them choose a password.
async fn person(h: &Harness, name: &str, identifier: &str, password: &str) -> (String, String) {
    let (status, body) = command(
        h,
        &h.space,
        ordo_host::credential_ops::CREATE_PERSON,
        &owner_token(),
        json!({"display": name}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    let principal = row["principal"].as_str().unwrap().to_string();
    let member = row["member"].as_str().unwrap().to_string();
    let ticket = row["ticket"].as_str().unwrap().to_string();
    let got = h
        .ops
        .redeem_password(&ticket, identifier, password)
        .await
        .expect("the person chooses a password");
    assert_eq!(got.0, principal);
    (principal, member)
}

/// **An administrator resets a managed identity and its old sessions stop.**
#[tokio::test(flavor = "multi_thread")]
async fn an_administrator_resets_a_managed_identity_and_its_old_sessions_stop() {
    let h = harness("ordo_cred_reset", false).await;
    let (principal, member) = person(&h, "Ana", "ana@example.test", "correct-horse").await;

    // Two sessions the person has, on two devices.
    let phone = session_token(&principal, now() - 500);
    let laptop = session_token(&principal, now() - 100);
    assert!(
        door_open(&h, &phone).await,
        "the control: the person is served"
    );
    assert!(door_open(&h, &laptop).await);

    let (status, body) = command(
        &h,
        &h.space,
        ordo_host::credential_ops::RESET_CREDENTIAL,
        &owner_token(),
        json!({"member": member}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["rows"][0]["ticket"]
        .as_str()
        .expect("a new ticket")
        .to_string();

    // Every old session stops, on the next request.
    assert!(!door_open(&h, &phone).await, "the phone session must stop");
    assert!(
        !door_open(&h, &laptop).await,
        "the laptop session must stop"
    );
    // And cannot be continued.
    let claims =
        ordo_identity::verify(&TokenKey::from_secret(&SECRET), &laptop, IStamp(now())).unwrap();
    assert!(
        ordo_identity::session::refresh(
            h.revocations.as_ref(),
            &claims,
            IStamp(now()),
            3600,
            SESSION_MAX
        )
        .is_err(),
        "an ended session cannot be refreshed"
    );
    // A session that starts after the reset is not touched.
    assert!(door_open(&h, &session_token(&principal, now() + 5)).await);

    // The old password is gone, and the new ticket sets a new one.
    let store = &h.ops.credentials;
    assert!(store.hash_for("ana@example.test").unwrap().is_none());
    h.ops
        .redeem_password(&ticket, "ana@example.test", "a new password")
        .await
        .expect("the new ticket works");
    let hash = store
        .hash_for("ana@example.test")
        .unwrap()
        .expect("enrolled again");
    assert!(Argon2idHasher::weak_for_tests().verify("a new password", &hash));
    assert!(!Argon2idHasher::weak_for_tests().verify("correct-horse", &hash));

    // On the record, at deployment scope and in the tenant's stream.
    let audit = h.access.current().take_audit().join("\n");
    assert!(audit.contains("enrolment"), "{audit}");
}

/// **The same administrator is refused for an independent identity**, one
/// this tenant admitted but did not create. And nothing about the person
/// changes.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_administrator_is_refused_for_an_independent_identity() {
    let h = harness("ordo_cred_independent", false).await;
    // An identity that signed itself up, and was admitted by the owner.
    let dir = PgDirectoryStore::open(cfg("ordo_cred_independent")).unwrap();
    let independent = dir
        .create_principal(ordo_identity::PrincipalKind::Human, "Ivy", IStamp(now()))
        .unwrap();
    h.ops
        .credentials
        .put(
            "ivy@example.test",
            &Argon2idHasher::weak_for_tests().hash("ivys own"),
        )
        .unwrap();
    let (status, body) = command(
        &h,
        &h.space,
        member_ops::ADMIT,
        &owner_token(),
        json!({"principal": independent.0, "role": "viewer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let member = body["rows"][0]["member"].as_str().unwrap().to_string();

    let session = session_token(&independent.0, now() - 10);
    assert!(door_open(&h, &session).await);

    let (status, body) = command(
        &h,
        &h.space,
        ordo_host::credential_ops::RESET_CREDENTIAL,
        &owner_token(),
        json!({"member": member}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(code(&body), "tenant_refused", "{body}");
    assert!(door_open(&h, &session).await, "their session is untouched");
    assert!(h
        .ops
        .credentials
        .hash_for("ivy@example.test")
        .unwrap()
        .is_some());

    // The temporary password is refused for the same reason, before it is
    // refused for being switched off, so it is checked with it on.
    let allowed = harness("ordo_cred_independent_allowed", true).await;
    let dir = PgDirectoryStore::open(cfg("ordo_cred_independent_allowed")).unwrap();
    let ivy = dir
        .create_principal(ordo_identity::PrincipalKind::Human, "Ivy", IStamp(now()))
        .unwrap();
    let (_, body) = command(
        &allowed,
        &allowed.space,
        member_ops::ADMIT,
        &owner_token(),
        json!({"principal": ivy.0, "role": "viewer"}),
    )
    .await;
    let member = body["rows"][0]["member"].as_str().unwrap().to_string();
    let (status, body) = command(
        &allowed,
        &allowed.space,
        ordo_host::credential_ops::SET_TEMPORARY_PASSWORD,
        &owner_token(),
        json!({"member": member, "identifier": "ivy@example.test", "password": "typed"}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(code(&body), "tenant_refused");
}

/// **An identity admitted to a second tenant becomes independent**, on the
/// record, and the first tenant is then refused.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_admitted_to_a_second_tenant_becomes_independent() {
    let h = harness("ordo_cred_second", false).await;
    let (principal, member) = person(&h, "Bo", "bo@example.test", "bos-password").await;
    let managed = ManagedStore::new(h.pool.clone());
    assert_eq!(
        managed.managed_by(&principal).await.unwrap().as_deref(),
        Some(h.space.as_str())
    );

    // The twin tenant's owner admits Bo.
    let (status, body) = command(
        &h,
        TWIN,
        member_ops::ADMIT,
        &session_token(TWIN_OWNER, now()),
        json!({"principal": principal, "role": "viewer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        managed.managed_by(&principal).await.unwrap(),
        None,
        "Bo stopped being managed the moment a second tenant admitted them"
    );
    let released = h
        .pool
        .rows(&format!(
            "SELECT \"released_at\" FROM \"$managed\" WHERE \"principal\" = '{principal}'"
        ))
        .await
        .unwrap();
    assert!(released[0][0].is_some(), "and the row says when");

    let (status, body) = command(
        &h,
        &h.space,
        ordo_host::credential_ops::RESET_CREDENTIAL,
        &owner_token(),
        json!({"member": member}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(code(&body), "tenant_refused");
    assert!(
        h.ops
            .credentials
            .hash_for("bo@example.test")
            .unwrap()
            .is_some(),
        "the first tenant reached nothing"
    );
    let audit = h.access.current().take_audit().join("\n");
    assert!(
        audit.contains("identity-released") || audit.contains("enrolment"),
        "{audit}"
    );
}

/// The belt and braces: a person who belongs elsewhere without the release
/// having run (a row written by hand) is still refused at the reset.
#[tokio::test(flavor = "multi_thread")]
async fn a_membership_written_by_hand_elsewhere_still_stops_a_reset() {
    let h = harness("ordo_cred_by_hand", false).await;
    let (principal, member) = person(&h, "Cy", "cy@example.test", "cys-password").await;
    h.pool
        .rows(&format!(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
             \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") VALUES \
             ('{TWIN}', 'mem_by_hand', 1, '{principal}', 'Cy', 'principal', 1, 'viewer', \
              'active', CURRENT_DATE)"
        ))
        .await
        .unwrap();
    let (status, body) = command(
        &h,
        &h.space,
        ordo_host::credential_ops::RESET_CREDENTIAL,
        &owner_token(),
        json!({"member": member}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(code(&body), "tenant_refused");
}

/// **A temporary password is refused unless the deployment allows it, and is
/// recorded when it does.**
#[tokio::test(flavor = "multi_thread")]
async fn a_temporary_password_is_refused_unless_the_deployment_allows_it_and_is_recorded_when_it_does(
) {
    // Off, which is the default.
    let off = harness("ordo_cred_temp_off", false).await;
    let (principal, member) = person(&off, "Di", "di@example.test", "dis-own").await;
    let (status, body) = command(
        &off,
        &off.space,
        ordo_host::credential_ops::SET_TEMPORARY_PASSWORD,
        &owner_token(),
        json!({"member": member, "identifier": "di@example.test", "password": "typed-by-admin"}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(code(&body), "tenant_refused");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("does not allow"));
    // Audit 31: the refusal names the setting and the value.
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("ORDO_AUTH_TEMPORARY_PASSWORDS=allow"));
    let hash = off
        .ops
        .credentials
        .hash_for("di@example.test")
        .unwrap()
        .unwrap();
    assert!(
        Argon2idHasher::weak_for_tests().verify("dis-own", &hash),
        "refused means nothing changed: Di's own password stands"
    );
    assert!(!ManagedStore::new(off.pool.clone())
        .must_change(&principal)
        .await
        .unwrap());

    // On.
    let on = harness("ordo_cred_temp_on", true).await;
    let (principal, member) = person(&on, "Ed", "ed@example.test", "eds-own").await;
    let old_session = session_token(&principal, now() - 60);
    assert!(door_open(&on, &old_session).await);
    let (status, body) = command(
        &on,
        &on.space,
        ordo_host::credential_ops::SET_TEMPORARY_PASSWORD,
        &owner_token(),
        json!({"member": member, "identifier": "ed@example.test", "password": "typed-by-admin"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.to_string().contains("typed-by-admin"),
        "the answer never repeats the password"
    );
    let hash = on
        .ops
        .credentials
        .hash_for("ed@example.test")
        .unwrap()
        .unwrap();
    assert!(Argon2idHasher::weak_for_tests().verify("typed-by-admin", &hash));
    assert!(!Argon2idHasher::weak_for_tests().verify("eds-own", &hash));

    // Marked, so the sign-in can require a change.
    let managed = ManagedStore::new(on.pool.clone());
    assert!(managed.must_change(&principal).await.unwrap());
    // Their old sessions ended.
    assert!(!door_open(&on, &old_session).await);
    // Recorded, with who set it and no password in it.
    let audit = on.access.current().take_audit().join("\n");
    assert!(audit.contains("temporary-password-set"), "{audit}");
    assert!(audit.contains(OWNER), "{audit}");
    assert!(!audit.contains("typed-by-admin"), "{audit}");

    // Choosing their own password ends the temporary one.
    let (_, body) = command(
        &on,
        &on.space,
        ordo_host::credential_ops::RESET_CREDENTIAL,
        &owner_token(),
        json!({"member": member}),
    )
    .await;
    let ticket = body["rows"][0]["ticket"].as_str().unwrap();
    on.ops
        .redeem_password(ticket, "ed@example.test", "eds-new-own")
        .await
        .unwrap();
    assert!(!managed.must_change(&principal).await.unwrap());
}

/// **The give.** The administrator receives a ticket and never a password;
/// a taken identifier costs the person nothing; a ticket is single use.
#[tokio::test(flavor = "multi_thread")]
async fn the_give_hands_the_administrator_a_ticket_and_the_person_chooses_their_own_password() {
    let h = harness("ordo_cred_give", false).await;
    let (status, body) = command(
        &h,
        &h.space,
        ordo_host::credential_ops::CREATE_PERSON,
        &owner_token(),
        json!({"display": "Flo", "role": "editor"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    let ticket = row["ticket"].as_str().unwrap().to_string();
    assert!(ticket.starts_with("tkt_"));
    assert_eq!(row["role"], "editor");
    assert!(row.get("password").is_none());

    // She is a member the moment she is created, at the role named, and has
    // no way in until she redeems.
    let principal = row["principal"].as_str().unwrap();
    let role = h
        .pool
        .rows(&format!(
            "SELECT \"role\" FROM \"members\" WHERE \"principal\" = '{principal}'"
        ))
        .await
        .unwrap();
    assert_eq!(role[0][0].as_deref(), Some("editor"));

    // An identifier somebody holds is refused, and the ticket is not spent.
    h.ops
        .credentials
        .put("taken@example.test", "not-a-real-hash")
        .unwrap();
    assert_eq!(
        h.ops
            .redeem_password(&ticket, "taken@example.test", "pw")
            .await,
        Err(RedeemError::IdentifierTaken)
    );
    let got = h
        .ops
        .redeem_password(&ticket, "flo@example.test", "flos-own")
        .await
        .expect("still redeemable");
    assert_eq!(got.0, principal);
    // Single use.
    assert_eq!(
        h.ops
            .redeem_password(&ticket, "flo2@example.test", "pw")
            .await,
        Err(RedeemError::Refused)
    );
    // Not a ticket, and a ticket for something else.
    assert_eq!(
        h.ops
            .redeem_password("tkt_nope", "x@example.test", "pw")
            .await,
        Err(RedeemError::Refused)
    );
}

/// The store, on its own: every session started at or before an instant,
/// monotonic, seen by a second replica only after it reads, swept when it
/// cannot matter.
#[tokio::test(flavor = "multi_thread")]
async fn ending_every_session_is_monotonic_durable_and_swept() {
    let h = harness("ordo_cred_epoch", false).await;
    let who = PrincipalId("usr_epoch".into());
    let other = PrincipalId("usr_other".into());
    let t = now();

    // A second replica: its own store over the same database.
    let replica = PgSessionRevocations::open(h.pool.clone()).await.unwrap();
    h.revocations
        .end_all(&who, IStamp(t), IStamp(t + SESSION_MAX))
        .unwrap();
    assert!(h.revocations.principal_ended(&who, IStamp(t - 1)));
    assert!(
        h.revocations.principal_ended(&who, IStamp(t)),
        "a tie ends the session"
    );
    assert!(
        !h.revocations.principal_ended(&who, IStamp(t + 1)),
        "the next second is fresh"
    );
    assert!(
        !h.revocations.principal_ended(&other, IStamp(t - 1)),
        "and only this person"
    );

    assert!(
        !replica.principal_ended(&who, IStamp(t - 1)),
        "not seen until it reads"
    );
    replica.refresh().await.unwrap();
    assert!(
        replica.principal_ended(&who, IStamp(t - 1)),
        "and then seen"
    );

    // An earlier call never moves it back.
    h.revocations
        .end_all(&who, IStamp(t - 500), IStamp(t + SESSION_MAX))
        .unwrap();
    assert!(h.revocations.principal_ended(&who, IStamp(t)));

    // It survives a restart.
    let restarted = PgSessionRevocations::open(h.pool.clone()).await.unwrap();
    assert!(restarted.principal_ended(&who, IStamp(t - 1)));

    // And is dropped once past the ceiling, when every older token is dead
    // by its own expiry anyway.
    h.pool
        .rows("UPDATE \"$principalepoch\" SET \"until\" = 1")
        .await
        .unwrap();
    restarted.refresh().await.unwrap();
    assert!(!restarted.principal_ended(&who, IStamp(t - 1)));

    // The in-memory store agrees about the semantics.
    let memory = ordo_identity::MemorySessionRevocations::new();
    memory
        .end_all(&who, IStamp(t), IStamp(t + SESSION_MAX))
        .unwrap();
    assert!(memory.principal_ended(&who, IStamp(t)));
    assert!(!memory.principal_ended(&who, IStamp(t + 1)));
}

/// **A reset removes every second factor once it completes**, and not when an
/// administrator asks for it (owner, 2026-09-29: *"but only after the reset"*).
/// The person's factors stay until they choose a password with the reset's
/// ticket, and nobody else's are touched.
#[tokio::test(flavor = "multi_thread")]
async fn a_reset_removes_every_second_factor_only_when_the_person_redeems_it() {
    use ordo_host::second_factor_store::{Factor, MemorySecondFactorStore, SecondFactorStore};
    let h = harness("ordo_cred_reset_factors", false).await;
    let (principal, member) = person(&h, "Ana", "ana@example.test", "correct-horse").await;

    // What the deployment wires at boot: the store the second step keeps its
    // factors in, and the hook a completed reset runs.
    let store = Arc::new(MemorySecondFactorStore::new());
    let factor = |id: &str, who: &str, kind: &str| Factor {
        id: id.to_string(),
        principal: who.to_string(),
        kind: kind.to_string(),
        label: "phone".into(),
        secret: "sealed".into(),
        last_counter: 0,
        failures: 0,
        disabled_at: 0,
        enrolled_at: 1,
        last_used_at: 0,
    };
    store.insert(&factor("sf_a1", &principal, "totp")).unwrap();
    store
        .insert(&factor("sf_a2", &principal, "security-key"))
        .unwrap();
    store
        .insert(&factor("sf_other", "usr_someone_else", "totp"))
        .unwrap();
    let hook_store = store.clone();
    h.ops
        .after_redeem
        .set(Arc::new(move |who: &str| {
            ordo_host::second_factors::remove_all_for(hook_store.as_ref(), None, who)
        }))
        .ok()
        .expect("the hook is set once");

    // Asking for the reset removes nothing.
    let (status, body) = command(
        &h,
        &h.space,
        ordo_host::credential_ops::RESET_CREDENTIAL,
        &owner_token(),
        json!({"member": member}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["rows"][0]["ticket"].as_str().unwrap().to_string();
    assert_eq!(
        store.list(&principal).unwrap().len(),
        2,
        "a reset that is only asked for removes nothing"
    );

    // The person completes it, and every factor they held goes.
    h.ops
        .redeem_password(&ticket, "ana@example.test", "a new password")
        .await
        .expect("the ticket works");
    assert!(store.list(&principal).unwrap().is_empty());
    assert_eq!(
        store.list("usr_someone_else").unwrap().len(),
        1,
        "nobody else's factor is touched"
    );
    let audit = h.access.current().take_audit().join("\n");
    assert!(audit.contains("second-factors-removed"), "{audit}");
}
