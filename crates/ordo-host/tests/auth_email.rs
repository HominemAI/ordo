//! Sign-in with an emailed code (`plan-auth-federated.md` step 5, §7),
//! through the real routes. The outbox is the in-memory queue and the mail
//! worker is played by calling the provider's composer, as `mail.send` does,
//! so each test can read the message it would have sent. The Postgres stores
//! and the real outbox are `email_outbox.rs`.
//!
//! What it proves, by the names the plan gives and a few more:
//!
//! - a code signs in once, and only in the browser that asked (E4, E12);
//! - wrong, expired, used and another browser's codes are the one answer;
//! - the fifth wrong code ends the ceremony and the address still works (E3);
//! - `begin` answers the same for a known and an unknown address (E10, E11);
//! - the per-address, per-device and deployment limits (E7, E8, E9), none of
//!   which reads a client's address (E6, E14 withdrawn);
//! - a deleted account is mailed nothing (D4, E16);
//! - a duplicate delivery still signs in at most once;
//! - the human check, when on, stops a `begin` before anything is queued.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_host::auth::AuthState;
use ordo_host::email_code::{
    EmailCodePolicy, EmailCodeProvider, MemoryEmailCodeStore, Registered, MAIL_KIND, SUBJECT,
};
use ordo_host::mail::{Composer, MailJob, MemoryMailQueue, QueuedMail};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{
    AuthenticatorId, DirectoryStore, MemoryDirectoryStore, MemorySessionRevocations,
    ProviderRegistry, Timestamp, TokenKey,
};
use ordo_secevent::EventKind;
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"auth-email-test-secret-of-32-bytes!";
const NOW: u64 = 1_800_000_000;
const KIM: &str = "kim@example.org";

struct H {
    auth: Arc<AuthState>,
    app: axum::Router,
    email: Arc<EmailCodeProvider>,
    queue: Arc<MemoryMailQueue>,
    codes: Arc<MemoryEmailCodeStore>,
    dir: Arc<MemoryDirectoryStore>,
    clock: Arc<AtomicU64>,
}

fn harness_with(
    policy: EmailCodePolicy,
    provisioning: Provisioning,
    human: Option<ordo_host::auth::human::HumanCheck>,
) -> H {
    let queue = Arc::new(MemoryMailQueue::default());
    let codes = Arc::new(MemoryEmailCodeStore::new());
    let dir = Arc::new(MemoryDirectoryStore::new());
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let email = Arc::new(
        EmailCodeProvider::new(
            SECRET,
            policy,
            codes.clone(),
            queue.clone(),
            dir.clone(),
            provisioning,
        )
        .with_recorder(recorder.clone()),
    );
    let mut registry = ProviderRegistry::new();
    registry
        .register("email", Box::new(Registered(email.clone())))
        .expect("registered");
    let mut state = AuthState::new(
        registry,
        Directory::with_store(provisioning, dir.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(recorder)
    .with_email(email.clone());
    if let Some(check) = human {
        state = state.with_human_check(check);
    }
    let clock = Arc::new(AtomicU64::new(NOW));
    let read = clock.clone();
    state.now = Box::new(move || read.load(Ordering::SeqCst));
    let auth = Arc::new(state);
    let app = ordo_host::auth::router(Some(auth.clone()));
    H {
        auth,
        app,
        email,
        queue,
        codes,
        dir,
        clock,
    }
}

fn harness() -> H {
    harness_with(EmailCodePolicy::default(), Provisioning::AutoOnboard, None)
}

fn now(h: &H) -> u64 {
    h.clock.load(Ordering::SeqCst)
}

async fn post(h: &H, path: &str, body: Value, headers: &[(&str, &str)]) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let res = h
        .app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).expect("request"))
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

