//! `ordo plan` notes an external entity with no `access read` (review
//! finding 6). `shared` is the owner's opt-in for slice 1, so the behaviour
//! stays; the note is what makes "every member of every space reads every
//! row" visible at the point a deploy is reviewed.

use ordo_cli::{plan_artifact, plan_notes, Sources};

fn one(text: &str) -> Sources {
    Sources::from_parts(vec![("app.ordo".into(), text.to_string())])
}

const PROGRAM: &str = "\
external patrons (key: pat) from crm.customers shared {
  id <- \"customer_id\"
  name: text <- \"full_name\"
}
external regions (key: reg) from crm.regions shared {
  id: text <- code
  label: text <- label
  access read label == $caller
}
";

#[test]
fn an_external_with_no_read_guard_is_noted_once() {
    let notes = plan_notes(&one(PROGRAM));
    assert_eq!(
        notes,
        ["note: `patrons` has no `access read`: every member of every space reads every row"]
    );
    // The artifact itself is unchanged by the note: it goes to stderr.
    assert!(!plan_artifact(&one(PROGRAM), None)
        .expect("plans")
        .contains("note:"));
}

#[test]
fn a_program_without_externals_has_no_notes() {
    assert!(plan_notes(&one("entity notes { body: text }\n")).is_empty());
}
