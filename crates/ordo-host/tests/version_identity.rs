//! Increment 1 of `plan-versions.md`: a version is named by its source
//! identity, and an upgrade to that naming moves nothing.
//!
//! - A new version is stored under its full source identity, 64 hex.
//! - A version stored before 2026-11-03 keeps its 16-hex name. The registry
//!   fills in its source identity, and applying the same program again
//!   finds it there: already stored, already active, no tenant behind
//!   (board `program-versions-the-core-under-schema` ask 7).
//! - The same source built differently by a release is a new build of the
//!   same version, recorded in `$builds`, not a collision.
//! - A different source under a stored name is still refused.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_commandlog::Timestamp;
use ordo_conformance::Pg;
use ordo_exec::registry::{self, Applied, Artifact, RegistryRefusal};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, Activation, Stored};
use ordo_host::schema_apply::Program;

const PROGRAM: &str = r#"
entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
"#;

/// The registry's tables as they were before `source_id` and `$builds`.
const OLD_REGISTRY: &str = "\
CREATE TABLE \"$artifacts\" ( \
  \"version\" text PRIMARY KEY, \"artifact\" text NOT NULL, \"source\" text NOT NULL, \
  \"ddl\" text NOT NULL, \"applied_at\" bigint NOT NULL, \"actor\" text NOT NULL); \
CREATE TABLE \"$activations\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"version\" text NOT NULL REFERENCES \"$artifacts\" (\"version\"), \
  \"at\" bigint NOT NULL, \"actor\" text NOT NULL)";

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool")
}

fn artifact(p: &Program, version: &str) -> Artifact {
    Artifact {
        version: version.to_string(),
        artifact: ordo_plan::artifact::to_bytes(&p.doc),
        source: p.source.clone(),
        ddl: p.ddl.clone(),
        applied_at: Timestamp(1),
        actor: "test".into(),
    }
}

async fn one(pool: &Pool, sql: &str) -> Option<String> {
    pool.rows(sql)
        .await
        .expect("the query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_version_is_named_by_its_full_source_identity() {
    let pool = fresh("ordo_vid_new").await;
    let program = Program::compile(PROGRAM).expect("compiles");
    let fleet = SinglePool::new(pool.clone());
    let report = apply_op::run_program(&pool, &fleet, &program, vec![], "t", "t", true)
        .await
        .expect("applies");
    assert_eq!(report.version, program.source_id);
    assert_eq!(report.version.len(), 64);
    let builds = one(&pool, "SELECT count(*) FROM \"$builds\"").await;
    assert_eq!(builds.as_deref(), Some("1"), "the build is recorded");
    let release = one(&pool, "SELECT \"ordo_release\" FROM \"$builds\"").await;
    assert_eq!(release.as_deref(), Some(ordo_plan::artifact::ORDO_RELEASE));
}

/// **The upgrade moves nothing.** A deployment whose registry predates
/// source identity is serving the program under its 16-hex name. The same
/// program applied by this release is already stored and already active,
/// and the database's own `$migrations` still names the old version.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_stored_before_keeps_its_name() {
    let pool = fresh("ordo_vid_upgrade").await;
    let program = Program::compile(PROGRAM).expect("compiles");
    let fleet = SinglePool::new(pool.clone());

    // The deployment as the last release left it: the schema created, the
    // artifact stored and active under the build digest's 16-hex name.
    let legacy = ordo_plan::artifact::program_version(&program.doc, &program.ddl);
    let mut old = program.clone();
    old.version = legacy.clone();
    ordo_host::schema_apply::create_schema(&pool, &old, "t", "t")
        .await
        .expect("the schema");
    pool.rows(OLD_REGISTRY).await.expect("the old registry");
    let row = artifact(&program, &legacy);
    pool.rows(&format!(
        "INSERT INTO \"$artifacts\" VALUES ('{}', $${}$$, $${}$$, $${}$$, 1, 'test'); \
         INSERT INTO \"$activations\" (\"version\", \"at\", \"actor\") VALUES ('{}', 1, 'test')",
        row.version, row.artifact, row.source, row.ddl, legacy
    ))
    .await
    .expect("the old rows");

    let report = apply_op::run_program(&pool, &fleet, &program, vec![], "t", "t", true)
        .await
        .expect("applies");
    assert_eq!(report.version, legacy, "the stored name is kept");
    assert_eq!(report.stored, Stored::AlreadyPresent);
    assert!(
        matches!(report.activation, Activation::AlreadyActive { .. }),
        "nothing re-pointed: {:?}",
        report.activation
    );
    let filled = one(
        &pool,
        &format!("SELECT \"source_id\" FROM \"$artifacts\" WHERE \"version\" = '{legacy}'"),
    )
    .await;
    assert_eq!(filled.as_deref(), Some(program.source_id.as_str()));
    let at = ordo_exec::spacedb::recorded_version(&pool)
        .await
        .expect("reads");
    assert_eq!(at, ordo_exec::spacedb::TargetVersion::At(legacy));
}

/// The same source built differently is a new build of the same version.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_build_of_the_same_source_is_not_a_collision() {
    let pool = fresh("ordo_vid_rebuild").await;
    registry::ensure_tables(&pool).await.expect("tables");
    let program = Program::compile(PROGRAM).expect("compiles");

    // What an older release built: the same source, a different DDL.
    let mut older = artifact(&program, &program.source_id);
    older.ddl.push_str("\n-- rendered by an older release\n");
    assert!(matches!(
        registry::store(&pool, &older).await,
        Ok(Applied::Stored(_))
    ));

    let now = artifact(&program, &program.source_id);
    assert!(matches!(
        registry::store(&pool, &now).await,
        Ok(Applied::Rebuilt(_))
    ));
    let stored = registry::get(&pool, &program.source_id)
        .await
        .expect("reads")
        .expect("stored");
    assert_eq!(stored.ddl, program.ddl, "loads read this release's build");
    let builds = one(&pool, "SELECT count(*) FROM \"$builds\"").await;
    assert_eq!(builds.as_deref(), Some("2"), "both builds are recorded");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_different_source_under_a_stored_name_is_refused() {
    let pool = fresh("ordo_vid_collide").await;
    registry::ensure_tables(&pool).await.expect("tables");
    let program = Program::compile(PROGRAM).expect("compiles");
    let name = "0123456789abcdef";
    registry::store(&pool, &artifact(&program, name))
        .await
        .expect("stores");

    let other = Program::compile(&PROGRAM.replace("done:  bool", "done:  bool\n  due: int?"))
        .expect("compiles");
    assert!(matches!(
        registry::store(&pool, &artifact(&other, name)).await,
        Err(RegistryRefusal::VersionCollision { .. })
    ));
}
