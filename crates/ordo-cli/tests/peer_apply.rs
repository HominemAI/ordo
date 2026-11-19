//! `ordo apply --db` of a program with a `peer` line reads the peer's
//! schema from `ORDO_PEER_<NAME>_SCHEMA`, as the door's apply does
//! (reported fix, 2026-11-19, found wiring the federation e2e).
//!
//! What was seen: `ordo apply examples/note-viewer --direct` refused every
//! external from the peer as `external_missing` on table
//! `$unresolved peer notes.notes`, with the setting set. The cause: the
//! command assembled the program without `ordo_exec::peer::resolve`, which
//! `Program::compile` (the door's path) calls. The fix calls it in the same
//! place, after the artifact and the DDL are taken.
//!
//! A regression looks like `$unresolved peer` in the command's refusal, or an
//! unset peer setting that is not named.
//!
//! One test, in a binary of its own, because it sets the peer setting for
//! the whole process. Its database is `ordo_peerapply`.

use std::path::{Path, PathBuf};

mod support;
use ordo_conformance::Pg;
use support::operator;

const VIEWER: &str = r#"
peer journal

external entries from journal.notes shared {
  id: text <- id
  title: text <- title
  access read open
}

op titles =
  entries[id, title]
  sort title
  take 10
"#;

fn apply(db: &str, path: &Path) -> operator::Outcome {
    operator::run(
        "apply",
        &[
            path.display().to_string(),
            "--db".into(),
            db.into(),
            "--as".into(),
            "test".into(),
        ],
    )
}

#[test]
fn apply_reads_the_peer_schema_from_its_setting() {
    let db = "ordo_peerapply";
    let pg = Pg::named(db);
    pg.exec(
        "DROP SCHEMA public CASCADE; CREATE SCHEMA public; \
         DROP SCHEMA IF EXISTS alpha CASCADE; CREATE SCHEMA alpha; \
         CREATE TABLE alpha.notes (id text PRIMARY KEY, title text NOT NULL);",
    )
    .expect("the peer's table");

    let dir: PathBuf = std::env::temp_dir().join("ordo-peer-apply");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("viewer.ordo");
    std::fs::write(&path, VIEWER).expect("write program");

    // Unset: refused, naming the setting.
    std::env::remove_var("ORDO_PEER_JOURNAL_SCHEMA");
    let out = apply(db, &path);
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("ORDO_PEER_JOURNAL_SCHEMA"),
        "{}",
        out.report
    );

    // Set: the externals read `alpha`. `alpha` holds no Ordo deployment, so
    // the apply is still refused, now for that and by its real name.
    std::env::set_var("ORDO_PEER_JOURNAL_SCHEMA", "alpha");
    let out = apply(db, &path);
    assert!(
        !out.report.contains("$unresolved peer"),
        "the peer was not resolved: {}",
        out.report
    );
    assert!(out.report.contains("alpha"), "{}", out.report);
}
