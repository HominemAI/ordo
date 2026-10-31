//! **The guards corpus, executed** — `docs/guards-conformance.md` §5.
//!
//! Every case below is keyed by the id the document gives it, spelled
//! literally, because the audit that produced checklist 9o was `rg GC0`
//! and a themed test survives no such audit. The expected rows and error
//! classes are the document's; nothing here asserts what the compiler
//! happens to produce.
//!
//! Callers are real (`mem_1`..`mem_4`, `mem_g`, `mem_o`, and anonymous),
//! Postgres is real, and this suite owns its databases.
//!
//! # Reading the coverage
//!
//! `gc_coverage_is_reported_not_implied` at the bottom holds three lists
//! and prints their sizes:
//!
//! - **EXECUTED** — cases with a real assertion in this file. The test
//!   proves each id actually appears here, so the list cannot outrun the
//!   suite.
//! - **NOT_RUN** — cases that cannot run yet, each naming the unbuilt
//!   construct. Not a to-do list: a claim a reader can check.
//! - **DIVERGENT** — cases where the document and the implementation
//!   disagree. These are findings, not tests to bend, so the divergence is
//!   asserted as it stands *and* recorded, and the recording fails the
//!   moment the implementation changes. An entry retires when the
//!   disagreement is ruled — sometimes by fixing the implementation (9r/10f)
//!   and sometimes by fixing the document (10g), which is the same kind of
//!   fix and settles the case just as completely.

mod gc_support;

use gc_support::{
    class_of, db, fixture, schema_with, seeded_reads, ANA, ANON, BRAM, CLEO, DOV, GIL,
};
use ordo_conformance::write::WriteDb;
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_schema::{GuardBody, GuardSource};
use serde_json::{json, Value};

/// Run a read case as a principal, panicking with the case id.
fn exec(
    f: &Fixture,
    pg: &Pg,
    case: &str,
    src: &str,
    params: &[(&str, &str)],
    as_: Option<&str>,
) -> Value {
    f.run_case_as(pg, src, params, as_)
        .unwrap_or_else(|e| panic!("{case}: `{src}` as {as_:?} did not run: {e}"))
}

/// A fresh seed for one **write** invocation (§3.5): each write case's
/// readbacks are computed from the pristine seed plus that invocation.
fn fresh(f: &Fixture, pg: &Pg) -> WriteLog {
    f.reset(pg).expect("fresh seed");
    WriteLog::new()
}

/// A [`WriteDb`] that keeps every statement the protocol issues.
///
/// R-G10 asks for more than one class: "the guard semi-join runs whatever
/// the outcome, so absent and forbidden are naturally close in cost. That is
/// a conformance assertion, not a hope." A wall-clock band on a shared
/// cluster measures the cluster; the *statements* measure the code path, and
/// they are deterministic. Two refusals that issue the same statements in
/// the same order, differing only in the id the caller supplied, cost the
/// same for the same reason — and no rearrangement of the checks can pass
/// this while reintroducing a short-circuit.
struct Recording<'a> {
    inner: &'a Pg,
    seen: std::cell::RefCell<Vec<String>>,
}

impl WriteDb for Recording<'_> {
    fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        self.seen.borrow_mut().push(sql.to_string());
        self.inner.query(sql)
    }
}

/// Run a write against a recorder, returning the outcome and the statements.
fn record_write(
    f: &Fixture,
    pg: &Pg,
    log: &mut WriteLog,
    call: &str,
    as_: Option<&str>,
) -> (Result<Value, String>, Vec<String>) {
    let rec = Recording {
        inner: pg,
        seen: std::cell::RefCell::new(Vec::new()),
    };
    let out = f.run_write_on(&rec, log, call, as_);
    let seen = rec.seen.into_inner();
    (out, seen)
}

/// The statements with the caller's own ids blanked, so two runs over two
/// ids compare as the *shapes* they are.
fn shapes(seen: &[String], ids: &[&str]) -> Vec<String> {
    seen.iter()
        .map(|s| {
            let mut s = s.clone();
            for id in ids {
                s = s.replace(id, "«id»");
            }
            s
        })
        .collect()
}

// ═════════════════════════════════════════════════════════════════════════
// A. Read-guard filtering (GC001–GC010)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc001_to_gc004_one_op_four_principals_four_answers() {
    let f = fixture();
    let pg = seeded_reads(&f);

    // GC001 — the flagship.
    let gc001 = |as_| exec(&f, &pg, "GC001", "taskBoard", &[], as_);
    assert_eq!(
        gc001(ANA),
        json!([
            {"title": "audit books", "status": "open", "project": "proj_3"},
            {"title": "draft spec",  "status": "open", "project": "proj_1"},
            {"title": "review spec", "status": "done", "project": "proj_1"},
        ])
    );
    assert_eq!(
        gc001(BRAM),
        json!([
            {"title": "build site",  "status": "open", "project": "proj_2"},
            {"title": "draft spec",  "status": "open", "project": "proj_1"},
            {"title": "review spec", "status": "done", "project": "proj_1"},
            {"title": "ship site",   "status": "open", "project": "proj_2"},
        ])
    );
    assert_eq!(
        gc001(CLEO),
        json!([
            {"title": "build site", "status": "open", "project": "proj_2"},
            {"title": "ship site",  "status": "open", "project": "proj_2"},
        ])
    );
    // "empty result, not an error" — the document says so twice.
    assert_eq!(gc001(DOV), json!([]), "GC001: Dov holds no membership");

    // GC002 — the guard conjoins with the op's own condition.
    let gc002 = |as_| exec(&f, &pg, "GC002", "openTasks", &[], as_);
    assert_eq!(
        gc002(ANA),
        json!([
            {"title": "audit books", "project": "proj_3"},
            {"title": "draft spec",  "project": "proj_1"},
        ])
    );
    assert_eq!(
        gc002(BRAM),
        json!([
            {"title": "build site", "project": "proj_2"},
            {"title": "draft spec", "project": "proj_1"},
            {"title": "ship site",  "project": "proj_2"},
        ])
    );
    assert_eq!(gc002(DOV), json!([]));

    // GC003 — `or`-disjuncts: Cleo enters proj_3 by ownership with no
    // membership row, and Ana qualifies for proj_1 through BOTH disjuncts
    // and appears once (semi-join, never a join fan-out).
    let gc003 = |as_| exec(&f, &pg, "GC003", "myProjects", &[], as_);
    assert_eq!(
        gc003(ANA),
        json!([{"name": "Alpha", "owner": "mem_1"}, {"name": "Gamma", "owner": "mem_3"}])
    );
    assert_eq!(
        gc003(BRAM),
        json!([{"name": "Alpha", "owner": "mem_1"}, {"name": "Beta", "owner": "mem_2"}])
    );
    assert_eq!(
        gc003(CLEO),
        json!([{"name": "Beta", "owner": "mem_2"}, {"name": "Gamma", "owner": "mem_3"}])
    );
    assert_eq!(gc003(DOV), json!([]));

    // GC004 — the link entity's own guard. No sort → id order. Cleo owns
    // proj_3 and cannot see its roster: `projectMembers`' read guard has
    // no owner disjunct, and guards mean exactly what they say.
    let gc004 = |as_| exec(&f, &pg, "GC004", "teamRoster", &[], as_);
    assert_eq!(
        gc004(ANA),
        json!([
            {"project": "proj_1", "member": "mem_1", "canWrite": true},
            {"project": "proj_1", "member": "mem_2", "canWrite": false},
            {"project": "proj_3", "member": "mem_1", "canWrite": false},
        ])
    );
    assert_eq!(
        gc004(CLEO),
        json!([
            {"project": "proj_2", "member": "mem_2", "canWrite": true},
            {"project": "proj_2", "member": "mem_3", "canWrite": true},
        ])
    );
    assert_eq!(gc004(DOV), json!([]));
}

#[test]
fn gc005_to_gc010_conditions_stacking_grouping_and_take() {
    let f = fixture();
    let pg = seeded_reads(&f);

    // GC005 — caller conditions narrow the guarded set; they never widen it.
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC005",
            "openIn($p: $p)",
            &[("p", "\"proj_1\"")],
            ANA
        ),
        json!([{"title": "draft spec"}])
    );
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC005",
            "openIn($p: $p)",
            &[("p", "\"proj_2\"")],
            ANA
        ),
        json!([]),
        "GC005: the guard hides proj_2 from Ana, and a condition cannot widen it"
    );
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC005",
            "openIn($p: $p)",
            &[("p", "\"proj_2\"")],
            BRAM
        ),
        json!([{"title": "build site"}, {"title": "ship site"}])
    );

    // GC006 — conditions cannot launder the guard: Bram is ASSIGNED task_5
    // and is not a member of proj_3, so the row stays hidden.
    let gc006 = |as_| exec(&f, &pg, "GC006", "myAssignedTasks", &[], as_);
    assert_eq!(gc006(ANA), json!([{"title": "draft spec"}]));
    assert_eq!(gc006(CLEO), json!([{"title": "build site"}]));
    assert_eq!(
        gc006(BRAM),
        json!([]),
        "GC006: assignment does not launder access"
    );
    assert_eq!(gc006(DOV), json!([]));

    // GC007 (exec half) — dot-path guard stacking: the plan reads
    // `projects` too, so both read guards inline. The compile half is
    // `gc007_gc027_gc048_gc052_guard_provenance_is_disclosed`.
    assert_eq!(
        exec(&f, &pg, "GC007", "taskSheet", &[], ANA),
        json!([
            {"title": "audit books", "name": "Gamma"},
            {"title": "draft spec",  "name": "Alpha"},
            {"title": "review spec", "name": "Alpha"},
        ])
    );

    // GC008 — group over the guarded view: buckets are per-caller, and an
    // empty view yields no buckets at all.
    let gc008 = |as_| {
        exec(
            &f,
            &pg,
            "GC008",
            "tasks[status]\ngroup status { n: count }",
            &[],
            as_,
        )
    };
    assert_eq!(
        gc008(ANA),
        json!([{"status": "done", "n": 1}, {"status": "open", "n": 2}])
    );
    assert_eq!(gc008(CLEO), json!([{"status": "open", "n": 2}]));
    assert_eq!(gc008(DOV), json!([]), "GC008: no visible rows, no buckets");

    // GC009 — reduce over an empty guarded view: `count` of nothing is 0.
    // One row, not zero rows, and still not an error.
    let gc009 = |as_| {
        exec(
            &f,
            &pg,
            "GC009",
            "tasks[title]\nreduce { n: count }",
            &[],
            as_,
        )
    };
    assert_eq!(gc009(BRAM), json!([{"n": 4}]));
    assert_eq!(gc009(DOV), json!([{"n": 0}]));

    // GC010 — the guard precedes `sort`/`take` in the same body (R-G7):
    // take-1 ranks the CALLER-VISIBLE tasks.
    let gc010 = |as_| exec(&f, &pg, "GC010", "firstTask", &[], as_);
    assert_eq!(gc010(ANA), json!([{"title": "audit books"}]));
    assert_eq!(gc010(CLEO), json!([{"title": "build site"}]));
    assert_eq!(gc010(DOV), json!([]));
}

