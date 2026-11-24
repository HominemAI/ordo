//! **Orphans and pruning at the door** (`plan-versions.md` §2.7, increment
//! 14): `std.deployment.orphans` lists what versions left behind in every
//! database, and `std.deployment.prune` removes one, which is what `ordo
//! evolve --orphans` and `ordo evolve --prune <entity.field>` do.
//!
//! # Pruning asks twice
//!
//! Pruning destroys what was written to a column, in every database, and
//! cannot be undone. So (owner, 2026-10-03, `syntax-additions` ask 11):
//!
//! 1. `--prune <name>` alone prunes nothing. It answers what would go: the
//!    databases holding it, how many rows hold a value, the readers of its
//!    table, and the confirmation to type.
//! 2. `--prune <name> --confirm <name>` prunes. In production the
//!    deployment's name is typed too, `--confirm-deployment <domain>`.
//!
//! # What pruning refuses
//!
//! - `prune.enabled` off: off in production unless set
//!   (`ordo environments set prune on`), on elsewhere.
//! - A name no database records as an orphan.
//! - A field a served version still declares: the active version, or one a
//!   tenant is pinned to. Move the tenants first.
//!
//! Each database keeps a **tombstone**: its `$orphans` row with `pruned_at`
//! and `pruned_by`. The act is `orphan-pruned` on the deployment's stream.

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::orphans::{self, Orphan};
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::{json, Value};

use crate::versions::Versions;

/// Every database's orphans and tombstones.
pub const ORPHANS: &str = "std.deployment.orphans";
/// Remove one orphan from every database.
pub const PRUNE: &str = "std.deployment.prune";

/// Why a prune did not happen. The one typed error (rule 9);
/// [`PruneRefusal::to_exec_error`] is where it becomes a door answer.
#[derive(Debug)]
pub enum PruneRefusal {
    /// `prune.enabled` is off here.
    Disabled { environment: String },
    /// No database records `name` as an orphan.
    NotAnOrphan { name: String, orphans: Vec<String> },
    /// A served version still declares it.
    StillServed { name: String, versions: Vec<String> },
    /// In production, the deployment's name was not typed back.
    DeploymentUnconfirmed { expected: String },
    /// The typed confirmation is not the name.
    Unconfirmed { name: String, got: String },
    /// Another apply holds the deployment.
    Busy,
    /// A database fault.
    Store(String),
}

impl From<PoolError> for PruneRefusal {
    fn from(e: PoolError) -> PruneRefusal {
        PruneRefusal::Store(e.to_string())
    }
}

impl PruneRefusal {
    pub fn class(&self) -> &'static str {
        match self {
            PruneRefusal::Disabled { .. } => "prune_disabled",
            PruneRefusal::NotAnOrphan { .. } => "not_an_orphan",
            PruneRefusal::StillServed { .. } => "prune_still_served",
            PruneRefusal::DeploymentUnconfirmed { .. } => "prune_deployment_unconfirmed",
            PruneRefusal::Unconfirmed { .. } => "prune_unconfirmed",
            PruneRefusal::Busy => "apply_in_progress",
            PruneRefusal::Store(_) => "store_failed",
        }
    }

    /// **The one place a `PruneRefusal` becomes a door answer.**
    pub fn to_exec_error(&self) -> ExecError {
        let message = format!("nothing was pruned: {self}");
        match self {
            PruneRefusal::NotAnOrphan { .. } => ExecError::new(ErrorCode::NotFound, message),
            PruneRefusal::Unconfirmed { .. } | PruneRefusal::DeploymentUnconfirmed { .. } => {
                ExecError::new(ErrorCode::InvalidParam, message)
            }
            PruneRefusal::Busy => ExecError::new(ErrorCode::ApplyRefused, message),
            PruneRefusal::Store(_) => ExecError::of_fault(
                ordo_fault::FaultClass::Retryable,
                ErrorCode::SpaceUnavailable,
                message,
            ),
            _ => ExecError::new(ErrorCode::ApplyRefused, message),
        }
    }
}

