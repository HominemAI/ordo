//! **R-G20 — a routine's bound follows its trigger's provenance, not its
//! principal** (`guards-conformance.md`; `definition-language.md` §8,
//! "Trigger provenance sets the bound [settled — supersedes the blanket rule
//! above]"), against real Postgres and the `market` fixture the guards
//! corpus uses.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).
//!
//! # What is proved here, and what is not
//!
//! §7 and §8 give a tier-1 routine exactly one way to write: `each ->`, "per
//! row proposal emission" over the pipeline the routine just read. So a
//! routine's write set is a **function of its read set** — that is not a
//! shortcut this suite takes, it is the reason §8 gave routines `each ->`
//! instead of a general write language. Bound the read and the emission is
//! bounded by construction, which is why the decisive case in the corpus
//! (GC096's probe) is itself a read probe: replace the body's source with an
//! unfiltered `tasks[id, title]` and the run must *still* touch task_6 only.
//!
//! These cases therefore assert the row sets the three provenance arms make
//! visible, and assert the emission set as the function of them that
//! `each ->` defines. They are named for the corpus cases whose rule they
//! prove, not transcribed from them: `guards_corpus.rs` owns GC096–GC098 and
//! their `routine …` bodies need a declaration form that is `[lean]` and not
//! owed.

use ordo_conformance::{canonical, Fixture, Pg};
use ordo_exec::triggers::{self, TriggerContext, Woken};
use ordo_plan::plan;
use ordo_sema::{Clock, Provenance};
use ordo_syntax::OpFlags;
use serde_json::Value;

/// The three fixture principals with a stake in `tasks`, plus the owner and
/// the guest. GC029's fact this suite leans on: **no principal in the
/// fixture may write task_5** (proj_3's only member, mem_1, does not hold
/// `canWrite`).
const PRINCIPALS: &[Option<&str>] = &[
    Some("mem_1"),
    Some("mem_2"),
    Some("mem_3"),
    Some("mem_4"),
    Some("mem_g"),
    Some("mem_o"),
    None,
];

fn seeded(db: &str) -> (Fixture, Pg) {
    let f = Fixture::load().expect("the market fixture compiles");
    let pg = Pg::named(db);
    // A database per test function: `reset` starts with `DROP SCHEMA public
    // CASCADE`, and two suites sharing one database tear each other down.
    f.reset(&pg).expect("fixture DDL applies and seeds");
    (f, pg)
}

/// The corpus's ingested row: `task_6` in `proj_1`, arriving from outside.
fn ingest_task_6(f: &Fixture, pg: &Pg) {
    pg.exec(&format!(
        "INSERT INTO \"tasks\" (\"$space\", \"id\", \"project\", \"assignee\", \
           \"title\", \"status\") VALUES ('{}', 'task_6', 'proj_1', NULL, \
           'ingested work', 'open')",
        f.space().0
    ))
    .expect("ingest task_6");
}

/// Run a routine body under a given provenance and `run` header.
///
/// This is the routine runner, in the two lines that matter: the bound is
/// resolved from the trigger (`bound_for`), and the body is bound under it
/// (`bind_routine`). Everything else is the ordinary read path.
fn run_routine(
    f: &Fixture,
    pg: &Pg,
    source: &str,
    provenance: Provenance,
    flags: OpFlags,
) -> Value {
    let pipeline = ordo_syntax::parse_pipeline(source)
        .unwrap_or_else(|e| panic!("{source}: {} {}", e.code, e.message));
    let mut q = ordo_sema::bind_routine(
        &f.schema,
        &f.registry,
        &pipeline,
        &[],
        provenance,
        flags,
        Clock::CORPUS,
    )
    .unwrap_or_else(|e| panic!("{source}: bind: {e}"));
    f.resolve_blind(&mut q, &[]).expect("blind");
    let p = plan(&q, &f.plan_space()).unwrap_or_else(|e| panic!("{source}: plan: {e}"));
    let mut rows = pg.query(&p.sql).unwrap_or_else(|e| panic!("{source}: {e}"));
    f.open_host_values(&mut rows, &p).expect("host open");
    canonical(&rows, &p.outputs)
}

/// The SQL a routine body plans to. Two cases below assert on the plan
/// rather than on rows, because what they are about is a filter's PRESENCE
/// and a filter's ABSENCE — and `Provenance::Caller` leaves `$caller` a
/// prepared slot, which is a plan the harness's `psql` cannot execute and is
/// exactly the right shape for a registered op.
fn plan_sql(f: &Fixture, source: &str, provenance: Provenance, flags: OpFlags) -> String {
    let pipeline = ordo_syntax::parse_pipeline(source)
        .unwrap_or_else(|e| panic!("{source}: {} {}", e.code, e.message));
    let q = ordo_sema::bind_routine(
        &f.schema,
        &f.registry,
        &pipeline,
        &[],
        provenance,
        flags,
        Clock::CORPUS,
    )
    .unwrap_or_else(|e| panic!("{source}: bind: {e}"));
    plan(&q, &f.plan_space())
        .unwrap_or_else(|e| panic!("{source}: plan: {e}"))
        .sql
}

