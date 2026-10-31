//! Upgrade continuity: people, sign-ins and data survive an upgrade.
//!
//! The owner (2026-10-27): *"if we design some ordo, we have an application,
//! and then we upgrade the application, we want to make sure users are still
//! there, logins haven't changed, etc."* Live data must survive upgrades
//! (ruled 2026-10-19, `docs/corpus-rulings.md`).
//!
//! Two kinds of upgrade, one test each, the same state and the same checks:
//!
//! 1. **The app's program changes.** `ordo apply` of version 1.1 over a
//!    running version 1 (`tests/upgrade_fixture/`). 1.1 adds an optional
//!    field, an index, an entity, ops, and a guard clause that keeps
//!    everyone's access as it was.
//! 2. **Ordo itself is upgraded.** The previous `ordo-host` serves version 1
//!    and the state is made under it. It is stopped, and this build's
//!    `ordo-host` is started on the same databases with the same keys. Then
//!    version 1.1 is applied with this build too. Ignored by default: it
//!    needs the previous Ordo built, which `scripts/upgrade-continuity.sh`
//!    does and then runs this test.
//!
//! The state made before the upgrade:
//!
//! - **Ada** signs up with a passkey and owns her tenant. She holds a
//!   renewal cookie, 57 notes, a journal entry and a file on a note.
//! - **Bo** signs up with a passkey and accepts a share of one of Ada's notes.
//! - **Kim** signs in with an emailed code and has a tenant of her own.
//! - **Carl** is created by Ada as a member (editor) of her tenant, redeems
//!   the ticket for a password and signs in with it.
//! - An open **share link** on the shared note, opened once.
//! - A **delegation**: an agent Ada approved reads her notes.
//! - A **live feed** on Ada's notes, open across the upgrade.
//! - A **page cursor** handed out on the first page of 50.
//! - An **idempotency key** on a write that already answered.
//! - **Backend functions** (`upgrade_fixture/functions/`): `probe`, which
//!   uses the secret `probeKey`, and `greet`. The deployment owner sets the
//!   secret through the door, and Ada and Kim call both in their tenants,
//!   which leaves rows in `$fnruns`. Version 1.1 changes `probe`'s code and
//!   renames `greet` to `hello`. After the upgrade `probe` runs the new
//!   code in both tenants and its secret still opens; the secret is listed
//!   but never answered, and sealed at rest; the earlier runs are kept; and
//!   `greet` answers as a name that never existed. The program upgrade
//!   always has them. The Ordo upgrade has them when the previous Ordo does
//!   (`ORDO_UPGRADE_FUNCTIONS`, which the script sets).
//!
//! After the upgrade each item is checked by name (`Check`). A check that
//! fails is a continuity break. A break that is known and filed goes in
//! `KNOWN_*` with the reason: the ordinary test then tolerates it and prints
//! it, and a strict copy, `#[ignore = "KNOWN BREAK: ..."]` with an empty
//! list, fails on it until it is fixed. None is listed today (2026-10-27).
//!
//! Recorded by the first runs: across a program update the open feed carries
//! on and the page cursor continues; across the Ordo upgrade from ef37d9de
//! the feed's connection closes (a new one carries on) and the cursor is
//! refused `version_skew`, which the SDK answers by starting from page one.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`), or the
//! one `scripts/upgrade-continuity.sh` starts. Databases are prefixed
//! `ordo_upg_`.

mod mail_support;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ordo_conformance::Pg;
use ordo_exec::PoolConfig;
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use ordo_identity::CompleteRequest;
use serde_json::{json, Value};

const MASTER: &str = "the-upgrade-continuity-master-key";
const TOKEN_KEY: &str = "5555555555555555555555555555555555555555555555555555555555555555";
const PAGE_KEY: &str = "6666666666666666666666666666666666666666666666666666666666666666";
/// The page's origin, as a local deployment serves it.
const PAGE: &str = "http://localhost:5173";
const AGENT: &str = "agt_helper";
const CARL_ID: &str = "carl@example.org";
const CARL_PASSWORD: &str = "carl-chose-this-one";
const KIM_ID: &str = "kim@example.org";
/// The bytes of the file on Ada's note.
const FILE: &str = "milk, eggs, the file kept across the upgrade\n";
/// Notes Ada writes for the paged list: more than one page of 50.
const PAGED: usize = 55;

/// Every test boots whole deployments; they take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn fixture(version: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/upgrade_fixture")
        .join(version)
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
    .exec(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"));
}

// ── The deployment: databases, keys and settings that outlive a process ──

/// What stays the same across an upgrade: the control database, the blob
/// folder, the mail relay and every setting. A [`Host`] is one process
/// serving it.
struct World {
    db: String,
    blobs: PathBuf,
    relay: mail_support::FakeRelay,
}

impl World {
    async fn new(db: &str) -> World {
        drop_database(db);
        let admin = Pg {
            db: "postgres".into(),
            ..Pg::default()
        };
        admin
            .exec(&format!("CREATE DATABASE \"{db}\""))
            .expect("the control database");
        let blobs = std::env::temp_dir().join(format!("{db}-blobs"));
        let _ = std::fs::remove_dir_all(&blobs);
        std::fs::create_dir_all(&blobs).expect("blob folder");
        World {
            db: db.to_string(),
            blobs,
            relay: mail_support::FakeRelay::start_plaintext().await,
        }
    }

    /// Start `binary` on this deployment, with the settings a local
    /// deployment's README gives, every way in switched on.
    fn boot(&self, binary: &Path, tag: &str) -> Host {
        let port = free_port();
        let base = PoolConfig::from_env();
        let log = std::env::temp_dir().join(format!("ordo-upg-{}-{tag}.log", self.db));
        let _ = std::fs::remove_file(&log);
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .expect("log");
        let err = out.try_clone().expect("log");
        let relay_port = self.relay.port.to_string();
        let mut c = Command::new(binary);
        c.env("ORDO_PG_POOL_SIZE", "6")
            .env("ORDO_PG_HOST", &base.host)
            .env("ORDO_PG_PORT", base.port.to_string())
            .env("ORDO_PG_USER", &base.user)
            .env("ORDO_PG_DB", &self.db)
            .env("ORDO_SPACE", "system")
            .env("ORDO_KEY_PROVIDER", "local")
            .env("ORDO_MASTER_KEY", MASTER)
            .env("ORDO_DEV_UNSAFE_TEST_KEYS", "1")
            .env("ORDO_TOKEN_KEY", TOKEN_KEY)
            .env("ORDO_PAGE_KEY", PAGE_KEY)
            .env("ORDO_HOST_ADDR", format!("127.0.0.1:{port}"))
            .env("ORDO_AUTH_METHODS", "passkey,email,password")
            .env("ORDO_APEX_DOMAIN", "localhost")
            .env("ORDO_AUTH_ORIGINS", PAGE)
            .env("ORDO_CORS_ORIGINS", PAGE)
            .env("ORDO_AUTH_PROVISIONING", "auto-onboard")
            .env("ORDO_PROVISION_SELF_SERVE", "on-signup")
            .env("ORDO_BLOB_STORE", format!("fs:{}", self.blobs.display()))
            .env("ORDO_DEV_UNSAFE_BLOB_STORE", "1")
            .env("ORDO_MAIL_KIND", "smtp")
            .env("ORDO_MAIL_FROM", "noreply@example.com")
            .env("ORDO_MAIL_HOST", "localhost")
            .env("ORDO_MAIL_PORT", &relay_port)
            .env("ORDO_MAIL_TLS", "none")
            .env("ORDO_MAIL_SEND_SECS", "1")
            .env("ORDO_EGRESS_PRIVATE_HOSTS", format!("localhost,{FN_HOST}"))
            .env("ORDO_DELEGATION_AGENTS", format!("{AGENT}=Helper"))
            .env("ORDO_DEPLOYMENT_OWNERS", DEPLOYMENT_OWNER)
            .env_remove("ORDO_PG_ROUTING")
            .env_remove("ORDO_TENANT_CREDENTIALS")
            .env_remove("ORDO_TENANT_CREDENTIALS_DIR")
            .env_remove("ORDO_CHECKPOINT_KEY")
            .env_remove("ORDO_SECURITY_CHECKPOINT_KEY")
            .env_remove("ORDO_EXIT_BUNDLE_DIR")
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        let mut child = c.spawn().expect("spawn ordo-host");
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let said = std::fs::read_to_string(&log).unwrap_or_default();
                panic!(
                    "{} exited before serving: {status}\n{said}",
                    binary.display()
                );
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "{} never bound {port}",
                binary.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        Host {
            port,
            child: Some(child),
            log,
        }
    }

