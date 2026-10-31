//! The shopfront app (`examples/shopfront`) against the real `ordo-host`
//! binary and real Postgres: one deployment, one tenant, three page origins.
//!
//! The binary boots on a database that holds the shopfront program in the
//! deployment's own tenant, with the settings `examples/shopfront/dev/serve.sh`
//! gives: passkey sign-in on `localhost`, the three pages in
//! `ORDO_AUTH_ORIGINS` and `ORDO_CORS_ORIGINS`, the shop and the console in
//! `ORDO_AUTH_RETURN_URLS`, admission allowed (`auto-onboard`) and no tenant of
//! one's own (`ORDO_PROVISION_SELF_SERVE=off`). The test then acts as the
//! pages and `dev/seed.mjs` do: the owner signs up, is named in
//! `ORDO_DEPLOYMENT_OWNERS`, the door restarts, and the owner opens the tenant
//! to customers at role `editor` and opens the shop.
//!
//! What it proves:
//!
//! - **Orders.** A customer places orders and sees only their own. Staff see
//!   every order and mark one shipped, and the customer then sees it shipped.
//! - **Staff only.** A customer cannot change the catalogue, restock, ship an
//!   order, add themselves to the staff, or read the staff list.
//! - **The hand-off.** A person signed in on the login page's origin is handed
//!   to the shop's origin as the same person, and that session reads their
//!   orders from the shop's origin.
//! - **JSON attributes.** `productsWhere` returns the products whose
//!   `attributes.color` is the colour asked for, and no others.
//!
//! Needs a Postgres cluster (`scripts/conformance-pg.sh start`).

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
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use ordo_identity::CompleteRequest;
use serde_json::{json, Value};

const SOURCE: &str = include_str!("../../../examples/shopfront/shopfront.ordo");

/// The deployment's own tenant. In a single-tenant deployment it is the shop.
const HOME: &str = "shopfront";
const MASTER: &str = "the-shopfront-app-test-master-key";
const TOKEN_KEY: &str = "7777777777777777777777777777777777777777777777777777777777777777";
const PAGE_KEY: &str = "8888888888888888888888888888888888888888888888888888888888888888";

/// The three pages, as they run locally: one origin each.
const LOGIN: &str = "http://localhost:5180";
const SHOP: &str = "http://localhost:5181";
const ADMIN: &str = "http://localhost:5182";

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

/// A database holding the shopfront program and the deployment's custody,
/// the state `ordo apply` and the first boot leave.
async fn deployment_with_shopfront(db: &str) {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"));
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean schema");
    let plain = Fixture::from_source(SOURCE, "").expect("shopfront.ordo compiles");
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

    let mut prog = Fixture::from_source(SOURCE, "").expect("shopfront.ordo compiles");
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
        actor: "operator:shopfront-test".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:shopfront-test",
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
}

impl Deployment {
    /// The settings `examples/shopfront/dev/serve.sh` gives, with `owners` in
    /// `ORDO_DEPLOYMENT_OWNERS` when there are any.
    async fn boot(db: &str) -> Deployment {
        deployment_with_shopfront(db).await;
        let log = std::env::temp_dir().join(format!("ordo-shopfront-{db}.log"));
        let _ = std::fs::remove_file(&log);
        let (child, port) = spawn(db, &log, None);
        Deployment {
            port,
            child,
            db: db.to_string(),
            log,
        }
    }

