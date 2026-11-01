//! The notes app (`examples/notes`, `docs/plan-first-program.md` N1, N3, N4
//! and N8) against the real `ordo-host` binary and real Postgres.
//!
//! Nothing here builds the server's parts by hand. The binary boots on a
//! database that holds the notes program and custody, with passkey sign-in,
//! `ORDO_AUTH_PROVISIONING=auto-onboard`, `ORDO_PROVISION_SELF_SERVE=on-signup`
//! and attached routing, which is the configuration the README gives a
//! deployment owner. The test then acts as the page does. It sends the bytes
//! `clients/ordo-sdk` sends (`auth/passkey.ts`: `/auth/register`,
//! `/auth/complete`, `/auth/begin`; `auth/share.ts`: `/auth/share/peek`,
//! `/auth/share/accept`, `/auth/window`, `/auth/spaces`; `core/client.ts`: the
//! `/op/<name>` calls and their headers), with the software authenticator
//! standing in for the browser's `navigator.credentials`.
//!
//! What it proves:
//!
//! - **N1, the guard.** One person's note is absent from another's list, and
//!   a person calling another person's tenant is refused as the tenant not
//!   existing for them.
//! - **N3, sign-up and sign-in.** A passkey registration answers a token and
//!   a tenant of the person's own; a second person gets a different one; a
//!   later sign-in lands in the same tenant.
//! - **N4, another origin.** The page's origin is answered a preflight for
//!   `/op/*` and `/auth/*`, and an unlisted origin is not.
//! - **N8, sharing.** Share, accept, read in a window, cannot write, one note
//!   unshared, the window closed.
//! - **N5 and the client half of N8.** The page's own code (`examples/notes/web`,
//!   the SDK's sync manager and device replica) is run under Node against the
//!   same binary: notes reach the device through the live feed, are read with
//!   the network off, sit sealed in the database, and a closed share is erased
//!   from the reader's device, and a share link reads one note with no
//!   session and is refused after the owner revokes it. It needs `npm ci` in `examples/notes/web` and
//!   is skipped, loudly, without it. `ORDO_REQUIRE_WEB_TESTS=1` turns the
//!   skip into a failure.
//!
//! - **The other ways in.** An emailed code (the mail worker sends it to a
//!   relay on loopback, as Mailpit would be) lands a person in a tenant of
//!   their own, and a later code lands them in the same one. A guest writes a
//!   note, adds a passkey by a claim, and the passkey signs in to the same
//!   notes. These are the bodies `web/src/session.ts` and the SDK's
//!   `auth/email.ts`, `auth/guest.ts` and `auth/claim.ts` send.
//! - **Tags.** `notesTagged` asks each note's JSON `tags` array for one word.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

mod mail_support;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::KeyProvider as _;
use ordo_exec::{Pool, PoolConfig};
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use ordo_identity::CompleteRequest;
use serde_json::{json, Value};

const SOURCE: &str = include_str!("../../../examples/notes/notes.ordo");

/// The deployment's own space. It holds no notes.
const HOME: &str = "notes_home";
/// The principal `ORDO_DEPLOYMENT_OWNERS` names in these tests.
const DEPLOYMENT_OWNER: &str = "deployment_owner";

/// A bearer token for the deployment owner, minted under [`TOKEN_KEY`].
fn deployment_owner_token() -> String {
    let raw: Vec<u8> = (0..TOKEN_KEY.len() / 2)
        .map(|i| u8::from_str_radix(&TOKEN_KEY[2 * i..2 * i + 2], 16).expect("hex"))
        .collect();
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&key),
        &ordo_identity::TokenSpec {
            principal: ordo_identity::principal::PrincipalId(DEPLOYMENT_OWNER.into()),
            expires: ordo_identity::principal::Timestamp(u64::MAX / 2),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}
const MASTER: &str = "the-notes-app-test-master-key";
const TOKEN_KEY: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const PAGE_KEY: &str = "4444444444444444444444444444444444444444444444444444444444444444";

/// The page, as it runs locally: Vite's dev server.
const PAGE: &str = "http://localhost:5173";

/// Every test boots a deployment on a database of its own, a port of its own
/// and a log file named for its database, so tests share no state and run in
/// parallel. The exception is a test that counts server connections: it needs
/// the server to itself, or the headroom under `max_connections` it reads
/// means nothing. Those tests are named `connections_*`, take the write side
/// of this lock, and every other test takes the read side. The lock only
/// orders tests inside one process. Under nextest, which runs one process per
/// test, the `connections_*` tests are kept to one at a time by a test group
/// that matches that prefix.
static SERVER: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// How many deployments run at once in one process. Each holds a pool of 6
/// and its tenants' pools, and the server's `max_connections` is 100. Eight
/// at once ran it out ("too many clients already"), and a tenant then sat in
/// provisioning until the test gave up.
const AT_ONCE: usize = 4;
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(AT_ONCE);

/// A place to run a test that does not count connections: the read side of
/// [`SERVER`] and one of [`AT_ONCE`] slots. Held until the test ends.
async fn shared() -> (
    tokio::sync::RwLockReadGuard<'static, ()>,
    tokio::sync::SemaphorePermit<'static>,
) {
    let read = SERVER.read().await;
    let slot = SLOTS.acquire().await.expect("slots");
    (read, slot)
}

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

/// A database holding the notes program and the deployment's custody, the
/// state `ordo apply` and the first boot leave.
async fn deployment_with_notes(db: &str) {
    deployment_with_program(db, SOURCE).await
}

/// The same, with another program: notes v2, for its reminders.
async fn deployment_with_program(db: &str, program: &str) {
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
    let plain = Fixture::from_source(program, "").expect("notes.ordo compiles");
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

    let mut prog = Fixture::from_source(program, "").expect("notes.ordo compiles");
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
        source: program.to_string(),
        ddl,
        applied_at: ordo_commandlog::Timestamp(1),
        actor: "operator:notes-test".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:notes-test",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
}

/// A running deployment. Killed when dropped.
struct Deployment {
    port: u16,
    child: Child,
    db: String,
    log: std::path::PathBuf,
}

impl Deployment {
    /// The settings `examples/notes/README.md` gives, with the page's origin
    /// in `ORDO_CORS_ORIGINS` when `cors` says so.
    async fn boot(db: &str, cors: Option<&str>) -> Deployment {
        Self::boot_with(db, cors, &[]).await
    }

    /// `boot`, with more settings for the binary.
    async fn boot_with(db: &str, cors: Option<&str>, extra: &[(&str, &str)]) -> Deployment {
        deployment_with_notes(db).await;
        let (port, child, log) = spawn_host(db, "", cors, extra);
        Deployment {
            port,
            child,
            db: db.to_string(),
            log,
        }
    }

    /// `boot_with`, on another program.
    async fn boot_program(
        db: &str,
        program: &str,
        cors: Option<&str>,
        extra: &[(&str, &str)],
    ) -> Deployment {
        deployment_with_program(db, program).await;
        let (port, child, log) = spawn_host(db, "", cors, extra);
        Deployment {
            port,
            child,
            db: db.to_string(),
            log,
        }
    }

    /// One more server on the same database, as a deployment with two
    /// replicas runs. It is stopped when dropped, before the deployment.
    fn replica(&self, cors: Option<&str>, extra: &[(&str, &str)]) -> Replica {
        let (_, child, log) = spawn_host(&self.db, "-replica", cors, extra);
        Replica { child, log }
    }
}

/// A second server on a deployment's database. Stopped when dropped.
struct Replica {
    child: Child,
    log: std::path::PathBuf,
}

impl Drop for Replica {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status();
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
            let said = std::fs::read_to_string(&self.log).unwrap_or_default();
            let _ = std::fs::write(
                std::env::temp_dir().join("ordo-notes-replica-last.log"),
                said,
            );
        }
        let _ = std::fs::remove_file(&self.log);
    }
}

/// Start the binary on `db`, which already holds the program, with the
/// README's settings and `extra`. Answers its port, the process and its log.
fn spawn_host(
    db: &str,
    tag: &str,
    cors: Option<&str>,
    extra: &[(&str, &str)],
) -> (u16, Child, std::path::PathBuf) {
    let port = free_port();
    let base = PoolConfig::from_env();
    // The server writes to a file, so a full pipe never stops it and a
    // failing test can print what it said.
    let log = std::env::temp_dir().join(format!("ordo-notes-{db}{tag}.log"));
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
        // The deployment owner, who reads `/health/ready`'s detail.
        .env("ORDO_DEPLOYMENT_OWNERS", DEPLOYMENT_OWNER)
        .env("ORDO_HOST_ADDR", format!("127.0.0.1:{port}"))
        // The README's settings for a local deployment.
        .env("ORDO_AUTH_METHODS", "passkey")
        .env("ORDO_APEX_DOMAIN", "localhost")
        .env("ORDO_AUTH_ORIGINS", PAGE)
        .env("ORDO_AUTH_PROVISIONING", "auto-onboard")
        .env("ORDO_PROVISION_SELF_SERVE", "on-signup")
        // No routing and no credentials settings: every tenant gets a
        // database of its own on this server, dialled with the password
        // the server already holds (ruled 2026-10-06).
        .env_remove("ORDO_PG_ROUTING")
        .env_remove("ORDO_TENANT_CREDENTIALS")
        .env_remove("ORDO_TENANT_CREDENTIALS_DIR")
        .env_remove("ORDO_CHECKPOINT_KEY")
        .env_remove("ORDO_SECURITY_CHECKPOINT_KEY")
        .env_remove("ORDO_EXIT_BUNDLE_DIR")
        .env_remove("ORDO_CORS_ORIGINS")
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    if let Some(origins) = cors {
        c.env("ORDO_CORS_ORIGINS", origins);
    }
    for (k, v) in extra {
        c.env(k, v);
    }
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child, &log);
    (port, child, log)
}

impl Deployment {
    /// What `serve.sh` does when the program changes, against the
    /// deployment's own database: `ordo migrate --apply` when `migrate_first`,
    /// then `ordo apply`, which by its contract touches one database and so
    /// reaches no tenant's. The running server learns of the new version from
    /// the notification the activation sends.
    ///
    /// Returns once the running server has swapped to the version, which it
    /// does some moments after the activation: until then it still serves the
    /// old one, and an old tenant is not yet behind anything.
    async fn apply_program(&self, source: &str, migrate_first: bool) {
        let control = Pool::connect(cfg(&self.db))
            .await
            .expect("the control pool");
        let program = ordo_host::schema_apply::Program::compile(source).expect("compiles");
        if migrate_first {
            let (_, applied) =
                ordo_host::schema_apply::apply(&control, &program, "notes-test", "migrate").await;
            applied.expect("the deployment's own database takes the change");
        }
        let report = ordo_host::apply_op::run_program(
            &control,
            &ordo_exec::route::SinglePool::new(control.clone()),
            &program,
            vec![],
            "notes-test",
            "ordo apply",
            true,
        )
        .await
        .expect("the apply");
        assert!(
            matches!(
                report.activation,
                ordo_host::apply_op::Activation::Switched { .. }
            ),
            "{report:?}"
        );
        let swapped = format!("-> {}", program.version);
        eventually("the server swaps to the new version", || {
            self.log().contains(&swapped).then_some(())
        });
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
        (status, head.to_string(), json)
    }

