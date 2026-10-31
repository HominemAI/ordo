//! `text(64)` — the declared length bound of a `text` field, executed.
//!
//! DL §2 Types says `text` is "boundable `text(64)`" and DL §2 Parameters
//! spells `$payee: text(64)` with "bounds are enforced at the door, before
//! planning". Until 2026-10-04 neither spelling parsed (`examples/syntax`
//! README, Findings). This suite holds the two enforcers to one number:
//!
//! 1. **The database carries the bound for an open column** as a named
//!    `CHECK (char_length(col) <= n)` among its row invariants, the
//!    backstop for a value that does not come through the write path.
//! 2. **The write path refuses first**, and for a **sealed** column it is
//!    the only enforcer: the database holds ciphertext and cannot count
//!    the value's characters, so no `CHECK` is emitted for it and the
//!    refusal has to happen while the plaintext is still in hand. Its
//!    class is `check_violation`, the class the database's backstop
//!    raises; `bound_violation` is the door's answer for a parameter.
//! 3. **The count is characters, not bytes**, in both places. A value of
//!    eight non-ASCII characters fits `text(8)`; `octet_length` would have
//!    refused it in the database while the door admitted it.
//!
//! Real Postgres, this suite's own databases (`ordo_textbound_*`), one
//! per test because the tests run concurrently and each resets its own.

mod gc_support;

use gc_support::{class_of, db, ANA};
use ordo_conformance::{Fixture, WriteLog, FIXTURE, SEED};

const CLUSTER: &str = r#"
entity tags (key: tag) {
  name: text(8)
  note: text(8)? sealed
  body: text
}
"#;

const CLUSTER_OPS: &str = r#"
op addTag($name: text(8), $body: text) =
  set tags { name: $name, note: null, body: $body }

op addTagWithNote($name: text(8), $note: text(8), $body: text) =
  set tags { name: $name, note: $note, body: $body }

# The bound is on the FIELD, so a value that reaches it another way than a
# bounded parameter is refused too: here, concatenation of two parameters
# each short enough on its own.
op addJoined($a: text(8), $b: text(8)) =
  set tags { name: $a + $b, note: null, body: $a }
"#;

fn fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{CLUSTER}\n{CLUSTER_OPS}");
    Fixture::from_source(&src, SEED).expect("the fixture plus the bounded entity compiles")
}

/// The DDL names the constraint after the field and counts characters;
/// the sealed column gets none.
#[test]
fn the_ddl_carries_a_character_count_check_for_open_columns_only() {
    let f = fixture();
    let ddl = ordo_schema::ddl::ddl_for(&f.schema).expect("ddl");
    assert!(
        ddl.contains(
            r#"CONSTRAINT "tags$len$name<=8" CHECK (("name" IS NULL OR char_length("name") <= 8))"#
        ),
        "{ddl}"
    );
    assert!(
        !ddl.contains("tags$len$note"),
        "a sealed column holds ciphertext; its length is not the value's:\n{ddl}"
    );
    assert!(
        !ddl.contains("tags$len$body"),
        "an unbounded text has no check:\n{ddl}"
    );
}

/// Eight characters land, nine refuse, and the count is characters: eight
/// two-byte characters are eight, not sixteen.
#[test]
fn an_over_long_value_is_refused_before_the_write_and_a_fitting_one_lands() {
    let f = fixture();
    let pg = db("ordo_textbound_len");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    f.run_write_as(
        &pg,
        &mut log,
        r#"addTag($name: "abcdefgh", $body: "x")"#,
        ANA,
    )
    .expect("eight characters fit text(8)");
    f.run_write_as(
        &pg,
        &mut log,
        r#"addTag($name: "ééééééé€", $body: "x")"#,
        ANA,
    )
    .expect("eight non-ASCII characters fit text(8): the bound counts characters");

    let err = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addTag($name: "abcdefghi", $body: "x")"#,
            ANA,
        )
        .expect_err("nine characters do not fit text(8)");
    assert_eq!(class_of(&err), "check_violation", "{err}");
    assert!(
        err.contains("tags.name") && err.contains("text(8)"),
        "{err}"
    );

    // The refusal left no trace.
    let rows = pg
        .query(r#"SELECT count(*) FROM "tags" WHERE "name" = 'abcdefghi'"#)
        .expect("read back");
    assert_eq!(rows[0][0].as_deref(), Some("0"));
}

/// The sealed column has no `CHECK`, so this is the one enforcer it has,
/// and it refuses the same way the open column does.
#[test]
fn a_sealed_bounded_field_is_refused_by_the_write_path() {
    let f = fixture();
    let pg = db("ordo_textbound_sealed");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    f.run_write_as(
        &pg,
        &mut log,
        r#"addTagWithNote($name: "a", $note: "12345678", $body: "x")"#,
        ANA,
    )
    .expect("eight characters fit the sealed text(8)");
    let err = f
        .run_write_as(
            &pg,
            &mut log,
            r#"addTagWithNote($name: "a", $note: "123456789", $body: "x")"#,
            ANA,
        )
        .expect_err("nine do not");
    assert_eq!(class_of(&err), "check_violation", "{err}");
    assert!(err.contains("tags.note"), "{err}");
}

/// The bound belongs to the field: two parameters that each fit, joined
/// into a value that does not, refuse at the field.
#[test]
fn the_bound_is_the_fields_and_catches_a_value_built_from_short_parameters() {
    let f = fixture();
    let pg = db("ordo_textbound_joined");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    f.run_write_as(&pg, &mut log, r#"addJoined($a: "abcd", $b: "efgh")"#, ANA)
        .expect("4 + 4 fits");
    let err = f
        .run_write_as(&pg, &mut log, r#"addJoined($a: "abcde", $b: "efgh")"#, ANA)
        .expect_err("5 + 4 does not");
    assert_eq!(class_of(&err), "check_violation", "{err}");
}
