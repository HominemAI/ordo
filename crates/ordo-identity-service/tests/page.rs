//! The service's own page (step 2): each page and file is served with the
//! headers it needs, and the two JSON routes the account page added
//! (`POST /me`, `POST /passkeys/{id}/remove`) do what the page relies on.

mod support;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity_service::page::{ASSETS, PAGES, PAGE_CSP};
use ordo_identity_service::people::Actor;
use serde_json::{json, Value};
use support::*;
use tower::ServiceExt;

/// A plain GET, as a browser loads a page: no `Origin`, no cookie.
async fn get(h: &H, path: &str) -> (StatusCode, HeaderMap, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let res = h.svc.router().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn join(h: &H, name: &str, auth: &TestAuthenticator) -> String {
    let by = Actor {
        id: "unix:test",
        client: "host",
    };
    let link = h.svc.invite(by, "test").await.unwrap().link;
    let secret = secret_of(&link);
    let (s, opts, _) = h
        .post("/join/begin", None, json!({ "secret": secret }))
        .await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let mut body = registration(auth, &challenge(&opts));
    body["secret"] = json!(secret);
    body["displayName"] = json!(name);
    let (s, done, headers) = h.post("/join/finish", None, body).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    cookie_of(&headers)
}

async fn add_passkey(h: &H, cookie: &str, auth: &TestAuthenticator) -> (StatusCode, Value) {
    let (s, opts, _) = h.post("/passkeys/begin", Some(cookie), json!({})).await;
    if s != StatusCode::OK {
        return (s, opts);
    }
    let (s, v, _) = h
        .post(
            "/passkeys/finish",
            Some(cookie),
            registration(auth, &challenge(&opts)),
        )
        .await;
    (s, v)
}

async fn touch(h: &H, cookie: &str, auth: &TestAuthenticator) {
    let (_, opts, _) = h.post("/touch/begin", Some(cookie), json!({})).await;
    let (s, v, _) = h
        .post(
            "/touch/finish",
            Some(cookie),
            assertion(auth, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

/// Under a base path (`ORDO_IDENTITY_BASE_PATH`, step 7): the console
/// passes the service through at `/identity` on its own origin, so an
/// upgraded deployment needs no new port. Every page, file and call the
/// page makes is under it; the root still answers the door and the console
/// on loopback; the session cookie is scoped to the base path; an invite
/// link opens the page under it.
#[tokio::test(flavor = "multi_thread")]
async fn under_a_base_path_the_page_its_files_and_calls_stay_under_it() {
    use ordo_identity_service::config::BASE_PATH_ENV;
    let h = boot_with("base_path", &[(BASE_PATH_ENV, "/identity")]).await;
    let (s, _, html) = get(&h, "/identity/sign-in").await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        html.contains("href=\"/identity/page/identity.css\""),
        "{html}"
    );
    assert!(html.contains("src=\"/identity/page/sign-in.js\""), "{html}");
    assert!(
        html.contains("name=\"ordo-base\" content=\"/identity\""),
        "{html}"
    );
    assert!(!html.contains("=\"/page/"), "{html}");
    let (s, _, js) = get(&h, "/identity/page/common.js").await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        js.contains("fetch(BASE + path"),
        "the page's calls take the base"
    );
    let (s, headers, _) = get(&h, "/identity/").await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/identity/account");
    // The root keeps answering, for loopback callers.
    assert_eq!(get(&h, "/health/ready").await.0, StatusCode::OK);
    assert_eq!(get(&h, "/.well-known/jwks.json").await.0, StatusCode::OK);
    assert_eq!(
        get(&h, "/identity/.well-known/jwks.json").await.0,
        StatusCode::OK
    );

    let by = Actor {
        id: "unix:test",
        client: "host",
    };
    let link = h.svc.invite(by, "test").await.unwrap().link;
    assert!(
        link.starts_with(&format!("{ORIGIN}/identity/join#")),
        "{link}"
    );
    let secret = secret_of(&link);
    let auth = authenticator(31, b"base-path-key");
    let (s, opts, _) = h
        .post("/identity/join/begin", None, json!({ "secret": secret }))
        .await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let mut body = registration(&auth, &challenge(&opts));
    body["secret"] = json!(secret);
    body["displayName"] = json!("Ada");
    let (s, done, headers) = h.post("/identity/join/finish", None, body).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    let set = headers["set-cookie"].to_str().unwrap();
    assert!(set.contains("Path=/identity;"), "{set}");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_page_is_sent_with_a_strict_policy_never_framed_and_never_stored() {
    let h = boot_with("page_headers", &[]).await;
    assert!(PAGE_CSP.contains("frame-ancestors 'none'"));
    assert!(PAGE_CSP.contains("script-src 'self'"));
    assert!(PAGE_CSP.contains("default-src 'none'"));
    assert!(!PAGE_CSP.contains("unsafe"), "{PAGE_CSP}");
    for (path, _) in PAGES {
        let (status, headers, body) = get(&h, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(
            headers["content-type"], "text/html; charset=utf-8",
            "{path}"
        );
        assert_eq!(headers["content-security-policy"], PAGE_CSP, "{path}");
        assert_eq!(headers["cache-control"], "no-store", "{path}");
        assert_eq!(headers["x-frame-options"], "DENY", "{path}");
        assert_eq!(headers["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(headers["referrer-policy"], "no-referrer", "{path}");
        assert_eq!(
            headers["cross-origin-opener-policy"], "same-origin",
            "{path}"
        );
        assert!(
            headers["permissions-policy"]
                .to_str()
                .unwrap()
                .contains("publickey-credentials-get=(self)"),
            "{path}"
        );
        assert!(body.starts_with("<!doctype html>"), "{path}");
        assert!(
            body.contains(r#"<script type="module" src="/page/"#),
            "{path}"
        );
        assert!(
            body.contains(r#"role="alert""#),
            "{path}: no announced problem line"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_scripts_and_stylesheet_are_served_and_nothing_else_is() {
    let h = boot_with("page_assets", &[]).await;
    for (name, kind, text) in ASSETS {
        let (status, headers, body) = get(&h, &format!("/page/{name}")).await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(headers["content-type"], *kind, "{name}");
        assert_eq!(headers["x-content-type-options"], "nosniff", "{name}");
        assert_eq!(headers["cross-origin-resource-policy"], "same-origin");
        assert_eq!(headers["cache-control"], "no-cache", "{name}");
        assert_eq!(&body, text, "{name}");
    }
    for missing in ["/page/nope.js", "/page/..%2Fsrc%2Flib.rs", "/page/"] {
        let (status, _, _) = get(&h, missing).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }
    let (status, headers, _) = get(&h, "/").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/account");
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_names_the_origins_it_may_return_to_and_the_api_keeps_its_own_policy() {
    let h = boot_with("page_return", &[]).await;
    let (_, _, body) = get(&h, "/sign-in").await;
    assert!(
        body.contains(&format!(
            r#"<meta name="ordo-return-origins" content="{ORIGIN} {CONSOLE}">"#
        )),
        "{body}"
    );
    let (_, headers, _) = get(&h, "/health/live").await;
    assert_eq!(
        headers["content-security-policy"],
        "default-src 'none'; frame-ancestors 'none'; base-uri 'none'"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn me_answers_the_signed_in_person_and_nobody_else() {
    let h = boot_with("page_me", &[]).await;
    let (s, v, _) = h.post("/me", None, json!({})).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("not_signed_in"))
    );

    let dana = authenticator(3, b"dana-1");
    let cookie = join(&h, "Dana", &dana).await;
    let (s, v, _) = h.post("/me", Some(&cookie), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["displayName"], "Dana");
    assert_eq!(v["role"], "admin");
    assert_eq!(v["recoveryCodesLeft"], 10);
    assert_eq!(v["mustEnrol"], false);
    assert_eq!(v["touchFresh"], true);
    assert_eq!(v["rpId"], "localhost");
    let keys = v["passkeys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["registeredAt"], START);
    assert!(keys[0]["id"].as_str().is_some_and(|i| !i.is_empty()));

    h.clock.advance(301);
    let (_, v, _) = h.post("/me", Some(&cookie), json!({})).await;
    assert_eq!(v["touchFresh"], false);

    // The cookie from another site's page is refused.
    let (s, v, _) = h
        .call(
            "POST",
            "/me",
            &[("origin", "https://evil.example"), ("cookie", &cookie)],
            json!({}),
        )
        .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("origin_refused"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_passkey_is_removed_with_a_fresh_touch_and_never_the_last() {
    let h = boot_with("page_remove", &[]).await;
    let first = authenticator(4, b"dana-first");
    let second = authenticator(5, b"dana-second");
    let cookie = join(&h, "Dana", &first).await;
    let (s, v) = add_passkey(&h, &cookie, &second).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        v.get("recoveryCodes").is_none(),
        "an added passkey makes no codes"
    );

    let (_, me, _) = h.post("/me", Some(&cookie), json!({})).await;
    let ids: Vec<String> = me["passkeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2);
    let second_id = ids
        .iter()
        .find(|i| **i == ordo_identity::webauthn::b64url_encode(b"dana-second"))
        .expect("the second passkey is listed")
        .clone();

    // Stale touch: refused, nothing removed.
    h.clock.advance(301);
    let path = format!("/passkeys/{second_id}/remove");
    let (s, v, _) = h.post(&path, Some(&cookie), json!({})).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("presence_required"))
    );

    // Someone else's passkey is not theirs to remove.
    let eli = authenticator(6, b"eli-1");
    let eli_cookie = join(&h, "Eli", &eli).await;
    let (s, v, _) = h.post(&path, Some(&eli_cookie), json!({})).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("unknown_passkey"))
    );

    touch(&h, &cookie, &first).await;
    let (s, v, _) = h.post(&path, Some(&cookie), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v, _) = h.post(&path, Some(&cookie), json!({})).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("unknown_passkey"))
    );

    // The last one stays.
    let first_id = ordo_identity::webauthn::b64url_encode(b"dana-first");
    let (s, v, _) = h
        .post(
            &format!("/passkeys/{first_id}/remove"),
            Some(&cookie),
            json!({}),
        )
        .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::CONFLICT, Some("last_passkey"))
    );
    let (_, me, _) = h.post("/me", Some(&cookie), json!({})).await;
    assert_eq!(me["passkeys"].as_array().unwrap().len(), 1);

    // The removed passkey no longer signs in.
    let (_, opts, _) = h.post("/sign-in/begin", None, json!({})).await;
    let (s, _, _) = h
        .post(
            "/sign-in/finish",
            None,
            assertion(&second, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let rows = h
        .svc
        .store()
        .pool()
        .rows("SELECT detail FROM audit WHERE action = 'passkey.removed'")
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}
