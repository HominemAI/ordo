//! Increment 1 of `plan-versions.md`: the developer's declared `@` version
//! names a version, and nothing about versioning is ever a refusal (owner,
//! 2026-11-03, board `program-versions-the-core-under-schema` ask 6).
//!
//! - The first apply of `@ 0.2.0` is version `0.2.0`.
//! - An edit to comments or spacing compiles to the same program: the same
//!   version, nothing stored, nothing re-pointed.
//! - A real change under the same number is kept apart as `0.2.0+r2`, with
//!   a warning to raise the number. Never refused.
//! - Applying `0.2.0`'s content again finds `0.2.0`.
//! - A stored build from another Ordo release is compared after compiling
//!   its source under this one, so a release's own changes move nothing.
//! - A program with no `@` is named as before, by its compiled digest.
//! - A version stored before 2026-11-03 keeps its 16-hex name (ask 7).
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_commandlog::Timestamp;
use ordo_conformance::Pg;
use ordo_exec::registry::{self, Applied, Artifact, RegistryRefusal};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, Activation, Report, Stored};
use ordo_host::schema_apply::Program;

const V020: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

/// `V020` after a lint pass: a comment, other spacing, blank lines.
const V020_LINTED: &str = "# The task list.
domain tasks @ 0.2.0



entity task {   # one row per task
\ttitle:   text
\tdone:    bool
}

op openTasks unbounded =
      task[title, done]   # everything
";

/// A real change, with the number left as it was.
const V020_CHANGED: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

/// The registry's tables as they were before 2026-11-03.
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

async fn apply(pool: &Pool, source: &str) -> Report {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "t",
        "t",
        true,
    )
    .await
    .expect("applies")
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
async fn the_declared_version_names_the_first_apply() {
    let pool = fresh("ordo_vid_first").await;
    let report = apply(&pool, V020).await;
    assert_eq!(report.version, "0.2.0");
    assert_eq!(report.stored, Stored::Written);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    let release = one(&pool, "SELECT \"ordo_release\" FROM \"$builds\"").await;
    assert_eq!(release.as_deref(), Some(ordo_plan::artifact::ORDO_RELEASE));
}

/// **A lint pass is not a release.**
#[tokio::test(flavor = "multi_thread")]
async fn comments_and_spacing_are_the_same_version() {
    let pool = fresh("ordo_vid_lint").await;
    apply(&pool, V020).await;
    let report = apply(&pool, V020_LINTED).await;
    assert_eq!(report.version, "0.2.0");
    assert_eq!(report.stored, Stored::AlreadyPresent);
    assert!(
        matches!(report.activation, Activation::AlreadyActive { .. }),
        "{:?}",
        report.activation
    );
    assert!(report.notes.is_empty(), "{:?}", report.notes);
}

/// **A change under the same number is kept apart, never refused.**
#[tokio::test(flavor = "multi_thread")]
async fn a_change_under_the_same_number_is_a_revision_with_a_warning() {
    let pool = fresh("ordo_vid_revision").await;
    apply(&pool, V020).await;
    let report = apply(&pool, V020_CHANGED).await;
    assert_eq!(report.version, "0.2.0+r2");
    assert_eq!(report.stored, Stored::Written);
    assert!(matches!(report.activation, Activation::Switched { .. }));
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.contains("0.2.0+r2") && n.contains("@ 0.2.0")),
        "the warning names the revision and the number to raise: {:?}",
        report.notes
    );

    // The first content again is the first version again.
    let back = apply(&pool, V020).await;
    assert_eq!(back.version, "0.2.0");
    assert_eq!(back.stored, Stored::AlreadyPresent);

    // A raised number is a version of its own, with no warning.
    let raised = apply(&pool, &V020_CHANGED.replace("@ 0.2.0", "@ 0.3.0")).await;
    assert_eq!(raised.version, "0.3.0");
    assert!(raised.notes.is_empty(), "{:?}", raised.notes);
}

