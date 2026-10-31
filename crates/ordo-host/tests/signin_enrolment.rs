//! **The four hooks enrolment left on the sign-in route**, against real
//! Postgres:
//!
//! 1. a sign-in that names a tenant runs that tenant's admission
//!    (`Admission::on_arrival`);
//! 2. a person redeems a password ticket at the door
//!    (`CredentialOps::redeem_password`);
//! 3. a temporary password cannot sign in until its holder chooses their own
//!    (`ManagedStore::must_change`);
//! 4. the deny list is re-read on a timer, which is proved against the
//!    running binary in `tests/runtime.rs`.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_conformance::{Fixture, Pg};
use ordo_host::admission::{Admission, Ceiling, PolicyChange, PolicyStore};
use ordo_host::auth::{AuthState, Enrolment};
use ordo_host::credential_ops::{CredentialOps, ManagedStore};
use ordo_host::member_ops::MemberOps;
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::sessions::PgSessionRevocations;
use ordo_host::tickets::{PgTicketStore, Purpose, TicketSpec, Tickets, Want};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{Argon2idHasher, LocalPasswordProvider, PasswordHasher};
use ordo_identity::{
    CredentialStore, DirectoryStore, PrincipalId, PrincipalKind, ProviderRegistry,
    SessionRevocations, Timestamp, TokenKey,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"signin-enrolment-test-secret";

/// The stand-in Discord, for the provider way of admission.
mod oauth2_support;

fn real_now() -> u64 {
    ordo_host::runtime::now_secs()
}

fn cfg(db: &str) -> ordo_exec::PoolConfig {
    ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 3,
        ..ordo_exec::PoolConfig::from_env()
    }
}

struct H {
    app: axum::Router,
    pool: ordo_exec::Pool,
    admission: Arc<Admission>,
    ops: Arc<CredentialOps>,
    directory: Arc<PgDirectoryStore>,
    credentials: Arc<ordo_host::credentials::PgCredentialStore>,
    revocations: Arc<PgSessionRevocations>,
    /// Added to the clock the routes read, so a test can move past the second
    /// in which an ended-sessions epoch was written.
    skew: Arc<AtomicU64>,
    space: String,
    fake: oauth2_support::FakeDiscord,
}

