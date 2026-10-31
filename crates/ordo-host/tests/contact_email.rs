//! A verified email address on an account (owner ruling 2026-10-22),
//! through the real routes. The outbox is the in-memory queue and the mail
//! worker is played by calling the composers, as `mail.send` does.
//!
//! What it proves:
//!
//! - an address counts only after the person types the emailed code;
//! - a wrong code stores nothing, and a confirmation code signs nobody in;
//! - a change tells the old address, and so does a removal;
//! - the requirement refuses an op it covers until an address is added, and
//!   names the setting at each layer;
//! - an outside connection's verified address counts, and never replaces
//!   one the person chose;
//! - no security event carries an address;
//! - a routine's notice reaches a passkey person after they add an address
//!   (against Postgres, `the_notice_reaches_a_passkey_person_at_the_added_address`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::BoundParams;
use ordo_host::auth::AuthState;
use ordo_host::contact_email::{
    ChangedNotice, ContactEmails, Requirement, CHANGED_KIND, CHANGED_SUBJECT, REQUIRED_ENV,
    SET_POLICY,
};
use ordo_host::email_code::{
    EmailCodePolicy, EmailCodeProvider, MemoryEmailCodeStore, Registered, CONFIRM_SUBJECT,
    MAIL_KIND,
};
use ordo_host::mail::{Composer, MailJob, MemoryMailQueue, QueuedMail};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{
    MemoryDirectoryStore, MemorySessionRevocations, PrincipalId, ProviderRegistry, SessionClaim,
    Timestamp, TokenKey, TokenSpec,
};
use ordo_secevent::EventKind;
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"contact-email-test-secret-32-bytes!";
const NOW: u64 = 1_800_000_000;
/// A person who signed up with a passkey: no address Ordo knows.
const ADA: &str = "p_ada";
const OLD: &str = "ada@example.org";
const NEW: &str = "ada@example.net";

struct H {
    auth: Arc<AuthState>,
    app: axum::Router,
    email: Arc<EmailCodeProvider>,
    queue: Arc<MemoryMailQueue>,
    contact: Arc<ContactEmails>,
}

fn harness_with(required: Requirement) -> H {
    harness_full(required, true)
}

fn harness_full(required: Requirement, touch: bool) -> H {
    let queue = Arc::new(MemoryMailQueue::default());
    let codes = Arc::new(MemoryEmailCodeStore::new());
    let dir = Arc::new(MemoryDirectoryStore::new());
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let email = Arc::new(
        EmailCodeProvider::new(
            SECRET,
            EmailCodePolicy::default(),
            codes,
            queue.clone(),
            dir.clone(),
            Provisioning::AutoOnboard,
        )
        .with_recorder(recorder.clone()),
    );
    // Email sign-in is on too, so a test can try a confirmation code there.
    let mut registry = ProviderRegistry::new();
    registry
        .register("email", Box::new(Registered(email.clone())))
        .expect("registered");
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, dir),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(recorder.clone())
    .with_email(email.clone());
    state.now = Box::new(|| NOW);
    let mut contact = ContactEmails::in_memory(required)
        .with_change_touch(touch)
        .with_mail(email.clone(), queue.clone())
        .with_stream(recorder);
    contact.now = Arc::new(|| NOW);
    let contact = Arc::new(contact);
    let auth = Arc::new(state);
    auth.wire_contact(contact.clone());
    let app = ordo_host::auth::router(Some(auth.clone()));
    H {
        auth,
        app,
        email,
        queue,
        contact,
    }
}

fn harness() -> H {
    harness_with(Requirement::Off)
}

/// A session token for `principal`, as a passkey sign-in mints it.
fn token(principal: &str) -> String {
    // A leading `!` is a session with no passkey touch in it.
    let (principal, touched) = match principal.strip_prefix('!') {
        Some(p) => (p, false),
        None => (principal, true),
    };
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(NOW + 3_600),
            client: None,
            user_presence_at: touched.then_some(Timestamp(NOW)),
            capability: None,
            delegation: None,
            session: Some(SessionClaim::starting_at(Timestamp(NOW))),
        },
    )
}

