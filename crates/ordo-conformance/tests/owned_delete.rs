//! Owner ruling `owned-delete` (2026-09-29): a plain `del` of a row deletes
//! every row that references it through an `owned` reference, including the
//! access rows Ordo derives for an entity with an `acl`. A plain reference
//! still refuses (`has_referrers`).
//!
//! Before the ruling, `refuse_with_referrers` ignored `owned`, so a board
//! with its creator's admin row could not be deleted at all.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_commandlog::store::Store;
use ordo_commandlog::{CommandKind, SpaceId};
use ordo_conformance::{Fixture, Pg, WriteLog, FIXTURE, SEED};
use serde_json::json;

const ANA: Option<&str> = Some("mem_1");
const BRAM: Option<&str> = Some("mem_2");

/// The fixture plus the delete ops these cases need, and one plain
/// reference into an owned row (`itemReturns.item -> orderItems`).
const EXTRA: &str = r#"
entity itemReturns (key: ret) {
  item -> orderItems
  note: text
}

op dropBoard($b: id) =
  del boards { id: $b }

op dropBoardsNamed($name: text) =
  del boards(name == $name)

op dropProject($p: id) =
  del projects { id: $p }

op dropOrder($o: id) =
  del orders { id: $o }

op dropProduct($p: id) =
  del products { id: $p }
"#;

fn setup(db: &str) -> (Fixture, Pg, WriteLog) {
    let src = format!("{FIXTURE}\n{EXTRA}");
    let f = Fixture::from_source(&src, SEED).expect("the fixture plus the delete ops compiles");
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    (f, pg, WriteLog::new())
}

fn ids(pg: &Pg, table: &str) -> Vec<String> {
    pg.query(&format!("SELECT \"id\" FROM \"{table}\" ORDER BY \"id\""))
        .expect("query")
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect()
}

