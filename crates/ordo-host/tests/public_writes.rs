//! Public writes (`design-public-path.md` §5; ruled `public-data` asks 17
//! and 25), through the router the server serves and real Postgres.
//!
//! - A visitor with no token may call a public write. It creates rows only,
//!   runs as `$anonymous` in the deployment's space, and answers each new
//!   row's id and nothing else.
//! - It cannot reach outside what the op names: no other op, no other space,
//!   no field or row the op does not create. Each attempt is tried here.
//! - The human check is on by default, and refuses with the settings that
//!   would fix it. The hourly cap is per op, and only checked writes use it.
//! - A public write drops the public answers that read its entity.
//! - Every public request is counted in monitoring, whatever its outcome.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use futures::future::BoxFuture;
use ordo_api::{
    HumanVerdict, HumanVerifier, PublicHooks, PublicMonitor, PublicPath, PublicSettings,
};
use ordo_conformance::{fixture_space, Fixture, Pg};
use ordo_host::runtime::{assemble_embedded, CustodyChoice, RuntimeConfig};
use serde_json::{json, Value};
use tower::util::ServiceExt;

const PROGRAM: &str = "
entity contacts {
  name:    text(120)
  email:   text(200)
  message: text(2000)
  status:  text(20) = \"new\"
}

entity subscribers {
  email: text(200) unique
  source: text(40) = \"site\"
}

entity guestbook {
  body: text(200) public
}

entity accounts {
  name: text
}

op contact($name: text(120), $email: text(200), $message: text(2000)) public =
  set contacts { name: $name, email: $email, message: $message }

op subscribe($email: text(200)) public =
  set subscribers { email: $email, rev: null }

op sign($body: text(200)) public =
  set guestbook { body: $body }

op guestbookList public =
  guestbook[body]
  sort body
  take 50

op contactList =
  contacts[name, email, message, status]
  sort name
  take 50

op addContact($name: text(120), $email: text(200), $message: text(2000)) =
  set contacts { name: $name, email: $email, message: $message, status: \"vip\" }

op addAccount($name: text) =
  set accounts { name: $name }
";

/// `mem_p` owns the deployment's space (`system`). `mem_1` owns the tenant
/// `conformance`, whose rows no visitor may touch.
const SEED: &str = r#"
INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
  ('system', 'mem_p', 'mem_p', 'Pat', 'principal', 1, 'owner', 'active', DATE '2026-01-01'),
  (@SPACE@, 'mem_1', 'mem_1', 'Ana', 'principal', 1, 'owner', 'active', DATE '2026-01-01');
INSERT INTO "contacts" ("$space", "id", "rev", "name", "email", "message", "status") VALUES
  ('system', 'ct_1', 1, 'Old', 'old@example.com', 'first', 'new');
INSERT INTO "subscribers" ("$space", "id", "rev", "email", "source") VALUES
  ('system', 'sb_1', 1, 'taken@example.com', 'seed');
"#;

const SECRET: [u8; 32] = [7u8; 32];

fn mint(principal: &str) -> String {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&SECRET),
        &ordo_identity::TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(u64::MAX / 2),
            client: None,
            // Present now: the owner's platform ops ask for it.
            user_presence_at: Some(Timestamp(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            )),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

struct Answer {
    status: u16,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
    fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: &[(&str, String)],
    body: Option<Value>,
) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap()
        .to_vec();
    Answer {
        status,
        headers,
        body,
    }
}

fn free_addr() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

async fn boot(stem: &str) -> (ordo_host::runtime::Runtime, Pg) {
    let pg = Pg::scoped(stem);
    let f = Fixture::from_source(PROGRAM, SEED).expect("the program compiles");
    f.reset(&pg).expect("seeded");
    let cfg = RuntimeConfig {
        blob_routing: ordo_host::runtime::BlobRouting::Single,
        pg: ordo_exec::PoolConfig {
            db: pg.db.clone(),
            size: 8,
            ..ordo_exec::PoolConfig::from_env()
        },
        addr: free_addr(),
        shutdown_grace: Duration::from_secs(1),
        space: fixture_space(),
        custody: CustodyChoice::Test,
        token_secret: SECRET,
        page_key: ordo_api::PageKey::from_secret(&[8u8; 32]),
        checkpoint: None,
        security_checkpoint: false,
        apex_domain: None,
        cors: None,
        blob: None,
        exit_bundles: None,
        import_bundles: None,
        replica: "public-writes-test".into(),
        usage_flush: Duration::from_secs(60),
        usage_rollup: Duration::from_secs(3600),
        sync: Default::default(),
        auth: None,
        credentials: None,
        presence: Default::default(),
        pausing: Default::default(),
        delegable: Default::default(),
        access_template: Default::default(),
        deployment_owners: Vec::new(),
        provisioning: Default::default(),
        recovery: Default::default(),
        second_factor: Default::default(),
        mail: None,
    };
    let f = Fixture::from_source(PROGRAM, SEED).unwrap();
    let rt = assemble_embedded(cfg, f).await.expect("boots");
    (rt, pg)
}

