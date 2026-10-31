//! The compile-time half of a person's own download (owner ruling
//! 2026-10-22): a mark the door could not honour fails the deploy.

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

const NOTES: &str = "\
entity notes {
  title: text
  body:  text
  download: bool = false
}
";

#[test]
fn a_marked_read_over_one_entity_compiles_and_is_recorded() {
    let src = format!(
        "{NOTES}\nop allNotes unbounded download =\n  notes(download == true)[id, title]\n"
    );
    compile(&src).expect("compiles");
    let program = ordo_syntax::parse_program(&src).unwrap();
    let schema = ordo_schema::build(&program).unwrap();
    let op = program
        .ops
        .iter()
        .find(|o| o.name.name == "allNotes")
        .unwrap();
    assert!(op.flags.download);
    assert_eq!(
        ordo_plan::download::download_entity(&schema, op).as_deref(),
        Some("notes")
    );
}

#[test]
fn a_field_and_an_op_named_download_still_work() {
    compile(&format!(
        "{NOTES}\nop download unbounded =\n  notes[id, download]\n"
    ))
    .expect("compiles");
}

#[test]
fn a_marked_write_is_refused() {
    refused(
        &format!("{NOTES}\nop add($t: text) download =\n  set notes {{ title: $t, body: \"\" }}\n"),
        "download_write",
    );
}

#[test]
fn a_marked_op_with_a_required_parameter_is_refused() {
    refused(
        &format!("{NOTES}\nop byTitle($t: text) unbounded download =\n  notes(title == $t)[id]\n"),
        "download_params",
    );
}

#[test]
fn two_marked_ops_over_one_entity_are_refused() {
    refused(
        &format!(
            "{NOTES}\nop a unbounded download =\n  notes[id]\n\nop b unbounded download =\n  notes[id, title]\n"
        ),
        "download_entity_twice",
    );
}
