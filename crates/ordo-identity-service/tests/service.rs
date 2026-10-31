//! The identity service against a real Postgres database: boot, keys and
//! the JWKS, the token's exact claims, expiry, audience, rotation, session
//! ends, people, sign-out, origins, and the audit chain.
//!
//! Each test makes its own database `ordo_identity_t_<name>` on the
//! cluster `ordo_exec::PoolConfig::from_env` names, and a manual clock.

mod support;

use axum::http::StatusCode;
use ordo_identity::deployment_token::{self, DeploymentTokenError, Expect};
use ordo_identity_service::audit;
use ordo_identity_service::config::*;
use ordo_identity_service::keys::{all_token_kids, ACTIVATE_AFTER_SECS, RETIRE_AFTER_SECS};
use ordo_identity_service::people::{self, Actor};
use ordo_identity_service::sessions::{self, NewSession, SessionKind};
use ordo_identity_service::{IdentityError, SessionEnd};
use serde_json::{json, Value};
use support::*;

#[tokio::test]
async fn boot_migrates_makes_one_key_of_each_kind_and_keeps_them_across_restarts() {
    let h = boot_with("boot", &[]).await;
    assert_eq!(h.svc.store().applied().await.unwrap(), vec![1, 2]);
    let (_, v, _) = h
        .call("GET", "/.well-known/jwks.json", &[], Value::Null)
        .await;
    let keys = v["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    let k = &keys[0];
    assert_eq!(
        (
            k["kty"].as_str(),
            k["crv"].as_str(),
            k["use"].as_str(),
            k["alg"].as_str()
        ),
        (Some("OKP"), Some("Ed25519"), Some("sig"), Some("EdDSA"))
    );
    let kid = k["kid"].as_str().unwrap().to_string();
    let (_, a, _) = h
        .call("GET", "/.well-known/ordo-audit-keys.json", &[], Value::Null)
        .await;
    let audit_kid = a["keys"][0]["kid"].as_str().unwrap().to_string();
    assert_ne!(kid, audit_kid, "the audit key is not a token key");

    // A restart reads the same keys.
    let again = reboot(&h.vars, &h.clock).await.expect("reboot");
    assert_eq!(again.keys().jwks()["keys"][0]["kid"], kid.as_str());
    assert_eq!(
        all_token_kids(again.store().pool(), "main")
            .await
            .unwrap()
            .len(),
        1
    );

    // Another key file opens none of them, and the refusal names it.
    let mut other = h.vars.clone();
    let path = temp("boot-otherkey").join("other.key");
    other.insert(KEY_FILE_ENV.to_string(), path.display().to_string());
    let e = reboot(&other, &h.clock).await.err().expect("refused");
    assert_eq!(e.setting_name(), Some(KEY_FILE_ENV), "{e}");
}

#[tokio::test]
async fn the_token_carries_exactly_the_claims_of_the_plan() {
    let h = boot_with("claims", &[]).await;
    let (pid, s) = h.person("Dana").await;
    h.clock.advance(5);
    let (status, v, headers) = h
        .call(
            "POST",
            "/token",
            &[
                ("cookie", &format!("ordo-identity={}", s.secret)),
                ("origin", CONSOLE),
            ],
            json!({ "aud": AUD }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(headers["access-control-allow-origin"], CONSOLE);
    assert_eq!(headers["access-control-allow-credentials"], "true");
    let token = v["token"].as_str().unwrap();
    let (header, payload) = segments(token);
    let kid = h.svc.keys().jwks()["keys"][0]["kid"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        header,
        format!(r#"{{"alg":"EdDSA","kid":"{kid}","typ":"ordo-deployment+jwt"}}"#)
    );
    let claims: Value = serde_json::from_str(&payload).unwrap();
    let jti = claims["jti"].as_str().unwrap();
    assert_eq!(jti.len(), 22, "128 random bits, base64url");
    assert_eq!(
        payload,
        format!(
            r#"{{"iss":"{ORIGIN}","aud":"{AUD}","sub":"{pid}","kind":"person","role":"admin","client":"console","sid":"{sid}","touched_at":{t},"iat":{iat},"exp":{exp},"jti":"{jti}"}}"#,
            sid = s.id,
            t = START,
            iat = START + 5,
            exp = START + 5 + 600,
        )
    );
    assert_eq!(v["exp"], START + 605);
    assert_eq!(v["sid"], s.id.as_str());
    let read = deployment_token::verify(token, &h.jwks().await, &h.expect()).expect("verifies");
    assert_eq!(read.sub, pid);
    assert_eq!(read.touched_at, Some(START));
}

#[tokio::test]
async fn a_token_expires_after_the_token_life_and_the_setting_changes_it() {
    let h = boot_with("expiry", &[(TOKEN_SECS_ENV, "120")]).await;
    let (_, s) = h.person("Dana").await;
    let issued = h.svc.token(&s.secret, AUD).await.unwrap();
    assert_eq!(issued.claims.exp, START + 120);
    let jwks = h.jwks().await;
    let at = |now| Expect { now, ..h.expect() };
    assert!(deployment_token::verify(&issued.token, &jwks, &at(START + 119)).is_ok());
    assert_eq!(
        deployment_token::verify(&issued.token, &jwks, &at(START + 120)),
        Err(DeploymentTokenError::Expired)
    );
}

#[tokio::test]
async fn another_audience_is_refused_when_asked_and_when_verified() {
    let h = boot_with("audience", &[]).await;
    let (_, s) = h.person("Dana").await;
    let (status, v, _) = h
        .call(
            "POST",
            "/token",
            &[],
            json!({ "aud": "ordo-deployment:other", "session": s.secret }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(v["error"], "wrong_audience");
    assert_eq!(v["setting"], DEPLOYMENT_ENV);
    let issued = h.svc.token(&s.secret, AUD).await.unwrap();
    let other = Expect {
        audience: "ordo-deployment:other",
        ..h.expect()
    };
    assert_eq!(
        deployment_token::verify(&issued.token, &h.jwks().await, &other),
        Err(DeploymentTokenError::WrongAudience)
    );
    let foreign = Expect {
        issuer: "http://localhost:9999",
        ..h.expect()
    };
    assert_eq!(
        deployment_token::verify(&issued.token, &h.jwks().await, &foreign),
        Err(DeploymentTokenError::WrongIssuer)
    );
}

#[tokio::test]
async fn rotation_publishes_first_signs_an_hour_later_and_keeps_the_old_key_until_a_day_after_its_last_token(
) {
    let h = boot_with("rotation", &[]).await;
    let (_, s) = h.person("Dana").await;
    let old_kid = h.svc.keys().jwks()["keys"][0]["kid"]
        .as_str()
        .unwrap()
        .to_string();
    let pool = h.svc.store().pool();

    let r = h
        .svc
        .keys()
        .rotate(pool, h.now(), false, "unix:ordo")
        .await
        .unwrap();
    assert_eq!(r.active_from, START + ACTIVATE_AFTER_SECS);
    assert!(r.retired.is_empty());
    assert_eq!(h.jwks().await.len(), 2, "published at once");

    // Until the new key starts, the old one signs.
    h.clock.advance(ACTIVATE_AFTER_SECS - 1);
    let last_old = h.svc.token(&s.secret, AUD).await.unwrap();
    assert_eq!(last_old.kid, old_kid);
    h.clock.advance(1);
    let first_new = h.svc.token(&s.secret, AUD).await.unwrap();
    assert_eq!(first_new.kid, r.kid);
    // The old key still verifies what it signed.
    let jwks = h.jwks().await;
    assert!(deployment_token::verify(&last_old.token, &jwks, &h.expect()).is_ok());
    assert!(deployment_token::verify(&first_new.token, &jwks, &h.expect()).is_ok());

    // A second before its retirement it is still published; then it goes.
    let retire_at = START + ACTIVATE_AFTER_SECS + 600 + RETIRE_AFTER_SECS;
    h.clock.advance(retire_at - 1 - h.now());
    h.svc.tick().await.unwrap();
    assert_eq!(h.jwks().await.len(), 2);
    h.clock.advance(1);
    h.svc.tick().await.unwrap();
    let jwks = h.jwks().await;
    assert_eq!(jwks.len(), 1);
    let then = Expect {
        now: last_old.claims.iat,
        ..h.expect()
    };
    assert_eq!(
        deployment_token::verify(&last_old.token, &jwks, &then)
            .unwrap_err()
            .word(),
        "kid-unknown",
        "a retired key verifies nothing"
    );
    let kids = all_token_kids(pool, "main").await.unwrap();
    assert_eq!(kids, vec![(old_kid, true), (r.kid, false)]);
    let rows = audit::rows(pool, "main").await.unwrap();
    assert!(rows
        .iter()
        .any(|r| r.action == "key.rotated" && r.actor == "unix:ordo"));
    assert!(rows
        .iter()
        .any(|r| r.action == "key.retired" && r.detail == "replaced"));
}

#[tokio::test]
async fn scheduled_rotation_follows_the_setting_and_zero_turns_it_off() {
    let h = boot_with("scheduled", &[(KEY_ROTATE_DAYS_ENV, "30")]).await;
    h.clock.advance(30 * 86_400 - 1);
    h.svc.tick().await.unwrap();
    assert_eq!(h.jwks().await.len(), 1);
    h.clock.advance(1);
    h.svc.tick().await.unwrap();
    assert_eq!(h.jwks().await.len(), 2);

    let off = boot_with("scheduled-off", &[(KEY_ROTATE_DAYS_ENV, "0")]).await;
    off.clock.advance(400 * 86_400);
    off.svc.tick().await.unwrap();
    assert_eq!(off.jwks().await.len(), 1);
}

#[tokio::test]
async fn rotate_now_signs_with_the_new_key_at_once_and_kills_the_old() {
    let h = boot_with("rotate-now", &[]).await;
    let (_, s) = h.person("Dana").await;
    let before = h.svc.token(&s.secret, AUD).await.unwrap();
    let r = h
        .svc
        .keys()
        .rotate(h.svc.store().pool(), h.now(), true, "unix:ordo")
        .await
        .unwrap();
    assert_eq!(r.retired, vec![before.kid.clone()]);
    let after = h.svc.token(&s.secret, AUD).await.unwrap();
    assert_eq!(after.kid, r.kid);
    let jwks = h.jwks().await;
    assert_eq!(jwks.len(), 1);
    assert_eq!(
        deployment_token::verify(&before.token, &jwks, &h.expect())
            .unwrap_err()
            .word(),
        "kid-unknown"
    );
    assert!(deployment_token::verify(&after.token, &jwks, &h.expect()).is_ok());
}

#[tokio::test]
async fn an_idle_session_ends_after_the_idle_hours_and_names_the_setting() {
    let h = boot_with("idle", &[(SESSION_IDLE_ENV, "12"), (SESSION_MAX_ENV, "24")]).await;
    let (_, s) = h.person("Dana").await;
    h.clock.advance(12 * 3600 - 1);
    h.svc
        .token(&s.secret, AUD)
        .await
        .expect("used just in time");
    h.clock.advance(12 * 3600);
    let e = h.svc.token(&s.secret, AUD).await.unwrap_err();
    assert!(
        matches!(
            e,
            IdentityError::SessionEnded(SessionEnd::Idle { limit_hours: 12 })
        ),
        "{e}"
    );
    assert_eq!(e.setting_name(), Some(SESSION_IDLE_ENV));
    // Ended for good: a later try says ended, not idle.
    let again = h.svc.token(&s.secret, AUD).await.unwrap_err();
    assert!(
        matches!(again, IdentityError::SessionEnded(SessionEnd::Ended)),
        "{again}"
    );
    let (status, v, _) = h
        .call(
            "POST",
            "/token",
            &[],
            json!({ "aud": AUD, "session": s.secret }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"], "session_ended");
    let rows = audit::rows(h.svc.store().pool(), "main").await.unwrap();
    assert!(rows
        .iter()
        .any(|r| r.action == "session.ended" && r.subject == s.id && r.detail.starts_with("idle")));
}

#[tokio::test]
async fn a_busy_session_still_ends_at_the_max_hours() {
    let h = boot_with("max", &[(SESSION_IDLE_ENV, "1"), (SESSION_MAX_ENV, "3")]).await;
    let (_, s) = h.person("Dana").await;
    assert_eq!(s.ends_at, START + 3 * 3600);
    for _ in 0..5 {
        h.clock.advance(1800);
        h.svc.token(&s.secret, AUD).await.expect("busy and young");
    }
    h.clock.advance(1800);
    let e = h.svc.token(&s.secret, AUD).await.unwrap_err();
    assert!(
        matches!(
            e,
            IdentityError::SessionEnded(SessionEnd::Absolute { limit_hours: 3 })
        ),
        "{e}"
    );
    assert_eq!(e.setting_name(), Some(SESSION_MAX_ENV));
}

#[tokio::test]
async fn ending_a_person_ends_their_sessions_and_the_last_admin_stays() {
    let h = boot_with("end-person", &[]).await;
    let (dana, ds) = h.person("Dana").await;
    let (eli, es) = h.person("Eli").await;
    let pool = h.svc.store().pool();
    let by = Actor {
        id: &eli,
        client: "console",
    };
    let ended = people::end(pool, "main", h.svc.keys(), &dana, by, h.now())
        .await
        .unwrap();
    assert_eq!(ended.sessions, 1);
    let e = h.svc.token(&ds.secret, AUD).await.unwrap_err();
    assert!(
        matches!(e, IdentityError::SessionEnded(SessionEnd::Ended)),
        "{e}"
    );
    h.svc
        .token(&es.secret, AUD)
        .await
        .expect("Eli is untouched");
    let last = people::end(pool, "main", h.svc.keys(), &eli, by, h.now())
        .await
        .unwrap_err();
    assert!(matches!(last, IdentityError::LastAdmin), "{last}");
    let again = people::end(pool, "main", h.svc.keys(), &dana, by, h.now())
        .await
        .unwrap_err();
    assert!(matches!(again, IdentityError::UnknownPerson), "{again}");
    assert_eq!(
        people::get(pool, "main", &dana)
            .await
            .unwrap()
            .unwrap()
            .ended_at,
        Some(START)
    );
}

#[tokio::test]
async fn sign_out_ends_the_session_and_clears_the_cookie() {
    let h = boot_with("sign-out", &[]).await;
    let (_, s) = h.person("Dana").await;
    let cookie = format!("ordo-identity={}", s.secret);
    let (status, _, headers) = h
        .call(
            "POST",
            "/sessions/end",
            &[("cookie", &cookie), ("origin", CONSOLE)],
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["set-cookie"]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    let e = h.svc.token(&s.secret, AUD).await.unwrap_err();
    assert!(
        matches!(e, IdentityError::SessionEnded(SessionEnd::Ended)),
        "{e}"
    );
    // Again, with nothing to end, still answers clean.
    let (status, _, _) = h
        .call(
            "POST",
            "/sessions/end",
            &[("cookie", &cookie), ("origin", CONSOLE)],
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_cookie_is_spent_only_from_an_allowed_origin() {
    let h = boot_with("origins", &[]).await;
    let (_, s) = h.person("Dana").await;
    let cookie = format!("ordo-identity={}", s.secret);
    for (origin, want) in [
        (Some("https://evil.example.com"), StatusCode::FORBIDDEN),
        // A cookie with no Origin is no browser call this service serves.
        (None, StatusCode::FORBIDDEN),
        (Some(ORIGIN), StatusCode::OK),
        (Some(CONSOLE), StatusCode::OK),
    ] {
        let mut headers = vec![("cookie", cookie.as_str())];
        if let Some(o) = origin {
            headers.push(("origin", o));
        }
        let (status, v, res) = h
            .call("POST", "/token", &headers, json!({ "aud": AUD }))
            .await;
        assert_eq!(status, want, "{origin:?}: {v}");
        if want == StatusCode::FORBIDDEN {
            assert_eq!(v["error"], "origin_refused");
            assert!(!res.contains_key("access-control-allow-origin"));
        }
    }
    // The preflight answers the console and nobody else.
    let (ok, _, res) = h
        .call(
            "OPTIONS",
            "/token",
            &[
                ("origin", CONSOLE),
                ("access-control-request-method", "POST"),
            ],
            Value::Null,
        )
        .await;
    assert_eq!(ok, StatusCode::NO_CONTENT);
    assert_eq!(res["access-control-allow-origin"], CONSOLE);
    let (no, _, _) = h
        .call(
            "OPTIONS",
            "/token",
            &[("origin", "https://evil.example.com")],
            Value::Null,
        )
        .await;
    assert_eq!(no, StatusCode::FORBIDDEN);
    // No session at all.
    let (status, v, _) = h.call("POST", "/token", &[], json!({ "aud": AUD })).await;
    assert_eq!(
        (status, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("not_signed_in"))
    );
}

#[tokio::test]
async fn a_machine_session_gets_a_machine_token_with_no_touch() {
    let h = boot_with("machine", &[]).await;
    let pool = h.svc.store().pool();
    let tx = pool.begin().await.unwrap();
    let s = sessions::open(
        &tx,
        "main",
        h.svc.keys(),
        h.svc.lifetimes(),
        NewSession {
            subject: "m_0123",
            kind: SessionKind::Machine,
            client: "ci-main",
            touched_at: Some(START),
            must_enrol: false,
        },
        h.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let issued = h.svc.token(&s.secret, AUD).await.unwrap();
    let (_, payload) = segments(&issued.token);
    assert!(!payload.contains("touched_at"), "{payload}");
    let c = deployment_token::verify(&issued.token, &h.jwks().await, &h.expect()).unwrap();
    assert_eq!(
        (c.kind, c.client.as_str()),
        (deployment_token::Kind::Machine, "ci-main")
    );
}

#[tokio::test]
async fn the_audit_chain_checkpoints_every_hundred_rows_and_names_a_tampered_row() {
    let h = boot_with("audit", &[]).await;
    // Boot wrote 2 rows (the 2 keys). Each person and session writes 2.
    for i in 0..49 {
        h.person(&format!("P{i}")).await;
    }
    let pool = h.svc.store().pool();
    let rows = audit::rows(pool, "main").await.unwrap();
    assert_eq!(rows.len(), 100);
    assert_eq!(rows[0].action, "key.made");
    assert_eq!(rows[0].prev, "0".repeat(64));
    let verifiers = audit::verifiers(pool, "main").await.unwrap();
    let checked = audit::verify(pool, "main", &verifiers).await.unwrap();
    assert_eq!((checked.rows, checked.checkpoints), (100, 1));

    // A row left uncovered gets a checkpoint once 10 minutes have passed.
    h.person("Late").await;
    h.svc.tick().await.unwrap();
    assert_eq!(
        audit::verify(pool, "main", &verifiers)
            .await
            .unwrap()
            .checkpoints,
        1
    );
    h.clock.advance(audit::CHECKPOINT_SECS);
    h.svc.tick().await.unwrap();
    assert_eq!(
        audit::verify(pool, "main", &verifiers)
            .await
            .unwrap()
            .checkpoints,
        2
    );

    // Change one row's words: the check names it.
    pool.rows("UPDATE audit SET detail = 'forged' WHERE deployment = 'main' AND seq = 37")
        .await
        .unwrap();
    let e = audit::verify(pool, "main", &verifiers).await.unwrap_err();
    match e {
        IdentityError::AuditBroken(audit::AuditBreak::Chain(t)) => {
            assert_eq!(t.index, 36);
            assert_eq!(t.kind, ordo_commandlog::TamperKind::HashMismatch);
        }
        other => panic!("{other}"),
    }
}

#[tokio::test]
async fn a_checkpoint_checked_with_a_key_that_did_not_sign_it_is_refused() {
    let h = boot_with("audit-key", &[]).await;
    for i in 0..49 {
        h.person(&format!("P{i}")).await;
    }
    let pool = h.svc.store().pool();
    let stranger = ordo_commandlog::Ed25519Signer::from_seed(
        h.svc.keys().audit_keys()["keys"][0]["kid"]
            .as_str()
            .unwrap(),
        &[9; 32],
    )
    .verifier();
    let e = audit::verify(pool, "main", &[stranger]).await.unwrap_err();
    assert!(
        matches!(
            e,
            IdentityError::AuditBroken(audit::AuditBreak::Checkpoint { upto: 100, .. })
        ),
        "{e}"
    );
    assert!(
        audit::verify(pool, "main", &[]).await.is_err(),
        "no key, no trust"
    );
}
