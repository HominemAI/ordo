//! **Equality parity** (board `guard-equality-uses-no-index`, owner ruling
//! 2026-10-26).
//!
//! The planner compiles `==` to a plain `=` where `ordo_plan::never_null`
//! proves a side is never null, and keeps `IS NOT DISTINCT FROM` everywhere
//! else. The ruling allows that only where both forms return the same rows.
//! This suite runs both forms and requires the same rows, in the same order,
//! over data built to break the rule if it were wrong:
//!
//! - every fixture's own seed, plus generated rows in every table;
//! - every optional field null on every other row, and every optional ref
//!   empty on those rows;
//! - every read op of each program (authored, `acl`-derived and the std
//!   library), the corpus cases written against each fixture, and every
//!   entity's read and write guard on its own;
//! - as each member of the space, and as nobody.
//!
//! The fixtures are the market corpus, the wide corpus and notes v2, whose
//! guards are the ones the benchmark measured.
//!
//! Every case the conformance harness runs elsewhere is checked the same way
//! on that suite's own data (`Fixture::equality_parity`).

use ordo_conformance::{Fixture, Pg};
use ordo_schema::ddl::{physical_columns, ColumnRole};
use ordo_schema::FieldTy;
use ordo_syntax::decl::{OpBody, ScalarTy};

const NOTES_V2: &str = include_str!("../../../examples/notes-v2/notes.ordo");

/// Generated rows per table, on top of the fixture's seed.
const GENERATED: usize = 12;

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// A value for one generated cell, row `i`.
fn value(f: &ordo_schema::Field, i: usize) -> String {
    if f.sealed {
        return "'\\x00'::bytea".into();
    }
    if let Some(values) = &f.enum_values {
        return quote(&values[i % values.len()]);
    }
    let FieldTy::Scalar(s) = &f.ty else {
        unreachable!("refs are filled separately")
    };
    match s {
        ScalarTy::Text | ScalarTy::Id => quote(&format!("v{}", i % 5)),
        ScalarTy::Uri => quote(&format!("https://example.invalid/{}", i % 5)),
        ScalarTy::Int | ScalarTy::Money(_) | ScalarTy::Duration(_) => format!("{}", i % 7),
        ScalarTy::Decimal(_) => format!("{}.5", i % 7),
        ScalarTy::Bool => i.is_multiple_of(2).to_string(),
        ScalarTy::Date => quote(&format!("2026-01-{:02}", 1 + i % 28)),
        ScalarTy::Datetime => quote(&format!("2026-01-01T00:00:{:02}Z", i % 60)),
        ScalarTy::Json => quote(&format!("{{\"k\": {}, \"tags\": [\"v{}\"]}}", i % 3, i % 2)),
        ScalarTy::Blob if f.inline => "'\\x00'::bytea".into(),
        ScalarTy::Blob => quote(&format!("{:032x}", i)),
    }
}

/// The ids a table holds, in id order.
fn ids(pg: &Pg, table: &str) -> Vec<String> {
    pg.query(&format!("SELECT id FROM {} ORDER BY id", ident(table)))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect()
}

