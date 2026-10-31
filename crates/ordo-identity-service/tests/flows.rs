//! The identity service's flows over HTTP, against Postgres and a software
//! authenticator: first start, sign-up by invite, sign-in, touch, recovery
//! codes, CLI grants, machine keys, the admin acts, and the host commands.

mod support;

use axum::http::StatusCode;
use ordo_identity::deployment_token::{self, Kind};
use ordo_identity::software_authenticator::TestAuthenticator;
use ordo_identity_service::audit;
use ordo_identity_service::cli::pkce_challenge;
use ordo_identity_service::commands;
use ordo_identity_service::config::{
    GUESSES_PER_ADDRESS_ENV, GUESSES_SHARED_ENV, PROVISIONER_KEY_ENV, PROXY_HEADER_ENV,
    PROXY_TRUSTED_ENV,
};
use ordo_identity_service::machine::{parse_secret, sign_challenge};
use ordo_identity_service::people::Actor;
use serde_json::{json, Value};
use support::*;

/// Sign up through an invite link with `auth`: the cookie and the answer.
async fn join(h: &H, link: &str, name: &str, auth: &TestAuthenticator) -> (String, Value) {
    let secret = secret_of(link);
    let (s, opts, _) = h
        .post("/join/begin", None, json!({ "secret": secret }))
        .await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let mut body = registration(auth, &challenge(&opts));
    body["secret"] = json!(secret);
    body["displayName"] = json!(name);
    let (s, done, headers) = h.post("/join/finish", None, body).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    (cookie_of(&headers), done)
}

async fn invite_link(h: &H) -> String {
    let by = Actor {
        id: "unix:test",
        client: "host",
    };
    h.svc.invite(by, "test").await.unwrap().link
}

