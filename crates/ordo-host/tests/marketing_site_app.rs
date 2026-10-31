//! The marketing site (`examples/marketing-site`) against the real
//! `ordo-host` binary and real Postgres.
//!
//! The site is public (`docs/howto-public.md`): a page with no sign-in reads
//! four public ops, and its contact form is a public write. The test acts as
//! `dev/serve.sh`, `dev/seed` and the page do. The site owner signs in as a
//! guest, is named in `ORDO_DEPLOYMENT_OWNERS` on the next start, and writes
//! the content into the deployment's own space. A visitor then calls the
//! door with no token at all.
//!
//! What it proves:
//!
//! - With no token, `catalogue`, `product`, `pricing` and `reviews` return
//!   the seeded content, readable from any page (`*`).
//! - The contact form writes an enquiry and answers its id alone. A visitor
//!   cannot read enquiries back, and cannot call the owner's writes.
//! - The human check is on unless the deployment turns it off, and its
//!   refusal names the settings.
//! - Proxy mode slows a busy visitor through the real server's socket, and
//!   never refuses.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::KeyProvider as _;
use ordo_exec::{Pool, PoolConfig};
use serde_json::{json, Value};

const SOURCE: &str = include_str!("../../../examples/marketing-site/site.ordo");

/// The deployment's own space, where the public content lives.
const HOME: &str = "system";
const MASTER: &str = "the-marketing-site-test-master-key";
const TOKEN_KEY: &str = "5555555555555555555555555555555555555555555555555555555555555555";
const PAGE_KEY: &str = "6666666666666666666666666666666666666666666666666666666666666666";

/// The page, as it runs locally: Vite's dev server on the example's port.
const PAGE: &str = "http://localhost:5183";

/// Every test boots a whole deployment with its own pools.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    l.local_addr().expect("addr").port()
}

