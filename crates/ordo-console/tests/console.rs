//! The console's step 1a, end to end through its router: setup code,
//! passkey enrolment and sign-in, sessions, step-up, recovery codes, a
//! second owner, `ordo console recover`, the audit log and the boot
//! refusals.
//!
//! Passkeys come from `ordo_identity`'s software authenticator: a real key
//! signing real WebAuthn responses. Each test has its own database on the
//! conformance cluster (`scripts/conformance-pg.sh start`; CI's service
//! container), read from `ORDO_PG_*`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use ordo_console::config::{
    DATABASE_URL_ENV, DATA_DIR_ENV, MODE_ENV, ORIGIN_ENV, PAGE_DIR_ENV, SESSION_IDLE_ENV,
    SESSION_MAX_ENV,
};
use ordo_console::{BootOptions, Clock, Console, ConsoleConfig, ConsoleError, DatabaseSettings};
use ordo_exec::PoolConfig;
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey, DEFAULT_ORIGIN};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{Argon2idHasher, CompleteRequest};
use serde_json::{json, Value};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;
const IDLE: u64 = 600;
const MAX: u64 = 3600;

fn cluster() -> PoolConfig {
    PoolConfig::from_env()
}

fn url_for(db: &str) -> String {
    let c = cluster();
    let mut url = format!(
        "host={} port={} user={} dbname={db}",
        c.host, c.port, c.user
    );
    if let Some(p) = &c.password {
        url.push_str(&format!(" password={p}"));
    }
    url
}

async fn fresh_db(name: &str) -> String {
    let db = format!("ordo_console_t_{name}");
    let c = cluster();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    url_for(&db)
}

fn vars(url: &str, dir: &std::path::Path) -> HashMap<String, String> {
    HashMap::from([
        (DATABASE_URL_ENV.to_string(), url.to_string()),
        (ORIGIN_ENV.to_string(), DEFAULT_ORIGIN.to_string()),
        ("ORDO_CONSOLE_RP_ID".to_string(), "example.com".to_string()),
        (DATA_DIR_ENV.to_string(), dir.display().to_string()),
        (
            PAGE_DIR_ENV.to_string(),
            dir.join("page").display().to_string(),
        ),
        (SESSION_IDLE_ENV.to_string(), IDLE.to_string()),
        (SESSION_MAX_ENV.to_string(), MAX.to_string()),
        // Step 1's cases run without the supervisor; `supervisor.rs` has
        // its own.
        (MODE_ENV.to_string(), "door-only".to_string()),
    ])
}

fn config(v: &HashMap<String, String>) -> Result<ConsoleConfig, ConsoleError> {
    ConsoleConfig::from_vars(&|k| v.get(k).cloned())
}

struct H {
    console: Console,
    router: Router,
    dir: PathBuf,
    url: String,
}

struct Answer {
    status: StatusCode,
    body: Value,
    cookie: Option<String>,
    headers: axum::http::HeaderMap,
}

const PAGE_INDEX: &str = "<!doctype html><title>Ordo console</title><div id=\"root\"></div>";

