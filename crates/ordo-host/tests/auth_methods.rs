//! `GET /auth/methods` (owner, 2026-10-10, `signin-methods-route`): the ways
//! in a deployment serves, public and without a token, so a page builds its
//! sign-in screen from the door.
//!
//! What it proves:
//!
//! - the answer follows the settings: passkey only; passkey, email and
//!   guest; an outside connection; the Turnstile check with its site key;
//! - no secret, client id, secret file or endpoint is anywhere in the body;
//! - it answers with no token, and a short `Cache-Control`;
//! - a deployment with no sign-in answers the same shape, all off;
//! - the site key setting is read, trimmed and refused when malformed.
//!
//! The tenant half (a tenant's providers listed, an unknown tenant looking
//! like one with none) needs Postgres and lives in `auth_tenant_providers.rs`
//! beside the harness that adds the providers.

mod oauth2_support;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use oauth2_support::{FakeDiscord, CLIENT_ID, CLIENT_SECRET};
use ordo_egress::{DenyList, TlsRoots};
use ordo_host::auth::guest::{GuestMode, Guests};
use ordo_host::auth::human::{self, HumanCheck};
use ordo_host::auth::AuthState;
use ordo_host::email_code::{EmailCodePolicy, EmailCodeProvider, MemoryEmailCodeStore, Registered};
use ordo_host::mail::MemoryMailQueue;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::{MemoryCeremonyStore, MemoryDirectoryStore, ProviderRegistry, TokenKey};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const CA: &[u8] = include_bytes!("fixture/ca.der");
const SITE_KEY: &str = "0x4AAAAAAA-public_site_key";
const TURNSTILE_SECRET: &str = "turnstile-secret-do-not-print";
const SECRET: &[u8] = b"auth-methods-test-secret-32-byte";

struct Build {
    passkey: bool,
    email: bool,
    guest: bool,
    discord: bool,
    human: Option<Option<&'static str>>,
    max_age: Option<u64>,
}

fn none() -> Build {
    Build {
        passkey: false,
        email: false,
        guest: false,
        discord: false,
        human: None,
        max_age: None,
    }
}

async fn app(b: Build) -> axum::Router {
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let store = Arc::new(MemoryDirectoryStore::new());
    let mut registry = ProviderRegistry::new();
    let passkey = Arc::new(ordo_identity::PasskeyProvider::with_stores(
        ordo_identity::RelyingParty::new("example.com", &["https://app.example.com"])
            .expect("a relying party"),
        Arc::new(ordo_identity::MemoryPasskeyStore::new()),
        ceremonies.clone(),
    ));
    if b.passkey {
        registry
            .register("passkey", Box::new(passkey.clone()))
            .expect("registered");
    }
    let fake = FakeDiscord::start().await;
    let discord = fake.provider("discord", ceremonies.clone());
    if b.discord {
        registry
            .register("discord", Box::new(discord.clone()))
            .expect("registered");
    }
    let email = Arc::new(EmailCodeProvider::new(
        SECRET,
        EmailCodePolicy::default(),
        Arc::new(MemoryEmailCodeStore::new()),
        Arc::new(MemoryMailQueue::default()),
        store.clone(),
        Provisioning::AutoOnboard,
    ));
    if b.email {
        registry
            .register("email", Box::new(Registered(email.clone())))
            .expect("registered");
    }
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store),
        TokenKey::from_secret(SECRET),
    )
    .with_recorder(Arc::new(ordo_secevent::Recorder::default()))
    .with_ceremonies(ceremonies);
    if b.passkey {
        state = state.with_passkey(passkey);
    }
    if b.discord {
        state = state.with_oauth2(discord);
    }
    if b.email {
        state = state.with_email(email);
    }
    state = state.with_guests(Guests::fixed(if b.guest {
        GuestMode::On
    } else {
        GuestMode::Off
    }));
    if let Some(key) = b.human {
        let check = HumanCheck::new(
            TURNSTILE_SECRET.to_string(),
            "https://localhost:1/turnstile/v0/siteverify",
            TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
            DenyList::categories_only().permitting_loopback_for_tests(),
        )
        .expect("a valid check")
        .with_site_key(key.map(str::to_string));
        state = state.with_human_check(check);
    }
    if let Some(secs) = b.max_age {
        state.methods_max_age = secs;
    }
    ordo_host::auth::router(Some(Arc::new(state)))
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Option<String>, String, Value) {
    let res = app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let cache = res
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let text = String::from_utf8_lossy(&bytes).to_string();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, cache, text, json)
}

#[tokio::test]
async fn a_passkey_only_deployment_says_so() {
    let app = app(Build {
        passkey: true,
        ..none()
    })
    .await;
    let (status, cache, _, body) = get(&app, "/auth/methods").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"passkey": true, "password": false, "email": false, "guest": false,
               "humanCheck": null, "connections": []})
    );
    let cache = cache.expect("a cache header");
    assert!(
        cache.contains("max-age=") && !cache.contains("no-store"),
        "{cache}"
    );
}

#[tokio::test]
async fn passkey_email_and_guest_are_each_reported() {
    let app = app(Build {
        passkey: true,
        email: true,
        guest: true,
        ..none()
    })
    .await;
    let (_, _, _, body) = get(&app, "/auth/methods").await;
    assert_eq!(body["passkey"], true);
    assert_eq!(body["email"], true);
    assert_eq!(body["guest"], true);
    assert_eq!(body["password"], false);
    assert_eq!(body["connections"], json!([]));
}

