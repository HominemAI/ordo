//! Every door refusal is recorded in the security stream.
//!
//! `AccessChecker::check` records its own denials. A refusal the door makes
//! before it (no token, an op nobody registered, a download that is off or
//! closed to the caller) must go through `router::door_refusal` or
//! `router::recorded`, which call `AccessChecker::record_refusal`. This scan
//! holds the door's sources to that:
//!
//! 1. the bare `door_denial` is called only by the three functions that wrap
//!    it;
//! 2. `denied_after_check` is called only right after `check` denied or the
//!    token gate closed a share grant, both of which recorded already;
//! 3. an `ErrorCode::AccessDenied` built anywhere else is inside a
//!    `recorded(` call, or is one of the few named below with the place that
//!    records it.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .to_path_buf()
}

const SOURCES: &[&str] = &[
    "ordo-api/src/router.rs",
    "ordo-api/src/download.rs",
    "ordo-api/src/blob.rs",
    "ordo-host/src/blob.rs",
    "ordo-host/src/serve.rs",
];

/// `AccessDenied` answers recorded somewhere other than at the line, each
/// with where. A new one is added here only with its recording place.
const RECORDED_ELSEWHERE: &[(&str, &str)] = &[
    // `door_denial` itself: every caller is held by rules 1 and 2.
    (
        "ordo-api/src/router.rs",
        "ApiError::new(ErrorCode::AccessDenied, message).with_op(op_name)",
    ),
    // `GET /live` with a token nobody signed: `token_known` records it.
    ("ordo-api/src/router.rs", "state.access.token_known(token)"),
    // The platform surfaces' one answer: after `authorize_in`, which
    // records a missing token, and `check`, which records the rest.
    (
        "ordo-host/src/serve.rs",
        "ApiError::new(ErrorCode::AccessDenied, \"not permitted\")",
    ),
    // A visitor and a file whose field is not public: `blob_get` records it
    // when the port answers this.
    (
        "ordo-host/src/blob.rs",
        "ApiError::new(ErrorCode::AccessDenied, \"missing bearer token\")",
    ),
];

fn code_lines(text: &str) -> Vec<&str> {
    // Stop at the unit tests: they build refusals to check them.
    let end = text.find("#[cfg(test)]").unwrap_or(text.len());
    text[..end].lines().collect()
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
}

#[test]
fn every_door_refusal_is_recorded() {
    let mut problems = Vec::new();
    for rel in SOURCES {
        let path = root().join(rel);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let lines = code_lines(&text);
        let mut current_fn = "";
        for (i, line) in lines.iter().enumerate() {
            if is_comment(line) {
                continue;
            }
            if let Some(at) = line.find("fn ") {
                let name = &line[at + 3..];
                current_fn = name.split(['(', '<']).next().unwrap_or("").trim();
            }
            let before = &lines[i.saturating_sub(10)..=i];
            let window = before.join("\n");
            if line.contains("door_denial(")
                && !line.contains("fn door_denial")
                && !matches!(current_fn, "denied_after_check" | "door_refusal")
            {
                problems.push(format!(
                    "{rel}:{}: bare door_denial: use door_refusal, or denied_after_check \
                     after check denied",
                    i + 1
                ));
            }
            if line.contains("denied_after_check(") && !line.contains("fn denied_after_check") {
                let near = &lines[i.saturating_sub(4)..=i].join("\n");
                if !(near.contains("AccessDecision::Deny")
                    || near.contains("TokenGate::ShareNotLive")
                    || near.contains("not_ready(")
                    || near.contains("presence_required("))
                {
                    problems.push(format!(
                        "{rel}:{}: denied_after_check without a check or gate denial before it",
                        i + 1
                    ));
                }
            }
            if line.contains("ErrorCode::AccessDenied") {
                let elsewhere = RECORDED_ELSEWHERE.iter().any(|(file, needle)| {
                    file == rel
                        && (window.contains(needle) || {
                            let after = &lines[i..(i + 4).min(lines.len())].join("\n");
                            after.contains(needle)
                        })
                });
                if !window.contains("recorded(") && !elsewhere {
                    problems.push(format!(
                        "{rel}:{}: an access_denied answer that is not recorded: build it \
                         inside router::recorded, or name where it is recorded in \
                         RECORDED_ELSEWHERE",
                        i + 1
                    ));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The scan must see something to hold: the recording path exists and is
/// called at the door.
#[test]
fn the_recording_path_is_in_use() {
    let router = std::fs::read_to_string(root().join("ordo-api/src/router.rs")).unwrap();
    assert!(router.contains("state.access.record_refusal("));
    assert!(
        router.matches("door_refusal(").count() >= 8,
        "door_refusal is barely used"
    );
    let download = std::fs::read_to_string(root().join("ordo-api/src/download.rs")).unwrap();
    assert!(download.contains("DoorRefusal::DownloadOff"));
    assert!(download.contains("DoorRefusal::DownloadNotForCaller"));
}
