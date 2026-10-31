//! Checklist 21c, security finding S6-003 — **a sealed value is bound to
//! the row it lives on.**
//!
//! Before this, a sealed column's associated data named the entity and the
//! field and nothing else, so somebody holding the database and no key could
//! copy an envelope from one row to another in the same column and it opened
//! as if it had always been there. These tests do exactly that, in the
//! database, and require the read to fail.
//!
//! The falsification the plan names is the first test: transplant an
//! envelope between two rows and require the open to fail. It fails without
//! the change, because the two rows then share one label.
//!
//! Real Postgres, this suite's own databases (`ordo_seals_*`).

mod gc_support;

use gc_support::{db, ANA};
use ordo_conformance::{Fixture, WriteLog, FIXTURE, SEED};

/// A unique-keyed entity with a sealed field, and the ops that reach it and
/// `orders.memo` in the shapes the seal has to answer for: a create, an
/// addressed update, a converge-or-create on a unique key, and a ranged
/// update over many rows.
const EXTRA: &str = r#"
entity vaults (key: vlt) {
  slug: text unique
  memo: text sealed
}

op putVault($slug: text, $memo: text) =
  set vaults { slug: $slug, memo: $memo }

op readVaults unbounded =
  vaults[slug, memo]

op memoDone($memo: text) =
  set orders(status == "done") { memo: $memo }
"#;

fn fixture() -> Fixture {
    Fixture::from_source(&format!("{FIXTURE}\n{EXTRA}"), SEED).expect("the fixture compiles")
}

fn stored(pg: &ordo_conformance::Pg, entity: &str, column: &str, id: &str) -> String {
    pg.query(&format!(
        r#"SELECT encode("{column}", 'hex') FROM "{entity}" WHERE "id" = '{id}'"#
    ))
    .expect("read the stored bytes")[0][0]
        .clone()
        .expect("a stored envelope")
}