    /// `ordo apply <dir>` with this build's CLI, in this process, as the
    /// command line runs it. It waits for the running server to serve it.
    async fn apply_current(&self, dir: &Path) -> String {
        let args = vec![
            dir.display().to_string(),
            "--as".into(),
            "upgrade-test".into(),
            "--db".into(),
            self.db.clone(),
        ];
        let out = tokio::task::spawn_blocking(move || ordo_cli::operator::run("apply", &args))
            .await
            .expect("apply ran");
        assert_eq!(out.code, 0, "ordo apply {}:\n{}", dir.display(), out.report);
        out.report
    }

    /// `ordo apply <dir>` with another build's `ordo` binary.
    fn apply_with(&self, cli: &Path, dir: &Path) -> String {
        let base = PoolConfig::from_env();
        let out = Command::new(cli)
            .arg("apply")
            .arg(dir)
            .args(["--as", "upgrade-test", "--db", &self.db])
            .env("ORDO_PG_HOST", &base.host)
            .env("ORDO_PG_PORT", base.port.to_string())
            .env("ORDO_PG_USER", &base.user)
            .env("ORDO_PG_DB", &self.db)
            .env("ORDO_MASTER_KEY", MASTER)
            .env("ORDO_DEV_UNSAFE_TEST_KEYS", "1")
            .output()
            .expect("run ordo apply");
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{} apply: {said}", cli.display());
        said
    }

    /// `ordo verify` with this build, in this process: `(exit code, report)`.
    async fn verify(&self, args: &[&str]) -> (i32, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let out = tokio::task::spawn_blocking(move || ordo_cli::operator::run("verify", &args))
            .await
            .expect("verify ran");
        (out.code, out.report)
    }
}

impl Drop for World {
    fn drop(&mut self) {
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
        let _ = std::fs::remove_dir_all(&self.blobs);
    }
}

/// One `ordo-host` process. Stopped (SIGTERM, then killed after 40 s) when
/// dropped or by [`Host::stop`].
struct Host {
    port: u16,
    child: Option<Child>,
    log: PathBuf,
}

impl Host {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status();
        let told = Instant::now();
        while told.elapsed() < Duration::from_secs(40) {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    /// One HTTP/1.1 request on a fresh socket: `(status, head, body bytes)`.
    fn raw(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(30))).ok();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        s.write_all(req.as_bytes()).expect("write");
        s.write_all(body).expect("write body");
        let mut raw = Vec::new();
        let _ = s.read_to_end(&mut raw);
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or(raw.len());
        let head = String::from_utf8_lossy(&raw[..split]).to_string();
        let rest = raw.get(split + 4..).unwrap_or_default().to_vec();
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        (status, head, rest)
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, Value) {
        let (status, head, bytes) = self.raw(method, path, headers, body.as_bytes());
        let text = String::from_utf8_lossy(&bytes);
        let json = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
        (status, head, json)
    }

    /// A POST to `/auth/...`, as the SDK sends it: `(status, head, body)`.
    fn auth(&self, path: &str, body: Value, token: Option<&str>) -> (u16, String, Value) {
        let bearer = token.map(|t| format!("Bearer {t}"));
        let mut headers = vec![
            ("Content-Type", "application/json"),
            ("Origin", PAGE),
            ("Accept", "application/json"),
        ];
        if let Some(b) = &bearer {
            headers.push(("Authorization", b));
        }
        self.send("POST", path, &headers, &body.to_string())
    }

    /// `/op/<name>`: reads carry their parameters in the query, writes in a
    /// JSON body with a fresh idempotency key.
    fn op(
        &self,
        space: &str,
        token: &str,
        method: &str,
        name: &str,
        params: Value,
    ) -> (u16, Value) {
        static KEY: AtomicU64 = AtomicU64::new(1);
        let key = format!("upg-{}", KEY.fetch_add(1, Ordering::SeqCst));
        self.op_keyed(space, token, method, name, params, &key)
    }

    fn op_keyed(
        &self,
        space: &str,
        token: &str,
        method: &str,
        name: &str,
        params: Value,
        key: &str,
    ) -> (u16, Value) {
        let bearer = format!("Bearer {token}");
        let mut headers = vec![
            ("X-Ordo-Space", space),
            ("Authorization", bearer.as_str()),
            ("Origin", PAGE),
            ("Accept", "application/json"),
        ];
        if method != "GET" {
            headers.push(("Idempotency-Key", key));
        }
        let (path, body) = if method == "POST" {
            headers.push(("Content-Type", "application/json"));
            (format!("/op/{name}"), params.to_string())
        } else {
            let q: Vec<String> = params
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| {
                            let v = v.as_str().map(str::to_string).unwrap_or(v.to_string());
                            format!("{k}={v}")
                        })
                        .collect()
                })
                .unwrap_or_default();
            if q.is_empty() {
                (format!("/op/{name}"), String::new())
            } else {
                (format!("/op/{name}?{}", q.join("&")), String::new())
            }
        };
        let (status, _, json) = self.send(method, &path, &headers, &body);
        (status, json)
    }

    /// Every row of a read, page after page (the door serves 50 a page).
    fn rows(&self, space: &str, token: &str, name: &str) -> Result<Vec<Value>, String> {
        let mut all = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..20 {
            let params = match &page {
                Some(p) => json!({ "$page": p }),
                None => json!({}),
            };
            let (status, body) = self.op(space, token, "GET", name, params);
            if status != 200 {
                return Err(format!("{name} in {space}: {status} {body}"));
            }
            all.extend(body["rows"].as_array().cloned().unwrap_or_default());
            match body["next"].as_str() {
                Some(n) => page = Some(n.to_string()),
                None => return Ok(all),
            }
        }
        Err(format!("{name} in {space}: more than 20 pages"))
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
        if std::thread::panicking() {
            let _ = std::fs::write(
                std::env::temp_dir().join("ordo-upgrade-last.log"),
                self.log(),
            );
            eprintln!("--- the server log is in ordo-upgrade-last.log ---");
        }
        let _ = std::fs::remove_file(&self.log);
    }
}

// ── People and their ways in ────────────────────────────────────────────

fn device(seed: u8, name: &str) -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(seed),
        credential_id: name.as_bytes().to_vec(),
        rp_id: "localhost".to_string(),
        origin: PAGE.to_string(),
        ..TestAuthenticator::es256()
    }
}

/// What a page holds after a sign-in.
#[derive(Clone, Debug)]
struct Session {
    token: String,
    space: String,
    principal: String,
    raw: Value,
    /// `name=value` of the renewal cookie the sign-in set, if any.
    cookie: Option<String>,
}

fn principal_of(token: &str) -> String {
    let payload = token.split('.').next().expect("a payload");
    let text = String::from_utf8(b64url_decode(payload).expect("base64url")).expect("text");
    text.split('|').nth(1).expect("a principal").to_string()
}

fn cookie_in(head: &str) -> Option<String> {
    head.lines()
        .find(|l| l.to_ascii_lowercase().starts_with("set-cookie:"))
        .and_then(|l| l.split_once(':'))
        .and_then(|(_, v)| v.trim().split(';').next().map(str::to_string))
        .filter(|c| !c.ends_with('='))
}

