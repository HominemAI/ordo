//! One identity service shared by many deployments (`crate::deployments`,
//! owner's go 2026-11-14): `hal` and `ordo` registered with one service,
//! each with its own members, client origins and return URLs. A token for
//! one is refused by the other's door. And the binary boots on its own,
//! outside a supervisor.

mod support;

use axum::http::StatusCode;
use ordo_identity::deployment_token::{self, DeploymentTokenError, Expect};
use ordo_identity_service::commands;
use ordo_identity_service::deployments::Registration;
use ordo_identity_service::people::Actor;
use serde_json::{json, Value};
use support::*;

const HAL: &str = "http://localhost:9101";
const ORDO: &str = "http://localhost:9102";
const AUD_HAL: &str = "ordo-deployment:hal";
const AUD_ORDO: &str = "ordo-deployment:ordo";

const HOST: Actor<'static> = Actor {
    id: "unix:test",
    client: "host",
};

/// A service with `hal` and `ordo` registered.
async fn two(name: &str) -> H {
    let h = boot_with(name, &[]).await;
    for (handle, origin) in [("hal", HAL), ("ordo", ORDO)] {
        h.svc
            .register_deployment(
                HOST,
                &Registration {
                    handle: handle.into(),
                    client_origins: vec![origin.into()],
                    return_urls: vec![format!("{origin}/d/")],
                },
            )
            .await
            .unwrap();
    }
    h
}

/// `POST /token` from the page at `origin`, with the session's cookie.
async fn token_at(h: &H, origin: &str, secret: &str, aud: &str) -> (StatusCode, Value) {
    let cookie = format!("ordo-identity={secret}");
    let (s, v, _) = h
        .call(
            "POST",
            "/token",
            &[("origin", origin), ("cookie", &cookie)],
            json!({ "aud": aud }),
        )
        .await;
    (s, v)
}

