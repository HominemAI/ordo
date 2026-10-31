//! Checklist 9p — creates through a MULTI-HOP inherited guard.
//!
//! `resolve_guards` walks owned refs to a fixed point, so a guard reaches
//! arbitrarily far down an ownership chain and `Guards::rebase` records the
//! hops it travelled. Reads have always honoured the whole chain
//! (`guard_cond_of` nests one `EXISTS` per hop). Creates refused past the
//! first hop:
//!
//! ```text
//! if g.rebase.len() != 1 { invalid_guard_walk:
//!     "create through a {n}-hop inherited guard is not yet supported" }
//! ```
//!
//! which made a grandchild under a guarded root **uncreatable** — not
//! forbidden to some callers, uncreatable by anyone, including the root's
//! own administrator. A refusal nobody can satisfy is not a guard.
//!
//! # Why the shape is composed here rather than added to `market.ordo`
//!
//! The shipped fixture has two three-level ownership chains
//! (`orderItems → orders → customers`, `taskNotes → tasks → projects`) and
//! neither produces a two-hop REBASE: `orders`/`customers` are unguarded,
//! and `tasks` carries its own `access` clauses, so `taskNotes` inherits
//! from `tasks` across one hop. A two-hop rebase needs a child of an
//! entity that is itself an inheritor — one more level. Adding it to
//! `market.ordo` would move the DDL golden, the plan artifact and the
//! entity counts several other suites assert about the shipped fixture, so
//! the level is added here, over the fixture's own guarded cluster:
//!
//! ```text
//! noteTags ──note──▶ taskNotes ──task──▶ tasks   (guard-bearing)
//!                                          └─project─▶ projects
//! ```
//!
//! `taskNotes` inherits `tasks`' clauses through `[task]`; `noteTags`
//! inherits the same clauses through `[note, task]`. The walk evaluated at
//! the far end is `tasks`' own write guard,
//! `project.projectMembers(member == $caller and canWrite)`.
//!
//! # What is asserted
//!
//! Both halves, because a generalisation that permits everything is worse
//! than the refusal it replaced:
//!
//! - Ana holds `canWrite` on proj_1 → the create LANDS, and the row is
//!   readable afterwards.
//! - Bram is a member of proj_1 with `canWrite = false` → the create is
//!   refused `access_guard`, and refused *there*: he can read note_1
//!   perfectly well, so the parent-visibility check (G12) passes and it is
//!   the guard at the end of the two-hop walk that says no.
//! - Dov is a member of nothing → he cannot even see the parent, so he
//!   gets `not_found`, which is R-G10 rather than the guard.

mod gc_support;

use gc_support::{class_of, db, ANA, ANON, BRAM, DOV};
use ordo_conformance::{Fixture, Pg, WriteLog, FIXTURE, SEED};
use serde_json::Value;

/// One more level under the fixture's guarded cluster, plus the two ops
/// that write and read it. Nothing here declares an `access` clause: the
/// point is what `noteTags` INHERITS.
const GRANDCHILD: &str = r#"
entity noteTags (key: ntag) {
  note  -> taskNotes  owned
  label: text
}

op addNoteTag($note: id, $label: text) =
  set noteTags { note: $note, label: $label }

op tagFeed unbounded =
  noteTags[label, note]
  sort label
"#;

fn deep_fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{GRANDCHILD}");
    Fixture::from_source(&src, SEED).expect("the fixture plus one more owned level compiles")
}

fn fresh(f: &Fixture, pg: &Pg) -> WriteLog {
    f.reset(pg).expect("fresh seed");
    WriteLog::new()
}

/// The guard `noteTags` inherits, and the path it inherits it along.
#[test]
fn a_grandchild_inherits_the_root_guard_across_two_hops() {
    let f = deep_fixture();
    let e = f
        .schema
        .entity("noteTags")
        .expect("noteTags is in the schema");
    let g = e.guards.as_ref().expect("noteTags inherits a guard");

    assert_eq!(
        g.source,
        ordo_schema::GuardSource::Inherited {
            from: "tasks".into()
        },
        "the nearest guard-BEARING ancestor is `tasks`; `taskNotes` only passes it on"
    );
    assert_eq!(
        g.rebase,
        vec!["note".to_string(), "task".to_string()],
        "two hops: the ref on the created row, then the ref on the row it names"
    );
    // Rendered, so `ordo check` and every manifest say what is actually
    // enforced rather than "inherited, somehow".
    assert_eq!(
        g.render(false),
        "note.task.project.projectMembers(member == $caller and canWrite)"
    );
}