    /// A POST to `/auth/...`, as `auth/passkey.ts` sends it.
    fn auth(&self, path: &str, body: Value, token: Option<&str>) -> (u16, Value) {
        let mut headers = vec![
            ("Content-Type", "application/json"),
            ("Origin", PAGE),
            ("Accept", "application/json"),
        ];
        let bearer;
        if let Some(t) = token {
            bearer = format!("Bearer {t}");
            headers.push(("Authorization", &bearer));
        }
        let (status, _, json) = self.send("POST", path, &headers, &body.to_string());
        (status, json)
    }

    /// A call to `/op/<name>`, as `core/client.ts` sends it: reads and
    /// deletes carry their parameters in the query, writes in a JSON body.
    fn op(
        &self,
        space: &str,
        token: &str,
        method: &str,
        name: &str,
        params: Value,
    ) -> (u16, Value) {
        static KEY: AtomicU64 = AtomicU64::new(1);
        let bearer = format!("Bearer {token}");
        let key = format!("notes-test-{}", KEY.fetch_add(1, Ordering::SeqCst));
        let mut headers = vec![
            ("X-Ordo-Space", space),
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
            let path = if q.is_empty() {
                format!("/op/{name}")
            } else {
                format!("/op/{name}?{}", q.join("&"))
            };
            (path, String::new())
        };
        let (status, _, json) = self.send(method, &path, &headers, &body);
        (status, json)
    }

    fn rows(&self, space: &str, token: &str, name: &str) -> Vec<Value> {
        let (status, body) = self.op(space, token, "GET", name, json!({}));
        assert_eq!(status, 200, "{name} in {space}: {body}");
        body["rows"].as_array().cloned().unwrap_or_default()
    }

    fn stop(self) {
        drop(self);
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
            let _ = std::fs::write(std::env::temp_dir().join("ordo-notes-last.log"), self.log());
            eprintln!("--- the server log is in ordo-notes-last.log ---");
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
        let _ = std::fs::remove_file(&self.log);
    }
}

fn wait_for_live(port: u16, child: &mut Child, log: &std::path::Path) {
    // 60 s: under nextest several deployments boot at once on one node
    // (run 41294339083 ran out at 15 s).
    for _ in 0..1200 {
        if let Ok(Some(status)) = child.try_wait() {
            let said = std::fs::read_to_string(log).unwrap_or_default();
            panic!("the binary exited before serving: {status}\n{said}");
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let said = std::fs::read_to_string(log).unwrap_or_default();
    panic!("the binary never bound {port} in 60 s\n{said}");
}

/// The browser's authenticator for one person: its own key and credential,
/// reporting the page's origin under the relying party `localhost`.
fn device(seed: u8, name: &str) -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(seed),
        credential_id: name.as_bytes().to_vec(),
        rp_id: "localhost".to_string(),
        origin: PAGE.to_string(),
        ..TestAuthenticator::es256()
    }
}

/// What a page holds after `registerPasskey` or `signInWithPasskey`.
struct Session {
    token: String,
    /// `raw.space`, the person's own tenant.
    space: String,
    raw: Value,
}

fn session_of(d: &Deployment, status: u16, done: Value) -> Session {
    assert_eq!(status, 200, "{done}");
    let done = tenant_ready(d, done);
    Session {
        token: done["token"].as_str().expect("a token").to_string(),
        space: done["space"].as_str().expect("a tenant").to_string(),
        raw: done,
    }
}

/// The sign-in's answer once the person's own tenant is made, as the
/// SDK's `waitForTenant` reads it: the sign-in answers first
/// (`provisioning: "in progress"`, owner 2026-10-21), and
/// `GET /auth/provisioning` says when the tenant is `ready`.
fn tenant_ready(d: &Deployment, mut done: Value) -> Value {
    if done["provisioning"] != "in progress" {
        return done;
    }
    let bearer = format!("Bearer {}", done["token"].as_str().expect("a token"));
    let headers = [("Origin", PAGE), ("Authorization", bearer.as_str())];
    let mut last = Value::Null;
    for _ in 0..600 {
        let (status, _, said) = d.send("GET", "/auth/provisioning", &headers, "");
        assert_eq!(status, 200, "{said}");
        last = said;
        match last["state"].as_str() {
            Some("ready") => {
                done["space"] = last["space"].clone();
                done["provisioning"] = Value::Null;
                return done;
            }
            Some("provisioning") => std::thread::sleep(Duration::from_millis(100)),
            _ => panic!("the tenant was not made: {last}"),
        }
    }
    panic!("the tenant was not ready in 60 seconds: {last}");
}

/// `registerPasskey(endpoint, { display })`, then wait for the door.
fn sign_up(d: &Deployment, device: &TestAuthenticator, display: &str) -> Session {
    let (status, done) = register(d, device, display);
    let session = session_of(d, status, done);
    settle(d, &session);
    session
}

/// The ceremony alone: `/auth/complete`'s status and answer.
fn register(d: &Deployment, device: &TestAuthenticator, display: &str) -> (u16, Value) {
    let (status, options) = d.auth(
        "/auth/register",
        json!({"method": "passkey", "display": display}),
        None,
    );
    assert_eq!(status, 200, "{options}");
    assert_eq!(options["rp_id"], "localhost", "{options}");
    let challenge = options["challenge"].as_str().expect("challenge");
    let CompleteRequest::PasskeyRegistration {
        challenge: c,
        client_data_json,
        attestation_object,
    } = device.register(challenge)
    else {
        panic!("not a registration");
    };
    let (status, done) = d.auth(
        "/auth/complete",
        json!({
            "method": "passkey", "kind": "passkey_registration", "challenge": c.0,
            "client_data_json": b64url_encode(&client_data_json),
            "attestation_object": b64url_encode(&attestation_object),
            "display": display,
        }),
        None,
    );
    (status, done)
}

/// Wait until the door serves the person's tenant, and say how long it took.
fn settle(d: &Deployment, s: &Session) {
    let (_, took) = eventually("the door serves the new tenant", || {
        let (status, _) = d.op(&s.space, &s.token, "GET", "myNotes", json!({}));
        (status == 200).then_some(())
    });
    eprintln!("the door served {} after {:?}", s.space, took);
}

