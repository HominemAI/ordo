//! **Tenant builds** (`plan-versions.md` §3.8, §4 row 16; board
//! `tenant-builds`, 27t): a named build serves one stored version to the
//! tenants the deployment owner names, while every other tenant follows
//! the active version.
//!
//! The owner, 2026-10-01: *"it may make sense to have custom builds /
//! branches for 'special' tenants, public tests of beta builds etc. so maybe
//! a branch could be promoted."* A build is that promotion's last step: the
//! version came through the promotion gate like any other (V-13), and the
//! build pins named production tenants to it.
//!
//! - **A build** is a name and a version (`$tenant_builds`). Setting it to
//!   another version moves every tenant it holds.
//! - **A member** is one tenant in one build (`$tenant_build_members`).
//!   Joining pins it to the build's version; leaving pins it to the active
//!   version. Each pin is a `$pins` row with cause `build`.
//! - **Only the build moves its tenants.** An activation, a rollback, a
//!   rollout and the revoked-version sweep leave them where they are
//!   ([`crate::versions::NOT_IN_A_BUILD`]); `ordo versions pin` and a move
//!   off a revoked version are refused, naming `ordo tenant-builds leave`.
//! - **Every pin passes the same checks as any pin**: a stored version, not
//!   revoked, in `versions.served`, and past the first step of a chain one
//!   that arrived through the gate ([`crate::promotion::require_arrival`]).
//! - **The database holds the version first.** A tenant with a database of
//!   its own has it brought forward to the version and gated, as a rollout
//!   moves one; a tenant on the deployment's own database is pinned only
//!   when that database holds the version (`ordo apply --no-activate`
//!   stores it there). Setting a build checks every member before it moves
//!   any.
//!
//! Who chooses the tenants is named tenants only. Whether builds share one
//! targeting model with feature flags (groups, a percentage) is
//! `settings-layers` ask 1, still open.
//!
//! The door ops are the deployment owner's, bound to the deployment's own
//! space, on `APPLY`'s terms (`crate::platform_ops::owner_only`). Each
//! change is written to the security record as a `deploy` event.

use std::collections::BTreeSet;

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_fault::FaultClass;
use serde_json::json;

use crate::apply_op::{ApplyLock, ApplyOps};
use crate::schema_apply::{self as sa, Program};
use crate::versions::{PinCause, VersionRefusal, Versions};

/// Every build and the tenants it holds.
pub const BUILDS: &str = "std.deployment.tenantBuilds";
/// Create a build, or set it to another version, moving its tenants.
pub const SET_BUILD: &str = "std.deployment.setTenantBuild";
/// Put one tenant in a build.
pub const JOIN_BUILD: &str = "std.deployment.joinTenantBuild";
/// Take one tenant out of its build, back to the active version.
pub const LEAVE_BUILD: &str = "std.deployment.leaveTenantBuild";
/// Remove a build that holds no tenant.
pub const REMOVE_BUILD: &str = "std.deployment.removeTenantBuild";

pub const OPS: [&str; 5] = [BUILDS, SET_BUILD, JOIN_BUILD, LEAVE_BUILD, REMOVE_BUILD];

/// The actor of a pin a build made is the person; this is the op string.
const OP: &str = "tenant_build";

/// A build's name: 1 to 40 characters, lowercase letters, digits and `-`,
/// starting with a letter.
pub const MAX_NAME_CHARS: usize = 40;

/// Why a tenant build act was refused. The one error of [`TenantBuilds`].
#[derive(Debug)]
pub enum BuildRefusal {
    /// A name that is not one. Fix the input.
    Name(String),
    /// No build under this name. `builds` names those there are.
    NoSuchBuild { name: String, builds: Vec<String> },
    /// The tenant is in no build. Nothing to leave.
    NotAMember { space: String },
    /// The tenant is in another build. It leaves that one first.
    OtherBuild { space: String, build: String },
    /// A build that still holds tenants is not removed.
    HasMembers { name: String, members: Vec<String> },
    /// The deployment's own database does not hold the version, so a
    /// tenant on it cannot be served it.
    DeploymentBehind { version: String, why: String },
    /// A tenant's own database did not take the version.
    TenantBehind {
        space: String,
        version: String,
        why: String,
        transient: bool,
    },
    /// Another apply holds the apply lock. Try again.
    Busy,
    /// The pin itself was refused.
    Version(VersionRefusal),
    /// The database.
    Store(PoolError),
}