    /// Stop the binary and start it again on the same database, with `owner`
    /// named as a deployment owner: what the README tells the app developer
    /// to do once the owner has signed up.
    fn restart_with_owner(&mut self, owner: &str) {
        stop(&mut self.child);
        let (child, port) = spawn(&self.db, &self.log, Some(owner));
        self.child = child;
        self.port = port;
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

    /// A POST to `/auth/...` from the page at `origin`.
    fn auth(&self, origin: &str, path: &str, body: Value, token: Option<&str>) -> (u16, Value) {
        let mut headers = vec![
            ("Content-Type", "application/json"),
            ("Origin", origin),
            ("Accept", "application/json"),
        ];
        let bearer;
        if let Some(t) = token {
            bearer = format!("Bearer {t}");
            headers.push(("Authorization", &bearer));
        }
        self.send("POST", path, &headers, &body.to_string())
    }

    /// A call to `/op/<name>` in the shop's tenant from the page at `origin`,
    /// as `core/client.ts` sends it: reads carry their parameters in the
    /// query, writes in a JSON body.
    fn op(&self, origin: &str, token: &str, name: &str, params: Value) -> (u16, Value) {
        static KEY: AtomicU64 = AtomicU64::new(1);
        let write = !is_read(name);
        let bearer = format!("Bearer {token}");
        let key = format!("shopfront-test-{}", KEY.fetch_add(1, Ordering::SeqCst));
        let mut headers = vec![
            ("X-Ordo-Space", HOME),
            ("Authorization", bearer.as_str()),
            ("Origin", origin),
            ("Accept", "application/json"),
        ];
        if write {
            headers.push(("Idempotency-Key", key.as_str()));
            headers.push(("Content-Type", "application/json"));
            return self.send(
                "POST",
                &format!("/op/{name}"),
                &headers,
                &params.to_string(),
            );
        }
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
        self.send("GET", &path, &headers, "")
    }

    /// The rows of a read that must succeed.
    fn rows(&self, origin: &str, token: &str, name: &str, params: Value) -> Vec<Value> {
        let (status, body) = self.op(origin, token, name, params);
        assert_eq!(status, 200, "{name}: {body}");
        body["rows"].as_array().cloned().unwrap_or_default()
    }

    /// A write that must succeed. Returns the first row of the answer.
    fn write(&self, origin: &str, token: &str, name: &str, params: Value) -> Value {
        let (status, body) = self.op(origin, token, name, params);
        assert_eq!(status, 200, "{name}: {body}");
        body["rows"][0].clone()
    }
}

/// The reads this test calls. Everything else is a write.
fn is_read(name: &str) -> bool {
    matches!(
        name,
        "me" | "myShops"
            | "catalogue"
            | "productsWhere"
            | "myOrders"
            | "allOrders"
            | "staffOf"
            | "std.space.admission"
            | "std.products.get"
    )
}

fn spawn(db: &str, log: &std::path::Path, owner: Option<&str>) -> (Child, u16) {
    let port = free_port();
    let base = PoolConfig::from_env();
    // The server writes to a file, so a full pipe never stops it and a
    // failing test can print what it said.
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .expect("log");
    let err = out.try_clone().expect("log");
    let pages = format!("{LOGIN},{SHOP},{ADMIN}");
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
        .env("ORDO_APEX_DOMAIN", "localhost")
        .env("ORDO_AUTH_ORIGINS", &pages)
        .env("ORDO_CORS_ORIGINS", &pages)
        .env("ORDO_AUTH_RETURN_URLS", format!("{SHOP},{ADMIN}"))
        .env("ORDO_AUTH_PROVISIONING", "auto-onboard")
        .env("ORDO_PROVISION_SELF_SERVE", "off")
        .env_remove("ORDO_DEPLOYMENT_OWNERS")
        .env_remove("ORDO_WILDCARD_ORIGINS")
        .env_remove("ORDO_PG_ROUTING")
        .env_remove("ORDO_TENANT_CREDENTIALS")
        .env_remove("ORDO_TENANT_CREDENTIALS_DIR")
        .env_remove("ORDO_CHECKPOINT_KEY")
        .env_remove("ORDO_SECURITY_CHECKPOINT_KEY")
        .env_remove("ORDO_EXIT_BUNDLE_DIR")
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    if let Some(o) = owner {
        c.env("ORDO_DEPLOYMENT_OWNERS", o);
    }
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child, log);
    (child, port)
}

fn stop(child: &mut Child) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status();
    // A server that does not answer its stop signal is killed, so a hung one
    // fails a test instead of hanging the run.
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