/// `signInWithPasskey(endpoint)`.
fn sign_in(d: &Deployment, device: &TestAuthenticator) -> Session {
    let (status, options) = d.auth("/auth/begin", json!({"method": "passkey"}), None);
    assert_eq!(status, 200, "{options}");
    let challenge = options["challenge"].as_str().expect("challenge");
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
    let (status, done) = d.auth(
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
    session_of(d, status, done)
}

/// Run `probe` until it answers `Some`, for at most thirty seconds.
///
/// Thirty since 2026-10-05: the door rebuilt within ten seconds locally but
/// twice took longer on CI, where several jobs share one node. The test
/// proves the door catches up, not how fast; a regression in rebuild time
/// shows in the elapsed time this returns, which callers may assert on.
///
/// The door learns of a new seat by a notification and rebuilds, on every
/// replica, some moments after the request that made it answers. So the
/// first call after a sign-up, an accept or a revocation can still see the
/// old door. The page does the same: it retries a refusal for a few seconds
/// before it believes it (`examples/notes/web/src/session.ts`).
fn eventually<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> (T, Duration) {
    let started = std::time::Instant::now();
    loop {
        if let Some(found) = probe() {
            return (found, started.elapsed());
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "still waiting after 30 s: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn code_of(body: &Value) -> String {
    body["error"]["code"].as_str().unwrap_or("").to_string()
}

fn titles(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["title"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The moment the page stamps a note with: one more second each call, so
/// "newest first" is decided by the notes and not by the test's speed.
fn stamp() -> String {
    static SECONDS: AtomicU64 = AtomicU64::new(0);
    let n = SECONDS.fetch_add(1, Ordering::SeqCst);
    format!("2026-09-29T12:{:02}:{:02}Z", (n / 60) % 60, n % 60)
}

fn create_note(d: &Deployment, s: &Session, title: &str, body: &str) -> String {
    let (status, out) = d.op(
        &s.space,
        &s.token,
        "POST",
        "createNote",
        json!({"title": title, "body": body, "tags": [], "created": stamp()}),
    );
    assert_eq!(status, 200, "{out}");
    out["rows"][0]["created"]
        .as_str()
        .expect("the created id")
        .to_string()
}

// ── N1 and N3 ────────────────────────────────────────────────────────────

/// Two people sign up with a passkey. Each is signed in with a tenant of
/// their own, each writes a note the other cannot see, one cannot reach the
/// other's tenant, and a later sign-in lands in the same tenant.
#[tokio::test(flavor = "multi_thread")]
async fn two_people_sign_up_and_each_keeps_notes_in_a_tenant_of_their_own() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_signup", Some(PAGE)).await;
    let ada_device = device(21, "ada");
    let bo_device = device(22, "bo");

    // N3: a form and a passkey, and the person is signed in with a tenant.
    let ada = sign_up(&d, &ada_device, "Ada");
    let bo = sign_up(&d, &bo_device, "Bo");
    assert_eq!(ada.raw["provisioning"], Value::Null, "{}", ada.raw);
    assert_ne!(ada.space, bo.space, "one person, one tenant");
    assert_ne!(ada.space, HOME);
    assert!(ada.raw["expires_at"].is_number() && ada.raw["session_expires_at"].is_number());

    // The list of one's own tenants is what the page calls after sign-in.
    let (status, _, spaces) = d.send(
        "GET",
        "/auth/spaces",
        &[("Authorization", &format!("Bearer {}", ada.token))],
        "",
    );
    assert_eq!(status, 200, "{spaces}");
    assert_eq!(spaces.as_array().map(Vec::len), Some(1), "{spaces}");
    assert_eq!(spaces[0]["space"], json!(ada.space));
    assert_eq!(spaces[0]["role"], "owner");
    assert_eq!(spaces[0]["via"], "member");
    assert_eq!(spaces[0]["name"], "Ada");

    // In its own database the person is the one owner.
    let theirs = Pool::connect(cfg(&ada.space))
        .await
        .expect("their database");
    let members = theirs
        .rows("SELECT \"role\", \"status\" FROM \"members\"")
        .await
        .expect("members");
    assert_eq!(
        members,
        vec![vec![Some("owner".to_string()), Some("active".to_string())]]
    );
    // A new tenant is empty.
    assert!(d.rows(&ada.space, &ada.token, "myNotes").is_empty());

    // N1: each person's note is their own.
    create_note(&d, &ada, "Groceries", "milk");
    create_note(&d, &bo, "Ideas", "a notes app");
    assert_eq!(
        titles(&d.rows(&ada.space, &ada.token, "myNotes")),
        ["Groceries"]
    );
    assert_eq!(titles(&d.rows(&bo.space, &bo.token, "myNotes")), ["Ideas"]);

    // Bo calling Ada's tenant is refused as if it were not there, and the
    // refusal for a tenant that does not exist is byte-identical.
    let (status, refused) = d.op(&ada.space, &bo.token, "GET", "myNotes", json!({}));
    assert_eq!(status, 403, "{refused}");
    let (status, absent) = d.op("spc_nothing_here", &bo.token, "GET", "myNotes", json!({}));
    assert_eq!(status, 403, "{absent}");
    assert_eq!(
        refused, absent,
        "an absent tenant and a forbidden one differ"
    );

    // Compare-and-set, delete.
    let rows = d.rows(&ada.space, &ada.token, "myNotes");
    let id = rows[0]["id"].as_str().expect("id").to_string();
    let rev = rows[0]["rev"].clone();
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "POST",
        "editNote",
        json!({"id": id, "rev": rev, "title": "Groceries", "body": "milk, eggs", "tags": ["home"]}),
    );
    assert_eq!(status, 200, "{out}");
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "POST",
        "editNote",
        json!({"id": id, "rev": rev, "title": "Groceries", "body": "stale", "tags": []}),
    );
    assert_eq!(code_of(&out), "cas_miss", "{status} {out}");
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "DELETE",
        "deleteNote",
        json!({"id": id}),
    );
    assert_eq!(status, 200, "{out}");
    assert!(d.rows(&ada.space, &ada.token, "myNotes").is_empty());
    // The row is deleted, and its access rows went with it.
    let kept = theirs
        .rows("SELECT count(*) FROM \"notes\"")
        .await
        .expect("notes");
    assert_eq!(kept, vec![vec![Some("0".to_string())]]);
    let access = theirs
        .rows("SELECT count(*) FROM \"notesAccess\"")
        .await
        .expect("access rows");
    assert_eq!(access, vec![vec![Some("0".to_string())]]);
    assert_eq!(titles(&d.rows(&bo.space, &bo.token, "myNotes")), ["Ideas"]);

    // A later sign-in lands in the same tenant, with the same notes.
    let again = sign_in(&d, &ada_device);
    assert_eq!(again.space, ada.space);
    let again_bo = sign_in(&d, &bo_device);
    assert_eq!(again_bo.space, bo.space);
    assert_eq!(
        titles(&d.rows(&again_bo.space, &again_bo.token, "myNotes")),
        ["Ideas"]
    );

    // Refresh continues the session: a new token, the same person and tenant.
    let (status, _, refreshed) = d.send(
        "POST",
        "/auth/refresh",
        &[("Authorization", &format!("Bearer {}", again.token))],
        "",
    );
    assert_eq!(status, 200, "{refreshed}");
    assert!(refreshed["expires_at"].is_number(), "{refreshed}");
    let renewed = refreshed["token"]
        .as_str()
        .expect("a new token")
        .to_string();
    assert_eq!(d.rows(&again.space, &renewed, "myNotes").len(), 0);

    // Signing out ends it: the token that was refreshed from, and the new one.
    let (status, _, _) = d.send(
        "POST",
        "/auth/logout",
        &[("Authorization", &format!("Bearer {renewed}"))],
        "",
    );
    assert_eq!(status, 204);
    let (status, refused) = d.op(&again.space, &renewed, "GET", "myNotes", json!({}));
    assert_eq!((status, code_of(&refused).as_str()), (403, "access_denied"));
    let (status, _, _) = d.send(
        "POST",
        "/auth/refresh",
        &[("Authorization", &format!("Bearer {renewed}"))],
        "",
    );
    assert_eq!(status, 401, "a logged-out session cannot be refreshed");
    // Signing in again starts a session that is not touched.
    let back = sign_in(&d, &ada_device);
    assert_eq!(back.space, ada.space);
    d.stop();
}

// ── 25e: what the notes app found, smaller ──────────────────────────────

/// 25e. The door served a new tenant 65 to 120 ms after sign-up answered,
/// so the page's first call could be refused. The replica that made the
/// tenant now rebuilds its door before it answers.
///
/// And the server sets nothing about routing or credentials (ruled
/// 2026-10-06): each tenant still gets a database of its own on the
/// deployment's server, and is served from it.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_with_no_routing_or_credentials_settings_gives_a_new_tenant_its_own_database_and_serves_it(
) {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_first_call", Some(PAGE)).await;
    for (seed, name) in [(61u8, "cy"), (62, "di"), (63, "ed")] {
        let (status, done) = register(&d, &device(seed, name), name);
        let s = session_of(&d, status, done);
        // No retry: the call that follows the answer is the page's first.
        let (status, out) = d.op(&s.space, &s.token, "GET", "myNotes", json!({}));
        assert_eq!(status, 200, "{name}'s first call: {out}");
        let (status, out) = d.op(
            &s.space,
            &s.token,
            "POST",
            "createNote",
            json!({"title": "first", "body": "", "tags": [], "created": stamp()}),
        );
        assert_eq!(status, 200, "{name}'s first write: {out}");
        // The note is in a database named for the tenant, not in the
        // deployment's own.
        let theirs = Pool::connect(cfg(&s.space))
            .await
            .expect("the tenant's own database");
        let rows = theirs
            .rows("SELECT count(*) FROM notes")
            .await
            .expect("notes");
        assert_eq!(rows[0][0].as_deref(), Some("1"), "{name}'s note");
        let control = Pool::connect(cfg(&d.db)).await.expect("the control pool");
        let held = control
            .rows(&format!(
                "SELECT count(*) FROM notes WHERE \"$space\" = '{}'",
                s.space
            ))
            .await
            .expect("notes");
        assert_eq!(
            held[0][0].as_deref(),
            Some("0"),
            "{name}'s note is in the deployment's own database"
        );
    }
    assert!(
        !d.log().contains("no database credential") && !d.log().contains("nothing answers"),
        "no tenant was refused for a missing credential:\n{}",
        d.log()
    );
    d.stop();
}

// ── 25i: a program update ────────────────────────────────────────────────

/// The notes program with one more field, for an update that adds.
fn with_pinned() -> String {
    let with = SOURCE.replace(
        "  created: datetime\n",
        "  created: datetime\n  pinned: bool = false\n",
    );
    assert_ne!(with, SOURCE, "the marker line moved");
    with
}

async fn has_column(pool: &Pool, table: &str, column: &str) -> bool {
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_name = '{table}' AND column_name = '{column}'"
        ))
        .await
        .expect("columns");
    rows[0][0].as_deref() == Some("1")
}

/// 25i. Two people sign up, the program changes, and the same people keep
/// working with the same notes. The update is applied the way `serve.sh`
/// applies it, against the deployment's own database only, so the tenants
/// are reached by the running server's catch-up and by nothing else. First
/// a field is added, then it is removed again, which is what the notes
/// program's `gone` field did.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_update_leaves_the_people_who_signed_up_working() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_update", Some(PAGE)).await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let bo = sign_up(&d, &device(22, "bo"), "Bo");
    create_note(&d, &ada, "Groceries", "milk");
    create_note(&d, &bo, "Ideas", "a notes app");
    let ada_db = Pool::connect(cfg(&ada.space))
        .await
        .expect("Ada's database");
    assert!(!has_column(&ada_db, "notes", "pinned").await);

    // The program gains a field.
    d.apply_program(&with_pinned(), true).await;
    for (who, want) in [(&ada, "Groceries"), (&bo, "Ideas")] {
        let (rows, took) = eventually("the tenant serves after the update", || {
            let (status, body) = d.op(&who.space, &who.token, "GET", "myNotes", json!({}));
            (status == 200).then(|| body["rows"].as_array().cloned().unwrap_or_default())
        });
        eprintln!("{} served again after {took:?}", who.space);
        assert_eq!(titles(&rows), [want], "the same person, the same notes");
    }
    assert!(
        has_column(&ada_db, "notes", "pinned").await,
        "the field reached the tenant's own database"
    );
    create_note(&d, &ada, "After the update", "still works");
    assert_eq!(
        d.rows(&ada.space, &ada.token, "myNotes").len(),
        2,
        "a write after the update lands"
    );

    // The field goes again. Nothing is dropped, and the person keeps working.
    d.apply_program(SOURCE, false).await;
    for who in [&ada, &bo] {
        eventually("the tenant serves after the field is removed", || {
            let (status, _) = d.op(&who.space, &who.token, "GET", "myNotes", json!({}));
            (status == 200).then_some(())
        });
    }
    assert!(
        has_column(&ada_db, "notes", "pinned").await,
        "a removed field's column is left in place"
    );
    let log = d.log();
    assert!(
        !log.contains("stays closed"),
        "no tenant was left closed:\n{log}"
    );
    // Signing in again after the update lands in the same tenant.
    let again = sign_in(&d, &device(21, "ada"));
    assert_eq!(again.space, ada.space);
    d.stop();
}

/// 25i. A change one tenant's data cannot take keeps that tenant closed, and
/// nobody else. Ada has two notes and the program makes titles unique, which
/// is applied only over an empty table, so her database refuses it. Bo has
/// written nothing, and his takes it. Ada is told her data is
/// being brought to the current version of the app, and the deployment owner
/// reads the refusal in `/health/ready`.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_one_tenants_data_cannot_take_closes_that_tenant_alone() {
    let _shared = shared().await;
    let d = Deployment::boot_with(
        "ordo_notes_refused",
        Some(PAGE),
        &[("ORDO_TENANT_CATCHUP_SECS", "1")],
    )
    .await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let bo = sign_up(&d, &device(22, "bo"), "Bo");
    create_note(&d, &ada, "Same", "one");
    create_note(&d, &ada, "Same", "two");
    // Bo has written nothing: a constraint is applied only over a table that
    // holds no rows, so an empty tenant takes it and a used one does not.

    let unique = SOURCE.replace("  title: text\n", "  title: text unique\n");
    assert_ne!(unique, SOURCE, "the marker line moved");
    d.apply_program(&unique, true).await;

    // Bo's database took it.
    eventually("Bo serves after the update", || {
        let (status, _) = d.op(&bo.space, &bo.token, "GET", "myNotes", json!({}));
        (status == 200).then_some(())
    });
    assert!(d.rows(&bo.space, &bo.token, "myNotes").is_empty());
    create_note(&d, &bo, "Ideas", "a notes app");

    // Ada's stays closed, and what she is told names no database and no
    // version.
    let (status, body) = d.op(&ada.space, &ada.token, "GET", "myNotes", json!({}));
    assert_eq!(status, 503, "{body}");
    assert_eq!(code_of(&body), "space_unavailable", "{body}");
    let said = body["error"]["message"].as_str().unwrap_or("").to_string();
    assert!(
        said.contains("brought to the current version of the app"),
        "{said}"
    );
    for leak in ["cannot reach", "database", "schema", "operator"] {
        assert!(!said.contains(leak), "`{leak}` in: {said}");
    }

    // The deployment owner reads the reason, with the refusal in it.
    let (reason, _) = eventually("readiness says why Ada is closed", || {
        let (_, _, ready) = d.send(
            "GET",
            "/health/ready",
            &[(
                "Authorization",
                &format!("Bearer {}", deployment_owner_token()),
            )],
            "",
        );
        ready["quarantined"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|q| q["space"] == json!(ada.space))
            .and_then(|q| q["reason"].as_str())
            .filter(|r| r.contains("refused"))
            .map(str::to_string)
    });
    assert!(reason.contains("schema behind"), "{reason}");
    let (_, _, ready) = d.send("GET", "/health/ready", &[], "");
    assert!(
        !ready["quarantined"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|q| q["space"] == json!(bo.space)),
        "Bo is not closed: {ready}"
    );
    // Ada's data is untouched.
    let ada_db = Pool::connect(cfg(&ada.space))
        .await
        .expect("Ada's database");
    let kept = ada_db
        .rows("SELECT count(*) FROM \"notes\"")
        .await
        .expect("notes");
    assert_eq!(kept, vec![vec![Some("2".to_string())]]);
    d.stop();
}

