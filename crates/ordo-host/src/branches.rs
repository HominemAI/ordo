//! **Dev branches are tenants on their own version** (`plan-versions.md`
//! §3.3, §4 row 5; `plan-environments.md` §2.2, ruled 2026-10-01).
//!
//! # The path
//!
//! `ordo apply <dir> --branch feature-x` is `std.deployment.apply` with a
//! `branch` parameter. On the first step of the chain (dev) it:
//!
//! 1. maps the branch name to a tenant name, strictly ([`tenant_name`]),
//!    and refuses a name another branch or another tenant holds;
//! 2. stores the version and does not activate it: the deployment keeps
//!    serving what it served (`apply_op::run_with`, `activate: false`);
//! 3. makes the branch's tenant the first time, with a database of its own
//!    and the person applying as its founding owner
//!    (`TenantOps::create`, the one way a tenant is made);
//! 4. brings that database to the version and pins the tenant to it.
//!
//! An apply again to the same branch moves its tenant to the new version.
//! Several branches are served at once, each on its own version (ruled
//! 2026-10-08, environments ask 6).
//!
//! # What a branch never touches
//!
//! Main's database and main's tenants. A branch's tables live in its own
//! database, so its additions are never in main's, and nothing is dropped
//! on return to main (D5).
//!
//! # Removal
//!
//! Only by command: `ordo branches delete <branch> --reason "..."`
//! (`std.deployment.deleteBranch`). A merge never deletes one (the
//! Supabase UAT loss, row g35). The tenant is retired and reaped: its
//! database dropped, its keys destroyed. Branches not applied to for
//! `branch.idle_list_after` are marked idle in the list, never removed.
//!
//! # The seed step
//!
//! A new branch tenant is filled by the CLI after the apply answers
//! `created`, through the door, as its owner (`ordo apply --seed <file>`).
//! The rows go through the write protocol, so sealed fields are sealed.

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_crypto::SpaceId;
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::{json, Value};

use crate::apply_op::{self, ApplyOps, ApplyRefused};
use crate::environments::{EnvironmentRefusal, Environments};

/// The branches this deployment holds, with which are idle.
pub const BRANCHES: &str = "std.deployment.branches";
/// Remove one branch's tenant: its database and its keys.
pub const DELETE_BRANCH: &str = "std.deployment.deleteBranch";
/// Set `branch.idle_list_after`, with a reason.
pub const SET_SETTING: &str = "std.deployment.setBranchSetting";

/// Every op of this module. `std.deployment.apply` with `branch` is the
/// apply's own op.
pub const OPS: [&str; 3] = [BRANCHES, DELETE_BRANCH, SET_SETTING];

/// The apply's parameter that names a branch.
pub const BRANCH_PARAM: &str = "branch";

/// What `$migrations` and the stream record for a branch's apply.
pub const BRANCH_OP_LABEL: &str = "std.deployment.apply --branch";

/// Every branch tenant's name starts with this.
pub const TENANT_PREFIX: &str = "branch-";

/// The longest branch name taken.
pub const MAX_BRANCH_CHARS: usize = 100;

/// The setting for when an unused branch is listed as idle.
pub const IDLE_SETTING: &str = "branch.idle_list_after";
/// Its environment variable.
pub const IDLE_ENV: &str = "ORDO_BRANCH_IDLE_LIST_AFTER";
/// Its value when unset: 30 days.
pub const IDLE_DEFAULT_SECS: u64 = 30 * 24 * 3600;

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$branches\" ( \
  \"branch\" text PRIMARY KEY, \
  \"tenant\" text NOT NULL UNIQUE, \
  \"space\" text NOT NULL, \
  \"version\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"created_at\" bigint NOT NULL, \
  \"applied_at\" bigint NOT NULL \
)";

const COLUMNS: &str = "\"branch\", \"tenant\", \"space\", \"version\", \"actor\", \
     \"created_at\"::text, \"applied_at\"::text";