/// The create gate compiles to one pinned ancestor and one chased ref, and
/// it is no longer a refusal.
#[test]
fn the_create_gate_pins_the_supplied_parent_and_chases_the_rest() {
    let f = deep_fixture();
    let vals = std::collections::BTreeMap::from([
        (
            "note".to_string(),
            ordo_sema::CreateVal::Id("note_1".into()),
        ),
        (
            "label".to_string(),
            ordo_sema::CreateVal::Text("urgent".into()),
        ),
    ]);
    let gate = ordo_sema::create_write_query(&f.schema, "noteTags", &vals, "mem_1")
        .expect("a two-hop create gate compiles — it used to be `invalid_guard_walk`");
    let ordo_sema::CreateGate::Check(q) = gate else {
        panic!("an inherited guard is never create-exempt: the ancestor already exists")
    };
    let sql = ordo_plan::plan(&q, &f.plan_space())
        .expect("the gate plans")
        .sql;

    // Hop zero is the value the caller supplied — there is no row to read
    // it from — and every hop above it is an ordinary ref chase.
    assert!(
        sql.contains("'note_1'"),
        "the supplied parent is pinned:\n{sql}"
    );
    for t in ["taskNotes", "tasks", "projectMembers", "members"] {
        assert!(sql.contains(t), "the walk reaches `{t}`:\n{sql}");
    }
    // Not a `taskNotes` guard: the walk's text belongs to `tasks`, and the
    // verb is WRITE, so `canWrite` must be in it.
    assert!(
        sql.contains("canWrite"),
        "the WRITE walk, not the read one:\n{sql}"
    );
    // Space-scoped like every other planned sub-query (the `$space` column
    // landed this week; guard sub-queries get it from the planner).
    assert!(
        sql.matches("\"$space\"").count() >= 4,
        "every table in the chain is space-scoped:\n{sql}"
    );
}

/// A minted id longer than 40 characters reaches the gate as `Text`: the
/// write path's literal classifier guesses ids by their spelling. The gate
/// knows the field is a ref, so it pins the parent anyway. Before this, both
/// an inherited and an own walk through a ref compiled to `false`, and the
/// shopfront example's owner could not add a product to her own shop
/// (2026-10-10).
#[test]
fn the_create_gate_pins_a_long_minted_parent_id() {
    let f = deep_fixture();
    let long = "note_w01a12523ee9c7c1acb15783fbcd0_000000000001";
    assert!(long.len() > 40);
    let vals = std::collections::BTreeMap::from([
        ("note".to_string(), ordo_sema::CreateVal::Text(long.into())),
        (
            "label".to_string(),
            ordo_sema::CreateVal::Text("urgent".into()),
        ),
    ]);
    let ordo_sema::CreateGate::Check(q) =
        ordo_sema::create_write_query(&f.schema, "noteTags", &vals, "mem_1").expect("compiles")
    else {
        panic!("an inherited guard is never create-exempt")
    };
    let sql = ordo_plan::plan(&q, &f.plan_space()).expect("plans").sql;
    assert!(
        sql.contains(&format!("'{long}'")),
        "inherited hop pinned:\n{sql}"
    );

    // An own walk through a ref: `tasks`' `project.projectMembers(...)`.
    let long = "proj_w01a12523ee9c7c1acb15783fbcd0_000000000001";
    let vals = std::collections::BTreeMap::from([
        (
            "project".to_string(),
            ordo_sema::CreateVal::Text(long.into()),
        ),
        ("title".to_string(), ordo_sema::CreateVal::Text("t".into())),
    ]);
    let ordo_sema::CreateGate::Check(q) =
        ordo_sema::create_write_query(&f.schema, "tasks", &vals, "mem_1").expect("compiles")
    else {
        panic!("a parent-rooted guard is never create-exempt")
    };
    let sql = ordo_plan::plan(&q, &f.plan_space()).expect("plans").sql;
    assert!(sql.contains(&format!("'{long}'")), "own hop pinned:\n{sql}");
    assert!(sql.contains("projectMembers"), "{sql}");
}

