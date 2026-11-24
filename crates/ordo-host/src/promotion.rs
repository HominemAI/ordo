//! **The promotion gate** (`plan-versions.md` §3.1, §4 row 4b). A version
//! moves from one environment to the next through the target, and the
//! target enforces the rules. The caller's own tree is never the source.
//!
//! The rules, each a refusal that says what to do:
//!
//! 1. The target has a step before it in its chain, and the source is that
//!    environment's ([`PromotionRefusal::NoStepBefore`], [`WrongSource`]).
//! 2. The step before has activated the version ([`NotActivated`]).
//! 3. The source arrives with the identity the step before recorded. The
//!    target recomputes it from the files it was handed and refuses a
//!    difference, naming the file ([`Edited`]).
//! 4. The target's own checks run before anything is stored: each secret a
//!    function declares has a value here ([`SecretsMissing`]), and each
//!    external entity checks out ([`External`]). The `requires` and reader
//!    checks are not built in the host yet and are not run here.
//! 5. The act is a named deployment owner's, recorded in `$promotions` and
//!    on the security stream.
//! 6. **A production deployment pins only a version that arrived through the
//!    gate** ([`require_arrival`]), once its chain has a step before it.
//!    Versions stored before the first environment was registered count as
//!    arrived, so a rollback to history still works.
//!
//! **Where the source comes from** is [`fetch_source`] and nothing else.
//! Today the caller hands the source over (the CLI reads it from the step
//! before's door as the person promoting, `std.deployment.versionSource`).
//! The owner's question 11 is open; when it is settled, the read from the
//! step before's door replaces that one function. No machine key is built.
//!
//! [`WrongSource`]: PromotionRefusal::WrongSource
//! [`NotActivated`]: PromotionRefusal::NotActivated
//! [`Edited`]: PromotionRefusal::Edited
//! [`SecretsMissing`]: PromotionRefusal::SecretsMissing
//! [`External`]: PromotionRefusal::External

use std::collections::BTreeMap;

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::route::PoolResolver;
use ordo_exec::{sql_lit, Pool, PoolError};

use crate::apply_op::{self, ApplyRefused, CodeFiles};
use crate::environments::{Environment, EnvironmentRefusal, Environments};
use crate::schema_apply::Program;

/// The environment whose pins the gate guards.
pub const GATED: &str = "production";

/// The op string recorded for a promotion.
pub const OP_LABEL: &str = "std.deployment.promote";

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$promotions\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"version\" text NOT NULL, \
  \"from_env\" text NOT NULL, \
  \"to_env\" text NOT NULL, \
  \"source_id\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

/// One version as the step before holds it: what the target is handed.
#[derive(Debug, Clone)]
pub struct SourceVersion {
    /// The step before's environment.
    pub env: String,
    /// The version's name there.
    pub version: String,
    /// The definition file's text.
    pub source: String,
    /// The source identity the step before recorded.
    pub source_id: String,
    /// Each function's code hash by code path, as the step before recorded.
    pub code_hashes: BTreeMap<String, String>,
    /// Each function's code, by path.
    pub code: CodeFiles,
    /// Whether the step before has activated it.
    pub activated: bool,
}

/// One stored promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub version: String,
    pub from_env: String,
    pub to_env: String,
    pub source_id: String,
    pub actor: String,
    pub reason: String,
    pub at: u64,
}

/// Why a promotion or a pin was refused, and what the caller can do.
#[derive(Debug)]
pub enum PromotionRefusal {
    /// This deployment is the first of its chain. Apply to it directly.
    NoStepBefore {
        own: String,
    },
    /// The source is not the environment before this one.
    WrongSource {
        given: String,
        expected: String,
    },
    /// The step before has not activated the version. Activate it there.
    NotActivated {
        env: String,
        version: String,
    },
    /// A file differs from what the step before recorded. Promote again from
    /// the step before, not from an edited tree.
    Edited {
        file: String,
        why: String,
    },
    /// Secrets a function declares have no value here. Set each.
    SecretsMissing(Vec<String>),
    /// An external entity does not check out here.
    External(Vec<String>),
    /// The program was refused as an apply would refuse it.
    Apply(ApplyRefused),
    /// The version did not arrive through the gate. Promote it.
    NotPromoted {
        version: String,
        own: String,
        before: String,
    },
    Environment(EnvironmentRefusal),
    Store(PoolError),
}

