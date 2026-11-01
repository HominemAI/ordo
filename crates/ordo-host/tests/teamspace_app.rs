//! The teamspace app (`examples/teamspace`) against the real `ordo-host`
//! binary and real Postgres.
//!
//! The binary boots on a database that holds the teamspace program and
//! custody, with the settings `examples/teamspace/README.md` gives: passkey
//! sign-in, `ORDO_AUTH_PROVISIONING=auto-onboard` and
//! `ORDO_PROVISION_SELF_SERVE=on-signup`. The test then acts as the page
//! does, with the software authenticator standing in for the browser.
//!
//! What it proves:
//!
//! - **A team workspace.** A founder signs up and gets a tenant, as its
//!   owner. An admin invites a newcomer with `std.space.invite`; the
//!   newcomer signs up with the invitation and joins that tenant as a
//!   member (`editor`).
//! - **Roles.** A member cannot change roles, invite, or manage projects
//!   and teams: the door refuses those ops (`access_denied`). A member reads
//!   only the tasks of projects whose team they are on.
//! - **Admin reports.** `reportByProject` and `reportByPerson` total every
//!   project, including one the admin is not on, with the right numbers for
//!   the data written here. The app's profile template keeps them for owners
//!   and managers, so the door refuses a member before the report runs.
//! - **A tenant's own profile.** The owner makes a group, puts a member in
//!   it, and opens the report to the group; the member then reads it, and a
//!   member outside the group is still refused.
//! - **Invite-only.** Under `ORDO_AUTH_PROVISIONING=invite-only`, a newcomer
//!   with an invitation joins, and a stranger without one is refused.
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
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::CompleteRequest;
use serde_json::{json, Value};

const SOURCE: &str = include_str!("../../../examples/teamspace/teamspace.ordo");

/// The deployment's own space. It holds no teams.
const HOME: &str = "teamspace_home";
const MASTER: &str = "the-teamspace-app-test-master-key";
const TOKEN_KEY: &str = "7777777777777777777777777777777777777777777777777777777777777777";
const PAGE_KEY: &str = "8888888888888888888888888888888888888888888888888888888888888888";

/// The page, as it runs locally: Vite's dev server.
const PAGE: &str = "http://localhost:5174";

/// The app's profile template, as `dev/serve.sh` sets it.
const TEMPLATE: &str = "reportByProject=owner,manager; reportByPerson=owner,manager";

/// The day the reports count overdue tasks against.
const TODAY: &str = "2026-10-10";

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

/// A database holding the teamspace program and the deployment's custody,
/// the state `ordo apply` and the first boot leave.
async fn deployment_with_teamspace(db: &str, program: &str) {
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
    let plain = Fixture::from_source(program, "").expect("teamspace.ordo compiles");
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

    let mut prog = Fixture::from_source(program, "").expect("teamspace.ordo compiles");
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
        actor: "operator:teamspace-test".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:teamspace-test",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
}

/// A running deployment. Killed, and its databases dropped, when dropped.
struct Deployment {
    port: u16,
    child: Child,
    db: String,
    log: std::path::PathBuf,
    /// False while the deployment is being restarted on the same database.
    owns_db: bool,
}

impl Deployment {
    /// The README's settings, with `provisioning` as
    /// `ORDO_AUTH_PROVISIONING`.
    async fn boot(db: &str, provisioning: &str) -> Deployment {
        Self::boot_program(db, provisioning, SOURCE).await
    }

    /// `boot`, with another version of the program.
    async fn boot_program(db: &str, provisioning: &str, program: &str) -> Deployment {
        deployment_with_teamspace(db, program).await;
        Self::start(db, provisioning)
    }

    /// The binary on a database that already holds the program.
    fn start(db: &str, provisioning: &str) -> Deployment {
        Self::start_sized(db, provisioning, "6")
    }

