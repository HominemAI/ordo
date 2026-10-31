//! The authorization matrix's deployment: the real `ordo-host` binary on
//! real Postgres, with the fixture program `authz.ordo`, passkey sign-in,
//! a file store, share links, delegation and console keys switched on.
//!
//! Nothing here builds the server's parts by hand. The test talks to the
//! binary over a raw socket, so it can send what a browser never would: a
//! lying `Content-Length`, a garbled bearer, a foreign `Origin`.

#![allow(dead_code)]

pub mod actors;
pub mod catalog;
pub mod identity;
pub mod policy;
pub mod routes;

/// The mail relay stand-in the other host tests use (plaintext, on loopback).
#[path = "../mail_support/mod.rs"]
pub mod mail_support;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::KeyProvider as _;
use ordo_exec::{Pool, PoolConfig};
use serde_json::Value;

pub const SOURCE: &str = include_str!("authz.ordo");

/// The deployment's own space. Public rows live here, and the public path
/// serves only `system` (`howto-public.md`).
pub const HOME: &str = "system";
pub const MASTER: &str = "the-authz-matrix-test-master-key";
pub const TOKEN_KEY: &str = "5151515151515151515151515151515151515151515151515151515151515151";
/// Another deployment's token key: a bearer signed with it is a forgery here.
pub const FOREIGN_TOKEN_KEY: &str =
    "6262626262626262626262626262626262626262626262626262626262626262";
pub const PAGE_KEY: &str = "7373737373737373737373737373737373737373737373737373737373737373";

/// The app's page: the one origin on `ORDO_CORS_ORIGINS` and
/// `ORDO_AUTH_ORIGINS`.
pub const PAGE: &str = "http://localhost:5181";
/// A page that is not the app's.
pub const FOREIGN: &str = "https://evil.example";
/// The agent `ORDO_DELEGATION_AGENTS` names.
pub const AGENT: &str = "agt_matrix";
/// Wrong share-link password tries before the lock: above the slowdown's 5,
/// so the slowdown rows meet the slowdown and not the lock.
pub const LINK_TRIES: u32 = 10;

