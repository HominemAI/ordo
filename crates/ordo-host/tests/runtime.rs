//! R1 — the `ordo-host` binary, run as a process.
//!
//! Every assertion here is about the **binary**, not about the library: the
//! suite spawns `target/debug/ordo-host` with an environment and reads what
//! it did. That is the only way to test the two properties R1 exists for —
//! that a missing secret is a non-zero exit *before any dial*, and that a
//! `SIGTERM` under load finishes what is in flight and says what the
//! security drain flushed.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_crypto::{KeyProvider as _, SpaceId};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ordo_conformance::{Fixture, Pg};
use ordo_exec::{Pool, PoolConfig};

const SPACE: &str = "acme";
const MASTER: &str = "an-operator-master-key-for-r1-tests";
const TOKEN_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PAGE_KEY: &str = "2222222222222222222222222222222222222222222222222222222222222222";

/// The **test's own** pool, beside the one the spawned server holds.
///
/// 2 rather than 4 for the child's reason: nothing in this file exercises
/// pool concurrency, so every connection above the floor is one this suite
/// holds for no property it is testing. Between this and
/// `ORDO_PG_POOL_SIZE=2` on the child, the suite's peak fell from 99
/// client backends to comfortably inside Postgres's stock limit.
fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    }
}

/// A free port for one spawned server, never handed out twice by this
/// process.
///
/// The probe listener closes before the child binds, so the kernel may
/// give the same number to a later `bind(:0)`. macOS counts up and does
/// not; Linux starts each search at a random port, so on CI two of this
/// file's ~45 servers, booting in parallel, could be handed the same port.
/// One server then failed to bind, while `wait_for_live` saw the other's
/// socket open and the test's requests reached a deployment that was not
/// its own. CI for 782a8b86: an upload answered with an empty 404, which is
/// axum's answer for a route that is not mounted, from a server booted
/// without a blob store. The window is not microseconds either:
/// `a_program_with_a_stored_blob_field_needs_a_store` holds its port across
/// two refused boots before it binds.
///
/// Every test binds its children through this, so remembering every port
/// handed out closes the race inside this file's process, and cargo runs
/// test binaries one at a time.
fn free_port() -> u16 {
    static HANDED: std::sync::Mutex<std::collections::BTreeSet<u16>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    for _ in 0..1000 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = l.local_addr().expect("addr").port();
        if HANDED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(port)
        {
            return port;
        }
    }
    panic!("no port this process has not handed out already in 1000 tries");
}

/// A deployment an operator has already `apply`d and `keys init`ed: the
/// shipped fixture's schema, its seed under `acme`, an active artifact in
/// the registry, and custody in `$keycustody`.
async fn deployment_ready(db: &str) -> Pool {
    deployment_of(
        db,
        include_str!("../../ordo-conformance/fixtures/market.ordo"),
    )
    .await
}

/// The same, over a program the caller composed.
async fn deployment_of(db: &str, source: &str) -> Pool {
    deployment_under(
        db,
        source,
        ordo_crypto::master::derive_from_secret(MASTER.as_bytes()),
    )
    .await
}

/// The same, under a master key the caller holds — a sealed file's, for
/// instance. `from_master`, never `new`: the caller's 32 bytes are already
/// the master.
async fn deployment_under(db: &str, source: &str, master: [u8; 32]) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };

    // **The schema first.** `Fixture::reset` opens with
    // `DROP SCHEMA public CASCADE`, so anything created before it — the
    // custody table above all — goes with it. Schema, then custody, then
    // seed is also the order an operator uses: `ordo apply` after
    // `ordo keys init` would make exactly this mistake.
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("clean schema");
    let plain = Fixture::from_source(source, "").expect("the program compiles");
    pg.exec(&plain.ddl).expect("ddl");
    pg.exec(ordo_crypto::SEALED_PAYLOAD_DDL)
        .expect("sealed payloads");

    let pool = Pool::connect(cfg(db)).await.expect("pool");
    ordo_exec::keystore::ensure_tables(&pool)
        .await
        .expect("custody table");
    let provider = ordo_crypto::LocalKeyProvider::from_master(
        master,
        Box::new(ordo_exec::keystore::PgKeyStore::open(cfg(db)).expect("store")),
    );
    provider
        .create_space(&ordo_crypto::SpaceId(SPACE.into()))
        .expect("custody minted");
    let keys: Arc<dyn ordo_crypto::KeyProvider> = Arc::new(provider);

    // The seed, under the SAME custody the binary will boot with: the
    // seed's blind-index digests are `HMAC(BIK(space), value)` and BIK is
    // SMK-derived, so a database seeded under one provider and served
    // under another has digests nothing can address.
    let mut f = Fixture::load().expect("fixture");
    f.keys = keys.clone();
    let f = f.in_space(SPACE);
    f.seed_into(&pg).expect("seed");
    // The program under test, compiled with the same custody so its
    // artifact and its DDL are the ones the binary will load.
    let mut prog = Fixture::from_source(source, "").expect("the program compiles");
    prog.keys = keys.clone();
    let prog = prog.in_space(SPACE);

    // The registry, the way `ordo apply` fills it.
    ordo_exec::registry::ensure_tables(&pool)
        .await
        .expect("registry");
    let doc =
        ordo_plan::artifact::program_artifact(&prog.schema, &prog.registry).expect("artifact");
    let ddl = prog.ddl.clone();
    let art = ordo_exec::registry::Artifact {
        version: ordo_plan::artifact::program_version(&doc, &ddl),
        artifact: ordo_plan::artifact::to_bytes(&doc),
        source: source.to_string(),
        ddl,
        applied_at: ordo_commandlog::Timestamp(1),
        actor: "operator:r1".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:r1",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");
    pool
}

/// The binary, with a complete environment. Callers strip what they want
/// missing.
fn host(db: &str, port: u16) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_ordo-host"));
    let base = PoolConfig::from_env();
    // **Each spawned server takes two connections, not eight.**
    //
    // Without this the child sizes its pool the way a production
    // deployment does — `min(available_parallelism, 8)` — and this file
    // runs ~22 tests in parallel, each standing up a real deployment. The
    // measured peak was **99 client backends** against Postgres's default
    // `max_connections = 100`, which is why this suite failed a different
    // subset almost every run and always passed under `--test-threads=1`.
    //
    // **That was never a property worth testing.** Nothing here exercises
    // pool concurrency; the connections were an accident of every test
    // booting a production-shaped deployment. Sizing them down is the fix,
    // and raising the server's limit was treating the symptom — it would
    // have left the same accident in place and moved the ceiling.
    //
    // 2 is `PoolConfig`'s own floor, and the reserve check at boot refuses
    // a reserve that is not smaller than the pool, so this is the smallest
    // value the deployment will accept.
    c.env("ORDO_PG_POOL_SIZE", "2")
        .env("ORDO_PG_HOST", &base.host)
        .env("ORDO_PG_PORT", base.port.to_string())
        .env("ORDO_PG_USER", &base.user)
        .env("ORDO_PG_DB", db)
        .env("ORDO_SPACE", SPACE)
        .env("ORDO_KEY_PROVIDER", "local")
        .env("ORDO_MASTER_KEY", MASTER)
        .env("ORDO_TOKEN_KEY", TOKEN_KEY)
        .env("ORDO_PAGE_KEY", PAGE_KEY)
        .env("ORDO_HOST_ADDR", format!("127.0.0.1:{port}"))
        // `local` is development only (S6-009), so a test that boots one
        // says so. The refusal tests below remove this again.
        .env("ORDO_DEV_UNSAFE_TEST_KEYS", "1")
        .env_remove("ORDO_CHECKPOINT_KEY")
        .env_remove("ORDO_SECURITY_CHECKPOINT_KEY")
        .env_remove("ORDO_EXIT_BUNDLE_DIR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
    elapsed: Duration,
}

fn run_to_completion(mut c: Command) -> Ran {
    let started = Instant::now();
    let out = c.output().expect("the binary runs");
    Ran {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        elapsed: started.elapsed(),
    }
}

/// One HTTP/1.1 request over a raw socket. No client dependency is added
/// for six lines, and a raw socket is also the honest test: what a probe
/// sends is bytes.
fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    // The server may still be binding when the first request goes out (seen
    // on CI under load, 2026-10-31), so connecting retries until a deadline.
    // A server that never listens still fails, once the 15 seconds are up.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut s = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(e)
                if e.kind() == std::io::ErrorKind::ConnectionRefused
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("connect: {e:?}"),
        }
    };
    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
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
    (status, raw)
}