// ═════════════════════════════════════════════════════════════════════════
// B. Write guards and atomicity (GC011–GC022)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc011_to_gc022_the_write_matrix() {
    let f = fixture();
    let pg = db("ordo_gc_write_b");

    // ── GC011 — allowed addressed converge ──────────────────────────────
    let mut log = fresh(&f, &pg);
    let r = f
        .run_write_as(
            &pg,
            &mut log,
            r#"renameTask($task: "task_1", $title: "draft spec v2")"#,
            ANA,
        )
        .expect("GC011: Ana holds canWrite in proj_1");
    assert_eq!(r, json!([{ "entity": "tasks", "updated": 1 }]));
    assert!(
        exec(&f, &pg, "GC011", "taskBoard", &[], ANA)
            .as_array()
            .unwrap()
            .contains(&json!({"title": "draft spec v2", "status": "open", "project": "proj_1"})),
        "GC011: the readback shows the new title"
    );

    // ── GC012 — disallowed addressed converge, on a row the caller MAY
    // read: `access_guard`, the informative case (R-G10). ──────────────
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"renameTask($task: "task_1", $title: "hijack")"#,
            BRAM,
        )
        .expect_err("GC012: Bram reads task_1 but canWrite is false");
    assert_eq!(class_of(&e), "access_guard", "GC012: {e}");
    assert_eq!(
        exec(&f, &pg, "GC012", "taskBoard", &[], ANA)
            .as_array()
            .unwrap()[1]["title"],
        json!("draft spec"),
        "GC012: nothing applied"
    );

    // ── GC013 — addressed write outside the read view is `not_found`,
    // not `access_guard` (R-G10, re-ruled). Contrast GC012 directly
    // above: same op, same row, and Bram gets `access_guard` BECAUSE he
    // can read it. ──────────────────────────────────────────────────────
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"renameTask($task: "task_1", $title: "x")"#,
            DOV,
        )
        .expect_err("GC013: Dov reads no task in proj_1");
    assert_eq!(class_of(&e), "not_found", "GC013: {e}");

    // ── GC014 — create allowed: the guard is evaluated against the
    // would-be row (R-G11). Bram holds canWrite in proj_2. ─────────────
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"addTask($project: "proj_2", $title: "deploy")"#,
        BRAM,
    )
    .expect("GC014: Bram has canWrite in proj_2");
    assert_eq!(
        exec(&f, &pg, "GC014", "openTasks", &[], BRAM),
        json!([
            {"title": "build site", "project": "proj_2"},
            {"title": "deploy",     "project": "proj_2"},
            {"title": "draft spec", "project": "proj_1"},
            {"title": "ship site",  "project": "proj_2"},
        ])
    );

    // ── GC015 — create denied: member without canWrite. The parent
    // resolves (Bram reads proj_1), so the would-be row's guard is what
    // fails, and the answer is informative. ────────────────────────────
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addTask($project: "proj_1", $title: "sneak")"#,
            BRAM,
        )
        .expect_err("GC015: canWrite is false in proj_1");
    assert_eq!(class_of(&e), "access_guard", "GC015: {e}");
    assert_eq!(
        exec(&f, &pg, "GC015", "openTasks", &[], BRAM)
            .as_array()
            .unwrap()
            .len(),
        3,
        "GC015: nothing created"
    );

    // ── GC016 — create denied on the PARENT: `not_found`, byte-identical
    // to a parent that never existed (R-G10 / V-010). ──────────────────
    let mut log = fresh(&f, &pg);
    let outside = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addTask($project: "proj_1", $title: "sneak")"#,
            DOV,
        )
        .expect_err("GC016: proj_1 is outside Dov's read view");
    let absent = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addTask($project: "proj_nonexistent", $title: "sneak")"#,
            DOV,
        )
        .expect_err("GC016: no such project");
    assert_eq!(class_of(&outside), "not_found", "GC016: {outside}");
    assert_eq!(
        outside, absent,
        "GC016: forbidden and absent must be indistinguishable in code AND message — \
         literally, not after masking the id"
    );

    // ── GC017 — the atomic group aborts. Ana may close task_1 but task_3
    // is in proj_2, outside her READ view, so the class is `not_found`;
    // the abort is what the case exists to pin. ────────────────────────
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"closePair($a: "task_1", $b: "task_3")"#,
            ANA,
        )
        .expect_err("GC017: task_3 is invisible to Ana");
    assert_eq!(class_of(&e), "not_found", "GC017: {e}");
    assert_eq!(
        pg.query(r#"SELECT "status" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("open"),
        "GC017: ZERO statements applied — the group aborts as a unit"
    );

    // ── GC018 — same op, qualified caller: both statements land. ───────
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"closePair($a: "task_3", $b: "task_4")"#,
        BRAM,
    )
    .expect("GC018: Bram holds canWrite in proj_2");
    assert_eq!(
        exec(&f, &pg, "GC018", "openTasks", &[], BRAM),
        json!([{"title": "draft spec", "project": "proj_1"}])
    );

    // ── GC019 — bulk delete ranges over the caller's WRITE view (R-G9).
    // Done tasks in the seed: task_2 (proj_1) only. ────────────────────
    let mut log = fresh(&f, &pg);
    let r = f
        .run_write_as(&pg, &mut log, "purgeDoneAll", BRAM)
        .expect("GC019: an empty selection is a vacuous no-op, not an error");
    assert_eq!(r, json!([{ "entity": "tasks", "deleted": 0 }]), "GC019");
    assert_eq!(
        exec(&f, &pg, "GC019", "taskBoard", &[], BRAM)
            .as_array()
            .unwrap()
            .len(),
        4,
        "GC019: task_2 is outside Bram's write view and survives"
    );

    let mut log = fresh(&f, &pg);
    let r = f
        .run_write_as(&pg, &mut log, "purgeDoneAll", ANA)
        .expect("GC019: Ana holds canWrite in proj_1");
    assert_eq!(r, json!([{ "entity": "tasks", "deleted": 1 }]), "GC019");
    assert_eq!(
        exec(&f, &pg, "GC019", "taskBoard", &[], ANA),
        json!([
            {"title": "audit books", "status": "open", "project": "proj_3"},
            {"title": "draft spec",  "status": "open", "project": "proj_1"},
        ])
    );

    // ── GC020 — bulk converge, same scoping. Cleo's write view is
    // task_3/task_4; task_1 and task_5 are open and unwritable by her. ──
    let mut log = fresh(&f, &pg);
    let r = f
        .run_write_as(&pg, &mut log, "shipAll", CLEO)
        .expect("GC020: bulk writes range over the write view");
    assert_eq!(r, json!([{ "entity": "tasks", "updated": 2 }]), "GC020");
    assert_eq!(
        exec(&f, &pg, "GC020", "taskBoard", &[], CLEO),
        json!([
            {"title": "build site", "status": "done", "project": "proj_2"},
            {"title": "ship site",  "status": "done", "project": "proj_2"},
        ])
    );
    assert_eq!(
        exec(&f, &pg, "GC020", "taskBoard", &[], ANA),
        json!([
            {"title": "audit books", "status": "open", "project": "proj_3"},
            {"title": "draft spec",  "status": "open", "project": "proj_1"},
            {"title": "review spec", "status": "done", "project": "proj_1"},
        ]),
        "GC020: rows outside Cleo's write view are untouched"
    );

    // ── GC021 — read guard ≠ write guard: Ana READS proj_3 (pm_5) but
    // only the owner writes projects. ──────────────────────────────────
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"renameProject($project: "proj_3", $name: "Gamma2")"#,
        CLEO,
    )
    .expect("GC021: Cleo owns proj_3");
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"renameProject($project: "proj_3", $name: "Mine")"#,
            ANA,
        )
        .expect_err("GC021: Ana reads proj_3 but does not own it");
    assert_eq!(class_of(&e), "access_guard", "GC021: {e}");

    // ── GC022 — hand-rolled meta-permission: membership rows are
    // owner-writable. Cleo holds canWrite in proj_2 — for TASKS, not for
    // the roster. ──────────────────────────────────────────────────────
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"addMember($project: "proj_2", $member: "mem_4")"#,
        BRAM,
    )
    .expect("GC022: Bram owns proj_2");
    assert!(
        exec(&f, &pg, "GC022", "teamRoster", &[], BRAM)
            .as_array()
            .unwrap()
            .contains(&json!({"project": "proj_2", "member": "mem_4", "canWrite": false})),
        "GC022: the roster gains the row"
    );
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addMember($project: "proj_2", $member: "mem_4")"#,
            CLEO,
        )
        .expect_err("GC022: canWrite on tasks is not ownership of the project");
    assert_eq!(class_of(&e), "access_guard", "GC022: {e}");
}

// ═════════════════════════════════════════════════════════════════════════
// C. Inheritance and override (GC023–GC029)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc023_and_gc028_inheritance_and_override_read_side() {
    let f = fixture();
    let pg = seeded_reads(&f);

    // GC023 — depth-2 read inheritance: `taskNotes` carries no `access`
    // clause, so tasks' clause applies rebased through the ownership path.
    let gc023 = |as_| exec(&f, &pg, "GC023", "noteFeed", &[], as_);
    assert_eq!(
        gc023(ANA),
        json!([
            {"body": "numbers pending", "task": "task_5"},
            {"body": "outline ready",   "task": "task_1"},
        ])
    );
    assert_eq!(
        gc023(BRAM),
        json!([
            {"body": "outline ready", "task": "task_1"},
            {"body": "site copy wip", "task": "task_3"},
        ])
    );
    assert_eq!(
        gc023(CLEO),
        json!([{"body": "site copy wip", "task": "task_3"}])
    );
    assert_eq!(gc023(DOV), json!([]));

    // GC028 — the override REPLACES the inherited guard, read side. Cleo
    // owns proj_3: the projects guard admits her by its owner disjunct,
    // and tasks' explicit clause (membership only) excludes her. Same
    // caller, adjacent entities, opposite answers.
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC028",
            "gammaProject($p: $p)",
            &[("p", "\"proj_3\"")],
            CLEO
        ),
        json!([{"name": "Gamma"}])
    );
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC028",
            "tasksIn($p: $p)",
            &[("p", "\"proj_3\"")],
            CLEO
        ),
        json!([]),
        "GC028: parent privilege does not leak down through a replaced guard"
    );
}

#[test]
fn gc024_to_gc029_inherited_write_guards() {
    let f = fixture();
    let pg = db("ordo_gc_write_c");

    // GC024 — inherited write guard, create at depth 2.
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"addNote($task: "task_3", $body: "final copy in")"#,
        CLEO,
    )
    .expect("GC024: Cleo holds canWrite in proj_2");
    assert_eq!(
        exec(&f, &pg, "GC024", "noteFeed", &[], CLEO),
        json!([
            {"body": "final copy in", "task": "task_3"},
            {"body": "site copy wip", "task": "task_3"},
        ])
    );

    // GC025 — inherited write guard denies, both ways.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addNote($task: "task_1", $body: "drive-by")"#,
            BRAM,
        )
        .expect_err("GC025: Bram reads note_1's task, lacks canWrite in proj_1");
    assert_eq!(class_of(&e), "access_guard", "GC025: {e}");
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addNote($task: "task_5", $body: "drive-by")"#,
            ANA,
        )
        .expect_err("GC025: nobody holds canWrite in proj_3");
    assert_eq!(class_of(&e), "access_guard", "GC025: {e}");

    // GC026 — the whole R-G10 ruling in one case: same op, same row, two
    // callers, and the difference is exactly whether the caller could
    // already see it.
    let mut log = fresh(&f, &pg);
    let visible = f
        .run_write_as(
            &pg,
            &mut log,
            r#"editNote($note: "note_3", $body: "x")"#,
            ANA,
        )
        .expect_err("GC026: Ana reads note_3, no proj_3 canWrite");
    assert_eq!(class_of(&visible), "access_guard", "GC026: {visible}");
    let hidden = f
        .run_write_as(
            &pg,
            &mut log,
            r#"editNote($note: "note_3", $body: "x")"#,
            CLEO,
        )
        .expect_err("GC026: Cleo owns proj_3, the inherited guard hides its notes");
    assert_eq!(class_of(&hidden), "not_found", "GC026: {hidden}");

    // GC029 — the override replaces the inherited guard, write side. If
    // tasks inherited projects' `owner == $caller` write guard, Cleo could
    // close task_5. She cannot, and neither can Ana: task_5 is writable by
    // nobody, exactly as the guards read.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(&pg, &mut log, r#"closeTask($task: "task_5")"#, CLEO)
        .expect_err("GC029: tasks' own guard hides task_5 from its project's owner");
    assert_eq!(class_of(&e), "not_found", "GC029: {e}");
    let e = f
        .run_write_as(&pg, &mut log, r#"closeTask($task: "task_5")"#, ANA)
        .expect_err("GC029: pm_5 makes task_5 readable to Ana; canWrite is false");
    assert_eq!(class_of(&e), "access_guard", "GC029: {e}");
}