async fn harness(db: &str) -> H {
    let f = Fixture::load().expect("fixture");
    f.reset(&Pg::named(db)).expect("seeded");
    let pool = ordo_exec::Pool::connect(cfg(db)).await.expect("pool");
    ordo_host::principal_directory::ensure_table(&pool)
        .await
        .expect("directory");
    ordo_host::credentials::ensure_table(&pool)
        .await
        .expect("credentials");
    ordo_host::sessions::ensure_table(&pool)
        .await
        .expect("sessions");
    ordo_host::tickets::ensure_table(&pool)
        .await
        .expect("tickets");
    ordo_host::credential_ops::ensure_table(&pool)
        .await
        .expect("credential ops");
    ordo_host::admission::ensure_table(&pool)
        .await
        .expect("admission");
    for t in [
        "$authenticator",
        "$principal",
        "$credential",
        "$ticket",
        "$managed",
        "$temporarypassword",
        "$principalepoch",
        "$sessionrevocation",
        "$admissionpolicy",
    ] {
        pool.rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("clean");
    }
    pool.rows("DELETE FROM \"members\" WHERE \"principal\" LIKE 'usr_%'")
        .await
        .expect("clean members");

    let stream: Arc<ordo_secevent::Recorder> = Arc::default();
    let members = Arc::new(MemberOps {
        control: pool.clone(),
        pools: Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        directory: None,
        stream: Some(stream.clone()),
        refresh: None,
        now: Arc::new(real_now),
        managed: Some(ManagedStore::new(pool.clone())),
    });
    let credentials =
        Arc::new(ordo_host::credentials::PgCredentialStore::open(cfg(db)).expect("credentials"));
    let directory = Arc::new(PgDirectoryStore::open(cfg(db)).expect("directory"));
    let tickets = Arc::new(
        Tickets::new(Arc::new(PgTicketStore::open(cfg(db)).expect("tickets")))
            .with_recorder(stream.clone()),
    );
    let revocations = Arc::new(
        PgSessionRevocations::open(pool.clone())
            .await
            .expect("revocations"),
    );
    let ops = Arc::new(CredentialOps {
        members: members.clone(),
        directory: directory.clone(),
        credentials: credentials.clone(),
        hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        tickets: tickets.clone(),
        managed: ManagedStore::new(pool.clone()),
        revocations: revocations.clone(),
        session_max_secs: 12 * 3600,
        allow_temporary: true,
        control: pool.clone(),
        after_redeem: Default::default(),
    });
    let admission = Arc::new(Admission {
        members,
        store: PolicyStore::new(pool.clone()),
        tickets: Some(tickets.clone()),
        ceiling: Ceiling::everything(),
        home: None,
    });

    let mut registry = ProviderRegistry::new();
    registry
        .register(
            "password",
            Box::new(LocalPasswordProvider::with_store(
                Argon2idHasher::weak_for_tests(),
                credentials.clone(),
            )),
        )
        .expect("password");
    let fake = oauth2_support::FakeDiscord::start().await;
    let discord = fake.provider(
        "discord",
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
    );
    registry
        .register("discord", Box::new(discord.clone()))
        .expect("discord");
    let skew = Arc::new(AtomicU64::new(0));
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, directory.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(stream)
    .with_oauth2(discord);
    let s = skew.clone();
    state.now = Box::new(move || real_now() + s.load(Ordering::SeqCst));
    state.wire_enrolment(Enrolment {
        tickets: Some(tickets),
        admission: Some(admission.clone()),
        credentials: Some(ops.clone()),
    });
    H {
        app: ordo_host::auth::router(Some(Arc::new(state))),
        pool,
        admission,
        ops,
        directory,
        credentials,
        revocations,
        skew,
        space: ordo_conformance::fixture_space().0,
        fake,
    }
}

async fn post(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
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

impl H {
    /// A password identity that already exists, with no principal until it
    /// first signs in.
    fn enrol_password(&self, identifier: &str, password: &str) {
        self.credentials
            .put(identifier, &Argon2idHasher::weak_for_tests().hash(password))
            .expect("enrolled");
    }

    /// Sign in with a password, naming whatever `extra` adds.
    async fn sign_in(&self, identifier: &str, password: &str, extra: Value) -> (StatusCode, Value) {
        let (_, begun) = post(
            &self.app,
            "/auth/begin",
            json!({"method": "password", "identifier": identifier}),
        )
        .await;
        let mut body = json!({
            "method": "password", "kind": "password", "challenge": begun["challenge"],
            "identifier": identifier, "password": password, "display": "Test"});
        for (k, v) in extra.as_object().expect("object") {
            body[k] = v.clone();
        }
        post(&self.app, "/auth/complete", body).await
    }

    fn principal_of(&self, token: &str) -> PrincipalId {
        ordo_identity::verify(
            &TokenKey::from_secret(SECRET),
            token,
            Timestamp(real_now() + self.skew.load(Ordering::SeqCst)),
        )
        .expect("a token this deployment minted")
        .principal
    }

    async fn role_of(&self, principal: &PrincipalId) -> Option<String> {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"role\" FROM \"members\" WHERE \"$space\" = '{}' AND \"principal\" = '{}'",
                self.space, principal.0
            ))
            .await
            .expect("read");
        rows.first().and_then(|r| r[0].clone())
    }

    async fn set_policy(&self, change: PolicyChange) {
        self.admission
            .set_policy(&self.space, &change, "mem_o", real_now())
            .await
            .expect("policy set");
    }

    fn ticket(
        &self,
        purpose: Purpose,
        principal: Option<&PrincipalId>,
        space: Option<&str>,
    ) -> String {
        self.ops
            .tickets
            .issue(TicketSpec {
                purpose,
                principal: principal.cloned(),
                display: "Guest".into(),
                space: space.map(str::to_string),
                role: space.map(|_| "editor".to_string()),
                issued_by: "mem_o".into(),
                ttl_secs: 0,
            })
            .expect("issued")
            .ticket
    }

    fn person(&self, name: &str) -> PrincipalId {
        self.directory
            .create_principal(PrincipalKind::Human, name, Timestamp(real_now()))
            .expect("minted")
    }
}

