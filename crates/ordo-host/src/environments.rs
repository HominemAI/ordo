//! **The chain of environments a deployment declares** (`plan-versions.md`
//! §3.1, §4 row 4a). Each environment is its own deployment; this registry
//! is what one deployment knows of the chain it sits in: dev, staging,
//! production, in order, and which of them it is.
//!
//! - Stored in `$environments` in the control database, with who, when and
//!   why, beside `$version_settings`.
//! - **A deployment that has registered nothing reads as production**, the
//!   only environment in its chain. Nothing is written for that, so an
//!   upgrade needs no action.
//! - Registering an environment puts it in the chain before this
//!   deployment's own, so `dev` then `staging` give dev, staging,
//!   production. `position` places it elsewhere.
//! - A command that names an environment checks [`Environments::require`].
//!   A name not in the chain is refused, and the refusal names the command
//!   that registers it.
//!
//! The promotion gate (4b) is not here. This is the registry it reads.

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::{sql_lit, Pool, PoolError};

/// What a deployment that registered nothing is.
pub const DEFAULT_OWN: &str = "production";

/// The longest environment name, in characters.
pub const MAX_NAME_CHARS: usize = 32;

/// Who the implicit production is registered by.
pub const SYSTEM: &str = "ordo:environments";

/// The command that registers an environment, for a refusal to name.
pub fn register_command(name: &str) -> String {
    format!("ordo environments register {name} --reason \"...\"")
}

/// One environment in the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub name: String,
    /// 1 is the first step of the chain, where work starts.
    pub position: u32,
    /// This deployment is this environment.
    pub own: bool,
    /// Who registered it. [`SYSTEM`] for the implicit production.
    pub actor: String,
    pub reason: String,
    /// Seconds since the epoch; 0 for the implicit production.
    pub at: u64,
}

/// Why a registry call was refused, and what the caller can do.
#[derive(Debug)]
pub enum EnvironmentRefusal {
    /// The name is not in the chain. Register it, or name one of `chain`.
    Missing { name: String, chain: Vec<String> },
    /// The name is already in the chain. Nothing to do.
    Registered { name: String },
    /// The name is not a name. Fix the input.
    Name(String),
    /// The reason is empty or too long. Fix the input.
    Reason(String),
    /// The position is outside the chain. Fix the input.
    Position { given: u32, max: u32 },
    /// The deployment's own environment cannot be removed. Say which
    /// environment this deployment is first.
    RemoveOwn { name: String },
    /// A stored promotion record names the environment, and records are
    /// kept as they are.
    Named {
        name: String,
        records: u64,
        act: &'static str,
    },
    /// The database failed. Retry.
    Store(PoolError),
}

impl std::fmt::Display for EnvironmentRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvironmentRefusal::Missing { name, chain } => write!(
                f,
                "environment `{name}` is not registered. This deployment's chain is {}. \
                 Register it: `{}`",
                chain.join(", "),
                register_command(name)
            ),
            EnvironmentRefusal::Registered { name } => {
                write!(f, "environment `{name}` is already registered")
            }
            EnvironmentRefusal::Name(why) | EnvironmentRefusal::Reason(why) => {
                write!(f, "{why}")
            }
            EnvironmentRefusal::Position { given, max } => write!(
                f,
                "position {given} is outside the chain: use 1 to {max}, or leave it out to \
                 put the environment before this deployment's own"
            ),
            EnvironmentRefusal::RemoveOwn { name } => write!(
                f,
                "`{name}` is the environment this deployment is, so it stays. Say this \
                 deployment is another one first: `ordo environments set-own <name> \
                 --reason \"...\"`"
            ),
            EnvironmentRefusal::Named { name, records, act } => write!(
                f,
                "`{name}` is named by {records} stored promotion record(s), which are kept \
                 as they are, so it cannot be {act}. Leave it registered. To use another \
                 name, register it: `ordo environments register <new-name> --reason \
                 \"...\"`"
            ),
            EnvironmentRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<PoolError> for EnvironmentRefusal {
    fn from(e: PoolError) -> Self {
        EnvironmentRefusal::Store(e)
    }
}