// ═════════════════════════════════════════════════════════════════════════
// D. `unguarded` (GC030–GC035)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc030_to_gc035_the_bypass_family() {
    let f = fixture();
    let pg = seeded_reads(&f);

    // GC030 — the bypass works for ANY principal, anonymous included
    // (R-G12). This is deliberate, and it is why the flag is loud.
    let all = json!([
        {"title": "audit books", "status": "open"},
        {"title": "build site",  "status": "open"},
        {"title": "draft spec",  "status": "open"},
        {"title": "review spec", "status": "done"},
        {"title": "ship site",   "status": "open"},
    ]);
    assert_eq!(exec(&f, &pg, "GC030", "allTasks", &[], DOV), all);
    assert_eq!(exec(&f, &pg, "GC030", "allTasks", &[], ANON), all);

    // GC031 — the manifest carries the flag AND what it bypasses.
    let op = f.registry.get("allTasks").expect("GC031: registered");
    assert!(op.flags.unguarded, "GC031: `unguarded: true`");
    let bypassed: Vec<String> = ["tasks"]
        .iter()
        .filter(|e| {
            f.schema
                .entity(e)
                .and_then(|x| x.guards.as_ref())
                .is_some_and(|g| matches!(g.read, GuardBody::Walk(_)))
        })
        .map(|e| format!("{e}.read"))
        .collect();
    assert_eq!(bypassed, ["tasks.read"], "GC031: the bypassed-guard list");
    let q =
        ordo_sema::bind_op_generic(&f.schema, &f.registry, "allTasks", ordo_sema::Clock::CORPUS)
            .expect("GC031: binds");
    assert_eq!(
        ordo_plan::cost::classify(&q).as_str(),
        "sql",
        "GC031: cost class"
    );

    // GC032 — a superfluous marker: `vendors` is unguarded, so the flag
    // would be decoration (R-G8).
    assert_eq!(
        reject_op(&f, "op v unguarded = vendors[name]"),
        "unguarded_superfluous"
    );

    // GC033 — the marker cannot strip a composed callee's guard:
    // `openTasks` is guarded, `leak`'s own body writes no guarded entity
    // source directly, so the marker is superfluous and the guard
    // survives regardless (R-G7/R-G8).
    //
    // `openTasks` is row-unbounded (§3.4) and composition-R9 puts the
    // acknowledgment on the CALLER, so the `unbounded` marker is part of the
    // case's text. The document used to omit it and did not compile; 10g
    // fixed the DOCUMENT, and this is now its text verbatim.
    assert_eq!(
        reject_op(&f, "op leak unbounded unguarded = openTasks[title]"),
        "unguarded_superfluous"
    );
    // The gate ordering the repair steps over, pinned so the corrected text
    // cannot quietly become unnecessary: without the acknowledgment the op
    // never reaches the bypass gate at all.
    assert_eq!(
        reject_op(&f, "op leak unguarded = openTasks[title]"),
        "unacknowledged_unbounded",
        "GC033: the row-bound gate answers first, which is why the case's text \
         carries `unbounded`"
    );

    // GC034 — composing a LIVE bypass requires the caller's own
    // acknowledgment (R-G8, mirroring composition-R9 for `unbounded`).
    assert_eq!(
        reject_op(
            &f,
            "op openAudit unbounded = allTasks(status == \"open\")[title]"
        ),
        "unacknowledged_unguarded"
    );

    // GC035 — same op, acknowledged: it compiles, the results are
    // caller-independent, and the manifest discloses the inherited bypass.
    let mut f2 = fixture();
    let prog = ordo_syntax::parse_program(
        "op openAudit unbounded unguarded = allTasks(status == \"open\")[title]",
    )
    .expect("GC035: parses");
    f2.registry.extend_from(&prog);
    ordo_plan::cost::check_registration(&f2.schema, &f2.registry).expect("GC035: registers");
    let four = json!([
        {"title": "audit books"}, {"title": "build site"},
        {"title": "draft spec"},  {"title": "ship site"},
    ]);
    assert_eq!(exec(&f2, &pg, "GC035", "openAudit", &[], ANA), four);
    assert_eq!(exec(&f2, &pg, "GC035", "openAudit", &[], DOV), four);
    assert!(
        f2.registry.get("openAudit").unwrap().flags.unguarded,
        "GC035: the flag is on the caller, greppable at the point of bypass"
    );
}

/// Register one op against the fixture and return the class it is refused
/// with. The registration gate is where R-G8's compile pair lands.
fn reject_op(f: &Fixture, src: &str) -> String {
    let prog = ordo_syntax::parse_program(src).expect("the case's op parses");
    let mut registry = f.registry.clone();
    registry.extend_from(&prog);
    let e = ordo_plan::cost::check_registration(&f.schema, &registry)
        .expect_err(&format!("`{src}` must be refused"));
    e.code.to_string()
}

// ═════════════════════════════════════════════════════════════════════════
// E. `acl` derivation (GC036–GC048)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc036_the_acl_derivation_has_the_pinned_shape() {
    let f = fixture();
    let link = f
        .schema
        .entity("boardsAccess")
        .expect("GC036: `acl (read, write, admin)` derives a real link entity");

    // `board -> boards owned`, `member -> members owned`.
    for (field, target) in [("board", "boards"), ("member", "members")] {
        match &link
            .field(field)
            .unwrap_or_else(|| panic!("GC036: {field}"))
            .ty
        {
            ordo_schema::FieldTy::Ref {
                target: t, owned, ..
            } => {
                assert_eq!(t, target, "GC036: `{field} -> {target}`");
                assert!(owned, "GC036: `{field}` is `owned`");
            }
            other => panic!("GC036: `{field}` is {other:?}, not a ref"),
        }
    }
    // `level: enum("read", "write", "admin")` — the pinned ladder, not
    // plain text. It is what makes the ordering below total: a value
    // outside it has no position in the order.
    assert_eq!(
        link.field("level")
            .expect("GC036: `level`")
            .enum_values
            .as_deref(),
        Some(["read".to_string(), "write".to_string(), "admin".to_string()].as_slice()),
        "GC036: `level: enum(\"read\", \"write\", \"admin\")`"
    );
    assert!(
        f.ddl
            .contains(r#"CHECK ("level" IN ('read', 'write', 'admin'))"#),
        "GC036: the ladder reaches the database, so a restore cannot leave it:\n{}",
        f.ddl
    );

    // `unique (board, member)` — R-G17's converge-or-create key.
    assert_eq!(
        link.unique_keys,
        vec![vec!["board".to_string(), "member".to_string()]],
        "GC036: `unique (board, member)`"
    );
    assert!(
        f.ddl.contains(
            r#"ALTER TABLE "boardsAccess" ADD CONSTRAINT "boardsAccess$board$member$uq" UNIQUE NULLS NOT DISTINCT ("$space", "board", "member");"#
        ),
        "GC036: the index leads with the space, like every other one, and carries \
         `NULLS NOT DISTINCT` — DL's one equality, in which `null == null` (9e):\n{}",
        f.ddl
    );

    // The derived admin ops exist, are registered, and are ops nobody
    // wrote: `market.ordo` declares none of these three.
    for name in [
        "grantBoardsAccess",
        "revokeBoardsAccess",
        "setBoardsAccessLevel",
    ] {
        assert!(
            f.registry.get(name).is_some(),
            "GC036: `acl` derives `{name}`"
        );
        assert!(
            !ordo_conformance::FIXTURE.contains(name),
            "GC036: `{name}` is DERIVED — the fixture must not declare it"
        );
    }

    // The levels are ordered left to right, and the order IS the ordering.
    assert_eq!(
        f.schema.entity("boards").unwrap().acl_levels.as_deref(),
        Some(["read".to_string(), "write".to_string(), "admin".to_string()].as_slice()),
        "GC036: levels in declaration order"
    );

    // Derived guards on the link, per R-G14: read at any level on the
    // parent row, write at the top level.
    let g = link
        .guards
        .as_ref()
        .expect("GC036: the link carries guards");
    assert_eq!(g.source, GuardSource::AclDerived);
    assert_eq!(
        g.render(true),
        "board.boardsAccess(member == $caller) or board.boardsAccess(member.groupMembers.group(member == $caller))",
        "GC036"
    );
    assert_eq!(
        g.render(false),
        "board.boardsAccess(member == $caller and level == \"admin\") or board.boardsAccess(member.groupMembers.group(member == $caller) and level == \"admin\")",
        "GC036"
    );

    // The root and its owned subtree are guard-bearing, and `cards`
    // inherits rather than declaring.
    assert_eq!(
        f.schema
            .entity("boards")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .source,
        GuardSource::AclDerived
    );
    assert_eq!(
        f.schema
            .entity("cards")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .source,
        GuardSource::Inherited {
            from: "boards".into()
        },
        "GC036: `cards` inherits the boards ACL down the owned ref"
    );
}

#[test]
fn gc037_gc038_gc046_acl_reads_at_any_level() {
    let f = fixture();
    let pg = seeded_reads(&f);

    // GC037 — any level reads (read ⊂ write ⊂ admin).
    let gc037 = |as_| exec(&f, &pg, "GC037", "boardList", &[], as_);
    assert_eq!(gc037(ANA), json!([{"name": "roadmap"}]));
    assert_eq!(
        gc037(BRAM),
        json!([{"name": "icebox"}, {"name": "roadmap"}])
    );
    assert_eq!(gc037(CLEO), json!([{"name": "roadmap"}]));
    assert_eq!(gc037(DOV), json!([]));

    // GC038 — the ACL inherits to the owned child.
    let gc038 = |as_| exec(&f, &pg, "GC038", "cardList", &[], as_);
    let board1 = json!([
        {"title": "q3 goals", "done": false, "board": "board_1"},
        {"title": "q4 goals", "done": true,  "board": "board_1"},
    ]);
    assert_eq!(gc038(ANA), board1);
    assert_eq!(
        gc038(BRAM),
        json!([
            {"title": "q3 goals", "done": false, "board": "board_1"},
            {"title": "q4 goals", "done": true,  "board": "board_1"},
            {"title": "someday",  "done": false, "board": "board_2"},
        ])
    );
    assert_eq!(gc038(CLEO), board1);
    assert_eq!(gc038(DOV), json!([]));

    // GC046 — the ACL rows themselves read at any level (R-G14): the
    // roster is visible to everyone on the board, guests included. No
    // sort → id order.
    let gc046 = |as_| exec(&f, &pg, "GC046", "aclRoster", &[], as_);
    let board1_rows = json!([
        {"board": "board_1", "member": "mem_1", "level": "admin"},
        {"board": "board_1", "member": "mem_2", "level": "write"},
        {"board": "board_1", "member": "mem_3", "level": "read"},
        {"board": "board_1", "member": "mem_g", "level": "read"},
    ]);
    assert_eq!(gc046(ANA), board1_rows);
    assert_eq!(
        gc046(BRAM),
        json!([
            {"board": "board_1", "member": "mem_1", "level": "admin"},
            {"board": "board_1", "member": "mem_2", "level": "write"},
            {"board": "board_1", "member": "mem_3", "level": "read"},
            {"board": "board_2", "member": "mem_2", "level": "admin"},
            {"board": "board_1", "member": "mem_g", "level": "read"},
        ]),
        "GC046: in id order, ba_1 … ba_5"
    );
    assert_eq!(gc046(GIL), board1_rows);
    assert_eq!(gc046(DOV), json!([]));
}

#[test]
fn gc040_a_read_level_cannot_write() {
    let f = fixture();
    let pg = db("ordo_gc_write_e");

    // GC040 — a read level cannot write. Cleo holds `read` on board_1
    // (ba_3) and Gil holds `read` (ba_5); both are inside the read view,
    // so both get the informative refusal.
    for who in [CLEO, GIL] {
        let mut log = fresh(&f, &pg);
        let e = f
            .run_write_as(
                &pg,
                &mut log,
                r#"setCardDone($card: "card_1", $done: true)"#,
                who,
            )
            .expect_err("GC040: a read-level holder cannot write");
        assert_eq!(class_of(&e), "access_guard", "GC040 as {who:?}: {e}");
        assert_eq!(
            pg.query(r#"SELECT "done" FROM "cards" WHERE "id" = 'card_1'"#)
                .unwrap()[0][0]
                .as_deref(),
            Some("f"),
            "GC040: nothing applied"
        );
    }
}

#[test]
fn gc039_gc041_write_needs_a_level_at_or_above_write() {
    let f = fixture();
    let pg = db("ordo_gc_write_e_ord");

    // The guard itself first, so the ordering is legible without running
    // a write: a **closed disjunction of `==` over the qualifying
    // levels**, which is how "at or above `write`" stays inside G1's
    // grammar. `level >= "write"` is unspellable there, by design.
    assert_eq!(
        f.schema
            .entity("boards")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .render(false),
        "boardsAccess(member == $caller and level == \"write\" \
         or member == $caller and level == \"admin\") or boardsAccess(member.groupMembers.group(member == $caller) and level == \"write\" \
         or member.groupMembers.group(member == $caller) and level == \"admin\")",
        "GC039: the root's write guard names every level at or above `write`"
    );
    assert_eq!(
        f.schema
            .entity("cards")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .render(false),
        "board.boardsAccess(member == $caller and level == \"write\" \
         or member == $caller and level == \"admin\") or board.boardsAccess(member.groupMembers.group(member == $caller) and level == \"write\" \
         or member.groupMembers.group(member == $caller) and level == \"admin\")",
        "GC039: and the owned child inherits it, rebased through `board`"
    );

    // GC039 — write requires level >= write: the write-holder and the
    // admin-holder both apply. Each invocation starts from the pristine
    // seed (§3.5).
    for who in [BRAM, ANA] {
        let mut log = fresh(&f, &pg);
        f.run_write_as(
            &pg,
            &mut log,
            r#"setCardDone($card: "card_1", $done: true)"#,
            who,
        )
        .unwrap_or_else(|e| panic!("GC039 as {who:?}: {e}"));
        assert_eq!(
            pg.query(r#"SELECT "done" FROM "cards" WHERE "id" = 'card_1'"#)
                .unwrap()[0][0]
                .as_deref(),
            Some("t"),
            "GC039 as {who:?}: applied"
        );
    }

    // GC041 — create into the owned child under the inherited ACL. Bram
    // holds `write` on board_1; Cleo holds `read`.
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"addCard($board: "board_1", $title: "q5 goals")"#,
        BRAM,
    )
    .expect("GC041: `write` creates");
    assert!(
        exec(&f, &pg, "GC041", "cardList", &[], CLEO)
            .as_array()
            .unwrap()
            .contains(&json!({"title": "q5 goals", "done": false, "board": "board_1"})),
        "GC041: and a read-level holder on the same board sees it"
    );

    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addCard($board: "board_1", $title: "sneak")"#,
            CLEO,
        )
        .expect_err("GC041: `read` is below the write rung");
    assert_eq!(class_of(&e), "access_guard", "GC041: {e}");
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "cards" WHERE "title" = 'sneak'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("0"),
        "GC041: nothing applied"
    );
}