impl std::fmt::Display for PruneRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let class = self.class();
        match self {
            PruneRefusal::Disabled { environment } => write!(
                f,
                "{class}: pruning is off in `{environment}` ({}). Turn it on with `{}`",
                crate::environments::PRUNE_ENABLED,
                crate::environments::PRUNE_COMMAND
            ),
            PruneRefusal::NotAnOrphan { name, orphans } => {
                if orphans.is_empty() {
                    write!(
                        f,
                        "{class}: `{name}` is not an orphan in any database, and there are none. \
                         `ordo evolve --orphans` lists them"
                    )
                } else {
                    write!(
                        f,
                        "{class}: `{name}` is not an orphan in any database. The orphans are: {}",
                        orphans.join(", ")
                    )
                }
            }
            PruneRefusal::StillServed { name, versions } => write!(
                f,
                "{class}: `{name}` is still declared by {}, which tenants are served. Move them \
                 to a version without it (`ordo versions pin` or `ordo apply`), then prune",
                versions
                    .iter()
                    .map(|v| format!("`{v}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            PruneRefusal::DeploymentUnconfirmed { expected } => write!(
                f,
                "{class}: this is production. Type the deployment's name too: \
                 `--confirm-deployment {expected}`"
            ),
            PruneRefusal::Unconfirmed { name, got } => write!(
                f,
                "{class}: the confirmation is the name typed again, `--confirm {name}`. \
                 Received `{got}`"
            ),
            PruneRefusal::Busy => write!(
                f,
                "{class}: another apply holds the deployment. Try again when it ends"
            ),
            PruneRefusal::Store(e) => write!(f, "{class}: {e}"),
        }
    }
}

/// One database's orphans, or why it could not be read.
struct Found {
    label: String,
    pool: Option<Pool>,
    orphans: Vec<Orphan>,
    error: Option<String>,
}

async fn every_database(ops: &crate::apply_op::ApplyOps) -> Result<Vec<Found>, PruneRefusal> {
    let targets = ops
        .pools
        .targets()
        .await
        .map_err(|e| PruneRefusal::Store(format!("could not list the databases: {e}")))?;
    let mut out = Vec::new();
    for t in targets {
        match t.pool {
            Ok(pool) => match orphans::list(&pool).await {
                Ok(list) => out.push(Found {
                    label: t.label,
                    pool: Some(pool),
                    orphans: list,
                    error: None,
                }),
                Err(e) => out.push(Found {
                    label: t.label,
                    pool: None,
                    orphans: Vec::new(),
                    error: Some(e.to_string()),
                }),
            },
            Err(e) => out.push(Found {
                label: t.label,
                pool: None,
                orphans: Vec::new(),
                error: Some(e),
            }),
        }
    }
    // `default` first, then by label.
    out.sort_by(|a, b| {
        (a.label != ordo_exec::route::DEFAULT_LABEL, &a.label)
            .cmp(&(b.label != ordo_exec::route::DEFAULT_LABEL, &b.label))
    });
    Ok(out)
}

fn orphan_json(o: &Orphan) -> Value {
    json!({
        "name": o.name(),
        "entity": o.entity,
        "field": o.field,
        "columns": o.columns,
        "sealed": o.sealed,
        "from": o.from_version,
        "to": o.to_version,
        "orphanedAt": o.orphaned_at,
        "dropAfter": o.drop_after,
        "due": o.is_live() && o.drop_after.is_some_and(|d| d <= crate::runtime::now_secs()),
        "prunedAt": o.pruned_at,
        "prunedBy": o.pruned_by,
    })
}