    /// `start`, with `ORDO_PG_POOL_SIZE` set to `pool`.
    fn start_sized(db: &str, provisioning: &str, pool: &str) -> Deployment {
        let port = free_port();
        let base = PoolConfig::from_env();
        let log = std::env::temp_dir().join(format!("ordo-teamspace-{db}-{port}.log"));
        let _ = std::fs::remove_file(&log);
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .expect("log");
        let err = out.try_clone().expect("log");
        let mut c = Command::new(env!("CARGO_BIN_EXE_ordo-host"));
        c.env("ORDO_PG_POOL_SIZE", pool)
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
            .env("ORDO_APEX_DOMAIN", "localhost")
            .env("ORDO_AUTH_ORIGINS", PAGE)
            .env("ORDO_CORS_ORIGINS", PAGE)
            .env("ORDO_AUTH_PROVISIONING", provisioning)
            .env("ORDO_PROVISION_SELF_SERVE", "on-signup")
            .env("ORDO_ACCESS_TEMPLATE", TEMPLATE)
            .env_remove("ORDO_PG_ROUTING")
            .env_remove("ORDO_TENANT_CREDENTIALS")
            .env_remove("ORDO_TENANT_CREDENTIALS_DIR")
            .env_remove("ORDO_CHECKPOINT_KEY")
            .env_remove("ORDO_SECURITY_CHECKPOINT_KEY")
            .env_remove("ORDO_EXIT_BUNDLE_DIR")
            .env_remove("ORDO_STEP_UP_EXEMPT_OPS")
            .env_remove("ORDO_AUTH_GUEST")
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        let mut child = c.spawn().expect("spawn");
        wait_for_live(port, &mut child, &log);
        Deployment {
            port,
            child,
            db: db.to_string(),
            log,
            owns_db: true,
        }
    }

    /// Stop the binary and start it again on the same database, under
    /// another `ORDO_AUTH_PROVISIONING`.
    fn restart(mut self, provisioning: &str) -> Deployment {
        self.owns_db = false;
        let db = self.db.clone();
        drop(self);
        Self::start(&db, provisioning)
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// One HTTP/1.1 request over a raw socket: `(status, body)`.
    fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, Value) {
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
        let (_, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let json = serde_json::from_str(body.trim()).unwrap_or(Value::Null);
        (status, json)
    }

    /// A POST to `/auth/...`, as `auth/passkey.ts` sends it.
    fn auth(&self, path: &str, body: Value) -> (u16, Value) {
        let headers = [
            ("Content-Type", "application/json"),
            ("Origin", PAGE),
            ("Accept", "application/json"),
        ];
        self.send("POST", path, &headers, &body.to_string())
    }

    /// A call to `/op/<name>`, as `core/client.ts` sends it: reads and
    /// deletes carry their parameters in the query, writes in a JSON body.
    fn op(&self, s: &Session, method: &str, name: &str, params: Value) -> (u16, Value) {
        static KEY: AtomicU64 = AtomicU64::new(1);
        let bearer = format!("Bearer {}", s.token);
        let key = format!("teamspace-test-{}", KEY.fetch_add(1, Ordering::SeqCst));
        let mut headers = vec![
            ("X-Ordo-Space", s.space.as_str()),
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
        self.send(method, &path, &headers, &body)
    }

    fn rows(&self, s: &Session, name: &str, params: Value) -> Vec<Value> {
        let (status, body) = self.op(s, "GET", name, params);
        assert_eq!(status, 200, "{name} as {}: {body}", s.who);
        body["rows"].as_array().cloned().unwrap_or_default()
    }

    /// A write that must land. Answers the first row of the result.
    fn write(&self, s: &Session, name: &str, params: Value) -> Value {
        let (status, body) = self.op(s, "POST", name, params);
        assert_eq!(status, 200, "{name} as {}: {body}", s.who);
        body["rows"][0].clone()
    }
}

impl Drop for Deployment {
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
            let _ = std::fs::write(
                std::env::temp_dir().join("ordo-teamspace-last.log"),
                self.log(),
            );
            eprintln!("--- the server log is in ordo-teamspace-last.log ---");
        }
        let _ = std::fs::remove_file(&self.log);
        if !self.owns_db {
            return;
        }
        // Every founder's tenant has a database of its own.
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

/// The browser's authenticator for one person.
fn device(seed: u8, name: &str) -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(seed),
        credential_id: name.as_bytes().to_vec(),
        rp_id: "localhost".to_string(),
        origin: PAGE.to_string(),
        ..TestAuthenticator::es256()
    }
}

/// What the page holds once signed in: the token and the team it works in.
struct Session {
    who: String,
    /// Their seat in the team, `members.id`, read from `me`.
    member: String,
    token: String,
    /// The team's tenant: the founder's own, or the one an invitation named.
    space: String,
    raw: Value,
}

