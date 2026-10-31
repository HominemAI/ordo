//! 18d R-S4 and R-S6 — **a tenant whose database is behind the serving
//! artifact is held closed, and named.**
//!
//! Serving a tenant from a database one version behind means a program
//! referencing tables that are not there. That fails per statement, late,
//! and differently for each request, which is the worst available shape:
//! the deployment looks healthy, the tenant does not work, and the error
//! an operator sees is whichever column happened to be touched first.
//!
//! 17aa's register gives the honest answer instead. The space is
//! quarantined with `schema behind` as its reason, every other tenant and
//! the control plane keep serving, and `/health/ready` names it in the one
//! list T5 assembles.
//!
//! # The four cases that must NOT close a space
//!
//! Each is a way a naive version comparison turns a working deployment
//! into a dead one, so each has a test:
//!
//! - **version unknown**: a database provisioned before R-S1 has a schema
//!   and no `$migrations` row. Nothing here guesses what it holds,
//!   because a guess written down is indistinguishable afterwards from a
//!   version that was really applied.
//! - **no active program**: a deployment waiting for its first `ordo
//!   apply` carries `(none)`, which no target can be at or behind.
//! - **an unnamed version**: the same absence spelled `""`. This one was
//!   **missed on the first pass** and closed three restore suites; it is
//!   the reason this list is four long rather than three.
//! - **at the serving version**: the ordinary case, and the one that
//!   would be caught by an inverted comparison.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::SpaceId;
use ordo_exec::route::{PoolResolver, SinglePool};
use ordo_exec::witness::Witness;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::deploy::Deployment;
use ordo_host::spaces::Spaces;

const SPACE: &str = "conformance";

async fn pool_on(db: &str) -> (Arc<Fixture>, Pool) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    let pg = Pg::named(db);
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    (f, pool)
}

/// A deployment pinned to `version`, over `pool`.
fn deployment_at(version: &str, f: Arc<Fixture>, pool: &Pool) -> Arc<Deployment> {
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

fn spaces_over(deployment: Arc<Deployment>, pool: &Pool) -> Arc<Spaces> {
    Spaces::routed(
        deployment,
        Arc::new(SinglePool::new(pool.clone())) as Arc<dyn PoolResolver>,
        Witness::new(pool.clone()),
    )
}

/// Write the row that says which version this database's tables are.
async fn record_version(pool: &Pool, version: &str) {
    ordo_exec::migrate::ensure_migrations_table(pool)
        .await
        .expect("$migrations");
    ordo_exec::migrate::record_migration(
        pool,
        &ordo_exec::migrate::MigrationRecord {
            seq: 0,
            kind: "create".into(),
            from_version: None,
            to_version: version.to_string(),
            actor: "test".into(),
            at: 1_000,
            changes: String::new(),
            ddl: String::new(),
            phase: "applied".into(),
            note: String::new(),
        },
    )
    .await
    .expect("the version row");
}

/// **A database at another version closes its space, with a reason that
/// names both versions.**
#[tokio::test(flavor = "multi_thread")]
async fn a_target_behind_the_artifact_quarantines_its_tenants() {
    let (f, pool) = pool_on("ordo_rs4_behind").await;
    record_version(&pool, "v_old").await;
    let spaces = spaces_over(deployment_at("v_new", f, &pool), &pool);

    let verdict = spaces.reconcile(SPACE).await;
    assert!(
        !verdict.serves(),
        "a space whose database is at another version must not serve: {}",
        verdict.why()
    );
    let why = verdict.why();
    assert!(
        why.contains("schema behind") && why.contains("v_old") && why.contains("v_new"),
        "the reason must name both versions so an operator can tell which \
         way round it is: {why}"
    );

    // And it is in the one list `/health/ready` publishes, rather than
    // only in a verdict somebody had to ask for.
    let held: Vec<String> = spaces.quarantined().into_iter().map(|(s, _)| s).collect();
    assert_eq!(held, vec![SPACE.to_string()], "{why}");

    // The door refuses it too, through the same question, so the boot's
    // answer and the first request's answer cannot disagree.
    assert!(
        spaces.resolve(SPACE).await.is_err(),
        "a quarantined space must not resolve"
    );
}

/// **A database at the serving version serves.** The inverted-comparison
/// case, which would close every tenant on a healthy deployment.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_at_the_serving_version_serves() {
    let (f, pool) = pool_on("ordo_rs4_current").await;
    record_version(&pool, "v_same").await;
    let spaces = spaces_over(deployment_at("v_same", f, &pool), &pool);

    let verdict = spaces.reconcile(SPACE).await;
    assert!(
        verdict.serves(),
        "a database at the serving version must serve: {}",
        verdict.why()
    );
    assert!(spaces.quarantined().is_empty());
}