// ══════════════ hook 1 · a sign-in that names a tenant ═══════════════════

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_that_names_a_tenant_runs_the_tenants_admission() {
    let h = harness("ordo_signin_arrival").await;
    h.enrol_password("ana@example.test", "correct-horse");

    // No tenant named: a sign-in, and nothing about admission.
    let (status, done) = h
        .sign_in("ana@example.test", "correct-horse", json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(done.get("admission").is_none());
    let ana = h.principal_of(done["token"].as_str().expect("token"));

    // A tenant that chose nothing admits nobody. The sign-in still stands.
    let (status, done) = h
        .sign_in(
            "ana@example.test",
            "correct-horse",
            json!({"space": h.space}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(done["token"].is_string(), "signed in to the deployment");
    assert_eq!(done["admission"]["outcome"], "refused");
    assert_eq!(h.role_of(&ana).await, None);

    // Opened by the app developer, it admits a stranger as a viewer.
    h.set_policy(PolicyChange {
        open: Some(true),
        ..Default::default()
    })
    .await;
    let (_, done) = h
        .sign_in(
            "ana@example.test",
            "correct-horse",
            json!({"space": h.space}),
        )
        .await;
    assert_eq!(done["admission"]["outcome"], "admitted");
    assert_eq!(done["admission"]["space"], json!(h.space));
    assert_eq!(h.role_of(&ana).await.as_deref(), Some("viewer"));

    // The second arrival is the same member.
    let (_, done) = h
        .sign_in(
            "ana@example.test",
            "correct-horse",
            json!({"space": h.space}),
        )
        .await;
    assert_eq!(done["admission"]["outcome"], "already-member");

    // A tenant that does not exist is the same answer as one that refuses.
    h.enrol_password("ben@example.test", "another-horse");
    let (status, done) = h
        .sign_in(
            "ben@example.test",
            "another-horse",
            json!({"space": "no_such_tenant"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["admission"]["outcome"], "refused");
}

/// A Discord sign-in naming the fixture's tenant, as the person `id` whose
/// address is `email`, verified or not.
async fn discord(h: &H, code: &str, id: &str, email: &str, verified: bool) -> (StatusCode, Value) {
    let mut p = oauth2_support::nelly();
    p["id"] = json!(id);
    p["email"] = json!(email);
    p["verified"] = json!(verified);
    let (state, binder) = oauth2_support::begin(&h.app, "discord").await;
    h.fake.authorize(code, p);
    let location = oauth2_support::callback(&h.app, "discord", &state, code).await;
    let handoff = oauth2_support::param(&location, "handoff").expect("a handoff");
    oauth2_support::redeem(
        &h.app,
        "discord",
        &handoff,
        &binder,
        json!({"space": h.space}),
    )
    .await
}

/// The provider way reads the connection the session came by (`via`): a
/// tenant that admits Discord sign-ins admits one at the rule's role, a
/// password sign-in to the same tenant is refused, and a rule that names a
/// domain admits only an address Discord marked verified in that domain.
#[tokio::test(flavor = "multi_thread")]
async fn the_provider_way_admits_by_the_connection_the_session_came_by() {
    use ordo_host::admission::ProviderRule;
    let h = harness("ordo_signin_provider_way").await;
    h.set_policy(PolicyChange {
        providers: Some(vec![ProviderRule {
            provider: "oauth2:discord".into(),
            domain: Some("example.com".into()),
            role: "editor".into(),
        }]),
        ..Default::default()
    })
    .await;
    let (status, done) = discord(&h, "c1", "3001", "nelly@example.com", true).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["via"], "oauth2:discord");
    assert_eq!(done["admission"]["outcome"], "admitted", "{done}");
    let nelly = h.principal_of(done["token"].as_str().unwrap());
    assert_eq!(h.role_of(&nelly).await.as_deref(), Some("editor"));

    // The same domain, unverified: no address, so the rule does not match.
    let (_, done) = discord(&h, "c2", "3002", "bea@example.com", false).await;
    assert_eq!(done["admission"]["outcome"], "refused", "{done}");
    // Another domain, verified: not this rule's.
    let (_, done) = discord(&h, "c3", "3003", "cy@elsewhere.org", true).await;
    assert_eq!(done["admission"]["outcome"], "refused", "{done}");

    // A password sign-in is not a Discord arrival.
    h.enrol_password("dee@example.com", "correct-horse");
    let (_, done) = h
        .sign_in(
            "dee@example.com",
            "correct-horse",
            json!({"space": h.space}),
        )
        .await;
    assert_eq!(done["admission"]["outcome"], "refused", "{done}");
}

/// An invitation is honoured at the role it names, and one for another
/// tenant is refused **without being spent**.
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_presented_at_sign_in_admits_at_its_role_and_a_wrong_one_is_not_spent() {
    let h = harness("ordo_signin_invitation").await;
    h.enrol_password("ben@example.test", "another-horse");
    let (_, first) = h
        .sign_in("ben@example.test", "another-horse", json!({}))
        .await;
    let ben = h.principal_of(first["token"].as_str().expect("token"));

    let elsewhere = h.ticket(Purpose::Invitation, None, Some("some_other_tenant"));
    let (_, done) = h
        .sign_in(
            "ben@example.test",
            "another-horse",
            json!({"space": h.space, "invitation": elsewhere}),
        )
        .await;
    assert_eq!(done["admission"]["outcome"], "refused");
    assert_eq!(h.role_of(&ben).await, None);
    assert!(
        h.ops
            .tickets
            .peek(
                &elsewhere,
                &Want {
                    purpose: Some(Purpose::Invitation),
                    ..Default::default()
                }
            )
            .is_ok(),
        "a refusal does not spend the ticket"
    );

    let ticket = h.ticket(Purpose::Invitation, None, Some(&h.space.clone()));
    let (_, done) = h
        .sign_in(
            "ben@example.test",
            "another-horse",
            json!({"space": h.space, "invitation": ticket}),
        )
        .await;
    assert_eq!(done["admission"]["outcome"], "admitted");
    assert_eq!(h.role_of(&ben).await.as_deref(), Some("editor"));
}

// ══════════════ hook 2 · a password ticket at the door ═══════════════════

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_redeemed_at_the_door_sets_a_first_password() {
    let h = harness("ordo_signin_redeem").await;
    let person = h.person("Newbie");
    let ticket = h.ticket(Purpose::Password, Some(&person), None);

    let (status, _) = post(
        &h.app,
        "/auth/password/redeem",
        json!({"ticket": ticket, "identifier": "newbie@example.test", "password": "first-pass"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, done) = h
        .sign_in("newbie@example.test", "first-pass", json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        h.principal_of(done["token"].as_str().expect("token")),
        person,
        "the person the ticket named"
    );

    // Spent: the same ticket, and a wrong or missing field, are the one answer.
    let (status, _) = post(
        &h.app,
        "/auth/password/redeem",
        json!({"ticket": ticket, "identifier": "again@example.test", "password": "second-pass"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post(
        &h.app,
        "/auth/password/redeem",
        json!({"ticket": "tkt_nothing", "identifier": "x", "password": "y"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post(&h.app, "/auth/password/redeem", json!({"ticket": ticket})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A taken identifier is told to the holder of a valid ticket, and costs
    // nothing: the ticket works for another identifier afterwards.
    let other = h.person("Other");
    let second = h.ticket(Purpose::Password, Some(&other), None);
    let (status, told) = post(
        &h.app,
        "/auth/password/redeem",
        json!({"ticket": second, "identifier": "newbie@example.test", "password": "pw"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(told["error"]["code"], "identifier_taken");
    let (status, _) = post(
        &h.app,
        "/auth/password/redeem",
        json!({"ticket": second, "identifier": "other@example.test", "password": "pw"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

// ═════════ hook 3 · a temporary password must be changed ═════════════════

#[tokio::test(flavor = "multi_thread")]
async fn a_temporary_password_cannot_sign_in_until_its_holder_chooses_their_own() {
    let h = harness("ordo_signin_must_change").await;
    let person = h.person("Temp");
    let identifier = "temp@example.test";
    h.enrol_password(identifier, "typed-by-an-owner");
    h.directory
        .insert_link(
            &ordo_identity::AuthenticatorId(format!("password:{identifier}")),
            &h.directory
                .principal(&person)
                .expect("read")
                .expect("exists"),
            Timestamp(real_now()),
        )
        .expect("linked");
    ManagedStore::new(h.pool.clone())
        .set_temporary(&person.0, "mem_o", real_now())
        .await
        .expect("marked");

    // The right password, and no session.
    let (status, refused) = h.sign_in(identifier, "typed-by-an-owner", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["error"]["code"], "password_change_required");
    assert!(refused.get("token").is_none(), "{refused}");
    // A wrong password is the ordinary refusal, and says nothing of this.
    let (status, wrong) = h.sign_in(identifier, "not-it", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!wrong.to_string().contains("change"));

    // Changing needs the current password, and a different new one.
    for (current, new) in [
        ("not-it", "chosen-by-me"),
        ("typed-by-an-owner", "typed-by-an-owner"),
        ("typed-by-an-owner", ""),
    ] {
        let (status, _) = post(
            &h.app,
            "/auth/password/change",
            json!({"identifier": identifier, "password": current, "new_password": new}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{current} -> {new}");
    }
    let (status, _) = post(
        &h.app,
        "/auth/password/change",
        json!({"identifier": "nobody@example.test", "password": "x", "new_password": "y"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an unknown identifier is no different"
    );

    let started = Timestamp(real_now());
    let (status, _) = post(
        &h.app,
        "/auth/password/change",
        json!({"identifier": identifier, "password": "typed-by-an-owner",
               "new_password": "chosen-by-me"}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Every session the person had ended, so one that began before the
    // change no longer counts, and one that begins after it does.
    assert!(h.revocations.principal_ended(&person, started));
    h.skew.fetch_add(5, Ordering::SeqCst);
    let (status, old) = h.sign_in(identifier, "typed-by-an-owner", json!({})).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the typed password is gone: {old}"
    );
    let (status, done) = h.sign_in(identifier, "chosen-by-me", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        h.principal_of(done["token"].as_str().expect("token")),
        person
    );
    assert!(
        !ManagedStore::new(h.pool.clone())
            .must_change(&person.0)
            .await
            .expect("readable"),
        "the marker is cleared"
    );
}

/// Without credential ops wired, neither password route answers and a
/// password sign-in is not held up.
#[tokio::test(flavor = "multi_thread")]
async fn the_password_routes_are_the_one_answer_where_nothing_is_wired() {
    let mut registry = ProviderRegistry::new();
    registry
        .register(
            "password",
            Box::new(LocalPasswordProvider::new(Argon2idHasher::weak_for_tests())),
        )
        .expect("password");
    let app = ordo_host::auth::router(Some(Arc::new(AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    ))));
    for path in ["/auth/password/redeem", "/auth/password/change"] {
        let (status, _) = post(
            &app,
            path,
            json!({"ticket": "t", "identifier": "i", "password": "p", "new_password": "n"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
}