#[test]
fn gc042_to_gc045_and_gc062_the_derived_admin_ops() {
    let f = fixture();
    let pg = db("ordo_gc_admin_ops");

    // GC042 — grant by the admin: the granted member's view opens.
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
        ANA,
    )
    .expect("GC042: Ana is admin of board_1");
    assert_eq!(
        exec(&f, &pg, "GC042", "boardList", &[], DOV),
        json!([{"name": "roadmap"}])
    );
    assert_eq!(
        exec(&f, &pg, "GC042", "cardList", &[], DOV),
        json!([
            {"title": "q3 goals", "done": false, "board": "board_1"},
            {"title": "q4 goals", "done": true,  "board": "board_1"},
        ])
    );

    // GC043 — the admin ops are gated by the TOP level only: `write` is
    // not enough, and neither is `read`. This is the one place the level
    // ordering deliberately stops short of the top.
    for who in [BRAM, CLEO] {
        let mut log = fresh(&f, &pg);
        let e = f
            .run_write_as(
                &pg,
                &mut log,
                r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
                who,
            )
            .expect_err("GC043: below admin on board_1");
        assert_eq!(class_of(&e), "access_guard", "GC043 as {who:?}: {e}");
    }

    // GC062 — and a guest cannot reach them either.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
            GIL,
        )
        .expect_err("GC062: Gil holds `read` on board_1");
    assert_eq!(class_of(&e), "access_guard", "GC062: {e}");

    // GC045 — admin is per-ROW, not global. Bram is admin of board_2 and
    // only `write` on board_1.
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"grantBoardsAccess($board: "board_2", $member: "mem_1", $level: "write")"#,
        BRAM,
    )
    .expect("GC045: Bram administers board_2");
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "read")"#,
            BRAM,
        )
        .expect_err("GC045: and only board_2");
    assert_eq!(class_of(&e), "access_guard", "GC045: {e}");

    // GC044 — revoke: admin succeeds and the revoked member's view closes.
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"revokeBoardsAccess($board: "board_1", $member: "mem_3")"#,
        ANA,
    )
    .expect("GC044: Ana is admin of board_1");
    assert_eq!(exec(&f, &pg, "GC044", "boardList", &[], CLEO), json!([]));

    // GC044, second half — a non-admin's revoke is `access_guard`, not a
    // vacuous no-op. The derived op is an ADDRESSED delete by
    // `unique (board, member)` (R-G17), which is exactly why it can
    // answer this: a ranged delete over an empty write view deletes
    // nothing and reports success.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"revokeBoardsAccess($board: "board_1", $member: "mem_2")"#,
            CLEO,
        )
        .expect_err("GC044: Cleo holds `read`");
    assert_eq!(class_of(&e), "access_guard", "GC044: {e}");
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "boardsAccess" WHERE "id" = 'ba_2'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("1"),
        "GC044: nothing applied"
    );
}

#[test]
fn gc083_the_derived_admin_ops_converge_on_their_unique_key() {
    let f = fixture();
    let pg = db("ordo_gc_admin_meta");

    // GC083, the compile half — `grantBoardsAccess` is
    // **converge-or-create** on `unique (board, member)`, and the mode
    // label says so. The word comes from the write path's own classifier,
    // so label and behaviour cannot drift.
    let grant = f.registry.get("grantBoardsAccess").expect("GC083: derived");
    let ordo_syntax::decl::OpBody::Write(stmts) = &grant.body else {
        panic!("GC083: a write op");
    };
    let ordo_syntax::decl::WriteStmt::Set { entity, fields, .. } = &stmts[0] else {
        panic!("GC083: a `set`");
    };
    assert_eq!(
        ordo_conformance::write::set_mode(&f.schema, &entity.name, fields).label(),
        "creates-or-converges",
        "GC083: R-G17's mode label"
    );
    // `revokeBoardsAccess` is a delete.
    let revoke = f
        .registry
        .get("revokeBoardsAccess")
        .expect("GC083: derived");
    let ordo_syntax::decl::OpBody::Write(rstmts) = &revoke.body else {
        panic!("GC083: a write op");
    };
    assert!(
        matches!(&rstmts[0], ordo_syntax::decl::WriteStmt::Del { .. }),
        "GC083: `revokeBoardsAccess` is a delete"
    );
    // Both are gated by the link's own write guard — the top level, and
    // nothing else. This is the `gatedBy` the case asks for, spelled as
    // the walk the manifest actually carries.
    assert_eq!(
        f.schema
            .entity("boardsAccess")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .render(false),
        "board.boardsAccess(member == $caller and level == \"admin\") or board.boardsAccess(member.groupMembers.group(member == $caller) and level == \"admin\")",
        "GC083: gated by `boardsAccess.level == \"admin\"`"
    );

    // GC083, the behaviour the label claims — regranting an existing
    // holder CONVERGES their level rather than minting a second row.
    // Without `unique (board, member)` there is no key to converge on and
    // this is two rows, which is how a revoke half-works.
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"grantBoardsAccess($board: "board_1", $member: "mem_3", $level: "write")"#,
        ANA,
    )
    .expect("GC083: Ana administers board_1");
    let rows = pg
        .query(
            r#"SELECT "id", "level" FROM "boardsAccess"
               WHERE "board" = 'board_1' AND "member" = 'mem_3'"#,
        )
        .unwrap();
    assert_eq!(rows.len(), 1, "GC083: one row, converged — not two");
    assert_eq!(rows[0][0].as_deref(), Some("ba_3"), "GC083: the SAME row");
    assert_eq!(rows[0][1].as_deref(), Some("write"), "GC083: level changed");

    // And the level change op reaches the same row by the same key.
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"setBoardsAccessLevel($board: "board_1", $member: "mem_3", $level: "admin")"#,
        ANA,
    )
    .expect("GC083: the level change is admin-gated and Ana is admin");
    assert_eq!(
        pg.query(r#"SELECT "level" FROM "boardsAccess" WHERE "id" = 'ba_3'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("admin")
    );

    // The ladder is closed: a level outside it is refused before any SQL
    // runs, so there is no row whose position in the ordering is
    // undefined. A guard that ENUMERATES the qualifying levels would
    // otherwise silently exclude such a row rather than refuse it.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"grantBoardsAccess($board: "board_1", $member: "mem_4", $level: "root")"#,
            ANA,
        )
        .expect_err("GC036: `root` is not a declared level");
    assert_eq!(class_of(&e), "param_type_mismatch", "{e}");
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "boardsAccess" WHERE "member" = 'mem_4'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("0"),
        "nothing applied"
    );
    // The converge path owes the same refusal — a level change is where
    // an off-ladder value would be easiest to slip in.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"setBoardsAccessLevel($board: "board_1", $member: "mem_3", $level: "root")"#,
            ANA,
        )
        .expect_err("GC036: and on the converge, not only the create");
    assert_eq!(class_of(&e), "param_type_mismatch", "{e}");
    assert_eq!(
        pg.query(r#"SELECT "level" FROM "boardsAccess" WHERE "id" = 'ba_3'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("read"),
        "nothing applied"
    );
}

#[test]
fn gc047_direct_writes_to_the_link_are_admin_gated() {
    let f = fixture();
    let pg = db("ordo_gc_write_e2");

    // GC047 — the derived guards protect the link entity against direct
    // writes too: grant-by-`set` is still admin-gated (R-G14). Bram holds
    // `write` on board_1, Ana holds `admin`.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"sneakGrant($board: "board_1", $member: "mem_4")"#,
            BRAM,
        )
        .expect_err("GC047: `write` is not `admin`");
    assert_eq!(class_of(&e), "access_guard", "GC047: {e}");

    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"sneakGrant($board: "board_1", $member: "mem_4")"#,
        ANA,
    )
    .expect("GC047: Ana is admin of board_1");
    assert!(
        exec(&f, &pg, "GC047", "aclRoster", &[], ANA)
            .as_array()
            .unwrap()
            .contains(&json!({"board": "board_1", "member": "mem_4", "level": "admin"})),
        "GC047: the grant lands"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// F. Guards × composition and refinement (GC049–GC058)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc049_to_gc056_guards_survive_composition() {
    let f = fixture();
    let pg = seeded_reads(&f);

    // GC049 — composing a guarded query keeps the guard: the guard rides
    // inside `openTasks`, which `specTasks` composes.
    let gc049 = |as_| exec(&f, &pg, "GC049", "specTasks", &[], as_);
    assert_eq!(gc049(ANA), json!([{"title": "draft spec"}]));
    assert_eq!(gc049(BRAM), json!([{"title": "draft spec"}]));
    assert_eq!(gc049(CLEO), json!([]));
    assert_eq!(gc049(DOV), json!([]));

    // GC050 — the refinement envelope: the guard applies first, in the
    // callee body, and the refinement can never reach rows it removed.
    let gc050 = |as_| {
        exec(
            &f,
            &pg,
            "GC050",
            "openTasks(project == $p)[title]",
            &[("p", "\"proj_2\"")],
            as_,
        )
    };
    assert_eq!(
        gc050(BRAM),
        json!([{"title": "build site"}, {"title": "ship site"}])
    );
    assert_eq!(gc050(ANA), json!([]));
    assert_eq!(gc050(DOV), json!([]));

    // GC051 — the guard runs before the callee's `take` (R-G7):
    // `firstTask` yields each caller's own top row, and the caller
    // condition then filters that ONE-ROW result — not "first matching".
    let gc051 = |as_| {
        exec(
            &f,
            &pg,
            "GC051",
            "firstTask(title contains \"audit\")[title]",
            &[],
            as_,
        )
    };
    assert_eq!(gc051(ANA), json!([{"title": "audit books"}]));
    assert_eq!(
        gc051(BRAM),
        json!([]),
        "GC051: his firstTask is (build site)"
    );
    assert_eq!(gc051(CLEO), json!([]));

    // GC053 — depth-2 composition, the guard intact into an aggregate.
    assert_eq!(
        exec(&f, &pg, "GC053", "specCount", &[], ANA),
        json!([{"n": 1}])
    );
    assert_eq!(
        exec(&f, &pg, "GC053", "specCount", &[], DOV),
        json!([{"n": 0}])
    );

    // GC054 — a composed guarded source with a refinement over
    // acl-guarded data; the callee's order is the result order.
    let gc054 = |as_| exec(&f, &pg, "GC054", "openCards", &[], as_);
    assert_eq!(
        gc054(ANA),
        json!([{"title": "q3 goals", "board": "board_1"}])
    );
    assert_eq!(
        gc054(BRAM),
        json!([
            {"title": "q3 goals", "board": "board_1"},
            {"title": "someday",  "board": "board_2"},
        ])
    );
    assert_eq!(
        gc054(GIL),
        json!([{"title": "q3 goals", "board": "board_1"}])
    );

    // GC055 — a guarded composed source under `match any` beside a
    // guarded entity source: BOTH guards compose. A project qualifies
    // only if the caller reads it AND has a visible open task in it.
    let gc055 = |as_| {
        exec(
            &f,
            &pg,
            "GC055",
            "projects[name]\nmatch any openTasks",
            &[],
            as_,
        )
    };
    assert_eq!(gc055(ANA), json!([{"name": "Alpha"}, {"name": "Gamma"}]));
    assert_eq!(gc055(BRAM), json!([{"name": "Alpha"}, {"name": "Beta"}]));
    assert_eq!(
        gc055(CLEO),
        json!([{"name": "Beta"}]),
        "GC055: proj_3 is readable via ownership but has no Cleo-visible tasks"
    );

    // GC056 — parameter flow-through leaves the guard intact.
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC056",
            "tasksIn($p: $p)",
            &[("p", "\"proj_1\"")],
            BRAM
        ),
        json!([{"title": "draft spec", "status": "open"}, {"title": "review spec", "status": "done"}])
    );
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC056",
            "tasksIn($p: $p)",
            &[("p", "\"proj_1\"")],
            CLEO
        ),
        json!([])
    );
}

#[test]
fn gc057_the_artifact_is_caller_independent() {
    let f = fixture();
    // One planned statement, and the SAME statement for every principal:
    // `$caller` is a binding slot, so no per-caller compilation happens.
    let pl = ordo_syntax::parse_pipeline("taskBoard").expect("GC057: parses");

    // Compiled with no caller, `$caller` is a reserved binding SLOT — the
    // plan carries it as a parameter and nothing else.
    let slot = ordo_sema::bind(&f.schema, &f.registry, &pl, ordo_sema::Clock::CORPUS)
        .expect("GC057: binds");
    let slot_plan = ordo_plan::plan(
        &slot,
        &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0),
    )
    .expect("GC057: plans");
    assert_eq!(
        slot_plan.params,
        vec!["caller".to_string()],
        "GC057: `$caller` is a binding slot in the plan, and the only one"
    );

    // And every principal gets that one plan with the slot filled: one
    // planned statement, identical up to the bound value. No per-caller
    // compilation ever happens.
    let mut shapes = std::collections::BTreeSet::new();
    for who in [ANA, BRAM, CLEO, DOV, GIL] {
        let q = ordo_sema::bind_as(
            &f.schema,
            &f.registry,
            &pl,
            &[],
            who,
            ordo_sema::Clock::CORPUS,
        )
        .expect("GC057: binds");
        let p = ordo_plan::plan(
            &q,
            &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0),
        )
        .expect("GC057: plans");
        assert!(
            p.params.is_empty(),
            "GC057: the slot is filled, not left open"
        );
        shapes.insert(p.sql.replace(who.unwrap(), "«caller»"));
    }
    assert_eq!(
        shapes.len(),
        1,
        "GC057: five principals, one plan — got {shapes:#?}"
    );
}

