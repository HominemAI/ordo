//! Stress: the door's store pool (one connection until 2026-10-24) under the notes e2e's shape
//! (board task `store-pool-held-blocks-signin`).
//!
//! In CI on 2026-10-24 (run 40123159223), after about 34 tenants had been
//! made, the store pool stayed in use for about 2 minutes and every emailed
//! sign-in was answered `503 server_busy`. This recreates that shape against
//! a real deployment assembled in this process: people sign up by emailed
//! code (each one a new tenant, as `ORDO_PROVISION_SELF_SERVE=on-signup`
//! makes), people sign in again, sessions are refreshed, the ceremony sweep
//! runs on a short tick, the mail outbox drains to a relay on loopback, and
//! the door rebuilds over every tenant on a short tick.
//!
//! **Ignored**: it runs for minutes and makes many databases. Run it through
//! `scripts/stress-store-pool.sh`, which starts a cluster of its own and
//! repeats it at several shapes. The shape comes from the environment:
//!
//! | Variable | Means | Default |
//! |---|---|---|
//! | `STRESS_WORKERS` | the door runtime's worker threads (CI has 2 to 4 cores) | 2 |
//! | `STRESS_PEOPLE` | people who sign up, so tenants made | 40 |
//! | `STRESS_CONCURRENCY` | client threads at once | 8 |
//! | `STRESS_REFRESHES` | refreshes after each sign-in | 3 |
//! | `STRESS_SECS` | the run's bound | 240 |
//! | `STRESS_DB` | the control database | `ordo_stress_store_pool` |
//!
//! It fails when any emailed sign-in is answered `server_busy` or a client
//! request times out, and prints what it counted either way. The door's own
//! log (`store pool: server_busy ...` and the `pool incident:` block) is on
//! stderr; the script keeps it.

mod mail_support;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::KeyProvider as _;
use ordo_exec::{Pool, PoolConfig};
use serde_json::{json, Value};

const SOURCE: &str = include_str!("../../../examples/notes/notes.ordo");
const HOME: &str = "stress_home";
const MASTER: &str = "the-store-pool-stress-master-key";
const TOKEN_KEY: &str = "5555555555555555555555555555555555555555555555555555555555555555";
const PAGE_KEY: &str = "6666666666666666666666666666666666666666666666666666666666666666";
const PAGE: &str = "http://localhost:5173";

fn knob<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    }
}

