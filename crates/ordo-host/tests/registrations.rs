//! **Enrolment E4: the sweep of registrations no tenant admitted** (owner
//! ruling F3), against real Postgres.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::{PoolResolver, Resolving, SinglePool, Target, Targets};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::registrations::{describe_ttl, sweep, sweep_guests, ttl_from_env};

const TTL: u64 = 1_000;
const NOW: u64 = 100_000;

async fn prepared(db: &str) -> Pool {
    let pg = Pg::named(db);
    let f = Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::principal_directory::ensure_table(&pool)
        .await
        .expect("directory");
    ordo_host::credentials::ensure_table(&pool)
        .await
        .expect("credentials");
    ordo_host::tickets::ensure_table(&pool)
        .await
        .expect("tickets");
    for t in ["$authenticator", "$principal", "$credential", "$ticket"] {
        pool.rows(&format!("DELETE FROM \"{t}\""))
            .await
            .expect("clean");
    }
    pool
}

/// A principal with a password link and a credential.
async fn enrol(pool: &Pool, id: &str, created_at: u64) {
    pool.rows(&format!(
        "INSERT INTO \"$principal\" (\"id\", \"kind\", \"display\", \"created_at\") \
         VALUES ('{id}', 'usr', '{id}', {created_at})"
    ))
    .await
    .expect("principal");
    pool.rows(&format!(
        "INSERT INTO \"$authenticator\" (\"authenticator\", \"principal\", \"linked_at\") \
         VALUES ('password:{id}@example.test', '{id}', {created_at})"
    ))
    .await
    .expect("link");
    pool.rows(&format!(
        "INSERT INTO \"$credential\" (\"identifier\", \"hash\") \
         VALUES ('{id}@example.test', '$argon2id$fake')"
    ))
    .await
    .expect("credential");
}

/// A guest: a principal whose one link is a guest secret's hash.
async fn guest(pool: &Pool, id: &str, created_at: u64) {
    pool.rows(&format!(
        "INSERT INTO \"$principal\" (\"id\", \"kind\", \"display\", \"created_at\") \
         VALUES ('{id}', 'usr', 'Guest', {created_at})"
    ))
    .await
    .expect("principal");
    link(pool, id, &format!("guest:hash-of-{id}"), created_at).await;
}

async fn link(pool: &Pool, id: &str, authenticator: &str, at: u64) {
    pool.rows(&format!(
        "INSERT INTO \"$authenticator\" (\"authenticator\", \"principal\", \"linked_at\") \
         VALUES ('{authenticator}', '{id}', {at})"
    ))
    .await
    .expect("link");
}

async fn admit(pool: &Pool, principal: &str) {
    let space = ordo_conformance::fixture_space().0;
    pool.rows(&format!(
        "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
         \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
         VALUES ('{space}', 'mem_{principal}', 1, '{principal}', '{principal}', 'principal', 1, \
                 'viewer', 'active', CURRENT_DATE)"
    ))
    .await
    .expect("admitted");
}

/// A seat the person left: the shape `Fixture::release_seat` leaves,
/// `status = left`, no role, and the principal unlinked as
/// `left:<principal>:<seat>`.
async fn left(pool: &Pool, principal: &str) {
    let space = ordo_conformance::fixture_space().0;
    pool.rows(&format!(
        "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
         \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
         VALUES ('{space}', 'mem_{principal}', 2, 'left:{principal}:mem_{principal}', \
                 '{principal}', 'principal', 1, NULL, 'left', CURRENT_DATE)"
    ))
    .await
    .expect("left");
}

