//! Owner ruling `owned-delete` (2026-09-29): the revert of a delete that
//! cascaded puts back every row it took, the owner first and the rows it
//! owned after, sealed fields included.
//!
//! And the other direction (checklist 25h): the revert of a create whose
//! row has since gained owned rows takes them with it, recorded one
//! command per row like the forward cascade, owned rows before the owner.
//! A plain referrer still refuses the revert, and a reverter who may not
//! delete every owned row reverts none of it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_commandlog::{CommandKind, CorrelationId, SpaceId, Timestamp};
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_exec::{undo, Executor, LoggedExecutor, Pool, PoolConfig};
use ordo_schema::Schema;

/// The market fixture plus an owned child with a sealed field, and the
/// two deletes these tests run.
const EXTRA: &str = r#"
entity orderNotes (key: onote) {
  order  -> orders  owned
  secret: text  sealed
}

op addOrder($customer: id, $total: money) =
  set orders { customer: $customer, placed: 2026-05-01, total: $total, memo: "gift" }

op addOrderItem($order: id, $product: id) =
  set orderItems { order: $order, product: $product, qty: 1, amount: 100 }

op addOrderNote($order: id, $secret: text) =
  set orderNotes { order: $order, secret: $secret }

op dropOrder($o: id) =
  del orders { id: $o }

op dropBoard($b: id) =
  del boards { id: $b }

op addProduct($name: text, $vendor: id, $category: id) =
  set products { vendor: $vendor, category: $category, name: $name, price: 100 }

op newProject($name: text, $owner: id) =
  set projects { name: $name, owner: $owner }

op joinProject($project: id, $member: id) =
  set projectMembers { project: $project, member: $member, canWrite: true }
"#;

struct Env {
    pool: Pool,
    fixture: Arc<Fixture>,
    schema: Schema,
    log: Arc<Mutex<WriteLog>>,
}

async fn env(db: &str) -> Env {
    let src = format!("{}\n{EXTRA}", ordo_conformance::FIXTURE);
    let f = Fixture::from_source(&src, ordo_conformance::SEED).expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    pool.rows("DELETE FROM \"$checkpoints\"").await.unwrap();
    let schema = f.schema.clone();
    Env {
        pool,
        fixture: Arc::new(f),
        schema,
        log: Arc::new(Mutex::new(WriteLog::new())),
    }
}

fn space() -> SpaceId {
    SpaceId("conformance".into())
}

/// A second handle on the same environment, for a revert run on its own
/// task while the test holds a transaction open against it.
fn env_handle(e: &Env) -> Env {
    Env {
        pool: e.pool.clone(),
        fixture: e.fixture.clone(),
        schema: e.schema.clone(),
        log: e.log.clone(),
    }
}

impl Env {
    fn executor(&self) -> LoggedExecutor {
        LoggedExecutor::new(
            Executor {
                pool: self.pool.clone(),
                schema: self.fixture.schema.clone(),
                registry: self.fixture.registry.clone(),
                keys: self.fixture.keys.clone(),
                space: ordo_conformance::fixture_space(),
            },
            "conformance",
        )
    }

    async fn write(&self, call: &str, caller: &str) {
        self.executor()
            .write_as(
                self.fixture.clone(),
                self.log.clone(),
                call.to_string(),
                caller.to_string(),
            )
            .await
            .unwrap_or_else(|e| panic!("write `{call}`: {e}"));
    }

    async fn group(&self, idx: usize) -> ordo_commandlog::CommandGroup {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = 'conformance' \
                 AND \"idx\" = {idx}"
            ))
            .await
            .unwrap();
        let hex = rows[0][0].clone().expect("a group at this index");
        let body = ordo_crypto::sealed::unhex(&hex).expect("a body");
        ordo_commandlog::chain::decode_group(&body).expect("decodes")
    }

    async fn correlation(&self, idx: usize) -> CorrelationId {
        CorrelationId(self.group(idx).await.correlation.0)
    }

    async fn one(&self, sql: &str) -> Option<String> {
        self.pool.rows(sql).await.unwrap().first()?.first()?.clone()
    }

    async fn count(&self, sql: &str) -> i64 {
        self.one(sql)
            .await
            .expect("a count")
            .parse()
            .expect("an int")
    }

    async fn revert(
        &self,
        idx: usize,
        principal: &str,
    ) -> Result<undo::Reverted, undo::RevertFailure> {
        undo::revert(
            &self.pool,
            &self.schema,
            Some(self.fixture.keys.as_ref()),
            None,
            None,
            &space(),
            &self.correlation(idx).await,
            principal,
            Timestamp(1),
        )
        .await
    }
}

