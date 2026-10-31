//! 5f — `ordo apply`, the Postgres artifact registry, and hot swap.
//!
//! The properties under test are the ones `engine-plan.md`'s F6 done-when
//! names, plus the two defects building it found:
//!
//! - a hot swap changes no in-flight request's version;
//! - rollback is a registry re-point;
//! - **a swap cannot install a registry the load gate has not seen** — the
//!   hole `ordo_api::RegistryHandle::swap` left open;
//! - **the shape report and the catalog move with the registry**, rather
//!   than remaining the boot-time snapshot they were.
//!
//! Everything runs against a real cluster, in databases prefixed
//! `ordo_5f_`, one per test, because these suites drop and rebuild schemas.

use std::sync::Arc;

use ordo_commandlog::Timestamp;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::registry::{self, Activated, Applied, Artifact, RegistryRefusal};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::deploy::{self, Deployment, LoadRefusal};

const WIDE: &str = include_str!("../../ordo-conformance/fixtures/wide.ordo");

/// A second program: the same schema, one more op. The DDL is identical, so
/// a swap between the two is purely a registry change — which is what makes
/// it a clean test of the swap rather than of a migration.
fn wide_plus_one() -> String {
    format!("{WIDE}\n\nop newestOrgs unbounded =\n  orgs[name, region]\n  sort -founded\n")
}

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    }
}

/// A database of this suite's own, with the wide fixture's schema applied.
/// `Pg::named` rather than `Pg::default`: test binaries run concurrently and
/// `Fixture::reset` opens with `DROP SCHEMA public CASCADE`.
fn prepared(db: &str) -> (Arc<Fixture>, Pg) {
    let pg = Pg::named(db);
    let f = Fixture::load_wide().expect("the wide fixture compiles");
    f.reset(&pg).expect("schema applied");
    (Arc::new(f), pg)
}

/// Compile a program the way `ordo apply` does and build the row it stores.
///
/// Through `Fixture::from_source` rather than `ordo_cli`, so this suite
/// does not pull the binary crate in to reach three lines of compiler.
fn artifact_of(source: &str, actor: &str) -> Artifact {
    let f = Fixture::from_source(source, "").expect("the program compiles");
    let (schema, registry) = (&f.schema, &f.registry);
    let doc = ordo_plan::artifact::program_artifact(schema, registry).expect("artifact");
    let ddl = ordo_schema::ddl_for(schema).expect("ddl");
    Artifact {
        version: ordo_plan::artifact::program_version(&doc, &ddl),
        artifact: ordo_plan::artifact::to_bytes(&doc),
        source: source.to_string(),
        ddl,
        applied_at: Timestamp(1),
        actor: actor.to_string(),
    }
}

async fn pool_for(db: &str) -> Pool {
    let pool = Pool::connect(cfg(db)).await.expect("pool boots");
    registry::ensure_tables(&pool)
        .await
        .expect("registry tables");
    pool
}

fn keys(f: &Fixture) -> Arc<dyn ordo_crypto::KeyProvider> {
    f.keys.clone()
}

// ── the registry ─────────────────────────────────────────────────────────