fn drop_database(name: &str) {
    let _ = Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
    .exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

/// A database holding the site program and the deployment's custody, the
/// state `ordo apply` and the first boot leave.
async fn deployment_with_site(db: &str) {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean schema");
    let plain = Fixture::from_source(SOURCE, "").expect("site.ordo compiles");
    pg.exec(&plain.ddl).expect("ddl");
    pg.exec(ordo_crypto::SEALED_PAYLOAD_DDL)
        .expect("sealed payloads");

    let pool = Pool::connect(cfg(db)).await.expect("pool");
    ordo_exec::keystore::ensure_tables(&pool)
        .await
        .expect("custody table");
    let provider = ordo_crypto::LocalKeyProvider::new(
        MASTER.as_bytes(),
        Box::new(ordo_exec::keystore::PgKeyStore::open(cfg(db)).expect("store")),
    );
    provider
        .create_space(&ordo_crypto::SpaceId(HOME.into()))
        .expect("custody minted");
    let keys: Arc<dyn ordo_crypto::KeyProvider> = Arc::new(provider);

    let mut prog = Fixture::from_source(SOURCE, "").expect("site.ordo compiles");
    prog.keys = keys;
    let prog = prog.in_space(HOME);
    ordo_exec::registry::ensure_tables(&pool)
        .await
        .expect("registry");
    let doc =
        ordo_plan::artifact::program_artifact(&prog.schema, &prog.registry).expect("artifact");
    let ddl = prog.ddl.clone();
    let art = ordo_exec::registry::Artifact {
        version: ordo_plan::artifact::program_version(&doc, &ddl),
        artifact: ordo_plan::artifact::to_bytes(&doc),
        source: SOURCE.to_string(),
        ddl,
        applied_at: ordo_commandlog::Timestamp(1),
        actor: "operator:site-test".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:site-test",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
}

/// A running deployment. Killed when dropped; its databases are dropped
/// too unless it is being restarted.
struct Deployment {
    port: u16,
    child: Child,
    db: String,
    log: std::path::PathBuf,
    keep: bool,
}

impl Deployment {
    /// The settings `examples/marketing-site/dev/serve.sh` sets, plus
    /// `extra`. Guest sign-in is on, which is how `dev/seed` becomes the
    /// site owner.
    async fn boot(db: &str, extra: &[(&str, &str)]) -> Deployment {
        deployment_with_site(db).await;
        Deployment::start(db, extra)
    }

    fn start(db: &str, extra: &[(&str, &str)]) -> Deployment {
        let port = free_port();
        let base = PoolConfig::from_env();
        // The server writes to a file, so a full pipe never stops it and a
        // failing test can print what it said.
        let log = std::env::temp_dir().join(format!("ordo-site-{db}-{port}.log"));
        let _ = std::fs::remove_file(&log);
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .expect("log");
        let err = out.try_clone().expect("log");
        let mut c = Command::new(env!("CARGO_BIN_EXE_ordo-host"));
        c.env("ORDO_PG_POOL_SIZE", "6")
            .env("ORDO_PG_HOST", &base.host)
            .env("ORDO_PG_PORT", base.port.to_string())
            .env("ORDO_PG_USER", &base.user)
            .env("ORDO_PG_DB", db)
            .env("ORDO_SPACE", HOME)
            .env("ORDO_KEY_PROVIDER", "local")
            .env("ORDO_MASTER_KEY", MASTER)
            .env("ORDO_DEV_UNSAFE_TEST_KEYS", "1")
            .env("ORDO_TOKEN_KEY", TOKEN_KEY)
            .env("ORDO_PAGE_KEY", PAGE_KEY)
            .env("ORDO_HOST_ADDR", format!("127.0.0.1:{port}"))
            // The README's settings for a local deployment.
            .env("ORDO_AUTH_METHODS", "passkey")
            .env("ORDO_AUTH_GUEST", "on")
            .env("ORDO_APEX_DOMAIN", "localhost")
            .env("ORDO_AUTH_ORIGINS", PAGE)
            .env("ORDO_AUTH_PROVISIONING", "auto-onboard")
            .env("ORDO_PROVISION_SELF_SERVE", "on-signup")
            .env("ORDO_CORS_ORIGINS", PAGE)
            .env_remove("ORDO_PG_ROUTING")
            .env_remove("ORDO_TENANT_CREDENTIALS")
            .env_remove("ORDO_TENANT_CREDENTIALS_DIR")
            .env_remove("ORDO_CHECKPOINT_KEY")
            .env_remove("ORDO_SECURITY_CHECKPOINT_KEY")
            .env_remove("ORDO_EXIT_BUNDLE_DIR")
            .env_remove("ORDO_DEPLOYMENT_OWNERS")
            .env_remove("ORDO_PUBLIC_HUMAN_CHECK")
            .env_remove("ORDO_PUBLIC_HUMAN_SECRET_FILE")
            .env_remove("ORDO_AUTH_HUMAN_CHECK_SECRET_FILE")
            .env_remove("ORDO_PROXY_HEADER")
            .env_remove("ORDO_PROXY_TRUSTED")
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        for (k, v) in extra {
            c.env(k, v);
        }
        let mut child = c.spawn().expect("spawn");
        wait_for_live(port, &mut child, &log);
        Deployment {
            port,
            child,
            db: db.to_string(),
            log,
            keep: false,
        }
    }

    /// Stop and start again on the same database, as a deployment owner who
    /// edits a setting does.
    fn restart(mut self, extra: &[(&str, &str)]) -> Deployment {
        self.keep = true;
        let db = self.db.clone();
        drop(self);
        Deployment::start(&db, extra)
    }

    /// What the server has printed so far.
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// One HTTP/1.1 request over a raw socket: `(status, headers, body)`.
    fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, Value) {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(20))).ok();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        s.write_all(req.as_bytes()).expect("write");
        let mut raw = String::new();
        s.read_to_string(&mut raw).ok();
        let status = raw
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let json = serde_json::from_str(body.trim()).unwrap_or(Value::Null);
        (status, head.to_ascii_lowercase(), json)
    }

    /// A POST to `/auth/...`, as `auth/passkey.ts` sends it.
    fn auth(&self, path: &str, body: Value) -> (u16, Value) {
        let headers = [
            ("Content-Type", "application/json"),
            ("Origin", PAGE),
            ("Accept", "application/json"),
        ];
        let (status, _, json) = self.send("POST", path, &headers, &body.to_string());
        (status, json)
    }

    /// A call to `/op/<name>` as the signed-in owner, in the deployment's
    /// space: reads carry their parameters in the query, writes in a body.
    fn op(&self, token: &str, method: &str, name: &str, params: Value) -> (u16, Value) {
        static KEY: AtomicU64 = AtomicU64::new(1);
        let bearer = format!("Bearer {token}");
        let key = format!("site-test-{}", KEY.fetch_add(1, Ordering::SeqCst));
        let mut headers = vec![
            ("X-Ordo-Space", HOME),
            ("Authorization", bearer.as_str()),
            ("Origin", PAGE),
            ("Accept", "application/json"),
        ];
        if method != "GET" {
            headers.push(("Idempotency-Key", key.as_str()));
        }
        let (path, body) = if method == "POST" {
            headers.push(("Content-Type", "application/json"));
            (format!("/op/{name}"), params.to_string())
        } else {
            (format!("/op/{name}"), String::new())
        };
        let (status, _, json) = self.send(method, &path, &headers, &body);
        (status, json)
    }

    /// What the page sends: no token, no space.
    fn visit(&self, path: &str, extra: &[(&str, &str)]) -> (u16, String, Value) {
        let mut headers = vec![
            ("Origin", "https://any.example"),
            ("Accept", "application/json"),
        ];
        headers.extend_from_slice(extra);
        self.send("GET", path, &headers, "")
    }

    /// The contact form, as `App.tsx` posts it.
    fn contact(&self, extra: &[(&str, &str)], body: Value) -> (u16, String, Value) {
        let mut headers = vec![
            ("Origin", "https://any.example"),
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
        ];
        headers.extend_from_slice(extra);
        self.send("POST", "/op/contact", &headers, &body.to_string())
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status();
        // A server that does not answer its stop signal is killed, so a hung
        // one fails a test instead of hanging the run.
        let told = std::time::Instant::now();
        while told.elapsed() < Duration::from_secs(40) {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            let _ = std::fs::write(std::env::temp_dir().join("ordo-site-last.log"), self.log());
            eprintln!("--- the server log is in ordo-site-last.log ---");
        }
        let _ = std::fs::remove_file(&self.log);
        if self.keep {
            return;
        }
        // Every person's tenant has a database of its own.
        let control = Pg {
            db: self.db.clone(),
            ..Pg::default()
        };
        if let Ok(tenants) = control.query("SELECT \"$space\" FROM \"$spaces\"") {
            for row in tenants {
                if let Some(Some(id)) = row.first() {
                    drop_database(id);
                }
            }
        }
        drop_database(&self.db);
    }
}