async fn count(pool: &Pool, table: &str, column: &str, value: &str) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"{table}\" WHERE \"{column}\" LIKE '%{value}%'"
    ))
    .await
    .expect("count")[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

/// **The named test.** A registration admitted nowhere is removed after the
/// ttl, with its links and its credential, and one admitted in time is kept
/// however old it is. A young one is kept whether admitted or not.
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_admitted_nowhere_is_removed_after_the_ttl_and_one_admitted_in_time_is_kept()
{
    let pool = prepared("ordo_registrations_sweep").await;
    let old = NOW - TTL - 1;
    enrol(&pool, "usr_lapsed", old).await;
    enrol(&pool, "usr_admitted", old).await;
    enrol(&pool, "usr_young", NOW - 10).await;
    admit(&pool, "usr_admitted").await;
    // An unredeemed ticket for the lapsed person goes with them.
    pool.rows(&format!(
        "INSERT INTO \"$ticket\" (\"hash\", \"purpose\", \"principal\", \"expires_at\") \
         VALUES ('h_lapsed', 'password', 'usr_lapsed', {})",
        NOW + 500
    ))
    .await
    .expect("ticket");

    let single = SinglePool::new(pool.clone());
    let report = sweep(&pool, &single, TTL, NOW, None).await.expect("a pass");
    assert_eq!(report.skipped, None);
    assert_eq!(report.removed, vec!["usr_lapsed".to_string()]);

    assert_eq!(count(&pool, "$principal", "id", "usr_lapsed").await, 0);
    assert_eq!(
        count(&pool, "$authenticator", "principal", "usr_lapsed").await,
        0
    );
    assert_eq!(
        count(&pool, "$credential", "identifier", "usr_lapsed").await,
        0
    );
    assert_eq!(count(&pool, "$ticket", "principal", "usr_lapsed").await, 0);

    for kept in ["usr_admitted", "usr_young"] {
        assert_eq!(count(&pool, "$principal", "id", kept).await, 1, "{kept}");
        assert_eq!(
            count(&pool, "$authenticator", "principal", kept).await,
            1,
            "{kept}"
        );
        assert_eq!(
            count(&pool, "$credential", "identifier", kept).await,
            1,
            "{kept}"
        );
    }

    // A second pass has nothing to do, and the young one goes when it ages.
    let again = sweep(&pool, &single, TTL, NOW, None).await.unwrap();
    assert!(again.removed.is_empty());
    let later = sweep(&pool, &single, TTL, NOW + 2 * TTL, None)
        .await
        .unwrap();
    assert_eq!(later.removed, vec!["usr_young".to_string()]);
    assert_eq!(count(&pool, "$principal", "id", "usr_admitted").await, 1);
}

/// A person who was admitted and then left every tenant is not a
/// registration no tenant admitted. Their account is kept until they
/// delete it (owner ruling 2026-10-06), however long ago they signed up.
/// Before the fix, the sweep read a left seat as no admission and removed
/// them, with a record that said their registration had expired.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_who_was_admitted_and_then_left_every_tenant_is_kept_by_the_sweep() {
    let pool = prepared("ordo_registrations_leaver").await;
    let old = NOW - TTL - 1;
    enrol(&pool, "usr_leaver", old).await;
    enrol(&pool, "usr_lapsed", old).await;
    left(&pool, "usr_leaver").await;

    let single = SinglePool::new(pool.clone());
    let report = sweep(&pool, &single, TTL, NOW, None).await.expect("a pass");
    assert_eq!(report.skipped, None);
    assert_eq!(
        report.removed,
        vec!["usr_lapsed".to_string()],
        "the one nobody ever admitted goes; the leaver stays"
    );
    assert_eq!(count(&pool, "$principal", "id", "usr_leaver").await, 1);
    assert_eq!(
        count(&pool, "$authenticator", "principal", "usr_leaver").await,
        1
    );
    assert_eq!(
        count(&pool, "$credential", "identifier", "usr_leaver").await,
        1
    );
}

/// A resolver with one database that cannot be reached.
struct OneDown {
    reachable: Pool,
}

impl PoolResolver for OneDown {
    fn resolve<'a>(&'a self, _s: &'a ordo_crypto::SpaceId) -> Resolving<'a> {
        let p = self.reachable.clone();
        Box::pin(async move { Ok(p) })
    }
    fn targets<'a>(&'a self) -> Targets<'a> {
        let p = self.reachable.clone();
        Box::pin(async move {
            Ok(vec![
                Target {
                    label: "default".into(),
                    pool: Ok(p),
                },
                Target {
                    label: "db.tenant-b:5432/tenant_b".into(),
                    pool: Err("connection refused".into()),
                },
            ])
        })
    }
}

