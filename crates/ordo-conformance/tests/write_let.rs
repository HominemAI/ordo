//! Checklist 9e — **`let x = set E { … }`**, the write-valued `let`.
//!
//! `definition-language.md` §3, *"Naming a freshly created row in the same
//! op"* **[settled]**, restated as R-G19 in `guards-conformance.md` and
//! again in `traps.md`, `modeling-guidance.md`, `tutorial.md` and
//! `sql-translation-corpus-2.md`. Five documents spell it and the parser
//! had no production for it at all, which is why `unbindable_write` sat in
//! the closed inventory with **no producer** — a class that refuses a shape
//! nobody could write down.
//!
//! # What is proved here that a unit test cannot prove
//!
//! 1. **The bound id is the id that was minted.** A descriptor whose `.id`
//!    was merely *plausible* would pass every compile-time check and leave
//!    a foreign key pointing at nothing. The only witness is the row in the
//!    database, reached through the FK the second statement wrote.
//! 2. **One command group.** R-G19's whole claim is that the create and
//!    the dependent write are *one* atomic group — "statements in a command
//!    group execute in order" — so the log has to show one group with both
//!    facts in it, not two groups that happened to succeed.
//! 3. **A non-`id` member is the create's own value.** `r.name` and the
//!    `name:` it came from must be the same value by construction, not by
//!    coincidence of two evaluations agreeing today.
//! 4. **`unbindable_write` now has a producer**, and it is the derived mode
//!    that decides — not a syntactic guess about the braces.
//!
//! Real Postgres, this suite's own databases (`ordo_9ed_let_*`).

mod gc_support;

use gc_support::ANA;
use ordo_commandlog::store::Store;
use ordo_commandlog::SpaceId;
use ordo_conformance::{Fixture, Pg, WriteLog, FIXTURE, SEED};
use serde_json::json;

/// A cluster of its own, so nothing here moves a number another suite
/// asserts about the shipped fixture.
///
/// Unguarded on purpose. R-G19's guard interaction (GC094: the create is
/// exempt under G10a, the dependent write is not, and it passes only
/// because the create's auto-grant is *already in the group*) needs the
/// harness to apply each statement before planning the next one, and this
/// harness plans every statement first and applies them afterwards. That
/// is a property of the write protocol's staging, not of `let`, and it is
/// recorded rather than worked around — see the checklist entry for 9e.
/// What this file proves is everything `let` itself owes.
const CLUSTER: &str = r#"
entity rooms (key: rm) {
  name: text
}

entity roomLogs (key: rl) {
  room -> rooms
  note: text = "opened"
}

# A unique key of its own, so the converge-or-create mode has somewhere to
# be posed without making every `set rooms { … }` above a keyed write.
entity codedRooms (key: cr) {
  code: text  unique
  name: text
}
"#;

const CLUSTER_OPS: &str = r#"
op openRoom($n: text) unguarded =
  let r = set rooms { name: $n }
  set roomLogs { room: r.id, note: r.name }

op openRoomPlain($n: text) unguarded =
  let r = set rooms { name: $n }
  set roomLogs { room: r.id }

# Two bindings in one group: a third statement names both. Nothing about
# the rule is limited to one binding.
op openTwo($a: text, $b: text) unguarded =
  let x = set rooms { name: $a }
  let y = set rooms { name: $b }
  set roomLogs { room: x.id, note: y.name }
"#;

fn fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{CLUSTER}\n{CLUSTER_OPS}");
    Fixture::from_source(&src, SEED).expect("the fixture plus the write-valued lets compiles")
}

/// Registration only: the op library above with one more op appended,
/// answering whether the compiler accepts it and with which class.
fn registers(op: &str) -> Result<(), String> {
    let src = format!("{FIXTURE}\n{CLUSTER}\n{CLUSTER_OPS}\n{op}");
    Fixture::from_source(&src, SEED).map(|_| ())
}

/// A registration failure arrives as `register: [<class>] <message>`.
fn class_of(e: &str) -> &str {
    e.split_once('[')
        .and_then(|(_, r)| r.split_once(']'))
        .map(|(c, _)| c)
        .unwrap_or(e)
}

