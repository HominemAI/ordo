//! S5 of `plan-space-routing.md`, up to the founding member: giving a new
//! space a database of its own.
//!
//! The founding member is not here because where a space's members live
//! under routing is open (`membership-home`). Everything before it is:
//! custody, the database, the schema, the attachment and its record.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_host::provision::HostAllowlist;
use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::Timestamp;
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::SpaceId;
use ordo_exec::route::Installed;
use ordo_exec::spacedb::{self, AttachRefusal, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::deploy::Deployment;
use ordo_host::provision::{prepare_target, Prepared, ProvisionRefusal, Target, SYSTEM_ACTOR};
use ordo_secevent::{EventKind, Recorder, Scope};

const NOW: u64 = 1_700_000_000;

/// The version the deployment these tests stand up is serving. Named here
/// because it is the value a fresh target must record, and a test that spelt
/// it twice could pass on a typo.
const SERVING: &str = "provision-target-v1";

/// The control database, the fixture, and **a deployment serving it**.
/// Since R-S1 that deployment is where `prepare_target` reads the schema
/// from, so a test that wants a target prepared has to stand one up.
async fn control(db: &str) -> (Pool, Arc<Fixture>, Arc<Deployment>) {
    control_sized(db, 2).await
}

/// The same, with room for a test that holds a connection of its own.
///
/// Since R-S5 an attach takes `ApplyLock`, which holds a **pooled**
/// connection for the whole of `prepare_target` while the rest of that
/// function reads the control database on another. So an attach needs two
/// control connections at once, and a test that also wants to read while an
/// attach is in flight needs three.
async fn control_sized(db: &str, size: usize) -> (Pool, Arc<Fixture>, Arc<Deployment>) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::scoped(db);
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        // The namespaced name `Pg::named` actually created, never the
        // bare stem: they stopped being the same string when test
        // databases gained a per-run suffix, and a pool built on the stem
        // would open a *different* database from the one the fixture
        // seeded.
        db: pg.db,
        size,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("control");
    spacedb::ensure_table(&pool).await.expect("table");
    pool.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    let deployment = serving(SERVING, &f, &pool);
    (pool, f, deployment)
}

/// A deployment pinned to one version over one fixture: the smallest thing
/// that answers `current()`, which is all `prepare_target` asks of it.
fn serving(version: &str, f: &Arc<Fixture>, pool: &Pool) -> Arc<Deployment> {
    Deployment::pinned(
        version,
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    )
}

/// This test's target databases start absent, so "created" means created.
fn drop_database(name: &str) {
    let _ = Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
    .exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

async fn database_exists(pool: &Pool, name: &str) -> bool {
    !pool
        .rows(&format!(
            "SELECT 1 FROM pg_database WHERE datname = '{name}'"
        ))
        .await
        .unwrap()
        .is_empty()
}

fn on(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.into(),
        credential: "local".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 0,
        attached_by: "whoever asked".into(),
    }
}

async fn prepare(
    control: &Pool,
    f: &Fixture,
    serving: &Deployment,
    space: &str,
    db: &str,
    stream: &Recorder,
) -> Result<Prepared, ProvisionRefusal> {
    let attachment = on(db);
    let credentials = Installed::new().as_configured("local");
    let template = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    prepare_target(
        control,
        f.keys.as_ref(),
        serving,
        space,
        &Target {
            attachment: &attachment,
            credentials: &credentials,
            template: &template,
            hosts: &HostAllowlist::of(&PoolConfig::from_env().host),
        },
        Some(stream),
        NOW,
    )
    .await
}

/// `prepare`, with the two things 18d's refusals turn on made explicit:
/// the custody the attachment declares, and the hosts the deployment
/// permits. Kept separate from `prepare` so the twenty tests written before
/// this increment keep asking exactly the question they asked.
#[allow(clippy::too_many_arguments)]
async fn prepare_as(
    control: &Pool,
    f: &Fixture,
    serving: &Deployment,
    space: &str,
    db: &str,
    custody: DbCustody,
    hosts: &HostAllowlist,
    stream: &Recorder,
) -> Result<Prepared, ProvisionRefusal> {
    let attachment = DbAttachment { custody, ..on(db) };
    let credentials = Installed::new().as_configured("local");
    let template = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    prepare_target(
        control,
        f.keys.as_ref(),
        serving,
        space,
        &Target {
            attachment: &attachment,
            credentials: &credentials,
            template: &template,
            hosts,
        },
        Some(stream),
        NOW,
    )
    .await
}

/// The allowlist every test but the allowlist's own uses: the one host the
/// conformance cluster is actually on.
fn allowing_the_test_cluster() -> HostAllowlist {
    HostAllowlist::of(&PoolConfig::from_env().host)
}

/// What the target's own `$migrations` says it is.
async fn recorded_version(db: &str) -> spacedb::TargetVersion {
    spacedb::recorded_version(&target_pool(db).await)
        .await
        .expect("the target answers about its version")
}

async fn target_pool(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 1,
        ..PoolConfig::from_env()
    })
    .await
    .expect("target")
}

async fn has_table(pool: &Pool, table: &str) -> bool {
    pool.rows(&format!(
        "SELECT to_regclass('public.\"{table}\"') IS NOT NULL"
    ))
    .await
    .unwrap()[0][0]
        .as_deref()
        == Some("t")
}

/// Custody events the stream received, as (scope, about, action).
fn custody_events(stream: &Recorder) -> Vec<(Scope, Option<String>, String)> {
    stream
        .drain_all(Timestamp(NOW + 3_600))
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::CustodyAction { action } => Some((e.scope, e.about.map(|a| a.0), action)),
            _ => None,
        })
        .collect()
}