pub fn cfg(db: &str) -> PoolConfig {
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

pub fn drop_database(name: &str) {
    let _ = Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
    .exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

/// A database holding the program and custody, as `ordo apply` and the
/// first boot leave it.
async fn deployment_with_program(db: &str) {
    drop_database(db);
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
    let plain = Fixture::from_source(SOURCE, "").expect("authz.ordo compiles");
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

    let mut prog = Fixture::from_source(SOURCE, "").expect("authz.ordo compiles");
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
        actor: "operator:authz-matrix".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:authz-matrix",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
}

/// One answer, kept whole: the matrix judges status, code and headers.
#[derive(Clone, Debug)]
pub struct Answer {
    pub status: u16,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The refusal's code: `error.code`, or `error` when it is a word.
    pub fn code(&self) -> String {
        let j = self.json();
        if let Some(c) = j["error"]["code"].as_str() {
            return c.to_string();
        }
        if let Some(c) = j["error"].as_str() {
            return c.to_string();
        }
        if let Some(c) = j["code"].as_str() {
            return c.to_string();
        }
        String::new()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Every `Set-Cookie` value.
    pub fn cookies(&self) -> Vec<String> {
        self.headers
            .iter()
            .filter(|(k, _)| k == "set-cookie")
            .map(|(_, v)| v.clone())
            .collect()
    }
}

/// A held-open event stream: what it has sent, and whether it has ended.
pub struct Stream {
    s: TcpStream,
    pub status: u16,
    /// Everything sent since the head, chunk framing included.
    pub text: String,
    pub closed: bool,
}

impl Stream {
    /// Read for at most `wait`, or until the server ends the stream.
    /// Answers what arrived in this call.
    pub fn read_for(&mut self, wait: Duration) -> String {
        let until = std::time::Instant::now() + wait;
        let mut got = String::new();
        let mut buf = [0u8; 8192];
        while !self.closed && std::time::Instant::now() < until {
            match self.s.read(&mut buf) {
                Ok(0) => self.closed = true,
                Ok(n) => {
                    let t = String::from_utf8_lossy(&buf[..n]).into_owned();
                    self.text.push_str(&t);
                    got.push_str(&t);
                    if got.ends_with("0\r\n\r\n") {
                        self.closed = true;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => self.closed = true,
            }
        }
        got
    }
}

/// A running deployment. Killed, and its databases dropped, when dropped.
pub struct Deployment {
    pub port: u16,
    child: Child,
    pub db: String,
    log: std::path::PathBuf,
    pub blobs: std::path::PathBuf,
    /// The mail relay the door sends emailed codes through: the outbox the
    /// rows read a code from.
    pub relay: mail_support::FakeRelay,
    /// Settings every start carries: mail, the emailed-code method and one
    /// outside provider.
    base: Vec<(String, String)>,
    secret: std::path::PathBuf,
    /// False for a second process on the same database (`replica`): it
    /// stops its own process and leaves the databases and the store to the
    /// deployment that made them.
    owner: bool,
}

/// The outside provider the world has on: an OAuth2 `discord` connection.
/// A `begin` builds the redirect without calling the provider, so no
/// stand-in server is needed for the rows that stop there.
pub const OUTSIDE_RETURN: &str = "http://localhost:4981";

impl Deployment {
    pub async fn boot(db: &str) -> Deployment {
        Deployment::boot_with(db, &[]).await
    }

    /// [`Deployment::boot`] with `extra` settings on every start of this
    /// process.
    pub async fn boot_with(db: &str, extra: &[(&str, &str)]) -> Deployment {
        deployment_with_program(db).await;
        let port = free_port();
        let log = std::env::temp_dir().join(format!("ordo-authz-{db}-{port}.log"));
        let blobs = std::env::temp_dir().join(format!("ordo-authz-blobs-{db}-{port}"));
        let _ = std::fs::remove_dir_all(&blobs);
        std::fs::create_dir_all(&blobs).expect("blob dir");
        let _ = std::fs::remove_file(&log);
        let relay = mail_support::FakeRelay::start_plaintext().await;
        let secret = std::env::temp_dir().join(format!("ordo-authz-secret-{db}-{port}"));
        std::fs::write(&secret, "matrix-discord-client-secret").expect("secret file");
        let relay_port = relay.port.to_string();
        let base: Vec<(String, String)> = [
            ("ORDO_AUTH_METHODS", "passkey,password,email,discord"),
            ("ORDO_MAIL_KIND", "smtp"),
            ("ORDO_MAIL_FROM", "noreply@example.com"),
            ("ORDO_MAIL_HOST", "localhost"),
            ("ORDO_MAIL_PORT", relay_port.as_str()),
            ("ORDO_MAIL_TLS", "none"),
            ("ORDO_MAIL_SEND_SECS", "1"),
            ("ORDO_EGRESS_PRIVATE_HOSTS", "localhost"),
            ("ORDO_PUBLIC_URL", "https://api.example.com"),
            ("ORDO_AUTH_RETURN_URLS", OUTSIDE_RETURN),
            ("ORDO_AUTH_DISCORD_PRESET", "discord"),
            ("ORDO_AUTH_DISCORD_CLIENT_ID", "matrix-client"),
            (
                "ORDO_AUTH_DISCORD_CLIENT_SECRET_FILE",
                secret.to_str().expect("utf-8 path"),
            ),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .chain(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
        let child = spawn_with(db, port, &log, &blobs, &base, &[]);
        Deployment {
            port,
            child,
            db: db.to_string(),
            log,
            blobs,
            relay,
            base,
            secret,
            owner: true,
        }
    }

    /// A second process on the same control database, keys and file store,
    /// on a port of its own. `extra` is applied after this process's own
    /// settings, so it can name a different `ORDO_REPLICA_ID`.
    pub async fn replica(&self, extra: &[(&str, &str)]) -> Deployment {
        let port = free_port();
        let log = std::env::temp_dir().join(format!("ordo-authz-{}-{port}.log", self.db));
        let _ = std::fs::remove_file(&log);
        let relay = mail_support::FakeRelay::start_plaintext().await;
        let child = spawn_with(&self.db, port, &log, &self.blobs, &self.base, extra);
        Deployment {
            port,
            child,
            db: self.db.clone(),
            log,
            blobs: self.blobs.clone(),
            relay,
            base: self.base.clone(),
            secret: self.secret.clone(),
            owner: false,
        }
    }

    /// Stop the binary and start it again on the same database, store and
    /// port, with `extra` settings. Tokens stay valid: the key is the same.
    pub fn restart(&mut self, extra: &[(&str, &str)]) {
        stop(&mut self.child);
        self.child = spawn_with(
            &self.db,
            self.port,
            &self.log,
            &self.blobs,
            &self.base,
            extra,
        );
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// One HTTP/1.1 request, sent as bytes. `body` is sent as given; a
    /// `Content-Length` is added unless `headers` carries one already, so a
    /// row can lie about it.
    pub fn raw(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Answer {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(20))).ok();
        let mut req =
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
        if !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        let mut bytes = req.into_bytes();
        bytes.extend_from_slice(body);
        // A server that answers before reading the whole body may reset the
        // connection under a write; the answer it sent is what counts.
        // No half-close: hyper ends a connection whose read side closes.
        let _ = s.write_all(&bytes);
        let mut raw = Vec::new();
        // Read to the end, except an event stream, which never ends: its
        // head is the answer the matrix judges.
        let mut buf = [0u8; 8192];
        loop {
            match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
            if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                if head.contains("content-type: text/event-stream") {
                    break;
                }
            }
        }
        parse_answer(&raw)
    }

    /// Open an event stream and keep the connection: the head is read, the
    /// frames are read later with [`Stream::read_for`].
    pub fn open_stream(&self, path: &str, headers: &[(String, String)]) -> Stream {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        let mut req =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        s.write_all(req.as_bytes()).expect("send");
        s.set_read_timeout(Some(Duration::from_millis(100))).ok();
        let mut raw = Vec::new();
        let mut buf = [0u8; 8192];
        let started = std::time::Instant::now();
        let end = loop {
            if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            match s.read(&mut buf) {
                Ok(0) => break raw.len(),
                Ok(n) => raw.extend_from_slice(&buf[..n]),
                Err(_) if started.elapsed() < Duration::from_secs(10) => {}
                Err(_) => break raw.len(),
            }
        };
        let head = String::from_utf8_lossy(&raw[..end.min(raw.len())]).into_owned();
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let rest = raw.get(end + 4..).unwrap_or(&[]);
        Stream {
            s,
            status,
            text: String::from_utf8_lossy(rest).into_owned(),
            closed: false,
        }
    }

    pub fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Answer {
        let h: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        self.raw(method, path, &h, body.as_bytes())
    }

    /// A POST to `/auth/...` from the app's page, as the SDK sends it.
    pub fn auth(&self, path: &str, body: &Value, token: Option<&str>) -> Answer {
        let bearer = token.map(|t| format!("Bearer {t}"));
        let mut h = vec![
            ("Content-Type", "application/json"),
            ("Origin", PAGE),
            ("Accept", "application/json"),
        ];
        if let Some(b) = &bearer {
            h.push(("Authorization", b));
        }
        self.send("POST", path, &h, &body.to_string())
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        stop(&mut self.child);
        if !self.owner {
            if std::thread::panicking() {
                let _ = std::fs::write(
                    std::env::temp_dir().join(format!("ordo-authz-replica-{}.log", self.port)),
                    self.log(),
                );
            }
            let _ = std::fs::remove_file(&self.log);
            return;
        }
        if std::thread::panicking() {
            let _ = std::fs::write(std::env::temp_dir().join("ordo-authz-last.log"), self.log());
            eprintln!("--- the server log is in ordo-authz-last.log ---");
        }
        let _ = std::fs::remove_file(&self.log);
        let _ = std::fs::remove_file(&self.secret);
        let _ = std::fs::remove_dir_all(&self.blobs);
        // `ORDO_AUTHZ_KEEP=1` leaves the databases for a look afterwards.
        if std::env::var("ORDO_AUTHZ_KEEP").is_ok_and(|v| v == "1") {
            return;
        }
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

fn stop(child: &mut Child) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status();
    let told = std::time::Instant::now();
    while told.elapsed() < Duration::from_secs(40) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The binary with the world's base settings, then `extra`.
fn spawn_with(
    db: &str,
    port: u16,
    log: &std::path::Path,
    blobs: &std::path::Path,
    base: &[(String, String)],
    extra: &[(&str, &str)],
) -> Child {
    let all: Vec<(&str, &str)> = base
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .chain(extra.iter().copied())
        .collect();
    spawn(db, port, log, blobs, &all)
}

/// The binary on `db`, which already holds the program.
fn spawn(
    db: &str,
    port: u16,
    log: &std::path::Path,
    blobs: &std::path::Path,
    extra: &[(&str, &str)],
) -> Child {
    let base = PoolConfig::from_env();
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
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
        // A held-open feed asks the door again each second, so the stream
        // rows see a refusal within a short wait.
        .env("ORDO_LIVE_RECHECK_SECS", "1")
        // Password too, so the slowdown rows reach the password ceremony.
        .env("ORDO_AUTH_METHODS", "passkey,password")
        .env("ORDO_APEX_DOMAIN", "localhost")
        .env("ORDO_AUTH_ORIGINS", PAGE)
        .env("ORDO_CORS_ORIGINS", PAGE)
        .env("ORDO_AUTH_PROVISIONING", "auto-onboard")
        .env("ORDO_PROVISION_SELF_SERVE", "on-signup")
        .env("ORDO_BLOB_STORE", format!("fs:{}", blobs.display()))
        .env("ORDO_DEV_UNSAFE_BLOB_STORE", "1")
        .env(
            "ORDO_DELEGATION_AGENTS",
            format!("{AGENT}=Matrix assistant"),
        )
        .env("ORDO_SHARE_LINK_PASSWORD_TRIES", LINK_TRIES.to_string())
        .env("ORDO_SHARE_LINK_PASSWORD_LOCK_SECS", "900")
        .env_remove("ORDO_DEPLOYMENT_OWNERS")
        .env_remove("ORDO_PG_ROUTING")
        .env_remove("ORDO_TENANT_CREDENTIALS")
        .env_remove("ORDO_TENANT_CREDENTIALS_DIR")
        .env_remove("ORDO_CHECKPOINT_KEY")
        .env_remove("ORDO_SECURITY_CHECKPOINT_KEY")
        .env_remove("ORDO_EXIT_BUNDLE_DIR")
        .env_remove("ORDO_STEP_UP_EXEMPT_OPS")
        .env_remove("ORDO_DELEGABLE_OPS")
        .env_remove("ORDO_ACCESS_TEMPLATE")
        .env_remove("ORDO_AUTH_GUEST")
        .env_remove("ORDO_PRESENCE_ON_READS")
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    for (k, v) in extra {
        c.env(k, v);
    }
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child, log);
    child
}

fn wait_for_live(port: u16, child: &mut Child, log: &std::path::Path) {
    for _ in 0..600 {
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

/// Status line, headers and body; a chunked body is joined.
fn parse_answer(raw: &[u8]) -> Answer {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut body = raw.get(split + 4..).unwrap_or(&[]).to_vec();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    if headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.contains("chunked"))
    {
        body = dechunk(&body);
    }
    Answer {
        status,
        headers,
        body,
    }
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = b.windows(2).position(|w| w == b"\r\n") {
        let size = std::str::from_utf8(&b[..eol])
            .ok()
            .and_then(|s| usize::from_str_radix(s.split(';').next().unwrap_or("").trim(), 16).ok())
            .unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = eol + 2;
        let end = (start + size).min(b.len());
        out.extend_from_slice(&b[start..end]);
        b = b.get(end + 2..).unwrap_or(&[]);
    }
    out
}

/// Wait for `probe` to answer, at most 30 s.
pub fn eventually<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
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