/// Drop every database a previous run of this test left: the control
/// database and the tenants it made, which carry its name's tenants'
/// prefix only through `$spacedb`, so they are read from there first.
fn drop_leftovers(db: &str) {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    if let Ok(rows) = pg.query(r#"SELECT DISTINCT "db" FROM "$spacedb""#) {
        for name in rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
        {
            let _ = admin.exec(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"));
        }
    }
    let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"));
}

/// The notes program applied and the deployment's custody made: what
/// `ordo apply` and the first boot leave (`tests/notes_app.rs`).
async fn deployment_with_notes(db: &str) {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    admin
        .exec(&format!("CREATE DATABASE {db}"))
        .expect("control database");
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    let plain = Fixture::from_source(SOURCE, "").expect("notes.ordo compiles");
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
    let mut prog = Fixture::from_source(SOURCE, "").expect("notes.ordo compiles");
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
        actor: "operator:stress".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:stress",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    l.local_addr().expect("addr").port()
}

/// One HTTP/1.1 request: `(status, body)`. Status 0 is no answer within
/// the read timeout.
fn send(port: u16, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, Value) {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return (0, Value::Null);
    };
    s.set_read_timeout(Some(Duration::from_secs(20))).ok();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
         Content-Type: application/json\r\nOrigin: {PAGE}\r\nAccept: application/json\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    if let Some(t) = token {
        req.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    if s.write_all(req.as_bytes()).is_err() {
        return (0, Value::Null);
    }
    let mut raw = String::new();
    s.read_to_string(&mut raw).ok();
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (
        status,
        serde_json::from_str(body.trim()).unwrap_or(Value::Null),
    )
}

#[derive(Default)]
struct Counts {
    signups: AtomicUsize,
    signins: AtomicUsize,
    refreshes: AtomicUsize,
    busy: AtomicUsize,
    other_failures: AtomicUsize,
    timeouts: AtomicUsize,
    no_mail: AtomicUsize,
}

impl Counts {
    fn line(&self) -> String {
        format!(
            "signups={} signins={} refreshes={} busy_503={} other_failures={} timeouts={} no_mail={}",
            self.signups.load(Ordering::Relaxed),
            self.signins.load(Ordering::Relaxed),
            self.refreshes.load(Ordering::Relaxed),
            self.busy.load(Ordering::Relaxed),
            self.other_failures.load(Ordering::Relaxed),
            self.timeouts.load(Ordering::Relaxed),
            self.no_mail.load(Ordering::Relaxed),
        )
    }
}

/// The code in the `n`th message sent to `address`, waiting up to 20 s.
fn code_for(relay: &mail_support::FakeRelay, address: &str, n: usize) -> Option<String> {
    let started = Instant::now();
    let rcpt = format!("<{address}>");
    while started.elapsed() < Duration::from_secs(20) {
        let mine: Vec<String> = relay
            .sessions()
            .into_iter()
            .filter(|s| s.commands.iter().any(|c| c.contains(&rcpt)))
            .filter_map(|s| s.data)
            .collect();
        if let Some(data) = mine.get(n) {
            let start = data.find("code is ")? + "code is ".len();
            return Some(
                data[start..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect(),
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// One emailed sign-in, then the tenant's readiness, then refreshes.
fn one_person(
    port: u16,
    relay: &mail_support::FakeRelay,
    counts: &Counts,
    address: &str,
    n: usize,
    refreshes: usize,
) {
    let count = |status: u16, said: &Value| {
        if status == 0 {
            counts.timeouts.fetch_add(1, Ordering::Relaxed);
        } else if status == 503 && said.to_string().contains("server_busy") {
            counts.busy.fetch_add(1, Ordering::Relaxed);
        } else {
            counts.other_failures.fetch_add(1, Ordering::Relaxed);
            eprintln!("stress: {address} answered {status}: {said}");
        }
    };
    let (status, begun) = send(
        port,
        "POST",
        "/auth/begin",
        None,
        &json!({"method": "email", "identifier": address}).to_string(),
    );
    if status != 200 {
        return count(status, &begun);
    }
    let Some(code) = code_for(relay, address, n) else {
        counts.no_mail.fetch_add(1, Ordering::Relaxed);
        eprintln!("stress: no mail reached {address} (#{n}) in 20 s");
        return;
    };
    let (status, done) = send(
        port,
        "POST",
        "/auth/complete",
        None,
        &json!({
            "method": "email", "kind": "email_code",
            "challenge": begun["challenge"], "binder": begun["binder"], "code": code,
        })
        .to_string(),
    );
    if status != 200 {
        return count(status, &done);
    }
    let Some(mut token) = done["token"].as_str().map(str::to_string) else {
        return count(status, &done);
    };
    if n == 0 {
        counts.signups.fetch_add(1, Ordering::Relaxed);
    } else {
        counts.signins.fetch_add(1, Ordering::Relaxed);
    }
    let mut space = done["space"].as_str().map(str::to_string);
    // The tenant, as the SDK's `waitForTenant` waits for it.
    if done["provisioning"] == "in progress" {
        let started = Instant::now();
        loop {
            let (status, said) = send(port, "GET", "/auth/provisioning", Some(&token), "");
            if status != 200 {
                return count(status, &said);
            }
            match said["state"].as_str() {
                Some("ready") => {
                    space = said["space"].as_str().map(str::to_string);
                    break;
                }
                Some("provisioning") if started.elapsed() < Duration::from_secs(60) => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                _ => return count(0, &said),
            }
        }
    }
    // The page's live feed, dropped the way a browser going offline drops
    // it: every other time before the first rows arrive.
    if let Some(space) = &space {
        feed_and_drop(port, space, &token, n.is_multiple_of(2));
    }
    for _ in 0..refreshes {
        let (status, said) = send(port, "POST", "/auth/refresh", Some(&token), "{}");
        if status != 200 {
            return count(status, &said);
        }
        if let Some(t) = said["token"].as_str() {
            token = t.to_string();
        }
        counts.refreshes.fetch_add(1, Ordering::Relaxed);
    }
}

/// Open `myNotes`' live feed and close it abruptly, at once or after the
/// first bytes (at most 2 s).
fn feed_and_drop(port: u16, space: &str, token: &str, at_once: bool) {
    let Ok(mut feed) = TcpStream::connect(("127.0.0.1", port)) else {
        return;
    };
    let request = format!(
        "GET /op/myNotes?live=1 HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\
         X-Ordo-Space: {space}\r\nAuthorization: Bearer {token}\r\nOrigin: {PAGE}\r\n\r\n"
    );
    if feed.write_all(request.as_bytes()).is_err() || at_once {
        return;
    }
    feed.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut buf = [0u8; 4096];
    let _ = feed.read(&mut buf);
}

/// Say what everything is doing, once, when the run has stalled.
fn stalled(db: &str, relay: &mail_support::FakeRelay) {
    eprintln!(
        "stress: STALLED: relay accepted {} connection(s), filed {} session(s)",
        relay.accepted.load(Ordering::SeqCst),
        relay.sessions().len()
    );
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    match pg.query(
        r#"SELECT "id", "kind", "attempts", "next_at" - extract(epoch FROM now())::bigint AS due_in, extract(epoch FROM now())::bigint - "queued_at" AS age FROM "$mailoutbox" ORDER BY "id""#,
    ) {
        Ok(rows) => eprintln!("stress: STALLED: outbox (id, kind, attempts, due in s, age s) {rows:?}"),
        Err(e) => eprintln!("stress: STALLED: outbox unreadable: {e}"),
    }
    let snapshot = std::env::temp_dir().join(format!("stress-stall-{}.txt", std::process::id()));
    let _ = std::process::Command::new("bash")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/pg-snapshot.sh"
        ))
        .arg(&snapshot)
        .arg("stalled")
        .env("PGHOST", &pg.host)
        .env("PGPORT", &pg.port)
        .status();
    eprintln!(
        "stress: STALLED: postgres\n{}",
        std::fs::read_to_string(&snapshot).unwrap_or_default()
    );
    // Every thread's stack (macOS `sample`), so a blocked runtime thread
    // shows where it is blocked.
    let stacks = std::env::temp_dir().join(format!("stress-stall-{}.sample", std::process::id()));
    let sampled = std::process::Command::new("sample")
        .arg(std::process::id().to_string())
        .arg("2")
        .arg("-file")
        .arg(&stacks)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    eprintln!(
        "stress: STALLED: thread stacks in {} ({sampled:?})",
        stacks.display()
    );
}

#[test]
#[ignore = "minutes long; run through scripts/stress-store-pool.sh"]
fn the_store_pool_stays_free_under_the_e2e_shape() {
    let workers: usize = knob("STRESS_WORKERS", 2);
    let people: usize = knob("STRESS_PEOPLE", 40);
    let concurrency: usize = knob("STRESS_CONCURRENCY", 8);
    let refreshes: usize = knob("STRESS_REFRESHES", 3);
    let bound = Duration::from_secs(knob("STRESS_SECS", 240));
    let db: String = knob("STRESS_DB", "ordo_stress_store_pool".to_string());
    eprintln!(
        "stress: workers={workers} people={people} concurrency={concurrency} \
         refreshes={refreshes} bound={}s db={db}",
        bound.as_secs()
    );

    // The relay on a runtime of its own: it stands for a mail server outside
    // the door, so a starved door runtime must not starve it too.
    let relay_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("relay runtime");
    let relay = Arc::new(relay_rt.block_on(mail_support::FakeRelay::start_plaintext()));

    // The door's runtime, sized like a CI runner's.
    let door_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("door runtime");

    drop_leftovers(&db);
    door_rt.block_on(deployment_with_notes(&db));

    let port = free_port();
    let base = PoolConfig::from_env();
    // The e2e stack's settings (examples/notes/e2e/stack.sh), with the
    // sweeps and the door's rebuild on short ticks.
    let settings: Vec<(&str, String)> = vec![
        ("ORDO_PG_POOL_SIZE", "3".into()),
        ("ORDO_PG_IDLE_SECS", "5".into()),
        ("ORDO_PG_HOST", base.host.clone()),
        ("ORDO_PG_PORT", base.port.to_string()),
        ("ORDO_PG_USER", base.user.clone()),
        ("ORDO_PG_DB", db.clone()),
        ("ORDO_SPACE", HOME.into()),
        ("ORDO_KEY_PROVIDER", "local".into()),
        ("ORDO_MASTER_KEY", MASTER.into()),
        ("ORDO_DEV_UNSAFE_TEST_KEYS", "1".into()),
        ("ORDO_TOKEN_KEY", TOKEN_KEY.into()),
        ("ORDO_PAGE_KEY", PAGE_KEY.into()),
        ("ORDO_HOST_ADDR", format!("127.0.0.1:{port}")),
        ("ORDO_AUTH_METHODS", "passkey,email".into()),
        ("ORDO_APEX_DOMAIN", "localhost".into()),
        ("ORDO_AUTH_ORIGINS", PAGE.into()),
        ("ORDO_CORS_ORIGINS", PAGE.into()),
        ("ORDO_AUTH_PROVISIONING", "auto-onboard".into()),
        ("ORDO_PROVISION_SELF_SERVE", "on-signup".into()),
        ("ORDO_PROVISION_SIGNUPS_PER_HOUR", "10000".into()),
        ("ORDO_AUTH_GUEST", "on".into()),
        ("ORDO_MAIL_KIND", "smtp".into()),
        ("ORDO_MAIL_FROM", "notes@example.com".into()),
        ("ORDO_MAIL_HOST", "localhost".into()),
        ("ORDO_MAIL_PORT", relay.port.to_string()),
        ("ORDO_MAIL_TLS", "none".into()),
        ("ORDO_MAIL_SEND_SECS", "1".into()),
        ("ORDO_EGRESS_PRIVATE_HOSTS", "localhost".into()),
        // Sign-in limits high enough that the load is not refused for rate.
        ("ORDO_AUTH_EMAIL_PER_HOUR", "1000".into()),
        ("ORDO_AUTH_EMAIL_PER_DAY", "10000".into()),
        ("ORDO_AUTH_EMAIL_DEPLOYMENT_PER_HOUR", "1000000".into()),
        (
            "ORDO_WORKER_AUTH_CEREMONY_SWEEP_TICK_MS",
            knob("STRESS_SWEEP_MS", 500u64).to_string(),
        ),
        (
            "ORDO_ACCESS_REFRESH_SECS",
            knob("STRESS_REFRESH_SECS", 5u64).to_string(),
        ),
    ];
    for (k, v) in &settings {
        std::env::set_var(k, v);
    }
    for k in [
        "ORDO_PG_ROUTING",
        "ORDO_TENANT_CREDENTIALS",
        "ORDO_TENANT_CREDENTIALS_DIR",
        "ORDO_CHECKPOINT_KEY",
        "ORDO_SECURITY_CHECKPOINT_KEY",
        "ORDO_EXIT_BUNDLE_DIR",
    ] {
        std::env::remove_var(k);
    }

    let serving = door_rt.spawn(async {
        let cfg = ordo_host::runtime::RuntimeConfig::from_env().expect("the settings read");
        let rt = ordo_host::runtime::assemble(cfg)
            .await
            .expect("the deployment assembles");
        ordo_host::runtime::serve(rt).await
    });
    let up = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            up.elapsed() < Duration::from_secs(60),
            "the door never bound {port}"
        );
        assert!(!serving.is_finished(), "the door stopped before serving");
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!("stress: the door serves on {port} after {:?}", up.elapsed());

    let counts = Arc::new(Counts::default());
    let next = Arc::new(AtomicUsize::new(0));
    let signed_up: Arc<Mutex<Vec<(String, usize, bool)>>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let clients: Vec<_> = (0..concurrency)
        .map(|c| {
            let (counts, next, signed_up, stop, relay) = (
                counts.clone(),
                next.clone(),
                signed_up.clone(),
                stop.clone(),
                relay.clone(),
            );
            std::thread::Builder::new()
                .name(format!("stress-client-{c}"))
                .spawn(move || {
                    let mut turn = c;
                    while !stop.load(Ordering::Relaxed) && started.elapsed() < bound {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i < people {
                            let address = format!("p{i}@example.org");
                            one_person(port, &relay, &counts, &address, 0, refreshes);
                            signed_up.lock().unwrap().push((address, 1, false));
                        } else {
                            // Everyone has signed up: sign people in again
                            // until the bound, as returning visitors do.
                            // One sign-in per address at a time, so each
                            // code read from the relay is that sign-in's.
                            let pick = {
                                let mut all = signed_up.lock().unwrap();
                                let len = all.len();
                                (0..len)
                                    .map(|k| (turn + k) % len)
                                    .find(|&k| !all[k].2)
                                    .map(|k| {
                                        turn = k + 1;
                                        let (a, n, busy) = &mut all[k];
                                        *busy = true;
                                        *n += 1;
                                        (k, a.clone(), *n - 1)
                                    })
                            };
                            match pick {
                                Some((k, address, n)) => {
                                    one_person(port, &relay, &counts, &address, n, refreshes);
                                    signed_up.lock().unwrap()[k].2 = false;
                                }
                                None => std::thread::sleep(Duration::from_millis(100)),
                            }
                            if i >= people + knob("STRESS_SIGNINS", 1_000_000usize) {
                                break;
                            }
                        }
                    }
                })
                .expect("client thread")
        })
        .collect();

    // Progress, so a stuck run says where it stuck. When no sign-in has
    // finished for 8 s, or a code was never mailed, the run is stalled: it says once what Postgres,
    // the relay, the outbox and every thread of this process are doing.
    let mut last = (0, Instant::now());
    let mut dumped = false;
    while clients.iter().any(|c| !c.is_finished()) {
        let done = counts.signups.load(Ordering::Relaxed) + counts.signins.load(Ordering::Relaxed);
        if done != last.0 {
            last = (done, Instant::now());
        }
        let no_mail = counts.no_mail.load(Ordering::Relaxed) > 0;
        if !dumped && (no_mail || last.1.elapsed() > Duration::from_secs(8)) {
            dumped = true;
            stalled(&db, &relay);
        }
        if started.elapsed() > bound + Duration::from_secs(60) {
            stop.store(true, Ordering::Relaxed);
            eprintln!("stress: past the bound; stopping the clients");
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
        if !started.elapsed().as_secs().is_multiple_of(5) {
            continue;
        }
        eprintln!(
            "stress: {:>4}s {}",
            started.elapsed().as_secs(),
            counts.line()
        );
    }
    stop.store(true, Ordering::Relaxed);
    for c in clients {
        let _ = c.join();
    }
    let summary = counts.line();
    eprintln!("stress: done in {}s {summary}", started.elapsed().as_secs());

    serving.abort();
    door_rt.shutdown_timeout(Duration::from_secs(10));
    relay_rt.shutdown_timeout(Duration::from_secs(2));
    if std::env::var("STRESS_KEEP_DB").is_err() {
        drop_leftovers(&db);
    }

    assert_eq!(
        counts.busy.load(Ordering::Relaxed),
        0,
        "emailed sign-ins were answered server_busy: {summary}"
    );
    assert_eq!(
        counts.no_mail.load(Ordering::Relaxed),
        0,
        "codes were never mailed: {summary}"
    );
    assert_eq!(
        counts.timeouts.load(Ordering::Relaxed),
        0,
        "requests went unanswered: {summary}"
    );
}
