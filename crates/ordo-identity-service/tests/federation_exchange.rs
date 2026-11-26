//! Tokens for one Ordo deployment to call another
//! (`plan-federation-grants.md` 5, step G2; `crate::exchange`).
//!
//! `notes` is the consumer: its consumer key is derived from a master key
//! and its public half registered. `viewer` is the holder. `hal` is
//! registered with no consumer key.
//!
//! What it proves, against a real service and database:
//!
//! - on behalf of a person (RFC 8693): `notes` trades the person's ID
//!   token and its client assertion for a token with `sub` the person,
//!   `aud` the holder, `act.sub` `notes`; it is audited;
//! - as itself (`client_credentials`): `sub` `notes`, `kind` `client`, a
//!   `typ` of its own;
//! - neither is ever a deployment token, an ID token, or a bearer the
//!   service's admin routes take;
//! - each refusal: no assertion, another key, no key registered, a spent
//!   `jti`, an expired or too long-lived assertion, another audience, an
//!   ended consumer; an audience not registered or the consumer itself; a
//!   subject token expired, foreign, not an ID token, issued to another
//!   client, or for a person no longer a member;
//! - lifetimes are the settings', and a token on behalf of a person never
//!   outlives the person's own;
//! - the consumer key is set at registration, by the route, by the
//!   deployment's own admin, never by another deployment's.
//!
//! A regression looks like a token for a consumer that did not sign, an
//! assertion that works twice, a token that names a person the consumer
//! never signed in, or a client token a holder could read as a person.
//!
//! Needs the conformance cluster.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use ordo_identity::deployment_token::{self, Expect};
use ordo_identity::federation::{form_encode, percent_decode};
use ordo_identity::jose::{self, Alg};
use ordo_identity::oidc::{check_id_token, IdTokenRules};
use ordo_identity::peer_token::{
    self, ASSERTION_TYPE, GRANT_CLIENT_CREDENTIALS, GRANT_TOKEN_EXCHANGE, TOKEN_TYPE_ID_TOKEN,
    TOKEN_TYPE_JWT,
};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::Timestamp;
use ordo_identity_service::audit;
use ordo_identity_service::config::{CLIENT_ASSERTION_SECS_ENV, PEER_TOKEN_SECS_ENV};
use ordo_identity_service::deployments::{Registration, SignUp};
use ordo_identity_service::people::Actor;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::*;
use tower::ServiceExt;

const NOTES: &str = "http://localhost:9301";
const VIEWER: &str = "http://localhost:9302";
const HAL: &str = "http://localhost:9303";
const VERIFIER: &str = "a-verifier-of-at-least-forty-three-characters-0123456789";
const MASTER: [u8; 32] = [9; 32];
const ENDPOINT: &str = "http://localhost:8082/oidc/token";

const HOST: Actor<'static> = Actor {
    id: "unix:test",
    client: "host",
};

fn callback(origin: &str) -> String {
    format!("{origin}/auth/callback/ids")
}

fn key_of(handle: &str) -> SigningKey {
    peer_token::consumer_key(&MASTER, handle)
}

/// `notes` with its consumer key, `viewer` and `hal` without; all open.
async fn three(name: &str, extra: &[(&str, &str)]) -> H {
    let h = boot_with(name, extra).await;
    for (handle, origin, key) in [
        ("notes", NOTES, true),
        ("viewer", VIEWER, false),
        ("hal", HAL, false),
    ] {
        h.svc
            .register_deployment(
                HOST,
                &Registration {
                    handle: handle.into(),
                    client_origins: vec![origin.into()],
                    return_urls: vec![callback(origin)],
                    sign_up: SignUp::Open,
                    consumer_key: key
                        .then(|| peer_token::public_text(&key_of(handle).verifying_key())),
                },
            )
            .await
            .unwrap();
    }
    h
}