/// The ceremony alone, with `extra` merged into `/auth/complete`'s body (the
/// page adds `space` and `invitation` when it holds an invitation).
fn register(
    d: &Deployment,
    device: &TestAuthenticator,
    display: &str,
    extra: Value,
) -> (u16, Value) {
    let (status, options) = d.auth(
        "/auth/register",
        json!({"method": "passkey", "display": display}),
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
    let mut body = json!({
        "method": "passkey", "kind": "passkey_registration", "challenge": c.0,
        "client_data_json": b64url_encode(&client_data_json),
        "attestation_object": b64url_encode(&attestation_object),
        "display": display,
    });
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        body[k] = v;
    }
    d.auth("/auth/complete", body)
}

/// The sign-in's answer once the founder's own tenant is made: the sign-in
/// answers first (`provisioning: "in progress"`, owner 2026-10-21), and
/// `GET /auth/provisioning` says when the tenant is `ready`.
fn tenant_ready(d: &Deployment, mut done: Value) -> Value {
    if done["provisioning"] != "in progress" {
        return done;
    }
    let bearer = format!("Bearer {}", done["token"].as_str().expect("a token"));
    let headers = [("Origin", PAGE), ("Authorization", bearer.as_str())];
    let mut last = Value::Null;
    for _ in 0..600 {
        let (status, said) = d.send("GET", "/auth/provisioning", &headers, "");
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

/// A founder signs up and lands in a tenant of their own.
fn found(d: &Deployment, device: &TestAuthenticator, display: &str) -> Session {
    let (status, done) = register(d, device, display, json!({}));
    assert_eq!(status, 200, "{done}");
    let done = tenant_ready(d, done);
    let token = done["token"].as_str().expect("a token").to_string();
    let mut s = Session {
        who: display.to_string(),
        member: String::new(),
        token,
        space: done["space"].as_str().expect("a tenant").to_string(),
        raw: done,
    };
    settle(d, &s);
    s.member = d.rows(&s, "me", json!({}))[0]["id"]
        .as_str()
        .expect("a seat")
        .to_string();
    s
}

/// An admin invites a colleague at `role`. Answers the ticket.
fn invite(d: &Deployment, admin: &Session, role: &str) -> String {
    let row = d.write(
        admin,
        "std.space.invite",
        json!({"role": role, "expires": "7d", "label": "Join the team"}),
    );
    assert_eq!(row["role"], role, "{row}");
    row["ticket"].as_str().expect("a ticket").to_string()
}

/// A newcomer signs up with an invitation and works in the team it names.
fn join(
    d: &Deployment,
    device: &TestAuthenticator,
    display: &str,
    team: &str,
    ticket: &str,
) -> Session {
    let (status, done) = register(
        d,
        device,
        display,
        json!({"space": team, "invitation": ticket}),
    );
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["admission"]["space"], team, "{done}");
    assert_eq!(done["admission"]["outcome"], "admitted", "{done}");
    let token = done["token"].as_str().expect("a token").to_string();
    let mut s = Session {
        who: display.to_string(),
        member: String::new(),
        token,
        space: team.to_string(),
        raw: done,
    };
    settle(d, &s);
    s.member = d.rows(&s, "me", json!({}))[0]["id"]
        .as_str()
        .expect("a seat")
        .to_string();
    s
}

/// Wait until the door serves the person in their team. The door learns of a
/// new tenant or a new seat some moments after the request that made it.
fn settle(d: &Deployment, s: &Session) {
    eventually(&format!("the door serves {} in their team", s.who), || {
        let (status, _) = d.op(s, "GET", "people", json!({}));
        (status == 200).then_some(())
    });
}

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

fn code_of(body: &Value) -> String {
    body["error"]["code"].as_str().unwrap_or("").to_string()
}

/// The page's clock: one more second each call.
fn stamp() -> String {
    static SECONDS: AtomicU64 = AtomicU64::new(0);
    let n = SECONDS.fetch_add(1, Ordering::SeqCst);
    format!("2026-10-01T09:{:02}:{:02}Z", (n / 60) % 60, n % 60)
}

/// `who`'s row in `people`, as `asker` reads it, found by seat. Not by name: an
/// invited person's `displayName` is their principal id today
/// (`examples/teamspace/README.md`, "What Ordo still owes this app").
fn person(d: &Deployment, asker: &Session, who: &Session) -> Option<Value> {
    d.rows(asker, "people", json!({}))
        .into_iter()
        .find(|r| r["id"] == json!(who.member))
}

/// The members id of a person in the team.
fn member_id(d: &Deployment, asker: &Session, who: &Session) -> String {
    person(d, asker, who)
        .and_then(|r| r["id"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("{} is not in the team", who.who))
}

fn role_in(d: &Deployment, asker: &Session, who: &Session) -> Value {
    person(d, asker, who)
        .map(|r| r["role"].clone())
        .unwrap_or(Value::Null)
}

fn new_project(d: &Deployment, admin: &Session, name: &str) -> String {
    let row = d.write(
        admin,
        "std.projects.create",
        json!({"name": name, "summary": format!("The {name} project"), "added": stamp()}),
    );
    row["created"].as_str().expect("the project id").to_string()
}

fn add_to_team(d: &Deployment, admin: &Session, project: &str, member: &str) {
    d.write(
        admin,
        "std.team.create",
        json!({"project": project, "member": member}),
    );
}

fn new_task(
    d: &Deployment,
    s: &Session,
    project: &str,
    title: &str,
    assignee: Option<&str>,
    due: Option<&str>,
) -> (String, i64) {
    let mut params = json!({"project": project, "title": title, "added": stamp()});
    if let Some(a) = assignee {
        params["assignee"] = json!(a);
    }
    if let Some(due) = due {
        params["due"] = json!(due);
    }
    // A create answers the new id. The revision is read back from the board.
    let id = d.write(s, "createTask", params)["created"]
        .as_str()
        .expect("the task id")
        .to_string();
    let rev = d
        .rows(s, "board", json!({"project": project}))
        .iter()
        .find(|r| r["id"] == json!(id))
        .and_then(|r| r["rev"].as_i64())
        .expect("the task on its board");
    (id, rev)
}

fn move_task(d: &Deployment, s: &Session, task: &(String, i64), status: &str) {
    d.write(
        s,
        "moveTask",
        json!({"id": task.0, "rev": task.1, "status": status}),
    );
}

// ═══════════════════════ a team workspace ════════════════════════════════

/// A founder signs up and owns a new tenant. They invite a colleague, who
/// signs up with the invitation and joins that tenant as a member. The
/// invitation works once, and the owner can change the colleague's role.
#[tokio::test(flavor = "multi_thread")]
async fn a_founder_gets_a_team_and_an_invited_colleague_joins_it_as_a_member() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot("ordo_teamspace_join", "auto-onboard").await;

    let maya = found(&d, &device(31, "maya"), "Maya");
    assert_ne!(maya.space, HOME);
    assert_eq!(role_in(&d, &maya, &maya), "owner");
    assert_eq!(
        person(&d, &maya, &maya).expect("Maya")["displayName"],
        "Maya"
    );

    let ticket = invite(&d, &maya, "editor");
    // Outstanding invitations are listed by id, never with the secret.
    let open = d.rows(&maya, "std.space.invitations", json!({}));
    assert_eq!(open.len(), 1, "{open:?}");
    assert!(!open[0].to_string().contains(&ticket));

    let ravi = join(&d, &device(32, "ravi"), "Ravi", &maya.space, &ticket);
    assert_eq!(role_in(&d, &maya, &ravi), "editor");
    // Ravi reads the same two people Maya does.
    assert_eq!(d.rows(&ravi, "people", json!({})).len(), 2);
    assert!(person(&d, &ravi, &maya).is_some());
    // On-signup gave Ravi a tenant of his own too. The team is Maya's.
    assert_ne!(ravi.raw["space"], json!(maya.space), "{}", ravi.raw);

    // The invitation is spent: a second newcomer holding it is not admitted.
    let (status, again) = register(
        &d,
        &device(33, "sam"),
        "Sam",
        json!({"space": maya.space, "invitation": ticket}),
    );
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["admission"]["outcome"], "refused", "{again}");
    assert_eq!(d.rows(&maya, "people", json!({})).len(), 2);

    // The owner changes a role.
    let ravi_id = member_id(&d, &maya, &ravi);
    d.write(
        &maya,
        "std.space.setRole",
        json!({"member": ravi_id, "role": "manager"}),
    );
    assert_eq!(role_in(&d, &maya, &ravi), "manager");
}