/// A delete that cascaded is one group of per-row commands, and reverting
/// it restores all of them, an owned row with a sealed field included.
#[tokio::test(flavor = "multi_thread")]
async fn reverting_a_cascading_delete_restores_every_row() {
    let e = env("ordo_owned_delete_revert").await;
    e.write(r#"addOrder($customer: "cust_1", $total: 300)"#, "mem_1")
        .await;
    let order = e.group(0).await.commands[0].row_id.clone();
    for p in ["prod_1", "prod_2"] {
        e.write(
            &format!(r#"addOrderItem($order: "{order}", $product: "{p}")"#),
            "mem_1",
        )
        .await;
    }
    e.write(
        &format!(r#"addOrderNote($order: "{order}", $secret: "keep this")"#),
        "mem_1",
    )
    .await;
    let note = e.group(3).await.commands[0].row_id.clone();

    e.write(&format!(r#"dropOrder($o: "{order}")"#), "mem_1")
        .await;
    let g = e.group(4).await;
    assert_eq!(
        g.commands.len(),
        4,
        "the order, its two items and its sealed note"
    );
    assert_eq!(g.commands.last().unwrap().row_id, order);
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"orders\" WHERE \"id\" = '{order}'"
        ))
        .await,
        0
    );
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"orderItems\" WHERE \"order\" = '{order}'"
        ))
        .await,
        0
    );
    assert_eq!(e.count("SELECT count(*) FROM \"orderNotes\"").await, 0);

    e.revert(4, "mem_1")
        .await
        .expect("the revert restores the order and everything it owned");

    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"orders\" WHERE \"id\" = '{order}'"
        ))
        .await,
        1
    );
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"orderItems\" WHERE \"order\" = '{order}'"
        ))
        .await,
        2
    );
    let restored = e
        .one(&format!(
            "SELECT encode(\"secret\", 'hex') FROM \"orderNotes\" WHERE \"id\" = '{note}'"
        ))
        .await
        .expect("the sealed note is back");
    assert!(!restored.is_empty(), "with its sealed value");
}

/// The same for a row with derived access rows.
#[tokio::test(flavor = "multi_thread")]
async fn reverting_a_board_delete_restores_its_access_rows() {
    let e = env("ordo_owned_delete_revert_acl").await;
    e.write(r#"dropBoard($b: "board_1")"#, "mem_1").await;
    assert_eq!(e.count("SELECT count(*) FROM \"boardsAccess\"").await, 1);
    e.revert(0, "mem_1")
        .await
        .expect("the revert restores the board, its cards and its access rows");
    assert_eq!(e.count("SELECT count(*) FROM \"boards\"").await, 2);
    assert_eq!(e.count("SELECT count(*) FROM \"cards\"").await, 3);
    assert_eq!(e.count("SELECT count(*) FROM \"boardsAccess\"").await, 5);
}

/// 25h. A create is reverted after its row gained owned rows: the revert
/// takes them with it, one `Delete` command each with its before-image,
/// owned rows before the owner, so the revert can itself be reverted.
#[tokio::test(flavor = "multi_thread")]
async fn reverting_a_create_takes_the_rows_it_has_since_gained() {
    let e = env("ordo_owned_create_revert").await;
    e.write(r#"addOrder($customer: "cust_1", $total: 300)"#, "mem_1")
        .await;
    let order = e.group(0).await.commands[0].row_id.clone();
    for p in ["prod_1", "prod_2"] {
        e.write(
            &format!(r#"addOrderItem($order: "{order}", $product: "{p}")"#),
            "mem_1",
        )
        .await;
    }
    e.write(
        &format!(r#"addOrderNote($order: "{order}", $secret: "keep this")"#),
        "mem_1",
    )
    .await;
    let items_before = e.count("SELECT count(*) FROM \"orderItems\"").await;

    let r = e
        .revert(0, "mem_1")
        .await
        .expect("the order goes, and so do the items and the note it gained");

    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"orders\" WHERE \"id\" = '{order}'"
        ))
        .await,
        0
    );
    assert_eq!(
        e.count("SELECT count(*) FROM \"orderItems\"").await,
        items_before - 2,
        "its two items went with it and nobody else's did"
    );
    assert_eq!(e.count("SELECT count(*) FROM \"orderNotes\"").await, 0);

    let cmds = &r.group.commands;
    assert_eq!(cmds.len(), 4, "the order, two items, one note");
    assert!(cmds.iter().all(|c| c.kind == CommandKind::Delete));
    assert!(cmds.iter().all(|c| c.before.is_some() && c.after.is_none()));
    assert_eq!(cmds.last().unwrap().row_id, order, "the owner goes last");
    assert!(
        cmds.last().unwrap().id.0.starts_with("rev-"),
        "the owner's command names the create it undoes: {}",
        cmds.last().unwrap().id.0
    );
    for c in &cmds[..3] {
        assert!(
            c.id.0.contains("-owned-"),
            "an owned row names the undo that took it: {}",
            c.id.0
        );
    }
    let note = cmds.iter().find(|c| c.entity == "orderNotes").unwrap();
    assert!(
        note.before.as_ref().unwrap().contains_key("secret"),
        "the sealed field is imaged, so the revert can be reverted"
    );

    // And it can: reverting the revert puts everything back.
    e.revert(4, "mem_1")
        .await
        .expect("the revert's own group is a cascade of deletes, which restores");
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"orders\" WHERE \"id\" = '{order}'"
        ))
        .await,
        1
    );
    assert_eq!(
        e.count("SELECT count(*) FROM \"orderItems\"").await,
        items_before
    );
    assert_eq!(e.count("SELECT count(*) FROM \"orderNotes\"").await, 1);
}