/// Open sign-up into `deployment`; the session cookie and the person.
async fn sign_up(
    h: &H,
    deployment: &str,
    name: &str,
    auth: &TestAuthenticator,
) -> (String, String) {
    let (s, opts, _) = h
        .post_from(
            "198.51.100.9",
            "/sign-up/begin",
            json!({ "deployment": deployment }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let mut body = registration(auth, &challenge(&opts));
    body["deployment"] = json!(deployment);
    body["displayName"] = json!(name);
    let (s, done, headers) = h.post_from("198.51.100.9", "/sign-up/finish", body).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    (
        cookie_of(&headers),
        done["person"].as_str().unwrap().to_string(),
    )
}

fn param(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?
        .1
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| percent_decode(v))
}

/// The ID token `client` gets for the signed-in browser `cookie`, through
/// the real authorization code flow.
async fn id_token(h: &H, client: &str, origin: &str, cookie: &str) -> String {
    let challenge = b64url_encode(&Sha256::digest(VERIFIER.as_bytes()));
    let query = form_encode(&[
        ("response_type", "code"),
        ("client_id", client),
        ("redirect_uri", &callback(origin)),
        ("scope", "openid"),
        ("nonce", "n-1"),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ]);
    let req = Request::builder()
        .method("GET")
        .uri(format!("/oidc/authorize?{query}"))
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let res = h.svc.router().oneshot(req).await.unwrap();
    let location = res.headers()["location"].to_str().unwrap().to_string();
    let code = param(&location, "code").expect("a code");
    let (s, v) = token(
        h,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &callback(origin)),
            ("client_id", client),
            ("code_verifier", VERIFIER),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["id_token"].as_str().unwrap().to_string()
}

/// `POST /oidc/token` with a form body.
async fn token(h: &H, pairs: &[(&str, &str)]) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/oidc/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form_encode(pairs)))
        .unwrap();
    let res = h.svc.router().oneshot(req).await.unwrap();
    let status = res.status();
    assert_eq!(res.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A fresh assertion for `client`, signed with `key`, living 60 seconds.
fn assertion(h: &H, client: &str, key: &SigningKey, jti: &str) -> String {
    peer_token::client_assertion(client, ENDPOINT, key, h.now(), h.now() + 60, jti)
}

async fn exchange(h: &H, assertion: &str, subject: &str, audience: &str) -> (StatusCode, Value) {
    token(
        h,
        &[
            ("grant_type", GRANT_TOKEN_EXCHANGE),
            ("client_assertion_type", ASSERTION_TYPE),
            ("client_assertion", assertion),
            ("subject_token", subject),
            ("subject_token_type", TOKEN_TYPE_ID_TOKEN),
            ("audience", audience),
        ],
    )
    .await
}

async fn as_itself(h: &H, assertion: &str, audience: &str) -> (StatusCode, Value) {
    token(
        h,
        &[
            ("grant_type", GRANT_CLIENT_CREDENTIALS),
            ("client_assertion_type", ASSERTION_TYPE),
            ("client_assertion", assertion),
            ("audience", audience),
        ],
    )
    .await
}

fn header_of(token: &str) -> Value {
    serde_json::from_str(&segments(token).0).unwrap()
}

async fn verified(h: &H, token: &str) -> serde_json::Map<String, Value> {
    let jwks = h.jwks().await;
    jose::verify_with_jwks(token, &jwks, &[Alg::EdDSA])
        .expect("signed by the service")
        .claims()
        .unwrap()
}

async fn log(h: &H) -> Vec<(String, String, String, String, String)> {
    audit::page(h.svc.store().pool(), h.svc.handle(), None, 500)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.action, r.actor, r.subject, r.outcome, r.detail))
        .collect()
}

fn refused(got: (StatusCode, Value), status: StatusCode, error: &str) {
    assert_eq!(
        (got.0, got.1["error"].as_str()),
        (status, Some(error)),
        "{}",
        got.1
    );
    if error != "bad_request" {
        assert!(got.1["error_description"].is_string(), "{}", got.1);
    }
}

