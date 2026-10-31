//! A mail failure is classified by its **type**, never by its words
//! (AGENTS.md rule 9, owner 2026-10-23; `docs/audit-service-interfaces.md`,
//! fix order step 5).
//!
//! `ordo_host::mail::MailError` keeps its cause (`source()`), the relay's
//! SMTP code or HTTP status (`relay_code()`), and answers through
//! `class()` and the `is_*` methods. The outbox decides retry or drop from
//! `SendPolicy::retries` alone. This scan reads the non-test source of the
//! mail code and the egress reply code and fails on a decision made by
//! matching text: a rendered error searched for a word, or an SMTP reply
//! line compared as a string instead of parsed to its number.

use std::path::Path;

const FILES: &[&str] = &[
    "ordo-host/src/mail.rs",
    "ordo-host/src/email_code.rs",
    "ordo-host/src/contact_email.rs",
    "ordo-egress/src/smtp.rs",
    "ordo-egress/src/refusal.rs",
];

/// Built so this file never holds the needles it looks for.
fn needles() -> Vec<String> {
    let mut out = Vec::new();
    for render in [".to_string()", ".operational()", ".disclosable()"] {
        for op in [".contains(", ".starts_with(", ".ends_with(", ".find("] {
            out.push([render, op].concat());
        }
    }
    for op in [".contains(", ".starts_with(", ".ends_with("] {
        out.push(["format!(\"{e}\")", op].concat());
        out.push(["format!(\"{err}\")", op].concat());
        out.push(["format!(\"{e:?}\")", op].concat());
    }
    // An SMTP code read from a reply line as text.
    for code in ["\"4", "\"5", "\"250", "\"354", "\"220", "\"421"] {
        out.push([".starts_with(", code].concat());
        out.push(["== ", code].concat());
    }
    // A mail error built from a bare sentence instead of its constructor.
    out.push(["MailError::Un", "available(\""].concat());
    out.push(["MailError::Re", "fused(\""].concat());
    out
}

fn code_lines(body: &str) -> Vec<&str> {
    let end = body.find("#[cfg(test)]\nmod ").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect()
}

fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn scan(body: &str, needles: &[String]) -> Vec<String> {
    let mut hits = Vec::new();
    for line in code_lines(body) {
        for n in needles {
            if line.contains(n.as_str()) {
                hits.push(format!("`{n}` in `{}`", line.trim()));
            }
        }
    }
    hits
}

#[test]
fn no_mail_error_is_classified_by_its_text() {
    let needles = needles();
    let mut wrong = Vec::new();
    for file in FILES {
        for hit in scan(&read(file), &needles) {
            wrong.push(format!("{file}: {hit}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "decide by MailError::class() or Refusal's variant, not by its words:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn the_scan_sees_a_text_match() {
    let seen = ["let busy = e", ".to_string()", ".contains(\"busy\");"].concat();
    assert_eq!(scan(&seen, &needles()).len(), 1);
    let clean = "let busy = e.is_busy();";
    assert!(scan(clean, &needles()).is_empty());
    let after_tests = format!(
        "fn f() {{}}\n#[cfg(test)]\nmod tests {{\n{}\n}}",
        ["e", ".to_string()", ".contains(\"x\")"].concat()
    );
    assert!(scan(&after_tests, &needles()).is_empty());
}

#[test]
fn the_files_exist_and_are_read() {
    for file in FILES {
        assert!(read(file).len() > 500, "{file}");
    }
}