impl std::fmt::Display for PromotionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PromotionRefusal::NoStepBefore { own } => write!(
                f,
                "`{own}` is the first step of this deployment's chain, so nothing is promoted \
                 into it. Apply to it directly: `ordo apply --env {own}`"
            ),
            PromotionRefusal::WrongSource { given, expected } => write!(
                f,
                "the source is `{given}`, and the step before this deployment is \
                 `{expected}`. Promote from `{expected}`"
            ),
            PromotionRefusal::NotActivated { env, version } => write!(
                f,
                "`{env}` has not activated {version}. Activate it there first (`ordo apply` \
                 or `ordo rollback` on `{env}`), then promote it"
            ),
            PromotionRefusal::Edited { file, why } => write!(
                f,
                "{file} {why}. A promotion carries what the step before recorded, never an \
                 edited tree. Promote again from the step before"
            ),
            PromotionRefusal::SecretsMissing(names) => write!(
                f,
                "a function declares secrets this deployment has no value for: {}. Set each: \
                 `ordo secret set <name>`",
                names.join(", ")
            ),
            PromotionRefusal::External(lines) => write!(
                f,
                "an external entity does not check out on this deployment: {}",
                lines.join("; ")
            ),
            PromotionRefusal::Apply(a) => write!(f, "{a}"),
            PromotionRefusal::NotPromoted {
                version,
                own,
                before,
            } => write!(
                f,
                "{version} did not arrive through the promotion gate, and `{own}` pins only \
                 versions that did. Promote it from `{before}`: `ordo promote {version} \
                 --from <host> --reason \"...\"`"
            ),
            PromotionRefusal::Environment(e) => write!(f, "{e}"),
            PromotionRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<PoolError> for PromotionRefusal {
    fn from(e: PoolError) -> Self {
        PromotionRefusal::Store(e)
    }
}

impl From<EnvironmentRefusal> for PromotionRefusal {
    fn from(e: EnvironmentRefusal) -> Self {
        PromotionRefusal::Environment(e)
    }
}

impl PromotionRefusal {
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            PromotionRefusal::Environment(e) => e.to_exec_error(what),
            PromotionRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
            PromotionRefusal::Apply(a) => ExecError::new(a.code(), message),
            _ => ExecError::new(ErrorCode::ApplyRefused, message),
        }
    }
}

/// **The one place the source of a promotion comes from.** The target takes
/// the version from the step before, never from the caller's tree. Today
/// the caller carries what the step before's door answered
/// (`std.deployment.versionSource`) and this checks it names the right
/// step. A read through the step before's door, once the owner settles
/// question 11, replaces the body and nothing around it.
pub async fn fetch_source(
    before: &Environment,
    supplied: SourceVersion,
) -> Result<SourceVersion, PromotionRefusal> {
    if supplied.env != before.name {
        return Err(PromotionRefusal::WrongSource {
            given: supplied.env,
            expected: before.name.clone(),
        });
    }
    Ok(supplied)
}

/// The step before `own` in `chain`.
fn step_before<'a>(chain: &'a [Environment], own: &Environment) -> Option<&'a Environment> {
    chain.iter().find(|e| e.position + 1 == own.position)
}

/// Recompute what the step before recorded from the files handed over.
/// Refuses naming the first file that differs.
fn check_files(src: &SourceVersion) -> Result<(), PromotionRefusal> {
    for (path, recorded) in &src.code_hashes {
        let Some(file) = src.code.get(path) else {
            return Err(PromotionRefusal::Edited {
                file: format!("`{path}`"),
                why: "was not handed over".to_string(),
            });
        };
        let got = ordo_plan::artifact::code_hash(file.text.as_bytes());
        if &got != recorded {
            return Err(PromotionRefusal::Edited {
                file: format!("`{path}`"),
                why: format!(
                    "is not what `{}` activated: it hashes to {got}, and {} recorded {recorded}",
                    src.env, src.env
                ),
            });
        }
    }
    use ordo_plan::artifact as a;
    let id = a::source_identity(&src.source, a::FIRST_EDITION, &src.code_hashes);
    if id != src.source_id {
        return Err(PromotionRefusal::Edited {
            file: "the definition file".to_string(),
            why: format!(
                "is not what `{}` activated: its identity is {id}, and {} recorded {}",
                src.env, src.env, src.source_id
            ),
        });
    }
    Ok(())
}