/// A person signed up to `notes`, and the ID token `notes` holds for them.
async fn dana(h: &H) -> (String, String, String) {
    let auth = authenticator(61, b"fed-dana");
    let (cookie, dana) = sign_up(h, "notes", "Dana", &auth).await;
    let t = id_token(h, "notes", NOTES, &cookie).await;
    (cookie, dana, t)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_gets_a_token_for_the_holder_on_behalf_of_a_person() {
    let h = three("fed-on-behalf", &[]).await;
    let (_, dana, subject) = dana(&h).await;
    let (s, v) = exchange(
        &h,
        &assertion(&h, "notes", &key_of("notes"), "j-1"),
        &subject,
        "viewer",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        v["issued_token_type"],
        "urn:ietf:params:oauth:token-type:access_token"
    );
    assert_eq!(v["token_type"], "Bearer");
    assert_eq!(
        v["expires_in"], 300,
        "the default of ORDO_IDENTITY_PEER_TOKEN_SECS"
    );
    let t = v["access_token"].as_str().unwrap();
    assert_eq!(header_of(t)["typ"], "ordo-peer+jwt");
    let c = verified(&h, t).await;
    assert_eq!(c["iss"], ORIGIN);
    assert_eq!(c["sub"], dana.as_str(), "the person");
    assert_eq!(c["aud"], "ordo-peer:viewer", "the holder");
    assert_eq!(
        c["act"],
        json!({ "sub": "notes" }),
        "acting through the consumer"
    );
    assert_eq!(c["client_id"], "notes");
    assert_eq!(c["kind"], "person");
    assert_eq!(c["exp"], h.now() + 300);
    assert!(c["jti"].is_string());

    // Audited, with the person as actor and the holder named.
    assert!(log(&h)
        .await
        .iter()
        .any(|(a, actor, subject, outcome, detail)| a == "oidc.exchange"
            && *actor == dana
            && subject == "notes"
            && outcome == "ok"
            && detail == "aud=viewer"));

    // It is no deployment token, no ID token, and no bearer for the
    // service's routes.
    let jwks = h.jwks().await;
    for audience in ["ordo-deployment:viewer", "ordo-peer:viewer", AUD] {
        let expect = Expect {
            issuer: ORIGIN,
            audience,
            now: h.now(),
            leeway_secs: 0,
        };
        assert!(deployment_token::verify(t, &jwks, &expect).is_err());
    }
    for client in ["viewer", "notes"] {
        let rules = IdTokenRules {
            issuer: ORIGIN,
            client_id: client,
            nonce: "n-1",
            skew_secs: 0,
        };
        assert!(check_id_token(&c, &rules, Timestamp(h.now())).is_err());
    }
    let (s, _) = h
        .bearer("GET", "/deployments/viewer/members", t, Value::Null)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // The exchanged token is not a subject token in turn.
    let (s, v) = exchange(
        &h,
        &assertion(&h, "notes", &key_of("notes"), "j-2"),
        t,
        "viewer",
    )
    .await;
    refused((s, v), StatusCode::BAD_REQUEST, "invalid_grant");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_gets_a_token_as_itself_that_never_reads_as_a_person() {
    let h = three("fed-as-itself", &[]).await;
    let (s, v) = as_itself(
        &h,
        &assertion(&h, "notes", &key_of("notes"), "c-1"),
        "viewer",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let t = v["access_token"].as_str().unwrap();
    assert_eq!(header_of(t)["typ"], "ordo-client+jwt");
    let c = verified(&h, t).await;
    assert_eq!(c["sub"], "notes");
    assert_eq!(c["aud"], "ordo-peer:viewer");
    assert_eq!(c["kind"], "client");
    assert_eq!(c["client_id"], "notes");
    assert!(c.get("act").is_none(), "no person behind it");
    assert!(log(&h)
        .await
        .iter()
        .any(|(a, actor, _, outcome, detail)| a == "oidc.client-token"
            && actor == "notes"
            && outcome == "ok"
            && detail == "aud=viewer"));
    // Never a subject token: it is not an ID token.
    let (s, v) = exchange(
        &h,
        &assertion(&h, "notes", &key_of("notes"), "c-2"),
        t,
        "viewer",
    )
    .await;
    refused((s, v), StatusCode::BAD_REQUEST, "invalid_grant");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_that_does_not_prove_itself_is_refused_and_writes_nothing() {
    let h = three("fed-client-refused", &[]).await;
    let (_, _, subject) = dana(&h).await;
    let before = log(&h).await.len();
    let notes = key_of("notes");
    let unauth = StatusCode::UNAUTHORIZED;

    // No assertion at all.
    let got = token(
        &h,
        &[
            ("grant_type", GRANT_CLIENT_CREDENTIALS),
            ("client_id", "notes"),
            ("audience", "viewer"),
        ],
    )
    .await;
    refused(got, unauth, "invalid_client");
    // The wrong assertion type.
    let got = token(
        &h,
        &[
            ("grant_type", GRANT_CLIENT_CREDENTIALS),
            ("client_assertion_type", "urn:other"),
            ("client_assertion", &assertion(&h, "notes", &notes, "a-0")),
            ("audience", "viewer"),
        ],
    )
    .await;
    refused(got, unauth, "invalid_client");
    // Signed with another key: another master key's.
    let other = peer_token::consumer_key(&[1; 32], "notes");
    refused(
        as_itself(&h, &assertion(&h, "notes", &other, "a-1"), "viewer").await,
        unauth,
        "invalid_client",
    );
    // A deployment with no consumer key registered.
    refused(
        as_itself(&h, &assertion(&h, "hal", &key_of("hal"), "a-2"), "viewer").await,
        unauth,
        "invalid_client",
    );
    // A client nobody registered.
    refused(
        as_itself(&h, &assertion(&h, "nobody", &notes, "a-3"), "viewer").await,
        unauth,
        "invalid_client",
    );
    // Expired, and living longer than ORDO_IDENTITY_CLIENT_ASSERTION_SECS.
    let old =
        peer_token::client_assertion("notes", ENDPOINT, &notes, h.now() - 120, h.now() - 1, "a-4");
    refused(
        as_itself(&h, &old, "viewer").await,
        unauth,
        "invalid_client",
    );
    let long =
        peer_token::client_assertion("notes", ENDPOINT, &notes, h.now(), h.now() + 121, "a-5");
    refused(
        as_itself(&h, &long, "viewer").await,
        unauth,
        "invalid_client",
    );
    // For another token endpoint.
    let elsewhere = peer_token::client_assertion(
        "notes",
        "https://other.example.com/oidc/token",
        &notes,
        h.now(),
        h.now() + 60,
        "a-6",
    );
    refused(
        as_itself(&h, &elsewhere, "viewer").await,
        unauth,
        "invalid_client",
    );
    // `client_id` naming another deployment than the assertion.
    let got = token(
        &h,
        &[
            ("grant_type", GRANT_CLIENT_CREDENTIALS),
            ("client_id", "hal"),
            ("client_assertion_type", ASSERTION_TYPE),
            ("client_assertion", &assertion(&h, "notes", &notes, "a-7")),
            ("audience", "viewer"),
        ],
    )
    .await;
    refused(got, unauth, "invalid_client");
    // Nothing above wrote an audit row.
    assert_eq!(log(&h).await.len(), before);

    // A `jti` works once, on either grant.
    let once = assertion(&h, "notes", &notes, "a-8");
    assert_eq!(as_itself(&h, &once, "viewer").await.0, StatusCode::OK);
    refused(
        as_itself(&h, &once, "viewer").await,
        unauth,
        "invalid_client",
    );
    refused(
        exchange(&h, &once, &subject, "viewer").await,
        unauth,
        "invalid_client",
    );

    // An ended consumer proves nothing.
    h.svc.end_deployment(HOST, "notes").await.unwrap();
    refused(
        as_itself(&h, &assertion(&h, "notes", &notes, "a-9"), "viewer").await,
        unauth,
        "invalid_client",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_audience_that_is_not_a_registered_deployment_is_refused() {
    let h = three("fed-target", &[]).await;
    let (_, _, subject) = dana(&h).await;
    let notes = key_of("notes");
    let bad = StatusCode::BAD_REQUEST;
    for (jti, aud) in [("t-1", "nobody"), ("t-2", "notes"), ("t-3", "main")] {
        refused(
            exchange(&h, &assertion(&h, "notes", &notes, jti), &subject, aud).await,
            bad,
            "invalid_target",
        );
    }
    refused(
        as_itself(&h, &assertion(&h, "notes", &notes, "t-4"), "nobody").await,
        bad,
        "invalid_target",
    );
    // An ended holder is no longer an audience.
    h.svc.end_deployment(HOST, "viewer").await.unwrap();
    refused(
        as_itself(&h, &assertion(&h, "notes", &notes, "t-5"), "viewer").await,
        bad,
        "invalid_target",
    );
    let rows = log(&h).await;
    assert!(rows
        .iter()
        .any(|(a, _, _, o, d)| a == "oidc.exchange" && o == "refused" && d == "audience nobody"));
    assert!(rows.iter().any(|(a, _, _, o, d)| a == "oidc.client-token"
        && o == "refused"
        && d == "audience viewer"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subject_token_the_consumer_may_not_exchange_is_refused() {
    let h = three("fed-subject", &[]).await;
    let (cookie, dana, subject) = dana(&h).await;
    let notes = key_of("notes");
    let bad = StatusCode::BAD_REQUEST;

    // Foreign: the same claims signed by a key that is not the service's.
    let mut claims = serde_json::Map::new();
    for (k, v) in verified(&h, &subject).await {
        claims.insert(k, v);
    }
    let forged = peer_token::sign_jwt(
        &json!({ "alg": "EdDSA", "typ": "JWT" }),
        &Value::Object(claims),
        &SigningKey::from_bytes(&[3; 32]),
    );
    refused(
        exchange(
            &h,
            &assertion(&h, "notes", &notes, "s-1"),
            &forged,
            "viewer",
        )
        .await,
        bad,
        "invalid_grant",
    );

    // Issued to another client: `viewer`'s ID token, presented by `notes`.
    let (s, v, _) = h
        .post(
            "/sign-up/join",
            Some(&cookie),
            json!({ "deployment": "viewer" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let viewers = id_token(&h, "viewer", VIEWER, &cookie).await;
    refused(
        exchange(&h, &assertion(&h, "notes", &notes, "s-2"), &viewers, "hal").await,
        bad,
        "invalid_grant",
    );

    // Not an ID token: a deployment token for the person.
    let (_, s) = h.person("Ola").await;
    let (st, v, _) = h
        .call(
            "POST",
            "/token",
            &[
                ("origin", CONSOLE),
                ("cookie", &format!("ordo-identity={}", s.secret)),
            ],
            json!({ "aud": AUD }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let deployment = v["token"].as_str().unwrap();
    refused(
        exchange(
            &h,
            &assertion(&h, "notes", &notes, "s-3"),
            deployment,
            "viewer",
        )
        .await,
        bad,
        "invalid_grant",
    );

    // A wrong subject token type.
    let got = token(
        &h,
        &[
            ("grant_type", GRANT_TOKEN_EXCHANGE),
            ("client_assertion_type", ASSERTION_TYPE),
            ("client_assertion", &assertion(&h, "notes", &notes, "s-4")),
            ("subject_token", &subject),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:access_token",
            ),
            ("audience", "viewer"),
        ],
    )
    .await;
    refused(got, bad, "bad_request");

    // A person no longer a member of the consumer.
    h.svc.end_member(HOST, "notes", &dana).await.unwrap();
    refused(
        exchange(
            &h,
            &assertion(&h, "notes", &notes, "s-5"),
            &subject,
            "viewer",
        )
        .await,
        bad,
        "invalid_grant",
    );

    // Expired: the ID token lives ORDO_IDENTITY_TOKEN_SECS, 600.
    h.clock.advance(601);
    refused(
        exchange(
            &h,
            &assertion(&h, "notes", &notes, "s-6"),
            &subject,
            "viewer",
        )
        .await,
        bad,
        "invalid_grant",
    );

    let rows = log(&h).await;
    for why in [
        "not signed by this service",
        "issued to another client",
        "not an ID token",
        "not a member",
        "expired",
    ] {
        assert!(
            rows.iter().any(|(a, _, _, o, d)| a == "oidc.exchange"
                && o == "refused"
                && d.starts_with(why)),
            "{why}: {rows:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lifetimes_are_settings_and_a_person_s_token_bounds_its_exchange() {
    let h = three(
        "fed-lifetimes",
        &[
            (PEER_TOKEN_SECS_ENV, "3600"),
            (CLIENT_ASSERTION_SECS_ENV, "600"),
        ],
    )
    .await;
    let (_, _, subject) = dana(&h).await;
    let notes = key_of("notes");
    // An assertion living 10 minutes is taken under the raised setting.
    let long =
        peer_token::client_assertion("notes", ENDPOINT, &notes, h.now(), h.now() + 600, "l-1");
    let (s, v) = as_itself(&h, &long, "viewer").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["expires_in"], 3600);
    // On behalf of a person, never past the ID token's own `exp`.
    h.clock.advance(100);
    let (s, v) = exchange(
        &h,
        &assertion(&h, "notes", &notes, "l-2"),
        &subject,
        "viewer",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        v["expires_in"], 500,
        "600 the ID token lived, less the 100 gone"
    );
    let c = verified(&h, v["access_token"].as_str().unwrap()).await;
    assert_eq!(c["exp"], h.now() + 500);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_consumer_key_is_set_by_the_registry_or_the_deployment_itself() {
    let h = three("fed-key-route", &[]).await;
    let viewer = key_of("viewer");
    let public = peer_token::public_text(&viewer.verifying_key());
    let listed = |v: &Value, handle: &str| {
        v["deployments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["handle"] == handle)
            .unwrap()["consumerKey"]
            .clone()
    };

    // An admin of `viewer` registers its key with the token they hold.
    let (ann, s) = h.person("Ann").await;
    h.svc.add_member(HOST, "viewer", &ann).await.unwrap();
    h.svc.add_member(HOST, "hal", &ann).await.unwrap();
    let cookie = format!("ordo-identity={}", s.secret);
    let token_for = |origin: &'static str, aud: &'static str| {
        let cookie = cookie.clone();
        let h = &h;
        async move {
            let (st, v, _) = h
                .call(
                    "POST",
                    "/token",
                    &[("origin", origin), ("cookie", &cookie)],
                    json!({ "aud": aud }),
                )
                .await;
            assert_eq!(st, StatusCode::OK, "{v}");
            v["token"].as_str().unwrap().to_string()
        }
    };
    let viewer_admin = token_for(VIEWER, "ordo-deployment:viewer").await;
    let hal_admin = token_for(HAL, "ordo-deployment:hal").await;

    // Another deployment's admin cannot set it.
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments/viewer/consumer-key",
            &hal_admin,
            json!({ "consumerKey": public }),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{v}");
    // A key that is not one.
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments/viewer/consumer-key",
            &viewer_admin,
            json!({ "consumerKey": "abc" }),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("bad_request")),
        "{v}"
    );
    // Before: `viewer` cannot call anyone.
    refused(
        as_itself(&h, &assertion(&h, "viewer", &viewer, "k-1"), "notes").await,
        StatusCode::UNAUTHORIZED,
        "invalid_client",
    );
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments/viewer/consumer-key",
            &viewer_admin,
            json!({ "consumerKey": public }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["consumerKey"], public.as_str());
    let (st, _) = as_itself(&h, &assertion(&h, "viewer", &viewer, "k-2"), "notes").await;
    assert_eq!(st, StatusCode::OK);

    // The registry shows it; the host command removes it, and the next
    // assertion is refused.
    let home = token_for(CONSOLE, AUD).await;
    let (st, v) = h.bearer("GET", "/deployments", &home, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(listed(&v, "viewer"), json!(public));
    assert_eq!(listed(&v, "hal"), Value::Null);
    let (code, out) = ordo_identity_service::commands::run(
        &h.svc,
        &[
            "deployment".into(),
            "consumer-key".into(),
            "viewer".into(),
            "--none".into(),
        ],
        "test",
    )
    .await;
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("no consumer key"), "{out}");
    refused(
        as_itself(&h, &assertion(&h, "viewer", &viewer, "k-3"), "notes").await,
        StatusCode::UNAUTHORIZED,
        "invalid_client",
    );
    assert!(log(&h)
        .await
        .iter()
        .any(|(a, _, s, o, _)| a == "deployment.consumer-key" && s == "viewer" && o == "ok"));
}

/// The call chain (`plan-federation-grants.md` 7, step G3): the service
/// writes the consumer's incoming `ordo_chain` with the consumer added last
/// into the token, under its signature, so a holder can refuse a loop.
#[tokio::test(flavor = "multi_thread")]
async fn the_call_chain_is_carried_in_the_token_with_the_consumer_last() {
    let h = three("fed-chain", &[]).await;
    let t = |v: &Value| v["access_token"].as_str().unwrap().to_string();

    // A call that starts at the consumer: the consumer alone.
    let (s, v) = as_itself(
        &h,
        &assertion(&h, "notes", &key_of("notes"), "ch-1"),
        "viewer",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(verified(&h, &t(&v)).await["chain"], json!(["notes"]));

    // A call that came to the consumer through `hal`.
    let (s, v) = token(
        &h,
        &[
            ("grant_type", GRANT_CLIENT_CREDENTIALS),
            ("client_assertion_type", ASSERTION_TYPE),
            (
                "client_assertion",
                &assertion(&h, "notes", &key_of("notes"), "ch-2"),
            ),
            ("audience", "viewer"),
            (peer_token::CHAIN_PARAM, "hal"),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(verified(&h, &t(&v)).await["chain"], json!(["hal", "notes"]));

    // A chain that is not handles is refused, naming the field.
    let (s, v) = token(
        &h,
        &[
            ("grant_type", GRANT_CLIENT_CREDENTIALS),
            ("client_assertion_type", ASSERTION_TYPE),
            (
                "client_assertion",
                &assertion(&h, "notes", &key_of("notes"), "ch-3"),
            ),
            ("audience", "viewer"),
            (peer_token::CHAIN_PARAM, "Not A Handle!"),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v.to_string().contains("ordo_chain"), "{v}");
}

async fn session_exchange(
    h: &H,
    assertion: &str,
    statement: &str,
    audience: &str,
) -> (StatusCode, Value) {
    token(
        h,
        &[
            ("grant_type", GRANT_TOKEN_EXCHANGE),
            ("client_assertion_type", ASSERTION_TYPE),
            ("client_assertion", assertion),
            ("subject_token", statement),
            ("subject_token_type", TOKEN_TYPE_JWT),
            ("audience", audience),
        ],
    )
    .await
}

/// Step G5, board `federation-build-5-choices-from-steps` ask 2: past the
/// person's 10-minute ID token, the consumer exchanges its own session
/// statement about them, signed with its consumer key.
#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_keeps_acting_for_a_person_with_its_own_session_statement() {
    let h = three("fed-session", &[]).await;
    let (_, dana, id_token) = dana(&h).await;

    // 20 minutes on, the ID token is spent.
    h.clock.advance(20 * 60);
    refused(
        exchange(
            &h,
            &assertion(&h, "notes", &key_of("notes"), "s-0"),
            &id_token,
            "viewer",
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_grant",
    );

    // The session statement still buys a token for her.
    let now = h.now();
    let statement = |iss: &str, sub: &str, key: &SigningKey, exp: u64| {
        peer_token::session_subject(iss, ORIGIN, sub, key, now, exp)
    };
    let (s, v) = session_exchange(
        &h,
        &assertion(&h, "notes", &key_of("notes"), "s-1"),
        &statement("notes", &dana, &key_of("notes"), now + 60),
        "viewer",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let t = v["access_token"].as_str().unwrap();
    assert_eq!(header_of(t)["typ"], "ordo-peer+jwt");
    let c = verified(&h, t).await;
    assert_eq!(c["sub"], dana.as_str());
    assert_eq!(c["act"], json!({ "sub": "notes" }));
    assert_eq!(c["kind"], "person");
    assert_eq!(c["exp"], now + 300, "its own span, not the statement's");

    // Each one that is not the consumer's own, live, for its own member is
    // refused.
    let cases = [
        (
            "signed with another key",
            statement("notes", &dana, &key_of("viewer"), now + 60),
        ),
        (
            "issued by another deployment",
            statement("viewer", &dana, &key_of("notes"), now + 60),
        ),
        ("expired", statement("notes", &dana, &key_of("notes"), now)),
        (
            "living too long",
            statement("notes", &dana, &key_of("notes"), now + 3600),
        ),
        (
            "not a member",
            statement("notes", "p_nobody", &key_of("notes"), now + 60),
        ),
        ("an ID token is not a statement", id_token.clone()),
    ];
    for (i, (why, st)) in cases.iter().enumerate() {
        let got = session_exchange(
            &h,
            &assertion(&h, "notes", &key_of("notes"), &format!("s-x{i}")),
            st,
            "viewer",
        )
        .await;
        assert_eq!(got.0, StatusCode::BAD_REQUEST, "{why}: {}", got.1);
        assert_eq!(got.1["error"], "invalid_grant", "{why}: {}", got.1);
    }
}
