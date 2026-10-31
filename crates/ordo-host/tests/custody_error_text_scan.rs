//! Key custody and the security-event drain keep a typed error with its
//! cause and are classified by type, never by words (AGENTS.md rule 9,
//! owner 2026-10-23; `docs/audit-service-interfaces.md`, fix order step 5b).
//!
//! `ordo_crypto::KeyError` and `ordo_secevent::StreamError` hold a
//! `StoreFault` (class and cause). `class()`, `is_busy()` and
//! `is_destroyed()` answer from the type. This scan reads the modules that
//! build or consume them and fails on a text-typed store error, a
//! discarded cause, or a decision made by reading words.

use std::path::Path;

/// Built so this file never holds the needles it looks for.
fn needle(parts: &[&str]) -> String {
    parts.concat()
}

fn needles() -> Vec<String> {
    vec![
        // A String-carrying store variant, built or declared.
        needle(&["KeyError::Store(", "format!"]),
        needle(&["KeyError::Store(\""]),
        needle(&["KeyError::Store(", "e.to_string"]),
        needle(&["StreamError::Store(", "format!"]),
        needle(&["StreamError::Store(\""]),
        needle(&["StreamError::Store(", "e.to_string"]),
        needle(&["    Store(", "String)"]),
        needle(&["StoreBusy", "("]),
        // A cause thrown away on the way to a custody or stream error.
        needle(&["map_err(|_| KeyError", "::Store"]),
        needle(&["map_err(|_| unav", "ailable(\"the tenant's key"]),
        needle(&["map_err(|_| \"the tenant's key", " custody"]),
        needle(&["map_err(|_| \"the client secret could not be", " opened"]),
        // A decision read from the words.
        needle(&["$keycustody", "\") &&"]),
        needle(&["to_string().contains(\"$key", "custody"]),
    ]
}

/// The modules this step owns, relative to `crates/`.
const MODULES: &[&str] = &[
    "ordo-crypto/src/custody.rs",
    "ordo-crypto/src/master.rs",
    "ordo-exec/src/keystore.rs",
    "ordo-exec/src/sealed.rs",
    "ordo-exec/src/secevent.rs",
    "ordo-secevent/src/store.rs",
    "ordo-host/src/secdrain.rs",
    "ordo-host/src/tenant_providers.rs",
];

fn read(rel: &str) -> String {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    std::fs::read_to_string(crates.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The source with its unit-test module cut off, and comments dropped.
fn code_lines(body: &str) -> Vec<&str> {
    let end = body.find("#[cfg(test)]\nmod ").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect()
}

#[test]
fn no_custody_or_drain_error_is_text_typed() {
    let needles = needles();
    let mut wrong = Vec::new();
    for rel in MODULES {
        let body = read(rel);
        for line in code_lines(&body) {
            if needles.iter().any(|n| line.contains(n.as_str())) {
                wrong.push(format!("{rel}: {}", line.trim()));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a custody or drain error built from text, or a cause discarded; use \
         `KeyError::caused_by` / `broken` / `refused` or `StoreFault::caused_by`:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn the_typed_errors_keep_their_cause_and_can_grow() {
    let key = read("ordo-crypto/src/custody.rs");
    assert!(key.contains("impl std::error::Error for KeyError"));
    assert!(key.contains("#[non_exhaustive]\npub enum KeyError"));
    assert!(key.contains("pub fn class(&self) -> FaultClass"));
    assert!(key.contains("pub fn is_destroyed(&self)"));
    let stream = read("ordo-secevent/src/store.rs");
    assert!(stream.contains("#[non_exhaustive]\npub enum StreamError"));
    assert!(stream.contains("Store(StoreFault)"));
    assert!(stream.contains("fn source(&self)"));
    let drain = read("ordo-host/src/secdrain.rs");
    assert!(drain.contains("pub error: StoreFault"));
}

/// The scan sees what it is for, and ignores tests and prose.
#[test]
fn the_scan_sees_a_text_match() {
    let needles = needles();
    let hits = |body: &str| {
        code_lines(body)
            .iter()
            .filter(|l| needles.iter().any(|n| l.contains(n.as_str())))
            .count()
    };
    let built = needle(&["Err(KeyError::Store(", "format!(\"x\"))"]);
    let dropped = needle(&["x.map_err(|_| KeyError", "::Store(y))"]);
    assert_eq!(hits(&built), 1);
    assert_eq!(hits(&dropped), 1);
    assert_eq!(hits(&format!("// {built}")), 0);
    assert_eq!(
        hits(&format!(
            "fn f() {{}}\n#[cfg(test)]\nmod tests {{\n{built}\n}}"
        )),
        0
    );
}
