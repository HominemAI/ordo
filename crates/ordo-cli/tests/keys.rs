//! R2b — the key ceremony, run as commands.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::path::PathBuf;

mod support;
use support::operator;

const PASS: &str = "correct horse battery staple";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ordo-keys-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).expect("tmp");
    d
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn passfile(d: &std::path::Path, name: &str, text: &str) -> String {
    let p = d.join(name);
    std::fs::write(&p, text).expect("write");
    p.display().to_string()
}

/// S6-009: the env master is development only for the operator commands too.
/// `ordo keys init` mints wrapped keys that reach backups, so with no file
/// flags and no development flag it refuses before it dials, and says what
/// to use instead.
#[test]
fn the_env_master_is_refused_by_ordo_keys_outside_development() {
    std::env::remove_var("ORDO_DEV_UNSAFE_TEST_KEYS");
    let out = operator::run(
        "keys",
        &args(&["init", "--space", "acme", "--as", "adam@example.com"]),
    );
    assert_ne!(out.code, 0, "{}", out.report);
    for needle in ["ORDO_MASTER_KEY", "sealed-master", "seal-master"] {
        assert!(out.report.contains(needle), "{needle}: {}", out.report);
    }
}

/// The whole ceremony, in the order an operator performs it: seal a master,
/// initialise a space, and prove the deployment's own custody opens under
/// it.
#[test]
fn seal_master_then_init_provisions_a_space() {
    let d = dir("ceremony");
    let master = d.join("master.key");
    let _ = std::fs::remove_file(&master);
    let pass = passfile(&d, "pass", PASS);

    let out = operator::run(
        "keys",
        &args(&[
            "seal-master",
            "--out",
            &master.display().to_string(),
            "--key-id",
            "acme-1",
            "--passphrase-file",
            &pass,
        ]),
    );
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("key-id  acme-1"), "{}", out.report);
    assert!(out.report.contains("fp      "), "{}", out.report);
    // The ceremony never prints the key, and it always prints the loss
    // table — an operator who is told once and acts on it is worth more
    // than a paragraph nobody finishes.
    assert!(!out.report.contains(PASS), "{}", out.report);
    assert!(out.report.contains("no escrow"), "{}", out.report);

    // A second seal over the same path is refused: clobbering a master key
    // file erases every space's custody.
    let again = operator::run(
        "keys",
        &args(&[
            "seal-master",
            "--out",
            &master.display().to_string(),
            "--passphrase-file",
            &pass,
        ]),
    );
    assert_eq!(again.code, 1, "{}", again.report);
    assert!(
        again.report.contains("never overwritten"),
        "{}",
        again.report
    );

    // A too-short passphrase is refused with the reason, because there is
    // one derivation between it and every space's keys.
    let weak = passfile(&d, "weak", "short");
    let refused = operator::run(
        "keys",
        &args(&[
            "seal-master",
            "--out",
            &d.join("weak.key").display().to_string(),
            "--passphrase-file",
            &weak,
        ]),
    );
    assert_eq!(refused.code, 1, "{}", refused.report);
    assert!(refused.report.contains("argon2id"), "{}", refused.report);

    // `init`, twice: idempotent, and the second run says nothing was
    // minted rather than reporting success indistinguishable from the
    // first.
    let db = "ordo_rt_keys_ceremony";
    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = ordo_conformance::Pg {
        db: db.into(),
        ..ordo_conformance::Pg::default()
    };
    pg.exec(r#"DROP TABLE IF EXISTS "$keycustody""#).ok();

    let init = |space: &str| {
        operator::run(
            "keys",
            &args(&[
                "init",
                "--space",
                space,
                "--as",
                "user:ada",
                "--db",
                db,
                "--master-key-file",
                &master.display().to_string(),
                "--passphrase-file",
                &pass,
            ]),
        )
    };
    let first = init("acme");
    assert_eq!(first.code, 0, "{}", first.report);
    assert!(
        first.report.contains("status  provisioned"),
        "{}",
        first.report
    );
    assert!(first.report.contains("epoch   1"), "{}", first.report);
    // A decision about a space belongs in the security-event stream.
    assert!(
        first.report.contains("audit   recorded"),
        "{}",
        first.report
    );

    let second = init("acme");
    assert_eq!(second.code, 0, "{}", second.report);
    assert!(
        second.report.contains("already provisioned"),
        "a second init must not read as a second tenant: {}",
        second.report
    );

    // The custody is real: it opens under the same sealed file and under
    // nothing else.
    let m = ordo_crypto::master::open_sealed_master(&master, PASS.as_bytes()).expect("opens");
    let store = ordo_exec::keystore::PgKeyStore::open(ordo_exec::PoolConfig {
        db: db.into(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    })
    .expect("store");
    let provider = ordo_crypto::LocalKeyProvider::from_master(m, Box::new(store));
    let space = ordo_crypto::SpaceId("acme".into());
    ordo_crypto::KeyProvider::blind_index(
        &provider,
        &space,
        "$custody",
        "probe",
        ordo_crypto::BlindValue::Null,
    )
    .expect("the ceremony's master opens the ceremony's custody");

    std::fs::remove_dir_all(&d).ok();
}

/// `rewrap-master` moves the wrapping and nothing else: the space's SMK is
/// unchanged, so every blind-index digest is unchanged, which is the whole
/// difference between a re-wrap and an SMK rotation.
#[test]
fn rewrap_master_moves_the_wrapping_and_no_digest() {
    let d = dir("rewrap");
    let old = d.join("old.key");
    let new = d.join("new.key");
    let _ = std::fs::remove_file(&old);
    let _ = std::fs::remove_file(&new);
    let old_pass = passfile(&d, "old-pass", PASS);
    let new_pass = passfile(&d, "new-pass", "a different passphrase entirely");

    for (path, p, id) in [(&old, &old_pass, "old"), (&new, &new_pass, "new")] {
        let out = operator::run(
            "keys",
            &args(&[
                "seal-master",
                "--out",
                &path.display().to_string(),
                "--key-id",
                id,
                "--passphrase-file",
                p,
            ]),
        );
        assert_eq!(out.code, 0, "{}", out.report);
    }

    let db = "ordo_rt_keys_rewrap";
    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = ordo_conformance::Pg {
        db: db.into(),
        ..ordo_conformance::Pg::default()
    };
    pg.exec(r#"DROP TABLE IF EXISTS "$keycustody""#).ok();

    for space in ["acme", "beta"] {
        let out = operator::run(
            "keys",
            &args(&[
                "init",
                "--space",
                space,
                "--as",
                "user:ada",
                "--db",
                db,
                "--master-key-file",
                &old.display().to_string(),
                "--passphrase-file",
                &old_pass,
            ]),
        );
        assert_eq!(out.code, 0, "{}", out.report);
    }

    let cfg = || ordo_exec::PoolConfig {
        db: db.into(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    };
    let digest = |m: [u8; 32], space: &str| {
        let store = ordo_exec::keystore::PgKeyStore::open(cfg()).expect("store");
        let p = ordo_crypto::LocalKeyProvider::from_master(m, Box::new(store));
        ordo_crypto::KeyProvider::blind_index(
            &p,
            &ordo_crypto::SpaceId(space.into()),
            "customers",
            "email",
            ordo_crypto::BlindValue::Text("ada@example.com"),
        )
        .expect("digest")
    };
    let old_m = ordo_crypto::master::open_sealed_master(&old, PASS.as_bytes()).expect("opens");
    let new_m = ordo_crypto::master::open_sealed_master(&new, b"a different passphrase entirely")
        .expect("opens");
    let before = digest(old_m, "acme");

    // A dry run does every unwrap and re-wrap and rolls back, so an
    // operator finds out the old passphrase is wrong before anything moves.
    let dry = operator::run(
        "keys",
        &args(&[
            "rewrap-master",
            "--from",
            &old.display().to_string(),
            "--from-passphrase-file",
            &old_pass,
            "--to",
            &new.display().to_string(),
            "--to-passphrase-file",
            &new_pass,
            "--as",
            "user:ada",
            "--db",
            db,
            "--dry-run",
        ]),
    );
    assert_eq!(dry.code, 0, "{}", dry.report);
    assert!(dry.report.contains("would re-wrap  2"), "{}", dry.report);
    // Nothing moved: the old master still opens.
    assert_eq!(digest(old_m, "acme"), before, "a dry run moved rows");

    let real = operator::run(
        "keys",
        &args(&[
            "rewrap-master",
            "--from",
            &old.display().to_string(),
            "--from-passphrase-file",
            &old_pass,
            "--to",
            &new.display().to_string(),
            "--to-passphrase-file",
            &new_pass,
            "--as",
            "user:ada",
            "--db",
            db,
        ]),
    );
    assert_eq!(real.code, 0, "{}", real.report);
    assert!(real.report.contains("re-wrapped  2"), "{}", real.report);
    assert!(
        real.report.contains("not an SMK rotation"),
        "the report must not let the two be confused: {}",
        real.report
    );

    // **The digest did not move.** BIKs are SMK-derived and the SMK is
    // unchanged, so every `sealed indexed` column still addresses its rows.
    // If this ever fails, a re-wrap has become a re-index and the cost
    // model in the docs is wrong by orders of magnitude.
    assert_eq!(digest(new_m, "acme"), before);
    // And the old master no longer opens anything.
    let store = ordo_exec::keystore::PgKeyStore::open(cfg()).expect("store");
    let stale = ordo_crypto::LocalKeyProvider::from_master(old_m, Box::new(store));
    assert!(
        ordo_crypto::KeyProvider::blind_index(
            &stale,
            &ordo_crypto::SpaceId("acme".into()),
            "customers",
            "email",
            ordo_crypto::BlindValue::Text("ada@example.com"),
        )
        .is_err(),
        "the retired master still opens custody"
    );

    // Re-wrapping to the same key is refused: a no-op that looks like it
    // worked is the worst answer available.
    let same = operator::run(
        "keys",
        &args(&[
            "rewrap-master",
            "--from",
            &new.display().to_string(),
            "--from-passphrase-file",
            &new_pass,
            "--to",
            &new.display().to_string(),
            "--to-passphrase-file",
            &new_pass,
            "--as",
            "user:ada",
            "--db",
            db,
        ]),
    );
    assert_eq!(same.code, 1, "{}", same.report);
    assert!(same.report.contains("no-op"), "{}", same.report);

    std::fs::remove_dir_all(&d).ok();
}

/// K2 and K3 as commands. One database, one story, in the order an
/// operator lives it: a payload sealed under epoch 1; `rotate` mints 2 and
/// enqueues the pass; `status` shows it; `destroy 1` refuses by name with
/// the count; the pass runs; `destroy` without the token prints the
/// consequence and stops; a token for the wrong epoch refuses; the right
/// token destroys, and the envelope captured before the pass no longer
/// opens; `reseal` afterwards has nothing to do.
#[test]
fn rotate_status_and_destroy_walk_the_epoch_lifecycle() {
    let d = dir("lifecycle");
    let master = d.join("master.key");
    let _ = std::fs::remove_file(&master);
    let pass = passfile(&d, "pass", PASS);
    let out = operator::run(
        "keys",
        &args(&[
            "seal-master",
            "--out",
            &master.display().to_string(),
            "--passphrase-file",
            &pass,
        ]),
    );
    assert_eq!(out.code, 0, "{}", out.report);

    let db = "ordo_rt_keys_lifecycle";
    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = ordo_conformance::Pg {
        db: db.into(),
        ..ordo_conformance::Pg::default()
    };
    for t in [
        "$keycustody",
        "$reseal",
        "$sealedpayload",
        "$artifacts",
        "$activations",
    ] {
        pg.exec(&format!(r#"DROP TABLE IF EXISTS "{t}""#)).ok();
    }

    let keys = |sub: &str, extra: &[&str]| {
        let mut v = vec![
            sub,
            "--space",
            "acme",
            "--as",
            "user:ada",
            "--db",
            db,
            "--master-key-file",
        ];
        let m = master.display().to_string();
        v.push(&m);
        v.push("--passphrase-file");
        v.push(&pass);
        v.extend_from_slice(extra);
        operator::run("keys", &args(&v))
    };
    let init = keys("init", &[]);
    assert_eq!(init.code, 0, "{}", init.report);

    // One payload under epoch 1, written the way the write protocol writes
    // one, through the ceremony's own custody.
    let cfg = || ordo_exec::PoolConfig {
        db: db.into(),
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    };
    let m = ordo_crypto::master::open_sealed_master(&master, PASS.as_bytes()).expect("opens");
    let provider = || {
        let store = ordo_exec::keystore::PgKeyStore::open(cfg()).expect("store");
        ordo_crypto::LocalKeyProvider::from_master(m, Box::new(store))
    };
    let space = ordo_crypto::SpaceId("acme".into());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let pool = rt.block_on(ordo_exec::Pool::connect(cfg())).expect("pool");
    rt.block_on(pool.ensure_platform_tables()).expect("tables");
    let p = provider();
    let lease = ordo_crypto::KeyProvider::release(&p, &space, ordo_crypto::Purpose::Write, "seed")
        .expect("lease");
    let handle = ordo_crypto::KeyProvider::mint_sealed_ref(&p, &lease, 1).expect("handle");
    let captured = ordo_crypto::KeyProvider::seal_at(
        &p,
        &lease,
        "orders",
        "memo",
        ordo_crypto::Anchor::Row("ord_1"),
        b"exposed",
    )
    .expect("seal");
    rt.block_on(pool.rows(&ordo_crypto::sealed::insert_sql(
        &space,
        &handle,
        1,
        "orders",
        "memo",
        "row:ord_1",
        "cg_seed",
        &captured,
    )))
    .expect("seeded");
    drop(p);

    // The current epoch cannot be destroyed, by name.
    let current = keys("destroy", &["--epoch", "1"]);
    assert_eq!(current.code, 1, "{}", current.report);
    assert!(current.report.contains("rotate"), "{}", current.report);

    let rotated = keys("rotate", &["--reason", "exposure"]);
    assert_eq!(rotated.code, 0, "{}", rotated.report);
    assert!(
        rotated.report.contains("rotated  1 -> 2"),
        "{}",
        rotated.report
    );
    assert!(rotated.report.contains("payloads 1"), "{}", rotated.report);
    assert!(
        rotated.report.contains("audit    recorded"),
        "{}",
        rotated.report
    );

    let status = keys("status", &[]);
    assert_eq!(status.code, 0, "{}", status.report);
    assert!(
        status.report.contains("current  epoch 2"),
        "{}",
        status.report
    );
    assert!(
        status.report.contains("pass pending on payloads"),
        "{}",
        status.report
    );
    assert!(status.report.contains("payloads"), "{}", status.report);

    // Refused by name, with the count.
    let refused = keys("destroy", &["--epoch", "1", "--confirm", "acme:1"]);
    assert_eq!(refused.code, 1, "{}", refused.report);
    assert!(
        refused
            .report
            .contains("refused: 1 payloads still under epoch 1"),
        "{}",
        refused.report
    );

    // A second `reseal` while the pass is open enqueues nothing new.
    let again = keys("reseal", &[]);
    assert_eq!(again.code, 0, "{}", again.report);
    assert!(again.report.contains("already open"), "{}", again.report);

    // The pass, driven here the way the worker drives it.
    let p = provider();
    let schema = ordo_schema::build(&ordo_syntax::parse_program("").expect("parses"))
        .expect("an empty schema");
    let engine = ordo_exec::reseal::Reseal {
        pool: &pool,
        keys: &p,
        schema: &schema,
        space: &space,
        chunk: 8,
        stores: None,
    };
    let mut completed = false;
    for _ in 0..32 {
        match rt.block_on(engine.step(1)).expect("a step") {
            ordo_exec::reseal::Step::Idle => break,
            ordo_exec::reseal::Step::Completed { .. } => completed = true,
            _ => {}
        }
    }
    assert!(completed);
    drop(p);

    // The consequence, then the token.
    let unconfirmed = keys("destroy", &["--epoch", "1"]);
    assert_eq!(unconfirmed.code, 1, "{}", unconfirmed.report);
    assert!(
        unconfirmed
            .report
            .contains("backups and archived log bundles taken before"),
        "{}",
        unconfirmed.report
    );
    assert!(
        unconfirmed.report.contains("--confirm acme:1"),
        "{}",
        unconfirmed.report
    );

    let wrong = keys("destroy", &["--epoch", "1", "--confirm", "acme:2"]);
    assert_eq!(wrong.code, 1, "{}", wrong.report);
    assert!(
        wrong.report.contains("does not name this space and epoch"),
        "{}",
        wrong.report
    );

    let destroyed = keys("destroy", &["--epoch", "1", "--confirm", "acme:1"]);
    assert_eq!(destroyed.code, 0, "{}", destroyed.report);
    assert!(
        destroyed.report.contains("destroyed epoch 1"),
        "{}",
        destroyed.report
    );
    assert!(
        destroyed.report.contains("audit     recorded"),
        "{}",
        destroyed.report
    );

    // The envelope captured before the pass refuses under a fresh provider;
    // the live payload still opens.
    let p = provider();
    let lease = ordo_crypto::KeyProvider::release(&p, &space, ordo_crypto::Purpose::Read, "check")
        .expect("lease");
    assert_eq!(
        ordo_crypto::KeyProvider::open_at(
            &p,
            &lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("ord_1"),
            &captured
        ),
        Err(ordo_crypto::KeyError::UnknownEpoch(1))
    );
    let row = rt
        .block_on(pool.rows(&ordo_crypto::sealed::lookup_sql(&space, &handle)))
        .expect("row");
    let stored = ordo_crypto::sealed::payload_of(&row[0]).expect("a payload");
    assert_eq!(stored.epoch, 2);
    assert_eq!(
        stored.anchor, "row:ord_1",
        "the payload kept the row's label"
    );
    assert_eq!(
        ordo_crypto::KeyProvider::open_at(
            &p,
            &lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::parse(&stored.anchor).expect("an anchor"),
            &stored.bytes
        )
        .expect("opens"),
        b"exposed"
    );

    let status = keys("status", &[]);
    assert_eq!(status.code, 0, "{}", status.report);
    let epochs = status.report.split("PASSES").next().unwrap_or_default();
    assert!(
        !epochs.contains("\n  1 "),
        "epoch 1 is gone from the epoch table: {}",
        status.report
    );
    let nothing = keys("reseal", &[]);
    assert_eq!(nothing.code, 0, "{}", nothing.report);
    assert!(
        nothing.report.contains("nothing to re-seal"),
        "{}",
        nothing.report
    );

    std::fs::remove_dir_all(&d).ok();
}
