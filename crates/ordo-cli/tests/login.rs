//! `ordo login` end to end (`plan-identity-service.md` 5.5, 5.6, 8 "CLI").
//!
//! The real identity service runs on a loopback port with a manual clock,
//! on its own Postgres database. A fake browser stands in for the person:
//! it opens a browser session with a fresh touch, approves the CLI's
//! challenge at `/cli/approve`, and follows the redirect to the CLI's
//! loopback port, as the service's page does. No real browser runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ordo_cli::operator::Outcome;
use ordo_cli::signin::{self, claims_of, parse_query, Ctx, SessionFile};
use ordo_exec::PoolConfig;
use ordo_identity::clock::Clock;
use ordo_identity_service::config::{
    BASE_PATH_ENV, CLIENT_ORIGINS_ENV, DATABASE_URL_ENV, DATA_DIR_ENV, ORIGIN_ENV, RP_ID_ENV,
};
use ordo_identity_service::people::{self, Actor};
use ordo_identity_service::sessions::{self, NewSession, SessionKind};
use ordo_identity_service::{BootOptions, IdentityConfig, Service};
use serde_json::{json, Value};

const START: u64 = 1_800_000_000;

struct World {
    svc: Service,
    clock: Clock,
    origin: String,
    person: String,
    dir: PathBuf,
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ordo-cli-login-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

async fn world(name: &str) -> World {
    world_at(name, "").await
}

/// A service whose page is reached under `base`, as the console passes it
/// through at `/identity` (`ORDO_IDENTITY_BASE_PATH`). `World::origin` is
/// then the whole address the CLI is given, path included.
async fn world_at(name: &str, base: &str) -> World {
    let c = PoolConfig::from_env();
    let db = format!("ordo_cli_login_{name}");
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    let mut url = format!(
        "host={} port={} user={} dbname={db}",
        c.host, c.port, c.user
    );
    if let Some(p) = &c.password {
        url.push_str(&format!(" password={p}"));
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let origin = format!("http://localhost:{port}");
    let dir = temp(name);
    let mut vars: HashMap<String, String> = HashMap::from([
        (DATABASE_URL_ENV.into(), url),
        (ORIGIN_ENV.into(), origin.clone()),
        (RP_ID_ENV.into(), "localhost".into()),
        (DATA_DIR_ENV.into(), dir.join("svc").display().to_string()),
        (CLIENT_ORIGINS_ENV.into(), "http://localhost:8081".into()),
    ]);
    if !base.is_empty() {
        vars.insert(BASE_PATH_ENV.into(), base.into());
    }
    let origin = format!("{origin}{base}");
    let cfg = IdentityConfig::from_vars(&|k| vars.get(k).cloned()).expect("config");
    let clock = Clock::manual(START);
    let svc = Service::boot(
        cfg,
        BootOptions {
            clock: clock.clone(),
            ..BootOptions::default()
        },
    )
    .await
    .expect("boot");
    let app = svc
        .router()
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let pool = svc.store().pool();
    let tx = pool.begin().await.unwrap();
    let by = Actor {
        id: "boot",
        client: "",
    };
    let p = people::add(&tx, svc.handle(), svc.keys(), "Adam", by, START)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    World {
        svc,
        clock,
        origin,
        person: p.id,
        dir: dir.join("config"),
    }
}

/// What the fake browser does with the link `ordo login` opens.
#[derive(Clone, Copy)]
enum Browser {
    /// Sign in, approve, and come back with the state it was given.
    Approve,
    /// Approve, and come back with another state.
    WrongState,
}

impl World {
    /// The person signs in at the service's page now: a browser session
    /// with a fresh touch.
    async fn browser_session(&self) -> String {
        let pool = self.svc.store().pool();
        let tx = pool.begin().await.unwrap();
        let s = sessions::open(
            &tx,
            self.svc.handle(),
            self.svc.keys(),
            self.svc.lifetimes(),
            NewSession {
                subject: &self.person,
                kind: SessionKind::Browser,
                client: "console",
                touched_at: Some(self.clock.now()),
                must_enrol: false,
            },
            self.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        s.secret
    }

    fn ctx(&self, vars: &[(&str, &str)], browser: Browser) -> (Ctx, Arc<Mutex<Vec<String>>>) {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let opened = Arc::new(Mutex::new(Vec::new()));
        let seen = opened.clone();
        let me = Fake {
            svc: self.svc.clone(),
            clock: self.clock.clone(),
            person: self.person.clone(),
            origin: self.origin.clone(),
        };
        let ctx = Ctx {
            vars: Arc::new(move |k| vars.get(k).cloned()),
            clock: self.clock.clone(),
            config_dir: Some(self.dir.clone()),
            // A script, unless a test says a person is at the terminal.
            interactive: false,
            opener: Arc::new(move |url: &str| {
                seen.lock().unwrap().push(url.to_string());
                let me = Fake {
                    svc: me.svc.clone(),
                    clock: me.clock.clone(),
                    person: me.person.clone(),
                    origin: me.origin.clone(),
                };
                let url = url.to_string();
                tokio::spawn(async move { me.approve(&url, browser).await });
                Ok(())
            }),
        };
        (ctx, opened)
    }

    async fn run(&self, ctx: &Ctx, line: &str) -> Outcome {
        let args: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        let (cmd, rest) = args.split_first().unwrap();
        signin::run_with(ctx, cmd, rest).await
    }
}

struct Fake {
    svc: Service,
    clock: Clock,
    person: String,
    origin: String,
}

impl Fake {
    async fn approve(&self, url: &str, browser: Browser) {
        let w = World {
            svc: self.svc.clone(),
            clock: self.clock.clone(),
            origin: self.origin.clone(),
            person: self.person.clone(),
            dir: PathBuf::new(),
        };
        let (base, query) = url.split_once('?').expect("a query");
        assert_eq!(base, format!("{}/cli", self.origin));
        let q = parse_query(query);
        let cookie = w.browser_session().await;
        let granted = call(
            &self.origin,
            "POST",
            "/cli/approve",
            Some(json!({ "session": cookie, "codeChallenge": q["code_challenge"] })),
        )
        .await;
        let code = granted["code"].as_str().expect("a code").to_string();
        let state = match browser {
            Browser::Approve => q["state"].clone(),
            Browser::WrongState => "someone-elses-state-0000".to_string(),
        };
        let back = format!(
            "http://127.0.0.1:{}/callback?code={code}&state={state}",
            q["port"]
        );
        // A browser asks for a favicon too; the CLI must ignore it.
        let _ = get(&format!("http://127.0.0.1:{}/favicon.ico", q["port"])).await;
        let _ = get(&back).await;
    }
}

async fn send(url: &str, method: &'static str, body: Option<Value>) -> (u16, Vec<u8>) {
    use futures::StreamExt;
    let (base, path) = {
        let rest = url.strip_prefix("http://").unwrap();
        let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
        (format!("http://{hostport}"), format!("/{path}"))
    };
    let endpoint = ordo_blob::http::Endpoint::parse(&base).unwrap();
    let mut headers = vec![];
    let body = body.map(|v| {
        headers.push(("content-type".to_string(), "application/json".to_string()));
        let b = v.to_string().into_bytes();
        let len = b.len() as u64;
        let s: ordo_blob::ByteStream = Box::pin(futures::stream::once(async move { Ok(b) }));
        (len, s)
    });
    let res = ordo_blob::http::send(
        &endpoint,
        ordo_blob::http::Request {
            method,
            path,
            headers,
            body,
        },
    )
    .await
    .expect("send");
    let status = res.status;
    let mut out = Vec::new();
    let mut s = res.body;
    while let Some(c) = s.next().await {
        out.extend_from_slice(&c.unwrap());
    }
    (status, out)
}

async fn get(url: &str) -> u16 {
    send(url, "GET", None).await.0
}

async fn call(origin: &str, method: &'static str, path: &str, body: Option<Value>) -> Value {
    let (status, out) = send(&format!("{origin}{path}"), method, body).await;
    let v: Value = serde_json::from_slice(&out).unwrap_or(Value::Null);
    assert!(status < 400, "{path} answered {status}: {v}");
    v
}

fn stored(dir: &Path, origin: &str) -> Option<signin::Stored> {
    SessionFile::load(dir)
        .unwrap()
        .sessions
        .get(origin)
        .cloned()
}

fn ok(o: &Outcome) -> &str {
    assert_eq!(o.code, 0, "{}", o.report);
    &o.report
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_whoami_refresh_touch_machine_key_and_logout() {
    let w = world("flow").await;
    let (ctx, opened) = w.ctx(&[], Browser::Approve);

    // Sign in through the fake browser.
    let out = w.run(&ctx, &format!("login {}", w.origin)).await;
    let report = ok(&out).to_string();
    assert!(report.contains(&w.person), "{report}");
    let link = opened.lock().unwrap()[0].clone();
    assert!(link.contains("code_challenge="), "{link}");

    // The file: 0600 in a 0700 folder, and its secrets never printed.
    let s = stored(&w.dir, &w.origin).expect("a stored session");
    assert!(!report.contains(&s.session));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&w.dir.join(signin::SESSIONS_FILE)), 0o600);
        assert_eq!(mode(&w.dir), 0o700);
    }
    let first = s.token.clone().expect("a token from login");

    // whoami: a person, through the session, never the secret.
    let out = w.run(&ctx, "whoami").await;
    let r = ok(&out);
    assert!(r.contains(&w.person) && r.contains("person"), "{r}");
    assert!(r.contains("ordo-deployment:main"), "{r}");
    assert!(!r.contains(&s.session) && !r.contains(&first), "{r}");

    // Within the token's life the stored token is reused.
    w.clock.advance(300);
    let c = signin::credential(&ctx, None, None).await.unwrap();
    assert_eq!(c.token, first);

    // Past it, a new one comes from the session.
    w.clock.advance(301);
    let c = signin::credential(&ctx, None, None).await.unwrap();
    assert_ne!(c.token, first, "an expired token is asked for again");
    assert_eq!(claims_of(&c.token)["exp"], json!(w.clock.now() + 600));
    assert_eq!(
        stored(&w.dir, &w.origin).unwrap().token,
        Some(c.token.clone())
    );

    // A write with a stale touch says to touch.
    let out = w.run(&ctx, "people machine-key add --name ci-main").await;
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("ordo login --touch"), "{}", out.report);

