//! Credential recovery through the real routes (`plan-passkey-recovery.md`
//! R3 onward), with a software authenticator standing in for the person's
//! hands and a pinned clock standing in for the days.
//!
//! Nothing fakes a gesture. `TestAuthenticator` holds a real key and emits
//! the bytes a real authenticator emits, and the routes verify them as they
//! would verify a person's.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::auth::AuthState;
use ordo_host::metrics::Metrics;
use ordo_host::recovery::{
    MemoryRecoveryStore, Open, OpenError, Path, Recovery, RecoveryPolicy, Settlement,
};
use ordo_host::recovery_codes::{MemoryCodeStore, RecoveryCodes};
use ordo_host::tickets::{MemoryTicketStore, Purpose, Tickets, Want};
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{Argon2idHasher, CompleteRequest, LocalPasswordProvider};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    AccessEngine, AccessProfile, MemorySessionRevocations, PasskeyProvider, PrincipalId,
    ProviderRegistry, RelyingParty, Role, SpaceId, TokenKey,
};
use ordo_secevent::{EventKind, RecordedDecision, Scope};
use serde_json::{json, Value};
use tower::ServiceExt;

const SECRET: &[u8] = b"recovery-test-secret";
const NOW: u64 = 1_000_000;
const SPACE: &str = "spc_clinic";
/// Stands for `std.space.export`: an owner-only op that asks for presence.
const GATED: &str = "space.export";
const PLAIN: &str = "records.list";
const DAY: u64 = 86_400;
/// The password challenges the harness lets wait at once.
const PASSWORD_PENDING_CAP: usize = 64;

struct Harness {
    auth: Arc<AuthState>,
    keys: Arc<ordo_identity::MemoryPasskeyStore>,
    revocations: Arc<MemorySessionRevocations>,
    recovery: Arc<Recovery>,
    tickets: Arc<Tickets>,
    codes: Arc<RecoveryCodes>,
    code_store: Arc<MemoryCodeStore>,
    /// The directory's store, which the deployment owner's ops are handed.
    store: Arc<ordo_identity::MemoryDirectoryStore>,
    /// Second factors: TOTP is wired, and a completed recovery removes them
    /// all, as a deployment does.
    factors: Arc<ordo_host::second_factor_store::MemorySecondFactorStore>,
    /// The contact email store, which a contest puts back.
    emails: Arc<ordo_host::contact_email::ContactEmails>,
    clock: Arc<AtomicU64>,
}

impl Harness {
    fn set_now(&self, at: u64) {
        self.clock.store(at, Ordering::SeqCst);
    }
    fn app(&self) -> axum::Router {
        ordo_host::auth::router(Some(self.auth.clone()))
    }
}

fn harness_with(policy: RecoveryPolicy, tenants: Vec<String>) -> Harness {
    harness_full(policy, tenants, Vec::new(), 30)
}

/// A deployment with a policy and a failure limit on wrong codes.
fn harness_codes(failures_per_minute: u32, policy: RecoveryPolicy) -> Harness {
    harness_full(policy, Vec::new(), Vec::new(), failures_per_minute)
}

fn harness_full(
    policy: RecoveryPolicy,
    tenants: Vec<String>,
    settlements: Vec<Arc<dyn Settlement>>,
    failures_per_minute: u32,
) -> Harness {
    let clock = Arc::new(AtomicU64::new(NOW));
    let keys = Arc::new(ordo_identity::MemoryPasskeyStore::new());
    let rp = RelyingParty::new("example.com", &["https://app.example.com"]).expect("rp");
    let passkey = Arc::new(PasskeyProvider::with_stores(
        rp,
        keys.clone(),
        Arc::new(ordo_identity::MemoryCeremonyStore::new()),
    ));
    let password = LocalPasswordProvider::new(Argon2idHasher::weak_for_tests())
        .with_pending_max(PASSWORD_PENDING_CAP);
    password
        .enroll("ana@example.test", "correct-horse")
        .expect("enrolled");
    let mut registry = ProviderRegistry::new();
    registry
        .register("password", Box::new(password))
        .expect("password");
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("passkey");
    let revocations = Arc::new(MemorySessionRevocations::new());
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let factors = Arc::new(ordo_host::second_factor_store::MemorySecondFactorStore::new());
    let remove_factors = Arc::new(ordo_host::second_factors::RemoveSecondFactors {
        store: factors.clone(),
        passkey: Some(passkey.clone()),
        recorder: None,
    });
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(recorder.clone())
    .with_passkey(passkey)
    // Reauth handles live in the ceremony store, and the second step
    // decides what a handle proves.
    .with_ceremonies(Arc::new(ordo_identity::MemoryCeremonyStore::new()));
    let read = clock.clone();
    state.now = Box::new(move || read.load(Ordering::SeqCst));
    let read = clock.clone();
    let tickets = Arc::new(
        Tickets::new(Arc::new(MemoryTicketStore::new()))
            .with_clock(Box::new(move || read.load(Ordering::SeqCst))),
    );
    let read = clock.clone();
    let code_store = Arc::new(MemoryCodeStore::new());
    let codes = Arc::new(
        RecoveryCodes::new(code_store.clone())
            .with_clock(Box::new(move || read.load(Ordering::SeqCst)))
            .with_failure_limit(failures_per_minute),
    );
    let read = clock.clone();
    let emails = Arc::new(ordo_host::contact_email::ContactEmails::in_memory(
        Default::default(),
    ));
    let mut service = Recovery::new(
        Arc::new(MemoryRecoveryStore::new()),
        tickets.clone(),
        policy,
    )
    .with_clock(Box::new(move || read.load(Ordering::SeqCst)))
    .with_recorder(recorder)
    .with_codes(codes.clone())
    .with_tenants(Box::new(move |_| tenants.clone()))
    .with_settlement(remove_factors)
    .with_settlement(Arc::new(ordo_host::recovery_settings::RestoreSettings {
        snapshots: Arc::new(ordo_host::recovery_settings::MemorySnapshots::new()),
        settings: vec![
            Arc::new(ordo_host::recovery_settings::SecondStepLevel(
                factors.clone(),
            )),
            Arc::new(ordo_host::recovery_settings::ContactEmailSetting {
                emails: emails.clone(),
                runtime: tokio::runtime::Handle::current(),
            }),
        ],
        keep_secs: 90 * DAY,
    }));
    for s in settlements {
        service = service.with_settlement(s);
    }
    let recovery = Arc::new(service);
    let state = Arc::new(state);
    let totp = Arc::new(ordo_host::second_factors::TotpMethod::new(
        factors.clone(),
        ordo_host::second_factors::Sealer::new(Arc::new(ordo_crypto::TestProvider::new())),
        Default::default(),
    ));
    state.wire_second_factors(Arc::new(
        ordo_host::second_factors::SecondFactors::new(Default::default())
            .with_store(factors.clone())
            .with_totp(totp),
    ));
    state.wire_enrolment(ordo_host::auth::Enrolment {
        tickets: Some(tickets.clone()),
        ..Default::default()
    });
    state.wire_recovery(recovery.clone());
    Harness {
        auth: state,
        keys,
        revocations,
        recovery,
        tickets,
        codes,
        code_store,
        store,
        factors,
        emails,
        clock,
    }
}