/// The ids a result names, in order — the emission set `each ->` would walk.
fn ids(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("rows")
        .iter()
        .map(|r| r["id"].as_str().expect("id").to_string())
        .collect()
}

fn unbounded() -> OpFlags {
    OpFlags {
        unbounded: true,
        ..OpFlags::default()
    }
}

fn unbounded_unguarded() -> OpFlags {
    OpFlags {
        unbounded: true,
        unguarded: true,
        ..OpFlags::default()
    }
}

/// The listener bound for a run woken on `task_6`, over the entities a body
/// might name.
fn woken_on_task_6(f: &Fixture, pg: &Pg) -> Provenance {
    let ctx = TriggerContext {
        group: Some("cg_ingest".into()),
        entity: Some("tasks".into()),
        ids: vec!["task_6".into()],
        events: vec!["cmd_1".into()],
        at: 100,
    };
    triggers::bound_for(
        pg,
        &f.schema,
        &Woken::Listener {
            entity: "tasks".into(),
        },
        &ctx,
        &[
            "tasks".into(),
            "taskNotes".into(),
            "projects".into(),
            "boards".into(),
        ],
    )
    .expect("bound")
}

// ── clock: guards pass (the rule GC097 asserts) ──────────────────────────

/// **Clock-woken: guards pass.** Nobody outside chose the moment, so there
/// is no confused deputy to be.
///
/// The proof is not "it returned some rows": it is that the row set is one
/// **no principal in the fixture can obtain**, so nothing weaker than
/// guards-passing could have produced it. task_5 belongs to proj_3, whose
/// only member holds no `canWrite` — GC029 — and a clock-driven rule reaches
/// it anyway. That is the point: a clock-driven rule can never silently do
/// nothing.
#[test]
fn gc097_clock_woken_reaches_rows_no_principal_can() {
    let (f, pg) = seeded("ordo_trig_clock_pass");
    let src = "tasks(status == \"open\")[id]";

    let all = ids(&run_routine(&f, &pg, src, Provenance::Clock, unbounded()));
    assert_eq!(
        all,
        vec!["task_1", "task_3", "task_4", "task_5"],
        "all four open tasks, across proj_1, proj_2 AND proj_3"
    );

    // The separating evidence. Three plausible wrong implementations of
    // "guards pass" — bind as the owner, bind as an admin-ish member, bind
    // with no caller — and none of them produces this set.
    for who in PRINCIPALS {
        let seen = ids(&f
            .run_case_as(&pg, src, &[], *who)
            .unwrap_or_else(|e| panic!("{who:?}: {e}")));
        assert_ne!(
            seen, all,
            "{who:?} sees the same rows as the clock — then this proves nothing"
        );
    }
}

/// The rationale the corpus forbids, made executable.
///
/// `guards-conformance.md` R-G20: *"The rationale 'a routine holds no
/// members row, so `$caller` is null and every guard evaluates false' is
/// wrong and must not appear in a case justification: it describes R-G12's
/// anonymous caller, and it would make every routine useless rather than
/// bounded."*
///
/// So the two must not be the same code path, and here is the difference
/// between them, in rows.
#[test]
fn a_routine_is_not_an_anonymous_caller() {
    let (f, pg) = seeded("ordo_trig_not_anon");
    let src = "tasks(status == \"open\")[id]";

    let anonymous = ids(&f.run_case_as(&pg, src, &[], None).expect("anon read"));
    assert!(
        anonymous.is_empty(),
        "G15: an anonymous caller sees nothing, by rule"
    );

    let clock = ids(&run_routine(&f, &pg, src, Provenance::Clock, unbounded()));
    assert_eq!(clock.len(), 4, "a routine is bounded, not useless");
}

// ── listener: bounded to $trigger.ids (the rule GC096 asserts) ───────────

