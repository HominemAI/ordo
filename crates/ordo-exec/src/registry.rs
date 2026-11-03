//! The Postgres artifact registry — F6's *"compiled ops are data"*, made
//! durable, plus the pointer a rollback re-points.
//!
//! `implementation.md` settles the shape: *"the artifact registry lives in
//! Postgres; nodes hold an in-memory cache. `ordo apply` writes new versions
//! and nudges nodes (LISTEN/NOTIFY) … Old artifacts are retained, so
//! **rollback = re-point the registry at the prior version hash**."* This is
//! the storage half of that sentence. The swap half is
//! `ordo_host::deploy` — this crate cannot name it, because the load gate a
//! swap must pass needs a compiler and `ordo-exec` sits below one.
//!
//! # Two tables, and why the pointer is an append-only log
//!
//! `$artifacts` is content-addressed by version hash and is never deleted
//! from. `$activations` records which version the deployment points at, one
//! row per re-point, newest wins.
//!
//! # Source identity and builds (2026-11-03)
//!
//! A version is named by its **source identity**
//! (`ordo_plan::artifact::source_identity`), the full SHA-256 of what the
//! developer wrote, so an Ordo release moves no version (`plan-versions.md`
//! §1.1). Each row carries it in `source_id`. A version stored before then
//! keeps its 16-hex name, and [`ensure_tables`] fills in its `source_id`
//! from the stored source, so the same program applied again finds it by
//! [`name_for`] and nothing moves.
//!
//! What a release compiled a version to is a **build**, one `$builds` row
//! each. A release that builds a stored version differently updates the
//! row's document and DDL, which loads read, and records the new build.
//!
//! The obvious alternative — a single-row pointer table updated in place —
//! was rejected for the reason every other record in this platform is
//! append-only: an operator investigating an incident needs to know *what
//! was serving at the time*, and an `UPDATE` on a one-row table answers only
//! *what is serving now*. Rollback is then not a special operation at all:
//! it is an ordinary activation naming an older version, which is the same
//! mechanism forwards, and it leaves a record saying a rollback happened.
//!
//! The foreign key from `$activations.version` to `$artifacts.version` is
//! load-bearing twice over. It makes activating a version nobody stored
//! impossible in the database as well as in this code, and it makes deleting
//! an artifact that was ever active impossible — so *"old artifacts are
//! retained"* is enforced by the schema rather than promised by prose.
//!
//! # What a stored artifact carries, and the honest gap in it
//!
//! `implementation.md` says the registry stores *"planned SQL, pinned refs,
//! mode label, cost class, scope manifest"*, and `ordo plan`'s byte-stable
//! document already carries all of that. So the artifact column is exactly
//! that document — no second format, and `ordo plan | ordo apply` is a
//! pipe rather than a translation.
//!
//! It also stores the **program source** and the **DDL**, which the spec
//! does not mention, and the reason is a real limitation rather than
//! belt-and-braces. This deployment's executor
//! (`ordo_host::FixtureExecutor`) does not run stored SQL: it binds and
//! plans through `ordo-sema`/`ordo-plan` per call, from a `Fixture` built
//! out of program text. So an artifact document on its own **cannot be
//! loaded into a serving registry** — there is nothing that turns `plan.sql`
//! back into an executable op. Storing the source is what makes a swap
//! possible at all, and it keeps the parser off the *request* path (it runs
//! once per swap) without making the claim `implementation.md` makes, which
//! is that the parser is not on the hot path at all. It is. That is a gap in
//! the executor, recorded here because this is the table that would stop
//! needing the column the day it closes.
//!
//! The DDL is stored because the load gate needs it: shape verification
//! builds its empty shadow from *the DDL the artifact asserts its database
//! holds*, and an artifact that travelled without one could only be verified
//! against whatever schema the verifying node happened to compile.

use ordo_commandlog::Timestamp;

use crate::{sql_lit, Pool, PoolError};