#[tokio::test]
async fn a_discord_connection_is_listed_by_name_label_and_kind_only() {
    let app = app(Build {
        passkey: true,
        discord: true,
        ..none()
    })
    .await;
    let (_, _, text, body) = get(&app, "/auth/methods").await;
    assert_eq!(
        body["connections"],
        json!([{"name": "discord", "label": "Discord", "kind": "oauth2"}])
    );
    // Nothing about the connection beyond what its button shows.
    for secret in [
        CLIENT_SECRET,
        CLIENT_ID,
        "oauth2/authorize",
        "localhost",
        "/api/",
    ] {
        assert!(!text.contains(secret), "`{secret}` is in {text}");
    }
}

#[tokio::test]
async fn turnstile_is_reported_with_its_public_site_key_and_never_its_secret() {
    let with_key = app(Build {
        passkey: true,
        human: Some(Some(SITE_KEY)),
        ..none()
    })
    .await;
    let (_, _, text, body) = get(&with_key, "/auth/methods").await;
    assert_eq!(
        body["humanCheck"],
        json!({"kind": "turnstile", "siteKey": SITE_KEY})
    );
    assert!(!text.contains(TURNSTILE_SECRET), "{text}");
    assert!(!text.contains("siteverify"), "{text}");

    let without = app(Build {
        passkey: true,
        human: Some(None),
        ..none()
    })
    .await;
    let (_, _, _, body) = get(&without, "/auth/methods").await;
    assert_eq!(body["humanCheck"], json!({"kind": "turnstile"}));
}

#[tokio::test]
async fn it_answers_with_no_token_and_a_tenant_query_adds_nothing_without_tenant_providers() {
    let app = app(Build {
        passkey: true,
        discord: true,
        ..none()
    })
    .await;
    let (plain_status, _, _, plain) = get(&app, "/auth/methods").await;
    assert_eq!(plain_status, StatusCode::OK);
    for tenant in ["spc_real", "spc_nobody", "not%20a%20space", ""] {
        let (status, _, _, body) = get(&app, &format!("/auth/methods?tenant={tenant}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, plain, "tenant `{tenant}`");
    }
}

#[tokio::test]
async fn a_deployment_with_no_sign_in_answers_the_same_shape_all_off() {
    let app = ordo_host::auth::router(None);
    let (status, _, _, body) = get(&app, "/auth/methods").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"passkey": false, "password": false, "email": false, "guest": false,
               "humanCheck": null, "connections": []})
    );
}

fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |k: &str| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.to_string())
    }
}

fn read(_: &str) -> Result<String, String> {
    Ok(TURNSTILE_SECRET.to_string())
}

#[test]
fn the_site_key_setting_is_optional_public_and_checked() {
    let deny = DenyList::categories_only;

    static PLAIN: [(&str, &str); 2] = [
        ("ORDO_AUTH_HUMAN_CHECK", "turnstile"),
        ("ORDO_AUTH_HUMAN_CHECK_SECRET_FILE", "/run/secrets/ts"),
    ];
    let got = human::from_vars(&vars(&PLAIN), &read, deny())
        .map_err(|e| e.1)
        .expect("valid")
        .expect("on");
    assert_eq!(got.site_key(), None);

    static SET: [(&str, &str); 3] = [
        ("ORDO_AUTH_HUMAN_CHECK", "turnstile"),
        ("ORDO_AUTH_HUMAN_CHECK_SECRET_FILE", "/run/secrets/ts"),
        ("ORDO_AUTH_HUMAN_CHECK_SITE_KEY", "  0x4AAA-key_1  "),
    ];
    let got = human::from_vars(&vars(&SET), &read, deny())
        .map_err(|e| e.1)
        .expect("valid")
        .expect("on");
    assert_eq!(got.site_key(), Some("0x4AAA-key_1"));

    static BAD: [(&str, &str); 3] = [
        ("ORDO_AUTH_HUMAN_CHECK", "turnstile"),
        ("ORDO_AUTH_HUMAN_CHECK_SECRET_FILE", "/run/secrets/ts"),
        ("ORDO_AUTH_HUMAN_CHECK_SITE_KEY", "<script>"),
    ];
    let err = human::from_vars(&vars(&BAD), &read, deny())
        .map(|_| ())
        .expect_err("refused");
    assert_eq!(err.0, "ORDO_AUTH_HUMAN_CHECK_SITE_KEY");
}

/// The `max-age` is `ORDO_AUTH_METHODS_MAX_AGE`, 60 by default (audit
/// 2026-10-11, finding 13).
#[tokio::test]
async fn the_max_age_is_the_setting() {
    let (_, cache, _, _) = get(&app(none()).await, "/auth/methods").await;
    assert_eq!(cache.as_deref(), Some("public, max-age=60"));
    let app = app(Build {
        max_age: Some(0),
        ..none()
    })
    .await;
    let (_, cache, _, _) = get(&app, "/auth/methods?tenant=t_x").await;
    assert_eq!(cache.as_deref(), Some("public, max-age=0"));
}
