//! Shared scaffolding for the guards corpus (`docs/guards-conformance.md`).
//!
//! Two things live here and nothing else: the caller dimension (§1) and the
//! part of the document's registered library (§3.4) that
//! `fixtures/market.ordo` does not already carry.
//!
//! # Why the missing ops are registered here rather than in the fixture
//!
//! §3.4's library is a *registration*, and the harness already has an entry
//! point for one: `Registry::extend_from`, the same path the composition
//! corpus's "cases may register additional local ops" uses. Registering
//! them here keeps the guards corpus from moving numbers that other
//! suites assert about the shipped fixture (`ordo-host`'s catalog and load
//! gate both count `f.registry.names()`), and keeps every op a reader can
//! see next to the case that needs it.
//!
//! Where `market.ordo` already registers the document's op under a
//! different parameter spelling (`addTask($project, $title)` for the
//! document's `addTask($p, $t)`), the fixture's spelling wins and the case
//! notes it. Where the document's op is absent, it is transcribed below.

#![allow(dead_code)]

use ordo_conformance::{Fixture, Pg};

// ── §1.1: the caller dimension ───────────────────────────────────────────
// `as: <principal>` binds `$caller`. `ora` is the space owner (mem_o): she
// holds no membership and no acl row, so anything she reaches comes from
// the door, never from data (R-G13).

pub const ANA: Option<&str> = Some("mem_1");
pub const BRAM: Option<&str> = Some("mem_2");
pub const CLEO: Option<&str> = Some("mem_3");
pub const DOV: Option<&str> = Some("mem_4");
pub const GIL: Option<&str> = Some("mem_g");
pub const ORA: Option<&str> = Some("mem_o");
pub const ANON: Option<&str> = None;

/// The §3.4 ops `market.ordo` does not already register, plus the local
/// ops individual cases declare inline. Transcribed from the document;
/// where the compiler demands an acknowledgment the document omits, the
/// marker is added and the divergence is recorded in `gc_divergences()`.
pub const LOCAL_OPS: &str = r#"
# ── §3.4, the ops market.ordo does not carry ────────────────────────────
op vendorNames unbounded =
  vendors[name]
  sort name

op aclRoster unbounded =
  boardsAccess[board, member, level]

op openCards unbounded =
  cardList(done == false)[title, board]

op tasksIn($p: id) unbounded =
  taskBoard(project == $p)[title, status]

op renameProject($project: id, $name: text) =
  set projects(id == $project) { name: $name }

op editNote($note: id, $body: text) =
  set taskNotes(id == $note) { body: $body }

op addMember($project: id, $member: id, $canWrite: bool = false) =
  set projectMembers { project: $project, member: $member, canWrite: $canWrite }

op setCardDone($card: id, $done: bool) =
  set cards(id == $card) { done: $done }

op closePair($a: id, $b: id) =
  set tasks(id == $a) { status: "done" }
  set tasks(id == $b) { status: "done" }

op shipAll =
  set tasks(status == "open") { status: "done" }

op purgeDoneAll =
  del tasks(status == "done")

op openBoard($name: text) =
  set boards { name: $name }

# ── local ops named by individual cases ─────────────────────────────────
# GC005 — caller conditions narrow the guarded set, never widen it.
op openIn($p: id) unbounded =
  tasks(status == "open" and project == $p)[title]
  sort title

# GC007 — dot-path guard stacking (R-G6).
op taskSheet unbounded =
  tasks[title, project.name]
  sort title

# GC028 — override replaces the inherited guard, read side.
op gammaProject($p: id) unbounded =
  projects(id == $p)[name]

# GC047 — the derived guards protect the link entity against direct writes.
op sneakGrant($board: id, $member: id) =
  set boardsAccess { board: $board, member: $member, level: "admin" }

# The grant/revoke ops are no longer here: `acl` DERIVES
# `grantBoardsAccess`, `revokeBoardsAccess` and `setBoardsAccessLevel`
# (GC036), so a hand-rolled pair would be testing this file rather than
# the derivation. `sneakGrant` above stays, because GC047's whole point is
# that a DIRECT `set boardsAccess { … }` is gated by the same guard the
# derived ops are.

# GC071 — a `$caller` condition and an entity guard conjoin.
op myOpenIn($p: id) unbounded =
  tasks(assignee == $caller and project == $p and status == "open")[title]

# GC088 — the projection every roster screen wants is `displayName`.
op teamNames($p: id) unbounded =
  projectMembers(project == $p)[member.displayName]

# GC089 — the read side of non-enumerability: a filtered read is empty.
op taskOne($id: id) unbounded =
  tasks(id == $id)[title]

# GC090 — the unguarded exception. `products` carries no guard at all.
op renameProduct($product: id, $name: text) =
  set products(id == $product) { name: $name }
"#;

/// The fixture with §3.4's registration completed.
pub fn fixture() -> Fixture {
    let mut f = Fixture::load().expect("the market fixture compiles");
    let program = ordo_syntax::parse_program(LOCAL_OPS)
        .unwrap_or_else(|e| panic!("the guards library parses: {} at {}", e.message, e.span));
    f.registry.extend_from(&program);
    // The same registration gate the shipped library passes — a locally
    // registered op is not exempt from the venue and row-bound rules.
    ordo_plan::cost::check_registration(&f.schema, &f.registry)
        .unwrap_or_else(|e| panic!("the guards library registers: {e}"));
    f
}

/// A database of this suite's own, created if the cluster is fresh.
///
/// One per test function, not one per suite: `Fixture::reset` drops and
/// rebuilds `public`, and two of those running at once collide inside
/// Postgres's own catalog (`pg_type_typname_nsp_index`). Cargo runs test
/// functions in one binary concurrently, so "the suite's database" is not
/// a fine enough grain.
pub fn db(name: &str) -> Pg {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {name}"));
    Pg {
        db: name.into(),
        ..Pg::default()
    }
}

/// The read suite's shared, seeded database. Reads mutate nothing, so the
/// exec cases share one — rebuilt exactly once per test binary, because
/// two concurrent rebuilds race inside Postgres's catalog.
pub fn seeded_reads(f: &Fixture) -> Pg {
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let pg = db("ordo_gc_reads");
    ONCE.get_or_init(|| f.reset(&pg))
        .clone()
        .expect("fixture DDL applies and seeds");
    pg
}

/// The fixture source with an extra declaration appended — how the reject
/// cases (GC069, GC074–GC079, GC088, GC099, GC100) pose a new entity to
/// the compiler, since a guard walk must resolve against the real schema.
pub fn schema_with(extra: &str) -> Result<ordo_schema::Schema, String> {
    let src = format!("{}\n\n{}", ordo_conformance::FIXTURE, extra);
    // `e.code`, not the literal `parse_error` this used to hard-code: a
    // `ParseError` carries its class now (checklist 9r), and a harness that
    // stamps the class it EXPECTS cannot report the one the compiler chose
    // — which is how GC077's divergence stayed invisible.
    let program = ordo_syntax::parse_program(&src)
        .map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
    let keys = ordo_schema::KeyTable::from_env().map_err(|e| format!("keys: {e}"))?;
    ordo_schema::build_with_keys(&program, &keys).map_err(|e| format!("{}: {}", e.code, e.message))
}

/// The error class of a harness failure string — every entry point in this
/// harness puts the class first, so the class is the text up to the colon.
pub fn class_of(err: &str) -> &str {
    err.split(':').next().unwrap_or(err).trim()
}
