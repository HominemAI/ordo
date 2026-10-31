//! One client to the door (AGENTS.md rule 9, `plan-console.md` 3).
//!
//! The console reaches the deployment through `src/door.rs` and nowhere
//! else: one client, one typed error (`DoorError`). This scan reads the
//! console's non-test source and fails on a socket, an HTTP client or a TLS
//! connector opened anywhere but there, beyond the one counted exception:
//! the supervisor's readiness probe, which asks the child it runs whether
//! `/health/ready` answers 200 and reads nothing else.

use std::path::{Path, PathBuf};

/// Built so this file never holds the needles it looks for.
fn needle(parts: &[&str]) -> String {
    parts.concat()
}

fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("dir") {
            let p = entry.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let rel = p
                    .strip_prefix(&root)
                    .expect("under src/")
                    .display()
                    .to_string();
                let text = std::fs::read_to_string(&p).expect("read");
                // Test modules are not the console's own calls.
                let text = match text.find("#[cfg(test)]") {
                    Some(i) => text[..i].to_string(),
                    None => text,
                };
                out.push((rel, text));
            }
        }
    }
    out
}

#[test]
fn the_deployment_is_reached_only_through_the_door_client() {
    let needles = [
        needle(&["TcpStream", "::connect"]),
        needle(&["hyper", "::client"]),
        needle(&["tokio_", "rustls"]),
        needle(&["Door", "Client::new"]),
    ];
    // File, needle, how many times, and why.
    let allowed: &[(&str, &str, usize)] = &[
        // The server builds the one client, once, at boot.
        ("server.rs", "DoorClient::new", 1),
        // The one client to the identity service (AGENTS.md rule 9): it
        // fetches the service's public keys and makes management calls,
        // and passes browsers through under the service's base path
        // (step 7). It never reaches the deployment.
        ("identity.rs", "TcpStream::connect", 2),
        ("identity.rs", "hyper::client", 2),
        ("identity.rs", "tokio_rustls", 2),
    ];
    let mut found = Vec::new();
    for (file, text) in sources() {
        if file == "door.rs" {
            continue;
        }
        for n in &needles {
            let count = text.matches(n.as_str()).count();
            if count == 0 {
                continue;
            }
            let ok = allowed
                .iter()
                .any(|(f, what, max)| *f == file && *what == n && count <= *max);
            if !ok {
                found.push(format!("{file}: {n} x{count}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "the deployment is reached outside src/door.rs; go through DoorClient: {found:?}"
    );
}

#[test]
fn every_door_failure_is_the_one_typed_error() {
    // A second error type for the door would be a second way of handling
    // its failures. `DoorError` is declared once, in door.rs.
    let decl = needle(&["enum ", "DoorError"]);
    let places: Vec<String> = sources()
        .into_iter()
        .filter(|(_, t)| t.contains(&decl))
        .map(|(f, _)| f)
        .collect();
    assert_eq!(places, vec!["door.rs".to_string()]);
    let (_, door) = sources()
        .into_iter()
        .find(|(f, _)| f == "door.rs")
        .expect("door.rs");
    assert!(
        door.contains(&needle(&["#[non_", "exhaustive]\npub enum DoorError"])),
        "DoorError is #[non_exhaustive]"
    );
}