/// A provider that passes `good`, cannot be reached on `down`, and fails
/// everything else.
struct FakeHuman;

impl HumanVerifier for FakeHuman {
    fn provider(&self) -> &'static str {
        "turnstile"
    }
    fn verify(&self, token: String) -> BoxFuture<'static, HumanVerdict> {
        Box::pin(async move {
            match token.as_str() {
                "good" => HumanVerdict::Pass,
                "down" => HumanVerdict::Unavailable("test".into()),
                _ => HumanVerdict::Fail,
            }
        })
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<(String, &'static str, &'static str)>>);

impl PublicMonitor for Recorder {
    fn count(&self, op: &str, kind: &'static str, outcome: &'static str) {
        self.0.lock().unwrap().push((op.to_string(), kind, outcome));
    }
}

impl Recorder {
    fn has(&self, op: &str, kind: &str, outcome: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(o, k, out)| o == op && *k == kind && *out == outcome)
            .count()
    }
}

/// The door with the given public settings, a fake provider and a recorder.
fn door_with(
    rt: &ordo_host::runtime::Runtime,
    settings: PublicSettings,
    human: bool,
) -> (axum::Router, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let mut api = rt.api.clone();
    api.public = PublicPath::with_hooks(
        settings,
        PublicHooks {
            purge: None,
            human: human.then(|| Arc::new(FakeHuman) as Arc<dyn HumanVerifier>),
            monitor: Some(recorder.clone()),
        },
    );
    (ordo_api::router(api), recorder)
}

fn unchecked() -> PublicSettings {
    PublicSettings {
        human_check: false,
        ..Default::default()
    }
}

fn form() -> Value {
    json!({ "name": "Vee", "email": "vee@example.com", "message": "hello" })
}

async fn visitor_post(
    app: &axum::Router,
    op: &str,
    extra: &[(&str, String)],
    body: Value,
) -> Answer {
    call(app, "POST", &format!("/op/{op}"), extra, Some(body)).await
}