/// A deployment token for a browser session.
async fn token(h: &H, cookie: &str) -> String {
    let (s, v, _) = h.post("/token", Some(cookie), json!({ "aud": AUD })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["token"].as_str().unwrap().to_string()
}

async fn sign_in(h: &H, auth: &TestAuthenticator) -> (StatusCode, Value, String) {
    let (_, opts, _) = h.post("/sign-in/begin", None, json!({})).await;
    let (s, v, headers) = h
        .post("/sign-in/finish", None, assertion(auth, &challenge(&opts)))
        .await;
    let cookie = if s == StatusCode::OK {
        cookie_of(&headers)
    } else {
        String::new()
    };
    (s, v, cookie)
}

#[tokio::test(flavor = "multi_thread")]
async fn first_start_mints_one_invite_that_signs_the_first_admin_up_once() {
    let h = boot_with("first-invite", &[]).await;
    let first = h.svc.first_start().await.unwrap();
    let minted = first.first_invite.expect("a first invite");
    assert!(first.provisioner.is_none());
    assert!(minted.link.starts_with(&format!("{ORIGIN}/join#")));
    assert_eq!(minted.expires_at, START + 7 * 86_400);
    let path = h.svc.first_invite_path();
    assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), minted.link);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // A second start mints nothing.
    let again = h.svc.first_start().await.unwrap();
    assert!(again.first_invite.is_none());

    let auth = authenticator(7, b"first-admin");
    let (cookie, done) = join(&h, &minted.link, "Adam", &auth).await;
    assert_eq!(done["recoveryCodes"].as_array().unwrap().len(), 10);
    assert!(done["person"].as_str().unwrap().starts_with("p_"));
    assert!(
        !path.exists(),
        "the first invite's file goes once it is used"
    );
    let t = token(&h, &cookie).await;
    let claims = deployment_token::verify(&t, &h.jwks().await, &h.expect()).unwrap();
    assert_eq!(
        (claims.kind, claims.touched_at),
        (Kind::Person, Some(START))
    );

    // Spent: the same link says only that it does not work.
    let (s, v, _) = h
        .post(
            "/join/begin",
            None,
            json!({ "secret": secret_of(&minted.link) }),
        )
        .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("code_refused"))
    );
    let rows = audit::rows(h.svc.store().pool(), "main").await.unwrap();
    assert!(rows
        .iter()
        .any(|r| r.action == "invite.refused" && r.outcome == "spent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_or_revoked_invite_does_not_work() {
    let h = boot_with("invite-ends", &[]).await;
    let old = invite_link(&h).await;
    h.clock.advance(7 * 86_400);
    let (s, _, _) = h
        .post("/join/begin", None, json!({ "secret": secret_of(&old) }))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let by = Actor {
        id: "unix:test",
        client: "host",
    };
    let m = h.svc.invite(by, "revoked").await.unwrap();
    h.svc.revoke_invite(by, &m.id).await.unwrap();
    let (s, _, _) = h
        .post("/join/begin", None, json!({ "secret": m.secret }))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let rows = audit::rows(h.svc.store().pool(), "main").await.unwrap();
    for why in ["expired", "revoked"] {
        assert!(
            rows.iter()
                .any(|r| r.action == "invite.refused" && r.outcome == why),
            "{why}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_provisioner_key_becomes_a_machine_that_signs_in_and_invites() {
    let seed = [5u8; 32];
    let public = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
    let hex: String = public
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let h = boot_with("provisioner", &[(PROVISIONER_KEY_ENV, &hex)]).await;
    let first = h.svc.first_start().await.unwrap();
    let id = first.provisioner.expect("the provisioner key");
    assert!(
        first.first_invite.is_none(),
        "no first invite with a provisioner"
    );
    assert!(h.svc.first_start().await.unwrap().provisioner.is_none());

    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let (_, begun, _) = h
        .call("POST", "/machine/begin", &[], json!({ "key": id }))
        .await;
    let ch = begun["challenge"].as_str().unwrap().to_string();
    let sig = sign_challenge(&key, &ch).unwrap();
    let (s, v, _) = h
        .call(
            "POST",
            "/machine/complete",
            &[],
            json!({ "key": id, "challenge": ch, "signature": sig }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let t = v["token"].as_str().unwrap();
    let (_, payload) = segments(t);
    assert!(!payload.contains("touched_at"));
    let c = deployment_token::verify(t, &h.jwks().await, &h.expect()).unwrap();
    assert_eq!(
        (c.kind, c.sub.as_str(), c.client.as_str()),
        (Kind::Machine, id.as_str(), "provisioner")
    );

    // The same challenge twice is refused.
    let (s, v, _) = h
        .call(
            "POST",
            "/machine/complete",
            &[],
            json!({ "key": id, "challenge": ch, "signature": sig }),
        )
        .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("machine_refused"))
    );

    // A machine writes with no touch (Q2).
    let (s, v) = h
        .bearer(
            "POST",
            "/invites",
            t,
            json!({ "label": "customer", "role": "admin" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["link"].as_str().unwrap().contains("/join#"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_needs_a_fresh_touch_and_touch_gives_a_token_that_carries_it() {
    let h = boot_with("touch", &[]).await;
    let auth = authenticator(7, b"dana");
    join(&h, &invite_link(&h).await, "Dana", &auth).await;
    h.clock.advance(60);
    let (s, v, cookie) = sign_in(&h, &auth).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    h.clock.advance(301);
    let stale = token(&h, &cookie).await;
    let (s, v) = h
        .bearer("POST", "/invites", &stale, json!({ "label": "x" }))
        .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("presence_required"))
    );
    assert_eq!(v["windowSecs"], 300);
    // Reads need no touch.
    let (s, v) = h.bearer("GET", "/people", &stale, Value::Null).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["people"].as_array().unwrap().len(), 1);

    let (_, opts, _) = h.post("/touch/begin", Some(&cookie), json!({})).await;
    let (s, v, _) = h
        .post(
            "/touch/finish",
            Some(&cookie),
            assertion(&auth, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["touchedAt"], h.now());
    let fresh = v["token"].as_str().unwrap();
    let (s, v) = h
        .bearer("POST", "/invites", fresh, json!({ "label": "x" }))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Another person's passkey does not touch this session.
    let eli = authenticator(9, b"eli");
    join(&h, &invite_link(&h).await, "Eli", &eli).await;
    let (_, opts, _) = h.post("/touch/begin", Some(&cookie), json!({})).await;
    let (s, _, _) = h
        .post(
            "/touch/finish",
            Some(&cookie),
            assertion(&eli, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

/// The console page runs the touch in its own page (plan 5.4 step 6): the
/// browser names the console's origin in the assertion, and the service
/// takes it because the console is a listed client under the RP ID. An
/// origin it does not list is refused at the door of the route, and an
/// assertion signed for one is refused too.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_page_touches_from_its_own_origin_and_an_unlisted_one_cannot() {
    let h = boot_with("console-touch", &[]).await;
    let auth = authenticator(7, b"dana");
    join(&h, &invite_link(&h).await, "Dana", &auth).await;
    let (s, v, cookie) = sign_in(&h, &auth).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    h.clock.advance(400);
    let mut at_console = authenticator(7, b"dana");
    at_console.origin = CONSOLE.to_string();
    let from_console = [("origin", CONSOLE), ("cookie", cookie.as_str())];
    let (s, opts, headers) = h
        .call("POST", "/touch/begin", &from_console, json!({}))
        .await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    assert_eq!(headers["access-control-allow-origin"], CONSOLE);
    let (s, v, _) = h
        .call(
            "POST",
            "/touch/finish",
            &from_console,
            assertion(&at_console, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["touchedAt"], h.now());

    // A page the service does not list cannot spend the cookie.
    let stranger = "http://localhost:9999";
    let (s, v, headers) = h
        .call(
            "POST",
            "/touch/begin",
            &[("origin", stranger), ("cookie", cookie.as_str())],
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert!(headers.get("access-control-allow-origin").is_none());
    // Nor does an assertion the browser made on such a page pass, even
    // when it is carried in from the console.
    let mut elsewhere = authenticator(7, b"dana");
    elsewhere.origin = stranger.to_string();
    let (_, opts, _) = h
        .call("POST", "/touch/begin", &from_console, json!({}))
        .await;
    let (s, _, _) = h
        .call(
            "POST",
            "/touch/finish",
            &from_console,
            assertion(&elsewhere, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

/// Only the touch may run in a client's page. Sign-up, sign-in and
/// enrolling another passkey accept the service's own origin alone, so a
/// ceremony a client page ran cannot open a session or add a key, while
/// the same person's touch from the console passes.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_touch_accepts_a_client_origin() {
    let h = boot_with("touch-only-widened", &[]).await;
    let link = invite_link(&h).await;

    // Sign-up whose registration the browser made on the console's page.
    let mut joiner = authenticator(8, b"erin");
    joiner.origin = CONSOLE.to_string();
    let secret = secret_of(&link);
    let (_, opts, _) = h
        .post("/join/begin", None, json!({ "secret": secret }))
        .await;
    let mut body = registration(&joiner, &challenge(&opts));
    body["secret"] = json!(secret);
    body["displayName"] = json!("Erin");
    let (s, v, _) = h.post("/join/finish", None, body).await;
    assert_ne!(s, StatusCode::OK, "a client page enrolled a passkey: {v}");

    // The same invite, from the service's page, still works.
    let auth = authenticator(7, b"dana");
    let (cookie, _) = join(&h, &link, "Dana", &auth).await;

    // Sign-in whose assertion names the console's origin.
    let mut at_console = authenticator(7, b"dana");
    at_console.origin = CONSOLE.to_string();
    let (_, opts, _) = h.post("/sign-in/begin", None, json!({})).await;
    let (s, v, _) = h
        .post(
            "/sign-in/finish",
            None,
            assertion(&at_console, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");

    // Enrolling another passkey from the console's page.
    let mut second = authenticator(9, b"dana-2");
    second.origin = CONSOLE.to_string();
    let (s, opts, _) = h.post("/passkeys/begin", Some(&cookie), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let (s, v, _) = h
        .post(
            "/passkeys/finish",
            Some(&cookie),
            registration(&second, &challenge(&opts)),
        )
        .await;
    assert_ne!(s, StatusCode::OK, "a client page enrolled a passkey: {v}");

    // The touch from the console's page passes.
    let from_console = [("origin", CONSOLE), ("cookie", cookie.as_str())];
    let (_, opts, _) = h
        .call("POST", "/touch/begin", &from_console, json!({}))
        .await;
    let (s, v, _) = h
        .call(
            "POST",
            "/touch/finish",
            &from_console,
            assertion(&at_console, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_code_signs_in_to_enrol_a_new_passkey_and_nothing_else() {
    let h = boot_with("recovery", &[]).await;
    let lost = authenticator(7, b"lost-phone");
    let (cookie, done) = join(&h, &invite_link(&h).await, "Dana", &lost).await;
    let code = done["recoveryCodes"][2].as_str().unwrap().to_string();
    h.post("/sessions/end", Some(&cookie), Value::Null).await;

    let (s, v, headers) = h.post("/recover", None, json!({ "code": code })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["mustEnrol"], true);
    let rc = cookie_of(&headers);
    let (s, v, _) = h.post("/token", Some(&rc), json!({ "aud": AUD })).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("enrol_a_new_passkey"))
    );

    let new = authenticator(8, b"new-phone");
    let (s, opts, _) = h.post("/passkeys/begin", Some(&rc), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    let (s, v, _) = h
        .post(
            "/passkeys/finish",
            Some(&rc),
            registration(&new, &challenge(&opts)),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["recoveryCodes"].as_array().unwrap().len(), 10);
    token(&h, &rc).await;

    // The lost passkey is gone; the new one signs in.
    assert_eq!(sign_in(&h, &lost).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(sign_in(&h, &new).await.0, StatusCode::OK);
    // The code is spent, and the rest of the old set was voided.
    for c in [&code, done["recoveryCodes"][3].as_str().unwrap()] {
        let (s, v, _) = h.post("/recover", None, json!({ "code": c })).await;
        assert_eq!(
            (s, v["error"].as_str()),
            (StatusCode::UNAUTHORIZED, Some("code_refused"))
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn guessing_is_counted_once_across_two_instances() {
    let h = boot_with("throttle", &[]).await;
    // A second instance of the same service, on the same database.
    let other = H {
        svc: reboot(&h.vars, &h.clock).await.expect("a second instance"),
        clock: h.clock.clone(),
        vars: h.vars.clone(),
    };
    let guess = json!({ "code": "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA" });
    // 30 tries split between them: each is a wrong code, none throttled.
    for i in 0..30 {
        let at = if i % 2 == 0 { &h } else { &other };
        let (s, _, _) = at.post("/recover", None, guess.clone()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "try {i}");
    }
    // The 31st, on either, is refused with when to try again.
    for at in [&h, &other] {
        let (s, v, _) = at.post("/recover", None, guess.clone()).await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
        assert_eq!(v["retryAfterSecs"], 60);
    }
    // Other kinds have their own window, and the window opens again.
    let (s, _, _) = other
        .post("/join/begin", None, json!({ "secret": "x" }))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    h.clock.advance(60);
    let (s, _, _) = h.post("/recover", None, guess).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cli_gets_a_session_by_pkce_and_refreshes_its_touch_the_same_way() {
    let h = boot_with("cli", &[]).await;
    let auth = authenticator(7, b"dana");
    let (cookie, done) = join(&h, &invite_link(&h).await, "Dana", &auth).await;
    let person = done["person"].as_str().unwrap().to_string();
    let verifier = "a-verifier-of-enough-length-for-pkce-0123456789";
    let (s, v, _) = h
        .post(
            "/cli/approve",
            Some(&cookie),
            json!({ "codeChallenge": pkce_challenge(verifier) }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let code = v["code"].as_str().unwrap().to_string();
    let (s, _, _) = h
        .call(
            "POST",
            "/cli/redeem",
            &[],
            json!({ "code": code, "verifier": "wrong" }),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v, _) = h
        .call(
            "POST",
            "/cli/redeem",
            &[],
            json!({ "code": code, "verifier": verifier }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let session = v["session"].as_str().unwrap().to_string();
    let (s, _, _) = h
        .call(
            "POST",
            "/cli/redeem",
            &[],
            json!({ "code": code, "verifier": verifier }),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "a code works once");

    let issued = h.svc.token(&session, AUD).await.unwrap();
    assert_eq!(
        (
            issued.claims.sub.as_str(),
            issued.claims.client.as_str(),
            issued.claims.touched_at
        ),
        (person.as_str(), "cli", Some(START))
    );

    // Later, `ordo login --touch`: a new approval refreshes the same session.
    h.clock.advance(400);
    let (_, opts, _) = h.post("/touch/begin", Some(&cookie), json!({})).await;
    h.post(
        "/touch/finish",
        Some(&cookie),
        assertion(&auth, &challenge(&opts)),
    )
    .await;
    let (_, v, _) = h
        .post(
            "/cli/approve",
            Some(&cookie),
            json!({ "codeChallenge": pkce_challenge(verifier) }),
        )
        .await;
    let code = v["code"].as_str().unwrap();
    let (s, v, _) = h
        .call(
            "POST",
            "/cli/redeem",
            &[],
            json!({ "code": code, "verifier": verifier, "session": session }),
        )
        .await;
    assert_eq!(
        (s, v["touched"].as_bool()),
        (StatusCode::OK, Some(true)),
        "{v}"
    );
    let again = h.svc.token(&session, AUD).await.unwrap();
    assert_eq!(again.claims.sid, issued.claims.sid);
    assert_eq!(again.claims.touched_at, Some(h.now()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_key_made_by_an_admin_signs_in_until_it_is_ended() {
    let h = boot_with("machine-keys", &[]).await;
    let (cookie, _) = join(&h, &invite_link(&h).await, "Dana", &authenticator(7, b"d")).await;
    let admin = token(&h, &cookie).await;
    let (s, v) = h
        .bearer(
            "POST",
            "/machine-keys",
            &admin,
            json!({ "name": "ci-main" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (id, key) = parse_secret(v["secret"].as_str().unwrap()).unwrap();
    assert_eq!(v["id"], id.as_str());
    let (s, v) = h
        .bearer(
            "POST",
            "/machine-keys",
            &admin,
            json!({ "name": "ci-main" }),
        )
        .await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::CONFLICT, Some("name_taken"))
    );

    let sign_in = || async {
        let (_, b, _) = h
            .call("POST", "/machine/begin", &[], json!({ "key": id }))
            .await;
        let ch = b["challenge"].as_str().unwrap().to_string();
        let sig = sign_challenge(&key, &ch).unwrap();
        h.call(
            "POST",
            "/machine/complete",
            &[],
            json!({ "key": id, "challenge": ch, "signature": sig }),
        )
        .await
    };
    let (s, v, _) = sign_in().await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let machine = v["token"].as_str().unwrap().to_string();
    let (s, _) = h
        .bearer("GET", "/machine-keys", &machine, Value::Null)
        .await;
    assert_eq!(s, StatusCode::OK);

    let (s, v) = h
        .bearer(
            "POST",
            &format!("/machine-keys/{id}/end"),
            &admin,
            Value::Null,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Its live token is refused here at once, and it signs in no more.
    let (s, v) = h.bearer("GET", "/people", &machine, Value::Null).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("session_ended"))
    );
    let (s, v, _) = sign_in().await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("machine_refused"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_person_over_http_refuses_their_token_here_at_once() {
    let h = boot_with("end-http", &[]).await;
    let (dana, _) = join(&h, &invite_link(&h).await, "Dana", &authenticator(7, b"d")).await;
    let (eli, done) = join(&h, &invite_link(&h).await, "Eli", &authenticator(9, b"e")).await;
    let eli_id = done["person"].as_str().unwrap().to_string();
    let (dt, et) = (token(&h, &dana).await, token(&h, &eli).await);
    let (s, v) = h
        .bearer("POST", &format!("/people/{eli_id}/end"), &dt, Value::Null)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, _) = h.bearer("GET", "/people", &et, Value::Null).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // A token from elsewhere is refused by its claims.
    let (s, v) = h.bearer("GET", "/people", "a.b.c", Value::Null).await;
    assert_eq!(
        (s, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("token_refused"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_host_commands_invite_break_glass_rotate_and_verify() {
    let h = boot_with("commands", &[]).await;
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    let (code, out) = commands::run(&h.svc, &args(&["invite", "--label", "Ops"]), "tester").await;
    assert_eq!(code, 0, "{out}");
    assert!(out.starts_with(&format!("{ORIGIN}/join#")), "{out}");

    let (code, out) = commands::run(&h.svc, &args(&["break-glass-token"]), "tester").await;
    assert_eq!(code, 0, "{out}");
    let t = out.trim();
    let c = deployment_token::verify(t, &h.jwks().await, &h.expect()).unwrap();
    assert_eq!(
        (c.kind, c.sub.as_str(), c.touched_at, c.exp),
        (
            Kind::Breakglass,
            "breakglass:tester",
            Some(START),
            START + 15 * 60
        )
    );
    let (s, v) = h
        .bearer("POST", "/invites", t, json!({ "label": "after a lockout" }))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (code, _) = commands::run(
        &h.svc,
        &args(&["break-glass-token", "--minutes", "61"]),
        "tester",
    )
    .await;
    assert_eq!(code, 1);

    let before = h.svc.keys().jwks()["keys"][0]["kid"].clone();
    let (code, out) = commands::run(&h.svc, &args(&["rotate-keys", "--now"]), "tester").await;
    assert_eq!(code, 0, "{out}");
    let after = h.svc.keys().jwks();
    assert_eq!(after["keys"].as_array().unwrap().len(), 1);
    assert_ne!(after["keys"][0]["kid"], before);

    let (code, out) = commands::run(&h.svc, &args(&["audit", "verify"]), "tester").await;
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("checks out"), "{out}");
    let k = &h.svc.keys().audit_keys()["keys"][0];
    let key = format!(
        "{}:{}",
        k["kid"].as_str().unwrap(),
        k["x"].as_str().unwrap()
    );
    let (code, out) =
        commands::run(&h.svc, &args(&["audit", "verify", "--key", &key]), "tester").await;
    assert_eq!(code, 0, "{out}");

    let rows = audit::rows(h.svc.store().pool(), "main").await.unwrap();
    for action in ["invite.issued", "break-glass", "key.rotated"] {
        assert!(
            rows.iter()
                .any(|r| r.action == action && r.actor == "unix:tester"),
            "{action}"
        );
    }
    let (code, out) = commands::run(&h.svc, &args(&["nonsense"]), "tester").await;
    assert_eq!(code, 1);
    assert!(out.contains("usage"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_address_over_its_limit_is_refused_while_another_gets_in() {
    let h = boot_with("throttle-addr", &[]).await;
    let other = H {
        svc: reboot(&h.vars, &h.clock).await.expect("a second instance"),
        clock: h.clock.clone(),
        vars: h.vars.clone(),
    };
    let guess = json!({ "code": "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA" });
    // The first address uses its 30 tries, split across both instances.
    for i in 0..30 {
        let at = if i % 2 == 0 { &h } else { &other };
        let (s, _, _) = at
            .post_from("198.51.100.1", "/recover", guess.clone())
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "try {i}");
    }
    // Its 31st is refused on either instance.
    for at in [&h, &other] {
        let (s, v, _) = at
            .post_from("198.51.100.1", "/recover", guess.clone())
            .await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
        assert_eq!(v["retryAfterSecs"], 60);
    }
    // Another address still gets in, on either instance.
    for at in [&h, &other] {
        let (s, _, _) = at
            .post_from("198.51.100.2", "/recover", guess.clone())
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shared_ceiling_refuses_everyone_across_two_instances() {
    // Per address 2, ceiling 5.
    let h = boot_with(
        "throttle-ceiling",
        &[(GUESSES_PER_ADDRESS_ENV, "2"), (GUESSES_SHARED_ENV, "5")],
    )
    .await;
    let other = H {
        svc: reboot(&h.vars, &h.clock).await.expect("a second instance"),
        clock: h.clock.clone(),
        vars: h.vars.clone(),
    };
    let guess = json!({ "code": "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA" });
    // Five addresses, one try each, alternating instances.
    for i in 0..5 {
        let at = if i % 2 == 0 { &h } else { &other };
        let (s, _, _) = at
            .post_from(&format!("203.0.113.{i}"), "/recover", guess.clone())
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "try {i}");
    }
    // A sixth, new address is refused by the ceiling on either instance.
    for at in [&h, &other] {
        let (s, v, _) = at
            .post_from("203.0.113.99", "/recover", guess.clone())
            .await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    }
    // Other kinds have their own ceiling.
    let (s, _, _) = other
        .post_from("203.0.113.99", "/join/begin", json!({ "secret": "x" }))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // The window opens again.
    h.clock.advance(60);
    let (s, _, _) = h.post_from("203.0.113.99", "/recover", guess).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_address_does_not_use_up_the_shared_ceiling() {
    let h = boot_with(
        "throttle-flood",
        &[(GUESSES_PER_ADDRESS_ENV, "2"), (GUESSES_SHARED_ENV, "4")],
    )
    .await;
    let guess = json!({ "code": "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA" });
    for _ in 0..20 {
        h.post_from("198.51.100.1", "/recover", guess.clone()).await;
    }
    // The flood used 2 of the 4; two other addresses still fit.
    for a in ["198.51.100.2", "198.51.100.3"] {
        let (s, _, _) = h.post_from(a, "/recover", guess.clone()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{a}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn behind_a_trusted_proxy_the_header_names_the_visitor_and_elsewhere_it_is_ignored() {
    let h = boot_with(
        "throttle-proxy",
        &[
            (PROXY_HEADER_ENV, "X-Forwarded-For"),
            (PROXY_TRUSTED_ENV, "10.0.0.0/8"),
            (GUESSES_PER_ADDRESS_ENV, "2"),
        ],
    )
    .await;
    let guess = json!({ "code": "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA" });
    let via = |proxy: &'static str, who: &'static str| {
        let h = &h;
        let guess = guess.clone();
        async move {
            h.call_from(
                Some(proxy),
                "POST",
                "/recover",
                &[("origin", ORIGIN), ("x-forwarded-for", who)],
                guess,
            )
            .await
            .0
        }
    };
    // Visitor .1 behind the trusted proxy 10.0.0.1: 2 tries, then refused.
    assert_eq!(
        via("10.0.0.1", "198.51.100.1").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        via("10.0.0.1", "198.51.100.1").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        via("10.0.0.1", "198.51.100.1").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Another visitor behind the same proxy gets in.
    assert_eq!(
        via("10.0.0.1", "198.51.100.2").await,
        StatusCode::UNAUTHORIZED
    );
    // From an untrusted peer the header is ignored: a spoofed value changes nothing.
    assert_eq!(
        via("192.0.2.7", "198.51.100.3").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        via("192.0.2.7", "198.51.100.4").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        via("192.0.2.7", "198.51.100.5").await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

/// The console's "Deployment people" screen reads the audit log through
/// `GET /audit`: newest first, paged, and a check of the chain run now;
/// a tampered row turns the check into a problem the screen shows red.
#[tokio::test(flavor = "multi_thread")]
async fn the_audit_page_is_newest_first_and_says_whether_the_chain_holds() {
    let h = boot_with("audit-page", &[]).await;
    let (dana, _) = join(&h, &invite_link(&h).await, "Dana", &authenticator(7, b"d")).await;
    invite_link(&h).await;
    let t = token(&h, &dana).await;
    let (s, v) = h.bearer("GET", "/audit?limit=2", &t, Value::Null).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let newest = entries[0]["seq"].as_u64().unwrap();
    let next = entries[1]["seq"].as_u64().unwrap();
    assert!(newest > next, "{v}");
    assert_eq!(v["verified"]["ok"], true, "{v}");
    let (_, older) = h
        .bearer(
            "GET",
            &format!("/audit?before={next}&limit=50"),
            &t,
            Value::Null,
        )
        .await;
    let older = older["entries"].as_array().unwrap();
    assert!(!older.is_empty());
    assert!(older.iter().all(|e| e["seq"].as_u64().unwrap() < next));
    let (s, _) = h.bearer("GET", "/audit?limit=x", &t, Value::Null).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = h.bearer("GET", "/audit", "a.b.c", Value::Null).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    h.svc
        .store()
        .pool()
        .rows("UPDATE audit SET detail = 'changed' WHERE seq = 1")
        .await
        .unwrap();
    let (s, v) = h.bearer("GET", "/audit", &t, Value::Null).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["verified"]["ok"], false, "{v}");
    assert!(v["verified"]["problem"].as_str().unwrap().len() > 3);
}
