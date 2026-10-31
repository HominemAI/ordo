//! The device's local refinement, proved against the server.
//!
//! `clients/ordo-sdk/src/replica/refine.ts` answers filters and sorts over
//! rows already on the device, and it must give the answer the server would.
//! The case table is one JSON file, `fixtures/refine-parity.json`, read by
//! this test and by `clients/ordo-sdk/test/refine-parity.test.ts`. Each case
//! is a refinement (`where` and `sort`, in the SDK's `LocalQuery` shape) and
//! the ids the server answers, in order.
//!
//! This half turns each case into an Ordo query, runs it through the real
//! read path on Postgres, and requires the recorded ids. So the expected
//! answers the TypeScript half checks are proved on the server and not just
//! written down. Two tests keep the table honest: one runs every case, and
//! one requires that the table still covers the corners that once differed
//! (null logic, `in` with null, code point order, NFC, null placement).
//!
//! Rules under test, all from the server:
//!
//! - Two-valued null logic (`definition-language.md` §2): `null == x` is
//!   `IS NOT DISTINCT FROM`, `!=` is `IS DISTINCT FROM`, ordering with a null
//!   operand is false. `in` is a disjunction of equalities, so a null in the
//!   list matches a null value.
//! - Text orders by code point (`COLLATE "C"`), over NFC text. A filter value
//!   is normalized before it is compared (`ordo_syntax::text`).
//! - `NULLS LAST` in both directions, then the id as the tiebreak.
//! - Money and `decimal` compare as exact numbers (27ak): `m` is a
//!   `money(2)` column seeded in minor units from the wire's `"2.50"`, and
//!   `r` a `decimal(4)`. A case's value is the wire's decimal string, written
//!   into the Ordo condition as the literal it spells, so the binder's
//!   conversion is under test too.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::{Fixture, Pg};
use serde_json::Value;

const TABLE: &str = include_str!("../fixtures/refine-parity.json");

const SCHEMA: &str =
    "entity probes {\n  n: int?\n  t: text?\n  b: bool?\n  m: money(2)?\n  r: decimal(4)?\n}\n";

fn table() -> Value {
    serde_json::from_str(TABLE).expect("refine-parity.json parses")
}

fn sql_text(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        other => panic!("not a cell: {other}"),
    }
}

/// The seed: every row of the table, in the order given.
fn seed() -> String {
    let rows: Vec<String> = table()["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| {
            // `m` holds minor units: the wire's "2.50" is 250 in the column.
            let m = match r["m"].as_str() {
                Some(s) => ordo_syntax::numeric::minor_units(s, 2)
                    .expect("a money(2) value")
                    .to_string(),
                None => "NULL".into(),
            };
            let d = r["r"].as_str().unwrap_or("NULL");
            format!(
                "  (@SPACE@, {}, {}, {}, {}, {m}, {d})",
                sql_text(&r["id"]),
                sql_text(&r["n"]),
                sql_text(&r["t"]),
                sql_text(&r["b"])
            )
        })
        .collect();
    format!(
        "INSERT INTO \"probes\" (\"$space\", \"id\", \"n\", \"t\", \"b\", \"m\", \"r\") VALUES\n{};\n",
        rows.join(",\n")
    )
}

/// A literal in Ordo source. The lexer normalizes a string to NFC, which is
/// the same step the door applies to a caller's value.
fn literal(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        other => panic!("not a literal: {other}"),
    }
}

/// A decimal field's value is the wire's decimal string; in source it is
/// the numeric literal that string spells.
fn literal_for(field: &str, v: &Value) -> String {
    let decimals = table()["decimals"].clone();
    let exact = decimals
        .as_array()
        .is_some_and(|d| d.iter().any(|f| f == field));
    match v {
        Value::String(s) if exact => s.clone(),
        other => literal(other),
    }
}

fn condition(c: &Value) -> String {
    let field = c["field"].as_str().expect("field");
    let op = c["op"].as_str().expect("op");
    let value = &c["value"];
    let literal = |v: &Value| literal_for(field, v);
    match op {
        "is_null" => format!("{field} == null"),
        "eq" => format!("{field} == {}", literal(value)),
        "ne" => format!("{field} != {}", literal(value)),
        "lt" => format!("{field} < {}", literal(value)),
        "le" => format!("{field} <= {}", literal(value)),
        "gt" => format!("{field} > {}", literal(value)),
        "ge" => format!("{field} >= {}", literal(value)),
        "in" => {
            let members: Vec<String> = value
                .as_array()
                .expect("in takes an array")
                .iter()
                .map(literal)
                .collect();
            format!("{field} in ({})", members.join(", "))
        }
        other => panic!("unknown op {other}"),
    }
}

