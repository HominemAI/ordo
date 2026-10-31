//! `ordo identity recover`: the deployment owner's way back in, run as a
//! command against a real database.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

mod support;
use ordo_identity::{AuthenticatorId, DirectoryStore, PrincipalId, PrincipalKind, Timestamp};
use support::operator;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// A database with the directory's tables and one person in it.
fn deployment(db: &str) -> (ordo_exec::PoolConfig, String) {
    let pg = ordo_conformance::Pg::named(db);
    let fixture = ordo_conformance::Fixture::load().expect("fixture");
    fixture.reset(&pg).expect("seeded");
    let cfg = ordo_exec::PoolConfig {
        db: db.to_string(),
        size: 1,
        ..ordo_exec::PoolConfig::from_env()
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let pool = rt
        .block_on(ordo_exec::Pool::connect(cfg.clone()))
        .expect("pool");
    rt.block_on(ordo_host::principal_directory::ensure_table(&pool))
        .expect("directory tables");
    let _ = rt.block_on(pool.rows("DELETE FROM \"$authenticator\""));
    let _ = rt.block_on(pool.rows("DELETE FROM \"$principal\""));
    let _ = rt.block_on(pool.rows("DELETE FROM \"$recovery\""));
    drop(pool);
    let store = ordo_host::principal_directory::PgDirectoryStore::open(cfg.clone()).expect("store");
    let id = store
        .create_principal(PrincipalKind::Human, "Ana", Timestamp(1))
        .expect("minted");
    let p = store.principal(&id).expect("read").expect("exists");
    store
        .insert_link(&AuthenticatorId("passkey:lost".into()), &p, Timestamp(1))
        .expect("linked");
    (cfg, id.0)
}

#[test]
fn the_host_opens_a_recovery_and_prints_its_ticket_once() {
    let db = "ordo_cli_identity";
    let (_, principal) = deployment(db);

    let out = operator::run(
        "identity",
        &args(&[
            "recover",
            "--principal",
            &principal,
            "--as",
            "adam",
            "--ref",
            "TICKET-7",
            "--db",
            db,
        ]),
    );
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("recovery case rec_"), "{}", out.report);
    assert!(
        out.report.contains("tkt_"),
        "the ticket is printed: {}",
        out.report
    );
    assert!(
        out.report.contains("evidence: host-break-glass"),
        "{}",
        out.report
    );
    assert!(
        out.report.contains("recorded: 2 entries"),
        "the case and its ticket are recorded: {}",
        out.report
    );

    // One live case per person: a second is refused, and prints no ticket.
    let again = operator::run(
        "identity",
        &args(&[
            "recover",
            "--principal",
            &principal,
            "--as",
            "adam",
            "--db",
            db,
        ]),
    );
    assert_eq!(again.code, 1, "{}", again.report);
    assert!(
        again.report.contains("already has a recovery in progress"),
        "{}",
        again.report
    );
    assert!(!again.report.contains("tkt_"), "{}", again.report);
}

#[test]
fn the_host_refuses_a_stranger_and_an_evidence_class_off_the_list() {
    let db = "ordo_cli_identity_refusals";
    let (_, _) = deployment(db);
    let out = operator::run(
        "identity",
        &args(&[
            "recover",
            "--principal",
            "usr_nobody",
            "--as",
            "adam",
            "--db",
            db,
        ]),
    );
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(out.report.contains("no principal"), "{}", out.report);

    let out = operator::run(
        "identity",
        &args(&[
            "recover",
            "--principal",
            "usr_nobody",
            "--as",
            "adam",
            "--evidence",
            "he-said-so",
            "--db",
            db,
        ]),
    );
    assert_eq!(out.code, 2, "usage: {}", out.report);
    assert!(out.report.contains("host-break-glass"), "{}", out.report);

    let out = operator::run("identity", &args(&["recover", "--as", "adam", "--db", db]));
    assert_eq!(out.code, 2, "{}", out.report);
}

// ── R8: reconciliation ───────────────────────────────────────────────────

/// Append one event to the deployment's stream, as the drain does.
fn append(db: &str, kind: ordo_secevent::EventKind, subject: ordo_secevent::Subject) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let pool = ordo_exec::Pool::connect(ordo_exec::PoolConfig {
            db: db.to_string(),
            size: 1,
            ..ordo_exec::PoolConfig::from_env()
        })
        .await
        .expect("pool");
        let recorder = ordo_secevent::Recorder::default();
        recorder.observe(
            ordo_secevent::Observation {
                space: None,
                subject,
                op: "test".into(),
                kind,
                at: ordo_commandlog::Timestamp(10),
                correlation: ordo_secevent::correlation::mint(),
            },
            ordo_secevent::Relationship::None,
        );
        let tx = pool.begin().await.expect("tx");
        for e in recorder.drain_all(ordo_commandlog::Timestamp(10)) {
            tx.append_security_entry(&e).await.expect("appended");
        }
        tx.commit().await.expect("committed");
    });
}

