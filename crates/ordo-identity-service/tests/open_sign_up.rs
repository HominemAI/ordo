//! Open sign-up, per deployment (owner, 2026-11-17): a registered
//! deployment whose mode is `open` takes anyone who makes an account with
//! a passkey, as an app member of it and nothing else. One passkey joins
//! several open deployments. A deployment in `invite` mode, and the
//! service's own, refuse it. Each try is counted per address.

mod support;

use axum::http::StatusCode;
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity_service::audit;
use ordo_identity_service::commands;
use ordo_identity_service::config::SIGN_UPS_PER_ADDRESS_ENV;
use ordo_identity_service::deployments::{Registration, SignUp};
use ordo_identity_service::people::Actor;
use serde_json::{json, Value};
use support::*;

const NOTES: &str = "http://localhost:9201";
const VIEWER: &str = "http://localhost:9202";
const HAL: &str = "http://localhost:9203";

const HOST: Actor<'static> = Actor {
    id: "unix:test",
    client: "host",
};

/// A service with `notes` and `note-viewer` open, and `hal` by invite.
async fn three(name: &str, extra: &[(&str, &str)]) -> H {
    let h = boot_with(name, extra).await;
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
                    return_urls: vec![format!("{origin}/")],
                    sign_up,
                },
            )
            .await
            .unwrap();
    }
    h
}

/// Open sign-up into `deployment` from the service's page at `peer`:
/// the status and the answer, and the session cookie on success.
async fn sign_up(
    h: &H,
    peer: &str,
    deployment: &str,
    name: &str,
    auth: &TestAuthenticator,
) -> (StatusCode, Value, String) {
    let (s, opts, _) = h
        .post_from(peer, "/sign-up/begin", json!({ "deployment": deployment }))
        .await;
    if s != StatusCode::OK {
        return (s, opts, String::new());
    }
    let mut body = registration(auth, &challenge(&opts));
    body["deployment"] = json!(deployment);
    body["displayName"] = json!(name);
    let (s, done, headers) = h.post_from(peer, "/sign-up/finish", body).await;
    let cookie = if s == StatusCode::OK {
        cookie_of(&headers)
    } else {
        String::new()
    };
    (s, done, cookie)
}

/// `POST /token` from the service's own page, with the session's cookie.
async fn token(h: &H, cookie: &str, aud: &str) -> (StatusCode, Value) {
    let (s, v, _) = h.post("/token", Some(cookie), json!({ "aud": aud })).await;
    (s, v)
}

/// The service's own admin token, touched now.
async fn own_token(h: &H) -> String {
    let (_, s) = h.person("Adam").await;
    h.svc.token(&s.secret, AUD).await.unwrap().token
}