/// A member works on tasks and nothing more: the door refuses role changes,
/// invitations, project management and the admin reports, and the guard
/// hides projects they are not on.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_cannot_change_roles_manage_projects_or_read_the_admin_report() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot("ordo_teamspace_roles", "auto-onboard").await;
    let maya = found(&d, &device(41, "maya"), "Maya");
    let ticket = invite(&d, &maya, "editor");
    let ravi = join(&d, &device(42, "ravi"), "Ravi", &maya.space, &ticket);
    let maya_id = member_id(&d, &maya, &maya);
    let ravi_id = member_id(&d, &maya, &ravi);

    let launch = new_project(&d, &maya, "Launch");
    let hiring = new_project(&d, &maya, "Hiring");
    add_to_team(&d, &maya, &launch, &maya_id);
    add_to_team(&d, &maya, &launch, &ravi_id);
    add_to_team(&d, &maya, &hiring, &maya_id);
    let shortlist = new_task(&d, &maya, &hiring, "Shortlist", None, None);

    // The door: each of these is refused before it runs.
    for (method, op, params) in [
        (
            "POST",
            "std.space.setRole",
            json!({"member": ravi_id, "role": "owner"}),
        ),
        (
            "POST",
            "std.space.setRole",
            json!({"member": maya_id, "role": "editor"}),
        ),
        ("POST", "std.space.invite", json!({"role": "editor"})),
        (
            "POST",
            "std.projects.create",
            json!({"name": "Mine", "added": stamp()}),
        ),
        (
            "POST",
            "std.team.create",
            json!({"project": hiring, "member": ravi_id}),
        ),
        ("DELETE", "std.projects.delete", json!({"id": launch})),
        ("GET", "std.projects.list", json!({})),
    ] {
        let (status, body) = d.op(&ravi, method, op, params);
        assert_eq!(
            (status, code_of(&body).as_str()),
            (403, "access_denied"),
            "{op}: {body}"
        );
    }
    assert_eq!(role_in(&d, &maya, &ravi), "editor");
    assert_eq!(role_in(&d, &maya, &maya), "owner");

    // The guard: Ravi is on Launch and not on Hiring.
    let mine: Vec<Value> = d
        .rows(&ravi, "myProjects", json!({}))
        .iter()
        .map(|r| r["name"].clone())
        .collect();
    assert_eq!(mine, [json!("Launch")]);
    let task = new_task(&d, &ravi, &launch, "Draft the post", Some(&ravi_id), None);
    move_task(&d, &ravi, &task, "doing");
    assert_eq!(d.rows(&ravi, "board", json!({"project": launch})).len(), 1);
    assert!(d
        .rows(&ravi, "board", json!({"project": hiring}))
        .is_empty());
    // A task Ravi cannot read cannot be changed either: it is not found.
    let (status, body) = d.op(
        &ravi,
        "POST",
        "moveTask",
        json!({"id": shortlist.0, "rev": shortlist.1, "status": "done"}),
    );
    assert_eq!(
        (status, code_of(&body).as_str()),
        (404, "not_found"),
        "{body}"
    );

    // A stale revision is refused and overwrites nothing.
    let (status, body) = d.op(
        &ravi,
        "POST",
        "moveTask",
        json!({"id": task.0, "rev": task.1, "status": "done"}),
    );
    assert_eq!(code_of(&body), "cas_miss", "{status} {body}");

    // The reports: refused at the door for a member, by the template, and
    // every project for the owner.
    for op in ["reportByProject", "reportByPerson"] {
        let (status, body) = d.op(&ravi, "GET", op, json!({"today": TODAY}));
        assert_eq!(
            (status, code_of(&body).as_str()),
            (403, "access_denied"),
            "{op} is refused by the door, not answered with no rows: {body}"
        );
    }
    assert_eq!(
        d.rows(&maya, "reportByProject", json!({"today": TODAY}))
            .len(),
        2
    );
}