// ── N4 ───────────────────────────────────────────────────────────────────

/// The listed origin is answered a preflight for the op routes and the auth
/// routes and its calls carry the allow header. An origin that is not listed
/// is refused, and so is every origin when the variable is unset.
#[tokio::test(flavor = "multi_thread")]
async fn a_page_on_another_origin_reaches_the_door_and_an_unlisted_one_does_not() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_cors", Some(PAGE)).await;

    fn preflight(
        d: &Deployment,
        origin: &str,
        path: &str,
        method: &str,
        headers: &str,
    ) -> (u16, String, Value) {
        d.send(
            "OPTIONS",
            path,
            &[
                ("Origin", origin),
                ("Access-Control-Request-Method", method),
                ("Access-Control-Request-Headers", headers),
            ],
            "",
        )
    }
    fn allowed(head: &str, name: &str) -> Option<String> {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    }

    for (path, method, headers) in [
        (
            "/op/myNotes",
            "GET",
            "authorization,x-ordo-space,x-ordo-version",
        ),
        (
            "/op/createNote",
            "POST",
            "authorization,content-type,idempotency-key,x-ordo-space,x-ordo-version",
        ),
        (
            "/op/deleteNote",
            "DELETE",
            "authorization,idempotency-key,x-ordo-space,x-ordo-version",
        ),
        ("/auth/register", "POST", "content-type"),
        ("/auth/complete", "POST", "content-type"),
        ("/auth/share/accept", "POST", "authorization,content-type"),
        ("/auth/window", "POST", "authorization,content-type"),
        ("/auth/spaces", "GET", "authorization"),
    ] {
        let (status, head, _) = preflight(&d, PAGE, path, method, headers);
        assert_eq!(status, 204, "{path}: {head}");
        assert_eq!(
            allowed(&head, "access-control-allow-origin").as_deref(),
            Some(PAGE),
            "{path}: {head}"
        );
        // The renewal cookie is set, sent and cleared on `/auth/` only, so
        // those routes allow credentials, beside the exact origin. The op
        // routes read no cookie and never do (`ordo_api::cors`).
        let credentials = allowed(&head, "access-control-allow-credentials");
        if path.starts_with("/auth/") {
            assert_eq!(
                credentials.as_deref(),
                Some("true"),
                "{path}: an auth route allows credentials: {head}"
            );
        } else {
            assert!(
                credentials.is_none(),
                "{path}: an op route never allows credentials: {head}"
            );
        }
    }

    // An origin that is not listed.
    let (status, head, _) = preflight(
        &d,
        "https://evil.example",
        "/op/myNotes",
        "GET",
        "authorization",
    );
    assert_eq!(status, 403, "{head}");
    assert!(
        allowed(&head, "access-control-allow-origin").is_none(),
        "{head}"
    );

    // A real call from the page carries the header on the answer, and the
    // page can read the headers the SDK reads.
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let (status, head, _) = d.send(
        "GET",
        "/op/myNotes",
        &[
            ("Origin", PAGE),
            ("X-Ordo-Space", &ada.space),
            ("Authorization", &format!("Bearer {}", ada.token)),
        ],
        "",
    );
    assert_eq!(status, 200, "{head}");
    assert_eq!(
        allowed(&head, "access-control-allow-origin").as_deref(),
        Some(PAGE)
    );
    let exposed = allowed(&head, "access-control-expose-headers").unwrap_or_default();
    for name in ["X-Ordo-Position", "X-Ordo-Head", "X-Ordo-Op-Version"] {
        assert!(exposed.contains(name), "{name} is not exposed: {exposed}");
    }
    d.stop();

    // With the variable unset the door answers no origin: the preflight
    // reaches the router, which has no `OPTIONS` route, so a browser reports
    // the block and the page never sends the real call.
    let d = Deployment::boot("ordo_notes_cors_unset", None).await;
    let (status, head, _) = preflight(&d, PAGE, "/op/myNotes", "GET", "authorization");
    assert_eq!(
        status, 405,
        "an unset allowlist answered a preflight: {head}"
    );
    assert!(
        allowed(&head, "access-control-allow-origin").is_none(),
        "{head}"
    );
    d.stop();
}

// ── N5, and the page's half of N8 ────────────────────────────────────────

fn principal_of(token: &str) -> String {
    let payload = token.split('.').next().expect("a payload");
    let text = String::from_utf8(b64url_decode(payload).expect("base64url")).expect("text");
    text.split('|').nth(1).expect("a principal").to_string()
}

/// The device replica, the sync manager and the guest's window, driven by the
/// page's own modules under Node against the running binary.
#[tokio::test(flavor = "multi_thread")]
async fn the_page_keeps_notes_on_the_device_and_erases_a_closed_share() {
    let _shared = shared().await;
    let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/notes/web");
    if !web.join("node_modules/.bin/vitest").exists() {
        let said =
            "examples/notes/web has no node_modules: run `npm ci` there to run the page's tests";
        assert!(std::env::var("ORDO_REQUIRE_WEB_TESTS").is_err(), "{said}");
        eprintln!("SKIPPED: {said}");
        return;
    }
    let d = Deployment::boot("ordo_notes_device", Some(PAGE)).await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let bo = sign_up(&d, &device(22, "bo"), "Bo");
    let live = json!({
        "url": format!("http://127.0.0.1:{}", d.port),
        "ada": {"token": ada.token, "space": ada.space, "principal": principal_of(&ada.token)},
        "bo": {"token": bo.token, "space": bo.space, "principal": principal_of(&bo.token)},
    });
    let ran = Command::new("npx")
        .args(["vitest", "run", "test/live.test.ts"])
        .current_dir(&web)
        .env("NOTES_LIVE", live.to_string())
        .output()
        .expect("npx runs");
    let said = format!(
        "{}\n{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(
        ran.status.success(),
        "the page's device tests failed:\n{said}"
    );
    assert!(
        said.contains("8 passed") && !said.contains("skipped"),
        "the page's device tests did not all run:\n{said}"
    );
    eprintln!("{said}");
    d.stop();
}

// ── N8 ───────────────────────────────────────────────────────────────────

