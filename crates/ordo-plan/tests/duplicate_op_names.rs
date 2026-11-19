//! Two ops may not share a name (`duplicate_op_name`).
//!
//! The registry is one flat name space. Before this refusal a second op of
//! one name silently replaced the first, so a request for the name ran the
//! later body with the later access rules (reported fix, 2026-11-19, found
//! beside PR 29). A regression looks like any of these programs compiling,
//! or `Registry::extend_from` swapping an op already registered.

use ordo_sema::Registry;

const TASKS: &str = "entity tasks {\n  title: text\n  done: bool\n}\n\n";

fn parse(src: &str) -> ordo_syntax::Program {
    ordo_syntax::parse_program(src).expect("parses")
}

fn compile(src: &str) -> Result<(), String> {
    let program = parse(src);
    let schema = ordo_schema::build(&program).map_err(|e| format!("{}: {}", e.code, e.message))?;
    let mut registry = Registry::from_program(&program);
    registry.extend_from(&ordo_schema::acl_admin_ops(&schema));
    registry.extend_from(&ordo_schema::std_ops(&schema));
    ordo_plan::cost::check_registration(&schema, &registry)
        .map_err(|e| format!("{}: {}", e.code, e.message))
}

fn refused(r: Result<(), String>, says: &[&str]) {
    match r {
        Ok(()) => panic!("expected `duplicate_op_name`, and it compiled"),
        Err(e) => {
            assert!(e.starts_with("duplicate_op_name"), "{e}");
            for s in says {
                assert!(e.contains(s), "the message should say {s:?}: {e}");
            }
        }
    }
}

#[test]
fn two_authored_ops_of_one_name_are_refused() {
    let src = format!(
        "{TASKS}op open =\n  tasks(done == false)[title]\n  take 50\n\n\
         op open =\n  tasks(done == true)[title]\n  take 50\n"
    );
    refused(
        compile(&src),
        &["op `open`", "declared twice", "line", "Rename one"],
    );
}

#[test]
fn a_public_op_cannot_hide_behind_a_later_private_one_of_its_name() {
    let src = format!(
        "{TASKS}op open public =\n  tasks(done == false)[title]\n  take 50\n\n\
         op open =\n  tasks[title]\n  take 50\n"
    );
    refused(compile(&src), &["op `open`"]);
}

#[test]
fn the_registry_keeps_the_first_op_and_reports_the_second() {
    let a = parse(&format!(
        "{TASKS}op open =\n  tasks(done == false)[title]\n  take 50\n"
    ));
    let b = parse("op open =\n  tasks(done == true)[title]\n  take 50\n");
    let mut registry = Registry::from_program(&a);
    let first = registry.get("open").expect("op").span;
    registry.extend_from(&b);
    assert_eq!(registry.get("open").expect("op").span, first, "replaced");
    let schema = ordo_schema::build(&a).expect("schema");
    let e = ordo_plan::cost::check_registration(&schema, &registry).expect_err("refused");
    assert_eq!(e.code, "duplicate_op_name", "{}", e.message);
}

#[test]
fn an_op_named_like_a_routine_op_is_refused() {
    // The schema build refuses this when both sit in one program
    // (`invalid_routine`). A registry built apart from the schema's program
    // has no such guard, so the routine op must not replace the authored one.
    let routines = parse(&format!(
        "{TASKS}routine sweep {{\n  trigger every day 08:00Z\n  run:\n    tasks(done == true)[id]\n    each ->\n      set tasks {{ id: id, done: false }}\n}}\n"
    ));
    let schema = ordo_schema::build(&routines).expect("schema");
    let authored = parse("op sweep =\n  tasks[title]\n  take 50\n");
    let registry = Registry::from_program(&authored);
    let e = ordo_plan::cost::check_routines(&schema, &registry).expect_err("refused");
    assert_eq!(e.code, "duplicate_op_name", "{}", e.message);
    assert!(e.message.contains("routine `sweep`"), "{}", e.message);
}

#[test]
fn distinct_names_compile() {
    compile(&format!(
        "{TASKS}op open =\n  tasks(done == false)[title]\n  take 50\n\n\
         op closed =\n  tasks(done == true)[title]\n  take 50\n"
    ))
    .expect("compiles");
}