fn wait_for_live(port: u16, child: &mut Child, log: &std::path::Path) {
    for _ in 0..300 {
        if let Ok(Some(status)) = child.try_wait() {
            let said = std::fs::read_to_string(log).unwrap_or_default();
            panic!("the binary exited before serving: {status}\n{said}");
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the binary never bound {port}");
}

fn code_of(body: &Value) -> String {
    body["error"]["code"].as_str().unwrap_or("").to_string()
}

/// Run `probe` until it answers `Some`, for at most thirty seconds.
fn eventually<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let started = std::time::Instant::now();
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "still waiting after 30 s: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn names(rows: &[Value], key: &str) -> Vec<String> {
    rows.iter()
        .map(|r| r[key].as_str().unwrap_or("").to_string())
        .collect()
}

/// The site owner: a guest whose principal the next start names in
/// `ORDO_DEPLOYMENT_OWNERS`, as `dev/seed` and `dev/serve.sh` do.
struct Owner {
    token: String,
    principal: String,
}

fn sign_in_owner(d: &Deployment) -> Owner {
    let (status, done) = d.auth(
        "/auth/complete",
        json!({"method": "guest", "kind": "new", "display": "Site owner"}),
    );
    assert_eq!(status, 200, "{done}");
    let token = done["token"].as_str().expect("a token").to_string();
    // As `dev/seed` reads it: the token is signed, not sealed.
    let mut key = [0u8; 32];
    for (i, b) in key.iter_mut().enumerate() {
        *b = u8::from_str_radix(&TOKEN_KEY[i * 2..i * 2 + 2], 16).unwrap();
    }
    let claims = ordo_identity::token::verify(
        &ordo_identity::TokenKey::from_secret(&key),
        &token,
        ordo_identity::principal::Timestamp(0),
    )
    .expect("the token verifies");
    Owner {
        token,
        principal: claims.principal.0,
    }
}

/// One write as the owner. Answers the new row's id.
fn write(d: &Deployment, o: &Owner, op: &str, params: Value) -> String {
    let (status, out) = d.op(&o.token, "POST", op, params);
    assert_eq!(status, 200, "{op}: {out}");
    out["rows"][0]["created"]
        .as_str()
        .unwrap_or_else(|| panic!("{op} answered no id: {out}"))
        .to_string()
}

/// Boot, sign the owner in, start again naming them, and seed three
/// products, two plans and two testimonials, as `dev/seed` does.
async fn seeded_site(db: &str, extra: &[(&str, &str)]) -> (Deployment, Owner) {
    let first = Deployment::boot(db, extra).await;
    let owner = sign_in_owner(&first);
    let mut settings: Vec<(&str, &str)> = extra.to_vec();
    settings.push(("ORDO_DEPLOYMENT_OWNERS", &owner.principal));
    let d = first.restart(&settings);
    eventually("the owner may write the deployment's space", || {
        let (status, _) = d.op(&owner.token, "GET", "enquiryList", json!({}));
        (status == 200).then_some(())
    });
    for (i, slug) in ["alpha", "beta", "gamma"].iter().enumerate() {
        write(
            &d,
            &owner,
            "addProduct",
            json!({
                "slug": slug, "name": format!("Product {slug}"), "tagline": "A tagline",
                "description": "A description.", "price": "19.50",
                "features": ["Fast", "Private"], "rank": i as i64 + 1,
            }),
        );
    }
    for (i, name) in ["Starter", "Team"].iter().enumerate() {
        write(
            &d,
            &owner,
            "addPlan",
            json!({
                "name": name, "tagline": "For you", "price": "9.00", "period": "month",
                "features": ["One"], "highlighted": i == 1, "rank": i as i64 + 1,
            }),
        );
    }
    for (i, who) in ["Ada", "Bo"].iter().enumerate() {
        write(
            &d,
            &owner,
            "addTestimonial",
            json!({
                "author": who, "role": "Lead", "company": "Acme",
                "quote": "It works.", "stars": 5, "rank": i as i64 + 1,
            }),
        );
    }
    (d, owner)
}

fn enquiry() -> Value {
    json!({"name": "Vee", "email": "vee@example.com", "company": "Acme", "message": "Hello"})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_visitor_reads_the_site_with_no_token_from_any_page() {
    let _serial = SERIAL.lock().await;
    let (d, _owner) = seeded_site("ordo_site_read", &[]).await;

    let read = |path: &str| {
        eventually("the public read", || {
            let (status, head, body) = d.visit(path, &[]);
            (status == 200).then_some((head, body))
        })
    };
    let (head, catalogue) = read("/op/catalogue");
    assert!(head.contains("access-control-allow-origin: *"), "{head}");
    assert!(head.contains("x-ordo-cache:"), "{head}");
    let rows = catalogue["rows"].as_array().expect("rows");
    assert_eq!(names(rows, "slug"), vec!["alpha", "beta", "gamma"]);
    assert_eq!(rows[0]["features"], json!(["Fast", "Private"]));

    let (_, one) = read("/op/product?slug=beta");
    assert_eq!(
        names(one["rows"].as_array().unwrap(), "name"),
        vec!["Product beta"]
    );
    let (_, plans) = read("/op/pricing");
    assert_eq!(
        names(plans["rows"].as_array().unwrap(), "name"),
        vec!["Starter", "Team"]
    );
    assert_eq!(plans["rows"][1]["highlighted"], true);
    let (_, reviews) = read("/op/reviews");
    assert_eq!(
        names(reviews["rows"].as_array().unwrap(), "author"),
        vec!["Ada", "Bo"]
    );

    // The owner's ops and the enquiries stay closed to a visitor.
    for path in ["/op/enquiryList", "/op/std.products.list"] {
        let (status, _, body) = d.visit(path, &[]);
        assert_eq!(
            (status, code_of(&body)),
            (403, "access_denied".into()),
            "{path}: {body}"
        );
    }
    d.stop_quietly();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_contact_form_writes_an_enquiry_that_no_visitor_reads_back() {
    let _serial = SERIAL.lock().await;
    let (d, owner) = seeded_site("ordo_site_contact", &[("ORDO_PUBLIC_HUMAN_CHECK", "off")]).await;

    let (status, head, body) = d.contact(&[], enquiry());
    assert_eq!(status, 200, "{body}");
    assert!(head.contains("access-control-allow-origin: *"), "{head}");
    let row = body["rows"][0].as_object().expect("a row");
    assert_eq!(row.keys().collect::<Vec<_>>(), ["created"], "{body}");
    assert!(!body.to_string().contains("vee@example.com"), "{body}");

    // The owner reads it; a visitor cannot write the owner's ops.
    let (status, listed) = d.op(&owner.token, "GET", "enquiryList", json!({}));
    assert_eq!(status, 200, "{listed}");
    assert_eq!(
        names(listed["rows"].as_array().unwrap(), "email"),
        vec!["vee@example.com"]
    );
    let (status, _, body) = d.send(
        "POST",
        "/op/addProduct",
        &[("Content-Type", "application/json")],
        &json!({"slug": "evil", "name": "Evil", "tagline": "", "description": "",
                "price": "1.00", "features": [], "rank": 1})
        .to_string(),
    );
    assert_eq!(
        (status, code_of(&body)),
        (403, "access_denied".into()),
        "{body}"
    );
    let (_, catalogue) = eventually("the catalogue", || {
        let (s, _, b) = d.visit("/op/catalogue", &[]);
        (s == 200).then_some(((), b))
    });
    assert_eq!(
        catalogue["rows"].as_array().unwrap().len(),
        3,
        "nothing was added"
    );
    d.stop_quietly();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_human_check_is_on_unless_the_deployment_turns_it_off() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot("ordo_site_human", &[]).await;
    let (status, _, body) = d.contact(&[], enquiry());
    assert_eq!(
        (status, code_of(&body)),
        (403, "human_check_failed".into()),
        "{body}"
    );
    let text = body.to_string();
    assert!(text.contains("ORDO_PUBLIC_HUMAN_SECRET_FILE"), "{text}");
    assert!(text.contains("ORDO_AUTH_HUMAN_CHECK_SECRET_FILE"), "{text}");
    assert!(text.contains("ORDO_PUBLIC_HUMAN_CHECK=off"), "{text}");
    d.stop_quietly();
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_mode_slows_a_busy_visitor_through_the_real_socket() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot(
        "ordo_site_proxy",
        &[
            ("ORDO_PROXY_HEADER", "X-Forwarded-For"),
            ("ORDO_PROXY_TRUSTED", "127.0.0.1"),
            ("ORDO_PROXY_SLOW_AFTER", "2"),
            ("ORDO_PROXY_SLOW_MS", "500"),
        ],
    )
    .await;
    assert!(d.log().contains("proxy mode: on"), "{}", d.log());
    let timed = |visitor: &str| {
        let started = std::time::Instant::now();
        let (status, _, body) = d.visit("/op/pricing", &[("X-Forwarded-For", visitor)]);
        assert_eq!(status, 200, "{body}");
        started.elapsed()
    };
    for _ in 0..2 {
        assert!(timed("203.0.113.9") < Duration::from_millis(500));
    }
    assert!(
        timed("203.0.113.9") >= Duration::from_millis(500),
        "slowed, still served"
    );
    assert!(
        timed("203.0.113.10") < Duration::from_millis(500),
        "another visitor"
    );
    d.stop_quietly();
}

/// The two address limits, back as slow-downs in proxy mode (28a): wrong
/// share-link secrets and sign-ups past their limit wait, and the answer is
/// the one each would get anyway, never `429`.
#[tokio::test(flavor = "multi_thread")]
async fn the_address_limits_slow_and_never_refuse_in_proxy_mode() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot(
        "ordo_site_limits",
        &[
            ("ORDO_PROXY_HEADER", "X-Forwarded-For"),
            ("ORDO_PROXY_TRUSTED", "127.0.0.1"),
            ("ORDO_PROXY_SLOW_MS", "3000"),
            ("ORDO_SHARE_LINK_MISS_LIMIT", "2"),
            ("ORDO_PROVISION_SIGNUPS_PER_ADDRESS_PER_HOUR", "1"),
        ],
    )
    .await;
    let post = |path: &str, body: Value, visitor: &str| {
        let started = std::time::Instant::now();
        let (status, _, answer) = d.send(
            "POST",
            path,
            &[
                ("Content-Type", "application/json"),
                ("Origin", PAGE),
                ("X-Forwarded-For", visitor),
            ],
            &body.to_string(),
        );
        (status, answer, started.elapsed())
    };
    // The slow-down is 3 s so that a busy runner's ordinary request (well
    // under a second) never looks slowed; the line between the two is half.
    const SLOWED: Duration = Duration::from_millis(1500);
    let wrong = json!({"link": "not-a-real-link-secret"});
    for _ in 0..2 {
        let (status, _, took) = post("/auth/link", wrong.clone(), "203.0.113.9");
        assert_eq!(status, 403);
        assert!(took < SLOWED, "{took:?}");
    }
    let (status, answer, took) = post("/auth/link", wrong.clone(), "203.0.113.9");
    assert_eq!(status, 403, "the same refusal, never 429: {answer}");
    assert!(took >= SLOWED, "slowed: {took:?}");
    let (_, _, took) = post("/auth/link", wrong, "203.0.113.10");
    assert!(took < SLOWED, "another address: {took:?}");

    let begin = json!({"method": "passkey"});
    let (status, answer, took) = post("/auth/register", begin.clone(), "198.51.100.7");
    assert_eq!(status, 200, "{answer}");
    assert!(took < SLOWED, "{took:?}");
    let (status, answer, took) = post("/auth/register", begin, "198.51.100.7");
    assert_eq!(status, 200, "slowed, still served: {answer}");
    assert!(took >= SLOWED, "slowed: {took:?}");
    d.stop_quietly();
}

impl Deployment {
    fn stop_quietly(self) {
        drop(self);
    }
}
