//! S2 of `plan-space-routing.md`: the database attachment, against real
//! Postgres.
//!
//! What has to be true: **a repeat is a no-op that reports success, and a
//! different target is refused**, because repointing a space that already
//! has a database would leave its rows and its chain behind. And the refusal
//! has to hold under a race, since two attaches arriving together is the
//! ordinary shape of a retried provision.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::custody::SpaceId;
use ordo_exec::spacedb::{self, AttachRefusal, DbAttachOutcome, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};

async fn pool(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    Fixture::load()
        .expect("fixture")
        .reset(&pg)
        .expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    spacedb::ensure_table(&pool).await.expect("table");
    pool.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    pool
}

fn target(db: &str, at: i64, by: &str) -> DbAttachment {
    DbAttachment {
        host: "db-eu-1.internal".into(),
        port: 5432,
        user: "ordo_acme".into(),
        db: db.into(),
        credential: "acme-prod".into(),
        custody: DbCustody::PlatformManaged,
        attached_at: at,
        attached_by: by.into(),
    }
}

/// Everything written comes back, and a space nobody attached is on the
/// default database.
#[tokio::test]
async fn an_attachment_round_trips() {
    let pool = pool("ordo_spacedb_round_trip").await;
    let space = SpaceId("spc_acme".into());
    assert!(
        spacedb::lookup(&pool, &space).await.unwrap().is_none(),
        "a space with no row is on the default database"
    );

    let want = DbAttachment {
        custody: DbCustody::OwnPerimeter,
        port: 6432,
        ..target("acme", 100, "mem_owner")
    };
    assert_eq!(
        spacedb::attach(&pool, &space, &want).await.unwrap(),
        DbAttachOutcome::Attached
    );
    assert_eq!(
        spacedb::lookup(&pool, &space).await.unwrap(),
        Some(want),
        "every field, custody and port included, is what was attached"
    );
}

/// A retry is the same attachment and leaves the row alone; a different
/// target is refused and leaves the row alone too.
#[tokio::test]
async fn a_repeat_is_unchanged_and_a_different_target_is_refused() {
    let pool = pool("ordo_spacedb_repeat").await;
    let space = SpaceId("spc_acme".into());
    spacedb::attach(&pool, &space, &target("acme", 100, "mem_owner"))
        .await
        .unwrap();

    // An hour later, by another member. Same target, so the same
    // attachment: rewriting `attached_at` would make the consent page report
    // a custody change that did not happen.
    assert_eq!(
        spacedb::attach(&pool, &space, &target("acme", 999, "mem_other"))
            .await
            .unwrap(),
        DbAttachOutcome::Unchanged
    );

    // A different database is a move, and moving is not built.
    let err = spacedb::attach(&pool, &space, &target("acme_2", 200, "mem_owner"))
        .await
        .expect_err("repointing a space would leave its rows behind");
    assert!(
        matches!(&err, AttachRefusal::WouldMove { current } if current.ends_with("/acme")),
        "the refusal names the database the space is in: {err}"
    );
    // So is a custody change on the same database: it changes what
    // deleting the space does, which is a change to a live space's terms.
    let err = spacedb::attach(
        &pool,
        &space,
        &DbAttachment {
            custody: DbCustody::OwnPerimeter,
            ..target("acme", 300, "mem_owner")
        },
    )
    .await
    .expect_err("a custody change on an attached space is refused too");
    assert!(matches!(err, AttachRefusal::WouldMove { .. }));

    let stored = spacedb::lookup(&pool, &space)
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(
        (stored.db.as_str(), stored.attached_at, stored.custody),
        ("acme", 100, DbCustody::PlatformManaged),
        "neither the retry nor either refusal touched the row"
    );
}

