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
use ordo_identity_service::deployments::{self, Registration, SignUp};
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

/// `notes` and `note-viewer` open, `hal` by invite. The two apps register
/// their callback alone, as an app does since 2026-11-19. `hal` is
/// registered as before then: its page is also a client origin.
async fn three(name: &str) -> H {
    three_with(name, &[]).await
}

async fn three_with(name: &str, extra: &[(&str, &str)]) -> H {
    let h = boot_with(name, extra).await;
    for (handle, origin, sign_up, client) in [
        ("notes", NOTES, SignUp::Open, false),
        ("note-viewer", VIEWER, SignUp::Open, false),
        ("hal", HAL, SignUp::Invite, true),
    ] {
        h.svc
            .register_deployment(
                HOST,
                &Registration {
                    handle: handle.into(),
                    client_origins: if client {
                        vec![origin.into()]
                    } else {
                        Vec::new()
                    },
                    return_urls: vec![callback(origin)],
                    sign_up,
                    consumer_key: None,
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
    assert_eq!(
        v["token_endpoint_auth_methods_supported"],
        json!(["none", "private_key_jwt"])
    );
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

    // The exchange is in the audit log; the code alone is not.
    let log = actions(&h).await;
    assert!(
        log.iter()
            .any(|(a, actor, subject, outcome)| a == "oidc.token"
                && *actor == dana
                && subject == "notes"
                && outcome == "ok"),
        "{log:?}"
    );
    assert!(!log.iter().any(|(a, ..)| a == "oidc.code"), "{log:?}");

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

// ───────────── review of PRs 26 and 27, 2026-11-19 ─────────────

/// A call from `origin` carrying the session cookie: status, body, and
/// the CORS credentials grant it got, if any.
async fn from_page(
    h: &H,
    origin: &str,
    cookie: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value, Option<String>) {
    let (s, v, headers) = h
        .call(
            "POST",
            path,
            &[("origin", origin), ("cookie", cookie)],
            body,
        )
        .await;
    let grant = headers
        .get("access-control-allow-credentials")
        .map(|v| v.to_str().unwrap().to_string());
    (s, v, grant)
}

/// An app registered by its callback is no caller: its page, with the
/// session cookie, gets no CORS grant and no answer from any route that
/// spends the cookie. The console, a client origin, still does.
#[tokio::test(flavor = "multi_thread")]
async fn an_app_origin_gets_no_credentialed_answer_and_the_console_still_does() {
    let h = three("oidc-app-origin").await;
    let auth = authenticator(45, b"oidc-ida");
    let (cookie, ida) = sign_up(&h, "notes", "Ida", &auth).await;

    let (s, _, headers) = h
        .call(
            "OPTIONS",
            "/me",
            &[("origin", NOTES), ("access-control-request-method", "POST")],
            Value::Null,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "no preflight for an app's page");
    assert!(headers.get("access-control-allow-origin").is_none());
    for (path, body) in [
        ("/me", json!({})),
        ("/touch/begin", json!({})),
        ("/touch/finish", json!({ "challenge": "x" })),
        (
            "/cli/approve",
            json!({ "codeChallenge": challenge_of(VERIFIER) }),
        ),
        ("/sign-up/join", json!({ "deployment": "note-viewer" })),
    ] {
        let (s, v, grant) = from_page(&h, NOTES, &cookie, path, body).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{path}: {v}");
        assert_eq!(v["error"], "origin_refused", "{path}");
        assert!(grant.is_none(), "{path}");
    }
    let joined = deployments::role_of(h.svc.store().pool(), h.svc.handle(), "note-viewer", &ida)
        .await
        .unwrap();
    assert!(joined.is_none(), "the app's page joined nobody");

    // The console, a client origin of the service's own deployment.
    let (s, v, grant) = from_page(&h, CONSOLE, &cookie, "/me", json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(grant.as_deref(), Some("true"));
}

/// Defence in depth for a deployment registered before 2026-11-19, whose
/// app page is still a client origin: a reauth with no `aud` (the home's
/// token) and a CLI approval answer only the service's page or a client
/// origin of its own deployment, and joining answers the service's page
/// alone. `deployment set` then drops the page from the client origins.
#[tokio::test(flavor = "multi_thread")]
async fn a_registered_client_origin_cannot_reauth_for_the_home_or_approve_or_join() {
    let h = three("oidc-legacy-origin").await;
    let auth = authenticator(46, b"oidc-jo");
    let (cookie, jo) = sign_up(&h, "notes", "Jo", &auth).await;
    // Jo is also an admin of the service's own deployment.
    h.svc.add_member(HOST, h.svc.handle(), &jo).await.unwrap();

    // hal's page is a client origin, so the cookie is spent there and the
    // reauth ceremony may run there.
    let (s, opts, grant) = from_page(&h, HAL, &cookie, "/touch/begin", json!({})).await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    assert_eq!(grant.as_deref(), Some("true"));
    let mut at_hal = authenticator(46, b"oidc-jo");
    at_hal.origin = HAL.to_string();
    let (s, v, _) = from_page(
        &h,
        HAL,
        &cookie,
        "/touch/finish",
        assertion(&at_hal, &challenge(&opts)),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "no home token for hal's page: {v}"
    );
    assert_eq!(v["error"], "origin_refused");
    assert!(v.get("token").is_none());

    let (s, v, _) = from_page(
        &h,
        HAL,
        &cookie,
        "/cli/approve",
        json!({ "codeChallenge": challenge_of(VERIFIER) }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    for from in [HAL, CONSOLE] {
        let (s, v, _) = from_page(
            &h,
            from,
            &cookie,
            "/sign-up/join",
            json!({ "deployment": "note-viewer" }),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "joining is the service's page's: {v}"
        );
    }

    // The console, the home's client, still reauths with no `aud`, and
    // the service's own page still approves and joins.
    let (_, opts, _) = from_page(&h, CONSOLE, &cookie, "/touch/begin", json!({})).await;
    let mut at_console = authenticator(46, b"oidc-jo");
    at_console.origin = CONSOLE.to_string();
    let (s, v, _) = from_page(
        &h,
        CONSOLE,
        &cookie,
        "/touch/finish",
        assertion(&at_console, &challenge(&opts)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["token"].is_string());
    let (s, v, _) = h
        .post(
            "/cli/approve",
            Some(&cookie),
            json!({ "codeChallenge": challenge_of(VERIFIER) }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v, _) = h
        .post(
            "/sign-up/join",
            Some(&cookie),
            json!({ "deployment": "note-viewer" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // `deployment set` drops hal's page from the client origins, keeping
    // its callback; then its page gets no credentialed answer.
    let args: Vec<String> = ["deployment", "set", "hal", "--no-origins"]
        .iter()
        .map(|a| a.to_string())
        .collect();
    let (code, out) = ordo_identity_service::commands::run(&h.svc, &args, "test").await;
    assert_eq!(code, 0, "{out}");
    let (s, _, grant) = from_page(&h, HAL, &cookie, "/me", json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(grant.is_none());
    let d = h
        .svc
        .registered()
        .into_iter()
        .find(|d| d.handle == "hal")
        .unwrap();
    assert!(d.client_origins.is_empty());
    assert_eq!(d.return_urls, vec![callback(HAL)]);
}

/// The OIDC flow takes a redirect URI only when it is exactly one the
/// deployment lists: a longer path or a query under it is refused on the
/// service's page and never followed.
#[tokio::test(flavor = "multi_thread")]
async fn an_inexact_redirect_uri_is_refused_here() {
    let h = three("oidc-exact-redirect").await;
    let auth = authenticator(47, b"oidc-kai");
    let (cookie, _) = sign_up(&h, "notes", "Kai", &auth).await;
    for wrong in [
        format!("{}/more", callback(NOTES)),
        format!("{}?next=/x", callback(NOTES)),
        format!("{}?", callback(NOTES)),
    ] {
        let mut pairs = ask("notes", NOTES);
        set(&mut pairs, "redirect_uri", Some(&wrong));
        let (s, location, body) = authorize(&h, &pairs, Some(&cookie)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{wrong}");
        assert!(location.is_none(), "{wrong}");
        assert!(body.contains("redirect URI"), "{body}");
    }
    // The exact one works.
    code_for(&h, &ask("notes", NOTES), &cookie).await;
}

async fn codes(h: &H) -> u64 {
    h.svc
        .store()
        .pool()
        .rows("SELECT count(*) FROM oidc_codes")
        .await
        .unwrap()[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

/// A signed-in member who loops the authorization gets so many codes a
/// minute and no more, writes no audit row per code, and the minute tick
/// deletes spent and expired codes.
#[tokio::test(flavor = "multi_thread")]
async fn authorize_is_limited_per_session_and_old_codes_are_deleted() {
    let h = three_with(
        "oidc-authorize-limit",
        &[("ORDO_IDENTITY_AUTHORIZE_PER_SESSION", "3")],
    )
    .await;
    let auth = authenticator(48, b"oidc-lee");
    let (cookie, _) = sign_up(&h, "notes", "Lee", &auth).await;
    let pairs = ask("notes", NOTES);
    let rows_before = actions(&h).await.len();

    let first = code_for(&h, &pairs, &cookie).await;
    code_for(&h, &pairs, &cookie).await;
    code_for(&h, &pairs, &cookie).await;
    assert_eq!(
        actions(&h).await.len(),
        rows_before,
        "no audit row per code"
    );
    let (s, location, _) = authorize(&h, &pairs, Some(&cookie)).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let location = location.unwrap();
    assert_eq!(
        param(&location, "error").as_deref(),
        Some("temporarily_unavailable"),
        "{location}"
    );
    assert!(param(&location, "code").is_none());
    assert_eq!(codes(&h).await, 3);

    // One is spent: the tick deletes it at once, the rest once expired.
    let (s, v) = redeem(&h, "notes", NOTES, &first, VERIFIER).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    h.svc.tick().await.unwrap();
    assert_eq!(codes(&h).await, 2);
    h.clock.advance(61);
    h.svc.tick().await.unwrap();
    assert_eq!(codes(&h).await, 0);
    // A new minute, a new allowance.
    code_for(&h, &pairs, &cookie).await;
}

/// Under a base path the issuer carries it, discovery is served under it,
/// and two services under different paths on one origin are two issuers.
/// Deployment tokens keep the origin as `iss`, as doors check it.
#[tokio::test(flavor = "multi_thread")]
async fn under_a_base_path_the_issuer_carries_it_and_deployment_tokens_do_not() {
    use ordo_identity::oidc::OidcConnection;
    use ordo_identity::origin::OriginList;
    use ordo_identity_service::config::BASE_PATH_ENV;

    let a = three_with("oidc-base-a", &[(BASE_PATH_ENV, "/identity")]).await;
    let b = three_with("oidc-base-b", &[(BASE_PATH_ENV, "/accounts")]).await;
    let issuer = format!("{ORIGIN}/identity");
    let (s, v, _) = a
        .call(
            "GET",
            "/identity/.well-known/openid-configuration",
            &[],
            Value::Null,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["issuer"], issuer.as_str());
    assert_eq!(
        v["authorization_endpoint"],
        format!("{issuer}/oidc/authorize")
    );
    assert_eq!(v["token_endpoint"], format!("{issuer}/oidc/token"));
    assert_eq!(v["jwks_uri"], format!("{issuer}/.well-known/jwks.json"));
    let (_, other, _) = b
        .call(
            "GET",
            "/accounts/.well-known/openid-configuration",
            &[],
            Value::Null,
        )
        .await;
    assert_ne!(other["issuer"], v["issuer"]);

    let connection = |issuer: &str| OidcConnection {
        name: "ids".into(),
        issuer: issuer.into(),
        client_id: "notes".into(),
        client_secret: None,
        scope: "openid".into(),
        endpoints: None,
        use_email: false,
        display_claims: vec![],
        avatar_claim: None,
        skew_secs: 0,
        algs: vec![Alg::EdDSA],
        cache_secs: 0,
        stale_grace_secs: 0,
        redirect_uri: callback(NOTES),
        returns: OriginList::parse(NOTES, true, false).unwrap(),
        presence: Default::default(),
    };
    let (ca, cb) = (
        connection(v["issuer"].as_str().unwrap()),
        connection(other["issuer"].as_str().unwrap()),
    );
    assert_ne!(ca.fingerprint(), cb.fingerprint());
    assert_eq!(
        ca.discovery_url(),
        format!("{issuer}/.well-known/openid-configuration")
    );

    // A sign-in: the `iss` sent back and the ID token's carry the path.
    let auth = authenticator(49, b"oidc-max");
    let (cookie, _) = sign_up(&a, "notes", "Max", &auth).await;
    let (s, location, body) = authorize(&a, &ask("notes", NOTES), Some(&cookie)).await;
    assert_eq!(s, StatusCode::SEE_OTHER, "{body}");
    let location = location.unwrap();
    assert_eq!(param(&location, "iss").as_deref(), Some(issuer.as_str()));
    let code = param(&location, "code").unwrap();
    let (s, answer) = redeem(&a, "notes", NOTES, &code, VERIFIER).await;
    assert_eq!(s, StatusCode::OK, "{answer}");
    let (_, claims) = segments(answer["id_token"].as_str().unwrap());
    let claims: Value = serde_json::from_str(&claims).unwrap();
    assert_eq!(claims["iss"], issuer.as_str());

    // A deployment token still says the origin, and verifies as a door
    // verifies it.
    let (_, session) = a.person("Ned").await;
    let token = a.svc.token(&session.secret, AUD).await.unwrap().token;
    let jwks = a.jwks().await;
    let verified = deployment_token::verify(&token, &jwks, &a.expect()).expect("verifies");
    assert_eq!(verified.iss, ORIGIN);
}