impl Drop for Deployment {
    fn drop(&mut self) {
        stop(&mut self.child);
        if std::thread::panicking() {
            let _ = std::fs::write(
                std::env::temp_dir().join("ordo-shopfront-last.log"),
                self.log(),
            );
            eprintln!("--- the server log is in ordo-shopfront-last.log ---");
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
        let _ = std::fs::remove_file(&self.log);
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

/// Run `probe` until it answers `Some`, for at most thirty seconds. The door
/// learns of a new member or a new acl row some moments after the request
/// that made it answers, as `notes_app.rs` explains.
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

// ── People ───────────────────────────────────────────────────────────────

/// One person's authenticator, reporting the login page's origin under the
/// relying party `localhost`.
fn device(seed: u8, name: &str) -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(seed),
        credential_id: name.as_bytes().to_vec(),
        rp_id: "localhost".to_string(),
        origin: LOGIN.to_string(),
        ..TestAuthenticator::es256()
    }
}

/// What the login page holds after a sign-in.
struct Person {
    token: String,
    principal: String,
}

fn principal_of(token: &str) -> String {
    let payload = token.split('.').next().expect("a payload");
    let text = String::from_utf8(b64url_decode(payload).expect("base64url")).expect("text");
    text.split('|').nth(1).expect("a principal").to_string()
}

fn person(status: u16, done: &Value) -> Person {
    assert_eq!(status, 200, "{done}");
    let token = done["token"].as_str().expect("a token").to_string();
    Person {
        principal: principal_of(&token),
        token,
    }
}

/// `registerPasskey` on the login page. `space` is the tenant the person is
/// arriving at, which admits them by the tenant's open way in.
fn register(
    d: &Deployment,
    device: &TestAuthenticator,
    display: &str,
    space: Option<&str>,
) -> Person {
    let (status, options) = d.auth(
        LOGIN,
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
    let mut body = json!({
        "method": "passkey", "kind": "passkey_registration", "challenge": c.0,
        "client_data_json": b64url_encode(&client_data_json),
        "attestation_object": b64url_encode(&attestation_object),
        "display": display,
    });
    if let Some(s) = space {
        body["space"] = json!(s);
    }
    let (status, done) = d.auth(LOGIN, "/auth/complete", body, None);
    if space.is_some() {
        assert_eq!(done["admission"]["outcome"], "admitted", "{done}");
    }
    person(status, &done)
}

/// `signInWithPasskey` on the login page.
fn sign_in(d: &Deployment, device: &TestAuthenticator) -> Person {
    let (status, options) = d.auth(LOGIN, "/auth/begin", json!({"method": "passkey"}), None);
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
        LOGIN,
        "/auth/complete",
        json!({
            "method": "passkey", "kind": "passkey_assertion", "challenge": c.0,
            "credential_id": b64url_encode(&credential_id),
            "client_data_json": b64url_encode(&client_data_json),
            "authenticator_data": b64url_encode(&authenticator_data),
            "signature": b64url_encode(&signature),
            "space": HOME,
        }),
        None,
    );
    person(status, &done)
}

/// The caller's member id in the shop's tenant, once the door serves them.
fn member_id(d: &Deployment, origin: &str, p: &Person) -> String {
    eventually("the door serves the new member", || {
        let (status, body) = d.op(origin, &p.token, "me", json!({}));
        (status == 200)
            .then(|| body["rows"][0]["id"].as_str().map(str::to_string))
            .flatten()
    })
}

// ── The shop ─────────────────────────────────────────────────────────────

/// A small catalogue: `(sku, name, category, price, stock, attributes)`.
fn catalogue() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    i64,
    Value,
)> {
    vec![
        (
            "SF-APP-001",
            "Linen shirt",
            "Apparel",
            "48.00",
            10,
            json!({"color": "red", "size": "M"}),
        ),
        (
            "SF-APP-002",
            "Wool socks",
            "Apparel",
            "12.50",
            30,
            json!({"color": "grey", "size": "L"}),
        ),
        (
            "SF-KIT-001",
            "Enamel mug",
            "Kitchen",
            "9.99",
            40,
            json!({"color": "red", "capacity_ml": 350}),
        ),
        (
            "SF-KIT-002",
            "Oak board",
            "Kitchen",
            "34.00",
            5,
            json!({"color": "brown", "material": "oak"}),
        ),
        (
            "SF-HOM-001",
            "Cushion",
            "Home",
            "22.00",
            8,
            json!({"color": "blue"}),
        ),
        (
            "SF-HOM-002",
            "Lamp",
            "Home",
            "79.00",
            3,
            json!({"color": "red", "material": "steel"}),
        ),
        (
            "SF-GAR-001",
            "Trowel",
            "Garden",
            "15.25",
            20,
            json!({"material": "steel"}),
        ),
        // A colour that is not text matches no colour asked for as text.
        (
            "SF-GAR-002",
            "Seed tin",
            "Garden",
            "6.00",
            50,
            json!({"color": 7}),
        ),
    ]
}

