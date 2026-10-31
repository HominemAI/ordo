//! The compile-time half of the public path (`plan-syntax-additions.md` §1,
//! `design-public-path.md` §2): what a public op may disclose is proved at
//! registration, so the runtime can skip the guard walk and the audit row.

use ordo_sema::Registry;

fn compile(src: &str) -> Result<(), String> {
    let program = ordo_syntax::parse_program(src).map_err(|e| format!("parse: {}", e.message))?;
    let schema = ordo_schema::build(&program).map_err(|e| format!("{}: {}", e.code, e.message))?;
    let registry = Registry::from_program(&program);
    ordo_plan::cost::check_registration(&schema, &registry)
        .map_err(|e| format!("{}: {}", e.code, e.message))
}

fn refused(src: &str, class: &str) {
    match compile(src) {
        Ok(()) => panic!("expected `{class}`, and it compiled"),
        Err(e) => assert!(e.starts_with(class), "expected `{class}`, got {e}"),
    }
}

const PLANS: &str = "\
entity plans {
  name:   text  public
  price:  money public
  margin: money
}
";

#[test]
fn a_public_op_over_public_fields_compiles() {
    compile(&format!(
        "{PLANS}\nop pricing public =\n  plans(price > 0)[name, price]\n  sort name\n  take 50\n"
    ))
    .expect("compiles");
}

#[test]
fn public_on_a_sealed_field_is_refused() {
    refused(
        "entity plans {\n  notes: text sealed public\n}\n",
        "public_sealed",
    );
}

#[test]
fn an_entity_line_reaching_a_sealed_field_is_refused() {
    refused(
        "entity plans {\n  name: text\n  notes: text sealed\n  public\n}\n",
        "public_sealed",
    );
}

#[test]
fn a_public_op_reading_a_private_field_is_refused() {
    refused(
        &format!("{PLANS}\nop pricing public =\n  plans[name, margin]\n  take 50\n"),
        "public_reads_private",
    );
}

#[test]
fn a_condition_on_a_private_field_is_a_read() {
    // `plans(margin > 100)[name]` discloses the margin one bit per call.
    refused(
        &format!("{PLANS}\nop pricing public =\n  plans(margin > 100)[name]\n  take 50\n"),
        "public_reads_private",
    );
}

#[test]
fn a_sort_on_a_private_field_is_a_read() {
    refused(
        &format!("{PLANS}\nop pricing public =\n  plans[name]\n  sort margin\n  take 50\n"),
        "public_reads_private",
    );
}

#[test]
fn a_public_op_composing_a_private_op_is_refused() {
    refused(
        &format!(
            "{PLANS}\nop names =\n  plans[name]\n  take 50\n\nop pricing public =\n  names[name]\n  take 50\n"
        ),
        "public_composes_private",
    );
}

#[test]
fn a_public_op_composing_a_public_op_compiles() {
    compile(&format!(
        "{PLANS}\nop names public =\n  plans[name]\n  take 50\n\nop pricing public =\n  names[name]\n  take 10\n"
    ))
    .expect("compiles");
}

#[test]
fn caller_in_a_public_op_is_refused() {
    refused(
        "entity posts {\n  title: text public\n  author: text public\n}\n\
         op mine public =\n  posts(author == $caller)[title]\n  take 50\n",
        "public_caller",
    );
}

#[test]
fn public_star_cascades_to_owned_children() {
    let src = "\
entity pages {
  slug: text unique
  public*
}

entity sections {
  doc -> pages owned
  heading: text
}

op sectionList public =
  sections[heading, doc.slug]
  take 50
";
    compile(src).expect("the child is public through `public*`");
    let program = ordo_syntax::parse_program(src).unwrap();
    let schema = ordo_schema::build(&program).unwrap();
    let sections = schema.entity("sections").unwrap();
    assert!(sections.field("heading").unwrap().public);
    assert_eq!(
        sections.public.as_ref().unwrap().from.as_deref(),
        Some("pages")
    );
}

#[test]
fn plain_public_does_not_cascade() {
    refused(
        "entity pages {\n  slug: text\n  public\n}\n\
         entity sections {\n  doc -> pages owned\n  heading: text\n}\n\
         op sectionList public =\n  sections[heading]\n  take 50\n",
        "public_reads_private",
    );
}

#[test]
fn a_plain_ref_does_not_carry_the_cascade() {
    refused(
        "entity pages {\n  slug: text\n  public*\n}\n\
         entity notes {\n  doc -> pages\n  body: text\n}\n\
         op noteList public =\n  notes[body]\n  take 50\n",
        "public_reads_private",
    );
}

#[test]
fn public_on_a_guarded_entity_is_refused() {
    refused(
        "entity tasks {\n  title: text public\n  owner: text\n  access read owner == $caller\n  access write owner == $caller\n}\n",
        "public_guarded",
    );
}

