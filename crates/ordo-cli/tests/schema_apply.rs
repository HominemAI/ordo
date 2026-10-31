//! `ordo apply` against an empty target, and `ordo migrate --apply` after it:
//! `plan-schema-evolution.md` increment 2, plus the initial-create gap found
//! on 2026-09-20 (nothing in the tree created a deployment's schema; a first
//! deployment meant pasting DDL out of `$artifacts` into psql).
//!
//! Every case here goes through `ordo_cli::operator::run`, argv and all,
//! because the two commands' whole contract is what an operator types and
//! what the shell gets back, and because the dispatch (`migrate` did not
//! exist as a verb) is part of what is being proved.
//!
//! The load-bearing case is the one with **rows in it**: empty database,
//! apply, insert rows, add a field, apply again, rows still present and
//! readable through the new column. That is the difference between a
//! migration and a reset, and it is the property `ordo dev`'s interim
//! reset-and-reseed rule could not offer.
//!
//! Databases are prefixed `ordo_sapply_`, one per test function: these open
//! with `DROP SCHEMA public CASCADE`.

use std::path::PathBuf;

mod support;
use ordo_conformance::Pg;
use support::operator;

const V1: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  memo:  text?
}

op listA unbounded =
  a[name, n]
"#;

/// V1 plus an optional field, a constant-defaulted field and an index:
/// A2, A3 and A4, the three additive shapes an operator meets first.
const V2: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  memo:  text?
  tag:   text?
  n2:    int = 0
  index (name, n)
}

op listA unbounded =
  a[name, n, tag, n2]
"#;

/// V2 plus a new entity referencing `a`: A1 with its C5 riding along on
/// the column that arrives with it.
const V3: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  memo:  text?
  tag:   text?
  n2:    int = 0
  index (name, n)
}

entity c {
  owner -> a
  note:  text?
}

op listA unbounded =
  a[name, n, tag, n2]
"#;

/// V1 with `name` made required: B5, a transforming change over rows that
/// hold null.
const V1_REQUIRED: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text
  n:     int
  memo:  text?
}

op listA unbounded =
  a[name, n]
"#;

/// V1 with a row invariant on `a`: C9 over existing rows.
const V1_CHECK: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int
  memo:  text?
  check n >= 0
}

op listA unbounded =
  a[name, n]
"#;

