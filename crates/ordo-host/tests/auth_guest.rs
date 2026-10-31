//! Guest sign-in (`plan-auth-federated.md` §7A, step 8), through the real
//! routes. Discord is the fake TLS listener in `oauth2_support`, bound on
//! every address `localhost` answers.
//!
//! What it proves:
//!
//! - a guest comes back with its secret and with nothing else, and the
//!   directory never holds the secret;
//! - a guest that claims Discord, or adds a passkey by a claim, keeps its id,
//!   and what is held under that id stays reachable;
//! - a deleted guest cannot resume (§2.7 D6);
//! - guests off refuses a new guest and names the setting;
//! - the human check, when on, blocks a first guest without a token;
//! - under invite-only a new guest is refused like any newcomer.
//!
//! The sign-up ceiling and the sweep need Postgres: they are in
//! `provision_own.rs` and `registrations.rs`.

mod oauth2_support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use oauth2_support::{begin as discord_begin, callback, nelly, param, send, sign_in, FakeDiscord};
use ordo_api::{AccessChecker, AccessDecision};
use ordo_egress::{DenyList, TlsRoots};
use ordo_host::auth::guest::{self, GuestMode, GuestProvider, Guests};
use ordo_host::auth::human::HumanCheck;
use ordo_host::auth::AuthState;
use ordo_host::federation::MemoryIdentityAttrs;
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{AccessEngine, AccessProfile, CompleteRequest, Role, SpaceId};
use ordo_identity::{
    AuthenticatorId, DirectoryStore, MemoryCeremonyStore, MemoryDirectoryStore,
    MemorySessionRevocations, PrincipalId, ProviderRegistry, Timestamp, TokenKey,
};
use ordo_secevent::EventKind;
use serde_json::{json, Value};

const SECRET: &[u8] = b"auth-guest-test-secret-of-32-bytes!!";
const NOW: u64 = 1_800_000_000;
const NELLY_DISCORD: &str = "oauth2:discord:80351110224678912";
const SPACE: &str = "spc_chat";
const READ: &str = "chat.history";
const GATED: &str = "space.export";

const CA: &[u8] = include_bytes!("fixture/ca.der");

struct H {
    auth: Arc<AuthState>,
    app: axum::Router,
    dir: Arc<MemoryDirectoryStore>,
    clock: Arc<AtomicU64>,
    fake: FakeDiscord,
}

struct Opts {
    provisioning: Provisioning,
    guests: Option<GuestMode>,
    human: Option<HumanCheck>,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            provisioning: Provisioning::AutoOnboard,
            guests: Some(GuestMode::On),
            human: None,
        }
    }
}

async fn harness_with(o: Opts) -> H {
    let fake = FakeDiscord::start().await;
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let discord = fake.provider("discord", ceremonies.clone());
    let dir = Arc::new(MemoryDirectoryStore::new());
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let passkey = Arc::new(ordo_identity::PasskeyProvider::with_stores(
        ordo_identity::RelyingParty::new("example.com", &["https://app.example.com"])
            .expect("a relying party"),
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        ceremonies.clone(),
    ));
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("registered");
    registry
        .register("discord", Box::new(discord.clone()))
        .expect("registered");
    registry
        .register(guest::METHOD, Box::new(GuestProvider::new(dir.clone())))
        .expect("registered");
    let mut state = AuthState::new(
        registry,
        Directory::with_store(o.provisioning, dir.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_recorder(recorder)
    .with_passkey(passkey)
    .with_ceremonies(ceremonies)
    .with_oauth2(discord)
    .with_identity_attrs(Arc::new(MemoryIdentityAttrs::default()));
    if let Some(mode) = o.guests {
        state = state.with_guests(Guests::fixed(mode));
    }
    if let Some(check) = o.human {
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
        dir,
        clock,
        fake,
    }
}

async fn harness() -> H {
    harness_with(Opts::default()).await
}

async fn post(h: &H, path: &str, body: Value, bearer: Option<&str>) -> (StatusCode, Value) {
    let (status, _, answer) = send(&h.app, "POST", path, Some(body), bearer).await;
    (status, answer)
}

/// Make a guest: its token and its secret.
async fn new_guest(h: &H) -> (String, String) {
    let (status, answer) = post(
        h,
        "/auth/complete",
        json!({"method": "guest", "kind": "new", "display": "Guest"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["via"], "guest", "{answer}");
    (
        answer["token"].as_str().expect("a token").to_string(),
        answer["guest_secret"]
            .as_str()
            .expect("the secret, once")
            .to_string(),
    )
}

fn resume_body(secret: &str) -> Value {
    json!({"method": "guest", "kind": "resume", "secret": secret})
}

async fn resume(h: &H, secret: &str) -> (StatusCode, Value) {
    post(h, "/auth/complete", resume_body(secret), None).await
}

async fn discord_ceremony(h: &H, code: &str, profile: Value) -> Value {
    let (state, binder) = discord_begin(&h.app, "discord").await;
    h.fake.authorize(code, profile);
    let location = callback(&h.app, "discord", &state, code).await;
    let handoff = param(&location, "handoff").expect("a handoff");
    json!({"method": "discord", "kind": "handoff", "handoff": handoff, "binder": binder})
}

fn claims_of(token: &str) -> ordo_identity::TokenClaims {
    ordo_identity::verify(&TokenKey::from_secret(SECRET), token, Timestamp(NOW))
        .expect("a token this deployment minted")
}

fn owner_of(h: &H, authenticator: &str) -> Option<PrincipalId> {
    h.dir
        .principal_for(&AuthenticatorId(authenticator.into()))
        .expect("readable")
}

fn reasons(h: &H) -> Vec<String> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 1_000_000))
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::AuthenticationFailed { reason } => Some(reason),
            _ => None,
        })
        .collect()
}