/// Run `probe` until it answers `Some`, for at most `secs` seconds.
fn within<T>(secs: u64, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(
            started.elapsed() < Duration::from_secs(secs),
            "still waiting after {secs} s: {what}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The sign-in's answer once the person's tenant is made, as the SDK's
/// `waitForTenant` reads it.
fn tenant_ready(h: &Host, mut done: Value) -> Value {
    if done["provisioning"] != "in progress" {
        return done;
    }
    let bearer = format!("Bearer {}", done["token"].as_str().expect("a token"));
    let headers = [("Origin", PAGE), ("Authorization", bearer.as_str())];
    let last = within(60, "the tenant is made", || {
        let (status, _, said) = h.send("GET", "/auth/provisioning", &headers, "");
        assert_eq!(status, 200, "{said}");
        match said["state"].as_str() {
            Some("ready") => Some(said),
            Some("provisioning") => None,
            _ => panic!("the tenant was not made: {said}"),
        }
    });
    done["space"] = last["space"].clone();
    done
}

fn session_of(h: &Host, status: u16, head: &str, done: Value) -> Result<Session, String> {
    if status != 200 {
        return Err(format!("{status} {done}"));
    }
    let done = tenant_ready(h, done);
    let token = done["token"].as_str().ok_or("no token")?.to_string();
    Ok(Session {
        principal: principal_of(&token),
        space: done["space"].as_str().unwrap_or("").to_string(),
        token,
        cookie: cookie_in(head),
        raw: done,
    })
}

fn passkey_sign_up(h: &Host, device: &TestAuthenticator, display: &str) -> Session {
    let (status, _, options) = h.auth(
        "/auth/register",
        json!({"method": "passkey", "display": display}),
        None,
    );
    assert_eq!(status, 200, "{options}");
    let challenge = options["challenge"].as_str().expect("challenge");
    let CompleteRequest::PasskeyRegistration {
        challenge: c,
        client_data_json,
        attestation_object,
    } = device.register(challenge)
    else {
        panic!("not a registration");
    };
    let (status, head, done) = h.auth(
        "/auth/complete",
        json!({
            "method": "passkey", "kind": "passkey_registration", "challenge": c.0,
            "client_data_json": b64url_encode(&client_data_json),
            "attestation_object": b64url_encode(&attestation_object),
            "display": display,
        }),
        None,
    );
    session_of(h, status, &head, done).expect("a passkey sign-up")
}

fn passkey_sign_in(h: &Host, device: &TestAuthenticator) -> Result<Session, String> {
    let (status, _, options) = h.auth("/auth/begin", json!({"method": "passkey"}), None);
    if status != 200 {
        return Err(format!("begin: {status} {options}"));
    }
    let challenge = options["challenge"].as_str().ok_or("no challenge")?;
    let CompleteRequest::PasskeyAssertion {
        challenge: c,
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
    } = device.assert(challenge)
    else {
        panic!("not an assertion");
    };
    let (status, head, done) = h.auth(
        "/auth/complete",
        json!({
            "method": "passkey", "kind": "passkey_assertion", "challenge": c.0,
            "credential_id": b64url_encode(&credential_id),
            "client_data_json": b64url_encode(&client_data_json),
            "authenticator_data": b64url_encode(&authenticator_data),
            "signature": b64url_encode(&signature),
        }),
        None,
    );
    session_of(h, status, &head, done)
}

/// The code in the relay's `n`th message.
fn code_in_mail(relay: &mail_support::FakeRelay, n: usize) -> String {
    let data = within(30, "the mail worker sends the code", || {
        relay.sessions().get(n).and_then(|s| s.data.clone())
    });
    let start = data.find("code is ").expect("the code line") + "code is ".len();
    data[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

fn email_sign_in(
    h: &Host,
    relay: &mail_support::FakeRelay,
    address: &str,
    device: Option<&str>,
) -> Result<Session, String> {
    let n = relay.sessions().len();
    let mut begin = json!({"method": "email", "identifier": address});
    if let Some(marker) = device {
        begin["device"] = json!(marker);
    }
    let (status, _, begun) = h.auth("/auth/begin", begin, None);
    if status != 200 {
        return Err(format!("begin: {status} {begun}"));
    }
    let code = code_in_mail(relay, n);
    let (status, head, done) = h.auth(
        "/auth/complete",
        json!({
            "method": "email", "kind": "email_code",
            "challenge": begun["challenge"], "binder": begun["binder"], "code": code,
        }),
        None,
    );
    session_of(h, status, &head, done)
}

fn password_sign_in(h: &Host, identifier: &str, password: &str) -> Result<Session, String> {
    let (status, _, begun) = h.auth(
        "/auth/begin",
        json!({"method": "password", "identifier": identifier}),
        None,
    );
    if status != 200 {
        return Err(format!("begin: {status} {begun}"));
    }
    let (status, head, done) = h.auth(
        "/auth/complete",
        json!({
            "method": "password", "kind": "password", "challenge": begun["challenge"],
            "identifier": identifier, "password": password, "display": "Carl",
        }),
        None,
    );
    session_of(h, status, &head, done)
}

/// One more second each call, so "newest first" is decided by the data.
fn stamp() -> String {
    static SECONDS: AtomicU64 = AtomicU64::new(0);
    let n = SECONDS.fetch_add(1, Ordering::SeqCst);
    format!(
        "2026-10-27T{:02}:{:02}:{:02}Z",
        1 + n / 3600,
        (n / 60) % 60,
        n % 60
    )
}

fn titles(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["title"].as_str().unwrap_or("").to_string())
        .collect()
}

fn code_of(body: &Value) -> String {
    body["error"]["code"].as_str().unwrap_or("").to_string()
}

/// The first row of a write's answer.
fn first_row(status: u16, out: &Value, what: &str) -> Value {
    assert_eq!(status, 200, "{what}: {out}");
    out["rows"][0].clone()
}

// ── A live feed, read on its own thread ─────────────────────────────────

/// A `?live=1` feed on a raw socket. A thread reads it into `heard` until it
/// ends or is told to stop.
struct Feed {
    heard: Arc<Mutex<String>>,
    ended: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Feed {
    fn open(h: &Host, s: &Session, op: &str) -> Feed {
        let mut sock = TcpStream::connect(("127.0.0.1", h.port)).expect("connect");
        let request = format!(
            "GET /op/{op}?live=1 HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\
             X-Ordo-Space: {}\r\nAuthorization: Bearer {}\r\nOrigin: {PAGE}\r\n\r\n",
            s.space, s.token
        );
        sock.write_all(request.as_bytes()).expect("write");
        sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
        let heard = Arc::new(Mutex::new(String::new()));
        let ended = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (sink, end, halt) = (heard.clone(), ended.clone(), stop.clone());
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            // Bounded: no feed is read for more than ten minutes.
            let until = Instant::now() + Duration::from_secs(600);
            while !halt.load(Ordering::SeqCst) && Instant::now() < until {
                match sock.read(&mut buf) {
                    Ok(0) => {
                        end.store(true, Ordering::SeqCst);
                        return;
                    }
                    Ok(n) => sink
                        .lock()
                        .unwrap()
                        .push_str(&String::from_utf8_lossy(&buf[..n])),
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => {
                        end.store(true, Ordering::SeqCst);
                        return;
                    }
                }
            }
        });
        let feed = Feed {
            heard,
            ended,
            stop,
            reader: Some(reader),
        };
        within(30, "the feed's first load", || {
            feed.text().contains("\"rows\"").then_some(())
        });
        feed
    }

    fn text(&self) -> String {
        self.heard.lock().unwrap().clone()
    }

    fn len(&self) -> usize {
        self.heard.lock().unwrap().len()
    }

    fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

// ── The state made before the upgrade ───────────────────────────────────

struct Seeded {
    ada: Session,
    bo: Session,
    kim: Session,
    carl: Session,
    /// Kim's device marker, which a later emailed code is sent with.
    kim_device: String,
    /// Every title in Ada's own list, newest first.
    ada_titles: Vec<String>,
    journal_body: String,
    carl_journal: String,
    attachment: String,
    file_handle: String,
    link_secret: String,
    link_token: String,
    agent_token: String,
    cursor: String,
    /// What the pages after the cursor held when it was handed out.
    cursor_rest: Vec<String>,
    idem_key: String,
    idem_params: Value,
    idem_answer: Value,
}

fn ada_device() -> TestAuthenticator {
    device(41, "ada-upgrade")
}

fn bo_device() -> TestAuthenticator {
    device(42, "bo-upgrade")
}

fn create_note(h: &Host, s: &Session, title: &str) -> String {
    let (status, out) = h.op(
        &s.space,
        &s.token,
        "POST",
        "createNote",
        json!({"title": title, "body": format!("{title} body"), "created": stamp()}),
    );
    first_row(status, &out, "createNote")["created"]
        .as_str()
        .expect("the created id")
        .to_string()
}

/// Wait until the door serves `s` in its tenant.
fn settle(h: &Host, s: &Session) {
    within(30, "the door serves the new tenant", || {
        let (status, _) = h.op(&s.space, &s.token, "GET", "myNotes", json!({}));
        (status == 200).then_some(())
    });
}

fn seed(h: &Host, w: &World) -> Seeded {
    // Passkeys. Ada's sign-up sets a renewal cookie.
    let ada = passkey_sign_up(h, &ada_device(), "Ada");
    let bo = passkey_sign_up(h, &bo_device(), "Bo");
    settle(h, &ada);
    settle(h, &bo);
    assert!(ada.cookie.is_some(), "the sign-up set no renewal cookie");

    // An emailed code.
    let kim = email_sign_in(h, &w.relay, KIM_ID, None).expect("Kim's emailed code");
    let kim_device = kim.raw["device"]
        .as_str()
        .expect("a device marker")
        .to_string();
    settle(h, &kim);
    create_note(h, &kim, "Kim's note");

    // Ada's notes: one to share, one to keep, and more than a page.
    let groceries = create_note(h, &ada, "Groceries");
    create_note(h, &ada, "Diary");
    for i in 0..PAGED {
        create_note(h, &ada, &format!("p{i:02}"));
    }

    // Ada makes Carl a member with a password of his own.
    let (status, out) = h.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.space.createPerson",
        json!({"display": "Carl", "role": "editor", "purpose": "password"}),
    );
    let made = first_row(status, &out, "std.space.createPerson");
    let ticket = made["ticket"].as_str().expect("a ticket").to_string();
    let (status, _, said) = h.auth(
        "/auth/password/redeem",
        json!({"ticket": ticket, "identifier": CARL_ID, "password": CARL_PASSWORD}),
        None,
    );
    assert_eq!(status, 204, "the ticket was not redeemed: {said}");
    let mut carl = password_sign_in(h, CARL_ID, CARL_PASSWORD).expect("Carl's password");
    carl.space = ada.space.clone();
    let carl_journal = "Carl's first day".to_string();
    let (status, out) = h.op(
        &ada.space,
        &carl.token,
        "POST",
        "writeJournal",
        json!({"body": carl_journal}),
    );
    first_row(status, &out, "Carl writes in Ada's tenant");

    // Ada's journal entry.
    let journal_body = "Ada's private thought".to_string();
    let (status, out) = h.op(
        &ada.space,
        &ada.token,
        "POST",
        "writeJournal",
        json!({"body": journal_body}),
    );
    first_row(status, &out, "writeJournal");

    // A file on the shared note.
    let (status, out) = h.op(
        &ada.space,
        &ada.token,
        "POST",
        "addAttachment",
        json!({"note": groceries, "name": "list.txt", "created": stamp()}),
    );
    let attachment = first_row(status, &out, "addAttachment")["created"]
        .as_str()
        .expect("attachment id")
        .to_string();
    let bearer = format!("Bearer {}", ada.token);
    let (status, _, said) = h.raw(
        "PUT",
        &format!("/blob/attachments/file/{attachment}"),
        &[
            ("X-Ordo-Space", &ada.space),
            ("Authorization", &bearer),
            ("Origin", PAGE),
            ("Content-Type", "text/plain"),
        ],
        FILE.as_bytes(),
    );
    assert_eq!(status, 204, "upload: {}", String::from_utf8_lossy(&said));
    let files = h.rows(&ada.space, &ada.token, "myFiles").expect("myFiles");
    let file_handle = files[0]["file"].as_str().expect("a handle").to_string();
    assert_eq!(read_file(h, &ada, &file_handle).as_deref(), Ok(FILE));

    // A share of Groceries, accepted by Bo.
    let (status, out) = h.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.space.share",
        json!({
            "ops": "myNotes",
            "rows": json!([{"entity": "notes", "id": groceries, "level": "read"}]).to_string(),
            "expires": "365d",
            "label": "Groceries",
        }),
    );
    let share_ticket = first_row(status, &out, "std.space.share")["ticket"]
        .as_str()
        .expect("a ticket")
        .to_string();
    let (status, _, accepted) = h.auth(
        "/auth/share/accept",
        json!({"ticket": share_ticket, "display_name": "Bo"}),
        Some(&bo.token),
    );
    assert_eq!(status, 200, "{accepted}");
    assert!(accepted["seat"].is_string(), "{accepted}");
    assert_eq!(
        window_titles(h, &bo, &ada.space),
        Ok(vec!["Groceries".to_string()])
    );

    // An open link on the same note.
    let (status, out) = h.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.space.shareLink",
        json!({
            "ops": "myNotes",
            "rows": json!([{"entity": "notes", "id": groceries, "level": "read"}]).to_string(),
            "expires": "7d",
            "label": "for the shop",
        }),
    );
    let link_secret = first_row(status, &out, "std.space.shareLink")["link"]
        .as_str()
        .expect("a link")
        .to_string();
    let link_token = open_link(h, &link_secret).expect("the link opens");
    assert_eq!(
        h.rows(&ada.space, &link_token, "myNotes")
            .map(|r| titles(&r)),
        Ok(vec!["Groceries".to_string()])
    );

    // A delegation: the agent asks, Ada approves, the agent redeems.
    let (status, _, asked) = h.auth(
        "/auth/delegation/request",
        json!({"space": ada.space, "agent": AGENT, "person": ada.principal, "ops": ["myNotes"]}),
        None,
    );
    assert_eq!(status, 201, "{asked}");
    let request = asked["request"].as_str().expect("request").to_string();
    let binder = asked["binder"].as_str().expect("binder").to_string();
    let (status, out) = h.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.delegation.approve",
        json!({"request": request}),
    );
    assert_eq!(status, 200, "approve: {out}");
    let (status, _, redeemed) = h.auth(
        "/auth/delegation/redeem",
        json!({"request": request, "binder": binder}),
        None,
    );
    assert_eq!(status, 200, "{redeemed}");
    let agent_token = redeemed["token"].as_str().expect("token").to_string();
    if let Err(e) = h.rows(&ada.space, &agent_token, "myNotes") {
        panic!("the agent cannot read before the upgrade: {e}\nredeemed: {redeemed}");
    }

    // The first page of the paged list, and its cursor.
    let (status, page) = h.op(&ada.space, &ada.token, "GET", "notesPage", json!({}));
    assert_eq!(status, 200, "{page}");
    let first_page = page["rows"].as_array().expect("rows").len();
    assert_eq!(first_page, 50, "a page is 50 rows");
    let cursor = page["next"].as_str().expect("a cursor").to_string();
    let listed = h
        .rows(&ada.space, &ada.token, "notesPage")
        .expect("notesPage");
    let cursor_rest = titles(&listed)[50..].to_vec();

    // A write that answered, and its key.
    let idem_key = "upgrade-idempotent-write".to_string();
    let idem_params = json!({"title": "Written once", "body": "", "created": stamp()});
    let (status, idem_answer) = h.op_keyed(
        &ada.space,
        &ada.token,
        "POST",
        "createNote",
        idem_params.clone(),
        &idem_key,
    );
    assert_eq!(status, 200, "{idem_answer}");

    let ada_titles = titles(&h.rows(&ada.space, &ada.token, "myNotes").expect("myNotes"));
    assert_eq!(ada_titles.len(), PAGED + 3);
    Seeded {
        ada,
        bo,
        kim,
        carl,
        kim_device,
        ada_titles,
        journal_body,
        carl_journal,
        attachment,
        file_handle,
        link_secret,
        link_token,
        agent_token,
        cursor,
        cursor_rest,
        idem_key,
        idem_params,
        idem_answer,
    }
}

