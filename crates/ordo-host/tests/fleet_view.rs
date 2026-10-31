//! 18d R-S2: the control database's **fleet view**, one row per target with
//! the version it is known to be at and when that was last confirmed.
//!
//! The requirement's sharpest clause is the one these tests are mostly
//! about: *"The fleet view is a cache of the targets' own rows and is never
//! the authority; a target's `$migrations` is."*
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::{target_label, DEFAULT_LABEL};
use ordo_exec::{fleet, Pool, PoolConfig};

async fn control(db: &str) -> (Pool, Arc<Fixture>) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    f.reset(&Pg::named(db)).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 3,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("control");
    fleet::ensure_table(&pool).await.expect("$fleet");
    pool.rows("DELETE FROM \"$fleet\"").await.expect("empty");
    (pool, f)
}

/// **A confirmation is a version and the moment it was taken**, and reading
/// it back gives both.
#[tokio::test(flavor = "multi_thread")]
async fn a_confirmation_carries_the_version_and_when_it_was_taken() {
    let (pool, _f) = control("ordo_fleet_basic").await;
    let label = target_label("db.internal", 5432, "acme");

    fleet::confirm(&pool, &label, "v1", 1_700_000_000)
        .await
        .expect("confirm");

    let got = fleet::lookup(&pool, &label)
        .await
        .expect("lookup")
        .expect("a row");
    assert_eq!(got.version, "v1");
    assert_eq!(got.confirmed_at, 1_700_000_000);
    assert_eq!(got.label, "db.internal:5432/acme");
}

/// **A target that has never reported is absent, not present with a blank.**
///
/// Absence is the honest answer: this table records observations and there
/// has been none. A row saying "unknown" would be indistinguishable from a
/// pre-R-S1 database, which is a different fact with a different fix.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_that_never_reported_is_absent_rather_than_blank() {
    let (pool, _f) = control("ordo_fleet_absent").await;
    assert!(fleet::lookup(&pool, "db.internal:5432/never")
        .await
        .expect("lookup")
        .is_none());
    assert!(fleet::all(&pool).await.expect("all").is_empty());
}

/// **A later confirmation replaces an earlier one**, including when it names
/// an older version.
///
/// Last writer wins deliberately. A replica that observed an older version
/// later is reporting something real — a target that was restored or rolled
/// back — and a cache that refused the write would hide exactly that.
#[tokio::test(flavor = "multi_thread")]
async fn a_later_confirmation_wins_even_when_the_version_went_backwards() {
    let (pool, _f) = control("ordo_fleet_replace").await;
    let label = target_label("db.internal", 5432, "acme");

    fleet::confirm(&pool, &label, "v2", 200).await.expect("v2");
    fleet::confirm(&pool, &label, "v1", 300).await.expect("v1");

    let got = fleet::lookup(&pool, &label)
        .await
        .expect("lookup")
        .expect("a row");
    assert_eq!(
        (got.version.as_str(), got.confirmed_at),
        ("v1", 300),
        "a rollback observed later must be visible, not discarded"
    );
    assert_eq!(fleet::all(&pool).await.expect("all").len(), 1, "one row");
}

/// **One row per target, in label order**, and the default database is one
/// of them.
#[tokio::test(flavor = "multi_thread")]
async fn the_fleet_lists_every_target_including_the_default() {
    let (pool, _f) = control("ordo_fleet_list").await;
    fleet::confirm(&pool, &target_label("db.internal", 5432, "b"), "v1", 1)
        .await
        .unwrap();
    fleet::confirm(&pool, DEFAULT_LABEL, "v1", 2).await.unwrap();
    fleet::confirm(&pool, &target_label("db.internal", 5432, "a"), "v1", 3)
        .await
        .unwrap();

    let labels: Vec<String> = fleet::all(&pool)
        .await
        .expect("all")
        .into_iter()
        .map(|c| c.label)
        .collect();
    assert_eq!(
        labels,
        vec![
            "db.internal:5432/a".to_string(),
            "db.internal:5432/b".to_string(),
            "default".to_string(),
        ]
    );
}

/// **Forgetting a target removes it and leaves the others.**
///
/// For a detach (18e). Asserted now because the function exists now, and an
/// untested delete is the one that takes the wrong row.
#[tokio::test(flavor = "multi_thread")]
async fn forgetting_one_target_leaves_the_rest() {
    let (pool, _f) = control("ordo_fleet_forget").await;
    let a = target_label("db.internal", 5432, "a");
    let b = target_label("db.internal", 5432, "b");
    fleet::confirm(&pool, &a, "v1", 1).await.unwrap();
    fleet::confirm(&pool, &b, "v1", 1).await.unwrap();

    fleet::forget(&pool, &a).await.expect("forget");
    let left: Vec<String> = fleet::all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.label)
        .collect();
    assert_eq!(left, vec![b]);
}

/// **The label the fleet view keys on is the label the resolver hands out.**
///
/// Two spellings of one database would be two rows describing one target,
/// and whichever an operator read first would look authoritative. Asserted
/// against `target_label` rather than against a literal, because the point
/// is that there is one definition.
#[tokio::test(flavor = "multi_thread")]
async fn the_fleet_key_is_the_resolvers_own_label() {
    let (pool, _f) = control("ordo_fleet_label").await;
    let env = PoolConfig::from_env();
    let label = target_label(&env.host, env.port, "ordo_fleet_label_t");
    fleet::confirm(&pool, &label, "v1", 1).await.unwrap();

    // What a resolver would call the same database.
    let attachment = ordo_exec::spacedb::DbAttachment {
        host: env.host.clone(),
        port: env.port,
        user: env.user.clone(),
        db: "ordo_fleet_label_t".into(),
        credential: "cred-label-x".into(),
        custody: ordo_exec::spacedb::DbCustody::PlatformManaged,
        attached_at: 0,
        attached_by: "test".into(),
    };
    let from_resolver = target_label(&attachment.host, attachment.port, &attachment.db);
    assert!(
        fleet::lookup(&pool, &from_resolver)
            .await
            .expect("lookup")
            .is_some(),
        "the resolver's label `{from_resolver}` does not find the row written under `{label}`"
    );

    // And the label carries no credential, which is what makes it printable.
    assert!(!from_resolver.contains("cred-label-x"), "{from_resolver}");
}