/// The whole of sharing, through the routes the SDK calls: the sharer shares
/// one note, the reader accepts and reads it in a window, cannot write, sees
/// a second share without a second seat, and loses each when the sharer
/// unshares one note and then closes the window.
#[tokio::test(flavor = "multi_thread")]
async fn a_note_shared_with_another_person_is_read_only_and_can_be_taken_back() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_share", Some(PAGE)).await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let bo = sign_up(&d, &device(22, "bo"), "Bo");

    let groceries = create_note(&d, &ada, "Groceries", "milk");
    let secret = create_note(&d, &ada, "Diary", "not for Bo");
    create_note(&d, &ada, "Trip", "pack");

    // Ada shares Groceries: `std.space.share` in her own tenant.
    let share = |note: &str, label: &str| -> String {
        let (status, out) = d.op(
            &ada.space,
            &ada.token,
            "POST",
            "std.space.share",
            json!({
                "ops": "myNotes",
                "rows": json!([{"entity": "notes", "id": note, "level": "read"}]).to_string(),
                "expires": "365d",
                "label": label,
            }),
        );
        assert_eq!(status, 200, "{out}");
        out["rows"][0]["ticket"]
            .as_str()
            .expect("a ticket")
            .to_string()
    };
    let ticket = share(&groceries, "Groceries");

    // Bo opens the link: peek needs no session and does not spend it.
    let (status, peek) = d.auth("/auth/share/peek", json!({ "ticket": ticket }), None);
    assert_eq!(status, 200, "{peek}");
    assert_eq!(peek["space"], json!(ada.space));
    assert_eq!(peek["ops"], json!(["myNotes"]));
    assert_eq!(peek["rows"][0]["id"], json!(groceries));
    assert!(peek["discloses"].is_string() && peek["notice"].is_string());

    // Nothing is readable before Bo accepts.
    let (status, _) = d.auth("/auth/window", json!({"space": ada.space}), Some(&bo.token));
    assert_eq!(status, 403, "no window before an accept");

    // Bo consents.
    let (status, accepted) = d.auth(
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Bo"}),
        Some(&bo.token),
    );
    assert_eq!(status, 200, "{accepted}");
    let seat = accepted["seat"].as_str().expect("seat").to_string();
    assert_eq!(accepted["space"], json!(ada.space));

    // The ticket is spent.
    let (status, _) = d.auth(
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Bo"}),
        Some(&bo.token),
    );
    assert_eq!(status, 403, "a spent ticket is refused");

    // Bo's list of places: his own, and Ada's as a guest.
    let (_, _, spaces) = d.send(
        "GET",
        "/auth/spaces",
        &[("Authorization", &format!("Bearer {}", bo.token))],
        "",
    );
    let via = |space: &str| -> Value {
        spaces
            .as_array()
            .and_then(|a| a.iter().find(|s| s["space"] == json!(space)))
            .map(|s| s["via"].clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(via(&bo.space), "member", "{spaces}");
    assert_eq!(via(&ada.space), "guest", "{spaces}");

    // The window, and what is read through it.
    let window = |space: &str| -> (u16, Value) {
        d.auth("/auth/window", json!({ "space": space }), Some(&bo.token))
    };
    let (status, w) = window(&ada.space);
    assert_eq!(status, 200, "{w}");
    let wtoken = w["token"].as_str().expect("window token").to_string();
    assert!(w["expires_at"].is_number());
    assert_eq!(
        titles(&d.rows(&ada.space, &wtoken, "myNotes")),
        ["Groceries"],
        "the window reads the shared note and nothing else"
    );

    // Bo's own session is not a way in.
    let (status, refused) = d.op(&ada.space, &bo.token, "GET", "myNotes", json!({}));
    assert_eq!((status, code_of(&refused).as_str()), (403, "access_denied"));

    // Read-only: every write is refused through the window.
    for (method, op, params) in [
        (
            "POST",
            "createNote",
            json!({"title": "x", "body": "y", "tags": [], "created": stamp()}),
        ),
        (
            "POST",
            "editNote",
            json!({"id": groceries, "rev": 1, "title": "x", "body": "y", "tags": []}),
        ),
        ("DELETE", "deleteNote", json!({"id": groceries})),
        (
            "DELETE",
            "unshareNote",
            json!({"note": groceries, "member": seat}),
        ),
        (
            "POST",
            "grantNotesAccess",
            json!({"note": secret, "member": seat, "level": "read"}),
        ),
    ] {
        let (status, out) = d.op(&ada.space, &wtoken, method, op, params);
        assert_eq!(status, 403, "{op} through a window: {out}");
    }
    // Nothing changed.
    assert_eq!(titles(&d.rows(&ada.space, &ada.token, "myNotes")).len(), 3);

    // A window token is no good in Bo's own tenant, nor is it a session.
    let (status, _) = d.op(&bo.space, &wtoken, "GET", "myNotes", json!({}));
    assert_ne!(status, 200, "a window token reached another tenant");
    let (status, _) = d.auth("/auth/window", json!({"space": ada.space}), Some(&wtoken));
    assert_ne!(status, 200, "a window opened a window");

    // A second note from the same sharer: one more consent to the note, the
    // same seat, and both notes are read.
    let second = share(&secret, "Diary");
    let (status, accepted) = d.auth(
        "/auth/share/accept",
        json!({"ticket": second, "display_name": "Bo"}),
        Some(&bo.token),
    );
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(
        accepted["seat"],
        json!(seat),
        "one seat per person per tenant"
    );
    let (_, w) = window(&ada.space);
    let wtoken = w["token"].as_str().expect("window token").to_string();
    let mut both = titles(&d.rows(&ada.space, &wtoken, "myNotes"));
    both.sort();
    assert_eq!(both, ["Diary", "Groceries"]);

    // Ada sees who holds a note: the derived list of access rows.
    let (status, acl) = d.op(
        &ada.space,
        &ada.token,
        "GET",
        "std.notesAccess.list",
        json!({}),
    );
    assert_eq!(status, 200, "{acl}");
    let held: Vec<(String, String, String)> = acl["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter(|r| r["member"] == json!(seat))
        .map(|r| {
            (
                r["note"].as_str().unwrap_or("").to_string(),
                r["level"].as_str().unwrap_or("").to_string(),
                r["id"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(held.len(), 2, "{acl}");
    assert!(held.iter().all(|(_, level, _)| level == "read"));

    // Ada unshares one note: it leaves Bo's next load.
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "DELETE",
        "unshareNote",
        json!({"note": secret, "member": seat}),
    );
    assert_eq!(status, 200, "{out}");
    assert_eq!(
        titles(&d.rows(&ada.space, &wtoken, "myNotes")),
        ["Groceries"],
        "an unshared note is gone at the next load"
    );

    // Ada closes the window: the next call is refused, and no new window
    // can be opened.
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.space.revokeShare",
        json!({ "seat": seat }),
    );
    assert_eq!(status, 200, "{out}");
    let (status, refused) = d.op(&ada.space, &wtoken, "GET", "myNotes", json!({}));
    assert_eq!((status, code_of(&refused).as_str()), (403, "access_denied"));
    let (status, _) = window(&ada.space);
    assert_eq!(status, 403, "a closed window cannot be opened again");
    let (_, _, spaces) = d.send(
        "GET",
        "/auth/spaces",
        &[("Authorization", &format!("Bearer {}", bo.token))],
        "",
    );
    assert_eq!(spaces.as_array().map(Vec::len), Some(1), "{spaces}");

    // Ada's own notes are untouched, and Bo's tenant is his.
    assert_eq!(d.rows(&ada.space, &ada.token, "myNotes").len(), 3);
    assert!(d.rows(&bo.space, &bo.token, "myNotes").is_empty());
    d.stop();
}

/// Closing a whole window reaches a feed already open on it, as taking back
/// one note does: `std.space.revokeShare` signals the entities the share
/// touched, so the reader's open `?live=1` feed hears it at once and does not
/// wait for its next reconnect.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_share_window_reaches_a_feed_open_on_it() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_window_feed", Some(PAGE)).await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let bo = sign_up(&d, &device(22, "bo"), "Bo");
    let groceries = create_note(&d, &ada, "Groceries", "milk");
    let (status, out) = d.op(
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
    assert_eq!(status, 200, "{out}");
    let ticket = out["rows"][0]["ticket"]
        .as_str()
        .expect("ticket")
        .to_string();
    let (status, accepted) = d.auth(
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Bo"}),
        Some(&bo.token),
    );
    assert_eq!(status, 200, "{accepted}");
    let seat = accepted["seat"].as_str().expect("seat").to_string();
    let (status, w) = d.auth("/auth/window", json!({"space": ada.space}), Some(&bo.token));
    assert_eq!(status, 200, "{w}");
    let window = Session {
        token: w["token"].as_str().expect("window token").to_string(),
        space: ada.space.clone(),
        raw: json!(null),
    };

    // Bo's feed on the window carries the shared note.
    let mut feed = open_feed(&d, &window);
    // Drain what the feed already carries, so only a signal after the close
    // counts.
    feed.set_read_timeout(Some(Duration::from_millis(700))).ok();
    let mut buf = [0u8; 4096];
    while matches!(feed.read(&mut buf), Ok(n) if n > 0) {}

    // Ada closes the whole window.
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.space.revokeShare",
        json!({ "seat": seat }),
    );
    assert_eq!(status, 200, "{out}");

    // The open feed is told, with no reconnect. The close signals the feed,
    // the feed asks the door again before it answers, and the door refuses:
    // the feed's last frame is the refusal, and nothing follows it.
    let mut heard = String::new();
    let until = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < until && !heard.contains("event: error") {
        match feed.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => heard.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(_) => {}
        }
    }
    assert!(
        heard.contains("event: error") && heard.contains("\"code\":\"access_denied\""),
        "closing the window never reached the open feed; it heard: {heard:?}\n{}",
        d.log()
    );
    assert!(
        !heard.contains("event: invalidate"),
        "nothing but the refusal reaches a closed window's feed: {heard:?}"
    );
    d.stop();
}

/// Notes are listed newest first by the moment the page stamped them. The ids
/// the platform mints do not order by age past nine, so the program sorts by
/// `created`, which the page supplies.
#[tokio::test(flavor = "multi_thread")]
async fn notes_are_listed_newest_first_past_ten() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_order", Some(PAGE)).await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    for i in 0..12 {
        create_note(&d, &ada, &format!("n{i}"), "b");
    }
    let rows = d.rows(&ada.space, &ada.token, "myNotes");
    let want: Vec<String> = (0..12).rev().map(|i| format!("n{i}")).collect();
    assert_eq!(titles(&rows), want);
    d.stop();
}

/// 25c. The door listens for a write on each database a tenant lives in. A
/// tenant made after the deployment booted has a database the boot never
/// listened to, so a `?live=1` feed on it carried its first load and then
/// only heartbeats. Creating the tenant now adds the listener.
#[tokio::test(flavor = "multi_thread")]
async fn a_feed_on_a_tenant_made_after_boot_carries_a_note_written_later() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_feed", Some(PAGE)).await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");

    // The feed, opened before the write, on a raw socket the test keeps.
    let mut feed = TcpStream::connect(("127.0.0.1", d.port)).expect("connect");
    let bearer = format!("Bearer {}", ada.token);
    let request = format!(
        "GET /op/myNotes?live=1 HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\
         X-Ordo-Space: {}\r\nAuthorization: {bearer}\r\nOrigin: {PAGE}\r\n\r\n",
        ada.space
    );
    feed.write_all(request.as_bytes()).expect("write");
    feed.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let heard = Arc::new(std::sync::Mutex::new(String::new()));
    // Where the write happened in what was heard; nothing before it counts.
    let mark = Arc::new(AtomicUsize::new(usize::MAX));
    let (sink, from) = (heard.clone(), mark.clone());
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let until = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < until {
            match feed.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => sink
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(_) => {}
            }
            let at = from.load(Ordering::SeqCst);
            let text = sink.lock().unwrap();
            if at <= text.len() && text[at..].contains("event: invalidate") {
                break;
            }
        }
    });
    // The first load has arrived, and nothing else has.
    eventually("the feed answers", || {
        heard.lock().unwrap().contains("\"rows\"").then_some(())
    });
    std::thread::sleep(Duration::from_millis(500));
    mark.store(heard.lock().unwrap().len(), Ordering::SeqCst);

    create_note(&d, &ada, "written after the feed opened", "b");
    reader.join().expect("reader");
    let text = heard.lock().unwrap().clone();
    let at = mark.load(Ordering::SeqCst);
    assert!(
        text.len() >= at && text[at..].contains("event: invalidate"),
        "a write to the tenant never reached its feed; it heard: {text}\n{}",
        d.log()
    );
    d.stop();
}

// ── 25f, the hung server ─────────────────────────────────────────────────

/// A live feed on a raw socket, read until its first load has arrived.
fn open_feed(d: &Deployment, s: &Session) -> TcpStream {
    open_feed_on(d, s, "myNotes")
}

