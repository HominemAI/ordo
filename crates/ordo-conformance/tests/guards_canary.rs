//! **The no-guards canary** (checklist 9o).
//!
//! Every other guard test in this repo asserts that a guarded read returns
//! the rows `docs/guards-conformance.md` says it must, and that a guarded
//! write is refused with the class the document names. All of those tests
//! are *green*. None of them, on its own, is evidence — because a suite
//! that would stay green with the authorization system deleted proves
//! nothing about the authorization system.
//!
//! This file is the check that makes the rest of them mean something. It
//! takes the compiled fixture, **strips every guard from the schema**, and
//! re-runs a representative slice of the guarded assertions against the
//! same database. Each one must now come out *wrong*: reads must return
//! rows the guard was hiding, and writes the guard refused must land.
//!
//! If a case in the table below passes with the guards removed, the case
//! is decoration, and this file says so by name.
//!
//! # How the guards are removed, and why this way
//!
//! `Schema::entities[i].guards` is the resolved, effective guard — own,
//! inherited down an owned subtree, or `acl`-derived. Everything
//! downstream reads it: `ordo_sema::bind_as` inlines the read guard as a
//! semi-join, and the write protocol asks `guard_filter_query` for the
//! caller's write view. Setting it to `None` is therefore the whole
//! authorization system turned off at its single source, and it is done
//! **here, in a test, on a value this test owns** — there is no runtime
//! flag, no environment variable and no production code path that can do
//! this. A `--no-guards` switch would be a far worse thing to add to a
//! codebase than a canary is a good thing to have in it.
//!
//! The neutering is proven to be *only* about guards, not about data: the
//! stripped schema generates **byte-identical DDL**. Same tables, same
//! columns, same constraints, same seed — so nothing but the guards can
//! account for any difference the cases below observe.

mod gc_support;

use gc_support::{ANA, ANON, BRAM, CLEO, DOV, GIL};
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_schema::ddl_for;
use serde_json::{json, Value};

/// What a failure of this file means, spelled out in the failure itself.
/// A canary that fails silently, or with a message a reader has to decode,
/// is a canary nobody acts on.
const VERDICT: &str = "\n\
    ────────────────────────────────────────────────────────────────────\n\
    THE NO-GUARDS CANARY FAILED.\n\
    \n\
    This assertion produced the SAME answer with every access guard\n\
    stripped out of the schema as it does with them in place. That means\n\
    the guarded case above is not testing the guard: it would stay green\n\
    if the authorization system were deleted, and so would any coverage\n\
    number that counts it.\n\
    \n\
    A green guards suite is only evidence if this file is red without\n\
    guards. Fix the case — or the guard — before trusting the suite.\n\
    ────────────────────────────────────────────────────────────────────";

/// The fixture as authored: guards intact.
fn guarded() -> Fixture {
    gc_support::fixture()
}

/// The same fixture with the authorization system removed.
///
/// Test-only by construction: it mutates a `Fixture` value this function
/// just created. Nothing in `crates/*/src` can reach this state.
fn unguarded() -> Fixture {
    let mut f = guarded();
    for e in &mut f.schema.entities {
        e.guards = None;
    }
    f
}

fn seeded(f: &Fixture, db: &str) -> Pg {
    let pg = gc_support::db(db);
    f.reset(&pg).expect("fixture DDL applies and seeds");
    pg
}

// ── 0. The neutering is about guards and nothing else ────────────────────

/// The stripped schema must generate **byte-identical DDL**.
///
/// This is what makes every case below a statement about authorization
/// rather than about the shape of the database. If stripping guards also
/// dropped a column or a constraint, a divergence downstream could be the
/// missing column talking, and the canary would prove nothing.
#[test]
fn stripping_guards_changes_no_table() {
    let g = guarded();
    let u = unguarded();
    assert_eq!(
        ddl_for(&u.schema).expect("neutered ddl"),
        g.ddl,
        "removing guards changed the generated DDL — the canary's cases \
         would then be measuring the schema, not the guards"
    );

    // And it really did remove something: the fixture is guard-bearing, so
    // a neutering that found nothing to neuter is a broken canary.
    let guarded_entities = g
        .schema
        .entities
        .iter()
        .filter(|e| e.guards.is_some())
        .count();
    assert!(
        guarded_entities >= 7,
        "the market fixture should carry guards on at least the seven \
         guard-cluster entities; found {guarded_entities}"
    );
    assert_eq!(
        u.schema
            .entities
            .iter()
            .filter(|e| e.guards.is_some())
            .count(),
        0,
        "the neutering must leave no guard standing"
    );
}

// ── 1. Read-guard assertions, with and without guards ────────────────────

