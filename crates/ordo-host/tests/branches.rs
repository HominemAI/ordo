//! Dev branches are tenants on their own version (`plan-versions.md` §3.3,
//! §4 row 5; `plan-environments.md` §2.2).
//!
//! A real control database, with every tenant given a database of its own
//! on the same cluster, as the runtime assembles a deployment.
//!
//! What it proves:
//!
//! - 2 branches are 2 tenants, each on its own version, each in a database
//!   of its own, while the deployment keeps serving main's version and
//!   main's database never sees a branch's tables;
//! - an apply again moves the branch's tenant to the new version;
//! - branch names map to tenant names strictly, and a collision is refused
//!   naming `ordo branches delete`;
//! - a branch is removed only by its delete, which drops its database and
//!   frees the name; an unused branch is listed as idle and kept;
//! - a deployment past the first step of its chain refuses a branch naming
//!   dev, and a server with no tenant databases refuses one naming why.
//!
//! A regression looks like a branch's field in main's database, a branch
//! tenant on main's version, a collision applied, or a delete that leaves
//! the database standing.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::{KeyProvider, LocalKeyProvider, MemoryKeyStore, SpaceId};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, ApplyOps};
use ordo_host::branches::{self, BranchRefusal};
use ordo_host::environments::Environments;
use ordo_host::platform_ops;
use ordo_host::tenant_ops::{Routing, TenantCluster, TenantOps};
use ordo_host::tenants;
use ordo_host::versions::{Provenance, Versions};

const ADA: &str = "p_ada";
const HOME: &str = "ordo_home";

const MAIN: &str = "domain notes @ 0.1.0\n\n\
    entity notes {\n  title: text\n  body: text = \"\"\n}\n\n\
    op allNotes unbounded =\n  notes[title, body]\n";

/// Branch A: a field main does not have.
fn branch_a(version: &str) -> String {
    MAIN.replace("@ 0.1.0", &format!("@ {version}")).replace(
        "  body: text = \"\"\n",
        "  body: text = \"\"\n  starred: bool = false\n",
    ) + "\nop starredNotes unbounded =\n  notes(starred == true)[title]\n"
}

/// Branch B: an entity main does not have.
fn branch_b() -> String {
    MAIN.replace("@ 0.1.0", "@ 0.3.0")
        + "\nentity labels {\n  name: text\n}\n\nop allLabels unbounded =\n  labels[name]\n"
}

struct Dev {
    pool: Pool,
    pg: Pg,
    apply: Arc<ApplyOps>,
    tenants: Arc<TenantOps>,
    reap: ordo_host::reap_op::ReapOps,
}

fn keys() -> Arc<dyn KeyProvider> {
    let k: Arc<dyn KeyProvider> = Arc::new(LocalKeyProvider::new(
        b"branches master key",
        Box::new(MemoryKeyStore::new()),
    ));
    k.create_space(&SpaceId(HOME.to_string())).expect("home");
    let _ = k.create_space(&SpaceId(ordo_host::provision::SYSTEM_SPACE.to_string()));
    k
}

/// Dev: main on 0.1.0, every tenant a database of its own when `routed`.
async fn dev(stem: &str, routed: bool) -> Dev {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 4,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    let keys = keys();
    let own = PoolConfig::from_env().password.clone();
    let creds = ordo_host::runtime::tenant_credentials(None, own);
    let template = PoolConfig {
        size: 2,
        ..PoolConfig::from_env()
    };
    let resolver: Arc<dyn ordo_exec::route::PoolResolver> =
        Arc::new(ordo_exec::route::AttachedPools::new(
            pool.clone(),
            pool.clone(),
            template.clone(),
            creds.clone(),
        ));
    apply_op::run(
        &pool,
        resolver.as_ref(),
        MAIN,
        &apply_op::CodeFiles::new(),
        ADA,
        "t",
        true,
    )
    .await
    .expect("main applied");
    let f = Arc::new({
        let mut f = Fixture::from_source(MAIN, "").unwrap();
        f.keys = keys.clone();
        f
    });
    let serving = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "0.1.0",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .unwrap();
    let tenants = if routed {
        TenantOps::with_routing(
            pool.clone(),
            keys.clone(),
            HOME,
            Routing {
                serving,
                credentials: creds.clone(),
                template: template.clone(),
                hosts: ordo_host::provision::HostAllowlist::of(&PoolConfig::from_env().host),
                default_cluster: TenantCluster::from_env(&PoolConfig::from_env()),
                stream: None,
            },
        )
    } else {
        TenantOps::new(pool.clone(), keys.clone(), HOME)
    };
    let reap = ordo_host::reap_op::ReapOps {
        pool: pool.clone(),
        keys: keys.clone(),
        home: HOME.to_string(),
        credentials: creds,
        template,
        stream: None,
        now: Arc::new(ordo_host::runtime::now_secs),
    };
    Dev {
        apply: ApplyOps::new(pool.clone(), resolver, HOME),
        pool,
        pg,
        tenants,
        reap,
    }
}