/// The case as Ordo source: filter, project the id, sort.
fn source(case: &Value) -> String {
    let where_: Vec<String> = case["where"]
        .as_array()
        .expect("where")
        .iter()
        .map(condition)
        .collect();
    let mut src = if where_.is_empty() {
        "probes[id]".to_string()
    } else {
        format!("probes({})[id]", where_.join(", "))
    };
    let keys: Vec<String> = case["sort"]
        .as_array()
        .expect("sort")
        .iter()
        .map(|k| {
            let dash = if k["direction"] == "desc" { "-" } else { "" };
            format!("{dash}{}", k["field"].as_str().expect("field"))
        })
        .collect();
    if !keys.is_empty() {
        src.push_str(&format!("\nsort {}", keys.join(", ")));
    }
    src
}

fn seeded() -> (Fixture, Pg) {
    let seed: &'static str = Box::leak(seed().into_boxed_str());
    let f = Fixture::from_source(SCHEMA, seed).expect("the probe schema compiles");
    let pg = Pg::named("ordo_refine_parity");
    f.reset_once(&pg).expect("seeded");
    (f, pg)
}

#[test]
fn the_server_gives_every_recorded_answer() {
    let (f, pg) = seeded();
    let mut failures = Vec::new();
    let t = table();
    let cases = t["cases"].as_array().expect("cases");
    for case in cases {
        let src = source(case);
        let got: Vec<String> = match f.run_case(&pg, &src) {
            Ok(v) => v
                .as_array()
                .expect("rows")
                .iter()
                .map(|r| r["id"].as_str().unwrap_or_default().to_string())
                .collect(),
            Err(e) => {
                failures.push(format!("{}\n  {src}\n  error: {e}", case["name"]));
                continue;
            }
        };
        let want: Vec<String> = case["expect"]
            .as_array()
            .expect("expect")
            .iter()
            .map(|v| v.as_str().expect("id").to_string())
            .collect();
        if got != want {
            failures.push(format!(
                "{}\n  {}\n  server: {got:?}\n  table:  {want:?}",
                case["name"],
                src.replace('\n', " | ")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from the server:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// The table is only a guard while it keeps reaching the corners. Fail if a
/// case is dropped and one of them goes uncovered.
#[test]
fn the_table_still_reaches_the_corners() {
    let t = table();
    let cases = t["cases"].as_array().expect("cases");
    let has = |pred: &dyn Fn(&Value) -> bool| cases.iter().any(pred);
    let cond = |c: &Value, op: &str, field: &str| {
        c["where"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["op"] == op && w["field"] == field)
    };
    assert!(has(&|c| cond(c, "ne", "n")), "ne on a nullable int");
    assert!(has(&|c| cond(c, "ne", "t")), "ne on nullable text");
    assert!(has(&|c| cond(c, "ne", "b")), "ne on a nullable bool");
    assert!(has(&|c| cond(c, "is_null", "n")), "is_null");
    assert!(
        has(&|c| c["where"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["op"] == "in" && w["value"].as_array().unwrap().contains(&Value::Null))),
        "in with a null member"
    );
    assert!(
        has(&|c| c["where"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["op"] == "in" && !w["value"].as_array().unwrap().contains(&Value::Null))),
        "in without a null member"
    );
    assert!(
        has(&|c| c["where"].as_array().unwrap().iter().any(|w| {
            matches!(w["op"].as_str(), Some("lt" | "gt" | "le" | "ge"))
                && w["value"]
                    .as_str()
                    .is_some_and(|s| s.chars().any(|ch| (ch as u32) > 0xFFFF))
        })),
        "an ordering test against a character outside the basic plane"
    );
    assert!(
        has(&|c| c["where"].as_array().unwrap().iter().any(|w| {
            matches!(w["op"].as_str(), Some("lt" | "gt" | "le" | "ge"))
                && w["value"]
                    .as_str()
                    .is_some_and(|s| s.chars().any(|ch| (0xE000..=0xFFFF).contains(&(ch as u32))))
        })),
        "an ordering test against a character between U+E000 and U+FFFF"
    );
    assert!(
        has(&|c| c["where"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| { w["value"].as_str().is_some_and(|s| s.contains('\u{0301}')) })),
        "a decomposed filter value, which the server normalizes"
    );
    let sorted = |dir: &str, field: &str| {
        has(&|c| {
            c["sort"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k["direction"] == dir && k["field"] == field)
        })
    };
    assert!(
        sorted("asc", "n") && sorted("desc", "n"),
        "int sort both ways"
    );
    // 27ak: exact decimals compare and sort as numbers on both sides.
    assert!(has(&|c| cond(c, "eq", "m")), "money equality by value");
    assert!(has(&|c| cond(c, "gt", "m")), "money ordering");
    assert!(has(&|c| cond(c, "eq", "r")), "decimal equality by value");
    assert!(
        sorted("asc", "m") && sorted("desc", "m") && sorted("desc", "r"),
        "exact decimal sorts"
    );
    assert!(
        sorted("asc", "t") && sorted("desc", "t"),
        "text sort both ways"
    );
}