/// The spec's example, executed: create a row, name it, and point the next
/// statement at it. The id in the foreign key is the id the mint produced,
/// and the database is what says so.
#[test]
fn a_named_create_hands_its_id_to_the_next_statement() {
    let f = fixture();
    let pg = Pg::named("ordo_9ed_let_id");
    f.reset(&pg).expect("fresh seed");
    let mut log = WriteLog::new();

    let r = f
        .run_write_as(&pg, &mut log, r#"openRoom($n: "atrium")"#, ANA)
        .expect("the create-and-seed shape applies");
    assert_eq!(
        r,
        json!([
            { "entity": "rooms",    "created": "rm_w1" },
            { "entity": "roomLogs", "created": "rl_w2" },
        ]),
        "two creates, reported in statement order"
    );

    // The join is the assertion. A descriptor that guessed an id would
    // have left `room` pointing at a row that does not exist, and the FK
    // would have refused the insert — but a `let` that silently bound
    // NOTHING and left the column null would not have, so the readback
    // checks the value and not merely that the write survived.
    assert_eq!(
        pg.query(
            r#"SELECT l."id", r."name", l."note" FROM "roomLogs" l
               JOIN "rooms" r ON r."id" = l."room" AND r."$space" = l."$space""#
        )
        .expect("readback"),
        vec![vec![
            Some("rl_w2".into()),
            Some("atrium".into()),
            // `r.name` is the create's OWN expression for `name`, carried
            // into the second statement rather than re-derived — so this
            // cell and the one beside it cannot disagree.
            Some("atrium".into()),
        ]],
        "the FK resolves, and the non-id descriptor member is the created \
         row's own value"
    );
}

/// R-G19: *"two statements in one atomic group"*. The log is where that is
/// visible — one group, both commands, in the order the op wrote them.
#[test]
fn the_create_and_the_dependent_write_are_one_command_group() {
    let f = fixture();
    let pg = Pg::named("ordo_9ed_let_group");
    f.reset(&pg).expect("fresh seed");
    let mut log = WriteLog::new();

    f.run_write_as(&pg, &mut log, r#"openRoomPlain($n: "annex")"#, ANA)
        .expect("applied");

    let space = SpaceId("conformance".into());
    assert_eq!(log.store.len(&space), 1, "ONE group, not two");
    let g = log.store.get(&space, 0).expect("the group");
    assert_eq!(g.group.commands.len(), 2, "with both facts in it");
    let entities: Vec<&str> = g.group.commands.iter().map(|c| c.entity.as_str()).collect();
    assert_eq!(
        entities,
        vec!["rooms", "roomLogs"],
        "in statement order — which is what makes the second able to name \
         the first"
    );
}

/// Nothing in the rule is limited to one binding, and a later statement may
/// name any earlier one.
#[test]
fn two_bindings_in_one_group_stay_distinct() {
    let f = fixture();
    let pg = Pg::named("ordo_9ed_let_two");
    f.reset(&pg).expect("fresh seed");
    let mut log = WriteLog::new();

    f.run_write_as(&pg, &mut log, r#"openTwo($a: "east", $b: "west")"#, ANA)
        .expect("applied");
    assert_eq!(
        pg.query(
            r#"SELECT r."name", l."note" FROM "roomLogs" l
               JOIN "rooms" r ON r."id" = l."room" AND r."$space" = l."$space""#
        )
        .expect("readback"),
        vec![vec![Some("east".into()), Some("west".into())]],
        "the log's ROOM is x and its NOTE is y — the two descriptors did not \
         collapse into one"
    );
}

/// **`unbindable_write` gets its first producer.**
///
/// DL: *"`let` binds a `set` whose derived mode is CREATE… The other modes
/// bind nothing and are a compile error (`unbindable_write`): CONVERGE and
/// CAS address a row whose id the caller already holds — there is nothing
/// to learn — and BULK affects an unknown number of rows, so there is no
/// single `.id` to name."* All three, plus the composite converge-or-create
/// that DL does not spell but that the same reason covers.
#[test]
fn only_a_create_can_be_bound() {
    // CONVERGE — `id` in the braces addresses.
    let e = registers("op a($i: id, $n: text) unguarded =\n  let r = set rooms { id: $i, name: $n }\n  set roomLogs { room: r.id }")
        .expect_err("a converge binds nothing");
    assert_eq!(class_of(&e), "unbindable_write", "{e}");
    assert!(e.contains("converges"), "the DERIVED mode is named: {e}");

    // CAS — `id` + `rev`, still an address.
    let e = registers("op b($i: id, $v: int, $n: text) unguarded =\n  let r = set rooms { id: $i, rev: $v, name: $n }\n  set roomLogs { room: r.id }")
        .expect_err("a CAS binds nothing");
    assert_eq!(class_of(&e), "unbindable_write", "{e}");

    // BULK — a ranged write has no single row.
    let e = registers("op c($n: text, $m: text) unguarded =\n  let r = set rooms(name == $n) { name: $m }\n  set roomLogs { room: r.id }")
        .expect_err("a bulk write binds nothing");
    assert_eq!(class_of(&e), "unbindable_write", "{e}");
    assert!(e.contains("bulk"), "{e}");

    // CONVERGE-OR-CREATE by a unique key. DL's list says CONVERGE; this is
    // the mode that MIGHT converge, and might is enough — the statement can
    // return a row the caller already had, so there is no fresh id to name.
    let e = registers("op d($c: text, $n: text) unguarded =\n  let r = set codedRooms { code: $c, name: $n }\n  set roomLogs { room: r.id }")
        .expect_err("converge-or-create binds nothing");
    assert_eq!(class_of(&e), "unbindable_write", "{e}");
    assert!(e.contains("creates-or-converges"), "{e}");

    // …and the one case the spec does not spell, ruled and pinned: a
    // `rev: null` create-once **is** bindable. Its derived label is already
    // `creates` (§6 rule 1 — "the statement asserts the row's absence, so
    // converging is not one of the things it can do"), and DL's exclusion
    // list is given with its reasons, none of which this shape matches: it
    // addresses no row by an id the caller holds, and it affects exactly
    // one row. Refusing it would refuse a statement for a property it does
    // not have.
    registers("op e($c: text, $n: text) unguarded =\n  let r = set codedRooms { code: $c, rev: null, name: $n }\n  set roomLogs { room: r.id }")
        .expect("a create-once produces exactly one row with one fresh id");
}

/// The scope rule: *"usable in later statements of the same op, never in
/// the same statement and never in a condition of the statement that
/// produced it."* Self-reference and forward reference are the same check —
/// the name is simply not bound yet — and both are `invalid_binding_expr`,
/// the class for a right-hand side whose shape the statement cannot mean.
/// No class was minted for them.
#[test]
fn a_name_reaches_only_the_statements_after_it() {
    // Forward: the reference precedes the binding.
    let e = registers(
        "op f($n: text) unguarded =\n  set roomLogs { room: r.id }\n  let r = set rooms { name: $n }",
    )
    .expect_err("a name is not in scope before it is bound");
    assert_eq!(class_of(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("no bound row"), "{e}");

    // Self: the reference is in the statement that binds it.
    let e = registers("op g($n: text) unguarded =\n  let r = set rooms { name: r.name }")
        .expect_err("a name is not in scope in its own statement");
    assert_eq!(class_of(&e), "invalid_binding_expr", "{e}");

    // A descriptor is `id`, `rev` and the fields the statement WROTE —
    // not the entity's other columns. `code` exists on `rooms`; this
    // create did not write it, so the descriptor does not carry it.
    let e = registers("op h($n: text) unguarded =\n  let r = set rooms { name: $n }\n  set roomLogs { room: r.id, note: r.code }")
        .expect_err("a row descriptor is not a row");
    assert_eq!(class_of(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("descriptor"), "{e}");

    // One name, one created row: a second binding of the same name would
    // make every later reference ambiguous.
    let e = registers("op i($n: text) unguarded =\n  let r = set rooms { name: $n }\n  let r = set rooms { name: $n }\n  set roomLogs { room: r.id }")
        .expect_err("a name is bound once");
    assert_eq!(class_of(&e), "invalid_binding_expr", "{e}");
    assert!(e.contains("bound twice"), "{e}");
}
