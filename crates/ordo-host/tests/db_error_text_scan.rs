//! A database error is classified by its **type**, never by its words
//! (AGENTS.md rule 9, owner 2026-10-23; `docs/audit-service-interfaces.md`,
//! fix order step 1).
//!
//! `ordo_exec::PoolError` is the database's one typed error: `class()` says
//! busy, retryable, refused or broken, the narrower questions are methods on
//! it (`is_busy`, `is_lock_timeout`, `is_missing_table`,
//! `is_foreign_key_violation`, ...), and the driver error is its `source()`.
//! This scan reads every crate's non-test source and fails on code that
//! decides by matching an error's text instead: the server-busy words, a
//! SQLSTATE spelled in a string, or Postgres's English.
//!
//! The few places still allowed are listed by file with a count and a
//! reason. Each is a place a later fix-order step removes; the count stops
//! a second one joining it.

use std::path::{Path, PathBuf};

/// Built so this file never holds the needles it looks for.
fn needle(parts: &[&str]) -> String {
    parts.concat()
}

fn needles() -> Vec<String> {
    // The helper that reads the busy words.
    let mut out = vec![needle(&["carries_server", "_busy("])];
    // The busy words compared, however the constant is spelled.
    for constant in [
        "SATURATED_MESSAGE",
        "ordo_exec::SATURATED_MESSAGE",
        "SERVER_BUSY_MESSAGE",
        "ordo_api::SERVER_BUSY_MESSAGE",
    ] {
        for op in [
            ".contains(",
            ".starts_with(",
            ".ends_with(",
            ".strip_suffix(",
            "== ",
            "!= ",
        ] {
            out.push(needle(&[op, constant]));
        }
    }
    for method in ["contains", "starts_with", "ends_with"] {
        for words in [
            "server_busy",
            "does not exist",
            "violates foreign key",
            "violates unique",
            "violates check",
            "lock timeout",
            "could not serialize",
            "deadlock",
            "connection refused",
            "connection reset",
            "password authentication",
            "40001",
            "40P01",
            "55P03",
            "55p03",
            "57014",
            "42P01",
            "23503",
            "23505",
            "28P01",
            "28000",
        ] {
            out.push(needle(&[".", method, "(\"", words]));
        }
    }
    out
}

/// File (relative to `crates/`), needle fragment, how many times, and why.
const ALLOWED: &[(&str, &str, usize)] = &[
    // `with_password_hint` adds a remedy to a boot or log line and decides
    // nothing. One of the cases it hints on, the driver's "password
    // missing", has no SQLSTATE to read.
    ("ordo-host/src/runtime.rs", "28P01", 1),
    ("ordo-host/src/runtime.rs", "28000", 1),
];

fn sources() -> Vec<(String, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = std::fs::read_dir(&crates)
        .expect("crates/")
        .map(|e| e.expect("entry").path().join("src"))
        .filter(|p| p.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("dir") {
            let p = entry.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let rel = p
                    .strip_prefix(&crates)
                    .expect("under crates/")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&p).expect("source")));
            }
        }
    }
    out.sort();
    assert!(
        out.len() > 200
            && out.iter().any(|(p, _)| p == "ordo-exec/src/pool_error.rs")
            && out.iter().any(|(p, _)| p == "ordo-api/src/error.rs"),
        "the scan is reading the wrong tree"
    );
    out
}

/// The source with its unit-test module cut off, and comments dropped: a
/// test may assert on what an error says, and prose may quote it.
fn code_lines(body: &str) -> Vec<&str> {
    let end = body.find("#[cfg(test)]\nmod ").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect()
}

#[test]
fn no_database_error_is_classified_by_its_text() {
    let needles = needles();
    let mut wrong = Vec::new();
    let mut used: Vec<(String, String, usize)> = Vec::new();
    for (file, body) in sources() {
        for line in code_lines(&body) {
            for n in &needles {
                if !line.contains(n.as_str()) {
                    continue;
                }
                let allow = ALLOWED
                    .iter()
                    .find(|(f, frag, _)| *f == file && n.contains(frag));
                match allow {
                    Some((f, frag, _)) => {
                        match used
                            .iter_mut()
                            .find(|(uf, ufrag, _)| uf == f && ufrag == frag)
                        {
                            Some(u) => u.2 += 1,
                            None => used.push((f.to_string(), frag.to_string(), 1)),
                        }
                    }
                    None => wrong.push(format!("{file}: {}", line.trim())),
                }
            }
        }
    }
    for (f, frag, n) in &used {
        let (_, _, may) = ALLOWED
            .iter()
            .find(|(af, afrag, _)| af == f && afrag == frag)
            .expect("listed");
        if n > may {
            wrong.push(format!(
                "{f}: `{frag}` {n} times, {may} allowed; decide by the error's type"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a database error classified by its text; use `PoolError::class()` or an \
         `is_*` method on the type instead:\n{}",
        wrong.join("\n")
    );
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
    let busy = needle(&["if ordo_api::carries_server", "_busy(&e.to_string()) {}"]);
    let missing = needle(&[
        "Err(e) if e.to_string().contains(\"does not",
        " exist\") => {}",
    ]);
    let words = needle(&["if why.ends_with(ordo_exec::SATURATED", "_MESSAGE) {}"]);
    assert_eq!(hits(&busy), 1);
    assert_eq!(hits(&missing), 1);
    assert_eq!(hits(&words), 1);
    assert_eq!(hits(&format!("// {missing}")), 0);
    assert_eq!(
        hits(&format!(
            "fn f() {{}}\n#[cfg(test)]\nmod tests {{\n{missing}\n}}"
        )),
        0
    );
}

/// The allowances shrink as the fix order lands: one that is no longer
/// used must be taken off the list, so it cannot be spent again later.
#[test]
fn every_allowance_is_still_used() {
    let needles = needles();
    let sources = sources();
    for (file, frag, _) in ALLOWED {
        let body = sources
            .iter()
            .find(|(p, _)| p == file)
            .map(|(_, b)| b.as_str())
            .unwrap_or_else(|| panic!("{file} is gone; take its allowance off the list"));
        let used = code_lines(body).iter().any(|l| {
            needles
                .iter()
                .any(|n| n.contains(frag) && l.contains(n.as_str()))
        });
        assert!(
            used,
            "{file} no longer matches `{frag}`; take it off the list"
        );
    }
}
