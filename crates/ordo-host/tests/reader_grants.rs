//! `reader <name> reads <table>, …` and the grants it makes (owner ruling
//! 2026-11-18, `example-apps-notes-v2-plus-note_viewer` ask 17): consent is
//! enforced by Postgres.
//!
//! Two real roles in one database. The writer (`ordo_rg_writer`) owns schema
//! `alpha` and is the app being read. The reader (`ordo_rg_reader`) owns
//! schema `beta` and reads `alpha` through `peer journal`.
//!
//! What it proves:
//!
//! - A granted table reads. A table of the writer that is not on the line is
//!   refused BY POSTGRES when the reader queries it directly, so the grant is
//!   the control and Ordo's own check is not. The sign-in tables are granted
//!   by column only.
//! - The reader refuses at start with no grant, and the refusal quotes the
//!   `reader` line to add and names `ORDO_READER_<NAME>_ROLE`.
//! - A table taken off the line is revoked, and so is a whole line.
//! - A table an apply creates is granted by that same apply.
//! - A role that reads the peer with no grant (a superuser, the owner of the
//!   peer's schema) is refused, and starts with `ORDO_PEER_SHARED_ROLE=on`.
//! - A `reader` line with no role setting refuses, naming it. So do a role
//!   the server does not have and this deployment's own role.
//!
//! A regression looks like a reader that can read a table the writer did not
//! list, a reader that starts without a grant, or a grant left behind after
//! its line is gone.
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_rgrant_`. The two roles are cluster-wide and made once.

use std::collections::BTreeSet;

use ordo_conformance::{Fixture, Pg, SEED};
use ordo_exec::peer::{self, SchemaRefusal};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{self as sa, Program};

const WRITER: &str = "ordo_rg_writer";
const READER: &str = "ordo_rg_reader";
/// Both roles' password, for a cluster that asks for one.
const PASSWORD: &str = "ordo_rg_password";

/// The app being read: notes, and a table it does not share.
const ALPHA: &str = r#"
entity notes {
  title: text
  owner -> members?
  access read owner == $caller
  access write owner == $caller
}

entity secrets {
  body: text
}
"#;

/// The reader: the writer's notes and secrets through a peer, `$caller` on
/// the notes.
const BETA: &str = r#"
peer journal

external entries from journal.notes shared {
  id: text <- id
  title: text <- title
  owner: text <- owner
  access read owner == $caller
}

external vault from journal.secrets shared {
  id: text <- id
  body: text <- body
  access read open
}
"#;

fn cfg(db: &str, user: &str, schema: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        user: user.into(),
        size: 2,
        schema: schema.into(),
        // CI's cluster asks for a password; the local one trusts.
        password: Some(PASSWORD.into()),
        ..PoolConfig::from_env()
    }
}

/// The writer's schema with these reader lines.
fn alpha_with(readers: &str) -> ordo_schema::Schema {
    Program::compile(&format!("{readers}\n{ALPHA}"))
        .expect("A compiles")
        .schema
}

fn lookup(var: &str) -> Option<String> {
    (var == "ORDO_READER_VIEWER_ROLE").then(|| READER.to_string())
}

/// The reader's program, its peer resolved to `alpha`.
fn beta() -> (ordo_schema::Schema, BTreeSet<String>) {
    let mut f = Fixture::from_source(BETA, SEED).expect("B compiles");
    peer::resolve_with(&mut f.schema, "beta", |v| {
        (v == "ORDO_PEER_JOURNAL_SCHEMA").then(|| "alpha".to_string())
    })
    .expect("the peer resolves");
    let callers = peer::callers(&f.schema, &f.registry);
    assert!(callers.contains("journal"), "B uses $caller on journal");
    (f.schema, callers)
}

struct World {
    pg: Pg,
    /// The writer, in `alpha`.
    a: Pool,
    /// The reader, in `beta`.
    b: Pool,
}

/// Both roles, both schemas, the writer's tables applied and its sign-in
/// tables made as a start makes them. No grant yet.
async fn world(db: &str) -> World {
    let pg = Pg::scoped(db);
    for role in [WRITER, READER] {
        pg.exec(&format!(
            "DO $$ BEGIN CREATE ROLE {role} LOGIN PASSWORD '{PASSWORD}'; \
             EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;"
        ))
        .expect("role");
    }
    pg.exec(&format!(
        "GRANT CREATE ON DATABASE {db} TO {WRITER}, {READER}; \
         CREATE SCHEMA alpha AUTHORIZATION {WRITER}; \
         CREATE SCHEMA beta AUTHORIZATION {READER};",
        db = pg.db
    ))
    .expect("schemas");
    let a = Pool::connect(cfg(&pg.db, WRITER, "alpha"))
        .await
        .expect("A boots");
    let (plan, outcome) = sa::apply(&a, &Program::compile(ALPHA).unwrap(), "t", "t").await;
    assert!(plan.expect("plans").refusals.is_empty());
    assert!(outcome.is_ok(), "{outcome:?}");
    ordo_host::principal_directory::ensure_table(&a)
        .await
        .expect("A's directory");
    ordo_host::federation::ensure_fingerprint(&a, "identity", "oidc", "fp", "sub", 0)
        .await
        .expect("A's connections");
    pg.exec(
        "INSERT INTO alpha.notes (\"$space\", id, title) VALUES ('s', 'note_1', 'hello'); \
         INSERT INTO alpha.secrets (\"$space\", id, body) VALUES ('s', 'secr_1', 'shh');",
    )
    .expect("seed");
    let b = Pool::connect(cfg(&pg.db, READER, "beta"))
        .await
        .expect("B boots");
    World { pg, a, b }
}

/// Whether the reader's own query is refused by Postgres for privilege.
async fn denied(b: &Pool, sql: &str) -> bool {
    match b.rows(sql).await {
        Ok(_) => false,
        Err(e) => {
            assert_eq!(e.sqlstate(), Some("42501"), "{sql}: {e}");
            true
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_granted_table_reads_and_postgres_refuses_the_rest() {
    let w = world("ordo_rgrant_read").await;
    // Before any grant, Postgres refuses the reader everything.
    assert!(denied(&w.b, "SELECT title FROM alpha.notes").await);

    let lines = peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("granted");
    assert!(
        lines[0].contains("viewer") && lines[0].contains(READER),
        "{lines:?}"
    );
    let rows =
        w.b.rows("SELECT title FROM alpha.notes")
            .await
            .expect("reads");
    assert_eq!(rows, [[Some("hello".to_string())]]);
    // The control is the grant: the reader's own SQL, with no Ordo check
    // in the way, is refused on a table the line does not name.
    assert!(denied(&w.b, "SELECT body FROM alpha.secrets").await);
    // The sign-in tables by column: what the `$caller` guard reads, and
    // nothing else.
    w.b.rows(
        "SELECT id, principal, kind, status FROM alpha.members; \
         SELECT id, deleted_at FROM alpha.\"$principal\"; \
         SELECT authenticator, principal, retired_case FROM alpha.\"$authenticator\"; \
         SELECT name, kind, fingerprint, subject_claim FROM alpha.\"$authconnection\";",
    )
    .await
    .expect("the sign-in columns read");
    assert!(denied(&w.b, "SELECT \"displayName\" FROM alpha.members").await);
    assert!(denied(&w.b, "SELECT * FROM alpha.\"$principal\"").await);
    assert!(denied(&w.b, "SELECT 1 FROM alpha.\"$commandlog\"").await);

    // The reader's start check agrees, and the peer link reads through the
    // column grants.
    let (schema, callers) = beta();
    let only_notes = {
        let mut s = schema.clone();
        s.externals.retain(|e| e.name == "entries");
        s
    };
    peer::check_access(&w.b, &only_notes, &callers, false)
        .await
        .expect("granted");
    peer::check(&w.b, &only_notes).await.expect("an Ordo peer");

    // Unchanged, a second pass sends nothing and still agrees.
    peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("again");
    w.b.rows("SELECT title FROM alpha.notes")
        .await
        .expect("reads");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reader_refuses_to_start_without_a_grant_and_quotes_the_line() {
    let w = world("ordo_rgrant_start").await;
    let (schema, callers) = beta();
    let e = peer::check_access(&w.b, &schema, &callers, false)
        .await
        .expect_err("no grant");
    assert!(matches!(e, SchemaRefusal::NotGranted { .. }), "{e}");
    let text = e.to_string();
    assert!(
        text.contains("reader <name> reads notes, secrets")
            && text.contains(&format!("ORDO_READER_<NAME>_ROLE={READER}"))
            && text.contains("journal's definition file"),
        "{text}"
    );

    // Granted the notes only: still refused, naming the secrets.
    peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("granted");
    let e = peer::check_access(&w.b, &schema, &callers, false)
        .await
        .expect_err("secrets not granted");
    let text = e.to_string();
    assert!(
        text.contains("cannot read `secrets`") && !text.contains("`notes`,"),
        "{text}"
    );

    // Both: it starts.
    peer::grant_readers_with(
        &w.a,
        &alpha_with("reader viewer reads notes, secrets"),
        lookup,
    )
    .await
    .expect("granted");
    peer::check_access(&w.b, &schema, &callers, false)
        .await
        .expect("starts");
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_table_or_the_line_revokes() {
    let w = world("ordo_rgrant_revoke").await;
    peer::grant_readers_with(
        &w.a,
        &alpha_with("reader viewer reads notes, secrets"),
        lookup,
    )
    .await
    .expect("granted");
    w.b.rows("SELECT body FROM alpha.secrets")
        .await
        .expect("reads");

    // `secrets` off the line: revoked.
    peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("reconciled");
    assert!(denied(&w.b, "SELECT body FROM alpha.secrets").await);
    w.b.rows("SELECT title FROM alpha.notes")
        .await
        .expect("still reads");

    // The whole line gone: everything revoked, the setting no longer read.
    peer::grant_readers_with(&w.a, &alpha_with(""), |_| None)
        .await
        .expect("reconciled");
    assert!(denied(&w.b, "SELECT title FROM alpha.notes").await);
    assert!(denied(&w.b, "SELECT id FROM alpha.members").await);
    let left =
        w.pg.query(&format!(
            "SELECT has_schema_privilege('{READER}', 'alpha', 'USAGE')::text, \
             (SELECT count(*) FROM alpha.\"$readergrant\")::text"
        ))
        .expect("asked");
    assert_eq!(
        left,
        [[Some("false".to_string()), Some("0".to_string())]],
        "no usage and no record left"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_table_an_apply_creates_is_granted_by_that_apply() {
    let w = world("ordo_rgrant_apply").await;
    // The apply reads the setting from the environment. Only this test in
    // the binary uses this nickname.
    std::env::set_var("ORDO_READER_APPLIED_ROLE", READER);
    let v1 = Program::compile(&format!("reader applied reads notes\n{ALPHA}")).unwrap();
    let (_, outcome) = sa::apply(&w.a, &v1, "t", "t").await;
    assert!(outcome.is_ok(), "{outcome:?}");
    w.b.rows("SELECT title FROM alpha.notes")
        .await
        .expect("reads");

    let v2 = Program::compile(&format!(
        "reader applied reads notes, later\n{ALPHA}\nentity later {{\n  what: text\n}}\n"
    ))
    .unwrap();
    let (plan, outcome) = sa::apply(&w.a, &v2, "t", "t").await;
    assert!(plan.expect("plans").refusals.is_empty());
    assert!(outcome.is_ok(), "{outcome:?}");
    w.b.rows("SELECT what FROM alpha.later")
        .await
        .expect("granted in the same apply");
    std::env::remove_var("ORDO_READER_APPLIED_ROLE");

    // A line whose setting is unset refuses the apply, naming it.
    let v3 = Program::compile(&format!(
        "reader nobody reads notes\n{ALPHA}\nentity later {{\n  what: text\n}}\n"
    ))
    .unwrap();
    let (_, outcome) = sa::apply(&w.a, &v3, "t", "t").await;
    let Err(sa::ApplyRefusal::Internal(text)) = outcome else {
        panic!("refused: {outcome:?}")
    };
    assert!(
        text.contains("ORDO_READER_NOBODY_ROLE is not set"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_role_is_refused_unless_the_setting_allows_it() {
    let w = world("ordo_rgrant_shared").await;
    let (schema, callers) = beta();
    // The writer's own role, reading its own schema as a peer: it owns it.
    let e = peer::check_access(&w.a, &schema, &callers, false)
        .await
        .expect_err("the owner");
    assert!(matches!(e, SchemaRefusal::SharedRole { .. }), "{e}");
    let text = e.to_string();
    assert!(
        text.contains("owns schema `alpha`") && text.contains("ORDO_PEER_SHARED_ROLE=on"),
        "{text}"
    );
    // A superuser (the test cluster's own role).
    let su = Pool::connect(PoolConfig {
        db: w.pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("boots");
    let e = peer::check_access(&su, &schema, &callers, false)
        .await
        .expect_err("a superuser");
    assert!(e.to_string().contains("is a superuser"), "{e}");
    // With the setting on, both start, and say so.
    for pool in [&w.a, &su] {
        let lines = peer::check_access(pool, &schema, &callers, true)
            .await
            .expect("allowed");
        assert!(lines[0].contains("ORDO_PEER_SHARED_ROLE=on"), "{lines:?}");
    }
    // The setting's values.
    assert!(!peer::shared_role_value(None).unwrap());
    assert!(peer::shared_role_value(Some("on")).unwrap());
    assert!(!peer::shared_role_value(Some("off")).unwrap());
    let e = peer::shared_role_value(Some("maybe")).expect_err("not a switch");
    assert!(e.to_string().contains("ORDO_PEER_SHARED_ROLE"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_line_needs_a_role_setting_naming_another_role() {
    let w = world("ordo_rgrant_unset").await;
    let alpha = alpha_with("reader viewer reads notes");
    // Unset.
    let e = peer::grant_readers_with(&w.a, &alpha, |_| None)
        .await
        .expect_err("unset");
    assert!(matches!(e, SchemaRefusal::ReaderUnset { .. }), "{e}");
    assert!(
        e.to_string().contains("ORDO_READER_VIEWER_ROLE is not set"),
        "{e}"
    );
    // A role the server does not have.
    let e = peer::grant_readers_with(&w.a, &alpha, |v| {
        (v == "ORDO_READER_VIEWER_ROLE").then(|| "ordo_rg_nobody".to_string())
    })
    .await
    .expect_err("no such role");
    assert!(matches!(e, SchemaRefusal::ReaderRoleMissing { .. }), "{e}");
    // This deployment's own role: refused by default, no grants with the
    // shared setting on.
    let own = |shared: bool| {
        move |v: &str| match v {
            "ORDO_READER_VIEWER_ROLE" => Some(WRITER.to_string()),
            "ORDO_PEER_SHARED_ROLE" if shared => Some("on".to_string()),
            _ => None,
        }
    };
    let e = peer::grant_readers_with(&w.a, &alpha, own(false))
        .await
        .expect_err("own role");
    assert!(matches!(e, SchemaRefusal::ReaderOwnRole { .. }), "{e}");
    assert!(e.to_string().contains("ORDO_PEER_SHARED_ROLE=on"), "{e}");
    let lines = peer::grant_readers_with(&w.a, &alpha, own(true))
        .await
        .expect("allowed");
    assert!(lines[0].contains("no grants are made"), "{lines:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_the_gate_refuses_leaves_the_grants_as_they_were() {
    let w = world("ordo_rgrant_gate").await;
    // The gate reads the setting from the environment. Only this test in
    // the binary uses this nickname.
    std::env::set_var("ORDO_READER_GATED_ROLE", READER);
    let v1 = Program::compile(&format!("reader gated reads notes\n{ALPHA}")).unwrap();
    let (_, outcome) = sa::apply(&w.a, &v1, "t", "t").await;
    assert!(outcome.is_ok(), "{outcome:?}");
    w.b.rows("SELECT title FROM alpha.notes")
        .await
        .expect("reads");
    assert!(denied(&w.b, "SELECT body FROM alpha.secrets").await);

    // A version that adds `secrets` to the line, and an entity the live
    // database does not have: the gate refuses it for drift.
    let v2 = Fixture::from_source(
        &format!("reader gated reads notes, secrets\n{ALPHA}\nentity later {{\n  what: text\n}}\n"),
        "",
    )
    .expect("v2 compiles");
    let keys: std::sync::Arc<dyn ordo_crypto::KeyProvider> =
        std::sync::Arc::new(ordo_crypto::TestProvider::with_space("s"));
    let r = ordo_host::deploy::verify_embedded(
        v2,
        &w.a,
        &cfg(&w.pg.db, WRITER, "alpha"),
        keys,
        ordo_crypto::SpaceId("s".into()),
        "v2",
    )
    .await;
    std::env::remove_var("ORDO_READER_GATED_ROLE");
    let Err(e) = r else {
        panic!("the gate refuses a version the database does not hold")
    };
    assert!(
        matches!(e, ordo_host::deploy::LoadRefusal::SchemaDrift { .. }),
        "{e}"
    );
    // The grants are exactly as v1 left them.
    assert!(
        denied(&w.b, "SELECT body FROM alpha.secrets").await,
        "a refused version changed the grants"
    );
    w.b.rows("SELECT title FROM alpha.notes")
        .await
        .expect("still reads");
}
