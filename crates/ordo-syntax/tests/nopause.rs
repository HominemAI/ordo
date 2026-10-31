//! The `nopause` op word (checklist 27ar, owner 2026-10-05): an op that keeps
//! answering while its tenant is paused, `op billing nopause = ...`, beside
//! `unguarded` and `unbounded`.

use ordo_syntax::parse_program;

#[test]
fn nopause_after_the_name_marks_the_op() {
    let p = parse_program("op billing nopause =\n  invoices[id]\n  take 5\n").expect("parses");
    assert!(p.ops[0].flags.nopause);
    assert!(!p.ops[0].flags.unguarded);
}

#[test]
fn nopause_sits_beside_the_other_flags_in_any_order() {
    let p = parse_program(
        "op purge($id: id) unguarded nopause unbounded =\n  del invoices { id: $id }\n",
    )
    .expect("parses");
    let f = p.ops[0].flags;
    assert!(f.nopause && f.unguarded && f.unbounded);
}

#[test]
fn an_op_without_the_word_is_refused_while_paused() {
    let p = parse_program("op billing =\n  invoices[id]\n  take 5\n").expect("parses");
    assert!(!p.ops[0].flags.nopause);
}

#[test]
fn nopause_is_reserved_so_nothing_can_be_named_it() {
    assert!(parse_program("op nopause =\n  invoices[id]\n  take 5\n").is_err());
    assert!(parse_program("entity jobs {\n  nopause: bool\n}\n").is_err());
}