fn rows(pg: &Pg, sql: &str) -> Vec<Vec<Option<String>>> {
    pg.query(sql).expect("query")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_public_write_creates_a_row_as_the_anonymous_principal() {
    let (rt, pg) = boot("ordo_public_write_create").await;
    let (app, _) = door_with(&rt, unchecked(), false);

    let a = visitor_post(&app, "contact", &[], form()).await;
    assert_eq!(a.status, 200, "{}", a.text());
    assert_eq!(
        a.header("cache-control"),
        "no-store",
        "a write is never cached"
    );
    // Write-only: the answer names the new row's id and nothing of it.
    let answer = a.json();
    let rows_out = answer["rows"].as_array().expect("rows");
    assert_eq!(rows_out.len(), 1);
    let row = rows_out[0].as_object().expect("row");
    assert_eq!(row.keys().collect::<Vec<_>>(), ["created"], "{}", a.text());
    assert!(!a.text().contains("vee@example.com"), "{}", a.text());
    assert!(
        a.header("x-ordo-head").is_empty(),
        "no signed head: it would show the deployment space's history"
    );

    // It landed in the deployment's space, with the op's default.
    let landed = rows(
        &pg,
        "SELECT \"$space\", \"status\" FROM \"contacts\" WHERE \"email\" = 'vee@example.com'",
    );
    assert_eq!(
        landed,
        vec![vec![Some("system".to_string()), Some("new".to_string())]]
    );

    // The command log names the anonymous principal and no address.
    let actor = rows(
        &pg,
        "SELECT \"actor\" FROM \"$commandlog\" WHERE \"space\" = 'system' ORDER BY \"idx\" DESC LIMIT 1",
    );
    let actor = actor[0][0].clone().unwrap_or_default();
    assert!(actor.contains("$anonymous"), "{actor}");
}

/// A visitor tries every way out of the op's grant. Each one is refused,
/// and nothing outside the op's one create lands.
#[tokio::test(flavor = "multi_thread")]
async fn a_public_write_cannot_reach_outside_what_its_op_names() {
    let (rt, pg) = boot("ordo_public_write_escape").await;
    let (app, _) = door_with(&rt, unchecked(), false);
    let denial = |op: &str| {
        format!(
            r#"{{"error":{{"code":"access_denied","message":"missing bearer token","op":"{op}"}}}}"#
        )
        .into_bytes()
    };

    // A write that is not public: the door's one answer, byte for byte.
    let a = visitor_post(&app, "addContact", &[], form()).await;
    assert_eq!((a.status, a.body.clone()), (403, denial("addContact")));
    let a = visitor_post(&app, "addAccount", &[], json!({ "name": "x" })).await;
    assert_eq!((a.status, a.body.clone()), (403, denial("addAccount")));
    // Platform ops are never public.
    let a = visitor_post(&app, "std.space.create", &[], json!({})).await;
    assert_eq!(a.status, 403, "{}", a.text());

    // Another space, by header or by envelope: the same answer.
    let tenant = fixture_space().0;
    let a = visitor_post(&app, "contact", &[("x-ordo-space", tenant.clone())], form()).await;
    assert_eq!((a.status, a.body.clone()), (403, denial("contact")));
    let mut envelope = form();
    envelope["$space"] = json!(tenant);
    let a = visitor_post(&app, "contact", &[], envelope).await;
    assert_eq!((a.status, a.body.clone()), (403, denial("contact")));

    // A field the op does not set, an id to overwrite a row, a caller.
    let mut extra = form();
    extra["status"] = json!("vip");
    let a = visitor_post(&app, "contact", &[], extra).await;
    assert_eq!(a.code(), "unknown_param", "{}", a.text());
    let mut addressed = form();
    addressed["id"] = json!("ct_1");
    let a = visitor_post(&app, "contact", &[], addressed).await;
    assert_eq!(a.code(), "unknown_param", "{}", a.text());
    let mut caller = form();
    caller["$caller"] = json!("mem_p");
    let a = visitor_post(&app, "contact", &[], caller).await;
    assert!(a.status >= 400, "{}", a.text());
    let mut refined = form();
    refined["$refine"] = json!({ "where": [] });
    let a = visitor_post(&app, "contact", &[], refined).await;
    assert!(a.status >= 400, "{}", a.text());

    // The wrong method cannot turn it into a read or a delete.
    let a = call(&app, "GET", "/op/contact", &[], None).await;
    assert_eq!(a.status, 405, "{}", a.text());
    let a = call(&app, "DELETE", "/op/contact", &[], None).await;
    assert_eq!(a.status, 405, "{}", a.text());

    // A token that is not valid is not a visitor: it gets the token answer.
    let a = visitor_post(
        &app,
        "contact",
        &[
            ("authorization", "Bearer not-a-token".into()),
            ("x-ordo-space", "system".into()),
        ],
        form(),
    )
    .await;
    assert_eq!(
        (a.status, a.code().as_str()),
        (403, "access_denied"),
        "{}",
        a.text()
    );

    // Idempotency is shared by every visitor, so it is bound to the payload.
    let key = ("idempotency-key", "k-1".to_string());
    let first = visitor_post(&app, "contact", std::slice::from_ref(&key), form()).await;
    assert_eq!(first.status, 200, "{}", first.text());
    let replay = visitor_post(&app, "contact", std::slice::from_ref(&key), form()).await;
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body, first.body, "an identical submission replays");
    let mut other = form();
    other["message"] = json!("someone else's");
    let clash = visitor_post(&app, "contact", &[key], other).await;
    assert_eq!(clash.code(), "idempotency_mismatch", "{}", clash.text());

    // Nothing else changed: the seeded row is as it was, nothing reached
    // the tenant, no account was made, and exactly one contact was added.
    let seeded = rows(
        &pg,
        "SELECT \"name\", \"status\", \"rev\" FROM \"contacts\" WHERE \"id\" = 'ct_1'",
    );
    assert_eq!(
        seeded,
        vec![vec![
            Some("Old".to_string()),
            Some("new".to_string()),
            Some("1".to_string())
        ]]
    );
    let in_tenant = rows(
        &pg,
        &format!("SELECT count(*) FROM \"contacts\" WHERE \"$space\" = '{tenant}'"),
    );
    assert_eq!(in_tenant[0][0].as_deref(), Some("0"));
    assert_eq!(
        rows(&pg, "SELECT count(*) FROM \"accounts\"")[0][0].as_deref(),
        Some("0")
    );
    assert_eq!(
        rows(
            &pg,
            "SELECT count(*) FROM \"contacts\" WHERE \"status\" = 'vip'"
        )[0][0]
            .as_deref(),
        Some("0")
    );
    assert_eq!(
        rows(&pg, "SELECT count(*) FROM \"contacts\"")[0][0].as_deref(),
        Some("2"),
        "the seeded row and the one submission"
    );

    // And a member still reads what visitors wrote: the data is private, not
    // lost.
    let listed = call(
        &app,
        "GET",
        "/op/contactList",
        &[
            ("authorization", format!("Bearer {}", mint("mem_p"))),
            ("x-ordo-space", "system".into()),
        ],
        None,
    )
    .await;
    assert_eq!(listed.status, 200, "{}", listed.text());
    assert!(listed.text().contains("vee@example.com"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_human_check_is_on_by_default_and_names_its_settings() {
    let (rt, pg) = boot("ordo_public_write_human").await;
    let count = || {
        rows(&pg, "SELECT count(*) FROM \"contacts\"")[0][0]
            .clone()
            .unwrap_or_default()
    };

    // The shipped settings with no provider: refused, naming both fixes.
    let (app, rec) = door_with(&rt, PublicSettings::default(), false);
    let a = visitor_post(&app, "contact", &[], form()).await;
    assert_eq!(
        (a.status, a.code().as_str()),
        (403, "human_check_failed"),
        "{}",
        a.text()
    );
    assert!(
        a.text().contains("ORDO_PUBLIC_HUMAN_SECRET_FILE"),
        "{}",
        a.text()
    );
    assert!(
        a.text().contains("ORDO_AUTH_HUMAN_CHECK_SECRET_FILE"),
        "{}",
        a.text()
    );
    assert!(
        a.text().contains("ORDO_PUBLIC_HUMAN_CHECK=off"),
        "{}",
        a.text()
    );
    assert_eq!(rec.has("contact", "write", "human_check_failed"), 1);

    // With a provider: no token, a refused token, an unreachable provider.
    let (app, _) = door_with(&rt, PublicSettings::default(), true);
    let a = visitor_post(&app, "contact", &[], form()).await;
    assert_eq!(a.code(), "human_check_failed");
    assert!(a.text().contains("X-Ordo-Human"), "{}", a.text());
    for bad in ["bot", "down"] {
        let a = visitor_post(&app, "contact", &[("x-ordo-human", bad.into())], form()).await;
        assert_eq!(a.code(), "human_check_failed", "{bad}: {}", a.text());
    }
    let huge = "g".repeat(5000);
    let a = visitor_post(&app, "contact", &[("x-ordo-human", huge)], form()).await;
    assert_eq!(a.code(), "human_check_failed");
    assert_eq!(count(), "1", "nothing written by a failed check");

    // A confirmed token writes.
    let a = visitor_post(&app, "contact", &[("x-ordo-human", "good".into())], form()).await;
    assert_eq!(a.status, 200, "{}", a.text());
    assert_eq!(count(), "2");
}

#[tokio::test(flavor = "multi_thread")]
async fn public_writes_past_the_hourly_cap_are_refused_by_name() {
    let (rt, _pg) = boot("ordo_public_write_cap").await;
    let (app, rec) = door_with(
        &rt,
        PublicSettings {
            writes_per_hour: 2,
            ..Default::default()
        },
        true,
    );
    let human = |t: &str| vec![("x-ordo-human", t.to_string())];

    // Failed checks do not use the hour.
    for _ in 0..5 {
        let a = visitor_post(&app, "contact", &human("bot"), form()).await;
        assert_eq!(a.code(), "human_check_failed");
    }
    for _ in 0..2 {
        let a = visitor_post(&app, "contact", &human("good"), form()).await;
        assert_eq!(a.status, 200, "{}", a.text());
    }
    let over = visitor_post(&app, "contact", &human("good"), form()).await;
    assert_eq!(
        (over.status, over.code().as_str()),
        (429, "quota_exceeded"),
        "{}",
        over.text()
    );
    assert!(
        over.text().contains("ORDO_PUBLIC_WRITES_PER_HOUR"),
        "{}",
        over.text()
    );
    // A full hour is refused before the provider is asked.
    let over = visitor_post(&app, "contact", &human("bot"), form()).await;
    assert_eq!(over.code(), "quota_exceeded");
    // The cap is per op.
    let a = visitor_post(&app, "sign", &human("good"), json!({ "body": "hi" })).await;
    assert_eq!(a.status, 200, "{}", a.text());
    assert_eq!(rec.has("contact", "write", "limited"), 2);
    assert_eq!(rec.has("contact", "write", "written"), 2);

    // Zero turns the cap off.
    let (app, _) = door_with(
        &rt,
        PublicSettings {
            writes_per_hour: 0,
            human_check: false,
            ..Default::default()
        },
        false,
    );
    for _ in 0..5 {
        assert_eq!(
            visitor_post(&app, "sign", &[], json!({ "body": "x" }))
                .await
                .status,
            200
        );
    }
}

/// A public write is an ordinary write: it drops the public answers that
/// read its entity, as a member's write does.
#[tokio::test(flavor = "multi_thread")]
async fn a_public_write_drops_the_public_answers_that_read_its_entity() {
    let (rt, _pg) = boot("ordo_public_write_invalidates").await;
    let (app, rec) = door_with(&rt, unchecked(), false);

    let first = call(&app, "GET", "/op/guestbookList", &[], None).await;
    assert_eq!(first.header("x-ordo-cache"), "miss");
    let second = call(&app, "GET", "/op/guestbookList", &[], None).await;
    assert_eq!(second.header("x-ordo-cache"), "hit");

    let a = visitor_post(&app, "sign", &[], json!({ "body": "was here" })).await;
    assert_eq!(a.status, 200, "{}", a.text());

    let mut fresh = None;
    for _ in 0..100 {
        let a = call(&app, "GET", "/op/guestbookList", &[], None).await;
        if a.header("x-ordo-cache") == "miss" {
            fresh = Some(a);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let fresh = fresh.expect("the public answer was dropped after the write");
    assert!(fresh.text().contains("was here"), "{}", fresh.text());

    // Every public request was counted: reads, the write, and a refusal.
    assert!(rec.has("guestbookList", "read", "miss") >= 2);
    assert!(rec.has("guestbookList", "read", "hit") >= 1);
    assert_eq!(rec.has("sign", "write", "written"), 1);
    let _ = call(&app, "GET", "/op/contactList", &[], None).await;
    let _ = call(&app, "GET", "/op/noSuchOp", &[], None).await;
    assert_eq!(
        rec.has("-", "door", "refused"),
        2,
        "named `-`, never by the name asked"
    );
}

/// Proxy mode (`ordo_host::proxy`): a busy visitor behind a trusted proxy is
/// slowed down and still served. Another visitor, and a caller with a
/// token, are not slowed. Nothing is refused.
#[tokio::test(flavor = "multi_thread")]
async fn proxy_mode_slows_a_busy_visitor_and_never_refuses() {
    let (rt, _pg) = boot("ordo_proxy_mode").await;
    let (settings, trusted) = ordo_host::proxy::from_vars(&|k: &str| match k {
        "ORDO_PROXY_HEADER" => Some("X-Forwarded-For".into()),
        "ORDO_PROXY_TRUSTED" => Some("10.0.0.0/8".into()),
        "ORDO_PROXY_SLOW_AFTER" => Some("2".into()),
        "ORDO_PROXY_SLOW_MS" => Some("300".into()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    let pacer = Arc::new(ordo_host::proxy::Pacer::new(settings, trusted, None));
    let (inner, rec) = door_with(&rt, unchecked(), false);
    let app = ordo_host::proxy::apply(inner, Some(pacer));

    let timed = |visitor: &'static str, token: bool| {
        let app = app.clone();
        async move {
            let mut req = Request::get("/op/guestbookList").header("x-forwarded-for", visitor);
            if token {
                req = req
                    .header("authorization", format!("Bearer {}", mint("mem_p")))
                    .header("x-ordo-space", "system");
            }
            let mut req = req.body(Body::empty()).unwrap();
            req.extensions_mut().insert(axum::extract::ConnectInfo(
                "10.0.0.1:443".parse::<std::net::SocketAddr>().unwrap(),
            ));
            let started = std::time::Instant::now();
            let status = app.oneshot(req).await.unwrap().status().as_u16();
            (status, started.elapsed())
        }
    };
    for _ in 0..2 {
        let (status, took) = timed("203.0.113.9", false).await;
        assert_eq!(status, 200);
        assert!(took < Duration::from_millis(300), "{took:?}");
    }
    let (status, took) = timed("203.0.113.9", false).await;
    assert_eq!(status, 200, "slowed, still served");
    assert!(took >= Duration::from_millis(300), "{took:?}");
    let (status, took) = timed("203.0.113.10", false).await;
    assert_eq!(status, 200);
    assert!(
        took < Duration::from_millis(300),
        "another visitor: {took:?}"
    );
    for _ in 0..3 {
        let (_, took) = timed("203.0.113.9", true).await;
        assert!(
            took < Duration::from_millis(300),
            "a token is not paced: {took:?}"
        );
    }
    assert!(
        rec.has("guestbookList", "read", "hit") + rec.has("guestbookList", "read", "miss") >= 4
    );
}

/// A tenant's own CORS origins (`design-public-path.md` §8). Off by
/// default, with a refusal that names the setting; once allowed, the owner's
/// list lets that origin read that tenant's answers and no other space's.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_lists_its_own_origins_when_the_deployment_allows_it() {
    let (rt, _pg) = boot("ordo_tenant_origins").await;
    let tenant = fixture_space().0;
    let owner = |space: &str| {
        vec![
            ("authorization", format!("Bearer {}", mint("mem_1"))),
            ("x-ordo-space", space.to_string()),
        ]
    };

    // The shipped default: no tenant may list origins, and the refusal says
    // which setting allows it.
    let app = ordo_host::runtime::door(&rt);
    let a = call(
        &app,
        "POST",
        "/op/std.space.setOrigins",
        &owner(&tenant),
        Some(json!({ "origins": "https://shop.example" })),
    )
    .await;
    assert_eq!(a.code(), "tenant_refused", "{}", a.text());
    assert!(
        a.text().contains("ORDO_CORS_TENANT_ORIGINS_MAX=10"),
        "{}",
        a.text()
    );

    // Allowed: the owner sets a list, and the layer answers it per space.
    let map = Arc::new(ordo_host::tenant_origins::TenantOriginMap::new(
        5,
        rt.pool.clone(),
    ));
    let set = map
        .execute(
            ordo_host::tenant_origins::SET_ORIGINS,
            &tenant,
            &ordo_api::BoundParams(
                json!({ "origins": "https://shop.example" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            "mem_1",
            1,
        )
        .await
        .expect("set");
    assert_eq!(set.rows[0]["origins"], "https://shop.example");
    // Another node learns it from the table.
    let other = ordo_host::tenant_origins::TenantOriginMap::new(5, rt.pool.clone());
    other.load().await.expect("load");
    assert_eq!(other.get(&tenant), ["https://shop.example"]);

    let app = ordo_api::cors::apply_layers(
        ordo_api::router(rt.api.clone()),
        ordo_api::cors::Cors {
            deployment: None,
            tenants: Some(map as Arc<dyn ordo_api::cors::TenantOrigins>),
            public_ops: Some(rt.api.registry.clone() as Arc<dyn ordo_api::cors::PublicOps>),
        },
    );
    let mut headers = owner(&tenant);
    headers.push(("origin", "https://shop.example".into()));
    let a = call(&app, "GET", "/op/contactList", &headers, None).await;
    assert_eq!(a.status, 200, "{}", a.text());
    assert_eq!(
        a.header("access-control-allow-origin"),
        "https://shop.example"
    );

    let mut headers = vec![
        ("authorization", format!("Bearer {}", mint("mem_p"))),
        ("x-ordo-space", "system".to_string()),
    ];
    headers.push(("origin", "https://shop.example".into()));
    let a = call(&app, "GET", "/op/contactList", &headers, None).await;
    assert_eq!(a.status, 200, "{}", a.text());
    assert_eq!(
        a.header("access-control-allow-origin"),
        "",
        "the tenant's origin cannot read another space's answers"
    );

    // A public answer is readable from any page.
    let a = call(
        &app,
        "GET",
        "/op/guestbookList",
        &[("origin", "https://anyone.example".into())],
        None,
    )
    .await;
    assert_eq!(a.header("access-control-allow-origin"), "*");
}

/// Ask 31: a create keyed on a unique field gives a visitor the same answer
/// for a new email and for one already on the list, and writes nothing for
/// the second.
#[tokio::test(flavor = "multi_thread")]
async fn a_keyed_public_create_answers_alike_for_a_new_and_an_existing_email() {
    let (rt, pg) = boot("ordo_public_write_keyed").await;
    let (app, rec) = door_with(&rt, unchecked(), false);
    let table = || {
        rows(
            &pg,
            "SELECT \"id\", \"rev\", \"email\", \"source\" FROM \"subscribers\" ORDER BY \"email\"",
        )
    };
    let before = table();
    assert_eq!(before.len(), 1);

    let fresh = visitor_post(
        &app,
        "subscribe",
        &[],
        json!({ "email": "new@example.com" }),
    )
    .await;
    assert_eq!(fresh.status, 200, "{}", fresh.text());
    let after_new = table();
    assert_eq!(after_new.len(), 2, "the new email was written");

    let dup = visitor_post(
        &app,
        "subscribe",
        &[],
        json!({ "email": "taken@example.com" }),
    )
    .await;
    assert_eq!(dup.status, 200, "{}", dup.text());
    assert_eq!(fresh.body, dup.body, "byte-identical answers");
    assert_eq!(fresh.header("cache-control"), dup.header("cache-control"));
    assert_eq!(fresh.header("content-type"), dup.header("content-type"));
    assert!(
        fresh.json()["rows"].as_array().is_some_and(Vec::is_empty),
        "no id is named: {}",
        fresh.text()
    );
    assert!(!fresh.text().contains("sb_1") && !dup.text().contains("sb_1"));

    // The existing row is untouched and no second row exists.
    assert_eq!(table(), after_new, "nothing written for the duplicate");
    assert!(after_new.contains(&vec![
        Some("sb_1".to_string()),
        Some("1".to_string()),
        Some("taken@example.com".to_string()),
        Some("seed".to_string()),
    ]));
    // The same email twice in a row is also alike.
    let again = visitor_post(
        &app,
        "subscribe",
        &[],
        json!({ "email": "new@example.com" }),
    )
    .await;
    assert_eq!(again.body, fresh.body);
    assert_eq!(rec.has("subscribe", "write", "written"), 1);
    assert_eq!(rec.has("subscribe", "write", "duplicate"), 2);
}

/// Ask 32: bad tokens never use the hour, however many arrive.
#[tokio::test(flavor = "multi_thread")]
async fn a_flood_of_bad_tokens_leaves_the_hourly_cap_for_a_real_visitor() {
    let (rt, _pg) = boot("ordo_public_write_flood").await;
    let (app, rec) = door_with(
        &rt,
        PublicSettings {
            writes_per_hour: 3,
            ..Default::default()
        },
        true,
    );
    for i in 0..60 {
        let t = if i % 2 == 0 { "bot" } else { "down" };
        let a = visitor_post(&app, "contact", &[("x-ordo-human", t.into())], form()).await;
        assert_eq!(a.code(), "human_check_failed", "{}", a.text());
    }
    for _ in 0..3 {
        let a = visitor_post(&app, "contact", &[("x-ordo-human", "good".into())], form()).await;
        assert_eq!(a.status, 200, "{}", a.text());
    }
    let over = visitor_post(&app, "contact", &[("x-ordo-human", "good".into())], form()).await;
    assert_eq!(over.code(), "quota_exceeded");
    assert_eq!(rec.has("contact", "write", "human_check_failed"), 60);
    assert_eq!(rec.has("contact", "write", "written"), 3);
}
