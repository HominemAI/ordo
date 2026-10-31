//! Every call to an outside identity provider goes through one client, and a
//! failure of one is classified by its **type**, never by its words
//! (AGENTS.md rule 9, owner 2026-10-23; `docs/audit-service-interfaces.md`,
//! fix order step 3).
//!
//! `ordo_identity::federation::ProviderClient` builds every request
//! (discovery, token exchange, userinfo, keys). `ProviderError::class()` and
//! its `is_*` methods say what a failure means. This scan reads the
//! sign-in provider code and fails on:
//!
//! - a request to a provider built or sent outside `federation.rs`;
//! - an HTTP client or the egress `fetch` named in provider code other than
//!   the host's one transport (`ordo-host/src/federation.rs`);
//! - a failure sorted by the word it carries (`kind == "rejected"`,
//!   `reason() == "provider-busy"`, `to_string().contains(..)`).

use std::path::{Path, PathBuf};

fn needle(parts: &[&str]) -> String {
    parts.concat()
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(crates_dir().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("dir") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Non-test code lines, comments dropped. Unit tests may build fakes.
fn code_lines(body: &str) -> Vec<&str> {
    let end = body.find("#[cfg(test)]\nmod ").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect()
}

/// The sign-in provider code, relative to `crates/`.
fn provider_files() -> Vec<String> {
    let mut out = Vec::new();
    let mut all = Vec::new();
    rs_files(&crates_dir().join("ordo-identity/src"), &mut all);
    for p in all {
        let rel = p
            .strip_prefix(crates_dir())
            .expect("under crates/")
            .to_string_lossy()
            .replace('\\', "/");
        if !rel.ends_with("_tests.rs") && !rel.ends_with("/tests.rs") {
            out.push(rel);
        }
    }
    for rel in [
        "ordo-host/src/federation.rs",
        "ordo-host/src/oidc.rs",
        "ordo-host/src/tenant_providers.rs",
        "ordo-host/src/auth/oidc.rs",
        "ordo-host/src/auth/federated.rs",
        "ordo-host/src/auth/tenant.rs",
    ] {
        if crates_dir().join(rel).is_file() {
            out.push(rel.to_string());
        }
    }
    assert!(
        out.iter().any(|f| f == "ordo-identity/src/oidc.rs")
            && out.iter().any(|f| f == "ordo-identity/src/oauth2.rs")
            && out.iter().any(|f| f == "ordo-host/src/federation.rs"),
        "the scan is reading the wrong tree"
    );
    out
}

fn text_needles() -> Vec<String> {
    vec![
        needle(&[".kind ", "== \""]),
        needle(&[".kind() ", "== \""]),
        needle(&[".reason() ", "== \""]),
        needle(&["== \"provider", "-busy\""]),
        needle(&["== \"rej", "ected\""]),
        needle(&[".to_string().contains", "("]),
        needle(&["format!(\"{e}\").contains", "("]),
        needle(&["why.contains", "("]),
        needle(&["why.starts_with", "("]),
    ]
}

fn client_needles() -> Vec<String> {
    vec![
        needle(&["ordo_egress", "::fetch("]),
        needle(&["reqw", "est"]),
        needle(&["hyper", "::client"]),
        needle(&["ur", "eq::"]),
        needle(&["TcpStr", "eam"]),
    ]
}

#[test]
fn no_provider_failure_is_classified_by_its_text() {
    let needles = text_needles();
    let mut wrong = Vec::new();
    for file in provider_files() {
        for line in code_lines(&read(&file)) {
            // The one allowed match: `strict_json` reads back its own
            // duplicate-member sentinel out of serde's error. It parses
            // bytes; it is no provider failure.
            if file == "ordo-identity/src/jose.rs" && line.contains("contains(DUPLICATE)") {
                continue;
            }
            for n in &needles {
                if line.contains(n.as_str()) {
                    wrong.push(format!("{file}: {}", line.trim()));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a provider failure classified by its text; use `ProviderError::class()` or an \
         `is_*` method instead:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn every_provider_call_goes_through_the_provider_client() {
    let net = client_needles();
    let mut wrong = Vec::new();
    for file in provider_files() {
        let body = read(&file);
        for line in code_lines(&body) {
            // The one transport implementation, over the egress kernel.
            if file != "ordo-host/src/federation.rs" {
                for n in &net {
                    if line.contains(n.as_str()) {
                        wrong.push(format!("{file}: {}", line.trim()));
                    }
                }
            }
            // A request is built, and the transport called, in
            // `federation.rs` (`ProviderClient`) alone.
            if file != "ordo-identity/src/federation.rs" {
                for n in [
                    needle(&["FederationRequest", " {"]),
                    needle(&["transport", ".fetch("]),
                ] {
                    if line.contains(n.as_str()) {
                        wrong.push(format!("{file}: {}", line.trim()));
                    }
                }
                if line.trim_start().starts_with(".fetch(") {
                    wrong.push(format!("{file}: {}", line.trim()));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "a call to an identity provider outside `ProviderClient`:\n{}",
        wrong.join("\n")
    );
}

/// The scan sees what it is for.
#[test]
fn the_scan_sees_a_text_match_and_a_stray_call() {
    let hit = |needles: &[String], line: &str| needles.iter().any(|n| line.contains(n.as_str()));
    let text = text_needles();
    let net = client_needles();
    assert!(hit(&text, "if f.kind == \"rejected\" {"));
    assert!(hit(&text, "if f.reason() == \"provider-busy\" {"));
    assert!(hit(&text, "e.to_string().contains(\"busy\")"));
    assert!(hit(&net, "let r = ordo_egress::fetch(dest, roots)"));
    assert!(hit(&net, "let c = reqwest::Client::new();"));
    assert!(!hit(&text, "match f.class() {"));
    assert!(code_lines("// f.kind == \"rejected\"\nlet x = 1;")
        .iter()
        .all(|l| !hit(&text, l)));
}
