//! **`each ->` routine bodies** (DL §8, Tier 1; owner ruling 2026-10-08,
//! `triggers` ask 1).
//!
//! A routine reads rows with a pipeline and runs its writes once for each.
//! These cases drive `Fixture::run_routine_on` against real Postgres, with
//! the per-run limit and the change bound handed in directly, so each
//! property is visible on its own:
//!
//! 1. every matching row is written, and no other;
//! 2. a run handles at most the limit, and says when more matched;
//! 3. a run is one command group, attributed `routine:<name>@<version>`;
//! 4. a change run reaches only the rows its trigger reaches.
//!
//! The end-to-end path, with the program's own wiring and the real
//! reachability walk, is `ordo-host/tests/routines.rs`.
//!
//! Real Postgres, this suite's own databases (`ordo_each_*`).

use ordo_commandlog::Actor;
use ordo_conformance::{Fixture, Pg, WriteLog};
use std::collections::BTreeMap;

const SHAPES: &str = r#"domain lease @ 1.0.0

entity units (key: un) {
  name: text
}

entity leases (key: ls) {
  unit -> units
  end:    date
  status: text = "active"
  rent:   money(2) = 0
}

entity alerts (key: al) {
  key:    text unique
  title:  text
  status: text = "active"
}

op addUnit($name: text) =
  set units { name: $name }

op addLease($unit: id, $end: date, $rent: money) =
  set leases { unit: $unit, end: $end, rent: $rent }
"#;

const ROUTINES: &str = r#"
# Expire leases past their end: the write takes the row out of the
# selection, so the next run picks up where this one stopped.
routine expire {
  trigger every day 08:00Z
  run($today: date):
    leases(status == "active" and end < $today)[id]
    each ->
      set leases { id: id, status: "expired" }
}

# One alert per lease ending soon, converged on its key, carrying values
# from the row and from a row one ref away.
routine leaseAlerts in lease {
  trigger every day 08:00Z
  run($today: date):
    leases(status == "active" and end <= $today + 60d)
      [id, end, unit.name, rent]
    each ->
      set alerts { key: "leaseExpiry:" + id,
                   title: unit.name + " lease ends " + end + " at " + rent }
}

# Woken by a change: mark the unit of each expired lease vacant.
routine vacate {
  trigger when lease.leases.converged
  run:
    leases(status == "expired")[id, unit]
    each ->
      set units { id: unit, name: "vacant" }
}
"#;

fn fixture() -> Fixture {
    Fixture::from_source(&format!("{SHAPES}{ROUTINES}"), "").expect("the program checks")
}

fn routine<'a>(f: &'a Fixture, name: &str) -> &'a ordo_syntax::RoutineDecl {
    f.schema
        .routines
        .iter()
        .find(|r| r.name.name == name)
        .expect("declared")
}

/// 2026-10-08T09:00:00Z.
const NOW: u64 = 1_791_450_000;

fn fresh(db: &str) -> (Fixture, Pg, WriteLog) {
    let f = fixture();
    let pg = Pg::named(db);
    f.reset(&pg).expect("fresh database");
    (f, pg, WriteLog::new())
}

fn created(v: &serde_json::Value) -> String {
    v[0]["created"].as_str().expect("a create").to_string()
}

fn add_unit(f: &Fixture, pg: &Pg, log: &mut WriteLog, name: &str) -> String {
    created(
        &f.run_write_as(
            pg,
            log,
            &format!("addUnit($name: \"{name}\")"),
            Some("mem_1"),
        )
        .expect("unit"),
    )
}

fn add_lease(f: &Fixture, pg: &Pg, log: &mut WriteLog, unit: &str, end: &str) -> String {
    created(
        &f.run_write_as(
            pg,
            log,
            &format!("addLease($unit: \"{unit}\", $end: {end}, $rent: 12.50)"),
            Some("mem_1"),
        )
        .expect("lease"),
    )
}

fn run(
    f: &Fixture,
    pg: &Pg,
    log: &mut WriteLog,
    name: &str,
    rows: &ordo_conformance::routine::RoutineRows,
    each_max: usize,
) -> serde_json::Value {
    let out = f
        .run_routine_on(
            pg,
            log,
            routine(f, name),
            &format!("{name}@v1"),
            rows,
            NOW,
            each_max,
        )
        .expect("the run applies");
    log.commit_pending().expect("appended");
    out
}

