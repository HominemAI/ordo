//! Door-only mode (`plan-identity-service.md` 3.2, step 7): a console
//! without `ORDO_CONSOLE_SUPERVISOR_SOCKET` has no runtime routes. Its
//! Runtime screen is read-only, what the deployment reports through its
//! door, and every runtime write is refused, naming the setting.
//!
//! The console binary starts no process in either mode: the supervisor
//! (`ordo-supervisor`) runs the deployment. The scans below hold it to
//! that.
//!
//! The Runtime cases need the conformance cluster (`ORDO_PG_*`); each has
//! its own database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use ordo_console::config::{
    DATABASE_URL_ENV, DATA_DIR_ENV, DOOR_URL_ENV, MODE_ENV, ORIGIN_ENV, PAGE_DIR_ENV,
    SUPERVISOR_SOCKET_ENV,
};
use ordo_console::{BootOptions, Clock, Console, ConsoleConfig, Mode};
use ordo_exec::PoolConfig;
use ordo_identity::software_authenticator::{TestAuthenticator, DEFAULT_ORIGIN};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{Argon2idHasher, CompleteRequest};
use serde_json::{json, Value};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;

/// Strings only a spawn path holds: the supervisor's launcher argument and
/// its log prefixes. The console's binary carries none.
const SPAWN_PATH_MARKERS: &[&str] = &[
    "--supervised-child",
    "ordo-console launcher: ",
    "ordo-supervisor launcher: ",
];

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn the_console_binary_holds_no_spawn_path() {
    let bin = std::fs::read(env!("CARGO_BIN_EXE_ordo-console")).expect("the ordo-console binary");
    for m in SPAWN_PATH_MARKERS {
        assert!(!contains(&bin, m), "`{m}` is in the console's binary");
    }
}

/// No source file of the console can start a process.
#[test]
fn no_console_source_can_start_a_process() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    assert!(files.len() > 10, "found only {} files", files.len());
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for p in [
            "Command::new",
            "tokio::process",
            "process::Command",
            "fork(",
            "execv",
        ] {
            assert!(
                !text.contains(p),
                "{} holds `{p}`: the console starts no process",
                f.display()
            );
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn base_vars() -> HashMap<&'static str, String> {
    HashMap::from([
        (
            DATABASE_URL_ENV,
            "host=/tmp user=console dbname=c".to_string(),
        ),
        (ORIGIN_ENV, "https://console.example.com".to_string()),
    ])
}

/// The mode follows the socket setting alone. `ORDO_CONSOLE_MODE`, which
/// an old compose file may still set, is no longer read and refuses
/// nothing.
#[test]
fn the_mode_follows_the_socket_setting() {
    let v = base_vars();
    let cfg = ConsoleConfig::from_vars(&|k| v.get(k).cloned()).expect("config");
    assert_eq!(cfg.mode, Mode::DoorOnly);
    assert!(cfg.runtime.socket.is_none());

    let mut v = base_vars();
    v.insert(SUPERVISOR_SOCKET_ENV, "/run/ordo/supervisor.sock".into());
    let cfg = ConsoleConfig::from_vars(&|k| v.get(k).cloned()).expect("supervise");
    assert_eq!(cfg.mode, Mode::Supervise);

    for old in ["supervise", "door-only", "anything"] {
        let mut v = base_vars();
        v.insert(MODE_ENV, old.into());
        let cfg = ConsoleConfig::from_vars(&|k| v.get(k).cloned()).expect("an old mode boots");
        assert_eq!(cfg.mode, Mode::DoorOnly, "{old}");
    }
}

// ── the Runtime screen in door-only mode ─────────────────────────────────

fn url_for(db: &str) -> String {
    let c = PoolConfig::from_env();
    let mut url = format!(
        "host={} port={} user={} dbname={db}",
        c.host, c.port, c.user
    );
    if let Some(p) = &c.password {
        url.push_str(&format!(" password={p}"));
    }
    url
}

struct H {
    console: Console,
    router: Router,
    dir: PathBuf,
}

async fn boot(name: &str, door: Option<String>) -> H {
    let db = format!("ordo_console_t_dooronly_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir = std::env::temp_dir().join(format!(
        "ordo-console-dooronly-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("page")).expect("page dir");
    std::fs::write(dir.join("page/index.html"), "<div id=\"root\"></div>").expect("index");
    let mut v: HashMap<&str, String> = HashMap::from([
        (DATABASE_URL_ENV, url_for(&db)),
        (ORIGIN_ENV, DEFAULT_ORIGIN.to_string()),
        ("ORDO_CONSOLE_RP_ID", "example.com".to_string()),
        (DATA_DIR_ENV, dir.display().to_string()),
        (PAGE_DIR_ENV, dir.join("page").display().to_string()),
    ]);
    if let Some(d) = door {
        v.insert(DOOR_URL_ENV, d);
    }
    let cfg = ConsoleConfig::from_vars(&|k| v.get(k).cloned()).expect("config");
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
    }
}