/// Two attaches for one space with different targets, at once. Exactly one
/// wins, the other is told what won, and the table holds one row.
///
/// Delete-then-insert, the bucket attachment's statement shape, would let
/// both succeed in turn with the second silently replacing the first; this
/// is why the database attachment inserts only where no row exists.
#[tokio::test(flavor = "multi_thread")]
async fn racing_attaches_leave_one_winner() {
    let pool = Arc::new(pool("ordo_spacedb_race").await);
    let space = SpaceId("spc_acme".into());
    let mut tasks = Vec::new();
    for db in ["acme_a", "acme_b", "acme_c", "acme_d"] {
        let pool = pool.clone();
        let space = space.clone();
        tasks.push(tokio::spawn(async move {
            spacedb::attach(&pool, &space, &target(db, 1, "mem_owner")).await
        }));
    }
    let mut attached = 0;
    let mut refused = 0;
    for t in tasks {
        match t.await.expect("task") {
            Ok(DbAttachOutcome::Attached) => attached += 1,
            Err(AttachRefusal::WouldMove { .. }) => refused += 1,
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
    assert_eq!((attached, refused), (1, 3), "exactly one target wins");
    let rows = pool
        .rows("SELECT count(*) FROM \"$spacedb\" WHERE \"$space\" = 'spc_acme'")
        .await
        .unwrap();
    assert_eq!(rows[0][0].as_deref(), Some("1"));
}

/// One space's attachment is not another's.
#[tokio::test]
async fn attachments_are_per_space() {
    let pool = pool("ordo_spacedb_tenancy").await;
    let a = SpaceId("spc_a".into());
    let b = SpaceId("spc_b".into());
    spacedb::attach(&pool, &a, &target("a_db", 1, "mem_a"))
        .await
        .unwrap();
    assert!(spacedb::lookup(&pool, &b).await.unwrap().is_none());
    assert_eq!(
        spacedb::attach(&pool, &b, &target("b_db", 1, "mem_b"))
            .await
            .unwrap(),
        DbAttachOutcome::Attached
    );
    assert_eq!(
        spacedb::lookup(&pool, &a).await.unwrap().unwrap().db,
        "a_db"
    );
}

/// A custody this build cannot read is an error, not a default.
///
/// A bucket attachment falls back to the stronger position. For a database
/// either fallback prints a consent sentence that may be false about what
/// deleting the space does, so the lookup refuses to answer instead.
#[tokio::test]
async fn an_unknown_custody_is_an_error_not_a_default() {
    let pool = pool("ordo_spacedb_unknown_custody").await;
    let space = SpaceId("spc_acme".into());
    spacedb::attach(&pool, &space, &target("acme", 1, "mem_owner"))
        .await
        .unwrap();
    pool.rows(
        "UPDATE \"$spacedb\" SET \"custody\" = 'tenant-sealed' WHERE \"$space\" = 'spc_acme'",
    )
    .await
    .unwrap();
    let err = spacedb::lookup(&pool, &space)
        .await
        .expect_err("an unreadable custody must not be guessed");
    assert!(err.to_string().contains("tenant-sealed"), "{err}");
}

// ── R-S1: the schema and the record of which version it is ──────────────
//
// `plan-space-routing.md` §9.2. A target that holds the tables without the
// row naming their version has to have that version inferred from its live
// shape, and a shape does not tell two versions apart when they declare the
// same tables. These three pin the property, its failure mode, and what a
// database created before it existed reads as.

/// An **empty** database: created, connected, and nothing else. `members`
/// is absent, which is what makes it a fresh target rather than a seeded
/// one, and `$migrations` is absent because nothing has applied a schema.
async fn empty(db: &str) -> Pool {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"));
    admin
        .exec(&format!("CREATE DATABASE {db}"))
        .expect("an empty target");
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("the empty target connects")
}

fn fixture_ddl() -> String {
    Fixture::load().expect("fixture").ddl
}

/// **A fresh target records the version it received.** The tables and the
/// `$migrations` row naming them arrive together, so the next apply reads
/// the version instead of guessing it from the shape.
#[tokio::test]
async fn a_fresh_target_records_the_version_it_received() {
    let pool = empty("ordo_spacedb_records_version").await;
    assert_eq!(
        spacedb::recorded_version(&pool).await.unwrap(),
        spacedb::TargetVersion::Unknown,
        "before the apply there is nothing to know"
    );

    assert!(
        spacedb::apply_schema_at_version(
            &pool,
            &fixture_ddl(),
            "artifact-v7",
            "system:provisioning",
            "A1 — new entity `vendor`",
            1_700_000_000_000,
        )
        .await
        .expect("the schema applies"),
        "an empty target is applied to, not skipped"
    );

    assert_eq!(
        spacedb::recorded_version(&pool).await.unwrap(),
        spacedb::TargetVersion::At("artifact-v7".into()),
        "the target names the version its tables are"
    );
    let row = pool
        .rows(
            "SELECT \"kind\", \"from_version\", \"actor\", \"phase\", \"changes\" \
             FROM \"$migrations\"",
        )
        .await
        .unwrap();
    assert_eq!(row.len(), 1, "one apply, one row");
    let cell = |i: usize| row[0][i].clone();
    assert_eq!(cell(0), Some("create".into()), "the whole schema, created");
    assert_eq!(
        cell(1),
        None,
        "there is no version before the one a database is born at"
    );
    assert_eq!(cell(2), Some("system:provisioning".into()));
    assert_eq!(cell(3), Some("applied".into()));
    assert_eq!(cell(4), Some("A1 — new entity `vendor`".into()));
}

/// **The tables and their version row arrive together or not at all.**
///
/// The row is written inside the same simple-protocol query as the DDL, so
/// Postgres' one implicit transaction covers both. Forced here from the
/// `$migrations` side: a table of that name with the wrong columns makes
/// the `CREATE TABLE IF NOT EXISTS` a no-op and the `INSERT` an error, and
/// the schema that ran just before it must roll back with it.
///
/// **Observed failing first** with the insert sent as a second
/// `pool.rows(...)` call: `members` survives the failed insert, which is
/// exactly the target-with-tables-and-no-version state R-S1 forbids.
#[tokio::test]
async fn the_tables_and_their_version_row_arrive_together_or_not_at_all() {
    let pool = empty("ordo_spacedb_all_or_nothing").await;
    pool.rows("CREATE TABLE \"$migrations\" (\"name\" text)")
        .await
        .expect("a `$migrations` the insert cannot use");

    let err = spacedb::apply_schema_at_version(
        &pool,
        &fixture_ddl(),
        "artifact-v7",
        "system:provisioning",
        "A1 — new entity `vendor`",
        1_700_000_000_000,
    )
    .await
    .expect_err("the version row cannot be written, so the apply fails");
    assert!(err.to_string().contains("$migrations"), "{err}");

    let members = pool
        .rows("SELECT to_regclass('public.members') IS NOT NULL")
        .await
        .unwrap();
    assert_eq!(
        members[0][0].as_deref(),
        Some("f"),
        "the tables rolled back with the row that would have named them"
    );
}

/// **A database with a schema and no version row reads as version unknown,
/// rather than erroring and rather than being guessed at.**
///
/// This is every target provisioned before R-S1. A row written now would
/// name whichever version happens to be serving, about tables some earlier
/// artifact created, and nothing afterwards could tell that guess from a
/// version that was really applied.
#[tokio::test]
async fn a_database_from_before_the_version_row_reads_as_unknown() {
    let pool = empty("ordo_spacedb_pre_rs1").await;
    assert!(spacedb::apply_schema(&pool, &fixture_ddl())
        .await
        .expect("the pre-R-S1 apply, which writes no row"));

    let live = pool
        .rows("SELECT to_regclass('public.members') IS NOT NULL")
        .await
        .unwrap();
    assert_eq!(live[0][0].as_deref(), Some("t"), "it has a schema");
    assert_eq!(
        spacedb::recorded_version(&pool).await.unwrap(),
        spacedb::TargetVersion::Unknown,
        "and no record of which one, which is an answer and not an error"
    );

    // And the recording form does not retrofit one: `members` is present,
    // so it declines, and the database is left exactly as it was found.
    assert!(!spacedb::apply_schema_at_version(
        &pool,
        &fixture_ddl(),
        "artifact-v7",
        "system:provisioning",
        "",
        1_700_000_000_000,
    )
    .await
    .expect("a target that already has a schema is left alone"),);
    assert_eq!(
        spacedb::recorded_version(&pool).await.unwrap(),
        spacedb::TargetVersion::Unknown,
        "still unknown: an operator resolves this once, with knowledge this \
         process does not have"
    );
}