/// Store, activate, roll back. The pointer is an append-only log, so the
/// rollback reads forwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_rollback_is_a_re_point_and_leaves_a_record() {
    let db = "ordo_5f_registry";
    let (_f, _pg) = prepared(db);
    let pool = pool_for(db).await;

    let one = artifact_of(WIDE, "operator:a");
    let two = artifact_of(&wide_plus_one(), "operator:b");
    assert_ne!(
        one.version, two.version,
        "two different programs must not share a version hash"
    );

    assert_eq!(
        registry::store(&pool, &one).await.expect("stored"),
        Applied::Stored(one.version.clone())
    );
    // Applying the same bytes twice is the ordinary case for a deploy
    // script that reran, and it is reported rather than silently ignored.
    assert_eq!(
        registry::store(&pool, &one).await.expect("stored"),
        Applied::AlreadyStored(one.version.clone())
    );
    registry::store(&pool, &two).await.expect("stored");

    registry::activate(&pool, &one.version, "operator:a", Timestamp(10))
        .await
        .expect("activated");
    assert_eq!(
        registry::activate(&pool, &two.version, "operator:b", Timestamp(20))
            .await
            .expect("activated"),
        Activated::Switched {
            from: Some(one.version.clone()),
            to: two.version.clone(),
        }
    );

    // The rollback: the same call, naming the earlier version.
    assert_eq!(
        registry::activate(&pool, &one.version, "operator:c", Timestamp(30))
            .await
            .expect("rolled back"),
        Activated::Switched {
            from: Some(two.version.clone()),
            to: one.version.clone(),
        }
    );
    assert_eq!(
        registry::active_version(&pool).await.unwrap().as_deref(),
        Some(one.version.as_str())
    );

    // History reads forwards: three re-points, newest first, and the
    // rollback is one of them rather than an erasure of the deploy it
    // undid. An operator asking "what was serving at 25?" can answer.
    let log = registry::activations(&pool, 10).await.expect("log");
    let versions: Vec<&str> = log.iter().map(|a| a.version.as_str()).collect();
    assert_eq!(
        versions,
        vec![
            one.version.as_str(),
            two.version.as_str(),
            one.version.as_str()
        ]
    );
    assert_eq!(log[0].actor, "operator:c");

    // Both artifacts are retained. `implementation.md`'s "old artifacts are
    // retained, so rollback = re-point" is only true if this holds.
    assert_eq!(registry::stored(&pool).await.expect("stored").len(), 2);
}

/// A version hash is a content address, so the same name with different
/// bytes is refused rather than overwritten.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_may_not_be_re_stored_with_different_bytes() {
    let db = "ordo_5f_collision";
    let (_f, _pg) = prepared(db);
    let pool = pool_for(db).await;

    let mut a = artifact_of(WIDE, "operator:a");
    registry::store(&pool, &a).await.expect("stored");
    a.artifact.push_str("  \n");
    let refusal = registry::store(&pool, &a)
        .await
        .expect_err("a changed document under a stored version is refused");
    assert!(
        matches!(refusal, RegistryRefusal::VersionCollision { .. }),
        "{refusal}"
    );

    // And nothing can be pointed at a version nobody stored — refused in
    // the code so the operator gets their own string back, and refused by
    // the foreign key underneath so it is not merely a convention.
    let refusal = registry::activate(&pool, "0000000000000000", "op", Timestamp(1))
        .await
        .expect_err("unknown version");
    assert!(
        matches!(refusal, RegistryRefusal::UnknownVersion(v) if v == "0000000000000000"),
        "expected UnknownVersion"
    );
}

// ── the swap ─────────────────────────────────────────────────────────────

/// The binary boots with `boot_with_platform_ops`, so two properties of the
/// swap now carry weight:
///
/// - **a swap keeps the platform ops.** They live beside the `Loaded`, not
///   in it, so installing a new version must leave `std.space.callersOf`
///   served;
/// - **a swap re-vets the names.** A version that compiled a name the
///   platform serves is refused and the old version keeps serving, on the
///   terms the boot applies. Otherwise the boot's refusal would be a check a
///   deploy could walk around.
///
/// The collision is set up from the platform's side, as
/// `tests/callers_of.rs` does, because no program can spell `std.space.*`
/// (`space` is a keyword): a platform spec named `newestOrgs` is clear of
/// the first version and collides with the second.
#[tokio::test(flavor = "multi_thread")]
async fn platform_ops_survive_a_swap_and_a_colliding_version_is_refused() {
    use ordo_api::OpRegistry;
    use ordo_host::platform_ops;

    let db = "ordo_5f_platform_swap";
    let (f, _pg) = prepared(db);
    let pool = pool_for(db).await;
    let space = ordo_conformance::fixture_space();

    let one = artifact_of(WIDE, "operator:a");
    let two = artifact_of(&wide_plus_one(), "operator:a");
    registry::store(&pool, &one).await.expect("stored");
    registry::store(&pool, &two).await.expect("stored");
    registry::activate(&pool, &one.version, "operator:a", Timestamp(1))
        .await
        .expect("activated");
    let load = |version: String| {
        let pool = pool.clone();
        let keys = keys(&f);
        let space = space.clone();
        async move {
            deploy::load_version(&pool, &cfg(db), &version, keys, space)
                .await
                .expect("gated")
        }
    };

    // The real platform list: a swap keeps it.
    let node =
        Deployment::boot_with_platform_ops(load(one.version.clone()).await, platform_ops::specs())
            .expect("the wide program compiles no reserved name");
    assert!(node.get(platform_ops::CALLERS_OF).is_some());
    assert!(matches!(
        node.swap(load(two.version.clone()).await),
        deploy::Swapped::Installed { .. }
    ));
    assert!(
        node.get(platform_ops::CALLERS_OF).is_some(),
        "the swap dropped the platform ops"
    );
    assert!(node.get("newestOrgs").is_some());

    // A platform list that the second version collides with.
    let mut colliding = platform_ops::specs();
    colliding[0].name = "newestOrgs".to_string();
    let node = Deployment::boot_with_platform_ops(load(one.version.clone()).await, colliding)
        .expect("the first version does not compile `newestOrgs`");
    match node.swap(load(two.version.clone()).await) {
        deploy::Swapped::Refused(why) => assert!(why.contains("newestOrgs"), "{why}"),
        other => panic!("a colliding version was not refused: {other:?}"),
    }
    assert_eq!(
        node.current().version,
        one.version,
        "a refused swap must leave the old version serving"
    );
}