impl H {
    async fn send(&self, req: Request<Body>) -> (StatusCode, Value, Option<String>) {
        let res = self.router.clone().oneshot(req).await.expect("answer");
        let status = res.status();
        let cookie = res
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::to_string);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            cookie,
        )
    }

    async fn post(
        &self,
        path: &str,
        body: Value,
        cookie: Option<&str>,
    ) -> (StatusCode, Value, Option<String>) {
        let mut req = Request::post(format!("/d/main/api/{path}"))
            .header("origin", DEFAULT_ORIGIN)
            .header("content-type", "application/json");
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        self.send(req.body(Body::from(body.to_string())).unwrap())
            .await
    }

    async fn get(&self, path: &str, cookie: &str) -> (StatusCode, Value) {
        let req = Request::get(path).header("cookie", cookie);
        let (s, b, _) = self.send(req.body(Body::empty()).unwrap()).await;
        (s, b)
    }

    /// Enrol the first owner with the setup code; the session cookie.
    async fn owner(&self) -> String {
        let code = std::fs::read_to_string(self.dir.join("setup-code")).expect("setup code");
        let code = code.trim();
        let (s, begun, _) = self
            .post("setup/begin", json!({ "code": code }), None)
            .await;
        assert_eq!(s, StatusCode::OK, "{begun}");
        let challenge = begun["publicKey"]["challenge"]
            .as_str()
            .expect("challenge")
            .to_string();
        let CompleteRequest::PasskeyRegistration {
            client_data_json,
            attestation_object,
            ..
        } = TestAuthenticator::es256().register(&challenge)
        else {
            unreachable!()
        };
        let body = json!({
            "code": code,
            "challenge": challenge,
            "clientDataJSON": b64url_encode(&client_data_json),
            "attestationObject": b64url_encode(&attestation_object),
        });
        let (s, done, cookie) = self.post("setup/finish", body, None).await;
        assert_eq!(s, StatusCode::OK, "{done}");
        cookie.expect("a session cookie")
    }
}

/// A door that answers `/health/ready`, as a deployment the console does
/// not run would.
async fn stub_door() -> String {
    let app = Router::new().route(
        "/health/ready",
        get(|| async { Json(json!({ "status": "ready", "program_active": false })) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// A loopback address with nothing listening.
async fn silent_door() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_runtime_screen_shows_what_the_deployment_reports_and_changes_nothing() {
    let h = boot("reports", Some(stub_door().await)).await;
    assert!(h.console.runtime().is_none());
    let cookie = h.owner().await;

    let (s, ready) = h.get("/health/ready", &cookie).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ready["deployment"], "door-only");

    let (s, rt) = h.get("/d/main/api/runtime", &cookie).await;
    assert_eq!(s, StatusCode::OK, "{rt}");
    assert_eq!(rt["mode"], "door-only");
    assert_eq!(rt["answering"], true, "{rt}");
    assert_eq!(rt["reported"]["ready"]["status"], "ready", "{rt}");
    // Deep health needs the console linked to the deployment; unlinked, it
    // says why and the rest still shows.
    assert!(rt["reported"]["deep"]["error"].is_string(), "{rt}");
    assert!(rt["reported"]["boot"].is_null(), "{rt}");
    assert!(
        rt["message"].as_str().unwrap().contains("door-only"),
        "{rt}"
    );

    // Every runtime write is refused with the setting named, touch or not.
    for (path, body) in [
        (
            "runtime/apply",
            json!({ "set": {}, "unset": [], "secrets": {}, "reason": "x" }),
        ),
        ("runtime/rollback", json!({ "revision": 1 })),
        ("runtime/restart", json!({})),
    ] {
        let (s, refused, _) = h.post(path, body, Some(&cookie)).await;
        assert_ne!(s, StatusCode::OK, "{path} answered {refused}");
        assert!(
            refused["error"] == "door_only" || refused["error"] == "step_up_required",
            "{path}: {refused}"
        );
        if refused["error"] == "door_only" {
            assert_eq!(refused["setting"], SUPERVISOR_SOCKET_ENV, "{refused}");
        }
    }
    let (_, revs) = h.get("/d/main/api/runtime/revisions", &cookie).await;
    assert_eq!(
        revs["revisions"],
        json!([]),
        "door-only mode keeps no revisions"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_door_is_not_answering_and_no_more() {
    let h = boot("silent", Some(silent_door().await)).await;
    let cookie = h.owner().await;
    let (s, rt) = h.get("/d/main/api/runtime", &cookie).await;
    assert_eq!(s, StatusCode::OK, "{rt}");
    assert_eq!(rt["answering"], false, "{rt}");
    assert_eq!(
        rt["reported"]["ready"]["error"], "deployment_not_answering",
        "{rt}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_door_address_the_screen_names_the_setting() {
    let h = boot("nodoor", None).await;
    let cookie = h.owner().await;
    let (s, rt) = h.get("/d/main/api/runtime", &cookie).await;
    assert_eq!(s, StatusCode::OK, "{rt}");
    assert_eq!(rt["answering"], false);
    assert!(
        rt["doorError"].as_str().unwrap().contains(DOOR_URL_ENV),
        "{rt}"
    );
}