fn harness() -> Harness {
    harness_with(RecoveryPolicy::default(), Vec::new())
}

async fn send(
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

async fn post(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    send(app, path, Some(body), None).await
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

/// A second device, distinct from `es256()`.
fn phone() -> TestAuthenticator {
    TestAuthenticator::ed25519()
}

/// Sign up with `device`: the token of that sign-in.
async fn sign_up(app: &axum::Router, device: &TestAuthenticator) -> String {
    let (status, options) = post(app, "/auth/register", json!({"method": "passkey"})).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = post(
        app,
        "/auth/complete",
        registration_body(&device.register(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["token"].as_str().expect("token").to_string()
}

/// Sign in with `device`. The whole answer, so a test can read a refusal.
async fn sign_in_answer(app: &axum::Router, device: &TestAuthenticator) -> (StatusCode, Value) {
    let (_, options) = post(app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    post(app, "/auth/complete", assertion_body(&device.assert(ch))).await
}

async fn sign_in(app: &axum::Router, device: &TestAuthenticator) -> String {
    let (status, done) = sign_in_answer(app, device).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["token"].as_str().expect("token").to_string()
}

fn principal_of(token: &str, at: u64) -> PrincipalId {
    ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        token,
        ordo_identity::Timestamp(at),
    )
    .expect("a token this deployment minted")
    .principal
}

/// A door over an engine where `principal` owns `SPACE`, `space.export` asks
/// for presence and `records.list` does not.
fn door_for(principal: &PrincipalId, h: &Harness, at: u64) -> IdentityAccess {
    let space = SpaceId(SPACE.into());
    let mut engine = AccessEngine::new();
    engine.set_profile(
        &space,
        AccessProfile::new()
            .allow(Role::Owner, PLAIN)
            .allow(Role::Owner, GATED),
    );
    engine.set_membership(&space, principal, Role::Owner);
    engine.require_step_up(GATED);
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(move || at),
        Arc::new(Metrics::new()),
    )
    .with_revocations(h.revocations.clone())
}

/// The deployment owner opens a recovery at the host and gets the ticket.
fn host_recovery(h: &Harness, who: &PrincipalId) -> String {
    h.recovery
        .open(&Open {
            principal: &who.0,
            path: Path::Host,
            opened_by: "adam",
            authority: "host",
            evidence: "host-break-glass",
            reference: "TICKET-12",
            space: "",
            extra_delay_secs: 0,
            approvals: 0,
        })
        .expect("opened")
        .ticket
        .expect("a host recovery matures at once")
}

/// Register `device` with `ticket`: begin, then complete.
async fn recover_with(
    app: &axum::Router,
    device: &TestAuthenticator,
    ticket: &str,
) -> (StatusCode, Value) {
    let (status, options) = post(app, "/auth/recover/begin", json!({"ticket": ticket})).await;
    if status != StatusCode::OK {
        return (status, options);
    }
    let ch = options["challenge"].as_str().expect("challenge");
    let mut body = registration_body(&device.register(ch));
    body["ticket"] = json!(ticket);
    post(app, "/auth/recover/complete", body).await
}

fn drained(h: &Harness) -> Vec<ordo_secevent::StreamEntry> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 100 * DAY))
}

// ══════════════════════════ the recovery itself ══════════════════════════

/// **A recovered passkey is refused export until the quarantine ends.** The
/// person signs in with it at once and reads and writes as before; the op
/// that asks for presence says no, and says when it will say yes.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_passkey_is_refused_export_until_quarantine_ends() {
    let h = harness();
    let app = h.app();
    let (lost, kept) = (TestAuthenticator::es256(), phone());
    let old_session = sign_up(&app, &lost).await;
    let ana = principal_of(&old_session, NOW);
    let ticket = host_recovery(&h, &ana);

    h.set_now(NOW + 60);
    let (status, done) = recover_with(&app, &kept, &ticket).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["recovered"], true);
    assert_eq!(done["presence_available_at"], NOW + 60 + DAY);

    // The session she had is over.
    let door = door_for(&ana, &h, NOW + 61);
    assert_eq!(door.check(&old_session, SPACE, PLAIN), AccessDecision::Deny);

    // She signs in with the new passkey a moment later.
    h.set_now(NOW + 120);
    let (status, answer) = sign_in_answer(&app, &kept).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["presence_available_at"], NOW + 60 + DAY);
    let token = answer["token"].as_str().expect("token").to_string();
    assert_eq!(principal_of(&token, NOW + 120), ana, "the same person");

    let door = door_for(&ana, &h, NOW + 121);
    let _ = door.take_events();
    assert!(
        matches!(
            door.check(&token, SPACE, PLAIN),
            AccessDecision::Allow { .. }
        ),
        "ordinary calls work at once"
    );
    assert_eq!(door.check(&token, SPACE, GATED), AccessDecision::Deny);
    assert_eq!(
        door.take_events().last().expect("an event").kind,
        EventKind::denied(RecordedDecision::StepUpRequired),
        "export is refused for want of presence, and for nothing else"
    );

    // A step-up with the same passkey gains none, and says when it will.
    let (_, opts) = send(&app, "/auth/step-up/begin", None, Some(&token)).await;
    let ch = opts["challenge"].as_str().expect("challenge");
    let (status, stepped) = send(
        &app,
        "/auth/step-up",
        Some(assertion_body(&kept.assert(ch))),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stepped}");
    assert_eq!(stepped["presence_available_at"], NOW + 60 + DAY);
    let after_step_up = stepped["token"].as_str().expect("token");
    assert_eq!(
        door.check(after_step_up, SPACE, GATED),
        AccessDecision::Deny,
        "stepping up with a quarantined passkey gains nothing"
    );

    // Once the day is over the same passkey passes.
    h.set_now(NOW + 60 + DAY);
    let token = sign_in(&app, &kept).await;
    let door = door_for(&ana, &h, NOW + 60 + DAY + 1);
    assert!(
        matches!(
            door.check(&token, SPACE, GATED),
            AccessDecision::Allow { .. }
        ),
        "after the quarantine the passkey carries presence"
    );
}