    // `--touch`: the same session, a fresh touch.
    let out = w.run(&ctx, "login --touch").await;
    ok(&out);
    let after = stored(&w.dir, &w.origin).unwrap();
    assert_eq!(after.session, s.session, "a touch opens no new session");
    let out = w.run(&ctx, "people machine-key add --name ci-main").await;
    let r = ok(&out).to_string();
    let secret = r
        .split_whitespace()
        .find(|t| t.starts_with("ordo-mk-v1:"))
        .expect("the key's secret, shown once")
        .to_string();
    let out = w.run(&ctx, "people machine-key list").await;
    assert!(ok(&out).contains("ci-main"));
    let out = w.run(&ctx, "people list").await;
    assert!(ok(&out).contains("Adam"));

    // A machine key signs in with no browser and no session file.
    let empty = temp("flow-ci");
    let (mut ci, ci_opened) = w.ctx(
        &[
            ("ORDO_MACHINE_KEY", &secret),
            ("ORDO_IDENTITY_URL", &w.origin),
        ],
        Browser::Approve,
    );
    ci.config_dir = Some(empty.clone());
    let out = w.run(&ci, "whoami").await;
    let r = ok(&out).to_string();
    assert!(r.contains("machine") && r.contains("m_"), "{r}");
    assert!(ci_opened.lock().unwrap().is_empty(), "no browser for CI");
    assert!(!empty.join(signin::SESSIONS_FILE).exists());

