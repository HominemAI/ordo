//! Every refusal class a user can see should be findable in the spec.
//!
//! The platform's discipline is that every refusal has a **named class from
//! a closed set**. The wire half of that is enforced —
//! `ordo-api/tests/inventory_is_closed.rs` compares `ErrorCode` against
//! `api-engine-design.md` in both directions, and found two real defects the
//! day it was written. The **compile** half had no such check, and it turns
//! out the set is not closed in practice: classes were minted where they were
//! needed, which is how you get two names for one mistake and a conformance
//! case asserting a class no document defines.
//!
//! This is deliberately a **census with a ratchet**, not a pass/fail on the
//! whole set. Closing the gap needs a ruling — which document owns
//! schema-build classes, and whether `composition-conformance.md` §0 is meant
//! to be the whole compile inventory or only the composition part — and that
//! ruling is not a test's to take. What a test *can* do is stop the number
//! growing while the question is open, which is the failure mode that
//! produced 24 of these in the first place.
//!
//! The count may only go **down**. A new class must either be documented or
//! come with a deliberate decision to add to this list, and the second is
//! something a reviewer sees.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Crates whose errors an author of an `.ordo` program can actually see.
/// `ordo-identity` and the rest are excluded because their `code:` fields
/// are not this inventory (and matching them produces noise like `authcode`).
const AUTHOR_FACING: &[&str] = &["ordo-syntax", "ordo-schema", "ordo-sema", "ordo-plan"];

/// Classes produced by code and named in no specification document.
///
/// **This list may only shrink.** Each entry is a class an author can be
/// shown with nothing in the documents to explain it. They are recorded
/// rather than fixed because the fix is a ruling (see the checklist), and a
/// silent gap is worse than a listed one.
const UNDOCUMENTED: &[&str] = &[
    // Emptied by the 9x triage (2026-09-03): twenty classes documented in
    // `composition-conformance.md` §0 under schema-build and binding
    // headings, two named as internal vocabulary, and `type_error` collapsed
    // into `argument_type_mismatch` — one mistake, one class.
    //
    // An empty list is the point. A new undocumented class now fails the
    // build outright rather than joining a crowd.
];

fn repo_root() -> PathBuf {
    // `crates/ordo-conformance` → repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for crate_name in AUTHOR_FACING {
        let src = root.join("crates").join(crate_name).join("src");
        collect(&src, &mut out);
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every class string the author-facing crates raise.
///
/// Read out of the source rather than from a registry constant on purpose:
/// a constant is a second list that can drift from the `err(...)` calls it
/// claims to describe, and drift is the thing under test.
fn produced(root: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in rust_sources(root) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let krate = path
            .components()
            .rev()
            .nth(1)
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        for cls in scan(&text) {
            out.entry(cls).or_default().insert(krate.clone());
        }
    }
    out
}

/// `err("class"` and `code: "class"`, across line breaks.
fn scan(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["err(", "code:"] {
        let mut from = 0;
        while let Some(i) = text[from..].find(marker) {
            let start = from + i + marker.len();
            let rest = &text[start..];
            let trimmed = rest.trim_start();
            if let Some(body) = trimmed.strip_prefix('"') {
                if let Some(end) = body.find('"') {
                    let cls = &body[..end];
                    if !cls.is_empty()
                        && cls
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                        && cls.contains('_')
                    {
                        out.push(cls.to_string());
                    }
                }
            }
            from = start;
        }
    }
    out
}

fn spec_text(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(root.join("docs"))
        .expect("docs/")
        .flatten()
    {
        let p = e.path();
        // A checklist records work and an audit records findings; neither
        // defines a contract. A class that appears only in one of those is
        // undocumented for this purpose, and counting them would let a
        // document that MERELY LISTS the gap close it — which this test
        // demonstrated on itself the moment an audit naming all 24 landed.
        let name = p.file_name().map(|f| f.to_string_lossy().into_owned());
        let is_record = name.as_deref().is_some_and(|n| {
            n == "roadmap-checklist.md" || n.starts_with("audit-") || n.starts_with("critique-")
        });
        if p.extension().is_some_and(|x| x == "md") && !is_record {
            out.push(std::fs::read_to_string(&p).unwrap_or_default());
        }
    }
    out
}

#[test]
fn no_new_refusal_class_arrives_undocumented() {
    let root = repo_root();
    let produced = produced(&root);
    let docs = spec_text(&root);
    assert!(
        produced.len() > 30,
        "the scan found only {} classes, so this test would pass vacuously — \
         the `err(...)` shape it matches has probably changed",
        produced.len()
    );

    let undocumented: Vec<&String> = produced
        .keys()
        .filter(|c| !docs.iter().any(|d| d.contains(c.as_str())))
        .collect();

    let allowed: BTreeSet<&str> = UNDOCUMENTED.iter().copied().collect();
    let fresh: Vec<&&String> = undocumented
        .iter()
        .filter(|c| !allowed.contains(c.as_str()))
        .collect();
    assert!(
        fresh.is_empty(),
        "these refusal classes are raised by the compiler and appear in no \
         specification document: {fresh:?}\n\
         Either document the class, or add it to UNDOCUMENTED with a reason — \
         which is a decision a reviewer should see rather than a silence."
    );

    // The ratchet. A class that gets documented must leave the list, or the
    // list stops meaning anything and hides the next one.
    let stale: Vec<&&str> = allowed
        .iter()
        .filter(|c| docs.iter().any(|d| d.contains(**c)))
        .collect();
    assert!(
        stale.is_empty(),
        "these are documented now and must come off UNDOCUMENTED: {stale:?}"
    );
}

/// The composition inventory is a table of classes; every one of them should
/// have a producer, or the document is promising a refusal nothing raises.
///
/// The wire inventory's equivalent check found two real defects. This one is
/// narrower — it only reports, because `composition-conformance.md` §0 also
/// lists classes raised at the door (`bound_violation`) and by the parser,
/// which are not in the crates scanned here.
#[test]
fn the_composition_inventory_is_reported_against_its_producers() {
    let root = repo_root();
    let produced = produced(&root);
    let doc = std::fs::read_to_string(root.join("docs/composition-conformance.md")).expect("doc");

    let mut documented = Vec::new();
    for line in doc.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("| `") {
            if let Some(end) = rest.find('`') {
                let cls = &rest[..end];
                if cls.contains('_') && !cls.contains(' ') {
                    documented.push(cls.to_string());
                }
            }
        }
    }
    assert!(
        documented.len() > 15,
        "the §0 table did not parse ({} rows) — this would pass vacuously",
        documented.len()
    );

    // Classes the document names that these crates do not raise. Not an
    // assertion: the parser and the door raise some of them, and this test
    // cannot see those. It exists so the list is visible in a run rather
    // than discovered by an author hitting nothing.
    let missing: Vec<&String> = documented
        .iter()
        .filter(|c| !produced.contains_key(*c))
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "note: composition inventory classes with no producer in {AUTHOR_FACING:?}: {missing:?}"
        );
    }
}