async fn apply(d: &Dev, branch: &str, source: &str) -> Result<branches::Applied, BranchRefusal> {
    branches::apply(
        &d.apply,
        &d.tenants,
        branch,
        source,
        &apply_op::CodeFiles::new(),
        &Provenance::default(),
        ADA,
        ordo_host::runtime::now_secs(),
    )
    .await
}

/// The columns of `table` in database `db`.
async fn columns(db: &str, table: &str) -> Vec<String> {
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 1,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool on the branch's database");
    pool.rows(&format!(
        "SELECT column_name FROM information_schema.columns WHERE table_name = '{table}' \
         ORDER BY column_name"
    ))
    .await
    .unwrap()
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect()
}

async fn database_of(d: &Dev, space: &str) -> String {
    ordo_exec::spacedb::lookup(&d.pool, &SpaceId(space.to_string()))
        .await
        .unwrap()
        .expect("an attachment")
        .db
}

fn drop_db(d: &Dev, db: &str) {
    let _ =
        d.pg.exec(&format!("DROP DATABASE IF EXISTS \"{db}\" WITH (FORCE)"));
}

#[tokio::test(flavor = "multi_thread")]
async fn two_branches_are_served_side_by_side_on_their_own_versions() {
    let d = dev("ordo_branches_two", true).await;
    let a = apply(&d, "feature/a", &branch_a("0.2.0")).await.expect("a");
    let b = apply(&d, "feature-b", &branch_b()).await.expect("b");
    assert!(a.created && b.created);
    assert_eq!(a.branch.tenant, "branch-feature-a");
    assert_eq!(b.branch.tenant, "branch-feature-b");
    assert_eq!(a.report.version, "0.2.0");
    assert_eq!(b.report.version, "0.3.0");
    // Stored, never activated: the deployment serves main.
    assert_eq!(
        a.report.schema,
        apply_op::SchemaOutcome::Untouched,
        "main's database is not touched"
    );
    let versions = Versions::on(&d.pool);
    assert_eq!(
        versions.active_version().await.unwrap().as_deref(),
        Some("0.1.0")
    );
    // Each tenant is pinned to its branch's version.
    let pin = |s: &str| {
        let s = s.to_string();
        let v = Versions::on(&d.pool);
        async move { v.pin_of(&s).await.unwrap().and_then(|p| p.version) }
    };
    assert_eq!(pin(&a.branch.space).await.as_deref(), Some("0.2.0"));
    assert_eq!(pin(&b.branch.space).await.as_deref(), Some("0.3.0"));
    // Each in a database of its own, holding its branch's tables.
    let db_a = database_of(&d, &a.branch.space).await;
    let db_b = database_of(&d, &b.branch.space).await;
    assert_ne!(db_a, db_b);
    assert!(columns(&db_a, "notes")
        .await
        .contains(&"starred".to_string()));
    assert!(!columns(&db_b, "notes")
        .await
        .contains(&"starred".to_string()));
    assert!(columns(&db_b, "labels").await.contains(&"name".to_string()));
    // Main's database has neither branch's additions.
    assert!(!columns(&d.pg.db, "notes")
        .await
        .contains(&"starred".to_string()));
    assert!(columns(&d.pg.db, "labels").await.is_empty());
    // The person applying is the branch's founding owner.
    let owners = Pool::connect(PoolConfig {
        db: db_a.clone(),
        size: 1,
        ..PoolConfig::from_env()
    })
    .await
    .unwrap()
    .rows(&format!(
        "SELECT \"principal\", \"role\" FROM \"members\" WHERE \"$space\" = '{}'",
        a.branch.space
    ))
    .await
    .unwrap();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0][0].as_deref(), Some(ADA));
    assert_eq!(owners[0][1].as_deref(), Some("owner"));

    // An apply again moves the branch's tenant; nothing new is made.
    let again = apply(&d, "feature/a", &branch_a("0.2.1"))
        .await
        .expect("again");
    assert!(!again.created);
    assert_eq!(again.from.as_deref(), Some("0.2.0"));
    assert_eq!(again.branch.space, a.branch.space);
    assert_eq!(pin(&a.branch.space).await.as_deref(), Some("0.2.1"));
    assert_eq!(
        versions.active_version().await.unwrap().as_deref(),
        Some("0.1.0")
    );

    // Listed, and idle only once unused for the setting.
    let all = branches::list(&d.pool).await.unwrap();
    assert_eq!(all.len(), 2);
    let now = ordo_host::runtime::now_secs();
    assert!(all
        .iter()
        .all(|b| !b.idle(now, branches::IDLE_DEFAULT_SECS)));
    assert!(all.iter().all(|b| b.idle(
        now + branches::IDLE_DEFAULT_SECS,
        branches::IDLE_DEFAULT_SECS
    )));

    // A name that maps to a tenant another branch holds is refused.
    let e = apply(&d, "Feature/A", &branch_a("0.2.1"))
        .await
        .unwrap_err();
    assert!(matches!(e, BranchRefusal::Collision { .. }), "{e}");
    assert!(
        e.to_string()
            .contains("ordo branches delete feature/a --reason"),
        "{e}"
    );

    // Deleted only by command: the tenant is reaped, its database dropped.
    let e = branches::delete(&d.apply, &d.reap, "feature/a", " ", ADA, now)
        .await
        .unwrap_err();
    assert!(matches!(e, BranchRefusal::Reason(_)), "{e}");
    branches::delete(&d.apply, &d.reap, "feature/a", "merged", ADA, now)
        .await
        .expect("deleted");
    let t = tenants::lookup(&d.pool, &a.branch.space)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, tenants::State::Reaped);
    let left =
        d.pg.query(&format!(
            "SELECT count(*) FROM pg_database WHERE datname = '{db_a}'"
        ))
        .unwrap();
    assert_eq!(
        left[0][0].as_deref(),
        Some("0"),
        "the branch's database is dropped"
    );
    assert_eq!(branches::list(&d.pool).await.unwrap().len(), 1);
    let e = branches::delete(&d.apply, &d.reap, "feature/a", "again", ADA, now)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("ordo branches list"), "{e}");
    // The name is free again.
    let back = apply(&d, "Feature/A", &branch_a("0.2.1"))
        .await
        .expect("back");
    assert!(back.created);
    assert_ne!(back.branch.space, a.branch.space);

    drop_db(&d, &db_b);
    drop_db(&d, &database_of(&d, &back.branch.space).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_is_refused_past_dev_and_without_tenant_databases() {
    // No tenant databases: refused naming why, nothing stored.
    let d = dev("ordo_branches_nodb", false).await;
    let e = apply(&d, "feature-x", &branch_a("0.2.0"))
        .await
        .unwrap_err();
    assert!(matches!(e, BranchRefusal::NoDatabases), "{e}");
    assert!(e.to_string().contains("database of its own"), "{e}");
    assert!(Versions::on(&d.pool).get("0.2.0").await.unwrap().is_none());

    // A name that is not one.
    let e = apply(&d, "x y", &branch_a("0.2.0")).await.unwrap_err();
    assert!(e.to_string().contains("not a branch name"), "{e}");

    // A deployment past the first step: refused naming dev.
    let envs = Environments::on(&d.pool);
    let later = ordo_host::runtime::now_secs() + 60;
    envs.register("dev", Some(1), ADA, "the chain", later)
        .await
        .expect("dev registered");
    let e = apply(&d, "feature-x", &branch_a("0.2.0"))
        .await
        .unwrap_err();
    assert!(matches!(e, BranchRefusal::NotDev(_)), "{e}");
    assert!(e.to_string().contains("branches live in dev"), "{e}");
    assert!(e.to_string().contains("`dev`"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_that_is_not_a_branch_keeps_its_name() {
    let d = dev("ordo_branches_taken", true).await;
    let id = tenants::mint_space_id();
    tenants::reserve(&d.pool, &id, "branch-feature-z", ADA, 100, None)
        .await
        .unwrap();
    tenants::activate(&d.pool, &id, 100).await.unwrap();
    let e = apply(&d, "feature-z", &branch_a("0.2.0"))
        .await
        .unwrap_err();
    assert!(matches!(e, BranchRefusal::TenantTaken { .. }), "{e}");
    assert!(e.to_string().contains("Rename the branch"), "{e}");
}
