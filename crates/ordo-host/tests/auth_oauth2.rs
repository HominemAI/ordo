//! OAuth2 sign-in served (`plan-auth-federated.md` step 3): Discord through
//! the real routes, the egress kernel and a real TLS listener standing in for
//! Discord (`oauth2_support`). Nothing here calls Discord.
//!
//! What it proves, by the names the plan gives:
//!
//! - a Discord sign-in mints a token the door accepts, under the name and the
//!   `via` the provider vouched for, and never a name the page posted;
//! - the subject is the configured claim and nothing else (A4);
//! - a mutable subject claim refuses the boot unless acknowledged (A3);
//! - a deleted account cannot sign in through Discord, and its Discord id
//!   mints nobody new (§2.7 D1, D2, D7);
//! - a deletion between the callback and the handoff mints no token (D3);
//! - a person reads their own linked identities and nobody else's;
//! - the client secret appears in no answer and no event.

mod oauth2_support;

use std::sync::Arc;

use axum::http::StatusCode;
use oauth2_support::{
    begin, callback, nelly, param, redeem, send, sign_in, FakeDiscord, CLIENT_SECRET, RETURN,
};
use ordo_api::{AccessChecker, AccessDecision};
use ordo_host::auth::AuthState;
use ordo_host::federation::MemoryIdentityAttrs;
use ordo_host::metrics::Metrics;
use ordo_host::IdentityAccess;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{
    AccessEngine, AccessProfile, AuthenticatorId, DirectoryStore, MemoryCeremonyStore,
    MemorySessionRevocations, PrincipalId, ProviderRegistry, Role, SpaceId, Timestamp, TokenKey,
};
use ordo_secevent::EventKind;
use serde_json::{json, Value};

const SECRET: &[u8] = b"auth-oauth2-test-secret";
const NOW: u64 = 1_800_000_000;
const SPACE: &str = "spc_guild";
const PLAIN: &str = "posts.list";
const GATED: &str = "space.export";

struct H {
    auth: Arc<AuthState>,
    app: axum::Router,
    store: Arc<ordo_identity::MemoryDirectoryStore>,
    revocations: Arc<MemorySessionRevocations>,
    fake: FakeDiscord,
}

async fn harness() -> H {
    let fake = FakeDiscord::start().await;
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let discord = fake.provider("discord", ceremonies.clone());
    let mut registry = ProviderRegistry::new();
    registry
        .register("discord", Box::new(discord.clone()))
        .expect("registered");
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let revocations = Arc::new(MemorySessionRevocations::new());
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store.clone()),
        TokenKey::from_secret(SECRET),
    )
    .with_revocations(revocations.clone())
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_ceremonies(ceremonies)
    .with_oauth2(discord)
    .with_identity_attrs(Arc::new(MemoryIdentityAttrs::default()));
    state.now = Box::new(|| NOW);
    let auth = Arc::new(state);
    let app = ordo_host::auth::router(Some(auth.clone()));
    H {
        auth,
        app,
        store,
        revocations,
        fake,
    }
}

fn principal_of(token: &str) -> ordo_identity::TokenClaims {
    ordo_identity::verify(&TokenKey::from_secret(SECRET), token, Timestamp(NOW))
        .expect("a token this deployment minted")
}

fn door_for(principal: &PrincipalId, h: &H) -> IdentityAccess {
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
        Box::new(|| NOW + 60),
        Arc::new(Metrics::new()),
    )
    .with_revocations(h.revocations.clone())
}