/// Add [`GENERATED`] rows to every table and empty every optional field on
/// every other row. Returns the optional columns that now hold a null.
///
/// Each row is inserted in its own savepoint, so a row a `check` or a unique
/// key refuses is skipped and the rest land.
fn null_saturate(f: &Fixture, pg: &Pg) -> Vec<String> {
    let space = f.space().0.clone();
    let null_digest = |entity: &str, field: &str| -> String {
        let d = f
            .keys
            .blind_index(f.space(), entity, field, ordo_crypto::BlindValue::Null)
            .expect("a null digest");
        format!("'\\x{}'::bytea", ordo_crypto::hex(&d))
    };

    // Tables whose required refs all point at tables with rows, until none
    // is left: a ref is filled from its target, so the target goes first.
    let mut pending: Vec<&ordo_schema::Entity> = f.schema.entities.iter().collect();
    let mut done: Vec<String> = Vec::new();
    for _ in 0..pending.len() + 1 {
        let (ready, later): (Vec<_>, Vec<_>) = pending.into_iter().partition(|e| {
            e.fields.iter().all(|fl| match &fl.ty {
                FieldTy::Ref { target, .. } if !fl.optional => {
                    target == &e.name || done.contains(target) || !ids(pg, target).is_empty()
                }
                _ => true,
            })
        });
        pending = later;
        for e in ready {
            let mut stmts = Vec::new();
            for i in 0..GENERATED {
                let id = format!("{}_gen{i:02}", e.key);
                let mut cols = Vec::new();
                let mut vals = Vec::new();
                for pc in physical_columns(e) {
                    let v = match pc.role {
                        ColumnRole::Space => quote(&space),
                        ColumnRole::Path => quote(&format!("{id}/")),
                        ColumnRole::BlindIndex(fl) => null_digest(&e.name, &fl.name),
                        ColumnRole::Field(fl) if fl.name == "id" => quote(&id),
                        ColumnRole::Field(fl) if fl.name == "rev" => "1".into(),
                        ColumnRole::Field(fl) => match &fl.ty {
                            // A tree's parent stays empty: its path says root.
                            FieldTy::Ref { tree: true, .. } if fl.optional => "NULL".into(),
                            FieldTy::Ref { target, .. } => {
                                let t = ids(pg, target);
                                if t.is_empty() {
                                    "NULL".into()
                                } else {
                                    quote(&t[(i * 7 + e.name.len()) % t.len()])
                                }
                            }
                            FieldTy::Scalar(_) => value(fl, i),
                        },
                    };
                    cols.push(ident(&pc.name));
                    vals.push(v);
                }
                stmts.push(format!(
                    "BEGIN INSERT INTO {} ({}) VALUES ({}); \
                     EXCEPTION WHEN OTHERS THEN NULL; END;",
                    ident(&e.name),
                    cols.join(", "),
                    vals.join(", ")
                ));
            }
            pg.exec(&format!("DO $gen$ BEGIN {} END $gen$;", stmts.join(" ")))
                .unwrap_or_else(|err| panic!("generating {}: {err}", e.name));
            done.push(e.name.clone());
        }
        if pending.is_empty() {
            break;
        }
    }
    assert!(
        pending.is_empty(),
        "tables no rows could be generated for: {:?}",
        pending.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // Every optional field null on every other row. All at once where the
    // table allows it, else column by column, else on one row.
    let mut nulled = Vec::new();
    for e in &f.schema.entities {
        let optional: Vec<&ordo_schema::Field> = e
            .fields
            .iter()
            .filter(|fl| fl.optional && !fl.implicit)
            .collect();
        if optional.is_empty() {
            continue;
        }
        let every_other = format!(
            "id IN (SELECT id FROM (SELECT id, row_number() OVER (ORDER BY id) AS n \
             FROM {t}) x WHERE n % 2 = 0)",
            t = ident(&e.name)
        );
        let one = format!(
            "id = (SELECT id FROM {t} ORDER BY id LIMIT 1)",
            t = ident(&e.name)
        );
        let set = |cols: &[&ordo_schema::Field]| {
            cols.iter()
                .map(|fl| format!("{} = NULL", ident(&fl.column())))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let all = format!(
            "UPDATE {} SET {} WHERE {every_other}",
            ident(&e.name),
            set(&optional)
        );
        if pg.exec(&all).is_ok() {
            nulled.extend(optional.iter().map(|fl| format!("{}.{}", e.name, fl.name)));
            continue;
        }
        for fl in optional {
            for rows in [&every_other, &one] {
                let stmt = format!("UPDATE {} SET {} WHERE {rows}", ident(&e.name), set(&[fl]));
                if pg.exec(&stmt).is_ok() {
                    nulled.push(format!("{}.{}", e.name, fl.name));
                    break;
                }
            }
        }
    }
    pg.exec("ANALYZE").expect("analyze");
    nulled
}

/// A literal for a parameter of type `ty`, trial `k`, or `None` when the
/// type has no literal here. Ids walk through `ids`.
fn literal(ty: ScalarTy, k: usize, ids: &[String]) -> Option<String> {
    Some(match ty {
        ScalarTy::Id => format!("\"{}\"", ids[k % ids.len()]),
        ScalarTy::Text => ["\"\"", "\"v1\"", "\"a\""][k % 3].to_string(),
        ScalarTy::Int | ScalarTy::Money(_) | ScalarTy::Decimal(_) => format!("{}", k % 3),
        ScalarTy::Bool => k.is_multiple_of(2).to_string(),
        ScalarTy::Date => "2026-01-02".into(),
        ScalarTy::Datetime => "2026-01-01T00:00:30Z".into(),
        ScalarTy::Uri => "\"https://example.invalid/1\"".into(),
        ScalarTy::Json => "\"{}\"".into(),
        ScalarTy::Duration(_) | ScalarTy::Blob => return None,
    })
}

#[derive(Default, Debug)]
struct Tally {
    /// Queries whose two forms are different SQL, run both ways.
    compared: usize,
    /// Queries the rule left exactly as they were.
    same_sql: usize,
    /// Queries that did not bind, or left a slot unbound (not compared).
    skipped: usize,
    /// Rows the compared queries returned, summed.
    rows: usize,
    mismatches: Vec<String>,
}

impl Tally {
    fn check(&mut self, f: &Fixture, pg: &Pg, what: &str, mut q: ordo_sema::Query) {
        if f.resolve_blind(&mut q, &[]).is_err() {
            self.skipped += 1;
            return;
        }
        let space = f.plan_space();
        let (Ok(legacy), Ok(proven)) = (
            ordo_plan::plan_legacy_equality(&q, &space),
            ordo_plan::plan_proven_equality(&q, &space),
        ) else {
            self.skipped += 1;
            return;
        };
        if !legacy.params.is_empty() {
            self.skipped += 1;
            return;
        }
        if legacy.sql == proven.sql {
            self.same_sql += 1;
            return;
        }
        // A guard's own query has no ORDER BY, so its rows come in no
        // promised order: compare those as sets. Everything else is in a
        // total order and must agree row for row.
        let ordered = legacy.sql.contains("\nORDER BY ");
        let run = |sql: &str| {
            pg.query(sql).map(|mut rows| {
                if !ordered {
                    rows.sort();
                }
                rows
            })
        };
        let a = run(&legacy.sql);
        let b = run(&proven.sql);
        match (&a, &b) {
            (Ok(x), Ok(y)) if x == y => {
                self.compared += 1;
                self.rows += x.len();
            }
            (Err(_), Err(_)) => self.skipped += 1,
            _ => self.mismatches.push(format!(
                "{what}\nnull-safe:\n{}\n→ {a:?}\nproven:\n{}\n→ {b:?}",
                legacy.sql, proven.sql
            )),
        }
    }
}

/// Run every read op, every extra case and every guard of `f` both ways.
fn parity(name: &str, f: &Fixture, pg: &Pg, cases: &[String]) -> Tally {
    let nulled = null_saturate(f, pg);
    let members = ids(pg, "members");
    let mut all_ids: Vec<String> = Vec::new();
    for e in &f.schema.entities {
        all_ids.extend(ids(pg, &e.name).into_iter().step_by(5));
    }
    all_ids.push("missing_id".into());
    let callers: Vec<Option<&str>> = std::iter::once(None)
        .chain(members.iter().take(6).map(|m| Some(m.as_str())))
        .collect();

    let mut t = Tally::default();
    let mut names: Vec<&String> = f.registry.names().collect();
    names.sort();
    let mut calls: Vec<String> = cases.to_vec();
    for n in names {
        let op = f.registry.get(n).expect("listed");
        if !matches!(op.body, OpBody::Read(_)) {
            continue;
        }
        if op.params.is_empty() {
            calls.push(n.clone());
            continue;
        }
        for k in 0..3 {
            let args: Option<Vec<String>> = op
                .params
                .iter()
                .enumerate()
                .map(|(j, p)| {
                    literal(p.ty, k + j, &all_ids).map(|v| format!("${}: {v}", p.name.name))
                })
                .collect();
            match args {
                Some(args) => calls.push(format!("{n}({})", args.join(", "))),
                None => t.skipped += 1,
            }
        }
    }
    for call in &calls {
        let Ok(pl) = ordo_syntax::parse_pipeline(call) else {
            t.skipped += 1;
            continue;
        };
        for caller in &callers {
            match ordo_sema::bind_as(
                &f.schema,
                &f.registry,
                &pl,
                &[],
                *caller,
                ordo_sema::Clock::CORPUS,
            ) {
                Ok(q) => t.check(f, pg, &format!("{call} as {caller:?}"), q),
                Err(_) => t.skipped += 1,
            }
        }
    }
    for e in &f.schema.entities {
        for write in [false, true] {
            for caller in &callers {
                if let Ok(Some(q)) =
                    ordo_sema::guard_filter_query(&f.schema, &e.name, write, *caller)
                {
                    let verb = if write { "write" } else { "read" };
                    t.check(f, pg, &format!("{} {verb} guard as {caller:?}", e.name), q);
                }
            }
        }
    }
    eprintln!(
        "{name}: {} queries compared both ways ({} rows), {} unchanged by the rule, \
         {} not run; {} callers; optional columns holding nulls: {}",
        t.compared,
        t.rows,
        t.same_sql,
        t.skipped,
        callers.len(),
        nulled.join(", ")
    );
    t
}

fn assert_parity(t: &Tally, at_least: usize) {
    assert!(
        t.mismatches.is_empty(),
        "{} queries answered differently:\n\n{}",
        t.mismatches.len(),
        t.mismatches.join("\n\n")
    );
    assert!(
        t.compared >= at_least,
        "only {} queries compared, so the test proves little: {t:?}",
        t.compared
    );
}

#[test]
fn the_market_corpus_answers_the_same_both_ways() {
    let f = Fixture::load().expect("market compiles");
    let pg = Pg::scoped("ordo_eq_parity_market");
    f.reset(&pg).expect("seeded");
    let cases: Vec<String> = ordo_conformance::covered_case_sources()
        .into_iter()
        .map(|(_, s)| s.to_string())
        .collect();
    let t = parity("market", &f, &pg, &cases);
    assert_parity(&t, 100);
}

#[test]
fn the_wide_corpus_answers_the_same_both_ways() {
    let f = Fixture::load_wide().expect("wide compiles");
    let pg = Pg::scoped("ordo_eq_parity_wide");
    f.reset(&pg).expect("seeded");
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("../corpus/ordo.json")).expect("ordo.json");
    let cases: Vec<String> = corpus["cases"]
        .as_object()
        .expect("cases")
        .values()
        .filter_map(|c| c["ordo"].as_str().map(str::to_string))
        .collect();
    assert!(cases.len() > 20, "the wide corpus cases were found");
    // The shapes the rule treats differently, each against optional sides:
    // both sides possibly null, one side never null, under `not`, `!=`, a
    // hop through an optional ref, a hop through a required one, and a
    // comparison inside `match none`.
    let mut cases = cases;
    cases.extend(
        [
            "people(nickname == name)[name]\nsort name",
            "people(nickname == externalId)[name]\nsort name",
            "people(nickname != name)[name]\nsort name",
            "people(nickname != externalId)[name]\nsort name",
            "people(not (nickname == name))[name]\nsort name",
            "people(not (nickname == externalId))[name]\nsort name",
            "people(not (age == 3))[name]\nsort name",
            "people(age != 3)[name]\nsort name",
            "people(team.name == name)[name]\nsort name",
            "people(team.name != \"v1\")[name]\nsort name",
            "people(not (team.name == \"v1\"))[name]\nsort name",
            "people(org.name == \"v1\")[name]\nsort name",
            "people(not (org.name == name))[name]\nsort name",
            "people(team.org == org)[name]\nsort name",
            "people(not (team.org == org))[name]\nsort name",
            "people(lower(name) == lower(nickname))[name]\nsort name",
            "people(name + \"x\" == nickname + \"x\")[name]\nsort name",
            "people(coalesce(nickname, name) == name)[name]\nsort name",
            "projects(sponsor == lead)[name]\nsort name",
            "projects(sponsor != lead)[name]\nsort name",
            "projects(not (sponsor == lead))[name]\nsort name",
            "projects(sponsor.team == lead.team)[name]\nsort name",
        ]
        .map(str::to_string),
    );
    let t = parity("wide", &f, &pg, &cases);
    assert_parity(&t, 30);
}

#[test]
fn notes_v2_answers_the_same_both_ways() {
    let f = Fixture::from_source(NOTES_V2, "").expect("notes v2 compiles");
    let pg = Pg::scoped("ordo_eq_parity_notes");
    f.reset(&pg).expect("schema");
    let t = parity("notes v2", &f, &pg, &[]);
    assert_parity(&t, 30);
}
