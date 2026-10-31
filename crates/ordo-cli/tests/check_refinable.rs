//! `ordo check` prints each read's refinable fields (refinement R7; ruled
//! 2026-10-05, `refinement-default` ask 4: *"Yes"*).
//!
//! One line per read, beside its class and outputs, grouping the fields a
//! condition may name by what each allows: any operator, equality only, or
//! refused. It is computed from the program alone, by the same function
//! that writes the descriptor's `ops[].refine`, and it never changes the
//! exit code. A regression reads as a missing line, a sealed field printed
//! under `any operator`, a write with a refinable line, or the line and
//! `interface.json` naming different fields.

mod support;

use ordo_cli::{check_sources, interface_json, Sources};
use ordo_conformance::FIXTURE;
use ordo_plan::interface::{refinable_fields, FieldRefine};

const MARKET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../ordo-conformance/fixtures/market.ordo"
);

const CLINIC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/syntax/05-sealed-fields.ordo"
);

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("p.ordo".into(), text.to_string())])
}

/// The block `ordo check` prints for one op: its name line and every
/// indented line under it.
fn block<'a>(report: &'a str, op: &str) -> Vec<&'a str> {
    let mut lines = report
        .lines()
        .skip_while(|l| l.trim_end() != format!("  {op}") && !l.starts_with(&format!("  {op}  ")));
    let head = lines
        .next()
        .unwrap_or_else(|| panic!("no op `{op}` in:\n{report}"));
    std::iter::once(head)
        .chain(lines.take_while(|l| l.starts_with("    ")))
        .collect()
}

fn refinable_of(report: &str, op: &str) -> Option<String> {
    block(report, op)
        .into_iter()
        .find_map(|l| l.trim().strip_prefix("refinable: ").map(str::to_string))
}

/// The market fixture's reads: an open read, a sealed output with no
/// opt-in, and a count that names its list's fields.
#[test]
fn market_reads_print_their_refinable_fields() {
    let report = check_sources(&one(FIXTURE)).expect("checks");
    assert_eq!(
        refinable_of(&report, "memoDigest").as_deref(),
        Some("placed, $id (any operator); memo (refused)"),
        "{report}"
    );
    assert_eq!(
        refinable_of(&report, "orderMemos").as_deref(),
        Some("placed, total, $id (any operator); memo (refused)")
    );
    // A count's conditions validate against its list's manifest, so the
    // line names the list's fields, and never the count's own `n`.
    let list = refinable_of(&report, "std.orders.list").expect("list line");
    assert_eq!(
        refinable_of(&report, "std.orders.count").as_deref(),
        Some(list.as_str())
    );
    assert!(!list.contains(" n,") && !list.starts_with("n,"), "{list}");
    // The line sits under the class and the shape, in that order.
    let b = block(&report, "memoDigest");
    assert!(b[1].trim().starts_with("class "), "{b:?}");
    assert!(b[2].trim().starts_with('['), "{b:?}");
    assert!(b[3].trim().starts_with("refinable: "), "{b:?}");
}

/// Writes and deletes print no refinable line: the door refuses `$refine`
/// on a non-read before the tier is read.
#[test]
fn writes_print_nothing() {
    let report = check_sources(&one(FIXTURE)).expect("checks");
    for op in [
        "std.orders.create",
        "std.orders.update",
        "std.orders.delete",
    ] {
        assert_eq!(
            refinable_of(&report, op),
            None,
            "{op}: {:?}",
            block(&report, op)
        );
    }
}

/// The three groups, from the sealed-fields example: `mrn` is `sealed
/// indexed refinable` (equality only), `email` is indexed with no opt-in,
/// `name` and `diagnosis` have no index (refused). Under a count every
/// sealed field is refused: if you may not read the rows, you may not
/// measure them.
#[test]
fn sealed_fields_are_equality_only_or_refused() {
    let src = Sources::read(std::path::Path::new(CLINIC)).expect("reads");
    let report = check_sources(&src).expect("checks");
    assert_eq!(
        refinable_of(&report, "std.patients.list").as_deref(),
        Some(
            "id, rev, ward, admitted (any operator); mrn (equality only); \
             name, email, diagnosis (refused)"
        ),
        "{report}"
    );
    assert_eq!(
        refinable_of(&report, "std.patients.count").as_deref(),
        Some("id, rev, ward, admitted (any operator); name, email, diagnosis, mrn (refused)")
    );
}

/// A sealed field opted in with no index is refused at the door
/// (`sealed_unsupported`), so the line says `refused`, not `equality only`.
#[test]
fn a_refinable_sealed_field_with_no_index_is_refused() {
    let report = check_sources(&one("domain t @ 1.0.0\n\
         entity notes {\n  title: text\n  body: text sealed refinable\n  std sealed\n}\n"))
    .expect("checks");
    assert_eq!(
        refinable_of(&report, "std.notes.list").as_deref(),
        Some("id, rev, title (any operator); body (refused)"),
        "{report}"
    );
}

/// `ordo check` and `interface.json` cannot disagree: every field the line
/// says a condition may use is in the descriptor's `refine.fields`, every
/// output is on the line, and a read the descriptor calls `none` prints
/// nothing.
#[test]
fn the_line_agrees_with_the_descriptor() {
    for path in [MARKET, CLINIC] {
        let src = Sources::read(std::path::Path::new(path)).expect("reads");
        let iface: serde_json::Value =
            serde_json::from_str(&interface_json(&src).expect("descriptor")).expect("json");
        let art: serde_json::Value =
            serde_json::from_str(&ordo_cli::plan_artifact(&src, None).expect("artifact"))
                .expect("json");
        let ops = art["ops"].as_array().expect("ops");
        let entities = art["entities"].as_array().expect("entities");
        for op in ops {
            let name = op["op"].as_str().expect("name");
            let d = iface["ops"]
                .as_array()
                .expect("ops")
                .iter()
                .find(|o| o["op"] == name)
                .expect("descriptor op");
            let fields = refinable_fields(op, entities, ops);
            if d["refine"]["tier"] == "none" {
                assert!(fields.is_empty(), "{name}");
                continue;
            }
            let listed: Vec<&str> = d["refine"]["fields"]
                .as_array()
                .expect("fields")
                .iter()
                .filter_map(|f| f.as_str())
                .collect();
            for (f, allows) in &fields {
                if *allows != FieldRefine::Refused {
                    assert!(listed.contains(&f.as_str()), "{name}.{f}");
                }
            }
            for f in &listed {
                assert!(fields.iter().any(|(n, _)| n == f), "{name}.{f}");
            }
        }
    }
}

/// The line never changes the exit code: the sealed-fields example has
/// refused fields on several reads and still exits 0.
#[test]
fn the_line_never_fails_the_check() {
    let out = support::ordo()
        .args(["check", CLINIC])
        .output()
        .expect("runs ordo");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("(refused)"), "{stdout}");
    assert!(stdout.contains("mrn (equality only)"), "{stdout}");
}