#[test]
fn a_public_write_that_creates_compiles() {
    compile(
        "entity contacts {\n  name: text(120)\n  message: text(2000)\n}\n\
         op contact($name: text(120), $message: text(2000)) public =\n  \
         set contacts { name: $name, message: $message }\n",
    )
    .expect("a plain create compiles");
}

#[test]
fn a_public_create_once_on_a_unique_field_compiles() {
    // Ruled 2026-10-12 (ask 31): the door answers alike for a taken value.
    compile(
        "entity subscribers {\n  email: text(200) unique\n}\n\
         op subscribe($email: text(200)) public =\n  set subscribers { email: $email, rev: null }\n",
    )
    .expect("a create-once on a unique field compiles");
}

#[test]
fn a_public_write_may_create_two_linked_rows() {
    compile(
        "entity enquiries {\n  name: text(120)\n}\n\
         entity lines {\n  enquiry -> enquiries\n  body: text(200)\n}\n\
         op enquire($name: text(120), $body: text(200)) public =\n  \
         let e = set enquiries { name: $name }\n  \
         set lines { enquiry: e.id, body: $body }\n",
    )
    .expect("a ref to a row this op creates compiles");
}

#[test]
fn a_public_write_that_changes_a_row_is_refused() {
    let entity = "entity contacts {\n  name: text(120)\n}\n";
    for body in [
        "op c($id: text, $name: text(120)) public =\n  set contacts { id: $id, name: $name }\n",
        "op c($id: text, $name: text(120)) public =\n  set contacts(id == $id) { name: $name }\n",
        "op c($name: text(120)) public =\n  set contacts(name == $name) { name: \"x\" }\n",
        "op c($id: text) public =\n  del contacts { id: $id }\n",
        "op c($name: text(120)) public =\n  del contacts(name == $name)\n",
    ] {
        refused(&format!("{entity}{body}"), "public_write_not_create");
    }
}

#[test]
fn a_public_write_keyed_on_a_unique_field_is_refused_unless_create_once() {
    // A converge would change a row the visitor did not write.
    refused(
        "entity subscribers {\n  email: text(200) unique\n}\n\
         op subscribe($email: text(200)) public =\n  set subscribers { email: $email }\n",
        "public_write_not_create",
    );
}

#[test]
fn a_public_write_into_a_guarded_entity_is_refused() {
    refused(
        "entity notes {\n  owner: text\n  body: text(200)\n  access read open\n  access write owner == $caller\n}\n\
         op note($body: text(200)) public =\n  set notes { owner: \"x\", body: $body }\n",
        "public_write_guarded",
    );
}

#[test]
fn a_public_write_naming_the_caller_is_refused() {
    refused(
        "entity notes {\n  owner: text\n  body: text(200)\n}\n\
         op note($body: text(200)) public =\n  set notes { owner: $caller, body: $body }\n",
        "public_caller",
    );
}

#[test]
fn a_public_write_aiming_a_ref_at_a_private_row_is_refused() {
    refused(
        "entity accounts {\n  name: text\n}\n\
         entity notes {\n  account -> accounts\n  body: text(200)\n}\n\
         op note($account: text, $body: text(200)) public =\n  \
         set notes { account: $account, body: $body }\n",
        "public_write_ref",
    );
    // Aimed at a public entity, the visitor learns nothing new.
    compile(
        "entity posts {\n  title: text public\n}\n\
         entity comments {\n  post -> posts\n  body: text(200)\n}\n\
         op comment($post: text, $body: text(200)) public =\n  \
         set comments { post: $post, body: $body }\n",
    )
    .expect("a ref to a public entity compiles");
}

#[test]
fn files_follow_their_row() {
    // A blob field on a public entity is public, so a public op may serve
    // its handle and the blob door may serve its bytes.
    let src = "entity pages {\n  slug: text\n  hero: blob?\n  public\n}\n";
    let program = ordo_syntax::parse_program(src).unwrap();
    let schema = ordo_schema::build(&program).unwrap();
    assert!(
        schema
            .entity("pages")
            .unwrap()
            .field("hero")
            .unwrap()
            .public
    );
    refused(
        "entity pages {\n  slug: text\n  scan: blob? private\n  public\n}\n",
        "public_private",
    );
}

#[test]
fn the_artifact_carries_the_marks_only_when_set() {
    let src = format!("{PLANS}\nop pricing public =\n  plans[name, price]\n  take 50\n");
    let program = ordo_syntax::parse_program(&src).unwrap();
    let schema = ordo_schema::build(&program).unwrap();
    let registry = Registry::from_program(&program);
    let art = ordo_plan::artifact::op_artifact(&schema, &registry, "pricing").unwrap();
    assert_eq!(art["flags"]["public"], true);
    let entities = ordo_plan::artifact::entity_artifacts(&schema);
    let plans = entities.iter().find(|e| e["entity"] == "plans").unwrap();
    let name = plans["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "name")
        .unwrap();
    assert_eq!(name["public"], true);
    let margin = plans["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "margin")
        .unwrap();
    assert!(margin.get("public").is_none(), "absent when not set");
}