/// Both tables. `IF NOT EXISTS` for the same reason every other DDL constant
/// in this crate has it — a deployment's migration story owns creation, and
/// this is what a test harness and a first boot call.
pub const REGISTRY_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$artifacts\" ( \
  \"version\" text PRIMARY KEY, \
  \"artifact\" text NOT NULL, \
  \"source\" text NOT NULL, \
  \"ddl\" text NOT NULL, \
  \"applied_at\" bigint NOT NULL, \
  \"actor\" text NOT NULL \
); \
CREATE TABLE IF NOT EXISTS \"$activations\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"version\" text NOT NULL REFERENCES \"$artifacts\" (\"version\"), \
  \"at\" bigint NOT NULL, \
  \"actor\" text NOT NULL \
); \
ALTER TABLE \"$artifacts\" ADD COLUMN IF NOT EXISTS \"source_id\" text; \
ALTER TABLE \"$artifacts\" ADD COLUMN IF NOT EXISTS \"edition\" integer NOT NULL DEFAULT 1; \
CREATE INDEX IF NOT EXISTS \"$artifacts_source_id\" ON \"$artifacts\" (\"source_id\"); \
CREATE TABLE IF NOT EXISTS \"$builds\" ( \
  \"version\" text NOT NULL REFERENCES \"$artifacts\" (\"version\"), \
  \"build_id\" text NOT NULL, \
  \"ordo_release\" text NOT NULL, \
  \"artifact\" text NOT NULL, \
  \"ddl\" text NOT NULL, \
  \"recorded_at\" bigint NOT NULL, \
  PRIMARY KEY (\"version\", \"build_id\") \
)";

/// The channel a replica listens on to learn a new version is active.
///
/// Separate from [`crate::jobs::INVALIDATE_CHANNEL`]: an invalidation says
/// *your cached rows may be stale*, and a deploy says *your whole registry
/// is*. A subscriber that conflated them would refetch rows on every deploy
/// and reload the registry on every write.
pub const DEPLOY_CHANNEL: &str = "ordo.deploy";

/// One stored version of the deployment's program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// [`ordo_plan::artifact::program_version`] of the document below **and
    /// of the `ddl` below**. Both, because one version has to mean one
    /// deployable state: `plan-schema-evolution.md` §1.1(a).
    pub version: String,
    /// The byte-stable `ordo plan` document, verbatim.
    pub artifact: String,
    /// The program text it was compiled from. See the module doc for why
    /// this column exists and what would remove it.
    pub source: String,
    /// The schema the artifact asserts its database holds — what the load
    /// gate builds its empty shadow from.
    pub ddl: String,
    pub applied_at: Timestamp,
    /// Asserted by the operator tool, never authenticated: `ordo apply`
    /// holds database credentials, so this attributes rather than proves.
    pub actor: String,
}

/// One re-point of the deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    pub seq: i64,
    pub version: String,
    pub at: Timestamp,
    pub actor: String,
}

/// What storing an artifact did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// A version the registry had not seen.
    Stored(String),
    /// Byte-identical to what is already stored under this version. Reported
    /// rather than silently no-oped, because re-running a deploy script and
    /// deploying something new are different facts to whoever is watching.
    AlreadyStored(String),
    /// The same source as the version stored under this name, built
    /// differently by this Ordo release. The row now holds this build, and
    /// `$builds` keeps both. No version moved.
    Rebuilt(String),
}

/// What activating did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activated {
    /// The deployment now points at `to`; it pointed at `from`.
    Switched { from: Option<String>, to: String },
    /// It already pointed there. No row appended: a rollback log full of
    /// re-activations of the current version is a log nobody reads.
    AlreadyActive(String),
}