fn statuses(pg: &Pg) -> Vec<(String, String)> {
    pg.query(r#"SELECT "id", "status" FROM "leases" ORDER BY "id""#)
        .expect("readback")
        .into_iter()
        .map(|r| {
            (
                r[0].clone().unwrap_or_default(),
                r[1].clone().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn an_each_routine_touches_every_matching_row_and_no_other() {
    let (f, pg, mut log) = fresh("ordo_each_matching");
    let u = add_unit(&f, &pg, &mut log, "A1");
    let past: Vec<String> = ["2026-01-01", "2026-09-30", "2026-10-07"]
        .iter()
        .map(|d| add_lease(&f, &pg, &mut log, &u, d))
        .collect();
    let future: Vec<String> = ["2026-10-08", "2027-01-01"]
        .iter()
        .map(|d| add_lease(&f, &pg, &mut log, &u, d))
        .collect();

    let out = run(&f, &pg, &mut log, "expire", &None, 500);
    assert_eq!(out["rows"], 3, "{out}");
    assert_eq!(out["more"], false, "{out}");

    for (id, status) in statuses(&pg) {
        let want = if past.contains(&id) {
            "expired"
        } else {
            assert!(future.contains(&id));
            "active"
        };
        assert_eq!(status, want, "lease {id}");
    }
}

#[test]
fn an_each_routine_stops_at_the_per_run_limit_and_the_next_run_finishes_the_rest() {
    let (f, pg, mut log) = fresh("ordo_each_limit");
    let u = add_unit(&f, &pg, &mut log, "A1");
    for d in [
        "2026-01-01",
        "2026-01-02",
        "2026-01-03",
        "2026-01-04",
        "2026-01-05",
    ] {
        add_lease(&f, &pg, &mut log, &u, d);
    }
    let expired = |pg: &Pg| statuses(pg).iter().filter(|(_, s)| s == "expired").count();

    let first = run(&f, &pg, &mut log, "expire", &None, 2);
    assert_eq!(
        (first["rows"].clone(), first["more"].clone()),
        (2.into(), true.into())
    );
    assert_eq!(expired(&pg), 2, "the limit holds");

    let second = run(&f, &pg, &mut log, "expire", &None, 2);
    assert_eq!(
        (second["rows"].clone(), second["more"].clone()),
        (2.into(), true.into())
    );
    assert_eq!(expired(&pg), 4);

    let third = run(&f, &pg, &mut log, "expire", &None, 2);
    assert_eq!(
        (third["rows"].clone(), third["more"].clone()),
        (1.into(), false.into())
    );
    assert_eq!(expired(&pg), 5, "the next runs finish the rest");

    let before = log.len();
    let fourth = run(&f, &pg, &mut log, "expire", &None, 2);
    assert_eq!(fourth["rows"], 0);
    assert_eq!(
        log.len(),
        before,
        "a run with nothing to do appends nothing"
    );
}

#[test]
fn an_each_run_is_one_command_group_attributed_to_the_routine() {
    let (f, pg, mut log) = fresh("ordo_each_group");
    let u = add_unit(&f, &pg, &mut log, "A1");
    for d in ["2026-01-01", "2026-02-01", "2026-03-01"] {
        add_lease(&f, &pg, &mut log, &u, d);
    }
    let before = log.len();
    run(&f, &pg, &mut log, "expire", &None, 500);
    assert_eq!(
        log.len(),
        before + 1,
        "one group for the run, not one a row"
    );
    let g = log.last().expect("the run's group");
    assert_eq!(g.actor, Actor::Routine("expire@v1".into()));
    assert_eq!(g.commands.len(), 3, "every row's write is in it: {g:?}");
    assert!(g.commands.iter().all(|c| c.entity == "leases"));
}

#[test]
fn an_each_emission_reads_the_rows_fields_and_converges_on_a_key() {
    let (f, pg, mut log) = fresh("ordo_each_values");
    let u = add_unit(&f, &pg, &mut log, "Flat 2");
    let soon = add_lease(&f, &pg, &mut log, &u, "2026-11-01");
    add_lease(&f, &pg, &mut log, &u, "2027-06-01");

    run(&f, &pg, &mut log, "leaseAlerts", &None, 500);
    // A second run converges the same alert rather than adding one.
    run(&f, &pg, &mut log, "leaseAlerts", &None, 500);
    let alerts = pg
        .query(r#"SELECT "key", "title" FROM "alerts""#)
        .expect("readback");
    assert_eq!(
        alerts,
        vec![vec![
            Some(format!("leaseExpiry:{soon}")),
            Some("Flat 2 lease ends 2026-11-01 at 12.50".into()),
        ]]
    );
}

#[test]
fn a_change_run_of_an_each_routine_reaches_only_the_changed_rows() {
    let (f, pg, mut log) = fresh("ordo_each_bound");
    let near = add_unit(&f, &pg, &mut log, "Near");
    let far = add_unit(&f, &pg, &mut log, "Far");
    let changed = add_lease(&f, &pg, &mut log, &near, "2026-01-01");
    let other = add_lease(&f, &pg, &mut log, &far, "2026-01-01");
    run(&f, &pg, &mut log, "expire", &None, 500);

    // The change was to `changed`. What it reaches is that lease and the
    // unit it points at; `other` and its unit are out of reach, though
    // the pipeline's condition matches `other` too.
    let mut reach = BTreeMap::new();
    reach.insert("leases".to_string(), vec![changed.clone()]);
    reach.insert("units".to_string(), vec![near.clone()]);
    let out = run(&f, &pg, &mut log, "vacate", &Some(reach), 500);
    assert_eq!(
        out["rows"], 1,
        "the pipeline read the reachable lease only: {out}"
    );
    let names = pg
        .query(r#"SELECT "id", "name" FROM "units" ORDER BY "name""#)
        .expect("readback");
    assert_eq!(
        names,
        vec![
            vec![Some(far.clone()), Some("Far".into())],
            vec![Some(near.clone()), Some("vacant".into())],
        ]
    );

    // A lease the change reaches whose unit it does not: the converge of
    // that unit is out of bound and is left alone.
    let mut reach = BTreeMap::new();
    reach.insert("leases".to_string(), vec![other.clone()]);
    let before = log.len();
    let out = run(&f, &pg, &mut log, "vacate", &Some(reach), 500);
    assert_eq!(out["rows"], 1, "{out}");
    assert_eq!(log.len(), before, "nothing in reach to write");
    let far_name = pg
        .query(&format!(
            r#"SELECT "name" FROM "units" WHERE "id" = '{far}'"#
        ))
        .expect("readback");
    assert_eq!(far_name, vec![vec![Some("Far".into())]]);
}

// ── what the checker refuses ─────────────────────────────────────────────

fn checks(routine: &str) -> Result<(), String> {
    Fixture::from_source(&format!("{SHAPES}{routine}"), "").map(|_| ())
}

#[test]
fn a_name_the_row_does_not_carry_is_refused() {
    let e = checks(
        "routine r {\n  trigger every 5m\n  run:\n    leases[id]\n    each ->\n      \
         set leases { id: id, status: unit }\n}\n",
    )
    .expect_err("`unit` was not projected");
    assert!(e.contains("invalid_routine"), "{e}");
    assert!(e.contains("not a field of the row"), "{e}");
}

#[test]
fn a_change_routine_may_converge_by_id_and_may_not_create_without_unguarded() {
    checks(
        "routine r {\n  trigger when lease.leases.created\n  run:\n    leases[id]\n    each ->\n      \
         set leases { id: id, status: \"seen\" }\n}\n",
    )
    .expect("a converge by id is bounded at run time");
    let e = checks(
        "routine r {\n  trigger when lease.leases.created\n  run:\n    leases[id]\n    each ->\n      \
         set alerts { key: id, title: \"new\" }\n}\n",
    )
    .expect_err("a keyed create reaches past the changed rows");
    assert!(e.contains("routine_out_of_bound"), "{e}");
    checks(
        "routine r {\n  trigger when lease.leases.created\n  run unguarded:\n    leases[id]\n    each ->\n      \
         set alerts { key: id, title: \"new\" }\n}\n",
    )
    .expect("unguarded ranges wider on purpose");
}

#[test]
fn an_each_emission_passes_the_write_rules() {
    let e = checks(
        "routine r {\n  trigger every 5m\n  run:\n    leases[id]\n    each ->\n      \
         set alerts { key: id }\n}\n",
    )
    .expect_err("`title` is required");
    assert!(e.contains("insert_incomplete"), "{e}");
}
