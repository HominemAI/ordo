//! `ordo restore --record --tenant` across versions (`plan-versions.md`
//! §2.9, §4 row 17).
//!
//! A tenant's own database is restored from a snapshot taken on 0.1.0
//! while the deployment serves 0.2.0. The restore record reads the version
//! the restored database is at, and the tenant's pin follows it, cause
//! `restore`. A restored database at a version this deployment does not
//! store is refused before the marker, naming the version.
//!
//! The tenant's database is reached with the deployment's own credential
//! (`$deployment`), as the door reaches it, with no credentials directory.
//!
//! A regression looks like a marker written with the pin left ahead of
//! data it could serve, or a pin moved to a version that is not stored.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::process::Output;

mod support;
use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Image, SpaceId, Timestamp,
    Value,
};
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::undo::{self, Db};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::versions::Versions;
use ordo_host::{apply_op, tenants};

const PROGRAM: &str = "domain notes @ 0.1.0\n\n\
    entity notes {\n  title: text\n}\n\n\
    op allNotes unbounded =\n  notes[title]\n";

async fn pool_on(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool")
}

fn group(space: &str, n: usize) -> CommandGroup {
    let after: Image = [
        ("id".to_string(), Value::Id(format!("not_{n}"))),
        ("rev".to_string(), Value::Int(1)),
        ("title".to_string(), Value::Text(format!("note {n}"))),
    ]
    .into_iter()
    .collect();
    CommandGroup {
        correlation: CorrelationId(format!("cg_{n}")),
        actor: Actor::User(format!("usr_{n}")),
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: SpaceId(space.to_string()),
            entity: "notes".into(),
            row_id: format!("not_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(after),
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        backfill: None,
        restored_unverified: false,
        restore: None,
    }
}

/// A control database storing 0.1.0 and 0.2.0 (active), a tenant pinned to
/// 0.2.0, and its own database: a short chain, and `$migrations` saying
/// the restored data is at `data`.
async fn restored(stem: &str, data: &str) -> (Pool, Pg, Pg, String) {
    let control_pg = Pg::scoped(&format!("{stem}_control"));
    let target_pg = Pg::scoped(&format!("{stem}_target"));
    let control = pool_on(&control_pg.db).await;
    tenants::ensure_table(&control).await.unwrap();
    for source in [PROGRAM.to_string(), PROGRAM.replace("@ 0.1.0", "@ 0.2.0")] {
        apply_op::run(
            &control,
            &SinglePool::new(control.clone()),
            &source,
            &apply_op::CodeFiles::new(),
            "p_ada",
            "t",
            true,
        )
        .await
        .expect("applied");
    }
    let id = tenants::mint_space_id();
    tenants::reserve(&control, &id, "acme", "p_ada", 100, None)
        .await
        .unwrap();
    Versions::on(&control)
        .pin(&id, "0.2.0", "p_ada", 100)
        .await
        .unwrap();
    tenants::activate(&control, &id, 100).await.unwrap();

    let target = pool_on(&target_pg.db).await;
    for n in 0..3 {
        let tx = target.begin().await.unwrap();
        tx.append_group(&group(&id, n)).await.expect("append");
        tx.commit().await.unwrap();
    }
    target
        .rows(ordo_exec::migrate::MIGRATIONS_DDL)
        .await
        .unwrap();
    target
        .rows(&format!(
            "INSERT INTO \"$migrations\" (\"kind\", \"from_version\", \"to_version\", \
             \"actor\", \"at\", \"changes\", \"ddl\", \"phase\", \"note\") VALUES \
             ('create', NULL, '{data}', 'p_ada', 1, '', '', 'applied', 'the snapshot')"
        ))
        .await
        .unwrap();
    drop(target);

    let env = PoolConfig::from_env();
    ordo_exec::spacedb::ensure_table(&control).await.unwrap();
    ordo_exec::spacedb::attach(
        &control,
        &ordo_crypto::SpaceId(id.clone()),
        &ordo_exec::spacedb::DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: target_pg.db.clone(),
            credential: ordo_exec::route::OWN_CREDENTIAL.into(),
            custody: ordo_exec::spacedb::DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "p_ada".into(),
        },
    )
    .await
    .expect("attached");
    (control, control_pg, target_pg, id)
}

fn record(control_db: &str, space: &str) -> Output {
    let pg = Pg::default();
    support::ordo()
        .args([
            "restore",
            "--record",
            "--direct",
            "--tenant",
            "--space",
            space,
            "--control-db",
            control_db,
            "--as",
            "ops@example.test",
            "--source",
            "pitr",
            "--at",
            "1757000000",
        ])
        .env("ORDO_PG_HOST", &pg.host)
        .env("ORDO_PG_PORT", &pg.port)
        .env("ORDO_PG_USER", &pg.user)
        .env_remove("ORDO_TENANT_CREDENTIALS")
        .env_remove("ORDO_TENANT_CREDENTIALS_DIR")
        .env_remove("ORDO_CHECKPOINT_KEY_ID")
        .env_remove("ORDO_CHECKPOINT_PUBKEY")
        .output()
        .expect("run ordo")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_database_on_an_older_version_moves_the_pin_to_it() {
    let (control, control_pg, target_pg, id) = restored("ordo_rv_cli1", "0.1.0").await;
    let out = record(&control_pg.db, &id);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    assert!(
        stdout.contains("version PINNED 0.2.0 -> 0.1.0 · cause restore"),
        "{stdout}"
    );
    assert!(stdout.contains("ordo versions pin"), "{stdout}");
    let pin = Versions::on(&control).pin_of(&id).await.unwrap().unwrap();
    assert_eq!(pin.version.as_deref(), Some("0.1.0"));
    let last = Versions::on(&control)
        .pin_history(&id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        (last.cause.as_str(), last.actor.as_str()),
        ("restore", "ops@example.test")
    );
    // The marker is in the tenant's own database.
    let target = pool_on(&target_pg.db).await;
    let markers = undo::restore_markers(Db::Pool(&target), &SpaceId(id.clone()))
        .await
        .unwrap();
    assert_eq!(markers.len(), 1, "{markers:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_database_on_a_version_not_stored_is_refused_before_the_marker() {
    let (control, control_pg, target_pg, id) = restored("ordo_rv_cli2", "0.1.7").await;
    let out = record(&control_pg.db, &id);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "{stdout}{stderr}");
    let all = format!("{stdout}{stderr}");
    assert!(all.contains("REFUSED"), "{all}");
    assert!(all.contains("0.1.7"), "{all}");
    assert!(all.contains("ordo promote 0.1.7"), "{all}");
    assert!(all.contains("Nothing was recorded"), "{all}");
    let pin = Versions::on(&control).pin_of(&id).await.unwrap().unwrap();
    assert_eq!(pin.version.as_deref(), Some("0.2.0"));
    let target = pool_on(&target_pg.db).await;
    let markers = undo::restore_markers(Db::Pool(&target), &SpaceId(id.clone()))
        .await
        .unwrap();
    assert!(markers.is_empty(), "no marker: {markers:?}");
}