/// Write a program to its own directory so the commands can be given the
/// path an operator would give them.
fn program(db: &str, tag: &str, text: &str) -> String {
    let dir: PathBuf = std::env::temp_dir().join(format!("ordo-sapply-{db}-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("p.ordo");
    std::fs::write(&path, text).expect("write program");
    path.display().to_string()
}

fn empty(db: &str) -> Pg {
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    pg
}

fn apply(path: &str, db: &str) -> operator::Outcome {
    operator::run(
        "apply",
        &[
            path.into(),
            "--db".into(),
            db.into(),
            "--as".into(),
            "test".into(),
        ],
    )
}

fn migrate(path: &str, db: &str) -> operator::Outcome {
    operator::run(
        "migrate",
        &[
            path.into(),
            "--apply".into(),
            "--db".into(),
            db.into(),
            "--as".into(),
            "test".into(),
        ],
    )
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

fn tables(pg: &Pg) -> Vec<String> {
    pg.query(
        "SELECT relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND relkind = 'r' AND relname NOT LIKE '$%' \
         ORDER BY 1",
    )
    .expect("catalog")
    .into_iter()
    .filter_map(|r| r.first().cloned().flatten())
    .collect()
}

// ── the case this increment exists for ───────────────────────────────────

/// Empty database, apply, rows, a field, apply again, rows still there.
#[test]
fn first_apply_creates_and_a_later_apply_keeps_the_rows() {
    let _g = SETTING.lock().unwrap_or_else(|e| e.into_inner());
    let db = "ordo_sapply_rows";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    let v2 = program(db, "v2", V2);

    // A first apply creates the whole schema and activates the artifact.
    let out = apply(&v1, db);
    assert_eq!(
        out.code, 0,
        "a first apply against an empty target:\n{}",
        out.report
    );
    assert!(
        out.report.contains("schema     created"),
        "the report must say the schema was created, because before this \
         increment it silently was not:\n{}",
        out.report
    );
    assert_eq!(
        tables(&pg),
        vec!["a", "b", "groupMembers", "members"],
        "every declared entity, and the platform's two, exist"
    );
    let v1_version = one(
        &pg,
        "SELECT version FROM \"$activations\" ORDER BY seq DESC LIMIT 1",
    );
    assert!(!v1_version.is_empty(), "the first apply activated");

    // Rows. These must survive everything below.
    pg.exec(
        "INSERT INTO a (\"$space\", id, rev, name, n) VALUES \
           ('s1','a_1',1,'one',1), ('s1','a_2',1,NULL,2), ('s2','a_3',1,'three',3);",
    )
    .expect("rows");

    // `ordo apply` of the changed program: the three changes are class A,
    // so the apply makes them itself (owner ruling 2026-10-22), then
    // activates. Same path as `ordo migrate --apply`.
    let out = apply(&v2, db);
    assert_eq!(
        out.code, 0,
        "safe additive changes apply themselves:\n{}",
        out.report
    );
    for expected in [
        "schema     migrated",
        "A2 ",
        "A3 ",
        "A4 ",
        "ORDO_APPLY_SAFE_CHANGES",
    ] {
        assert!(
            out.report.contains(expected),
            "the report should carry `{expected}`:\n{}",
            out.report
        );
    }

    assert_eq!(
        one(&pg, "SELECT count(*) FROM a"),
        "3",
        "the rows are still there"
    );
    assert_eq!(
        pg.query("SELECT id, tag, n2 FROM a ORDER BY id")
            .expect("read"),
        vec![
            vec![Some("a_1".into()), None, Some("0".into())],
            vec![Some("a_2".into()), None, Some("0".into())],
            vec![Some("a_3".into()), None, Some("0".into())],
        ],
        "the new optional column reads null and the constant default reads its literal, \
         on every existing row"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT indisvalid FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid \
             WHERE i.relname = 'a$name$n$ix'"
        ),
        "t",
        "the concurrent index is present and valid"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT phase FROM \"$migrations\" ORDER BY seq DESC LIMIT 1"
        ),
        "applied",
        "the apply is recorded"
    );
    assert_eq!(
        one(&pg, "SELECT count(*) FROM \"$migrations\""),
        "2",
        "one row for the create, one for the additive apply"
    );

    // The version moved with the schema (13a's own defect: a schema-only
    // change must move the version).
    let v2_version = one(
        &pg,
        "SELECT version FROM \"$activations\" ORDER BY seq DESC LIMIT 1",
    );
    assert_ne!(
        v2_version, v1_version,
        "the deploy identity moved with the schema"
    );
    let out = apply(&v2, db);
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("schema     matches"),
        "a second apply finds the schema already there and says so:\n{}",
        out.report
    );

    // Nothing left to do is a success, not a refusal.
    let out = migrate(&v2, db);
    assert_eq!(
        out.code, 0,
        "an up-to-date schema has nothing to apply:\n{}",
        out.report
    );
    assert!(out.report.contains("nothing to apply"), "{}", out.report);
    assert_eq!(one(&pg, "SELECT count(*) FROM a"), "3");
}

/// V1 with `n` indexed through the field modifier (reported fix 25w): the
/// single-column form of `index (a, b)`.
const V1_FIELD_INDEX: &str = r#"
entity b {
  title: text
}

entity a {
  name:  text?
  n:     int index
  memo:  text?
}

op listA unbounded =
  a[name, n]
"#;