async fn call(
    h: &H,
    method: &str,
    path: &str,
    body: Option<Value>,
    who: &str,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if !who.is_empty() {
        b = b.header("authorization", format!("Bearer {}", token(who)));
    }
    let body = body.map(|v| v.to_string()).unwrap_or_default();
    let res = h
        .app
        .clone()
        .oneshot(b.body(Body::from(body)).expect("request"))
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

/// What the mail worker would send now: `(kind, to, subject, text)` for each
/// queued job, composed as `mail.send` does.
fn deliver(h: &H) -> Vec<(String, String, String, String)> {
    h.queue
        .take()
        .into_iter()
        .enumerate()
        .filter_map(|(i, (job, _))| compose(h, &job, i as i64))
        .collect()
}

fn compose(h: &H, job: &MailJob, id: i64) -> Option<(String, String, String, String)> {
    let queued = QueuedMail {
        id,
        kind: job.kind().to_string(),
        to: job.to().clone(),
        payload: job.payload().to_string(),
        attempt: 1,
        queued_at: NOW,
    };
    let msg = match job.kind() {
        MAIL_KIND => h.email.compose(&queued, NOW),
        CHANGED_KIND => ChangedNotice.compose(&queued, NOW),
        other => panic!("no composer for {other}"),
    }
    .expect("composed")?;
    Some((
        job.kind().to_string(),
        msg.to().as_str().to_string(),
        msg.subject().to_string(),
        msg.text().to_string(),
    ))
}

fn code_in(text: &str) -> String {
    let start = text.find("address is ").expect("the code line") + "address is ".len();
    text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

/// Ask for a code for `address` as `who`. Answers the challenge, the binder
/// and the code that was mailed.
async fn begin(h: &H, who: &str, address: &str) -> (String, String, String) {
    let (status, body) = call(
        h,
        "POST",
        "/auth/email/begin",
        Some(json!({"address": address})),
        who,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = deliver(h);
    assert_eq!(sent.len(), 1, "one message");
    let (kind, to, subject, text) = &sent[0];
    assert_eq!(kind, MAIL_KIND);
    assert_eq!(to, address);
    assert_eq!(subject, CONFIRM_SUBJECT);
    (
        body["challenge"].as_str().expect("challenge").into(),
        body["binder"].as_str().expect("binder").into(),
        code_in(text),
    )
}

async fn complete(
    h: &H,
    who: &str,
    c: &(String, String, String),
    code: &str,
) -> (StatusCode, Value) {
    call(
        h,
        "POST",
        "/auth/email/complete",
        Some(json!({"challenge": c.0, "binder": c.1, "code": code})),
        who,
    )
    .await
}

async fn add(h: &H, who: &str, address: &str) -> Value {
    let c = begin(h, who, address).await;
    let (status, body) = complete(h, who, &c, &c.2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn wrong(code: &str) -> String {
    let flipped = if code.starts_with('0') { '1' } else { '0' };
    format!("{flipped}{}", &code[1..])
}

/// The enrolment acts recorded, as `(act, detail)`.
fn acts(h: &H) -> Vec<(String, String)> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 1_000_000))
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Enrolment { act, detail } => Some((act, detail)),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_counts_only_after_its_emailed_code() {
    let h = harness();
    let (status, read) = call(&h, "GET", "/auth/email", None, ADA).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["address"], Value::Null, "a passkey person has none");
    assert_eq!(read["required"], "off", "optional by default");
    assert_eq!(read["mail"], true);

    let c = begin(&h, ADA, OLD).await;
    let (_, read) = call(&h, "GET", "/auth/email", None, ADA).await;
    assert_eq!(read["address"], Value::Null, "nothing until the code");

    let (status, done) = complete(&h, ADA, &c, &c.2).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["address"], OLD);
    assert_eq!(done["replaced"], false);
    let (_, read) = call(&h, "GET", "/auth/email", None, ADA).await;
    assert_eq!(read["address"], OLD);
    assert_eq!(read["source"], "code");
    assert_eq!(read["verified_at"], NOW);

    // The code works once.
    let (status, again) = complete(&h, ADA, &c, &c.2).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{again}");

    let acts = acts(&h);
    assert!(
        acts.iter().any(|(a, d)| a == "contact-email-added"
            && d.contains(ADA)
            && d.contains("source=code")),
        "{acts:?}"
    );
    assert!(
        acts.iter().all(|(_, d)| !d.contains('@')),
        "no event carries an address: {acts:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_code_stores_nothing_and_a_confirmation_code_signs_nobody_in() {
    let h = harness();
    let c = begin(&h, ADA, OLD).await;
    let (status, body) = complete(&h, ADA, &c, &wrong(&c.2)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "authentication_failed");
    assert_eq!(h.contact.address_of(ADA).await.unwrap(), None);

    // Another person holding the challenge, the binder and the code gets
    // the one refusal.
    let (status, _) = complete(&h, "p_eve", &c, &c.2).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.contact.address_of("p_eve").await.unwrap(), None);

    // The sign-in route will not take a confirmation code.
    let c = begin(&h, ADA, OLD).await;
    let (status, body) = call(
        &h,
        "POST",
        "/auth/complete",
        Some(json!({"method": "email", "kind": "email_code",
            "challenge": c.0, "binder": c.1, "code": c.2})),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.get("token").is_none());

    // No session, no route.
    let (status, _) = call(
        &h,
        "POST",
        "/auth/email/begin",
        Some(json!({"address": OLD})),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_tells_the_old_address_and_so_does_a_removal() {
    let h = harness();
    add(&h, ADA, OLD).await;
    acts(&h);

    let done = add(&h, ADA, NEW).await;
    assert_eq!(done["replaced"], true);
    let told = deliver(&h);
    assert_eq!(told.len(), 1, "{told:?}");
    let (kind, to, subject, text) = &told[0];
    assert_eq!((kind.as_str(), to.as_str()), (CHANGED_KIND, OLD));
    assert_eq!(subject, CHANGED_SUBJECT);
    assert!(text.contains("was replaced on"), "{text}");
    assert!(
        !text.contains(NEW),
        "the new address is not told to the old"
    );
    assert_eq!(
        h.contact.address_of(ADA).await.unwrap().map(|s| s.address),
        Some(NEW.to_string())
    );
    assert!(acts(&h).iter().any(|(a, _)| a == "contact-email-changed"));

    let (status, body) = call(&h, "POST", "/auth/email/remove", None, ADA).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], true);
    let told = deliver(&h);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].1, NEW);
    assert!(told[0].3.contains("was removed from"));
    assert_eq!(h.contact.address_of(ADA).await.unwrap(), None);
    let acts = acts(&h);
    assert!(
        acts.iter().any(|(a, _)| a == "contact-email-removed"),
        "{acts:?}"
    );
    assert!(acts.iter().all(|(_, d)| !d.contains('@')), "{acts:?}");

    // Nothing left to remove.
    let (_, body) = call(&h, "POST", "/auth/email/remove", None, ADA).await;
    assert_eq!(body["removed"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_requirement_refuses_an_op_until_an_address_is_added() {
    let h = harness_with(Requirement::parse("setReminder").unwrap());
    let why = h
        .contact
        .refusal(ADA, "setReminder")
        .await
        .expect("refused without an address");
    assert!(why.contains(REQUIRED_ENV), "names the setting: {why}");
    assert!(why.contains("/auth/email/begin"), "says what to do: {why}");
    assert_eq!(
        h.contact.refusal(ADA, "createNote").await,
        None,
        "only the named op"
    );
    let (_, read) = call(&h, "GET", "/auth/email", None, ADA).await;
    assert_eq!(read["required"], json!(["setReminder"]));
    assert_eq!(read["setting"], REQUIRED_ENV);

    add(&h, ADA, OLD).await;
    assert_eq!(
        h.contact.refusal(ADA, "setReminder").await,
        None,
        "allowed now"
    );

    // The app developer's layer: `all` covers the app's ops, never a
    // platform op, and the refusal names the op that set it.
    let set = |w: &str| BoundParams(json!({"required": w}).as_object().unwrap().clone());
    let out = h
        .contact
        .execute(SET_POLICY, "home", &set("all"), "p_dev")
        .await
        .expect("set");
    assert_eq!(out.rows[0]["required"], "all");
    assert_eq!(out.rows[0]["source"], SET_POLICY);
    let why = h
        .contact
        .refusal("p_lee", "createNote")
        .await
        .expect("refused");
    assert!(why.contains(SET_POLICY), "{why}");
    assert_eq!(h.contact.refusal("p_lee", "std.space.setRole").await, None);
    // `default` hands it back to the deployment.
    h.contact
        .execute(SET_POLICY, "home", &set("default"), "p_dev")
        .await
        .expect("cleared");
    assert_eq!(h.contact.refusal("p_lee", "createNote").await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_mail_nothing_can_be_required_or_confirmed() {
    let contact = ContactEmails::in_memory(Requirement::Off);
    let set = BoundParams(json!({"required": "all"}).as_object().unwrap().clone());
    let e = contact
        .execute(SET_POLICY, "home", &set, "p_dev")
        .await
        .expect_err("refused");
    assert!(e.message.contains("ORDO_MAIL_KIND"), "{}", e.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_outside_connections_verified_address_counts_and_never_replaces_a_chosen_one() {
    let h = harness();
    assert!(h
        .contact
        .adopt("p_dee", "dee@example.org", "discord", NOW)
        .await
        .unwrap());
    let s = h
        .contact
        .address_of("p_dee")
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(
        (s.address.as_str(), s.source.as_str()),
        ("dee@example.org", "discord")
    );

    add(&h, ADA, OLD).await;
    assert!(!h
        .contact
        .adopt(ADA, "other@example.org", "discord", NOW)
        .await
        .unwrap());
    assert_eq!(
        h.contact.address_of(ADA).await.unwrap().unwrap().address,
        OLD
    );
}

/// The mail path finds an added address: a routine's `notify` queues a
/// notice for a passkey person once they confirmed one. Postgres, because
/// the notice query reads the control tables.
#[tokio::test(flavor = "multi_thread")]
async fn the_notice_reaches_a_passkey_person_at_the_added_address() {
    let pg = ordo_conformance::Pg::scoped("contact_email_notice");
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::principal_directory::ensure_table(&pool)
        .await
        .expect("directory");
    ordo_host::contact_email::ensure_table(&pool)
        .await
        .expect("contact");
    ordo_host::mail::ensure_table(&pool).await.expect("outbox");
    pool.rows(&format!(
        r#"INSERT INTO "$principal" ("id", "kind", "display", "created_at")
           VALUES ('{ADA}', 'person', 'Ada', {NOW})"#
    ))
    .await
    .expect("principal");
    pool.rows(&format!(
        r#"INSERT INTO "$authenticator" ("authenticator", "principal", "linked_at")
           VALUES ('passkey:cred-ada', '{ADA}', {NOW})"#
    ))
    .await
    .expect("passkey");

    let recipients = || async {
        pool.rows(r#"SELECT "recipient" FROM "$mailoutbox" WHERE "kind" = 'routine.notice'"#)
            .await
            .expect("outbox")
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect::<Vec<_>>()
    };
    let who = vec![ADA.to_string()];
    let n = ordo_host::routines::queue_notices(&pool, &who, "A reminder is due", NOW)
        .await
        .expect("queued");
    assert_eq!(
        n, 0,
        "a passkey person with no address is told in the app only"
    );
    assert!(recipients().await.is_empty());

    let contact = ContactEmails::on_pool(Requirement::Off, pool.clone());
    contact
        .confirm(ADA, OLD, "code", NOW)
        .await
        .expect("stored");
    assert_eq!(
        contact.address_of(ADA).await.unwrap().map(|s| s.address),
        Some(OLD.to_string())
    );
    let n = ordo_host::routines::queue_notices(&pool, &who, "A reminder is due", NOW)
        .await
        .expect("queued");
    assert_eq!(n, 1);
    assert_eq!(recipients().await, vec![OLD.to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_or_removing_an_address_asks_for_a_touch_unless_the_app_developer_turns_it_off() {
    let h = harness();
    // A first address needs no touch.
    let first = begin(&h, "!p_ada", OLD).await;
    let (status, body) = complete(&h, "!p_ada", &first, &first.2).await;
    assert_eq!(status, StatusCode::OK, "adding needs no touch: {body}");

    // Changing it does, and the refusal names the setting.
    let change = begin(&h, "!p_ada", NEW).await;
    let (status, body) = complete(&h, "!p_ada", &change, &change.2).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "presence_required");
    assert_eq!(body["error"]["setting"], "ORDO_EMAIL_CHANGE_TOUCH");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("ORDO_EMAIL_CHANGE_TOUCH"));
    let (_, read) = call(&h, "GET", "/auth/email", None, "!p_ada").await;
    assert_eq!(read["address"], OLD, "nothing changed");
    // The same code still works with a touch.
    let (status, body) = complete(&h, "p_ada", &change, &change.2).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["replaced"], true);

    // Removing it does too.
    let (status, body) = call(&h, "POST", "/auth/email/remove", None, "!p_ada").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "presence_required");
    let (status, body) = call(&h, "POST", "/auth/email/remove", None, "p_ada").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], true);

    // The app developer turns it off; its layer outranks the deployment's.
    let set = |w: &str| BoundParams(json!({"changeTouch": w}).as_object().unwrap().clone());
    let out = h
        .contact
        .execute(SET_POLICY, "home", &set("off"), "p_dev")
        .await
        .expect("set");
    assert_eq!(out.rows[0]["changeTouch"], "off");
    assert_eq!(out.rows[0]["changeTouchSource"], SET_POLICY);
    h.queue.take();
    add(&h, "!p_ada", OLD).await;
    let (status, body) = call(&h, "POST", "/auth/email/remove", None, "!p_ada").await;
    assert_eq!(status, StatusCode::OK, "no touch asked now: {body}");
    // An unknown word is refused.
    let bad = h
        .contact
        .execute(SET_POLICY, "home", &set("maybe"), "p_dev")
        .await;
    assert!(bad.is_err());
    // `default` hands it back to the deployment, which defaults to on.
    let out = h
        .contact
        .execute(SET_POLICY, "home", &set("default"), "p_dev")
        .await
        .expect("cleared");
    assert_eq!(out.rows[0]["changeTouch"], "on");
    assert_eq!(out.rows[0]["changeTouchSource"], "ORDO_EMAIL_CHANGE_TOUCH");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_can_turn_the_touch_off() {
    let h = harness_full(Requirement::Off, false);
    add(&h, "!p_ada", OLD).await;
    let (status, body) = call(&h, "POST", "/auth/email/remove", None, "!p_ada").await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_address_table_refuses_a_covered_op_and_says_to_retry() {
    let h = harness_with(Requirement::parse("setReminder").unwrap());
    add(&h, ADA, OLD).await;
    assert!(h.contact.gate(ADA, "setReminder").await.is_none());

    h.contact.set_unreadable(true);
    let gate = h
        .contact
        .gate(ADA, "setReminder")
        .await
        .expect("refused, not let through");
    assert!(gate.retryable, "a later try may pass");
    assert!(gate.message.contains(REQUIRED_ENV), "{}", gate.message);
    assert!(
        h.contact.gate(ADA, "createNote").await.is_none(),
        "an op the requirement does not cover is not held"
    );

    h.contact.set_unreadable(false);
    assert!(h.contact.gate(ADA, "setReminder").await.is_none());
    // A person with no address is refused, and that one is not retryable.
    let none = h
        .contact
        .gate("p_lee", "setReminder")
        .await
        .expect("refused");
    assert!(!none.retryable);
}
