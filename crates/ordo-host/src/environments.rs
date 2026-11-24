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
//! - **Each environment may carry its door** ([`Door`]): the address it is
//!   served at, its own space, and its handle at the identity service. The
//!   promotion gate reads the version from the step before's door, so the
//!   target decides where the step before is, never the caller.
//! - **`ordo apply` goes to the first step of the chain**
//!   ([`check_apply_target`]). That is dev in the default chain, and the
//!   deployment itself when it registered nothing, so a single deployment
//!   applies as before.
//! - `promotion.skip_staging` ([`PromotionSettings`]), off by default: with
//!   it on, a promotion past staging takes the version from the step before
//!   staging. Stored in `$promotion_settings` with who, when and why.
//!
//! The promotion gate (4b) is `crate::promotion`. This is the registry it
//! reads.

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

/// The command that records where an environment's door is.
pub fn door_command(name: &str) -> String {
    format!(
        "ordo environments door {name} <address> --space <its own space> --handle <its \
         handle at the identity service> --reason \"...\""
    )
}

/// The one promotion setting, off by default (`plan-versions.md` §3.1, §7).
pub const SKIP_STAGING: &str = "promotion.skip_staging";

/// `prune.enabled` (`plan-versions.md` §2.7, §7): whether `ordo evolve
/// --prune` may remove an orphaned column here. Off in production and on
/// in every other environment unless set.
pub const PRUNE_ENABLED: &str = "prune.enabled";

/// The command that sets it, for a refusal to name.
pub const PRUNE_COMMAND: &str = "ordo environments set prune on --reason \"...\"";

/// Whether a setting name given to `ordo environments set` is
/// `prune.enabled`.
pub fn is_prune_setting(name: &str) -> bool {
    matches!(
        name.trim(),
        "prune" | "prune.enabled" | "prune_enabled" | "prune-enabled"
    )
}

/// The environment `promotion.skip_staging` skips.
pub const STAGING: &str = "staging";

/// The longest door address, in characters.
pub const MAX_ADDRESS_CHARS: usize = 256;

/// Where an environment's deployment answers: what the promotion gate on
/// the next step reads the version from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Door {
    /// `https://host[:port]`, or `http://` to loopback.
    pub address: String,
    /// That deployment's own space, the one its deploy ops answer in.
    pub space: String,
    /// Its handle at the identity service (`aud: ordo-deployment:<handle>`),
    /// so `ordo promote` can ask the person's sign-in for a token there.
    pub handle: Option<String>,
}

impl Door {
    /// A door from its parts, checked: the address is `scheme://host[:port]`
    /// ([`crate::step_door::StepDoor::parse`]), the space and handle are
    /// names.
    pub fn checked(
        address: &str,
        space: &str,
        handle: Option<&str>,
    ) -> Result<Door, EnvironmentRefusal> {
        let address = address.trim().trim_end_matches('/').to_string();
        if address.chars().count() > MAX_ADDRESS_CHARS {
            return Err(EnvironmentRefusal::Door(format!(
                "the address is longer than {MAX_ADDRESS_CHARS} characters"
            )));
        }
        crate::step_door::StepDoor::parse(&address, crate::step_door::plain_http_allowed())
            .map_err(|e| EnvironmentRefusal::Door(e.to_string()))?;
        let space = space.trim();
        let space_ok = !space.is_empty()
            && space.chars().count() <= 64
            && space
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !space_ok {
            return Err(EnvironmentRefusal::Door(format!(
                "`{space}` is not a space name: the space that deployment was started with \
                 (its ORDO_SPACE), 1 to 64 letters, digits, `_` or `-`"
            )));
        }
        let handle = handle.map(str::trim).filter(|h| !h.is_empty());
        if let Some(h) = handle {
            let ok = h.chars().count() <= 63
                && h.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !ok {
                return Err(EnvironmentRefusal::Door(format!(
                    "`{h}` is not a deployment handle: lowercase letters, digits and dashes, \
                     at most 63 (that deployment's ORDO_IDENTITY_DEPLOYMENT)"
                )));
            }
        }
        Ok(Door {
            address,
            space: space.to_string(),
            handle: handle.map(str::to_string),
        })
    }
}