/// **Custody, a database with the schema in it, and the attachment**, with
/// the system named as the chooser and one record on the deployment stream.
#[tokio::test(flavor = "multi_thread")]
async fn provisioning_creates_custody_database_and_row() {
    let (control, f, serving) = control("ordo_provision_create").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_create_t");
    drop_database(&target_db);
    let stream = Recorder::default();

    let out = prepare(&control, &f, &serving, "spc_routed", &target_db, &stream)
        .await
        .expect("prepared");
    assert_eq!(
        out,
        Prepared::Attached {
            space: "spc_routed".into(),
            db: target_db.clone()
        }
    );

    assert!(
        f.keys.current_epoch(&SpaceId("spc_routed".into())).is_ok(),
        "custody exists"
    );
    assert!(database_exists(&control, &target_db).await);
    let tpool = target_pool(&target_db).await;
    assert!(
        has_table(&tpool, "members").await,
        "the schema is in the new database"
    );
    assert!(
        has_table(&tpool, "$commandlog").await,
        "and so are the platform tables"
    );

    let row = spacedb::lookup(&control, &SpaceId("spc_routed".into()))
        .await
        .unwrap()
        .expect("attached");
    assert_eq!(row.db, target_db);
    assert_eq!(
        (row.attached_by.as_str(), row.attached_at),
        (SYSTEM_ACTOR, NOW as i64),
        "the system chose, whoever asked, and the time is the call's"
    );

    let events = custody_events(&stream);
    // The scope and the subject, which is what this test is about. The
    // action's payload is `the_custody_record_names_the_target_its_terms_and_its_occupancy`'s,
    // so the two do not both have to change when the record grows a field.
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        (events[0].0.clone(), events[0].1.clone()),
        (Scope::Deployment, Some("spc_routed".to_string())),
        "one custody record, on the deployment stream, about the new space"
    );
    assert!(
        events[0].2.starts_with("space_attached_database "),
        "{}",
        events[0].2
    );
}

/// **An interruption at the worst point is completed by a retry.** The
/// database exists and already holds the schema, so re-running the DDL
/// would fail; the retry must see the schema and skip it, then attach. A
/// third call is a retry of a finished provision: no change, no record.
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_provision_is_completed_by_a_retry() {
    let (control, f, serving) = control("ordo_provision_retry").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_retry_t");
    drop_database(&target_db);
    // What an interruption after step 5 leaves: keys, a database, a schema,
    // and no attachment.
    f.keys
        .create_space(&SpaceId("spc_resumed".into()))
        .expect("custody");
    let cfg = PoolConfig {
        db: target_db.clone(),
        ..PoolConfig::from_env()
    };
    assert!(ordo_exec::create_database(&cfg, &target_db)
        .await
        .expect("created"));
    let tpool = target_pool(&target_db).await;
    assert!(
        spacedb::apply_schema(&tpool, &ordo_schema::ddl_for(&f.schema).unwrap())
            .await
            .expect("schema")
    );

    let stream = Recorder::default();
    let first = prepare(&control, &f, &serving, "spc_resumed", &target_db, &stream)
        .await
        .expect("the retry completes the provision");
    assert!(matches!(first, Prepared::Attached { .. }), "{first:?}");
    let again = prepare(&control, &f, &serving, "spc_resumed", &target_db, &stream)
        .await
        .expect("a retry of a finished provision is quiet");
    assert!(
        matches!(again, Prepared::AlreadyAttached { .. }),
        "{again:?}"
    );
    assert_eq!(
        custody_events(&stream).len(),
        1,
        "two calls, one custody event: a retry is not a custody change"
    );
}

/// **A space that already holds rows on the default is not attached**, and
/// the refusal writes nothing: no database, no keys, no row.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_holding_rows_on_the_default_is_not_attached() {
    let (control, f, serving) = control("ordo_provision_occupied").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_occupied_t");
    drop_database(&target_db);
    control
        .rows(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
             \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") VALUES \
             ('spc_occupied', 'mem_x', 1, 'p_x', 'x', 'principal', 1, 'owner', 'active', \
             DATE '2026-01-01')",
        )
        .await
        .expect("a row on the default");

    let stream = Recorder::default();
    let err = prepare(&control, &f, &serving, "spc_occupied", &target_db, &stream)
        .await
        .expect_err("attaching would leave the row behind");
    assert!(
        matches!(err, ProvisionRefusal::Occupied { rows: 1, .. }),
        "{err}"
    );
    assert!(!database_exists(&control, &target_db).await);
    assert!(spacedb::lookup(&control, &SpaceId("spc_occupied".into()))
        .await
        .unwrap()
        .is_none());
    assert!(custody_events(&stream).is_empty());
}

/// **A space attached to one database is not moved to another**, and the
/// second database is never created.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_attached_elsewhere_is_not_moved() {
    let (control, f, serving) = control("ordo_provision_moved").await;
    let target_a = ordo_conformance::test_db_name("ordo_provision_moved_a");
    let target_b = ordo_conformance::test_db_name("ordo_provision_moved_b");
    drop_database(&target_a);
    drop_database(&target_b);
    let stream = Recorder::default();
    prepare(&control, &f, &serving, "spc_placed", &target_a, &stream)
        .await
        .expect("placed on a");

    let err = prepare(&control, &f, &serving, "spc_placed", &target_b, &stream)
        .await
        .expect_err("a second database would be a move");
    assert!(
        matches!(
            err,
            ProvisionRefusal::Attach(AttachRefusal::WouldMove { .. })
        ),
        "{err}"
    );
    assert!(
        !database_exists(&control, &target_b).await,
        "the refused target was never created"
    );
}

// ── the founding member and the door's directory (S5, second half) ──────

fn member_status(
    engine: &ordo_identity::AccessEngine,
    space: &str,
    member: &str,
) -> Option<ordo_identity::principal::MembershipStatus> {
    engine.membership_status(
        &ordo_identity::principal::SpaceId(space.into()),
        &ordo_identity::principal::PrincipalId(member.into()),
    )
}

