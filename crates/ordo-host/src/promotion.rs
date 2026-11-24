//! **The promotion gate** (`plan-versions.md` §3.1, §4 row 4b). A version
//! moves from one environment to the next through the target, and the
//! target enforces the rules. The caller's own tree is never the source.
//!
//! The rules, each a refusal that says what to do:
//!
//! 1. The target has a step before it in its chain ([`NoStepBefore`]), and
//!    knows where that step's door is ([`NoDoor`]).
//! 2. **The target reads the version from the step before itself**
//!    ([`fetch_source`]), as the person promoting (owner ruling 2026-11-24,
//!    Q11). The step before must answer as the environment the target
//!    expects ([`WrongSource`]), and must have read the same person
//!    ([`NotSamePerson`]).
//! 3. The step before has activated the version, as it says itself
//!    ([`NotActivated`]). Nothing the caller says about it is trusted.
//! 4. The files arrive with the identity the step before recorded. The
//!    target recomputes it and refuses a difference, naming the file
//!    ([`Edited`]).
//! 5. The target's own checks run before anything is stored: each secret a
//!    function declares has a value here ([`SecretsMissing`]), each external
//!    entity checks out ([`External`]), and each `reader` line has its role
//!    ([`Readers`]). The `requires` check is not here: the language has no
//!    `requires` yet (the parser refuses it, `ordo_syntax::auth`), and the
//!    host derives no needs (`plan-configuration-in-the-file.md` §3, step 2).
//! 6. The act is a named deployment owner's, recorded in `$promotions` and
//!    on the security stream.
//! 7. **Every deployment past the first step of its chain pins only a
//!    version that arrived through the gate** ([`require_arrival`]):
//!    staging and production alike (owner, 2026-11-24). Versions stored
//!    before the first environment was registered count as arrived, so a
//!    rollback to history still works.
//!
//! **Who reads the step before** is [`SourceCredential`], one seam. Today
//! it is the person promoting, with their own token for the step before
//! from the same sign-in. A GitHub workflow's token is a second variant
//! when it is built; nothing else in the gate changes. No machine key.
//!
//! [`NoStepBefore`]: PromotionRefusal::NoStepBefore
//! [`NoDoor`]: PromotionRefusal::NoDoor
//! [`WrongSource`]: PromotionRefusal::WrongSource
//! [`NotSamePerson`]: PromotionRefusal::NotSamePerson
//! [`NotActivated`]: PromotionRefusal::NotActivated
//! [`Edited`]: PromotionRefusal::Edited
//! [`SecretsMissing`]: PromotionRefusal::SecretsMissing
//! [`External`]: PromotionRefusal::External
//! [`Readers`]: PromotionRefusal::Readers

use std::collections::BTreeMap;

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::route::PoolResolver;
use ordo_exec::{sql_lit, Pool, PoolError};

use crate::apply_op::{self, ApplyRefused, CodeFiles};
use crate::environments::{self as envs_mod, Environment, EnvironmentRefusal, Environments};
use crate::schema_apply::Program;
use crate::step_door::{StepDoor, StepDoorError};

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

/// **Who reads the version from the step before.** The one seam between
/// the gate and the credential it reads with.
pub enum SourceCredential {
    /// The person promoting: their own token for the step before, from the
    /// same sign-in at the identity service. The step before checks the
    /// token and that they are an admin there; the target checks the step
    /// before read the same person it is serving.
    Person { token: String },
}

impl SourceCredential {
    fn bearer(&self) -> &str {
        match self {
            SourceCredential::Person { token } => token,
        }
    }
}

impl std::fmt::Debug for SourceCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceCredential::Person { .. } => f.write_str("SourceCredential::Person(..)"),
        }
    }
}

/// One version as the step before holds it: what the target read.
#[derive(Debug, Clone)]
pub struct SourceVersion {
    /// The step before's environment, as it names itself.
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
    /// Whether the step before has activated it, as the step before says.
    pub activated: bool,
    /// Who the step before read this as: the principal its door proved.
    pub read_by: String,
}