/// A deployment with the shop open: Olive owns the deployment and opened the
/// tenant to customers at role `editor`, opened the shop and filled its
/// catalogue, and Sam is on its staff.
struct Shop {
    d: Deployment,
    shop: String,
    olive: Person,
    sam: Person,
    /// SKU to product id.
    products: Vec<(String, String)>,
}

impl Shop {
    fn product(&self, sku: &str) -> &str {
        &self
            .products
            .iter()
            .find(|(s, _)| s == sku)
            .expect("a product")
            .1
    }
}

async fn open_shop(db: &str) -> Shop {
    let mut d = Deployment::boot(db).await;

    // The owner signs up on the login page. No tenant admits her yet: she is
    // named on the server, and the door restarts.
    let olive_device = device(31, "olive");
    let first = register(&d, &olive_device, "Olive", None);
    d.restart_with_owner(&first.principal);
    let olive = sign_in(&d, &olive_device);
    assert_eq!(olive.principal, first.principal);

    // Customers come in by the open way in, at the role that lets them write.
    let admission = eventually("the owner may set the tenant's admission", || {
        let (status, body) = d.op(
            ADMIN,
            &olive.token,
            "std.space.setAdmission",
            json!({"open": true, "role": "editor"}),
        );
        (status == 200).then_some(body)
    });
    assert_eq!(admission["rows"][0]["open"], true, "{admission}");
    assert_eq!(admission["rows"][0]["role"], "editor", "{admission}");

    let shop = d.write(
        ADMIN,
        &olive.token,
        "openShop",
        json!({"name": "Shopfront"}),
    )["created"]
        .as_str()
        .expect("the shop's id")
        .to_string();
    let mut products = Vec::new();
    for (sku, name, category, price, stock, attributes) in catalogue() {
        let row = d.write(
            ADMIN,
            &olive.token,
            "addProduct",
            json!({
                "shop": shop, "sku": sku, "name": name, "category": category,
                "price": price, "stock": stock, "attributes": attributes,
            }),
        );
        let id = row["created"]
            .as_str()
            .or_else(|| row["id"].as_str())
            .unwrap_or_else(|| panic!("an id for {sku}: {row}"))
            .to_string();
        products.push((sku.to_string(), id));
    }

    // Sam signs up as anyone does, and Olive puts him on the staff.
    let sam = register(&d, &device(32, "sam"), "Sam", Some(HOME));
    let sam_member = member_id(&d, ADMIN, &sam);
    d.write(
        ADMIN,
        &olive.token,
        "grantShopsAccess",
        json!({"shop": shop, "member": sam_member, "level": "staff"}),
    );
    eventually("Sam sees the shop he works in", || {
        let (status, body) = d.op(ADMIN, &sam.token, "myShops", json!({}));
        (status == 200 && body["rows"].as_array().is_some_and(|r| r.len() == 1)).then_some(())
    });

    Shop {
        d,
        shop,
        olive,
        sam,
        products,
    }
}

/// A customer signs up on the login page, and is admitted.
fn customer(s: &Shop, seed: u8, name: &str) -> Person {
    let p = register(&s.d, &device(seed, name), name, Some(HOME));
    member_id(&s.d, SHOP, &p);
    p
}