async fn directory(control: &Pool, f: &Fixture) -> ordo_host::directory::Directory {
    ordo_host::directory::routed_engine(
        f,
        control,
        &Installed::new().as_configured("local"),
        &PoolConfig {
            size: 1,
            ..PoolConfig::from_env()
        },
    )
    .await
    .expect("the directory builds")
}

async fn members_in(pool: &Pool, space: &str) -> Vec<String> {
    pool.rows(&format!(
        "SELECT \"id\" FROM \"members\" WHERE \"$space\" = '{space}' ORDER BY \"id\""
    ))
    .await
    .unwrap()
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect()
}

/// **A space on its own database has its members there, and the door reads
/// them from there.** The founding owner is in the target and not in the
/// control database; the routed directory makes them a member; a stale copy
/// of the space's members on the default is not honored; and the spaces that
/// live on the default are still read from it.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_on_its_own_database_has_its_members_there() {
    let (control, f, serving) = control("ordo_provision_members").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_members_t");
    drop_database(&target_db);
    let attachment = on(&target_db);
    let credentials = Installed::new().as_configured("local");
    let template = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    let target = Target {
        attachment: &attachment,
        credentials: &credentials,
        template: &template,
        hosts: &HostAllowlist::of(&PoolConfig::from_env().host),
    };
    let owner = ordo_host::provision::FoundingOwner {
        member_id: "mem_home",
        principal: "mem_home",
        display_name: "Home's owner",
    };
    let provision = || {
        ordo_host::provision::provision_space_on(
            &control,
            f.keys.as_ref(),
            &serving,
            "spc_home",
            &owner,
            &target,
            None,
            NOW,
        )
    };

    assert_eq!(
        provision().await.expect("provisioned"),
        ordo_host::provision::Provisioned::Created {
            space: "spc_home".into()
        }
    );
    assert_eq!(
        provision().await.expect("a retry is quiet"),
        ordo_host::provision::Provisioned::AlreadyExists {
            space: "spc_home".into()
        }
    );

    let tpool = target_pool(&target_db).await;
    assert_eq!(members_in(&tpool, "spc_home").await, vec!["mem_home"]);
    assert!(
        members_in(&control, "spc_home").await.is_empty(),
        "the founding member is in the space's own database and nowhere else"
    );

    let d = directory(&control, &f).await;
    assert!(d.unread.is_empty(), "{:?}", d.unread);
    assert_eq!(
        member_status(&d.engine, "spc_home", "mem_home"),
        Some(ordo_identity::principal::MembershipStatus::Active),
        "the door reads the owner from the space's own database"
    );
    assert!(
        member_status(&d.engine, ordo_conformance::FIXTURE_SPACE, "mem_o").is_some(),
        "and a space on the default is still read from the default"
    );

    // A stale copy on the default does not speak for an attached space.
    control
        .rows(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
             \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") VALUES \
             ('spc_home', 'mem_stale', 1, 'mem_stale', 'stale', 'principal', 1, 'owner', \
             'active', DATE '2026-01-01')",
        )
        .await
        .expect("a stale row on the default");
    let d = directory(&control, &f).await;
    assert_eq!(
        member_status(&d.engine, "spc_home", "mem_stale"),
        None,
        "a member row on the wrong database grants nothing"
    );
    assert!(member_status(&d.engine, "spc_home", "mem_home").is_some());
}

/// **A target that cannot be read is reported, and only its spaces go
/// without members.** The deployment's directory still builds, the default's
/// spaces are still served, and the unread target is named rather than
/// silently missing.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_that_cannot_be_read_is_reported_and_only_its_spaces_lose_members() {
    let (control, f, _serving) = control("ordo_provision_unread").await;
    spacedb::attach(
        &control,
        &SpaceId("spc_gone".into()),
        &DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            ..on("nowhere")
        },
    )
    .await
    .expect("attached");

    let d = directory(&control, &f).await;
    assert_eq!(d.unread.len(), 1, "{:?}", d.unread);
    assert!(
        d.unread[0].contains("nowhere") && d.unread[0].contains("1 space"),
        "{}",
        d.unread[0]
    );
    assert!(member_status(&d.engine, ordo_conformance::FIXTURE_SPACE, "mem_o").is_some());
}

/// **The door's read of a tenant database does not deadlock with a blob
/// write** (two instances, `concurrent_writes_stay_consistent`,
/// 2026-10-27).
///
/// The read used to dial with `Pool::connect`, which runs the platform
/// tables' DDL in one transaction. That held `ACCESS EXCLUSIVE` on
/// `$sealedpayload` and waited for `$blobref`, while a blob write held
/// `$blobref` and waited for `$sealedpayload`. Postgres killed one of them,
/// and when it was the read the door denied every member of the space until
/// the next rebuild.
///
/// The write is stood in for by `LOCK TABLE ... IN ROW EXCLUSIVE MODE`,
/// the lock its `INSERT`s take, in the same order.
#[tokio::test(flavor = "multi_thread")]
async fn the_doors_read_does_not_deadlock_with_a_blob_write() {
    let (control, f, _serving) = control_sized("ordo_provision_door_ddl", 3).await;
    // The tenant's database is the control database: it has every platform
    // table and a `members` table, which is all the read needs.
    let db = control.config().db;
    spacedb::attach(&control, &SpaceId("spc_busy".into()), &on(&db))
        .await
        .expect("attached");

    let write = control.begin().await.expect("a write begins");
    write
        .rows(r#"LOCK TABLE "$blobref" IN ROW EXCLUSIVE MODE"#)
        .await
        .expect("the write holds its blob reference");

    let read = {
        let (control, f) = (control.clone(), f.clone());
        tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(10), directory(&control, &f)).await
        })
    };
    // Long enough for the old read to take `$sealedpayload` and queue on
    // `$blobref`.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let second = write
        .rows(r#"LOCK TABLE "$sealedpayload" IN ROW EXCLUSIVE MODE"#)
        .await;
    write.rollback().await.ok();

    assert!(
        second.is_ok(),
        "the write's second insert was not killed by a deadlock: {second:?}"
    );
    let d = read
        .await
        .expect("the read task")
        .expect("the door's read did not wait on the write");
    assert!(
        d.unread.is_empty(),
        "every database was read: {:?}",
        d.unread
    );
}

