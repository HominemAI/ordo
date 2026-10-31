//! **An invitation lets a newcomer sign up under invite-only** (owner ruling
//! 2026-10-09, federated ask 9; checklist 25ar), with every method: a
//! passkey, a password, an emailed code and an outside provider. Against
//! real Postgres.
//!
//! Each test runs under `ORDO_AUTH_PROVISIONING=invite-only`. A newcomer
//! with a valid invitation is minted and admitted at its role. Without one,
//! or with one that is spent, expired, for another tenant or for a named
//! person, they get the one refusal and nothing is stored. Two sign-ups
//! racing with one invitation spend it once.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_conformance::{Fixture, Pg};
use ordo_host::admission::{Admission, Ceiling, PolicyStore};
use ordo_host::auth::{AuthState, Enrolment};
use ordo_host::email_code::{
    EmailCodePolicy, EmailCodeProvider, MemoryEmailCodeStore, Registered, MAIL_KIND,
};
use ordo_host::mail::{Composer, MemoryMailQueue, QueuedMail};
use ordo_host::member_ops::MemberOps;
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::tickets::{hash_of, PgTicketStore, Purpose, TicketSpec, Tickets, Want};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{Argon2idHasher, CompleteRequest, LocalPasswordProvider};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    AuthenticatorId, CredentialStore, DirectoryStore, MemorySessionRevocations, PasskeyProvider,
    PrincipalId, PrincipalKind, ProviderRegistry, RelyingParty, Timestamp, TokenKey,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"auth-invitation-test-secret-of-32b!";

/// The stand-in Discord, for the outside-provider method.
mod oauth2_support;

fn now() -> u64 {
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
    tickets: Arc<Tickets>,
    directory: Arc<PgDirectoryStore>,
    credentials: Arc<ordo_host::credentials::PgCredentialStore>,
    passkeys: Arc<ordo_identity::MemoryPasskeyStore>,
    email: Arc<EmailCodeProvider>,
    queue: Arc<MemoryMailQueue>,
    space: String,
    fake: oauth2_support::FakeDiscord,
}

/// An invite-only deployment with all four methods, a tenant whose
/// admission has nothing switched on, and invitations.
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
    ordo_host::tickets::ensure_table(&pool)
        .await
        .expect("tickets");
    ordo_host::admission::ensure_table(&pool)
        .await
        .expect("admission");
    for t in [
        "$authenticator",
        "$principal",
        "$credential",
        "$ticket",
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
        now: Arc::new(now),
        managed: None,
    });
    let credentials =
        Arc::new(ordo_host::credentials::PgCredentialStore::open(cfg(db)).expect("credentials"));
    let directory = Arc::new(PgDirectoryStore::open(cfg(db)).expect("directory"));
    let tickets = Arc::new(
        Tickets::new(Arc::new(PgTicketStore::open(cfg(db)).expect("tickets")))
            .with_recorder(stream.clone()),
    );
    let admission = Arc::new(Admission {
        members,
        store: PolicyStore::new(pool.clone()),
        tickets: Some(tickets.clone()),
        ceiling: Ceiling::invitation_only(),
        home: None,
    });

    let passkeys = Arc::new(ordo_identity::MemoryPasskeyStore::new());
    let passkey = Arc::new(PasskeyProvider::with_stores(
        RelyingParty::new("example.com", &["https://app.example.com"]).expect("rp"),
        passkeys.clone(),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
    ));
    let queue = Arc::new(MemoryMailQueue::default());
    let email = Arc::new(EmailCodeProvider::new(
        SECRET,
        EmailCodePolicy::default(),
        Arc::new(MemoryEmailCodeStore::new()),
        queue.clone(),
        directory.clone(),
        Provisioning::InviteOnly,
    ));
    let fake = oauth2_support::FakeDiscord::start().await;
    let discord = fake.provider(
        "discord",
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
    );

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
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("passkey");
    registry
        .register("email", Box::new(Registered(email.clone())))
        .expect("email");
    registry
        .register("discord", Box::new(discord.clone()))
        .expect("discord");
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::InviteOnly, directory.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(stream)
    .with_passkey(passkey)
    .with_email(email.clone())
    .with_oauth2(discord);
    state.now = Box::new(now);
    state.wire_enrolment(Enrolment {
        tickets: Some(tickets.clone()),
        admission: Some(admission),
        credentials: None,
    });
    H {
        app: ordo_host::auth::router(Some(Arc::new(state))),
        pool,
        tickets,
        directory,
        credentials,
        passkeys,
        email,
        queue,
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

fn with(mut body: Value, extra: &Value) -> Value {
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        body[k] = v;
    }
    body
}