/// **The probe.** An unfiltered source, a `run` header carrying `unbounded`,
/// and the run still reaches exactly the ingested row.
///
/// GC096: *"replace the routine body's source with `tasks[id, title]`
/// (unfiltered, so the run header takes `run() unbounded:`) and re-run the
/// same ingest. → **still one note, on task_6 only.** Every row outside
/// `$trigger.ids` is invisible to the run exactly as a guard would make it —
/// the routine cannot be steered into work the outsider who fired the
/// trigger could never have done themselves."*
///
/// And the marker discipline that goes with it: **`unbounded` is an
/// acknowledgment about *work*, and it does not widen the listener bound.**
/// Only `unguarded` does. Two markers, two decisions, neither implying the
/// other.
#[test]
fn gc096_listener_woken_is_bounded_even_with_an_unfiltered_source() {
    let (f, pg) = seeded("ordo_trig_listener_bound");
    ingest_task_6(&f, &pg);
    let bound = woken_on_task_6(&f, &pg);

    // Sanity: unbounded by the source, six tasks exist and a clock run sees
    // every one of them. So the listener's answer below is the bound doing
    // work, not an empty table.
    let everything = ids(&run_routine(
        &f,
        &pg,
        "tasks[id, title]",
        Provenance::Clock,
        unbounded(),
    ));
    assert_eq!(everything.len(), 6);

    let seen = ids(&run_routine(
        &f,
        &pg,
        "tasks[id, title]",
        bound.clone(),
        unbounded(),
    ));
    assert_eq!(
        seen,
        vec!["task_6"],
        "unfiltered source, `unbounded` header — and still only the ingested row"
    );

    // `each ->` walks the rows the pipeline produced, so the emission set IS
    // this set: one note, on task_6. There is no second place for a tier-1
    // routine's writes to come from.
    assert_eq!(seen.len(), 1);

    // A source that filters on something else cannot escape either: the
    // bound is conjunctive with whatever the body already said.
    let seen = ids(&run_routine(
        &f,
        &pg,
        "tasks(status == \"open\")[id]",
        bound,
        unbounded(),
    ));
    assert_eq!(seen, vec!["task_6"]);
}

/// **A listener that must range wider says so** — GC098.
///
/// `unguarded` on the `run` header lifts the bound, "because one greppable
/// word says a person decided to lift it". Nothing else does: the two runs
/// below differ in exactly one flag.
#[test]
fn gc098_unguarded_lifts_the_bound_and_unbounded_does_not() {
    let (f, pg) = seeded("ordo_trig_unguarded");
    ingest_task_6(&f, &pg);
    let bound = woken_on_task_6(&f, &pg);
    let src = "tasks(status == \"open\")[id]";

    let bounded = ids(&run_routine(&f, &pg, src, bound.clone(), unbounded()));
    let lifted = ids(&run_routine(&f, &pg, src, bound, unbounded_unguarded()));

    assert_eq!(bounded, vec!["task_6"]);
    assert_eq!(
        lifted,
        vec!["task_1", "task_3", "task_4", "task_5", "task_6"],
        "notes on every open task, not only task_6"
    );
}

/// What "reachable from `$trigger.ids`" resolves to, in rows.
///
/// One declared ref hop in either direction, and **`false` for everything
/// else**. The last assertion is the one that matters: `boards` has no ref
/// path to `tasks`, so a listener-woken body naming it sees nothing — "every
/// other row is invisible exactly as a guard would make it". A bound that is
/// tighter than the specification can never be a confused deputy; one that
/// is looser is the escalation R-G20 exists to close.
#[test]
fn reachability_is_one_declared_hop_and_closed_by_default() {
    let (f, pg) = seeded("ordo_trig_reach");
    ingest_task_6(&f, &pg);
    pg.exec(&format!(
        "INSERT INTO \"taskNotes\" (\"$space\", \"id\", \"task\", \"body\") \
         VALUES ('{}', 'note_6', 'task_6', 'from the feed')",
        f.space().0
    ))
    .expect("note on the ingested row");
    let bound = woken_on_task_6(&f, &pg);

    // The trigger entity itself.
    assert_eq!(
        ids(&run_routine(
            &f,
            &pg,
            "tasks[id]",
            bound.clone(),
            unbounded()
        )),
        vec!["task_6"]
    );
    // Child side: `taskNotes.task -> tasks`.
    assert_eq!(
        ids(&run_routine(
            &f,
            &pg,
            "taskNotes[id]",
            bound.clone(),
            unbounded()
        )),
        vec!["note_6"],
        "the notes on the trigger row, and no others"
    );
    // Parent side: `tasks.project -> projects`.
    assert_eq!(
        ids(&run_routine(
            &f,
            &pg,
            "projects[id]",
            bound.clone(),
            unbounded()
        )),
        vec!["proj_1"],
        "the project the trigger row hangs off, and no others"
    );
    // Unreachable: no declared ref between `boards` and `tasks`, in either
    // direction. Invisible, not unbounded.
    assert!(
        ids(&run_routine(&f, &pg, "boards[id]", bound, unbounded())).is_empty(),
        "an entity the walk cannot reach yields nothing"
    );
}

