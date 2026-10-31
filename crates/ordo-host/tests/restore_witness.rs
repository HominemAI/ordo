//! **The test the `restore-witness` ruling asks for** (R2d-iii): a
//! point-in-time restore of an *attached space's whole database*, caught at
//! `Spaces::resolve` because the record it is caught by is in the control
//! database and the restore never touched it.
//!
//! This is the case the space's own tables cannot see. `$commandlog` and
//! `$checkpoints` both live on the target, so a restore of the target rolls
//! them back together, they agree with each other afterwards, and
//! `continuity` over the target alone reads `Intact`. The first assertion
//! below is exactly that reading, kept so the failure this table closes
//! stays visible rather than becoming folklore.
//!
//! The backup is a Postgres template copy, which is a real point-in-time
//! copy of a whole database and is the mechanism R3's drill is ruled to use:
//! a cargo test cannot run `pg_basebackup` and a WAL replay against the
//! shared cluster the suites use.

use ordo_host::provision::HostAllowlist;
use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Ed25519Signer, Image,
    RestoreSource, SpaceId as LogSpace, Timestamp,
};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::{AttachedPools, Installed, PoolResolver};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::undo::{self, Continuity, Db, RecordSource, RestoreRecord};
use ordo_exec::witness::Witness;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::provision::{prepare_target, Target, SYSTEM_ACTOR};
use ordo_host::spaces::{SpaceRefusal, Spaces};

const CONTROL: &str = "ordo_rw_control";
const TARGET: &str = "ordo_rw_target";
const SPACE: &str = "spc_attached";
const NOW: u64 = 1_700_000_000;

fn admin() -> Pg {
    Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
}

fn drop_database(name: &str) {
    let _ = admin().exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

async fn connect(db: &str, size: usize) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .unwrap_or_else(|e| panic!("{db}: {e}"))
}

fn attachment(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.into(),
        credential: "local".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: NOW as i64,
        attached_by: SYSTEM_ACTOR.into(),
    }
}