// ── §10 T1: `runtime::door_for` reads `routed_engine` ─────────────────────
//
// `door_for` (`runtime.rs`) now builds its engine through
// `directory::routed_engine` rather than `deployment_engine_pooled`
// (`plan-space-routing.md` §9.3 D-2). These two pin the properties T1's
// done-when names, at the same seam the tests above already exercise.
// `door_for` itself has no logic of its own past the call, so the seam
// this function is the honest place to pin it at.

/// **D-2, closed: an attached tenant's founding member is seen by the
/// door.** Before this increment the door's engine was built from
/// `deployment_engine_pooled`, which reads `members` from the control pool
/// only; a tenant provisioned onto its own target (per `membership-home`)
/// has its founding member there and nowhere else, so the door would deny
/// every request in that tenant. `routed_engine` is the fix, and this is
/// the property it exists for.
#[tokio::test(flavor = "multi_thread")]
async fn an_attached_tenants_founding_member_is_seen_by_the_door() {
    let (control, f, serving) = control("ordo_door_founding_member").await;
    let target_db = ordo_conformance::test_db_name("ordo_door_founding_member_t");
    drop_database(&target_db);
    let attachment = on(&target_db);
    let credentials = Installed::new().as_configured("local");
    let template = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    let target = Target {
        attachment: &attachment,
        credentials: &credentials,
        template: &template,
        hosts: &HostAllowlist::of(&PoolConfig::from_env().host),
    };
    let owner = ordo_host::provision::FoundingOwner {
        member_id: "mem_door",
        principal: "mem_door",
        display_name: "The door's own tenant",
    };
    ordo_host::provision::provision_space_on(
        &control,
        f.keys.as_ref(),
        &serving,
        "spc_door_tenant",
        &owner,
        &target,
        None,
        NOW,
    )
    .await
    .expect("provisioned onto its own target");

    let d = directory(&control, &f).await;
    assert!(d.unread.is_empty(), "{:?}", d.unread);
    assert_eq!(
        member_status(&d.engine, "spc_door_tenant", "mem_door"),
        Some(ordo_identity::principal::MembershipStatus::Active),
        "the door reads the attached tenant's founding member from its own database"
    );
}

/// **A target the door cannot read is reported rather than fatal.** Read
/// failure on one tenant's database (unreachable here, and no different
/// from a wrong credential or a dead host) must not fail the door's own
/// build: the deployment still boots and serves every space whose database
/// does answer. Only the unreadable target's own spaces go without
/// members, and it is named, not silently dropped.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_the_door_cannot_read_is_reported_rather_than_fatal() {
    let (control, f, _serving) = control("ordo_door_unread_target").await;
    spacedb::attach(
        &control,
        &SpaceId("spc_door_unreachable".into()),
        &DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            ..on("nowhere-the-door-cannot-reach")
        },
    )
    .await
    .expect("attached");

    // The build itself must not fail: `routed_engine` (what `door_for`
    // calls) returns `Ok` with the unread target named, never an `Err` that
    // would refuse the whole boot over one tenant's database.
    let d = directory(&control, &f).await;
    assert_eq!(
        d.unread.len(),
        1,
        "the unreachable target is reported, not swallowed: {:?}",
        d.unread
    );
    assert!(
        d.unread[0].contains("nowhere-the-door-cannot-reach"),
        "the report names which target, for the operator log `door_for` prints it to: {}",
        d.unread[0]
    );
    assert!(
        member_status(&d.engine, ordo_conformance::FIXTURE_SPACE, "mem_o").is_some(),
        "every other space is still served"
    );
}

// ── R-S1: the version a target receives, and the ones that predate it ────
//
// `plan-space-routing.md` §9.2. `prepare_target` reads the schema from
// `Deployment::current()` rather than from its caller, and the record of
// which version it applied goes into the target inside the same
// all-or-nothing apply. `ordo-exec`'s `tests/spacedb.rs` pins the apply
// itself; these two pin what provisioning does with it.

/// **A fresh target records the version it received**, and the version is
/// the deployment's rather than the caller's: nothing in this test names a
/// schema or a version to `prepare_target`, and the row that comes back
/// names the one the deployment was serving.
///
/// **Observed failing first** with the `$migrations` write left out of the
/// apply: the target has `members` and reads as version unknown, which is
/// the pre-R-S1 state and is what the second test below asserts about a
/// database this code did not create.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_target_records_the_version_it_received() {
    let (control, f, serving) = control("ordo_provision_version").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_version_t");
    drop_database(&target_db);
    let stream = Recorder::default();

    prepare(&control, &f, &serving, "spc_versioned", &target_db, &stream)
        .await
        .expect("prepared");

    assert_eq!(
        recorded_version(&target_db).await,
        spacedb::TargetVersion::At(SERVING.into()),
        "the target names the version that was serving when its tables were made"
    );

    // The row is a `create` from nothing, by the system, and it lists the
    // schema it created, in the shape an apply against the control database
    // writes, so an operator reads one table and not two.
    let target = target_pool(&target_db).await;
    let row = target
        .rows(
            "SELECT \"kind\", \"from_version\", \"actor\", \"phase\", \"changes\", \"at\" \
             FROM \"$migrations\"",
        )
        .await
        .expect("$migrations is in the target, which is the database it describes");
    assert_eq!(row.len(), 1, "one provision, one row");
    assert_eq!(row[0][0].as_deref(), Some("create"));
    assert_eq!(row[0][1], None, "a new database has no version before this");
    assert_eq!(row[0][2].as_deref(), Some(SYSTEM_ACTOR));
    assert_eq!(row[0][3].as_deref(), Some("applied"));
    assert_eq!(
        row[0][5].as_deref(),
        Some((NOW * 1_000).to_string().as_str()),
        "`at` is milliseconds, the unit every other `$migrations` row carries"
    );
    for e in &f.schema.entities {
        assert!(
            row[0][4]
                .as_deref()
                .unwrap_or_default()
                .contains(&format!("A1 — new entity `{}`", e.name)),
            "the row lists what it created: {:?}",
            row[0][4]
        );
    }
}