/// Why the registry refused. Every variant is a thing an operator does
/// something about.
#[derive(Debug)]
#[non_exhaustive]
pub enum RegistryRefusal {
    /// This version hash is stored, with **different bytes**.
    ///
    /// A version stored before 2026-11-03 is named by a truncated digest,
    /// so that name is not a security boundary — and this refusal is what
    /// stops a name collision from silently replacing a program that is
    /// deployed. A newer name is the full source identity, and reaches here
    /// only from a hand-assembled `Artifact`. Whichever of the
    /// two documents is right, overwriting the other in place would leave
    /// every activation row pointing at a version whose content changed
    /// underneath it, which is the one thing a content address must not do.
    VersionCollision {
        version: String,
    },
    /// Nothing is stored under this version, so there is nothing to point
    /// at. The foreign key would refuse it too; refusing here means the
    /// operator gets the version they typed back rather than a constraint
    /// name.
    UnknownVersion(String),
    Store(PoolError),
}

impl std::fmt::Display for RegistryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryRefusal::VersionCollision { version } => write!(
                f,
                "version `{version}` is already stored with different bytes — refusing to \
                 overwrite an artifact that activations already name"
            ),
            RegistryRefusal::UnknownVersion(v) => write!(
                f,
                "no artifact is stored under version `{v}` — apply it before pointing at it"
            ),
            RegistryRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RegistryRefusal {}

impl From<PoolError> for RegistryRefusal {
    fn from(e: PoolError) -> RegistryRefusal {
        RegistryRefusal::Store(e)
    }
}

/// Create the tables if they are not there, and give every stored version
/// its source identity. Idempotent.
///
/// A row stored before `source_id` existed gets it computed from its own
/// stored source and document, in edition 1 (`plan-versions.md` §1.1). Its
/// name stays: every tenant database records it.
pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    pool.rows(REGISTRY_DDL).await?;
    let unnamed = pool
        .rows("SELECT \"version\", \"artifact\", \"source\" FROM \"$artifacts\" WHERE \"source_id\" IS NULL")
        .await?;
    for row in unnamed {
        let (Some(Some(version)), Some(Some(artifact)), Some(Some(source))) =
            (row.first(), row.get(1), row.get(2))
        else {
            continue;
        };
        pool.rows(&format!(
            "UPDATE \"$artifacts\" SET \"source_id\" = {} WHERE \"version\" = {} AND \"source_id\" IS NULL",
            sql_lit(&source_id_of(artifact, source)),
            sql_lit(version),
        ))
        .await?;
    }
    Ok(())
}

/// The source identity of a stored row: its source, edition 1, and the
/// code hashes its document carries.
fn source_id_of(artifact: &str, source: &str) -> String {
    use ordo_plan::artifact as a;
    let doc: serde_json::Value = serde_json::from_str(artifact).unwrap_or_default();
    a::source_identity(source, a::FIRST_EDITION, &a::code_hashes(&doc))
}

/// The name a source identity is stored under, if any: the identity
/// itself, or the 16-hex name of a version stored before 2026-11-03. Where
/// two old builds of one source were stored under two names, the one
/// applied last.
pub async fn name_for(pool: &Pool, source_id: &str) -> Result<Option<String>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"version\" FROM \"$artifacts\" WHERE \"source_id\" = {} \
             ORDER BY \"applied_at\" DESC, \"version\" DESC LIMIT 1",
            sql_lit(source_id)
        ))
        .await?;
    Ok(rows.first().and_then(|r| r.first().cloned().flatten()))
}

/// Record that this release built `version` to `artifact` and `ddl`. A
/// build already recorded is left as it is.
async fn record_build(
    pool: &Pool,
    version: &str,
    artifact: &str,
    ddl: &str,
    at: Timestamp,
) -> Result<(), PoolError> {
    use ordo_plan::artifact as a;
    pool.rows(&format!(
        "INSERT INTO \"$builds\" \
           (\"version\", \"build_id\", \"ordo_release\", \"artifact\", \"ddl\", \"recorded_at\") \
         VALUES ({}, {}, {}, {}, {}, {}) ON CONFLICT DO NOTHING",
        sql_lit(version),
        sql_lit(&a::build_identity_of(artifact, ddl, a::ORDO_RELEASE)),
        sql_lit(a::ORDO_RELEASE),
        sql_lit(artifact),
        sql_lit(ddl),
        at.0,
    ))
    .await
    .map(|_| ())
}