/// The secrets the program declares that have no value in this deployment.
async fn missing_secrets(pool: &Pool, program: &Program) -> Result<Vec<String>, PromotionRefusal> {
    if program.schema.secrets.is_empty() {
        return Ok(vec![]);
    }
    let set: Vec<String> = match pool.rows("SELECT \"name\" FROM \"$secrets\"").await {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect(),
        Err(e) if e.is_missing_table() => vec![],
        Err(e) => return Err(e.into()),
    };
    Ok(program
        .schema
        .secrets
        .iter()
        .map(|s| s.name.name.clone())
        .filter(|n| !set.contains(n))
        .collect())
}

/// What a promotion did.
pub struct Promoted {
    pub from: String,
    pub to: String,
    pub source_id: String,
    pub report: apply_op::Report,
}

/// **Promote a version into this deployment.** Checks every gate rule and
/// the target's own checks before it stores anything, then stores the
/// version, records the promotion and activates it when asked.
#[allow(clippy::too_many_arguments)]
pub async fn promote(
    pool: &Pool,
    pools: &dyn PoolResolver,
    supplied: SourceVersion,
    actor: &str,
    reason: &str,
    activate: bool,
    at: u64,
) -> Result<Promoted, PromotionRefusal> {
    crate::versions::check_reason(reason)
        .map_err(|e| PromotionRefusal::Environment(EnvironmentRefusal::Reason(e.to_string())))?;
    let envs = Environments::on(pool);
    let chain = envs.chain().await?;
    let own = envs.own().await?;
    let Some(before) = step_before(&chain, &own).cloned() else {
        return Err(PromotionRefusal::NoStepBefore { own: own.name });
    };
    let src = fetch_source(&before, supplied).await?;
    if !src.activated {
        return Err(PromotionRefusal::NotActivated {
            env: src.env,
            version: src.version,
        });
    }
    check_files(&src)?;

    let (mut program, broken, notes) =
        apply_op::compile_checked(&src.source).map_err(PromotionRefusal::Apply)?;
    if !broken.is_empty() {
        return Err(PromotionRefusal::Apply(ApplyRefused::Broken {
            ops: broken,
        }));
    }
    let to_store = apply_op::bind_code(&mut program, &src.code).map_err(PromotionRefusal::Apply)?;
    if program.source_id != src.source_id {
        return Err(PromotionRefusal::Edited {
            file: "the definition file".to_string(),
            why: format!(
                "compiles here to identity {}, and {} recorded {}",
                program.source_id, src.env, src.source_id
            ),
        });
    }

    // The target's own checks, before anything is stored.
    let missing = missing_secrets(pool, &program).await?;
    if !missing.is_empty() {
        return Err(PromotionRefusal::SecretsMissing(missing));
    }
    let external = crate::external_check::check(pool, &program.schema)
        .await
        .map_err(|e| PromotionRefusal::Apply(ApplyRefused::Internal(e)))?;
    if !external.problems.is_empty() {
        return Err(PromotionRefusal::External(external.refusals()));
    }

    for bytes in to_store.values() {
        crate::functions::store_code(pool, bytes)
            .await
            .map_err(|e| PromotionRefusal::Apply(ApplyRefused::Internal(e)))?;
    }
    crate::versions::Versions::on(pool)
        .ensure()
        .await
        .map_err(|e| PromotionRefusal::Apply(ApplyRefused::Internal(e.to_string())))?;
    // The version's name here, so its record is written before the
    // activation that reads it, and taken back when the apply fails.
    let named = crate::schema_apply::name_version(pool, &program)
        .await
        .map_err(|e| PromotionRefusal::Apply(ApplyRefused::Internal(format!("{e:?}"))))?;
    pool.rows(DDL).await?;
    let seq = record(
        pool,
        &Promotion {
            version: named.version.clone(),
            from_env: src.env.clone(),
            to_env: own.name.clone(),
            source_id: src.source_id.clone(),
            actor: actor.to_string(),
            reason: reason.trim().to_string(),
            at,
        },
    )
    .await?;
    match apply_op::run_program_with(
        pool,
        pools,
        &program,
        notes,
        actor,
        OP_LABEL,
        activate,
        &crate::versions::Provenance::default(),
    )
    .await
    {
        Ok(report) => Ok(Promoted {
            from: src.env,
            to: own.name,
            source_id: src.source_id,
            report,
        }),
        Err(e) => {
            let _ = pool
                .rows(&format!(
                    "DELETE FROM \"$promotions\" WHERE \"seq\" = {seq}"
                ))
                .await;
            Err(PromotionRefusal::Apply(e))
        }
    }
}