fn assert_refused(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "authentication_failed", "{body}");
}

/// A door where `principal` is a member of the guest's chat: reading its
/// history is what a guest's data is, keyed by the principal's id.
fn door_for(principal: &PrincipalId) -> IdentityAccess {
    let space = SpaceId(SPACE.into());
    let mut engine = AccessEngine::new();
    engine.set_profile(
        &space,
        AccessProfile::new()
            .allow(Role::Editor, READ)
            .allow(Role::Editor, GATED),
    );
    engine.set_membership(&space, principal, Role::Editor);
    engine.require_step_up(GATED);
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW + 60),
        Arc::new(Metrics::new()),
    )
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
    json!({"method": "passkey", "kind": "passkey_registration", "challenge": challenge.0,
           "client_data_json": b64url_encode(client_data_json),
           "attestation_object": b64url_encode(attestation_object)})
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
    json!({"method": "passkey", "kind": "passkey_assertion", "challenge": challenge.0,
           "credential_id": b64url_encode(credential_id),
           "client_data_json": b64url_encode(client_data_json),
           "authenticator_data": b64url_encode(authenticator_data),
           "signature": b64url_encode(signature)})
}

// ── the tests ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_guest_resumes_with_its_secret_and_with_nothing_else() {
    let h = harness().await;
    let (token, secret) = new_guest(&h).await;
    let me = claims_of(&token).principal;
    assert!(
        claims_of(&token).user_presence_at.is_none(),
        "a guest proves no presence"
    );

    // The directory holds the hash of the secret, never the secret.
    let held = h.dir.authenticators_of(&me).expect("readable");
    assert_eq!(held.len(), 1);
    assert!(guest::is_guest(&held[0]), "{held:?}");
    assert!(!held[0].0.contains(&secret));
    assert_eq!(held[0], guest::authenticator_of(&secret));

    // The secret signs the guest in again, as the same person, and is not
    // handed out a second time.
    h.clock.fetch_add(3_600, Ordering::SeqCst);
    let (status, again) = resume(&h, &secret).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(claims_of(again["token"].as_str().unwrap()).principal, me);
    assert_eq!(again["via"], "guest");
    assert!(again.get("guest_secret").is_none(), "{again}");

    // Nothing else does. Another secret of the right shape, a wrong length,
    // no secret, and the token in place of the secret: one refusal each, and
    // none of them mints anybody.
    let stranger = ordo_identity::federation::random_secret();
    for body in [
        resume_body(&stranger),
        resume_body(&secret[..secret.len() - 1]),
        json!({"method": "guest", "kind": "resume"}),
        resume_body(&token),
    ] {
        let (status, answer) = post(&h, "/auth/complete", body, None).await;
        assert_refused(status, &answer);
    }
    let (status, answer) = post(
        &h,
        "/auth/complete",
        json!({"method": "guest", "kind": "resume"}),
        Some(&token),
    )
    .await;
    assert_refused(status, &answer);
    assert_eq!(owner_of(&h, &guest::authenticator_of(&stranger).0), None);

    // A guest has no `begin`, and a body cannot pass itself off as new.
    let (status, answer) = post(&h, "/auth/begin", json!({"method": "guest"}), None).await;
    assert_refused(status, &answer);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guest_that_claims_discord_keeps_its_id_and_its_data() {
    let h = harness().await;
    let (token, secret) = new_guest(&h).await;
    let me = claims_of(&token).principal;
    // What the guest holds is held under its id: a seat in a chat.
    let door = door_for(&me);
    assert!(matches!(
        door.check(&token, SPACE, READ),
        AccessDecision::Allow { .. }
    ));

    // The guest proves itself again with its secret, then completes Discord.
    let (status, opened) = post(&h, "/auth/claim/begin", resume_body(&secret), Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let window = opened["claim"].as_str().unwrap().to_string();
    let mut second = discord_ceremony(&h, "code-claim", nelly()).await;
    second["claim"] = json!(window);
    let (status, answer) = post(&h, "/auth/claim/complete", second, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["linked"], NELLY_DISCORD, "{answer}");
    assert_eq!(answer["authenticators"], 2, "{answer}");

    // Same id: Discord signs in as the guest, the secret still does, and
    // the seat answers the Discord session.
    assert_eq!(owner_of(&h, NELLY_DISCORD), Some(me.clone()));
    let (status, by_discord) = sign_in(&h.app, &h.fake, "discord", "code-again", nelly()).await;
    assert_eq!(status, StatusCode::OK, "{by_discord}");
    let discord_token = by_discord["token"].as_str().unwrap();
    assert_eq!(claims_of(discord_token).principal, me);
    assert!(matches!(
        door.check(discord_token, SPACE, READ),
        AccessDecision::Allow { .. }
    ));
    let (status, again) = resume(&h, &secret).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(claims_of(again["token"].as_str().unwrap()).principal, me);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guest_adds_a_passkey_through_a_claim() {
    let h = harness().await;
    let (token, secret) = new_guest(&h).await;
    let me = claims_of(&token).principal;
    let door = door_for(&me);
    assert_eq!(
        door.check(&token, SPACE, GATED),
        AccessDecision::Deny,
        "a guest cannot step up"
    );

    let (status, opened) = post(&h, "/auth/claim/begin", resume_body(&secret), Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let window = opened["claim"].as_str().unwrap().to_string();
    let (status, options) = post(
        &h,
        "/auth/claim/passkey",
        json!({"claim": window}),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let device = TestAuthenticator::es256();
    let mut body = registration_body(&device.register(options["challenge"].as_str().unwrap()));
    body["claim"] = json!(window);
    let (status, answer) = post(&h, "/auth/claim/complete", body, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["linked"], device.authenticator_id().0, "{answer}");

    // The passkey signs in as the guest, with presence.
    let (status, options) = post(&h, "/auth/begin", json!({"method": "passkey"}), None).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let (status, done) = post(
        &h,
        "/auth/complete",
        assertion_body(&device.assert(options["challenge"].as_str().unwrap())),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let by_passkey = done["token"].as_str().unwrap();
    assert_eq!(claims_of(by_passkey).principal, me);
    assert!(matches!(
        door.check(by_passkey, SPACE, GATED),
        AccessDecision::Allow { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_guest_cannot_resume() {
    let h = harness().await;
    let (token, secret) = new_guest(&h).await;
    let me = claims_of(&token).principal;
    h.dir.mark_deleted(&me, Timestamp(NOW)).unwrap();
    let _ = reasons(&h);

    let (status, answer) = resume(&h, &secret).await;
    assert_refused(status, &answer);
    assert!(reasons(&h).contains(&"identity-deleted".to_string()));
    // The link stays (D2), naming the deleted account and nobody new.
    assert_eq!(
        owner_of(&h, &guest::authenticator_of(&secret).0),
        Some(me.clone())
    );
    // A deleted guest's secret opens no claim either.
    let (status, answer) = post(&h, "/auth/claim/begin", resume_body(&secret), Some(&token)).await;
    assert_refused(status, &answer);
}

#[tokio::test(flavor = "multi_thread")]
async fn guests_off_refuses_a_new_guest() {
    for guests in [Some(GuestMode::Off), None] {
        let h = harness_with(Opts {
            guests,
            ..Opts::default()
        })
        .await;
        let (status, answer) = post(
            &h,
            "/auth/complete",
            json!({"method": "guest", "kind": "new"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
        assert_eq!(answer["error"]["code"], "guests_off", "{answer}");
        assert_eq!(answer["error"]["setting"], "ORDO_AUTH_GUEST", "{answer}");
        assert_eq!(answer["error"]["value"], "on", "{answer}");
        assert!(answer.get("token").is_none());
        assert!(reasons(&h)
            .iter()
            .any(|r| r.starts_with("guests-off") && r.contains("ORDO_AUTH_GUEST=on")));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_human_check_blocks_a_first_guest_without_a_token() {
    // A siteverify nobody listens on: a missing token is refused before
    // any request would be made.
    let check = HumanCheck::new(
        "turnstile-secret".to_string(),
        "https://localhost:1/turnstile/v0/siteverify",
        TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
        DenyList::categories_only().permitting_loopback_for_tests(),
    )
    .expect("a valid check");
    let h = harness_with(Opts {
        human: Some(check),
        ..Opts::default()
    })
    .await;
    for body in [
        json!({"method": "guest", "kind": "new"}),
        json!({"method": "guest", "kind": "new", "human": "not-a-real-token"}),
    ] {
        let (status, answer) = post(&h, "/auth/complete", body, None).await;
        assert_refused(status, &answer);
        assert!(answer.get("guest_secret").is_none());
    }
    let r = reasons(&h);
    assert!(r.contains(&"human-check-missing".to_string()), "{r:?}");
    assert!(
        r.iter().any(|x| x.starts_with("human-check-")),
        "the second is rejected or unreachable: {r:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn under_invite_only_a_new_guest_is_refused_like_any_newcomer() {
    let h = harness_with(Opts {
        provisioning: Provisioning::InviteOnly,
        ..Opts::default()
    })
    .await;
    let (status, answer) = post(
        &h,
        "/auth/complete",
        json!({"method": "guest", "kind": "new"}),
        None,
    )
    .await;
    assert_refused(status, &answer);
    assert!(reasons(&h).contains(&"not-invited".to_string()));
}