fn read_file(h: &Host, s: &Session, handle: &str) -> Result<String, String> {
    let bearer = format!("Bearer {}", s.token);
    let (status, _, bytes) = h.raw(
        "GET",
        &format!("/blob/{handle}"),
        &[
            ("X-Ordo-Space", &s.space),
            ("Authorization", &bearer),
            ("Origin", PAGE),
        ],
        b"",
    );
    let text = String::from_utf8_lossy(&bytes).to_string();
    if status != 200 {
        return Err(format!("{status} {text}"));
    }
    // The body may come chunked; the bytes are in it whole.
    if text.contains(FILE) {
        Ok(FILE.to_string())
    } else {
        Err(format!("other bytes: {text:?}"))
    }
}

/// Bo's window on `space`, and the titles read through it.
fn window_titles(h: &Host, bo: &Session, space: &str) -> Result<Vec<String>, String> {
    let (status, _, w) = h.auth("/auth/window", json!({"space": space}), Some(&bo.token));
    if status != 200 {
        return Err(format!("window: {status} {w}"));
    }
    let token = w["token"].as_str().ok_or("no window token")?;
    h.rows(space, token, "myNotes").map(|r| titles(&r))
}

fn open_link(h: &Host, secret: &str) -> Result<String, String> {
    let (status, _, out) = h.auth("/auth/link", json!({"link": secret}), None);
    if status != 200 {
        return Err(format!("{status} {out}"));
    }
    out["token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("no token: {out}"))
}

// ── The checks after the upgrade ────────────────────────────────────────

/// One named check and what it found.
struct Check {
    name: &'static str,
    outcome: Result<String, String>,
}

fn check(name: &'static str, outcome: Result<String, String>) -> Check {
    Check { name, outcome }
}

fn expect_eq<T: PartialEq + std::fmt::Debug>(got: T, want: T) -> Result<String, String> {
    if got == want {
        Ok(String::new())
    } else {
        Err(format!("got {got:?}, want {want:?}"))
    }
}

/// Everything made before the upgrade, checked after it.
async fn verify(h: &Host, w: &World, s: &Seeded) -> Vec<Check> {
    let mut out = Vec::new();

    // Sessions held across the upgrade.
    for (name, who) in [
        ("ada's session token still reads", &s.ada),
        ("bo's session token still reads", &s.bo),
        ("kim's session token still reads", &s.kim),
    ] {
        out.push(check(
            name,
            h.rows(&who.space, &who.token, "myNotes")
                .map(|_| String::new()),
        ));
    }
    out.push(check(
        "carl's session token still reads in ada's tenant",
        h.rows(&s.ada.space, &s.carl.token, "myJournal")
            .and_then(|r| {
                let bodies: Vec<&str> = r.iter().filter_map(|r| r["body"].as_str()).collect();
                expect_eq(bodies, vec![s.carl_journal.as_str()])
            }),
    ));

    // The same credentials sign in, to the same person and tenant.
    out.push(check(
        "ada signs in with the same passkey",
        passkey_sign_in(h, &ada_device()).and_then(|a| {
            expect_eq(
                (a.principal, a.space),
                (s.ada.principal.clone(), s.ada.space.clone()),
            )
        }),
    ));
    out.push(check(
        "bo signs in with the same passkey",
        passkey_sign_in(h, &bo_device()).and_then(|b| {
            expect_eq(
                (b.principal, b.space),
                (s.bo.principal.clone(), s.bo.space.clone()),
            )
        }),
    ));
    out.push(check(
        "kim signs in with an emailed code",
        email_sign_in(h, &w.relay, KIM_ID, Some(&s.kim_device)).and_then(|k| {
            expect_eq(
                (k.principal, k.space),
                (s.kim.principal.clone(), s.kim.space.clone()),
            )
        }),
    ));
    out.push(check(
        "carl signs in with his password",
        password_sign_in(h, CARL_ID, CARL_PASSWORD)
            .and_then(|c| expect_eq(c.principal, s.carl.principal.clone())),
    ));

    // The renewal cookie brings Ada back, as a reload does.
    let cookie = s.ada.cookie.clone().unwrap_or_default();
    let (status, _, renewed) = h.send(
        "POST",
        "/auth/refresh",
        &[("Cookie", &cookie), ("Origin", PAGE)],
        "",
    );
    out.push(check(
        "ada's renewal cookie still signs her in",
        if status == 200 {
            renewed["token"]
                .as_str()
                .map(principal_of)
                .ok_or_else(|| format!("no token: {renewed}"))
                .and_then(|p| expect_eq(p, s.ada.principal.clone()))
        } else {
            Err(format!("{status} {renewed}"))
        },
    ));

    // Rows.
    out.push(check(
        "ada's notes are all there",
        h.rows(&s.ada.space, &s.ada.token, "myNotes")
            .and_then(|r| expect_eq(titles(&r), s.ada_titles.clone())),
    ));
    out.push(check(
        "kim's note is there",
        h.rows(&s.kim.space, &s.kim.token, "myNotes")
            .and_then(|r| expect_eq(titles(&r), vec!["Kim's note".to_string()])),
    ));
    out.push(check(
        "ada's journal is hers alone",
        h.rows(&s.ada.space, &s.ada.token, "myJournal")
            .and_then(|r| {
                let bodies: Vec<&str> = r.iter().filter_map(|r| r["body"].as_str()).collect();
                expect_eq(bodies, vec![s.journal_body.as_str()])
            }),
    ));

    // The file.
    out.push(check(
        "the file's row and bytes are there",
        h.rows(&s.ada.space, &s.ada.token, "myFiles").and_then(|r| {
            expect_eq(
                (r.len(), r[0]["id"].as_str().map(str::to_string)),
                (1, Some(s.attachment.clone())),
            )?;
            read_file(h, &s.ada, &s.file_handle).map(|_| String::new())
        }),
    ));

    // The share: Bo's window reads Groceries and nothing else.
    out.push(check(
        "bo's accepted share still reads the one note",
        window_titles(h, &s.bo, &s.ada.space)
            .and_then(|t| expect_eq(t, vec!["Groceries".to_string()])),
    ));

    // The share link: the token handed out before, and the link opened again.
    out.push(check(
        "the link token handed out before still reads",
        h.rows(&s.ada.space, &s.link_token, "myNotes")
            .and_then(|r| expect_eq(titles(&r), vec!["Groceries".to_string()])),
    ));
    out.push(check(
        "the share link opens again",
        open_link(h, &s.link_secret).and_then(|t| {
            h.rows(&s.ada.space, &t, "myNotes")
                .and_then(|r| expect_eq(titles(&r), vec!["Groceries".to_string()]))
        }),
    ));

    // The delegation.
    out.push(check(
        "the agent's delegated token still reads",
        h.rows(&s.ada.space, &s.agent_token, "myNotes")
            .and_then(|r| expect_eq(titles(&r), s.ada_titles.clone())),
    ));

    // The page cursor: it continues, or it is refused with `version_skew`
    // and page one serves again (what the SDK does with that refusal).
    let (status, page) = h.op(
        &s.ada.space,
        &s.ada.token,
        "GET",
        "notesPage",
        json!({"$page": s.cursor}),
    );
    out.push(check(
        "the page cursor continues or is refused as version_skew",
        match (status, code_of(&page).as_str()) {
            (200, _) => page["rows"]
                .as_array()
                .map(|r| titles(r))
                .ok_or_else(|| format!("{page}"))
                .and_then(|t| expect_eq(t, s.cursor_rest.clone()))
                .map(|_| "continued".to_string()),
            (409, "version_skew") => {
                let (status, again) =
                    h.op(&s.ada.space, &s.ada.token, "GET", "notesPage", json!({}));
                if status == 200 {
                    Ok("refused as version_skew; page one serves".to_string())
                } else {
                    Err(format!("page one after version_skew: {status} {again}"))
                }
            }
            _ => Err(format!("{status} {page}")),
        },
    ));

    // The idempotency key: the same write with the same key answers what it
    // answered, and writes nothing more.
    let (status, again) = h.op_keyed(
        &s.ada.space,
        &s.ada.token,
        "POST",
        "createNote",
        s.idem_params.clone(),
        &s.idem_key,
    );
    out.push(check(
        "a write retried with its idempotency key is not written twice",
        if status != 200 {
            Err(format!("{status} {again}"))
        } else {
            h.rows(&s.ada.space, &s.ada.token, "myNotes").and_then(|r| {
                let n = titles(&r).iter().filter(|t| *t == "Written once").count();
                expect_eq(
                    (n, again["rows"][0]["created"].clone()),
                    (1, s.idem_answer["rows"][0]["created"].clone()),
                )
            })
        },
    ));
    out
}

/// The chains, read once the server has stopped and so has written out
/// every security event it held (the drain writes on a 30 s tick, and on
/// stop): the deployment's security stream, every sign-in link against it,
/// and Ada's tenant's command chain and security stream.
async fn chains(w: &World, s: &Seeded) -> Vec<Check> {
    let mut out = Vec::new();
    let (code, report) = w.verify(&["--deployment", "--db", &w.db]).await;
    out.push(check(
        "the deployment's security stream verifies",
        if code == 0 {
            Ok(String::new())
        } else {
            Err(report)
        },
    ));
    let (code, report) = w.verify(&["--identity", "--db", &w.db]).await;
    out.push(check(
        "every sign-in link is explained by the security stream",
        if code == 0 {
            Ok(String::new())
        } else {
            Err(report)
        },
    ));
    let (code, report) = w
        .verify(&[
            "--space",
            &s.ada.space,
            "--db",
            &s.ada.space,
            "--control-db",
            &w.db,
            "--from-genesis",
        ])
        .await;
    out.push(check(
        "ada's tenant's command chain and security stream verify",
        if code == 0 {
            Ok(String::new())
        } else {
            Err(report)
        },
    ));
    out
}

/// The live feed across the upgrade: after a write it carries the change,
/// or it has ended (cleanly: an error frame, or the connection closed) and
/// a new one carries it, as the SDK reconnects.
fn feed_after(h: &Host, s: &Seeded, feed: &Feed) -> Check {
    let mark = feed.len();
    create_note(h, &s.ada, "Written after the upgrade");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let text = feed.text();
        if text.len() > mark && text[mark..].contains("event: invalidate") {
            return check("the open feed", Ok("carried on".into()));
        }
        if feed.ended() || (text.len() > mark && text[mark..].contains("event: error")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let text = feed.text();
    let how = if feed.ended() {
        "ended: the connection closed".to_string()
    } else if text[mark..].contains("event: error") {
        format!("ended with an error frame: {}", &text[mark..])
    } else {
        return check(
            "the open feed",
            Err(format!(
                "heard nothing after a write, and did not end: {:?}",
                &text[mark..]
            )),
        );
    };
    // A feed opened again carries a later write.
    let again = Feed::open(h, &s.ada, "myNotes");
    let mark = again.len();
    create_note(h, &s.ada, "Written after the feed reopened");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let text = again.text();
        if text.len() > mark && text[mark..].contains("event: invalidate") {
            return check("the open feed", Ok(format!("{how}; a new feed carries on")));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    check(
        "the open feed",
        Err(format!("{how}; a new feed heard nothing after a write")),
    )
}

/// What version 1.1 adds works on version 1's rows, and the new guard
/// clause gives nobody a row they could not read before.
fn v1_1_works(h: &Host, s: &Seeded) -> Vec<Check> {
    let mut out = Vec::new();
    let note = h
        .rows(&s.ada.space, &s.ada.token, "myNotes")
        .ok()
        .and_then(|r| {
            r.iter()
                .find(|r| r["title"] == "Groceries")
                .map(|r| (r["id"].clone(), r["rev"].clone()))
        });
    out.push(check(
        "a v1 note takes the new optional field",
        match &note {
            None => Err("Groceries is not listed".into()),
            Some((id, rev)) => {
                let (status, body) = h.op(
                    &s.ada.space,
                    &s.ada.token,
                    "POST",
                    "colorNote",
                    json!({"id": id, "rev": rev, "color": "green"}),
                );
                expect_eq(status, 200).map_err(|e| format!("{e}: {body}"))
            }
        },
    ));
    out.push(check(
        "the new entity and its ops work on a v1 note",
        match &note {
            None => Err("Groceries is not listed".into()),
            Some((id, _)) => {
                let (status, body) = h.op(
                    &s.ada.space,
                    &s.ada.token,
                    "POST",
                    "labelNote",
                    json!({"note": id, "name": "shopping"}),
                );
                if status != 200 {
                    Err(format!("{status} {body}"))
                } else {
                    h.rows(&s.ada.space, &s.ada.token, "myLabels")
                        .and_then(|r| expect_eq(r.len(), 1))
                }
            }
        },
    ));
    out.push(check(
        "the widened journal guard gives carl nothing new",
        h.rows(&s.ada.space, &s.carl.token, "myJournal")
            .and_then(|r| {
                let bodies: Vec<&str> = r.iter().filter_map(|r| r["body"].as_str()).collect();
                expect_eq(bodies, vec![s.carl_journal.as_str()])
            }),
    ));
    out
}

// ── Backend functions across an upgrade ──────────────────────────────────

/// The principal `ORDO_DEPLOYMENT_OWNERS` names. It sets the secret.
const DEPLOYMENT_OWNER: &str = "upgrade_owner";
/// The host `probe` may reach and `probeKey` is bound to. It resolves to
/// loopback where the system resolves `*.localhost`; elsewhere the fetch
/// fails at resolution. Either way it fails after the value opens.
const FN_HOST: &str = "upgrade.localhost";
/// The secret's value. No answer, log, record or stored byte may hold it.
const FN_SECRET: &str = "sk-upgrade-0123456789abcdef";
/// The reason words a fetch gets when the secret did not open, or before
/// the kernel tries to open it.
const SECRET_WORDS: &[&str] = &[
    "secret_not_set",
    "secret_unknown",
    "secret_not_listed",
    "secret_host_mismatch",
    "secret_store_unavailable",
    "host_not_allowed",
];

/// Version `version` of the fixture with its functions: `app.ordo`, the
/// matching `functions/<version>/functions.ordo` and its `fn/` code, put
/// in one directory as `ordo apply` reads a program.
fn with_functions(version: &str) -> PathBuf {
    let src = fixture("functions").join(version);
    let dir = std::env::temp_dir().join(format!("ordo-upg-fn-{version}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fn")).expect("the program directory");
    std::fs::copy(fixture(version).join("app.ordo"), dir.join("app.ordo")).expect("app.ordo");
    std::fs::copy(src.join("functions.ordo"), dir.join("functions.ordo")).expect("functions.ordo");
    for f in std::fs::read_dir(src.join("fn")).expect("fn/") {
        let f = f.expect("a code file").path();
        std::fs::copy(&f, dir.join("fn").join(f.file_name().expect("a name"))).expect("code");
    }
    dir
}

/// A bearer token for the deployment owner, with a passkey touch just now,
/// minted under [`TOKEN_KEY`] as the server mints one.
fn deployment_owner_token() -> String {
    let raw: Vec<u8> = (0..TOKEN_KEY.len() / 2)
        .map(|i| u8::from_str_radix(&TOKEN_KEY[2 * i..2 * i + 2], 16).expect("hex"))
        .collect();
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&key),
        &ordo_identity::TokenSpec {
            principal: ordo_identity::principal::PrincipalId(DEPLOYMENT_OWNER.into()),
            expires: ordo_identity::principal::Timestamp(now + 3600),
            client: None,
            user_presence_at: Some(ordo_identity::principal::Timestamp(now)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// A listener on loopback that counts connections and closes each before
/// TLS. `probe` dials it where `upgrade.localhost` resolves, so the
/// secret's value is never sent.
struct FarEnd {
    port: u16,
    hits: Arc<AtomicU64>,
}

impl FarEnd {
    fn start() -> FarEnd {
        let v6 = std::net::TcpListener::bind("[::1]:0").ok();
        let port = v6
            .as_ref()
            .map_or(0, |l| l.local_addr().expect("addr").port());
        let v4 = std::net::TcpListener::bind(("127.0.0.1", port))
            .or_else(|_| std::net::TcpListener::bind(("127.0.0.1", 0)))
            .expect("a loopback port");
        let port = v4.local_addr().expect("addr").port();
        let v6 = v6.filter(|l| l.local_addr().map(|a| a.port()).ok() == Some(port));
        let hits = Arc::new(AtomicU64::new(0));
        for l in [Some(v4), v6].into_iter().flatten() {
            let hits = hits.clone();
            std::thread::spawn(move || {
                for c in l.incoming() {
                    hits.fetch_add(1, Ordering::SeqCst);
                    drop(c);
                }
            });
        }
        FarEnd { port, hits }
    }
}

/// `POST /fn/<name>` as the page sends it: `(status, body)`.
fn call_fn(h: &Host, s: &Session, name: &str, params: Value) -> (u16, Value) {
    let bearer = format!("Bearer {}", s.token);
    let headers = [
        ("X-Ordo-Space", s.space.as_str()),
        ("Authorization", bearer.as_str()),
        ("Origin", PAGE),
        ("Accept", "application/json"),
        ("Content-Type", "application/json"),
    ];
    let (status, _, body) = h.send(
        "POST",
        &format!("/fn/{name}"),
        &headers,
        &params.to_string(),
    );
    (status, body)
}

/// What the functions left before the upgrade.
struct FnSeed {
    far: FarEnd,
    /// Every `$fnruns` row before the upgrade: seq, function, version,
    /// space and outcome.
    runs: Vec<Vec<Option<String>>>,
}

fn fn_runs(w: &World) -> Result<Vec<Vec<Option<String>>>, String> {
    Pg {
        db: w.db.clone(),
        ..Pg::default()
    }
    .query(
        "SELECT \"seq\", \"function\", \"version\", \"space\", \"outcome\" FROM \"$fnruns\" \
         ORDER BY \"seq\"",
    )
}

/// `probe`'s answer in `s`'s tenant once it answers `code`, for at most
/// `secs` seconds. A server takes up a new version's functions as it swaps
/// to the version.
fn probe_answers(
    h: &Host,
    s: &Session,
    far: &FarEnd,
    code: &str,
    secs: u64,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let (status, body) = call_fn(h, s, "probe", json!({ "port": far.port }));
        if status == 200 && body["result"]["code"] == code {
            return Ok(body["result"].clone());
        }
        if started.elapsed() > Duration::from_secs(secs) {
            return Err(format!("want code {code}, last answer {status} {body}"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The secret opened for the call: its fetch failed past the secret step.
fn secret_opened(answer: &Value) -> Result<String, String> {
    let reason = answer["reason"].as_str().unwrap_or("");
    if reason == "answered" || reason.is_empty() || SECRET_WORDS.contains(&reason) {
        Err(format!("the fetch ended with `{reason}`: {answer}"))
    } else {
        Ok(format!("the fetch went on and failed with `{reason}`"))
    }
}

/// The owner sets `probeKey` through the door, and Ada and Kim call the
/// functions in their tenants under version 1.
///
/// `previous` is the previous Ordo. Up to f9835982 its door granted no
/// role a function, so every call answered `access_denied`. Such a
/// previous Ordo still deploys the functions and sets the secret, and no
/// run is recorded before the upgrade.
fn seed_functions(h: &Host, w: &World, s: &Seeded, previous: bool) -> FnSeed {
    let (status, body) = h.op(
        "system",
        &deployment_owner_token(),
        "POST",
        "std.deployment.setSecret",
        json!({ "name": "probeKey", "value": FN_SECRET }),
    );
    assert_eq!(status, 200, "set probeKey: {body}");
    assert!(
        !body.to_string().contains(FN_SECRET),
        "the set answered the value"
    );

    let far = FarEnd::start();
    if previous {
        let (status, body) = call_fn(h, &s.ada, "probe", json!({ "port": far.port }));
        if status == 403 && body["error"]["code"] == "access_denied" {
            eprintln!(
                "note: the previous Ordo refuses every function call; its functions are \
                 deployed and its secret is set, and no run is recorded before the upgrade"
            );
            let runs = fn_runs(w).expect("$fnruns");
            assert!(runs.is_empty(), "a refused call left a run: {runs:?}");
            return FnSeed { far, runs };
        }
    }
    for who in [&s.ada, &s.kim] {
        let answer = probe_answers(h, who, &far, "one", 60).expect("probe answers under v1");
        secret_opened(&answer).expect("the secret opens under v1");
    }
    let (status, body) = call_fn(h, &s.ada, "greet", json!({}));
    assert_eq!(
        (status, &body["result"]["name"]),
        (200, &json!("greet")),
        "{body}"
    );
    let runs = fn_runs(w).expect("$fnruns");
    assert_eq!(runs.len(), 3, "one run row per call: {runs:?}");
    FnSeed { far, runs }
}

/// The functions after an upgrade: `probe` answers `code` in both tenants
/// and its secret still opens, the secret is listed and never answered,
/// and every run from before is kept. `renamed` is version 1.1, where
/// `greet` became `hello`.
fn functions_after(
    h: &Host,
    w: &World,
    s: &Seeded,
    f: &FnSeed,
    code: &str,
    renamed: bool,
) -> Vec<Check> {
    let mut out = Vec::new();
    for (runs, opens, who) in [
        (
            "probe runs the serving code in ada's tenant",
            "the secret still opens in ada's tenant",
            &s.ada,
        ),
        (
            "probe runs the serving code in kim's tenant",
            "the secret still opens in kim's tenant",
            &s.kim,
        ),
    ] {
        let answer = probe_answers(h, who, &f.far, code, 60);
        out.push(check(runs, answer.clone().map(|a| a.to_string())));
        out.push(check(opens, answer.and_then(|a| secret_opened(&a))));
    }

    let (greet, greet_body) = call_fn(h, &s.ada, "greet", json!({}));
    if renamed {
        // A removed function answers as a name the program never had.
        let (none, none_body) = call_fn(h, &s.ada, "neverDeclared", json!({}));
        let code_of = |b: &Value| b["error"]["code"].clone();
        out.push(check(
            "a removed function answers as an unknown one",
            if greet != 200 && (greet, code_of(&greet_body)) == (none, code_of(&none_body)) {
                Ok(format!("{greet} {}", code_of(&greet_body)))
            } else {
                Err(format!(
                    "greet: {greet} {greet_body}; never declared: {none} {none_body}"
                ))
            },
        ));
        let (status, body) = call_fn(h, &s.ada, "hello", json!({}));
        out.push(check(
            "the renamed function answers under its new name",
            expect_eq(
                (status, body["result"]["name"].clone()),
                (200, json!("hello")),
            )
            .map_err(|e| format!("{e}: {body}")),
        ));
    } else {
        out.push(check(
            "greet still answers",
            expect_eq(
                (greet, greet_body["result"]["name"].clone()),
                (200, json!("greet")),
            )
            .map_err(|e| format!("{e}: {greet_body}")),
        ));
    }

    let (status, listed) = h.op(
        "system",
        &deployment_owner_token(),
        "GET",
        "std.deployment.secrets",
        json!({}),
    );
    out.push(check(
        "the secret is listed by name and version, never by value",
        if status != 200 {
            Err(format!("{status} {listed}"))
        } else if listed.to_string().contains(FN_SECRET) {
            Err("the listing holds the value".into())
        } else {
            let rows = listed["rows"].as_array().cloned().unwrap_or_default();
            match rows.iter().find(|r| r["name"] == "probeKey") {
                Some(r) => expect_eq(
                    (r["version"].clone(), r["setBy"].clone()),
                    (json!(1), json!(DEPLOYMENT_OWNER)),
                ),
                None => Err(format!("probeKey is not listed: {listed}")),
            }
        },
    ));
    let control = Pg {
        db: w.db.clone(),
        ..Pg::default()
    };
    out.push(check(
        "the secret is sealed at rest",
        control
            .query("SELECT encode(\"value\", 'escape') FROM \"$secrets\"")
            .and_then(|rows| {
                if rows.is_empty() {
                    Err("no row in $secrets".into())
                } else if format!("{rows:?}").contains(FN_SECRET) {
                    Err("$secrets holds the value in the clear".into())
                } else {
                    Ok(String::new())
                }
            }),
    ));
    out.push(check(
        "no run record holds the secret",
        control
            .query("SELECT \"principal\", \"message\", \"logs\" FROM \"$fnruns\"")
            .and_then(|rows| {
                if format!("{rows:?}").contains(FN_SECRET) {
                    Err("a $fnruns row holds the value".into())
                } else {
                    Ok(format!(
                        "{} connection(s) reached the listener",
                        f.far.hits.load(Ordering::SeqCst)
                    ))
                }
            }),
    ));
    out.push(check(
        "the run records from before are kept",
        fn_runs(w).and_then(|now| {
            if now.len() < f.runs.len() {
                return Err(format!("{} rows, {} before", now.len(), f.runs.len()));
            }
            expect_eq(&now[..f.runs.len()], &f.runs[..])
                .map(|_| format!("{} before, {} now", f.runs.len(), now.len()))
        }),
    ));
    out
}

/// Print every check, and fail on any that is not in `known`. A known break
/// that now holds fails too, so the list is kept true.
fn judge(what: &str, checks: &[Check], known: &[(&str, &str)]) {
    let mut failed = Vec::new();
    eprintln!("── {what} ──");
    for c in checks {
        let known_as = known
            .iter()
            .find(|(n, _)| *n == c.name)
            .map(|(_, why)| *why);
        match (&c.outcome, known_as) {
            (Ok(note), None) => eprintln!("ok      {} {note}", c.name),
            (Err(e), Some(why)) => eprintln!("KNOWN   {} ({why}): {e}", c.name),
            (Err(e), None) => {
                eprintln!("BROKEN  {}: {e}", c.name);
                failed.push(format!("{}: {e}", c.name));
            }
            (Ok(_), Some(why)) => {
                eprintln!("FIXED?  {} was a known break ({why}) and now holds", c.name);
                failed.push(format!(
                    "{} is listed as a known break but holds: take it off the list",
                    c.name
                ));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{what}: {} continuity check(s) failed:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

// ── The program upgrade ──────────────────────────────────────────────────

/// Breaks known across a program update of the fixture. Empty: none.
const KNOWN_PROGRAM: &[(&str, &str)] = &[];

async fn program_upgrade(known: &[(&str, &str)]) {
    let _serial = SERIAL.lock().await;
    let w = World::new("ordo_upg_program").await;
    let host = env!("CARGO_BIN_EXE_ordo-host");
    let h = w.boot(Path::new(host), "v1");
    // Each version with its backend functions: 1.1 changes `probe`'s code
    // and renames `greet` to `hello`.
    w.apply_current(&with_functions("v1")).await;
    let s = seed(&h, &w);
    let fns = seed_functions(&h, &w, &s, false);
    let feed = Feed::open(&h, &s.ada, "myNotes");

    w.apply_current(&with_functions("v1_1")).await;
    // The running server serves 1.1 once a new op answers.
    within(60, "the server serves 1.1", || {
        let (status, _) = h.op(&s.ada.space, &s.ada.token, "GET", "myLabels", json!({}));
        (status == 200).then_some(())
    });

    let mut checks = vec![feed_after(&h, &s, &feed)];
    drop(feed);
    // The feed check wrote two notes; the lists expect them.
    let mut s = s;
    s.ada_titles = titles(
        &h.rows(&s.ada.space, &s.ada.token, "myNotes")
            .expect("myNotes"),
    );
    checks.extend(verify(&h, &w, &s).await);
    checks.extend(v1_1_works(&h, &s));
    checks.extend(functions_after(&h, &w, &s, &fns, "two", true));
    let log = h.log();
    checks.push(check(
        "no tenant was left closed",
        if log.contains("stays closed") {
            Err("the server closed a tenant".into())
        } else {
            Ok(String::new())
        },
    ));
    let mut h = h;
    h.stop();
    checks.extend(chains(&w, &s).await);
    judge("program v1 to v1.1", &checks, known);
}

/// Version 1.1 applied over a running version 1 keeps every person, every
/// way in, every session and every row.
#[tokio::test(flavor = "multi_thread")]
async fn a_compatible_program_update_keeps_everyone_and_everything() {
    program_upgrade(KNOWN_PROGRAM).await;
}

// ── The Ordo upgrade ─────────────────────────────────────────────────────

/// Breaks known across the Ordo upgrade from the pinned previous build.
const KNOWN_BINARY: &[(&str, &str)] = &[];

fn previous(var: &str) -> PathBuf {
    let p = std::env::var(var).unwrap_or_else(|_| {
        panic!("{var} names the previous Ordo's binary; run scripts/upgrade-continuity.sh")
    });
    let p = PathBuf::from(p);
    assert!(p.is_file(), "{var}={} is not a file", p.display());
    p
}

/// `ORDO_UPGRADE_FUNCTIONS`: `1` when the previous Ordo has backend
/// functions, `0` when it predates them. `scripts/upgrade-continuity.sh`
/// sets it from the previous Ordo's source. Unset is refused, so a run
/// never skips the functions without saying so.
fn previous_has_functions() -> bool {
    match std::env::var("ORDO_UPGRADE_FUNCTIONS").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        other => panic!(
            "ORDO_UPGRADE_FUNCTIONS is {other:?}; set it to 1 or 0, or run \
             scripts/upgrade-continuity.sh, which sets it"
        ),
    }
}

async fn binary_upgrade(known: &[(&str, &str)]) {
    let old_host = previous("ORDO_UPGRADE_OLD_HOST");
    let old_cli = previous("ORDO_UPGRADE_OLD_CLI");
    let functions = previous_has_functions();
    let _serial = SERIAL.lock().await;
    let w = World::new("ordo_upg_binary").await;

    // The previous Ordo serves version 1, and the state is made under it.
    // With functions when the previous Ordo has them.
    let (v1, v1_1) = if functions {
        (with_functions("v1"), with_functions("v1_1"))
    } else {
        (fixture("v1"), fixture("v1_1"))
    };
    let mut old = w.boot(&old_host, "old");
    w.apply_with(&old_cli, &v1);
    let s = seed(&old, &w);
    let fns = functions.then(|| seed_functions(&old, &w, &s, true));
    let feed = Feed::open(&old, &s.ada, "myNotes");
    old.stop();
    let old_log = old.log();
    drop(old);

    // This build starts on the same databases with the same keys.
    let h = w.boot(Path::new(env!("CARGO_BIN_EXE_ordo-host")), "new");
    within(60, "the new server serves Ada's tenant", || {
        let (status, _) = h.op(&s.ada.space, &s.ada.token, "GET", "myJournal", json!({}));
        (status == 200).then_some(())
    });
    let mut checks = vec![check(
        "the open feed ended when the old server stopped",
        if feed.ended() {
            Ok(String::new())
        } else {
            Err(format!(
                "still open after the old server exited: {}",
                old_log.len()
            ))
        },
    )];
    drop(feed);
    checks.push(feed_after_reopen(&h, &s));
    let mut s = s;
    s.ada_titles = titles(
        &h.rows(&s.ada.space, &s.ada.token, "myNotes")
            .expect("myNotes"),
    );
    checks.extend(verify(&h, &w, &s).await);
    // The previous Ordo's function and secret, called on this build.
    if let Some(fns) = &fns {
        checks.extend(functions_after(&h, &w, &s, fns, "one", false));
    }

    // And version 1.1 applied by this build over data the previous made.
    w.apply_current(&v1_1).await;
    within(60, "the server serves 1.1", || {
        let (status, _) = h.op(&s.ada.space, &s.ada.token, "GET", "myLabels", json!({}));
        (status == 200).then_some(())
    });
    checks.extend(v1_1_works(&h, &s));
    if let Some(fns) = &fns {
        checks.extend(functions_after(&h, &w, &s, fns, "two", true));
    }
    let mut h = h;
    h.stop();
    checks.extend(chains(&w, &s).await);
    judge("previous Ordo to this one, then v1 to v1.1", &checks, known);
}

/// A new feed on the new server carries a write, as the SDK's reconnect does.
fn feed_after_reopen(h: &Host, s: &Seeded) -> Check {
    let feed = Feed::open(h, &s.ada, "myNotes");
    let mark = feed.len();
    create_note(h, &s.ada, "Written on the new server");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        let text = feed.text();
        if text.len() > mark && text[mark..].contains("event: invalidate") {
            return check(
                "a feed reopened on the new server carries a write",
                Ok(String::new()),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    check(
        "a feed reopened on the new server carries a write",
        Err("heard nothing after a write".into()),
    )
}

/// The previous Ordo's state, served by this build: every person, way in,
/// session and row. Run by `scripts/upgrade-continuity.sh`, which builds the
/// previous Ordo and names its binaries.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the previous Ordo built: scripts/upgrade-continuity.sh"]
async fn the_previous_ordo_hands_over_to_this_one() {
    binary_upgrade(KNOWN_BINARY).await;
}
