//! One edge mapper (fix order step 6, `docs/audit-service-interfaces.md`).
//!
//! `crates/ordo-api/src/error.rs` is the only place an error status and the
//! wire error envelope are chosen: `ErrorCode` and `EdgeCode` each carry
//! their status, and `ApiError` and `EdgeError` build the body. A handler
//! names what happened and never a number.
//!
//! This scan reads the non-test source of `ordo-host` and `ordo-api` and
//! fails on:
//!
//! - a `StatusCode::` that is not a success or a redirect, outside the
//!   edge, beyond the short counted list below;
//! - an `{"error": ...}` body written by hand;
//! - a second `IntoResponse` for an error type, beyond the one listed.

use std::path::{Path, PathBuf};

/// Built so this file never holds the needles it looks for.
fn needle(parts: &[&str]) -> String {
    parts.concat()
}

/// The statuses a handler may name itself: they answer what was asked, and
/// none of them is an error.
const NOT_ERRORS: &[&str] = &[
    "OK",
    "CREATED",
    "ACCEPTED",
    "NO_CONTENT",
    "PARTIAL_CONTENT",
    "NOT_MODIFIED",
    "SEE_OTHER",
];

/// File (relative to `crates/`), the status named, how many times, and why.
const ALLOWED: &[(&str, &str, usize)] = &[
    // A CORS preflight from an origin that is not allowed: an empty 403 the
    // browser reads, never a body a client parses.
    ("ordo-api/src/cors.rs", "FORBIDDEN", 1),
    // The replay of a stored response: the status the first answer had,
    // chosen by the edge when it was first given.
    ("ordo-api/src/router.rs", "from_u16", 1),
    // A match on a response the proxy already got, to pace link misses.
    // It reads a status; it does not choose one.
    ("ordo-host/src/proxy.rs", "FORBIDDEN", 1),
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
                if rel == "ordo-api/src/error.rs" {
                    continue;
                }
                out.push((rel, std::fs::read_to_string(&p).expect("source")));
            }
        }
    }
    out.sort();
    assert!(
        out.len() > 100
            && out.iter().any(|(p, _)| p == "ordo-host/src/serve.rs")
            && out.iter().any(|(p, _)| p == "ordo-api/src/router.rs"),
        "the scan is reading the wrong tree"
    );
    out
}

/// The source with its unit-test module cut off and comment lines dropped.
fn code(body: &str) -> String {
    let end = body.find("#[cfg(test)]\nmod ").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `StatusCode::NAME` in `code`, by name.
fn statuses(code: &str) -> Vec<String> {
    let marker = needle(&["StatusCode", "::"]);
    code.match_indices(&marker)
        .map(|(i, _)| {
            code[i + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        })
        .collect()
}

/// How many `{"error": {` or `{"error": format!(` bodies `code` writes.
fn envelopes(code: &str) -> usize {
    let key = needle(&["\"err", "or\""]);
    code.match_indices(&key)
        .filter(|(i, _)| {
            let rest = code[i + key.len()..].trim_start();
            let Some(rest) = rest.strip_prefix(':') else {
                return false;
            };
            let rest = rest.trim_start();
            rest.starts_with('{') || rest.starts_with("format!")
        })
        .count()
}

#[test]
fn only_the_edge_chooses_an_error_status() {
    let mut wrong = Vec::new();
    let mut used: Vec<(String, String, usize)> = Vec::new();
    for (file, body) in sources() {
        for name in statuses(&code(&body)) {
            if NOT_ERRORS.contains(&name.as_str()) {
                continue;
            }
            if !ALLOWED.iter().any(|(f, n, _)| *f == file && *n == name) {
                wrong.push(format!("{file}: StatusCode::{name}"));
                continue;
            }
            match used.iter_mut().find(|(f, n, _)| *f == file && *n == name) {
                Some(u) => u.2 += 1,
                None => used.push((file.clone(), name, 1)),
            }
        }
    }
    for (f, n, may) in ALLOWED {
        let found = used
            .iter()
            .find(|(uf, un, _)| uf == f && un == n)
            .map_or(0, |u| u.2);
        if found != *may {
            wrong.push(format!(
                "{f}: StatusCode::{n} {found} times, {may} listed; keep the list exact"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "an error status chosen outside `ordo-api/src/error.rs`. Return an \
         `ApiError` (an op class) or an `EdgeError` (an `EdgeCode`); each code \
         carries its own status:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn no_handler_writes_the_error_envelope_by_hand() {
    let wrong: Vec<String> = sources()
        .into_iter()
        .filter_map(|(file, body)| {
            let n = envelopes(&code(&body));
            (n > 0).then(|| format!("{file}: {n}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "an error body written by hand. `ApiError::envelope` and \
         `EdgeError::envelope` are the wire shape; add a field with \
         `EdgeError::with`:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn errors_answer_through_the_edges_responses() {
    let marker = needle(&["impl IntoResponse", " for "]);
    let allowed = [
        // Busy is `ApiError::server_busy`, anything else an `EdgeError`;
        // this impl only picks which.
        "ordo-host/src/auth/store_failure.rs",
    ];
    let mut wrong = Vec::new();
    for (file, body) in sources() {
        let n = code(&body).matches(&marker).count();
        let may = usize::from(allowed.contains(&file.as_str()));
        if n != may {
            wrong.push(format!("{file}: {n} (listed: {may})"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a response type outside the edge. Convert into `ApiError` or \
         `EdgeError` instead:\n{}",
        wrong.join("\n")
    );
}

/// The scan sees what it is for, and ignores tests and prose.
#[test]
fn the_scan_sees_a_hand_built_answer() {
    let status = needle(&["(StatusCode::", "CONFLICT, Json(body))"]);
    assert_eq!(statuses(&code(&status)), vec!["CONFLICT".to_string()]);
    assert!(statuses(&code(&format!("// {status}"))).is_empty());
    assert!(statuses(&code(&format!(
        "fn f() {{}}\n#[cfg(test)]\nmod tests {{\n{status}\n}}"
    )))
    .is_empty());
    let body = needle(&["json!({\"err", "or\": {\"code\": \"x\"}})"]);
    assert_eq!(envelopes(&code(&body)), 1);
    let flat = needle(&["json!({ \"err", "or\": format!(\"{e}\") })"]);
    assert_eq!(envelopes(&code(&flat)), 1);
    let read = needle(&["m[\"err", "or\"][\"code\"]"]);
    assert_eq!(envelopes(&code(&read)), 0);
}