/// One row of the canary table: a *guarded* corpus assertion, restated
/// here by its GC id, plus what the same read answers with guards gone.
struct ReadProbe {
    /// The corpus case this row makes load-bearing.
    case: &'static str,
    source: &'static str,
    caller: Option<&'static str>,
    /// `docs/guards-conformance.md`'s expected rows — the document's
    /// values, not the compiler's.
    guarded: Value,
}

/// A representative slice of the guarded read assertions: own guards
/// (GC001–GC003), a link entity's own guard (GC004), an inherited guard
/// (GC023), acl-derived guards and their inheritance (GC037, GC038,
/// GC046), a guest's single grant (GC059), the anonymous caller (GC072),
/// an aggregate over the guarded view (GC009), and a guard riding through
/// composition (GC049).
fn read_probes() -> Vec<ReadProbe> {
    vec![
        ReadProbe {
            case: "GC001",
            source: "taskBoard[title]",
            caller: ANA,
            guarded: json!([
                {"title": "audit books"}, {"title": "draft spec"}, {"title": "review spec"},
            ]),
        },
        ReadProbe {
            case: "GC002",
            source: "openTasks[title]",
            caller: DOV,
            guarded: json!([]),
        },
        ReadProbe {
            case: "GC003",
            source: "myProjects[name]",
            caller: CLEO,
            guarded: json!([{"name": "Beta"}, {"name": "Gamma"}]),
        },
        ReadProbe {
            case: "GC004",
            source: "teamRoster[member]",
            caller: CLEO,
            guarded: json!([{"member": "mem_2"}, {"member": "mem_3"}]),
        },
        ReadProbe {
            case: "GC009",
            source: "tasks[title]\nreduce { n: count }",
            caller: DOV,
            guarded: json!([{"n": 0}]),
        },
        ReadProbe {
            case: "GC023",
            source: "noteFeed[body]",
            caller: CLEO,
            guarded: json!([{"body": "site copy wip"}]),
        },
        ReadProbe {
            case: "GC037",
            source: "boardList[name]",
            caller: ANA,
            guarded: json!([{"name": "roadmap"}]),
        },
        ReadProbe {
            case: "GC038",
            source: "cardList[title]",
            caller: ANA,
            guarded: json!([{"title": "q3 goals"}, {"title": "q4 goals"}]),
        },
        ReadProbe {
            case: "GC046",
            source: "boardsAccess[member]",
            caller: GIL,
            guarded: json!([
                {"member": "mem_1"}, {"member": "mem_2"},
                {"member": "mem_3"}, {"member": "mem_g"},
            ]),
        },
        ReadProbe {
            case: "GC049",
            source: "specTasks[title]",
            caller: CLEO,
            guarded: json!([]),
        },
        ReadProbe {
            case: "GC059",
            source: "boardList[name]",
            caller: GIL,
            guarded: json!([{"name": "roadmap"}]),
        },
        ReadProbe {
            case: "GC072",
            source: "openTasks[title]",
            caller: ANON,
            guarded: json!([]),
        },
    ]
}

#[test]
fn every_guarded_read_answers_differently_without_guards() {
    let g = guarded();
    let pg = seeded(&g, "ordo_gc_canary_read");
    let u = unguarded();

    let mut checked = 0usize;
    for p in read_probes() {
        // Half one: the guarded answer is the document's answer.
        let got = g
            .run_case_as(&pg, p.source, &[], p.caller)
            .unwrap_or_else(|e| panic!("{} `{}` as {:?}: {e}", p.case, p.source, p.caller));
        assert_eq!(
            got, p.guarded,
            "{}: the guarded read must return what guards-conformance.md states",
            p.case
        );

        // Half two, and the point of this file: with the guard gone, it
        // must NOT.
        let bare = u
            .run_case_as(&pg, p.source, &[], p.caller)
            .unwrap_or_else(|e| panic!("{} unguarded `{}`: {e}", p.case, p.source));
        assert_ne!(
            bare, p.guarded,
            "{} (`{}` as {:?}) is not load-bearing.{VERDICT}",
            p.case, p.source, p.caller
        );
        checked += 1;
    }
    println!("no-guards canary: {checked} guarded read assertions are load-bearing");
}

// ── 2. Write-guard refusals, with and without guards ─────────────────────

/// A guarded write the corpus says must be REFUSED. With guards it fails
/// with the document's class; without them it must succeed — a refusal
/// that survives the neutering was never the guard's doing.
struct WriteProbe {
    case: &'static str,
    call: &'static str,
    caller: Option<&'static str>,
    /// The error class `docs/guards-conformance.md` states.
    class: &'static str,
}