/// A listener whose batch reached no rows reaches no rows.
///
/// Empty is the same answer as unreachable, deliberately: "no trigger rows"
/// and "never resolved" must not be two visibility outcomes, or the
/// difference becomes a bypass the first time somebody forgets to populate
/// the map.
#[test]
fn an_empty_trigger_id_set_reaches_nothing() {
    let (f, pg) = seeded("ordo_trig_empty_ids");
    let ctx = TriggerContext::clock(100); // no ids, by construction
    let bound = triggers::bound_for(
        &pg,
        &f.schema,
        &Woken::Listener {
            entity: "tasks".into(),
        },
        &ctx,
        &["tasks".into()],
    )
    .expect("bound");
    assert!(ids(&run_routine(&f, &pg, "tasks[id]", bound, unbounded())).is_empty());
}

// ── the ordinary path is untouched ───────────────────────────────────────

/// `Provenance::Caller` must be the ordinary path exactly, or every guard
/// case in the corpus is now answering a different question.
///
/// The regression that matters: the provenance seam replaced the `if
/// !self.unguarded` that guards were applied behind, so the arm an ordinary
/// caller takes has to plan **byte-identical SQL** to the entry point that
/// existed before it. Rows would be a weaker assertion — two different
/// filters can agree on one seed.
#[test]
fn an_ordinary_caller_plans_byte_identically_to_before_the_seam() {
    let f = Fixture::load().expect("fixture");
    for src in [
        "tasks(status == \"open\")[id]",
        "taskNotes[id, body]",
        "projects[id, name]",
        "taskBoard",
        "orders[id]",
    ] {
        let pipeline = ordo_syntax::parse_pipeline(src).expect("parse");
        let before =
            ordo_sema::bind(&f.schema, &f.registry, &pipeline, Clock::CORPUS).expect("bind");
        let before = plan(&before, &f.plan_space()).expect("plan").sql;
        let after = plan_sql(&f, src, Provenance::Caller, OpFlags::default());
        assert_eq!(before, after, "{src}");
    }
}

/// The clock arm is not `unguarded` wearing a different name.
///
/// They produce the same rows here and mean different things, and the
/// difference is what the artifact reports: `unguarded` is an author's
/// acknowledgment that gets published as `bypassed_guards`, while
/// guards-passing for a clock run is the platform's own wake-up. Conflating
/// them would make the artifact report a decision nobody made.
#[test]
fn the_clock_arm_is_not_the_unguarded_flag() {
    let (f, pg) = seeded("ordo_trig_clock_vs_flag");
    let src = "tasks(status == \"open\")[id]";
    let by_provenance = ids(&run_routine(
        &f,
        &pg,
        src,
        Provenance::Clock,
        OpFlags::default(),
    ));
    let by_flag = ids(&run_routine(
        &f,
        &pg,
        src,
        Provenance::Caller,
        OpFlags {
            unguarded: true,
            ..OpFlags::default()
        },
    ));
    assert_eq!(by_provenance, by_flag, "same rows");
    assert_eq!(by_provenance.len(), 4);
    // …and the clock arm needs no flag to get them, which is the whole of
    // §8's "a clock-woken routine's direct sources need nothing".
    assert_eq!(
        by_provenance,
        ids(&run_routine(
            &f,
            &pg,
            src,
            Provenance::Clock,
            OpFlags::default()
        ))
    );
}

/// The bound does not leak into a composed callee — R-G13 for provenance.
///
/// §8: "Composing a callee whose bypass is live requires the marker in
/// either case, because that marker is about the callee's semantics, not the
/// caller's identity." A registered op's own entity sources are therefore
/// bound the way that op declares, never the way the body that composed it
/// was woken.
///
/// Asserted on the plan, because the property is one filter being present
/// and another being absent, and the callee's guard leaves `$caller` a
/// prepared slot by design.
#[test]
fn the_bound_does_not_leak_into_a_composed_callee() {
    let f = Fixture::load().expect("fixture");
    let bound = Provenance::Listener(
        ordo_sema::TriggerRows::default().with("tasks", vec!["task_6".into()]),
    );

    // Directly: the listener bound is right there in the SQL.
    let direct = plan_sql(&f, "tasks[id]", bound.clone(), unbounded());
    assert!(
        direct.contains("task_6"),
        "the bound applies to a direct source"
    );
    assert!(
        !direct.contains("projectMembers"),
        "and it REPLACES the guard rather than joining it — one bound, not two"
    );

    // Through a registered op: the callee's own guard, and no trigger bound.
    let composed = plan_sql(&f, "taskBoard", bound, unbounded());
    assert!(
        composed.contains("projectMembers"),
        "the callee keeps its own guards: {composed}"
    );
    assert!(
        !composed.contains("task_6"),
        "the caller's trigger bound must not reach inside it: {composed}"
    );
}