#[test]
fn gc058_guard_internals_are_not_outputs() {
    let f = fixture();
    // `canWrite` is read by tasks' WRITE guard and is not in `openTasks`'
    // output shape (R-G16). There is no introspection channel through
    // which a caller can probe another principal's guard facts.
    let e = f
        .bind_case("openTasks(canWrite == true)[title]")
        .expect_err("GC058: `canWrite` is not an output field");
    assert!(
        e.contains("not_output_field"),
        "GC058: expected `not_output_field`, got: {e}"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// G. Guest access (GC059–GC066)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc059_and_gc060_the_guest_reaches_exactly_their_grant() {
    let f = fixture();
    let pg = seeded_reads(&f);

    // GC059 — ba_5 is read on board_1: nothing more, nothing less.
    assert_eq!(
        exec(&f, &pg, "GC059", "boardList", &[], GIL),
        json!([{"name": "roadmap"}])
    );
    assert_eq!(
        exec(&f, &pg, "GC059", "cardList", &[], GIL),
        json!([
            {"title": "q3 goals", "done": false, "board": "board_1"},
            {"title": "q4 goals", "done": true,  "board": "board_1"},
        ])
    );

    // GC060 — no role fallback: empty results, never an error, never a
    // default view.
    assert_eq!(exec(&f, &pg, "GC060", "openTasks", &[], GIL), json!([]));
    assert_eq!(exec(&f, &pg, "GC060", "myProjects", &[], GIL), json!([]));
}

#[test]
fn gc061_and_gc066_the_guest_window_opens_and_closes() {
    let f = fixture();
    let pg = db("ordo_gc_write_g");

    // GC061 — a guest write attempt at read level.
    let mut log = fresh(&f, &pg);
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addCard($board: "board_1", $title: "guest card")"#,
            GIL,
        )
        .expect_err("GC061: Gil holds `read` on board_1");
    assert_eq!(class_of(&e), "access_guard", "GC061: {e}");

    // GC066 — revocation closes the window at the next call. One seed:
    // revoke, then read. Through the DERIVED op, which is what an admin
    // surface actually calls.
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"revokeBoardsAccess($board: "board_1", $member: "mem_g")"#,
        ANA,
    )
    .expect("GC066: Ana is admin of board_1");
    assert_eq!(exec(&f, &pg, "GC066", "boardList", &[], GIL), json!([]));
    assert_eq!(exec(&f, &pg, "GC066", "cardList", &[], GIL), json!([]));
}

// ═════════════════════════════════════════════════════════════════════════
// H. `$caller` lifecycle (GC067–GC073)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc069_caller_in_a_shape_default_is_refused() {
    // Defaults are evaluated from the closed set (literals, `today`,
    // `now`, `newid`) with no session in scope.
    let e = schema_with("entity drafts {\n  author -> members = $caller\n  body: text\n}")
        .expect_err("GC069: `$caller` in a shape default");
    assert_eq!(class_of(&e), "caller_in_default", "GC069: {e}");
}

#[test]
fn gc071_and_gc072_caller_conditions_and_the_anonymous_principal() {
    let f = fixture();
    let pg = seeded_reads(&f);

    // GC071 — a `$caller` condition and an entity guard conjoin; neither
    // substitutes for the other.
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC071",
            "myOpenIn($p: $p)",
            &[("p", "\"proj_2\"")],
            CLEO
        ),
        json!([{"title": "build site"}])
    );
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC071",
            "myOpenIn($p: $p)",
            &[("p", "\"proj_2\"")],
            BRAM
        ),
        json!([]),
        "GC071: no visible assignment there"
    );

    // GC072 (read half) — every guard evaluates false for an anonymous
    // caller, as a defined rule rather than null-comparison folklore.
    assert_eq!(exec(&f, &pg, "GC072", "openTasks", &[], ANON), json!([]));
    assert_eq!(exec(&f, &pg, "GC072", "myProjects", &[], ANON), json!([]));
    assert_eq!(exec(&f, &pg, "GC072", "cardList", &[], ANON), json!([]));
    assert_eq!(exec(&f, &pg, "GC072", "noteFeed", &[], ANON), json!([]));

    // GC072 (write half) — R-G12 routed through R-G10. Every guarded row
    // is outside an anonymous caller's read view, so an ADDRESSED write
    // answers `not_found`, byte-identical to a row that never existed.
    // This was `access_guard` until 10d: the wrong class, and the sharpest
    // form of the leak R-G10 closes, since an anonymous caller with a
    // stolen id list could sort the real ids from the invented ones.
    //
    // Its own database: `seeded_reads` is shared across this binary's read
    // cases, and a write case that resets it pulls the seed out from under
    // whichever read case is mid-query.
    let wpg = db("ordo_10d_anon_write");
    let mut log = fresh(&f, &wpg);
    let pg = &wpg;
    let real = f
        .run_write_as(pg, &mut log, r#"closeTask($task: "task_1")"#, ANON)
        .expect_err("GC072: an anonymous write is refused");
    let made_up = f
        .run_write_as(pg, &mut log, r#"closeTask($task: "task_404")"#, ANON)
        .expect_err("GC072: an anonymous write is refused");
    assert_eq!(class_of(&real), "not_found", "GC072: {real}");
    assert_eq!(
        real, made_up,
        "GC072: and the row that exists is refused identically to the one that does not"
    );
    // The refusal that is still `access_guard`, and R-G12 says why in as
    // many words: a CREATING write "addresses no row to be coy about, and
    // the guard simply evaluates false".
    let created = f
        .run_write_as(
            pg,
            &mut log,
            r#"addTask($project: "proj_1", $title: "x")"#,
            ANON,
        )
        .expect_err("GC072: an anonymous create is refused");
    assert_eq!(class_of(&created), "access_guard", "GC072: {created}");
    // Nothing was applied by any of the three: the seed's four open tasks
    // are still exactly four, and there is no fifth.
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "tasks""#).unwrap()[0][0].as_deref(),
        Some("5"),
        "GC072: an anonymous write never lands, whatever it is told"
    );
    assert_eq!(
        pg.query(r#"SELECT "status" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("open"),
        "GC072: and the row the addressed write named is untouched"
    );
}

#[test]
fn gc070_caller_in_a_write_rhs_binds_the_session_principal() {
    let f = fixture();
    let pg = db("ordo_gc_write_h");

    // GC070 — legal, and it binds the session principal (R-G2).
    let mut log = fresh(&f, &pg);
    f.run_write_as(&pg, &mut log, r#"claimTask($task: "task_3")"#, BRAM)
        .expect("GC070: Bram holds canWrite in proj_2");
    assert_eq!(
        pg.query(r#"SELECT "assignee" FROM "tasks" WHERE "id" = 'task_3'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("mem_2")
    );
    assert_eq!(
        exec(&f, &pg, "GC070", "myAssignedTasks", &[], BRAM),
        json!([{"title": "build site"}]),
        "GC070: task_5 is also assigned to Bram and stays guard-hidden"
    );
}

#[test]
fn gc073_caller_scoping_is_disclosed_metadata() {
    let f = fixture();
    // `myAssignedTasks` uses `$caller` in its own body; `taskBoard` gets
    // it from the guard. Both are caller-scoped, and the guard clause is
    // what the catalog surface names — visibility shaping lives in the
    // op, never in an invisible policy.
    //
    // Caller-scoping is observable as a compiled fact: the plan of a
    // caller-scoped op mentions the principal, so two principals get two
    // statements from one artifact.
    for op in ["myAssignedTasks", "taskBoard"] {
        let pl = ordo_syntax::parse_pipeline(op).expect("GC073: parses");
        let ana = ordo_sema::bind_as(
            &f.schema,
            &f.registry,
            &pl,
            &[],
            ANA,
            ordo_sema::Clock::CORPUS,
        )
        .expect("GC073: binds");
        let bram = ordo_sema::bind_as(
            &f.schema,
            &f.registry,
            &pl,
            &[],
            BRAM,
            ordo_sema::Clock::CORPUS,
        )
        .expect("GC073: binds");
        let ana_sql = ordo_plan::plan(
            &ana,
            &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0),
        )
        .expect("plans")
        .sql;
        let bram_sql = ordo_plan::plan(
            &bram,
            &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0),
        )
        .expect("plans")
        .sql;
        assert_ne!(ana_sql, bram_sql, "GC073: `{op}` is caller-scoped");
        assert!(
            ana_sql.contains("mem_1"),
            "GC073: `{op}` binds the principal"
        );
    }

    // The two ops are caller-scoped for different reasons, and the
    // artifact says so differently. `taskBoard`'s `$caller` is
    // guard-injected, so the caller-free compilation carries it as a
    // reserved binding slot; `myAssignedTasks` names `$caller` in its own
    // body, and a caller-free compilation of it does not exist at all.
    // Either way, no `taskBoard`/`myAssignedTasks` plan is caller-blind.
    let pl = ordo_syntax::parse_pipeline("taskBoard").expect("GC073: parses");
    let slot = ordo_sema::bind(&f.schema, &f.registry, &pl, ordo_sema::Clock::CORPUS)
        .expect("GC073: binds");
    assert_eq!(
        ordo_plan::plan(
            &slot,
            &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0)
        )
        .expect("plans")
        .params,
        vec!["caller".to_string()],
        "GC073: guard-injected caller-scoping is a binding slot in the artifact"
    );
    let pl = ordo_syntax::parse_pipeline("myAssignedTasks").expect("GC073: parses");
    let e = ordo_sema::bind(&f.schema, &f.registry, &pl, ordo_sema::Clock::CORPUS)
        .expect_err("GC073: a body `$caller` has no caller-free compilation");
    assert_eq!(e.code, "unbound_required_param", "GC073: {e:?}");
    // A read with no `$caller` anywhere — body or guard — carries no such
    // slot, so the flag distinguishes rather than decorating everything.
    let pl = ordo_syntax::parse_pipeline("vendorNames").expect("GC073: parses");
    let q = ordo_sema::bind(&f.schema, &f.registry, &pl, ordo_sema::Clock::CORPUS)
        .expect("GC073: binds");
    assert!(
        ordo_plan::plan(
            &q,
            &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0)
        )
        .expect("plans")
        .params
        .is_empty(),
        "GC073: `vendorNames` is not caller-scoped"
    );
    let g = f.schema.entity("tasks").unwrap().guards.as_ref().unwrap();
    assert_eq!(
        g.render(true),
        "project.projectMembers(member == $caller)",
        "GC073: the guard clause is what the catalog names"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// I. Guard grammar rejects (GC074–GC079)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc074_to_gc079_the_guard_grammar_is_closed() {
    // Each of these cases is about the READ walk, and each entity spells
    // BOTH verbs — the write one `open` — because an `access` declaration is
    // both verbs or neither (spec G9) and a half-guarded entity is refused
    // `incomplete_access_clause` before the walk is looked at. The corpus
    // text used to declare `access read` alone and therefore did not compile;
    // 10g fixed the DOCUMENT, applying R-G21's own prescribed repair ("the
    // symmetric case … is the same error and the same fix: `access read
    // open`"). This is now the doc's text verbatim, not a local repair.
    let open_write = "\n  access write  open";

    // GC074 — R-G1 direct: a guard on a sealed field of the entity itself.
    assert_eq!(
        class_of(
            &schema_with(&format!(
                "entity privateDocs {{\n  owner -> members\n  label: text = \"\"   sealed\n  \
             body:  text\n  access read  owner == $caller or label == \"public\"{open_write}\n}}"
            ))
            .unwrap_err()
        ),
        "guard_on_sealed",
        "GC074"
    );

    // GC075 — R-G1 through a walk: `orders.memo` is sealed in the base
    // `market` fixture.
    assert_eq!(
        class_of(&schema_with(
            &format!("entity orderFlags {{\n  order   -> orders  owned\n  handler -> members\n  \
             note: text\n  access read  handler == $caller or order.memo == \"gift\"{open_write}\n}}")
        ).unwrap_err()),
        "guard_on_sealed",
        "GC075"
    );

    // GC076 — no `$caller` in any disjunct: a caller-blind guard is a
    // hidden filter, not authorization — even built from legal atoms.
    assert_eq!(
        class_of(
            &schema_with(&format!(
                "entity banners {{\n  project -> projects  owned\n  live: bool = false\n  \
             access read  live{open_write}\n}}"
            ))
            .unwrap_err()
        ),
        "invalid_guard_walk",
        "GC076"
    );

    // GC077 — functions and operators outside the closed grammar. The
    // document's class, and now the compiler's: `ParseError` carries a
    // class (9r), so `> 3` after a well-formed atom is refused as the
    // walk's own defect rather than as unreadable text. Was DIVERGENT.
    assert_eq!(
        class_of(
            &schema_with(&format!(
                "entity notices {{\n  project -> projects  owned\n  title: text\n  \
             access read  project.owner == $caller or length(title) > 3{open_write}\n}}"
            ))
            .unwrap_err()
        ),
        "invalid_guard_walk",
        "GC077"
    );

    // GC078 — the walk must follow declared refs: `boardsAccess` declares
    // no ref to `memos`, so the reverse step is unreachable from here.
    assert_eq!(
        class_of(
            &schema_with(&format!(
                "entity memos {{\n  author -> members\n  body: text\n  \
             access read  boardsAccess(member == $caller){open_write}\n}}"
            ))
            .unwrap_err()
        ),
        "invalid_guard_walk",
        "GC078"
    );

    // GC079 — `access` takes exactly `read` or `write`; `admin` exists
    // only as an `acl` level. (No repair needed: it never parses.)
    assert_eq!(
        class_of(
            &schema_with(
                "entity wikis {\n  owner -> members\n  title: text\n  \
             access admin  owner == $caller\n}"
            )
            .unwrap_err()
        ),
        "parse_error",
        "GC079"
    );

    // The gate the repair above steps over is itself worth pinning: a
    // half-guarded entity is refused, which is what makes `open` the only
    // way to say "deliberately not guarded on this verb" (R-G21).
    assert_eq!(
        class_of(
            &schema_with(
                "entity halfGuarded {\n  project -> projects  owned\n  body: text\n  \
             access read  project.projectMembers(member == $caller)\n}"
            )
            .unwrap_err()
        ),
        "incomplete_access_clause",
        "an entity guarded on one verb only is refused"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// J. Metadata and cost (GC080–GC082) and the disclosure cases
// ═════════════════════════════════════════════════════════════════════════

/// The document's scope manifest for one op, spelled the way §J spells it:
/// `tasks.{status, title, project}`, entity-sorted, fields sorted.
///
/// This reads `ordo_plan::artifact::op_scope` — the compiler's own
/// derivation off the bound IR — so a case asserting it is asserting the
/// document against the engine, not against a second reading of the source
/// text written to agree.
fn scope_of(f: &Fixture, op: &str) -> Vec<String> {
    ordo_plan::artifact::op_scope(&f.schema, &f.registry, op)
        .iter()
        .map(|s| {
            if s.read.is_empty() && s.written.is_empty() {
                s.entity.clone()
            } else {
                let all: Vec<&str> = s
                    .read
                    .iter()
                    .chain(s.written.iter())
                    .map(|x| x.as_str())
                    .collect();
                format!("{}.{{{}}}", s.entity, all.join(", "))
            }
        })
        .collect()
}

#[test]
fn gc007_gc027_gc048_gc052_guard_provenance_is_disclosed() {
    let f = fixture();

    // GC007 (compile half) — the plan reads `projects` as well as
    // `tasks`, so both read guards inline and both are disclosed.
    let q = ordo_sema::bind_op_generic(
        &f.schema,
        &f.registry,
        "taskSheet",
        ordo_sema::Clock::CORPUS,
    )
    .expect("GC007: binds");
    let sql = ordo_plan::plan(
        &q,
        &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0),
    )
    .expect("GC007: plans")
    .sql;
    for guard in [
        "projectMembers", // tasks' guard
        "projects",       // the dot-path target, guarded in its own right
    ] {
        assert!(
            sql.contains(guard),
            "GC007: `{guard}` must appear in the plan:\n{sql}"
        );
    }
    assert_eq!(
        f.schema
            .entity("projects")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .render(true),
        "owner == $caller or projectMembers(member == $caller)",
        "GC007: projects' own read guard is disclosed alongside tasks'"
    );
    // The scope half (9l): the document asks for `projects.{name}` plus the
    // guard-read fields, and the manifest now states exactly that at field
    // grain. `projects` carries `{name}` because the dot-path reads it;
    // `owner` is absent because nothing reads it — see the recorded
    // divergence about the dot-hop join carrying no guard of its own.
    assert_eq!(
        scope_of(&f, "taskSheet"),
        [
            "projectMembers.{member, project}",
            "projects.{name}",
            "tasks.{project, title}",
        ],
        "GC007: scope manifest — projects.{{name}} plus the guard reads"
    );

    // GC027 — inheritance is disclosed, REBASED, and attributed.
    let notes = f
        .schema
        .entity("taskNotes")
        .unwrap()
        .guards
        .as_ref()
        .unwrap();
    assert_eq!(
        notes.render(true),
        "task.project.projectMembers(member == $caller)",
        "GC027: the rebased walk"
    );
    assert_eq!(
        notes.source,
        GuardSource::Inherited {
            from: "tasks".into()
        },
        "GC027: attributed `inherited from tasks`"
    );
    // The scope half: the rebased walk's own reads are in the manifest —
    // `tasks.{project}` and `projectMembers.{project, member}`, neither of
    // which `noteFeed` names. `projects` is in the walk with no column of
    // it read at all: the semi-join asks whether the row exists.
    assert_eq!(
        scope_of(&f, "noteFeed"),
        [
            "projectMembers.{member, project}",
            "projects",
            "taskNotes.{body, task}",
            "tasks.{project}",
        ],
        "GC027: the inherited guard's reads are disclosed at field grain"
    );

    // GC048 — inherited-ACL disclosure on `cards`.
    let cards = f.schema.entity("cards").unwrap().guards.as_ref().unwrap();
    assert_eq!(
        cards.source,
        GuardSource::Inherited {
            from: "boards".into()
        },
        "GC048: attributed `inherited from boards (acl)`"
    );
    assert_eq!(
        cards.render(true),
        "board.boardsAccess(member == $caller) or board.boardsAccess(member.groupMembers.group(member == $caller))",
        "GC048: the boards ACL read guard, rebased through the owned ref"
    );
    // The scope half: the derived link entity's own fields appear as guard
    // reads. `level` does not — this is a READ, and the read guard never
    // tests it (the same post-pruning discipline GC080 states for
    // `canWrite`). `guards-conformance.md`'s prose used to list `level`
    // here too; that was the document's error, ruled and retired — see
    // `corpus-rulings.md`.
    assert_eq!(
        scope_of(&f, "cardList"),
        [
            "boards",
            "boardsAccess.{board, member}",
            "cards.{board, done, title}",
            // The group walk the derivation now emits, disclosed in the
            // scope exactly as DL asks of it: the derivation "discloses that
            // it does by showing the walk it generated".
            "groupMembers.{group, member}",
            "members",
        ],
        "GC048: the acl link entity's guard reads are disclosed"
    );

    // GC052 — a composed manifest discloses the callee's guard: the
    // guard is tasks', reached through `openTasks`.
    let q = ordo_sema::bind_op_generic(
        &f.schema,
        &f.registry,
        "specTasks",
        ordo_sema::Clock::CORPUS,
    )
    .expect("GC052: binds");
    let sql = ordo_plan::plan(
        &q,
        &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0),
    )
    .expect("GC052: plans")
    .sql;
    assert!(
        sql.contains("projectMembers"),
        "GC052: tasks' read guard travels through the composition:\n{sql}"
    );
    assert_eq!(ordo_plan::cost::classify(&q).as_str(), "sql", "GC052");
    // GC052's scope half, the document's words exactly: `tasks.{status,
    // title, project}` + `projectMembers.{project, member}`. Composition
    // flattens before the scope is derived, so the callee's guard reads are
    // the caller's exposure with no special case.
    assert_eq!(
        scope_of(&f, "specTasks"),
        [
            "projectMembers.{member, project}",
            "projects",
            "tasks.{project, status, title}",
        ],
        "GC052: a composed manifest carries the callee's guard reads"
    );
}