fn order(s: &Shop, who: &Person, sku: &str, quantity: i64, placed: &str) -> String {
    s.d.write(
        SHOP,
        &who.token,
        "placeOrder",
        json!({"product": s.product(sku), "quantity": quantity, "placed": placed, "note": ""}),
    )["created"]
        .as_str()
        .expect("the order's id")
        .to_string()
}

fn ids(rows: &[Value]) -> Vec<String> {
    let mut v: Vec<String> = rows
        .iter()
        .map(|r| r["id"].as_str().unwrap_or("").to_string())
        .collect();
    v.sort();
    v
}

fn code_of(body: &Value) -> String {
    body["error"]["code"].as_str().unwrap_or("").to_string()
}

// ── The tests ────────────────────────────────────────────────────────────

/// Ada and Bo each place orders and each sees only their own. Sam sees all
/// of them, marks one of Ada's shipped, and the stock goes down by its
/// quantity. Ada then sees it shipped. A second shipment of the same order
/// is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_customer_sees_only_their_own_orders_and_staff_see_all_and_ship_them() {
    let _serial = SERIAL.lock().await;
    let s = open_shop("ordo_shopfront_orders").await;
    let ada = customer(&s, 41, "Ada");
    let bo = customer(&s, 42, "Bo");

    let a1 = order(&s, &ada, "SF-KIT-001", 2, "2026-09-02T10:00:00Z");
    let a2 = order(&s, &ada, "SF-HOM-001", 1, "2026-09-03T10:00:00Z");
    let b1 = order(&s, &bo, "SF-APP-002", 3, "2026-09-04T10:00:00Z");

    let mine = s.d.rows(SHOP, &ada.token, "myOrders", json!({}));
    assert_eq!(ids(&mine), ids(&[json!({"id": a1}), json!({"id": a2})]));
    let mug = mine
        .iter()
        .find(|r| r["id"] == json!(a1))
        .expect("the mug order");
    assert_eq!(mug["productName"], "Enamel mug", "{mug}");
    assert!(mug["shipped"].is_null(), "not shipped yet: {mug}");
    assert_eq!(
        ids(&s.d.rows(SHOP, &bo.token, "myOrders", json!({}))),
        vec![b1.clone()]
    );
    // `allOrders` is guarded the same way: a customer sees their own.
    assert_eq!(
        ids(&s.d.rows(SHOP, &bo.token, "allOrders", json!({}))),
        vec![b1.clone()]
    );

    // Staff see every order, with who placed it.
    let all = s.d.rows(ADMIN, &s.sam.token, "allOrders", json!({}));
    let mut want = vec![a1.clone(), a2.clone(), b1.clone()];
    want.sort();
    assert_eq!(ids(&all), want);
    let bos = all
        .iter()
        .find(|r| r["id"] == json!(b1))
        .expect("Bo's order");
    // A member admitted by a sign-in is named by the name they gave (25ba).
    assert_eq!(bos["customer"], json!("Bo"), "{bos}");

    // Sam ships Ada's mugs. The stock goes from 40 to 38.
    let mug_id = s.product("SF-KIT-001").to_string();
    s.d.write(
        ADMIN,
        &s.sam.token,
        "markShipped",
        json!({"order": a1, "product": mug_id, "quantity": 2,
               "shipped": "2026-09-05T09:00:00Z", "carrier": "Royal Mail"}),
    );
    let stock =
        s.d.rows(SHOP, &ada.token, "catalogue", json!({}))
            .into_iter()
            .find(|r| r["sku"] == "SF-KIT-001")
            .expect("the mug")["stock"]
            .clone();
    assert_eq!(stock, json!(38));
    let mine = s.d.rows(SHOP, &ada.token, "myOrders", json!({}));
    let mug = mine
        .iter()
        .find(|r| r["id"] == json!(a1))
        .expect("the mug order");
    assert_eq!(mug["carrier"], "Royal Mail", "{mug}");
    assert!(!mug["shipped"].is_null(), "{mug}");

    // Shipping it again is refused, and the stock is taken once.
    let (status, again) = s.d.op(
        ADMIN,
        &s.sam.token,
        "markShipped",
        json!({"order": a1, "product": mug_id, "quantity": 2,
               "shipped": "2026-09-06T09:00:00Z", "carrier": "Royal Mail"}),
    );
    assert_ne!(status, 200, "{again}");

    // The owner opened the shop, so she is on its staff too.
    assert_eq!(
        s.d.rows(ADMIN, &s.olive.token, "allOrders", json!({}))
            .len(),
        3
    );
}

