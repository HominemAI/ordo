//! A file-storage error is classified by its **type**, never by its words
//! (AGENTS.md rule 9, owner 2026-10-23; `docs/audit-service-interfaces.md`,
//! fix order step 4).
//!
//! `ordo_blob::BlobError` is the one typed error: `class()` says busy,
//! retryable, refused or broken, and the narrower questions are methods on
//! it (`is_transient`, `is_not_found`, `is_forbidden`, `is_bad_input`,
//! `http_status`, `retry_after`). The cause is its `source()`. This scan
//! reads the blob client, the blob door and the bucket code and fails on code
//! that decides by matching an error's text: the display of an error
//! compared, an HTTP status or a store's words searched for in a string.

use std::path::Path;

/// Built so this file never holds the needles it looks for.
fn needle(parts: &[&str]) -> String {
    parts.concat()
}

fn needles() -> Vec<String> {
    let mut out = Vec::new();
    for method in ["contains", "starts_with", "ends_with", "find"] {
        // The error's own display, searched.
        out.push(needle(&[".to_string().", method, "("]));
        out.push(needle(&["{e}\").", method, "("]));
        for words in [
            "HTTP ",
            "SlowDown",
            "Slow Down",
            "server_busy",
            "503",
            "429",
            "408",
            "timed out",
            "connection refused",
            "connection reset",
            "AccessDenied",
            "NoSuchKey",
            "RequestTimeout",
        ] {
            out.push(needle(&[".", method, "(\"", words]));
        }
    }
    out
}

/// Where file-storage errors are made, read and answered.
fn files() -> Vec<(String, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    let mut stack = vec![crates.join("ordo-blob/src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("dir") {
            let p = entry.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    for f in [
        "ordo-host/src/blob.rs",
        "ordo-host/src/bucket_ops.rs",
        "ordo-exec/src/bucket.rs",
        "ordo-exec/src/blobgc.rs",
    ] {
        out.push(crates.join(f));
    }
    let mut named: Vec<(String, String)> = out
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&crates)
                .expect("under crates/")
                .to_string_lossy()
                .replace('\\', "/");
            (rel, std::fs::read_to_string(&p).expect("source"))
        })
        .collect();
    named.sort();
    assert!(
        named.len() >= 10 && named.iter().any(|(p, _)| p == "ordo-blob/src/s3.rs"),
        "the scan is reading the wrong tree"
    );
    named
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
fn no_blob_error_is_classified_by_its_text() {
    let needles = needles();
    let mut wrong = Vec::new();
    for (file, body) in files() {
        for line in code_lines(&body) {
            if needles.iter().any(|n| line.contains(n.as_str())) {
                wrong.push(format!("{file}: {}", line.trim()));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a file-storage error classified by its text; use `BlobError::class()` or an \
         `is_*` method on the type instead:\n{}",
        wrong.join("\n")
    );
}

/// The one place the door turns a class into its answer: the blob door does
/// not ask the blob error for its variant or text outside `from_blob`.
#[test]
fn the_door_reads_the_class_and_never_the_variant() {
    let (_, door) = files()
        .into_iter()
        .find(|(f, _)| f == "ordo-host/src/blob.rs")
        .expect("door");
    let code = code_lines(&door).join("\n");
    for variant in [
        "BlobError::Refused",
        "BlobError::Status",
        "BlobError::Caused",
    ] {
        assert!(
            !code.contains(variant),
            "the door matches a blob error by variant ({variant}); ask its class"
        );
    }
    assert!(code.contains("fn by_class("), "the door's one mapper");
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
    let by_status = needle(&["if e.to_string().contains(\"HTT", "P 503\") {}"]);
    let by_words = needle(&["if body.contains(\"Slow", "Down\") {}"]);
    assert_eq!(hits(&by_status), 1);
    assert_eq!(hits(&by_words), 1);
    assert_eq!(hits(&format!("// {by_status}")), 0);
    assert_eq!(
        hits(&format!(
            "{by_status}\n#[cfg(test)]\nmod tests {{ {by_words} }}"
        )),
        1
    );
}
