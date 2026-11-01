//! The wire inventory is closed in **two** directions, and until now it was
//! checked in neither.
//!
//! `api-engine-design.md` carries the table a caller reads; `ErrorCode::ALL`
//! carries the set a deployment can actually emit. Nothing compared them, and
//! the cost of that showed up as checklist 9w: `arithmetic_error` had a
//! producer, a status, an end-to-end test and a place on the wire, and did not
//! appear in the document's table at all. An inventory that does not name a
//! class the engine emits is not closed — it is a list.
//!
//! So this asserts both directions:
//!
//! - **Code without document** is the 9w failure. A class a caller can receive
//!   and cannot look up is undocumented API surface.
//! - **Document without code** is the opposite failure, and is *sometimes
//!   legitimate* — the table deliberately carries classes that are specified
//!   ahead of the engine. Those are allow-listed BY NAME with the reason the
//!   table itself gives, so "specified, not yet built" stays distinguishable
//!   from "quietly dropped".
//!
//! The status column is compared too. A document that names the right class
//! with the wrong code is a subtler version of the same bug.

use ordo_api::error::ErrorCode;

const DOC: &str = include_str!("../../../docs/api-engine-design.md");

/// Classes the document specifies that `ordo-api` does not yet implement.
/// Each is marked as such in the table's own prose; this list is the
/// machine-readable half of that sentence, and it may only ever shrink.
///
/// **It is empty**, and that is the point of it: `refinement_unbounded` was
/// its sole entry — documented in the wire inventory, with the request-time
/// cost gate that raises it unbuilt — and the stale-entry assertion below is
/// what made "it is built now" impossible to forget. The class has a
/// producer ([`ordo_api::refine::check_cost`]), so the name came off. A
/// class specified ahead of its engine may be added back; a class with no
/// producer at all is the recurring defect this whole file exists to catch.
const SPECIFIED_NOT_BUILT: &[&str] = &[];

/// Every `| `class` | status | …` row of the inventory table.
fn documented() -> Vec<(String, u16)> {
    let mut out = Vec::new();
    for line in DOC.lines() {
        let line = line.trim();
        if !line.starts_with("| `") {
            continue;
        }
        let mut cells = line.split('|').map(str::trim);
        cells.next(); // the empty cell before the first pipe
        let (Some(name), Some(status)) = (cells.next(), cells.next()) else {
            continue;
        };
        let name = name.trim_matches('`');
        // The table's own status column is a bare number; a row whose second
        // cell is not one belongs to a different table in the same document.
        let Ok(status) = status.parse::<u16>() else {
            continue;
        };
        out.push((name.to_string(), status));
    }
    assert!(
        out.len() > 20,
        "the inventory table did not parse — {} rows found, so this test would \
         pass vacuously and prove nothing about a document it can no longer read",
        out.len()
    );
    out
}

#[test]
fn every_class_the_engine_can_emit_is_in_the_document() {
    let doc = documented();
    let missing: Vec<&str> = ErrorCode::ALL
        .iter()
        .map(|c| c.as_str())
        .filter(|c| !doc.iter().any(|(n, _)| n == c))
        .collect();
    assert!(
        missing.is_empty(),
        "these classes reach callers and appear in no row of \
         `api-engine-design.md`'s inventory: {missing:?}"
    );
}

#[test]
fn every_documented_class_is_one_the_engine_has() {
    let doc = documented();
    let unbuilt: Vec<&str> = doc
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| ErrorCode::from_class(n).is_none())
        .filter(|n| !SPECIFIED_NOT_BUILT.contains(n))
        .collect();
    assert!(
        unbuilt.is_empty(),
        "the document promises classes `ordo-api` cannot produce, and they are \
         not on the specified-not-built list: {unbuilt:?}"
    );

    // …and the allow-list may not outlive what it excuses. A name that has
    // since been built must leave the list, or it hides the next omission.
    let stale: Vec<&&str> = SPECIFIED_NOT_BUILT
        .iter()
        .filter(|n| ErrorCode::from_class(n).is_some())
        .collect();
    assert!(
        stale.is_empty(),
        "these are built now and must come off SPECIFIED_NOT_BUILT: {stale:?}"
    );
}