/// A live feed on `op`, read up to its first rows.
fn open_feed_on(d: &Deployment, s: &Session, op: &str) -> TcpStream {
    let mut feed = TcpStream::connect(("127.0.0.1", d.port)).expect("connect");
    let request = format!(
        "GET /op/{op}?live=1 HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\
         X-Ordo-Space: {}\r\nAuthorization: Bearer {}\r\nOrigin: {PAGE}\r\n\r\n",
        s.space, s.token
    );
    feed.write_all(request.as_bytes()).expect("write");
    feed.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = [0u8; 4096];
    let mut heard = String::new();
    while !heard.contains("\"rows\"") {
        let n = feed.read(&mut buf).expect("the feed answers");
        assert!(n > 0, "the feed closed at once");
        heard.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    feed
}

/// `/health/live` on a fresh connection, failed if it takes over 5 s.
fn live(d: &Deployment, what: &str) {
    let mut s = TcpStream::connect(("127.0.0.1", d.port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(b"GET /health/live HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write");
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    assert!(
        raw.starts_with("HTTP/1.1 200"),
        "/health/live did not answer {what}: {raw:?}\n{}",
        d.log()
    );
}

/// 25f (a). A server told to stop while a live feed is open stops accepting
/// and exits within its grace, and the feed is cut.
#[tokio::test(flavor = "multi_thread")]
async fn sigterm_with_a_live_feed_open_ends_the_process_within_its_grace() {
    let _shared = shared().await;
    let mut d = Deployment::boot_with(
        "ordo_notes_sigterm",
        Some(PAGE),
        &[("ORDO_SHUTDOWN_GRACE_SECS", "3")],
    )
    .await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let mut feed = open_feed(&d, &ada);
    // A second feed from a window that was then abandoned: never read again,
    // never closed.
    let _abandoned = open_feed(&d, &ada);
    live(&d, "before the signal");

    let told = std::time::Instant::now();
    Command::new("kill")
        .args(["-TERM", &d.child.id().to_string()])
        .status()
        .expect("kill");
    let mut exited = None;
    while told.elapsed() < Duration::from_secs(15) {
        if let Some(status) = d.child.try_wait().expect("wait") {
            exited = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        exited.is_some(),
        "the server was still running 15 s after SIGTERM (grace 3 s)\n{}",
        d.log()
    );
    // The feed was cut.
    feed.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut buf = [0u8; 4096];
    let cut = loop {
        match feed.read(&mut buf) {
            Ok(0) => break true,
            Ok(_) => {}
            Err(_) => break false,
        }
    };
    assert!(cut, "the feed was still open after the process exited");
}

/// 25f, the cause. The page opens a share link and asks `/auth/share/peek`
/// more than once at a time (a development page runs its effects twice, and
/// a person opens the link in two windows). Each call reads the ticket
/// table through a store whose answer travels over a connection the door's
/// own runtime drives. A call that waits for that answer on a runtime
/// worker, instead of on a thread of its own, stops the workers that would
/// have read it, and the whole server stops accepting with every thread
/// idle.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_share_peeks_never_stop_the_door_accepting() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_peeks", Some(PAGE)).await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let note = create_note(&d, &ada, "Groceries", "milk");
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.space.share",
        json!({
            "ops": "myNotes",
            "rows": json!([{"entity": "notes", "id": note, "level": "read"}]).to_string(),
            "expires": "365d",
            "label": "Groceries",
        }),
    );
    assert_eq!(status, 200, "{out}");
    let ticket = out["rows"][0]["ticket"]
        .as_str()
        .expect("a ticket")
        .to_string();
    let port = d.port;
    for round in 0..20 {
        let callers: Vec<_> = (0..16)
            .map(|_| {
                let ticket = ticket.clone();
                std::thread::spawn(move || {
                    let body = json!({ "ticket": ticket }).to_string();
                    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
                    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
                    let request = format!(
                        "POST /auth/share/peek HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
                         Origin: {PAGE}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    s.write_all(request.as_bytes()).expect("write");
                    let mut raw = String::new();
                    let _ = s.read_to_string(&mut raw);
                    raw.starts_with("HTTP/1.1 200")
                })
            })
            .collect();
        let answered = callers
            .into_iter()
            .map(|c| c.join().expect("caller"))
            .filter(|ok| *ok)
            .count();
        assert_eq!(answered, 16, "round {round}: peeks went unanswered");
        live(&d, &format!("after round {round} of peeks"));
    }
    d.stop();
}

/// 25f (b). The owner's sequence: sign-ups, a share, an accept, more
/// sign-ups, with a feed open. The door keeps answering.
#[tokio::test(flavor = "multi_thread")]
async fn the_door_keeps_answering_through_sign_ups_a_share_and_an_accept_with_a_feed_open() {
    let _shared = shared().await;
    let d = Deployment::boot_with("ordo_notes_keeps", Some(PAGE), &[]).await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    let _feed = open_feed(&d, &ada);
    live(&d, "after the first sign-up");
    let bo = sign_up(&d, &device(22, "bo"), "Bo");
    live(&d, "after the second sign-up");
    let note = create_note(&d, &ada, "Groceries", "milk");
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.space.share",
        json!({
            "ops": "myNotes",
            "rows": json!([{"entity": "notes", "id": note, "level": "read"}]).to_string(),
            "expires": "365d",
            "label": "Groceries",
        }),
    );
    assert_eq!(status, 200, "{out}");
    let ticket = out["rows"][0]["ticket"].as_str().expect("a ticket");
    let (status, _) = d.auth("/auth/share/peek", json!({ "ticket": ticket }), None);
    assert_eq!(status, 200);
    let (status, accepted) = d.auth(
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Bo"}),
        Some(&bo.token),
    );
    assert_eq!(status, 200, "{accepted}");
    live(&d, "after the accept");
    for i in 0..4u8 {
        sign_up(&d, &device(40 + i, &format!("p{i}")), &format!("P{i}"));
        live(&d, &format!("after later sign-up {i}"));
    }
    d.stop();
}

// ── 25g, what a tenant's database costs the server ───────────────────────

/// Server connections this deployment holds, per database: the control
/// database and one per tenant. Read from `pg_stat_activity`, which is the
/// server's own count and the one `max_connections` is enforced against.
fn connections_by_database(d: &Deployment) -> Vec<(String, usize)> {
    let pg = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let control = Pg {
        db: d.db.clone(),
        ..Pg::default()
    };
    let mut names = vec![d.db.clone()];
    for row in control
        .query("SELECT \"$space\" FROM \"$spaces\"")
        .unwrap_or_default()
    {
        if let Some(Some(id)) = row.first() {
            names.push(id.clone());
        }
    }
    let list = names
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(",");
    pg.query(&format!(
        "SELECT datname, count(*) FROM pg_stat_activity \
         WHERE datname IN ({list}) AND pid <> pg_backend_pid() GROUP BY 1 ORDER BY 1"
    ))
    .expect("pg_stat_activity")
    .into_iter()
    .map(|r| {
        (
            r[0].clone().unwrap_or_default(),
            r[1].clone().unwrap_or_default().parse().unwrap_or(0),
        )
    })
    .collect()
}

/// Connections held by the tenants' databases, not the control database.
fn tenant_connections(d: &Deployment) -> usize {
    connections_by_database(d)
        .into_iter()
        .filter(|(name, _)| *name != d.db)
        .map(|(_, n)| n)
        .sum()
}

/// 25g. Thirty people sign up on a server whose `max_connections` is the
/// default 100. Every sign-up succeeds, and afterwards the tenants hold few
/// connections between them, because a tenant that is not being used holds
/// none.
#[tokio::test(flavor = "multi_thread")]
async fn connections_thirty_people_sign_up_on_a_default_server() {
    let _alone = SERVER.write().await;
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let max: usize = admin
        .query("SHOW max_connections")
        .expect("max_connections")[0][0]
        .clone()
        .unwrap_or_default()
        .parse()
        .expect("a number");
    assert_eq!(max, 100, "this test proves the fix on the default server");
    let d = Deployment::boot_with(
        "ordo_notes_thirty",
        Some(PAGE),
        &[("ORDO_PROVISION_SIGNUPS_PER_HOUR", "1000")],
    )
    .await;
    for i in 0..30u8 {
        let s = sign_up(&d, &device(100 + i, &format!("p{i}")), &format!("P{i}"));
        let held = connections_by_database(&d);
        eprintln!(
            "after sign-up {i}: {} tenant connections, control {}",
            tenant_connections(&d),
            held.iter()
                .find(|(n, _)| *n == d.db)
                .map(|(_, n)| *n)
                .unwrap_or(0)
        );
        create_note(&d, &s, "a first note", "b");
    }
    let held = connections_by_database(&d);
    eprintln!("per database: {held:?}");
    let total: usize = held.iter().map(|(_, n)| n).sum();
    assert!(
        total < max / 2,
        "thirty tenants hold {total} of {max} connections between them: {held:?}"
    );
    d.stop();
}

/// 25g. A tenant that is not being used holds no connection. A person signs
/// up, writes, and opens a feed, which holds a pool connection and a
/// listener. The feed closes, and within the idle period the tenant's
/// connections are all released. The tenant then works again, on connections
/// dialled when it asked, and a new feed still hears a write.
#[tokio::test(flavor = "multi_thread")]
async fn connections_an_idle_tenant_releases_its_connections_within_the_idle_period() {
    let _alone = SERVER.write().await;
    let idle_secs = 2u64;
    let d = Deployment::boot_with(
        "ordo_notes_idle",
        Some(PAGE),
        &[
            ("ORDO_PG_IDLE_SECS", &idle_secs.to_string()),
            ("ORDO_PG_POOL_SIZE", "6"),
        ],
    )
    .await;
    let ada = sign_up(&d, &device(21, "ada"), "Ada");
    create_note(&d, &ada, "one", "b");
    let feed = open_feed(&d, &ada);
    let in_use = tenant_connections(&d);
    eprintln!("in use, a feed open: {in_use} tenant connections");
    assert!(in_use >= 2, "a feed holds a listener and a pool connection");

    // The feed goes away. The clock starts at the last thing the tenant did.
    drop(feed);
    let closed = std::time::Instant::now();
    let mut left = tenant_connections(&d);
    while left > 0 && closed.elapsed() < Duration::from_secs(idle_secs * 4 + 4) {
        std::thread::sleep(Duration::from_millis(100));
        left = tenant_connections(&d);
    }
    let took = closed.elapsed();
    eprintln!("released after {took:?}, {left} left");
    assert_eq!(
        left,
        0,
        "an idle tenant still holds connections {took:?} after its feed closed: {:?}\n{}",
        connections_by_database(&d),
        d.log()
    );
    assert!(
        took <= Duration::from_secs(idle_secs * 2 + 2),
        "released, but only after {took:?} with an idle period of {idle_secs}s"
    );

    // Nothing was lost by letting go: the tenant dials again when asked.
    let feed = open_feed(&d, &ada);
    create_note(&d, &ada, "two", "b");
    let mut feed = feed;
    feed.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut heard = String::new();
    let mut buf = [0u8; 4096];
    while !heard.contains("event: invalidate") {
        let n = feed.read(&mut buf).expect("the feed carries the write");
        assert!(n > 0, "the feed closed");
        heard.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    drop(feed);
    d.stop();
}

// ── Tags, a JSON array ───────────────────────────────────────────────────

/// `notesTagged` answers the notes whose `tags` array holds the word, and no
/// other. A tag is matched as a whole element, so `work` does not match
/// `homework`.
#[tokio::test(flavor = "multi_thread")]
async fn notes_tagged_lists_only_the_notes_whose_tags_hold_the_word() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_tags", Some(PAGE)).await;
    let ada = sign_up(&d, &device(31, "ada-tags"), "Ada");
    for (title, tags) in [
        ("Standup", json!(["work"])),
        ("Groceries", json!(["home"])),
        ("Maths", json!(["homework", "school"])),
        ("Plan", json!(["work", "ideas"])),
    ] {
        let (status, out) = d.op(
            &ada.space,
            &ada.token,
            "POST",
            "createNote",
            json!({"title": title, "body": "", "tags": tags, "created": stamp()}),
        );
        assert_eq!(status, 200, "{out}");
    }
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "GET",
        "notesTagged",
        json!({"tag": "work"}),
    );
    assert_eq!(status, 200, "{out}");
    let rows = out["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(titles(&rows), vec!["Plan", "Standup"], "newest first");
    assert_eq!(
        rows[0]["tags"],
        json!(["work", "ideas"]),
        "the array comes back as JSON"
    );
    let mine = d.rows(&ada.space, &ada.token, "myNotes");
    assert_eq!(mine.len(), 4, "{mine:?}");
    d.stop();
}

// ── Other ways in ────────────────────────────────────────────────────────

/// The code in the `n`th message the relay received, as the person reads it.
fn code_in_mail(relay: &mail_support::FakeRelay, n: usize) -> String {
    let (data, _) = eventually("the mail worker sends the code", || {
        relay.sessions().get(n).and_then(|s| s.data.clone())
    });
    let start = data.find("code is ").expect("the code line") + "code is ".len();
    data[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

/// `beginEmailSignIn`, then `completeEmailSignIn` with the code the relay got.
fn sign_in_by_email(
    d: &Deployment,
    relay: &mail_support::FakeRelay,
    n: usize,
    address: &str,
    device: Option<&str>,
) -> Session {
    let mut begin = json!({"method": "email", "identifier": address});
    if let Some(marker) = device {
        begin["device"] = json!(marker);
    }
    let (status, begun) = d.auth("/auth/begin", begin, None);
    assert_eq!(status, 200, "{begun}");
    let code = code_in_mail(relay, n);
    assert_eq!(code.len(), 8, "eight digits by default");
    let (status, done) = d.auth(
        "/auth/complete",
        json!({
            "method": "email", "kind": "email_code",
            "challenge": begun["challenge"], "binder": begun["binder"], "code": code,
        }),
        None,
    );
    session_of(d, status, done)
}

/// A deployment with `ORDO_AUTH_METHODS=passkey,email` sends a code through
/// its mail channel. The first sign-in makes an account and a tenant of the
/// person's own, as a passkey sign-up does. A second code, sent with the
/// device marker the first answered, lands them in the same tenant with
/// their note.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_signs_in_with_an_emailed_code_and_a_later_code_finds_the_same_notes() {
    let _shared = shared().await;
    let relay = mail_support::FakeRelay::start_plaintext().await;
    let port = relay.port.to_string();
    let d = Deployment::boot_with(
        "ordo_notes_email",
        Some(PAGE),
        &[
            ("ORDO_AUTH_METHODS", "passkey,email"),
            ("ORDO_MAIL_KIND", "smtp"),
            ("ORDO_MAIL_FROM", "noreply@example.com"),
            ("ORDO_MAIL_HOST", "localhost"),
            ("ORDO_MAIL_PORT", &port),
            ("ORDO_MAIL_TLS", "none"),
            ("ORDO_MAIL_SEND_SECS", "1"),
            ("ORDO_EGRESS_PRIVATE_HOSTS", "localhost"),
        ],
    )
    .await;

    let kim = sign_in_by_email(&d, &relay, 0, "kim@example.org", None);
    assert_eq!(kim.raw["via"], "email-otp", "{}", kim.raw);
    let marker = kim.raw["device"]
        .as_str()
        .expect("a device marker for the page to keep")
        .to_string();
    settle(&d, &kim);
    create_note(&d, &kim, "By email", "");

    let again = sign_in_by_email(&d, &relay, 1, "kim@example.org", Some(&marker));
    assert_eq!(again.space, kim.space, "the same tenant");
    assert_eq!(principal_of(&again.token), principal_of(&kim.token));
    assert_eq!(
        titles(&d.rows(&again.space, &again.token, "myNotes")),
        vec!["By email"]
    );
    d.stop();
}

/// Notes v2, with its reminders.
const V2: &str = include_str!("../../../examples/notes-v2/notes.ordo");

/// Wait up to `secs` for `probe`, for what a 30 second schedule decides.
fn within<T>(secs: u64, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let started = std::time::Instant::now();
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(
            started.elapsed() < Duration::from_secs(secs),
            "still waiting after {secs} s: {what}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Who a relay session was addressed to.
fn recipients(s: &mail_support::Session) -> Vec<String> {
    s.commands
        .iter()
        .filter_map(|c| c.strip_prefix("RCPT TO:"))
        .map(|a| a.trim().trim_matches(['<', '>']).to_lowercase())
        .collect()
}

/// Notes v2's reminders (owner ruling 2026-10-22), on two servers that share
/// one database. Kim, who signs in by email, shares a note with Lee and sets
/// a reminder on it that is already due. The `fireReminders` routine marks it
/// fired once, and `notify admin` mails Kim once: the schedule fires once for
/// the deployment, and the outbox is drained by one server per message. Lee
/// holds `read` on the note and is mailed nothing. A later period finds the
/// reminder fired and sends nothing more.
#[tokio::test(flavor = "multi_thread")]
async fn a_due_reminder_fires_once_and_mails_only_the_owner_with_two_servers_running() {
    let _shared = shared().await;
    let relay = mail_support::FakeRelay::start_plaintext().await;
    let port = relay.port.to_string();
    // v2 keeps files on notes, so the door needs a blob store.
    let blobs = format!(
        "fs:{}",
        std::env::temp_dir()
            .join(format!("ordo-notes-remind-{}", std::process::id()))
            .display()
    );
    let settings = [
        ("ORDO_BLOB_STORE", blobs.as_str()),
        ("ORDO_DEV_UNSAFE_BLOB_STORE", "1"),
        ("ORDO_AUTH_METHODS", "passkey,email"),
        ("ORDO_MAIL_KIND", "smtp"),
        ("ORDO_MAIL_FROM", "noreply@example.com"),
        ("ORDO_MAIL_HOST", "localhost"),
        ("ORDO_MAIL_PORT", port.as_str()),
        ("ORDO_MAIL_TLS", "none"),
        ("ORDO_MAIL_SEND_SECS", "1"),
        ("ORDO_EGRESS_PRIVATE_HOSTS", "localhost"),
    ];
    let d = Deployment::boot_program("ordo_notes_remind", V2, Some(PAGE), &settings).await;
    let second = d.replica(Some(PAGE), &settings);
    assert!(
        d.log().contains("fireReminders (every 30s"),
        "the routine is wired at boot:\n{}",
        d.log()
    );

    let kim = sign_in_by_email(&d, &relay, 0, "kim@example.org", None);
    settle(&d, &kim);
    let lee = sign_in_by_email(&d, &relay, 1, "lee@example.org", None);
    settle(&d, &lee);
    // Ada signs up with a passkey, which sharing asks for. She has no email
    // sign-in, so her reminders are told in the app only.
    let ada = sign_up(&d, &device(41, "ada"), "Ada");
    settle(&d, &ada);
    let note = create_note(&d, &kim, "Call the dentist", "");
    let shared = create_note(&d, &ada, "Book the venue", "");

    // Ada shares her note with Lee, read-only. Lee signs in by email, so
    // only `notify admin` keeps the reminder from him.
    let (status, out) = d.op(
        &ada.space,
        &ada.token,
        "POST",
        "std.space.share",
        json!({
            "ops": "allNotes",
            "rows": json!([{"entity": "notes", "id": shared, "level": "read"}]).to_string(),
            "expires": "365d",
            "label": "Venue",
        }),
    );
    assert_eq!(status, 200, "{out}");
    let ticket = out["rows"][0]["ticket"].as_str().expect("a ticket");
    let (status, accepted) = d.auth(
        "/auth/share/accept",
        json!({"ticket": ticket, "display_name": "Lee"}),
        Some(&lee.token),
    );
    assert_eq!(status, 200, "{accepted}");

    // A reminder already due on each note.
    for (who, on) in [(&kim, &note), (&ada, &shared)] {
        let (status, out) = d.op(
            &who.space,
            &who.token,
            "POST",
            "setReminder",
            json!({"note": on, "due": "2026-01-01T09:00:00Z"}),
        );
        assert_eq!(status, 200, "{out}");
    }

    let fired_on = |d: &Deployment, who: &Session, on: &str| -> Option<String> {
        let rows = d.rows(&who.space, &who.token, "myReminders");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["note"], json!(on));
        rows[0]["fired"].as_str().map(str::to_string)
    };
    let fired = |d: &Deployment| fired_on(d, &kim, &note);
    // A page reads its reminders live. The routine's write must reach it:
    // the feed says `invalidate`, and the page reads `myReminders` again.
    let mut feed = open_feed_on(&d, &kim, "myReminders");
    within(75, "the routine marks the reminder fired", || fired(&d));
    let mut heard = String::new();
    let mut buf = [0u8; 4096];
    within(
        30,
        "the live feed tells the page to read its reminders again",
        || {
            if let Ok(n) = feed.read(&mut buf) {
                heard.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            heard.contains("event: invalidate").then_some(())
        },
    );
    within(30, "and Ada's", || fired_on(&d, &ada, &shared));
    let notice = within(30, "the notice reaches the relay", || {
        relay
            .sessions()
            .get(2)
            .filter(|s| s.data.is_some())
            .cloned()
    });
    assert_eq!(
        recipients(&notice),
        vec!["kim@example.org"],
        "only the owner"
    );
    let data = notice.data.expect("a message");
    assert!(
        data.contains("Subject: A reminder on one of your notes is due"),
        "{data}"
    );
    assert!(!data.contains("dentist"), "nothing from the note: {data}");

    // One more period: still fired once, and nothing more is sent.
    let first = fired(&d);
    std::thread::sleep(Duration::from_secs(35));
    assert_eq!(fired(&d), first, "fired once");
    let sessions = relay.sessions();
    assert_eq!(sessions.len(), 3, "two codes and one notice");
    for s in sessions
        .iter()
        .filter(|s| recipients(s) == ["lee@example.org"])
    {
        let m = s.data.as_deref().unwrap_or_default();
        assert!(
            m.contains("code is"),
            "Lee is mailed his code and nothing else: {m}"
        );
    }
    let queued = format!(
        "{}{}",
        d.log(),
        std::fs::read_to_string(&second.log).unwrap_or_default()
    )
    .matches("routine fireReminders: queued 1 notices for 1 people")
    .count();
    assert_eq!(
        queued, 1,
        "one run told one person, on one of the two servers"
    );
    drop(second);
    d.stop();
}

/// With `ORDO_AUTH_GUEST=on` the page's guest button makes an account and a
/// tenant at once. The guest writes a note, then adds a passkey by a claim:
/// the guest secret is the first proof and the new passkey the second. The
/// passkey then signs in to the same account, tenant and note, and the door
/// lists it, so the page stops offering to add one.
#[tokio::test(flavor = "multi_thread")]
async fn a_guest_adds_a_passkey_and_the_passkey_signs_in_to_the_same_notes() {
    let _shared = shared().await;
    let d =
        Deployment::boot_with("ordo_notes_guest", Some(PAGE), &[("ORDO_AUTH_GUEST", "on")]).await;

    let (status, started) = d.auth(
        "/auth/complete",
        json!({"method": "guest", "kind": "new", "display": "Guest"}),
        None,
    );
    let secret = started["guest_secret"]
        .as_str()
        .expect("the secret, once")
        .to_string();
    let guest = session_of(&d, status, started);
    assert_eq!(guest.raw["via"], "guest", "{}", guest.raw);
    settle(&d, &guest);
    create_note(&d, &guest, "Kept", "");

    let passkeys = |s: &Session| -> usize {
        let bearer = format!("Bearer {}", s.token);
        let (status, _, list) = d.send(
            "GET",
            "/auth/authenticators",
            &[("Authorization", &bearer), ("Origin", PAGE)],
            "",
        );
        assert_eq!(status, 200, "{list}");
        list["authenticators"]
            .as_array()
            .expect("rows")
            .iter()
            .filter(|r| r["kind"] == "passkey" && r["state"] == "active")
            .count()
    };
    assert_eq!(passkeys(&guest), 0, "a guest starts with no passkey");

    // `beginClaim` with `guestCeremony(secret)`, then `claimPasskey`.
    let (status, opened) = d.auth(
        "/auth/claim/begin",
        json!({"method": "guest", "kind": "resume", "secret": secret}),
        Some(&guest.token),
    );
    assert_eq!(status, 200, "{opened}");
    let claim = opened["claim"].as_str().expect("a claim").to_string();
    let (status, options) = d.auth(
        "/auth/claim/passkey",
        json!({"claim": claim}),
        Some(&guest.token),
    );
    assert_eq!(status, 200, "{options}");
    let key = device(32, "guest-passkey");
    let CompleteRequest::PasskeyRegistration {
        challenge,
        client_data_json,
        attestation_object,
    } = key.register(options["challenge"].as_str().expect("challenge"))
    else {
        panic!("not a registration");
    };
    let (status, linked) = d.auth(
        "/auth/claim/complete",
        json!({
            "claim": claim,
            "method": "passkey", "kind": "passkey_registration", "challenge": challenge.0,
            "client_data_json": b64url_encode(&client_data_json),
            "attestation_object": b64url_encode(&attestation_object),
        }),
        Some(&guest.token),
    );
    assert_eq!(status, 200, "{linked}");

    let back = sign_in(&d, &key);
    assert_eq!(principal_of(&back.token), principal_of(&guest.token));
    assert_eq!(back.space, guest.space, "the same tenant");
    assert_eq!(
        titles(&d.rows(&back.space, &back.token, "myNotes")),
        vec!["Kept"]
    );
    assert_eq!(passkeys(&back), 1, "the door lists the new passkey");
    d.stop();
}

/// The binary mounts every sign-in route the page and the SDK call. Until
/// 2026-10-10 the claim, hand-off, callback and identity routes were mounted
/// only on the in-process router the other suites use, and the binary
/// answered them 404. A route that is there refuses an empty body in its own
/// words; only a missing one answers 404 or 405.
#[tokio::test(flavor = "multi_thread")]
async fn the_served_door_mounts_the_claim_hand_off_callback_and_identity_routes() {
    let _shared = shared().await;
    let d = Deployment::boot("ordo_notes_routes", Some(PAGE)).await;
    for (method, path) in [
        ("POST", "/auth/claim/begin"),
        ("POST", "/auth/claim/passkey"),
        ("POST", "/auth/claim/complete"),
        ("POST", "/auth/handoff"),
        ("POST", "/auth/handoff/redeem"),
        ("GET", "/auth/identities"),
        ("GET", "/auth/callback/discord?code=x&state=y"),
        ("GET", "/auth/callback/t/acme/sso?code=x&state=y"),
    ] {
        let body = if method == "POST" { "{}" } else { "" };
        let (status, _, answer) = d.send(
            method,
            path,
            &[("Content-Type", "application/json"), ("Origin", PAGE)],
            body,
        );
        assert!(
            status != 404 && status != 405,
            "{method} {path} is not mounted: {status} {answer}"
        );
    }
    d.stop();
}

// ── guard-equality-uses-no-index: the 5,000-note benchmark ───────────────

/// Notes in the benchmark tenant, and access rows: one each.
const BENCH_NOTES: usize = 5_000;

/// `EXPLAIN (ANALYZE)` of `sql` in `pg`, as text, and its execution time.
/// `set` runs first in the same session (a planner setting, or nothing).
fn explain(pg: &Pg, set: &str, sql: &str) -> (String, f64) {
    let rows = pg
        .query(&format!(
            "{set}EXPLAIN (ANALYZE, BUFFERS) {}",
            sql.trim_end_matches(';')
        ))
        .expect("explain runs");
    let text: Vec<String> = rows
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    let ms = text
        .iter()
        .find_map(|l| l.strip_prefix("Execution Time: "))
        .and_then(|l| l.trim_end_matches(" ms").parse::<f64>().ok())
        .expect("an execution time");
    (text.join("\n"), ms)
}

/// The median of `n` runs of `f`, in milliseconds.
fn median_ms(n: usize, mut f: impl FnMut()) -> f64 {
    let mut times: Vec<f64> = (0..n)
        .map(|_| {
            let t = std::time::Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    times[n / 2]
}

/// Board task `guard-equality-uses-no-index` (audit 2026-10-25, item 1).
///
/// Notes v2 on the real binary, one person with 5,000 notes and 5,000
/// `notesAccess` rows. It prints the plan of `myNotes` compiled both ways
/// (every `==` as `IS NOT DISTINCT FROM`, and by the never-null rule) with
/// `EXPLAIN ANALYZE`, the door's read time, and a device sync's load time
/// (`?live=1&sync=1`, 50 rows a page, no wait between pages).
///
/// It asserts what the fix promises, on whichever form the server compiles:
/// `notesAccess` read once for the list rather than once per note, an index
/// on it usable (shown with sequential scans priced out, since at this size
/// Postgres rightly prefers one hashed pass over the table), a read under
/// 10 ms and a sync under 3 s.
/// Slow and timing-bound, so `#[ignore]`: `scripts/guard-index-bench.sh`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a benchmark: scripts/guard-index-bench.sh"]
async fn my_notes_at_five_thousand_notes_uses_the_access_index() {
    let _shared = shared().await;
    let blobs = format!(
        "fs:{}",
        std::env::temp_dir()
            .join(format!("ordo-notes-bench-{}", std::process::id()))
            .display()
    );
    let settings = [
        ("ORDO_BLOB_STORE", blobs.as_str()),
        ("ORDO_DEV_UNSAFE_BLOB_STORE", "1"),
        ("ORDO_SYNC_PAGE_WAIT_MS", "0"),
    ];
    let d = Deployment::boot_program("ordo_notes_guard_bench", V2, Some(PAGE), &settings).await;
    let ada = sign_up(&d, &device(51, "ada"), "Ada");
    let first = create_note(&d, &ada, "The first note", "");

    // The rest are written straight into Ada's tenant: 5,000 notes through
    // the door would measure the write path, not the read.
    let tenant = Pg {
        db: ada.space.clone(),
        ..Pg::default()
    };
    let member = tenant
        .query(&format!(
            "SELECT member FROM \"notesAccess\" WHERE note = '{first}'"
        ))
        .expect("the creator's access row")[0][0]
        .clone()
        .expect("a member");
    let more = BENCH_NOTES - 1;
    tenant
        .exec(&format!(
            "INSERT INTO notes (\"$space\", id, rev, title, body, tags, created, pinned, archived) \
             SELECT \"$space\", 'note_bench_' || g, 1, 'Note ' || g, '', tags, \
                    created + make_interval(secs => g), false, NULL \
             FROM notes, generate_series(1, {more}) g WHERE id = '{first}'; \
             INSERT INTO \"notesAccess\" (\"$space\", id, rev, note, member, level) \
             SELECT \"$space\", 'access_bench_' || g, 1, 'note_bench_' || g, member, level \
             FROM \"notesAccess\", generate_series(1, {more}) g WHERE note = '{first}'; \
             ANALYZE;"
        ))
        .expect("the benchmark rows");

    // The SQL compiled both ways, for Ada, and Postgres's plan of each.
    let f = Fixture::from_source(V2, "")
        .expect("notes v2 compiles")
        .in_space(&ada.space);
    let pl = ordo_syntax::parse_pipeline("myNotes").expect("parses");
    let q = ordo_sema::bind_as(
        &f.schema,
        &f.registry,
        &pl,
        &[],
        Some(&member),
        ordo_sema::Clock::CORPUS,
    )
    .expect("binds");
    let legacy = ordo_plan::plan_legacy_equality(&q, &f.plan_space()).expect("plans");
    let proven = ordo_plan::plan_proven_equality(&q, &f.plan_space()).expect("plans");
    let served = ordo_plan::plan(&q, &f.plan_space()).expect("plans");
    let (legacy_plan, legacy_ms) = explain(&tenant, "", &legacy.sql);
    let (proven_plan, proven_ms) = explain(&tenant, "", &proven.sql);
    eprintln!("── myNotes, every == null-safe: {legacy_ms:.1} ms in Postgres\n{legacy_plan}");
    eprintln!("── myNotes, by the never-null rule: {proven_ms:.1} ms in Postgres\n{proven_plan}");
    // Every row of this tenant's access table is Ada's, so Postgres rightly
    // reads it once into a hash rather than probing the index 5,000 times.
    // With sequential scans priced out, each form shows what it CAN use: the
    // null-safe form has nothing but a scan per note, and `=` has the index.
    let off = "SET enable_seqscan = off; ";
    let (legacy_forced, _) = explain(&tenant, off, &legacy.sql);
    let (proven_forced, _) = explain(&tenant, off, &proven.sql);
    eprintln!("── sequential scans off, every == null-safe:\n{legacy_forced}");
    eprintln!("── sequential scans off, by the never-null rule:\n{proven_forced}");

    // Through the door, as the page reads it.
    let (status, body) = d.op(&ada.space, &ada.token, "GET", "myNotes", json!({}));
    assert_eq!(status, 200, "{body}");
    let read_ms = median_ms(9, || {
        let (status, _) = d.op(&ada.space, &ada.token, "GET", "myNotes", json!({}));
        assert_eq!(status, 200);
    });

    // A device sync: every load frame, until the one that says complete.
    let started = std::time::Instant::now();
    let mut feed = TcpStream::connect(("127.0.0.1", d.port)).expect("connect");
    feed.write_all(
        format!(
            "GET /op/myNotes?live=1&sync=1 HTTP/1.1\r\nHost: localhost\r\n\
             Accept: text/event-stream\r\nX-Ordo-Space: {}\r\n\
             Authorization: Bearer {}\r\nOrigin: {PAGE}\r\n\r\n",
            ada.space, ada.token
        )
        .as_bytes(),
    )
    .expect("write");
    feed.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let mut buf = [0u8; 65536];
    let mut heard = String::new();
    while !heard.contains("\"complete\":true") {
        let n = feed.read(&mut buf).expect("the sync answers");
        assert!(n > 0, "the sync closed before it completed");
        heard.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    let sync_ms = started.elapsed().as_secs_f64() * 1000.0;
    let pages = heard.matches("event: load").count();
    drop(feed);

    let served_form = if served.sql == proven.sql {
        "the never-null rule"
    } else {
        "every == null-safe"
    };
    eprintln!(
        "── {BENCH_NOTES} notes, debug build, the server compiling by {served_form}:\n   \
         myNotes through the door: {read_ms:.1} ms (median of 9)\n   \
         sync load: {sync_ms:.0} ms for {pages} pages"
    );

    let (served_plan, served_forced) = if served.sql == proven.sql {
        (&proven_plan, &proven_forced)
    } else {
        (&legacy_plan, &legacy_forced)
    };
    // The defect: the access table read once per note, whatever the scan.
    // Fixed, it is read once for the whole list.
    let per_note = |plan: &str| {
        plan.lines()
            .any(|l| l.contains("\"notesAccess\"") && l.contains(&format!("loops={BENCH_NOTES})")))
    };
    assert!(
        !per_note(served_plan) && !per_note(served_forced),
        "the served form reads notesAccess once per note:\n{served_plan}\n{served_forced}"
    );
    assert!(
        served_forced.contains("Index Scan") && served_forced.contains("notesAccess$"),
        "the served form cannot use an index on notesAccess:\n{served_forced}"
    );
    assert!(read_ms < 10.0, "myNotes took {read_ms:.1} ms");
    assert!(sync_ms < 3_000.0, "the sync took {sync_ms:.0} ms");
    d.stop();
}