/// `begin` as a page sends it. Answers the challenge and the binder.
async fn begin(h: &H, address: &str) -> (String, String) {
    let (status, body) = post(
        h,
        "/auth/begin",
        json!({"method": "email", "identifier": address}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (
        body["challenge"].as_str().expect("challenge").to_string(),
        body["binder"].as_str().expect("binder").to_string(),
    )
}

async fn complete(h: &H, challenge: &str, binder: &str, code: &str) -> (StatusCode, Value) {
    complete_with(h, challenge, binder, code, json!({})).await
}

async fn complete_with(
    h: &H,
    challenge: &str,
    binder: &str,
    code: &str,
    extra: Value,
) -> (StatusCode, Value) {
    let mut body = json!({
        "method": "email", "kind": "email_code",
        "challenge": challenge, "binder": binder, "code": code,
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        body[k] = v;
    }
    post(h, "/auth/complete", body, &[]).await
}

/// What the mail worker would send now: every queued job composed, in
/// order. `None` where the composer decided nothing was owed.
fn deliver(h: &H) -> Vec<(String, Option<String>)> {
    let jobs = h.queue.take();
    jobs.iter()
        .enumerate()
        .map(|(i, (job, _))| (job.to().as_str().to_string(), compose(h, job, i as i64)))
        .collect()
}

/// Compose one job as `mail.send` does, and read the code out of the text.
fn compose(h: &H, job: &MailJob, id: i64) -> Option<String> {
    assert_eq!(job.kind(), MAIL_KIND);
    let queued = QueuedMail {
        id,
        kind: job.kind().to_string(),
        to: job.to().clone(),
        payload: job.payload().to_string(),
        attempt: 1,
        queued_at: now(h),
    };
    let msg = h.email.compose(&queued, now(h)).expect("composed")?;
    assert_eq!(msg.subject(), SUBJECT);
    assert_eq!(msg.to().as_str(), job.to().as_str());
    Some(code_in(msg.text()))
}

fn code_in(text: &str) -> String {
    let start = text.find("code is ").expect("the code line") + "code is ".len();
    text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

/// Begin, deliver, and answer the code that was sent.
async fn sign_in(h: &H, address: &str) -> (StatusCode, Value) {
    let (challenge, binder) = begin(h, address).await;
    let sent = deliver(h);
    let code = sent[0].1.clone().expect("a code was sent");
    complete(h, &challenge, &binder, &code).await
}

fn wrong(code: &str) -> String {
    let flipped = if code.starts_with('0') { '1' } else { '0' };
    format!("{flipped}{}", &code[1..])
}

fn events(h: &H) -> Vec<EventKind> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 1_000_000))
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

fn failure_reasons(h: &H) -> Vec<String> {
    events(h)
        .into_iter()
        .filter_map(|k| match k {
            EventKind::AuthenticationFailed { reason } => Some(reason),
            _ => None,
        })
        .collect()
}

fn principal_of(token: &str) -> ordo_identity::TokenClaims {
    ordo_identity::verify(&TokenKey::from_secret(SECRET), token, Timestamp(NOW))
        .expect("a token this deployment minted")
}

fn refused(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "authentication_failed", "{body}");
    assert!(body.get("token").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_signs_in_once() {
    let h = harness();
    let (challenge, binder) = begin(&h, " Kim@Example.ORG ").await;
    let sent = deliver(&h);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].0, KIM,
        "the address is normalised before it is mailed"
    );
    let code = sent[0].1.clone().expect("a code");
    assert_eq!(code.len(), 8, "eight digits by default");

    let (status, done) = complete(&h, &challenge, &binder, &code).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["via"], "email-otp");
    let claims = principal_of(done["token"].as_str().expect("token"));
    assert_eq!(
        h.dir
            .principal_for(&AuthenticatorId(format!("email:{KIM}")))
            .unwrap(),
        Some(claims.principal.clone()),
        "the address is linked as `email:<address>`"
    );
    assert!(
        claims.user_presence_at.is_none(),
        "email carries no presence"
    );
    assert!(
        done["device"].is_string(),
        "the browser is given a device marker"
    );

    // The same code, the same browser: the ceremony is spent.
    let (status, again) = complete(&h, &challenge, &binder, &code).await;
    refused(status, &again);
    assert_eq!(h.codes.open_ceremonies(), 0);

    // A second sign-in resolves to the same person.
    let (status, back) = sign_in(&h, KIM).await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert_eq!(
        principal_of(back["token"].as_str().unwrap()).principal,
        claims.principal
    );
}

