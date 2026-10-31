//! The verifier is offline, and this is where that is a fact rather than a
//! claim.
//!
//! §7's documentation obligation is *"a reference verifier shipped with the
//! SDKs, so the common case is a library call rather than an implementation
//! project"*, and `implementation.md` wants a chain proof *"verifiable
//! offline by a published verifier"*. The word doing the work is **offline**:
//! this binary is handed to somebody whose whole reason for running it is
//! that they do not trust the deployment, so "it does not phone home" has to
//! be checkable in less time than reading the source.
//!
//! It is checkable in the time it takes to read one dependency list.

use std::path::Path;

fn manifest() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("this crate's own manifest")
}

fn dependency_names() -> Vec<String> {
    let src = manifest();
    let mut out = Vec::new();
    let mut inside = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == "[dependencies]";
            continue;
        }
        if !inside || t.starts_with('#') || t.is_empty() {
            continue;
        }
        if let Some((name, _)) = t.split_once('=') {
            out.push(name.trim().trim_matches('"').to_string());
        }
    }
    out
}

#[test]
fn the_verifier_cannot_reach_a_deployment() {
    let deps = dependency_names();
    assert!(!deps.is_empty(), "the parser found no dependencies at all");
    for forbidden in [
        // A driver, a pool, a connection string.
        "ordo-exec",
        "tokio-postgres",
        "postgres",
        "sqlx",
        // A socket of any kind.
        "ordo-egress",
        "reqwest",
        "ureq",
        "hyper",
        "axum",
        "tokio",
        // Key material.
        "ordo-crypto",
        "ordo-host",
        "ordo-identity",
    ] {
        assert!(
            !deps.iter().any(|d| d == forbidden),
            "`{forbidden}` would give the published verifier a way to reach the very \
             deployment its user is refusing to trust"
        );
    }
}

#[test]
fn the_dependency_set_is_the_one_that_was_argued_for() {
    let mut deps = dependency_names();
    deps.sort();
    assert_eq!(
        deps,
        vec!["ordo-commandlog", "serde_json"],
        "the verifier's dependency set changed; if that was deliberate, say so here and \
         in the manifest's comment — this list is what a recipient reads instead of \
         auditing a call graph"
    );
}

#[test]
fn nothing_in_the_source_opens_a_socket_or_reads_the_environment() {
    // The manifest is the strong check; this is the cheap second one, and it
    // catches the case where a forbidden capability arrives through a
    // dependency that was already allowed.
    for file in [
        "src/lib.rs",
        "src/proof.rs",
        "src/attestation.rs",
        "src/main.rs",
    ] {
        let body =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
        let code = body
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//") && !t.starts_with("//!")
            })
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in ["TcpStream", "std::net", "tokio::", "var(", "var_os("] {
            assert!(
                !code.contains(forbidden),
                "{file} contains `{forbidden}`; the verifier reads its input and does \
                 arithmetic, and nothing else"
            );
        }
    }
}