#[test]
fn gc080_gc081_gc082_guard_semi_joins_classify_sql() {
    let f = fixture();

    // GC080 — a guard semi-join is still an `sql`-class read.
    let q = ordo_sema::bind_op_generic(
        &f.schema,
        &f.registry,
        "openTasks",
        ordo_sema::Clock::CORPUS,
    )
    .expect("GC080: binds");
    assert_eq!(ordo_plan::cost::classify(&q).as_str(), "sql", "GC080");
    // …and the scope manifest the case states, at field grain (9l):
    // `tasks.{status, title, project}` + `projectMembers.{project, member}`
    // as guard reads, with `canWrite` ABSENT — the read guard never touches
    // it, which is the post-pruning discipline (composition R-C11).
    assert_eq!(
        scope_of(&f, "openTasks"),
        [
            "projectMembers.{member, project}",
            "projects",
            "tasks.{project, status, title}",
        ],
        "GC080: scope manifest, guard reads folded in"
    );

    // GC081 — write-op metadata: the mode label is unchanged by guards,
    // and the write guard is the precondition.
    let close = f.registry.get("closeTask").expect("GC081: registered");
    let ordo_syntax::decl::OpBody::Write(stmts) = &close.body else {
        panic!("GC081: `closeTask` is a write op");
    };
    // A conditioned `set` addresses rows that already exist, which is what
    // makes the label `converges`; the classifier that publishes it reads
    // exactly this shape. Guards do not enter the derivation at all —
    // which is the assertion: they leave the mode label unchanged.
    assert!(
        matches!(
            &stmts[0],
            ordo_syntax::decl::WriteStmt::Set { cond: Some(_), .. }
        ),
        "GC081: `closeTask` is a conditioned set — mode label `converges`"
    );
    // The unconditioned form, from the one classifier, so the word above
    // is not this test's own invention.
    let add = f.registry.get("addTask").expect("GC081: registered");
    let ordo_syntax::decl::OpBody::Write(add_stmts) = &add.body else {
        panic!("GC081: `addTask` is a write op");
    };
    let ordo_syntax::decl::WriteStmt::Set { entity, fields, .. } = &add_stmts[0] else {
        panic!("GC081: a `set`");
    };
    assert_eq!(
        ordo_conformance::write::set_mode(&f.schema, &entity.name, fields).label(),
        "creates",
        "GC081: and a guarded create is still labelled `creates`"
    );
    // §6 rule 1's four labels, from the one classifier every renderer
    // reads. The conditioned forms are the pair the old three-way
    // heuristic collapsed: `id ==` converges ONE row, and any other
    // condition writes every row it matches.
    let label = |op: &str| {
        let d = f.registry.get(op).expect("registered");
        let ordo_syntax::decl::OpBody::Write(s) = &d.body else {
            panic!("{op} is a write op")
        };
        ordo_plan::writes::op_label(&f.schema, s)
    };
    assert_eq!(label("closeTask"), "converges", "GC081");
    assert_eq!(
        label("shipAll"),
        "bulk",
        "GC081: rule 1's fourth label — a ranged set is not a converge"
    );
    assert_eq!(label("addTask"), "creates", "GC081");
    assert_eq!(
        label("closePair"),
        "converges",
        "GC081: two statements, one label, because both agree"
    );
    // The write guard's own reads are in the scope manifest, and `canWrite`
    // is there — the WRITE guard is the one that tests it.
    assert_eq!(
        scope_of(&f, "closeTask"),
        [
            "projectMembers.{canWrite, member, project}",
            "projects",
            "tasks.{project, status}",
        ],
        "GC081: guard reads projectMembers.{{project, member, canWrite}}"
    );
    assert_eq!(
        f.schema
            .entity("tasks")
            .unwrap()
            .guards
            .as_ref()
            .unwrap()
            .render(false),
        "project.projectMembers(member == $caller and canWrite)",
        "GC081: the write guard, listed as a precondition"
    );

    // GC082 — the depth-2 rebased guard is still one indexed semi-join
    // chain, and still `sql`.
    let q =
        ordo_sema::bind_op_generic(&f.schema, &f.registry, "noteFeed", ordo_sema::Clock::CORPUS)
            .expect("GC082: binds");
    assert_eq!(ordo_plan::cost::classify(&q).as_str(), "sql", "GC082");
    let sql = ordo_plan::plan(
        &q,
        &ordo_plan::Space::Bound(ordo_conformance::fixture_space().0),
    )
    .expect("GC082: plans")
    .sql;
    for t in ["taskNotes", "tasks", "projectMembers"] {
        assert!(sql.contains(t), "GC082: the walk reaches `{t}`:\n{sql}");
    }
    // The document's manifest for it, at field grain: `taskNotes.{body,
    // task}` + `tasks.{project}` + `projectMembers.{project, member}`.
    // `tasks.project` is the case a naive derivation misses — nothing in
    // `noteFeed`'s source text mentions `tasks` at all.
    assert_eq!(
        scope_of(&f, "noteFeed"),
        [
            "projectMembers.{member, project}",
            "projects",
            "taskNotes.{body, task}",
            "tasks.{project}",
        ],
        "GC082: manifest taskNotes.{{body, task}} + tasks.{{project}} + \
         projectMembers.{{project, member}}"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// L. Platform `members`, non-enumerability, acl bootstrap (GC088–GC093)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc088_members_is_platform_provided_and_undeclarable() {
    // The reject half: the name is reserved in the entity namespace — a
    // domain refs `members`, it never declares it.
    let e = schema_with("entity members {\n  name: text\n}").unwrap_err();
    assert_eq!(class_of(&e), "parse_error", "GC088: {e}");

    // The exec half: the projection every roster screen wants is
    // `displayName`, and `members.name` does not exist.
    let f = fixture();
    let pg = seeded_reads(&f);
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC088",
            "teamNames($p: $p)",
            &[("p", "\"proj_1\"")],
            ANA
        ),
        json!([{"displayName": "Ana"}, {"displayName": "Bram"}])
    );
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC088",
            "teamNames($p: $p)",
            &[("p", "\"proj_1\"")],
            DOV
        ),
        json!([]),
        "GC088: the link entity's own guard, and Dov holds none"
    );
    let e = f
        .bind_case("projectMembers(project == \"proj_1\")[member.name]")
        .expect_err("GC088: there is no `members.name`");
    // The document's class, and now the binder's. A miss is classed by the
    // POSITION that named the field; this one is a projection, so it is
    // `projection_unknown_field` whether the path is dotted or not.
    // `unknown_field` is internal vocabulary that no closed inventory
    // names. Was DIVERGENT.
    assert!(
        e.contains("projection_unknown_field") && e.contains("`name`"),
        "GC088: {e}"
    );
}