/// The admin report's numbers for a team of four and three projects, one of
/// which the owner is not on. A manager reads the same numbers.
#[tokio::test(flavor = "multi_thread")]
async fn the_admin_report_totals_every_project_and_person() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot("ordo_teamspace_report", "auto-onboard").await;
    let maya = found(&d, &device(51, "maya"), "Maya");
    let ticket = invite(&d, &maya, "manager");
    let lena = join(&d, &device(52, "lena"), "Lena", &maya.space, &ticket);
    let ticket = invite(&d, &maya, "editor");
    let ravi = join(&d, &device(53, "ravi"), "Ravi", &maya.space, &ticket);
    // A manager may invite at or below their own role, and not above it.
    let ticket = invite(&d, &lena, "editor");
    let tom = join(&d, &device(54, "tom"), "Tom", &maya.space, &ticket);
    let (status, body) = d.op(&lena, "POST", "std.space.invite", json!({"role": "owner"}));
    assert_eq!(
        (status, code_of(&body).as_str()),
        (400, "tenant_refused"),
        "a manager cannot invite an owner: {body}"
    );

    let maya_id = member_id(&d, &maya, &maya);
    let lena_id = member_id(&d, &maya, &lena);
    let ravi_id = member_id(&d, &maya, &ravi);
    let tom_id = member_id(&d, &maya, &tom);

    // Three projects. Maya is not on Ops, which Lena made.
    let site = new_project(&d, &maya, "Website");
    let app = new_project(&d, &maya, "Mobile app");
    let ops = new_project(&d, &lena, "Ops");
    for m in [&maya_id, &ravi_id] {
        add_to_team(&d, &maya, &site, m);
    }
    for m in [&maya_id, &ravi_id, &tom_id] {
        add_to_team(&d, &maya, &app, m);
    }
    for m in [&lena_id, &tom_id] {
        add_to_team(&d, &lena, &ops, m);
    }

    // Website, 4 tasks: todo and overdue, doing, review, done (past its due
    // date, and done, so not overdue).
    new_task(
        &d,
        &maya,
        &site,
        "Pick a font",
        Some(&ravi_id),
        Some("2026-10-01"),
    );
    let t = new_task(
        &d,
        &ravi,
        &site,
        "Build the header",
        Some(&ravi_id),
        Some("2026-10-20"),
    );
    move_task(&d, &ravi, &t, "doing");
    let t = new_task(&d, &ravi, &site, "Write copy", Some(&maya_id), None);
    move_task(&d, &maya, &t, "review");
    let t = new_task(
        &d,
        &maya,
        &site,
        "Buy the domain",
        Some(&maya_id),
        Some("2026-09-01"),
    );
    move_task(&d, &maya, &t, "done");
    // Mobile app, 3 tasks: two todo (one unassigned, one overdue), one done.
    new_task(&d, &tom, &app, "Sketch screens", None, None);
    new_task(
        &d,
        &tom,
        &app,
        "Set up the build",
        Some(&tom_id),
        Some("2026-10-09"),
    );
    let t = new_task(&d, &ravi, &app, "Pick a name", Some(&ravi_id), None);
    move_task(&d, &ravi, &t, "done");
    // Ops, 2 tasks, both doing, one overdue. Maya cannot see them.
    let t = new_task(
        &d,
        &lena,
        &ops,
        "Rotate keys",
        Some(&lena_id),
        Some("2026-10-05"),
    );
    move_task(&d, &lena, &t, "doing");
    let t = new_task(&d, &tom, &ops, "Back up the database", Some(&tom_id), None);
    move_task(&d, &tom, &t, "doing");
    assert!(d.rows(&maya, "board", json!({"project": ops})).is_empty());

    let by_project = |s: &Session| -> Vec<Value> {
        d.rows(s, "reportByProject", json!({"today": TODAY}))
            .iter()
            .map(|r| {
                json!([
                    r["name"],
                    r["total"],
                    r["todo"],
                    r["doing"],
                    r["review"],
                    r["done"],
                    r["overdue"]
                ])
            })
            .collect()
    };
    let want = vec![
        json!(["Mobile app", 3, 2, 0, 0, 1, 1]),
        json!(["Ops", 2, 0, 2, 0, 0, 1]),
        json!(["Website", 4, 1, 1, 1, 1, 1]),
    ];
    assert_eq!(by_project(&maya), want);
    assert_eq!(by_project(&lena), want, "a manager reads the same report");

    // One row per person, found by members id: [role, total, open, done, overdue].
    let by_person = d.rows(&maya, "reportByPerson", json!({"today": TODAY}));
    assert_eq!(by_person.len(), 4, "{by_person:?}");
    let row = |id: &str| -> Value {
        let r = by_person
            .iter()
            .find(|r| r["id"] == json!(id))
            .unwrap_or_else(|| panic!("no row for {id}: {by_person:?}"));
        json!([r["role"], r["total"], r["open"], r["done"], r["overdue"]])
    };
    assert_eq!(row(&lena_id), json!(["manager", 1, 1, 0, 1]));
    assert_eq!(row(&maya_id), json!(["owner", 2, 1, 1, 0]));
    assert_eq!(row(&ravi_id), json!(["editor", 3, 2, 1, 1]));
    assert_eq!(row(&tom_id), json!(["editor", 2, 2, 0, 1]));
}