async fn members(h: &H, own: &str, handle: &str) -> Vec<Value> {
    let (st, v) = h
        .bearer(
            "GET",
            &format!("/deployments/{handle}/members"),
            own,
            Value::Null,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v["members"].as_array().unwrap().clone()
}

async fn actions(h: &H) -> Vec<(String, String, String)> {
    audit::page(h.svc.store().pool(), h.svc.handle(), None, 200)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.action, r.actor, r.subject))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn open_sign_up_makes_an_app_member_of_that_deployment_only() {
    let h = three("open-member", &[]).await;
    let own = own_token(&h).await;
    let auth = authenticator(31, b"open-dana");
    let (st, done, cookie) = sign_up(&h, "198.51.100.7", "notes", "Dana", &auth).await;
    assert_eq!(st, StatusCode::OK, "{done}");
    assert_eq!(done["recoveryCodes"].as_array().unwrap().len(), 10);
    assert_eq!(done["deployment"], "notes");
    let dana = done["person"].as_str().unwrap().to_string();

    // A member of notes, as an app member, and of nothing else.
    let listed = members(&h, &own, "notes").await;
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0]["id"].as_str(), listed[0]["role"].as_str()),
        (Some(dana.as_str()), Some("member"))
    );
    assert!(members(&h, &own, "note-viewer").await.is_empty());
    assert!(!h.svc.has_home(&dana).await.unwrap());

    // An app member holds no token: the deployment's tokens are admin
    // tokens. Not for notes, not for the service's own, not for hal.
    let (st, v) = token(&h, &cookie, "ordo-deployment:notes").await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("app_member_only")),
        "{v}"
    );
    for aud in [AUD, "ordo-deployment:hal", "ordo-deployment:note-viewer"] {
        let (st, v) = token(&h, &cookie, aud).await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("not_a_member")),
            "{aud}: {v}"
        );
    }

    // The account page works for them.
    let (st, me, _) = h.post("/me", Some(&cookie), json!({})).await;
    assert_eq!(
        (st, me["person"].as_str()),
        (StatusCode::OK, Some(dana.as_str()))
    );

    // Recorded: the account, the membership, and how it came in.
    let log = actions(&h).await;
    assert!(
        log.iter()
            .any(|(a, actor, subject)| a == "sign-up.open" && *actor == dana && subject == "notes"),
        "{log:?}"
    );
    assert!(log
        .iter()
        .any(|(a, _, s)| a == "member.added" && *s == dana));
    assert!(log
        .iter()
        .any(|(a, _, s)| a == "person.added" && *s == dana));

    // An admin of notes makes the app member an admin: then a token.
    h.svc.add_member(HOST, "notes", &dana).await.unwrap();
    let (st, v) = token(&h, &cookie, "ordo-deployment:notes").await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_passkey_joins_a_second_open_deployment() {
    let h = three("open-second", &[]).await;
    let own = own_token(&h).await;
    let auth = authenticator(32, b"open-eli");
    let (st, done, _) = sign_up(&h, "198.51.100.8", "notes", "Eli", &auth).await;
    assert_eq!(st, StatusCode::OK, "{done}");
    let eli = done["person"].as_str().unwrap().to_string();

    // Later, on a fresh page: sign in with the same passkey, then join.
    let (_, opts, _) = h.post("/sign-in/begin", None, json!({})).await;
    let (st, v, headers) = h
        .post("/sign-in/finish", None, assertion(&auth, &challenge(&opts)))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let cookie = cookie_of(&headers);
    let (st, v, _) = h
        .post(
            "/sign-up/join",
            Some(&cookie),
            json!({ "deployment": "note-viewer" }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["member"], json!(eli));
    let listed = members(&h, &own, "note-viewer").await;
    assert_eq!(listed[0]["id"], json!(eli));
    assert_eq!(listed[0]["role"], "member");
    assert_eq!(members(&h, &own, "notes").await[0]["id"], json!(eli));

    // Joining again changes nothing.
    let (st, _, _) = h
        .post(
            "/sign-up/join",
            Some(&cookie),
            json!({ "deployment": "note-viewer" }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(members(&h, &own, "note-viewer").await.len(), 1);

    // hal takes people by invite: no joining it this way.
    let (st, v, _) = h
        .post(
            "/sign-up/join",
            Some(&cookie),
            json!({ "deployment": "hal" }),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("invite_only")),
        "{v}"
    );
    assert!(v["message"].as_str().unwrap().contains("invite"), "{v}");
    // No session, no join.
    let (st, _, _) = h
        .post("/sign-up/join", None, json!({ "deployment": "notes" }))
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    let log = actions(&h).await;
    assert!(
        log.iter().any(|(a, actor, subject)| a == "sign-up.joined"
            && *actor == eli
            && subject == "note-viewer"),
        "{log:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_by_invite_and_the_service_itself_refuse_open_sign_up() {
    let h = three("open-refused", &[]).await;
    let auth = authenticator(33, b"open-fay");
    for (handle, status, code) in [
        ("hal", StatusCode::FORBIDDEN, "invite_only"),
        ("main", StatusCode::FORBIDDEN, "invite_only"),
        ("nobody", StatusCode::NOT_FOUND, "unknown_deployment"),
    ] {
        let (st, v, _) = sign_up(&h, "198.51.100.9", handle, "Fay", &auth).await;
        assert_eq!(
            (st, v["error"].as_str()),
            (status, Some(code)),
            "{handle}: {v}"
        );
    }
    let (st, v, _) = sign_up(&h, "198.51.100.9", "hal", "Fay", &auth).await;
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("takes people by invite"),
        "{st} {v}"
    );

    // A ceremony begun for notes does not finish into note-viewer.
    let (_, opts, _) = h
        .post_from(
            "198.51.100.9",
            "/sign-up/begin",
            json!({ "deployment": "notes" }),
        )
        .await;
    let mut body = registration(&auth, &challenge(&opts));
    body["deployment"] = json!("note-viewer");
    body["displayName"] = json!("Fay");
    let (st, v, _) = h.post_from("198.51.100.9", "/sign-up/finish", body).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("code_refused")),
        "{v}"
    );
}

