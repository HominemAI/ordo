//! Attack-catalog entries that target the console server, replayed against
//! a real console: its router, its own Postgres database, its setup code.
//! One test per catalog id, named by the id
//! (`security/attack-catalog/COVERAGE.md` points each id here). Each test
//! expects today's safe answer; `docs/corpus-rulings.md` wins over a
//! catalog's `expected_safe`.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`; CI's
//! service container), read from `ORDO_PG_*`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use ordo_console::config::{
    DATABASE_URL_ENV, DATA_DIR_ENV, MODE_ENV, ORIGIN_ENV, PAGE_DIR_ENV, SESSION_IDLE_ENV,
    SESSION_MAX_ENV,
};
use ordo_console::{BootOptions, Clock, Console, ConsoleConfig};
use ordo_exec::PoolConfig;
use ordo_identity::software_authenticator::DEFAULT_ORIGIN;
use ordo_identity::Argon2idHasher;
use serde_json::{json, Value};
use tower::ServiceExt;

const PAGE_INDEX: &str = "<!doctype html><title>Ordo console</title><div id=\"root\"></div>";
const WRONG_CODE: &str = "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA";
const FOREIGN: &str = "https://evil.test";

struct World {
    router: Router,
    dir: std::path::PathBuf,
    _console: Console,
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
    fn error(&self) -> String {
        self.json()["error"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

async fn boot(name: &str) -> World {
    let c = PoolConfig::from_env();
    let db = format!("ordo_console_t_cat_{name}");
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let mut url = format!(
        "host={} port={} user={} dbname={db}",
        c.host, c.port, c.user
    );
    if let Some(p) = &c.password {
        url.push_str(&format!(" password={p}"));
    }
    let dir = std::env::temp_dir().join(format!("ordo-console-cat-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("page/assets")).expect("page dir");
    std::fs::write(dir.join("page/index.html"), PAGE_INDEX).expect("index");
    std::fs::write(dir.join("page/assets/index-abc.js"), "export {};").expect("asset");
    let v: HashMap<String, String> = HashMap::from([
        (DATABASE_URL_ENV.to_string(), url),
        (ORIGIN_ENV.to_string(), DEFAULT_ORIGIN.to_string()),
        ("ORDO_CONSOLE_RP_ID".to_string(), "example.com".to_string()),
        (DATA_DIR_ENV.to_string(), dir.display().to_string()),
        (
            PAGE_DIR_ENV.to_string(),
            dir.join("page").display().to_string(),
        ),
        (SESSION_IDLE_ENV.to_string(), "600".to_string()),
        (SESSION_MAX_ENV.to_string(), "3600".to_string()),
        (MODE_ENV.to_string(), "door-only".to_string()),
    ]);
    let cfg = ConsoleConfig::from_vars(&|k| v.get(k).cloned()).expect("config");
    let console = Console::boot(
        cfg,
        BootOptions {
            clock: Clock::manual(1_800_000_000),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
    .expect("boot");
    World {
        router: console.router(),
        dir,
        _console: console,
    }
}

impl World {
    async fn send(&self, req: Request<Body>) -> Answer {
        let res = self.router.clone().oneshot(req).await.expect("answer");
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        Answer {
            status,
            headers,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    async fn get(&self, path: &str) -> Answer {
        self.send(Request::get(path).body(Body::empty()).unwrap())
            .await
    }

    /// A POST with the given Origin (None sends no Origin header).
    async fn post(&self, path: &str, origin: Option<&str>, body: Value) -> Answer {
        let mut req = Request::post(path).header("content-type", "application/json");
        if let Some(o) = origin {
            req = req.header("origin", o);
        }
        self.send(req.body(Body::from(body.to_string())).unwrap())
            .await
    }

    fn setup_code(&self) -> String {
        std::fs::read_to_string(self.dir.join("setup-code"))
            .expect("the setup code file")
            .trim()
            .to_string()
    }
}

fn assert_page_headers(h: &HeaderMap) {
    let csp = h["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("default-src 'self'"), "{csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(h["x-frame-options"], "DENY");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["referrer-policy"], "no-referrer");
    assert_eq!(h["cache-control"], "no-store");
}

async fn wrong_setup_code_is_refused(name: &str) {
    let w = boot(name).await;
    let a = w
        .post(
            "/d/main/api/setup/begin",
            Some(DEFAULT_ORIGIN),
            json!({ "code": WRONG_CODE }),
        )
        .await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{}", a.text);
    assert_eq!(a.error(), "code_refused");
}

async fn traversal_is_a_404(name: &str, paths: &[&str]) {
    let w = boot(name).await;
    for p in paths {
        let a = w.get(p).await;
        // The page fallback under the handle answers 200 with the page, never
        // a file; every other path is a 404.
        let page = a.status == StatusCode::OK && a.text == PAGE_INDEX;
        assert!(
            page || a.status == StatusCode::NOT_FOUND,
            "{p}: {}",
            a.status
        );
        assert!(!a.text.contains("root:"), "{p}: served a file");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn FID_CONSOLE_001() {
    let w = boot("f001").await;
    for p in ["session", "owners", "runtime"] {
        let a = w.get(&format!("/d/main/api/{p}")).await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{p}");
        assert_eq!(a.error(), "not_signed_in", "{p}");
    }
    let s = w.get("/d/main/api/status").await;
    assert_eq!(s.status, StatusCode::OK);
    assert_eq!(s.json(), json!({ "deployment": "main", "hasOwner": false }));
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn FID_CONSOLE_002() {
    wrong_setup_code_is_refused("f002").await;
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn FID_CONSOLE_003() {
    let w = boot("f003").await;
    let a = w
        .post(
            "/d/main/api/setup/begin",
            Some(FOREIGN),
            json!({ "code": w.setup_code() }),
        )
        .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.text);
    assert_eq!(a.error(), "origin_refused");
    // The real code is still unused: the foreign call did not spend it.
    let ok = w
        .post(
            "/d/main/api/setup/begin",
            Some(DEFAULT_ORIGIN),
            json!({ "code": w.setup_code() }),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text);
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn FID_CONSOLE_004() {
    traversal_is_a_404(
        "f004",
        &[
            "/assets/../../../../etc/passwd",
            "/assets/%2e%2e/%2e%2e/%2e%2e/etc/passwd",
            "/d/main/api/../../etc/passwd",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn FID_CONSOLE_005() {
    let w = boot("f005").await;
    let a = w.get("/d/main/").await;
    assert_eq!(a.status, StatusCode::OK);
    assert_page_headers(&a.headers);
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_040() {
    wrong_setup_code_is_refused("i040").await;
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_041() {
    let w = boot("i041").await;
    for i in 0..30 {
        let a = w
            .post(
                "/d/main/api/setup/begin",
                Some(DEFAULT_ORIGIN),
                json!({ "code": format!("AAAA-AAAA-AAAA-AAAA-AAAA-{i:04}") }),
            )
            .await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED, "try {i}: {}", a.text);
        assert_eq!(a.error(), "code_refused", "try {i}");
    }
    // No lockout: the real code still works after 30 wrong ones.
    let ok = w
        .post(
            "/d/main/api/setup/begin",
            Some(DEFAULT_ORIGIN),
            json!({ "code": w.setup_code() }),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text);
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_042() {
    let w = boot("i042").await;
    let a = w
        .post(
            "/d/main/api/recovery",
            Some(DEFAULT_ORIGIN),
            json!({ "code": WRONG_CODE }),
        )
        .await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{}", a.text);
    assert_eq!(a.error(), "code_refused");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_043() {
    let w = boot("i043").await;
    let a = w
        .post(
            "/d/main/api/setup/begin",
            Some(FOREIGN),
            json!({ "code": "WRONG" }),
        )
        .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.text);
    assert_eq!(a.error(), "origin_refused");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_044() {
    let w = boot("i044").await;
    let a = w
        .post("/d/main/api/setup/begin", None, json!({ "code": "WRONG" }))
        .await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "{}", a.text);
    assert_eq!(a.error(), "origin_refused");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_045() {
    let w = boot("i045").await;
    let req = Request::builder()
        .method(Method::OPTIONS)
        .uri("/d/main/api/setup/begin")
        .header("origin", FOREIGN)
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();
    let a = w.send(req).await;
    assert_eq!(a.status, StatusCode::METHOD_NOT_ALLOWED, "{}", a.text);
    assert!(
        a.headers.get("access-control-allow-origin").is_none(),
        "no CORS grant"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_046() {
    let w = boot("i046").await;
    let a = w
        .post(
            "/d/main/api/signin/finish",
            Some(DEFAULT_ORIGIN),
            json!({ "id": "x", "response": {} }),
        )
        .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    assert_eq!(a.error(), "bad_request");
    assert!(a.headers.get("set-cookie").is_none(), "no session issued");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_047() {
    let w = boot("i047").await;
    let a = w.get("/d/nonexistent/api/session").await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.text);
    assert_eq!(a.error(), "unknown_deployment");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_048() {
    traversal_is_a_404(
        "i048",
        &["/d/main/../../../../etc/passwd", "/../../../etc/passwd"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_049() {
    let w = boot("i049").await;
    for p in ["session", "door/keys"] {
        let a = w.get(&format!("/d/main/api/{p}")).await;
        assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{p}: {}", a.text);
        assert_eq!(a.error(), "not_signed_in", "{p}");
    }
    // The invite is a write: same refusal, with a good Origin.
    let a = w
        .post("/d/main/api/owners/invite", Some(DEFAULT_ORIGIN), json!({}))
        .await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "{}", a.text);
    assert_eq!(a.error(), "not_signed_in");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(non_snake_case)]
async fn ID_050() {
    let w = boot("i050").await;
    let a = w.get("/d/main/").await;
    assert_eq!(a.status, StatusCode::OK);
    assert_page_headers(&a.headers);
}