/// The versions a tenant is served that still declare `entity.field` (or
/// `entity`): the active one and every one a tenant is pinned to.
async fn served_declaring(pool: &Pool, o: &Orphan) -> Result<Vec<String>, PruneRefusal> {
    let v = Versions::on(pool);
    let store = |e: crate::versions::VersionRefusal| PruneRefusal::Store(e.to_string());
    let mut served: Vec<String> = v
        .tenants_per_version()
        .await
        .map_err(store)?
        .into_keys()
        .collect();
    if let Some(a) = v.active_version().await.map_err(store)? {
        served.push(a);
    }
    served.sort();
    served.dedup();
    let mut out = Vec::new();
    for name in served {
        let Some(a) = v.get(&name).await.map_err(store)? else {
            continue;
        };
        let Ok(p) = crate::schema_apply::Program::from_artifact(&a) else {
            continue;
        };
        let declares = p
            .schema
            .native(&o.entity)
            .is_some_and(|e| o.is_entity() || e.field(&o.field).is_some());
        if declares {
            out.push(name);
        }
    }
    Ok(out)
}

/// How many of the space's rows hold a value in the orphan, in one database.
async fn rows_holding(pool: &Pool, o: &Orphan) -> Result<i64, PoolError> {
    let q = ordo_schema::ddl::q;
    let present = pool
        .rows(&format!(
            "SELECT column_name::text FROM information_schema.columns \
             WHERE table_schema = {} AND table_name = {}",
            pool.schema_lit(),
            sql_lit(&o.entity)
        ))
        .await?;
    let present: Vec<String> = present
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    if present.is_empty() {
        return Ok(0);
    }
    let filter = if o.is_entity() {
        "true".to_string()
    } else {
        let cols: Vec<String> = o
            .columns
            .iter()
            .filter(|c| present.contains(c))
            .map(|c| format!("{} IS NOT NULL", q(c)))
            .collect();
        if cols.is_empty() {
            return Ok(0);
        }
        cols.join(" OR ")
    };
    let rows = pool
        .rows(&format!(
            "SELECT count(*) FROM {} WHERE {filter}",
            q(&o.entity)
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0))
}

/// `std.deployment.orphans`.
pub async fn list(ops: &crate::apply_op::ApplyOps) -> Result<Value, PruneRefusal> {
    let all = every_database(ops).await?;
    Ok(json!({
        "databases": all.iter().map(|d| json!({
            "database": d.label,
            "orphans": d.orphans.iter().map(orphan_json).collect::<Vec<_>>(),
            "error": d.error,
        })).collect::<Vec<_>>(),
    }))
}

