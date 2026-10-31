//! Two gaps in what a write body may carry (audit 2026-10-08, findings 4
//! and 5), executed against Postgres.
//!
//! - A `json` document written as a literal in a routine body, or imported,
//!   obeys the same `ORDO_JSON_MAX_BYTES` and `ORDO_JSON_MAX_DEPTH` the
//!   door applies to a parameter. `fit_json` is the one gate every path
//!   crosses.
//! - A `datetime` literal in a write body is a held value, like a date.
//!
//! A regression looks like an over-limit document stored, or
//! `{ at: 2026-01-01T00:00:00Z }` refused with "pass it as a parameter".
//!
//! Its own databases, prefixed `ordo_wlit_`.

mod gc_support;

use gc_support::{class_of, db, ANA};
use ordo_conformance::{Fixture, WriteLog, FIXTURE, SEED};

fn fixture(ops: &str) -> Fixture {
    let src = format!(
        "{FIXTURE}\nentity pages (key: pg) {{\n  title: text\n  doc: json = {{}}\n  at: datetime?\n}}\n\n{ops}"
    );
    Fixture::from_source(&src, SEED).expect("compiles")
}

#[test]
fn an_imported_or_routine_json_document_over_the_limit_is_refused() {
    let limits = ordo_schema::json::limits();
    let big = format!("{{\\\"t\\\":\\\"{}\\\"}}", "x".repeat(limits.max_bytes));
    let deep = format!(
        "{}1{}",
        "[".repeat(limits.max_depth + 1),
        "]".repeat(limits.max_depth + 1)
    );
    let ops = format!(
        "op bigLiteral =\n  set pages {{ title: \"t\", doc: \"{big}\" }}\n\
         op deepLiteral =\n  set pages {{ title: \"t\", doc: \"{deep}\" }}\n\
         op okLiteral =\n  set pages {{ title: \"t\", doc: \"[1,[2]]\" }}\n"
    );
    let f = fixture(&ops);
    let pg = db("ordo_wlit_json");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let err = f
        .run_write_as(&pg, &mut log, "bigLiteral", ANA)
        .expect_err("over the size limit");
    assert_eq!(class_of(&err), "bound_violation", "{err}");
    assert!(err.contains("ORDO_JSON_MAX_BYTES"), "{err}");
    let err = f
        .run_write_as(&pg, &mut log, "deepLiteral", ANA)
        .expect_err("over the depth limit");
    assert_eq!(class_of(&err), "bound_violation", "{err}");
    assert!(err.contains("ORDO_JSON_MAX_DEPTH"), "{err}");

    f.run_write_as(&pg, &mut log, "okLiteral", ANA)
        .expect("a document inside the limits lands");
    let rows = pg.query(r#"SELECT count(*) FROM "pages""#).expect("read");
    assert_eq!(rows[0][0].as_deref(), Some("1"), "only the fitting one");
}

#[test]
fn a_datetime_literal_in_a_write_body_is_stored() {
    let f = fixture("op stamp =\n  set pages { title: \"t\", at: 2026-01-01T00:00:00Z }\n");
    let pg = db("ordo_wlit_datetime");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();
    f.run_write_as(&pg, &mut log, "stamp", ANA)
        .expect("a datetime literal is a held value");
    let rows = pg
        .query(r#"SELECT ("at" = TIMESTAMPTZ '2026-01-01T00:00:00Z')::text FROM "pages""#)
        .expect("read back");
    assert_eq!(rows[0][0].as_deref(), Some("true"));
}