fn door(h: &H, aud: &'static str) -> Expect<'static> {
    Expect {
        issuer: ORIGIN,
        audience: aud,
        now: h.now(),
        leeway_secs: 0,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_for_hal_is_refused_by_the_door_of_ordo() {
    let h = two("shared-two").await;
    let (adam, s) = h.person("Adam").await;
    h.svc.add_member(HOST, "hal", &adam).await.unwrap();

    let (st, v) = token_at(&h, HAL, &s.secret, AUD_HAL).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let t = v["token"].as_str().unwrap();
    let jwks = h.jwks().await;
    let claims = deployment_token::verify(t, &jwks, &door(&h, AUD_HAL)).unwrap();
    assert_eq!(
        (claims.aud.as_str(), claims.sub.as_str()),
        (AUD_HAL, adam.as_str())
    );
    assert_eq!(
        deployment_token::verify(t, &jwks, &door(&h, AUD_ORDO)).unwrap_err(),
        DeploymentTokenError::WrongAudience
    );
    assert_eq!(
        deployment_token::verify(t, &jwks, &h.expect()).unwrap_err(),
        DeploymentTokenError::WrongAudience,
        "nor by the service's own deployment"
    );

    // Not a member of ordo: no ordo token, from any page.
    let (st, v) = token_at(&h, ORDO, &s.secret, AUD_ORDO).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("not_a_member"))
    );
    // hal's page may not ask for ordo's token, member or not.
    h.svc.add_member(HOST, "ordo", &adam).await.unwrap();
    let (st, v) = token_at(&h, HAL, &s.secret, AUD_ORDO).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("origin_refused"))
    );
    let (st, v) = token_at(&h, ORDO, &s.secret, AUD_ORDO).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let t = v["token"].as_str().unwrap();
    assert!(deployment_token::verify(t, &jwks, &door(&h, AUD_ORDO)).is_ok());

    // One sign-in, one session: the same session holds both tokens, and
    // the service's own.
    assert!(h.svc.token(&s.secret, AUD).await.is_ok());

    // A deployment nobody registered is refused by name.
    let (st, v) = token_at(&h, HAL, &s.secret, "ordo-deployment:nobody").await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("wrong_audience"))
    );

    // Ended: no new token for it, at once.
    h.svc.end_deployment(HOST, "hal").await.unwrap();
    let (st, v) = token_at(&h, HAL, &s.secret, AUD_HAL).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_deployment_lists_its_own_return_urls() {
    let h = two("shared-return").await;
    let back = |deployment: Option<&'static str>, ret: &'static str| {
        let h = &h;
        async move {
            let mut body = json!({ "return": ret });
            if let Some(d) = deployment {
                body["deployment"] = json!(d);
            }
            let (s, v, _) = h.post("/sign-in/return", None, body).await;
            (s, v)
        }
    };
    let (s, v) = back(Some("hal"), "http://localhost:9101/d/hal/settings").await;
    assert_eq!(
        (s, v["to"].as_str()),
        (StatusCode::OK, Some("http://localhost:9101/d/hal/settings"))
    );
    // ordo's address is not on hal's list, and the reverse.
    for (d, ret) in [
        (Some("hal"), "http://localhost:9102/d/main"),
        (Some("ordo"), "http://localhost:9101/d/hal"),
        // Under the origin, outside the listed path.
        (Some("hal"), "http://localhost:9101/elsewhere"),
        (Some("hal"), "https://evil.example.com/d/"),
        (Some("nobody"), "http://localhost:9101/d/"),
    ] {
        let (s, v) = back(d, ret).await;
        assert_eq!(
            (s, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("return_refused")),
            "{d:?} {ret}"
        );
    }
    // No deployment named: any deployment's list, and the home's clients.
    assert_eq!(
        back(None, "http://localhost:9102/d/main").await.0,
        StatusCode::OK
    );
    assert_eq!(back(None, "http://localhost:8081/").await.0, StatusCode::OK);
    assert_eq!(
        back(Some("main"), "http://localhost:8081/x").await.0,
        StatusCode::OK
    );
    // A path on the service itself is always allowed.
    assert_eq!(back(Some("hal"), "/account").await.0, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registered_deployments_token_reaches_its_own_people_only() {
    let h = two("shared-scope").await;
    let (adam, s) = h.person("Adam").await;
    let (dana, _) = h.person("Dana").await;
    h.svc.add_member(HOST, "hal", &adam).await.unwrap();
    h.svc.add_member(HOST, "ordo", &dana).await.unwrap();
    let (_, v) = token_at(&h, HAL, &s.secret, AUD_HAL).await;
    let hal = v["token"].as_str().unwrap().to_string();

    // People: hal's members, not every account.
    let (st, v) = h.bearer("GET", "/people", &hal, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let ids: Vec<&str> = v["people"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert_eq!(ids, vec![adam.as_str()]);

    // An invite from hal's console makes a member of hal, with no access
    // to the service's own deployment.
    let (st, v) = h
        .bearer("POST", "/invites", &hal, json!({ "label": "Eve" }))
        .await;
    assert_eq!(
        (st, v["deployment"].as_str()),
        (StatusCode::OK, Some("hal")),
        "{v}"
    );
    let link = v["link"].as_str().unwrap().to_string();
    let auth = authenticator(31, b"eve");
    let secret = secret_of(&link);
    let (_, opts, _) = h
        .post("/join/begin", None, json!({ "secret": secret }))
        .await;
    let mut body = registration(&auth, &challenge(&opts));
    body["secret"] = json!(secret);
    body["displayName"] = json!("Eve");
    let (st, joined, headers) = h.post("/join/finish", None, body).await;
    assert_eq!(st, StatusCode::OK, "{joined}");
    let eve = joined["person"].as_str().unwrap().to_string();
    let eve_secret = cookie_of(&headers).split_once('=').unwrap().1.to_string();
    assert_eq!(
        token_at(&h, HAL, &eve_secret, AUD_HAL).await.0,
        StatusCode::OK
    );
    let (st, v) = token_at(&h, CONSOLE, &eve_secret, AUD).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("not_a_member"))
    );
    let (_, v) = h.bearer("GET", "/invites", &hal, Value::Null).await;
    assert_eq!(v["invites"].as_array().unwrap().len(), 1);

    // Ending from hal ends the membership, never the account.
    let (st, v) = h
        .bearer("POST", &format!("/people/{eve}/end"), &hal, Value::Null)
        .await;
    assert_eq!(
        (st, v["deployment"].as_str()),
        (StatusCode::OK, Some("hal")),
        "{v}"
    );
    assert_eq!(
        token_at(&h, HAL, &eve_secret, AUD_HAL).await.0,
        StatusCode::FORBIDDEN
    );
    // And not someone hal does not hold.
    let (st, _) = h
        .bearer("POST", &format!("/people/{dana}/end"), &hal, Value::Null)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // hal's last member stays.
    let (st, v) = h
        .bearer("POST", &format!("/people/{adam}/end"), &hal, Value::Null)
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::CONFLICT, Some("last_admin"))
    );

    // The service's own acts answer the service's own token alone.
    let (st, v) = h.bearer("GET", "/machine-keys", &hal, Value::Null).await;
    assert_eq!(
        (st, v["managedAt"].as_str()),
        (StatusCode::OK, Some("main"))
    );
    for (method, path) in [("GET", "/audit"), ("GET", "/deployments")] {
        let (st, v) = h.bearer(method, path, &hal, Value::Null).await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("home_only")),
            "{path}"
        );
    }
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments",
            &hal,
            json!({ "handle": "x", "clientOrigins": [HAL] }),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("home_only"))
    );
    let (st, _) = h
        .bearer("GET", "/deployments/ordo/members", &hal, Value::Null)
        .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "hal's token reaches no other deployment's members"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_service_registers_a_deployment_by_route_and_serves_its_page_at_once() {
    let h = boot_with("shared-route", &[]).await;
    let (adam, s) = h.person("Adam").await;
    let own = h.svc.token(&s.secret, AUD).await.unwrap().token;

    // Before: hal's page is a stranger.
    async fn pre(h: &H) -> (StatusCode, Option<axum::http::HeaderValue>) {
        let (st, _, headers) = h
            .call(
                "OPTIONS",
                "/token",
                &[("origin", HAL), ("access-control-request-method", "POST")],
                Value::Null,
            )
            .await;
        (st, headers.get("access-control-allow-origin").cloned())
    }
    assert_ne!(pre(&h).await.0, StatusCode::NO_CONTENT);

    let (st, v) = h
        .bearer(
            "POST",
            "/deployments",
            &own,
            json!({ "handle": "hal", "clientOrigins": [HAL] }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["audience"], AUD_HAL);
    assert_eq!(v["returnUrls"], json!([format!("{HAL}/")]));
    let (st, allow) = pre(&h).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(allow.unwrap(), HAL);

    // Taken while live.
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments",
            &own,
            json!({ "handle": "hal", "clientOrigins": [HAL] }),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::CONFLICT, Some("deployment_taken"))
    );

    let (st, _) = h
        .bearer(
            "POST",
            "/deployments/hal/members",
            &own,
            json!({ "person": adam }),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (_, v) = h.bearer("GET", "/deployments", &own, Value::Null).await;
    assert_eq!(v["deployments"][0]["handle"], "hal");
    let (_, v) = h
        .bearer("GET", "/deployments/hal/members", &own, Value::Null)
        .await;
    assert_eq!(v["members"][0]["id"], json!(adam));
    assert_eq!(
        token_at(&h, HAL, &s.secret, AUD_HAL).await.0,
        StatusCode::OK
    );

    // A stale touch may not register.
    h.clock.advance(400);
    let own = h.svc.token(&s.secret, AUD).await.unwrap().token;
    let (st, v) = h
        .bearer(
            "POST",
            "/deployments",
            &own,
            json!({ "handle": "ordo", "clientOrigins": [ORDO] }),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("presence_required"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_commands_register_and_add_members() {
    let h = boot_with("shared-commands", &[]).await;
    let (adam, s) = h.person("Adam").await;
    let run = |args: &[&str]| {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let svc = h.svc.clone();
        async move { commands::run(&svc, &args, "test").await }
    };
    let (code, out) = run(&[
        "deployment",
        "add",
        "hal",
        "--origin",
        HAL,
        "--return",
        &format!("{HAL}/d/"),
    ])
    .await;
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("ORDO_IDENTITY_DEPLOYMENT=hal"), "{out}");
    let (code, out) = run(&["member", "add", "hal", &adam]).await;
    assert_eq!(code, 0, "{out}");
    let (_, out) = run(&["deployment", "list"]).await;
    assert!(out.contains("hal") && out.contains(HAL), "{out}");
    let (_, out) = run(&["member", "list", "hal"]).await;
    assert!(out.contains(&adam), "{out}");
    assert_eq!(
        token_at(&h, HAL, &s.secret, AUD_HAL).await.0,
        StatusCode::OK
    );
    let (code, out) = run(&["invite", "--deployment", "hal", "--label", "Eve"]).await;
    assert_eq!(code, 0, "{out}");
    let (code, out) = run(&["invite", "--deployment", "nobody"]).await;
    assert_eq!(code, 1, "{out}");
    let (code, _) = run(&["deployment", "add", "x"]).await;
    assert_eq!(code, 1, "--origin is required");
    let (code, out) = run(&["deployment", "end", "hal"]).await;
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        token_at(&h, HAL, &s.secret, AUD_HAL).await.0,
        StatusCode::FORBIDDEN
    );
}

/// The binary on its own, as its own container would run it: only its own
/// settings, no supervisor. It migrates, mints the first invite, and
/// answers health and its keys.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_boots_alone_with_its_own_settings() {
    use std::io::{Read as _, Write as _};

    let url = fresh_db("shared-standalone").await;
    let dir = temp("shared-standalone");
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ordo-identity-service"))
        .env_clear()
        .env("ORDO_IDENTITY_DATABASE_URL", &url)
        .env("ORDO_IDENTITY_ORIGIN", format!("http://localhost:{port}"))
        .env("ORDO_IDENTITY_LISTEN", format!("127.0.0.1:{port}"))
        .env("ORDO_IDENTITY_DATA_DIR", dir.display().to_string())
        .env("ORDO_IDENTITY_DEPLOYMENT", "hominem")
        .env("ORDO_IDENTITY_CLIENT_ORIGINS", HAL)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the binary starts");
    let get = |path: &str| -> Option<String> {
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
        write!(
            c,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .ok()?;
        let mut out = String::new();
        c.read_to_string(&mut out).ok()?;
        Some(out)
    };
    let mut ready = None;
    for _ in 0..200 {
        if let Some(r) = get("/health/ready") {
            ready = Some(r);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let jwks = get("/.well-known/jwks.json");
    let _ = child.kill();
    let out = child.wait_with_output().unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    let ready = ready.unwrap_or_else(|| panic!("never answered:\n{log}"));
    assert!(
        ready.contains("\"identityDatabase\":\"ok\""),
        "{ready}\n{log}"
    );
    assert!(jwks.unwrap_or_default().contains("\"EdDSA\""));
    assert!(log.contains("for deployment `hominem`"), "{log}");
    assert!(dir.join("first-invite").exists(), "{log}");
    assert!(dir.join("identity.key").exists());
}