/// **A database with `members` and no `$migrations` row reads as version
/// unknown**, rather than erroring and rather than being retrofitted with a
/// row that would be a guess.
///
/// This is the state every target provisioned before R-S1 is in. The
/// database here is built by hand into exactly that state, and then given
/// to `prepare_target`, which is the path a retried provision takes. It
/// completes the attachment, because that is the half that was missing, and
/// it leaves the version alone, because the version is the half it cannot
/// honestly supply: the tables were created by an artifact nobody recorded,
/// and a row naming today's would be indistinguishable from one that was
/// really applied. R-S4 reports it and an operator resolves it once.
///
/// **Observed failing first** with `apply_schema_at_version` changed to
/// write its row whenever one is missing rather than only when it created
/// the tables: the target then claims to be at the serving version, which
/// is a claim nothing checked.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_provisioned_before_this_increment_reads_as_version_unknown() {
    let (control, f, serving) = control("ordo_provision_pre_rs1").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_pre_rs1_t");
    drop_database(&target_db);

    // What a pre-R-S1 provision left behind: custody, a database, a schema,
    // no version row, and no attachment.
    f.keys
        .create_space(&SpaceId("spc_legacy".into()))
        .expect("custody");
    let cfg = PoolConfig {
        db: target_db.clone(),
        ..PoolConfig::from_env()
    };
    assert!(ordo_exec::create_database(&cfg, &target_db)
        .await
        .expect("created"));
    assert!(
        spacedb::apply_schema(&target_pool(&target_db).await, &f.ddl)
            .await
            .expect("the schema, with no record of which one"),
    );
    assert_eq!(
        recorded_version(&target_db).await,
        spacedb::TargetVersion::Unknown,
        "the state this test is about"
    );

    let stream = Recorder::default();
    let out = prepare(&control, &f, &serving, "spc_legacy", &target_db, &stream)
        .await
        .expect("a pre-R-S1 database is not an error to finish provisioning onto");
    assert!(
        matches!(out, Prepared::Attached { .. }),
        "the missing half, the attachment, is completed: {out:?}"
    );

    assert_eq!(
        recorded_version(&target_db).await,
        spacedb::TargetVersion::Unknown,
        "and the version is still unknown, because it is not this code's to invent"
    );
}

// ── 18d §9.8: what the door refuses ────────────────────────────────────

/// **A host the deployment was not told to allow is not attached**, and the
/// refusal lands before anything is created (R-X3).
///
/// Ruled 2026-09-21 (`db-allowlist`). The host on an attachment is an
/// address this deployment will dial, chosen by data, which is what
/// `ordo-egress` exists to refuse for HTTP; a Postgres dial does not pass
/// through that kernel.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_the_deployment_does_not_allow_is_not_attached() {
    let (control, f, serving) = control("ordo_provision_host").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_host_t");
    drop_database(&target_db);
    let stream = Recorder::default();

    let err = prepare_as(
        &control,
        &f,
        &serving,
        "spc_host",
        &target_db,
        DbCustody::PlatformManaged,
        // Allows one host, and it is not the one the attachment names.
        &HostAllowlist::of("db.somewhere.else"),
        &stream,
    )
    .await
    .expect_err("the host is not on the list");
    assert!(
        matches!(err, ProvisionRefusal::HostNotAllowed { .. }),
        "{err}"
    );

    // Before the dial means before all of it.
    assert!(!database_exists(&control, &target_db).await);
    assert!(spacedb::lookup(&control, &SpaceId("spc_host".into()))
        .await
        .unwrap()
        .is_none());
    assert!(custody_events(&stream).is_empty());
}

/// **An empty allowlist allows nothing**, which is the whole of the default
/// this ships with.
///
/// Written as its own test rather than as a line in the one above, because
/// "unset" and "set to something else" are different states and the
/// dangerous one is unset: a deployment whose operator never configured
/// this is the one an open default would leave undefended.
#[tokio::test(flavor = "multi_thread")]
async fn an_unset_allowlist_permits_no_host_at_all() {
    let env = PoolConfig::from_env();
    assert!(!HostAllowlist::of("").allows(&env.host));
    assert!(!HostAllowlist::default().allows(&env.host));
    assert!(!HostAllowlist::of("").allows("localhost"));
    // And a list that names it does allow it, so the test above is not
    // passing because nothing is ever allowed.
    assert!(HostAllowlist::of(&env.host).allows(&env.host));
}

/// **The allowlist matches a host, not a suffix of one.**
///
/// A check written with `contains` or `ends_with` would let
/// `evil-db.internal` through a list naming `db.internal`, which is the
/// standard way an allowlist becomes decorative.
#[tokio::test(flavor = "multi_thread")]
async fn the_allowlist_matches_whole_hosts_and_not_suffixes() {
    let list = HostAllowlist::of("db.internal, 10.0.0.4");
    assert!(list.allows("db.internal"));
    assert!(list.allows("10.0.0.4"));
    assert!(!list.allows("evil-db.internal"));
    assert!(!list.allows("db.internal.evil.com"));
    assert!(!list.allows("10.0.0.40"));
    // Case and surrounding space are spellings, not different hosts.
    assert!(list.allows("DB.Internal"));
    assert!(list.allows("  db.internal  "));
}