async fn boot(name: &str) -> H {
    let url = fresh_db(name).await;
    let dir = std::env::temp_dir().join(format!("ordo-console-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // A stand-in for the built page: an index and one hashed asset.
    std::fs::create_dir_all(dir.join("page/assets")).expect("page dir");
    std::fs::write(dir.join("page/index.html"), PAGE_INDEX).expect("index");
    std::fs::write(dir.join("page/assets/index-abc.js"), "export {};").expect("asset");
    let cfg = config(&vars(&url, &dir)).expect("config");
    let console = Console::boot(
        cfg,
        BootOptions {
            clock: Clock::manual(START),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("boot");
    H {
        router: console.router(),
        console,
        dir,
        url,
    }
}

impl H {
    fn api(&self, path: &str) -> String {
        format!("/d/main/api/{path}")
    }

    async fn send(&self, req: Request<Body>) -> Answer {
        let res = self.router.clone().oneshot(req).await.expect("answer");
        let status = res.status();
        let headers = res.headers().clone();
        let cookie = headers
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::to_string);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Answer {
            status,
            body,
            cookie,
            headers,
        }
    }

    async fn post(&self, path: &str, body: Value, cookie: Option<&str>) -> Answer {
        let mut req = Request::post(self.api(path))
            .header("origin", DEFAULT_ORIGIN)
            .header("content-type", "application/json");
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        self.send(req.body(Body::from(body.to_string())).unwrap())
            .await
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> Answer {
        let mut req = Request::get(path);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        self.send(req.body(Body::empty()).unwrap()).await
    }

    fn setup_code(&self) -> String {
        std::fs::read_to_string(self.dir.join("setup-code"))
            .expect("the setup code file")
            .trim()
            .to_string()
    }

    fn clock(&self) -> &Clock {
        self.console.clock()
    }

    async fn audit(&self) -> Vec<(String, String, String)> {
        self.console
            .store()
            .pool()
            .rows("SELECT actor, action, outcome FROM audit ORDER BY id")
            .await
            .expect("audit")
            .into_iter()
            .map(|r| {
                (
                    r[0].clone().unwrap_or_default(),
                    r[1].clone().unwrap_or_default(),
                    r[2].clone().unwrap_or_default(),
                )
            })
            .collect()
    }
}

fn registration(auth: &TestAuthenticator, challenge: &str) -> Value {
    let CompleteRequest::PasskeyRegistration {
        client_data_json,
        attestation_object,
        ..
    } = auth.register(challenge)
    else {
        unreachable!()
    };
    json!({
        "challenge": challenge,
        "clientDataJSON": b64url_encode(&client_data_json),
        "attestationObject": b64url_encode(&attestation_object),
    })
}

fn assertion(auth: &TestAuthenticator, challenge: &str) -> Value {
    let CompleteRequest::PasskeyAssertion {
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
        ..
    } = auth.assert(challenge)
    else {
        unreachable!()
    };
    json!({
        "challenge": challenge,
        "credentialId": b64url_encode(&credential_id),
        "clientDataJSON": b64url_encode(&client_data_json),
        "authenticatorData": b64url_encode(&authenticator_data),
        "signature": b64url_encode(&signature),
    })
}

fn challenge(a: &Answer) -> String {
    a.body["publicKey"]["challenge"]
        .as_str()
        .unwrap_or_else(|| panic!("no challenge in {}", a.body))
        .to_string()
}

fn second_key() -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(11),
        credential_id: b"credential-two".to_vec(),
        ..TestAuthenticator::es256()
    }
}

fn third_key() -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::ed25519(13),
        credential_id: b"credential-three".to_vec(),
        ..TestAuthenticator::es256()
    }
}

/// Enrol with a setup code; the session cookie and the whole answer.
async fn enrol(h: &H, code: &str, auth: &TestAuthenticator) -> (String, Answer) {
    let begun = h.post("setup/begin", json!({ "code": code }), None).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    let mut body = registration(auth, &challenge(&begun));
    body["code"] = json!(code);
    let done = h.post("setup/finish", body, None).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    (done.cookie.clone().expect("a session cookie"), done)
}

async fn sign_in(h: &H, auth: &TestAuthenticator) -> Answer {
    let begun = h.post("signin/begin", json!({}), None).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    h.post("signin/finish", assertion(auth, &challenge(&begun)), None)
        .await
}