/// Open sign-up into hal begun from one address: the status.
async fn begin_hal(h: &H) -> StatusCode {
    h.post_from(
        "198.51.100.10",
        "/sign-up/begin",
        json!({ "deployment": "hal" }),
    )
    .await
    .0
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mode_is_set_at_registration_changed_later_and_never_open_at_home() {
    let h = boot_with("open-mode", &[]).await;
    let own = own_token(&h).await;
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments",
            &own,
            json!({ "handle": "notes", "clientOrigins": [NOTES], "signUp": "open" }),
        )
        .await;
    assert_eq!(
        (st, v["signUp"].as_str()),
        (StatusCode::OK, Some("open")),
        "{v}"
    );
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments",
            &own,
            json!({ "handle": "hal", "clientOrigins": [HAL] }),
        )
        .await;
    assert_eq!(
        (st, v["signUp"].as_str()),
        (StatusCode::OK, Some("invite")),
        "{v}"
    );
    let (_, v) = h.bearer("GET", "/deployments", &own, Value::Null).await;
    let modes: Vec<(String, String)> = v["deployments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["handle"].as_str().unwrap().to_string(),
                d["signUp"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        modes,
        vec![
            ("hal".to_string(), "invite".to_string()),
            ("notes".to_string(), "open".to_string())
        ]
    );
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments",
            &own,
            json!({ "handle": "x", "clientOrigins": [HAL], "signUp": "sometimes" }),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("bad_request"))
    );

    // The service's own deployment is never open.
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments/main/sign-up",
            &own,
            json!({ "signUp": "open" }),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::CONFLICT, Some("home_invite_only")),
        "{v}"
    );

    // hal opened: open sign-up works at once; closed again: refused.
    assert_eq!(begin_hal(&h).await, StatusCode::FORBIDDEN);
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments/hal/sign-up",
            &own,
            json!({ "signUp": "open" }),
        )
        .await;
    assert_eq!(
        (st, v["signUp"].as_str()),
        (StatusCode::OK, Some("open")),
        "{v}"
    );
    assert_eq!(begin_hal(&h).await, StatusCode::OK);
    // The host command does the same, and the list shows it.
    let run = |args: &[&str]| {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let svc = h.svc.clone();
        async move { commands::run(&svc, &args, "test").await }
    };
    let (code, out) = run(&["deployment", "sign-up", "hal", "invite"]).await;
    assert_eq!(code, 0, "{out}");
    assert_eq!(begin_hal(&h).await, StatusCode::FORBIDDEN);
    let (_, out) = run(&["deployment", "list"]).await;
    assert!(
        out.contains("sign-up: invite") && out.contains("sign-up: open"),
        "{out}"
    );
    let (code, out) = run(&["deployment", "sign-up", "main", "open"]).await;
    assert_eq!(code, 1, "{out}");
    let (code, out) = run(&[
        "deployment",
        "add",
        "viewer",
        "--origin",
        VIEWER,
        "--sign-up",
        "open",
    ])
    .await;
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Sign-up: open"), "{out}");

    let log = actions(&h).await;
    assert!(
        log.iter()
            .filter(|(a, _, s)| a == "deployment.sign-up" && s == "hal")
            .count()
            == 2,
        "{log:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn open_sign_up_is_counted_per_address() {
    let h = three("open-throttle", &[(SIGN_UPS_PER_ADDRESS_ENV, "2")]).await;
    let begin = |peer: &'static str| {
        let h = &h;
        async move {
            h.post_from(peer, "/sign-up/begin", json!({ "deployment": "notes" }))
                .await
        }
    };
    for _ in 0..2 {
        assert_eq!(begin("198.51.100.11").await.0, StatusCode::OK);
    }
    let (st, v, _) = begin("198.51.100.11").await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("too_many_attempts")),
        "{v}"
    );
    assert_eq!(v["retryAfterSecs"], 60);
    assert!(v["message"].as_str().unwrap().contains("try again"), "{v}");
    // Another address is not held up, and the window passes.
    assert_eq!(begin("198.51.100.12").await.0, StatusCode::OK);
    h.clock.advance(61);
    assert_eq!(begin("198.51.100.11").await.0, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_member_leaves_freely_and_the_last_admin_stays() {
    let h = three("open-leave", &[]).await;
    let (adam, _) = h.person("Adam").await;
    h.svc.add_member(HOST, "notes", &adam).await.unwrap();
    let auth = authenticator(34, b"open-gil");
    let (_, done, _) = sign_up(&h, "198.51.100.13", "notes", "Gil", &auth).await;
    let gil = done["person"].as_str().unwrap().to_string();
    let err = h.svc.end_member(HOST, "notes", &adam).await.unwrap_err();
    assert_eq!(err.code(), "last_admin");
    h.svc.end_member(HOST, "notes", &gil).await.unwrap();
}
