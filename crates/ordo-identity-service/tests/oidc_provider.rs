//! The identity service as an OIDC provider for app users (owner,
//! 2026-11-17, ask 3). A registered deployment is a public client: its
//! handle is the `client_id`, its return URLs are its redirect URIs, PKCE
//! `S256` is required.
//!
//! What it proves:
//!
//! - discovery says what the service does, with the issuer deployment
//!   tokens carry;
//! - a code with PKCE exchanges for an ID token whose claims Ordo's own
//!   OIDC client accepts, and whose `sub` is the same for every deployment;
//! - each refusal: wrong verifier, reused code, expired code, wrong
//!   redirect, unknown client, a client secret, a non-member, an ended
//!   member, no PKCE; an unknown client or redirect is shown here and never
//!   followed;
//! - an ID token is never a deployment token.
//!
//! A regression looks like a code that works twice, a redirect to an
//! address the deployment does not list, a token for someone who is not a
//! member, or an ID token the admin routes accept.
//!
//! Needs the conformance cluster.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_identity::deployment_token::{self, Expect};
use ordo_identity::federation::{form_encode, percent_decode};
use ordo_identity::jose::{self, Alg};
use ordo_identity::oidc::{check_id_token, IdTokenRules};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::Timestamp;
use ordo_identity_service::audit;
use ordo_identity_service::deployments::{Registration, SignUp};
use ordo_identity_service::people::Actor;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use support::*;
use tower::ServiceExt;

const NOTES: &str = "http://localhost:9201";
const VIEWER: &str = "http://localhost:9202";
const HAL: &str = "http://localhost:9203";
const VERIFIER: &str = "a-verifier-of-at-least-forty-three-characters-0123456789";
const NONCE: &str = "nonce-1";

const HOST: Actor<'static> = Actor {
    id: "unix:test",
    client: "host",
};

fn callback(origin: &str) -> String {
    format!("{origin}/auth/callback/ids")
}

fn challenge_of(verifier: &str) -> String {
    b64url_encode(&Sha256::digest(verifier.as_bytes()))
}