impl EnvironmentRefusal {
    /// The door's answer: the caller's input is `invalid_param`, a name
    /// that is missing `not_found`, and a store failure keeps its class.
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            EnvironmentRefusal::Missing { .. } => ExecError::new(ErrorCode::NotFound, message),
            EnvironmentRefusal::Registered { .. } => ExecError::new(ErrorCode::Duplicate, message),
            EnvironmentRefusal::RemoveOwn { .. } | EnvironmentRefusal::Named { .. } => {
                ExecError::new(ErrorCode::ApplyRefused, message)
            }
            EnvironmentRefusal::Name(_)
            | EnvironmentRefusal::Reason(_)
            | EnvironmentRefusal::Position { .. } => {
                ExecError::new(ErrorCode::InvalidParam, message)
            }
            EnvironmentRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
        }
    }
}

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$environments\" ( \
  \"name\" text PRIMARY KEY, \
  \"pos\" integer NOT NULL, \
  \"own\" boolean NOT NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

const READ_SQL: &str = "SELECT \"name\", \"pos\"::text, \"own\"::text, \"actor\", \"reason\", \
     \"at\"::text FROM \"$environments\" ORDER BY \"pos\"";

/// A name as stored: 1 to [`MAX_NAME_CHARS`] of a to z, 0 to 9 and `-`,
/// starting with a letter.
pub fn check_name(name: &str) -> Result<String, EnvironmentRefusal> {
    let name = name.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(name.to_string())
    } else {
        Err(EnvironmentRefusal::Name(format!(
            "`{name}` is not an environment name: 1 to {MAX_NAME_CHARS} characters, a to z, \
             0 to 9 and `-`, starting with a letter"
        )))
    }
}

fn implicit() -> Environment {
    Environment {
        name: DEFAULT_OWN.to_string(),
        position: 1,
        own: true,
        actor: SYSTEM.to_string(),
        reason: "the deployment as it was before environments were registered".to_string(),
        at: 0,
    }
}

fn parse(rows: &[Vec<Option<String>>]) -> Vec<Environment> {
    rows.iter()
        .filter_map(|r| {
            let get = |i: usize| r.get(i).cloned().flatten();
            Some(Environment {
                name: get(0)?,
                position: get(1)?.parse().ok()?,
                own: get(2)? == "true",
                actor: get(3)?,
                reason: get(4)?,
                at: get(5)?.parse().ok()?,
            })
        })
        .collect()
}

/// The one client for the environment registry.
pub struct Environments<'a> {
    pool: &'a Pool,
}

impl<'a> Environments<'a> {
    pub fn on(pool: &'a Pool) -> Self {
        Environments { pool }
    }

    async fn ensure(&self) -> Result<(), PoolError> {
        if self.pool.rows(DDL).await.is_err() {
            self.pool.rows(DDL).await?;
        }
        Ok(())
    }

    /// The chain in order, first step first. A deployment that registered
    /// nothing answers production alone.
    pub async fn chain(&self) -> Result<Vec<Environment>, EnvironmentRefusal> {
        match self.pool.rows(READ_SQL).await {
            Ok(rows) if rows.is_empty() => Ok(vec![implicit()]),
            Ok(rows) => Ok(parse(&rows)),
            Err(e) if e.is_missing_table() => Ok(vec![implicit()]),
            Err(e) => Err(e.into()),
        }
    }

    /// Which environment this deployment is.
    pub async fn own(&self) -> Result<Environment, EnvironmentRefusal> {
        Ok(self
            .chain()
            .await?
            .into_iter()
            .find(|e| e.own)
            .unwrap_or_else(implicit))
    }

    /// **The environment `name`, or the refusal that names the command
    /// which registers it.** Every command that names an environment
    /// calls this first.
    pub async fn require(&self, name: &str) -> Result<Environment, EnvironmentRefusal> {
        let name = name.trim();
        let chain = self.chain().await?;
        match chain.iter().find(|e| e.name == name) {
            Some(e) => Ok(e.clone()),
            None => Err(EnvironmentRefusal::Missing {
                name: name.to_string(),
                chain: chain.into_iter().map(|e| e.name).collect(),
            }),
        }
    }