/// The tenant's owner opens the report to a group of their own: a member in
/// the group reads every project, and a member outside it is still refused
/// at the door. Another team's members are untouched.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_owner_opens_the_report_to_a_group() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot("ordo_teamspace_group", "auto-onboard").await;
    let maya = found(&d, &device(61, "maya"), "Maya");
    let ticket = invite(&d, &maya, "editor");
    let ravi = join(&d, &device(62, "ravi"), "Ravi", &maya.space, &ticket);
    let ticket = invite(&d, &maya, "editor");
    let tom = join(&d, &device(63, "tom"), "Tom", &maya.space, &ticket);
    let ravi_id = member_id(&d, &maya, &ravi);
    new_project(&d, &maya, "Website");
    new_project(&d, &maya, "Ops");

    let leads = d.write(&maya, "std.space.createGroup", json!({"name": "Leads"}));
    let leads = leads["group"].as_str().expect("the group's id").to_string();
    d.write(
        &maya,
        "std.space.addToGroup",
        json!({"group": leads, "member": ravi_id}),
    );
    d.write(
        &maya,
        "std.space.setOpAccess",
        json!({"op": "reportByProject", "roles": "owner,manager", "groups": leads}),
    );

    assert_eq!(
        d.rows(&ravi, "reportByProject", json!({"today": TODAY}))
            .len(),
        2,
        "a member in Leads reads every project"
    );
    let (status, body) = d.op(&tom, "GET", "reportByProject", json!({"today": TODAY}));
    assert_eq!(
        (status, code_of(&body).as_str()),
        (403, "access_denied"),
        "a member outside Leads is refused at the door: {body}"
    );
    // The other report was not opened.
    let (status, _) = d.op(&ravi, "GET", "reportByPerson", json!({"today": TODAY}));
    assert_eq!(status, 403);

    // The profile says what the owner changed.
    let line = d.rows(&maya, "std.space.profile", json!({"op": "reportByProject"}));
    assert_eq!(line[0]["groups"], json!([leads]));
    assert_eq!(line[0]["changed"], json!(true));

    // A founder of another team keeps the template.
    let ana = found(&d, &device(64, "ana"), "Ana");
    let ticket = invite(&d, &ana, "editor");
    let bo = join(&d, &device(65, "bo"), "Bo", &ana.space, &ticket);
    let (status, _) = d.op(&bo, "GET", "reportByProject", json!({"today": TODAY}));
    assert_eq!(status, 403);
}