impl SourceVersion {
    /// What `std.deployment.versionSource` answers, read.
    pub fn from_answer(row: &serde_json::Value) -> Result<SourceVersion, String> {
        let text = |k: &str| {
            row[k]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("no `{k}`"))
        };
        Ok(SourceVersion {
            env: text("env")?,
            version: text("version")?,
            source: text("source")?,
            source_id: text("sourceId")?,
            code_hashes: serde_json::from_value(row["codeHashes"].clone())
                .map_err(|_| "`codeHashes` is not an object of text".to_string())?,
            code: apply_op::parse_code(row["code"].as_str()).map_err(|e| e.to_string())?,
            activated: row["activated"]
                .as_bool()
                .ok_or_else(|| "no `activated`".to_string())?,
            read_by: text("readBy")?,
        })
    }
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
    /// This deployment does not know where the step before answers.
    /// Record its door.
    NoDoor {
        env: String,
    },
    /// The step before's door answers as another environment. Correct the
    /// door recorded for it.
    WrongSource {
        given: String,
        expected: String,
    },
    /// The step before read the version as someone else. Sign in to both
    /// as the same person.
    NotSamePerson {
        env: String,
        there: String,
        here: String,
    },
    /// Reading from the step before failed.
    Source {
        env: String,
        error: StepDoorError,
        skip_hint: bool,
    },
    /// The step before has not activated the version. Activate it there.
    NotActivated {
        env: String,
        version: String,
        skip_hint: bool,
    },
    /// A file differs from what the step before recorded: its stored copy
    /// was changed after it was applied there.
    Edited {
        file: String,
        why: String,
    },
    /// Secrets a function declares have no value here. Set each.
    SecretsMissing(Vec<String>),
    /// An external entity does not check out here.
    External(Vec<String>),
    /// A `reader` line has no usable role here. The refusal names the
    /// setting.
    Readers(String),
    /// The program was refused as an apply would refuse it.
    Apply(ApplyRefused),
    /// The version did not arrive through the gate. Promote it.
    NotPromoted {
        version: String,
        own: String,
        before: String,
        first: String,
    },
    Environment(EnvironmentRefusal),
    Store(PoolError),
}

/// The line that names `promotion.skip_staging`, for a refusal about a
/// step before that is staging.
fn skip_line(skip_hint: bool) -> String {
    if skip_hint {
        format!(
            ". To promote from the step before staging instead, set {}: `ordo environments \
             set skip-staging on --reason \"...\"`",
            envs_mod::SKIP_STAGING
        )
    } else {
        String::new()
    }
}

impl std::fmt::Display for PromotionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PromotionRefusal::NoStepBefore { own } => write!(
                f,
                "`{own}` is the first step of this deployment's chain, so nothing is promoted \
                 into it. Apply to it directly: `ordo apply <path> --host <this deployment>`"
            ),
            PromotionRefusal::NoDoor { env } => write!(
                f,
                "this deployment does not know where `{env}`, the step before it, answers, so \
                 it cannot read the version there. Record it: `{}`",
                envs_mod::door_command(env)
            ),
            PromotionRefusal::WrongSource { given, expected } => write!(
                f,
                "the door recorded for `{expected}` answers as `{given}`. Record `{expected}`'s \
                 own door: `{}`",
                envs_mod::door_command(expected)
            ),
            PromotionRefusal::NotSamePerson { env, there, here } => write!(
                f,
                "`{env}` read the version as `{there}`, and this deployment serves you as \
                 `{here}`. A promotion is one person's act: sign in to both through the same \
                 identity service (`ordo login`), as an admin of both, and pass that \
                 sign-in's token for `{env}`"
            ),
            PromotionRefusal::Source {
                env,
                error,
                skip_hint,
            } => write!(f, "reading from `{env}`: {error}{}", skip_line(*skip_hint)),
            PromotionRefusal::NotActivated {
                env,
                version,
                skip_hint,
            } => write!(
                f,
                "`{env}` has not activated {version}. Activate it there first (`ordo apply` \
                 or `ordo rollback` on `{env}`), then promote it{}",
                skip_line(*skip_hint)
            ),
            PromotionRefusal::Edited { file, why } => write!(
                f,
                "{file} {why}. A promotion carries exactly what the step before recorded. \
                 Apply the version again on the step before, then promote it"
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
            PromotionRefusal::Readers(why) => {
                write!(
                    f,
                    "a `reader` line does not check out on this deployment: {why}"
                )
            }
            PromotionRefusal::Apply(a) => write!(f, "{a}"),
            PromotionRefusal::NotPromoted {
                version,
                own,
                before,
                first,
            } => write!(
                f,
                "{version} did not arrive through the promotion gate, and `{own}` takes only \
                 versions that did. Promote it from `{before}`: `ordo promote {version} --host \
                 <this deployment> --reason \"...\"`. A change starts on `{first}`: `ordo \
                 apply <path> --host <{first}'s address>`"
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
            PromotionRefusal::Source { error, .. } if error.retryable() => {
                ExecError::new(ErrorCode::SpaceUnavailable, message)
            }
            PromotionRefusal::Apply(a) => ExecError::new(a.code(), message),
            _ => ExecError::new(ErrorCode::ApplyRefused, message),
        }
    }
}