#[test]
fn gc089_and_gc090_non_enumerability_and_its_one_exception() {
    let f = fixture();
    let pg = db("ordo_gc_write_l");

    // ── GC089 — the four answers must be indistinguishable in pairs ────
    let mut log = fresh(&f, &pg);
    let (forbidden, forbidden_sql) = record_write(
        &f,
        &pg,
        &mut log,
        r#"renameTask($task: "task_1", $title: "x")"#,
        DOV,
    );
    let forbidden = forbidden.expect_err("GC089: task_1 exists, Dov may not reach it");
    let (absent, absent_sql) = record_write(
        &f,
        &pg,
        &mut log,
        r#"renameTask($task: "task_404", $title: "x")"#,
        DOV,
    );
    let absent = absent.expect_err("GC089: task_404 never existed");
    assert_eq!(class_of(&forbidden), "not_found", "GC089: {forbidden}");
    assert_eq!(
        forbidden, absent,
        "GC089: same status, same code, same MESSAGE — literally the same bytes, not the \
         same bytes once the id is masked out"
    );
    // The same claim from the other side: nothing about the refusal varies
    // with the id, including its length.
    assert_eq!(
        forbidden.len(),
        absent.len(),
        "GC089: a length is an oracle too"
    );
    // ── GC089's timing assertion, made deterministic. The document asks
    // that the two invocations be cost-close because the guard semi-join
    // runs whatever the outcome. Asserting a wall-clock band on a shared
    // cluster measures the cluster; asserting the STATEMENTS measures the
    // code path, and a short-circuit ahead of the guard would show up here
    // as a missing statement rather than as a flaky millisecond. ────────
    assert_eq!(
        shapes(&forbidden_sql, &["task_1", "task_404"]),
        shapes(&absent_sql, &["task_1", "task_404"]),
        "GC089: forbidden and absent must do the same work, statement for statement — \
         forbidden issued {} statements, absent {}",
        forbidden_sql.len(),
        absent_sql.len()
    );
    assert!(
        forbidden_sql
            .iter()
            .any(|s| s.contains("projectMembers") || s.contains("EXISTS")),
        "GC089: and the work they share is the guard semi-join, not an existence probe"
    );
    let informative = f
        .run_write_as(
            &pg,
            &mut log,
            r#"renameTask($task: "task_1", $title: "x")"#,
            BRAM,
        )
        .expect_err("GC089: Bram may read task_1 (pm_2)");
    assert_eq!(
        class_of(&informative),
        "access_guard",
        "GC089: the third answer, and the only informative one: {informative}"
    );

    // The read side, same rule: a FILTERED read is empty, not an error —
    // R-G10 governs ADDRESSED reads and writes.
    assert_eq!(
        exec(
            &f,
            &pg,
            "GC089",
            "taskOne($id: $id)",
            &[("id", "\"task_1\"")],
            DOV
        ),
        json!([]),
        "GC089: the same shape a caller gets for a row that does not exist"
    );

    // ── GC090 — the unguarded exception. `products` carries no `access`
    // clause and no inherited `acl`, so there is no read view to be
    // outside of and `not_found` means only "absent". ─────────────────
    assert!(
        f.schema.entity("products").unwrap().guards.is_none(),
        "GC090: `products` must be unguarded for this case to mean anything"
    );
    let mut log = fresh(&f, &pg);
    f.run_write_as(
        &pg,
        &mut log,
        r#"renameProduct($product: "prod_1", $name: "x")"#,
        DOV,
    )
    .expect("GC090: no guard to fail");
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            r#"renameProduct($product: "prod_404", $name: "x")"#,
            DOV,
        )
        .expect_err("GC090: absent, and honestly so");
    assert_eq!(class_of(&e), "not_found", "GC090: {e}");
}