/// Under invite-only nobody new can found a team, and a newcomer with an
/// invitation joins the team that issued it.
#[tokio::test(flavor = "multi_thread")]
async fn under_invite_only_an_invited_newcomer_joins_and_a_stranger_is_refused() {
    let _serial = SERIAL.lock().await;
    // The founder signs up while the deployment is open. Then the deployment
    // owner closes it and restarts the server.
    let d = Deployment::boot("ordo_teamspace_invite_only", "auto-onboard").await;
    let maya = found(&d, &device(61, "maya"), "Maya");
    let d = d.restart("invite-only");
    eventually("the restarted door serves Maya", || {
        (d.op(&maya, "GET", "people", json!({})).0 == 200).then_some(())
    });

    // A stranger is refused, and is given no token.
    let (status, refused) = register(&d, &device(62, "eve"), "Eve", json!({}));
    assert_ne!(status, 200, "{refused}");
    assert!(refused.get("token").is_none(), "{refused}");

    let ticket = invite(&d, &maya, "editor");
    let ravi = join(&d, &device(63, "ravi"), "Ravi", &maya.space, &ticket);
    assert_eq!(role_in(&d, &maya, &ravi), "editor");
    let launch = new_project(&d, &maya, "Launch");
    add_to_team(&d, &maya, &launch, &member_id(&d, &maya, &ravi));
    new_task(&d, &ravi, &launch, "First task", None, None);
    assert_eq!(d.rows(&ravi, "board", json!({"project": launch})).len(), 1);
}