/// The whole path: apply two versions, boot on the first, swap to the
/// second, roll back. The registry, the executor's program, the manifests
/// and the shape report all move together, and an in-flight request does
/// not.
#[tokio::test(flavor = "multi_thread")]
async fn a_hot_swap_moves_the_whole_version_and_a_rollback_moves_it_back() {
    let db = "ordo_5f_swap";
    let (f, _pg) = prepared(db);
    let pool = pool_for(db).await;
    let space = ordo_conformance::fixture_space();

    let one = artifact_of(WIDE, "operator:a");
    let two = artifact_of(&wide_plus_one(), "operator:a");
    registry::store(&pool, &one).await.expect("stored");
    registry::store(&pool, &two).await.expect("stored");
    registry::activate(&pool, &one.version, "operator:a", Timestamp(1))
        .await
        .expect("activated");

    let first = deploy::load_active(&pool, &cfg(db), keys(&f), space.clone())
        .await
        .expect("the active version passes the gate");
    assert_eq!(first.version(), one.version);
    assert!(
        !first.report().verified.is_empty(),
        "the gate must have covered something: {}",
        first.report().summary()
    );
    let node = Deployment::boot(first);

    // Before the swap: the door does not know the new op.
    use ordo_api::OpRegistry;
    assert!(node.get("newestOrgs").is_none());
    let before = node.current();

    // The nudge a replica would receive names the version, and loading it
    // is the same gated path as the boot.
    registry::activate(&pool, &two.version, "operator:a", Timestamp(2))
        .await
        .expect("activated");
    let candidate = deploy::load_version(&pool, &cfg(db), &two.version, keys(&f), space.clone())
        .await
        .expect("the new version passes the gate");
    assert_eq!(
        node.swap(candidate),
        deploy::Swapped::Installed {
            from: one.version.clone(),
            to: two.version.clone(),
        }
    );

    // Everything moved, in one write.
    let after = node.current();
    assert!(node.get("newestOrgs").is_some(), "the door sees the new op");
    assert_eq!(after.version, two.version);
    assert!(
        after.manifests.iter().any(|(n, _)| n == "newestOrgs"),
        "the catalog republishes with the swap — it is a rendering of the \
         same pointer the swap moved"
    );
    assert!(
        after
            .report
            .as_deref()
            .expect("a swapped-in version carries its own report")
            .verified
            .iter()
            .any(|o| o == "newestOrgs"),
        "the shape report must be about the version that is serving, not \
         the one that booted"
    );

    // The in-flight request's handle is untouched. `before` is what a call
    // that started before the swap is still holding: its registry, its
    // manifests, its executor's program.
    assert_eq!(before.version, one.version);
    assert!(before.specs.get("newestOrgs").is_none());
    assert!(!before.manifests.iter().any(|(n, _)| n == "newestOrgs"));

    // Rollback: re-point, load, swap. No new mechanism.
    registry::activate(&pool, &one.version, "operator:a", Timestamp(3))
        .await
        .expect("rolled back");
    let back = deploy::load_active(&pool, &cfg(db), keys(&f), space)
        .await
        .expect("the prior version still passes the gate");
    assert_eq!(
        node.swap(back),
        deploy::Swapped::Installed {
            from: two.version.clone(),
            to: one.version.clone(),
        }
    );
    assert!(node.get("newestOrgs").is_none());
    assert_eq!(node.current().version, one.version);
}