/// A recovery ticket is a passkey registration and nothing else. No door
/// that takes a password accepts it, no other registration door accepts it,
/// and a refusal spends nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_ticket_cannot_be_redeemed_with_a_password() {
    let h = harness();
    let app = h.app();
    let lost = TestAuthenticator::es256();
    let session = sign_up(&app, &lost).await;
    let ana = principal_of(&session, NOW);
    let ticket = host_recovery(&h, &ana);

    // `redeem_password` asks a ticket for the purpose `password`, the
    // enrolment door for `passkey`, an invitation for `invitation`. Each
    // is refused, and none spends it.
    for purpose in [Purpose::Password, Purpose::Passkey, Purpose::Invitation] {
        let want = Want {
            purpose: Some(purpose),
            ..Default::default()
        };
        assert!(
            h.tickets.redeem_for(&ticket, &want).is_err(),
            "{purpose:?} accepted a recovery ticket"
        );
    }
    let (status, _) = post(
        &app,
        "/auth/register",
        json!({"method": "passkey", "ticket": ticket}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the sign-up door");
    assert!(h.recovery.look(&ticket).is_ok(), "nothing was spent");

    // Still good for the one thing it is for.
    let (status, done) = recover_with(&app, &phone(), &ticket).await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

/// A ticket recovers once. The second presentation is the one refusal, and a
/// ceremony begun with one ticket cannot finish with another.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_completes_once_and_only_with_its_own_ticket() {
    let h = harness();
    let app = h.app();
    let lost = TestAuthenticator::es256();
    let session = sign_up(&app, &lost).await;
    let ana = principal_of(&session, NOW);
    let ticket = host_recovery(&h, &ana);

    // A ceremony begun with this ticket, finished with a made-up one.
    let (_, options) = post(&app, "/auth/recover/begin", json!({"ticket": ticket})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let mut body = registration_body(&phone().register(ch));
    body["ticket"] = json!(format!("tkt_{}", "ab".repeat(32)));
    let (status, _) = post(&app, "/auth/recover/complete", body).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.keys.len(), 1, "a refused recovery stored no key");

    let second = phone();
    let (status, done) = recover_with(&app, &second, &ticket).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let third = {
        let mut d = TestAuthenticator::es256();
        d.credential_id = b"credential-three".to_vec();
        d
    };
    let (status, _) = recover_with(&app, &third, &ticket).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the ticket is spent");
    assert_eq!(h.keys.len(), 2, "the second attempt left no key behind");
}

/// The passkey a recovery retired can contest it, and can do nothing else
/// with the person's account.
#[tokio::test(flavor = "multi_thread")]
async fn a_retired_passkey_contests_and_the_recovery_is_undone() {
    let h = harness();
    let app = h.app();
    let (lost, thief) = (TestAuthenticator::es256(), phone());
    let session = sign_up(&app, &lost).await;
    let ana = principal_of(&session, NOW);
    let ticket = host_recovery(&h, &ana);
    h.set_now(NOW + 60);
    let (status, _) = recover_with(&app, &thief, &ticket).await;
    assert_eq!(status, StatusCode::OK);

    // The retired passkey does not sign in. Its holder is told why.
    h.set_now(NOW + 120);
    let (status, told) = sign_in_answer(&app, &lost).await;
    assert_eq!(status, StatusCode::CONFLICT, "{told}");
    assert_eq!(told["error"]["code"], "recovery_contestable");
    assert_eq!(told["error"]["contest_until"], NOW + 60 + 3 * DAY);

    // It contests.
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&lost.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["restored"], 1);
    assert_eq!(h.keys.len(), 1, "the recovery's passkey is gone entirely");

    // The thief's passkey signs nobody in. The owner's does, later.
    h.set_now(NOW + 200);
    let (status, _) = sign_in_answer(&app, &thief).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let token = sign_in(&app, &lost).await;
    assert_eq!(principal_of(&token, NOW + 200), ana);

    let kinds: Vec<String> = drained(&h)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Recovery { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert!(
        kinds.iter().any(|a| a == "contested"),
        "the contest is recorded: {kinds:?}"
    );
}

/// The contest window closes. Past it the retired passkeys are removed with
/// their keys, the case is closed, and a contest finds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_window_removes_the_retired_passkeys_and_their_keys() {
    let h = harness();
    let app = h.app();
    let (lost, kept) = (TestAuthenticator::es256(), phone());
    let session = sign_up(&app, &lost).await;
    let ana = principal_of(&session, NOW);
    let ticket = host_recovery(&h, &ana);
    let (status, _) = recover_with(&app, &kept, &ticket).await;
    assert_eq!(status, StatusCode::OK);

    h.set_now(NOW + 3 * DAY + 1);
    let gone = {
        let mut dir = h.auth.directory.lock().expect("directory");
        h.recovery.sweep(&mut dir).expect("swept")
    };
    assert_eq!(gone.len(), 1);
    let passkey = h.auth.passkey.as_ref().expect("passkey");
    passkey.remove_credential(&lost.credential_id).expect("key");
    assert_eq!(h.keys.len(), 1);
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, _) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&lost.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "nothing left to contest");
    assert!(h.recovery.live_for(&ana.0).expect("read").is_none());
}

/// A person has one live case. A second is refused until the first is over.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_has_one_live_case() {
    let h = harness();
    let app = h.app();
    let session = sign_up(&app, &TestAuthenticator::es256()).await;
    let ana = principal_of(&session, NOW);
    let _ = host_recovery(&h, &ana);
    let again = h.recovery.open(&Open {
        principal: &ana.0,
        path: Path::Host,
        opened_by: "adam",
        authority: "host",
        evidence: "host-break-glass",
        reference: "",
        space: "",
        extra_delay_secs: 0,
        approvals: 0,
    });
    assert!(matches!(again, Err(OpenError::Live)));
}

/// A case with a delay yields no ticket until it matures, and an active
/// passkey of the person can cancel it before then.
#[tokio::test(flavor = "multi_thread")]
async fn a_delayed_case_yields_its_ticket_late_and_an_active_passkey_cancels_it() {
    let policy = RecoveryPolicy {
        host_delay_secs: 3_600,
        ..RecoveryPolicy::default()
    };
    let h = harness_with(policy, Vec::new());
    let app = h.app();
    let device = TestAuthenticator::es256();
    let session = sign_up(&app, &device).await;
    let ana = principal_of(&session, NOW);
    let opened = h
        .recovery
        .open(&Open {
            principal: &ana.0,
            path: Path::Host,
            opened_by: "adam",
            authority: "host",
            evidence: "host-break-glass",
            reference: "",
            space: "",
            extra_delay_secs: 0,
            approvals: 0,
        })
        .expect("opened");
    assert!(opened.ticket.is_none(), "a delay holds the ticket back");
    assert!(h.recovery.issue_ticket(&opened.case).is_err());

    // The person still holds a passkey and does not want this: it cancels.
    h.set_now(NOW + 60);
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&device.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["cancelled"], opened.case);
    h.set_now(NOW + 3_600);
    assert!(
        h.recovery.issue_ticket(&opened.case).is_err(),
        "a cancelled case yields nothing"
    );
}