    // From a file, too.
    let key_file = empty.join("key");
    std::fs::write(&key_file, format!("{secret}\n")).unwrap();
    let kf = key_file.display().to_string();
    let (mut ci2, _) = w.ctx(
        &[
            ("ORDO_MACHINE_KEY_FILE", &kf),
            ("ORDO_IDENTITY_URL", &w.origin),
        ],
        Browser::Approve,
    );
    ci2.config_dir = Some(empty.clone());
    let machine = signin::credential(&ci2, None, None).await.unwrap();
    assert_eq!(claims_of(&machine.token)["kind"], "machine");

    // ORDO_TOKEN wins over the session when given.
    let (with_raw, _) = w.ctx(&[("ORDO_TOKEN", &machine.token)], Browser::Approve);
    let c = signin::credential(&with_raw, None, None).await.unwrap();
    assert_eq!(c.token, machine.token);
    assert_eq!(c.source, signin::Source::TokenEnv);

    // logout ends the session at the service and deletes it here.
    let secret_before = stored(&w.dir, &w.origin).unwrap().session;
    let out = w.run(&ctx, "logout").await;
    ok(&out);
    assert!(stored(&w.dir, &w.origin).is_none());
    let (status, _) = send(
        &format!("{}/token", w.origin),
        "POST",
        Some(json!({ "session": secret_before, "aud": "ordo-deployment:main" })),
    )
    .await;
    assert_eq!(status, 401, "the service ended the session");
    let out = w.run(&ctx, "whoami").await;
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("ordo login"), "{}", out.report);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_callback_with_another_state_is_refused_and_nothing_is_stored() {
    let w = world("state").await;
    let (ctx, _) = w.ctx(&[], Browser::WrongState);
    let out = w
        .run(&ctx, &format!("login {} --wait-secs 20", w.origin))
        .await;
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("state"), "{}", out.report);
    assert!(out.report.contains("`ordo login` again"), "{}", out.report);
    assert!(stored(&w.dir, &w.origin).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_deployment_is_refused_at_login_and_names_the_setting() {
    let w = world("aud").await;
    let (ctx, _) = w.ctx(&[], Browser::Approve);
    let out = w
        .run(&ctx, &format!("login {} --deployment other", w.origin))
        .await;
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("wrong_audience"), "{}", out.report);
    assert!(
        out.report.contains("ORDO_IDENTITY_DEPLOYMENT"),
        "{}",
        out.report
    );
    assert!(stored(&w.dir, &w.origin).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn at_a_terminal_a_stale_touch_runs_the_touch_flow_and_retries_once() {
    let w = world("terminal").await;
    let (mut ctx, opened) = w.ctx(&[], Browser::Approve);
    ok(&w.run(&ctx, &format!("login {}", w.origin)).await);
    w.clock.advance(600);

    // A script: the refusal and its hint, no browser.
    let out = w.run(&ctx, "people machine-key add --name ci-a").await;
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("ordo login --touch"), "{}", out.report);
    assert_eq!(
        opened.lock().unwrap().len(),
        1,
        "only the login opened a browser"
    );

    // A person at a terminal: the touch runs, and the same call succeeds.
    ctx.interactive = true;
    let out = w.run(&ctx, "people machine-key add --name ci-b").await;
    let r = ok(&out);
    assert!(r.contains("ordo-mk-v1:"), "{r}");
    assert_eq!(
        opened.lock().unwrap().len(),
        2,
        "the touch opened the browser"
    );
    let out = w.run(&ctx, "people machine-key list").await;
    let r = ok(&out);
    assert!(r.contains("ci-b") && !r.contains("ci-a"), "{r}");

    // A machine key is never touched, at a terminal or not.
    let secret = {
        let out = w.run(&ctx, "people machine-key add --name ci-c").await;
        ok(&out)
            .split_whitespace()
            .find(|t| t.starts_with("ordo-mk-v1:"))
            .unwrap()
            .to_string()
    };
    let (mut ci, _) = w.ctx(
        &[
            ("ORDO_MACHINE_KEY", &secret),
            ("ORDO_IDENTITY_URL", &w.origin),
        ],
        Browser::Approve,
    );
    ci.interactive = true;
    assert!(!signin::touch_if_interactive(&ci, None, None).await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_session_and_no_key_says_how_to_sign_in() {
    let w = world("none").await;
    let (ctx, _) = w.ctx(&[], Browser::Approve);
    let out = w.run(&ctx, "whoami").await;
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("ordo login"), "{}", out.report);
    assert!(out.report.contains("ORDO_MACHINE_KEY"), "{}", out.report);
    let (ci, _) = w.ctx(
        &[("ORDO_MACHINE_KEY", "ordo-mk-v1:m_x:bad")],
        Browser::Approve,
    );
    let out = w.run(&ci, "whoami").await;
    assert_eq!(out.code, 2, "{}", out.report);
    assert!(
        out.report.contains("setting: ORDO_MACHINE_KEY"),
        "{}",
        out.report
    );
}

/// The default one image serves the identity service under the console's
/// origin at `/identity`, with no port of its own. `ordo login` given that
/// address signs in, and later commands find the session by it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_under_the_consoles_identity_path() {
    let w = world_at("basepath", "/identity").await;
    assert!(w.origin.ends_with("/identity"), "{}", w.origin);
    let (ctx, opened) = w.ctx(&[], Browser::Approve);
    let out = w.run(&ctx, &format!("login {}", w.origin)).await;
    let report = ok(&out).to_string();
    assert!(report.contains(&w.person), "{report}");
    let link = opened.lock().unwrap()[0].clone();
    assert!(
        link.starts_with(&format!("{}/cli?", w.origin)),
        "the approval page is under the path: {link}"
    );
    assert!(
        stored(&w.dir, &w.origin).is_some(),
        "the session is kept under the whole address"
    );
    let out = w.run(&ctx, "whoami").await;
    assert!(ok(&out).contains(&w.person));
    let out = w.run(&ctx, &format!("logout {}", w.origin)).await;
    ok(&out);
    assert!(stored(&w.dir, &w.origin).is_none());
}