/// **The control database is not a target**, whichever way its host is
/// spelt (R-X2).
///
/// A tenant pointed at the control plane is a tenant whose rows sit among
/// identity, grants, `$artifacts` and every other tenant's attachment. The
/// check is on identity rather than on the host string, so `localhost` and
/// `127.0.0.1` are one database here and not two.
#[tokio::test(flavor = "multi_thread")]
async fn the_control_database_is_not_a_target() {
    let (control, f, serving) = control("ordo_provision_control").await;
    let stream = Recorder::default();

    // **The control database's own name, read off the pool** rather than
    // spelled again. Since test databases are namespaced per run
    // (`Pg::named`), the bare stem is a *different* database and this test
    // passed for the wrong reason the moment the suffix arrived: the
    // refusal it asserts stopped firing, because the target it named was
    // no longer the control database. Asking the pool cannot drift.
    let control_db = control.config().db.clone();
    let err = prepare_as(
        &control,
        &f,
        &serving,
        "spc_control",
        &control_db,
        DbCustody::PlatformManaged,
        &allowing_the_test_cluster(),
        &stream,
    )
    .await
    .expect_err("the control database is not a tenant's");
    assert!(
        matches!(err, ProvisionRefusal::ControlDatabase { .. }),
        "{err}"
    );
    assert!(spacedb::lookup(&control, &SpaceId("spc_control".into()))
        .await
        .unwrap()
        .is_none());
    assert!(custody_events(&stream).is_empty());
}

/// **`localhost` and `127.0.0.1` are one database, not two.**
///
/// The half of R-X2 a string comparison fails. Asserted on the predicate
/// directly as well as through the door, because the door can only be
/// pointed at whichever spelling the environment happens to use and the
/// bug is precisely about the other one.
#[tokio::test(flavor = "multi_thread")]
async fn the_loopback_spellings_name_one_control_database() {
    let (control, f, serving) = control("ordo_provision_loopback").await;
    let env = PoolConfig::from_env();
    // Only meaningful where the cluster is reached over loopback, which is
    // how both the laptop and CI reach it.
    if !["localhost", "127.0.0.1", "::1"].contains(&env.host.as_str()) {
        return;
    }
    let other = if env.host == "localhost" {
        "127.0.0.1"
    } else {
        "localhost"
    };
    assert_ne!(other, env.host, "the two spellings must differ as strings");

    // The control pool's own database, whatever name this run gave it: the
    // predicate compares the database too, so the stem would be a different
    // database and nothing would be refused.
    let attachment = DbAttachment {
        host: other.into(),
        ..on(&control.config().db)
    };
    let credentials = Installed::new().as_configured("local");
    let template = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    let hosts = HostAllowlist::of(&format!("{}, {other}", env.host));
    let stream = Recorder::default();
    let err = prepare_target(
        &control,
        f.keys.as_ref(),
        &serving,
        "spc_loopback",
        &Target {
            attachment: &attachment,
            credentials: &credentials,
            template: &template,
            hosts: &hosts,
        },
        Some(&stream),
        NOW,
    )
    .await
    .expect_err("a second spelling of the control database is still it");
    assert!(
        matches!(err, ProvisionRefusal::ControlDatabase { .. }),
        "spelt `{other}` where the control pool says `{}`: {err}",
        env.host
    );
}

/// **A second tenant is not put into a database that already holds one**
/// (D-D).
///
/// Ruled 2026-09-24: *"1 tenant 1 database. same for buckets.
/// database/schema not cluster."* The question asked was the narrow one —
/// whether a second tenant inside a declared own perimeter is refused — and
/// the answer was the general rule, so this asserts the general rule.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_tenant_is_not_put_into_an_occupied_database() {
    let (control, f, serving) = control("ordo_provision_perim").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_perim_t");
    drop_database(&target_db);
    let stream = Recorder::default();
    let hosts = allowing_the_test_cluster();

    prepare_as(
        &control,
        &f,
        &serving,
        "spc_first",
        &target_db,
        DbCustody::OwnPerimeter,
        &hosts,
        &stream,
    )
    .await
    .expect("the first tenant's own database");

    let err = prepare_as(
        &control,
        &f,
        &serving,
        "spc_second",
        &target_db,
        // Even declaring itself platform-managed. The refusal is about the
        // tenant already there, not about what the newcomer claims.
        DbCustody::PlatformManaged,
        &hosts,
        &stream,
    )
    .await
    .expect_err("that perimeter is the first tenant's");
    match &err {
        ProvisionRefusal::DatabaseOccupied { space, holder } => {
            assert_eq!(space, "spc_second");
            assert_eq!(holder, "spc_first");
        }
        other => panic!("{other}"),
    }
    assert!(spacedb::lookup(&control, &SpaceId("spc_second".into()))
        .await
        .unwrap()
        .is_none());
}

/// **The refusal does not depend on either tenant's custody** (D-D).
///
/// The mirror of the test above with the custodies swapped: a
/// platform-managed tenant sitting, an `OwnPerimeter` tenant arriving.
/// Under the narrower reading this increment was first built to, these two
/// orders produced two different refusals. Under the ruling they are one
/// rule, and this is what says so.
#[tokio::test(flavor = "multi_thread")]
async fn one_tenant_per_database_does_not_depend_on_custody() {
    let (control, f, serving) = control("ordo_provision_claim").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_claim_t");
    drop_database(&target_db);
    let stream = Recorder::default();
    let hosts = allowing_the_test_cluster();

    prepare_as(
        &control,
        &f,
        &serving,
        "spc_sitting",
        &target_db,
        DbCustody::PlatformManaged,
        &hosts,
        &stream,
    )
    .await
    .expect("a shared-tier tenant");

    let err = prepare_as(
        &control,
        &f,
        &serving,
        "spc_claiming",
        &target_db,
        DbCustody::OwnPerimeter,
        &hosts,
        &stream,
    )
    .await
    .expect_err("a perimeter with somebody else inside is not a perimeter");
    match &err {
        ProvisionRefusal::DatabaseOccupied { space, holder } => {
            assert_eq!(space, "spc_claiming");
            assert_eq!(holder, "spc_sitting");
        }
        other => panic!("{other}"),
    }
}