/// A field's `index` arrives on an existing table the way `index (a, b)`
/// does: A4, concurrently, and valid when it lands (reported fix 25w).
#[test]
fn a_field_modifier_index_is_added_to_an_existing_table() {
    let db = "ordo_sapply_field_index";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    let v2 = program(db, "v2", V1_FIELD_INDEX);
    let out = apply(&v1, db);
    assert_eq!(out.code, 0, "{}", out.report);
    pg.exec(
        "INSERT INTO a (\"$space\", id, rev, name, n) VALUES \
           ('s1','a_1',1,'one',1), ('s1','a_2',1,NULL,2);",
    )
    .expect("rows");

    let out = migrate(&v2, db);
    assert_eq!(out.code, 0, "the index is class A:\n{}", out.report);
    assert!(out.report.contains("A4 "), "{}", out.report);
    assert!(out.report.contains("a$n$ix"), "{}", out.report);
    assert_eq!(
        one(
            &pg,
            "SELECT indisvalid FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid \
             WHERE i.relname = 'a$n$ix'"
        ),
        "t",
        "the concurrent index is present and valid"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM a"), "2");
    let out = apply(&v2, db);
    assert_eq!(
        out.code, 0,
        "the migrated database serves v2:\n{}",
        out.report
    );
    let out = migrate(&v2, db);
    assert!(out.report.contains("nothing to apply"), "{}", out.report);
}

// ── a new entity, with its reference ─────────────────────────────────────

/// A1 with a C5 on the column that arrives with it: the foreign key is the
/// column's own declaration, so it is applied with the table rather than
/// refused as a class-C change over existing data.
#[test]
fn a_new_entity_arrives_with_its_foreign_key() {
    let db = "ordo_sapply_entity";
    let pg = empty(db);
    let v2 = program(db, "v2", V2);
    let v3 = program(db, "v3", V3);
    assert_eq!(apply(&v2, db).code, 0);
    pg.exec("INSERT INTO a (\"$space\", id, rev, n) VALUES ('s1','a_1',1,1);")
        .expect("rows");

    let out = migrate(&v3, db);
    assert_eq!(out.code, 0, "a new entity is class A:\n{}", out.report);
    assert!(out.report.contains("A1 "), "{}", out.report);
    assert_eq!(tables(&pg), vec!["a", "b", "c", "groupMembers", "members"]);
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_constraint WHERE conname = 'c$owner$fk' AND contype = 'f'"
        ),
        "1",
        "the composite same-space foreign key is live"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM a"), "1");
    // The ref is indexed with the table it arrives on (25y).
    assert_eq!(
        one(
            &pg,
            "SELECT pg_get_indexdef(i.oid) FROM pg_class i WHERE i.relname = 'c$owner$ix'"
        ),
        r#"CREATE INDEX "c$owner$ix" ON public.c USING btree ("$space", owner)"#
    );
    assert_eq!(apply(&v3, db).code, 0, "and v3 activates over it");
}

/// Reported fix 25y. A ref field added to a table that holds rows gets its
/// index from the diff: A4, concurrently, valid when it lands, and nothing
/// after that.
#[test]
fn a_ref_added_to_an_existing_table_gets_its_index() {
    let db = "ordo_sapply_ref_index";
    let pg = empty(db);
    let v3 = program(db, "v3", V3);
    let v4 = program(
        db,
        "v4",
        &V3.replace("  note:  text?\n", "  note:  text?\n  reviewer -> a?\n"),
    );
    assert_eq!(apply(&v3, db).code, 0);
    pg.exec(
        "INSERT INTO a (\"$space\", id, rev, n) VALUES ('s1','a_1',1,1); \
         INSERT INTO c (\"$space\", id, rev, owner) VALUES ('s1','c_1',1,'a_1');",
    )
    .expect("rows");

    let out = migrate(&v4, db);
    assert_eq!(
        out.code, 0,
        "the field and its index are class A:\n{}",
        out.report
    );
    assert!(out.report.contains("A4 "), "{}", out.report);
    assert!(out.report.contains("c$reviewer$ix"), "{}", out.report);
    assert_eq!(
        one(
            &pg,
            "SELECT indisvalid FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid \
             WHERE i.relname = 'c$reviewer$ix'"
        ),
        "t",
        "the concurrent index is present and valid"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM c"), "1");
    assert_eq!(apply(&v4, db).code, 0, "the migrated database serves v4");
    let out = migrate(&v4, db);
    assert!(out.report.contains("nothing to apply"), "{}", out.report);
}

// ── refusals ─────────────────────────────────────────────────────────────

/// A transforming change is refused by its code, and nothing is written.
#[test]
fn a_transforming_change_is_refused_by_name_and_writes_nothing() {
    let db = "ordo_sapply_classb";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    let req = program(db, "req", V1_REQUIRED);
    assert_eq!(apply(&v1, db).code, 0);
    pg.exec(
        "INSERT INTO a (\"$space\", id, rev, name, n) VALUES \
           ('s1','a_1',1,'one',1), ('s1','a_2',1,NULL,2);",
    )
    .expect("rows");
    let before = pg
        .query("SELECT attname, attnotnull FROM pg_attribute WHERE attrelid = 'a'::regclass ORDER BY attnum")
        .expect("catalog");

    let out = migrate(&req, db);
    assert_eq!(out.code, 1, "B5 over nulls refuses:\n{}", out.report);
    assert!(out.report.contains("B5 "), "{}", out.report);
    assert!(
        out.report.contains("REFUSED"),
        "the refusal reaches the shell and the page:\n{}",
        out.report
    );
    assert_eq!(
        pg.query("SELECT attname, attnotnull FROM pg_attribute WHERE attrelid = 'a'::regclass ORDER BY attnum")
            .expect("catalog"),
        before,
        "a refused apply changes nothing"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM \"$migrations\" WHERE kind = 'additive'"
        ),
        "0",
        "nothing was attempted, so nothing is recorded as attempted"
    );
}