/// A settlement (what the second factor registers to remove every second
/// factor) runs when a recovery completes and not when it is opened.
#[tokio::test(flavor = "multi_thread")]
async fn a_settlement_runs_only_once_the_recovery_completes() {
    struct Count(Arc<AtomicU64>);
    impl Settlement for Count {
        fn name(&self) -> &'static str {
            "second-factors"
        }
        fn settle(&self, _principal: &str, _case: &str) -> Result<(), ordo_identity::StoreFault> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let ran = Arc::new(AtomicU64::new(0));
    let h = harness_full(
        RecoveryPolicy::default(),
        Vec::new(),
        vec![Arc::new(Count(ran.clone()))],
        30,
    );
    let app = h.app();
    let session = sign_up(&app, &TestAuthenticator::es256()).await;
    let ana = principal_of(&session, NOW);
    let ticket = host_recovery(&h, &ana);
    assert_eq!(ran.load(Ordering::SeqCst), 0, "opening removes nothing");
    let (status, _) = recover_with(&app, &phone(), &ticket).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ran.load(Ordering::SeqCst), 1, "completing does");
}

/// **An act is recorded at deployment scope and mirrored into each tenant the
/// person belongs to**, naming the person, the act and the class of
/// authority, and nothing about any other tenant.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_is_recorded_and_mirrored_into_each_tenant() {
    let h = harness_with(
        RecoveryPolicy::default(),
        vec!["spc_a".to_string(), "spc_b".to_string()],
    );
    let app = h.app();
    let session = sign_up(&app, &TestAuthenticator::es256()).await;
    let ana = principal_of(&session, NOW);
    let _ = drained(&h);
    let ticket = host_recovery(&h, &ana);
    let (status, _) = recover_with(&app, &phone(), &ticket).await;
    assert_eq!(status, StatusCode::OK);

    let acts: Vec<(Scope, String, String)> = drained(&h)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Recovery { act, detail } => Some((e.scope, act, detail)),
            _ => None,
        })
        .collect();
    for act in ["opened", "completed"] {
        let scopes: Vec<&Scope> = acts
            .iter()
            .filter(|(_, a, _)| a == act)
            .map(|(s, _, _)| s)
            .collect();
        assert_eq!(scopes.len(), 3, "{act}: deployment plus two tenants");
        assert!(scopes.contains(&&Scope::Deployment));
        assert!(scopes.contains(&&Scope::Space(ordo_commandlog::SpaceId("spc_a".into()))));
        assert!(scopes.contains(&&Scope::Space(ordo_commandlog::SpaceId("spc_b".into()))));
    }
    let opened = acts
        .iter()
        .find(|(_, a, _)| a == "opened")
        .map(|(_, _, d)| d.clone())
        .expect("opened");
    assert!(opened.contains("authority=host"), "{opened}");
    assert!(opened.contains("evidence=host-break-glass"), "{opened}");
    assert!(
        acts.iter()
            .all(|(_, _, d)| !d.contains("spc_a") && !d.contains("spc_b")),
        "an entry never names a tenant"
    );
}

// ══════════════════════════ R4 · recovery codes ═════════════════════════

/// `POST` with a bearer and no body, returning the status and the body.
async fn post_bearer(app: &axum::Router, path: &str, token: &str) -> (StatusCode, Value) {
    send(app, path, None, Some(token)).await
}