/// The promotion settings this deployment holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PromotionSettings {
    /// `promotion.skip_staging`.
    pub skip_staging: bool,
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
    /// Where its deployment answers, when recorded here.
    pub door: Option<Door>,
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
    /// The door's address, space or handle is not usable. Fix the input.
    Door(String),
    /// A promotion setting that does not exist, or a value it does not take.
    Setting(String),
    /// `ordo apply` goes to the first step of the chain, and this
    /// deployment is a later one. Apply to the first step, or promote.
    NotFirstStep {
        own: String,
        first: String,
        address: Option<String>,
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
            EnvironmentRefusal::Door(why) | EnvironmentRefusal::Setting(why) => {
                write!(f, "{why}")
            }
            EnvironmentRefusal::NotFirstStep {
                own,
                first,
                address,
            } => write!(
                f,
                "`ordo apply` goes to the first step of the chain, `{first}`, and this \
                 deployment is `{own}`, which takes a version only by promotion. Apply to \
                 `{first}`: `ordo apply <path> --host {}`. Then promote it here: `ordo \
                 promote <version> --host <this deployment> --reason \"...\"`",
                address.as_deref().unwrap_or("<its address>")
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
            EnvironmentRefusal::RemoveOwn { .. }
            | EnvironmentRefusal::Named { .. }
            | EnvironmentRefusal::NotFirstStep { .. } => {
                ExecError::new(ErrorCode::ApplyRefused, message)
            }
            EnvironmentRefusal::Name(_)
            | EnvironmentRefusal::Reason(_)
            | EnvironmentRefusal::Door(_)
            | EnvironmentRefusal::Setting(_)
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

/// The door columns, added after 4a: a table 4a made gains them on the
/// next read or write.
const DOOR_DDL: &str = "\
ALTER TABLE \"$environments\" ADD COLUMN IF NOT EXISTS \"door_address\" text; \
ALTER TABLE \"$environments\" ADD COLUMN IF NOT EXISTS \"door_space\" text; \
ALTER TABLE \"$environments\" ADD COLUMN IF NOT EXISTS \"door_handle\" text";

const READ_SQL: &str = "SELECT \"name\", \"pos\"::text, \"own\"::text, \"actor\", \"reason\", \
     \"at\"::text, \"door_address\", \"door_space\", \"door_handle\" FROM \"$environments\" \
     ORDER BY \"pos\"";

/// `$promotion_settings`: every change to a promotion setting, newest last.
const SETTINGS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$promotion_settings\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"setting\" text NOT NULL, \
  \"value\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

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
        door: None,
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
                door: match (get(6), get(7)) {
                    (Some(address), Some(space)) => Some(Door {
                        address,
                        space,
                        handle: get(8),
                    }),
                    _ => None,
                },
            })
        })
        .collect()
}

/// **Where a promotion into `own` takes its version from**: the step before
/// it, or with `promotion.skip_staging` on and that step `staging`, the
/// step before staging. `None` for the first step of the chain.
pub fn promotion_source<'a>(
    chain: &'a [Environment],
    own: &Environment,
    settings: PromotionSettings,
) -> Option<&'a Environment> {
    let mut earlier: Vec<&Environment> =
        chain.iter().filter(|e| e.position < own.position).collect();
    earlier.sort_by_key(|e| std::cmp::Reverse(e.position));
    let mut it = earlier.into_iter();
    let first = it.next()?;
    if settings.skip_staging && first.name == STAGING {
        return it.next().or(Some(first));
    }
    Some(first)
}