/// E4: the code alone, read from a mailbox and typed in another browser,
/// signs nobody in, and does not burn the real browser's ceremony.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_without_the_asking_browser_is_refused() {
    let h = harness();
    let (challenge, binder) = begin(&h, KIM).await;
    let code = deliver(&h)[0].1.clone().unwrap();
    let (_, other_binder) = begin(&h, "someone@example.net").await;
    deliver(&h);

    for _ in 0..10 {
        let (status, body) = complete(&h, &challenge, &other_binder, &code).await;
        refused(status, &body);
    }
    let (status, body) = complete(&h, &challenge, "", &code).await;
    refused(status, &body);
    // Ten tries from the wrong browser cost the asking browser nothing.
    let (status, done) = complete(&h, &challenge, &binder, &code).await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

/// E12: wrong, expired and unknown are one answer.
#[tokio::test(flavor = "multi_thread")]
async fn an_expired_code_and_a_wrong_code_are_the_one_answer() {
    let h = harness();
    let (challenge, binder) = begin(&h, KIM).await;
    let code = deliver(&h)[0].1.clone().unwrap();

    let (ws, wrong_body) = complete(&h, &challenge, &binder, &wrong(&code)).await;
    let (ms, malformed) = complete(&h, &challenge, &binder, "12ab").await;
    let (us, unknown) = complete(&h, "00", &binder, &code).await;
    h.clock.store(NOW + 600, Ordering::SeqCst);
    let (es, expired) = complete(&h, &challenge, &binder, &code).await;
    for (status, body) in [
        (ws, &wrong_body),
        (ms, &malformed),
        (us, &unknown),
        (es, &expired),
    ] {
        refused(status, body);
    }
    assert_eq!(wrong_body, expired);
    assert_eq!(wrong_body, unknown);
    assert_eq!(wrong_body, malformed);
    let reasons = failure_reasons(&h);
    assert!(
        reasons.contains(&"bad-credentials".to_string()),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"unknown-challenge".to_string()),
        "{reasons:?}"
    );
}

/// E3: the fifth wrong code consumes the ceremony, so the right one no
/// longer works, and a new `begin` for the same address does.
#[tokio::test(flavor = "multi_thread")]
async fn five_wrong_codes_end_the_ceremony_and_leave_the_address_usable() {
    let h = harness();
    let (challenge, binder) = begin(&h, KIM).await;
    let code = deliver(&h)[0].1.clone().unwrap();
    for _ in 0..5 {
        let (status, body) = complete(&h, &challenge, &binder, &wrong(&code)).await;
        refused(status, &body);
    }
    assert_eq!(h.codes.open_ceremonies(), 0, "the fifth failure ended it");
    let (status, body) = complete(&h, &challenge, &binder, &code).await;
    refused(status, &body);

    let (status, done) = sign_in(&h, KIM).await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

/// E10, E11: the answer to `begin` has the same shape for an address that
/// has an account and one that does not, and both queue exactly one job.
/// The difference is decided by the worker, out of the caller's sight.
#[tokio::test(flavor = "multi_thread")]
async fn begin_answers_the_same_for_a_known_and_an_unknown_address() {
    let h = harness_with(EmailCodePolicy::default(), Provisioning::InviteOnly, None);
    // Kim has an account; nobody else may be onboarded.
    {
        let mut dir = h.auth.directory.lock().unwrap();
        let p = dir
            .create_principal(ordo_identity::PrincipalKind::Human, "Kim", Timestamp(NOW))
            .unwrap();
        dir.link_first(&p, &AuthenticatorId(format!("email:{KIM}")), Timestamp(NOW))
            .unwrap();
    }
    let ask = |a: &'static str| {
        post(
            &h,
            "/auth/begin",
            json!({"method": "email", "identifier": a}),
            &[],
        )
    };
    let (ks, known) = ask(KIM).await;
    let (us, unknown) = ask("stranger@example.org").await;
    assert_eq!((ks, us), (StatusCode::OK, StatusCode::OK));
    let keys = |v: &Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(keys(&known), keys(&unknown));
    assert_eq!(keys(&known), vec!["binder", "challenge", "method"]);
    assert_eq!(known["method"], unknown["method"]);
    for v in [&known, &unknown] {
        assert_eq!(v["challenge"].as_str().unwrap().len(), 64);
        assert_eq!(v["binder"].as_str().unwrap().len(), 64);
    }
    let _ = failure_reasons(&h);

    let sent = deliver(&h);
    assert_eq!(sent.len(), 2, "both begins queued one job");
    assert!(sent[0].1.is_some(), "the known address is mailed");
    assert!(sent[1].1.is_none(), "the unknown one is not (§7.5 row 3)");
    assert_eq!(failure_reasons(&h), vec!["email-not-admitted".to_string()]);
}

