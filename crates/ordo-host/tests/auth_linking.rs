//! Linking by claim (`plan-auth-federated.md` step 7, §2.5, owner ruling
//! R12: *"Existing account logs in and claims another account."*), through
//! the real routes. Discord is the fake TLS listener in `oauth2_support`,
//! bound on every address `localhost` answers. Emailed codes go through the
//! in-memory outbox, composed as the mail worker would compose them.
//!
//! What it proves:
//!
//! - an account that signs in again and completes another sign-in's
//!   ceremony inside the window holds both, and both resolve to it;
//! - a stolen token alone cannot open a window, and a window opened by
//!   somebody else's ceremony does not exist;
//! - a verified email match links nothing;
//! - a claim into a deleted account, or of a deleted account's sign-in, is
//!   refused (D5); a live account's sign-in is not taken from it;
//! - the window closes, is spent once, belongs to its session, and a
//!   mistyped code does not spend it;
//! - a claimed address is mailed under invite-only;
//! - a hand-off session claims only by two fresh ceremonies;
//! - a claim adds a new passkey, vouched for by any sign-in the account
//!   holds, which then signs in with presence; it takes no passkey
//!   assertion, and a stolen token alone adds none;
//! - linking switched off refuses and names the setting;
//! - every link and every refusal is in the security stream.

mod oauth2_support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use oauth2_support::{begin as discord_begin, callback, nelly, param, send, sign_in, FakeDiscord};
use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::auth::claim::{self, Linking};
use ordo_host::auth::AuthState;
use ordo_host::email_code::{
    EmailCodePolicy, EmailCodeProvider, MemoryEmailCodeStore, Registered, MAIL_KIND,
};
use ordo_host::federation::MemoryIdentityAttrs;
use ordo_host::mail::{Composer, MailJob, MemoryMailQueue, QueuedMail};
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{AccessEngine, AccessProfile, CompleteRequest, Role, SpaceId};
use ordo_identity::{
    AuthenticatorId, DirectoryStore, MemoryCeremonyStore, MemoryDirectoryStore,
    MemorySessionRevocations, PrincipalId, PrincipalKind, ProviderRegistry, Timestamp, TokenKey,
};
use ordo_secevent::EventKind;
use serde_json::{json, Value};

const SECRET: &[u8] = b"auth-linking-test-secret-of-32-bytes";
const NOW: u64 = 1_800_000_000;
const KIM: &str = "kim@example.org";
const NELLY_DISCORD: &str = "oauth2:discord:80351110224678912";
const SPACE: &str = "spc_guild";
const GATED: &str = "space.export";

/// A door where `space.export` requires presence and `principal` owns the
/// space.
fn door_for(principal: &PrincipalId, h: &H) -> IdentityAccess {
    let space = SpaceId(SPACE.into());
    let mut engine = AccessEngine::new();
    engine.set_profile(&space, AccessProfile::new().allow(Role::Owner, GATED));
    engine.set_membership(&space, principal, Role::Owner);
    engine.require_step_up(GATED);
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW + 60),
        Arc::new(Metrics::new()),
    )
    .with_revocations(h.revocations.clone())
}

/// What a browser posts for a passkey assertion.
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

struct H {
    auth: Arc<AuthState>,
    app: axum::Router,
    email: Arc<EmailCodeProvider>,
    queue: Arc<MemoryMailQueue>,
    dir: Arc<MemoryDirectoryStore>,
    clock: Arc<AtomicU64>,
    fake: FakeDiscord,
    revocations: Arc<MemorySessionRevocations>,
}