fn events(h: &H) -> Vec<EventKind> {
    h.auth
        .recorder
        .as_ref()
        .expect("recorder")
        .drain_all(ordo_commandlog::Timestamp(NOW + 10_000))
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

#[tokio::test(flavor = "multi_thread")]
async fn a_discord_sign_in_through_a_real_tls_listener_mints_a_token_the_door_accepts() {
    let h = harness().await;
    let (status, done) = sign_in(&h.app, &h.fake, "discord", "code-1", nelly()).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["via"], "oauth2:discord", "{done}");
    let token = done["token"].as_str().expect("a token");
    let claims = principal_of(token);
    assert_eq!(
        claims.session.as_ref().and_then(|s| s.via.as_deref()),
        Some("oauth2:discord"),
        "the way in is a claim under the MAC"
    );
    assert!(
        claims.user_presence_at.is_none(),
        "Discord proves no presence"
    );

    // The door lets the person in where they are a member, and the gated op
    // stays shut: presence comes from a passkey, not from Discord.
    let door = door_for(&claims.principal, &h);
    assert_eq!(
        door.check(token, SPACE, PLAIN),
        AccessDecision::Allow {
            principal: claims.principal.0.clone(),
            agent: None,
        }
    );
    assert_eq!(door.check(token, SPACE, GATED), AccessDecision::Deny);

    // The principal carries the name Discord vouched for, never the one the
    // page posted, and is linked under `oauth2:discord:<id>`.
    {
        let dir = h.auth.directory.lock().unwrap();
        let p = dir.principal(&claims.principal).unwrap().expect("minted");
        assert_eq!(p.display, "Nelly");
        assert_eq!(
            dir.principal_for(&AuthenticatorId("oauth2:discord:80351110224678912".into()))
                .unwrap(),
            Some(claims.principal.clone())
        );
    }

    // The exchange carried PKCE and the secret; the userinfo call carried the
    // access token from that exchange and nothing else.
    let seen = h.fake.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen[0].line.starts_with("POST /api/oauth2/token"));
    assert_eq!(seen[1].authorization.as_deref(), Some("Bearer at-code-1"));

    // A second sign-in is the same person, and mints nobody.
    let (status, again) = sign_in(&h.app, &h.fake, "discord", "code-2", nelly()).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(
        principal_of(again["token"].as_str().unwrap()).principal,
        claims.principal
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_subject_is_the_configured_claim_and_nothing_else() {
    let h = harness().await;
    // The same Discord id under a new username and a new address is the
    // same person: only `id` names them.
    let (_, first) = sign_in(&h.app, &h.fake, "discord", "c1", nelly()).await;
    let mut renamed = nelly();
    renamed["username"] = json!("someone-else");
    renamed["global_name"] = json!("Someone Else");
    renamed["email"] = json!("other@example.com");
    let (_, second) = sign_in(&h.app, &h.fake, "discord", "c2", renamed).await;
    assert_eq!(
        principal_of(first["token"].as_str().unwrap()).principal,
        principal_of(second["token"].as_str().unwrap()).principal
    );

    // A profile whose `id` is not a string or an integer names nobody: the
    // browser goes back with the one error word and no handoff.
    for bad in [json!(1.5), json!(null), json!({"id": 1}), json!("")] {
        let mut profile = nelly();
        profile["id"] = bad.clone();
        let (state, _) = begin(&h.app, "discord").await;
        h.fake.authorize("bad", profile);
        let location = callback(&h.app, "discord", &state, "bad").await;
        assert_eq!(
            param(&location, "error").as_deref(),
            Some("authentication_failed"),
            "{bad}: {location}"
        );
        assert!(param(&location, "handoff").is_none());
    }
    assert!(failure_reasons(&h).iter().any(|r| r == "subject-claim"));
}

#[test]
fn a_mutable_subject_claim_refuses_the_boot() {
    let vars = |pairs: Vec<(&'static str, &'static str)>| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    };
    let secret = |_: &str| -> Result<String, String> { Ok("s".into()) };
    let returns = ordo_host::federation::parse_return_urls(RETURN, false).expect("a list");
    let base = vec![
        ("ORDO_AUTH_DISCORD_PRESET", "discord"),
        ("ORDO_AUTH_DISCORD_CLIENT_ID", "1"),
        (
            "ORDO_AUTH_DISCORD_CLIENT_SECRET_FILE",
            "/run/secrets/discord",
        ),
        ("ORDO_AUTH_DISCORD_SUBJECT_CLAIM", "username"),
    ];
    let refused = ordo_host::federation::connection_from_vars(
        "discord",
        &vars(base.clone()),
        &secret,
        "https://api.example.com",
        &returns,
    )
    .err()
    .expect("a mutable subject refuses the boot");
    assert!(
        refused.contains("username") && refused.contains("MUTABLE_SUBJECT"),
        "{refused}"
    );

    let mut acknowledged = base;
    acknowledged.push(("ORDO_AUTH_DISCORD_MUTABLE_SUBJECT", "acknowledged"));
    assert!(ordo_host::federation::connection_from_vars(
        "discord",
        &vars(acknowledged),
        &secret,
        "https://api.example.com",
        &returns,
    )
    .is_ok());
}