#[test]
fn the_document_and_the_engine_agree_on_every_status() {
    let disagree: Vec<String> = documented()
        .into_iter()
        .filter_map(|(name, doc_status)| {
            let code = ErrorCode::from_class(&name)?;
            let real = code.status().as_u16();
            (real != doc_status).then(|| format!("`{name}`: document {doc_status}, engine {real}"))
        })
        .collect();
    assert!(
        disagree.is_empty(),
        "a class documented with the wrong status is a quieter version of an \
         undocumented class: {disagree:?}"
    );
}

// ── the parameter-type inventory, in the same two directions ─────────────

/// `ParamTy`'s wire table had exactly the failure this file was written
/// for, in a second inventory nobody was checking.
///
/// `$t: datetime` parses, has a test in `declparse.rs`, and had **no row**
/// in §5's table and **no member** in `ParamTy` — so `ordo_host::param_ty`'s
/// wildcard arm bound it at the door as unbounded text with no ISO-8601
/// validation. That is code-without-document and document-without-code at
/// once, in the table whose whole job is to be the closed inventory a
/// caller reads.
///
/// So: the same two directions, over the same document.
fn documented_param_types() -> Vec<String> {
    let mut out = Vec::new();
    let mut in_table = false;
    for line in DOC.lines() {
        if line.contains("| `ParamTy` | Wire shape |") {
            in_table = true;
            continue;
        }
        if in_table {
            let t = line.trim();
            if !t.starts_with('|') {
                break;
            }
            if let Some(name) = t
                .trim_start_matches('|')
                .trim()
                .strip_prefix('`')
                .and_then(|r| r.split('`').next())
            {
                if name != "---" && !name.is_empty() {
                    // `text(n)` is spelled with its bound in the document,
                    // because that is what an author writes; the inventory
                    // member is `text`.
                    out.push(name.split('(').next().unwrap_or(name).to_string());
                }
            }
        }
    }
    out
}

/// Every member the door can bind is named in the document, and every row
/// in the document is a member the door can bind.
#[test]
fn every_parameter_type_the_engine_binds_is_documented() {
    use ordo_api::spec::ParamTy;
    let documented = documented_param_types();
    assert!(
        documented.len() >= 8,
        "the §5 table scan found {documented:?}, which is too few — the table's \
         shape has probably changed and this test would pass vacuously"
    );

    // The inventory, written out. `ParamTy` has a payload variant
    // (`Text { max_len }`) so there is no derivable `ALL`; the exhaustive
    // match below is what keeps this list honest — a new member fails to
    // compile here until somebody gives it a name, and then fails this
    // assertion until somebody gives it a row.
    let all = [
        ParamTy::Int,
        ParamTy::Money { places: 2 },
        ParamTy::Decimal { places: 4 },
        ParamTy::Text { max_len: None },
        ParamTy::Date,
        ParamTy::Datetime,
        ParamTy::Bool,
        ParamTy::Id,
        ParamTy::Duration,
        ParamTy::Uri,
        ParamTy::Json,
        ParamTy::Blob,
    ];
    for ty in &all {
        let name = match ty {
            ParamTy::Int => "int",
            ParamTy::Money { .. } => "money",
            ParamTy::Decimal { .. } => "decimal",
            ParamTy::Text { .. } => "text",
            ParamTy::Date => "date",
            ParamTy::Datetime => "datetime",
            ParamTy::Bool => "bool",
            ParamTy::Id => "id",
            ParamTy::Duration => "duration",
            ParamTy::Uri => "uri",
            ParamTy::Json => "json",
            ParamTy::Blob => "blob",
        };
        assert!(
            documented.contains(&name.to_string()),
            "`{name}` is bindable at the door and has no row in §5's wire table — \
             a parameter type a caller can send and cannot look up is undocumented \
             API surface: {documented:?}"
        );
    }
    assert_eq!(
        all.len(),
        documented.len(),
        "the document names {} parameter types and the engine binds {} — a row \
         with no member is a promise nothing keeps: {documented:?}",
        documented.len(),
        all.len()
    );
}