/// Issue a set for the person whose session is `token`.
async fn issue_codes(app: &axum::Router, token: &str) -> Vec<String> {
    let (status, body) = post_bearer(app, "/auth/recovery-codes", token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["codes"]
        .as_array()
        .expect("codes")
        .iter()
        .map(|c| c.as_str().expect("code").to_string())
        .collect()
}

async fn redeem_code(app: &axum::Router, code: &str) -> (StatusCode, Value) {
    post(app, "/auth/recover/code", json!({"code": code})).await
}

/// A set is ten codes of twenty-four characters shown as six groups of four,
/// from the Crockford alphabet, and the table holds none of them.
#[tokio::test(flavor = "multi_thread")]
async fn a_set_is_ten_grouped_codes_and_the_table_holds_none_of_them() {
    let h = harness();
    let app = h.app();
    let token = sign_up(&app, &TestAuthenticator::es256()).await;
    let codes = issue_codes(&app, &token).await;
    assert_eq!(codes.len(), 10);
    let mut seen = std::collections::BTreeSet::new();
    for c in &codes {
        let groups: Vec<&str> = c.split('-').collect();
        assert_eq!(groups.len(), 6, "{c}");
        assert!(groups.iter().all(|g| g.len() == 4), "{c}");
        assert!(
            c.chars()
                .all(|ch| ch == '-' || "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(ch)),
            "{c}"
        );
        seen.insert(c.clone());
    }
    assert_eq!(seen.len(), 10, "no two alike");
    let held = h.code_store.hashes();
    assert_eq!(held.len(), 10);
    for c in &codes {
        let plain = c.replace('-', "");
        assert!(!held.contains(c) && !held.contains(&plain));
        assert!(held.contains(&ordo_host::recovery_codes::hash_of(&plain)));
    }
}

/// **A code redeems once.** It opens a case and returns a ticket for a new
/// passkey, a second use is the one refusal, another code while the case is
/// live is refused **without being spent**, and completing the recovery
/// voids what is left.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_redeems_once() {
    let h = harness();
    let app = h.app();
    let (lost, kept) = (TestAuthenticator::es256(), phone());
    let session = sign_up(&app, &lost).await;
    let ana = principal_of(&session, NOW);
    let codes = issue_codes(&app, &session).await;

    let (status, done) = redeem_code(&app, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["remaining"], 9);
    let ticket = done["ticket"].as_str().expect("a code works at once");
    let (status, _) = redeem_code(&app, &codes[0]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a used code");

    // A second code while the case is live is refused and is not burned.
    let (status, _) = redeem_code(&app, &codes[1]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.codes.remaining(&ana.0).expect("count"), 9);

    // The ticket is for a new passkey, and completing voids the rest.
    let (status, done) = recover_with(&app, &kept, ticket).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(h.codes.remaining(&ana.0).expect("count"), 0);
    let (status, _) = redeem_code(&app, &codes[2]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the set was voided");
    let used = ordo_host::recovery_codes::hash_of(&codes[0].replace('-', ""));
    assert_eq!(h.code_store.used_as(&used).as_deref(), Some("recovery"));
}

/// A code can be typed the way people type: lower case, no dashes, and the
/// characters Crockford treats as the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_typed_loosely_is_the_same_code() {
    let h = harness();
    let app = h.app();
    let session = sign_up(&app, &TestAuthenticator::es256()).await;
    let codes = issue_codes(&app, &session).await;
    let loose = codes[3].replace('-', " ").to_lowercase();
    let (status, done) = redeem_code(&app, &loose).await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

/// **A password session cannot issue codes**, nor can a passkey session whose
/// presence has gone stale, nor one whose passkey is quarantined: a stolen
/// token, or a way in that came through recovery, must not mint a way back.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_session_cannot_issue_codes() {
    let h = harness();
    let app = h.app();
    // A password session.
    let (_, begun) = post(
        &app,
        "/auth/begin",
        json!({"method": "password", "identifier": "ana@example.test"}),
    )
    .await;
    let (status, done) = post(
        &app,
        "/auth/complete",
        json!({"method": "password", "kind": "password",
               "challenge": begun["challenge"], "identifier": "ana@example.test",
               "password": "correct-horse", "display": "Ana"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let password_token = done["token"].as_str().expect("token").to_string();
    let (status, body) = post_bearer(&app, "/auth/recovery-codes", &password_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "presence_required");

    // A passkey session, presence fresh, then stale.
    let (lost, kept) = (TestAuthenticator::es256(), phone());
    let token = sign_up(&app, &lost).await;
    let ana = principal_of(&token, NOW);
    h.set_now(NOW + 301);
    let (status, _) = post_bearer(&app, "/auth/recovery-codes", &token).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "presence gone stale");
    h.set_now(NOW + 10);
    assert_eq!(issue_codes(&app, &token).await.len(), 10, "fresh works");

    // A quarantined passkey: recovered through the host, signed in with.
    let ticket = host_recovery(&h, &ana);
    h.set_now(NOW + 60);
    let (status, _) = recover_with(&app, &kept, &ticket).await;
    assert_eq!(status, StatusCode::OK);
    h.set_now(NOW + 120);
    let recovered = sign_in(&app, &kept).await;
    let (status, body) = post_bearer(&app, "/auth/recovery-codes", &recovered).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    h.set_now(NOW + 60 + DAY);
    let after = sign_in(&app, &kept).await;
    assert_eq!(issue_codes(&app, &after).await.len(), 10, "after the day");
}

/// Issuing a new set voids the old one.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_set_voids_the_old_one() {
    let h = harness();
    let app = h.app();
    let token = sign_up(&app, &TestAuthenticator::es256()).await;
    let old = issue_codes(&app, &token).await;
    let fresh = issue_codes(&app, &token).await;
    let (status, _) = redeem_code(&app, &old[0]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old set is void");
    let (status, _) = redeem_code(&app, &fresh[0]).await;
    assert_eq!(status, StatusCode::OK);
}

/// **A recovery made with a stolen code is contested**: the old passkey comes
/// back, the thief's is removed with its key, and the owner can issue a new
/// set.
#[tokio::test(flavor = "multi_thread")]
async fn a_contested_code_recovery_restores_the_old_passkey_and_removes_the_new() {
    let h = harness();
    let app = h.app();
    let (owner, thief) = (TestAuthenticator::es256(), phone());
    let session = sign_up(&app, &owner).await;
    let ana = principal_of(&session, NOW);
    let codes = issue_codes(&app, &session).await;

    // The thief has a piece of paper.
    h.set_now(NOW + 60);
    let (status, done) = redeem_code(&app, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (status, _) = recover_with(&app, &thief, done["ticket"].as_str().expect("ticket")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(h.keys.len(), 2);

    // The owner signs in with the passkey they still hold, is told, contests.
    h.set_now(NOW + 3_600);
    let (status, told) = sign_in_answer(&app, &owner).await;
    assert_eq!(status, StatusCode::CONFLICT, "{told}");
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, contested) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&owner.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{contested}");
    assert_eq!(h.keys.len(), 1, "the thief's passkey and its key are gone");

    h.set_now(NOW + 3_700);
    let (status, _) = sign_in_answer(&app, &thief).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let back = sign_in(&app, &owner).await;
    assert_eq!(principal_of(&back, NOW + 3_700), ana);
    assert_eq!(
        issue_codes(&app, &back).await.len(),
        10,
        "the owner is not stranded: a new set is theirs to issue"
    );
}

/// A deployment that has been sent too many wrong codes in a minute answers
/// `429` to every redemption until the window rolls, even a right code.
#[tokio::test(flavor = "multi_thread")]
async fn too_many_wrong_codes_are_throttled() {
    let h = harness_codes(3, RecoveryPolicy::default());
    let app = h.app();
    let session = sign_up(&app, &TestAuthenticator::es256()).await;
    let codes = issue_codes(&app, &session).await;
    for _ in 0..3 {
        let (status, _) = redeem_code(&app, "0000-0000-0000-0000-0000-0000").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, body) = redeem_code(&app, &codes[0]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    h.set_now(NOW + 61);
    let (status, _) = redeem_code(&app, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "the window rolled");
}

/// A deployment may put a delay on the code path. The case then matures
/// later, and the answer carries no ticket.
#[tokio::test(flavor = "multi_thread")]
async fn a_delay_on_the_code_path_holds_the_ticket_back() {
    let policy = RecoveryPolicy {
        code_delay_secs: 3_600,
        ..RecoveryPolicy::default()
    };
    let h = harness_codes(30, policy);
    let app = h.app();
    let session = sign_up(&app, &TestAuthenticator::es256()).await;
    let codes = issue_codes(&app, &session).await;
    let (status, done) = redeem_code(&app, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(done.get("ticket").is_none(), "no ticket before it matures");
    assert_eq!(done["matures_at"], NOW + 3_600);
    let case = done["case"].as_str().expect("case");
    assert!(h.recovery.issue_ticket(case).is_err());
    h.set_now(NOW + 3_600);
    assert!(h.recovery.issue_ticket(case).is_ok());
}

// ═══════════════ audit F-1, F-3, F7 (2026-10-24): the thief's chain ═══════════════

/// A request with a bearer and, when given, a reauth handle in `x-reauth`.
async fn send_reauth(
    app: &axum::Router,
    path: &str,
    body: Option<Value>,
    bearer: &str,
    reauth: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", format!("Bearer {bearer}"));
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    if let Some(r) = reauth {
        b = b.header("x-reauth", r);
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

/// `/auth/complete {"reauth": true}` with an assertion from `device`, for the
/// session `token`: the reauth handle.
async fn passkey_reauth(app: &axum::Router, device: &TestAuthenticator, token: &str) -> String {
    let (_, options) = post(app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let mut body = assertion_body(&device.assert(ch));
    body["reauth"] = json!(true);
    let (status, done) = send_reauth(app, "/auth/complete", Some(body), token, None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["reauth"]
        .as_str()
        .expect("a reauth handle")
        .to_string()
}

/// A stolen code, redeemed and turned into a recovery with `thief`. The case.
async fn stolen_code_recovery(
    h: &Harness,
    app: &axum::Router,
    owner: &TestAuthenticator,
    thief: &TestAuthenticator,
) -> (PrincipalId, String) {
    let session = sign_up(app, owner).await;
    let ana = principal_of(&session, NOW);
    let codes = issue_codes(app, &session).await;
    h.set_now(NOW + 60);
    let (status, done) = redeem_code(app, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (status, done) = recover_with(app, thief, done["ticket"].as_str().expect("ticket")).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    (ana, done["case"].as_str().expect("case").to_string())
}

fn recovery_details(h: &Harness, act: &str) -> Vec<String> {
    drained(h)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::Recovery { act: a, detail } if a == act => Some(detail),
            _ => None,
        })
        .collect()
}

/// **F-1.** The reauth handle stands in for presence only for a person who
/// holds no passkey. A thief in through a stolen code holds a quarantined
/// one, gets a handle from it, and is still refused codes.
#[tokio::test(flavor = "multi_thread")]
async fn a_quarantined_passkey_cannot_issue_codes_with_a_reauth_handle() {
    let h = harness();
    let app = h.app();
    let (owner, thief) = (TestAuthenticator::es256(), phone());
    let (ana, _) = stolen_code_recovery(&h, &app, &owner, &thief).await;
    assert_eq!(h.codes.remaining(&ana.0).expect("count"), 0);

    h.set_now(NOW + 120);
    let token = sign_in(&app, &thief).await;
    let handle = passkey_reauth(&app, &thief, &token).await;
    let (status, body) =
        send_reauth(&app, "/auth/recovery-codes", None, &token, Some(&handle)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "presence_required", "{body}");
    assert_eq!(
        h.codes.remaining(&ana.0).expect("count"),
        0,
        "no code was minted"
    );
}

/// The stand-in still works for the person it is for: one with no passkey.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_with_no_passkey_issues_codes_with_a_reauth_handle() {
    let h = harness();
    let app = h.app();
    let password = |reauth: bool| {
        let app = app.clone();
        async move {
            let (_, b) = post(
                &app,
                "/auth/begin",
                json!({"method": "password", "identifier": "ana@example.test"}),
            )
            .await;
            let mut body = json!({"method": "password", "kind": "password",
                "challenge": b["challenge"], "identifier": "ana@example.test",
                "password": "correct-horse", "display": "Ana"});
            if reauth {
                body["reauth"] = json!(true);
            }
            body
        }
    };
    let (status, done) = post(&app, "/auth/complete", password(false).await).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let token = done["token"].as_str().expect("token").to_string();
    let (status, done) = send_reauth(
        &app,
        "/auth/complete",
        Some(password(true).await),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let handle = done["reauth"].as_str().expect("handle");
    let (status, body) =
        send_reauth(&app, "/auth/recovery-codes", None, &token, Some(handle)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 10);
}

/// **F-1.** Past the quarantine the recovered passkey has presence and may
/// issue codes. The owner's contest voids them, ends the thief's sessions,
/// and says so in the record.
#[tokio::test(flavor = "multi_thread")]
async fn a_contest_voids_the_codes_the_recovered_passkey_issued() {
    let h = harness();
    let app = h.app();
    let (owner, thief) = (TestAuthenticator::es256(), phone());
    let (ana, _) = stolen_code_recovery(&h, &app, &owner, &thief).await;

    let after_quarantine = NOW + 60 + DAY + 10;
    h.set_now(after_quarantine);
    let token = sign_in(&app, &thief).await;
    let minted = issue_codes(&app, &token).await;
    assert_eq!(h.codes.remaining(&ana.0).expect("count"), 10);

    h.set_now(after_quarantine + 60);
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&owner.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        h.codes.remaining(&ana.0).expect("count"),
        0,
        "every code issued since the recovery is void"
    );
    let (status, _) = redeem_code(&app, &minted[0]).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a minted code opens nothing"
    );
    assert!(
        ordo_identity::SessionRevocations::principal_ended(
            h.revocations.as_ref(),
            &ana,
            ordo_identity::Timestamp(after_quarantine),
        ),
        "the thief's session ended"
    );
    let contested = recovery_details(&h, "contested");
    assert!(
        contested.iter().any(|d| d.contains("codes-voided=yes")),
        "{contested:?}"
    );
}

/// **F-3.** The full chain: a stolen code, the owner without a device. The
/// deployment owner closes the thief's contest window, opens a case at once,
/// the thief's passkey cannot cancel it, and the owner is back in.
#[tokio::test(flavor = "multi_thread")]
async fn closing_the_contest_lets_the_owner_recover_at_once_and_the_thief_cannot_cancel() {
    let policy = RecoveryPolicy {
        host_delay_secs: 3_600,
        ..RecoveryPolicy::default()
    };
    let h = harness_with(policy, Vec::new());
    let app = h.app();
    let (lost, thief) = (TestAuthenticator::es256(), phone());
    let (ana, case) = stolen_code_recovery(&h, &app, &lost, &thief).await;
    let open = |h: &Harness| {
        h.recovery.open(&Open {
            principal: &ana.0,
            path: Path::Host,
            opened_by: "adam",
            authority: "host",
            evidence: "host-break-glass",
            reference: "T-1",
            space: "",
            extra_delay_secs: 0,
            approvals: 0,
        })
    };

    // Inside the window nothing opens.
    h.set_now(NOW + 120);
    assert!(matches!(open(&h), Err(OpenError::Live)));

    // A reason off the list, and a case that is not a completed recovery,
    // are refused.
    assert!(h
        .recovery
        .close_contest(h.store.as_ref(), &case, "because", "adam")
        .is_err());
    assert!(h
        .recovery
        .close_contest(h.store.as_ref(), "rec_nothing", "recovery-disputed", "adam")
        .is_err());

    let closed = h
        .recovery
        .close_contest(h.store.as_ref(), &case, "recovery-disputed", "adam")
        .expect("closed");
    assert_eq!(closed.retired.len(), 1, "the lost passkey");
    assert!(
        h.recovery
            .close_contest(h.store.as_ref(), &case, "recovery-disputed", "adam")
            .is_err(),
        "once"
    );
    let opened = open(&h).expect("a new case opens at once");

    // The thief's passkey, added by a case whose window has not run out,
    // cannot cancel it.
    h.set_now(NOW + 180);
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, _) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&thief.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the thief cannot cancel");

    // Nor can the lost passkey undo the closed case.
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, _) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&lost.assert(ch)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a closed contest stays closed"
    );

    // The case matures and the owner registers a new device.
    h.set_now(NOW + 120 + 3_601);
    let (ticket, _) = h.recovery.issue_ticket(&opened.case).expect("matured");
    let fresh = {
        let mut d = TestAuthenticator::es256();
        d.credential_id = b"credential-fresh".to_vec();
        d
    };
    let (status, done) = recover_with(&app, &fresh, &ticket).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    h.set_now(NOW + 120 + 3_700);
    let back = sign_in(&app, &fresh).await;
    assert_eq!(principal_of(&back, NOW + 120 + 3_700), ana);
    let (status, _) = sign_in_answer(&app, &thief).await;
    assert_ne!(status, StatusCode::OK, "the thief's passkey is retired");

    let records = recovery_details(&h, "contest-closed");
    assert!(
        records
            .iter()
            .any(|d| d.contains("reason=recovery-disputed") && d.contains("by=adam")),
        "{records:?}"
    );
}

/// **F-3.** Without the close, the case the owner opens once the window ends
/// is theirs, and a passkey a recovery added may cancel only after its own
/// window: inside it the cancel is refused and recorded.
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_passkey_cannot_cancel_a_case_inside_its_window() {
    let policy = RecoveryPolicy {
        host_delay_secs: 3_600,
        ..RecoveryPolicy::default()
    };
    let h = harness_with(policy, Vec::new());
    let app = h.app();
    let (lost, thief) = (TestAuthenticator::es256(), phone());
    let (ana, case) = stolen_code_recovery(&h, &app, &lost, &thief).await;
    h.set_now(NOW + 120);
    h.recovery
        .close_contest(h.store.as_ref(), &case, "recovery-disputed", "adam")
        .expect("closed");
    h.recovery
        .open(&Open {
            principal: &ana.0,
            path: Path::Host,
            opened_by: "adam",
            authority: "host",
            evidence: "host-break-glass",
            reference: "T-2",
            space: "",
            extra_delay_secs: 0,
            approvals: 0,
        })
        .expect("opened");
    // A day in, still inside the three-day window the case was given.
    h.set_now(NOW + DAY);
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, _) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&thief.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let refused: Vec<String> = drained(&h)
        .into_iter()
        .map(|e| format!("{:?}", e.kind))
        .filter(|k| k.contains("recovered-passkey-in-contest-window"))
        .collect();
    assert!(!refused.is_empty(), "the refusal is recorded");
}

/// **F7.** Password challenges are capped. Past the cap a begin is refused,
/// and the refusal names the setting that raises it.
#[tokio::test(flavor = "multi_thread")]
async fn password_begins_past_the_cap_are_refused_naming_the_setting() {
    let h = harness();
    let app = h.app();
    let begin = || {
        post(
            &app,
            "/auth/begin",
            json!({"method": "password", "identifier": "ana@example.test"}),
        )
    };
    for _ in 0..PASSWORD_PENDING_CAP {
        let (status, body) = begin().await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["challenge"].as_str().expect("challenge");
        assert_eq!(id.len(), "pwd-ch-".len() + 32, "128 bits: {id}");
    }
    let (status, body) = begin().await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(
        body.to_string().contains("ORDO_AUTH_PASSWORD_PENDING_MAX"),
        "{body}"
    );
    // The challenges expire and the room comes back.
    h.set_now(NOW + 301);
    let (status, _) = begin().await;
    assert_eq!(status, StatusCode::OK);
}

// ═══════════ owner ruling 2026-10-25: what a contest undoes ═══════════

/// RFC 4648 base32 back to bytes: what a person's app does with the secret.
fn unbase32(s: &str) -> Vec<u8> {
    const A: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in s.bytes() {
        let v = A.iter().position(|a| *a == c).expect("base32") as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// A route that asks for proof: with the bearer alone, and again with a
/// reauth handle from `device` when it answers that it needs one.
async fn with_proof(
    app: &axum::Router,
    path: &str,
    body: Value,
    token: &str,
    device: &TestAuthenticator,
) -> (StatusCode, Value) {
    let (status, out) = send(app, path, Some(body.clone()), Some(token)).await;
    if status != StatusCode::FORBIDDEN {
        return (status, out);
    }
    let handle = passkey_reauth(app, device, token).await;
    send_reauth(app, path, Some(body), token, Some(&handle)).await
}

/// Enrol a TOTP for the session `token`, proven by `device`. The factor id.
async fn enrol_totp(
    app: &axum::Router,
    token: &str,
    device: &TestAuthenticator,
    now: u64,
) -> String {
    let (status, begun) = with_proof(
        app,
        "/auth/factors/totp/begin",
        json!({"label": "phone"}),
        token,
        device,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let secret = unbase32(begun["secret"].as_str().expect("secret"));
    let code = ordo_identity::totp::totp_at(&secret, now, ordo_identity::totp::DIGITS);
    let (status, done) = send(
        app,
        "/auth/factors/totp/confirm",
        Some(json!({"factor": begun["factor"], "code": code})),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    begun["factor"].as_str().expect("factor id").to_string()
}

/// Link `new` to the person whose session is `token`, vouched by `voucher`.
async fn link_passkey(
    app: &axum::Router,
    token: &str,
    new: &TestAuthenticator,
    voucher: &TestAuthenticator,
) {
    let (status, opts) = send(app, "/auth/authenticators/link/begin", None, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{opts}");
    let reg = opts["registration"]["challenge"].as_str().expect("reg");
    let vouch = opts["vouch"]["challenge"].as_str().expect("vouch");
    let (status, done) = send(
        app,
        "/auth/authenticators/link",
        Some(json!({
            "registration": registration_body(&new.register(reg)),
            "vouch": assertion_body(&voucher.assert(vouch)),
        })),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

/// **A contest removes every passkey and second factor enrolled since the
/// case opened, and names each one** (owner ruling 2026-10-25). A thief in
/// through a stolen code waits out the quarantine, links a second passkey,
/// enrols a TOTP and asks for it after passkeys too. The owner contests:
/// both go, each is recorded, and the owner signs in with the restored
/// passkey without being asked for the thief's code. The owner's spare
/// passkey, linked before the case opened, still signs in.
#[tokio::test(flavor = "multi_thread")]
async fn a_contest_removes_what_the_thief_enrolled_since_the_case_opened() {
    let h = harness();
    let app = h.app();
    let device = |seed: u8, id: &[u8]| TestAuthenticator {
        key: ordo_identity::software_authenticator::TestKey::es256(seed),
        credential_id: id.to_vec(),
        ..TestAuthenticator::es256()
    };
    let (owner, spare, thief, thief_spare) = (
        TestAuthenticator::es256(),
        device(21, b"owner-spare"),
        phone(),
        device(22, b"thief-spare"),
    );
    let session = sign_up(&app, &owner).await;
    let ana = principal_of(&session, NOW);
    link_passkey(&app, &session, &spare, &owner).await;
    let codes = issue_codes(&app, &session).await;

    // The code is stolen: the case opens at NOW + 60 and completes at once.
    h.set_now(NOW + 60);
    let (status, done) = redeem_code(&app, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (status, done) = recover_with(&app, &thief, done["ticket"].as_str().expect("ticket")).await;
    assert_eq!(status, StatusCode::OK, "{done}");

    // Past the quarantine the thief adds a passkey and a TOTP, and asks for
    // the TOTP after a passkey too.
    let after_quarantine = NOW + 60 + DAY + 10;
    h.set_now(after_quarantine);
    let token = sign_in(&app, &thief).await;
    link_passkey(&app, &token, &thief_spare, &thief).await;
    let totp_id = enrol_totp(&app, &token, &thief, after_quarantine).await;
    ordo_host::second_factor_store::SecondFactorStore::set_person_mode(
        h.factors.as_ref(),
        &ana.0,
        ordo_identity::second_step::PersonMode::AlsoForPasskeys,
        after_quarantine,
    )
    .expect("mode");
    assert_eq!(h.factors.all().len(), 1, "the thief's TOTP is enrolled");

    // The owner contests.
    h.set_now(after_quarantine + 60);
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&owner.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");

    // Both are gone, the passkey's key with its link.
    assert!(h.factors.all().is_empty(), "the thief's TOTP is removed");
    assert_eq!(
        ordo_identity::DirectoryStore::principal_for(
            h.store.as_ref(),
            &thief_spare.authenticator_id()
        )
        .expect("read"),
        None
    );
    assert_eq!(h.keys.len(), 2, "only the owner's two keys are left");

    // Each is recorded by kind and id, and no secret.
    let removed = recovery_details(&h, "contest-removed");
    assert_eq!(removed.len(), 2, "{removed:?}");
    assert!(
        removed
            .iter()
            .any(|d| d.contains("kind=totp") && d.contains(&format!("id={totp_id}"))),
        "{removed:?}"
    );
    assert!(
        removed.iter().any(|d| d.contains("kind=passkey")
            && d.contains(&format!("id={}", thief_spare.authenticator_id().0))),
        "{removed:?}"
    );

    // The owner signs in with the restored passkey, and is not asked for a
    // second step. The spare from before the case signs in too. Neither of
    // the thief's passkeys does.
    h.set_now(after_quarantine + 120);
    let (status, signed) = sign_in_answer(&app, &owner).await;
    assert_eq!(status, StatusCode::OK, "{signed}");
    assert!(signed.get("second_step").is_none(), "{signed}");
    assert_eq!(
        principal_of(
            signed["token"].as_str().expect("a token"),
            after_quarantine + 120
        ),
        ana
    );
    let token = sign_in(&app, &spare).await;
    assert_eq!(principal_of(&token, after_quarantine + 120), ana);
    for gone in [&thief, &thief_spare] {
        let (status, _) = sign_in_answer(&app, gone).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

/// Owner ruling 2026-10-25 (board `after-a-contest-what-comes-back`): a
/// contest puts back the sign-in settings the thief changed during the case.
/// The owner had a contact email and never chose a second-step level. The
/// thief asks for a second step after a passkey and moves the email. The
/// contest puts the email back and leaves the level as if never chosen, and
/// each reset is recorded without an address.
#[tokio::test(flavor = "multi_thread")]
async fn a_contest_puts_back_the_settings_the_thief_changed() {
    use ordo_host::second_factor_store::SecondFactorStore;
    let h = harness();
    let app = h.app();
    let (owner, thief) = (TestAuthenticator::es256(), phone());
    let session = sign_up(&app, &owner).await;
    let ana = principal_of(&session, NOW);
    let codes = issue_codes(&app, &session).await;
    h.emails
        .confirm(&ana.0, "ana@owner.example", "code", NOW)
        .await
        .expect("the owner's address");

    // The code is stolen: the case opens at NOW + 60 and completes at once.
    h.set_now(NOW + 60);
    let (status, done) = redeem_code(&app, &codes[0]).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (status, done) = recover_with(&app, &thief, done["ticket"].as_str().expect("ticket")).await;
    assert_eq!(status, StatusCode::OK, "{done}");

    // Past the quarantine the thief changes both settings.
    let later = NOW + 60 + DAY + 10;
    h.set_now(later);
    h.factors
        .set_person_mode(
            &ana.0,
            ordo_identity::second_step::PersonMode::AlsoForPasskeys,
            later,
        )
        .expect("level");
    h.emails
        .confirm(&ana.0, "thief@elsewhere.example", "code", later)
        .await
        .expect("the thief's address");

    // The owner contests.
    h.set_now(later + 60);
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&owner.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");

    // The level is as if never chosen. The address is the owner's again.
    assert_eq!(h.factors.chosen_person_mode(&ana.0).expect("read"), None);
    let back = h
        .emails
        .added_address(&ana.0)
        .await
        .expect("read")
        .expect("an address");
    assert_eq!(back.address, "ana@owner.example");
    assert_eq!(back.verified_at, NOW);

    // Each reset is recorded by kind, old and new, and no address.
    let resets = recovery_details(&h, "contest-reset");
    assert_eq!(resets.len(), 2, "{resets:?}");
    assert!(
        resets.iter().any(|d| d.contains("kind=second-step-level")
            && d.contains("old=also-for-passkeys")
            && d.contains("new=none")),
        "{resets:?}"
    );
    assert!(
        resets.iter().any(|d| d.contains("kind=contact-email")
            && d.contains("old=address(code)")
            && d.contains("new=address(code)")),
        "{resets:?}"
    );
    assert!(
        resets.iter().all(|d| !d.contains("example")),
        "no address in the record: {resets:?}"
    );
}

/// A setting the owner never had stays absent: an address the thief added
/// during the case is removed.
#[tokio::test(flavor = "multi_thread")]
async fn a_contact_email_the_owner_never_had_is_removed_by_a_contest() {
    let h = harness();
    let app = h.app();
    let (owner, thief) = (TestAuthenticator::es256(), phone());
    let session = sign_up(&app, &owner).await;
    let ana = principal_of(&session, NOW);
    let codes = issue_codes(&app, &session).await;
    h.set_now(NOW + 60);
    let (_, done) = redeem_code(&app, &codes[0]).await;
    let (status, done) = recover_with(&app, &thief, done["ticket"].as_str().expect("ticket")).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let later = NOW + 60 + DAY + 10;
    h.set_now(later);
    h.emails
        .confirm(&ana.0, "thief@elsewhere.example", "code", later)
        .await
        .expect("the thief's address");
    h.set_now(later + 60);
    let (_, options) = post(&app, "/auth/begin", json!({"method": "passkey"})).await;
    let ch = options["challenge"].as_str().expect("challenge");
    let (status, done) = post(
        &app,
        "/auth/recover/contest",
        assertion_body(&owner.assert(ch)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(h
        .emails
        .added_address(&ana.0)
        .await
        .expect("read")
        .is_none());
    let resets = recovery_details(&h, "contest-reset");
    assert_eq!(resets.len(), 1, "{resets:?}");
    assert!(resets[0].contains("old=address(code)") && resets[0].contains("new=none"));
}