async fn harness_with(provisioning: Provisioning, linking: Linking) -> H {
    let fake = FakeDiscord::start().await;
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let discord = fake.provider("discord", ceremonies.clone());
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
            provisioning,
        )
        .with_recorder(recorder.clone()),
    );
    let passkey = Arc::new(ordo_identity::PasskeyProvider::with_stores(
        ordo_identity::RelyingParty::new("example.com", &["https://app.example.com"])
            .expect("a relying party"),
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        ceremonies.clone(),
    ));
    let revocations = Arc::new(MemorySessionRevocations::new());
    let mut registry = ProviderRegistry::new();
    registry
        .register("passkey", Box::new(passkey.clone()))
        .expect("registered");
    registry
        .register("discord", Box::new(discord.clone()))
        .expect("registered");
    registry
        .register("email", Box::new(Registered(email.clone())))
        .expect("registered");
    let mut state = AuthState::new(
        registry,
        Directory::with_store(provisioning, dir.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(recorder)
    .with_passkey(passkey)
    .with_ceremonies(ceremonies)
    .with_oauth2(discord)
    .with_identity_attrs(Arc::new(MemoryIdentityAttrs::default()))
    .with_email(email.clone())
    .with_linking(linking);
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
        dir,
        clock,
        fake,
        revocations,
    }
}

async fn harness() -> H {
    harness_with(Provisioning::AutoOnboard, Linking::default()).await
}

fn now(h: &H) -> u64 {
    h.clock.load(Ordering::SeqCst)
}

async fn post(h: &H, path: &str, body: Value, bearer: Option<&str>) -> (StatusCode, Value) {
    let (status, _, answer) = send(&h.app, "POST", path, Some(body), bearer).await;
    (status, answer)
}

// ── emailed codes ──────────────────────────────────────────────────────

/// `begin` for an emailed code, with whatever else the page sends.
async fn email_begin(h: &H, address: &str, extra: Value) -> (String, String) {
    let mut body = json!({"method": "email", "identifier": address});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        body[k] = v;
    }
    let (status, answer) = post(h, "/auth/begin", body, None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    (
        answer["challenge"].as_str().expect("challenge").to_string(),
        answer["binder"].as_str().expect("binder").to_string(),
    )
}

/// What the mail worker would send now, in order: `None` where nothing was
/// owed.
fn deliver(h: &H) -> Vec<Option<String>> {
    h.queue
        .take()
        .iter()
        .enumerate()
        .map(|(i, (job, _))| compose(h, job, i as i64))
        .collect()
}

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
    let text = msg.text();
    let start = text.find("code is ").expect("the code line") + "code is ".len();
    Some(
        text[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect(),
    )
}

/// An emailed code's completed ceremony, as a `/auth/complete` body.
async fn email_ceremony(h: &H, address: &str, extra: Value) -> Value {
    let (challenge, binder) = email_begin(h, address, extra).await;
    let sent = deliver(h);
    let code = sent[0].clone().expect("a code was sent");
    json!({"method": "email", "kind": "email_code",
           "challenge": challenge, "binder": binder, "code": code})
}