/// `$branch_versions`: each version a branch apply stored first. Kept when
/// the branch is deleted, because the version stays stored.
const VERSIONS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$branch_versions\" ( \
  \"version\" text PRIMARY KEY, \
  \"branch\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

/// **The `@` numbers on main's line**: every declared number stored here,
/// less those of versions a branch stored first and that were never
/// activated. What main's rising-number check counts
/// (`schema_apply::name_version`), so a branch at 0.5.x never refuses
/// main's 0.4.0. A version main activates counts from then on.
pub async fn main_declared_versions(pool: &Pool) -> Result<Vec<String>, PoolError> {
    const ALL: &str = "SELECT DISTINCT \"declared\" FROM \"$artifacts\" WHERE \"declared\" <> ''";
    const MAINS: &str = "SELECT DISTINCT a.\"declared\" FROM \"$artifacts\" a \
         WHERE a.\"declared\" <> '' \
         AND NOT EXISTS (SELECT 1 FROM \"$branch_versions\" b WHERE b.\"version\" = a.\"version\" \
           AND NOT EXISTS (SELECT 1 FROM \"$activations\" x WHERE x.\"version\" = a.\"version\"))";
    let rows = match pool.rows(MAINS).await {
        Ok(rows) => rows,
        // No branch ever stored a version, or nothing was ever activated.
        Err(e) if e.is_missing_table() => match pool.rows(ALL).await {
            Ok(rows) => rows,
            Err(e) if e.is_missing_table() => return Ok(vec![]),
            Err(e) => return Err(e),
        },
        Err(e) => return Err(e),
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect())
}

/// Why a branch op was refused. Each names what fixes it.
#[derive(Debug)]
pub enum BranchRefusal {
    /// The branch name is not one. Fix the input.
    Name(String),
    /// Another branch maps to the same tenant name. Delete it, or rename.
    Collision {
        branch: String,
        other: String,
        tenant: String,
    },
    /// A tenant that is not a branch holds the name.
    TenantTaken { tenant: String, branch: String },
    /// This deployment is not the first step of its chain.
    NotDev(EnvironmentRefusal),
    /// This server gives no tenant a database of its own.
    NoDatabases,
    /// No branch has that name.
    Missing { branch: String },
    /// The apply itself refused.
    Apply(ApplyRefused),
    /// Making, moving or removing the tenant failed.
    Tenant(String),
    /// A reason is missing or too long.
    Reason(String),
    /// A setting value that does not read.
    Setting(String),
    /// The database failed. Retry.
    Store(PoolError),
}

impl std::fmt::Display for BranchRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BranchRefusal::Name(why) | BranchRefusal::Reason(why) | BranchRefusal::Setting(why) => {
                write!(f, "{why}")
            }
            BranchRefusal::Collision {
                branch,
                other,
                tenant,
            } => write!(
                f,
                "branch `{branch}` maps to tenant `{tenant}`, which branch `{other}` already \
                 holds. Branch names map to tenant names strictly, so 2 branches that differ \
                 only in case or in `/`, `.` and `_` collide. Delete the other: `ordo \
                 branches delete {other} --reason \"...\"`, or rename this branch"
            ),
            BranchRefusal::TenantTaken { tenant, branch } => write!(
                f,
                "tenant `{tenant}` exists and is not a branch, so branch `{branch}` cannot \
                 take its name. Rename the branch"
            ),
            BranchRefusal::NotDev(e) => write!(
                f,
                "branches live in dev, the first step of the chain. {e}. Apply the branch \
                 there: `ordo apply <path> --branch <name> --host <dev>`"
            ),
            BranchRefusal::NoDatabases => write!(
                f,
                "a branch is a tenant with a database of its own, and this server gives no \
                 tenant a database. Run the dev deployment with tenant databases (the \
                 runtime's tenant cluster, ORDO_TENANT_PG_HOST or the deployment's own)"
            ),
            BranchRefusal::Missing { branch } => write!(
                f,
                "no branch is named `{branch}`. List them: `ordo branches list`"
            ),
            BranchRefusal::Apply(e) => write!(f, "{e}"),
            BranchRefusal::Tenant(why) => write!(f, "{why}"),
            BranchRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<PoolError> for BranchRefusal {
    fn from(e: PoolError) -> Self {
        BranchRefusal::Store(e)
    }
}