/// An owned row re-parented while the revert runs stays with its new owner.
///
/// The cascade finds the owned rows, then locks and deletes them. Another
/// transaction that moves one to a different owner in between must win.
/// What makes it win is the revert's isolation: the whole revert is one
/// SERIALIZABLE transaction (`undo.rs` §6), so the lock on a row that
/// changed under it is a serialization fault, and the caller's retry sees
/// the row with its new owner and leaves it. Pinned here so a move to a
/// weaker isolation, where a plain read followed by a lock would keep the
/// stale owner, cannot pass unnoticed.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_re_parented_during_the_revert_is_not_taken() {
    let e = env("ordo_owned_reparent_revert").await;
    e.write(r#"addOrder($customer: "cust_1", $total: 300)"#, "mem_1")
        .await;
    e.write(r#"addOrder($customer: "cust_1", $total: 400)"#, "mem_1")
        .await;
    let first = e.group(0).await.commands[0].row_id.clone();
    let second = e.group(1).await.commands[0].row_id.clone();
    e.write(
        &format!(r#"addOrderItem($order: "{first}", $product: "prod_1")"#),
        "mem_1",
    )
    .await;
    let item = e.group(2).await.commands[0].row_id.clone();

    // A transaction that moves the item to the second order holds its row
    // lock, uncommitted, while the revert of the first order begins.
    let tx = e.pool.begin().await.expect("a transaction");
    tx.rows(&format!(
        "UPDATE \"orderItems\" SET \"order\" = '{second}' WHERE \"id\" = '{item}'"
    ))
    .await
    .expect("re-parented, uncommitted");

    let reverting = {
        let e2 = env_handle(&e);
        tokio::spawn(async move { e2.revert(0, "mem_1").await })
    };
    // Give the revert time to reach the item and block on the lock.
    tokio::time::sleep(Duration::from_millis(300)).await;
    tx.commit().await.expect("the re-parenting commits first");

    // The first attempt met the concurrent update: a serialization fault,
    // never a delete of the moved row. The retry runs on a fresh snapshot.
    let r = match reverting.await.expect("the revert task runs") {
        Ok(r) => r,
        Err(undo::RevertFailure::Store(ordo_exec::PoolError::Serialization(_))) => {
            assert_eq!(
                e.count(&format!(
                    "SELECT count(*) FROM \"orderItems\" WHERE \"id\" = '{item}'"
                ))
                .await,
                1,
                "a serialization fault leaves everything in place"
            );
            e.revert(0, "mem_1")
                .await
                .expect("retried on a fresh snapshot, the first order owns nothing any more")
        }
        Err(other) => panic!("the revert must retry or succeed, not refuse: {other:?}"),
    };
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"orders\" WHERE \"id\" = '{first}'"
        ))
        .await,
        0
    );
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"orderItems\" WHERE \"id\" = '{item}' \
             AND \"order\" = '{second}'"
        ))
        .await,
        1,
        "the item belongs to the second order now and must survive"
    );
    assert_eq!(
        r.group.commands.len(),
        1,
        "only the order itself was deleted: {:?}",
        r.group
            .commands
            .iter()
            .map(|c| (&c.entity, &c.row_id))
            .collect::<Vec<_>>()
    );
}

