//! C1 of `plan-client-cache.md`: the transaction a position-reporting read
//! runs in holds one snapshot across its statements.
//!
//! The executor reads a page and then the space's command-log position in one
//! `OpClass::SnapshotRead` transaction. This pins the property that makes that
//! sound: a write committed after the transaction began is invisible to its
//! second statement, where under `OpClass::Read` it would not be.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Image, SpaceId, Timestamp,
    Value,
};
use ordo_conformance::Pg;
use ordo_exec::{OpClass, Pool, PoolConfig, TxConn};

const SPACE: &str = "spc_snapshot";

async fn pool(db: &str) -> Pool {
    let _ = Pg::named(db);
    Pool::connect(PoolConfig {
        db: db.into(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot")
}

fn group(n: usize) -> CommandGroup {
    let after: Image = [("id".to_string(), Value::Id(format!("row_{n}")))]
        .into_iter()
        .collect();
    CommandGroup {
        correlation: CorrelationId(format!("snap_{n}")),
        actor: Actor::User("usr_1".into()),
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: SpaceId(SPACE.into()),
            entity: "rows".into(),
            row_id: format!("row_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(after),
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

async fn append(p: &Pool, n: usize) {
    let tx = p.begin().await.unwrap();
    tx.append_group(&group(n)).await.expect("append");
    tx.commit().await.expect("commit");
}

async fn position(tx: &TxConn) -> u64 {
    tx.rows(&format!(
        "SELECT COALESCE(max(\"idx\") + 1, 0) FROM \"$commandlog\" WHERE \"space\" = '{SPACE}'"
    ))
    .await
    .unwrap()[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_read_does_not_see_a_write_committed_after_it_began() {
    let p = pool("ordo_snapshot_read").await;
    p.rows(&format!(
        "DELETE FROM \"$commandlog\" WHERE \"space\" = '{SPACE}'"
    ))
    .await
    .unwrap();
    append(&p, 0).await;

    let read = p.begin_for(OpClass::SnapshotRead).await.unwrap();
    assert_eq!(position(&read).await, 1);
    append(&p, 1).await;
    assert_eq!(
        position(&read).await,
        1,
        "the second statement reads the first statement's snapshot"
    );
    read.commit().await.unwrap();

    let after = p.begin_for(OpClass::SnapshotRead).await.unwrap();
    assert_eq!(
        position(&after).await,
        2,
        "a new transaction sees the write"
    );
    after.commit().await.unwrap();

    // Under the class reads used before C1, the second statement does see
    // it, which is why a position-reporting read needs a class of its own.
    let rc = p.begin_for(OpClass::Read).await.unwrap();
    let before = position(&rc).await;
    append(&p, 2).await;
    assert_eq!(
        position(&rc).await,
        before + 1,
        "READ COMMITTED takes a new snapshot for every statement"
    );
    rc.commit().await.unwrap();
}