/// `std.deployment.prune`.
pub async fn prune(
    ops: &crate::apply_op::ApplyOps,
    name: &str,
    confirm: Option<&str>,
    confirm_deployment: Option<&str>,
    caller: &str,
) -> Result<Value, PruneRefusal> {
    let pool = &ops.pool;
    let envs = crate::environments::Environments::on(pool);
    let store = |e: crate::environments::EnvironmentRefusal| PruneRefusal::Store(e.to_string());
    let own = envs.own().await.map_err(store)?;
    let (enabled, _) = envs.prune_enabled().await.map_err(store)?;
    if !enabled {
        return Err(PruneRefusal::Disabled {
            environment: own.name,
        });
    }
    let name = name.trim();
    let all = every_database(ops).await?;
    let holding: Vec<(&Found, &Orphan)> = all
        .iter()
        .flat_map(|d| {
            d.orphans
                .iter()
                .filter(|o| o.is_live() && o.name() == name)
                .map(move |o| (d, o))
        })
        .collect();
    let Some((_, first)) = holding.first() else {
        let mut names: Vec<String> = all
            .iter()
            .flat_map(|d| d.orphans.iter().filter(|o| o.is_live()).map(Orphan::name))
            .collect();
        names.sort();
        names.dedup();
        return Err(PruneRefusal::NotAnOrphan {
            name: name.to_string(),
            orphans: names,
        });
    };
    let still = served_declaring(pool, first).await?;
    if !still.is_empty() {
        return Err(PruneRefusal::StillServed {
            name: name.to_string(),
            versions: still,
        });
    }
    let active = crate::schema_apply::active_schema(pool).await;
    let readers: Vec<String> = active
        .as_ref()
        .map(|s| {
            s.readers
                .iter()
                .filter(|r| r.tables.contains(&first.entity))
                .map(|r| r.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let deployment = active
        .as_ref()
        .and_then(|s| s.domain.clone())
        .unwrap_or_else(|| ops.home.clone());
    let production = own.name == crate::environments::DEFAULT_OWN;

    let mut databases = Vec::new();
    for (d, o) in &holding {
        let rows = match &d.pool {
            Some(p) => rows_holding(p, o).await?,
            None => 0,
        };
        databases.push(json!({ "database": d.label, "rows": rows, "columns": o.columns }));
    }
    let not_read: Vec<Value> = all
        .iter()
        .filter_map(|d| {
            d.error
                .as_ref()
                .map(|e| json!({ "database": d.label, "why": e }))
        })
        .collect();
    let mut row = json!({
        "name": name,
        "pruned": false,
        "databases": databases,
        "notRead": not_read,
        "readers": readers,
        "environment": own.name,
        "confirm": format!(
            "ordo evolve --prune {name} --confirm {name}{}",
            if production { format!(" --confirm-deployment {deployment}") } else { String::new() }
        ),
    });

    // The first ask: nothing is pruned without the name typed again.
    let Some(typed) = confirm.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(row);
    };
    if typed != name {
        return Err(PruneRefusal::Unconfirmed {
            name: name.to_string(),
            got: typed.to_string(),
        });
    }
    // A destructive command in production asks for the deployment's name.
    if production && confirm_deployment.map(str::trim) != Some(deployment.as_str()) {
        return Err(PruneRefusal::DeploymentUnconfirmed {
            expected: deployment,
        });
    }

    let lock = match crate::apply_op::ApplyLock::take(pool).await {
        Ok(Some(l)) => l,
        Ok(None) => return Err(PruneRefusal::Busy),
        Err(e) => return Err(PruneRefusal::Store(e.to_string())),
    };
    let at = crate::runtime::now_secs();
    let mut pruned = Vec::new();
    for (d, o) in &holding {
        let Some(p) = &d.pool else { continue };
        let sql = orphans::drop_sql(p.schema(), o);
        orphans::prune(p, o, &sql, caller, at).await?;
        pruned.push(d.label.clone());
    }
    // The reader grants follow: a pruned column is gone from every grant.
    if let Some(schema) = &active {
        if let Err(e) = ordo_exec::peer::grant_readers(pool, schema).await {
            eprintln!("prune: the reader grants were not refreshed: {e}");
        }
    }
    drop(lock);
    ops.record(
        caller,
        PRUNE,
        "orphan-pruned",
        format!("name={name} databases={} by={caller}", pruned.join("+")),
    );
    row["pruned"] = json!(true);
    row["prunedIn"] = json!(pruned);
    row["prunedAt"] = json!(at);
    Ok(row)
}

/// The two ops' specs.
pub fn specs() -> Vec<ordo_api::OpSpec> {
    use ordo_api::{
        FieldSpec, NotSyncable, OpCost, OpKind, OpSpec, ParamSpec, ParamTy, RefineTier, Syncability,
    };
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
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
        spec(ORPHANS, OpKind::Read, vec![], &["databases"]),
        spec(
            PRUNE,
            OpKind::Write,
            vec![
                text("name", 256, true),
                text("confirm", 256, false),
                text("confirmDeployment", 128, false),
            ],
            &[
                "name",
                "pruned",
                "databases",
                "notRead",
                "readers",
                "environment",
                "confirm",
                "prunedIn",
                "prunedAt",
            ],
        ),
    ]
}

/// Run one of the two. The caller is the deployment owner on the home
/// space (`deploy_ops::execute` checked).
pub async fn execute(
    ops: &crate::apply_op::ApplyOps,
    name: &str,
    bound: &ordo_api::BoundParams,
    caller: &str,
) -> Result<Value, ExecError> {
    let text = |k: &str| {
        bound
            .0
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    match name {
        ORPHANS => list(ops).await.map_err(|e| e.to_exec_error()),
        PRUNE => prune(
            ops,
            text("name").unwrap_or_default(),
            text("confirm"),
            text("confirmDeployment"),
            caller,
        )
        .await
        .map_err(|e| e.to_exec_error()),
        other => Err(ExecError::internal(format!(
            "`{other}` is not an orphan op"
        ))),
    }
}