/// **Never on incomplete knowledge.** A tenant whose database is down may
/// have admitted the person, so the pass removes nobody.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_that_cannot_be_read_ends_the_pass_with_nothing_removed() {
    let pool = prepared("ordo_registrations_unreadable").await;
    enrol(&pool, "usr_lapsed", NOW - TTL - 1).await;

    let resolver = OneDown {
        reachable: pool.clone(),
    };
    let report = sweep(&pool, &resolver, TTL, NOW, None)
        .await
        .expect("a pass");
    assert!(report.removed.is_empty());
    let why = report.skipped.expect("it says why it stopped");
    assert!(why.contains("tenant_b"), "{why}");
    assert_eq!(count(&pool, "$principal", "id", "usr_lapsed").await, 1);
}

/// The lifetime defaults to 90 days, an override is a number, and anything
/// else refuses.
#[test]
fn the_lifetime_defaults_to_ninety_days_and_a_bad_one_refuses() {
    let var = "ORDO_AUTH_REGISTRATION_TTL_SECS";
    let ninety = 90 * 24 * 60 * 60;
    std::env::remove_var(var);
    assert_eq!(ttl_from_env(), Ok(ninety));
    std::env::set_var(var, "  ");
    assert_eq!(ttl_from_env(), Ok(ninety));
    std::env::set_var(var, "604800");
    assert_eq!(ttl_from_env(), Ok(604_800));
    for bad in ["0", "-5", "a week", "1.5"] {
        std::env::set_var(var, bad);
        assert!(ttl_from_env().is_err(), "{bad}");
    }
    std::env::remove_var(var);
}

/// The census line names the lifetime in use and where it came from.
#[test]
fn the_census_line_names_the_lifetime_in_use() {
    let d = describe_ttl(90 * 24 * 60 * 60);
    assert!(
        d.contains("7776000s") && d.contains("90 days") && d.contains("the default"),
        "{d}"
    );
    let o = describe_ttl(604_800);
    assert!(
        o.contains("604800s") && o.contains("7 days") && o.contains("ORDO_AUTH"),
        "{o}"
    );
}

/// A guest's own lifetime (`ORDO_AUTH_GUEST_TTL_SECS`, `plan-auth-federated.md`
/// §7A). A guest that claimed nothing and that no tenant admitted is removed
/// after it. A guest that claimed another sign-in, one that was admitted, a
/// young one and an ordinary registration of the same age are kept.
#[tokio::test(flavor = "multi_thread")]
async fn an_unclaimed_guest_is_swept_after_its_ttl() {
    let pool = prepared("ordo_registrations_guests").await;
    const GUEST_TTL: u64 = 100;
    let old = NOW - GUEST_TTL - 1;
    guest(&pool, "usr_guest_lapsed", old).await;
    guest(&pool, "usr_guest_claimed", old).await;
    link(
        &pool,
        "usr_guest_claimed",
        "oauth2:discord:80351110224678912",
        old + 1,
    )
    .await;
    guest(&pool, "usr_guest_admitted", old).await;
    admit(&pool, "usr_guest_admitted").await;
    guest(&pool, "usr_guest_young", NOW - 10).await;
    enrol(&pool, "usr_person", old).await;

    let single = SinglePool::new(pool.clone());
    let report = sweep_guests(&pool, &single, GUEST_TTL, NOW, None)
        .await
        .expect("a pass");
    assert_eq!(report.skipped, None);
    assert_eq!(report.removed, vec!["usr_guest_lapsed".to_string()]);
    assert_eq!(
        count(&pool, "$principal", "id", "usr_guest_lapsed").await,
        0
    );
    assert_eq!(
        count(&pool, "$authenticator", "principal", "usr_guest_lapsed").await,
        0
    );
    for kept in [
        "usr_guest_claimed",
        "usr_guest_admitted",
        "usr_guest_young",
        "usr_person",
    ] {
        assert_eq!(count(&pool, "$principal", "id", kept).await, 1, "{kept}");
    }

    // The young guest goes when it ages past the guest lifetime; the
    // registration lifetime still covers everyone else.
    let later = sweep_guests(&pool, &single, GUEST_TTL, NOW + GUEST_TTL, None)
        .await
        .unwrap();
    assert_eq!(later.removed, vec!["usr_guest_young".to_string()]);
    let all = sweep(&pool, &single, TTL, NOW + 2 * TTL, None)
        .await
        .unwrap();
    assert_eq!(
        all.removed,
        vec!["usr_guest_claimed".to_string(), "usr_person".to_string()]
    );
}