/// Store a version. Idempotent on identical bytes; a new build of the same
/// source replaces the row's document and DDL ([`Applied::Rebuilt`]); any
/// other difference is refused — see [`RegistryRefusal::VersionCollision`].
/// Every build stored is recorded in `$builds`.
pub async fn store(pool: &Pool, a: &Artifact) -> Result<Applied, RegistryRefusal> {
    let source_id = source_id_of(&a.artifact, &a.source);
    if let Some(existing) = get(pool, &a.version).await? {
        // The artifact document AND the DDL decide identity: together they
        // are what the version hashes
        // (`ordo_plan::artifact::program_version`). `source`, `applied_at`
        // and `actor` are provenance, and two operators applying the same
        // program from two checkouts must not collide on a whitespace
        // difference in a file the compiler already normalised away.
        //
        // The `ddl` half is the storage-boundary form of
        // `plan-schema-evolution.md` §1.1(a). Before it, a schema-only
        // change produced identical document bytes, this branch answered
        // `AlreadyStored`, and the differing DDL was DISCARDED — the
        // operator was told the change was already deployed while the
        // constraint they added was not. The version now moves on a DDL
        // change, so reaching here with a differing `ddl` means either a
        // truncated-digest collision or a hand-assembled `Artifact`; both
        // are refusals, for the reason on `VersionCollision`.
        //
        // Since 2026-11-03 one difference is not a collision: the same
        // source, built differently by this Ordo release. That is a new
        // build of the same version, and it moves nothing
        // (`plan-versions.md` §1.1).
        if existing.artifact == a.artifact && existing.ddl == a.ddl {
            record_build(pool, &a.version, &a.artifact, &a.ddl, a.applied_at).await?;
            return Ok(Applied::AlreadyStored(a.version.clone()));
        }
        if source_id_of(&existing.artifact, &existing.source) != source_id {
            return Err(RegistryRefusal::VersionCollision {
                version: a.version.clone(),
            });
        }
        pool.rows(&format!(
            "UPDATE \"$artifacts\" SET \"artifact\" = {}, \"ddl\" = {} WHERE \"version\" = {}",
            sql_lit(&a.artifact),
            sql_lit(&a.ddl),
            sql_lit(&a.version),
        ))
        .await?;
        record_build(pool, &a.version, &a.artifact, &a.ddl, a.applied_at).await?;
        return Ok(Applied::Rebuilt(a.version.clone()));
    }
    pool.rows(&format!(
        "INSERT INTO \"$artifacts\" \
           (\"version\", \"artifact\", \"source\", \"ddl\", \"applied_at\", \"actor\", \"source_id\") \
         VALUES ({}, {}, {}, {}, {}, {}, {})",
        sql_lit(&a.version),
        sql_lit(&a.artifact),
        sql_lit(&a.source),
        sql_lit(&a.ddl),
        a.applied_at.0,
        sql_lit(&a.actor),
        sql_lit(&source_id),
    ))
    .await?;
    record_build(pool, &a.version, &a.artifact, &a.ddl, a.applied_at).await?;
    Ok(Applied::Stored(a.version.clone()))
}

/// Point the deployment at a stored version, and nudge every replica.
///
/// The `pg_notify` rides the **same transaction** as the activation row, so
/// a replica cannot be told to load a version whose activation rolled back.
/// This is the property `jobs::notify` exists for and the reason it is a
/// method on `TxConn` rather than on `Pool`.
pub async fn activate(
    pool: &Pool,
    version: &str,
    actor: &str,
    at: Timestamp,
) -> Result<Activated, RegistryRefusal> {
    if get(pool, version).await?.is_none() {
        return Err(RegistryRefusal::UnknownVersion(version.to_string()));
    }
    let from = active_version(pool).await?;
    if from.as_deref() == Some(version) {
        return Ok(Activated::AlreadyActive(version.to_string()));
    }
    let tx = pool.begin().await?;
    tx.rows(&format!(
        "INSERT INTO \"$activations\" (\"version\", \"at\", \"actor\") VALUES ({}, {}, {})",
        sql_lit(version),
        at.0,
        sql_lit(actor),
    ))
    .await?;
    tx.notify(DEPLOY_CHANNEL, version).await?;
    tx.commit().await?;
    Ok(Activated::Switched {
        from,
        to: version.to_string(),
    })
}

