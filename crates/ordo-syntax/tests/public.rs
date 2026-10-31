//! The `public` word (`plan-syntax-additions.md` §1, ruled 2026-10-03): a
//! field modifier, an entity line (`public`, `public*`) and an op flag.

use ordo_syntax::parse_program;

#[test]
fn public_after_a_field_marks_the_field() {
    let p =
        parse_program("entity plans {\n  name: text public\n  margin: money\n}\n").expect("parses");
    let e = &p.entities[0];
    assert!(e.fields[0].public);
    assert!(!e.fields[1].public);
    assert!(e.public.is_none(), "a field's mark is not the entity line");
}

#[test]
fn public_on_its_own_line_marks_the_entity() {
    let p = parse_program("entity pages {\n  slug: text unique\n  body: text\n  public\n}\n")
        .expect("parses");
    let e = &p.entities[0];
    let mark = e.public.expect("the entity line");
    assert!(!mark.cascade);
    assert!(
        !e.fields[1].public,
        "the line after `body` is the entity's, not the field's"
    );
}

#[test]
fn public_star_cascades() {
    let p = parse_program("entity pages {\n  public*\n  slug: text\n}\n").expect("parses");
    assert!(p.entities[0].public.expect("marked").cascade);
}

#[test]
fn public_after_the_params_marks_the_op() {
    let p = parse_program("op pricing public =\n  plans[name]\n  take 5\n").expect("parses");
    assert!(p.ops[0].flags.public);
    let p = parse_program("op pricing =\n  plans[name]\n  take 5\n").expect("parses");
    assert!(!p.ops[0].flags.public);
}

#[test]
fn public_sits_beside_the_other_flags() {
    let p = parse_program("op pricing($n: int <= 10) unbounded public =\n  plans[name]\n")
        .expect("parses");
    assert!(p.ops[0].flags.public && p.ops[0].flags.unbounded);
}

#[test]
fn public_is_reserved_so_nothing_can_be_named_it() {
    assert!(parse_program("op public =\n  plans[name]\n  take 5\n").is_err());
    assert!(parse_program("entity jobs {\n  public: bool\n}\n").is_err());
}

#[test]
fn a_second_entity_line_is_refused() {
    assert!(parse_program("entity pages {\n  public\n  public*\n  slug: text\n}\n").is_err());
}