/// E7, E10: five messages an hour to one address; the sixth `begin` is
/// answered as usual and nothing is sent.
#[tokio::test(flavor = "multi_thread")]
async fn the_sixth_message_in_an_hour_to_one_address_is_not_sent() {
    let h = harness();
    // Aligned to the start of an hour so the five land in one window.
    h.clock.store(NOW - NOW % 3600, Ordering::SeqCst);
    for i in 0..5 {
        begin(&h, KIM).await;
        assert!(deliver(&h)[0].1.is_some(), "message {} is sent", i + 1);
    }
    let (status, body) = post(
        &h,
        "/auth/begin",
        json!({"method": "email", "identifier": KIM}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(deliver(&h)[0].1.is_none(), "the sixth is not");
    assert!(failure_reasons(&h).contains(&"email-limit-recipient".to_string()));
    // Another address is not affected.
    begin(&h, "lee@example.org").await;
    assert!(deliver(&h)[0].1.is_some());
    // The next hour, Kim is mailed again.
    h.clock.fetch_add(3600, Ordering::SeqCst);
    begin(&h, KIM).await;
    assert!(deliver(&h)[0].1.is_some());
}

/// E7: the daily limit, as its own setting.
#[tokio::test(flavor = "multi_thread")]
async fn the_daily_limit_holds_across_hours() {
    let policy = EmailCodePolicy {
        per_hour: 2,
        per_day: 3,
        ..EmailCodePolicy::default()
    };
    let h = harness_with(policy, Provisioning::AutoOnboard, None);
    h.clock.store(NOW - NOW % 86_400, Ordering::SeqCst);
    let mut sent = 0;
    for _hour in 0..3 {
        for _ in 0..2 {
            begin(&h, KIM).await;
            sent += usize::from(deliver(&h)[0].1.is_some());
        }
        h.clock.fetch_add(3600, Ordering::SeqCst);
    }
    assert_eq!(sent, 3, "three a day, whatever the hourly allowance");
}

/// E8: a browser that signed in before has its own lane, which the
/// per-address limit does not consume, so a stranger spending Kim's
/// allowance does not lock her out of her own device.
#[tokio::test(flavor = "multi_thread")]
async fn a_returning_device_has_its_own_lane() {
    let h = harness();
    h.clock.store(NOW - NOW % 3600, Ordering::SeqCst);
    let (_, done) = sign_in(&h, KIM).await;
    let marker = done["device"].as_str().expect("a marker").to_string();
    // Somebody else spends the rest of Kim's hour.
    for _ in 0..4 {
        begin(&h, KIM).await;
        deliver(&h);
    }
    begin(&h, KIM).await;
    assert!(deliver(&h)[0].1.is_none(), "the address lane is spent");

    // Kim's own browser, with its marker: three more this hour.
    let mut lane = Vec::new();
    for _ in 0..4 {
        let (status, body) = post(
            &h,
            "/auth/begin",
            json!({"method": "email", "identifier": KIM, "device": marker}),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        lane.push(deliver(&h)[0].1.is_some());
    }
    assert_eq!(lane, vec![true, true, true, false]);
    assert!(failure_reasons(&h).contains(&"email-limit-device".to_string()));

    // A marker is bound to the address it was given for.
    h.clock.fetch_add(3600, Ordering::SeqCst);
    for _ in 0..5 {
        begin(&h, "lee@example.org").await;
        deliver(&h);
    }
    post(
        &h,
        "/auth/begin",
        json!({"method": "email", "identifier": "lee@example.org", "device": marker}),
        &[],
    )
    .await;
    assert!(
        deliver(&h)[0].1.is_none(),
        "Kim's marker is no lane for Lee"
    );
}

/// E9: the deployment's hourly ceiling stops every send and says so in the
/// security stream.
#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_ceiling_stops_sends_and_says_so() {
    let policy = EmailCodePolicy {
        deployment_per_hour: 3,
        ..EmailCodePolicy::default()
    };
    let h = harness_with(policy, Provisioning::AutoOnboard, None);
    h.clock.store(NOW - NOW % 3600, Ordering::SeqCst);
    let mut sent = Vec::new();
    for i in 0..5 {
        begin(&h, &format!("person{i}@example.org")).await;
        sent.push(deliver(&h)[0].1.is_some());
    }
    assert_eq!(sent, vec![true, true, true, false, false]);
    let reasons = failure_reasons(&h);
    assert_eq!(
        reasons
            .iter()
            .filter(|r| *r == "email-limit-deployment")
            .count(),
        2,
        "{reasons:?}"
    );
}

/// No limit reads a client's address (owner, 2026-10-06): a forwarded
/// header changes nothing, so varying it buys no extra messages.
#[tokio::test(flavor = "multi_thread")]
async fn no_limit_reads_a_forwarded_header() {
    let h = harness();
    h.clock.store(NOW - NOW % 3600, Ordering::SeqCst);
    let mut sent = Vec::new();
    for i in 0..7 {
        let ip = format!("203.0.113.{i}");
        let (status, body) = post(
            &h,
            "/auth/begin",
            json!({"method": "email", "identifier": KIM}),
            &[
                ("x-forwarded-for", &ip),
                ("x-real-ip", &ip),
                ("forwarded", &format!("for={ip}")),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        sent.push(deliver(&h)[0].1.is_some());
    }
    assert_eq!(sent.iter().filter(|s| **s).count(), 5);
}

/// D4, E16: an address linked to a deleted account is mailed nothing, and
/// `begin` answers as usual.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_account_is_sent_no_email_code() {
    let h = harness();
    let (_, done) = sign_in(&h, KIM).await;
    let person = principal_of(done["token"].as_str().unwrap()).principal;
    h.auth
        .directory
        .lock()
        .unwrap()
        .mark_deleted(&person, Timestamp(NOW))
        .expect("deleted");
    let _ = failure_reasons(&h);

    let (status, body) = post(
        &h,
        "/auth/begin",
        json!({"method": "email", "identifier": KIM}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(deliver(&h)[0].1.is_none(), "nothing is sent");
    assert_eq!(failure_reasons(&h), vec!["identity-deleted".to_string()]);
    assert_eq!(
        h.codes.open_ceremonies(),
        0,
        "and nothing is left to guess at"
    );
    assert_eq!(
        h.dir
            .principal_for(&AuthenticatorId(format!("email:{KIM}")))
            .unwrap(),
        Some(person),
        "the address still names the deleted account"
    );
}

/// Delivery is at least once: the worker may compose one row twice. The
/// second compose mints a new code and the first stops working; the
/// ceremony signs in once, and the duplicate gives no extra guesses.
#[tokio::test(flavor = "multi_thread")]
async fn a_resend_after_a_duplicate_delivery_still_verifies_at_most_once() {
    let h = harness();
    h.clock.store(NOW - NOW % 3600, Ordering::SeqCst);
    let (challenge, binder) = begin(&h, KIM).await;
    let jobs = h.queue.take();
    let (job, _) = &jobs[0];
    let first = compose(&h, job, 1).expect("first delivery");
    // Three wrong answers before the duplicate arrives.
    for _ in 0..3 {
        complete(&h, &challenge, &binder, &wrong(&first)).await;
    }
    let second = compose(&h, job, 1).expect("the duplicate delivery");

    let (status, body) = complete(&h, &challenge, &binder, &first).await;
    refused(status, &body);
    let (status, done) = complete(&h, &challenge, &binder, &second).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (status, body) = complete(&h, &challenge, &binder, &second).await;
    refused(status, &body);

    // The duplicate was not counted again: four more messages fit this hour.
    for _ in 0..4 {
        begin(&h, KIM).await;
        assert!(deliver(&h)[0].1.is_some());
    }
    begin(&h, KIM).await;
    assert!(deliver(&h)[0].1.is_none());

    // And its attempts carried over: 3 wrong + the old code = 4 tries, so
    // one wrong answer ends a fresh duplicate-delivered ceremony at five.
    let (c2, b2) = begin_at_next_hour(&h).await;
    let jobs = h.queue.take();
    let a = compose(&h, &jobs[0].0, 2).unwrap();
    for _ in 0..4 {
        complete(&h, &c2, &b2, &wrong(&a)).await;
    }
    let b = compose(&h, &jobs[0].0, 2).unwrap();
    let (status, body) = complete(&h, &c2, &b2, &wrong(&b)).await;
    refused(status, &body);
    let (status, body) = complete(&h, &c2, &b2, &b).await;
    refused(status, &body);
}

async fn begin_at_next_hour(h: &H) -> (String, String) {
    h.clock.fetch_add(3600, Ordering::SeqCst);
    begin(h, KIM).await
}

/// E5: a new code for an address ends every older ceremony for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_code_ends_the_older_ceremony() {
    let h = harness();
    let (c1, b1) = begin(&h, KIM).await;
    let old = deliver(&h)[0].1.clone().unwrap();
    let (c2, b2) = begin(&h, KIM).await;
    let new = deliver(&h)[0].1.clone().unwrap();
    let (status, body) = complete(&h, &c1, &b1, &old).await;
    refused(status, &body);
    let (status, done) = complete(&h, &c2, &b2, &new).await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

/// §7.1: the message carries the code and nothing about the account.
#[tokio::test(flavor = "multi_thread")]
async fn the_message_holds_the_code_and_nothing_else() {
    let h = harness();
    begin(&h, KIM).await;
    let jobs = h.queue.take();
    let (job, _) = &jobs[0];
    assert_eq!(job.payload().len(), 64, "the outbox row holds the handle");
    let queued = QueuedMail {
        id: 1,
        kind: job.kind().to_string(),
        to: job.to().clone(),
        payload: job.payload().to_string(),
        attempt: 1,
        queued_at: NOW,
    };
    let msg = h.email.compose(&queued, NOW).unwrap().unwrap();
    let code = code_in(msg.text());
    assert!(!job.payload().contains(&code));
    assert!(!msg.text().contains("kim"), "{}", msg.text());
    assert!(msg.text().contains("10 minutes"), "{}", msg.text());
}

/// E15: with the human check on, a `begin` without a token is the one
/// refusal and nothing is queued.
#[tokio::test(flavor = "multi_thread")]
async fn with_turnstile_on_a_begin_without_a_token_queues_nothing() {
    let check = ordo_host::auth::human::HumanCheck::new(
        "turnstile-secret".into(),
        "https://challenges.example.invalid/siteverify",
        ordo_egress::TlsRoots::webpki(),
        ordo_egress::DenyList::categories_only(),
    )
    .expect("check");
    let h = harness_with(
        EmailCodePolicy::default(),
        Provisioning::AutoOnboard,
        Some(check),
    );
    let (status, body) = post(
        &h,
        "/auth/begin",
        json!({"method": "email", "identifier": KIM}),
        &[],
    )
    .await;
    refused(status, &body);
    assert!(h.queue.take().is_empty(), "nothing was queued");
    assert_eq!(h.codes.open_ceremonies(), 0, "no ceremony was filed");
    assert_eq!(failure_reasons(&h), vec!["human-check-missing".to_string()]);
}

/// A malformed address is the one refusal, and nothing is filed or queued.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_address_is_refused_before_anything_is_queued() {
    let h = harness();
    for bad in [
        "kim",
        "kim@example.org\r\nBcc: x@example.net",
        "a@b@example.org",
    ] {
        let (status, body) = post(
            &h,
            "/auth/begin",
            json!({"method": "email", "identifier": bad}),
            &[],
        )
        .await;
        refused(status, &body);
    }
    assert!(h.queue.take().is_empty());
}