/// The version the deployment currently points at, if any.
pub async fn active_version(pool: &Pool) -> Result<Option<String>, PoolError> {
    let rows = pool
        .rows("SELECT \"version\" FROM \"$activations\" ORDER BY \"seq\" DESC LIMIT 1")
        .await?;
    Ok(rows.first().and_then(|r| r.first().cloned().flatten()))
}

/// When `version` was last made active, in seconds. `None` when it never
/// was, or when this database has no registry.
pub async fn activated_at(pool: &Pool, version: &str) -> Result<Option<u64>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT max(\"at\") FROM \"$activations\" WHERE \"version\" = {}",
            sql_lit(version)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|s| s.parse().ok()))
}

/// The artifact the deployment currently serves.
///
/// `None` means *nothing has ever been activated* — a fresh database, not a
/// fault. A node that finds it has no artifact to load has been pointed at a
/// database `ordo apply` has never run against, and saying "no active
/// version" is more useful than inventing an empty program.
pub async fn active(pool: &Pool) -> Result<Option<Artifact>, PoolError> {
    let Some(v) = active_version(pool).await? else {
        return Ok(None);
    };
    get(pool, &v).await
}

/// One stored version by hash.
pub async fn get(pool: &Pool, version: &str) -> Result<Option<Artifact>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"version\", \"artifact\", \"source\", \"ddl\", \"applied_at\", \"actor\" \
             FROM \"$artifacts\" WHERE \"version\" = {}",
            sql_lit(version)
        ))
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let col = |i: usize| -> Result<String, PoolError> {
        row.get(i)
            .and_then(|c| c.clone())
            .ok_or_else(|| PoolError::db(format!("artifact row is missing column {i}")))
    };
    Ok(Some(Artifact {
        version: col(0)?,
        artifact: col(1)?,
        source: col(2)?,
        ddl: col(3)?,
        applied_at: Timestamp(
            col(4)?
                .parse()
                .map_err(|_| PoolError::db("artifact `applied_at` is not a number"))?,
        ),
        actor: col(5)?,
    }))
}

/// Every stored version, newest application first: `(version, applied_at,
/// actor)`. What a deploy view lists.
pub async fn stored(pool: &Pool) -> Result<Vec<(String, Timestamp, String)>, PoolError> {
    let rows = pool
        .rows(
            "SELECT \"version\", \"applied_at\", \"actor\" FROM \"$artifacts\" \
             ORDER BY \"applied_at\" DESC, \"version\" DESC",
        )
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone()?,
                Timestamp(r.get(1)?.clone()?.parse().ok()?),
                r.get(2)?.clone()?,
            ))
        })
        .collect())
}

/// The re-point log, newest first. `limit` bounds it because an operator
/// reading a deploy history wants the last few and a deployment that has run
/// for a year has thousands.
pub async fn activations(pool: &Pool, limit: usize) -> Result<Vec<Activation>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"seq\", \"version\", \"at\", \"actor\" FROM \"$activations\" \
             ORDER BY \"seq\" DESC LIMIT {limit}"
        ))
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(Activation {
                seq: r.first()?.clone()?.parse().ok()?,
                version: r.get(1)?.clone()?,
                at: Timestamp(r.get(2)?.clone()?.parse().ok()?),
                actor: r.get(3)?.clone()?,
            })
        })
        .collect())
}