/// The gate governs the swap, not just the boot.
///
/// This is the hole `RegistryHandle::swap` left: it took any
/// `Arc<dyn OpRegistry>`, so a deployment could boot gated and then install
/// something no gate had seen. `Deployment::swap` takes a `Verified` and
/// nothing else, and the only constructors for one run the gate — so the
/// property is that a divergent artifact cannot *produce* the value the
/// swap requires.
#[tokio::test(flavor = "multi_thread")]
async fn a_swap_cannot_install_a_version_the_gate_refused() {
    let db = "ordo_5f_gate";
    let (f, _pg) = prepared(db);
    let pool = pool_for(db).await;
    let space = ordo_conformance::fixture_space();

    let good = artifact_of(WIDE, "operator:a");
    registry::store(&pool, &good).await.expect("stored");
    registry::activate(&pool, &good.version, "operator:a", Timestamp(1))
        .await
        .expect("activated");
    let node = Deployment::boot(
        deploy::load_active(&pool, &cfg(db), keys(&f), space.clone())
            .await
            .expect("gated"),
    );
    let serving = node.current().version.clone();

    // A version whose asserted schema the live database does not have —
    // the realistic hot-swap failure: an artifact deployed before its
    // migration ran. It verifies perfectly against its own DDL, which is
    // exactly why the empty shadow alone cannot catch it.
    let mut ahead = artifact_of(WIDE, "operator:a");
    ahead.version = "deadbeefdeadbeef".into();
    ahead.ddl.push_str(
        "; CREATE TABLE \"invoices\" (\"$space\" text NOT NULL, \"id\" text NOT NULL, \
         \"total\" bigint NOT NULL, PRIMARY KEY (\"$space\", \"id\"))",
    );
    registry::store(&pool, &ahead).await.expect("stored");
    let refusal = deploy::load_version(&pool, &cfg(db), &ahead.version, keys(&f), space.clone())
        .await
        .expect_err("an artifact ahead of its migration must not be installable");
    let text = refusal.to_string();
    assert!(
        matches!(refusal, LoadRefusal::SchemaDrift { .. }),
        "expected schema drift, got {text}"
    );
    assert!(
        text.contains("invoices"),
        "the refusal must name what is missing: {text}"
    );

    // A version whose source does not compile under this binary — the
    // platform-upgrade case. Refused by name, not by panic.
    let mut broken = artifact_of(WIDE, "operator:a");
    broken.version = "cafebabecafebabe".into();
    broken.source = "entity oops { this is not a program }".into();
    registry::store(&pool, &broken).await.expect("stored");
    let refusal = deploy::load_version(&pool, &cfg(db), &broken.version, keys(&f), space)
        .await
        .expect_err("a version that does not compile must not be installable");
    assert!(matches!(refusal, LoadRefusal::Compile { .. }), "{refusal}");

    // Two refused loads later, the node is still serving what it was. A
    // node that cannot load a new version must not stop serving the old
    // one.
    assert_eq!(node.current().version, serving);
}

/// A node pointed at a database nobody has applied to says so, rather than
/// inventing an empty program.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_no_active_version_refuses_by_name() {
    let db = "ordo_5f_empty";
    let (f, _pg) = prepared(db);
    let pool = pool_for(db).await;
    let refusal = deploy::load_active(&pool, &cfg(db), keys(&f), ordo_conformance::fixture_space())
        .await
        .expect_err("nothing is active");
    assert!(matches!(refusal, LoadRefusal::NoActiveVersion), "{refusal}");
    assert!(refusal.to_string().contains("ordo apply"));
}