/// Every staff op a customer can name is refused, and the staff-only reads
/// show them nothing they did not already own.
#[tokio::test(flavor = "multi_thread")]
async fn a_customer_cannot_reach_the_staff_ops() {
    let _serial = SERIAL.lock().await;
    let s = open_shop("ordo_shopfront_staff_only").await;
    let ada = customer(&s, 43, "Ada");
    let ada_member = member_id(&s.d, SHOP, &ada);
    let mug = s.product("SF-KIT-001").to_string();
    let a1 = order(&s, &ada, "SF-KIT-001", 1, "2026-09-02T10:00:00Z");

    let refused = |name: &str, params: Value| {
        let (status, body) = s.d.op(ADMIN, &ada.token, name, params);
        assert_ne!(status, 200, "{name} must be refused to a customer: {body}");
        let code = code_of(&body);
        assert!(
            matches!(
                code.as_str(),
                "access_guard" | "not_found" | "access_denied"
            ),
            "{name}: {body}"
        );
        code
    };

    // The catalogue: she can read a product and may not write it.
    assert_eq!(
        refused("restock", json!({"product": mug, "by": 100})),
        "access_guard"
    );
    assert_eq!(
        refused(
            "addProduct",
            json!({"shop": s.shop, "sku": "SF-X-001", "name": "Mine", "category": "Home",
                   "price": "1.00", "stock": 1, "attributes": {}}),
        ),
        "not_found",
        "the shop is not hers to read, so it does not exist for her"
    );
    // Shipping her own order.
    assert_eq!(
        refused(
            "markShipped",
            json!({"order": a1, "product": mug, "quantity": 1,
                   "shipped": "2026-09-03T10:00:00Z", "carrier": "Me"}),
        ),
        "access_guard"
    );
    // Putting herself on the staff.
    refused(
        "grantShopsAccess",
        json!({"shop": s.shop, "member": ada_member, "level": "admin"}),
    );
    // Opening a shop of her own gives her that shop, and nothing in this one.
    // The staff list and the shops she holds a level on are empty to her.
    assert!(s.d.rows(ADMIN, &ada.token, "myShops", json!({})).is_empty());
    assert!(s
        .d
        .rows(ADMIN, &ada.token, "staffOf", json!({"shop": s.shop}))
        .is_empty());
    // Platform ops are the owner's.
    let (status, body) = s.d.op(
        ADMIN,
        &ada.token,
        "std.space.setAdmission",
        json!({"open": false}),
    );
    assert_ne!(status, 200, "{body}");

    // Nothing changed: the stock is what it was, and nobody new is staff.
    let stock =
        s.d.rows(SHOP, &ada.token, "catalogue", json!({}))
            .into_iter()
            .find(|r| r["sku"] == "SF-KIT-001")
            .expect("the mug")["stock"]
            .clone();
    assert_eq!(stock, json!(40));
    let staff =
        s.d.rows(ADMIN, &s.olive.token, "staffOf", json!({"shop": s.shop}));
    let names: Vec<&str> = staff.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(staff.len(), 2, "Olive and Sam: {names:?}");
}