async fn step_up(h: &H, cookie: &str, auth: &TestAuthenticator) -> Answer {
    let begun = h.post("stepup/begin", json!({}), Some(cookie)).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    h.post(
        "stepup/finish",
        assertion(auth, &challenge(&begun)),
        Some(cookie),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn health_answers_with_strict_headers() {
    let h = boot("health").await;
    let live = h.get("/health/live", None).await;
    assert_eq!(live.status, StatusCode::OK);
    let ready = h.get("/health/ready", None).await;
    assert_eq!(ready.status, StatusCode::OK);
    assert_eq!(ready.body["status"], "ready");
    assert_eq!(ready.body["consoleDatabase"], "ok");
    for a in [&live, &ready] {
        let csp = a.headers["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("default-src 'self'"), "{csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
        assert_eq!(a.headers["referrer-policy"], "no-referrer");
    }
    let root = h.get("/", None).await;
    assert_eq!(root.status, StatusCode::SEE_OTHER);
    assert_eq!(root.headers["location"], "/d/main/");
}

/// Raw bytes of an answer that is not JSON.
async fn raw(h: &H, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let res = h
        .router
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .expect("answer");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_is_served_under_the_handle_with_strict_headers() {
    let h = boot("page").await;
    for path in ["/d/main/", "/d/main/owners", "/d/main/owners/deeper"] {
        let (status, headers, body) = raw(&h, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body, PAGE_INDEX, "{path}");
        assert!(headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        assert!(headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("default-src 'self'"));
        assert_eq!(headers["cache-control"], "no-store");
    }
    // An unknown handle and an unknown call are refusals, not the page.
    assert_eq!(raw(&h, "/d/other/").await.0, StatusCode::NOT_FOUND);
    let (status, _, body) = raw(&h, "/d/main/api/nothing-here").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("not_found"), "{body}");
    // Hashed assets are cached for good; nothing outside assets/ is served.
    let (status, headers, body) = raw(&h, "/assets/index-abc.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "export {};");
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/javascript"));
    assert!(headers["cache-control"]
        .to_str()
        .unwrap()
        .contains("immutable"));
    for bad in [
        "/assets/../index.html",
        "/assets/missing.js",
        "/assets/index.html",
    ] {
        assert_eq!(raw(&h, bad).await.0, StatusCode::NOT_FOUND, "{bad}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn status_says_only_whether_an_owner_exists() {
    let h = boot("status").await;
    let before = h.get("/d/main/api/status", None).await;
    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(
        before.body,
        json!({ "deployment": "main", "hasOwner": false })
    );
    let code = h.setup_code();
    let _ = enrol(&h, &code, &TestAuthenticator::es256()).await;
    let after = h.get("/d/main/api/status", None).await;
    assert_eq!(
        after.body,
        json!({ "deployment": "main", "hasOwner": true })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_console_database_gets_no_deployment_tables() {
    let h = boot("own_tables").await;
    let rows = h
        .console
        .store()
        .pool()
        .rows(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name LIKE '$%'",
        )
        .await
        .unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("0"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_setup_code_works_once() {
    let h = boot("setup_once").await;
    let code = h.setup_code();
    let (_cookie, done) = enrol(&h, &code, &TestAuthenticator::es256()).await;
    let codes = done.body["recoveryCodes"].as_array().unwrap();
    assert_eq!(codes.len(), 10);
    assert_eq!(done.body["kind"], "first-run");
    assert!(
        !h.dir.join("setup-code").exists(),
        "the file goes once the code is used"
    );
    let again = h.post("setup/begin", json!({ "code": code }), None).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    assert_eq!(again.body["error"], "code_refused");
    let audit = h.audit().await;
    assert!(audit
        .iter()
        .any(|(actor, action, outcome)| actor.starts_with("owner:")
            && action == "setup.finish"
            && outcome == "ok"));
    assert!(audit
        .iter()
        .any(|(_, action, outcome)| action == "setup.begin" && outcome == "refused:code_refused"));

    // A reboot with an owner prints no new code.
    let cfg = config(&vars(&h.url, &h.dir)).unwrap();
    Console::boot(
        cfg,
        BootOptions {
            clock: Clock::manual(START),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("reboot");
    assert!(!h.dir.join("setup-code").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_signs_in_with_a_passkey() {
    let h = boot("sign_in").await;
    let auth = TestAuthenticator::es256();
    let (first, done) = enrol(&h, &h.setup_code(), &auth).await;
    let owner = done.body["owner"].as_str().unwrap().to_string();
    assert!(first.starts_with("__Host-ordo-console="));

    let signed = sign_in(&h, &auth).await;
    assert_eq!(signed.status, StatusCode::OK, "{}", signed.body);
    assert_eq!(signed.body["owner"], owner.as_str());
    let set = signed.headers["set-cookie"].to_str().unwrap();
    for part in ["HttpOnly", "SameSite=Strict", "Secure", "Path=/"] {
        assert!(set.contains(part), "{set}");
    }
    let cookie = signed.cookie.unwrap();
    let session = h.get("/d/main/api/session", Some(&cookie)).await;
    assert_eq!(session.status, StatusCode::OK);
    assert_eq!(session.body["owner"], owner.as_str());

    // A passkey the console never enrolled signs nobody in.
    let stranger = sign_in(&h, &second_key()).await;
    assert_eq!(stranger.status, StatusCode::UNAUTHORIZED);
    assert_eq!(stranger.body["error"], "passkey_refused");

    // A write without the console's own Origin is refused.
    let req = Request::post(h.api("signout"))
        .header("origin", "https://evil.example.org")
        .header("cookie", &cookie)
        .body(Body::from("{}"))
        .unwrap();
    let refused = h.send(req).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.body["error"], "origin_refused");

    let out = h.post("signout", json!({}), Some(&cookie)).await;
    assert_eq!(out.status, StatusCode::OK);
    let after = h.get("/d/main/api/session", Some(&cookie)).await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
    assert!(h
        .audit()
        .await
        .iter()
        .any(|(a, action, outcome)| *a == format!("owner:{owner}")
            && action == "signin.finish"
            && outcome == "ok"));
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_end_at_the_idle_and_absolute_limits() {
    let h = boot("expiry").await;
    let auth = TestAuthenticator::es256();
    let (idle_cookie, _) = enrol(&h, &h.setup_code(), &auth).await;
    h.clock().advance(IDLE + 1);
    let idle = h.get("/d/main/api/session", Some(&idle_cookie)).await;
    assert_eq!(idle.status, StatusCode::UNAUTHORIZED);
    assert_eq!(idle.body["error"], "session_ended");
    assert_eq!(idle.body["setting"], SESSION_IDLE_ENV);

    let busy = sign_in(&h, &auth).await.cookie.unwrap();
    let mut used = 0;
    while used + IDLE - 1 <= MAX {
        h.clock().advance(IDLE - 1);
        used += IDLE - 1;
        let ok = h.get("/d/main/api/session", Some(&busy)).await;
        assert_eq!(ok.status, StatusCode::OK, "at {used}: {}", ok.body);
    }
    h.clock().advance(IDLE - 1);
    let old = h.get("/d/main/api/session", Some(&busy)).await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    assert_eq!(old.body["setting"], SESSION_MAX_ENV);
}

#[tokio::test(flavor = "multi_thread")]
async fn step_up_needs_a_touch_in_the_last_300_seconds() {
    let h = boot("step_up").await;
    let auth = TestAuthenticator::es256();
    let (cookie, _) = enrol(&h, &h.setup_code(), &auth).await;
    h.clock().advance(301);
    let stale = h.post("owners/invite", json!({}), Some(&cookie)).await;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);
    assert_eq!(stale.body["error"], "step_up_required");
    assert_eq!(stale.body["windowSecs"], 300);

    // Another passkey cannot answer this owner's step-up.
    let begun = h.post("stepup/begin", json!({}), Some(&cookie)).await;
    let wrong = h
        .post(
            "stepup/finish",
            assertion(&second_key(), &challenge(&begun)),
            Some(&cookie),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    let touched = step_up(&h, &cookie, &auth).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.body);
    h.clock().advance(299);
    let fresh = h.post("owners/invite", json!({}), Some(&cookie)).await;
    assert_eq!(fresh.status, StatusCode::OK, "{}", fresh.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_code_works_once_and_forces_a_new_passkey_and_set() {
    let h = boot("recovery").await;
    let lost_key = TestAuthenticator::es256();
    let (cookie, done) = enrol(&h, &h.setup_code(), &lost_key).await;
    let codes: Vec<String> = done.body["recoveryCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    // A session the lost passkey opened elsewhere (a thief's, say).
    let elsewhere = sign_in(&h, &lost_key).await.cookie.unwrap();
    h.post("signout", json!({}), Some(&cookie)).await;

    let redeemed = h.post("recovery", json!({ "code": codes[0] }), None).await;
    assert_eq!(redeemed.status, StatusCode::OK, "{}", redeemed.body);
    assert_eq!(redeemed.body["mustEnrol"], true);
    // Enrolment not done yet (or abandoned): nothing is retired. The old
    // passkey still signs in, and its session still answers.
    assert_eq!(
        sign_in(&h, &lost_key).await.status,
        StatusCode::OK,
        "a recovery that enrolled nothing retired the old passkey"
    );
    assert_eq!(
        h.get("/d/main/api/session", Some(&elsewhere)).await.status,
        StatusCode::OK
    );
    let limited = redeemed.cookie.unwrap();
    let blocked = h.get("/d/main/api/owners", Some(&limited)).await;
    assert_eq!(blocked.status, StatusCode::FORBIDDEN);
    assert_eq!(blocked.body["error"], "enrol_a_new_passkey");

    let twice = h.post("recovery", json!({ "code": codes[0] }), None).await;
    assert_eq!(twice.status, StatusCode::UNAUTHORIZED);
    assert_eq!(twice.body["error"], "code_refused");

    let fresh_key = second_key();
    let begun = h.post("passkeys/begin", json!({}), Some(&limited)).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    let added = h
        .post(
            "passkeys/finish",
            registration(&fresh_key, &challenge(&begun)),
            Some(&limited),
        )
        .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.body);
    let new_codes = added.body["recoveryCodes"].as_array().unwrap();
    assert_eq!(new_codes.len(), 10);

    let now_open = h.get("/d/main/api/owners", Some(&limited)).await;
    assert_eq!(now_open.status, StatusCode::OK);
    let old = h.post("recovery", json!({ "code": codes[1] }), None).await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED, "the old set is void");
    assert_eq!(sign_in(&h, &fresh_key).await.status, StatusCode::OK);
    // Enrolment completed: the old passkey is retired with it, and the
    // session it opened is over.
    assert_eq!(
        sign_in(&h, &lost_key).await.status,
        StatusCode::UNAUTHORIZED,
        "a retired passkey signs in"
    );
    let ended = h.get("/d/main/api/session", Some(&elsewhere)).await;
    assert_eq!(ended.status, StatusCode::UNAUTHORIZED, "{}", ended.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_owner_is_invited_and_removing_one_needs_a_touch() {
    let h = boot("second_owner").await;
    let first_key = TestAuthenticator::es256();
    let (first, done) = enrol(&h, &h.setup_code(), &first_key).await;
    let first_owner = done.body["owner"].as_str().unwrap().to_string();
    let invite = h.post("owners/invite", json!({}), Some(&first)).await;
    assert_eq!(invite.status, StatusCode::OK, "{}", invite.body);
    let code = invite.body["code"].as_str().unwrap().to_string();

    let (second, joined) = enrol(&h, &code, &second_key()).await;
    let second_owner = joined.body["owner"].as_str().unwrap().to_string();
    assert_ne!(first_owner, second_owner);
    assert_eq!(joined.body["kind"], "invite");
    let reuse = h.post("setup/begin", json!({ "code": code }), None).await;
    assert_eq!(reuse.status, StatusCode::UNAUTHORIZED);

    let list = h.get("/d/main/api/owners", Some(&first)).await;
    assert_eq!(list.body["owners"].as_array().unwrap().len(), 2);
    let third = h.post("owners/invite", json!({}), Some(&first)).await;
    assert_eq!(third.status, StatusCode::CONFLICT);
    assert_eq!(third.body["error"], "owner_limit");

    h.clock().advance(301);
    let untouched = h
        .post(
            "owners/remove",
            json!({ "owner": second_owner }),
            Some(&first),
        )
        .await;
    assert_eq!(untouched.status, StatusCode::FORBIDDEN);
    assert_eq!(untouched.body["error"], "step_up_required");
    assert_eq!(step_up(&h, &first, &first_key).await.status, StatusCode::OK);
    let removed = h
        .post(
            "owners/remove",
            json!({ "owner": second_owner }),
            Some(&first),
        )
        .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
    let gone = h.get("/d/main/api/session", Some(&second)).await;
    assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        sign_in(&h, &second_key()).await.status,
        StatusCode::UNAUTHORIZED
    );

    let last = h
        .post(
            "owners/remove",
            json!({ "owner": first_owner }),
            Some(&first),
        )
        .await;
    assert_eq!(last.status, StatusCode::CONFLICT);
    assert_eq!(last.body["error"], "last_owner");
}

#[tokio::test(flavor = "multi_thread")]
async fn recover_issues_a_code_and_using_it_ends_every_session() {
    let h = boot("recover").await;
    let auth = TestAuthenticator::es256();
    let (one, done) = enrol(&h, &h.setup_code(), &auth).await;
    let owner = done.body["owner"].as_str().unwrap().to_string();
    let two = sign_in(&h, &auth).await.cookie.unwrap();

    let db =
        DatabaseSettings::from_vars(&|k| (k == DATABASE_URL_ENV).then(|| h.url.clone())).unwrap();
    let issued = ordo_console::recover::recover(&db, "main", None, "tester", h.clock())
        .await
        .expect("recover");
    assert_eq!(issued.owner.as_deref(), Some(owner.as_str()));
    assert!(h
        .audit()
        .await
        .iter()
        .any(|(actor, action, outcome)| actor == "unix:tester"
            && action == "console.recover"
            && outcome == "ok"));

    // Both sessions still work until the code is used.
    assert_eq!(
        h.get("/d/main/api/session", Some(&two)).await.status,
        StatusCode::OK
    );
    let (fresh, used) = enrol(&h, &issued.code, &third_key()).await;
    assert_eq!(used.body["kind"], "recover");
    assert_eq!(used.body["owner"], owner.as_str());
    assert_eq!(used.body["sessionsEnded"], 2);
    for old in [&one, &two] {
        let a = h.get("/d/main/api/session", Some(old)).await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED);
        assert_eq!(a.body["error"], "session_ended");
    }
    assert_eq!(
        h.get("/d/main/api/session", Some(&fresh)).await.status,
        StatusCode::OK
    );
    // The passkey the owner lost is retired with the recovery.
    assert_eq!(
        sign_in(&h, &auth).await.status,
        StatusCode::UNAUTHORIZED,
        "a retired passkey signs in"
    );
    assert_eq!(sign_in(&h, &third_key()).await.status, StatusCode::OK);
    let again = h
        .post("setup/begin", json!({ "code": issued.code }), None)
        .await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_refuses_without_a_console_database() {
    let dir = std::env::temp_dir();
    let mut v = vars("unused", &dir);
    v.remove(DATABASE_URL_ENV);
    let e = config(&v).unwrap_err();
    assert_eq!(e.setting_name(), Some(DATABASE_URL_ENV));
    assert!(e.to_string().contains(DATABASE_URL_ENV), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_refuses_the_deployments_own_database() {
    let dir = std::env::temp_dir();
    let mut v = vars("postgres://console@localhost:5432/ordo_app", &dir);
    v.insert("ORDO_PG_HOST".into(), "127.0.0.1".into());
    v.insert("ORDO_PG_DB".into(), "ordo_app".into());
    let e = config(&v).unwrap_err();
    assert_eq!(e.setting_name(), Some(DATABASE_URL_ENV));
    assert!(e.to_string().contains("deployment's own database"), "{e}");

    v.insert("ORDO_PG_DB".into(), "ordo_other".into());
    assert!(config(&v).is_ok(), "a different database boots");
    v.insert("ORDO_PG_DB".into(), "ordo_app".into());
    v.insert("ORDO_PG_PORT".into(), "5433".into());
    assert!(config(&v).is_ok(), "another cluster boots");
}

/// A console database nobody listens on is a boot refusal that names the
/// setting, the host and the port, and never the password.
#[tokio::test]
async fn an_unreachable_console_database_names_host_and_port() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let url = format!("postgres://console:s3cretpw@127.0.0.1:{port}/ordo_console");
    let db =
        DatabaseSettings::from_vars(&|k| (k == DATABASE_URL_ENV).then(|| url.clone())).unwrap();
    let err = ordo_console::store::Store::open(&db)
        .await
        .err()
        .expect("refused");
    let m = err.to_string();
    assert!(m.contains(DATABASE_URL_ENV), "{m}");
    assert!(m.contains(&format!("127.0.0.1:{port}")), "{m}");
    assert!(m.contains("listening on"), "{m}");
    assert!(!m.contains("s3cretpw"), "{m}");
}