impl From<PoolError> for BuildRefusal {
    fn from(e: PoolError) -> Self {
        BuildRefusal::Store(e)
    }
}

impl From<VersionRefusal> for BuildRefusal {
    fn from(e: VersionRefusal) -> Self {
        match e {
            VersionRefusal::Store(e) => BuildRefusal::Store(e),
            other => BuildRefusal::Version(other),
        }
    }
}

impl std::fmt::Display for BuildRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildRefusal::Name(n) => write!(
                f,
                "`{n}` is not a build name: 1 to {MAX_NAME_CHARS} characters, lowercase \
                 letters, digits and `-`, starting with a letter, like `beta`"
            ),
            BuildRefusal::NoSuchBuild { name, builds } => write!(
                f,
                "no_such_build: there is no tenant build `{name}`{}. `ordo tenant-builds set \
                 {name} <version> --reason \"...\"` makes it",
                if builds.is_empty() {
                    String::new()
                } else {
                    format!("; there are {}", builds.join(", "))
                }
            ),
            BuildRefusal::NotAMember { space } => write!(
                f,
                "not_in_a_build: tenant {space} is in no tenant build. `ordo tenant-builds list` \
                 shows who is"
            ),
            BuildRefusal::OtherBuild { space, build } => write!(
                f,
                "in_tenant_build: tenant {space} is in tenant build `{build}`, and a tenant is \
                 in one build at a time. `ordo tenant-builds leave {space}` takes it out first"
            ),
            BuildRefusal::HasMembers { name, members } => write!(
                f,
                "build_has_tenants: tenant build `{name}` still holds {}: {}. `ordo \
                 tenant-builds leave <tenant>` takes each out first",
                members.len(),
                members.join(", ")
            ),
            BuildRefusal::DeploymentBehind { version, why } => write!(
                f,
                "deployment_behind: the deployment's own database does not hold {version}, so \
                 a tenant on it cannot be served it ({why}). `ordo apply <dir> --no-activate` \
                 stores the version and brings the database to it"
            ),
            BuildRefusal::TenantBehind {
                space,
                version,
                why,
                transient,
            } => write!(
                f,
                "tenant_behind: tenant {space}'s own database did not take {version}: {why}. {}",
                if *transient {
                    "It could not be reached; try again"
                } else {
                    "`ordo evolve --tenant <tenant> --to <version>` runs the stored step on it"
                }
            ),
            BuildRefusal::Busy => write!(
                f,
                "busy: an apply holds the apply lock, and a tenant's database is brought \
                 forward only under it. Try again when it is done"
            ),
            BuildRefusal::Version(e) => write!(f, "{e}"),
            BuildRefusal::Store(e) => write!(f, "the control database: {e}"),
        }
    }
}

impl BuildRefusal {
    /// **The one place a `BuildRefusal` becomes a door answer.**
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            BuildRefusal::Name(_) => ExecError::new(ErrorCode::InvalidParam, message),
            BuildRefusal::NoSuchBuild { .. } | BuildRefusal::NotAMember { .. } => {
                ExecError::new(ErrorCode::NotFound, message)
            }
            BuildRefusal::OtherBuild { .. }
            | BuildRefusal::HasMembers { .. }
            | BuildRefusal::DeploymentBehind { .. }
            | BuildRefusal::TenantBehind {
                transient: false, ..
            } => ExecError::new(ErrorCode::ApplyRefused, message),
            BuildRefusal::TenantBehind { .. } | BuildRefusal::Busy => {
                ExecError::of_fault(FaultClass::Busy, ErrorCode::SpaceUnavailable, message)
            }
            BuildRefusal::Version(e) => e.to_exec_error(what),
            BuildRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
        }
    }
}

/// Check a build's name.
pub fn check_name(name: &str) -> Result<String, BuildRefusal> {
    let n = name.trim();
    let ok = !n.is_empty()
        && n.chars().count() <= MAX_NAME_CHARS
        && n.starts_with(|c: char| c.is_ascii_lowercase())
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(n.to_string())
    } else {
        Err(BuildRefusal::Name(n.to_string()))
    }
}

/// One build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub name: String,
    pub version: String,
    pub reason: String,
    pub actor: String,
    pub at: u64,
    /// `(tenant, joined by, joined at)`, by tenant.
    pub members: Vec<(String, String, u64)>,
}

