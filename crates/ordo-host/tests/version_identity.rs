//! Increment 1 of `plan-versions.md`: the developer's declared `@` version
//! names a version (owner, 2026-11-03, board
//! `program-versions-the-core-under-schema` asks 6 and 13).
//!
//! - The first apply of `@ 0.2.0` is version `0.2.0`.
//! - An edit to comments or spacing compiles to the same program: the same
//!   version, nothing stored, nothing re-pointed.
//! - A real change under a number already applied is refused, naming the
//!   next number, so the source always names what is applied.
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
use ordo_host::apply_op::{self, Activation, ApplyRefused, Report, Stored};
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
    try_apply(pool, source).await.expect("applies")
}

async fn try_apply(pool: &Pool, source: &str) -> Result<Report, ApplyRefused> {
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

/// **A real change needs a version of its own** (ask 13). The refusal
/// names the next number; nothing is stored and nothing switches.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_under_the_same_number_is_refused_naming_the_next() {
    let pool = fresh("ordo_vid_taken").await;
    apply(&pool, V020).await;
    let refused = try_apply(&pool, V020_CHANGED).await.expect_err("refused");
    assert_eq!(
        refused,
        ApplyRefused::VersionTaken {
            version: "0.2.0".into(),
            next: Some("0.2.1".into())
        }
    );
    assert_eq!(refused.class(), "version_taken");
    let text = refused.to_string();
    assert!(
        text.contains("@ 0.2.1") && text.contains("--bump"),
        "{text}"
    );
    assert_eq!(
        ordo_host::schema_apply::NameRefusal::read_taken(&text),
        Some(("0.2.0".into(), "0.2.1".into())),
        "`--bump` reads back what the door sends"
    );
    let stored = one(&pool, "SELECT count(*) FROM \"$artifacts\"").await;
    assert_eq!(stored.as_deref(), Some("1"), "nothing stored");

    // The preview says the same before anything is tried.
    assert!(matches!(
        apply_op::preview(&pool, V020_CHANGED).await,
        Err(ApplyRefused::VersionTaken { .. })
    ));

    // The first content again is the first version again.
    let back = apply(&pool, V020).await;
    assert_eq!(back.version, "0.2.0");
    assert_eq!(back.stored, Stored::AlreadyPresent);

    // The raised number is a version of its own.
    let raised = apply(&pool, &V020_CHANGED.replace("@ 0.2.0", "@ 0.2.1")).await;
    assert_eq!(raised.version, "0.2.1");
    assert_eq!(raised.stored, Stored::Written);
    assert!(matches!(raised.activation, Activation::Switched { .. }));

    // The next refusal counts up from the highest number stored.
    let again = try_apply(
        &pool,
        &V020_CHANGED.replace("due:   int?", "due:   int?\n  note:  text?"),
    )
    .await
    .expect_err("refused");
    assert!(
        matches!(&again, ApplyRefused::VersionTaken { next: Some(n), .. } if n == "0.2.2"),
        "{again:?}"
    );
}

/// A program with no `@` is named as before, by the digest of what it
/// compiles to: a lint pass keeps it, and a change moves it (ask 10 open).
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