impl H {
    /// An invitation to `space`, at `editor`, for whoever holds it, or for
    /// `principal` when named.
    fn invitation(&self, space: &str, principal: Option<&PrincipalId>) -> String {
        self.tickets
            .issue(TicketSpec {
                purpose: Purpose::Invitation,
                principal: principal.cloned(),
                display: "Guest".into(),
                space: Some(space.to_string()),
                role: Some("editor".into()),
                issued_by: "mem_o".into(),
                ttl_secs: 0,
            })
            .expect("issued")
            .ticket
    }

    /// `space` and `invitation`, as a page posts them.
    fn invited(&self, ticket: &str) -> Value {
        json!({"space": self.space, "invitation": ticket})
    }

    fn unspent(&self, ticket: &str) -> bool {
        self.tickets
            .peek(
                ticket,
                &Want {
                    purpose: Some(Purpose::Invitation),
                    ..Default::default()
                },
            )
            .is_ok()
    }

    /// A password identity with no principal: what a newcomer has before
    /// their first sign-in.
    fn enrol_password(&self, identifier: &str) {
        use ordo_identity::provider::PasswordHasher;
        self.credentials
            .put(
                identifier,
                &Argon2idHasher::weak_for_tests().hash("correct-horse"),
            )
            .expect("enrolled");
    }

    async fn password(&self, identifier: &str, extra: Value) -> (StatusCode, Value) {
        let (_, begun) = post(
            &self.app,
            "/auth/begin",
            json!({"method": "password", "identifier": identifier}),
        )
        .await;
        let body = json!({
            "method": "password", "kind": "password", "challenge": begun["challenge"],
            "identifier": identifier, "password": "correct-horse", "display": "New"});
        post(&self.app, "/auth/complete", with(body, &extra)).await
    }

    async fn passkey(&self, device: &TestAuthenticator, extra: Value) -> (StatusCode, Value) {
        let (status, options) =
            post(&self.app, "/auth/register", json!({"method": "passkey"})).await;
        assert_eq!(status, StatusCode::OK, "{options}");
        let CompleteRequest::PasskeyRegistration {
            challenge,
            client_data_json,
            attestation_object,
        } = device.register(options["challenge"].as_str().expect("challenge"))
        else {
            panic!("not a registration");
        };
        let body = json!({
            "method": "passkey", "kind": "passkey_registration", "challenge": challenge.0,
            "client_data_json": b64url_encode(&client_data_json),
            "attestation_object": b64url_encode(&attestation_object),
            "display": "New",
        });
        post(&self.app, "/auth/complete", with(body, &extra)).await
    }