    /// **Register an environment** in the chain. `position` is 1 for the
    /// first step; left out, the environment goes before this deployment's
    /// own. Answers the chain as it stands.
    pub async fn register(
        &self,
        name: &str,
        position: Option<u32>,
        actor: &str,
        reason: &str,
        at: u64,
    ) -> Result<Vec<Environment>, EnvironmentRefusal> {
        let name = check_name(name)?;
        let reason = crate::versions::check_reason(reason)
            .map_err(|e| EnvironmentRefusal::Reason(e.to_string()))?;
        self.ensure().await?;
        let tx = self.pool.begin().await?;
        tx.rows("LOCK TABLE \"$environments\" IN EXCLUSIVE MODE")
            .await?;
        let mut chain = parse(&tx.rows(READ_SQL).await?);
        if chain.is_empty() {
            let own = implicit();
            tx.rows(&format!(
                "INSERT INTO \"$environments\" (\"name\", \"pos\", \"own\", \"actor\", \
                 \"reason\", \"at\") VALUES ({}, 1, true, {}, {}, 0)",
                sql_lit(&own.name),
                sql_lit(&own.actor),
                sql_lit(&own.reason)
            ))
            .await?;
            chain = vec![own];
        }
        if chain.iter().any(|e| e.name == name) {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::Registered { name });
        }
        let max = chain.len() as u32 + 1;
        let own_at = chain.iter().find(|e| e.own).map_or(max, |e| e.position);
        let pos = position.unwrap_or(own_at);
        if pos < 1 || pos > max {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::Position { given: pos, max });
        }
        tx.rows(&format!(
            "UPDATE \"$environments\" SET \"pos\" = \"pos\" + 1 WHERE \"pos\" >= {pos}"
        ))
        .await?;
        tx.rows(&format!(
            "INSERT INTO \"$environments\" (\"name\", \"pos\", \"own\", \"actor\", \"reason\", \
             \"at\") VALUES ({}, {pos}, false, {}, {}, {at})",
            sql_lit(&name),
            sql_lit(actor),
            sql_lit(&reason)
        ))
        .await?;
        let rows = tx.rows(READ_SQL).await?;
        tx.commit().await?;
        Ok(parse(&rows))
    }

    /// The chain read inside `tx`, with the implicit production written
    /// when nothing is stored, so a change has a row to change.
    async fn locked_chain(
        &self,
        tx: &ordo_exec::TxConn,
    ) -> Result<Vec<Environment>, EnvironmentRefusal> {
        tx.rows("LOCK TABLE \"$environments\" IN EXCLUSIVE MODE")
            .await?;
        let chain = parse(&tx.rows(READ_SQL).await?);
        if !chain.is_empty() {
            return Ok(chain);
        }
        let own = implicit();
        tx.rows(&format!(
            "INSERT INTO \"$environments\" (\"name\", \"pos\", \"own\", \"actor\", \"reason\", \
             \"at\") VALUES ({}, 1, true, {}, {}, 0)",
            sql_lit(&own.name),
            sql_lit(&own.actor),
            sql_lit(&own.reason)
        ))
        .await?;
        Ok(vec![own])
    }

    /// How many stored promotion records name `name`, as the step before or
    /// the target. A deployment that never promoted has none.
    pub async fn promotions_naming(&self, name: &str) -> Result<u64, EnvironmentRefusal> {
        let sql = format!(
            "SELECT count(*)::text FROM \"$promotions\" WHERE \"from_env\" = {n} OR \"to_env\" = {n}",
            n = sql_lit(name)
        );
        match self.pool.rows(&sql).await {
            Ok(rows) => Ok(rows
                .first()
                .and_then(|r| r.first().cloned().flatten())
                .and_then(|c| c.parse().ok())
                .unwrap_or(0)),
            Err(e) if e.is_missing_table() => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    /// **Say which environment this deployment is.** `name` must be
    /// registered. Answers the chain and whether anything changed.
    pub async fn set_own(
        &self,
        name: &str,
        reason: &str,
    ) -> Result<(Vec<Environment>, bool), EnvironmentRefusal> {
        crate::versions::check_reason(reason)
            .map_err(|e| EnvironmentRefusal::Reason(e.to_string()))?;
        self.ensure().await?;
        let tx = self.pool.begin().await?;
        let chain = self.locked_chain(&tx).await?;
        let name = name.trim();
        if !chain.iter().any(|e| e.name == name) {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::Missing {
                name: name.to_string(),
                chain: chain.into_iter().map(|e| e.name).collect(),
            });
        }
        if chain.iter().any(|e| e.own && e.name == name) {
            tx.rollback().await?;
            return Ok((chain, false));
        }
        tx.rows(&format!(
            "UPDATE \"$environments\" SET \"own\" = (\"name\" = {})",
            sql_lit(name)
        ))
        .await?;
        let rows = tx.rows(READ_SQL).await?;
        tx.commit().await?;
        Ok((parse(&rows), true))
    }

    /// **Remove an environment** from the chain. Refused for this
    /// deployment's own, and for one a stored promotion record names.
    pub async fn remove(
        &self,
        name: &str,
        reason: &str,
    ) -> Result<Vec<Environment>, EnvironmentRefusal> {
        crate::versions::check_reason(reason)
            .map_err(|e| EnvironmentRefusal::Reason(e.to_string()))?;
        self.ensure().await?;
        let name = name.trim();
        let tx = self.pool.begin().await?;
        let chain = self.locked_chain(&tx).await?;
        let Some(found) = chain.iter().find(|e| e.name == name).cloned() else {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::Missing {
                name: name.to_string(),
                chain: chain.into_iter().map(|e| e.name).collect(),
            });
        };
        if found.own {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::RemoveOwn {
                name: name.to_string(),
            });
        }
        let records = self.promotions_naming(name).await?;
        if records > 0 {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::Named {
                name: name.to_string(),
                records,
                act: "removed",
            });
        }
        tx.rows(&format!(
            "DELETE FROM \"$environments\" WHERE \"name\" = {}",
            sql_lit(name)
        ))
        .await?;
        tx.rows(&format!(
            "UPDATE \"$environments\" SET \"pos\" = \"pos\" - 1 WHERE \"pos\" > {}",
            found.position
        ))
        .await?;
        let rows = tx.rows(READ_SQL).await?;
        tx.commit().await?;
        Ok(parse(&rows))
    }

    /// **Rename an environment.** Refused for one a stored promotion record
    /// names, and for a name already in the chain.
    pub async fn rename(
        &self,
        name: &str,
        to: &str,
        reason: &str,
    ) -> Result<Vec<Environment>, EnvironmentRefusal> {
        let to = check_name(to)?;
        crate::versions::check_reason(reason)
            .map_err(|e| EnvironmentRefusal::Reason(e.to_string()))?;
        self.ensure().await?;
        let name = name.trim();
        let tx = self.pool.begin().await?;
        let chain = self.locked_chain(&tx).await?;
        if !chain.iter().any(|e| e.name == name) {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::Missing {
                name: name.to_string(),
                chain: chain.into_iter().map(|e| e.name).collect(),
            });
        }
        if chain.iter().any(|e| e.name == to) {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::Registered { name: to });
        }
        let records = self.promotions_naming(name).await?;
        if records > 0 {
            tx.rollback().await?;
            return Err(EnvironmentRefusal::Named {
                name: name.to_string(),
                records,
                act: "renamed",
            });
        }
        tx.rows(&format!(
            "UPDATE \"$environments\" SET \"name\" = {} WHERE \"name\" = {}",
            sql_lit(&to),
            sql_lit(name)
        ))
        .await?;
        let rows = tx.rows(READ_SQL).await?;
        tx.commit().await?;
        Ok(parse(&rows))
    }

    /// When the first environment besides the implicit production was
    /// registered. `None` before that.
    pub async fn first_registered_at(&self) -> Result<Option<u64>, EnvironmentRefusal> {
        Ok(self
            .chain()
            .await?
            .iter()
            .filter(|e| e.at > 0)
            .map(|e| e.at)
            .min())
    }
}