/// **The falsification for columns.** Two rows of one column, each holding
/// its own memo. The envelope of one is copied over the other, in the
/// database, by somebody with no key. The read of the row that received it
/// must fail, and the row that lost nothing must still read.
#[test]
fn an_envelope_moved_to_another_row_does_not_open() {
    let f = fixture();
    let pg = db("ordo_seals_transplant");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    for (id, memo) in [("ord_1", "for the first"), ("ord_2", "for the second")] {
        f.run_write_as(
            &pg,
            &mut log,
            &format!(r#"setMemo($order: "{id}", $memo: "{memo}")"#),
            ANA,
        )
        .expect("an addressed sealed write");
    }
    let one = f
        .run_case(&pg, r#"orders(id == "ord_1")[memo]"#)
        .expect("row one reads");
    assert_eq!(one[0]["memo"], "for the first");

    // The transplant: no key, no write path, only the database.
    pg.exec(
        r#"UPDATE "orders" SET "memo" = (SELECT "memo" FROM "orders" WHERE "id" = 'ord_1')
           WHERE "id" = 'ord_2'"#,
    )
    .expect("the operator's UPDATE");
    assert_eq!(
        stored(&pg, "orders", "memo", "ord_1"),
        stored(&pg, "orders", "memo", "ord_2"),
        "the two rows now hold the same bytes"
    );

    let moved = f
        .run_case(&pg, r#"orders(id == "ord_2")[memo]"#)
        .expect_err("a moved envelope must not open");
    assert!(moved.starts_with("sealed_open_failed"), "{moved}");
    let still = f
        .run_case(&pg, r#"orders(id == "ord_1")[memo]"#)
        .expect("the row that kept its own envelope still reads");
    assert_eq!(still[0]["memo"], "for the first");
}

/// The same falsification over the create path, which seals before the row
/// exists: the id is minted first and the seal names it.
#[test]
fn a_created_row_holds_an_envelope_that_opens_only_at_its_own_id() {
    let f = fixture();
    let pg = db("ordo_seals_create");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let mut ids = Vec::new();
    for (slug, memo) in [("a", "alpha"), ("b", "beta")] {
        let r = f
            .run_write_as(
                &pg,
                &mut log,
                &format!(r#"putVault($slug: "{slug}", $memo: "{memo}")"#),
                ANA,
            )
            .expect("a create with a sealed field");
        ids.push(r[0]["created"].as_str().unwrap().to_string());
    }
    let read = f.run_case(&pg, "readVaults").expect("both rows read");
    assert_eq!(read.as_array().unwrap().len(), 2);

    pg.exec(&format!(
        r#"UPDATE "vaults" SET "memo" = (SELECT "memo" FROM "vaults" WHERE "id" = '{}')
           WHERE "id" = '{}'"#,
        ids[0], ids[1]
    ))
    .expect("the operator's UPDATE");
    let e = f
        .run_case(&pg, "readVaults")
        .expect_err("the read that meets the moved envelope fails");
    assert!(e.starts_with("sealed_open_failed"), "{e}");
}

/// **Question 1: an upsert onto a unique key.** The probe resolves the key
/// under the row lock and the value is sealed at the id it found. A second
/// write to the same key converges onto the same row and still opens.
#[test]
fn an_upsert_seals_at_the_row_the_key_resolved() {
    let f = fixture();
    let pg = db("ordo_seals_upsert");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let created = f
        .run_write_as(
            &pg,
            &mut log,
            r#"putVault($slug: "k", $memo: "first")"#,
            ANA,
        )
        .expect("the create branch");
    let id = created[0]["created"].as_str().unwrap().to_string();
    f.run_write_as(
        &pg,
        &mut log,
        r#"putVault($slug: "k", $memo: "second")"#,
        ANA,
    )
    .expect("the converge branch");

    let rows = pg.query(r#"SELECT "id" FROM "vaults""#).expect("the table");
    assert_eq!(rows.len(), 1, "one row, converged and not duplicated");
    assert_eq!(rows[0][0].as_deref(), Some(id.as_str()));
    let read = f
        .run_case(&pg, "readVaults")
        .expect("the converged row opens");
    assert_eq!(read[0]["memo"], "second");
}

/// **Question 2: a ranged write to a sealed field.** One statement used to
/// seal once and copy the envelope to every row, so the rows held identical
/// bytes. Now each row is sealed at its own id: every one opens where it
/// is, and no two share bytes.
#[test]
fn a_ranged_write_seals_every_row_at_its_own_id() {
    let f = fixture();
    let pg = db("ordo_seals_ranged");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    let r = f
        .run_write_as(&pg, &mut log, r#"memoDone($memo: "shipped")"#, ANA)
        .expect("a ranged sealed write");
    let touched = r[0]["updated"].as_u64().unwrap();
    assert_eq!(touched, 4, "ord_1, ord_2, ord_4 and ord_7 are done");

    let read = f
        .run_case(&pg, r#"orders(status == "done")[memo]"#)
        .expect("every touched row opens at its own id");
    assert_eq!(read.as_array().unwrap().len(), 4);
    assert!(read
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["memo"] == "shipped"));

    let mut seen = std::collections::BTreeSet::new();
    for id in ["ord_1", "ord_2", "ord_4", "ord_7"] {
        assert!(
            seen.insert(stored(&pg, "orders", "memo", id)),
            "`{id}` shares its bytes with another row of the same range"
        );
    }
}

/// A chain image's payload is a copy of the row's envelope, so it keeps the
/// row's label: the sealed payload store records which row it images.
#[test]
fn a_chain_payload_records_the_row_it_images() {
    let f = fixture();
    let pg = db("ordo_seals_payload");
    f.reset(&pg).expect("seeded");
    let mut log = WriteLog::new();

    f.run_write_as(
        &pg,
        &mut log,
        r#"setMemo($order: "ord_3", $memo: "imaged")"#,
        ANA,
    )
    .expect("an addressed sealed write");
    let anchors = pg
        .query(r#"SELECT DISTINCT "anchor" FROM "$sealedpayload" WHERE "entity" = 'orders'"#)
        .expect("the payload store");
    let anchors: Vec<_> = anchors.iter().filter_map(|r| r[0].clone()).collect();
    assert_eq!(anchors, vec!["row:ord_3".to_string()]);
}
