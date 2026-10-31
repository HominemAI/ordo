//! The console on the supervisor's socket (`plan-identity-service.md` 3.2,
//! step 7): the Runtime screen, apply, rollback and restart are calls on
//! the socket, the history stays in the console's database, and at the
//! upgrade the console's own live revision is carried over to the
//! supervisor, so the deployment runs what it ran before.
//!
//! The supervisor is a stand-in that speaks the real protocol
//! (`support/fake_supervisor.rs`). Needs the conformance cluster for the
//! console's own database.

mod support {
    pub mod fake_supervisor;
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use ordo_console::config::{
    DATABASE_URL_ENV, DATA_DIR_ENV, ORIGIN_ENV, PAGE_DIR_ENV, SUPERVISOR_SOCKET_ENV,
};
use ordo_console::runtime::spec::{SecretRef, Spec};
use ordo_console::runtime::{Synced, CARRY_OVER_ACTION};
use ordo_console::sealing::ConsoleKey;
use ordo_console::store::Store;
use ordo_console::{BootOptions, Clock, Console, ConsoleConfig, DatabaseSettings};
use ordo_exec::PoolConfig;
use ordo_identity::software_authenticator::{TestAuthenticator, DEFAULT_ORIGIN};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{Argon2idHasher, CompleteRequest};
use serde_json::{json, Value};
use support::fake_supervisor::{value_of, FakeSupervisor};
use tower::ServiceExt;

const START: u64 = 1_800_000_000;

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

struct Place {
    db: String,
    dir: PathBuf,
}

async fn place(name: &str) -> Place {
    let db = format!("ordo_console_t_runtime_{name}");
    let c = PoolConfig::from_env();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let dir = std::env::temp_dir().join(format!(
        "ordo-console-runtime-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("page")).expect("page dir");
    std::fs::write(dir.join("page/index.html"), "<div id=\"root\"></div>").expect("index");
    Place { db, dir }
}

fn vars(p: &Place, sup: &FakeSupervisor) -> HashMap<&'static str, String> {
    HashMap::from([
        (DATABASE_URL_ENV, url_for(&p.db)),
        (ORIGIN_ENV, DEFAULT_ORIGIN.to_string()),
        ("ORDO_CONSOLE_RP_ID", "example.com".to_string()),
        (DATA_DIR_ENV, p.dir.display().to_string()),
        (PAGE_DIR_ENV, p.dir.join("page").display().to_string()),
        (SUPERVISOR_SOCKET_ENV, sup.socket.display().to_string()),
    ])
}

async fn boot(p: &Place, sup: &FakeSupervisor) -> H {
    let v = vars(p, sup);
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
        dir: p.dir.clone(),
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

    async fn post(&self, path: &str, body: Value, cookie: Option<&str>) -> (StatusCode, Value) {
        let mut req = Request::post(format!("/d/main/api/{path}"))
            .header("origin", DEFAULT_ORIGIN)
            .header("content-type", "application/json");
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        let (s, b, _) = self
            .send(req.body(Body::from(body.to_string())).unwrap())
            .await;
        (s, b)
    }

    async fn get(&self, path: &str, cookie: &str) -> (StatusCode, Value) {
        let req = Request::get(format!("/d/main/api/{path}")).header("cookie", cookie);
        let (s, b, _) = self.send(req.body(Body::empty()).unwrap()).await;
        (s, b)
    }

    /// Enrol the first owner with the setup code; the session cookie, with
    /// a fresh touch (the clock stands still).
    async fn owner(&self) -> String {
        let code = std::fs::read_to_string(self.dir.join("setup-code")).expect("setup code");
        let code = code.trim();
        let req = Request::post("/d/main/api/setup/begin")
            .header("origin", DEFAULT_ORIGIN)
            .header("content-type", "application/json")
            .body(Body::from(json!({ "code": code }).to_string()))
            .unwrap();
        let (s, begun, _) = self.send(req).await;
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
        let req = Request::post("/d/main/api/setup/finish")
            .header("origin", DEFAULT_ORIGIN)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (s, done, cookie) = self.send(req).await;
        assert_eq!(s, StatusCode::OK, "{done}");
        cookie.expect("a session cookie")
    }
}

/// Apply, refuse, roll back and restart through the socket. The console
/// files each revision, hands the supervisor the whole spec, keeps a secret
/// it has no value for, sends one a person typed as its value, and records
/// how the supervisor answered.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_manages_the_deployment_through_the_socket() {
    let p = place("manage").await;
    let sup = FakeSupervisor::start(
        "manage",
        &[
            ("ORDO_PG_HOST", "db"),
            ("ORDO_TOKEN_KEY", "the-token-key"),
            ("ORDO_PG_PASSWORD_FILE", "/run/secrets/pg"),
        ],
        None,
    )
    .await;
    let h = boot(&p, &sup).await;
    assert!(h.console.runtime().unwrap().synced());
    let cookie = h.owner().await;

    // Revision 1, built by the supervisor, is filed in the history.
    let (s, revs) = h.get("runtime/revisions", &cookie).await;
    assert_eq!(s, StatusCode::OK, "{revs}");
    assert_eq!(revs["live"], 1, "{revs}");
    assert_eq!(revs["revisions"][0]["number"], 1, "{revs}");
    assert_eq!(revs["revisions"][0]["outcome"], "good", "{revs}");
    assert!(!revs.to_string().contains("the-token-key"), "{revs}");

    let (s, rt) = h.get("runtime", &cookie).await;
    assert_eq!(s, StatusCode::OK, "{rt}");
    assert_eq!(rt["mode"], "supervise");
    assert_eq!(rt["current"]["revision"], 1);
    assert_eq!(rt["status"]["state"], "ready");
    assert_eq!(rt["children"]["identity"]["state"], "ready", "{rt}");
    assert!(!rt.to_string().contains("the-token-key"), "{rt}");

    // A setting and a new secret: revision 2. The token key the console
    // never held is kept; the file stays a file; the typed value goes.
    let (s, applied) = h
        .post(
            "runtime/apply",
            json!({ "set": { "ORDO_WORKERS": "4" },
                    "secrets": { "ORDO_PAGE_KEY": { "value": "a-page-key" } },
                    "reason": "more workers" }),
            Some(&cookie),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{applied}");
    assert_eq!(applied["revision"], 2, "{applied}");
    let sent = sup.applied();
    let req = &sent[0];
    assert_eq!(req.revision, 2);
    assert_eq!(req.settings["ORDO_WORKERS"], "4");
    assert_eq!(req.settings["ORDO_PG_HOST"], "db");
    assert!(req.secrets["ORDO_TOKEN_KEY"].keep);
    assert_eq!(
        req.secrets["ORDO_PG_PASSWORD"].file.as_deref(),
        Some("/run/secrets/pg")
    );
    assert_eq!(
        req.secrets["ORDO_PAGE_KEY"].value.as_deref(),
        Some("a-page-key")
    );
    let live = sup.live();
    assert_eq!(live.number, 2);
    assert_eq!(
        value_of(&live, "ORDO_TOKEN_KEY").as_deref(),
        Some("the-token-key")
    );
    // The value is sealed in the console's database, never in a row.
    let rows = h
        .console
        .store()
        .pool()
        .rows("SELECT settings::text || secret_refs::text FROM revisions")
        .await
        .unwrap();
    assert!(!format!("{rows:?}").contains("a-page-key"));

    // check-config refuses: the row says so, and the live one stays.
    let (s, refused) = h
        .post(
            "runtime/apply",
            json!({ "set": { "ORDO_FAKE_CHECK": "refuse" } }),
            Some(&cookie),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"], "settings_refused", "{refused}");
    let row3 = h
        .console
        .store()
        .revision("main", 3)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row3.outcome.as_deref(), Some("refused"));
    assert_eq!(sup.live().number, 2);

    // A name the supervisor would refuse never files a row.
    let (s, bad) = h
        .post(
            "runtime/apply",
            json!({ "set": { "LD_PRELOAD": "/tmp/x.so" } }),
            Some(&cookie),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
    assert!(h
        .console
        .store()
        .revision("main", 4)
        .await
        .unwrap()
        .is_none());

    // Roll back to 1: the page key goes, the token key is kept.
    let (s, back) = h
        .post("runtime/rollback", json!({ "revision": 1 }), Some(&cookie))
        .await;
    assert_eq!(s, StatusCode::OK, "{back}");
    assert_eq!(back["revision"], 4, "{back}");
    let req = sup.applied().last().cloned().unwrap();
    assert!(!req.secrets.contains_key("ORDO_PAGE_KEY"));
    assert!(req.secrets["ORDO_TOKEN_KEY"].keep);
    assert!(!req.settings.contains_key("ORDO_WORKERS"));

    let (s, _) = h.post("runtime/restart", json!({}), Some(&cookie)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(sup.state.lock().unwrap().restarts, 1);
    let _ = std::fs::remove_dir_all(&p.dir);
}

/// The upgrade (owner: an upgrade affects nothing): the console ran the
/// deployment itself and its live revision is 2. The new supervisor starts
/// on an empty volume with revision 1 from the container. At its first
/// boot on the socket the console applies its revision 2, with the secret
/// values it sealed, a file as a file, and a value it cannot open kept as
/// the supervisor holds it. Audited; a second boot changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_upgrade_carries_the_consoles_live_revision_over() {
    let p = place("carry").await;
    // The console's database as the console before step 7 left it.
    let db = DatabaseSettings {
        pool: ordo_exec::dburl::parse_database_url(&url_for(&p.db), 2).unwrap(),
    };
    let store = Store::open(&db).await.unwrap();
    store.migrate(START).await.unwrap();
    store
        .ensure_deployment("main", None, "supervise", START)
        .await
        .unwrap();
    let key = ConsoleKey::load_or_create(&p.dir.join("console.key")).unwrap();
    let token = store
        .put_secret(
            "main",
            "ORDO_TOKEN_KEY",
            &key.seal("main", "ORDO_TOKEN_KEY", b"the-new-token-key"),
            START,
        )
        .await
        .unwrap();
    // Sealed with a key this console no longer has.
    let lost = ConsoleKey::load_or_create(&p.dir.join("other.key")).unwrap();
    let page = store
        .put_secret(
            "main",
            "ORDO_PAGE_KEY",
            &lost.seal("main", "ORDO_PAGE_KEY", b"unreadable"),
            START,
        )
        .await
        .unwrap();
    let mut one = Spec::default();
    one.settings.insert("ORDO_PG_HOST".into(), "db".into());
    let n1 = store
        .insert_revision(
            "main",
            &one.settings_json(),
            &one.secrets_json(),
            "system",
            "",
            START,
        )
        .await
        .unwrap();
    store
        .decide_revision("main", n1, "good", "", START)
        .await
        .unwrap();
    let mut two = one.clone();
    two.settings.insert("ORDO_PG_HOST".into(), "db2".into());
    two.settings.insert("ORDO_WORKERS".into(), "8".into());
    for (name, version) in [("ORDO_TOKEN_KEY", token), ("ORDO_PAGE_KEY", page)] {
        two.secrets.insert(
            name.into(),
            SecretRef::Managed {
                version,
                changed_at: START,
            },
        );
    }
    two.secrets.insert(
        "ORDO_PG_PASSWORD".into(),
        SecretRef::Referenced {
            path: "/run/secrets/pg".into(),
            changed_at: START,
        },
    );
    let n2 = store
        .insert_revision(
            "main",
            &two.settings_json(),
            &two.secrets_json(),
            "owner:o1",
            "",
            START,
        )
        .await
        .unwrap();
    store
        .decide_revision("main", n2, "good", "", START)
        .await
        .unwrap();
    store.set_live_revision("main", n2).await.unwrap();
    drop(store);

    let sup = FakeSupervisor::start(
        "carry",
        &[
            ("ORDO_PG_HOST", "db"),
            ("ORDO_TOKEN_KEY", "the-old-token-key"),
            ("ORDO_PAGE_KEY", "the-page-key"),
            ("ORDO_PG_PASSWORD_FILE", "/run/secrets/pg"),
        ],
        None,
    )
    .await;
    let h = boot(&p, &sup).await;
    let sent = sup.applied();
    assert_eq!(sent.len(), 1, "{sent:?}");
    let req = &sent[0];
    assert_eq!((req.revision, req.actor.as_str()), (2, "system"));
    assert!(req.reason.contains("carried over"), "{}", req.reason);
    assert_eq!(req.settings["ORDO_PG_HOST"], "db2");
    assert_eq!(req.settings["ORDO_WORKERS"], "8");
    assert_eq!(
        req.secrets["ORDO_TOKEN_KEY"].value.as_deref(),
        Some("the-new-token-key")
    );
    assert!(req.secrets["ORDO_PAGE_KEY"].keep);
    assert_eq!(
        req.secrets["ORDO_PG_PASSWORD"].file.as_deref(),
        Some("/run/secrets/pg")
    );
    let live = sup.live();
    assert_eq!(live.number, 2);
    assert_eq!(
        value_of(&live, "ORDO_PAGE_KEY").as_deref(),
        Some("the-page-key")
    );

    let store = h.console.store();
    assert_eq!(store.live_revision("main").await.unwrap(), Some(2));
    let audit = store
        .audit_page("main", None, Some(CARRY_OVER_ACTION), 10)
        .await
        .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(
        (audit[0].outcome.as_str(), audit[0].detail.as_str()),
        ("ok", "revision=2")
    );

    // A second boot finds them in step and applies nothing.
    let again = h.console.runtime().unwrap().sync().await.unwrap();
    assert_eq!(again, Synced::InStep { live: 2 });
    drop(h);
    let h2 = boot(&p, &sup).await;
    assert_eq!(sup.applied().len(), 1);
    assert!(h2.console.runtime().unwrap().synced());
    let _ = std::fs::remove_dir_all(&p.dir);
}

async fn page_of(h: &H) -> String {
    let res = h
        .router
        .clone()
        .oneshot(Request::get("/d/main/").body(Body::empty()).unwrap())
        .await
        .expect("answer");
    assert_eq!(res.status(), StatusCode::OK);
    let b = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    String::from_utf8(b.to_vec()).expect("utf-8")
}

async fn status_of(h: &H) -> Value {
    let req = Request::get("/d/main/api/status")
        .body(Body::empty())
        .unwrap();
    h.send(req).await.1
}

/// Step 8 (owner, 2026-10-30). Under the supervisor with no identity
/// service, the console keeps its old sign-in exactly as before, and every
/// page and the status carry a banner naming the 2 settings.
#[tokio::test(flavor = "multi_thread")]
async fn without_the_identity_service_the_old_sign_in_stays_with_a_banner() {
    use ordo_console::server::IDENTITY_OFF_BANNER;
    for setting in [
        "ORDO_IDENTITY_DATABASE_URL",
        "ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true",
    ] {
        assert!(IDENTITY_OFF_BANNER.contains(setting), "{setting}");
    }
    let p = place("banner_old").await;
    let sup = FakeSupervisor::start("banner-old", &[("ORDO_PG_HOST", "db")], None).await;
    let h = boot(&p, &sup).await;
    let html = page_of(&h).await;
    assert!(
        html.starts_with("<p class=\"note\" role=\"note\" aria-label=\"Sign-in notice\" id=\"ordo-console-notice\">"),
        "{html}"
    );
    assert!(
        html.contains("ORDO_IDENTITY_SHARE_CONSOLE_DB_SERVER=true"),
        "{html}"
    );
    assert!(html.ends_with("<div id=\"root\"></div>"), "{html}");
    let s = status_of(&h).await;
    assert_eq!(s["notice"], IDENTITY_OFF_BANNER, "{s}");
    assert_eq!(s["hasOwner"], false, "{s}");
    // The setup code is printed and signs the first owner in, as before.
    let cookie = h.owner().await;
    let (st, who) = h.get("session", &cookie).await;
    assert_eq!(st, StatusCode::OK, "{who}");
    assert_eq!(status_of(&h).await["hasOwner"], true);
    let _ = std::fs::remove_dir_all(&p.dir);
}

/// Step 8: under the supervisor with the identity service, the old
/// sign-in pieces are gone (no setup code, its routes answer that they
/// moved, naming the setting) and there is no banner.
#[tokio::test(flavor = "multi_thread")]
async fn with_the_identity_service_the_old_sign_in_is_gone_and_there_is_no_banner() {
    let p = place("banner_new").await;
    let sup = FakeSupervisor::start("banner-new", &[("ORDO_PG_HOST", "db")], None).await;
    let mut v = vars(&p, &sup);
    // Nothing answers here; the console boots anyway and refuses tokens
    // until the service is up.
    v.insert("ORDO_IDENTITY_URL", "http://127.0.0.1:9".to_string());
    v.insert("ORDO_IDENTITY_ORIGIN", DEFAULT_ORIGIN.to_string());
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
    let h = H {
        router: console.router(),
        console,
        dir: p.dir.clone(),
    };
    assert_eq!(page_of(&h).await, "<div id=\"root\"></div>");
    let s = status_of(&h).await;
    assert!(s.get("notice").is_none(), "{s}");
    assert!(s.get("identity").is_some(), "{s}");
    assert!(!h.console.setup_code_path().exists());
    let (st, b) = h.post("signin/begin", json!({}), None).await;
    assert_eq!(st, StatusCode::GONE, "{b}");
    assert_eq!(b["error"], "moved_to_identity_service", "{b}");
    assert_eq!(b["setting"], "ORDO_IDENTITY_URL", "{b}");
    let _ = std::fs::remove_dir_all(&p.dir);
}
