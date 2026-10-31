//! The release rule resolved against the entities (checklist 27aq,
//! `plan-member-release.md` §2.5).

use ordo_schema::build;
use ordo_syntax::parse_program;

const SHAPES: &str = "\
entity people {
  member  -> members unique
  manager -> members?
}

entity invoices {
  owner  -> members
  status: text = \"open\"
  note:   text sealed
}

entity drafts {
  author -> members
  body:   text
}
";

fn schema_err(rule: &str) -> String {
    let program = parse_program(&format!("{SHAPES}\n{rule}\n")).expect("parses");
    build(&program).expect_err("refused").code.to_string()
}

#[test]
fn a_rule_over_declared_entities_builds() {
    let program = parse_program(&format!(
        "{SHAPES}\nrelease member when not invoices(owner == $member and status == \"open\") {{\n  \
         remove drafts.author\n  reassign invoices.owner to people(member == $member).manager or owner\n}}\n"
    ))
    .expect("parses");
    let schema = build(&program).expect("builds");
    let r = schema.release.as_ref().expect("the rule is carried");
    assert_eq!(
        ordo_schema::render_release(r),
        "release member when not invoices(owner == $member and status == \"open\") {\n  \
         remove drafts.author\n  reassign invoices.owner to people(member == $member).manager or owner\n}"
    );
    // No line, no rule: locked.
    let bare = build(&parse_program(SHAPES).expect("parses")).expect("builds");
    assert!(bare.release.is_none());
}

#[test]
fn release_on_sealed_is_refused() {
    assert_eq!(
        schema_err("release member when not invoices(owner == $member and note == \"x\")"),
        "release_on_sealed"
    );
}

#[test]
fn release_unknown_entity_is_refused() {
    assert_eq!(
        schema_err("release member when not bills(owner == $member)"),
        "release_unknown_entity"
    );
    assert_eq!(
        schema_err("release member open {\n  remove bills.owner\n}"),
        "release_unknown_entity"
    );
}

#[test]
fn an_effect_names_a_ref_to_members() {
    assert_eq!(
        schema_err("release member open {\n  remove drafts.body\n}"),
        "invalid_release_effect"
    );
    assert_eq!(
        schema_err("release member open {\n  remove members.id\n}"),
        "invalid_release_effect"
    );
    assert_eq!(
        schema_err(
            "release member open {\n  reassign drafts.author to invoices(owner == $member).status\n}"
        ),
        "invalid_release_effect"
    );
    assert_eq!(
        schema_err(
            "release member open {\n  remove drafts.author\n  reassign drafts.author to owner\n}"
        ),
        "duplicate_release_effect"
    );
}

#[test]
fn a_walk_the_graph_cannot_resolve_is_the_walks_class() {
    assert_eq!(
        schema_err("release member when not invoices(payer == $member)"),
        "invalid_guard_walk"
    );
}