/// The shop page sends Ada to the login page with a binder's hash. The login
/// page, where she is signed in, asks the door for a code. The shop page
/// redeems it from its own origin and gets a session for the same person,
/// which reads her orders.
#[tokio::test(flavor = "multi_thread")]
async fn the_hand_off_carries_a_session_from_the_login_page_to_the_shop() {
    let _serial = SERIAL.lock().await;
    let s = open_shop("ordo_shopfront_handoff").await;
    let ada_device = device(44, "ada");
    register(&s.d, &ada_device, "Ada", Some(HOME));
    // A later visit: she signs in again on the login page.
    let ada = sign_in(&s.d, &ada_device);
    member_id(&s.d, LOGIN, &ada);
    let a1 = order(&s, &ada, "SF-APP-001", 1, "2026-09-02T10:00:00Z");

    // The shop page's binder (`startSubdomainSignIn`).
    let binder = "shop-page-binder-0123456789abcdefghijklmnopqrstuv";
    let hash = ordo_identity::federation::handle_of(binder);
    let return_to = format!("{SHOP}/orders?tab=mine");

    // The login page issues the code (`issueHandoff`).
    let (status, issued) = s.d.auth(
        LOGIN,
        "/auth/handoff",
        json!({"return_to": return_to, "binder_hash": hash}),
        Some(&ada.token),
    );
    assert_eq!(status, 200, "{issued}");
    let redirect = issued["redirect"].as_str().expect("a redirect");
    assert!(
        redirect.starts_with(&format!("{return_to}&handoff_code=")),
        "{redirect}"
    );
    let code = redirect.split_once("handoff_code=").expect("a code").1;

    // The shop page redeems it from its own origin (`finishSubdomainSignIn`).
    let (status, done) = s.d.auth(
        SHOP,
        "/auth/handoff/redeem",
        json!({"code": code, "binder": binder}),
        None,
    );
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["via"], "handoff");
    let shop_token = done["token"].as_str().expect("a session");
    assert_ne!(shop_token, ada.token);
    assert_eq!(principal_of(shop_token), ada.principal, "the same person");
    let mine = s.d.rows(SHOP, shop_token, "myOrders", json!({}));
    assert_eq!(ids(&mine), vec![a1]);

    // The code is spent.
    let (status, again) = s.d.auth(
        SHOP,
        "/auth/handoff/redeem",
        json!({"code": code, "binder": binder}),
        None,
    );
    assert_eq!(status, 401, "{again}");

    // A page the deployment does not list is not handed a session.
    let (status, body) = s.d.auth(
        LOGIN,
        "/auth/handoff",
        json!({"return_to": "http://localhost:5199/", "binder_hash": hash}),
        Some(&ada.token),
    );
    assert_eq!(status, 401, "{body}");
}

/// `productsWhere` asks each product's JSON `attributes` for its colour.
#[tokio::test(flavor = "multi_thread")]
async fn a_json_attribute_filter_returns_the_products_of_that_colour() {
    let _serial = SERIAL.lock().await;
    let s = open_shop("ordo_shopfront_attributes").await;
    let ada = customer(&s, 45, "Ada");

    let skus = |color: &str| -> Vec<String> {
        let mut v: Vec<String> =
            s.d.rows(SHOP, &ada.token, "productsWhere", json!({"color": color}))
                .iter()
                .map(|r| r["sku"].as_str().unwrap_or("").to_string())
                .collect();
        v.sort();
        v
    };
    assert_eq!(skus("red"), ["SF-APP-001", "SF-HOM-002", "SF-KIT-001"]);
    assert_eq!(skus("blue"), ["SF-HOM-001"]);
    assert!(skus("purple").is_empty());
    // `7` is a number in the document, so the text "7" does not match it.
    assert!(skus("7").is_empty());

    // A retired product leaves the filter.
    let lamp = s.product("SF-HOM-002").to_string();
    // The owner holds the derived `std` reads, which carry `rev`.
    let rev = s.d.rows(
        ADMIN,
        &s.olive.token,
        "std.products.get",
        json!({"id": lamp}),
    )[0]["rev"]
        .clone();
    let (status, row) = s.d.op(
        ADMIN,
        &s.sam.token,
        "editProduct",
        json!({"id": lamp, "rev": rev, "name": "Lamp", "category": "Home",
               "price": "79.00", "active": false, "attributes": {"color": "red", "material": "steel"}}),
    );
    assert_eq!(status, 200, "{row}");
    assert_eq!(skus("red"), ["SF-APP-001", "SF-KIT-001"]);
}