/// **A database from before the version row serves**, because nothing
/// here guesses what it holds.
///
/// This is the case that makes the difference between a comparison and a
/// presumption. Every database provisioned before R-S1 is in this state,
/// so treating "no row" as "behind" would quarantine every existing
/// tenant on the first boot after the upgrade.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_with_no_version_row_is_not_treated_as_behind() {
    let (f, pool) = pool_on("ordo_rs4_unknown").await;
    // Deliberately no `record_version`: this is the pre-R-S1 shape.
    let spaces = spaces_over(deployment_at("v_new", f, &pool), &pool);

    let verdict = spaces.reconcile(SPACE).await;
    assert!(
        verdict.serves(),
        "version unknown is not version behind: {}",
        verdict.why()
    );
    assert!(spaces.quarantined().is_empty());
}

/// **A deployment with no active program quarantines nobody.**
///
/// `Deployment::empty` carries `(none)`, which is not a version any
/// target can be at or behind. Without this, the ordinary first boot
/// against a database awaiting its first `ordo apply` would close every
/// space on it.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_with_no_program_quarantines_nobody() {
    let (f, pool) = pool_on("ordo_rs4_noprogram").await;
    record_version(&pool, "v_whatever").await;
    let spaces = spaces_over(deployment_at("(none)", f, &pool), &pool);

    let verdict = spaces.reconcile(SPACE).await;
    assert!(
        verdict.serves(),
        "a deployment waiting for its first apply must not close anybody: {}",
        verdict.why()
    );
    assert!(spaces.quarantined().is_empty());
}

/// **A deployment whose version is the empty string quarantines nobody
/// either**, which is the same absence as `(none)` spelled differently.
///
/// This case is not hypothetical and the test exists because it was
/// missed. The first version of this guard checked only for `(none)`,
/// and an embedded or pinned deployment can carry `""`. That closed
/// three restore suites on the day it landed, with the refusal reading
/// *"serves ``"* — a comparison against nothing at all, which is the
/// tell. A deployment that cannot name its own version has no basis to
/// call any target behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_with_an_unnamed_version_quarantines_nobody() {
    let (f, pool) = pool_on("ordo_rs4_unnamed").await;
    record_version(&pool, "v_real").await;
    let spaces = spaces_over(deployment_at("", f, &pool), &pool);

    let verdict = spaces.reconcile(SPACE).await;
    assert!(
        verdict.serves(),
        "an unnamed serving version is an absence, not a version to be \
         behind: {}",
        verdict.why()
    );
    assert!(spaces.quarantined().is_empty());
}

/// **A recovered target lifts its own quarantine**, on the next resolve
/// and with no restart, which is the property the register has for every
/// other reason it holds.
#[tokio::test(flavor = "multi_thread")]
async fn an_applied_target_lifts_its_own_quarantine() {
    let (f, pool) = pool_on("ordo_rs4_lifts").await;
    record_version(&pool, "v_old").await;
    let spaces = spaces_over(deployment_at("v_new", f, &pool), &pool);

    assert!(!spaces.reconcile(SPACE).await.serves());
    assert_eq!(spaces.quarantined().len(), 1);

    // The apply reaches this database: its row now names the serving
    // version.
    record_version(&pool, "v_new").await;

    assert!(
        spaces.resolve(&SpaceId(SPACE.to_string()).0).await.is_ok(),
        "an applied target must serve again without a restart"
    );
    assert!(
        spaces.quarantined().is_empty(),
        "and its quarantine must lift"
    );
}