/// **R-G10, on the delete verb: authorization decides before structure.**
///
/// Not a corpus case — a defect the 10d/10e sweep found next door to one.
/// `plan_addressed_del` ran `refuse_tree_parent` and `refuse_with_referrers`
/// *before* the two visibility checks, so a row the caller could not read
/// answered `tree_has_children` or `has_referrers` while an id that never
/// existed answered `not_found`. Two ids, two classes, and the difference is
/// a fact about a row the caller may not see: the enumeration oracle R-G10
/// exists to close, reached through the one verb GC089 does not exercise.
///
/// Four callers, four answers, and the order of the checks is exactly what
/// separates them. `task_1` exists in proj_1 and is referenced by `tlink_1`.
#[test]
fn an_addressed_delete_decides_authorization_before_structure() {
    // `market.ordo` spells the addressed delete only for `folders` and
    // `categories`, neither of which is both guarded and referenced. The
    // referrer is an association (`taskLinks.task`): since `owned-delete`
    // (2026-09-29) an owned referrer such as `note_1` goes with its task,
    // so only a plain one still refuses.
    let src = format!(
        "{}\nentity taskLinks (key: tlink) {{\n  task -> tasks\n}}\n{}\n\
         op dropTask($task: id) =\n  del tasks {{ id: $task }}\n",
        ordo_conformance::FIXTURE,
        gc_support::LOCAL_OPS
    );
    let f = Fixture::from_source(&src, ordo_conformance::SEED).expect("the program compiles");
    let pg = db("ordo_10d_del_order");

    // Dov holds no membership: task_1 is outside his read view. He must be
    // told exactly what he is told about an id that never existed —
    // `not_found`, and not one byte more, even though task_1 has a referrer
    // and task_404 could not have one.
    let mut log = fresh(&f, &pg);
    pg.exec(
        "INSERT INTO \"taskLinks\" (\"$space\", \"id\", \"task\") \
         VALUES ('conformance', 'tlink_1', 'task_1')",
    )
    .expect("a plain referrer of task_1");
    let forbidden = f
        .run_write_as(&pg, &mut log, r#"dropTask($task: "task_1")"#, DOV)
        .expect_err("task_1 is outside Dov's read view");
    let absent = f
        .run_write_as(&pg, &mut log, r#"dropTask($task: "task_404")"#, DOV)
        .expect_err("task_404 never existed");
    assert_eq!(
        class_of(&forbidden),
        "not_found",
        "a referenced row the caller cannot read is not `has_referrers`: {forbidden}"
    );
    assert_eq!(
        forbidden, absent,
        "and it is byte-identical to the id that never existed"
    );

    // Bram may READ task_1 (pm_2) but holds no `canWrite` there. He already
    // knows the row is there, so the informative answer is the safe one —
    // and it is still the WRITE guard's answer, not the referrer's.
    let unwritable = f
        .run_write_as(&pg, &mut log, r#"dropTask($task: "task_1")"#, BRAM)
        .expect_err("Bram may read task_1 but not write it");
    assert_eq!(
        class_of(&unwritable),
        "access_guard",
        "a caller who may not delete at all is not told about the referrer: {unwritable}"
    );

    // Ana holds `canWrite` in proj_1, so she reaches the structural check —
    // which still fires. Moving it after the guards did not remove it.
    let referenced = f
        .run_write_as(&pg, &mut log, r#"dropTask($task: "task_1")"#, ANA)
        .expect_err("tlink_1 still points at task_1");
    assert_eq!(
        class_of(&referenced),
        "has_referrers",
        "the structural refusal survives, for the caller entitled to hear it: {referenced}"
    );
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("1"),
        "and none of the four refusals deleted anything"
    );
}

#[test]
fn gc091_and_gc092_the_acl_bootstrap() {
    let f = fixture();
    let pg = db("ordo_gc_write_acl");

    // The compile-side disclosure that makes the rest reviewable: the acl
    // root's write guard is self-rooted, so a create is guard-exempt.
    assert!(
        ordo_sema::create_exempt(&f.schema, "boards"),
        "GC091: `boards` carries `create: guard-exempt (self-rooted guard)`"
    );
    assert!(
        !ordo_sema::create_exempt(&f.schema, "cards"),
        "GC091: `cards`' guard is inherited through the owned ref and therefore \
         PARENT-rooted — evaluated normally on a create (GC041 stands unchanged)"
    );
    assert!(
        !ordo_sema::create_exempt(&f.schema, "tasks"),
        "GC091: a guard rooted at an already-existing row is not exempt"
    );

    // GC091 — Cleo holds no acl row anywhere on `boards`; without the
    // exemption the derived guard would refuse the very create the
    // same-group grant exists to make reachable.
    let mut log = fresh(&f, &pg);
    f.run_write_as(&pg, &mut log, r#"openBoard($name: "q6")"#, CLEO)
        .expect("GC091: the create is guard-exempt");
    let roster = exec(&f, &pg, "GC091", "aclRoster", &[], CLEO);
    assert!(
        roster.as_array().unwrap().iter().any(|r| {
            r["member"] == json!("mem_3")
                && r["level"] == json!("admin")
                && r["board"] != json!("board_1")
        }),
        "GC091: the creator receives the top level, minted in the same command \
         group as the create — got {roster}"
    );

    // Dov holds nothing anywhere: what bounds this create is the DOOR
    // plus the op body, not a guard.
    let mut log = fresh(&f, &pg);
    f.run_write_as(&pg, &mut log, r#"openBoard($name: "q6")"#, DOV)
        .expect("GC091: applied likewise");
    assert!(
        exec(&f, &pg, "GC091", "aclRoster", &[], DOV)
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["member"] == json!("mem_4") && r["level"] == json!("admin")),
        "GC091: (board_3, mem_4, admin)"
    );
}

/// **GC095 — `let x = set …` binds a create, and only a create (R-G19).**
///
/// Unrunnable until 9e built the production: `LetDecl` bound a read
/// pipeline and there was no write-valued `let` at all, which is why
/// `unbindable_write` sat in the closed inventory with **no producer**.
/// This is that producer, and what raises it is the *derived mode* rather
/// than anything about the spelling.
///
/// GC094 — the same construct's guard interaction — is still not covered,
/// and 9e narrowed the reason to something that is not about `let`: see
/// `NOT_COVERED`.
#[test]
fn gc095_a_let_binds_a_create_and_only_a_create() {
    let e = Fixture::from_source(
        &format!(
            "{}\nop renameBoardBad($id: id, $n: text) =\n  \
               let b = set boards {{ id: $id, name: $n }}\n  \
               set boardsAccess {{ board: b.id, member: $caller, level: \"admin\" }}\n",
            ordo_conformance::FIXTURE
        ),
        ordo_conformance::SEED,
    )
    .map(|_| ())
    .expect_err("GC095: a converge binds nothing");
    assert!(
        e.contains("unbindable_write"),
        "GC095: `unbindable_write` — the derived mode is CONVERGE, which \
         addresses a row whose id the caller already holds: {e}"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// N. The explicit-open spelling (GC099–GC100)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn gc099_and_gc100_open_is_a_keyword_not_an_expression() {
    // GC099 — `access write open` is the explicit-open clause. The
    // `project` ref is what makes the read walk reachable (R-G3, GC078).
    let s = schema_with(
        "entity announcements {\n  project -> projects  owned\n  body:    text\n  \
         access read   project.projectMembers(member == $caller)\n  \
         access write  open\n}",
    )
    .expect("GC099: `open` is accepted");
    let g = s.entity("announcements").unwrap().guards.as_ref().unwrap();
    assert_eq!(g.write, GuardBody::Open, "GC099");
    assert_eq!(
        g.render(false),
        "open",
        "GC099: every manifest renders the word — greppable by design, one \
         word with one meaning, listed rather than omitted"
    );

    // GC100 — the withdrawn spelling. `open` is a keyword: it is the
    // entire clause or it is a parse error.
    assert_eq!(
        class_of(
            &schema_with(
                "entity announcements2 {\n  project -> projects  owned\n  body:    text\n  \
             access read   project.projectMembers(member == $caller)\n  \
             access write  true\n}"
            )
            .unwrap_err()
        ),
        "parse_error",
        "GC100: `access write true` never parsed and is withdrawn"
    );

    // Partial-open is also unspellable, and that is deliberate — and now
    // unspellable under the document's own class. It used to read `open`
    // as an ordinary path atom and land on the schema builder's
    // `invalid_guard_walk` ("`announcements3` has no field `open`"),
    // which is a message about a field the author never meant and one
    // that would go on being right the day somebody declares a `bool`
    // called `open`. Was DIVERGENT.
    assert_eq!(
        class_of(
            &schema_with(
                "entity announcements3 {\n  project -> projects  owned\n  body:    text\n  \
             access read   project.projectMembers(member == $caller) or open\n  \
             access write  open\n}"
            )
            .unwrap_err()
        ),
        "parse_error",
        "GC100: `open` cannot appear inside a walk, nowhere, not as an operand"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Coverage
// ═════════════════════════════════════════════════════════════════════════

/// Cases with a real, executing assertion in this file.
const EXECUTED: &[&str] = &[
    "GC001", "GC002", "GC003", "GC004", "GC005", "GC006", "GC007", "GC008", "GC009", "GC010",
    "GC011", "GC012", "GC013", "GC014", "GC015", "GC016", "GC017", "GC018", "GC019", "GC020",
    "GC021", "GC022", "GC023", "GC024", "GC025", "GC026", "GC027", "GC028", "GC029", "GC030",
    "GC031", "GC032", "GC033", "GC034", "GC035", "GC036", "GC037", "GC038", "GC046", "GC048",
    "GC039", "GC040", "GC041", "GC042", "GC043", "GC044", "GC045", "GC047", "GC049", "GC050",
    "GC051", "GC052", "GC053", "GC054", "GC055", "GC056", "GC057", "GC058", "GC059", "GC060",
    "GC061", "GC062", "GC066", "GC069", "GC070", "GC071", "GC072", "GC073", "GC074", "GC075",
    "GC076", "GC077", "GC078", "GC079", "GC080", "GC081", "GC082", "GC083", "GC088", "GC089",
    "GC090", "GC091", "GC092", "GC095", "GC099", "GC100",
];

/// Cases that execute in another crate, because what they exercise is the
/// host's and this crate cannot depend on it. Each names the test, and the
/// coverage test below checks that the test exists.
///
/// GC063 to GC065 were "not run" while sharing was to be a `share to guest:`
/// line in source. It is not: a share is registered by the host
/// (`share_ops::validate`, over `register_share`), and the cases run there
/// (`docs/plan-sharing.md` G8).
const EXECUTED_ELSEWHERE: &[(&str, &str)] = &[
    (
        "GC063",
        "crates/ordo-host/tests/sharing.rs::gc063_gc064_gc065_registration",
    ),
    (
        "GC064",
        "crates/ordo-host/tests/sharing.rs::gc063_gc064_gc065_registration",
    ),
    (
        "GC065",
        "crates/ordo-host/tests/sharing.rs::gc063_gc064_gc065_registration",
    ),
    // R-G22: the delegation is the door's, and this crate has no door.
    (
        "GC101",
        "crates/ordo-host/tests/delegation.rs::a_delegated_read_sees_exactly_the_delegators_rows_and_not_one_more",
    ),
];

/// Cases that cannot run yet, each naming the construct that is unbuilt.
/// Not a to-do list — a claim a reader can check against the tree.
const NOT_RUN: &[(&str, &str)] = &[
    (
        "GC067",
        "the door refuses a client-supplied `$caller` in `ordo-api`'s router; this \
               crate cannot depend on `ordo-api` (it depends on this one). Covered there",
    ),
    ("GC068", "same — the refusal is a router-tier assertion"),
    (
        "GC084",
        "`?live` subscriptions are `ordo-host`'s invalidation surface; this crate \
               cannot depend on it",
    ),
    ("GC085", "same"),
    (
        "GC086",
        "same — the derived admin ops exist now; `?live` does not",
    ),
    ("GC087", "same"),
    (
        "GC093",
        "the space-owner acl-administration capability is a DOOR capability and no \
               door exists in this crate. The ops it would reach exist now; the \
               capability that lets an owner past their guards does not",
    ),
    (
        "GC094",
        "the `let` production exists now (9e) and GC095 runs, but GC094's own \
               claim does not: the dependent write is authorized by the AUTO-GRANT \
               the create puts in the same group, and this harness plans every \
               statement's preconditions before applying any statement's SQL \
               (AA-004), so that row is not in the database when the second \
               statement's create gate probes for it. The blocker is the write \
               protocol's staging, not the binding — a ref to a group-created row \
               IS admitted, because that one is answerable without the database",
    ),
    (
        "GC096",
        "routines are `ordo-exec`'s trigger surface; this crate cannot depend on it",
    ),
    ("GC097", "same"),
    ("GC098", "same"),
];

/// Where `docs/guards-conformance.md` and the implementation disagree.
///
/// Each is asserted **as the implementation actually behaves**, so this
/// list fails the day the implementation is fixed and forces the entry to
/// be retired. A divergence is a finding, not a case to bend.
const DIVERGENT: &[(&str, &str)] = &[
    (
        "GC083",
        "the derived admin ops exist and carry their mode labels, but the manifest spells \
         their gate as the link entity's write WALK (`board.boardsAccess(member == $caller \
         and level == \"admin\")`) rather than as the case's `gatedBy: boardsAccess.level \
         == \"admin\"`, and carries no cost class for a write op at all — \
         `ordo_plan::op_artifact` sets `cost: null` for writes deliberately, so \"classify \
         `sql`\" has nothing to assert against. Same facts, different vocabulary; the case \
         is counted on the halves that have a surface.",
    ),
    (
        "GC007",
        "the document says the dot-path target's read guard INLINES (R-G6 stacking), and \
         it does not: `taskSheet`'s plan LEFT JOINs `projects` with no guard on the join \
         at all — the only `projects` reference under a guard is the one tasks' own walk \
         makes. The fixture hides it (a project member satisfies both guards), and the \
         field-scope manifest now shows the shape: `projects.{name}` is read while \
         `projects.owner` — which its own guard would test — is not.",
    ),
    // Retired 2026-08-30 (10g), both by fixing the DOCUMENT — the compiler was
    // right in each: `GC033` (+GC034/GC035) omitted the `unbounded` that
    // composition-R9 puts on the caller, and `GC074–GC078` declared one verb
    // where spec G9 requires both. See `docs/corpus-rulings.md`.
    //
    // Retired 2026-09-26: `GC048` said `guards-conformance.md` listed
    // `boardsAccess.{board, member, level}` as `cardList`'s guard reads,
    // when `level` is tested only by the WRITE guard and `cardList` is a
    // read — the same post-pruning discipline the document itself states
    // for `canWrite` in GC080. The two cases could not both be right, and
    // the engine already follows GC080's rule (the assertion above has
    // always excluded `level`). The owner's ruling ("implement the fix, I
    // will take your suggestion") settled it by fixing the DOCUMENT, not
    // the engine: `guards-conformance.md`'s GC048 prose drops `level` from
    // the manifest it quotes. See `docs/corpus-rulings.md`.
];

/// The claim and the suite cannot drift: every id in `EXECUTED` must
/// appear in a case above, and no id may be in two lists.
#[test]
fn gc_coverage_is_reported_not_implied() {
    let src = include_str!("guards_corpus.rs");

    for id in EXECUTED {
        // Twice: once in the list, at least once in a case.
        assert!(
            src.matches(id).count() >= 2,
            "{id} is claimed as executed but appears nowhere in a case"
        );
    }
    let host_tests = [
        (
            "crates/ordo-host/tests/sharing.rs",
            include_str!("../../ordo-host/tests/sharing.rs"),
        ),
        (
            "crates/ordo-host/tests/delegation.rs",
            include_str!("../../ordo-host/tests/delegation.rs"),
        ),
    ];
    for (id, test) in EXECUTED_ELSEWHERE {
        let (file, name) = test.split_once("::").expect("a test path");
        let src = host_tests
            .iter()
            .find(|(f, _)| *f == file)
            .unwrap_or_else(|| panic!("{id}: `{file}` is not one this check reads"))
            .1;
        assert!(
            src.contains(&format!("fn {name}(")),
            "{id} is claimed as executed in `{test}`, which does not exist"
        );
        assert!(
            !EXECUTED.contains(id) && !NOT_RUN.iter().any(|(n, _)| n == id),
            "{id} is in two lists"
        );
    }
    for (id, reason) in NOT_RUN {
        assert!(
            !EXECUTED.contains(id),
            "{id} cannot be both executed and not-run"
        );
        assert!(
            !reason.trim().is_empty(),
            "{id}: a not-run claim carries its reason"
        );
    }

    let total = 101;
    let covered = EXECUTED.len() + EXECUTED_ELSEWHERE.len();
    println!(
        "\nguards corpus coverage: {covered}/{total} GC cases execute\n  \
         ({} of them in another crate)\n  \
         not run: {} (each naming an unbuilt construct)\n  \
         document/implementation divergences recorded: {}\n  \
         accounted for: {}/{total}",
        EXECUTED_ELSEWHERE.len(),
        NOT_RUN.len(),
        DIVERGENT.len(),
        covered + NOT_RUN.len(),
    );
    for (id, reason) in NOT_RUN {
        println!("    {id}: {reason}");
    }
    for (id, what) in DIVERGENT {
        println!("  DIVERGENCE {id}: {what}");
    }
    assert!(covered < total, "the coverage claim must stay honest");
    assert_eq!(
        covered + NOT_RUN.len(),
        total,
        "every one of the corpus's {total} cases is either executed or explained"
    );
}

// ── The divergences, asserted as they stand ──────────────────────────────

/// **GC092 — a guest creator gets no auto-grant.**
///
/// Spec §3 bootstrap rule 4 and R-G11a: the auto-grant follows **space
/// membership, not authorship**, and Gil's `members` row carries a null
/// `role`, which is what makes him a guest. Without the rule, "a guest
/// invited into one corner of a space could manufacture rows they
/// exclusively administer and then extend access to principals of their
/// choosing" — the document's own words.
///
/// The create still **applies**. R-G11a's exemption is about the guard,
/// not about who the caller is; what bounds this create is the door plus
/// the op body. Refusing it would be a second, undeclared authorization
/// check — and it would also refuse rule 5's routine-created rows, which
/// the spec explicitly expects to land ungranted. The orphan is not an
/// orphan: bootstrap rule 6 makes the space owner the repair path
/// (GC093).
#[test]
fn gc092_a_guest_creator_gets_no_auto_grant() {
    let f = fixture();
    let pg = db("ordo_gc_guest_bootstrap");
    let mut log = fresh(&f, &pg);

    f.run_write_as(&pg, &mut log, r#"openBoard($name: "gil's board")"#, GIL)
        .expect("GC092: applied — the exemption is about the guard, not the caller");
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "boards" WHERE "name" = 'gil''s board'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("1"),
        "GC092: the board exists"
    );
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "boardsAccess" WHERE "member" = 'mem_g'"#)
            .unwrap()[0][0]
            .as_deref(),
        Some("1"),
        "GC092: only Gil's SEEDED row (ba_5) — zero access rows minted for board_3"
    );
    // The consequence the rule exists for: he cannot reach the board he
    // just made, and the roster he sees is unchanged.
    assert_eq!(
        exec(&f, &pg, "GC092", "boardList", &[], GIL),
        json!([{"name": "roadmap"}]),
        "GC092: **not** the board he just made"
    );
    assert_eq!(
        exec(&f, &pg, "GC092", "aclRoster", &[], GIL)
            .as_array()
            .unwrap()
            .len(),
        4,
        "GC092: unchanged — the four board_1 rows of GC046"
    );

    // The other half of the same rule, and the reason it is one rule
    // rather than two: a MEMBER creating the same row does receive the
    // top level, in the same command group (GC091 asserts the group; this
    // asserts that membership is what separates the two answers).
    let mut log = fresh(&f, &pg);
    f.run_write_as(&pg, &mut log, r#"openBoard($name: "cleo's board")"#, CLEO)
        .expect("GC092: a member's create");
    assert_eq!(
        pg.query(
            r#"SELECT "level" FROM "boardsAccess" AS a
               JOIN "boards" AS b ON b."id" = a."board"
               WHERE b."name" = 'cleo''s board'"#
        )
        .unwrap()[0][0]
            .as_deref(),
        Some("admin"),
        "GC092: a member creator receives the top level"
    );
}