    /// An emailed-code sign-in: `begin` with `extra`, the worker's
    /// decision, and, when a code was mailed, `complete` with `extra`.
    /// `None` when nothing was mailed.
    async fn email(&self, address: &str, extra: Value) -> Option<(StatusCode, Value)> {
        let (status, begun) = post(
            &self.app,
            "/auth/begin",
            with(json!({"method": "email", "identifier": address}), &extra),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{begun}");
        let jobs = self.queue.take();
        assert_eq!(jobs.len(), 1, "every begin queues one job");
        let job = &jobs[0].0;
        assert_eq!(job.kind(), MAIL_KIND);
        let queued = QueuedMail {
            id: 1,
            kind: job.kind().to_string(),
            to: job.to().clone(),
            payload: job.payload().to_string(),
            attempt: 1,
            queued_at: now(),
        };
        let msg = self.email.compose(&queued, now()).expect("composed")?;
        let text = msg.text();
        let at = text.find("code is ").expect("the code line") + "code is ".len();
        let code: String = text[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let body = json!({
            "method": "email", "kind": "email_code",
            "challenge": begun["challenge"], "binder": begun["binder"], "code": code});
        Some(post(&self.app, "/auth/complete", with(body, &extra)).await)
    }

    /// A Discord sign-in as the person `id`, the page posting `extra` with
    /// the handoff.
    async fn discord(&self, code: &str, id: &str, extra: Value) -> (StatusCode, Value) {
        let mut p = oauth2_support::nelly();
        p["id"] = json!(id);
        let (state, binder) = oauth2_support::begin(&self.app, "discord").await;
        self.fake.authorize(code, p);
        let location = oauth2_support::callback(&self.app, "discord", &state, code).await;
        let handoff = oauth2_support::param(&location, "handoff").expect("a handoff");
        oauth2_support::redeem(&self.app, "discord", &handoff, &binder, extra).await
    }

    fn principal_of(&self, done: &Value) -> PrincipalId {
        ordo_identity::verify(
            &TokenKey::from_secret(SECRET),
            done["token"].as_str().expect("a token"),
            Timestamp(now()),
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

    async fn count(&self, sql: &str) -> String {
        let rows = self.pool.rows(sql).await.expect("count");
        rows[0][0].clone().expect("a count")
    }

    /// How many principals and memberships exist: nothing a refusal may add.
    async fn stored(&self) -> (String, String) {
        (
            self.count(r#"SELECT count(*) FROM "$principal""#).await,
            self.count(r#"SELECT count(*) FROM "members" WHERE "principal" LIKE 'usr_%'"#)
                .await,
        )
    }

    fn known(&self, authenticator: &str) -> bool {
        self.directory
            .principal_for(&AuthenticatorId(authenticator.to_string()))
            .expect("directory")
            .is_some()
    }
}

/// The answer a newcomer without a valid invitation gets: a wrong password
/// for an identifier nobody holds.
async fn the_one_refusal(h: &H) -> (StatusCode, Value) {
    h.password("nobody@example.test", json!({})).await
}

/// Each method's newcomer signs up with the invitation and is admitted at
/// its role, and the invitation is spent.
fn assert_admitted(done: &(StatusCode, Value)) {
    assert_eq!(done.0, StatusCode::OK, "{}", done.1);
    assert_eq!(done.1["admission"]["outcome"], "admitted", "{}", done.1);
}

// ═══════════════════════ one test per method ═════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn invite_only_a_newcomer_with_an_invitation_signs_up_with_a_password() {
    let h = harness("ordo_invite_password").await;
    h.enrol_password("ana@example.test");
    h.enrol_password("ben@example.test");

    // Without an invitation: the one refusal, and nothing stored.
    let before = h.stored().await;
    let refused = h
        .password("ben@example.test", json!({"space": h.space}))
        .await;
    assert_eq!(refused, the_one_refusal(&h).await);
    assert_eq!(h.stored().await, before, "a refusal mints nothing");

    let ticket = h.invitation(&h.space, None);
    let done = h.password("ana@example.test", h.invited(&ticket)).await;
    assert_admitted(&done);
    let ana = h.principal_of(&done.1);
    assert_eq!(h.role_of(&ana).await.as_deref(), Some("editor"));
    assert!(!h.unspent(&ticket), "the invitation is spent");

    // Ana is now known: she signs in again with no invitation.
    let again = h.password("ana@example.test", json!({})).await;
    assert_eq!(again.0, StatusCode::OK, "{}", again.1);
    assert_eq!(h.principal_of(&again.1), ana);
}

#[tokio::test(flavor = "multi_thread")]
async fn invite_only_a_newcomer_with_an_invitation_signs_up_with_a_passkey() {
    let h = harness("ordo_invite_passkey").await;

    let stranger = TestAuthenticator::es256();
    let refused = h.passkey(&stranger, json!({"space": h.space})).await;
    assert_eq!(refused, the_one_refusal(&h).await);
    assert_eq!(h.passkeys.len(), 0, "no public key was left behind");

    let ticket = h.invitation(&h.space, None);
    let device = TestAuthenticator::es256();
    let done = h.passkey(&device, h.invited(&ticket)).await;
    assert_admitted(&done);
    let who = h.principal_of(&done.1);
    assert_eq!(h.role_of(&who).await.as_deref(), Some("editor"));
    assert_eq!(h.passkeys.len(), 1, "the newcomer's key is stored");
    assert!(!h.unspent(&ticket));
}

#[tokio::test(flavor = "multi_thread")]
async fn invite_only_a_newcomer_with_an_invitation_signs_up_with_an_emailed_code() {
    let h = harness("ordo_invite_email").await;

    // Without an invitation an unknown address is mailed nothing, and the
    // answer to `begin` is the same.
    assert!(
        h.email("stranger@example.org", json!({"space": h.space}))
            .await
            .is_none(),
        "an unknown address is not mailed under invite-only"
    );
    // With an invitation that is not valid, the same.
    let elsewhere = h.invitation("some_other_tenant", None);
    assert!(h
        .email("stranger@example.org", h.invited(&elsewhere))
        .await
        .is_none());

    let ticket = h.invitation(&h.space, None);
    let done = h
        .email("nelly@example.org", h.invited(&ticket))
        .await
        .expect("an invited newcomer is mailed a code");
    assert_admitted(&done);
    let nelly = h.principal_of(&done.1);
    assert_eq!(h.role_of(&nelly).await.as_deref(), Some("editor"));
    assert!(h.known("email:nelly@example.org"));
    assert!(!h.unspent(&ticket));
}

#[tokio::test(flavor = "multi_thread")]
async fn invite_only_a_newcomer_with_an_invitation_signs_up_through_discord() {
    let h = harness("ordo_invite_discord").await;

    let before = h.stored().await;
    let refused = h.discord("c1", "4001", json!({"space": h.space})).await;
    assert_eq!(refused, the_one_refusal(&h).await);
    assert_eq!(h.stored().await, before, "a refusal mints nothing");
    assert!(!h.known("oauth2:discord:4001"));

    let ticket = h.invitation(&h.space, None);
    let done = h.discord("c2", "4002", h.invited(&ticket)).await;
    assert_admitted(&done);
    assert_eq!(done.1["via"], "oauth2:discord");
    let who = h.principal_of(&done.1);
    assert_eq!(h.role_of(&who).await.as_deref(), Some("editor"));
    assert!(!h.unspent(&ticket));
}

// ═════════════════════ invitations that do not admit ═════════════════════

/// A spent, an expired, another tenant's, and a named person's invitation
/// each get the one refusal and store nothing. One that was refused is not
/// spent by the refusal.
#[tokio::test(flavor = "multi_thread")]
async fn invite_only_a_spent_expired_other_tenant_or_named_invitation_is_the_one_refusal() {
    let h = harness("ordo_invite_refused").await;
    h.enrol_password("first@example.test");
    h.enrol_password("late@example.test");
    let one = the_one_refusal(&h).await;

    let spent = h.invitation(&h.space, None);
    assert_admitted(&h.password("first@example.test", h.invited(&spent)).await);

    let expired = h.invitation(&h.space, None);
    h.pool
        .rows(&format!(
            r#"UPDATE "$ticket" SET "expires_at" = 1 WHERE "hash" = '{}'"#,
            hash_of(&expired)
        ))
        .await
        .expect("expired");

    let elsewhere = h.invitation("some_other_tenant", None);
    let someone = h
        .directory
        .create_principal(PrincipalKind::Human, "Someone", Timestamp(now()))
        .expect("minted");
    let named = h.invitation(&h.space, Some(&someone));

    let before = h.stored().await;
    for (what, ticket) in [
        ("spent", &spent),
        ("expired", &expired),
        ("another tenant's", &elsewhere),
        ("a named person's", &named),
    ] {
        let refused = h.password("late@example.test", h.invited(ticket)).await;
        assert_eq!(refused, one, "{what} invitation");
    }
    assert_eq!(h.stored().await, before, "nothing was minted or admitted");
    assert!(!h.known("password:late@example.test"));
    assert!(h.unspent(&elsewhere), "a refusal spends nothing");
    assert!(h.unspent(&named), "a named invitation stays its person's");
}

/// Two newcomers race with one invitation: exactly one is minted and
/// admitted, and the other gets the one refusal with nothing stored.
#[tokio::test(flavor = "multi_thread")]
async fn invite_only_two_newcomers_racing_with_one_invitation_spend_it_once() {
    let h = harness("ordo_invite_race").await;
    h.enrol_password("left@example.test");
    h.enrol_password("right@example.test");
    let one = the_one_refusal(&h).await;
    let ticket = h.invitation(&h.space, None);

    let (left, right) = tokio::join!(
        h.password("left@example.test", h.invited(&ticket)),
        h.password("right@example.test", h.invited(&ticket)),
    );
    let (won, lost) = match (left.0, right.0) {
        (StatusCode::OK, _) => (left, right),
        _ => (right, left),
    };
    assert_admitted(&won);
    assert_eq!(lost, one, "the loser gets the one refusal");
    assert_eq!(
        h.count(r#"SELECT count(*) FROM "$principal""#).await,
        "1",
        "one principal was minted"
    );
    assert_eq!(
        h.count(r#"SELECT count(*) FROM "members" WHERE "principal" LIKE 'usr_%'"#)
            .await,
        "1",
        "one membership was written"
    );
}

/// A mint that fails after the invitation was spent gives the invitation
/// back. The store error is injected by hiding `$principal` (as
/// `secdrain.rs` hides its table), so the take succeeds and the mint does
/// not. Afterwards the same invitation signs the newcomer up.
#[tokio::test(flavor = "multi_thread")]
async fn invite_only_an_invitation_spent_before_a_failed_mint_is_given_back() {
    let h = harness("ordo_invite_given_back").await;
    h.enrol_password("ana@example.test");
    let ticket = h.invitation(&h.space, None);

    h.pool
        .rows(r#"ALTER TABLE "$principal" RENAME TO "$principal_hidden""#)
        .await
        .expect("hidden");
    let failed = h.password("ana@example.test", h.invited(&ticket)).await;
    h.pool
        .rows(r#"ALTER TABLE "$principal_hidden" RENAME TO "$principal""#)
        .await
        .expect("restored");
    assert_ne!(failed.0, StatusCode::OK, "{}", failed.1);
    assert!(!h.known("password:ana@example.test"), "nobody was minted");
    assert!(h.unspent(&ticket), "the invitation was given back");

    let done = h.password("ana@example.test", h.invited(&ticket)).await;
    assert_admitted(&done);
    let ana = h.principal_of(&done.1);
    assert_eq!(h.role_of(&ana).await.as_deref(), Some("editor"));
    assert!(!h.unspent(&ticket));
}