/// The JSON body out of a raw response, or `Null` when there is none.
/// The headers end at the blank line; everything after it is the body,
/// because every route this suite reads answers without chunking.
fn json_body(raw: &str) -> serde_json::Value {
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .and_then(|body| serde_json::from_str(body.trim()).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Wait for the door to answer, or give up loudly. Thirty seconds: a
/// sealed master file runs its key derivation before binding, and under a
/// loaded machine (a full workspace run beside other builds) that alone
/// passed ten.
fn wait_for_live(port: u16, child: &mut Child) {
    for _ in 0..600 {
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            let mut out = String::new();
            if let Some(mut o) = child.stdout.take() {
                let _ = o.read_to_string(&mut out);
            }
            panic!("the binary exited before serving: {status}\n{out}\n{err}");
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    panic!("the binary never bound {port} in 30 s\n{err}");
}

fn kill(child: &mut Child, signal: &str) {
    let _ = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(child.id().to_string())
        .status();
}

// ── refusals, and that they precede the dial ─────────────────────────────

/// Every missing secret is a non-zero exit naming the variable — and it
/// happens **before any dial**, which the unroutable database proves: a
/// binary that dialled first would sit in a connect timeout instead of
/// answering in milliseconds.
#[test]
fn a_missing_secret_is_a_refusal_before_the_binary_dials() {
    let port = free_port();
    for (var, needle) in [
        ("ORDO_MASTER_KEY", "ORDO_MASTER_KEY"),
        ("ORDO_TOKEN_KEY", "ORDO_TOKEN_KEY"),
        ("ORDO_PAGE_KEY", "ORDO_PAGE_KEY"),
        ("ORDO_SPACE", "ORDO_SPACE"),
        ("ORDO_PG_HOST", "ORDO_PG_HOST"),
        ("ORDO_PG_USER", "ORDO_PG_USER"),
        ("ORDO_PG_DB", "ORDO_PG_DB"),
        ("ORDO_KEY_PROVIDER", "ORDO_KEY_PROVIDER"),
    ] {
        let mut c = host("ordo_rt_host_never_dialled", port);
        // A database that cannot be reached at all. Every refusal below
        // must arrive without noticing.
        c.env("ORDO_PG_HOST", "203.0.113.1")
            .env("ORDO_PG_PORT", "5432");
        if var == "ORDO_PG_HOST" {
            c.env_remove("ORDO_PG_HOST");
        }
        c.env_remove(var);
        let ran = run_to_completion(c);
        assert_ne!(
            ran.code, 0,
            "missing {var} was not a refusal:\n{}",
            ran.stdout
        );
        assert!(
            ran.stderr.contains(needle),
            "the refusal for {var} does not name it: {}",
            ran.stderr
        );
        assert!(
            ran.elapsed < Duration::from_secs(3),
            "missing {var} took {:?} — the binary dialled before it refused",
            ran.elapsed
        );
    }
}

/// G17 (`plan-console.md` 7): `ordo-host check-config` reads every setting
/// and refuses as boot would, then exits without dialling or serving. The
/// console runs it before every apply. The unroutable database proves it
/// never dials: a dial would sit in a connect timeout.
#[test]
fn check_config_refuses_as_boot_would_and_never_dials() {
    let port = free_port();
    let mut good = host("ordo_rt_host_never_dialled", port);
    good.arg("check-config")
        .env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432");
    let ran = run_to_completion(good);
    assert_eq!(
        ran.code, 0,
        "check-config refused a bootable set: {}",
        ran.stderr
    );
    assert!(ran.stdout.contains("would boot"), "{}", ran.stdout);
    assert!(
        ran.elapsed < Duration::from_secs(3),
        "check-config took {:?}; it dialled",
        ran.elapsed
    );

    let mut bad = host("ordo_rt_host_never_dialled", port);
    bad.arg("check-config")
        .env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432")
        .env_remove("ORDO_TOKEN_KEY");
    let ran = run_to_completion(bad);
    assert_ne!(
        ran.code, 0,
        "check-config accepted a missing ORDO_TOKEN_KEY"
    );
    assert!(
        ran.stderr.contains("ORDO_TOKEN_KEY"),
        "the refusal does not name the setting: {}",
        ran.stderr
    );
}

/// Console step 2b: the console's supervisor gives every secret setting as
/// `<NAME>_FILE`, never as a value. `check-config` reads the same settings
/// as boot, so a set given wholly in file form passes it, and an
/// unreadable file or both forms refuse by the variable's name.
#[test]
fn every_secret_setting_is_read_from_its_file_form() {
    let dir = std::env::temp_dir().join(format!("ordo-rt-secret-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let file = |name: &str, value: &str| {
        let p = dir.join(name);
        std::fs::write(&p, format!("{value}\n")).expect("write");
        p.display().to_string()
    };
    let token = file("token", TOKEN_KEY);
    let page = file("page", PAGE_KEY);
    let master = file("master", MASTER);
    let password = file("password", "not-checked-without-a-dial");
    let port = free_port();
    let by_file = || {
        let mut c = host("ordo_rt_host_never_dialled", port);
        c.arg("check-config")
            .env("ORDO_PG_HOST", "203.0.113.1")
            .env_remove("ORDO_TOKEN_KEY")
            .env_remove("ORDO_PAGE_KEY")
            .env_remove("ORDO_MASTER_KEY")
            .env_remove("ORDO_PG_PASSWORD")
            .env("ORDO_TOKEN_KEY_FILE", &token)
            .env("ORDO_PAGE_KEY_FILE", &page)
            .env("ORDO_MASTER_KEY_FILE", &master)
            .env("ORDO_PG_PASSWORD_FILE", &password);
        c
    };
    let ran = run_to_completion(by_file());
    assert_eq!(ran.code, 0, "the file forms were refused: {}", ran.stderr);

    let mut gone = by_file();
    gone.env(
        "ORDO_PAGE_KEY_FILE",
        dir.join("missing").display().to_string(),
    );
    let ran = run_to_completion(gone);
    assert_ne!(ran.code, 0, "an unreadable ORDO_PAGE_KEY_FILE was accepted");
    assert!(ran.stderr.contains("ORDO_PAGE_KEY_FILE"), "{}", ran.stderr);

    let mut both = by_file();
    both.env("ORDO_TOKEN_KEY", TOKEN_KEY);
    let ran = run_to_completion(both);
    assert_ne!(
        ran.code, 0,
        "ORDO_TOKEN_KEY and its file were both accepted"
    );
    assert!(ran.stderr.contains("ORDO_TOKEN_KEY_FILE"), "{}", ran.stderr);

    let mut pg = by_file();
    pg.env(
        "ORDO_PG_PASSWORD_FILE",
        dir.join("missing").display().to_string(),
    );
    let ran = run_to_completion(pg);
    assert_ne!(
        ran.code, 0,
        "an unreadable ORDO_PG_PASSWORD_FILE was accepted"
    );
    assert!(
        ran.stderr.contains("ORDO_PG_PASSWORD_FILE"),
        "{}",
        ran.stderr
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// S6-009, ruled 2026-09-28: `ORDO_KEY_PROVIDER=local` is refused outside
/// development. The refusal names both variables and points at the sealed
/// file, and it lands before the dial. With the development flag the same
/// configuration gets past custody, so the flag is proven to mean something.
#[test]
fn the_local_provider_is_refused_outside_development() {
    let port = free_port();
    let mut c = host("ordo_rt_host_never_dialled", port);
    c.env_remove("ORDO_DEV_UNSAFE_TEST_KEYS")
        .env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432");
    let ran = run_to_completion(c);
    assert_ne!(ran.code, 0, "the local provider was accepted");
    for needle in [
        "ORDO_KEY_PROVIDER",
        "ORDO_MASTER_KEY",
        "sealed-master",
        "ORDO_MASTER_KEY_PASSPHRASE_FILE",
    ] {
        assert!(
            ran.stderr.contains(needle),
            "the refusal does not mention {needle}: {}",
            ran.stderr
        );
    }
    assert!(ran.elapsed < Duration::from_secs(3), "it dialled first");

    let mut c = host("ordo_rt_host_never_dialled", port);
    c.env("ORDO_DEV_UNSAFE_TEST_KEYS", "1")
        .env("ORDO_PG_HOST", "127.0.0.1")
        .env("ORDO_PG_PORT", "1");
    let ran = run_to_completion(c);
    assert!(
        !ran.stderr.contains("sealed-master"),
        "the development flag did not admit local: {}",
        ran.stderr
    );
    assert!(
        ran.stderr.contains("database"),
        "expected the dial to be the next failure: {}",
        ran.stderr
    );
}

/// `ORDO_KEY_PROVIDER=test` is a refusal without the dev flag, and the
/// refusal names the provider. This is the check that stops a deployment
/// shipping under a master key published in this repository.
#[test]
fn the_test_provider_is_refused_without_the_dev_flag() {
    let port = free_port();
    let mut c = host("ordo_rt_host_never_dialled", port);
    c.env("ORDO_KEY_PROVIDER", "test")
        .env_remove("ORDO_DEV_UNSAFE_TEST_KEYS")
        .env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432");
    let ran = run_to_completion(c);
    assert_ne!(ran.code, 0, "the test provider was accepted");
    assert!(
        ran.stderr.contains("ORDO_KEY_PROVIDER") && ran.stderr.contains("repository"),
        "the refusal does not say why: {}",
        ran.stderr
    );
    assert!(ran.elapsed < Duration::from_secs(3), "it dialled first");

    // And it IS selectable with the opt-in — an opt-in nothing accepts is
    // not an opt-in, and the whole posture depends on the flag meaning
    // something.
    //
    // A *refused* connection rather than the black hole above: this half
    // is about getting past the custody check, and waiting out a TCP
    // timeout to prove it would add a minute to the suite for nothing.
    let mut c = host("ordo_rt_host_never_dialled", port);
    c.env("ORDO_KEY_PROVIDER", "test")
        .env("ORDO_DEV_UNSAFE_TEST_KEYS", "1")
        .env("ORDO_PG_HOST", "127.0.0.1")
        .env("ORDO_PG_PORT", "1");
    let ran = run_to_completion(c);
    assert!(
        !ran.stderr.contains("ORDO_KEY_PROVIDER"),
        "the flag did not select the provider: {}",
        ran.stderr
    );
    // It still fails — at the database, which is the next step and the
    // proof the custody check passed.
    assert!(
        ran.stderr.contains("database"),
        "expected the dial to be the next failure: {}",
        ran.stderr
    );
}

/// **An unprovisioned tenant closes one space and does not stop the
/// deployment** (17ac).
///
/// Until 17ac this was a boot refusal: `ORDO_SPACE` naming a tenant with
/// no custody exited before the listener bound. That was the last refusal
/// of the shape 17aa removed everywhere else, one space's state deciding
/// whether a whole process starts, and its own message named the way round
/// it (point `ORDO_SPACE` at the system space), so it was already
/// optional in practice and mandatory in code.
///
/// What it was protecting is asserted below and was never the refusal's to
/// hold: `current_epoch` reads and never creates, so a typo mints nothing.
/// What changes is that the typo is now one quarantined space with the
/// remedy as its reason, on a deployment that is up and answering.
#[tokio::test(flavor = "multi_thread")]
async fn an_unprovisioned_tenant_is_quarantined_and_the_deployment_serves() {
    const DB: &str = "ordo_rt_host_unprovisioned";
    const MISTYPED: &str = "a-space-nobody-provisioned";
    let pool = deployment_ready(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_SPACE", MISTYPED);
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    // Anonymous: the status alone (audit F12). The space nobody created
    // has no owner to ask with, and the detail (`quarantined`, with the
    // remedy in its reason) is read as the owner in `tests/spaces.rs`.
    let (status, raw) = request(port, "GET", "/health/ready", &[], "");
    assert_eq!(
        status, 200,
        "a tenant nobody created is not an outage: {raw}"
    );
    let ready = json_body(&raw);
    assert_eq!(ready, serde_json::json!({ "status": "ready" }), "{raw}");

    // And it minted nothing for the name that was mistyped, which is the
    // property `build_custody` has always had and 17ac must not spend.
    let rows = pool
        .rows(&format!(
            r#"SELECT count(*) FROM "$keycustody" WHERE "space" = '{MISTYPED}'"#
        ))
        .await
        .expect("count");
    assert_eq!(
        rows[0][0].as_deref(),
        Some("0"),
        "a typo in ORDO_SPACE minted custody for it"
    );

    kill(&mut child, "TERM");
    let _ = child.wait_with_output().expect("wait");
}

// ── the running deployment ───────────────────────────────────────────────

/// The whole of R1's done-when that needs a live process: it boots against
/// a service Postgres, the three health surfaces answer, the startup log
/// names what is disabled, and `SIGTERM` finishes in flight work and
/// reports what the drain flushed.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_boots_serves_and_drains_on_sigterm() {
    const DB: &str = "ordo_rt_host_boot";
    let _pool = deployment_ready(DB).await;
    let port = free_port();
    let mut child = host(DB, port).spawn().expect("spawn");
    wait_for_live(port, &mut child);

    // Liveness is open — a probe that needs a credential fails during
    // exactly the incident it exists for.
    let (status, _) = request(port, "GET", "/health/live", &[], "");
    assert_eq!(status, 200, "/health/live");

    // Deep health is owner-gated, and an unauthenticated caller is
    // refused rather than told what exists.
    let (status, _) = request(port, "GET", "/health/deep", &[], "");
    assert_eq!(status, 403, "/health/deep unauthenticated");

    // Readiness answers about this deployment. 200 or 503; both are
    // honest, and which one it is depends on the gate and the pool.
    let (status, ready_body) = ready_as_owner(port);
    assert!(
        status == 200 || status == 503,
        "/health/ready answered {status}: {ready_body}"
    );

    // A write through the door of the running binary, sealing a field.
    let (status, body) = request(
        port,
        "POST",
        "/op/setMemo",
        &[
            ("x-ordo-space", SPACE),
            ("authorization", &format!("Bearer {}", token())),
            ("content-type", "application/json"),
        ],
        &serde_json::json!({ "order": "ord_1", "memo": "through-the-binary" }).to_string(),
    );
    assert_eq!(status, 200, "the door refused a write: {body}");

    // …and it sealed under the operator's custody, not the repository's
    // constant. The binary is a separate process holding its own provider,
    // so this is the end-to-end form of R2a's assertion.
    let pg = Pg {
        db: DB.into(),
        ..Pg::default()
    };
    let rows = pg
        .query(r#"SELECT encode("memo", 'hex') FROM "orders" WHERE "id" = 'ord_1'"#)
        .expect("row");
    let sealed = unhex(rows[0][0].as_deref().expect("sealed memo"));
    let provider = ordo_crypto::LocalKeyProvider::new(
        MASTER.as_bytes(),
        Box::new(ordo_exec::keystore::PgKeyStore::open(cfg(DB)).expect("store")),
    );
    let space = ordo_crypto::SpaceId(SPACE.into());
    let lease =
        ordo_crypto::KeyProvider::release(&provider, &space, ordo_crypto::Purpose::Read, "r1")
            .expect("lease");
    assert_eq!(
        ordo_crypto::KeyProvider::open_at(
            &provider,
            &lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("ord_1"),
            &sealed
        )
        .expect("the operator's custody opens what the binary sealed"),
        b"through-the-binary"
    );

    // SIGTERM: the listener closes, the drain flushes, and the process
    // exits zero.
    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        out.status.code(),
        Some(0),
        "SIGTERM did not exit 0:\n{stdout}"
    );

    // The startup block, printed once, before the listener bound.
    assert!(
        stdout.starts_with("ordo-host "),
        "no startup block:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("space={SPACE}")),
        "the log does not name the space:\n{stdout}"
    );
    assert!(
        stdout.contains("custody     local"),
        "the log does not name the custody posture:\n{stdout}"
    );
    // A feature that is off and unmentioned is a feature an operator
    // believes is on. Two are off in this environment.
    for feature in ["checkpoint", "attest"] {
        assert!(
            stdout.contains(&format!("DISABLED    {feature}")),
            "`{feature}` is off and the log does not say so:\n{stdout}"
        );
    }
    // `ORDO_SECURITY_CHECKPOINT_KEY` is unset, so the boot made one
    // (`plan-identity-service.md` section 7) and security retention runs.
    assert!(
        !stdout.contains("DISABLED    secevent.retain"),
        "a security checkpoint key was made at first boot:\n{stdout}"
    );
    // Obligation 8 read the other way round: **every** worker the
    // supervisor knows about is in the census, either as running or as
    // DISABLED with a reason. A name in neither list is the one state an
    // operator cannot see. This program declares no listener, so the census
    // here is mostly the disabled half, and that is exactly the shape that
    // hid `provisioning.reconciler`: its registration sat inside the
    // `listeners.is_empty()` else-branch, so a deployment with no trigger
    // listener (which is every deployment until 12a) ran no reconciler and
    // said nothing about it. A stuck `provisioning` tenant then stayed
    // invisible instead of turning `failed` and joining `tenants::troubled`.
    // `secevent.drain` is in the list too. It is an `External` rather than a
    // `Worker` (the drain keeps its own start, flush and failure
    // guarantees), and a loop this process runs belongs in the census of the
    // loops this process runs, on the same terms `/health/deep` states it:
    // `supervised: false`.
    for named in ["provisioning.reconciler", "keys.reseal", "secevent.drain"] {
        assert!(
            stdout.contains(&format!("worker      {named}")),
            "`{named}` is not in the running census:\n{stdout}"
        );
    }

    // No secret, no derived secret, no length of a secret.
    for secret in [MASTER, TOKEN_KEY, PAGE_KEY] {
        assert!(
            !stdout.contains(secret),
            "a secret reached the log:\n{stdout}"
        );
    }
    // §1.5 step 3, and the whole of obligation 4: the workers are cancelled
    // and every final act runs. This is asserted through the log because
    // the log is the only thing a spawned process tells us, and it is
    // asserted at all because the failure is silent by construction: the
    // supervisor is held behind an `Arc` that `PlatformState` also holds,
    // so a shutdown written as `Arc::into_inner(..)` takes the `None` arm,
    // runs nothing, prints nothing, and exits 0. M1's partial usage period
    // is what goes missing, and nothing else in the tree would notice.
    // One `find_map` over the whole shape, not a `find_map` for the prefix
    // and an `and_then` for the rest: `shutdown: SIGTERM` is also a
    // `shutdown: ` line and matches first, so the split form reported the
    // absence of a number on a line that never carried one.
    let acts = stdout.lines().find_map(|l| {
        l.strip_prefix("shutdown: ")?
            .strip_suffix(" worker final act(s) ran")?
            .parse::<usize>()
            .ok()
    });
    assert!(
        acts.is_some_and(|n| n > 0),
        "the shutdown ran no worker final acts (obligation 4):\n{stdout}"
    );
    // And it ran them **before** the drain flushed, because the drain is
    // about to write the record a final act contributes to. Ordering, not
    // just presence: both lines exist in the wrong order too.
    let act_at = stdout
        .find("worker final act(s) ran")
        .expect("the final-act line");
    let flush_at = stdout.find("security drain flushed").unwrap_or(usize::MAX);
    assert!(
        act_at < flush_at,
        "the final acts ran after the drain flushed, so what they wrote is \
         not in the record:\n{stdout}"
    );

    // What the drain flushed is a number in the log, because a bounded,
    // known loss of audit record belongs in the log rather than in
    // nobody's memory.
    assert!(
        stdout.contains("security drain flushed"),
        "the shutdown did not report the drain:\n{stdout}"
    );
}

/// A database nobody has run `ordo apply` against is not a boot failure,
/// and — owner-ruled 2026-09-20, 17t — it is not a readiness failure
/// either: the process starts, liveness answers, and readiness answers
/// ready, because a platform whose job is to *receive* deployments is
/// ready by definition before it has one. The body says so honestly in
/// `program_active` while the node waits for a deploy.
///
/// Before 17t this asserted the opposite (503, naming `registry` and
/// `shape`) — see `crates/ordo-host/tests/health.rs` for that regression
/// pinned directly against the handler, without the cost of a spawned
/// binary.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_with_no_program_boots_and_is_ready() {
    const DB: &str = "ordo_rt_host_noprogram";
    // Custody, and deliberately no registry at all: a database `ordo apply`
    // has never touched. The binary creates the registry's tables itself
    // (found by the image smoke on 2026-09-19, which died on
    // `relation "$activations" does not exist`).
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {DB}"));
    let pool = Pool::connect(cfg(DB)).await.expect("pool");
    ordo_exec::keystore::ensure_tables(&pool)
        .await
        .expect("custody");
    pool.rows(r#"DROP TABLE IF EXISTS "$activations", "$artifacts", "$artifactpointer""#)
        .await
        .expect("a database with no registry");
    ordo_crypto::LocalKeyProvider::new(
        MASTER.as_bytes(),
        Box::new(ordo_exec::keystore::PgKeyStore::open(cfg(DB)).expect("store")),
    )
    .create_space(&ordo_crypto::SpaceId(SPACE.into()))
    .expect("custody minted");

    let port = free_port();
    let mut child = host(DB, port).spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let (status, _) = request(port, "GET", "/health/live", &[], "");
    assert_eq!(status, 200, "a no-program node is still alive");
    // Anonymous: the status alone (audit F12). That no program is active
    // (`program_active`) is the owner's detail, pinned in `tests/health.rs`.
    let (status, body) = request(port, "GET", "/health/ready", &[], "");
    assert_eq!(
        status, 200,
        "a correctly configured no-program node is ready: {body}"
    );
    assert!(
        body.contains(r#""status":"ready""#),
        "readiness must say ready: {body}"
    );

    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("program     (none)"),
        "the log does not say there is no program:\n{stdout}"
    );
}

// ── R2b: the sealed-master source ────────────────────────────────────────

/// **R2b's done-when.** The binary boots under
/// `ORDO_KEY_PROVIDER=sealed-master` from a file the ceremony wrote, seals
/// a real value through the door and reads it back — and a wrong passphrase
/// is a refusal that says nothing about which byte was wrong.
///
/// The master key never appears in this process's environment, which is the
/// entire point: `/proc/<pid>/environ` and `docker inspect` both print
/// `ORDO_MASTER_KEY`, and neither prints a file the process opened.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_boots_under_a_sealed_master_file() {
    const DB: &str = "ordo_rt_host_sealed";
    let dir = std::env::temp_dir().join(format!("ordo-rt-sealed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp");
    let key_file = dir.join("master.key");
    let pass_file = dir.join("pass");
    let _ = std::fs::remove_file(&key_file);
    std::fs::write(&pass_file, "a passphrase the environment never sees").expect("write");

    // The ceremony, through `ordo-crypto` directly: `tests/keys.rs` proves
    // the command, and this suite is about the binary.
    let fp = ordo_crypto::master::seal_master(
        &key_file,
        "r1-sealed",
        b"a passphrase the environment never sees",
    )
    .expect("sealed");
    let master = ordo_crypto::master::open_sealed_master(
        &key_file,
        b"a passphrase the environment never sees",
    )
    .expect("opens");

    // A deployment provisioned under that master.
    let source = include_str!("../../ordo-conformance/fixtures/market.ordo");
    deployment_under(DB, source, master).await;

    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_KEY_PROVIDER", "sealed-master")
        // No development flag: the sealed file is the production path and
        // must boot without one.
        .env_remove("ORDO_DEV_UNSAFE_TEST_KEYS")
        .env("ORDO_MASTER_KEY_FILE", &key_file)
        .env("ORDO_MASTER_KEY_PASSPHRASE_FILE", &pass_file)
        // The master is NOT in the environment. Removing it here is the
        // assertion: if the binary fell back to it, the boot below would
        // pass for the wrong reason.
        .env_remove("ORDO_MASTER_KEY");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let (status, body) = request(
        port,
        "POST",
        "/op/setMemo",
        &[
            ("x-ordo-space", SPACE),
            ("authorization", &format!("Bearer {}", token())),
            ("content-type", "application/json"),
        ],
        &serde_json::json!({ "order": "ord_1", "memo": "sealed-master" }).to_string(),
    );
    assert_eq!(status, 200, "the door refused a write: {body}");

    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("custody     sealed-master (file)"),
        "the log must name the posture:\n{stdout}"
    );
    // No secret, no derived secret, and no fingerprint-shaped leak of one.
    assert!(!stdout.contains("passphrase the environment"), "{stdout}");
    assert!(!stdout.contains(&fp), "{stdout}");

    // A wrong passphrase: refused, before any dial, with `Seal`'s
    // deliberately unexplanatory text.
    let wrong = dir.join("wrong-pass");
    std::fs::write(&wrong, "not that passphrase").expect("write");
    let mut c = host(DB, port);
    c.env("ORDO_KEY_PROVIDER", "sealed-master")
        .env("ORDO_MASTER_KEY_FILE", &key_file)
        .env("ORDO_MASTER_KEY_PASSPHRASE_FILE", &wrong)
        .env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432")
        .env_remove("ORDO_MASTER_KEY");
    let ran = run_to_completion(c);
    assert_ne!(ran.code, 0, "a wrong passphrase booted");
    assert!(
        ran.stderr.contains("sealed payload did not open"),
        "the refusal must be indistinct: {}",
        ran.stderr
    );
    assert!(
        ran.elapsed < Duration::from_secs(5),
        "it dialled before it opened the key file: {:?}",
        ran.elapsed
    );

    // And `sealed-master` with no file is a refusal that names the variable.
    let mut c = host(DB, port);
    c.env("ORDO_KEY_PROVIDER", "sealed-master")
        .env("ORDO_PG_HOST", "203.0.113.1")
        .env_remove("ORDO_MASTER_KEY");
    let ran = run_to_completion(c);
    assert!(
        ran.stderr.contains("ORDO_MASTER_KEY_FILE"),
        "{}",
        ran.stderr
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// `plan-production-ops.md` R1b: `$keycustody` holds wrapped keys, never the
/// master — that is what makes the table safe to sit inside the deployment's
/// own backup. A row whose `wrapped` bytes equal the master itself is the
/// misconfiguration landing anyway, and the boot must refuse rather than
/// serve under a master a copy of the database already contains.
#[tokio::test(flavor = "multi_thread")]
async fn a_master_stored_in_the_database_refuses_the_boot() {
    const DB: &str = "ordo_rt_host_master_leaked";
    let master = ordo_crypto::master::derive_from_secret(MASTER.as_bytes());
    deployment_under(
        DB,
        include_str!("../../ordo-conformance/fixtures/market.ordo"),
        master,
    )
    .await;

    // The leak: a row in `$keycustody` whose `wrapped` bytes are the master
    // itself, under a space name that is not the deployment's own — the
    // check must find it regardless of which space it landed under.
    let pg = Pg {
        db: DB.to_string(),
        ..Pg::default()
    };
    pg.exec(&format!(
        r#"INSERT INTO "$keycustody" ("space", "kind", "epoch", "wrapped")
           VALUES ('leaked', 'smk', 0, '\x{}'::bytea)"#,
        ordo_crypto::hex(&master)
    ))
    .expect("the leak inserts");

    let port = free_port();
    let ran = run_to_completion(host(DB, port));
    assert_ne!(ran.code, 0, "a master stored in the database booted");
    assert!(
        ran.stderr.contains("$keycustody"),
        "the refusal must name the table: {}",
        ran.stderr
    );

    // Remove the leak: the same deployment boots normally again, proving
    // the refusal was about the leaked row and not the deployment itself.
    pg.exec(r#"DELETE FROM "$keycustody" WHERE "space" = 'leaked'"#)
        .expect("cleanup");
    let port = free_port();
    let mut child = host(DB, port).spawn().expect("spawn");
    wait_for_live(port, &mut child);
    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── the blob store, and when a deployment must have one ──────────────────

/// The shipped fixture plus one entity with a stored blob field. Appended
/// rather than edited in, the way `blob_door.rs` and `guard_groups.rs` both
/// do it, so nothing here moves a number another suite asserts about
/// `market.ordo`.
const WITH_A_STORED_BLOB: &str = "
entity docs (key: doc) {
  owner -> members
  title:  text
  scan:   blob?
  access read   owner == $caller
  access write  owner == $caller
}
";

/// **The blob-store rule, both directions.** A program that declares a
/// stored blob field is a program whose door has a file route; a deployment
/// serving it with no store would 500 on the first upload, in production,
/// on the path the program was written for. So it is a refusal, and the
/// refusal names the field.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_with_a_stored_blob_field_needs_a_store() {
    const DB: &str = "ordo_rt_host_blob";
    let source = format!(
        "{}{WITH_A_STORED_BLOB}",
        include_str!("../../ordo-conformance/fixtures/market.ordo")
    );
    let _pool = deployment_of(DB, &source).await;
    let port = free_port();

    // No store: refused, and the refusal names both the variable and the
    // field that made it necessary.
    let ran = run_to_completion(host(DB, port));
    assert_ne!(ran.code, 0, "a stored blob field with no store booted");
    assert!(
        ran.stderr.contains("ORDO_BLOB_STORE") && ran.stderr.contains("docs.scan"),
        "the refusal names neither the variable nor the field: {}",
        ran.stderr
    );

    // A test-double store, without the flag that admits it: also refused,
    // and for its own reason. `FsStore` asks for this in its own module doc
    // — "a deployment configuration naming it outside dev mode refuses to
    // boot with the reason".
    let dir = std::env::temp_dir().join(format!("ordo-rt-blob-{}", std::process::id()));
    let mut c = host(DB, port);
    c.env("ORDO_BLOB_STORE", format!("fs:{}", dir.display()));
    let ran = run_to_completion(c);
    assert_ne!(
        ran.code, 0,
        "a test double was accepted as a deployment store"
    );
    assert!(
        ran.stderr.contains("ORDO_DEV_UNSAFE_BLOB_STORE"),
        "the refusal does not name the opt-in: {}",
        ran.stderr
    );

    // And with the flag it boots, mounts the two blob routes, and does a
    // real round trip: the bytes leave sealed and come back identical.
    let mut c = host(DB, port);
    c.env("ORDO_BLOB_STORE", format!("fs:{}", dir.display()))
        .env("ORDO_DEV_UNSAFE_BLOB_STORE", "1");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    // A row to hang the file on.
    let pg = Pg {
        db: DB.into(),
        ..Pg::default()
    };
    pg.exec(&format!(
        "INSERT INTO \"docs\" (\"$space\", \"id\", \"owner\", \"title\") \
         VALUES ('{SPACE}', 'doc_1', 'mem_1', 'a doc')"
    ))
    .expect("row");

    let bytes = "the bytes that went through the running binary";
    let (status, body) = request(
        port,
        "PUT",
        "/blob/docs/scan/doc_1",
        &[
            ("x-ordo-space", SPACE),
            ("authorization", &format!("Bearer {}", token())),
            ("content-type", "text/plain"),
        ],
        bytes,
    );
    assert_eq!(status, 204, "the upload was refused: {body}");

    // The read is by **handle**, not by `entity/field/id`: the write
    // addresses a row and mints the handle, and the row is what now holds
    // it. Reading it out of the column is what a client does with the
    // projection it got back.
    let handle = pg
        .query(r#"SELECT "scan" FROM "docs" WHERE "id" = 'doc_1'"#)
        .expect("row")[0][0]
        .clone()
        .expect("the write set the column");
    let (status, body) = request(
        port,
        "GET",
        &format!("/blob/{handle}"),
        &[
            ("x-ordo-space", SPACE),
            ("authorization", &format!("Bearer {}", token())),
        ],
        "",
    );
    assert_eq!(status, 200, "the download was refused: {body}");
    assert!(body.ends_with(bytes), "the bytes did not come back: {body}");

    // Sealed at rest: the object on disk is not the plaintext.
    let mut on_disk = String::new();
    for entry in walk(&dir) {
        if let Ok(s) = std::fs::read(&entry) {
            on_disk.push_str(&String::from_utf8_lossy(&s));
        }
    }
    assert!(
        !on_disk.is_empty(),
        "no object reached the store at {dir:?}"
    );
    assert!(
        !on_disk.contains(bytes),
        "the object store holds the plaintext"
    );

    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("blobs       fs") && stdout.contains("UNSAFE"),
        "the log does not name the store and its posture:\n{stdout}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

// ── helpers ──────────────────────────────────────────────────────────────

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// `GET /health/ready` as the space's owner (`mem_o`), who reads its detail.
fn ready_as_owner(port: u16) -> (u16, String) {
    request(
        port,
        "GET",
        "/health/ready",
        &[(
            "authorization",
            &format!("Bearer {}", token_for("mem_o", true)),
        )],
        "",
    )
}

/// A bearer token the running binary will accept: minted under the same
/// `ORDO_TOKEN_KEY` the child was given, for a member the seed contains.
fn token() -> String {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    let raw = unhex(TOKEN_KEY);
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&key),
        &ordo_identity::TokenSpec {
            principal: PrincipalId("mem_1".into()),
            expires: Timestamp(u64::MAX / 2),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

/// A token for `principal`, with user presence stamped now when `present`:
/// the platform ops are step-up gated, and a presence older than the
/// engine's window is refused.
fn token_for(principal: &str, present: bool) -> String {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    let raw = unhex(TOKEN_KEY);
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&key),
        &ordo_identity::TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(u64::MAX / 2),
            client: None,
            user_presence_at: present.then_some(Timestamp(now)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

// ── the platform ops, served by the binary ───────────────────────────────

/// The running binary serves `std.space.*`: `callersOf` answers the owner
/// and refuses an editor, the exit ops are registered when
/// `ORDO_EXIT_BUNDLE_DIR` is set, and an export writes its bundle there.
///
/// Before this, the binary booted with `Deployment::boot`, so the engine
/// granted these ops and the registry held none of them: every one was a
/// 404 on a real deployment.
#[tokio::test(flavor = "multi_thread")]
async fn the_binary_serves_the_platform_ops() {
    const DB: &str = "ordo_rt_host_platform";
    let _pool = deployment_ready(DB).await;
    let bundles = std::env::temp_dir().join(format!("ordo-rt-bundles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bundles);
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_EXIT_BUNDLE_DIR", &bundles);
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let ask = |principal: &str| {
        request(
            port,
            "GET",
            "/op/std.space.callersOf?op=setMemo",
            &[
                ("x-ordo-space", SPACE),
                (
                    "authorization",
                    &format!("Bearer {}", token_for(principal, true)),
                ),
            ],
            "",
        )
    };

    // The owner: 200, and the answer names callers of `setMemo`, the
    // owner among them.
    let (status, body) = ask("mem_o");
    assert_eq!(status, 200, "callersOf refused the owner: {body}");
    assert!(
        body.contains("\"principal\"") && body.contains("mem_o"),
        "callersOf answered without its callers: {body}"
    );

    // An editor: refused. Not 200, and not a 500 from a registered op with
    // no state behind it.
    let (status, body) = ask("mem_2");
    assert!(
        status == 403 || status == 404,
        "callersOf answered an editor with {status}: {body}"
    );

    // The exit round trip: an owner's export writes a bundle under the
    // configured directory.
    let (status, body) = request(
        port,
        "POST",
        "/op/std.space.export",
        &[
            ("x-ordo-space", SPACE),
            (
                "authorization",
                &format!("Bearer {}", token_for("mem_o", true)),
            ),
            ("content-type", "application/json"),
        ],
        &serde_json::json!({ "ciphertext": true }).to_string(),
    );
    assert_eq!(status, 200, "the export refused the owner: {body}");
    assert!(
        !walk(&bundles.join(SPACE)).is_empty(),
        "the export answered 200 and wrote nothing under {}",
        bundles.display()
    );

    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("platform    std.space.callersOf, std.space.export, std.space.delete"),
        "the log does not list the platform ops served:\n{stdout}"
    );
    // Not configured, so not registered, and the log says why.
    assert!(
        stdout.contains("DISABLED    std.space.attestTo")
            && stdout.contains("DISABLED    std.space.attachBucket"),
        "an unwired platform op is not named in the census:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&bundles);
}

/// Unset, the exit ops are unregistered rather than registered and broken:
/// the door answers for `std.space.export` as for any name nobody
/// registered, and the census names the variable.
#[tokio::test(flavor = "multi_thread")]
async fn without_a_bundle_dir_the_exit_ops_are_not_registered() {
    const DB: &str = "ordo_rt_host_noexit";
    let _pool = deployment_ready(DB).await;
    let port = free_port();
    let mut child = host(DB, port).spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let bearer = format!("Bearer {}", token_for("mem_o", true));
    let headers = [
        ("x-ordo-space", SPACE),
        ("authorization", bearer.as_str()),
        ("content-type", "application/json"),
    ];
    let (unwired, _) = request(port, "POST", "/op/std.space.export", &headers, "{}");
    let (nobody, _) = request(port, "POST", "/op/std.space.noSuchOp", &headers, "{}");
    assert_eq!(
        unwired, nobody,
        "an unwired exit op must answer as a name nobody registered"
    );
    // `callersOf` needs no configuration and is still served.
    let (status, body) = request(
        port,
        "GET",
        "/op/std.space.callersOf?op=setMemo",
        &headers,
        "",
    );
    assert_eq!(status, 200, "{body}");

    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("DISABLED    std.space.export") && stdout.contains("ORDO_EXIT_BUNDLE_DIR"),
        "the census does not name the variable:\n{stdout}"
    );

    // A relative path is refused before the dial.
    let mut c = host(DB, port);
    c.env("ORDO_EXIT_BUNDLE_DIR", "bundles")
        .env("ORDO_PG_HOST", "203.0.113.1");
    let ran = run_to_completion(c);
    assert_ne!(ran.code, 0, "a relative bundle directory was accepted");
    assert!(
        ran.stderr.contains("ORDO_EXIT_BUNDLE_DIR") && ran.elapsed < Duration::from_secs(5),
        "the refusal was slow or did not name the variable: {}",
        ran.stderr
    );
}

/// No program can spell a `std.space.*` name today: `space` is a keyword,
/// so `entity space` is a parse error and the std derivation never produces
/// `std.space.get`. The boot's reserved-name refusal is defence in depth
/// behind the grammar, and `tests/deploy.rs`'s
/// `platform_ops_survive_a_swap_and_a_colliding_version_is_refused` proves it
/// on the `Deployment` the binary boots. This pins the grammar half, so the
/// day the keyword is relaxed this test fails and the binary-level
/// collision test becomes writable.
#[test]
fn no_program_can_compile_a_std_space_name() {
    let source = format!(
        "{}\nentity space {{\n  name: text\n}}\n",
        include_str!("../../ordo-conformance/fixtures/market.ordo")
    );
    let refused = Fixture::from_source(&source, "").err().expect(
        "`entity space` compiled; write the binary-level reserved-name test \
         (a program deriving `std.space.*` must refuse to load)",
    );
    assert!(refused.contains("space"), "{refused}");
}

// ── 17v: the system space exists at boot ─────────────────────────────────

/// An empty database: created if this suite has never run, and emptied if
/// it has. Nothing else, so no `$keycustody`, no registry and no program
/// DDL. This is what a first deployment is pointed at.
fn empty_database(db: &str) -> Pg {
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
        .expect("an empty database");
    pg
}

/// 17v's done-when, as a process: a fresh database and a master key, with
/// no operator command in between, is a deployment that serves and has a
/// system space.
///
/// Three things are proved here and nowhere else, because each is a thing
/// the *binary* must do on the way up rather than a thing a library
/// function does when a test calls it:
///
/// 1. **The platform's own tables are created**, custody included. Before
///    17v `PgKeyStore::open` met a database with no `$keycustody` and
///    refused with `run ordo keys init`, which is the crash loop the first
///    cluster deployment hit.
/// 2. **The system space exists**, with its SMK and epoch 1's DEK in
///    `$keycustody`, minted by the boot rather than by a ceremony.
/// 3. **The door answers**, on a database no `ordo apply` has ever run
///    against, which is the no-program state a first deployment is
///    deployed *to*.
///
/// It then runs that deployment's first `ordo apply`, in the same process
/// and with no restart, because that is the moment `members` comes into
/// existence and therefore the moment the system space's founding row can
/// be written. The swap writes it, before the door it rebuilds reads the
/// table. One test rather than two: it is one deployment's first minutes,
/// and each binary this suite spawns is a pool on a cluster another suite
/// is using.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_database_and_a_master_key_boot_with_a_system_space() {
    const DB: &str = "ordo_rt_host_fresh_system";
    let pg = empty_database(DB);

    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_SPACE", ordo_host::provision::SYSTEM_SPACE)
        // Every test here spawns a binary and this suite runs them at
        // once, against one cluster another suite is also using. Two
        // connections are all these assertions need.
        .env("ORDO_PG_POOL_SIZE", "2");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let (status, _) = request(port, "GET", "/health/live", &[], "");
    assert_eq!(status, 200, "/health/live on a fresh database");

    let rows = pg
        .query(&format!(
            "SELECT \"kind\" FROM \"$keycustody\" WHERE \"space\" = '{}' ORDER BY \"kind\"",
            ordo_host::provision::SYSTEM_SPACE
        ))
        .expect("$keycustody exists, and the boot is what created it");
    let kinds: Vec<&str> = rows.iter().map(|r| r[0].as_deref().unwrap_or("")).collect();
    assert_eq!(
        kinds,
        vec!["dek", "smk"],
        "the system space has no custody of its own"
    );

    // The first `ordo apply`, in the two acts the command performs: the
    // program's DDL, which is what creates `members`, then the artifact and
    // the activation that notifies every replica on `ordo.deploy`.
    let source = include_str!("../../ordo-conformance/fixtures/market.ordo");
    let prog = Fixture::from_source(source, "").expect("the program compiles");
    pg.exec(&prog.ddl).expect("ddl");
    // One connection: a handful of statements takes a handful of
    // statements' worth of the cluster.
    let pool = Pool::connect(PoolConfig { size: 1, ..cfg(DB) })
        .await
        .expect("pool");
    let doc =
        ordo_plan::artifact::program_artifact(&prog.schema, &prog.registry).expect("artifact");
    let ddl = prog.ddl.clone();
    let art = ordo_exec::registry::Artifact {
        version: ordo_plan::artifact::program_version(&doc, &ddl),
        artifact: ordo_plan::artifact::to_bytes(&doc),
        source: source.to_string(),
        ddl,
        applied_at: ordo_commandlog::Timestamp(1),
        actor: "operator:17v".into(),
    };
    ordo_exec::registry::store(&pool, &art)
        .await
        .expect("store");
    ordo_exec::registry::activate(
        &pool,
        &art.version,
        "operator:17v",
        ordo_commandlog::Timestamp(2),
    )
    .await
    .expect("activate");

    let count = format!(
        "SELECT count(*) FROM \"members\" WHERE \"$space\" = '{}'",
        ordo_host::provision::SYSTEM_SPACE
    );
    // 30 s, not 5: on a loaded CI node the version swap and the founding
    // row came after 5 s (2026-11-01). The loop ends as soon as it lands.
    let mut founding = String::new();
    for _ in 0..600 {
        founding = pool.rows(&count).await.expect("count")[0][0]
            .clone()
            .unwrap_or_default();
        if founding == "1" {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // The node says in `$nodes` that it serves the version and finished
    // its tenant catch-up for it: what `ordo apply` waits on (2026-10-22).
    let mut said = None;
    for _ in 0..600 {
        let nodes = ordo_exec::nodes::live(&pool).await.expect("nodes");
        let p = ordo_exec::nodes::progress(&nodes, &art.version);
        if !nodes.is_empty() && p.settled() {
            said = Some(nodes);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let said = said.expect("the node never said it serves the version");
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].serving, art.version);
    assert_eq!(said[0].caught_up, art.version);

    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // A node that stops says so, so no apply waits for it.
    assert!(
        ordo_exec::nodes::live(&pool)
            .await
            .expect("nodes")
            .is_empty(),
        "the stopped node's row is still live:\n{stdout}"
    );
    drop(pool);
    assert_eq!(
        founding, "1",
        "the first apply did not complete the system space:\n{stdout}"
    );
    // The log says what the boot did about it, because a space the
    // platform created without being asked is something an operator should
    // read rather than discover in a table.
    assert!(
        stdout.contains(&format!(
            "  system      {}",
            ordo_host::provision::SYSTEM_SPACE
        )),
        "the startup log does not name the system space:\n{stdout}"
    );
    assert!(
        stdout.contains("custody only"),
        "on a database with no program, the founding row is owed and the log \
         should say so:\n{stdout}"
    );
}

/// The other half of the fork: on a deployment an `ordo apply` has already
/// reached, the boot writes the system space's founding member row, and a
/// second boot writes nothing.
///
/// This is what makes the system space a space the door can serve rather
/// than two rows in a key table. One row, across two boots, is the property
/// `provision_space`'s `ON CONFLICT DO NOTHING` gives and the reason this
/// path reuses it instead of writing its own insert.
#[tokio::test(flavor = "multi_thread")]
async fn the_system_space_gets_one_founding_row_across_two_boots() {
    const DB: &str = "ordo_rt_host_system_member";
    let pool = deployment_ready(DB).await;
    let members = format!(
        "SELECT count(*) FROM \"members\" WHERE \"$space\" = '{}'",
        ordo_host::provision::SYSTEM_SPACE
    );
    assert_eq!(
        pool.rows(&members).await.expect("count")[0][0].as_deref(),
        Some("0"),
        "the fixture already has a system space; this test would prove nothing"
    );

    for boot in 1..=2 {
        let port = free_port();
        let mut c = host(DB, port);
        c.env("ORDO_SPACE", ordo_host::provision::SYSTEM_SPACE)
            .env("ORDO_PG_POOL_SIZE", "2");
        let mut child = c.spawn().expect("spawn");
        wait_for_live(port, &mut child);
        kill(&mut child, "TERM");
        let out = child.wait_with_output().expect("wait");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let expected = if boot == 1 {
            "created at boot"
        } else {
            "present"
        };
        assert!(
            stdout.contains(expected),
            "boot {boot} should report `{expected}`:\n{stdout}"
        );
    }

    assert_eq!(
        pool.rows(&members).await.expect("count")[0][0].as_deref(),
        Some("1"),
        "two boots left two owners in the system space"
    );
    let row = pool
        .rows(&format!(
            "SELECT \"id\", \"principal\", \"role\" FROM \"members\" WHERE \"$space\" = '{}'",
            ordo_host::provision::SYSTEM_SPACE
        ))
        .await
        .expect("the founding row");
    assert_eq!(
        row[0][0].as_deref(),
        Some(ordo_host::provision::SYSTEM_MEMBER_ID)
    );
    assert_eq!(
        row[0][1].as_deref(),
        Some(ordo_host::provision::SYSTEM_PRINCIPAL)
    );
    assert_eq!(row[0][2].as_deref(), Some("owner"));
}

/// The grace bounds the drain, not the serving.
///
/// `serve` used to wrap the whole serve future in `timeout(grace + 1s)`, so
/// a deployment stopped of its own accord that long after it bound,
/// whatever traffic it was carrying, and reported a drain that had
/// overrun. Every other test here outlives its process by less than the
/// grace, so none of them could see it. The first cluster deployment did:
/// it came up correctly, served, and died on a timer
/// (`ordo-host: serve: shutdown exceeded its grace`, 2026-09-20).
///
/// A one-second grace and a three-second wait is the whole test.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_keeps_serving_long_past_its_shutdown_grace() {
    const DB: &str = "ordo_rt_host_past_grace";
    let _pool = deployment_ready(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_SHUTDOWN_GRACE_SECS", "1");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    std::thread::sleep(Duration::from_secs(3));

    assert!(
        matches!(child.try_wait(), Ok(None)),
        "the process exited on its own after its grace elapsed"
    );
    let (status, _) = request(port, "GET", "/health/live", &[], "");
    assert_eq!(status, 200, "still serving three seconds past a 1s grace");

    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── 17aa: reconciliation at boot, and the quarantine it leaves ──────────

/// **A silent restore quarantines the space and leaves the door up**
/// (17aa, superseding R2d-iii's process-wide refusal).
///
/// Nothing can stop somebody with `psql`. What the platform can do is refuse
/// to serve a chain that has moved backwards without saying so, and the
/// record it compares against is in the control database, so a restore of
/// the space's own tables cannot take it away. What it must *not* do is die
/// of it: the owner, 2026-09-21, *"the system should not be dependent on the
/// rest of the data designs, deployment space, or tenants"*, and the process
/// that refuses to start is the process through which `ordo restore
/// --record` would have explained the rewind.
///
/// So the boot reconciles instead: it reads the chain, compares it against
/// the witness, and holds the space closed with the reason in
/// `/health/ready`. The deployment serves, the affected space does not, and
/// the two facts are reported together.
///
/// **Observed failing first:** `wait_for_live` panicked with
/// `the binary exited before serving` and the boot refusal's text — the
/// process-wide refusal this test now denies.
#[tokio::test(flavor = "multi_thread")]
async fn a_rewound_space_is_quarantined_and_the_door_stays_up() {
    const DB: &str = "ordo_rt_host_rewound";
    let pool = deployment_ready(DB).await;
    let space = ordo_commandlog::SpaceId(SPACE.into());
    let witness = ordo_exec::witness::Witness::new(pool.clone());

    // Some history, and a checkpoint over it — witnessed, as every
    // checkpoint is.
    let f = Fixture::load().expect("fixture");
    for n in 0..4usize {
        let g = ordo_commandlog::CommandGroup {
            correlation: ordo_commandlog::CorrelationId(format!("boot_cg_{n}")),
            actor: ordo_commandlog::Actor::User("usr_boot".into()),
            commands: vec![ordo_commandlog::Command {
                id: ordo_commandlog::CommandId(format!("boot_cmd_{n}")),
                space: space.clone(),
                entity: "task".into(),
                row_id: format!("tsk_{n}"),
                kind: ordo_commandlog::CommandKind::Create,
                before: None,
                after: Some(ordo_commandlog::Image::new()),
                at: ordo_commandlog::Timestamp(1_000 + n as u64),
            }],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        };
        let tx = pool.begin().await.unwrap();
        tx.append_group(&g).await.expect("append");
        tx.commit().await.unwrap();
    }
    drop(f);
    let signer = ordo_commandlog::Ed25519Signer::from_seed("boot-key", &[77u8; 32]);
    ordo_exec::undo::sign_checkpoint(
        &pool,
        &space,
        &signer,
        ordo_commandlog::Timestamp(400),
        &witness,
    )
    .await
    .expect("a checkpoint at 4");

    // The rewind, and the shape a restore of the space's own tables leaves:
    // `$checkpoints` goes back with the chain, so the space's own tables
    // agree with each other and say nothing.
    let pg = Pg {
        db: DB.to_string(),
        ..Pg::default()
    };
    pg.exec(&format!(
        r#"DELETE FROM "$commandlog" WHERE "space" = '{SPACE}' AND "idx" >= 2;
           DELETE FROM "$checkpoints" WHERE "space" = '{SPACE}'"#
    ))
    .expect("the rewind");

    // The binary comes up on the rewound database, which is the whole
    // point: the door an operator records the restore through is the door
    // the old process-wide refusal removed.
    let port = free_port();
    let mut child = host(DB, port).spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let (status, raw) = ready_as_owner(port);
    assert_eq!(status, 200, "a quarantined tenant is not an outage: {raw}");
    let ready = json_body(&raw);
    assert_eq!(ready["status"], "ready", "{raw}");
    let held: Vec<&str> = ready["quarantined"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["space"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(
        held,
        [SPACE],
        "readiness must name the space the boot held closed: {raw}"
    );
    let why = ready["quarantined"][0]["reason"]
        .as_str()
        .unwrap_or_default();
    assert!(
        why.contains("no longer reaches"),
        "and say what reconciliation found: {raw}"
    );

    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("ordo restore --record"),
        "the startup report must name the lawful remedy: {stdout}"
    );

    // Recorded, the restore is lawful history and the deployment serves.
    ordo_exec::undo::record_restore(
        &pool,
        &witness,
        &space,
        ordo_exec::undo::RestoreRecord {
            operator: "ops@example.test".into(),
            source: ordo_commandlog::RestoreSource::Pitr {
                at: ordo_commandlog::Timestamp(1_757_000_000),
            },
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("the restore is recorded");

    let port = free_port();
    let mut child = host(DB, port).spawn().expect("spawn");
    wait_for_live(port, &mut child);
    let (_, raw) = ready_as_owner(port);
    assert_eq!(
        json_body(&raw)["quarantined"].as_array().map(Vec::len),
        Some(0),
        "a recorded restore reconciles as explained, and nothing is held closed: {raw}"
    );
    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(
        out.status.code(),
        Some(0),
        "a recorded restore serves: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **One list, not five — the headline of 18d T5's `R-F6`.** A tenant the
/// resolver's quarantine register holds closed and a tenant whose target
/// the door could not read at boot are two different sources that used to
/// answer two different questions: the register reached `/health/ready`
/// and the door's report reached only a line the boot printed and nowhere
/// else. `/health/ready` now merges both into the one `quarantined` array,
/// and the property that matters is that an operator reading it once sees
/// both kinds of trouble — each tenant named **exactly once**, not the
/// register's tenant twice or the door's tenant not at all.
///
/// The two tenants are produced by two different, unrelated mechanisms on
/// purpose: `SPACE` is rewound the way
/// `a_rewound_space_is_quarantined_and_the_door_stays_up` rewinds it, which
/// is the register's pre-T5 writer, and `GHOST` is attached to a target
/// nothing listens on, which only `routed_engine` (T1) ever touches. Two
/// spaces, from two mechanisms that share no code until `/health/ready`'s
/// merge, is the honest way to show they land in one list rather than
/// happening to agree because they were built the same way.
#[tokio::test(flavor = "multi_thread")]
async fn health_ready_names_every_troubled_tenant_from_one_list() {
    const DB: &str = "ordo_rt_host_t5_one_list";
    const GHOST: &str = "spc_t5_ghost";
    let pool = deployment_ready(DB).await;

    // The register's contribution: a chain that moved backwards on the
    // deployment's own space, unannounced.
    let space = ordo_commandlog::SpaceId(SPACE.into());
    let witness = ordo_exec::witness::Witness::new(pool.clone());
    for n in 0..4usize {
        let g = ordo_commandlog::CommandGroup {
            correlation: ordo_commandlog::CorrelationId(format!("t5_one_list_cg_{n}")),
            actor: ordo_commandlog::Actor::User("usr_t5".into()),
            commands: vec![ordo_commandlog::Command {
                id: ordo_commandlog::CommandId(format!("t5_one_list_cmd_{n}")),
                space: space.clone(),
                entity: "task".into(),
                row_id: format!("tsk_t5_{n}"),
                kind: ordo_commandlog::CommandKind::Create,
                before: None,
                after: Some(ordo_commandlog::Image::new()),
                at: ordo_commandlog::Timestamp(1_000 + n as u64),
            }],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        };
        let tx = pool.begin().await.unwrap();
        tx.append_group(&g).await.expect("append");
        tx.commit().await.unwrap();
    }
    let signer = ordo_commandlog::Ed25519Signer::from_seed("t5-one-list-key", &[91u8; 32]);
    ordo_exec::undo::sign_checkpoint(
        &pool,
        &space,
        &signer,
        ordo_commandlog::Timestamp(400),
        &witness,
    )
    .await
    .expect("a checkpoint at 4");
    let pg = Pg {
        db: DB.to_string(),
        ..Pg::default()
    };
    pg.exec(&format!(
        r#"DELETE FROM "$commandlog" WHERE "space" = '{SPACE}' AND "idx" >= 2;
           DELETE FROM "$checkpoints" WHERE "space" = '{SPACE}'"#
    ))
    .expect("the rewind");

    // The door's contribution: a second tenant attached to a target
    // nothing listens on, so `routed_engine` cannot read its members at
    // boot. No custody and no schema are needed for that read to fail —
    // only the attachment row.
    spacedb::ensure_table(&pool)
        .await
        .expect("the attachment table");
    spacedb::attach(
        &pool,
        &SpaceId(GHOST.into()),
        &DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            user: "ordo".into(),
            db: "t5_one_list_nowhere".into(),
            credential: "t5-one-list-nowhere".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "system:test".into(),
        },
    )
    .await
    .expect("attached");

    let port = free_port();
    let mut child = host(DB, port).spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let (status, raw) = ready_as_owner(port);
    assert_eq!(status, 200, "two closed tenants are not an outage: {raw}");
    let ready = json_body(&raw);
    // Read as a **sequence** before it is read as a map. A map would
    // collapse a duplicate silently, which is precisely the failure this
    // test exists to catch: the merge's whole job is that a space known
    // to both sources appears once rather than twice, and a
    // deduplicating collector would assert that for it.
    let listed: Vec<(String, String)> = ready["quarantined"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some((
                r["space"].as_str()?.to_string(),
                r["reason"].as_str()?.to_string(),
            ))
        })
        .collect();
    assert_eq!(
        listed.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(),
        vec![SPACE, GHOST],
        "the register's tenant and the door's tenant both appear, each \
         exactly once, in the array as published and not after any \
         deduplication a reader might do: {raw}"
    );
    let held: std::collections::BTreeMap<String, String> = listed.into_iter().collect();
    assert!(
        held[SPACE].contains("no longer reaches"),
        "the register's own reason still reads as a rewind: {raw}"
    );
    assert!(
        held[GHOST].contains("could not be read"),
        "the door's reason names its own failure, distinct from the \
         register's: {raw}"
    );

    kill(&mut child, "TERM");
    child.wait_with_output().expect("wait");
}

// ── every tenant has a database of its own (ruled 2026-09-25, 2026-10-06) ──

/// **`ORDO_PG_ROUTING=single` refuses the boot.** The setting is gone:
/// every tenant has a database of its own, so no deployment serves its
/// tenants from one database. Booting anyway would serve differently from
/// what the deployment owner wrote down. A mistyped value is refused the
/// same way, and before any dial.
#[test]
fn routing_single_refuses_the_boot() {
    for value in ["single", "attahced"] {
        let port = free_port();
        let mut c = host("ordo_rt_host_routing_gone", port);
        c.env("ORDO_PG_ROUTING", value);
        let ran = run_to_completion(c);
        assert_ne!(ran.code, 0, "`{value}` must not boot: {}", ran.stdout);
        assert!(
            ran.stderr.contains("ORDO_PG_ROUTING")
                && ran.stderr.contains("the setting is gone")
                && ran.stderr.contains("database of its own"),
            "the refusal names the setting and says every tenant has its own \
             database: {}",
            ran.stderr
        );
        assert!(
            ran.elapsed < Duration::from_secs(10),
            "the refusal came after a dial: {:?}",
            ran.elapsed
        );
    }
}

/// **`ORDO_PG_ROUTING=attached` boots, with no credentials directory, and
/// warns that the setting is gone.** It asked for what every deployment
/// now does.
#[tokio::test(flavor = "multi_thread")]
async fn routing_attached_boots_and_warns_that_the_setting_is_gone() {
    const DB: &str = "ordo_rt_host_routing_attached";
    let _pool = deployment_ready(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_PG_ROUTING", "attached")
        .env_remove("ORDO_TENANT_CREDENTIALS")
        .env_remove("ORDO_TENANT_CREDENTIALS_DIR");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);
    let (status, raw) = ready_as_owner(port);
    assert_eq!(status, 200, "it did not come up ready: {raw}");
    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ORDO_PG_ROUTING is set and the setting is gone"),
        "the boot did not warn: {stderr}"
    );
}

/// **A default tenant cluster dialled under a label of its own needs a
/// credential provider**, and the boot says so rather than every tenant
/// creation failing at its dial. Unset, the label is `$deployment`, which
/// needs none.
#[test]
fn a_default_tenant_credential_label_without_a_credential_provider_refuses_the_boot() {
    let port = free_port();
    let mut c = host("ordo_rt_host_tenant_label", port);
    c.env("ORDO_PG_TENANT_CREDENTIAL", "tenants")
        .env_remove("ORDO_TENANT_CREDENTIALS")
        .env_remove("ORDO_TENANT_CREDENTIALS_DIR");
    let ran = run_to_completion(c);
    assert_ne!(ran.code, 0, "{}", ran.stdout);
    assert!(
        ran.stderr.contains("ORDO_PG_TENANT_CREDENTIAL")
            && ran.stderr.contains("ORDO_TENANT_CREDENTIALS=dir"),
        "the refusal names the setting and what would fix it: {}",
        ran.stderr
    );
}

// ── 18e U5: which STORE a space's bytes are in ───────────────────────────

/// **A mistyped blob routing mode refuses**, exactly as a mistyped
/// database one does.
///
/// `ORDO_PG_ROUTING`'s argument carried across without weakening: a
/// deployment that meant `attached` and mistyped must be told, because the
/// silent alternative serves every tenant's bytes out of the deployment's
/// own bucket and looks perfectly healthy doing it. The refusal names the
/// variable and the two words that exist, because an operator who wrote a
/// third one has already decided what they want.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_blob_routing_value_refuses_at_boot() {
    const DB: &str = "ordo_rt_host_blob_routing_typo";
    let _pool = deployment_ready(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_BLOB_ROUTING", "attahced");
    let ran = run_to_completion(c);
    assert_ne!(ran.code, 0, "a mistyped mode must not boot: {}", ran.stdout);
    assert!(
        ran.stderr.contains("ORDO_BLOB_ROUTING") && ran.stderr.contains("single | attached"),
        "the refusal must name the variable and its legal values: {}",
        ran.stderr
    );
    // And it is refused on its own terms, not by being mistaken for the
    // database switch. Two variables that can be turned on independently
    // (§2 B-C1, D-S1) are two refusals.
    assert!(
        !ran.stderr.contains("ORDO_PG_ROUTING"),
        "the blob switch's refusal named the database switch: {}",
        ran.stderr
    );
}

/// **`attached` with no credential provider refuses** (§2 B-C1, §3 B-K1).
///
/// A `$spacebucket` row names a credential label and never a key pair, so
/// a node with no provider resolves an attachment and then holds no
/// material to sign to it. Without this refusal the node comes up, reports
/// ready, and fails one upload at a time, and §3 B-K3's point is that a
/// failed upload has already consumed the client's bytes, which is why
/// this costs milliseconds at boot instead.
#[tokio::test(flavor = "multi_thread")]
async fn attached_blob_routing_without_a_credential_provider_refuses() {
    const DB: &str = "ordo_rt_host_blob_routing_nocreds";
    let _pool = deployment_ready(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_BLOB_ROUTING", "attached")
        .env_remove("ORDO_TENANT_CREDENTIALS")
        .env_remove("ORDO_TENANT_CREDENTIALS_DIR");
    let ran = run_to_completion(c);
    assert_ne!(ran.code, 0, "{}", ran.stdout);
    assert!(
        ran.stderr.contains("ORDO_BLOB_ROUTING") && ran.stderr.contains("ORDO_TENANT_CREDENTIALS"),
        "the refusal must name both the switch and what would fix it: {}",
        ran.stderr
    );
}

/// **Turning the switch on changes nothing until a bucket is attached.**
///
/// This is the property that makes U5 safe to land ahead of the attach op
/// (U6), and it is asserted on what a client can see rather than on what
/// the runtime holds. `AttachedStores` reads `$spacebucket`, an empty
/// `$spacebucket` resolves every space to the deployment's own store, and
/// that store is the one `ORDO_BLOB_STORE` named. A deployment that flips
/// the variable and attaches nothing must therefore be indistinguishable
/// from one that did not, and if it is not, the switch is carrying a
/// behaviour change nobody asked for.
///
/// **Four observables, and the first two are the ones that matter.** A
/// round trip through the real door proves the write path and the read
/// path both resolve; the object's key on disk proves they resolved to the
/// *same place* rather than to two stores that happen to agree about
/// bytes. `/health/ready` and the startup line are T4's bar, kept, because
/// a switch that changed what a deployment says about itself would still
/// be a change an operator has to explain.
///
/// **The name is not the plan's.** §11 U5 calls this
/// `a_deployment_that_attaches_nothing_behaves_identically`, which is
/// also, word for word, what §10 T4 calls its own, and both land in this
/// file, so one of them has to say which "nothing" it means. T4's is the
/// database's and keeps the plan's spelling because it was there first;
/// this one names the bucket.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_that_attaches_no_bucket_behaves_identically() {
    const DB: &str = "ordo_rt_host_blob_routing_noop";
    let source = format!(
        "{}{WITH_A_STORED_BLOB}",
        include_str!("../../ordo-conformance/fixtures/market.ordo")
    );
    let _pool = deployment_of(DB, &source).await;
    let pg = Pg {
        db: DB.into(),
        ..Pg::default()
    };

    // An empty credential directory in BOTH modes, so the one difference
    // between the two runs is the variable under test. A provider that
    // holds no labels is a legitimate deployment and is what a node that
    // has installed nothing yet is.
    let creds = std::env::temp_dir().join(format!("ordo_u5_creds_{}", std::process::id()));
    std::fs::create_dir_all(&creds).expect("credential dir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&creds, std::fs::Permissions::from_mode(0o700)).expect("mode");
    }

    let bytes = "the bytes a deployment that attached nothing still round-trips";
    let mut observed = Vec::new();
    for (n, mode) in ["single", "attached"].iter().enumerate() {
        let dir = std::env::temp_dir().join(format!("ordo-u5-noop-{mode}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let port = free_port();
        let mut c = host(DB, port);
        c.env("ORDO_BLOB_ROUTING", mode)
            .env("ORDO_BLOB_STORE", format!("fs:{}", dir.display()))
            .env("ORDO_DEV_UNSAFE_BLOB_STORE", "1")
            .env("ORDO_TENANT_CREDENTIALS", "dir")
            .env("ORDO_TENANT_CREDENTIALS_DIR", &creds);
        let mut child = c.spawn().expect("spawn");
        wait_for_live(port, &mut child);

        // A row of this run's own, so the two runs never share a handle
        // and the comparison below is of two independent uploads.
        let doc = format!("doc_{n}");
        pg.exec(&format!(
            "INSERT INTO \"docs\" (\"$space\", \"id\", \"owner\", \"title\") \
             VALUES ('{SPACE}', '{doc}', 'mem_1', 'a doc')"
        ))
        .expect("row");

        let (put, put_body) = request(
            port,
            "PUT",
            &format!("/blob/docs/scan/{doc}"),
            &[
                ("x-ordo-space", SPACE),
                ("authorization", &format!("Bearer {}", token())),
                ("content-type", "text/plain"),
            ],
            bytes,
        );
        // Checked before the column is read: a refused upload leaves the
        // column empty, and the message should name the refusal, not the
        // empty column (a CI run on 2026-10-05 failed here with no body).
        if put != 204 {
            kill(&mut child, "TERM");
            let out = child.wait_with_output().expect("wait");
            panic!(
                "{mode}: the upload was refused: {put} {put_body}\nserver stderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let handle = pg
            .query(&format!(
                "SELECT \"scan\" FROM \"docs\" WHERE \"id\" = '{doc}'"
            ))
            .expect("row")[0][0]
            .clone()
            .expect("the write set the column");
        let (get, get_body) = request(
            port,
            "GET",
            &format!("/blob/{handle}"),
            &[
                ("x-ordo-space", SPACE),
                ("authorization", &format!("Bearer {}", token())),
            ],
            "",
        );

        // Where the object landed, as the store's own layout: the path
        // components under the store root, with the handle's hex replaced
        // by its shape. The hex differs between runs by construction (it
        // is keyed-random per upload), and the *layout* is what must not.
        let mut layout: Vec<Vec<String>> = walk(&dir)
            .iter()
            .filter_map(|p| p.strip_prefix(&dir).ok().map(|r| r.to_path_buf()))
            .map(|r| {
                r.components()
                    .map(|c| {
                        let s = c.as_os_str().to_string_lossy().into_owned();
                        if s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                            "<handle>".to_string()
                        } else {
                            s
                        }
                    })
                    .collect()
            })
            .collect();
        layout.sort();

        let (status, raw) = ready_as_owner(port);
        assert_eq!(status, 200, "{mode} did not come up ready: {raw}");
        let ready = json_body(&raw);

        kill(&mut child, "TERM");
        let out = child.wait_with_output().expect("wait");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let blob_line = stdout
            .lines()
            .find(|l| l.trim_start().starts_with("blobs"))
            .map(|l| l.replace(&dir.display().to_string(), "<dir>"))
            .unwrap_or_default();

        assert_eq!(put, 204, "{mode}: the upload was refused: {put_body}");
        assert_eq!(get, 200, "{mode}: the download was refused: {get_body}");
        assert!(
            get_body.ends_with(bytes),
            "{mode}: the bytes did not come back: {get_body}"
        );
        observed.push((
            layout,
            blob_line,
            ready["status"].clone(),
            ready["program_active"].clone(),
            ready["quarantined"].clone(),
            ready["checks"].clone(),
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
    let _ = std::fs::remove_dir_all(&creds);

    assert_eq!(
        observed[0], observed[1],
        "turning blob routing on with nothing attached changed where a \
         space's bytes go, or what the deployment reports about itself"
    );
    assert_eq!(
        observed[0].0.len(),
        1,
        "the round trip wrote no object, so the comparison above compared \
         two empty stores: {:?}",
        observed[0].0
    );
    assert_eq!(
        observed[0].0[0],
        vec![SPACE.to_string(), "<handle>".to_string()],
        "the object key is no longer <space>/<handle>"
    );
}

// ── S2: the relying party from the environment ───────────────────────────

/// The binary with authentication configured, and every passkey variable
/// stripped, so a test names exactly the ones it sets.
fn host_with_auth(db: &str, port: u16, methods: &str) -> Command {
    let mut c = host(db, port);
    c.env("ORDO_AUTH_METHODS", methods)
        .env_remove("ORDO_APEX_DOMAIN")
        .env_remove("ORDO_AUTH_ORIGINS")
        .env_remove("ORDO_AUTH_RELATED_ORIGINS")
        .env_remove("ORDO_AUTH_RP_NAME")
        .env_remove("ORDO_AUTH_USER_VERIFICATION");
    // A database that cannot be reached: the refusals below must arrive
    // before any dial.
    c.env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432");
    c
}

fn refused(c: Command) -> Ran {
    let ran = run_to_completion(c);
    assert_ne!(ran.code, 0, "the boot was not refused:\n{}", ran.stdout);
    assert!(
        ran.elapsed < Duration::from_secs(3),
        "the binary dialled before it refused ({:?})",
        ran.elapsed
    );
    ran
}

/// A database with custody and no program, ready for a boot.
async fn custody_only(db: &str) {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pool = Pool::connect(cfg(db)).await.expect("pool");
    ordo_exec::keystore::ensure_tables(&pool)
        .await
        .expect("custody");
    ordo_crypto::LocalKeyProvider::new(
        MASTER.as_bytes(),
        Box::new(ordo_exec::keystore::PgKeyStore::open(cfg(db)).expect("store")),
    )
    .create_space(&ordo_crypto::SpaceId(SPACE.into()))
    .expect("custody minted");
}

#[test]
fn passkey_without_an_apex_domain_refuses_the_boot() {
    let mut c = host_with_auth("ordo_rt_never_dialled", free_port(), "passkey");
    c.env("ORDO_AUTH_ORIGINS", "https://app.example.com");
    let ran = refused(c);
    assert!(ran.stderr.contains("ORDO_APEX_DOMAIN"), "{}", ran.stderr);
    assert!(ran.stderr.contains("strands"), "{}", ran.stderr);
}

#[test]
fn passkey_without_origins_refuses_the_boot() {
    let mut c = host_with_auth("ordo_rt_never_dialled", free_port(), "password,passkey");
    c.env("ORDO_APEX_DOMAIN", "example.com");
    let ran = refused(c);
    assert!(ran.stderr.contains("ORDO_AUTH_ORIGINS"), "{}", ran.stderr);
}

#[test]
fn an_origin_outside_the_apex_refuses_the_boot_by_name() {
    let mut c = host_with_auth("ordo_rt_never_dialled", free_port(), "passkey");
    c.env("ORDO_APEX_DOMAIN", "example.com").env(
        "ORDO_AUTH_ORIGINS",
        "https://app.example.com,https://app.other.test",
    );
    let ran = refused(c);
    assert!(ran.stderr.contains("ORDO_AUTH_ORIGINS"), "{}", ran.stderr);
    assert!(
        ran.stderr.contains("https://app.other.test"),
        "{}",
        ran.stderr
    );
    assert!(
        ran.stderr.contains("ORDO_AUTH_RELATED_ORIGINS"),
        "the refusal points at the explicit option: {}",
        ran.stderr
    );
}

#[test]
fn a_bad_apex_a_bad_related_origin_and_a_bad_word_each_refuse_by_name() {
    for (var, value, apex, related) in [
        (
            "ORDO_APEX_DOMAIN",
            "https://example.com",
            "https://example.com",
            "",
        ),
        (
            "ORDO_AUTH_RELATED_ORIGINS",
            "http://app.other.test",
            "example.com",
            "http://app.other.test",
        ),
        (
            "ORDO_AUTH_USER_VERIFICATION",
            "sometimes",
            "example.com",
            "",
        ),
    ] {
        let mut c = host_with_auth("ordo_rt_never_dialled", free_port(), "passkey");
        c.env("ORDO_APEX_DOMAIN", apex)
            .env("ORDO_AUTH_ORIGINS", "https://app.example.com");
        if !related.is_empty() {
            c.env("ORDO_AUTH_RELATED_ORIGINS", related);
        }
        if var == "ORDO_AUTH_USER_VERIFICATION" {
            c.env(var, value);
        }
        let ran = refused(c);
        assert!(ran.stderr.contains(var), "{var}: {}", ran.stderr);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn password_alone_still_boots_without_an_apex_domain() {
    const DB: &str = "ordo_rt_host_pwonly";
    custody_only(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_AUTH_METHODS", "password")
        .env_remove("ORDO_APEX_DOMAIN");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);
    let (status, _) = request(port, "GET", "/.well-known/webauthn", &[], "");
    assert_eq!(status, 404, "no passkey, nothing to publish");
    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!stdout.contains("passkey     relying party"), "{stdout}");
}

/// The relying party reaches the running binary: the census names it, and
/// the related origins are published at the well-known path.
#[tokio::test(flavor = "multi_thread")]
async fn a_passkey_deployment_names_its_relying_party_and_publishes_its_related_origins() {
    const DB: &str = "ordo_rt_host_passkey";
    custody_only(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_AUTH_METHODS", "password,passkey")
        .env("ORDO_APEX_DOMAIN", "example.com")
        .env("ORDO_AUTH_ORIGINS", "https://app.example.com")
        .env("ORDO_AUTH_RELATED_ORIGINS", "https://app.other-brand.test")
        .env("ORDO_AUTH_RP_NAME", "Example");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);
    let (status, body) = request(port, "GET", "/.well-known/webauthn", &[], "");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        json_body(&body),
        serde_json::json!({"origins": ["https://app.other-brand.test"]})
    );
    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("relying party example.com (Example); 1 origin(s) under it, 1 related"),
        "{stdout}"
    );
}

// ── hook 4: the deny list is re-read ─────────────────────────────────────

/// A session token under the child's `ORDO_TOKEN_KEY`.
fn session_token_for(principal: &str, id: &ordo_identity::SessionId) -> String {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    let raw = unhex(TOKEN_KEY);
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&key),
        &ordo_identity::TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(now + 3_000),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: Some(ordo_identity::SessionClaim {
                id: id.clone(),
                started_at: Timestamp(now),
                second_step: None,
                via: None,
                upstream_presence: None,
            }),
        },
    )
}

/// A session token with presence proved a moment ago.
fn session_token_with_presence(principal: &str, id: &ordo_identity::SessionId) -> String {
    use ordo_identity::principal::{PrincipalId, Timestamp};
    let raw = unhex(TOKEN_KEY);
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    ordo_identity::mint(
        &ordo_identity::TokenKey::from_secret(&key),
        &ordo_identity::TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(now + 3_000),
            client: None,
            user_presence_at: Some(Timestamp(now)),
            capability: None,
            delegation: None,
            session: Some(ordo_identity::SessionClaim {
                id: id.clone(),
                started_at: Timestamp(now),
                second_step: None,
                via: None,
                upstream_presence: None,
            }),
        },
    )
}

/// **A second replica stops serving a session another one ended, without a
/// restart.** The deny list answers from memory, so a logout written by
/// another replica is invisible to this one until something re-reads the
/// table. Here the "other replica" is a second store on the same database,
/// and the assertion is that the running binary starts refusing the session
/// on its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_replica_stops_serving_an_ended_session_without_a_restart() {
    const DB: &str = "ordo_rt_host_sessions_refresh";
    custody_only(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_AUTH_METHODS", "password")
        .env("ORDO_WORKER_SESSIONS_REFRESH_TICK_MS", "200");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let id = ordo_identity::SessionId::mint();
    let bearer = session_token_for("usr_ana", &id);
    let auth = [("authorization", format!("Bearer {bearer}"))];
    let headers: Vec<(&str, &str)> = auth.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (status, body) = request(port, "POST", "/auth/refresh", &headers, "");
    assert_eq!(status, 200, "a live session refreshes: {body}");

    // Another replica ends it.
    let pool = Pool::connect(cfg(DB)).await.expect("pool");
    let other = ordo_host::sessions::PgSessionRevocations::open(pool)
        .await
        .expect("a second replica's deny list");
    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        + 3_600;
    ordo_identity::SessionRevocations::revoke(
        &other,
        &id,
        ordo_identity::principal::Timestamp(until),
    )
    .expect("revoked");

    let mut refused = false;
    for _ in 0..50 {
        let (status, _) = request(port, "POST", "/auth/refresh", &headers, "");
        if status == 401 {
            refused = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    kill(&mut child, "TERM");
    let out = child.wait_with_output().expect("wait");
    assert!(
        refused,
        "ten seconds after another replica ended the session, this one still served it"
    );
    let _ = out;
}

// ── the presence setting ─────────────────────────────────────────────────

/// A misspelt exemption is a refusal that names it and lists the ops that
/// exist, before any dial.
#[test]
fn an_exempt_op_that_names_nothing_refuses_the_boot_by_name() {
    let mut c = host("ordo_rt_never_dialled", free_port());
    c.env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432")
        .env(
            "ORDO_STEP_UP_EXEMPT_OPS",
            "std.space.admit,std.space.admitt",
        );
    let ran = refused(c);
    assert!(
        ran.stderr.contains("ORDO_STEP_UP_EXEMPT_OPS"),
        "{}",
        ran.stderr
    );
    assert!(ran.stderr.contains("std.space.admitt"), "{}", ran.stderr);
    assert!(ran.stderr.contains("std.space.callersOf"), "{}", ran.stderr);
}

/// The strict setting takes `on` or `off`; anything else names it.
#[test]
fn a_presence_on_reads_value_that_is_not_on_or_off_refuses_the_boot() {
    let mut c = host("ordo_rt_never_dialled", free_port());
    c.env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432")
        .env("ORDO_PRESENCE_ON_READS", "yes");
    let ran = refused(c);
    assert!(
        ran.stderr.contains("ORDO_PRESENCE_ON_READS"),
        "{}",
        ran.stderr
    );
}

/// **A read answers a signed-in person with no touch; a change asks**
/// (owner, 2026-10-22). Three boots of the same deployment: with nothing set
/// an owner with no fresh passkey assertion is answered `callersOf` (a read)
/// and refused `export` (a change). Strict (`ORDO_PRESENCE_ON_READS=on`)
/// refuses `callersOf` too. Strict with `callersOf` exempted answers it, and
/// `export`, which was not named, still asks.
#[tokio::test(flavor = "multi_thread")]
async fn reads_need_no_presence_strict_restores_it_and_a_named_op_stops_asking() {
    const DB: &str = "ordo_rt_host_presence";
    let _pool = deployment_ready(DB).await;
    let bundles = std::env::temp_dir().join(format!("ordo-rt-presence-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bundles);

    for (strict, exempt) in [(false, false), (true, false), (true, true)] {
        let port = free_port();
        let mut c = host(DB, port);
        c.env("ORDO_EXIT_BUNDLE_DIR", &bundles)
            .env_remove("ORDO_STEP_UP_EXEMPT_OPS")
            .env_remove("ORDO_PRESENCE_ON_READS");
        if strict {
            c.env("ORDO_PRESENCE_ON_READS", "on");
        }
        if exempt {
            c.env("ORDO_STEP_UP_EXEMPT_OPS", "std.space.callersOf");
        }
        let mut child = c.spawn().expect("spawn");
        wait_for_live(port, &mut child);

        let no_presence = format!("Bearer {}", token_for("mem_o", false));
        let (status, body) = request(
            port,
            "GET",
            "/op/std.space.callersOf?op=setMemo",
            &[("x-ordo-space", SPACE), ("authorization", &no_presence)],
            "",
        );
        assert_eq!(
            status,
            if strict && !exempt { 403 } else { 200 },
            "callersOf with no presence, strict={strict} exempt={exempt}: {body}"
        );
        let (status, body) = request(
            port,
            "POST",
            "/op/std.space.export",
            &[
                ("x-ordo-space", SPACE),
                ("authorization", &no_presence),
                ("content-type", "application/json"),
            ],
            &serde_json::json!({ "ciphertext": true }).to_string(),
        );
        assert_eq!(status, 403, "export was not named and still asks: {body}");

        kill(&mut child, "TERM");
        let out = child.wait_with_output().expect("wait");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if exempt {
            assert!(
                stdout.contains("presence    NOT required on: std.space.callersOf"),
                "{stdout}"
            );
        } else {
            assert!(
                stdout.contains("presence    required on every platform op that changes something"),
                "{stdout}"
            );
        }
        if strict {
            assert!(stdout.contains("required on reads too"), "{stdout}");
        } else {
            assert!(stdout.contains("not required on reads"), "{stdout}");
        }
    }
    let _ = std::fs::remove_dir_all(&bundles);
}

// ── the recovery routes are served by the binary ─────────────────────────

/// **A booted passkey deployment serves every recovery route and has wired
/// what they stand on.** The routes are on the platform router, not only on
/// the auth router the harnesses mount, and recovery has its stores: a session
/// with no presence is answered `403 presence_required` by the code route,
/// which an unwired one could not say (it would refuse with a `401`).
#[tokio::test(flavor = "multi_thread")]
async fn a_booted_deployment_serves_the_recovery_routes_and_has_wired_recovery() {
    const DB: &str = "ordo_rt_host_recovery_routes";
    custody_only(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_AUTH_METHODS", "password,passkey")
        .env("ORDO_APEX_DOMAIN", "example.com")
        .env("ORDO_AUTH_ORIGINS", "https://app.example.com");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let id = ordo_identity::SessionId::mint();
    let bearer = session_token_for("usr_ana", &id);
    let auth = [("authorization", format!("Bearer {bearer}"))];
    let headers: Vec<(&str, &str)> = auth.iter().map(|(k, v)| (*k, v.as_str())).collect();

    // Routed and wired: presence is what is missing, and it is said.
    let (status, body) = request(port, "POST", "/auth/recovery-codes", &headers, "");
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("presence_required"), "{body}");
    // Routed: a ticket that never existed is the one refusal, not a 404.
    let json_type = [("content-type", "application/json")];
    let junk = format!("{{\"ticket\":\"tkt_{}\"}}", "ab".repeat(32));
    for path in [
        "/auth/recover/begin",
        "/auth/recover/complete",
        "/auth/recover/code",
    ] {
        let (status, body) = request(port, "POST", path, &json_type, &junk);
        assert_eq!(status, 401, "{path}: {body}");
    }
    let (status, body) = request(port, "POST", "/auth/recover/contest", &json_type, "{}");
    assert_eq!(status, 401, "{body}");
    let (status, body) = request(port, "GET", "/auth/authenticators", &headers, "");
    assert_eq!(status, 200, "{body}");

    kill(&mut child, "TERM");
    let _ = child.wait_with_output().expect("wait");
}

/// **The deployment owner's recovery ops are registered on the binary and
/// wired to recovery.** A deployment owner with presence reaches the op and
/// is told the person does not exist (which an unregistered op, or one with
/// no state, could not say), and one with no presence is refused first.
#[tokio::test(flavor = "multi_thread")]
async fn a_booted_deployment_serves_the_deployment_owners_recovery_ops() {
    const DB: &str = "ordo_rt_host_recovery_ops";
    let _pool = deployment_ready(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_AUTH_METHODS", "password,passkey")
        .env("ORDO_APEX_DOMAIN", "example.com")
        .env("ORDO_AUTH_ORIGINS", "https://app.example.com");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    let body =
        serde_json::json!({"principal": "usr_nobody", "evidence": "host-break-glass"}).to_string();
    for (presence, want) in [(false, 403), (true, 404)] {
        let bearer = format!("Bearer {}", token_for("mem_o", presence));
        let (status, out) = request(
            port,
            "POST",
            "/op/std.identity.recover",
            &[
                ("x-ordo-space", SPACE),
                ("authorization", &bearer),
                ("content-type", "application/json"),
            ],
            &body,
        );
        assert_eq!(status, want, "presence={presence}: {out}");
        if presence {
            assert!(out.contains("no such principal"), "{out}");
        }
    }
    kill(&mut child, "TERM");
    let _ = child.wait_with_output().expect("wait");
}

// ── recovery's settings ──────────────────────────────────────────────────

/// **A quarantine longer than the contest window refuses the boot**, before
/// any dial and naming the variable. A person must be able to contest before
/// the block on step-up lifts, so a longer quarantine would let a recovery's
/// new passkey pass step-up while the person who lost it still cannot say no.
#[test]
fn a_quarantine_longer_than_the_contest_window_refuses_the_boot() {
    let mut c = host("ordo_rt_never_dialled", free_port());
    c.env("ORDO_PG_HOST", "203.0.113.1")
        .env("ORDO_PG_PORT", "5432")
        .env("ORDO_RECOVERY_QUARANTINE_SECS", "200000")
        .env("ORDO_RECOVERY_CONTEST_SECS", "100000");
    let ran = refused(c);
    assert!(
        ran.stderr.contains("ORDO_RECOVERY_QUARANTINE_SECS"),
        "{}",
        ran.stderr
    );
    assert!(ran.stderr.contains("contest window"), "{}", ran.stderr);
}

/// **A delay below its floor refuses the boot**, naming the floor. A delay of
/// nothing on a case the deployment owner opens through the door would make it
/// no stronger than the weakest proof behind it.
#[test]
fn a_delay_below_the_floor_refuses_the_boot() {
    for (var, value) in [
        ("ORDO_RECOVERY_OPERATOR_DELAY_SECS", "60"),
        ("ORDO_RECOVERY_QUARANTINE_SECS", "0"),
        ("ORDO_RECOVERY_SUCCESSION_DELAY_SECS", "10"),
    ] {
        let mut c = host("ordo_rt_never_dialled", free_port());
        c.env("ORDO_PG_HOST", "203.0.113.1")
            .env("ORDO_PG_PORT", "5432")
            .env(var, value);
        let ran = refused(c);
        assert!(ran.stderr.contains(var), "{var}: {}", ran.stderr);
        assert!(
            ran.stderr.contains("floor of 3600"),
            "{var}: {}",
            ran.stderr
        );
    }
}

/// A word that is not a path, an approvals count outside 1 to 10, and a value
/// that is not a number each refuse by name.
#[test]
fn a_bad_recovery_setting_refuses_the_boot_by_name() {
    for (var, value) in [
        ("ORDO_RECOVERY_PATHS", "code,carrier-pigeon"),
        ("ORDO_RECOVERY_OPERATOR_APPROVALS", "0"),
        ("ORDO_RECOVERY_TENANT_APPROVALS", "11"),
        ("ORDO_RECOVERY_HOST_DELAY_SECS", "soon"),
        ("ORDO_RECOVERY_HOLD_DEFAULT", "maybe"),
    ] {
        let mut c = host("ordo_rt_never_dialled", free_port());
        c.env("ORDO_PG_HOST", "203.0.113.1")
            .env("ORDO_PG_PORT", "5432")
            .env(var, value);
        let ran = refused(c);
        assert!(ran.stderr.contains(var), "{var}: {}", ran.stderr);
    }
}

/// Audit 33 (2026-10-09): a boot refusal for a number names the variable and
/// the default to unset it for, not only what is wrong.
#[test]
fn a_bad_number_or_a_zero_refuses_the_boot_with_the_default_to_use() {
    for (var, value) in [
        ("ORDO_PG_PORT", "five"),
        ("ORDO_PG_POOL_SIZE", "0"),
        ("ORDO_PG_ACQUIRE_TIMEOUT_MS", "soon"),
    ] {
        let mut c = host("ordo_rt_never_dialled", free_port());
        c.env("ORDO_PG_HOST", "203.0.113.1")
            .env("ORDO_PG_PORT", "5432")
            .env(var, value);
        let ran = refused(c);
        assert!(ran.stderr.contains(var), "{var}={value}: {}", ran.stderr);
        assert!(
            ran.stderr.contains("unset it for the default"),
            "{var}={value}: {}",
            ran.stderr
        );
    }
}

// ── the second step's settings ───────────────────────────────────────────

/// **An unknown second-step word refuses the boot**, by name, before any dial.
/// A setting that quietly became `enabled` is a security setting nobody chose,
/// and so is a number that is not one.
#[test]
fn an_unknown_second_factor_word_refuses_the_boot() {
    for (var, value) in [
        ("ORDO_AUTH_SECOND_FACTOR", "sometimes"),
        ("ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER", "many"),
        ("ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER", "0"),
        ("ORDO_AUTH_SECOND_STEP_ATTEMPTS", "0"),
        ("ORDO_AUTH_SECOND_STEP_SECS", "5"),
        ("ORDO_AUTH_REAUTH_SECS", "soon"),
    ] {
        let mut c = host("ordo_rt_never_dialled", free_port());
        c.env("ORDO_PG_HOST", "203.0.113.1")
            .env("ORDO_PG_PORT", "5432")
            .env(var, value);
        let ran = refused(c);
        assert!(ran.stderr.contains(var), "{var}={value}: {}", ran.stderr);
    }
}

/// **A booted deployment serves the second-step routes and has wired them.**
/// The routes are on the platform router the binary serves and not only on the
/// auth router the harnesses mount (the lesson recovery's R3 taught), and a
/// code-generating factor's secret is sealed under the deployment's own key
/// custody: begin answers a URI, which it could not if any of that were
/// unwired.
#[tokio::test(flavor = "multi_thread")]
async fn a_booted_deployment_serves_the_second_step_routes_and_has_wired_them() {
    const DB: &str = "ordo_rt_host_second_step";
    custody_only(DB).await;
    let port = free_port();
    let mut c = host(DB, port);
    c.env("ORDO_AUTH_METHODS", "password,passkey")
        .env("ORDO_APEX_DOMAIN", "example.com")
        .env("ORDO_AUTH_ORIGINS", "https://app.example.com");
    let mut child = c.spawn().expect("spawn");
    wait_for_live(port, &mut child);

    // A session with fresh presence, the proof a change to a factor needs.
    let id = ordo_identity::SessionId::mint();
    let bearer = session_token_with_presence("usr_ana", &id);
    let auth = [("authorization", format!("Bearer {bearer}"))];
    let headers: Vec<(&str, &str)> = auth.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let json_type = [("content-type", "application/json")];

    // Wired: a session enrols, and the answer is the person's app's to read.
    let mut both: Vec<(&str, &str)> = headers.clone();
    both.extend(json_type);
    let (status, body) = request(port, "POST", "/auth/factors/totp/begin", &both, "{}");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("otpauth://totp/"), "{body}");
    let (status, body) = request(port, "POST", "/auth/factors/key/begin", &both, "{}");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"resident_key\":\"discouraged\""), "{body}");
    let (status, body) = request(port, "GET", "/auth/factors", &headers, "");
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("\"factors\":[]"),
        "nothing is enrolled yet: {body}"
    );
    // Without presence a session is told to prove a first factor again.
    let plain = session_token_for("usr_ana", &id);
    let plain_auth = [("authorization", format!("Bearer {plain}"))];
    let mut plain_both: Vec<(&str, &str)> =
        plain_auth.iter().map(|(k, v)| (*k, v.as_str())).collect();
    plain_both.extend(json_type);
    let (status, body) = request(port, "POST", "/auth/factors/totp/begin", &plain_both, "{}");
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("reauthentication_required"), "{body}");
    // A bearer is needed: nothing enrols without one.
    let (status, _) = request(port, "POST", "/auth/factors/totp/begin", &json_type, "{}");
    assert_eq!(status, 401);
    // Routed: a handle nobody minted is the one refusal, not a 404.
    let junk = format!(
        "{{\"handle\":\"ss_nothing\",\"binder\":\"{}\",\"kind\":\"totp\",\"code\":\"000000\"}}",
        "0".repeat(64)
    );
    for path in ["/auth/second-step", "/auth/second-step/begin"] {
        let (status, body) = request(port, "POST", path, &json_type, &junk);
        assert_eq!(status, 401, "{path}: {body}");
    }

    kill(&mut child, "TERM");
    let _ = child.wait_with_output().expect("wait");
}
