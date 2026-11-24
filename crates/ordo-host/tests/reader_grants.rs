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

/// **`$readergrant` records each column it granted**
/// (`plan-evolution-federation.md` §3.1). The line grants `notes` by
/// column (one is orphaned) and the sign-in contract by column; a person
/// grants `secrets` to the same role by hand. The record lists exactly what
/// Ordo granted. With the line gone, every recorded table and column is
/// revoked, and the hand grant is left as it was. A row from before grants
/// were recorded still revokes everything, as it did.
///
/// A regression looks like a hand grant taken by Ordo, a recorded column
/// left granted after its line is gone, or a record that does not match
/// the grants.
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_line_revokes_what_it_recorded_and_nothing_granted_by_hand() {
    let w = world("ordo_rgrant_bycol").await;
    w.pg.exec("ALTER TABLE alpha.notes ADD COLUMN mood text")
        .expect("a column a version left");
    ordo_exec::orphans::record(
        &w.a,
        &[ordo_exec::orphans::Left {
            entity: "notes".into(),
            field: Some("mood".into()),
            columns: vec!["mood".into()],
            sealed: false,
        }],
        Some("1.0.0"),
        "2.0.0",
        "t",
        1_900_000_000,
    )
    .await
    .expect("orphaned");
    peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("granted by column");
    w.pg.exec(&format!("GRANT SELECT ON alpha.secrets TO {READER}"))
        .expect("granted by hand");
    // A start with nothing changed writes nothing, hand grants and all.
    peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("unchanged");
    let record = |w: &World| {
        w.pg.query("SELECT \"grants\" FROM alpha.\"$readergrant\"")
            .expect("record")
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .unwrap_or_default()
    };
    let grants = record(&w);
    assert!(
        grants.contains(r#"["notes","title"]"#) && !grants.contains(r#"["notes","mood"]"#),
        "{grants}"
    );
    assert!(
        !grants.contains("secrets"),
        "a hand grant is not Ordo's: {grants}"
    );
    w.b.rows("SELECT body FROM alpha.secrets")
        .await
        .expect("the hand grant reads");

    let privilege = |w: &World, what: &str| {
        w.pg.query(&format!("SELECT {what}::text"))
            .expect("asked")
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .unwrap_or_default()
    };
    peer::grant_readers_with(&w.a, &alpha_with(""), |_| None)
        .await
        .expect("the line is gone");
    assert_eq!(
        privilege(
            &w,
            &format!("has_column_privilege('{READER}', 'alpha.notes', 'title', 'SELECT')")
        ),
        "false",
        "a recorded column is revoked"
    );
    assert_eq!(
        privilege(
            &w,
            &format!("has_table_privilege('{READER}', 'alpha.secrets', 'SELECT')")
        ),
        "true",
        "the hand grant is left"
    );
    assert_eq!(
        privilege(
            &w,
            &format!("has_schema_privilege('{READER}', 'alpha', 'USAGE')")
        ),
        "false",
        "Ordo's usage is revoked"
    );

    // A row from a release before grants were recorded: everything goes.
    peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("granted again");
    w.pg.exec("UPDATE alpha.\"$readergrant\" SET \"grants\" = NULL")
        .expect("an old row");
    peer::grant_readers_with(&w.a, &alpha_with(""), |_| None)
        .await
        .expect("the line is gone");
    assert_eq!(
        privilege(
            &w,
            &format!("has_table_privilege('{READER}', 'alpha.secrets', 'SELECT')")
        ),
        "false",
        "with no record, everything in the schema is revoked, as before"
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

/// The reader program, mapping `extra` beside `id` and `title`.
fn reader_of(extra: &str) -> ordo_schema::Schema {
    let mut f = Fixture::from_source(
        &format!(
            "peer journal\n\n\
             external entries from journal.notes shared {{\n  id: text <- id\n  \
             title: text <- title\n{extra}  access read open\n}}\n"
        ),
        SEED,
    )
    .expect("the reader compiles");
    peer::resolve_with(&mut f.schema, "beta", |v| {
        (v == "ORDO_PEER_JOURNAL_SCHEMA").then(|| "alpha".to_string())
    })
    .expect("resolves");
    f.schema
}

/// **A reader reads only columns the read app still writes** (owner ruling
/// 2026-11-23, Q1; `plan-versions.md` §2.7). The writer's `notes.mood` is
/// orphaned by a version that stopped declaring it. The grant becomes one
/// by column, without `mood`: Postgres refuses the reader that column and
/// still serves the rest. The reader's start check passes, and its external
/// check holds the entity with `external_changed`, naming `mood`, the peer
/// and the fix. A reader that does not map `mood` reads on.
#[tokio::test(flavor = "multi_thread")]
async fn a_reader_loses_an_orphaned_column_with_a_named_error() {
    let w = world("ordo_rgrant_orphan").await;
    w.pg.exec("ALTER TABLE alpha.notes ADD COLUMN mood text; UPDATE alpha.notes SET mood = 'calm'")
        .expect("the column v1 wrote");
    peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("granted whole");
    w.b.rows("SELECT mood FROM alpha.notes")
        .await
        .expect("read while written");

    ordo_exec::orphans::record(
        &w.a,
        &[ordo_exec::orphans::Left {
            entity: "notes".into(),
            field: Some("mood".into()),
            columns: vec!["mood".into()],
            sealed: false,
        }],
        Some("1.0.0"),
        "2.0.0",
        "t",
        1_900_000_000,
    )
    .await
    .expect("orphaned");
    peer::grant_readers_with(&w.a, &alpha_with("reader viewer reads notes"), lookup)
        .await
        .expect("granted by column");
    assert!(denied(&w.b, "SELECT mood FROM alpha.notes").await);
    w.b.rows("SELECT id, title FROM alpha.notes")
        .await
        .expect("the rest still reads");

    let maps = reader_of("  mood: text? <- mood\n");
    peer::check_access(&w.b, &maps, &BTreeSet::new(), false)
        .await
        .expect("a table granted by column is still granted");
    let lines = ordo_host::external_check::check(&w.b, &maps)
        .await
        .expect("checks")
        .refusals();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("external_changed on `entries`")
            && lines[0].contains("column `mood` is no longer written by `journal`")
            && lines[0].contains("ordo introspect --external"),
        "{lines:?}"
    );

    let lines = ordo_host::external_check::check(&w.b, &reader_of(""))
        .await
        .expect("checks")
        .refusals();
    assert!(lines.is_empty(), "{lines:?}");
}

/// **A rename on a read table** (`plan-evolution-federation.md` §3.1,
/// §3.3; owner ruling 2026-11-23, Q1). The writer renames `notes.title` to
/// `heading` with `prev`. While the mark is declared, both columns are
/// written and the reader holds both (the union): its check passes and says
/// `title` is changing, naming `heading`. The version that drops the mark
/// stops writing `title`: it is recorded as an orphan, the reader keeps
/// `heading` alone, and its check holds the entity, naming `title` and its
/// new name, and so does `ordo rollback --dry-run` for the reader's version
/// that maps `title` (§7, test 6). A version that declares `title` again
/// clears the comment.
///
/// A regression looks like a reader still reading `title` once nothing
/// writes it, a refusal that does not name `heading`, or a hold while the
/// rename is still written.
#[tokio::test(flavor = "multi_thread")]
async fn a_renamed_column_is_read_until_the_rename_lands_then_refused_naming_its_new_name() {
    let w = world("ordo_rgrant_rename").await;
    let program = |fields: &str| {
        Program::compile(&format!(
            "domain journal @ 1.0.0\n\nreader viewer reads notes\n\n\
             entity notes {{\n{fields}  owner -> members?\n  \
             access read owner == $caller\n  access write owner == $caller\n}}\n"
        ))
        .expect("compiles")
    };
    let renaming = program("  heading: text = \"\" prev(title)\n");
    let landed = program("  heading: text = \"\"\n");
    // The rename's expand, as its step runs it: the new column, copied.
    w.pg.exec(
        "ALTER TABLE alpha.notes ADD COLUMN heading text NOT NULL DEFAULT ''; \
         UPDATE alpha.notes SET heading = title;",
    )
    .expect("the expand");

    // The reader's version that maps `title`, stored as an apply stores it.
    // A program compiles with its peer's schema from the environment, as a
    // node and `ordo rollback --dry-run` compile it. Only this test's
    // reader declares `peer journal`, so no other test reads the setting.
    std::env::set_var("ORDO_PEER_JOURNAL_SCHEMA", "alpha");
    let mapping_title = Program::compile(
        "domain viewer @ 1.0.0\n\npeer journal\n\n\
         external entries from journal.notes shared {\n  id: text <- id\n  \
         title: text <- title\n  access read open\n}\n",
    )
    .expect("the reader compiles");
    ordo_host::versions::Versions::on(&w.b)
        .ensure()
        .await
        .expect("registry");
    ordo_exec::registry::store(
        &w.b,
        &ordo_exec::registry::Artifact {
            version: mapping_title.version.clone(),
            artifact: ordo_plan::artifact::to_bytes(&mapping_title.doc),
            source: mapping_title.source.clone(),
            ddl: mapping_title.ddl.clone(),
            applied_at: ordo_commandlog::Timestamp(0),
            actor: "dev:ada".into(),
        },
    )
    .await
    .expect("stored");

    // While the rename is declared: the union, and the reader is told.
    peer::grant_readers_with(&w.a, &renaming.schema, lookup)
        .await
        .expect("granted");
    let dry = ordo_host::evolve::rollback_check(&w.b, "1.0.0")
        .await
        .expect("checks");
    assert!(dry.refusals.is_empty(), "{:?}", dry.refusals);
    w.b.rows("SELECT title, heading FROM alpha.notes")
        .await
        .expect("both columns read while both are written");
    let report = ordo_host::external_check::check(&w.b, &reader_of(""))
        .await
        .expect("checks");
    assert!(report.refusals().is_empty(), "{:?}", report.refusals());
    let changing: Vec<&String> = report
        .notes
        .iter()
        .filter(|n| n.starts_with(ordo_host::external_check::CHANGING))
        .collect();
    assert_eq!(changing.len(), 1, "{:?}", report.notes);
    assert!(
        changing[0].contains("`alpha.notes.title`")
            && changing[0].contains("is renamed to `heading` by `journal`"),
        "{changing:?}"
    );

    // The rename lands: `title` is an orphan, and the reader loses it.
    let names = sa::record_orphans(&w.a, &renaming.schema, &landed, Some("1.0.0"), "t")
        .await
        .expect("recorded");
    assert_eq!(names, vec!["notes.title".to_string()]);
    peer::grant_readers_with(&w.a, &landed.schema, lookup)
        .await
        .expect("granted by column");
    assert!(denied(&w.b, "SELECT title FROM alpha.notes").await);
    w.b.rows("SELECT id, heading FROM alpha.notes")
        .await
        .expect("the new column reads");
    let lines = ordo_host::external_check::check(&w.b, &reader_of(""))
        .await
        .expect("checks")
        .refusals();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("external_changed on `entries`")
            && lines[0]
                .contains("column `title` (it is now `heading`) is no longer written by `journal`")
            && lines[0].contains("ordo introspect --external"),
        "{lines:?}"
    );
    // A rollback to the version that maps `title` is told before it runs.
    let dry = ordo_host::evolve::rollback_check(&w.b, "1.0.0")
        .await
        .expect("checks");
    assert_eq!(dry.refusals.len(), 1, "{:?}", dry.refusals);
    assert!(
        dry.refusals[0].contains("column `title` (it is now `heading`)"),
        "{:?}",
        dry.refusals
    );

    // `title` declared again: written again, granted again, no comment.
    let again = program("  heading: text = \"\"\n  title: text = \"\"\n");
    sa::record_orphans(&w.a, &landed.schema, &again, Some("1.0.0"), "t")
        .await
        .expect("revived");
    peer::grant_readers_with(&w.a, &again.schema, lookup)
        .await
        .expect("granted whole");
    let report = ordo_host::external_check::check(&w.b, &reader_of(""))
        .await
        .expect("checks");
    assert!(report.refusals().is_empty(), "{:?}", report.refusals());
    assert!(
        !report
            .notes
            .iter()
            .any(|n| n.starts_with(ordo_host::external_check::CHANGING)),
        "{:?}",
        report.notes
    );
}

/// **The sign-in contract** (owner ruling 2026-11-23, Q16;
/// `plan-evolution-federation.md` §5, step F6). Every column of
/// `ordo_exec::sign_in_contract::CONTRACT` is in the tables a deployment
/// makes, and the reader is granted it. The writer's release is granted to
/// the reader too, and the reader's release check names the writer as a
/// peer on another release; the writer's names the reader by its role.
///
/// A regression looks like a contract column missing from a fresh
/// deployment (a release dropped it instead of retiring it), the release
/// not granted, or a peer on another release not named.
#[tokio::test(flavor = "multi_thread")]
async fn the_sign_in_contract_is_granted_and_the_release_check_names_the_peer_on_another_release() {
    use ordo_exec::sign_in_contract::{self as contract, PeerRelease};
    use ordo_host::release_check;

    let w = world("ordo_rgrant_contract").await;
    let alpha = alpha_with("reader viewer reads notes");
    peer::grant_readers_with(&w.a, &alpha, lookup)
        .await
        .expect("granted");
    let (beta, _) = beta();
    let at_alpha = |v: &str| (v == "ORDO_PEER_JOURNAL_SCHEMA").then(|| "alpha".to_string());

    // Every contract column is there and the reader reads it.
    for (t, cols) in contract::columns() {
        let list = cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        w.b.rows(&format!("SELECT {list} FROM alpha.\"{t}\" LIMIT 1"))
            .await
            .unwrap_or_else(|e| panic!("contract columns of {t}: {e}"));
    }

    // A writer from before the release table records none.
    assert_eq!(
        contract::release_of(&w.b, "alpha").await.expect("reads"),
        PeerRelease::Unrecorded
    );
    let n = release_check::neighbours(&w.b, std::slice::from_ref(&beta), at_alpha)
        .await
        .expect("reads");
    assert!(
        n[0].line(release_check::release()).contains("records none"),
        "{n:?}"
    );

    // The writer starts under another release. Until its grants are made
    // again, the reader is not granted the table.
    contract::record_release(&w.a, "0.4.0", 10)
        .await
        .expect("recorded");
    assert_eq!(
        contract::release_of(&w.b, "alpha").await.expect("reads"),
        PeerRelease::NotGranted
    );
    peer::grant_readers_with(&w.a, &alpha, lookup)
        .await
        .expect("granted again");
    assert_eq!(
        contract::release_of(&w.b, "alpha").await.expect("reads"),
        PeerRelease::Known("0.4.0".into())
    );
    // The release, and nothing else of the row.
    assert!(denied(&w.b, "SELECT started_at FROM alpha.\"$ordorelease\"").await);

    // The reader's release check names the peer on another release.
    let n = release_check::neighbours(&w.b, std::slice::from_ref(&beta), at_alpha)
        .await
        .expect("reads");
    assert_eq!(n.len(), 1, "{n:?}");
    let line = n[0].line(release_check::release());
    assert!(
        line == "peer journal (schema `alpha`): Ordo 0.4.0, ANOTHER RELEASE",
        "{line}"
    );
    // Unset, the setting is named.
    let n = release_check::neighbours(&w.b, std::slice::from_ref(&beta), |_| None)
        .await
        .expect("reads");
    assert!(
        n[0].line(release_check::release())
            .contains("`ORDO_PEER_JOURNAL_SCHEMA` is not set"),
        "{n:?}"
    );

    // The same release reads as this one.
    contract::record_release(&w.a, release_check::release(), 20)
        .await
        .expect("recorded");
    let n = release_check::neighbours(&w.b, std::slice::from_ref(&beta), at_alpha)
        .await
        .expect("reads");
    assert!(
        n[0].line(release_check::release())
            .ends_with("this release"),
        "{n:?}"
    );

    // The writer's release check names its reader by role.
    let n = release_check::neighbours(&w.a, &[alpha], |_| None)
        .await
        .expect("reads");
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(
        (n[0].kind, n[0].name.as_str(), n[0].at.as_str()),
        ("reader", "viewer", READER)
    );
    let _ = &w.pg;
}