impl BranchRefusal {
    /// The door's answer, keeping the apply's own code and a store's class.
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            BranchRefusal::Name(_) | BranchRefusal::Reason(_) | BranchRefusal::Setting(_) => {
                ExecError::new(ErrorCode::InvalidParam, message)
            }
            BranchRefusal::Collision { .. } | BranchRefusal::TenantTaken { .. } => {
                ExecError::new(ErrorCode::Duplicate, message)
            }
            BranchRefusal::NotDev(_) | BranchRefusal::NoDatabases => {
                ExecError::new(ErrorCode::ApplyRefused, message)
            }
            BranchRefusal::Missing { .. } => ExecError::new(ErrorCode::NotFound, message),
            BranchRefusal::Apply(e) => ExecError::new(e.code(), message),
            BranchRefusal::Tenant(_) => ExecError::new(ErrorCode::TenantRefused, message),
            BranchRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
        }
    }
}

/// **A branch name's tenant name.** Strict and one way: lower case, each
/// run of `/`, `.`, `_` and `-` one `-`, then [`TENANT_PREFIX`]. A branch
/// name is 1 to [`MAX_BRANCH_CHARS`] of letters, digits, `/`, `.`, `_` and
/// `-`, with at least one letter or digit.
pub fn tenant_name(branch: &str) -> Result<String, BranchRefusal> {
    let b = branch.trim();
    let ok = !b.is_empty()
        && b.chars().count() <= MAX_BRANCH_CHARS
        && b.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
        && b.chars().any(|c| c.is_ascii_alphanumeric());
    if !ok {
        return Err(BranchRefusal::Name(format!(
            "`{b}` is not a branch name: 1 to {MAX_BRANCH_CHARS} characters, letters, \
             digits, `/`, `.`, `_` and `-`, with at least one letter or digit"
        )));
    }
    let mut out = String::from(TENANT_PREFIX);
    let mut dash = true;
    for c in b.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    Ok(out)
}

/// One branch as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub branch: String,
    pub tenant: String,
    pub space: String,
    pub version: String,
    pub actor: String,
    pub created_at: u64,
    pub applied_at: u64,
}

impl Branch {
    fn parse(row: &[Option<String>]) -> Branch {
        let cell = |i: usize| row.get(i).and_then(|c| c.clone()).unwrap_or_default();
        Branch {
            branch: cell(0),
            tenant: cell(1),
            space: cell(2),
            version: cell(3),
            actor: cell(4),
            created_at: cell(5).parse().unwrap_or(0),
            applied_at: cell(6).parse().unwrap_or(0),
        }
    }

    /// Unused for at least `idle_after` seconds at `now`.
    pub fn idle(&self, now: u64, idle_after: u64) -> bool {
        now.saturating_sub(self.applied_at) >= idle_after
    }

    pub fn to_json(&self, now: u64, idle_after: u64) -> Value {
        json!({
            "branch": self.branch,
            "tenant": self.tenant,
            "space": self.space,
            "version": self.version,
            "actor": self.actor,
            "createdAt": self.created_at,
            "appliedAt": self.applied_at,
            "idle": self.idle(now, idle_after),
        })
    }
}

async fn ensure(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(DDL).await.is_err() {
        pool.rows(DDL).await?;
    }
    Ok(())
}