/// **Two platform-managed tenants do not share one database either** (D-D).
///
/// This test asserted the opposite until 2026-09-24, and it was not wrong
/// about the code: R-X1 describes a **shared tier** in as many words — *"a
/// hundred tenants attached to one database share one pool and one
/// credential"* — and this asserted that tier worked. The ruling removed
/// the tier from the attach path: *"1 tenant 1 database."* So the assertion
/// is inverted rather than deleted, because the behaviour it named is the
/// behaviour that changed.
///
/// **What did not change is the cluster.** Both tenants below still reach
/// the same Postgres server on the same credential; what they may not share
/// is the database. *"database/schema not cluster."*
#[tokio::test(flavor = "multi_thread")]
async fn two_tenants_do_not_share_one_database_whatever_their_custody() {
    let (control, f, serving) = control("ordo_provision_shared").await;
    let target_t = ordo_conformance::test_db_name("ordo_provision_shared_t");
    drop_database(&target_t);
    let target_u = ordo_conformance::test_db_name("ordo_provision_shared_u");
    drop_database(&target_u);
    let stream = Recorder::default();
    let hosts = allowing_the_test_cluster();

    prepare_as(
        &control,
        &f,
        &serving,
        "spc_sharing_a",
        &target_t,
        DbCustody::PlatformManaged,
        &hosts,
        &stream,
    )
    .await
    .expect("the first tenant");

    let err = prepare_as(
        &control,
        &f,
        &serving,
        "spc_sharing_b",
        &target_t,
        DbCustody::PlatformManaged,
        &hosts,
        &stream,
    )
    .await
    .expect_err("a database holds one tenant");
    assert!(
        matches!(err, ProvisionRefusal::DatabaseOccupied { .. }),
        "{err}"
    );

    // **Its own database on the same server is fine**, which is the half of
    // the ruling that keeps a cluster shareable. Without this the test
    // above would also pass if attaching had simply stopped working.
    prepare_as(
        &control,
        &f,
        &serving,
        "spc_sharing_b",
        &target_u,
        DbCustody::PlatformManaged,
        &hosts,
        &stream,
    )
    .await
    .expect("a second database on the same cluster");

    // And occupancy says one apiece, which is now the only value it can
    // hold for an attached tenant.
    let env = PoolConfig::from_env();
    for db in [&target_t, &target_u] {
        let in_there = spacedb::occupants(&control, &env.host, env.port, db)
            .await
            .expect("occupants");
        assert_eq!(in_there.len(), 1, "{db}: {in_there:?}");
    }
}

/// **The custody record says where the tenant was put, on what terms, and
/// who else is in there** (T8).
///
/// Before this increment the record said `space_attached_database` and
/// nothing else, so it recorded that an attach happened and not one fact
/// about it. An audit record that cannot answer "which database" is not an
/// audit record of a placement.
///
/// **`occupancy=1` is now the only value this can hold** for an attached
/// tenant, since D-D made a database hold one. It stays in the record
/// rather than being dropped as a constant: it is the field that says the
/// invariant held at the moment of the attach, and a record that asserted
/// nothing about it could not tell a reader whether a `2` was ever written.
#[tokio::test(flavor = "multi_thread")]
async fn the_custody_record_names_the_target_its_terms_and_its_occupancy() {
    let (control, f, serving) = control("ordo_provision_record").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_record_t");
    drop_database(&target_db);
    let stream = Recorder::default();
    let hosts = allowing_the_test_cluster();
    let env = PoolConfig::from_env();

    prepare_as(
        &control,
        &f,
        &serving,
        "spc_recorded",
        &target_db,
        DbCustody::PlatformManaged,
        &hosts,
        &stream,
    )
    .await
    .expect("attached");

    let events = custody_events(&stream);
    assert_eq!(events.len(), 1, "{events:?}");
    let action = &events[0].2;

    // The target's identity, spelt the one way `DbAttachment::identity`
    // spells it, so the record and the `$spacedb` row cannot drift apart.
    let expected = DbAttachment {
        custody: DbCustody::PlatformManaged,
        ..on(&target_db)
    };
    assert!(
        action.contains(&expected.identity()),
        "the record does not name the target: {action}"
    );
    // Each fact individually, so a failure says which one is missing.
    assert!(action.contains(&env.host), "{action}");
    assert!(action.contains(&target_db), "{action}");
    assert!(action.contains("local"), "the credential label: {action}");
    assert!(action.contains("platform-managed"), "{action}");
    assert!(action.contains("occupancy=1"), "{action}");
    assert!(action.contains("outcome=attached"), "{action}");
}

/// **A provision interrupted after the database was made records that it was
/// completed, not that it was fresh** (T8).
///
/// The distinction matters to whoever reads the stream afterwards: a
/// database this deployment did not create is a database whose prior
/// contents it did not choose.
#[tokio::test(flavor = "multi_thread")]
async fn a_completed_interrupted_provision_records_that_it_was_completed() {
    let (control, f, serving) = control("ordo_provision_resumed").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_resumed_t");
    drop_database(&target_db);

    // The interruption: the database exists, the attachment row does not.
    let cfg = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    assert!(
        ordo_exec::create_database(&cfg, &target_db)
            .await
            .expect("the half-finished provision"),
        "the fixture must be the one that creates it"
    );

    let stream = Recorder::default();
    prepare_as(
        &control,
        &f,
        &serving,
        "spc_resumed",
        &target_db,
        DbCustody::PlatformManaged,
        &allowing_the_test_cluster(),
        &stream,
    )
    .await
    .expect("the retry finishes it");

    let events = custody_events(&stream);
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        events[0].2.contains("outcome=completed-interrupted"),
        "{}",
        events[0].2
    );
}