/// **The one place the source of a promotion comes from**: the step
/// before's own door, read as `credential`. Refused when no door is
/// recorded for it, when the door answers as another environment, and when
/// it read a different person than `caller`.
pub async fn fetch_source(
    before: &Environment,
    version: &str,
    credential: &SourceCredential,
    caller: &str,
    skip_hint: bool,
) -> Result<SourceVersion, PromotionRefusal> {
    let Some(door) = &before.door else {
        return Err(PromotionRefusal::NoDoor {
            env: before.name.clone(),
        });
    };
    let source = |error| PromotionRefusal::Source {
        env: before.name.clone(),
        error,
        skip_hint,
    };
    let client =
        StepDoor::parse(&door.address, crate::step_door::plain_http_allowed()).map_err(source)?;
    let row = client
        .version_source(&door.space, credential.bearer(), version)
        .await
        .map_err(source)?;
    let src = SourceVersion::from_answer(&row).map_err(|why| {
        source(StepDoorError::Answer {
            address: door.address.clone(),
            why: format!("a version that cannot be read: {why}"),
        })
    })?;
    if src.env != before.name {
        return Err(PromotionRefusal::WrongSource {
            given: src.env,
            expected: before.name.clone(),
        });
    }
    match credential {
        SourceCredential::Person { .. } if src.read_by != caller => {
            Err(PromotionRefusal::NotSamePerson {
                env: before.name.clone(),
                there: src.read_by,
                here: caller.to_string(),
            })
        }
        SourceCredential::Person { .. } => Ok(src),
    }
}

/// Recompute what the step before recorded from the files it answered.
/// Refuses naming the first file that differs.
fn check_files(src: &SourceVersion) -> Result<(), PromotionRefusal> {
    for (path, recorded) in &src.code_hashes {
        let Some(file) = src.code.get(path) else {
            return Err(PromotionRefusal::Edited {
                file: format!("`{path}`"),
                why: "was not in what the step before answered".to_string(),
            });
        };
        let got = ordo_plan::artifact::code_hash(file.text.as_bytes());
        if &got != recorded {
            return Err(PromotionRefusal::Edited {
                file: format!("`{path}`"),
                why: format!(
                    "is not what `{}` recorded: it hashes to {got}, and {} recorded {recorded}",
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
                "is not what `{}` recorded: its identity is {id}, and {} recorded {}",
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
#[derive(Debug)]
pub struct Promoted {
    pub from: String,
    pub to: String,
    pub source_id: String,
    pub report: apply_op::Report,
}

/// What a promotion asks for: which version, read as whom, and whether to
/// activate it here.
pub struct PromoteRequest<'a> {
    /// The version's name or order number on the step before.
    pub version: &'a str,
    pub credential: &'a SourceCredential,
    pub reason: &'a str,
    pub activate: bool,
}

/// **Promote a version into this deployment.** Reads it from the step
/// before as `actor`, checks every gate rule and the target's own checks
/// before it stores anything, then stores the version, records the
/// promotion and activates it when asked.
pub async fn promote(
    pool: &Pool,
    pools: &dyn PoolResolver,
    req: PromoteRequest<'_>,
    actor: &str,
    at: u64,
) -> Result<Promoted, PromotionRefusal> {
    let PromoteRequest {
        version,
        credential,
        reason,
        activate,
    } = req;
    crate::versions::check_reason(reason)
        .map_err(|e| PromotionRefusal::Environment(EnvironmentRefusal::Reason(e.to_string())))?;
    let envs = Environments::on(pool);
    let chain = envs.chain().await?;
    let own = envs.own().await?;
    let settings = envs.promotion_settings().await?;
    let Some(before) = envs_mod::promotion_source(&chain, &own, settings).cloned() else {
        return Err(PromotionRefusal::NoStepBefore { own: own.name });
    };
    // Name `promotion.skip_staging` when the step before is staging and
    // the setting is off.
    let skip_hint = before.name == envs_mod::STAGING
        && !settings.skip_staging
        && own.name != envs_mod::STAGING
        && chain.iter().any(|e| e.position < before.position);
    let src = fetch_source(&before, version, credential, actor, skip_hint).await?;
    if !src.activated {
        return Err(PromotionRefusal::NotActivated {
            env: src.env,
            version: src.version,
            skip_hint,
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
    ordo_exec::peer::check_readers(pool, &program.schema)
        .await
        .map_err(|e| PromotionRefusal::Readers(e.to_string()))?;

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

/// **A deployment past the first step of its chain pins only a version
/// that arrived through the gate**: staging and production alike (owner,
/// 2026-11-24). The first step, and a deployment that registered nothing,
/// are not held to this. A version stored before the first environment was
/// registered counts as arrived.
pub async fn require_arrival(pool: &Pool, version: &str) -> Result<(), PromotionRefusal> {
    let envs = Environments::on(pool);
    let chain = envs.chain().await?;
    let Some(own) = chain.iter().find(|e| e.own).cloned() else {
        return Ok(());
    };
    let Some(first) = chain.iter().min_by_key(|e| e.position).cloned() else {
        return Ok(());
    };
    if own.position <= first.position {
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
    let settings = envs.promotion_settings().await?;
    let before = envs_mod::promotion_source(&chain, &own, settings)
        .map(|e| e.name.clone())
        .unwrap_or_default();
    Err(PromotionRefusal::NotPromoted {
        version: version.to_string(),
        own: own.name,
        before,
        first: first.name,
    })
}