fn write_probes() -> Vec<WriteProbe> {
    vec![
        // Readable but unwritable: the informative refusal (R-G10).
        WriteProbe {
            case: "GC012",
            call: r#"renameTask($task: "task_1", $title: "hijack")"#,
            caller: BRAM,
            class: "access_guard",
        },
        // Outside the read view: indistinguishable from absent (R-G10).
        WriteProbe {
            case: "GC013",
            call: r#"renameTask($task: "task_1", $title: "x")"#,
            caller: DOV,
            class: "not_found",
        },
        // Create denied against the would-be row (R-G11).
        WriteProbe {
            case: "GC015",
            call: r#"addTask($project: "proj_1", $title: "sneak")"#,
            caller: BRAM,
            class: "access_guard",
        },
        // Create denied on the parent, which the caller cannot read.
        WriteProbe {
            case: "GC016",
            call: r#"addTask($project: "proj_1", $title: "sneak")"#,
            caller: DOV,
            class: "not_found",
        },
        // Read guard ≠ write guard: Ana reads proj_3, only Cleo writes it.
        WriteProbe {
            case: "GC021",
            call: r#"renameProject($project: "proj_3", $name: "Mine")"#,
            caller: ANA,
            class: "access_guard",
        },
        // Inherited write guard, depth 2.
        WriteProbe {
            case: "GC025",
            call: r#"addNote($task: "task_1", $body: "drive-by")"#,
            caller: BRAM,
            class: "access_guard",
        },
        // Override replaces the inherited guard: task_5 is writable by
        // nobody, and Cleo cannot even see it.
        WriteProbe {
            case: "GC029",
            call: r#"closeTask($task: "task_5")"#,
            caller: CLEO,
            class: "not_found",
        },
        // acl: read level cannot write (R-G14).
        WriteProbe {
            case: "GC040",
            call: r#"finishCard($card: "card_1")"#,
            caller: CLEO,
            class: "access_guard",
        },
        // acl: create into the owned child at read level.
        WriteProbe {
            case: "GC041",
            call: r#"addCard($board: "board_1", $title: "sneak")"#,
            caller: CLEO,
            class: "access_guard",
        },
        // Guest write attempt.
        WriteProbe {
            case: "GC061",
            call: r#"addCard($board: "board_1", $title: "guest card")"#,
            caller: GIL,
            class: "access_guard",
        },
    ]
}

#[test]
fn every_guarded_write_refusal_disappears_without_guards() {
    let g = guarded();
    let u = unguarded();

    let mut checked = 0usize;
    for p in write_probes() {
        // Half one: with guards, the corpus's class.
        let pg = seeded(&g, "ordo_gc_canary_write");
        let mut log = WriteLog::new();
        let err = g
            .run_write_as(&pg, &mut log, p.call, p.caller)
            .expect_err(&format!("{}: `{}` must be refused", p.case, p.call));
        assert!(
            err.starts_with(p.class),
            "{}: expected `{}`, got: {err}",
            p.case,
            p.class
        );

        // Half two: with the guard gone, the same call must LAND. A write
        // still refused here was refused by something other than the
        // authorization system — a foreign key, a mode rule, a typo — and
        // the case above is not evidence of a guard.
        let pg = seeded(&g, "ordo_gc_canary_write");
        let mut log = WriteLog::new();
        match u.run_write_as(&pg, &mut log, p.call, p.caller) {
            Ok(_) => checked += 1,
            Err(e) => panic!(
                "{} (`{}` as {:?}) is not load-bearing: it is still refused \
                 with every guard removed, as `{e}`.{VERDICT}",
                p.case, p.call, p.caller
            ),
        }
    }
    println!("no-guards canary: {checked} guarded write refusals are load-bearing");
}

/// The other direction, and the one a coverage count cannot see: with
/// guards removed, a caller who legitimately holds nothing can reach
/// everything. This is the whole system in one assertion — Dov holds no
/// membership row, no ACL row and no ownership anywhere in the seed.
#[test]
fn without_guards_a_principal_who_holds_nothing_reaches_everything() {
    let g = guarded();
    let pg = seeded(&g, "ordo_gc_canary_reach");
    let u = unguarded();

    for (case, src, projected) in [
        ("GC001", "taskBoard[title]", 5),
        ("GC003", "myProjects[name]", 3),
        ("GC004", "teamRoster[member]", 5),
        ("GC023", "noteFeed[body]", 3),
        ("GC037", "boardList[name]", 2),
        ("GC038", "cardList[title]", 3),
        ("GC046", "boardsAccess[member]", 5),
    ] {
        let held = g
            .run_case_as(&pg, src, &[], DOV)
            .expect("guarded read runs");
        assert_eq!(
            held.as_array().unwrap().len(),
            0,
            "{case}: Dov holds nothing, so `{src}` must be empty for him"
        );
        let bare = u
            .run_case_as(&pg, src, &[], DOV)
            .expect("unguarded read runs");
        assert_eq!(
            bare.as_array().unwrap().len(),
            projected,
            "{case}: with guards removed `{src}` must hand Dov every row \
             in the table — if it does not, the guard was not what was \
             hiding them.{VERDICT}"
        );
    }
}