/// Every branch, by name. A deployment that never made one answers none.
pub async fn list(pool: &Pool) -> Result<Vec<Branch>, BranchRefusal> {
    match pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM \"$branches\" ORDER BY \"branch\""
        ))
        .await
    {
        Ok(rows) => Ok(rows.iter().map(|r| Branch::parse(r)).collect()),
        Err(e) if e.is_missing_table() => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

/// One branch by its name.
pub async fn get(pool: &Pool, branch: &str) -> Result<Option<Branch>, BranchRefusal> {
    Ok(list(pool)
        .await?
        .into_iter()
        .find(|b| b.branch == branch.trim()))
}

/// `branch.idle_list_after` in seconds: `<n>d`, `<n>h`, `<n>m` or `<n>s`.
pub fn parse_idle(value: &str) -> Option<u64> {
    let v = value.trim();
    let (n, unit) = v.split_at(v.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = n.parse().ok()?;
    let per = match unit {
        "d" => 86_400,
        "h" => 3_600,
        "m" => 60,
        "s" => 1,
        _ => return None,
    };
    (n > 0).then_some(n * per)
}

/// The idle setting from [`IDLE_ENV`] alone: the default a stored value
/// replaces. 30 days when unset.
pub fn idle_after_from_env() -> u64 {
    idle_from_env().0
}

/// [`IDLE_ENV`], and what is wrong with it when it does not read (the list
/// names the setting and the default it used).
pub fn idle_from_env() -> (u64, Option<String>) {
    match std::env::var(IDLE_ENV) {
        Ok(v) if !v.trim().is_empty() => match parse_idle(&v) {
            Some(secs) => (secs, None),
            None => (
                IDLE_DEFAULT_SECS,
                Some(format!(
                    "{IDLE_ENV} is `{v}`, which does not read: use <n>d, <n>h, <n>m or <n>s, \
                     from 1. Listed with the default, 30d"
                )),
            ),
        },
        _ => (IDLE_DEFAULT_SECS, None),
    }
}

/// The value stored for `branch.idle_list_after` in `$version_settings`,
/// the latest change; `None` when never set or cleared.
pub async fn stored_idle(pool: &Pool) -> Result<Option<String>, PoolError> {
    let rows = match pool
        .rows(&format!(
            "SELECT \"value\" FROM \"$version_settings\" WHERE \"setting\" = {} \
             ORDER BY \"seq\" DESC LIMIT 1",
            sql_lit(IDLE_SETTING)
        ))
        .await
    {
        Ok(r) => r,
        Err(e) if e.is_missing_table() => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(rows.first().and_then(|r| r.first().cloned().flatten()))
}

/// **The idle setting in force**: the stored value, else [`IDLE_ENV`],
/// else 30 days. With what is wrong with the value that did not read.
pub async fn idle_setting(pool: &Pool) -> Result<(u64, Option<String>), PoolError> {
    if let Some(v) = stored_idle(pool).await? {
        return Ok(match parse_idle(&v) {
            Some(secs) => (secs, None),
            None => {
                let (secs, _) = idle_from_env();
                (
                    secs,
                    Some(format!(
                        "{IDLE_SETTING} is stored as `{v}`, which does not read. Set it: \
                         `ordo branches set idle-list-after <n>d --reason \"...\"`"
                    )),
                )
            }
        });
    }
    Ok(idle_from_env())
}

/// **Set `branch.idle_list_after`**, or clear it with `default` (the
/// environment variable, then 30 days again), with who and why. Checked
/// first; kept in `$version_settings` beside the version settings.
pub async fn set_idle(
    pool: &Pool,
    value: &str,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<(u64, Option<String>), BranchRefusal> {
    let reason =
        crate::versions::check_reason(reason).map_err(|e| BranchRefusal::Reason(e.to_string()))?;
    let v = value.trim();
    let stored = if v == "default" {
        None
    } else if parse_idle(v).is_some() {
        Some(v.to_string())
    } else {
        return Err(BranchRefusal::Setting(format!(
            "`{v}` is not a value for {IDLE_SETTING}: use <n>d, <n>h, <n>m or <n>s, from 1, or \
             `default` to read {IDLE_ENV} again"
        )));
    };
    pool.rows(crate::versions::SETTINGS_DDL).await?;
    pool.rows(&format!(
        "INSERT INTO \"$version_settings\" (\"setting\", \"value\", \"actor\", \"reason\", \"at\") \
         VALUES ({}, {}, {}, {}, {at})",
        sql_lit(IDLE_SETTING),
        stored.as_deref().map(sql_lit).unwrap_or_else(|| "NULL".into()),
        sql_lit(actor),
        sql_lit(&reason),
    ))
    .await?;
    Ok(idle_setting(pool).await?)
}

/// The checks that need no write, in order: the name, the chain, tenant
/// databases, and who holds the name. Answers the tenant name and the
/// branch already recorded, if any.
async fn check(
    ops: &ApplyOps,
    gives_databases: bool,
    branch: &str,
) -> Result<(String, Option<Branch>), BranchRefusal> {
    let tenant = tenant_name(branch)?;
    let chain = Environments::on(&ops.pool)
        .chain()
        .await
        .map_err(BranchRefusal::NotDev)?;
    crate::environments::check_apply_target(&chain).map_err(BranchRefusal::NotDev)?;
    if !gives_databases {
        return Err(BranchRefusal::NoDatabases);
    }
    let all = list(&ops.pool).await?;
    if let Some(b) = all.iter().find(|b| b.branch == branch.trim()) {
        return Ok((tenant, Some(b.clone())));
    }
    if let Some(other) = all.iter().find(|b| b.tenant == tenant) {
        return Err(BranchRefusal::Collision {
            branch: branch.trim().to_string(),
            other: other.branch.clone(),
            tenant,
        });
    }
    crate::tenants::ensure_table(&ops.pool).await?;
    let taken = crate::tenants::all(&ops.pool).await?.into_iter().any(|t| {
        t.name == tenant
            && !matches!(
                t.state,
                crate::tenants::State::Failed | crate::tenants::State::Reaped
            )
    });
    if taken {
        return Err(BranchRefusal::TenantTaken {
            tenant,
            branch: branch.trim().to_string(),
        });
    }
    Ok((tenant, None))
}

/// What one branch apply did.
#[derive(Debug)]
pub struct Applied {
    pub report: apply_op::Report,
    pub branch: Branch,
    /// The tenant was made by this apply, so the seed step runs.
    pub created: bool,
    /// The version the tenant was on before, when it was moved.
    pub from: Option<String>,
}

impl Applied {
    pub fn to_json(&self) -> Value {
        let mut row = self.report.to_json();
        row["branch"] = json!({
            "branch": self.branch.branch,
            "tenant": self.branch.tenant,
            "space": self.branch.space,
            "version": self.branch.version,
            "created": self.created,
            "from": self.from,
        });
        row
    }
}

/// **`ordo apply --branch`**: store the version without activating it,
/// make the branch's tenant the first time, bring its database to the
/// version and pin it there.
#[allow(clippy::too_many_arguments)]
pub async fn apply(
    ops: &ApplyOps,
    tenants: &crate::tenant_ops::TenantOps,
    branch: &str,
    source: &str,
    code: &apply_op::CodeFiles,
    provenance: &crate::versions::Provenance,
    caller: &str,
    at: u64,
) -> Result<Applied, BranchRefusal> {
    let (tenant, existing) = check(ops, tenants.gives_databases(), branch).await?;
    let branch = branch.trim();
    let report = apply_op::run_with(
        &ops.pool,
        ops.pools.as_ref(),
        source,
        code,
        caller,
        BRANCH_OP_LABEL,
        false,
        provenance,
    )
    .await
    .map_err(BranchRefusal::Apply)?;
    let version = report.version.clone();
    let versions = crate::versions::Versions::on(&ops.pool);
    let artifact = versions
        .get(&version)
        .await
        .map_err(|e| BranchRefusal::Tenant(e.to_string()))?
        .ok_or_else(|| BranchRefusal::Tenant(format!("{version} was not stored")))?;
    let mut program = crate::schema_apply::Program::compile(&artifact.source)
        .map_err(|e| BranchRefusal::Tenant(format!("{version} does not compile here: {e}")))?;
    program.version = version.clone();

    ensure(&ops.pool).await?;
    // A version this branch stored first stays off main's number line until
    // main activates it.
    if matches!(report.stored, apply_op::Stored::Written) {
        ops.pool.rows(VERSIONS_DDL).await?;
        ops.pool
            .rows(&format!(
                "INSERT INTO \"$branch_versions\" (\"version\", \"branch\", \"at\") \
                 VALUES ({}, {}, {at}) ON CONFLICT DO NOTHING",
                sql_lit(&version),
                sql_lit(branch)
            ))
            .await?;
    }
    let (space, created) = match existing {
        Some(b) => (b.space, false),
        None => {
            // The one way a tenant is made, with the person applying as its
            // founding owner. Its database is made at the version serving.
            let mut bound = BoundParams::default();
            bound.0.insert("name".into(), json!(tenant));
            bound.0.insert("owner".into(), json!(caller));
            let made = tenants
                .create(&ops.home, &bound, caller)
                .await
                .map_err(|e| {
                    BranchRefusal::Tenant(format!("the branch's tenant: {}", e.message))
                })?;
            let space = made
                .rows
                .first()
                .and_then(|r| r["space"].as_str())
                .map(str::to_string)
                .ok_or_else(|| BranchRefusal::Tenant("no tenant was made".into()))?;
            // Recorded before its database moves, so an apply again finds
            // it and finishes the move.
            ops.pool
                .rows(&format!(
                    "INSERT INTO \"$branches\" ({}) VALUES ({}, {}, {}, {}, {}, {at}, {at})",
                    "\"branch\", \"tenant\", \"space\", \"version\", \"actor\", \
                     \"created_at\", \"applied_at\"",
                    sql_lit(branch),
                    sql_lit(&tenant),
                    sql_lit(&space),
                    sql_lit(""),
                    sql_lit(caller),
                ))
                .await?;
            (space, true)
        }
    };

    let sid = SpaceId(space.clone());
    let space_pool = ops
        .pools
        .resolve(&sid)
        .await
        .map_err(|e| BranchRefusal::Tenant(format!("the branch's database: {e}")))?;
    crate::schema_apply::bring_forward_from(
        &space_pool,
        Some(&ops.pool),
        &program,
        caller,
        BRANCH_OP_LABEL,
    )
    .await
    .map_err(|e| {
        BranchRefusal::Tenant(format!(
            "the branch's database could not be brought to {version}: {e}. Start the branch \
             again: `ordo branches delete {branch} --reason \"...\"`, then apply"
        ))
    })?;
    let moved = versions
        .pin(&space, &version, caller, at)
        .await
        .map_err(|e| BranchRefusal::Tenant(format!("pinning the branch to {version}: {e}")))?;
    ops.pool
        .rows(&format!(
            "UPDATE \"$branches\" SET \"version\" = {}, \"applied_at\" = {at} \
             WHERE \"branch\" = {}",
            sql_lit(&version),
            sql_lit(branch)
        ))
        .await?;
    ops.record(
        caller,
        apply_op::APPLY,
        "branch-applied",
        format!(
            "branch={branch} tenant={tenant} space={space} version={version} created={created} \
             by={caller}"
        ),
    );
    let branch = get(&ops.pool, branch)
        .await?
        .ok_or_else(|| BranchRefusal::Missing {
            branch: branch.to_string(),
        })?;
    Ok(Applied {
        report,
        branch,
        created,
        from: moved.from.filter(|f| *f != version),
    })
}

/// **Delete a branch**: its tenant is retired and reaped, so its database
/// is dropped and its keys destroyed, and the name is free again.
pub async fn delete(
    ops: &ApplyOps,
    reap: &crate::reap_op::ReapOps,
    branch: &str,
    reason: &str,
    caller: &str,
    at: u64,
) -> Result<(Branch, Value), BranchRefusal> {
    let reason =
        crate::versions::check_reason(reason).map_err(|e| BranchRefusal::Reason(e.to_string()))?;
    let Some(b) = get(&ops.pool, branch).await? else {
        return Err(BranchRefusal::Missing {
            branch: branch.trim().to_string(),
        });
    };
    crate::tenants::retire(
        &ops.pool,
        &b.space,
        &format!("branch {} deleted by {caller}: {reason}", b.branch),
        at,
    )
    .await?;
    let mut bound = BoundParams::default();
    bound.0.insert("tenant".into(), json!(b.space));
    let reaped = reap
        .reap(&ops.home, &bound, caller)
        .await
        .map_err(|e| BranchRefusal::Tenant(format!("the branch's tenant: {}", e.message)))?;
    ops.pool
        .rows(&format!(
            "DELETE FROM \"$branches\" WHERE \"branch\" = {}",
            sql_lit(&b.branch)
        ))
        .await?;
    // Every replica's door forgets the tenant's members.
    let _ = crate::deploy::announce_members(&ops.pool, &b.space).await;
    ops.record(
        caller,
        DELETE_BRANCH,
        "branch-deleted",
        format!(
            "branch={} tenant={} space={} by={caller} reason={reason}",
            b.branch, b.tenant, b.space
        ),
    );
    Ok((b, reaped.rows.into_iter().next().unwrap_or(Value::Null)))
}

/// Whether an apply names a branch.
pub fn names_branch(bound: &BoundParams) -> bool {
    bound
        .0
        .get(BRANCH_PARAM)
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.trim().is_empty())
}

/// `std.deployment.apply` with `branch`. Bound to the deployment's own
/// space, on the apply's terms.
pub async fn apply_op(
    ops: &ApplyOps,
    tenants: &crate::tenant_ops::TenantOps,
    space: &str,
    bound: &BoundParams,
    caller: &str,
) -> Result<ExecResult, ExecError> {
    ops.home_only(space)?;
    let text = |k: &str| bound.0.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let source = text("program")
        .ok_or_else(|| ExecError::internal("`$program` did not reach the platform op"))?;
    if bound.0.get("activate").and_then(|v| v.as_bool()) == Some(true) {
        return Err(ExecError::new(
            ErrorCode::InvalidParam,
            "a branch's version is never activated: it serves the branch's tenant alone. \
             Drop `activate`, or drop `--branch` to apply to the deployment",
        ));
    }
    let code = apply_op::parse_code(text("code").as_deref())
        .map_err(|refusal| ExecError::new(refusal.code(), refusal.to_string()))?;
    let provenance = crate::versions::Provenance {
        label: text("label"),
        git_commit: text("gitCommit"),
    };
    let branch = text(BRANCH_PARAM).unwrap_or_default();
    let done = apply(
        ops,
        tenants,
        &branch,
        &source,
        &code,
        &provenance,
        caller,
        crate::runtime::now_secs(),
    )
    .await
    .map_err(|e| e.to_exec_error("the branch was not applied"))?;
    Ok(ExecResult {
        position: None,
        rows: vec![done.to_json()],
        next_cursor: None,
    })
}

/// Run one branch op. Bound to the deployment's own space.
pub async fn execute(
    ops: &ApplyOps,
    reap: Option<&crate::reap_op::ReapOps>,
    name: &str,
    space: &str,
    bound: &BoundParams,
    caller: &str,
) -> Result<ExecResult, ExecError> {
    ops.home_only(space)?;
    let now = crate::runtime::now_secs();
    let text = |k: &str| bound.0.get(k).and_then(|v| v.as_str()).unwrap_or_default();
    let row = match name {
        BRANCHES => {
            let (idle_after, problem) = idle_setting(&ops.pool).await.map_err(|e| {
                BranchRefusal::Store(e).to_exec_error("the idle setting could not be read")
            })?;
            let all = list(&ops.pool)
                .await
                .map_err(|e| e.to_exec_error("the branches could not be read"))?;
            json!({
                "branches": all.iter().map(|b| b.to_json(now, idle_after)).collect::<Vec<_>>(),
                "idleAfter": idle_after,
                "idleSetting": IDLE_SETTING,
                "idleProblem": problem,
            })
        }
        SET_SETTING => {
            let (idle_after, problem) =
                set_idle(&ops.pool, text("value"), caller, text("reason"), now)
                    .await
                    .map_err(|e| e.to_exec_error("the branch setting was not set"))?;
            ops.record(
                caller,
                SET_SETTING,
                "branch-setting",
                format!(
                    "setting={IDLE_SETTING} value={} by={caller} reason={}",
                    text("value").trim(),
                    text("reason").trim()
                ),
            );
            json!({
                "setting": IDLE_SETTING, "idleAfter": idle_after, "idleProblem": problem,
            })
        }
        DELETE_BRANCH => {
            let reap = reap.ok_or_else(|| {
                ExecError::internal(
                    "this deployment registered the branch ops and wired no reap for them",
                )
            })?;
            let (b, reaped) = delete(ops, reap, text("branch"), text("reason"), caller, now)
                .await
                .map_err(|e| e.to_exec_error("the branch was not deleted"))?;
            let (idle_after, _) = idle_setting(&ops.pool)
                .await
                .unwrap_or((IDLE_DEFAULT_SECS, None));
            json!({ "branch": b.to_json(now, idle_after), "reaped": reaped })
        }
        other => return Err(ExecError::internal(format!("`{other}` is not a branch op"))),
    };
    Ok(ExecResult {
        position: None,
        rows: vec![row],
        next_cursor: None,
    })
}

fn param(name: &str, max: u32, required: bool) -> ParamSpec {
    ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    }
}

/// The branch ops' specs. Owner only, on the deployment's own space.
pub fn specs() -> Vec<OpSpec> {
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>, fields: &[&str]| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        reads_external: false,
        sync: Syncability::NotSyncable(if kind == OpKind::Read {
            NotSyncable::NoAnchor
        } else {
            NotSyncable::NotARead
        }),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            BRANCHES,
            OpKind::Read,
            vec![],
            &["branches", "idleAfter", "idleSetting", "idleProblem"],
        ),
        spec(
            DELETE_BRANCH,
            OpKind::Write,
            vec![
                param("branch", MAX_BRANCH_CHARS as u32 * 4, true),
                param("reason", 4096, true),
            ],
            &["branch", "reaped"],
        ),
        spec(
            SET_SETTING,
            OpKind::Write,
            vec![param("value", 64, true), param("reason", 4096, true)],
            &["setting", "idleAfter", "idleProblem"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_name_maps_to_one_tenant_name() {
        assert_eq!(tenant_name("feature-x").unwrap(), "branch-feature-x");
        assert_eq!(tenant_name("Feature/X").unwrap(), "branch-feature-x");
        assert_eq!(tenant_name("feature__x").unwrap(), "branch-feature-x");
        assert_eq!(
            tenant_name("adam/notes.v3").unwrap(),
            "branch-adam-notes-v3"
        );
        assert_eq!(tenant_name("-x-").unwrap(), "branch-x");
    }

    #[test]
    fn a_name_that_is_not_a_branch_is_refused_naming_the_rule() {
        for bad in ["", "  ", "---", "a b", "x;drop", &"a".repeat(101)] {
            let e = tenant_name(bad).unwrap_err().to_string();
            assert!(e.contains("not a branch name"), "{bad}: {e}");
        }
    }

    #[test]
    fn the_idle_setting_reads_days_hours_minutes_seconds() {
        assert_eq!(parse_idle("30d"), Some(30 * 86_400));
        assert_eq!(parse_idle("2h"), Some(7_200));
        assert_eq!(parse_idle("5m"), Some(300));
        assert_eq!(parse_idle("1s"), Some(1));
        assert_eq!(parse_idle("0d"), None);
        assert_eq!(parse_idle("3w"), None);
        assert_eq!(parse_idle("d"), None);
    }

    #[test]
    fn a_collision_names_the_command_that_frees_the_name() {
        let e = BranchRefusal::Collision {
            branch: "Feature/X".into(),
            other: "feature-x".into(),
            tenant: "branch-feature-x".into(),
        };
        assert!(e
            .to_string()
            .contains("ordo branches delete feature-x --reason"));
    }
}