async fn record(pool: &Pool, p: &Promotion) -> Result<i64, PoolError> {
    let rows = pool
        .rows(&format!(
            "INSERT INTO \"$promotions\" (\"version\", \"from_env\", \"to_env\", \"source_id\", \
             \"actor\", \"reason\", \"at\") VALUES ({}, {}, {}, {}, {}, {}, {}) RETURNING \"seq\"::text",
            sql_lit(&p.version),
            sql_lit(&p.from_env),
            sql_lit(&p.to_env),
            sql_lit(&p.source_id),
            sql_lit(&p.actor),
            sql_lit(&p.reason),
            p.at
        ))
        .await?;
    Ok(rows
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Every stored promotion, newest first.
pub async fn promotions(pool: &Pool, limit: usize) -> Result<Vec<Promotion>, PoolError> {
    let sql = format!(
        "SELECT \"version\", \"from_env\", \"to_env\", \"source_id\", \"actor\", \"reason\", \
         \"at\"::text FROM \"$promotions\" ORDER BY \"seq\" DESC LIMIT {}",
        limit.clamp(1, 500)
    );
    match pool.rows(&sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .filter_map(|r| {
                let g = |i: usize| r.get(i).cloned().flatten();
                Some(Promotion {
                    version: g(0)?,
                    from_env: g(1)?,
                    to_env: g(2)?,
                    source_id: g(3)?,
                    actor: g(4)?,
                    reason: g(5)?,
                    at: g(6)?.parse().ok()?,
                })
            })
            .collect()),
        Err(e) if e.is_missing_table() => Ok(vec![]),
        Err(e) => Err(e),
    }
}

/// **A production deployment pins only a version that arrived through the
/// gate**, once its chain has a step before it. A deployment that registered
/// nothing, or is not production, is not held to this. A version stored
/// before the first environment was registered counts as arrived.
pub async fn require_arrival(pool: &Pool, version: &str) -> Result<(), PromotionRefusal> {
    let envs = Environments::on(pool);
    let own = envs.own().await?;
    if own.name != GATED || own.position <= 1 {
        return Ok(());
    }
    let sql = format!(
        "SELECT 1 FROM \"$promotions\" WHERE \"version\" = {} AND \"to_env\" = {} LIMIT 1",
        sql_lit(version),
        sql_lit(&own.name)
    );
    match pool.rows(&sql).await {
        Ok(rows) if !rows.is_empty() => return Ok(()),
        Ok(_) => {}
        Err(e) if e.is_missing_table() => {}
        Err(e) => return Err(e.into()),
    }
    if let Some(first) = envs.first_registered_at().await? {
        let versions = crate::versions::Versions::on(pool);
        versions
            .ensure()
            .await
            .map_err(|e| PromotionRefusal::Apply(ApplyRefused::Internal(e.to_string())))?;
        let stored = versions
            .stored()
            .await
            .map_err(|e| PromotionRefusal::Apply(ApplyRefused::Internal(e.to_string())))?;
        if stored.iter().any(|(v, t, _)| v == version && t.0 < first) {
            return Ok(());
        }
    }
    let chain = envs.chain().await?;
    let before = step_before(&chain, &own)
        .map(|e| e.name.clone())
        .unwrap_or_default();
    Err(PromotionRefusal::NotPromoted {
        version: version.to_string(),
        own: own.name,
        before,
    })
}