/// **Where `ordo apply` may go** (owner, 2026-11-24: it goes to dev by
/// default). With no environment named, the deployment must be the first
/// step of its chain: dev in the default chain, or the deployment itself
/// when it registered nothing, so a single deployment applies as it always
/// did. A later step is refused, naming the first step's address when it
/// is recorded. With a name, the CLI's `--env` check applies instead.
pub fn check_apply_target(chain: &[Environment]) -> Result<(), EnvironmentRefusal> {
    let Some(own) = chain.iter().find(|e| e.own) else {
        return Ok(());
    };
    let Some(first) = chain.iter().min_by_key(|e| e.position) else {
        return Ok(());
    };
    if own.position <= first.position {
        return Ok(());
    }
    Err(EnvironmentRefusal::NotFirstStep {
        own: own.name.clone(),
        first: first.name.clone(),
        address: first.door.as_ref().map(|d| d.address.clone()),
    })
}

/// The `SET` list that writes a door, or clears it.
fn door_set(door: Option<&Door>) -> String {
    match door {
        Some(d) => format!(
            "\"door_address\" = {}, \"door_space\" = {}, \"door_handle\" = {}",
            sql_lit(&d.address),
            sql_lit(&d.space),
            d.handle
                .as_deref()
                .map(sql_lit)
                .unwrap_or_else(|| "NULL".into())
        ),
        None => {
            "\"door_address\" = NULL, \"door_space\" = NULL, \"door_handle\" = NULL".to_string()
        }
    }
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
        self.pool.rows(DOOR_DDL).await?;
        Ok(())
    }

    /// The chain in order, first step first. A deployment that registered
    /// nothing answers production alone.
    pub async fn chain(&self) -> Result<Vec<Environment>, EnvironmentRefusal> {
        let rows = match self.pool.rows(READ_SQL).await {
            Err(e) if e.is_missing_column() => {
                // A table 4a made: give it the door columns, then read.
                self.ensure().await?;
                self.pool.rows(READ_SQL).await
            }
            other => other,
        };
        match rows {
            Ok(rows) if rows.is_empty() => Ok(vec![implicit()]),
            Ok(rows) => Ok(parse(&rows)),
            Err(e) if e.is_missing_table() => Ok(vec![implicit()]),
            Err(e) => Err(e.into()),
        }
    }

    /// The promotion settings: each one's latest value, its default when
    /// never set.
    pub async fn promotion_settings(&self) -> Result<PromotionSettings, EnvironmentRefusal> {
        let sql = format!(
            "SELECT \"value\" FROM \"$promotion_settings\" WHERE \"setting\" = {} \
             ORDER BY \"seq\" DESC LIMIT 1",
            sql_lit(SKIP_STAGING)
        );
        match self.pool.rows(&sql).await {
            Ok(rows) => Ok(PromotionSettings {
                skip_staging: rows
                    .first()
                    .and_then(|r| r.first().cloned().flatten())
                    .is_some_and(|v| v == "on"),
            }),
            Err(e) if e.is_missing_table() => Ok(PromotionSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// **Set a promotion setting**, with a reason. `promotion.skip_staging`
    /// takes `on` or `off`. Answers the settings and whether anything
    /// changed.
    pub async fn set_promotion_setting(
        &self,
        setting: &str,
        value: &str,
        actor: &str,
        reason: &str,
        at: u64,
    ) -> Result<(PromotionSettings, bool), EnvironmentRefusal> {
        let setting = setting.trim();
        let named = setting.strip_prefix("promotion.").unwrap_or(setting);
        if !matches!(named, "skip_staging" | "skip-staging") {
            return Err(EnvironmentRefusal::Setting(format!(
                "`{setting}` is not a promotion setting. There are {SKIP_STAGING} and \
                 {PRUNE_ENABLED}"
            )));
        }
        let on = match value.trim() {
            "on" => true,
            "off" => false,
            other => {
                return Err(EnvironmentRefusal::Setting(format!(
                    "{SKIP_STAGING} takes `on` or `off`, not `{other}`"
                )))
            }
        };
        let reason = crate::versions::check_reason(reason)
            .map_err(|e| EnvironmentRefusal::Reason(e.to_string()))?;
        let now = self.promotion_settings().await?;
        if now.skip_staging == on {
            return Ok((now, false));
        }
        self.pool.rows(SETTINGS_DDL).await?;
        self.pool
            .rows(&format!(
                "INSERT INTO \"$promotion_settings\" (\"setting\", \"value\", \"actor\", \
                 \"reason\", \"at\") VALUES ({}, {}, {}, {}, {at})",
                sql_lit(SKIP_STAGING),
                sql_lit(if on { "on" } else { "off" }),
                sql_lit(actor),
                sql_lit(&reason)
            ))
            .await?;
        Ok((PromotionSettings { skip_staging: on }, true))
    }

    /// `prune.enabled`: its latest value, or off in production and on
    /// elsewhere when never set. Answers the value and whether it was set.
    pub async fn prune_enabled(&self) -> Result<(bool, bool), EnvironmentRefusal> {
        let sql = format!(
            "SELECT \"value\" FROM \"$promotion_settings\" WHERE \"setting\" = {} \
             ORDER BY \"seq\" DESC LIMIT 1",
            sql_lit(PRUNE_ENABLED)
        );
        let set = match self.pool.rows(&sql).await {
            Ok(rows) => rows.first().and_then(|r| r.first().cloned().flatten()),
            Err(e) if e.is_missing_table() => None,
            Err(e) => return Err(e.into()),
        };
        Ok(match set {
            Some(v) => (v == "on", true),
            None => (self.own().await?.name != DEFAULT_OWN, false),
        })
    }

    /// **Set `prune.enabled`**, `on` or `off`, with a reason. Answers the
    /// value and whether anything changed.
    pub async fn set_prune_enabled(
        &self,
        value: &str,
        actor: &str,
        reason: &str,
        at: u64,
    ) -> Result<(bool, bool), EnvironmentRefusal> {
        let on = match value.trim() {
            "on" => true,
            "off" => false,
            other => {
                return Err(EnvironmentRefusal::Setting(format!(
                    "{PRUNE_ENABLED} takes `on` or `off`, not `{other}`"
                )))
            }
        };
        let reason = crate::versions::check_reason(reason)
            .map_err(|e| EnvironmentRefusal::Reason(e.to_string()))?;
        let (now, set) = self.prune_enabled().await?;
        if set && now == on {
            return Ok((on, false));
        }
        self.pool.rows(SETTINGS_DDL).await?;
        self.pool
            .rows(&format!(
                "INSERT INTO \"$promotion_settings\" (\"setting\", \"value\", \"actor\", \
                 \"reason\", \"at\") VALUES ({}, {}, {}, {}, {at})",
                sql_lit(PRUNE_ENABLED),
                sql_lit(if on { "on" } else { "off" }),
                sql_lit(actor),
                sql_lit(&reason)
            ))
            .await?;
        Ok((on, now != on))
    }

    /// **Record where an environment's door is**, or clear it with `None`.
    /// The environment must be registered. Answers the chain.
    pub async fn set_door(
        &self,
        name: &str,
        door: Option<Door>,
        reason: &str,
    ) -> Result<Vec<Environment>, EnvironmentRefusal> {
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
        tx.rows(&format!(
            "UPDATE \"$environments\" SET {} WHERE \"name\" = {}",
            door_set(door.as_ref()),
            sql_lit(name)
        ))
        .await?;
        let rows = tx.rows(READ_SQL).await?;
        tx.commit().await?;
        Ok(parse(&rows))
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
        self.register_with_door(name, position, None, actor, reason, at)
            .await
    }

    /// [`Environments::register`], with where its door is.
    pub async fn register_with_door(
        &self,
        name: &str,
        position: Option<u32>,
        door: Option<Door>,
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
        if door.is_some() {
            tx.rows(&format!(
                "UPDATE \"$environments\" SET {} WHERE \"name\" = {}",
                door_set(door.as_ref()),
                sql_lit(&name)
            ))
            .await?;
        }
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