// ── 18d R-S5: attach and apply are serialised against each other ────────

/// The lock both sides contend on, which is `apply_op::ApplyLock` rather
/// than one of this increment's own: R-S5 asks for **one** advisory lock
/// taken by both, and the apply path already had it.
async fn lock_held(pool: &Pool) -> ordo_host::apply_op::ApplyLock {
    ordo_host::apply_op::ApplyLock::take(pool)
        .await
        .expect("no error taking the apply lock")
        .expect("the lock is free to start with")
}

/// **An attach waits for a schema apply rather than interleaving with it**,
/// and then completes (R-S5).
///
/// The window: a tenant attached while an apply is walking `targets()`
/// receives whichever schema the attach read, and is never revisited by a
/// walk already past it. The result is a database at a version the
/// deployment does not know it has.
///
/// The lock is held here directly rather than by running a real apply,
/// because what must be asserted is the **exclusion**, and a concurrent
/// apply would make the test a race about which one won.
#[tokio::test(flavor = "multi_thread")]
async fn an_attach_waits_for_a_routing_change_and_then_proceeds() {
    let (control, f, serving) = control_sized("ordo_provision_rs5", 3).await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_rs5_t");
    drop_database(&target_db);

    // Whoever is applying.
    let held = lock_held(&control).await;

    // The attacher gets clones of what is already standing. Calling
    // `control()` again would re-run the fixture reset and wipe the
    // database this test is about.
    let (pool, fixture, deployment) = (control.clone(), f.clone(), serving.clone());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let stream = Recorder::default();
        let _ = tx.send(());
        prepare_as(
            &pool,
            &fixture,
            &deployment,
            "spc_rs5",
            &target_db,
            DbCustody::PlatformManaged,
            &allowing_the_test_cluster(),
            &stream,
        )
        .await
        .map(|_| ())
    });
    rx.await.expect("the attach started");

    // It is **waiting**, not refusing. A refusal here would be the failure
    // the plan names: an attach that refuses during every deploy is an
    // attach that looks broken.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert!(
        !handle.is_finished(),
        "the attach did not wait for the routing lock"
    );
    assert!(
        spacedb::lookup(&control, &SpaceId("spc_rs5".into()))
            .await
            .unwrap()
            .is_none(),
        "the attach wrote its row while a routing change was in flight"
    );

    // The apply finishes.
    drop(held);

    // And the attach completes on its own, rather than having to be retried.
    let done = tokio::time::timeout(std::time::Duration::from_secs(20), handle)
        .await
        .expect("the attach never resumed after the lock was released")
        .expect("the attach task panicked");
    done.expect("the attach failed rather than waiting");
    assert!(spacedb::lookup(&control, &SpaceId("spc_rs5".into()))
        .await
        .unwrap()
        .is_some());
}

/// **Two deployments sharing one cluster do not contend.**
///
/// Advisory locks are scoped to a database. If this were taken on anything
/// but the control database, every deployment on a shared Postgres would
/// serialise its deploys against every other one's, which would look like
/// the platform being slow rather than like a lock. Asserted by taking both
/// at once: if they were one lock, the second would block for
/// `LOCK_WAIT_MS` and come back `None`.
#[tokio::test(flavor = "multi_thread")]
async fn the_routing_lock_is_scoped_to_one_control_database() {
    let (a, _fa, _sa) = control("ordo_provision_rs5_a").await;
    let (b, _fb, _sb) = control("ordo_provision_rs5_b").await;
    let held_a = lock_held(&a).await;
    // Bounded, so a failure here is a quick failure rather than a hang.
    let held_b = ordo_host::apply_op::ApplyLock::take(&b)
        .await
        .expect("no error")
        .expect("a second deployment's routing lock is not the first one's");
    drop(held_a);
    drop(held_b);
}

/// **A newly provisioned database appears in the fleet view** (R-S2).
///
/// A tenant attached and never applied to again would otherwise be missing
/// from the operator's listing until the next deploy, which is exactly the
/// window in which somebody asks where a just-created tenant went.
#[tokio::test(flavor = "multi_thread")]
async fn a_provisioned_target_is_recorded_in_the_fleet_view() {
    let (control, f, serving) = control("ordo_provision_fleet").await;
    let target_db = ordo_conformance::test_db_name("ordo_provision_fleet_t");
    drop_database(&target_db);
    let stream = Recorder::default();

    prepare_as(
        &control,
        &f,
        &serving,
        "spc_fleeted",
        &target_db,
        DbCustody::PlatformManaged,
        &allowing_the_test_cluster(),
        &stream,
    )
    .await
    .expect("attached");

    let env = PoolConfig::from_env();
    let label = ordo_exec::route::target_label(&env.host, env.port, &target_db);
    let row = ordo_exec::fleet::lookup(&control, &label)
        .await
        .expect("fleet view")
        .unwrap_or_else(|| panic!("no fleet row for `{label}`"));

    // **The target's own version, compared against the target.** R-S2 makes
    // `$migrations` the authority and this row a copy, so the assertion is
    // between the two rather than against a literal.
    let theirs = Pool::connect(PoolConfig {
        db: target_db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("the tenant's own database");
    match spacedb::recorded_version(&theirs).await.expect("their row") {
        spacedb::TargetVersion::At(v) => assert_eq!(
            row.version, v,
            "the fleet view disagrees with the database it caches"
        ),
        other => panic!("a freshly provisioned database knows its version: {other:?}"),
    }
    assert_eq!(row.confirmed_at, NOW as i64, "the call's own clock");
}