/// A `check` over rows that violate it refuses; over a table with no rows
/// it applies, because there is no data for the probe to rule on.
#[test]
fn a_check_refuses_over_violating_rows_and_applies_over_an_empty_table() {
    let db = "ordo_sapply_check";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    let chk = program(db, "chk", V1_CHECK);
    assert_eq!(apply(&v1, db).code, 0);
    pg.exec("INSERT INTO a (\"$space\", id, rev, n) VALUES ('s1','a_1',1,-5);")
        .expect("rows");

    let out = migrate(&chk, db);
    assert_eq!(
        out.code, 1,
        "C9 over a violating row refuses:\n{}",
        out.report
    );
    assert!(out.report.contains("C9 "), "{}", out.report);
    assert!(out.report.contains("probe_not_empty"), "{}", out.report);

    pg.exec("DELETE FROM a").expect("clear");
    let out = migrate(&chk, db);
    assert_eq!(
        out.code, 0,
        "C9 over an empty table applies:\n{}",
        out.report
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_constraint WHERE conrelid = 'a'::regclass AND contype = 'c'"
        ),
        "1",
        "the row invariant is live"
    );
    assert!(
        pg.exec("INSERT INTO a (\"$space\", id, rev, n) VALUES ('s1','a_9',1,-1);")
            .is_err(),
        "and it is enforced"
    );
}

/// The target is neither empty nor what the serving version declared: a
/// column the serving artifact asserts was dropped by hand. Refuse, and
/// say which way out exists, rather than apply on top of a database the
/// deployment is already refusing to load.
#[test]
fn a_target_that_matches_neither_version_is_refused() {
    let db = "ordo_sapply_drift";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    let v2 = program(db, "v2", V2);
    assert_eq!(apply(&v1, db).code, 0);
    pg.exec("ALTER TABLE a DROP COLUMN memo")
        .expect("hand alter");

    let out = migrate(&v2, db);
    assert_eq!(
        out.code, 1,
        "drift under the serving version refuses:\n{}",
        out.report
    );
    assert!(
        out.report.contains("serving_schema_drifted") && out.report.contains("a.memo"),
        "the refusal names the class and the missing column:\n{}",
        out.report
    );
    assert!(
        !tables(&pg).is_empty() && one(&pg, "SELECT count(*) FROM information_schema.columns WHERE table_name = 'a' AND column_name = 'tag'") == "0",
        "nothing was applied"
    );

    // And `ordo apply` refuses to activate over it too.
    let out = apply(&v2, db);
    assert_eq!(out.code, 1, "{}", out.report);
}

