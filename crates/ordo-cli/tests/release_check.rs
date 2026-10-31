//! `ordo check` and the release rule (checklist 27aq).
//!
//! The owner asked that a program with no release line say so, *"so if a dev
//! is trying to figure out why users can't leave he's not hunting for it"*
//! (2026-10-05, `person-can-leave` ask 4).

use ordo_cli::{check_sources, Sources};

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("app.ordo".into(), text.to_string())])
}

const SHAPES: &str = "\
entity invoices {
  owner  -> members
  status: text = \"open\"
}
";

#[test]
fn no_release_line_says_members_are_locked() {
    let out = check_sources(&one(SHAPES)).expect("checks");
    assert!(
        out.contains("RELEASE\n  no release rule: members cannot leave on their own (locked)"),
        "{out}"
    );
}

#[test]
fn a_release_line_is_printed_as_written() {
    let out = check_sources(&one(&format!(
        "{SHAPES}\nrelease member when not invoices(owner == $member and status == \"open\")\n"
    )))
    .expect("checks");
    assert!(
        out.contains(
            "RELEASE\n  release member when not invoices(owner == $member and status == \"open\")\n"
        ),
        "{out}"
    );
}

/// A walk the guard compiler refuses fails the check, not the first leave.
#[test]
fn a_mistyped_release_literal_fails_the_check() {
    let e = check_sources(&one(&format!(
        "{SHAPES}\nrelease member when not invoices(owner == $member and status == 3)\n"
    )))
    .unwrap_err();
    assert!(e.contains("registration error"), "{e}");
}