// ═══════════════════════ fixed defects ═══════════════════════════════════
//
// Each guards a defect the example found (`docs/roadmap-checklist.md`).

/// A member adds a task to a project they are on and is refused one on a
/// project they are not on (25ay: the create gate used to misread a long
/// parent id as text and refuse every such create).
#[tokio::test(flavor = "multi_thread")]
async fn a_member_adds_tasks_only_to_projects_they_are_on() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot_program("ordo_teamspace_guarded", "auto-onboard", SOURCE).await;
    let maya = found(&d, &device(81, "maya"), "Maya");
    let ticket = invite(&d, &maya, "editor");
    let ravi = join(&d, &device(82, "ravi"), "Ravi", &maya.space, &ticket);
    let launch = new_project(&d, &maya, "Launch");
    let hiring = new_project(&d, &maya, "Hiring");
    add_to_team(&d, &maya, &launch, &ravi.member);
    new_task(&d, &ravi, &launch, "Draft the post", None, None);
    let (status, body) = d.op(
        &ravi,
        "POST",
        "createTask",
        json!({"project": hiring, "title": "Sneak in", "added": stamp()}),
    );
    assert_eq!(
        (status, code_of(&body).as_str()),
        (403, "access_guard"),
        "{body}"
    );
}

/// 25ba. A colleague who joins by invitation is shown by the name they gave
/// when they signed up, not by their account id.
#[tokio::test(flavor = "multi_thread")]
async fn an_invited_colleague_is_shown_by_the_name_they_gave() {
    let _serial = SERIAL.lock().await;
    let d = Deployment::boot("ordo_teamspace_names", "auto-onboard").await;
    let maya = found(&d, &device(71, "maya"), "Maya");
    let ticket = invite(&d, &maya, "editor");
    let ravi = join(&d, &device(72, "ravi"), "Ravi", &maya.space, &ticket);
    assert_eq!(
        person(&d, &maya, &ravi).expect("Ravi")["displayName"],
        "Ravi"
    );
}

/// Reported fix tenant-access-lags-after-join: a tenant just made, and a
/// person just admitted to one, are served on their first call. Several
/// founders and colleagues at once, on the benchmark's pool size of 3, so
/// the door's rebuilds overlap. No retry: the first answer must be 200, and
/// a 403 here is the defect. (A door that could not rebuild answers 503
/// `server_busy`, never 403; `door_behind.rs` holds that half.)
#[tokio::test(flavor = "multi_thread")]
async fn a_new_tenant_and_a_new_colleague_are_served_on_their_first_call() {
    let _serial = SERIAL.lock().await;
    deployment_with_teamspace("ordo_teamspace_first_call", SOURCE).await;
    let d = Deployment::start_sized("ordo_teamspace_first_call", "auto-onboard", "3");
    std::thread::scope(|sc| {
        for round in 0..6u8 {
            let d = &d;
            sc.spawn(move || {
                let (status, done) = register(
                    d,
                    &device(100 + round * 2, &format!("founder{round}")),
                    &format!("Founder {round}"),
                    json!({}),
                );
                assert_eq!(status, 200, "{done}");
                let done = tenant_ready(d, done);
                let founder = Session {
                    who: format!("Founder {round}"),
                    member: String::new(),
                    token: done["token"].as_str().expect("a token").to_string(),
                    space: done["space"].as_str().expect("a tenant").to_string(),
                    raw: done,
                };
                let (status, said) = d.op(&founder, "GET", "people", json!({}));
                assert_eq!(status, 200, "the founder's first call: {said}");

                let ticket = invite(d, &founder, "editor");
                let (status, done) = register(
                    d,
                    &device(101 + round * 2, &format!("colleague{round}")),
                    &format!("Colleague {round}"),
                    json!({"space": founder.space, "invitation": ticket}),
                );
                assert_eq!(status, 200, "{done}");
                assert_eq!(done["admission"]["outcome"], "admitted", "{done}");
                let colleague = Session {
                    who: format!("Colleague {round}"),
                    member: String::new(),
                    token: done["token"].as_str().expect("a token").to_string(),
                    space: founder.space.clone(),
                    raw: done,
                };
                let (status, said) = d.op(&colleague, "GET", "people", json!({}));
                assert_eq!(status, 200, "the colleague's first call: {said}");
                assert_eq!(said["rows"].as_array().map(Vec::len), Some(2), "{said}");
            });
        }
    });
}