/// The two halves that matter, against real Postgres.
#[test]
fn a_two_hop_create_permits_the_writer_and_refuses_the_reader() {
    let f = deep_fixture();
    let pg = db("ordo_gc_multihop");
    let mut log = fresh(&f, &pg);

    // ── permitted ───────────────────────────────────────────────────────
    // note_1 → task_1 → proj_1, and Ana holds pm_1 on proj_1 with
    // `canWrite = true`. Two hops of rebase, then the ancestor's walk.
    let out = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addNoteTag($note: "note_1", $label: "urgent")"#,
            ANA,
        )
        .expect("Ana may write in proj_1, so she may tag a note in it");
    let created = out[0]["created"]
        .as_str()
        .expect("a created id")
        .to_string();
    assert!(
        created.starts_with("ntag_"),
        "the row is minted under its own key: {created}"
    );

    // It landed, and it reads back through the same two-hop guard.
    assert_eq!(
        f.run_case_as(&pg, "tagFeed[label]", &[], ANA)
            .expect("the read plans and runs"),
        serde_json::json!([{"label": "urgent"}])
    );

    // ── refused, and refused by the GUARD ───────────────────────────────
    // Bram is a member of proj_1 (pm_2) with `canWrite = false`. He can
    // read note_1, so G12's parent-visibility check passes and the answer
    // comes from the guard at the far end of the walk: `access_guard`, the
    // informative refusal, on a row he can already see (R-G10).
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addNoteTag($note: "note_1", $label: "sneaky")"#,
            BRAM,
        )
        .expect_err("Bram reads proj_1 but does not write it");
    assert_eq!(class_of(&e), "access_guard", "{e}");
    assert_eq!(
        rows(&f, &pg, "tagFeed[label]", ANA),
        Value::Array(vec![]),
        "a refused create applies nothing"
    );

    // Dov holds no membership anywhere, so he never reaches the guard:
    // the parent is outside his read view and absence is the answer.
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addNoteTag($note: "note_1", $label: "sneaky")"#,
            DOV,
        )
        .expect_err("Dov cannot see note_1 at all");
    assert_eq!(class_of(&e), "not_found", "{e}");

    // And an anonymous caller is refused too — the walk's `$caller` slot
    // has nothing to bind.
    assert!(
        f.run_write_as(
            &pg,
            &mut log,
            r#"addNoteTag($note: "note_1", $label: "sneaky")"#,
            ANON,
        )
        .is_err(),
        "no principal, no guard"
    );

    // ── a parent that does not exist ────────────────────────────────────
    // Hop zero is pinned by a literal, so a bad id makes the chain empty
    // rather than making the guard vacuous.
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addNoteTag($note: "note_404", $label: "urgent")"#,
            ANA,
        )
        .expect_err("there is no note_404");
    assert_eq!(class_of(&e), "not_found", "{e}");

    // ── the other project ───────────────────────────────────────────────
    // note_3 → task_5 → proj_3, where Ana is a member with
    // `canWrite = false` (pm_5). Same caller, same op, different end of
    // the walk: the second hop is what decides.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addNoteTag($note: "note_3", $label: "urgent")"#,
            ANA,
        )
        .expect_err("Ana reads proj_3 but does not write it");
    assert_eq!(class_of(&e), "access_guard", "{e}");

    // Cleo holds pm_4 on proj_2 with `canWrite = true`; note_2 → task_3 →
    // proj_2. The permitted half is not "Ana specifically".
    let out = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addNoteTag($note: "note_2", $label: "copy")"#,
            gc_support::CLEO,
        )
        .expect("Cleo writes proj_2");
    assert!(out[0]["created"].is_string());
}

fn rows(f: &Fixture, pg: &Pg, case: &str, who: Option<&str>) -> Value {
    f.run_case_as(pg, case, &[], who).expect("the read runs")
}