#[test]
fn a_row_is_deleted_with_the_rows_it_owns_and_its_derived_acl_rows() {
    let (f, pg, mut log) = setup("ordo_owned_delete_acl");
    let r = f
        .run_write_as(&pg, &mut log, r#"dropBoard($b: "board_1")"#, ANA)
        .expect("Ana administers board_1, so she may delete it and everything it owns");
    assert_eq!(r, json!([{ "entity": "boards", "deleted": 1 }]));
    assert_eq!(ids(&pg, "boards"), ["board_2"]);
    assert_eq!(ids(&pg, "cards"), ["card_3"], "its cards went with it");
    assert_eq!(
        ids(&pg, "boardsAccess"),
        ["ba_4"],
        "so did all four derived access rows, hers included"
    );

    // One command per row, owned rows before the owner, so a revert can
    // put the owner back first.
    let space = SpaceId("conformance".into());
    let g = log.store.get(&space, 0).expect("the group");
    let cmds = &g.group.commands;
    assert_eq!(cmds.len(), 7, "board, two cards, four access rows");
    assert!(cmds.iter().all(|c| c.kind == CommandKind::Delete));
    assert!(cmds.iter().all(|c| c.before.is_some() && c.after.is_none()));
    assert_eq!(
        cmds.last().unwrap().row_id,
        "board_1",
        "the owner goes last"
    );
}

#[test]
fn a_caller_who_cannot_delete_every_owned_row_deletes_none_of_them() {
    let (f, pg, mut log) = setup("ordo_owned_delete_guard");
    // Bram holds `write` on board_1: he may write the board and its cards,
    // and may not delete the access rows, which need `admin`.
    let e = f
        .run_write_as(&pg, &mut log, r#"dropBoard($b: "board_1")"#, BRAM)
        .expect_err("the access rows are owned rows he may not delete");
    assert!(e.starts_with("access_guard"), "{e}");
    assert!(!e.contains("ba_"), "the refusal names no row: {e}");
    assert_eq!(ids(&pg, "boards"), ["board_1", "board_2"]);
    assert_eq!(ids(&pg, "cards").len(), 3, "all or nothing");
    assert_eq!(ids(&pg, "boardsAccess").len(), 5);
}

#[test]
fn ownership_cascades_through_every_level() {
    let (f, pg, mut log) = setup("ordo_owned_delete_deep");
    // proj_1 owns projectMembers pm_1, pm_2 and tasks task_1, task_2;
    // task_1 owns note_1.
    let r = f
        .run_write_as(&pg, &mut log, r#"dropProject($p: "proj_1")"#, ANA)
        .expect("Ana owns proj_1 and may write everything under it");
    assert_eq!(r, json!([{ "entity": "projects", "deleted": 1 }]));
    assert_eq!(ids(&pg, "projects"), ["proj_2", "proj_3"]);
    assert_eq!(ids(&pg, "tasks"), ["task_3", "task_4", "task_5"]);
    assert_eq!(ids(&pg, "taskNotes"), ["note_2", "note_3"], "a grandchild");
    assert_eq!(ids(&pg, "projectMembers"), ["pm_3", "pm_4", "pm_5"]);
    let space = SpaceId("conformance".into());
    let g = log.store.get(&space, 0).expect("the group");
    assert_eq!(g.group.commands.len(), 6);
    assert_eq!(g.group.commands.last().unwrap().row_id, "proj_1");
}

#[test]
fn a_ranged_delete_takes_owned_rows_with_each_match() {
    let (f, pg, mut log) = setup("ordo_owned_delete_bulk");
    let r = f
        .run_write_as(&pg, &mut log, r#"dropBoardsNamed($name: "icebox")"#, BRAM)
        .expect("Bram administers board_2");
    assert_eq!(r, json!([{ "entity": "boards", "deleted": 1 }]));
    assert_eq!(ids(&pg, "boards"), ["board_1"]);
    assert_eq!(ids(&pg, "cards"), ["card_1", "card_2"]);
    assert_eq!(ids(&pg, "boardsAccess").len(), 4);
}

#[test]
fn a_plain_reference_still_refuses() {
    let (f, pg, mut log) = setup("ordo_owned_delete_plain");
    // orderItems.product is an association: a product with items refuses.
    let before = ids(&pg, "products");
    let e = f
        .run_write_as(&pg, &mut log, r#"dropProduct($p: "prod_1")"#, ANA)
        .expect_err("order items still point at the product");
    assert!(e.starts_with("has_referrers"), "{e}");
    assert_eq!(ids(&pg, "products"), before, "nothing deleted");
}

#[test]
fn an_owned_row_that_something_else_references_refuses_the_whole_delete() {
    let (f, pg, mut log) = setup("ordo_owned_delete_shared");
    let items_before = ids(&pg, "orderItems");
    let item = &items_before[0];
    let order = pg
        .query(&format!(
            "SELECT \"order\" FROM \"orderItems\" WHERE \"id\" = '{item}'"
        ))
        .unwrap()[0][0]
        .clone()
        .unwrap();
    pg.exec(&format!(
        "INSERT INTO \"itemReturns\" (\"$space\", \"id\", \"item\", \"note\") \
         VALUES ('conformance', 'ret_1', '{item}', 'damaged')"
    ))
    .expect("a plain referrer of an owned row");
    let orders_before = ids(&pg, "orders");
    let e = f
        .run_write_as(&pg, &mut log, &format!(r#"dropOrder($o: "{order}")"#), ANA)
        .expect_err("an item the order owns is referenced from outside");
    assert!(e.starts_with("has_referrers"), "{e}");
    assert!(e.contains("itemReturns.item"), "{e}");
    assert_eq!(ids(&pg, "orders"), orders_before, "the order stays");
    assert_eq!(ids(&pg, "orderItems"), items_before, "and so do its items");

    // Without the outside referrer the same delete lands and takes its items.
    pg.exec("DELETE FROM \"itemReturns\"").unwrap();
    f.run_write_as(&pg, &mut log, &format!(r#"dropOrder($o: "{order}")"#), ANA)
        .expect("nothing outside points in now");
    assert!(!ids(&pg, "orders").contains(&order));
    assert!(ids(&pg, "orderItems").len() < items_before.len());
}
