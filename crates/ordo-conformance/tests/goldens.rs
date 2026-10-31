//! Golden files for the compiler's byte-stable artifacts (P2, P4).
//!
//! Nothing here touches a database. These are **compile-tier** tests: the
//! done-when clauses they close are about what the compiler emits, and a
//! compiler that emits different bytes on two runs is broken whether or
//! not Postgres would accept either.
//!
//! Regenerate every golden with:
//!
//! ```text
//! ORDO_UPDATE_GOLDEN=1 cargo test --workspace
//! ```
//!
//! then read the diff. See `ordo_conformance::golden`.

use std::path::PathBuf;

use ordo_conformance::{assert_golden, covered_case_sources, Fixture};
use ordo_schema::ddl_for;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn fixture() -> Fixture {
    Fixture::load().expect("the market fixture compiles")
}

// ── P2: golden DDL ───────────────────────────────────────────────────────

/// P2's done-when, in full: the market fixture's generated DDL is
/// byte-stable, `categories`' tree columns and `orderItems`' ownership FK
/// and `customers.email`'s digest column and unique constraint included.
/// Those three are named in the plan because they are the parts most
/// likely to be silently dropped; the golden covers them by covering
/// everything, and the assertions below say so out loud so a regeneration
/// that lost one is caught by a named test rather than by a reader's eye.
#[test]
fn market_fixture_ddl_is_byte_stable() {
    let f = fixture();
    assert_golden(&golden("market.ddl.sql"), &f.ddl);

    // `Fixture::load` already ran `ddl_for`; running it again on the same
    // schema must give the same bytes. This is the cheap half of the
    // stability claim — the expensive half is the golden file, which
    // proves it across RUNS and machines rather than within one process.
    assert_eq!(
        f.ddl,
        ddl_for(&f.schema).expect("ddl"),
        "ddl_for is a pure function of the schema"
    );
}

/// P2 calls the `acl`-derived link entity out separately — "byte-identical
/// to the hand-written equivalent" — because it is the one table in the
/// schema that no author wrote. `boards` declares `acl (read, write,
/// admin)` and `boardsAccess` is what that derives. Its own golden, so a
/// change to acl derivation shows up as a change to this small file
/// instead of as three lines inside a six-hundred-line one.
#[test]
fn acl_derived_link_entity_ddl_is_byte_stable() {
    let f = fixture();
    assert!(
        f.schema.entity("boardsAccess").is_some(),
        "the fixture's `acl` declaration still derives the link entity"
    );
    assert_golden(
        &golden("boardsAccess.ddl.sql"),
        &statements_mentioning(&f.ddl, "boardsAccess"),
    );
}

/// Every statement of the generated DDL that names `entity`, in emission
/// order. Statement boundaries are `;` at the end of a line, which is the
/// shape `ddl_for` emits and the only shape it emits.
fn statements_mentioning(ddl: &str, entity: &str) -> String {
    let needle = format!("\"{entity}\"");
    let mut out = String::new();
    let mut stmt = String::new();
    for line in ddl.lines() {
        stmt.push_str(line);
        stmt.push('\n');
        if line.ends_with(';') {
            if stmt.contains(&needle) {
                out.push_str(&stmt);
            }
            stmt.clear();
        }
    }
    assert!(stmt.is_empty(), "the DDL ended mid-statement: {stmt:?}");
    out
}

// ── P4: golden SQL, per corpus case ──────────────────────────────────────

/// P4's done-when asks for byte-stable SQL per case. One golden file per
/// case rather than one big one: a case's statement is the unit a reviewer
/// reads, and `git diff` naming `C006.sql` says which case changed before
/// anyone opens anything.
///
/// The set is `covered_case_sources()` — the same table `corpus.rs`
/// executes, so a case cannot be goldened without also being run.
#[test]
fn corpus_case_sql_is_byte_stable() {
    let f = fixture();
    for (id, src) in covered_case_sources() {
        let p = f
            .compile(src)
            .unwrap_or_else(|e| panic!("{id} compiles: {e}"));
        // The case's own source text goes in the file. A golden SQL
        // statement with no statement of what was compiled is unreviewable
        // — the reader would have to find the case in another file to know
        // whether the SQL is right.
        let body = format!(
            "-- {id}\n-- source:\n{}\n\n{}\n",
            src.lines()
                .map(|l| format!("--   {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
            p.sql
        );
        assert_golden(&golden(&format!("sql/{id}.sql")), &body);
    }
}

/// The within-one-run half of P4's stability claim: compiling the same
/// case twice in one process yields the same bytes. Cheap, and it catches
/// the failure mode the golden files cannot — a `HashMap` iteration that
/// happens to be stable in one process but is not stable by construction
/// would still produce two different strings often enough for this to
/// catch it, and a golden recorded from the same wrong run would not.
#[test]
fn compiling_twice_gives_the_same_bytes() {
    let f = fixture();
    for (id, src) in covered_case_sources() {
        let a = f.compile(src).unwrap_or_else(|e| panic!("{id}: {e}"));
        let b = f.compile(src).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(a.sql, b.sql, "{id} compiled to two different statements");
    }
    // And so does the DDL, from two independent compilations of the same
    // source — two `Fixture`s, so two `HashMap`s with two different
    // iteration orders if any of this depended on one.
    assert_eq!(fixture().ddl, fixture().ddl);
}