/// §2.7 D1, D2, D7. A deleted account keeps its links, so its Discord id
/// names it and nobody else: signing in again is the one refusal, and it
/// does not mint a fresh account beside the deleted one.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_account_cannot_sign_in_through_discord() {
    let h = harness().await;
    let (_, done) = sign_in(&h.app, &h.fake, "discord", "c1", nelly()).await;
    let person = principal_of(done["token"].as_str().unwrap()).principal;
    h.auth
        .directory
        .lock()
        .unwrap()
        .mark_deleted(&person, Timestamp(NOW))
        .expect("marked deleted");
    let _ = failure_reasons(&h);

    let (status, again) = sign_in(&h.app, &h.fake, "discord", "c2", nelly()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{again}");
    assert_eq!(again["error"]["code"], "authentication_failed");
    assert!(again.get("token").is_none());
    assert_eq!(failure_reasons(&h), vec!["identity-deleted".to_string()]);
    assert_eq!(
        h.store
            .principal_for(&AuthenticatorId("oauth2:discord:80351110224678912".into()))
            .unwrap(),
        Some(person),
        "the id still names the deleted account, and nobody new was minted"
    );
}

/// §2.7 D3: the handoff resolves the person when it is redeemed, so a
/// deletion inside the sixty seconds between the callback and the page's
/// redemption wins.
#[tokio::test(flavor = "multi_thread")]
async fn a_deletion_between_callback_and_handoff_mints_no_token() {
    let h = harness().await;
    let (_, done) = sign_in(&h.app, &h.fake, "discord", "c1", nelly()).await;
    let person = principal_of(done["token"].as_str().unwrap()).principal;

    let (state, binder) = begin(&h.app, "discord").await;
    h.fake.authorize("c2", nelly());
    let location = callback(&h.app, "discord", &state, "c2").await;
    let handoff = param(&location, "handoff").expect("a handoff");
    h.auth
        .directory
        .lock()
        .unwrap()
        .mark_deleted(&person, Timestamp(NOW))
        .expect("marked deleted");
    let (status, answer) = redeem(&h.app, "discord", &handoff, &binder, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
    assert!(answer.get("token").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handoff_is_redeemed_once_and_only_with_its_binder() {
    let h = harness().await;
    let (state, binder) = begin(&h.app, "discord").await;
    h.fake.authorize("c1", nelly());
    let location = callback(&h.app, "discord", &state, "c1").await;
    let handoff = param(&location, "handoff").expect("a handoff");

    // Another browser, holding no binder for it: refused, and not burned.
    let (status, _) = redeem(&h.app, "discord", &handoff, "not-the-binder", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, ok) = redeem(&h.app, "discord", &handoff, &binder, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{ok}");
    let (status, _) = redeem(&h.app, "discord", &handoff, &binder, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "one-shot");

    // The callback is good once too.
    let (status, _, _) = send(
        &h.app,
        "GET",
        &format!(
            "/auth/callback/discord?state={}&code=c1",
            ordo_identity::federation::percent(&state)
        ),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a spent state has nowhere to go"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unlisted_return_address_is_refused_before_any_redirect_exists() {
    let h = harness().await;
    // Another path on the listed origin is accepted (owner ruling
    // 2026-10-09, `subdomain-signin` ask 2); another origin is not.
    let (status, _, body) = send(
        &h.app,
        "POST",
        "/auth/begin",
        Some(json!({"method": "discord", "return_to": "https://app.example.com/signed-in/elsewhere"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for return_to in [
        Value::Null,
        json!("https://app.example.com.evil.example/signed-in"),
        json!("https://evil.example/steal"),
    ] {
        let (status, _, body) = send(
            &h.app,
            "POST",
            "/auth/begin",
            Some(json!({"method": "discord", "return_to": return_to})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{return_to}: {body}");
        assert!(body.get("redirect").is_none());
    }
    // The provider's own error comes back as the one word, and no request
    // reaches the provider.
    let (state, _) = begin(&h.app, "discord").await;
    let (status, location, _) = send(
        &h.app,
        "GET",
        &format!(
            "/auth/callback/discord?state={}&error=access_denied",
            ordo_identity::federation::percent(&state)
        ),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        location.as_deref(),
        Some(format!("{RETURN}?error=authentication_failed").as_str())
    );
    assert!(h.fake.seen.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_reads_their_own_linked_identities_and_nobody_elses() {
    let h = harness().await;
    let (_, nelly_in) = sign_in(&h.app, &h.fake, "discord", "c1", nelly()).await;
    let mut other = nelly();
    other["id"] = json!("99");
    other["global_name"] = json!("Bea");
    other["email"] = json!("bea@example.com");
    other["verified"] = json!(false);
    let (_, bea_in) = sign_in(&h.app, &h.fake, "discord", "c2", other).await;

    let (status, _, list) = send(
        &h.app,
        "GET",
        "/auth/identities",
        None,
        Some(nelly_in["token"].as_str().unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        list,
        json!([{
            "connection": "discord",
            "subject": "80351110224678912",
            "display": "Nelly",
            "avatar": "https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.png",
            "email": "nelly@example.com",
            "linkedAt": NOW,
        }])
    );

    // Bea sees Bea, and her unverified address is not kept.
    let (_, _, list) = send(
        &h.app,
        "GET",
        "/auth/identities",
        None,
        Some(bea_in["token"].as_str().unwrap()),
    )
    .await;
    assert_eq!(list.as_array().map(Vec::len), Some(1), "{list}");
    assert_eq!(list[0]["subject"], "99");
    assert_eq!(list[0]["email"], Value::Null);

    let (status, _, _) = send(&h.app, "GET", "/auth/identities", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// O16: the client secret is in the token request's body and nowhere else.
#[tokio::test(flavor = "multi_thread")]
async fn the_client_secret_appears_in_no_answer_and_no_event() {
    let h = harness().await;
    let (_, ok) = sign_in(&h.app, &h.fake, "discord", "c1", nelly()).await;
    // A failing exchange too: an unknown code.
    let (state, _) = begin(&h.app, "discord").await;
    let location = callback(&h.app, "discord", &state, "never-issued").await;
    assert_eq!(
        param(&location, "error").as_deref(),
        Some("authentication_failed"),
        "a code the provider did not issue is a refused sign-in: {location}"
    );
    let recorded = format!("{:?}", events(&h));
    for text in [ok.to_string(), location, recorded] {
        assert!(!text.contains(CLIENT_SECRET), "{text}");
        assert!(
            !text.contains("at-c1"),
            "the access token is never shown: {text}"
        );
    }
}

/// A provider that answers its token exchange with a 5xx or a 429 is down
/// or rate limiting. The person is told "try again later"
/// (`identity_provider_unavailable`), never that their sign-in failed.
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_outage_or_rate_limit_is_try_again_not_sign_in_failed() {
    let h = harness().await;
    for code in ["provider-503", "provider-429"] {
        let (state, _) = begin(&h.app, "discord").await;
        let location = callback(&h.app, "discord", &state, code).await;
        assert_eq!(
            param(&location, "error").as_deref(),
            Some("identity_provider_unavailable"),
            "{code}: {location}"
        );
    }
    // A code the provider refuses is still a refused sign-in.
    let (state, _) = begin(&h.app, "discord").await;
    let location = callback(&h.app, "discord", &state, "never-issued").await;
    assert_eq!(
        param(&location, "error").as_deref(),
        Some("authentication_failed"),
        "{location}"
    );
}

/// A provider the deployment cannot reach (here: its endpoint resolves to
/// loopback under the deployment's real deny list) is told apart from a
/// refused sign-in, because it says nothing about the person.
#[tokio::test(flavor = "multi_thread")]
async fn a_provider_inside_the_perimeter_is_unavailable_and_never_dialled() {
    let fake = FakeDiscord::start().await;
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let discord = fake.provider_with(
        "discord",
        ceremonies.clone(),
        ordo_egress::DenyList::categories_only(),
    );
    let mut registry = ProviderRegistry::new();
    registry
        .register("discord", Box::new(discord.clone()))
        .expect("registered");
    let mut state = AuthState::new(
        registry,
        Directory::new(Provisioning::AutoOnboard),
        TokenKey::from_secret(SECRET),
    )
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_oauth2(discord);
    state.now = Box::new(|| NOW);
    let app = ordo_host::auth::router(Some(Arc::new(state)));
    let (state, _) = begin(&app, "discord").await;
    fake.authorize("c1", nelly());
    let location = callback(&app, "discord", &state, "c1").await;
    assert_eq!(
        param(&location, "error").as_deref(),
        Some("identity_provider_unavailable"),
        "{location}"
    );
    assert!(fake.seen.lock().unwrap().is_empty(), "no socket was opened");
}