/// A registry that says a version is serving over a database that holds
/// none of its tables is not an empty target. Refuse rather than create.
#[test]
fn an_active_version_over_no_tables_is_not_an_empty_target() {
    let db = "ordo_sapply_ghost";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    assert_eq!(apply(&v1, db).code, 0);
    pg.exec("DROP TABLE a, b, members, \"groupMembers\" CASCADE")
        .expect("drop");

    let out = apply(&v1, db);
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(
        out.report.contains("registry says") || out.report.contains("serving_schema_drifted"),
        "the refusal explains the registry disagrees with the catalog:\n{}",
        out.report
    );
    assert!(
        tables(&pg).is_empty(),
        "nothing was created over a registry that disagrees with the catalog"
    );
}

// ── safe changes apply themselves (owner ruling 2026-10-22) ──────────────

/// The setting is process-wide, so the cases that set it hold this lock.
static SETTING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A new constant-default field over rows: `ordo apply` adds it, the rows
/// survive and read the default, and the change is recorded.
#[test]
fn apply_adds_a_constant_default_field_over_rows_and_records_it() {
    let _g = SETTING.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("ORDO_APPLY_SAFE_CHANGES");
    let db = "ordo_sapply_selfsafe";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    let v2 = program(db, "v2", V2);
    assert_eq!(apply(&v1, db).code, 0);
    pg.exec(
        "INSERT INTO a (\"$space\", id, rev, name, n) VALUES \
           ('s1','a_1',1,'one',1), ('s2','a_2',1,NULL,2);",
    )
    .expect("rows");
    let out = apply(&v2, db);
    assert_eq!(out.code, 0, "{}", out.report);
    assert!(out.report.contains("schema     migrated"), "{}", out.report);
    assert_eq!(one(&pg, "SELECT count(*) FROM a"), "2", "rows survive");
    assert_eq!(
        one(&pg, "SELECT count(*) FROM a WHERE n2 = 0"),
        "2",
        "the field is filled with its default on every row"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT phase FROM \"$migrations\" ORDER BY seq DESC LIMIT 1"
        ),
        "applied",
        "recorded in $migrations"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM \"$migrations\" WHERE kind = 'additive'"
        ),
        "1",
        "one additive record"
    );
}

/// A data-risk change is still refused by `ordo apply`, and the command it
/// suggests carries this deployment's real database name.
#[test]
fn apply_still_refuses_a_data_risk_change_and_names_the_database() {
    let _g = SETTING.lock().unwrap_or_else(|e| e.into_inner());
    std::env::remove_var("ORDO_APPLY_SAFE_CHANGES");
    let db = "ordo_sapply_selfrisk";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    let req = program(db, "req", V1_REQUIRED);
    assert_eq!(apply(&v1, db).code, 0);
    pg.exec("INSERT INTO a (\"$space\", id, rev, name, n) VALUES ('s1','a_1',1,NULL,1);")
        .expect("rows");
    let out = apply(&req, db);
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(
        out.report.contains(&format!("--db {db}")),
        "the command names the real database:\n{}",
        out.report
    );
    assert!(!out.report.contains("--db …"), "{}", out.report);
}

/// The setting off restores the refusal for a safe change.
#[test]
fn the_setting_off_restores_the_refusal() {
    let _g = SETTING.lock().unwrap_or_else(|e| e.into_inner());
    let db = "ordo_sapply_selfoff";
    let pg = empty(db);
    let v1 = program(db, "v1", V1);
    let v2 = program(db, "v2", V2);
    assert_eq!(apply(&v1, db).code, 0);
    std::env::set_var("ORDO_APPLY_SAFE_CHANGES", "off");
    let out = apply(&v2, db);
    std::env::remove_var("ORDO_APPLY_SAFE_CHANGES");
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(
        out.report.contains("ordo migrate --apply")
            && out.report.contains("ORDO_APPLY_SAFE_CHANGES"),
        "the refusal names the command and the setting:\n{}",
        out.report
    );
    assert!(out.report.contains(&format!("--db {db}")), "{}", out.report);
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM \"$migrations\" WHERE kind = 'additive'"
        ),
        "0",
        "nothing was applied"
    );
}
