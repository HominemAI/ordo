//! Every sign-in route tells a busy server from a broken store **through one
//! interface** (owner ruling 2026-10-23): a store error converts into
//! `ordo_host::auth::StoreFailure`, and the route answers that. This scan
//! holds the auth sources to it, so a new route cannot answer a store
//! failure spot by spot.
//!
//! What it refuses, in `src/auth.rs` and `src/auth/*.rs`:
//!
//! - a `503` built by hand anywhere (the edge chooses every status,
//!   `tests/edge_scan.rs`);
//! - the `credential_store_unavailable` answer written out by hand;
//! - a `StoreFailure::Unavailable` made up on the spot instead of converted
//!   from the error;
//! - a store error dropped on the way: `map_err(|_| ...Unavailable...)`, a
//!   bare `Unavailable(_)` arm that answers, and `Result<_, ()>`.
//!
//! The behaviour is in `tests/auth_busy.rs`. This is the structure.

use std::path::{Path, PathBuf};

fn sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![root.join("auth.rs")];
    let mut dir: Vec<PathBuf> = std::fs::read_dir(root.join("auth"))
        .expect("src/auth")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    dir.sort();
    files.extend(dir);
    let read: Vec<(PathBuf, String)> = files
        .into_iter()
        .map(|p| {
            let body = std::fs::read_to_string(&p).expect("source");
            (p, body)
        })
        .collect();
    assert!(
        read.len() > 15 && read.iter().any(|(p, _)| p.ends_with("store_failure.rs")),
        "the scan is reading the wrong tree"
    );
    read
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Built so this file never holds the needles it looks for.
fn needle(parts: &[&str]) -> String {
    parts.concat()
}

#[test]
fn only_the_store_failure_type_answers_a_store_503() {
    // File and how many `503`s it may build that are not a store's.
    // None: since fix order step 6 every 503 is chosen by the edge
    // (`ordo_api::EdgeCode`, `ErrorCode`), and `tests/edge_scan.rs` holds
    // every source to that. A route that is not a store names its own
    // `EdgeCode`; a store that did not answer is a `StoreFailure`.
    let allowed: &[(&str, usize)] = &[];
    let status = needle(&["StatusCode::", "SERVICE_UNAVAILABLE"]);
    let mut wrong = Vec::new();
    for (p, body) in sources() {
        let n = name(&p);
        if n == "store_failure.rs" {
            continue;
        }
        let found = body.matches(&status).count();
        let may = allowed
            .iter()
            .find(|(f, _)| *f == n)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        if found != may {
            wrong.push(format!("{n}: {found} (listed: {may})"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a sign-in source builds a 503 itself. A store that did not answer \
         is a `StoreFailure` (`.into_response()` or `.answer(code, message)`), \
         so a full pool is `server_busy`. If this 503 is not a store's, list it \
         here with why:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn no_route_writes_the_store_answer_or_a_failure_by_hand() {
    let banned = [
        needle(&["store_", "unreadable"]),
        needle(&["store_", "failed("]),
        needle(&["\"credential_store_", "unavailable\""]),
        needle(&["StoreFailure::", "Unavailable("]),
        needle(&[", ()", ">"]),
    ];
    let mut wrong = Vec::new();
    for (p, body) in sources() {
        if name(&p) == "store_failure.rs" {
            continue;
        }
        for (i, line) in body.lines().enumerate() {
            for b in &banned {
                if line.contains(b.as_str()) {
                    wrong.push(format!("{}:{}: {}", name(&p), i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "convert the store's error into a `StoreFailure` instead:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn no_store_error_is_dropped_on_the_way_to_the_answer() {
    let dropped_arm = needle(&["Unavailable(", "_)"]);
    let dropped_map = needle(&["map_err(|", "_|"]);
    let mut wrong = Vec::new();
    for (p, body) in sources() {
        if name(&p) == "store_failure.rs" {
            continue;
        }
        let lines: Vec<&str> = body.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let at = format!("{}:{}: {}", name(&p), i + 1, line.trim());
            if line.contains(&dropped_map) && line.contains("Unavailable(") {
                wrong.push(at.clone());
            }
            if line.contains(&dropped_arm) {
                // Fine: the error is bound (`e @`), only a stream word is
                // read from it (`=> "word"`), or it goes on whole to
                // `ceremony_refusal`.
                let next: String = lines[i + 1..lines.len().min(i + 3)].concat();
                let ok = line.contains('@')
                    || line.contains("=> \"")
                    || line.contains("ceremony_refusal")
                    || next.contains("ceremony_refusal")
                    || next.trim_start().starts_with('"');
                if !ok {
                    wrong.push(at);
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a store error is dropped before it is answered, so a full pool \
         cannot be told from a broken store. Keep it and convert it into a \
         `StoreFailure`:\n{}",
        wrong.join("\n")
    );
}