/// The same for a row with derived access rows: the board's own create
/// group carries the creator's admin row, and the card it gained later is
/// cascaded.
#[tokio::test(flavor = "multi_thread")]
async fn reverting_a_board_create_takes_its_cards_and_access_rows() {
    let e = env("ordo_owned_create_revert_acl").await;
    e.write(r#"addBoard($name: "scratch")"#, "mem_1").await;
    let board = e
        .group(0)
        .await
        .commands
        .iter()
        .find(|c| c.entity == "boards")
        .expect("the board")
        .row_id
        .clone();
    e.write(
        &format!(r#"addCard($board: "{board}", $title: "first")"#),
        "mem_1",
    )
    .await;
    assert_eq!(e.count("SELECT count(*) FROM \"boards\"").await, 3);
    assert_eq!(e.count("SELECT count(*) FROM \"cards\"").await, 4);
    assert_eq!(e.count("SELECT count(*) FROM \"boardsAccess\"").await, 6);

    let r = e
        .revert(0, "mem_1")
        .await
        .expect("the creator administers the board, so she may undo it with what it owns");
    assert_eq!(e.count("SELECT count(*) FROM \"boards\"").await, 2);
    assert_eq!(e.count("SELECT count(*) FROM \"cards\"").await, 3);
    assert_eq!(e.count("SELECT count(*) FROM \"boardsAccess\"").await, 5);
    let cmds = &r.group.commands;
    assert_eq!(cmds.len(), 3, "the access row, the card, the board");
    assert_eq!(cmds.last().unwrap().row_id, board);
    assert_eq!(cmds[1].entity, "cards");
}

/// A plain reference still refuses, as it refuses a forward delete: the
/// revert names the referrer and deletes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_referrer_still_refuses_the_revert_of_a_create() {
    let e = env("ordo_owned_create_revert_plain").await;
    e.write(
        r#"addProduct($name: "widget", $vendor: "vend_1", $category: "cat_1")"#,
        "mem_1",
    )
    .await;
    let product = e.group(0).await.commands[0].row_id.clone();
    e.write(
        &format!(r#"addOrderItem($order: "ord_1", $product: "{product}")"#),
        "mem_1",
    )
    .await;
    let err = e
        .revert(0, "mem_1")
        .await
        .expect_err("an order item points at the product through a plain ref");
    let msg = err.to_string();
    assert!(msg.starts_with("has_referrers"), "{msg}");
    assert!(msg.contains("orderItems.product"), "{msg}");
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"products\" WHERE \"id\" = '{product}'"
        ))
        .await,
        1,
        "nothing deleted"
    );
}

/// The reverter must be allowed to delete every owned row, and the
/// cascade is all or nothing: Ana owns the project and may delete it and
/// its member rows, and may not delete the task Bram added, because only a
/// member with `canWrite` writes tasks. The refusal counts and never names.
#[tokio::test(flavor = "multi_thread")]
async fn a_reverter_who_may_not_delete_an_owned_row_reverts_none_of_it() {
    let e = env("ordo_owned_create_revert_guard").await;
    e.write(r#"newProject($name: "Delta", $owner: "mem_1")"#, "mem_1")
        .await;
    let project = e.group(0).await.commands[0].row_id.clone();
    e.write(
        &format!(r#"joinProject($project: "{project}", $member: "mem_2")"#),
        "mem_1",
    )
    .await;
    e.write(
        &format!(r#"addTask($project: "{project}", $title: "plan")"#),
        "mem_2",
    )
    .await;
    let task = e.group(2).await.commands[0].row_id.clone();

    let err = e
        .revert(0, "mem_1")
        .await
        .expect_err("the task is an owned row Ana may not delete");
    let msg = err.to_string();
    assert!(msg.starts_with("access_guard"), "{msg}");
    assert!(msg.contains("1 row you may not delete"), "{msg}");
    assert!(!msg.contains(&task), "the refusal names no row: {msg}");
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"projects\" WHERE \"id\" = '{project}'"
        ))
        .await,
        1,
        "all or nothing"
    );
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"projectMembers\" WHERE \"project\" = '{project}'"
        ))
        .await,
        1
    );
    assert_eq!(
        e.count(&format!(
            "SELECT count(*) FROM \"tasks\" WHERE \"id\" = '{task}'"
        ))
        .await,
        1
    );
}
