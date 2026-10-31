//! Golden files: the only way a "byte-stable" done-when clause is worth
//! anything.
//!
//! P2 asks for byte-stable DDL, P4 for byte-stable per-case SQL and
//! artifact metadata, P7 for a byte-stable `ordo plan` artifact. Until
//! this module existed nothing in the tree compared any of those against a
//! recorded expectation, so every one of those clauses was a promise with
//! no test behind it. A golden file makes the promise checkable: the bytes
//! are in the repository, a change to them is a line in a diff, and a
//! reviewer sees the SQL change in the same commit as the compiler change
//! that caused it.
//!
//! # Regenerating
//!
//! ```text
//! ORDO_UPDATE_GOLDEN=1 cargo test --workspace
//! ```
//!
//! That rewrites every golden this run touched, then `git diff` is the
//! change under review. **A golden diff is a behaviour change** — the
//! point of regenerating is to read it, not to make the test quiet.
//!
//! The command is repeated verbatim in every failure message. A golden
//! test nobody can update becomes a golden test everybody deletes, and
//! there is no way to discover an undocumented update mechanism from a
//! red test.
//!
//! # Why not a snapshot-testing crate
//!
//! `insta` and friends buy a review UI, inline snapshots and a pending-file
//! workflow. What this repository needs is `assert_eq!` on two strings with
//! a way to rewrite one of them, which is the forty lines below — against a
//! workspace whose entire external dependency list is nine crates chosen
//! one at a time. The goldens here are large, separate artifacts (a DDL
//! script, a SQL statement, an artifact document) that WANT to be files a
//! reviewer opens and a tool can consume, not inline literals; that is the
//! half of a snapshot library's value this case does not use.

use std::path::Path;

/// The environment variable that rewrites goldens instead of comparing.
pub const UPDATE_VAR: &str = "ORDO_UPDATE_GOLDEN";

const HOW: &str = "regenerate with:  ORDO_UPDATE_GOLDEN=1 cargo test --workspace\n\
                   then READ the diff — a golden change is a behaviour change.";

/// Compare `actual` against the golden at `path`, or rewrite it when
/// `ORDO_UPDATE_GOLDEN` is set. Panics on mismatch, which is what a test
/// wants.
///
/// `path` should be absolute — call it as
/// `Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/…")` so the
/// test does not depend on the working directory the runner chose.
pub fn assert_golden(path: &Path, actual: &str) {
    if std::env::var_os(UPDATE_VAR).is_some() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        }
        std::fs::write(path, actual).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        return;
    }
    let expected = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => panic!("missing golden {}: {e}\n{HOW}", path.display()),
    };
    if expected == actual {
        return;
    }
    panic!(
        "golden mismatch: {}\n{}\n{HOW}",
        path.display(),
        first_difference(&expected, actual)
    );
}

/// The first differing line, with its number — enough to see what moved
/// without printing two thousand lines of DDL into a test log. The full
/// diff is `git diff` after a regeneration, which is the tool that is
/// actually good at this.
fn first_difference(expected: &str, actual: &str) -> String {
    let e: Vec<&str> = expected.lines().collect();
    let a: Vec<&str> = actual.lines().collect();
    for i in 0..e.len().max(a.len()) {
        let (le, la) = (e.get(i), a.get(i));
        if le != la {
            return format!(
                "  first difference at line {}\n    golden: {}\n    actual: {}",
                i + 1,
                le.map(|s| format!("{s:?}"))
                    .unwrap_or("<end of file>".into()),
                la.map(|s| format!("{s:?}"))
                    .unwrap_or("<end of file>".into()),
            );
        }
    }
    // Same lines, different bytes: a trailing-newline difference, which is
    // exactly the kind of thing a line-wise report would hide.
    format!(
        "  every line matches but the bytes do not — trailing newline?\n    \
         golden {} bytes, actual {} bytes",
        expected.len(),
        actual.len()
    )
}