/// A program with no `@` is named as before, by the digest of what it
/// compiles to: a lint pass keeps it, a change moves it, and neither warns.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_with_no_declared_version_is_named_by_its_compiled_digest() {
    let pool = fresh("ordo_vid_undeclared").await;
    let bare = V020.replace("domain tasks @ 0.2.0\n", "");
    let first = apply(&pool, &bare).await;
    let program = Program::compile(&bare).expect("compiles");
    assert_eq!(
        first.version,
        ordo_plan::artifact::program_version(&program.doc, &program.ddl)
    );
    let linted = apply(&pool, &format!("# tasks\n\n{bare}")).await;
    assert_eq!(linted.version, first.version);
    assert_eq!(linted.stored, Stored::AlreadyPresent);
    let changed = apply(&pool, &V020_CHANGED.replace("domain tasks @ 0.2.0\n", "")).await;
    assert_ne!(changed.version, first.version);
    assert!(changed.notes.is_empty(), "{:?}", changed.notes);
}

/// **An Ordo release's own changes move nothing.** The stored build of
/// `0.2.0` came from another release and differs from what this one
/// compiles. Its source is compiled under this release, matches, and the
/// row takes this release's build.
#[tokio::test(flavor = "multi_thread")]
async fn a_build_from_another_release_is_compared_under_this_one() {
    let pool = fresh("ordo_vid_release").await;
    apply(&pool, V020).await;
    pool.rows(
        "UPDATE \"$builds\" SET \"ordo_release\" = '0.4.0', \"build_id\" = 'older'; \
         UPDATE \"$artifacts\" SET \"ddl\" = \"ddl\" || E'\\n-- rendered by 0.4.0'",
    )
    .await
    .expect("an older build");

    let report = apply(&pool, V020).await;
    assert_eq!(report.version, "0.2.0");
    assert_eq!(report.stored, Stored::Rebuilt);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    let builds = one(&pool, "SELECT count(*) FROM \"$builds\"").await;
    assert_eq!(builds.as_deref(), Some("2"), "both builds are kept");
}

/// **The upgrade moves nothing** (ask 7). A deployment whose registry
/// predates declared versions serves the program under its 16-hex name.
/// The same program, linted, applied by this release is already stored and
/// already active, and the database's `$migrations` still names it.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_stored_before_keeps_its_name() {
    let pool = fresh("ordo_vid_upgrade").await;
    let program = Program::compile(V020).expect("compiles");

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

    let report = apply(&pool, V020_LINTED).await;
    assert_eq!(report.version, legacy, "the stored name is kept");
    assert_eq!(report.stored, Stored::AlreadyPresent);
    assert!(
        matches!(report.activation, Activation::AlreadyActive { .. }),
        "nothing re-pointed: {:?}",
        report.activation
    );
    let declared = one(
        &pool,
        &format!("SELECT \"declared\" FROM \"$artifacts\" WHERE \"version\" = '{legacy}'"),
    )
    .await;
    assert_eq!(declared.as_deref(), Some("0.2.0"));
    let at = ordo_exec::spacedb::recorded_version(&pool)
        .await
        .expect("reads");
    assert_eq!(at, ordo_exec::spacedb::TargetVersion::At(legacy));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_different_program_stored_under_a_taken_name_is_refused() {
    let pool = fresh("ordo_vid_collide").await;
    registry::ensure_tables(&pool).await.expect("tables");
    let program = Program::compile(V020).expect("compiles");
    assert!(matches!(
        registry::store(&pool, &artifact(&program, "0.2.0")).await,
        Ok(Applied::Stored(_))
    ));
    let other = Program::compile(V020_CHANGED).expect("compiles");
    assert!(matches!(
        registry::store(&pool, &artifact(&other, "0.2.0")).await,
        Err(RegistryRefusal::VersionCollision { .. })
    ));
}