/// What an act moved: each tenant and the version it is on now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Moved {
    pub build: String,
    pub version: String,
    pub tenants: Vec<String>,
}

/// **The one client for tenant builds** (AGENTS.md rule 9). Pins go
/// through [`Versions::pin_as`] with [`PinCause::Build`].
pub struct TenantBuilds<'a> {
    ops: &'a ApplyOps,
}

impl<'a> TenantBuilds<'a> {
    pub fn on(ops: &'a ApplyOps) -> TenantBuilds<'a> {
        TenantBuilds { ops }
    }

    fn pool(&self) -> &Pool {
        &self.ops.pool
    }

    async fn ensure(&self) -> Result<(), BuildRefusal> {
        crate::versions::ensure_build_tables(self.pool()).await?;
        Ok(())
    }

    /// Every build, by name, with its tenants.
    pub async fn list(&self) -> Result<Vec<Build>, BuildRefusal> {
        self.ensure().await?;
        let rows = self
            .pool()
            .rows(
                "SELECT \"name\", \"version\", \"reason\", \"actor\", \"at\" \
                 FROM \"$tenant_builds\" ORDER BY \"name\"",
            )
            .await?;
        let members = self
            .pool()
            .rows(
                "SELECT \"build\", \"space\", \"actor\", \"at\" FROM \"$tenant_build_members\" \
                 ORDER BY \"space\"",
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let name = r.first()?.clone()?;
                Some(Build {
                    members: members
                        .iter()
                        .filter(|m| m.first().cloned().flatten().as_deref() == Some(&name))
                        .filter_map(|m| {
                            Some((
                                m.get(1)?.clone()?,
                                m.get(2)?.clone()?,
                                m.get(3)?.clone()?.parse().ok()?,
                            ))
                        })
                        .collect(),
                    version: r.get(1)?.clone()?,
                    reason: r.get(2)?.clone()?,
                    actor: r.get(3)?.clone()?,
                    at: r.get(4)?.clone()?.parse().ok()?,
                    name,
                })
            })
            .collect())
    }

    /// One build, or a refusal naming those there are.
    pub async fn get(&self, name: &str) -> Result<Build, BuildRefusal> {
        let all = self.list().await?;
        let names = all.iter().map(|b| b.name.clone()).collect();
        all.into_iter()
            .find(|b| b.name == name)
            .ok_or_else(|| BuildRefusal::NoSuchBuild {
                name: name.to_string(),
                builds: names,
            })
    }

    /// The build that holds `space`, if one does.
    pub async fn build_of(&self, space: &str) -> Result<Option<String>, BuildRefusal> {
        self.ensure().await?;
        Ok(self
            .pool()
            .rows(&format!(
                "SELECT \"build\" FROM \"$tenant_build_members\" WHERE \"space\" = {}",
                sql_lit(space)
            ))
            .await?
            .first()
            .and_then(|r| r.first().cloned().flatten()))
    }

    /// **Create a build, or set it to another version.** Every tenant it
    /// holds is checked first (its database holds the version), then each
    /// is pinned. A refusal moves none.
    pub async fn set(
        &self,
        name: &str,
        version: &str,
        reason: &str,
        actor: &str,
        at: u64,
    ) -> Result<Moved, BuildRefusal> {
        let name = check_name(name)?;
        let reason = crate::versions::check_reason(reason)?;
        let versions = Versions::on(self.pool());
        let version = versions.resolve(version).await?;
        versions.require_arrival(&version).await?;
        let existing = match self.get(&name).await {
            Ok(b) => Some(b),
            Err(BuildRefusal::NoSuchBuild { .. }) => None,
            Err(e) => return Err(e),
        };
        let members: Vec<String> = existing
            .iter()
            .flat_map(|b| b.members.iter().map(|m| m.0.clone()))
            .collect();
        // The version must be one a tenant may be pinned to, members or not.
        versions.check_served(&version).await?;
        if let Some(mark) = versions.revocation_of(&version).await? {
            return Err(BuildRefusal::Version(VersionRefusal::Revoked {
                named: versions.named(&version).await?,
                mark: Box::new(mark),
                setting: None,
            }));
        }
        self.ready_all(&members, &version).await?;
        self.pool()
            .rows(&format!(
                "INSERT INTO \"$tenant_builds\" (\"name\", \"version\", \"reason\", \"actor\", \"at\") \
                 VALUES ({n}, {v}, {r}, {a}, {at}) \
                 ON CONFLICT (\"name\") DO UPDATE SET \"version\" = {v}, \"reason\" = {r}, \
                   \"actor\" = {a}, \"at\" = {at}",
                n = sql_lit(&name),
                v = sql_lit(&version),
                r = sql_lit(&reason),
                a = sql_lit(actor),
            ))
            .await?;
        for space in &members {
            match versions
                .pin_as(space, &version, actor, at, PinCause::Build)
                .await
            {
                Ok(_) | Err(VersionRefusal::NoTenant { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Moved {
            build: name,
            version,
            tenants: members,
        })
    }

    /// **Put one tenant in a build**: its database is brought to the
    /// build's version, then it is pinned there.
    pub async fn join(
        &self,
        name: &str,
        space: &str,
        actor: &str,
        at: u64,
    ) -> Result<Moved, BuildRefusal> {
        let name = check_name(name)?;
        let build = self.get(&name).await?;
        match self.build_of(space).await? {
            Some(b) if b == name => {}
            Some(b) => {
                return Err(BuildRefusal::OtherBuild {
                    space: space.to_string(),
                    build: b,
                })
            }
            None => {}
        }
        let versions = Versions::on(self.pool());
        // A tenant there is, serving: its pin answers.
        if versions.pin_of(space).await?.is_none() {
            return Err(VersionRefusal::NoTenant {
                space: space.to_string(),
            }
            .into());
        }
        self.ready_all(&[space.to_string()], &build.version).await?;
        // Held by the build before its pin moves, so an activation in
        // between leaves it alone.
        self.pool()
            .rows(&format!(
                "INSERT INTO \"$tenant_build_members\" (\"space\", \"build\", \"actor\", \"at\") \
                 VALUES ({}, {}, {}, {at}) ON CONFLICT (\"space\") DO NOTHING",
                sql_lit(space),
                sql_lit(&name),
                sql_lit(actor),
            ))
            .await?;
        if let Err(e) = versions
            .pin_as(space, &build.version, actor, at, PinCause::Build)
            .await
        {
            self.pool()
                .rows(&format!(
                    "DELETE FROM \"$tenant_build_members\" WHERE \"space\" = {}",
                    sql_lit(space)
                ))
                .await?;
            return Err(e.into());
        }
        Ok(Moved {
            build: name,
            version: build.version,
            tenants: vec![space.to_string()],
        })
    }

    /// **Take one tenant out of its build**, pinned to the active version.
    pub async fn leave(&self, space: &str, actor: &str, at: u64) -> Result<Moved, BuildRefusal> {
        let Some(build) = self.build_of(space).await? else {
            return Err(BuildRefusal::NotAMember {
                space: space.to_string(),
            });
        };
        let versions = Versions::on(self.pool());
        let Some(active) = versions.active_version().await? else {
            return Err(VersionRefusal::NotStored {
                asked: "the active version".into(),
                stored: vec![],
            }
            .into());
        };
        self.ready_all(&[space.to_string()], &active).await?;
        self.pool()
            .rows(&format!(
                "DELETE FROM \"$tenant_build_members\" WHERE \"space\" = {}",
                sql_lit(space)
            ))
            .await?;
        match versions
            .pin_as(space, &active, actor, at, PinCause::Build)
            .await
        {
            Ok(_) | Err(VersionRefusal::NoTenant { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(Moved {
            build,
            version: active,
            tenants: vec![space.to_string()],
        })
    }

    /// **Remove a build** that holds no tenant.
    pub async fn remove(&self, name: &str) -> Result<Build, BuildRefusal> {
        let name = check_name(name)?;
        let build = self.get(&name).await?;
        if !build.members.is_empty() {
            return Err(BuildRefusal::HasMembers {
                name,
                members: build.members.iter().map(|m| m.0.clone()).collect(),
            });
        }
        // Under the same check: a join racing this sees no build.
        self.pool()
            .rows(&format!(
                "DELETE FROM \"$tenant_builds\" WHERE \"name\" = {} AND NOT EXISTS \
                 (SELECT 1 FROM \"$tenant_build_members\" WHERE \"build\" = {})",
                sql_lit(&name),
                sql_lit(&name)
            ))
            .await?;
        Ok(build)
    }

    /// Each tenant's database holds `version`: a database of its own is
    /// brought forward and gated, the deployment's own is checked. Nothing
    /// is pinned here.
    async fn ready_all(&self, spaces: &[String], version: &str) -> Result<(), BuildRefusal> {
        if spaces.is_empty() {
            return Ok(());
        }
        let artifact = Versions::on(self.pool())
            .get(version)
            .await?
            .ok_or_else(|| VersionRefusal::UnknownVersion(version.to_string()))?;
        let program =
            Program::from_artifact(&artifact).map_err(|why| BuildRefusal::DeploymentBehind {
                version: version.to_string(),
                why: format!("it does not compile here: {why}"),
            })?;
        let own: BTreeSet<String> = match ordo_exec::spacedb::all(self.pool()).await {
            Ok(a) => a.into_iter().map(|(s, _)| s.0).collect(),
            Err(e) if e.is_missing_table() => BTreeSet::new(),
            Err(e) => return Err(e.into()),
        };
        if spaces.iter().any(|s| !own.contains(s)) {
            match sa::schema_state(self.pool(), &program.schema).await {
                Ok(sa::SchemaState::Matches { .. }) => {}
                Ok(other) => {
                    return Err(BuildRefusal::DeploymentBehind {
                        version: version.to_string(),
                        why: format!("{other:?}"),
                    })
                }
                Err(why) => {
                    return Err(BuildRefusal::DeploymentBehind {
                        version: version.to_string(),
                        why,
                    })
                }
            }
        }
        let theirs: Vec<&String> = spaces.iter().filter(|s| own.contains(*s)).collect();
        if theirs.is_empty() {
            return Ok(());
        }
        let Some(_lock) = ApplyLock::take(self.pool()).await.ok().flatten() else {
            return Err(BuildRefusal::Busy);
        };
        for space in theirs {
            let behind = |why: String, transient: bool| BuildRefusal::TenantBehind {
                space: space.clone(),
                version: version.to_string(),
                why,
                transient,
            };
            let id = ordo_crypto::SpaceId(space.clone());
            let target = self
                .ops
                .pools
                .resolve(&id)
                .await
                .map_err(|e| behind(format!("its database cannot be reached ({e})"), true))?;
            sa::bring_forward_from(&target, Some(self.pool()), &program, OP, SET_BUILD)
                .await
                .map_err(|e| behind(format!("{}: {}", e.class, e.why), e.transient))?;
            match sa::schema_state(&target, &program.schema).await {
                Ok(sa::SchemaState::Matches { .. }) => {}
                Ok(other) => return Err(behind(format!("still_behind: {other:?}"), false)),
                Err(e) => return Err(behind(format!("it cannot be read ({e})"), true)),
            }
        }
        Ok(())
    }
}

// ── the door ─────────────────────────────────────────────────────────────

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
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let moved = ["build", "version", "ordinal", "tenants"];
    vec![
        spec(BUILDS, OpKind::Read, vec![], &["builds"]),
        spec(
            SET_BUILD,
            OpKind::Write,
            vec![
                text("name", 64, true),
                text("version", 256, true),
                text("reason", 2048, true),
            ],
            &moved,
        ),
        spec(
            JOIN_BUILD,
            OpKind::Write,
            vec![text("name", 64, true), text("tenant", 128, true)],
            &moved,
        ),
        spec(
            LEAVE_BUILD,
            OpKind::Write,
            vec![text("tenant", 128, true)],
            &moved,
        ),
        spec(
            REMOVE_BUILD,
            OpKind::Write,
            vec![text("name", 64, true), text("reason", 2048, true)],
            &["build", "version", "removed"],
        ),
    ]
}

/// Run one of them. `caller` is the principal the door proved.
pub async fn execute(
    ops: &ApplyOps,
    name: &str,
    space: &str,
    bound: &BoundParams,
    caller: &str,
) -> Result<ExecResult, ExecError> {
    ops.home_only(space)?;
    let b = TenantBuilds::on(ops);
    let now = crate::runtime::now_secs();
    let text = |k: &str| {
        bound
            .0
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let versions = Versions::on(&ops.pool);
    let moved_json = |m: &Moved, ordinal: Option<u64>| {
        json!({
            "build": m.build, "version": m.version, "ordinal": ordinal, "tenants": m.tenants,
        })
    };
    let row = match name {
        BUILDS => {
            let list = b
                .list()
                .await
                .map_err(|e| e.to_exec_error("the tenant builds could not be read"))?;
            let ordinals = versions.ordinals().await.unwrap_or_default();
            json!({
                "builds": list.iter().map(|b| json!({
                    "name": b.name,
                    "version": b.version,
                    "ordinal": ordinals.get(&b.version),
                    "reason": b.reason,
                    "setBy": b.actor,
                    "setAt": b.at,
                    "tenants": b.members.iter().map(|(t, by, at)| json!({
                        "tenant": t, "joinedBy": by, "joinedAt": at,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        }
        SET_BUILD => {
            let m = b
                .set(
                    &text("name"),
                    &text("version"),
                    &text("reason"),
                    caller,
                    now,
                )
                .await
                .map_err(|e| e.to_exec_error("the tenant build was not set"))?;
            ops.record(
                caller,
                SET_BUILD,
                "tenant-build-set",
                format!(
                    "build={} version={} tenants={} by={caller} reason={}",
                    m.build,
                    m.version,
                    m.tenants.len(),
                    text("reason")
                ),
            );
            moved_json(&m, versions.ordinal_of(&m.version).await.ok().flatten())
        }
        JOIN_BUILD => {
            let m = b
                .join(&text("name"), &text("tenant"), caller, now)
                .await
                .map_err(|e| e.to_exec_error("the tenant did not join the build"))?;
            ops.record(
                caller,
                JOIN_BUILD,
                "tenant-build-joined",
                format!(
                    "build={} version={} tenant={} by={caller}",
                    m.build,
                    m.version,
                    text("tenant")
                ),
            );
            moved_json(&m, versions.ordinal_of(&m.version).await.ok().flatten())
        }
        LEAVE_BUILD => {
            let m = b
                .leave(&text("tenant"), caller, now)
                .await
                .map_err(|e| e.to_exec_error("the tenant did not leave its build"))?;
            ops.record(
                caller,
                LEAVE_BUILD,
                "tenant-build-left",
                format!(
                    "build={} version={} tenant={} by={caller}",
                    m.build,
                    m.version,
                    text("tenant")
                ),
            );
            moved_json(&m, versions.ordinal_of(&m.version).await.ok().flatten())
        }
        REMOVE_BUILD => {
            let reason = crate::versions::check_reason(&text("reason"))
                .map_err(|e| e.to_exec_error("the tenant build was not removed"))?;
            let gone = b
                .remove(&text("name"))
                .await
                .map_err(|e| e.to_exec_error("the tenant build was not removed"))?;
            ops.record(
                caller,
                REMOVE_BUILD,
                "tenant-build-removed",
                format!(
                    "build={} version={} by={caller} reason={reason}",
                    gone.name, gone.version
                ),
            );
            json!({ "build": gone.name, "version": gone.version, "removed": true })
        }
        other => {
            return Err(ExecError::internal(format!(
                "`{other}` is not a tenant build op"
            )))
        }
    };
    Ok(ExecResult {
        position: None,
        rows: vec![row],
        next_cursor: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_name_is_lowercase_letters_digits_and_dashes() {
        assert_eq!(check_name(" beta ").unwrap(), "beta");
        assert!(check_name("beta-2").is_ok());
        for bad in ["", "Beta", "2beta", "beta build", "beta_2", &"b".repeat(41)] {
            let e = check_name(bad).unwrap_err().to_string();
            assert!(e.contains("like `beta`"), "{bad}: {e}");
        }
    }

    #[test]
    fn the_list_is_a_read_and_the_rest_are_changes() {
        let specs = specs();
        let kind = |n: &str| specs.iter().find(|s| s.name == n).map(|s| s.kind);
        assert_eq!(kind(BUILDS), Some(OpKind::Read));
        for op in [SET_BUILD, JOIN_BUILD, LEAVE_BUILD, REMOVE_BUILD] {
            assert_eq!(kind(op), Some(OpKind::Write), "{op}");
        }
        assert_eq!(specs.len(), OPS.len());
    }

    #[test]
    fn each_refusal_names_its_fix() {
        let r = BuildRefusal::NoSuchBuild {
            name: "beta".into(),
            builds: vec![],
        };
        assert!(r.to_string().contains("ordo tenant-builds set beta"));
        let r = BuildRefusal::HasMembers {
            name: "beta".into(),
            members: vec!["spc_a".into()],
        };
        assert!(r.to_string().contains("ordo tenant-builds leave"));
        let r = BuildRefusal::DeploymentBehind {
            version: "0.2.0".into(),
            why: "x".into(),
        };
        assert!(r.to_string().contains("--no-activate"));
    }
}