fn group(n: usize) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(format!("rw_cg_{n}")),
        actor: Actor::User("usr_rw".into()),
        commands: vec![Command {
            id: CommandId(format!("rw_cmd_{n}")),
            space: LogSpace(SPACE.into()),
            entity: "task".into(),
            row_id: format!("tsk_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(Image::new()),
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

async fn append_n(pool: &Pool, range: std::ops::Range<usize>) {
    for n in range {
        let tx = pool.begin().await.unwrap();
        tx.append_group(&group(n)).await.expect("append");
        tx.commit().await.unwrap();
    }
}

/// **A whole-database restore of an attached space is caught at resolve.**
///
/// **Observed failing first:** with the witness read removed from
/// `continuity`, `resolve` returns `Ok` on the restored target — which is
/// the first assertion of this test, made against an empty witness so that
/// it keeps being made.
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_database_restore_of_an_attached_space_is_caught_at_resolve() {
    // `target_db`/`backup_db` never go through `Pg::named`, so they are
    // namespaced explicitly, once, here (`test_db_name` is deterministic
    // within this process, so every later use below agrees on the same
    // name). `CONTROL` goes through `Pg::named` instead, which namespaces
    // it internally; `control_pg.db` is read back from that call rather
    // than re-namespaced.
    let target_db = ordo_conformance::test_db_name(TARGET);
    let backup_db = format!("{target_db}_backup");
    drop_database(&target_db);
    drop_database(&backup_db);

    // The control database, which holds `$spacedb` and `$chainwitness` and
    // is never restored in this test.
    let f = Arc::new(Fixture::load().expect("fixture"));
    let control_pg = Pg::scoped(CONTROL);
    f.reset(&control_pg).expect("seeded");
    let control = connect(&control_pg.db, 4).await;
    spacedb::ensure_table(&control).await.expect("$spacedb");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    let witness = Witness::new(control.clone());
    witness.ensure_table().await.expect("$chainwitness");
    control
        .rows(&format!(
            "DELETE FROM \"$chainwitness\" WHERE \"space\" = '{SPACE}'"
        ))
        .await
        .unwrap();

    // The space, provisioned onto a target database of its own.
    let credentials = Installed::new().as_configured("local");
    let template = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    // R-S1: `prepare_target` reads the schema from the deployment that is
    // serving, so this test stands one up rather than handing it a DDL.
    let serving = ordo_host::deploy::Deployment::pinned(
        "restore-witness-v1",
        f.clone(),
        ordo_exec::Executor {
            pool: control.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );
    prepare_target(
        &control,
        f.keys.as_ref(),
        &serving,
        SPACE,
        &Target {
            attachment: &attachment(&target_db),
            credentials: &credentials,
            template: &template,
            hosts: &HostAllowlist::of(&PoolConfig::from_env().host),
        },
        None,
        NOW,
    )
    .await
    .expect("the target is prepared");

    let space = LogSpace(SPACE.into());
    let signer = Ed25519Signer::from_seed("rw-key", &[83u8; 32]);

    // N groups on the target, and a checkpoint over them — witnessed on the
    // control pool, which is the whole point.
    let target = connect(&target_db, 2).await;
    append_n(&target, 0..4).await;
    undo::sign_checkpoint(&target, &space, &signer, Timestamp(400), &witness)
        .await
        .expect("the first checkpoint");

    // **The backup**: a template copy, which needs no other connection to
    // the source, so the pool is dropped first.
    drop(target);
    tokio::time::sleep(Duration::from_millis(200)).await;
    admin()
        .exec(&format!("CREATE DATABASE {backup_db} TEMPLATE {target_db}"))
        .expect("the template copy");

    // M more groups, and a second checkpoint, after the backup was taken.
    let target = connect(&target_db, 2).await;
    append_n(&target, 4..8).await;
    let undo::Checkpointed::Signed(cp8) =
        undo::sign_checkpoint(&target, &space, &signer, Timestamp(800), &witness)
            .await
            .expect("the second checkpoint")
    else {
        panic!("expected a fresh signature")
    };
    assert_eq!(cp8.upto, 8, "N + M");

    // **The restore**: the target is dropped and copied back from the
    // backup, under its own name. Everything the second checkpoint covered
    // is gone, including the checkpoint row itself.
    drop(target);
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop_database(&target_db);
    admin()
        .exec(&format!("CREATE DATABASE {target_db} TEMPLATE {backup_db}"))
        .expect("the restore");
    let target = connect(&target_db, 2).await;

    // **The failure this table exists to close**, asserted so it stays
    // visible: over the target alone, the restore is invisible. The chain
    // stops at 4 and the only checkpoint left says 4, so they agree.
    let blind = Witness::new(target.clone());
    blind.ensure_table().await.unwrap();
    target.rows("DELETE FROM \"$chainwitness\"").await.unwrap();
    assert_eq!(
        undo::continuity(Db::Pool(&target), &blind, &space)
            .await
            .unwrap(),
        Continuity::Intact,
        "the space's own tables were restored together, so they agree"
    );

    // The resolver, routed the way a deployment with attached spaces is.
    let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        template.clone(),
        Arc::new(credentials),
    ));
    let deployment = ordo_host::deploy::Deployment::empty(
        ordo_crypto::SpaceId(SPACE.into()),
        control.clone(),
        f.keys.clone(),
    )
    .expect("an empty deployment");
    let spaces = Spaces::routed(deployment, pools, witness.clone());

    let Err(refused) = spaces.resolve(SPACE).await else {
        panic!("a restored target resolved: the witness was not read");
    };
    let SpaceRefusal::Rewound { continuity, .. } = &refused else {
        panic!("expected Rewound, got {refused}")
    };
    assert!(
        matches!(
            continuity,
            Continuity::Rewound {
                checkpoint_upto: 8,
                source: RecordSource::Witness,
                ..
            }
        ),
        "{continuity:?}"
    );

    // Recorded, the space serves again — and the marker attests the head the
    // witness holds, without this test naming it.
    let r = undo::record_restore(
        &target,
        &witness,
        &space,
        RestoreRecord {
            operator: "ops@example.test".into(),
            source: RestoreSource::Pitr { at: Timestamp(NOW) },
            pre_restore_head: None,
            again: false,
            restored_from: None,
        },
    )
    .await
    .expect("recorded");
    assert!(r.witnessed);
    assert_eq!(r.defaulted, Some(undo::PreHeadSource::Witness));
    let (_, marker) = undo::latest_restore_marker(Db::Pool(&target), &space)
        .await
        .unwrap()
        .expect("a marker");
    assert_eq!(
        marker.pre_restore_head,
        Some((cp8.upto - 1, cp8.head)),
        "the second checkpoint's head, which only the control database still holds"
    );

    spaces
        .resolve(SPACE)
        .await
        .expect("a recorded restore is served");

    drop(target);
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop_database(&backup_db);
}
