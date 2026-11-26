//! `ordo apply --db` of a program with a `peer` block reads the peer's
//! settings, as the door's apply does (reported fix, 2026-11-19, found
//! wiring the federation e2e).
//!
//! What was seen: `ordo apply examples/note-viewer --direct` refused every
//! external from the peer as `external_missing`, with the setting set. The
//! cause: the command assembled the program without
//! `ordo_exec::peer::resolve`, which `Program::compile` (the door's path)
//! calls. The fix calls it in the same place, after the artifact and the
//! DDL are taken. Since G8 the setting is the holder's door,
//! `ORDO_PEER_<NAME>_URL`: a peer is never read from a schema.
//!
//! A regression looks like a peer's table refused as missing, or an unset
//! door that is not named.
//!
//! One test, in a binary of its own, because it sets the peer setting for
//! the whole process. Its database is `ordo_peerapply`.

use std::path::{Path, PathBuf};

mod support;
use ordo_conformance::Pg;
use support::operator;

const VIEWER: &str = r#"
peer journal {
  std.notes.list
}

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
fn apply_refuses_a_peer_with_no_door_naming_the_setting() {
    let db = "ordo_peerapply";
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("an empty database");

    let dir: PathBuf = std::env::temp_dir().join("ordo-peer-apply");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("viewer.ordo");
    std::fs::write(&path, VIEWER).expect("write program");

    std::env::remove_var("ORDO_PEER_JOURNAL_URL");
    let out = apply(db, &path);
    assert_ne!(out.code, 0, "{}", out.report);
    assert!(
        out.report.contains("ORDO_PEER_JOURNAL_URL is not set"),
        "{}",
        out.report
    );
    assert!(
        !out.report.contains("external_missing"),
        "a peer's table is never looked for in a schema: {}",
        out.report
    );
}
