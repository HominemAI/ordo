//! OIDC sign-in served (`plan-auth-federated.md` step 9): a company login
//! through the real routes, the egress kernel and a real TLS issuer
//! (`oidc_support`) that serves discovery, a JWKS and a token endpoint and
//! signs real ID tokens. Nothing here calls Google or Microsoft.
//!
//! What it proves:
//!
//! - a sign-in through the issuer mints a token the door accepts, under the
//!   name the ID token carried and `via` `oidc:<connection>`;
//! - a `state` begun at one connection is refused at another's callback, and
//!   no code is exchanged (O14);
//! - a rotated key verifies without a restart, and the retired key stops (O9);
//! - a callback that carries a token in its query is refused (O1);
//! - a discovery document naming another issuer is never used (O15);
//! - the client secret appears in no answer and no event (O16).

mod oauth2_support;
mod oidc_support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use oauth2_support::{redeem, send};
use oidc_support::{param, FakeIdp, CLIENT_SECRET, RETURN};
use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::auth::AuthState;
use ordo_host::federation::MemoryIdentityAttrs;
use ordo_host::metrics::Metrics;
use ordo_host::second_factors::{SecondFactorConfig, SecondFactors, SecondStepLevels};
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::jose::testing::Signer;
use ordo_identity::second_step::{DeploymentMode, TenantMode};
use ordo_identity::{
    AccessEngine, AccessProfile, AuthenticatorId, MemoryCeremonyStore, MemorySessionRevocations,
    PresenceMode, PresenceRule, PrincipalId, ProviderRegistry, Role, SpaceId, Timestamp, TokenKey,
    UpstreamAcceptance,
};
use ordo_secevent::EventKind;
use serde_json::{json, Value};

const SECRET: &[u8] = b"auth-oidc-test-secret";
const NOW: u64 = 1_800_000_000;
const SPACE: &str = "spc_staff";
const PLAIN: &str = "posts.list";

struct H {
    auth: Arc<AuthState>,
    app: axum::Router,
    revocations: Arc<MemorySessionRevocations>,
    idp: FakeIdp,
    clock: Arc<AtomicU64>,
}

/// A deployment with two connections to the same issuer, `corp` and
/// `corp2`, as two apps registered at one company login would be.
async fn harness() -> H {
    let idp = FakeIdp::start().await;
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let mut registry = ProviderRegistry::new();
    let corp = idp.provider("corp", ceremonies.clone());
    let corp2 = idp.provider("corp2", ceremonies.clone());
    registry
        .register("corp", Box::new(corp.clone()))
        .expect("registered");
    registry
        .register("corp2", Box::new(corp2.clone()))
        .expect("registered");
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let revocations = Arc::new(MemorySessionRevocations::new());
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_ceremonies(ceremonies)
    .with_oidc(corp)
    .with_oidc(corp2)
    .with_identity_attrs(Arc::new(MemoryIdentityAttrs::default()));
    let clock = Arc::new(AtomicU64::new(NOW));
    let c = clock.clone();
    state.now = Box::new(move || c.load(Ordering::SeqCst));
    let auth = Arc::new(state);
    let app = ordo_host::auth::router(Some(auth.clone()));
    H {
        auth,
        app,
        revocations,
        idp,
        clock,
    }
}

