//! Stored bytes are served from one place (board `blob-served-as-claimed-type`).
//!
//! A stored file's type is the uploader's claim. Served as claimed, a file
//! uploaded as `text/html` or `image/svg+xml` runs script on the door's
//! origin, and that script can read a fresh access token from
//! `/auth/refresh`. `ordo_api::blob::stored_bytes` sets the type, the
//! disposition, the sandbox policy and `nosniff` for every response that
//! carries stored bytes.
//!
//! This scan reads the non-test source of `ordo-host` and `ordo-api` and
//! fails on:
//!
//! - bytes fetched from file storage (`in_store(`) anywhere but the one
//!   `BlobPort` implementation;
//! - a file that reads bytes through the port (`port.read(`) and does not
//!   build its response with `stored_bytes(`, or that is not on the list
//!   below;
//! - the headers `stored_bytes` owns, set anywhere else.

use std::path::{Path, PathBuf};

/// Built so this file never holds the needles it looks for.
fn needle(parts: &[&str]) -> String {
    parts.concat()
}

/// The routes that serve stored bytes, by file (relative to `crates/`). A new
/// one is added here only once it builds its response with `stored_bytes`.
const SERVES_STORED_BYTES: &[&str] = &[
    // `GET /blob/{handle}`: one file, ranges included.
    "ordo-api/src/router.rs",
    // `GET /download`: a person's own files, in a zip.
    "ordo-api/src/download.rs",
];

/// The one place that fetches bytes from file storage: the `BlobPort` impl.
const FETCHES_FROM_STORE: &str = "ordo-host/src/blob.rs";

/// The one place the stored-bytes headers are set.
const SETS_THE_HEADERS: &str = "ordo-api/src/blob.rs";

/// Other mentions of those header names that set nothing, with why.
const MENTIONS: &[(&str, &str)] = &[
    // The CORS list of headers a page may read. It sets no header.
    ("ordo-api/src/cors.rs", "Content-Disposition"),
    // The door-wide hardening headers (`cors::harden`): the same `nosniff`
    // on every response, set only where a route set none, so a stored-bytes
    // response keeps what `stored_bytes` gave it.
    ("ordo-api/src/cors.rs", "nosniff"),
    ("ordo-api/src/cors.rs", "X-Content-Type-Options"),
];

fn sources() -> Vec<(String, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![crates.join("ordo-host/src"), crates.join("ordo-api/src")];
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
                let text = std::fs::read_to_string(&p).expect("read");
                out.push((rel, code_of(&text)));
            }
        }
    }
    out.sort();
    assert!(
        out.len() > 100
            && out.iter().any(|(p, _)| p == FETCHES_FROM_STORE)
            && out.iter().any(|(p, _)| p == SETS_THE_HEADERS),
        "the scan is reading the wrong tree"
    );
    out
}

/// The source with its test module and comment lines removed.
fn code_of(text: &str) -> String {
    let end = text
        .find(&needle(&["#[cfg(", "test)]\nmod "]))
        .unwrap_or(text.len());
    text[..end]
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn only_the_blob_port_fetches_from_file_storage() {
    let n = needle(&["in_", "store("]);
    let found: Vec<String> = sources()
        .into_iter()
        .filter(|(f, code)| code.contains(&n) && f != FETCHES_FROM_STORE)
        .map(|(f, _)| f)
        .collect();
    assert!(
        found.is_empty(),
        "bytes fetched from file storage outside the BlobPort: {found:?}. \
         Read them through BlobPort::read, and serve them with ordo_api::blob::stored_bytes."
    );
}

#[test]
fn every_route_serving_stored_bytes_uses_the_one_helper() {
    let read = needle(&["port.", "read("]);
    let helper = needle(&["stored_", "bytes("]);
    let mut readers = Vec::new();
    for (f, code) in sources() {
        if !code.contains(&read) {
            continue;
        }
        readers.push(f.clone());
        assert!(
            code.contains(&helper),
            "{f} reads stored bytes and does not build its response with \
             ordo_api::blob::stored_bytes: a file served as its claimed type runs script \
             on the door's origin"
        );
        assert!(
            SERVES_STORED_BYTES.contains(&f.as_str()),
            "{f} reads stored bytes and is not on SERVES_STORED_BYTES. Check it builds its \
             response with stored_bytes, then add it."
        );
    }
    for f in SERVES_STORED_BYTES {
        assert!(
            readers.iter().any(|r| r == f),
            "{f} is listed but no longer reads stored bytes; take it off the list"
        );
    }
}

#[test]
fn the_stored_bytes_headers_are_set_in_one_place() {
    let names = [
        needle(&["content-", "disposition"]),
        needle(&["content-", "security-policy"]),
        needle(&["x-content-", "type-options"]),
        needle(&["CONTENT_", "DISPOSITION"]),
        needle(&["CONTENT_", "SECURITY_POLICY"]),
        needle(&["X_CONTENT_", "TYPE_OPTIONS"]),
        needle(&["nos", "niff"]),
    ];
    let mut found = Vec::new();
    for (f, code) in sources() {
        if f == SETS_THE_HEADERS {
            continue;
        }
        for line in code.lines() {
            let lower = line.to_ascii_lowercase();
            for n in &names {
                if !lower.contains(&n.to_ascii_lowercase()) {
                    continue;
                }
                let allowed = MENTIONS
                    .iter()
                    .any(|(mf, text)| *mf == f && line.contains(text));
                if !allowed {
                    found.push(format!("{f}: {}", line.trim()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "stored-bytes headers set outside ordo_api::blob::stored_bytes: {found:#?}"
    );
}
