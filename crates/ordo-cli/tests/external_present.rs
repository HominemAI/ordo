//! `ordo plan` against a database lists what is in it and not Ordo's, under
//! the `EXTERNAL` heading (27j, `external-entities` slice C), and refuses a
//! translation whose table has changed. Nothing is changed in the database.
//!
//! A regression looks like a declared external's table listed as stray, an
//! Ordo table (`$` platform tables, the program's entities) listed as stray,
//! a stray table missing from the list, or a `CREATE` or `DROP` reaching
//! the other system's schema.
//!
//! Databases are prefixed `ordo_xpres_`.

use ordo_cli::migrate::plan_against_database;
use ordo_cli::Sources;
use ordo_conformance::Pg;

const PROGRAM: &str = r#"
entity notes {
  body: text
}

external patrons (key: pat) from crm.customers shared {
  id <- "customer_id"
  name: text <- "full_name"
  access read name == $caller
}
"#;

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("p.ordo".into(), text.to_string())])
}

fn foreign(db: &str) -> Pg {
    let pg = Pg::named(db);
    pg.exec(
        "DROP SCHEMA public CASCADE; CREATE SCHEMA public; \
         DROP SCHEMA IF EXISTS crm CASCADE; DROP SCHEMA IF EXISTS legacy CASCADE; \
         CREATE SCHEMA crm; \
         CREATE TABLE crm.customers (customer_id bigint PRIMARY KEY, full_name text NOT NULL, phone text); \
         CREATE TABLE crm.orders (id int PRIMARY KEY); \
         CREATE SCHEMA legacy; \
         CREATE TABLE public.stray (id int);",
    )
    .expect("the foreign side");
    pg
}

#[tokio::test]
async fn the_plan_lists_what_is_present_and_not_ordos_and_changes_nothing() {
    let db = "ordo_xpres_list";
    let pg = foreign(db);
    let out = plan_against_database(&one(PROGRAM), "p.ordo", db).await;
    assert_eq!(out.code, 0, "{}", out.report);
    let r = &out.report;
    assert!(r.contains("EXTERNAL\n"), "{r}");
    // The stray table and schema, and the table the program does not declare.
    for expected in [
        "table `crm.orders`",
        "table `public.stray`",
        "schema `legacy`, with no tables",
        "present, not Ordo's (nothing is changed):",
        // A column the translation does not read is a note.
        "note: `crm.customers` has a column the translation of `patrons` does not read: `phone`",
    ] {
        assert!(r.contains(expected), "missing `{expected}`:\n{r}");
    }
    // Not Ordo's list: the declared table, Ordo's own, the platform's.
    for absent in ["table `crm.customers`", "table `public.notes`", "public.$"] {
        assert!(!r.contains(absent), "`{absent}` listed:\n{r}");
    }
    let after = pg
        .query(
            "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname IN ('crm', 'legacy') AND relkind = 'r'",
        )
        .expect("catalog");
    assert_eq!(
        after[0][0].as_deref(),
        Some("2"),
        "no table was made or dropped"
    );
}

#[tokio::test]
async fn the_plan_refuses_a_changed_translation_and_names_the_column() {
    let db = "ordo_xpres_changed";
    let pg = foreign(db);
    pg.exec("ALTER TABLE crm.customers DROP COLUMN full_name")
        .expect("the other system drops a column");
    let out = plan_against_database(&one(PROGRAM), "p.ordo", db).await;
    assert_eq!(out.code, 1, "{}", out.report);
    assert!(
        out.report
            .contains("external_changed on `patrons`: column `full_name` is gone"),
        "{}",
        out.report
    );
    assert!(
        out.report
            .contains("ordo introspect --external crm.customers"),
        "{}",
        out.report
    );
}