fn link(cfg: &ordo_exec::PoolConfig, principal: &str, authenticator: &str, at: u64) {
    let store = ordo_host::principal_directory::PgDirectoryStore::open(cfg.clone()).expect("store");
    let p = store
        .principal(&PrincipalId(principal.to_string()))
        .expect("read")
        .expect("exists");
    store
        .insert_link(
            &AuthenticatorId(authenticator.to_string()),
            &p,
            Timestamp(at),
        )
        .expect("linked");
}

fn linked_event(added: &str) -> ordo_secevent::EventKind {
    ordo_secevent::EventKind::AuthenticatorLinked {
        added: added.to_string(),
        vouched_by: None,
    }
}

/// **A link row inserted by hand is reported.** Nothing that can write the
/// directory table can be stopped from writing one, so the design makes it
/// detectable: every way a link comes to exist writes an event, and a link no
/// event explains is named, with its principal, and the run exits 3.
#[test]
fn a_link_row_inserted_by_hand_is_reported() {
    let db = "ordo_cli_identity_reconcile";
    let (cfg, ana) = deployment(db);
    // The fixture's one link (`passkey:lost`) has no event yet. Explain it as
    // a sign-up does, then add one nobody explains.
    append(
        db,
        linked_event("passkey:lost"),
        ordo_secevent::Subject::member(&ana),
    );
    link(&cfg, &ana, "passkey:handmade", 50);

    let run = || operator::run("verify", &args(&["--identity", "--db", db]));
    let out = run();
    assert_eq!(out.code, 3, "{}", out.report);
    assert!(out.report.contains("passkey:handmade"), "{}", out.report);
    assert!(
        out.report.contains(&ana),
        "names the principal: {}",
        out.report
    );
    assert!(
        !out.report.contains("passkey:lost  principal"),
        "the explained link is not listed: {}",
        out.report
    );
    assert!(out.report.contains("FAILED"), "{}", out.report);
    assert!(
        out.report.contains("VERIFIED"),
        "the stream's own chain is verified beside it: {}",
        out.report
    );

    // Once an event explains it, the run is clean.
    append(
        db,
        linked_event("passkey:handmade"),
        ordo_secevent::Subject::member(&ana),
    );
    let out = run();
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("every link has a record in the stream"),
        "{}",
        out.report
    );
}

/// What explains a link is each way one is made, and no more: a recovery, a
/// password given, and an onboarding that explains **one** link of a principal
/// and not a second one attached to it.
#[test]
fn every_way_a_link_is_made_explains_it_and_an_onboarding_explains_one() {
    let db = "ordo_cli_identity_reconcile_ways";
    let (cfg, ana) = deployment(db);
    let store = ordo_host::principal_directory::PgDirectoryStore::open(cfg.clone()).expect("store");
    let bob = store
        .create_principal(PrincipalKind::Human, "Bob", Timestamp(2))
        .expect("minted")
        .0;
    // Ana's fixture link is explained by a recovery that added it.
    append(
        db,
        ordo_secevent::EventKind::Recovery {
            act: "completed".into(),
            detail: format!(
                "case=rec_1 principal={ana} added=passkey:lost retired=- contest-until=9"
            ),
        },
        ordo_secevent::Subject::member(&ana),
    );
    // A password given to Ana explains her password link.
    link(&cfg, &ana, "password:ana@example.test", 20);
    append(
        db,
        ordo_secevent::EventKind::Enrolment {
            act: "credential-set".into(),
            detail: format!("principal={ana} method=password by=self"),
        },
        ordo_secevent::Subject::member(&ana),
    );
    // Bob was onboarded, and has two links. The onboarding explains the first.
    link(&cfg, &bob, "passkey:bob-first", 30);
    link(&cfg, &bob, "passkey:bob-attached", 40);
    append(
        db,
        ordo_secevent::EventKind::PrincipalOnboarded,
        ordo_secevent::Subject::member(&bob),
    );

    let out = operator::run("verify", &args(&["--identity", "--db", db]));
    assert_eq!(out.code, 3, "{}", out.report);
    assert!(
        out.report.contains("passkey:bob-attached"),
        "{}",
        out.report
    );
    for explained in [
        "passkey:lost",
        "password:ana@example.test",
        "passkey:bob-first",
    ] {
        assert!(
            !out.report.contains(&format!("{explained}  principal")),
            "{explained} is explained: {}",
            out.report
        );
    }
    assert!(
        out.report.contains("1 link with no record"),
        "exactly one: {}",
        out.report
    );
}

/// `--identity` is the deployment's, so a space is a usage error.
#[test]
fn identity_reconciliation_takes_no_space() {
    let out = operator::run(
        "verify",
        &args(&[
            "--identity",
            "--space",
            "spc_1",
            "--db",
            "ordo_cli_identity",
        ]),
    );
    assert_eq!(out.code, 2, "{}", out.report);
}