/// The page's `begin`: `(authorize URL, state, binder)`.
async fn begin(h: &H, connection: &str) -> (String, String, String) {
    let (status, _, begun) = send(
        &h.app,
        "POST",
        "/auth/begin",
        Some(json!({"method": connection, "return_to": RETURN})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let url = begun["redirect"].as_str().expect("a redirect").to_string();
    assert!(
        url.starts_with(&format!("{}/authorize?", h.idp.issuer())),
        "{url}"
    );
    assert_eq!(param(&url, "response_type").as_deref(), Some("code"));
    assert_eq!(
        param(&url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert!(param(&url, "scope").unwrap().contains("openid"));
    let binder = begun["binder"].as_str().expect("a binder").to_string();
    assert!(!url.contains(&binder), "the binder never travels in a URL");
    (url.clone(), param(&url, "state").expect("a state"), binder)
}

/// The browser returning to `/auth/callback/<connection>`.
async fn callback(h: &H, connection: &str, query: &str) -> (StatusCode, Option<String>) {
    let (status, location, _) = send(
        &h.app,
        "GET",
        &format!("/auth/callback/{connection}?{query}"),
        None,
        None,
    )
    .await;
    (status, location)
}

fn q(state: &str, code: &str) -> String {
    format!(
        "state={}&code={}",
        ordo_identity::federation::percent(state),
        ordo_identity::federation::percent(code)
    )
}

fn nelly(at: u64) -> Value {
    json!({"sub": "00u1a2b3c4d5e6f7g8h9", "name": "Nelly Example",
           "preferred_username": "nelly", "email": "nelly@corp.example",
           "email_verified": true, "iat": at})
}

/// The whole sign-in at `connection`, at the harness's current clock.
async fn sign_in(h: &H, connection: &str, code: &str) -> (StatusCode, Value) {
    let (url, state, binder) = begin(h, connection).await;
    h.idp
        .authorize(&url, code, nelly(h.clock.load(Ordering::SeqCst)));
    let (status, location) = callback(h, connection, &q(&state, code)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = location.expect("a location");
    assert!(location.starts_with(RETURN), "{location}");
    let Some(handoff) = param(&location, "handoff") else {
        return (StatusCode::UNAUTHORIZED, json!({"location": location}));
    };
    redeem(&h.app, connection, &handoff, &binder, json!({})).await
}

fn events(h: &H) -> Vec<EventKind> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 100_000))
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

fn door_for(principal: &PrincipalId, h: &H) -> IdentityAccess {
    let space = SpaceId(SPACE.into());
    let mut engine = AccessEngine::new();
    engine.set_profile(&space, AccessProfile::new().allow(Role::Owner, PLAIN));
    engine.set_membership(&space, principal, Role::Owner);
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW + 60),
        Arc::new(Metrics::new()),
    )
    .with_revocations(h.revocations.clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_through_a_real_tls_idp_mints_a_token_the_door_accepts() {
    let h = harness().await;
    let (status, done) = sign_in(&h, "corp", "code-1").await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["via"], "oidc:corp", "{done}");
    let token = done["token"].as_str().expect("a token");
    let claims = ordo_identity::verify(&TokenKey::from_secret(SECRET), token, Timestamp(NOW))
        .expect("a token this deployment minted");
    assert!(
        claims.user_presence_at.is_none(),
        "an outside provider proves no presence"
    );
    let door = door_for(&claims.principal, &h);
    assert_eq!(
        door.check(token, SPACE, PLAIN),
        AccessDecision::Allow {
            principal: claims.principal.0.clone(),
            agent: None,
        }
    );
    // The name is the ID token's, never the one the page posted, and the
    // link is `oidc:<connection>:<sub>`.
    {
        let dir = h.auth.directory.lock().unwrap();
        let p = dir.principal(&claims.principal).unwrap().expect("minted");
        assert_eq!(p.display, "Nelly Example");
        assert_eq!(
            dir.principal_for(&AuthenticatorId("oidc:corp:00u1a2b3c4d5e6f7g8h9".into()))
                .unwrap(),
            Some(claims.principal.clone())
        );
    }
    // The person reads the identity back, with the verified address.
    let (status, _, ids) = send(&h.app, "GET", "/auth/identities", None, Some(token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids[0]["connection"], "corp", "{ids}");
    assert_eq!(ids[0]["subject"], "00u1a2b3c4d5e6f7g8h9");
    assert_eq!(ids[0]["email"], "nelly@corp.example");
    // Discovery once, the keys once, one exchange carrying PKCE.
    {
        let st = h.idp.state.lock().unwrap();
        assert_eq!(
            st.lines
                .iter()
                .filter(|l| l.contains("openid-configuration"))
                .count(),
            1
        );
        assert_eq!(st.jwks_fetches, 1);
        assert!(st.token_requests[0].contains_key("code_verifier"));
    }
    // A second sign-in is the same person, from the cached keys.
    let (status, again) = sign_in(&h, "corp", "code-2").await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let second = ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        again["token"].as_str().unwrap(),
        Timestamp(NOW),
    )
    .unwrap();
    assert_eq!(second.principal, claims.principal);
    assert_eq!(h.idp.state.lock().unwrap().jwks_fetches, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_from_one_connection_is_refused_at_another() {
    let h = harness().await;
    let (url, state, binder) = begin(&h, "corp").await;
    h.idp.authorize(&url, "code-1", nelly(NOW));
    // The callback arrives at corp2's path with corp's state.
    let (status, location) = callback(&h, "corp2", &q(&state, "code-1")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        location.is_none(),
        "no ceremony, nowhere to send the browser"
    );
    assert!(
        h.idp.state.lock().unwrap().token_requests.is_empty(),
        "no code was exchanged"
    );
    assert!(failure_reasons(&h).iter().any(|r| r == "unknown-state"));
    // The ceremony stays its own connection's, and completes there.
    let (status, location) = callback(&h, "corp", &q(&state, "code-1")).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let handoff = param(&location.unwrap(), "handoff").expect("a handoff");
    // Its handoff is not redeemable at the other connection either.
    let (status, _) = redeem(&h.app, "corp2", &handoff, &binder, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, done) = redeem(&h.app, "corp", &handoff, &binder, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rotated_key_verifies_without_a_restart() {
    let h = harness().await;
    let (status, _) = sign_in(&h, "corp", "code-1").await;
    assert_eq!(status, StatusCode::OK);
    // The issuer rotates from its RSA key to a new P-256 key and withdraws
    // the old one. A minute later (the refetch floor) the new key verifies.
    h.idp.rotate_to("ec-2", Signer::es256(7));
    h.clock.store(NOW + 61, Ordering::SeqCst);
    let (status, done) = sign_in(&h, "corp", "code-2").await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(h.idp.state.lock().unwrap().jwks_fetches, 2);
    // A token under the withdrawn key no longer verifies: it left the set
    // at that fetch, and the refetch floor keeps a spray of kids from
    // hammering the issuer.
    h.idp.state.lock().unwrap().keys.push((
        "rsa-1".into(),
        Arc::new(Signer::rsa(include_bytes!(
            "../../ordo-identity/tests/fixture/rsa-2048.pk8"
        ))),
    ));
    h.idp.state.lock().unwrap().signing = "rsa-1".into();
    h.clock.store(NOW + 62, Ordering::SeqCst);
    let (status, refused) = sign_in(&h, "corp", "code-3").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused}");
    assert_eq!(h.idp.state.lock().unwrap().jwks_fetches, 2);
    assert!(failure_reasons(&h).iter().any(|r| r == "kid-unknown"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_callback_carrying_a_token_in_its_query_is_refused() {
    let h = harness().await;
    let (url, state, _) = begin(&h, "corp").await;
    h.idp.authorize(&url, "code-1", nelly(NOW));
    let query = format!("{}&id_token=eyJhbGciOiJub25lIn0.e30.", q(&state, "code-1"));
    let (status, location) = callback(&h, "corp", &query).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = location.unwrap();
    assert_eq!(
        param(&location, "error").as_deref(),
        Some("authentication_failed")
    );
    assert!(h.idp.state.lock().unwrap().token_requests.is_empty());
    assert!(failure_reasons(&h)
        .iter()
        .any(|r| r == "front-channel-token"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_discovery_document_naming_another_issuer_is_never_used() {
    let h = harness().await;
    h.idp.state.lock().unwrap().advertised_issuer = Some("https://evil.example.com".into());
    let (status, _, body) = send(
        &h.app,
        "POST",
        "/auth/begin",
        Some(json!({"method": "corp", "return_to": RETURN})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "identity_provider_unavailable");
    assert_eq!(body["error"]["connection"], "corp");
    assert_eq!(body["error"]["setting"], "ORDO_AUTH_CORP_ISSUER");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("connection `corp`"),
        "{body}"
    );
    assert!(failure_reasons(&h)
        .iter()
        .any(|r| r == "identity-provider-unavailable:discovery-issuer-mismatch"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_secret_appears_in_no_answer_and_no_event() {
    let h = harness().await;
    let (_, done) = sign_in(&h, "corp", "code-1").await;
    // A refused one too: a code the issuer never issued.
    let (_, state, _) = begin(&h, "corp").await;
    let (_, location) = callback(&h, "corp", &q(&state, "made-up")).await;
    let all = format!("{done} {location:?} {:?}", events(&h));
    assert!(!all.contains(CLIENT_SECRET));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_issuer_on_a_private_address_names_the_setting_that_lets_it_through() {
    let idp = FakeIdp::start().await;
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    // The deployment's deny list as shipped: loopback is private.
    let corp = idp.provider_with(
        "corp",
        ceremonies.clone(),
        ordo_egress::DenyList::categories_only(),
    );
    let mut registry = ProviderRegistry::new();
    registry
        .register("corp", Box::new(corp.clone()))
        .expect("registered");
    let state = AuthState::new(
        registry,
        Directory::with_store(
            Provisioning::AutoOnboard,
            Arc::new(ordo_identity::MemoryDirectoryStore::new()),
        ),
        TokenKey::from_secret(SECRET),
    )
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_ceremonies(ceremonies)
    .with_oidc(corp);
    let app = ordo_host::auth::router(Some(Arc::new(state)));
    let (status, _, body) = send(
        &app,
        "POST",
        "/auth/begin",
        Some(json!({"method": "corp", "return_to": RETURN})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["setting"], "ORDO_EGRESS_PRIVATE_HOSTS");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("connection `corp`")
            && message.contains("ORDO_EGRESS_PRIVATE_HOSTS=localhost"),
        "{message}"
    );
    assert!(
        idp.state.lock().unwrap().lines.is_empty(),
        "nothing reached the issuer"
    );
}

// ── step 10: a provider's two-step claim as presence (§8.2) ──────────────

const HOME: &str = "spc_home";
const OTHER: &str = "spc_other";
const EXPORT: &str = "std.space.export";
const DELETE: &str = "std.space.delete";
const APPLY: &str = "std.deployment.apply";

fn rule(mode: PresenceMode) -> PresenceRule {
    PresenceRule {
        mode,
        ..PresenceRule::default()
    }
}

/// Three connections to one issuer: `corp` counts everywhere (`on`). `team`
/// and `plain` are the default (`off`): a tenant may accept either for its
/// own data (ask 10), and the tests have one tenant accept `team`.
/// The acceptance is what `presence::acceptance` builds at boot.
async fn presence_harness() -> (H, Arc<UpstreamAcceptance>) {
    let idp = FakeIdp::start().await;
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let mut registry = ProviderRegistry::new();
    let corp = idp.provider_with_presence("corp", ceremonies.clone(), rule(PresenceMode::On));
    let team = idp.provider_with_presence("team", ceremonies.clone(), rule(PresenceMode::Off));
    let plain = idp.provider("plain", ceremonies.clone());
    for p in [&corp, &team, &plain] {
        registry
            .register(p.name(), Box::new(p.clone()))
            .expect("registered");
    }
    let revocations = Arc::new(MemorySessionRevocations::new());
    let mut state = AuthState::new(
        registry,
        Directory::with_store(
            Provisioning::AutoOnboard,
            Arc::new(ordo_identity::MemoryDirectoryStore::new()),
        ),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_ceremonies(ceremonies)
    .with_oidc(corp.clone())
    .with_oidc(team.clone())
    .with_oidc(plain.clone())
    .with_identity_attrs(Arc::new(MemoryIdentityAttrs::default()));
    let clock = Arc::new(AtomicU64::new(NOW));
    let c = clock.clone();
    state.now = Box::new(move || c.load(Ordering::SeqCst));
    let auth = Arc::new(state);
    let app = ordo_host::auth::router(Some(auth.clone()));
    let acceptance = Arc::new(ordo_host::presence::acceptance(
        [corp.connection(), team.connection(), plain.connection()],
        HOME,
    ));
    (
        H {
            auth,
            app,
            revocations,
            idp,
            clock,
        },
        acceptance,
    )
}

/// The page's `begin`, asking for a two-step claim as presence.
async fn begin_presence(h: &H, connection: &str) -> (StatusCode, Value) {
    let (status, _, body) = send(
        &h.app,
        "POST",
        "/auth/begin",
        Some(json!({"method": connection, "return_to": RETURN, "presence": true})),
        None,
    )
    .await;
    (status, body)
}

/// A whole sign-in at `connection`, asking for presence or not, where the
/// issuer answers with `amr` and an `auth_time` `age` seconds ago.
async fn sign_in_claiming(
    h: &H,
    connection: &str,
    code: &str,
    presence: bool,
    amr: &[&str],
    age: u64,
) -> Value {
    sign_in_with(h, connection, code, presence, amr, age, json!({})).await
}

/// [`sign_in_claiming`], with `extra` in the completion (a `space`).
async fn sign_in_with(
    h: &H,
    connection: &str,
    code: &str,
    presence: bool,
    amr: &[&str],
    age: u64,
    extra: Value,
) -> Value {
    let (url, state, binder) = if presence {
        let (status, begun) = begin_presence(h, connection).await;
        assert_eq!(status, StatusCode::OK, "{begun}");
        let url = begun["redirect"].as_str().expect("a redirect").to_string();
        assert_eq!(
            param(&url, "max_age").as_deref(),
            Some("0"),
            "the provider is asked to prompt now"
        );
        let state = param(&url, "state").expect("a state");
        (url, state, begun["binder"].as_str().unwrap().to_string())
    } else {
        let (url, state, binder) = begin(h, connection).await;
        assert!(param(&url, "max_age").is_none(), "{url}");
        (url, state, binder)
    };
    let now = h.clock.load(Ordering::SeqCst);
    let mut claims = nelly(now);
    claims["amr"] = json!(amr);
    claims["auth_time"] = json!(now - age);
    h.idp.authorize(&url, code, claims);
    let (status, location) = callback(h, connection, &q(&state, code)).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let handoff = param(&location.expect("a location"), "handoff").expect("a handoff");
    let (status, done) = redeem(&h.app, connection, &handoff, &binder, extra).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done
}

fn upstream_of(done: &Value) -> Option<ordo_identity::UpstreamPresence> {
    let token = done["token"].as_str().expect("a token");
    ordo_identity::verify(&TokenKey::from_secret(SECRET), token, Timestamp(NOW))
        .expect("a token this deployment minted")
        .session
        .and_then(|s| s.upstream_presence)
}

/// A door where the person owns `SPACE`, `OTHER` and the deployment's own
/// space, every platform op asks for presence, and the acceptance is the
/// deployment's.
fn presence_door(
    principal: &PrincipalId,
    acceptance: &Arc<UpstreamAcceptance>,
    at: u64,
) -> IdentityAccess {
    let mut engine = AccessEngine::new();
    for space in [SPACE, OTHER, HOME] {
        let space = SpaceId(space.into());
        engine.set_profile(
            &space,
            AccessProfile::new()
                .allow(Role::Owner, EXPORT)
                .allow(Role::Owner, DELETE)
                .allow(Role::Owner, APPLY),
        );
        engine.set_membership(&space, principal, Role::Owner);
    }
    for op in [EXPORT, DELETE, APPLY] {
        engine.require_step_up(op);
    }
    engine.accept_upstream(acceptance.clone());
    IdentityAccess::new(
        engine,
        TokenKey::from_secret(SECRET),
        Box::new(move || at),
        Arc::new(Metrics::new()),
    )
}

fn allowed(door: &IdentityAccess, done: &Value, space: &str, op: &str) -> bool {
    matches!(
        door.check(done["token"].as_str().unwrap(), space, op),
        AccessDecision::Allow { .. }
    )
}

fn principal_of(done: &Value) -> PrincipalId {
    ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        done["token"].as_str().unwrap(),
        Timestamp(NOW),
    )
    .unwrap()
    .principal
}

fn upstream_acts(h: &H) -> Vec<(String, String)> {
    events(h)
        .into_iter()
        .filter_map(|k| match k {
            EventKind::UpstreamClaim { act, detail } => Some((act, detail)),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_two_step_claim_from_an_accepted_connection_exports_a_space_and_a_stale_one_does_not(
) {
    let (h, acceptance) = presence_harness().await;
    // Fresh: `mfa`, five seconds ago, from `corp`, whose setting is `on`.
    let fresh = sign_in_claiming(&h, "corp", "code-fresh", true, &["pwd", "mfa"], 5).await;
    let up = upstream_of(&fresh).expect("the claim counted");
    assert_eq!((up.via(), up.at()), ("corp", Timestamp(NOW - 5)));
    let who = principal_of(&fresh);
    let door = presence_door(&who, &acceptance, NOW + 10);
    assert!(
        allowed(&door, &fresh, SPACE, EXPORT),
        "fresh: the export runs"
    );
    assert!(
        allowed(&door, &fresh, HOME, APPLY),
        "`on` reaches the deployment's own acts"
    );
    // The same token, used after the window: the claim has aged out.
    let later = presence_door(&who, &acceptance, NOW + 400);
    assert!(!allowed(&later, &fresh, SPACE, EXPORT), "aged out at use");

    // Stale: the provider's `auth_time` is past the window at the callback.
    let stale = sign_in_claiming(&h, "corp", "code-stale", true, &["mfa"], 301).await;
    assert_eq!(upstream_of(&stale), None, "a stale claim mints nothing");
    assert!(!allowed(&door, &stale, SPACE, EXPORT), "stale: refused");

    // The stream says which connection vouched, and why the other did not.
    let acts = upstream_acts(&h);
    assert!(
        acts.iter().any(|(act, d)| act == "presence-vouched"
            && d.contains("connection=oidc:corp")
            && d.contains("vouched_by=corp")
            && d.contains("amr=pwd,mfa")),
        "{acts:?}"
    );
    assert!(
        acts.iter().any(|(act, d)| act == "presence-not-minted"
            && d.contains("connection=corp")
            && d.contains("reason=presence-stale")),
        "{acts:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenants_acceptance_does_not_reach_a_deployment_op() {
    let (h, acceptance) = presence_harness().await;
    // `team` is `off`. This tenant accepts it; no other does.
    acceptance.set_tenant(&SpaceId(SPACE.into()), ["team".to_string()].into());
    let done = sign_in_claiming(&h, "team", "code-team", true, &["mfa"], 5).await;
    assert_eq!(
        upstream_of(&done).map(|u| u.via().to_string()),
        Some("team".into())
    );
    let door = presence_door(&principal_of(&done), &acceptance, NOW + 10);
    assert!(
        allowed(&door, &done, SPACE, EXPORT),
        "the tenant's own data: its acceptance counts"
    );
    assert!(
        !allowed(&door, &done, SPACE, DELETE),
        "deleting the tenant is an act on the deployment"
    );
    assert!(
        !allowed(&door, &done, HOME, APPLY),
        "the deployment's own op follows the deployment's setting"
    );
    assert!(
        !allowed(&door, &done, HOME, EXPORT),
        "nothing in the deployment's own space"
    );
    assert!(
        !allowed(&door, &done, OTHER, EXPORT),
        "another tenant did not accept it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_not_accepted_mints_no_presence_even_with_a_fresh_mfa_claim() {
    let (h, acceptance) = presence_harness().await;
    // A sign-in at `plain` that does not ask carries no claim, however fresh
    // the provider's `mfa`.
    let done = sign_in_claiming(&h, "plain", "code-plain", false, &["mfa"], 1).await;
    assert_eq!(upstream_of(&done), None);
    let door = presence_door(&principal_of(&done), &acceptance, NOW + 10);
    assert!(!allowed(&door, &done, SPACE, EXPORT));
    // `team` is `off` and no tenant accepted it: the claim is in the token
    // and counts nowhere.
    let team = sign_in_claiming(&h, "team", "code-team", true, &["mfa"], 1).await;
    assert!(upstream_of(&team).is_some());
    let door = presence_door(&principal_of(&team), &acceptance, NOW + 10);
    for (space, op) in [(SPACE, EXPORT), (OTHER, EXPORT), (HOME, APPLY)] {
        assert!(!allowed(&door, &team, space, op), "{space} {op}");
    }
    // A sign-in that did not ask carries nothing even from `on`.
    let unasked = sign_in_claiming(&h, "corp", "code-unasked", false, &["mfa"], 1).await;
    assert_eq!(upstream_of(&unasked), None, "no max_age=0, no presence");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_single_factor_claim_mints_no_presence() {
    let (h, acceptance) = presence_harness().await;
    let done = sign_in_claiming(&h, "corp", "code-pwd", true, &["pwd"], 1).await;
    assert_eq!(upstream_of(&done), None);
    let door = presence_door(&principal_of(&done), &acceptance, NOW + 10);
    assert!(!allowed(&door, &done, SPACE, EXPORT));
    let acts = upstream_acts(&h);
    assert!(
        acts.iter()
            .any(|(act, d)| act == "presence-not-minted" && d.contains("presence-single-factor")),
        "{acts:?}"
    );
    assert!(
        acts.iter()
            .any(|(act, d)| act == "noted" && d.contains("amr=pwd")),
        "the claims are recorded whether or not they counted: {acts:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_presence_setting_is_off_by_default() {
    let idp = FakeIdp::start().await;
    assert_eq!(idp.connection("corp").presence.mode, PresenceMode::Off);
    assert_eq!(PresenceRule::default().mode, PresenceMode::Off);
    // And an acceptance built from a default connection counts nothing.
    let acceptance = ordo_host::presence::acceptance([&idp.connection("corp")], HOME);
    assert!(!acceptance.accepts(&SpaceId(SPACE.into()), EXPORT, "corp"));
}

// ── federated asks 10 and 11 (owner, 2026-10-09) ────────────────────────

/// **Ask 10.** A tenant may accept a connection the app developer left
/// `off`. Its claim then counts inside that tenant, for its own data, and
/// nowhere else.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_accepts_a_connection_the_app_developer_left_off_and_gets_presence_only_inside_it()
{
    let (h, acceptance) = presence_harness().await;
    assert_eq!(acceptance.mode("plain"), PresenceMode::Off);
    acceptance.set_tenant(&SpaceId(SPACE.into()), ["plain".to_string()].into());
    let done = sign_in_claiming(&h, "plain", "code-plain-t", true, &["mfa"], 5).await;
    assert_eq!(
        upstream_of(&done).map(|u| u.via().to_string()),
        Some("plain".into())
    );
    let door = presence_door(&principal_of(&done), &acceptance, NOW + 10);
    assert!(
        allowed(&door, &done, SPACE, EXPORT),
        "the tenant that accepted it"
    );
    assert!(!allowed(&door, &done, OTHER, EXPORT), "another tenant");
    assert!(!allowed(&door, &done, SPACE, DELETE), "its own delete");
    assert!(
        !allowed(&door, &done, HOME, EXPORT),
        "the deployment's own space"
    );
    assert!(!allowed(&door, &done, HOME, APPLY), "a deployment op");
}

/// A second-step method: the person holds a factor of it, or not. No
/// answer is ever right, so a step owed is a step never passed here.
struct Factor(bool);

impl ordo_host::second_factors::SecondStepMethod for Factor {
    fn kind(&self) -> ordo_identity::SecondStepKind {
        ordo_identity::SecondStepKind::Totp
    }
    fn holds(&self, _principal: &str) -> Result<bool, ordo_identity::StoreFault> {
        Ok(self.0)
    }
    fn verify(
        &self,
        _principal: &str,
        _body: &Value,
        _now: u64,
    ) -> Result<ordo_host::second_factors::Verdict, ordo_identity::StoreFault> {
        Ok(ordo_host::second_factors::Verdict::Rejected { locked: false })
    }
}

/// Give the routes the second step under `mode`, the person holding a
/// factor or not, with the levels reading the deployment's acceptance as
/// `assemble` wires them.
fn wire_second_step(
    h: &H,
    acceptance: &Arc<UpstreamAcceptance>,
    mode: DeploymentMode,
    holds: bool,
) -> Arc<SecondStepLevels> {
    let levels = Arc::new(SecondStepLevels::new(mode).with_upstream(Some(acceptance.clone())));
    let config = SecondFactorConfig {
        mode,
        ..SecondFactorConfig::default()
    };
    h.auth.wire_second_factors(Arc::new(
        SecondFactors::new(config)
            .with_levels(levels.clone())
            .with_method(Arc::new(Factor(holds))),
    ));
    levels
}

fn claims_of(done: &Value) -> ordo_identity::TokenClaims {
    ordo_identity::verify(
        &TokenKey::from_secret(SECRET),
        done["token"].as_str().expect("a token"),
        Timestamp(NOW),
    )
    .unwrap()
}

/// **Ask 11.** The deployment requires a second step and the person holds a
/// factor. A fresh `mfa` claim from `corp`, which the app developer set
/// `on`, stands in for it: a token at once, the door lets it in, and the
/// stream names the connection.
#[tokio::test(flavor = "multi_thread")]
async fn an_accepted_fresh_mfa_claim_skips_the_second_step() {
    let (h, acceptance) = presence_harness().await;
    let levels = wire_second_step(&h, &acceptance, DeploymentMode::Required, true);
    let done = sign_in_claiming(&h, "corp", "code-skip", true, &["pwd", "mfa"], 5).await;
    assert!(done.get("second_step").is_none(), "{done}");
    let claims = claims_of(&done);
    assert_eq!(claims.session.as_ref().unwrap().second_step, None);
    for space in [SPACE, OTHER, HOME] {
        assert!(!levels.refuses(&claims, space), "the door, in {space}");
    }
    let acts = upstream_acts(&h);
    assert!(
        acts.iter().any(|(act, d)| act == "second-step-satisfied"
            && d.contains("connection=corp")
            && d.contains("level=deployment")),
        "{acts:?}"
    );
}

/// A stale claim, a single-factor one, and a fresh one from a connection
/// nobody accepted each still go to Ordo's second step.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_a_single_factor_or_an_unaccepted_claim_still_takes_the_second_step() {
    let (h, acceptance) = presence_harness().await;
    wire_second_step(&h, &acceptance, DeploymentMode::Required, true);
    let cases: [(&str, &str, &[&str], u64, &str); 3] = [
        ("corp", "code-stale", &["mfa"], 301, "stale"),
        ("corp", "code-pwd", &["pwd"], 5, "single-factor"),
        ("plain", "code-plain", &["mfa"], 5, "not accepted"),
    ];
    for (connection, code, amr, age, why) in cases {
        let half = sign_in_claiming(&h, connection, code, true, amr, age).await;
        assert!(half["second_step"].is_string(), "{why}: {half}");
        assert!(half.get("token").is_none(), "{why}: {half}");
    }
    assert!(
        !upstream_acts(&h)
            .iter()
            .any(|(act, _)| act == "second-step-satisfied"),
        "nothing stood in"
    );
}

/// A tenant that requires a second step and accepts `plain` (left `off`)
/// lets a fresh `mfa` claim from it stand in for that tenant's step, and
/// for no other tenant's.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenants_acceptance_meets_its_own_second_step_and_no_other() {
    let (h, acceptance) = presence_harness().await;
    let levels = wire_second_step(&h, &acceptance, DeploymentMode::Enabled, false);
    levels.set(SPACE, TenantMode::Required);
    levels.set(OTHER, TenantMode::Required);
    acceptance.set_tenant(&SpaceId(SPACE.into()), ["plain".to_string()].into());
    let done = sign_in_with(
        &h,
        "plain",
        "code-tenant",
        true,
        &["mfa"],
        5,
        json!({ "space": SPACE }),
    )
    .await;
    assert!(done["token"].is_string(), "{done}");
    let claims = claims_of(&done);
    assert!(!levels.refuses(&claims, SPACE), "its own tenant");
    assert!(
        levels.refuses(&claims, OTHER),
        "another tenant's requirement"
    );
    let acts = upstream_acts(&h);
    assert!(
        acts.iter().any(|(act, d)| act == "second-step-satisfied"
            && d.contains("connection=plain")
            && d.contains("level=tenant")
            && d.contains(&format!("space={SPACE}"))),
        "{acts:?}"
    );
    // Naming the tenant that did not accept it: the step is owed.
    let half = sign_in_with(
        &h,
        "plain",
        "code-other",
        true,
        &["mfa"],
        5,
        json!({ "space": OTHER }),
    )
    .await;
    assert_eq!(half["enrol"], true, "{half}");
}