/// `notes` and `note-viewer` open, `hal` by invite.
async fn three(name: &str) -> H {
    let h = boot_with(name, &[]).await;
    for (handle, origin, sign_up) in [
        ("notes", NOTES, SignUp::Open),
        ("note-viewer", VIEWER, SignUp::Open),
        ("hal", HAL, SignUp::Invite),
    ] {
        h.svc
            .register_deployment(
                HOST,
                &Registration {
                    handle: handle.into(),
                    client_origins: vec![origin.into()],
                    return_urls: vec![callback(origin)],
                    sign_up,
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
            "198.51.100.7",
            "/sign-up/begin",
            json!({ "deployment": deployment }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let mut body = registration(auth, &challenge(&opts));
    body["deployment"] = json!(deployment);
    body["displayName"] = json!(name);
    let (s, done, headers) = h.post_from("198.51.100.7", "/sign-up/finish", body).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    (
        cookie_of(&headers),
        done["person"].as_str().unwrap().to_string(),
    )
}

/// The authorization request an Ordo app sends, for `client` at `origin`.
fn ask(client: &str, origin: &str) -> Vec<(String, String)> {
    [
        ("response_type", "code"),
        ("client_id", client),
        ("redirect_uri", &callback(origin)),
        ("scope", "openid profile"),
        ("state", "state-1"),
        ("nonce", NONCE),
        ("code_challenge", &challenge_of(VERIFIER)),
        ("code_challenge_method", "S256"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn encode(pairs: &[(String, String)]) -> String {
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    form_encode(&refs)
}

fn set(pairs: &mut Vec<(String, String)>, k: &str, v: Option<&str>) {
    pairs.retain(|(n, _)| n != k);
    if let Some(v) = v {
        pairs.push((k.into(), v.into()));
    }
}

/// `GET /oidc/authorize` as a browser: status, `Location`, body text.
async fn authorize(
    h: &H,
    pairs: &[(String, String)],
    cookie: Option<&str>,
) -> (StatusCode, Option<String>, String) {
    let mut req = Request::builder()
        .method("GET")
        .uri(format!("/oidc/authorize?{}", encode(pairs)));
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let res = h
        .svc
        .router()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let location = res
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string());
    assert_eq!(res.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&bytes).to_string(),
    )
}

/// A query parameter of a URL, decoded.
fn param(url: &str, key: &str) -> Option<String> {
    url.split_once('?')?
        .1
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| percent_decode(v))
}

/// The code a signed-in member gets back.
async fn code_for(h: &H, pairs: &[(String, String)], cookie: &str) -> String {
    let (status, location, body) = authorize(h, pairs, Some(cookie)).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let location = location.expect("a location");
    let redirect = pairs
        .iter()
        .find(|(k, _)| k == "redirect_uri")
        .unwrap()
        .1
        .clone();
    assert!(location.starts_with(&format!("{redirect}?")), "{location}");
    assert_eq!(param(&location, "state").as_deref(), Some("state-1"));
    assert_eq!(param(&location, "iss").as_deref(), Some(ORIGIN));
    param(&location, "code").expect("a code")
}

/// `POST /oidc/token` with a form body.
async fn exchange(h: &H, pairs: &[(&str, &str)]) -> (StatusCode, Value) {
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

async fn redeem(
    h: &H,
    client: &str,
    origin: &str,
    code: &str,
    verifier: &str,
) -> (StatusCode, Value) {
    exchange(
        h,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &callback(origin)),
            ("client_id", client),
            ("code_verifier", verifier),
        ],
    )
    .await
}

/// The ID token's claims, verified as Ordo's OIDC client verifies them.
async fn claims_of(h: &H, id_token: &str, client: &str) -> serde_json::Map<String, Value> {
    let jwks = h.jwks().await;
    let verified = jose::verify_with_jwks(id_token, &jwks, &[Alg::EdDSA]).expect("verifies");
    let claims = verified.claims().expect("claims");
    check_id_token(
        &claims,
        &IdTokenRules {
            issuer: ORIGIN,
            client_id: client,
            nonce: NONCE,
            skew_secs: 0,
        },
        Timestamp(h.now()),
    )
    .expect("the client's checks pass");
    claims
}

async fn actions(h: &H) -> Vec<(String, String, String, String)> {
    audit::page(h.svc.store().pool(), h.svc.handle(), None, 200)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.action, r.actor, r.subject, r.outcome))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_says_what_the_service_does() {
    let h = three("oidc-discovery").await;
    let (status, v, headers) = h
        .call("GET", "/.well-known/openid-configuration", &[], Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "max-age=600");
    assert_eq!(v["issuer"], ORIGIN, "the issuer deployment tokens carry");
    assert_eq!(
        v["authorization_endpoint"],
        format!("{ORIGIN}/oidc/authorize")
    );
    assert_eq!(v["token_endpoint"], format!("{ORIGIN}/oidc/token"));
    assert_eq!(v["jwks_uri"], format!("{ORIGIN}/.well-known/jwks.json"));
    assert_eq!(v["response_types_supported"], json!(["code"]));
    assert_eq!(v["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(v["id_token_signing_alg_values_supported"], json!(["EdDSA"]));
    assert_eq!(v["token_endpoint_auth_methods_supported"], json!(["none"]));
    assert_eq!(v["authorization_response_iss_parameter_supported"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_signs_in_with_a_code_and_pkce_and_gets_one_subject_everywhere() {
    let h = three("oidc-full").await;
    let pairs = ask("notes", NOTES);

    // Not signed in: to the sign-in page, which links to sign-up for an
    // open deployment, and comes back to this very request.
    let (status, location, _) = authorize(&h, &pairs, None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let page = location.unwrap();
    assert!(page.starts_with("/sign-in?deployment=notes&"), "{page}");
    assert_eq!(param(&page, "signUp").as_deref(), Some("open"));
    let back = param(&page, "return").unwrap();
    assert_eq!(back, format!("/oidc/authorize?{}", encode(&pairs)));

    let auth = authenticator(41, b"oidc-dana");
    let (cookie, dana) = sign_up(&h, "notes", "Dana", &auth).await;
    let code = code_for(&h, &pairs, &cookie).await;
    let (status, answer) = redeem(&h, "notes", NOTES, &code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["token_type"], "Bearer");
    assert!(answer["access_token"].is_string());
    let id_token = answer["id_token"].as_str().unwrap().to_string();
    let (header, _) = segments(&id_token);
    let header: Value = serde_json::from_str(&header).unwrap();
    assert_eq!(
        (header["alg"].as_str(), header["typ"].as_str()),
        (Some("EdDSA"), Some("JWT"))
    );
    let claims = claims_of(&h, &id_token, "notes").await;
    assert_eq!(
        claims["sub"],
        dana.as_str(),
        "the person's id at the service"
    );
    assert_eq!(claims["aud"], "notes");
    assert_eq!(claims["nonce"], NONCE);
    assert_eq!(claims["iat"], h.now());
    assert_eq!(claims["auth_time"], h.now());
    assert!(claims["exp"].as_u64().unwrap() > h.now());
    assert_eq!(claims["name"], "Dana");

    // Each code issued and each exchange is in the audit log.
    let log = actions(&h).await;
    for action in ["oidc.code", "oidc.token"] {
        assert!(
            log.iter().any(|(a, actor, subject, outcome)| a == action
                && *actor == dana
                && subject == "notes"
                && outcome == "ok"),
            "{action}: {log:?}"
        );
    }

    // Signed in, not yet a member of another open app: the sign-up page
    // joins her. Once joined, the same `sub`.
    let viewer = ask("note-viewer", VIEWER);
    let (_, location, _) = authorize(&h, &viewer, Some(&cookie)).await;
    assert!(location
        .unwrap()
        .starts_with("/sign-up?deployment=note-viewer&"));
    let (s, v, _) = h
        .post(
            "/sign-up/join",
            Some(&cookie),
            json!({ "deployment": "note-viewer" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let code = code_for(&h, &viewer, &cookie).await;
    let (status, answer) = redeem(&h, "note-viewer", VIEWER, &code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let other = claims_of(&h, answer["id_token"].as_str().unwrap(), "note-viewer").await;
    assert_eq!(other["sub"], claims["sub"], "one person, one subject");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_id_token_is_never_a_deployment_token() {
    let h = three("oidc-not-admin").await;
    let auth = authenticator(42, b"oidc-eve");
    let (cookie, _) = sign_up(&h, "notes", "Eve", &auth).await;
    let code = code_for(&h, &ask("notes", NOTES), &cookie).await;
    let (_, answer) = redeem(&h, "notes", NOTES, &code, VERIFIER).await;
    let id_token = answer["id_token"].as_str().unwrap();
    // The door's verifier refuses it, under the service's audience and
    // under the deployment's.
    let jwks = h.jwks().await;
    for audience in [AUD, "ordo-deployment:notes"] {
        let expect = Expect {
            issuer: ORIGIN,
            audience,
            now: h.now(),
            leeway_secs: 0,
        };
        assert!(deployment_token::verify(id_token, &jwks, &expect).is_err());
    }
    // So do the service's own admin routes.
    let (status, v) = h
        .bearer("GET", "/deployments/notes/members", id_token, Value::Null)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{v}");
    let (status, v) = h.bearer("GET", "/people", id_token, Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_bad_exchange_is_refused_and_a_code_works_once() {
    let h = three("oidc-exchange").await;
    let auth = authenticator(43, b"oidc-fay");
    let (cookie, fay) = sign_up(&h, "notes", "Fay", &auth).await;
    let pairs = ask("notes", NOTES);
    let refused = |(s, v): (StatusCode, Value)| {
        assert_eq!(
            (s, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_grant")),
            "{v}"
        );
    };

    // A wrong verifier spends the code: the right one then fails too.
    let code = code_for(&h, &pairs, &cookie).await;
    refused(redeem(&h, "notes", NOTES, &code, &format!("{VERIFIER}x")).await);
    refused(redeem(&h, "notes", NOTES, &code, VERIFIER).await);

    // Reused.
    let code = code_for(&h, &pairs, &cookie).await;
    assert_eq!(
        redeem(&h, "notes", NOTES, &code, VERIFIER).await.0,
        StatusCode::OK
    );
    refused(redeem(&h, "notes", NOTES, &code, VERIFIER).await);

    // Expired.
    let code = code_for(&h, &pairs, &cookie).await;
    h.clock.advance(61);
    refused(redeem(&h, "notes", NOTES, &code, VERIFIER).await);

    // Another redirect URI than the one it was issued for.
    let code = code_for(&h, &pairs, &cookie).await;
    refused(
        exchange(
            &h,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &format!("{NOTES}/auth/callback/other")),
                ("client_id", "notes"),
                ("code_verifier", VERIFIER),
            ],
        )
        .await,
    );

    // Another registered client.
    let code = code_for(&h, &pairs, &cookie).await;
    refused(redeem(&h, "note-viewer", NOTES, &code, VERIFIER).await);

    // An unknown client, and a client secret: `invalid_client`.
    let code = code_for(&h, &pairs, &cookie).await;
    let (s, v) = redeem(&h, "nobody", NOTES, &code, VERIFIER).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_client"))
    );
    let (s, v) = exchange(
        &h,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &callback(NOTES)),
            ("client_id", "notes"),
            ("client_secret", "s"),
            ("code_verifier", VERIFIER),
        ],
    )
    .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_client"))
    );
    assert!(v["error_description"]
        .as_str()
        .unwrap()
        .contains("CLIENT_AUTH=none"));
    let (s, v) = exchange(&h, &[("grant_type", "password")]).await;
    assert_eq!(v["error"], "unsupported_grant_type", "{s} {v}");

    // An ended member: the code issued before the end gets no token, and
    // the refusal is in the audit log.
    let code = code_for(&h, &pairs, &cookie).await;
    h.svc.end_member(HOST, "notes", &fay).await.unwrap();
    refused(redeem(&h, "notes", NOTES, &code, VERIFIER).await);
    let log = actions(&h).await;
    assert!(log
        .iter()
        .any(|(a, _, _, outcome)| a == "oidc.token" && outcome == "refused"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_client_or_redirect_is_shown_here_and_never_followed() {
    let h = three("oidc-authorize").await;
    let auth = authenticator(44, b"oidc-gus");
    let (cookie, _) = sign_up(&h, "notes", "Gus", &auth).await;

    let mut pairs = ask("nobody", NOTES);
    let (s, location, body) = authorize(&h, &pairs, Some(&cookie)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(location.is_none());
    assert!(
        body.contains("<h1>This sign-in link does not work</h1>"),
        "{body}"
    );
    assert!(
        body.contains("`nobody` is not a deployment registered"),
        "{body}"
    );

    pairs = ask("notes", NOTES);
    for wrong in [
        "https://evil.example/auth/callback/ids",
        "http://localhost:9201/auth/callback/ids#x",
        "http://localhost:9202/auth/callback/ids",
    ] {
        set(&mut pairs, "redirect_uri", Some(wrong));
        let (s, location, body) = authorize(&h, &pairs, Some(&cookie)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{wrong}");
        assert!(location.is_none(), "{wrong}");
        assert!(body.contains("redirect URI"), "{body}");
    }

    // Once the client and redirect check out, a refusal goes back to the
    // app as `error`, with the state and the issuer.
    for (k, v, error) in [
        ("code_challenge_method", None, "invalid_request"),
        ("code_challenge", Some("short"), "invalid_request"),
        ("nonce", None, "invalid_request"),
        ("scope", Some("profile"), "invalid_scope"),
        ("response_type", Some("token"), "unsupported_response_type"),
    ] {
        let mut pairs = ask("notes", NOTES);
        set(&mut pairs, k, v);
        let (s, location, _) = authorize(&h, &pairs, Some(&cookie)).await;
        assert_eq!(s, StatusCode::SEE_OTHER);
        let location = location.unwrap();
        assert!(location.starts_with(&callback(NOTES)), "{location}");
        assert_eq!(param(&location, "error").as_deref(), Some(error), "{k}");
        assert_eq!(param(&location, "state").as_deref(), Some("state-1"));
        assert!(param(&location, "code").is_none());
    }

    // A person who is not a member of an invite-only deployment gets no
    // code, and is not offered sign-up.
    let (s, location, _) = authorize(&h, &ask("hal", HAL), Some(&cookie)).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let location = location.unwrap();
    assert_eq!(param(&location, "error").as_deref(), Some("access_denied"));
    assert!(param(&location, "code").is_none());
    let (_, location, _) = authorize(&h, &ask("hal", HAL), None).await;
    let page = location.unwrap();
    assert!(page.starts_with("/sign-in?deployment=hal&"), "{page}");
    assert!(param(&page, "signUp").is_none());
}