/// Sign in by emailed code: the token.
async fn email_sign_in(h: &H, address: &str) -> String {
    let body = email_ceremony(h, address, json!({})).await;
    let (status, answer) = post(h, "/auth/complete", body, None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer["token"].as_str().expect("a token").to_string()
}

// ── Discord ────────────────────────────────────────────────────────────

/// A Discord ceremony up to its handoff, as a `/auth/complete` body.
async fn discord_ceremony(h: &H, code: &str, profile: Value) -> Value {
    let (state, binder) = discord_begin(&h.app, "discord").await;
    h.fake.authorize(code, profile);
    let location = callback(&h.app, "discord", &state, code).await;
    let handoff = param(&location, "handoff").expect("a handoff");
    json!({"method": "discord", "kind": "handoff", "handoff": handoff, "binder": binder})
}

fn mallory() -> Value {
    json!({"id": "99999999999999999", "username": "mallory", "global_name": "Mallory",
           "email": "mallory@example.net", "verified": true})
}

// ── reading the outcome ────────────────────────────────────────────────

fn claims_of(token: &str) -> ordo_identity::TokenClaims {
    ordo_identity::verify(&TokenKey::from_secret(SECRET), token, Timestamp(NOW))
        .expect("a token this deployment minted")
}

fn owner_of(h: &H, authenticator: &str) -> Option<PrincipalId> {
    h.dir
        .principal_for(&AuthenticatorId(authenticator.into()))
        .expect("readable")
}

fn held(h: &H, principal: &PrincipalId) -> usize {
    h.dir.authenticators_of(principal).expect("readable").len()
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

fn reasons(events: &[EventKind]) -> Vec<String> {
    events
        .iter()
        .filter_map(|k| match k {
            EventKind::AuthenticationFailed { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn assert_refused(status: StatusCode, body: &Value) {
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "authentication_failed", "{body}");
}

/// Open a window with an emailed code of the account's own address.
async fn open(h: &H, token: &str, address: &str) -> String {
    let first = email_ceremony(h, address, json!({})).await;
    let (status, answer) = post(h, "/auth/claim/begin", first, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer["claim"].as_str().expect("a claim").to_string()
}

async fn claim(h: &H, token: &str, claim: &str, mut ceremony: Value) -> (StatusCode, Value) {
    ceremony["claim"] = json!(claim);
    post(h, "/auth/claim/complete", ceremony, Some(token)).await
}

// ── the tests ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn an_existing_account_signs_in_and_claims_another_and_both_then_resolve_to_it() {
    let h = harness().await;
    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;
    let window = open(&h, &token, KIM).await;

    // Nelly's Discord carries a different address: a claim reads none.
    let second = discord_ceremony(&h, "code-claim", nelly()).await;
    let (status, answer) = claim(&h, &token, &window, second).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["linked"], NELLY_DISCORD, "{answer}");
    assert_eq!(answer["via"], "oauth2:discord", "{answer}");
    assert_eq!(answer["authenticators"], 2, "{answer}");
    assert!(answer.get("token").is_none(), "a claim mints no session");
    assert_eq!(owner_of(&h, NELLY_DISCORD), Some(kim.clone()));

    // The link is recorded naming both sign-ins: the one added and the one
    // whose fresh ceremony vouched for it.
    let seen = events(&h);
    let email_id = format!("email:{KIM}");
    assert!(
        seen.iter().any(|k| matches!(k,
            EventKind::AuthenticatorLinked { added, vouched_by }
                if added == NELLY_DISCORD && vouched_by.as_deref() == Some(email_id.as_str()))),
        "{seen:?}"
    );

    // Both sign-ins now resolve to Kim.
    let (status, by_discord) = sign_in(&h.app, &h.fake, "discord", "code-again", nelly()).await;
    assert_eq!(status, StatusCode::OK, "{by_discord}");
    assert_eq!(
        claims_of(by_discord["token"].as_str().unwrap()).principal,
        kim
    );
    let by_email = email_sign_in(&h, KIM).await;
    assert_eq!(claims_of(&by_email).principal, kim);

    // The person reads the claimed sign-in back among their identities.
    let (status, _, ids) = send(&h.app, "GET", "/auth/identities", None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{ids}");
    assert!(ids.to_string().contains("80351110224678912"), "{ids}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stolen_token_cannot_link_because_it_cannot_complete_the_first_ceremony() {
    let h = harness().await;
    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;
    let _ = events(&h);

    // The thief holds Kim's token and nothing else. A token is not a
    // ceremony, so no window opens.
    let (status, answer) = post(&h, "/auth/claim/begin", json!({}), Some(&token)).await;
    assert_refused(status, &answer);
    // The thief's own sign-ins complete, and vouch for nobody but the thief.
    let own_discord = discord_ceremony(&h, "code-mallory", mallory()).await;
    let (status, answer) = post(&h, "/auth/claim/begin", own_discord, Some(&token)).await;
    assert_refused(status, &answer);
    let own_email = email_ceremony(&h, "mallory@example.net", json!({})).await;
    let (status, answer) = post(&h, "/auth/claim/begin", own_email, Some(&token)).await;
    assert_refused(status, &answer);
    // With no window, the claimed ceremony goes nowhere.
    let second = discord_ceremony(&h, "code-mallory-2", mallory()).await;
    let (status, answer) = claim(&h, &token, "a-claim-nobody-opened", second).await;
    assert_refused(status, &answer);

    assert_eq!(held(&h, &kim), 1, "nothing was linked to Kim");
    assert_eq!(owner_of(&h, "oauth2:discord:99999999999999999"), None);
    let r = reasons(&events(&h));
    assert!(r.contains(&"malformed-request".to_string()), "{r:?}");
    assert_eq!(
        r.iter()
            .filter(|x| *x == "first-ceremony-not-this-account")
            .count(),
        2,
        "{r:?}"
    );
    assert!(r.contains(&"claim-window-closed".to_string()), "{r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verified_email_match_links_nothing() {
    let h = harness().await;
    // Nelly signs up with her address, then signs in with a Discord account
    // whose verified address is the same.
    let by_email = email_sign_in(&h, "nelly@example.com").await;
    let first = claims_of(&by_email).principal;
    let (status, answer) = sign_in(&h.app, &h.fake, "discord", "code-1", nelly()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let second = claims_of(answer["token"].as_str().unwrap()).principal;

    assert_ne!(first, second, "a verified email match joins nothing");
    assert_eq!(held(&h, &first), 1);
    assert_eq!(held(&h, &second), 1);
    assert_eq!(owner_of(&h, NELLY_DISCORD), Some(second));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_matching_verified_email_does_not_stand_in_for_the_first_ceremony() {
    let h = harness().await;
    let token = email_sign_in(&h, "nelly@example.com").await;
    let nelly_id = claims_of(&token).principal;
    // The Discord ceremony vouches for the same verified address. It is not
    // a sign-in the account holds, so it opens no window.
    let discord = discord_ceremony(&h, "code-1", nelly()).await;
    let (status, answer) = post(&h, "/auth/claim/begin", discord, Some(&token)).await;
    assert_refused(status, &answer);
    assert_eq!(held(&h, &nelly_id), 1);
    assert_eq!(owner_of(&h, NELLY_DISCORD), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_into_a_deleted_account_is_refused() {
    let h = harness().await;
    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;
    let window = open(&h, &token, KIM).await;
    // The account is deleted while the window is open.
    h.dir.mark_deleted(&kim, Timestamp(now(&h))).unwrap();

    let second = discord_ceremony(&h, "code-1", nelly()).await;
    let (status, answer) = claim(&h, &token, &window, second).await;
    assert_refused(status, &answer);
    assert_eq!(owner_of(&h, NELLY_DISCORD), None, "nothing revived it");
    let r = reasons(&events(&h));
    assert!(r.contains(&"identity-deleted".to_string()), "{r:?}");

    // A deleted account opens no window either: its address is mailed
    // nothing (D4), and a Discord it never held vouches for nobody.
    let _ = email_begin(&h, KIM, json!({})).await;
    assert_eq!(deliver(&h), vec![None]);
    let other = discord_ceremony(&h, "code-2", mallory()).await;
    let (status, answer) = post(&h, "/auth/claim/begin", other, Some(&token)).await;
    assert_refused(status, &answer);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_accounts_sign_in_cannot_be_claimed_out_of_it() {
    let h = harness().await;
    let (status, answer) = sign_in(&h.app, &h.fake, "discord", "code-old", nelly()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let old = claims_of(answer["token"].as_str().unwrap()).principal;
    h.dir.mark_deleted(&old, Timestamp(NOW)).unwrap();

    let token = email_sign_in(&h, KIM).await;
    let window = open(&h, &token, KIM).await;
    let second = discord_ceremony(&h, "code-claim", nelly()).await;
    let (status, answer) = claim(&h, &token, &window, second).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["error"]["code"], "identifier_taken", "{answer}");
    assert_eq!(
        owner_of(&h, NELLY_DISCORD),
        Some(old),
        "it stays where it was"
    );
    let r = reasons(&events(&h));
    assert!(r.iter().any(|x| x.starts_with("identity-deleted")), "{r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_that_names_another_account_is_not_taken_from_it() {
    let h = harness().await;
    let (status, answer) = sign_in(&h.app, &h.fake, "discord", "code-1", nelly()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let nelly_id = claims_of(answer["token"].as_str().unwrap()).principal;

    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;
    let window = open(&h, &token, KIM).await;
    let second = discord_ceremony(&h, "code-2", nelly()).await;
    let (status, answer) = claim(&h, &token, &window, second).await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["error"]["code"], "identifier_taken", "{answer}");
    assert_eq!(owner_of(&h, NELLY_DISCORD), Some(nelly_id));
    assert_eq!(held(&h, &kim), 1);
    assert!(reasons(&events(&h)).contains(&"identifier-taken".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_after_its_window_is_refused() {
    let h = harness().await;
    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;
    let window = open(&h, &token, KIM).await;
    h.clock
        .store(NOW + claim::DEFAULT_SECS + 1, Ordering::SeqCst);

    let second = discord_ceremony(&h, "code-1", nelly()).await;
    let (status, answer) = claim(&h, &token, &window, second).await;
    assert_refused(status, &answer);
    assert_eq!(held(&h, &kim), 1);
    assert!(reasons(&events(&h)).contains(&"claim-window-closed".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_links_once() {
    let h = harness().await;
    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;
    let window = open(&h, &token, KIM).await;
    let second = discord_ceremony(&h, "code-1", nelly()).await;
    let (status, answer) = claim(&h, &token, &window, second).await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    let third = discord_ceremony(&h, "code-2", mallory()).await;
    let (status, answer) = claim(&h, &token, &window, third).await;
    assert_refused(status, &answer);
    assert_eq!(held(&h, &kim), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_belongs_to_the_session_that_opened_it() {
    let h = harness().await;
    let one = email_sign_in(&h, KIM).await;
    let two = email_sign_in(&h, KIM).await;
    let kim = claims_of(&one).principal;
    let window = open(&h, &one, KIM).await;

    let second = discord_ceremony(&h, "code-1", nelly()).await;
    let (status, answer) = claim(&h, &two, &window, second).await;
    assert_refused(status, &answer);
    assert_eq!(held(&h, &kim), 1);
    assert!(reasons(&events(&h)).contains(&"claim-of-another-session".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claimed_address_is_mailed_under_invite_only_and_a_wrong_code_spends_nothing() {
    let h = harness_with(Provisioning::InviteOnly, Linking::default()).await;
    // Kim's account exists, as an invitation or an import would leave it.
    let kim = {
        let mut dir = h.auth.directory.lock().unwrap();
        let p = dir
            .create_principal(PrincipalKind::Human, "Kim", Timestamp(NOW))
            .unwrap();
        dir.link_first(&p, &AuthenticatorId(format!("email:{KIM}")), Timestamp(NOW))
            .unwrap();
        p
    };
    let token = email_sign_in(&h, KIM).await;
    assert_eq!(claims_of(&token).principal, kim);
    const WORK: &str = "kim@work.example";

    // Nobody holds the work address, so under invite-only it is not mailed.
    let _ = email_begin(&h, WORK, json!({})).await;
    assert_eq!(deliver(&h), vec![None]);

    // Named in an open claim, it is.
    let window = open(&h, &token, KIM).await;
    let (challenge, binder) = email_begin(&h, WORK, json!({"claim": window})).await;
    let code = deliver(&h)[0]
        .clone()
        .expect("the claimed address is mailed");
    let flipped = if code.starts_with('0') { '1' } else { '0' };
    let wrong = format!("{flipped}{}", &code[1..]);
    let ceremony = |c: &str| {
        json!({"method": "email", "kind": "email_code",
               "challenge": challenge, "binder": binder, "code": c})
    };
    let (status, answer) = claim(&h, &token, &window, ceremony(&wrong)).await;
    assert_refused(status, &answer);
    // The window is still open: a mistyped code spends nothing.
    let (status, answer) = claim(&h, &token, &window, ceremony(&code)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["linked"], format!("email:{WORK}"));
    assert_eq!(owner_of(&h, &format!("email:{WORK}")), Some(kim));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hand_off_session_claims_only_through_two_fresh_ceremonies() {
    let h = harness().await;
    let kim = claims_of(&email_sign_in(&h, KIM).await).principal;
    // A session that came by subdomain hand-off: no presence, `via=handoff`.
    let session = ordo_identity::SessionClaim::starting_at(Timestamp(NOW))
        .with_via(ordo_host::auth::handoff::VIA.to_string());
    let spec = ordo_identity::TokenSpec {
        principal: kim.clone(),
        expires: Timestamp(NOW + 3600),
        client: None,
        user_presence_at: None,
        capability: None,
        delegation: None,
        session: Some(session),
    };
    let handed = ordo_identity::mint(&TokenKey::from_secret(SECRET), &spec);

    // The hand-off session alone opens nothing.
    let (status, answer) = post(&h, "/auth/claim/begin", json!({}), Some(&handed)).await;
    assert_refused(status, &answer);
    // With a fresh ceremony of Kim's own, it claims, and gains nothing else.
    let window = open(&h, &handed, KIM).await;
    let second = discord_ceremony(&h, "code-1", nelly()).await;
    let (status, answer) = claim(&h, &handed, &window, second).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(answer.get("token").is_none());
    assert_eq!(owner_of(&h, NELLY_DISCORD), Some(kim));
    assert!(claims_of(&handed).user_presence_at.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_takes_no_passkey_assertion_and_a_registration_opens_no_window() {
    let h = harness().await;
    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;
    let window = open(&h, &token, KIM).await;
    let assertion = json!({"method": "passkey", "kind": "passkey_assertion",
        "challenge": "c", "credential_id": "AA", "client_data_json": "AA",
        "authenticator_data": "AA", "signature": "AA"});
    let (status, answer) = claim(&h, &token, &window, assertion).await;
    assert_refused(status, &answer);
    let registration = json!({"method": "passkey", "kind": "passkey_registration",
        "challenge": "c", "client_data_json": "AA", "attestation_object": "AA"});
    let (status, answer) = post(&h, "/auth/claim/begin", registration, Some(&token)).await;
    assert_refused(status, &answer);
    assert_eq!(held(&h, &kim), 1);
    let r = reasons(&events(&h));
    assert!(
        r.contains(&"passkey-assertion-not-claimable".to_string()),
        "{r:?}"
    );
    assert!(
        r.contains(&"registration-is-not-a-sign-in".to_string()),
        "{r:?}"
    );
}

/// The creation options for a claim's passkey, and the registration a
/// browser would post for them.
async fn passkey_registration(
    h: &H,
    token: &str,
    window: &str,
    device: &TestAuthenticator,
) -> Value {
    let (status, options) = post(
        h,
        "/auth/claim/passkey",
        json!({"claim": window}),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let challenge = options["challenge"].as_str().expect("a challenge");
    registration_body(&device.register(challenge))
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

#[tokio::test(flavor = "multi_thread")]
async fn a_discord_only_account_adds_a_passkey_by_a_claim_and_it_proves_presence() {
    let h = harness().await;
    let (status, answer) = sign_in(&h.app, &h.fake, "discord", "code-1", nelly()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let token = answer["token"].as_str().unwrap().to_string();
    let nelly_id = claims_of(&token).principal;

    // A fresh Discord ceremony opens the window; the new passkey is the
    // claimed ceremony.
    let first = discord_ceremony(&h, "code-2", nelly()).await;
    let (status, opened) = post(&h, "/auth/claim/begin", first, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let window = opened["claim"].as_str().unwrap().to_string();
    let device = TestAuthenticator::es256();
    let body = passkey_registration(&h, &token, &window, &device).await;
    let (status, answer) = claim(&h, &token, &window, body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let added = device.authenticator_id().0;
    assert_eq!(answer["linked"], added, "{answer}");
    assert_eq!(answer["authenticators"], 2, "{answer}");
    assert!(answer.get("token").is_none(), "a claim mints no session");

    // Recorded, naming the sign-in that vouched for it.
    let seen = events(&h);
    assert!(
        seen.iter().any(|k| matches!(k,
            EventKind::AuthenticatorLinked { added: a, vouched_by }
                if *a == added && vouched_by.as_deref() == Some(NELLY_DISCORD))),
        "{seen:?}"
    );

    // From now on it is an ordinary passkey: it signs in as Nelly, and its
    // presence passes an op that requires presence.
    let (status, options) = post(&h, "/auth/begin", json!({"method": "passkey"}), None).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let challenge = options["challenge"].as_str().unwrap();
    let (status, done) = post(
        &h,
        "/auth/complete",
        assertion_body(&device.assert(challenge)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let by_passkey = done["token"].as_str().unwrap();
    let claims = claims_of(by_passkey);
    assert_eq!(claims.principal, nelly_id);
    assert!(claims.user_presence_at.is_some());
    let door = door_for(&nelly_id, &h);
    assert!(matches!(
        door.check(by_passkey, SPACE, GATED),
        AccessDecision::Allow { .. }
    ));
    assert_eq!(door.check(&token, SPACE, GATED), AccessDecision::Deny);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stolen_token_alone_cannot_add_a_passkey() {
    let h = harness().await;
    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;
    let device = TestAuthenticator::es256();

    // No window: no creation options, and no registration lands.
    let (status, answer) = post(
        &h,
        "/auth/claim/passkey",
        json!({"claim": "nobody-opened-this"}),
        Some(&token),
    )
    .await;
    assert_refused(status, &answer);
    let (status, options) = post(&h, "/auth/register", json!({"method": "passkey"}), None).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let unbound = registration_body(&device.register(options["challenge"].as_str().unwrap()));
    let (status, answer) = claim(&h, &token, "nobody-opened-this", unbound).await;
    assert_refused(status, &answer);

    // Even inside a real window, a registration begun anywhere else (here,
    // a sign-up) is not this account's and does not land.
    let window = open(&h, &token, KIM).await;
    let (_, options) = post(&h, "/auth/register", json!({"method": "passkey"}), None).await;
    let unbound = registration_body(&device.register(options["challenge"].as_str().unwrap()));
    let (status, answer) = claim(&h, &token, &window, unbound).await;
    assert_refused(status, &answer);

    assert_eq!(held(&h, &kim), 1);
    assert_eq!(owner_of(&h, &device.authenticator_id().0), None);
    let r = reasons(&events(&h));
    assert!(r.contains(&"claim-window-closed".to_string()), "{r:?}");
    assert!(r.contains(&"claim-binding".to_string()), "{r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn linking_switched_off_refuses_and_names_the_setting() {
    let off = Linking {
        on: false,
        ..Linking::default()
    };
    let h = harness_with(Provisioning::AutoOnboard, off).await;
    let token = email_sign_in(&h, KIM).await;
    let kim = claims_of(&token).principal;

    let first = email_ceremony(&h, KIM, json!({})).await;
    let (status, answer) = post(&h, "/auth/claim/begin", first, Some(&token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["error"]["code"], "linking_off", "{answer}");
    assert_eq!(answer["error"]["setting"], claim::SETTING, "{answer}");
    assert_eq!(answer["error"]["value"], "on", "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ORDO_AUTH_LINKING=on"),
        "{answer}"
    );
    let second = discord_ceremony(&h, "code-1", nelly()).await;
    let (status, answer) = claim(&h, &token, "any", second).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    // A passkey by claim obeys the same setting.
    let (status, answer) = post(
        &h,
        "/auth/claim/passkey",
        json!({"claim": "any"}),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
    assert_eq!(answer["error"]["setting"], claim::SETTING, "{answer}");
    assert_eq!(held(&h, &kim), 1);
    let r = reasons(&events(&h));
    assert!(
        r.iter().any(|x| x.contains("ORDO_AUTH_LINKING=on")),
        "{r:?}"
    );
}

#[test]
fn the_linking_settings_refuse_what_they_do_not_take_and_name_themselves() {
    let vars = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    };
    assert_eq!(claim::from_vars(&vars(&[])).unwrap(), Linking::default());
    assert!(
        Linking::default().on,
        "on unless the app developer says off"
    );
    assert_eq!(Linking::default().window_secs, 300);
    let off = claim::from_vars(&vars(&[("ORDO_AUTH_LINKING", "off")])).unwrap();
    assert!(!off.on);
    let (var, detail) = claim::from_vars(&vars(&[("ORDO_AUTH_LINKING", "maybe")])).unwrap_err();
    assert_eq!(var, "ORDO_AUTH_LINKING");
    assert!(detail.contains("`on` or `off`"), "{detail}");
    let (var, _) = claim::from_vars(&vars(&[("ORDO_AUTH_LINKING_SECS", "30")])).unwrap_err();
    assert_eq!(var, "ORDO_AUTH_LINKING_SECS");
    let wide = claim::from_vars(&vars(&[("ORDO_AUTH_LINKING_SECS", "900")])).unwrap();
    assert_eq!(wide.window_secs, 900);
}
