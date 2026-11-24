//! **Staging clones named production tenants** (`plan-versions.md` §3.4,
//! §4 row 12). Ruled 2026-11-02 (D17) and 2026-11-23 (Q12).
//!
//! # The path
//!
//! 1. Staging asks production, as the person cloning, with their own token
//!    for production ([`clone_in`], through [`crate::step_door`], the one
//!    client to another deployment's door).
//! 2. **Production masks before anything leaves it** ([`clone_out`]): the
//!    tenant is exported at its own version, then every field not named
//!    safe in `clone.mask` is replaced ([`mask_bundle`]). Deny by default.
//!    Blobs are dropped, and so is the command log, whose images hold the
//!    values the rows held.
//! 3. Production seals the masked bundle to staging's public key, the one
//!    recorded on production for that environment ([`set_recipient`]).
//!    Only staging opens it.
//! 4. Staging opens it, makes a new tenant, pins it to the clone's version
//!    and imports the rows through the write protocol, so every value is
//!    sealed again under staging's own keys.
//!
//! # What is never carried
//!
//! - Credentials, passkeys, sessions and invites: they live in the
//!   deployment's directory, never in a tenant's rows, so the export never
//!   reads them.
//! - **A member's sign-in link** (Q12). Each cloned member's `principal` is
//!   [`CLONE_PRINCIPAL_PREFIX`] and its own id, which no sign-in ever
//!   yields, so a cloned member cannot sign in to the clone.
//! - Foreign tables. They are not Ordo's; staging needs its own copy of
//!   each and its own peer. The answer names them.
//! - A production database or bucket credential: the clone lands in
//!   staging's storage.
//!
//! # Who opens a clone
//!
//! **Ruled 2026-11-23 (Q12):** any staging deployment admin, or a person
//! the staging deployment allows (`clone.readers`). [`CloneAccess`] is what
//! the door asks. The person acts as the clone's founding owner, and is
//! recorded as the agent beside it.
//!
//! # The version
//!
//! The clone carries its version's source identity. Staging finds the
//! version it holds with that identity and pins the clone to it. A version
//! staging does not hold comes down with the clone through the promotion
//! gate ([`crate::promotion::arrive_with_clone`]): read from production as
//! the same person, checked, compiled and gated as a promotion is, stored
//! and never activated, and recorded in `$promotions` as a clone.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use base64::Engine as _;
use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_crypto::recipient::{RecipientError, RecipientPublic, RecipientSecret};
use ordo_crypto::{Anchor, KeyProvider, Purpose, SpaceId};
use ordo_exec::route::PoolResolver;
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_schema::{Entity, Field, FieldTy, Schema};
use ordo_syntax::ScalarTy;
use serde_json::{json, Value};

use crate::environments::{EnvironmentRefusal, Environments};
use crate::step_door::{StepDoor, StepDoorError};

/// Staging's public clone key: what production seals a clone to.
pub const CLONE_KEY: &str = "std.deployment.cloneKey";
/// Record on production the clone key of an environment that clones it.
pub const SET_CLONE_KEY: &str = "std.deployment.setCloneKey";
/// Production: one tenant, masked and sealed to an environment's key.
pub const CLONE_OUT: &str = "std.deployment.cloneOut";
/// Staging: clone a production tenant into a new tenant here.
pub const CLONE_IN: &str = "std.deployment.cloneIn";
/// The clones this deployment holds, and the clone settings.
pub const CLONES: &str = "std.deployment.clones";
/// Set `clone.mask` or `clone.readers`.
pub const SET_CLONE_SETTING: &str = "std.deployment.setCloneSetting";
/// What the clones would have sent: their mail, push and egress
/// (`crate::sinks`).
pub const SINKS: &str = "std.deployment.sinks";

/// Every op of this module.
pub const OPS: [&str; 7] = [
    CLONE_KEY,
    SET_CLONE_KEY,
    CLONE_OUT,
    CLONE_IN,
    CLONES,
    SET_CLONE_SETTING,
    SINKS,
];

/// Which fields leave production unmasked. Deny by default.
pub const MASK_SETTING: &str = "clone.mask";
/// Who besides the staging deployment's admins opens a clone.
pub const READERS_SETTING: &str = "clone.readers";
/// Which tenants' mail, push and egress go to the sink (`crate::sinks`).
pub const SINKS_SETTING: &str = "clone.sinks";

/// The sealed bundle's format tag, also bound into its associated data.
pub const FORMAT: &str = "ordo.clone.v1";

/// A cloned member's `principal`: this, then its own id. No sign-in yields
/// a principal that starts with `$`.
pub const CLONE_PRINCIPAL_PREFIX: &str = "$clone:";

/// What a masked `date` reads.
pub const MASKED_DATE: &str = "2000-01-01";
/// What a masked `datetime` reads.
pub const MASKED_DATETIME: &str = "2000-01-01T00:00:00Z";

const KEY_ENTITY: &str = "$clone_key";
const KEY_FIELD: &str = "secret";

const KEY_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$clone_key\" ( \
  \"id\" integer PRIMARY KEY, \
  \"public\" text NOT NULL, \
  \"sealed\" bytea NOT NULL, \
  \"at\" bigint NOT NULL \
)";

const RECIPIENTS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$clone_recipients\" ( \
  \"env\" text PRIMARY KEY, \
  \"public\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

const CLONES_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$clones\" ( \
  \"space\" text PRIMARY KEY, \
  \"name\" text NOT NULL, \
  \"from_env\" text NOT NULL, \
  \"from_tenant\" text NOT NULL, \
  \"version\" text NOT NULL, \
  \"source_id\" text NOT NULL, \
  \"opens_as\" text NOT NULL, \
  \"rows\" bigint NOT NULL, \
  \"actor\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

/// **No production tenant name stays in staging** (2026-11-24). Records
/// written before kept it in `from_name`, and a clone named by the old
/// default was `<that name>-clone`, here and in `$spaces`. Each such clone
/// gets the neutral name ([`default_name`]: `clone-` and its id's last 8),
/// then the column is dropped, so the names are gone, not hidden. A clone
/// the person named keeps its name. Run before every read and write of
/// `$clones`; once the column is gone it does nothing.
const FROM_NAME_MIGRATION: &str = "\
DO $mig$ BEGIN \
  IF EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = current_schema() \
             AND table_name = '$clones' AND column_name = 'from_name') THEN \
    IF to_regclass('\"$spaces\"') IS NOT NULL THEN \
      UPDATE \"$spaces\" s SET \"name\" = 'clone-' || right(c.\"space\", 8) FROM \"$clones\" c \
       WHERE s.\"$space\" = c.\"space\" AND s.\"name\" = c.\"from_name\" || '-clone'; \
    END IF; \
    UPDATE \"$clones\" SET \"name\" = 'clone-' || right(\"space\", 8) \
     WHERE \"name\" = \"from_name\" || '-clone'; \
    ALTER TABLE \"$clones\" DROP COLUMN \"from_name\"; \
  END IF; \
END $mig$";

/// `$clones`, made or brought up to date.
async fn ensure_clones(pool: &Pool) -> Result<(), PoolError> {
    pool.rows(CLONES_DDL).await?;
    pool.rows(FROM_NAME_MIGRATION).await?;
    Ok(())
}

// ── refusals ─────────────────────────────────────────────────────────────

/// Why a clone was refused, and what the caller can do.
#[derive(Debug)]
pub enum CloneRefusal {
    /// Production has no clone key recorded for the environment. Record it.
    NoKey {
        env: String,
    },
    /// An environment cannot clone from itself or into itself.
    OwnEnv {
        env: String,
    },
    /// No tenant here by that id or name.
    NoTenant {
        tenant: String,
    },
    /// The tenant has no pinned version, so there is no shape to export.
    NotPinned {
        tenant: String,
    },
    /// Staging does not know where production answers. Record its door.
    NoDoor {
        env: String,
    },
    /// Reaching production failed.
    Source {
        env: String,
        error: StepDoorError,
    },
    /// The door recorded for production answers as another environment.
    WrongSource {
        given: String,
        expected: String,
    },
    /// Production read the clone as another person.
    NotSamePerson {
        env: String,
        there: String,
        here: String,
    },
    /// The sealed bundle did not open, or its contents are not a clone.
    Bundle(String),
    /// The clone's version, which staging did not hold, did not pass the
    /// promotion gate on its way down.
    Version {
        version: String,
        from: String,
        refusal: Box<crate::promotion::PromotionRefusal>,
    },
    /// A setting that does not exist, or a value it does not take.
    Setting(String),
    /// The input is not usable: a name, a key, a reason.
    Input(String),
    /// Masking, export or import refused, with the cause.
    Data(String),
    /// Custody refused: the deployment's keys.
    Custody(String),
    Environment(EnvironmentRefusal),
    Store(PoolError),
}

impl std::fmt::Display for CloneRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloneRefusal::NoKey { env } => write!(
                f,
                "this deployment has no clone key recorded for `{env}`, so it cannot seal a \
                 clone to it. On `{env}`, print its key: `ordo clones key`. Then record it \
                 here: `ordo clones set-key {env} <key> --reason \"...\"`"
            ),
            CloneRefusal::OwnEnv { env } => write!(
                f,
                "`{env}` is this deployment's own environment. A clone goes from one \
                 deployment to another: name the environment it comes from, `--from \
                 production`"
            ),
            CloneRefusal::NoTenant { tenant } => write!(
                f,
                "no tenant here is `{tenant}`, by id or by name. List them: `ordo tenants`"
            ),
            CloneRefusal::NotPinned { tenant } => write!(
                f,
                "tenant `{tenant}` is not pinned to a version, so there is no shape to clone. \
                 Pin it: `ordo versions pin {tenant} <version> --reason \"...\"`"
            ),
            CloneRefusal::NoDoor { env } => write!(
                f,
                "this deployment does not know where `{env}` answers, so it cannot ask it for \
                 a clone. Record it: `{}`",
                crate::environments::door_command(env)
            ),
            CloneRefusal::Source { env, error } => {
                write!(f, "asking `{env}` for the clone: {error}")
            }
            CloneRefusal::WrongSource { given, expected } => write!(
                f,
                "the door recorded for `{expected}` answers as `{given}`. Record `{expected}`'s \
                 own door: `{}`",
                crate::environments::door_command(expected)
            ),
            CloneRefusal::NotSamePerson { env, there, here } => write!(
                f,
                "`{env}` made the clone for `{there}`, and this deployment serves you as \
                 `{here}`. A clone is one person's act: sign in to both through the same \
                 identity service (`ordo login`), as an admin of both, and pass that sign-in's \
                 token for `{env}`"
            ),
            CloneRefusal::Bundle(why) => write!(f, "the clone did not arrive whole: {why}"),
            CloneRefusal::Version {
                version,
                from,
                refusal,
            } => write!(
                f,
                "the tenant serves {version} on `{from}`, which this deployment does not hold, \
                 and it was refused on its way down with the clone: {refusal}"
            ),
            CloneRefusal::Setting(why) | CloneRefusal::Input(why) => write!(f, "{why}"),
            CloneRefusal::Data(why) => write!(f, "{why}"),
            CloneRefusal::Custody(why) => write!(f, "this deployment's keys refused: {why}"),
            CloneRefusal::Environment(e) => write!(f, "{e}"),
            CloneRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CloneRefusal {}

impl From<PoolError> for CloneRefusal {
    fn from(e: PoolError) -> Self {
        CloneRefusal::Store(e)
    }
}

impl From<EnvironmentRefusal> for CloneRefusal {
    fn from(e: EnvironmentRefusal) -> Self {
        CloneRefusal::Environment(e)
    }
}

impl CloneRefusal {
    /// Whether the same request could succeed later.
    pub fn retryable(&self) -> bool {
        match self {
            CloneRefusal::Source { error, .. } => error.retryable(),
            CloneRefusal::Version { refusal, .. } => matches!(
                refusal.as_ref(),
                crate::promotion::PromotionRefusal::Store(_)
            ),
            CloneRefusal::Store(_) => true,
            _ => false,
        }
    }

    /// The door's answer.
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            CloneRefusal::Environment(e) => e.to_exec_error(what),
            CloneRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
            CloneRefusal::Source { error, .. } if error.retryable() => {
                ExecError::new(ErrorCode::SpaceUnavailable, message)
            }
            CloneRefusal::Version { refusal, .. } => {
                let mut e = refusal.to_exec_error(what);
                e.message = message;
                e
            }
            CloneRefusal::NoTenant { .. } => ExecError::new(ErrorCode::NotFound, message),
            CloneRefusal::Setting(_) | CloneRefusal::Input(_) => {
                ExecError::new(ErrorCode::InvalidParam, message)
            }
            CloneRefusal::Custody(_) => ExecError::internal(message),
            _ => ExecError::new(ErrorCode::ApplyRefused, message),
        }
    }
}

fn reason_of(reason: &str) -> Result<String, CloneRefusal> {
    crate::versions::check_reason(reason).map_err(|e| CloneRefusal::Input(e.to_string()))
}

// ── settings ─────────────────────────────────────────────────────────────

/// `clone.mask`: which fields leave production as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mask {
    /// Every field not named in `safe` (`entity.field`) is masked. The
    /// default, with nothing safe.
    Deny { safe: BTreeSet<String> },
    /// Masking off: an opt-in the deployment owner sets by name (27v).
    Off,
}

impl Default for Mask {
    fn default() -> Self {
        Mask::Deny {
            safe: BTreeSet::new(),
        }
    }
}

impl Mask {
    /// As stored and shown: `deny`, `off`, or the safe fields.
    pub fn value(&self) -> String {
        match self {
            Mask::Off => "off".to_string(),
            Mask::Deny { safe } if safe.is_empty() => "deny".to_string(),
            Mask::Deny { safe } => safe.iter().cloned().collect::<Vec<_>>().join(","),
        }
    }

    /// From a stored or given value: `deny`, `off`, or a comma list of
    /// `entity.field` that are safe, every other field denied.
    pub fn parse(value: &str) -> Result<Mask, CloneRefusal> {
        let v = value.trim();
        match v {
            "" | "deny" => return Ok(Mask::default()),
            "off" => return Ok(Mask::Off),
            _ => {}
        }
        let mut safe = BTreeSet::new();
        for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let ok = part.split_once('.').is_some_and(|(e, f)| {
                let word = |w: &str| {
                    !w.is_empty()
                        && w.starts_with(|c: char| c.is_ascii_alphabetic())
                        && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                };
                word(e) && word(f)
            });
            if !ok {
                return Err(CloneRefusal::Setting(format!(
                    "`{part}` is not a field: {MASK_SETTING} takes `deny`, `off`, or the safe \
                     fields as `entity.field,entity.field`"
                )));
            }
            safe.insert(part.to_string());
        }
        Ok(Mask::Deny { safe })
    }
}

/// The clone settings this deployment holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloneSettings {
    /// `clone.mask`, read on the deployment a clone leaves.
    pub mask: Mask,
    /// `clone.readers`, read on the deployment a clone lands in.
    pub readers: BTreeSet<String>,
    /// `clone.sinks`, read on the deployment a clone lands in.
    pub sinks: crate::sinks::SinkScope,
}

fn parse_readers(value: &str) -> Result<BTreeSet<String>, CloneRefusal> {
    let v = value.trim();
    if v.is_empty() || v == "none" {
        return Ok(BTreeSet::new());
    }
    let mut out = BTreeSet::new();
    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let ok = part.chars().count() <= 128
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '@'));
        if !ok {
            return Err(CloneRefusal::Setting(format!(
                "`{part}` is not a person: {READERS_SETTING} takes `none`, or each person's \
                 subject at the identity service, comma separated"
            )));
        }
        out.insert(part.to_string());
    }
    Ok(out)
}

/// The clone settings as stored in `$version_settings` (owner, 2026-11-23,
/// Q15). Unset reads as the default: deny every field, no readers.
pub async fn settings(pool: &Pool) -> Result<CloneSettings, CloneRefusal> {
    let sql = format!(
        "SELECT DISTINCT ON (\"setting\") \"setting\", \"value\" FROM \"$version_settings\" \
         WHERE \"setting\" IN ({}, {}, {}) ORDER BY \"setting\", \"seq\" DESC",
        sql_lit(MASK_SETTING),
        sql_lit(READERS_SETTING),
        sql_lit(SINKS_SETTING)
    );
    let rows = match pool.rows(&sql).await {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => vec![],
        Err(e) => return Err(e.into()),
    };
    let mut out = CloneSettings::default();
    for r in rows {
        let name = r.first().cloned().flatten().unwrap_or_default();
        let value = r.get(1).cloned().flatten().unwrap_or_default();
        if name == MASK_SETTING {
            out.mask = Mask::parse(&value)?;
        } else if name == READERS_SETTING {
            out.readers = parse_readers(&value)?;
        } else if name == SINKS_SETTING {
            out.sinks = crate::sinks::SinkScope::parse(&value).map_err(CloneRefusal::Setting)?;
        }
    }
    Ok(out)
}

/// **Set a clone setting**, with a reason. Stored beside the version
/// settings, with who, when and why. Answers the settings after.
pub async fn set_setting(
    pool: &Pool,
    setting: &str,
    value: &str,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<CloneSettings, CloneRefusal> {
    let reason = reason_of(reason)?;
    let name = match setting.trim() {
        MASK_SETTING | "mask" => MASK_SETTING,
        READERS_SETTING | "readers" => READERS_SETTING,
        SINKS_SETTING | "sinks" => SINKS_SETTING,
        other => {
            return Err(CloneRefusal::Setting(format!(
                "`{other}` is not a clone setting. There are {MASK_SETTING}, \
                 {READERS_SETTING} and {SINKS_SETTING}"
            )))
        }
    };
    let stored = if name == MASK_SETTING {
        Mask::parse(value)?.value()
    } else if name == SINKS_SETTING {
        crate::sinks::SinkScope::parse(value)
            .map_err(CloneRefusal::Setting)?
            .word()
            .to_string()
    } else {
        let r = parse_readers(value)?;
        if r.is_empty() {
            "none".to_string()
        } else {
            r.into_iter().collect::<Vec<_>>().join(",")
        }
    };
    pool.rows(crate::versions::SETTINGS_DDL).await?;
    pool.rows(&format!(
        "INSERT INTO \"$version_settings\" (\"setting\", \"value\", \"actor\", \"reason\", \"at\") \
         VALUES ({}, {}, {}, {}, {at})",
        sql_lit(name),
        sql_lit(&stored),
        sql_lit(actor),
        sql_lit(&reason)
    ))
    .await?;
    settings(pool).await
}

// ── keys ─────────────────────────────────────────────────────────────────

/// **This deployment's clone key**, made on first use. The secret is sealed
/// under the deployment's own keys, in its own space, and never leaves it.
pub async fn own_key(
    pool: &Pool,
    keys: &dyn KeyProvider,
    home: &str,
    at: u64,
) -> Result<RecipientSecret, CloneRefusal> {
    pool.rows(KEY_DDL).await?;
    let home_id = SpaceId(home.to_string());
    let read = || async {
        pool.rows("SELECT \"public\", \"sealed\" FROM \"$clone_key\" WHERE \"id\" = 1")
            .await
    };
    let rows = read().await?;
    if rows.is_empty() {
        let fresh = RecipientSecret::generate();
        let lease = keys
            .release(&home_id, Purpose::Write, "clone-key")
            .map_err(|e| CloneRefusal::Custody(e.to_string()))?;
        let sealed = keys
            .seal_at(
                &lease,
                KEY_ENTITY,
                KEY_FIELD,
                Anchor::Row("1"),
                &fresh.to_bytes(),
            )
            .map_err(|e| CloneRefusal::Custody(e.to_string()))?;
        pool.rows(&format!(
            "INSERT INTO \"$clone_key\" (\"id\", \"public\", \"sealed\", \"at\") \
             VALUES (1, {}, {}, {at}) ON CONFLICT DO NOTHING",
            sql_lit(&fresh.public().hex()),
            ordo_crypto::sealed::bytea_lit(&sealed)
        ))
        .await?;
    }
    let rows = read().await?;
    let row = rows
        .first()
        .ok_or_else(|| CloneRefusal::Custody("the clone key was not stored".to_string()))?;
    let public = row.first().cloned().flatten().unwrap_or_default();
    let sealed = row
        .get(1)
        .cloned()
        .flatten()
        .and_then(|h| ordo_crypto::sealed::unhex(&h))
        .ok_or_else(|| CloneRefusal::Custody("the stored clone key is not readable".into()))?;
    let lease = keys
        .release(&home_id, Purpose::Read, "clone-key")
        .map_err(|e| CloneRefusal::Custody(e.to_string()))?;
    let bytes = keys
        .open_at(&lease, KEY_ENTITY, KEY_FIELD, Anchor::Row("1"), &sealed)
        .map_err(|e| CloneRefusal::Custody(e.to_string()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CloneRefusal::Custody("the stored clone key is not 32 bytes".into()))?;
    let secret = RecipientSecret::from_bytes(bytes);
    if secret.public().hex() != public {
        return Err(CloneRefusal::Custody(
            "the stored clone key does not match its public half".to_string(),
        ));
    }
    Ok(secret)
}

/// **Record the clone key of an environment that clones this one**, on the
/// deployment the clone leaves. The environment must be registered, and
/// not this deployment's own. A second call replaces the key.
pub async fn set_recipient(
    pool: &Pool,
    env: &str,
    key: &str,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<RecipientPublic, CloneRefusal> {
    let reason = reason_of(reason)?;
    let envs = Environments::on(pool);
    let target = envs.require(env).await?;
    if target.own {
        return Err(CloneRefusal::OwnEnv { env: target.name });
    }
    let public = RecipientPublic::parse(key).map_err(|e| CloneRefusal::Input(e.to_string()))?;
    pool.rows(RECIPIENTS_DDL).await?;
    pool.rows(&format!(
        "INSERT INTO \"$clone_recipients\" (\"env\", \"public\", \"actor\", \"reason\", \"at\") \
         VALUES ({e}, {k}, {a}, {r}, {at}) ON CONFLICT (\"env\") DO UPDATE SET \
         \"public\" = {k}, \"actor\" = {a}, \"reason\" = {r}, \"at\" = {at}",
        e = sql_lit(&target.name),
        k = sql_lit(&public.hex()),
        a = sql_lit(actor),
        r = sql_lit(&reason),
    ))
    .await?;
    Ok(public)
}

/// The clone key recorded for `env`, if any.
pub async fn recipient(pool: &Pool, env: &str) -> Result<Option<RecipientPublic>, CloneRefusal> {
    let sql = format!(
        "SELECT \"public\" FROM \"$clone_recipients\" WHERE \"env\" = {}",
        sql_lit(env)
    );
    match pool.rows(&sql).await {
        Ok(rows) => Ok(rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|k| RecipientPublic::parse(&k).ok())),
        Err(e) if e.is_missing_table() => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// What the sealed bundle is bound to: its format, the tenant, the
/// version's source identity and the environment it is for.
fn aad(tenant: &str, source_id: &str, env: &str) -> Vec<u8> {
    format!("{FORMAT}\n{tenant}\n{source_id}\n{env}").into_bytes()
}

// ── masking ──────────────────────────────────────────────────────────────

/// What masking did, for the answer and the record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaskReport {
    /// Rows per entity, as carried.
    pub rows: BTreeMap<String, u64>,
    /// The fields masked, as `entity.field`.
    pub masked: Vec<String>,
    /// The fields carried as they are because `clone.mask` names them
    /// safe, or masking is off.
    pub kept: Vec<String>,
    /// Members carried, each without its sign-in link.
    pub members: u64,
    /// Files dropped.
    pub blobs_dropped: u64,
    /// Command-log groups dropped.
    pub log_groups_dropped: u64,
    /// Masking was off for this clone.
    pub mask_off: bool,
}

impl MaskReport {
    pub fn total_rows(&self) -> u64 {
        self.rows.values().sum()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "rows": self.rows, "masked": self.masked, "kept": self.kept,
            "members": self.members, "blobsDropped": self.blobs_dropped,
            "logGroupsDropped": self.log_groups_dropped, "maskOff": self.mask_off,
        })
    }

    fn from_json(v: &Value) -> MaskReport {
        let strings = |k: &str| {
            v[k].as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        MaskReport {
            rows: serde_json::from_value(v["rows"].clone()).unwrap_or_default(),
            masked: strings("masked"),
            kept: strings("kept"),
            members: v["members"].as_u64().unwrap_or(0),
            blobs_dropped: v["blobsDropped"].as_u64().unwrap_or(0),
            log_groups_dropped: v["logGroupsDropped"].as_u64().unwrap_or(0),
            mask_off: v["maskOff"].as_bool().unwrap_or(false),
        }
    }
}

/// A stable, data-free tag for one row's field: 8 hex digits from the
/// entity, the field and the row id. Unique per row, so a `unique` field
/// stays unique, and carries nothing the row held.
fn tag(entity: &str, field: &str, id: &str) -> String {
    let h = ordo_crypto::hmac_sha256(
        b"ordo.clone.mask",
        format!("{entity}\0{field}\0{id}").as_bytes(),
    );
    ordo_crypto::hex(&h[..4])
}

/// A JSON document with its shape kept: every string a placeholder, every
/// number 0, every boolean false. The page that reads it still finds an
/// array where it expects one.
fn mask_json(v: &Value) -> Value {
    match v {
        Value::String(_) => Value::String("masked".to_string()),
        Value::Number(_) => json!(0),
        Value::Bool(_) => Value::Bool(false),
        Value::Array(a) => Value::Array(a.iter().map(mask_json).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (format!("k{}", tag("json", k, "")), mask_json(v)))
                .collect(),
        ),
        Value::Null => Value::Null,
    }
}

/// One field's masked value. Refs keep their target: an id names a row and
/// holds no data. Everything else is replaced by its type.
fn masked_value(entity: &Entity, f: &Field, id: &str, value: &Value) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    let s = match &f.ty {
        FieldTy::Ref { .. } => return value.clone(),
        FieldTy::Scalar(s) => *s,
    };
    if matches!(s, ScalarTy::Blob) || f.optional {
        return Value::Null;
    }
    if let Some(first) = f.enum_values.as_ref().and_then(|v| v.first()) {
        return Value::String(first.clone());
    }
    let t = tag(&entity.name, &f.name, id);
    match s {
        ScalarTy::Text => {
            let mut text = format!("masked-{t}");
            if let Some(max) = f.text_max {
                let max = (max as usize).max(1);
                if text.len() > max {
                    text = t.chars().take(max).collect();
                }
            }
            Value::String(text)
        }
        ScalarTy::Uri => Value::String(format!("https://masked.invalid/{t}")),
        ScalarTy::Id => value.clone(),
        ScalarTy::Int | ScalarTy::Duration(_) | ScalarTy::Money(_) => {
            if f.unique {
                json!(i64::from_str_radix(&t, 16).unwrap_or(0))
            } else {
                json!(0)
            }
        }
        ScalarTy::Decimal(_) => Value::String("0".to_string()),
        ScalarTy::Bool => Value::Bool(false),
        ScalarTy::Date => Value::String(MASKED_DATE.to_string()),
        ScalarTy::Datetime => Value::String(MASKED_DATETIME.to_string()),
        ScalarTy::Json => mask_json(value),
        ScalarTy::Blob => Value::Null,
    }
}

/// **Mask an exported bundle**, inside production, before it leaves.
///
/// - Every field of the app's entities not named safe is masked
///   ([`masked_value`]). With masking off, they are carried.
/// - `members`: ids, roles, kinds and status are kept. The name is masked
///   and the sign-in link dropped, with masking off too (Q12).
/// - Blob values are dropped, and so are the files.
/// - The command log is emptied: its images hold what the rows held.
/// - The group names in the access profile are masked.
///
/// The manifest's digests are written again over what is carried.
pub fn mask_bundle(
    bundle: &BTreeMap<String, Vec<u8>>,
    schema: &Schema,
    mask: &Mask,
) -> Result<(BTreeMap<String, Vec<u8>>, MaskReport), CloneRefusal> {
    let bad = |why: String| CloneRefusal::Data(format!("the export cannot be masked: {why}"));
    let manifest_bytes = bundle
        .get("manifest.json")
        .ok_or_else(|| bad("it has no manifest".to_string()))?;
    let mut manifest: Value =
        serde_json::from_slice(manifest_bytes).map_err(|e| bad(e.to_string()))?;
    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut report = MaskReport {
        mask_off: *mask == Mask::Off,
        ..MaskReport::default()
    };
    let mut member_names: BTreeMap<String, String> = BTreeMap::new();
    let mut masked: BTreeSet<String> = BTreeSet::new();
    let mut kept: BTreeSet<String> = BTreeSet::new();

    let entries = manifest["entities"].as_array().cloned().unwrap_or_default();
    let mut data_files = BTreeSet::new();
    for entry in &entries {
        let name = entry["entity"].as_str().unwrap_or_default().to_string();
        let file = entry["file"].as_str().unwrap_or_default().to_string();
        data_files.insert(file.clone());
        let entity = schema
            .entities
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| {
                bad(format!(
                    "it carries `{name}`, which the version does not have"
                ))
            })?;
        let body = bundle
            .get(&file)
            .ok_or_else(|| bad(format!("the manifest names `{file}` and it is missing")))?;
        let body = std::str::from_utf8(body).map_err(|e| bad(e.to_string()))?;
        let mut lines = String::new();
        let mut n = 0u64;
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            let mut row: serde_json::Map<String, Value> =
                serde_json::from_str(line).map_err(|e| bad(e.to_string()))?;
            let id = row
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            n += 1;
            for f in entity.fields.iter().filter(|f| !f.implicit) {
                let Some(value) = row.get(&f.name).cloned() else {
                    continue;
                };
                let key = format!("{}.{}", entity.name, f.name);
                let new = if entity.platform && entity.name == "members" {
                    match f.name.as_str() {
                        "principal" => Value::String(String::new()),
                        "displayName" => {
                            let kind = row.get("kind").and_then(Value::as_str).unwrap_or("");
                            let label = if kind == "group" { "Group" } else { "Member" };
                            let shown = format!("{label} {}", tag("members", "name", &id));
                            member_names.insert(id.clone(), shown.clone());
                            Value::String(shown)
                        }
                        "joined" => Value::String(MASKED_DATE.to_string()),
                        _ => value,
                    }
                } else if entity.platform {
                    value
                } else if ordo_schema::acl_link_root(schema, &entity.name).is_some() {
                    // An acl access row is who may do what: a note, a
                    // member and a level, none of it anyone's words. Masked,
                    // its level became the first one, and the clone's access
                    // changed (found 2026-11-24).
                    value
                } else if matches!(f.ty, FieldTy::Scalar(ScalarTy::Blob)) {
                    if !value.is_null() {
                        masked.insert(key);
                    }
                    Value::Null
                } else if matches!(f.ty, FieldTy::Ref { .. }) {
                    value
                } else {
                    let safe = match mask {
                        Mask::Off => true,
                        Mask::Deny { safe } => safe.contains(&key),
                    };
                    if safe {
                        kept.insert(key);
                        value
                    } else {
                        masked.insert(key);
                        masked_value(entity, f, &id, &value)
                    }
                };
                row.insert(f.name.clone(), new);
            }
            lines.push_str(&serde_json::to_string(&Value::Object(row)).expect("JSON"));
            lines.push('\n');
        }
        if entity.platform && entity.name == "members" {
            report.members = n;
        }
        report.rows.insert(name, n);
        out.insert(file, lines.into_bytes());
    }

    // The access profile: its groups' names are the members' names.
    let profile_file = manifest["profile"]["file"]
        .as_str()
        .unwrap_or(ordo_exec::export::PROFILE_MEMBER)
        .to_string();
    if let Some(bytes) = bundle.get(&profile_file) {
        let mut profile: Value = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        if let Some(groups) = profile["groups"].as_array_mut() {
            for g in groups {
                let id = g["id"].as_str().unwrap_or_default().to_string();
                g["name"] = json!(member_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("Group {}", tag("members", "name", &id))));
            }
        }
        out.insert(
            profile_file.clone(),
            (serde_json::to_string_pretty(&profile).expect("JSON") + "\n").into_bytes(),
        );
    }

    // The command log, emptied.
    report.log_groups_dropped = manifest["commandlog"]["groups"].as_u64().unwrap_or(0);
    out.insert("commandlog.jsonl".to_string(), Vec::new());
    manifest["commandlog"]["groups"] = json!(0);
    manifest["commandlog"]["note"] = json!(
        "emptied for a staging clone: its images hold the values the rows held, before masking"
    );

    // Files, dropped.
    report.blobs_dropped = manifest["blobs"]["objects"].as_u64().unwrap_or(0);
    manifest["blobs"]["objects"] = json!(0);
    manifest["blobs"]["files"] = json!([]);

    // Everything else the export wrote that is not data, a file or the log
    // is the deployment's own description of the version: carried as is.
    for (name, bytes) in bundle {
        if name == "manifest.json"
            || out.contains_key(name)
            || data_files.contains(name)
            || name.starts_with("blobs/")
        {
            continue;
        }
        out.insert(name.clone(), bytes.clone());
    }

    let carried: Vec<String> = out.keys().cloned().collect();
    manifest["members"] = json!(carried);
    manifest["digests"] = json!(out
        .iter()
        .map(|(name, bytes)| json!({
            "member": name,
            "bytes": bytes.len(),
            "sha256": ordo_exec::export::bundle_digest(bytes),
        }))
        .collect::<Vec<_>>());
    manifest["clone"] = json!({
        "format": FORMAT,
        "masked": masked.iter().collect::<Vec<_>>(),
        "kept": kept.iter().collect::<Vec<_>>(),
    });
    out.insert(
        "manifest.json".to_string(),
        (serde_json::to_string_pretty(&manifest).expect("JSON") + "\n").into_bytes(),
    );
    report.masked = masked.into_iter().collect();
    report.kept = kept.into_iter().collect();
    Ok((out, report))
}

fn pack(members: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let m: serde_json::Map<String, Value> = members
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(b64.encode(v))))
        .collect();
    serde_json::to_vec(&json!({ "format": FORMAT, "members": m })).expect("JSON")
}

fn unpack(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, CloneRefusal> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|e| CloneRefusal::Bundle(format!("it is not a clone: {e}")))?;
    if v["format"].as_str() != Some(FORMAT) {
        return Err(CloneRefusal::Bundle(format!("it is not `{FORMAT}`")));
    }
    let mut out = BTreeMap::new();
    for (k, val) in v["members"].as_object().into_iter().flatten() {
        let text = val
            .as_str()
            .ok_or_else(|| CloneRefusal::Bundle(format!("`{k}` is not base64")))?;
        let bytes = b64
            .decode(text)
            .map_err(|_| CloneRefusal::Bundle(format!("`{k}` is not base64")))?;
        out.insert(k.clone(), bytes);
    }
    Ok(out)
}

// ── production: the clone out ────────────────────────────────────────────

/// What production answers for a clone.
#[derive(Debug, Clone)]
pub struct CloneOut {
    /// This deployment's environment, as it names itself.
    pub env: String,
    /// The tenant's id. Never its name: a self-serve tenant is named after
    /// its person.
    pub tenant: String,
    pub version: String,
    pub source_id: String,
    /// The masked bundle, sealed to the environment's clone key.
    pub sealed: Vec<u8>,
    pub report: MaskReport,
    /// The foreign tables the version reads, which are not cloned.
    pub not_cloned: Vec<String>,
    /// Who production read the request as.
    pub read_by: String,
    /// The tenant has a database of its own on production. Staging makes
    /// the clone one of its own; nothing of production's is named.
    pub own_database: bool,
    /// The tenant keeps its files in a bucket of its own on production.
    pub own_bucket: bool,
}

impl CloneOut {
    pub fn to_json(&self) -> Value {
        json!({
            "env": self.env, "tenant": self.tenant,
            "version": self.version, "sourceId": self.source_id,
            "sealed": base64::engine::general_purpose::STANDARD.encode(&self.sealed),
            "report": self.report.to_json(), "notCloned": self.not_cloned,
            "readBy": self.read_by, "ownDatabase": self.own_database,
            "ownBucket": self.own_bucket,
        })
    }

    pub fn from_json(v: &Value) -> Result<CloneOut, String> {
        let text = |k: &str| {
            v[k].as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("no `{k}`"))
        };
        Ok(CloneOut {
            env: text("env")?,
            tenant: text("tenant")?,
            version: text("version")?,
            source_id: text("sourceId")?,
            sealed: base64::engine::general_purpose::STANDARD
                .decode(text("sealed")?)
                .map_err(|_| "`sealed` is not base64".to_string())?,
            report: MaskReport::from_json(&v["report"]),
            not_cloned: v["notCloned"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            read_by: text("readBy")?,
            own_database: v["ownDatabase"].as_bool().unwrap_or(false),
            own_bucket: v["ownBucket"].as_bool().unwrap_or(false),
        })
    }
}

/// The tenant `wanted` names, by id or by name, that serves.
async fn find_tenant(pool: &Pool, wanted: &str) -> Result<crate::tenants::Tenant, CloneRefusal> {
    let wanted = wanted.trim();
    let serving = |t: &crate::tenants::Tenant| {
        matches!(
            t.state,
            crate::tenants::State::Active | crate::tenants::State::Paused
        )
    };
    if let Some(t) = crate::tenants::lookup(pool, wanted).await? {
        if serving(&t) {
            return Ok(t);
        }
    }
    let by_name: Vec<_> = crate::tenants::all(pool)
        .await?
        .into_iter()
        .filter(|t| t.name == wanted && serving(t))
        .collect();
    match by_name.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(CloneRefusal::NoTenant {
            tenant: wanted.to_string(),
        }),
        many => Err(CloneRefusal::Input(format!(
            "{} tenants are named `{wanted}`. Name one by its id: {}",
            many.len(),
            many.iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn version_refusal(e: crate::versions::VersionRefusal) -> CloneRefusal {
    CloneRefusal::Data(format!("the version registry: {e}"))
}

/// **Production: one tenant, masked here and sealed to `to_env`'s clone
/// key.** Nothing unmasked leaves this function.
pub async fn clone_out(
    pool: &Pool,
    pools: &dyn PoolResolver,
    keys: &dyn KeyProvider,
    tenant: &str,
    to_env: &str,
    caller: &str,
) -> Result<CloneOut, CloneRefusal> {
    let envs = Environments::on(pool);
    let own = envs.own().await?;
    let to = envs.require(to_env).await?;
    if to.own {
        return Err(CloneRefusal::OwnEnv { env: to.name });
    }
    let Some(recipient_key) = recipient(pool, &to.name).await? else {
        return Err(CloneRefusal::NoKey { env: to.name });
    };
    let t = find_tenant(pool, tenant).await?;
    let versions = crate::versions::Versions::on(pool);
    let version = versions
        .pin_of(&t.id)
        .await
        .map_err(version_refusal)?
        .and_then(|p| p.version)
        .ok_or_else(|| CloneRefusal::NotPinned {
            tenant: t.id.clone(),
        })?;
    let artifact = versions
        .get(&version)
        .await
        .map_err(version_refusal)?
        .ok_or_else(|| CloneRefusal::Data(format!("{version} is pinned and not stored")))?;
    let source_id = versions
        .identity(&version)
        .await
        .map_err(version_refusal)?
        .map(|(_, id)| id)
        .unwrap_or_default();
    let fixture = crate::Fixture::from_source(&artifact.source, "").map_err(|e| {
        CloneRefusal::Data(format!("{version} does not compile under this binary: {e}"))
    })?;
    let sid = SpaceId(t.id.clone());
    let space_pool = pools
        .resolve(&sid)
        .await
        .map_err(|e| CloneRefusal::Data(format!("the tenant's database: {e}")))?;
    let mut bundle = ordo_exec::export::MemoryBundle::new();
    ordo_exec::export::export_space(
        &space_pool,
        &ordo_exec::export::ExportRequest {
            space: &sid,
            schema: &fixture.schema,
            artifact: &artifact,
            keys,
            blobs: None,
            sealed: ordo_exec::export::SealedMode::Open,
            correlation: &format!("clone_{}", t.id),
        },
        &mut bundle,
    )
    .await
    .map_err(|e| CloneRefusal::Data(format!("the export refused: {e}")))?;
    let members: BTreeMap<String, Vec<u8>> = bundle
        .names()
        .into_iter()
        .filter_map(|n| bundle.get_bytes(n).map(|b| (n.to_string(), b.to_vec())))
        .collect();
    let mask = settings(pool).await?.mask;
    let (masked, report) = mask_bundle(&members, &fixture.schema, &mask)?;
    let sealed = recipient_key.seal(&pack(&masked), &aad(&t.id, &source_id, &to.name));
    // Whether the tenant has storage of its own: a yes or no, never where
    // or with what credential.
    let own_database = !t.target.is_empty()
        || ordo_exec::spacedb::lookup(pool, &sid)
            .await
            .or_else(|e| {
                if e.is_missing_table() {
                    Ok(None)
                } else {
                    Err(e)
                }
            })?
            .is_some();
    let own_bucket = ordo_exec::bucket::lookup(pool, &sid)
        .await
        .or_else(|e| {
            if e.is_missing_table() {
                Ok(None)
            } else {
                Err(e)
            }
        })?
        .is_some();
    Ok(CloneOut {
        env: own.name,
        tenant: t.id,
        version,
        source_id,
        sealed,
        report,
        not_cloned: fixture
            .schema
            .externals
            .iter()
            .map(|e| e.name.clone())
            .collect(),
        read_by: caller.to_string(),
        own_database,
        own_bucket,
    })
}

// ── staging: the clone in ────────────────────────────────────────────────

/// What a clone into this deployment asks for.
pub struct CloneInRequest<'a> {
    /// The environment the tenant is cloned from. `production` by default.
    pub from: &'a str,
    /// The tenant there, by id or name.
    pub tenant: &'a str,
    /// The person's own token for that environment, from the same sign-in.
    /// Sent there once, never stored.
    pub token: &'a str,
    /// The new tenant's name here. [`default_name`] when left out.
    pub name: Option<&'a str>,
    pub reason: &'a str,
}

/// One clone this deployment holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneRecord {
    pub space: String,
    pub name: String,
    pub from_env: String,
    pub from_tenant: String,
    pub version: String,
    pub source_id: String,
    /// The cloned member a staging admin acts as.
    pub opens_as: String,
    pub rows: u64,
    pub actor: String,
    pub reason: String,
    pub at: u64,
}

impl CloneRecord {
    pub fn to_json(&self) -> Value {
        json!({
            "space": self.space, "name": self.name, "fromEnv": self.from_env,
            "fromTenant": self.from_tenant,
            "version": self.version, "sourceId": self.source_id,
            "opensAs": self.opens_as, "rows": self.rows, "actor": self.actor,
            "reason": self.reason, "at": self.at,
        })
    }
}

/// What a clone into this deployment did.
#[derive(Debug, Clone)]
pub struct Cloned {
    pub record: CloneRecord,
    pub report: MaskReport,
    pub not_cloned: Vec<String>,
    /// The version that came down with the clone, when staging did not
    /// hold it. Stored, not activated, recorded in `$promotions` as a
    /// clone.
    pub arrived: Option<String>,
    /// Where the clone's rows are in staging.
    pub database: Storage,
    /// Production keeps the tenant's files in a bucket of its own. Files are
    /// never cloned; the clone's new files go to staging's file store.
    pub own_bucket: bool,
}

/// Where a clone's rows are in staging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Storage {
    /// Production's tenant shares its deployment's database, and so does
    /// the clone.
    Shared,
    /// A database of its own on staging's tenant cluster, by its label.
    Own(String),
    /// Production's tenant has a database of its own, and this server was
    /// assembled with no tenant databases to give: the clone shares
    /// staging's.
    NotGiven,
}

impl Storage {
    pub fn to_json(&self) -> Value {
        match self {
            Storage::Shared => json!({ "database": "shared" }),
            Storage::Own(label) => json!({ "database": "own", "label": label }),
            Storage::NotGiven => json!({
                "database": "shared",
                "note": "production's tenant has a database of its own, and this server gives \
                         no tenant a database of its own",
            }),
        }
    }
}

/// The stored version here whose source identity is `source_id`.
async fn version_here(pool: &Pool, source_id: &str) -> Result<Option<String>, CloneRefusal> {
    let versions = crate::versions::Versions::on(pool);
    versions.ensure().await.map_err(version_refusal)?;
    for (name, _, _) in versions.stored().await.map_err(version_refusal)? {
        let id = versions
            .identity(&name)
            .await
            .map_err(version_refusal)?
            .map(|(_, id)| id)
            .unwrap_or_default();
        if !source_id.is_empty() && id == source_id {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// **A clone's name when the caller gives none**: `clone-` and the last 8
/// characters of its new id. Never production's tenant name: a self-serve
/// tenant is named after its person, and that name is personal data.
pub fn default_name(id: &str) -> String {
    let tail: String = id
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("clone-{tail}")
}

fn check_tenant_name(name: &str) -> Result<String, CloneRefusal> {
    let name = name.trim();
    let n = name.chars().count();
    if n == 0 || n > 128 || name.chars().any(char::is_control) {
        return Err(CloneRefusal::Input(format!(
            "`{name}` is not a tenant name: 1 to 128 characters, none of them control \
             characters. `--name <name>`"
        )));
    }
    Ok(name.to_string())
}

/// **Clone a tenant from another deployment into a new tenant here.**
///
/// Reads the clone from `req.from`'s door as `actor`, with `req.token`.
/// The other deployment masks and seals it; this one opens it, makes a new
/// tenant pinned to the clone's version, writes its members with no sign-in
/// link, and imports the rows under its own keys. Recorded in `$clones`.
#[allow(clippy::too_many_arguments)]
pub async fn clone_in(
    pool: &Pool,
    pools: &dyn PoolResolver,
    keys: &Arc<dyn KeyProvider>,
    home: &str,
    tenants: Option<&crate::tenant_ops::TenantOps>,
    req: CloneInRequest<'_>,
    actor: &str,
    at: u64,
) -> Result<Cloned, CloneRefusal> {
    let reason = reason_of(req.reason)?;
    let envs = Environments::on(pool);
    let own = envs.own().await?;
    let from_env = if req.from.trim().is_empty() {
        crate::environments::DEFAULT_OWN
    } else {
        req.from.trim()
    };
    let from = envs.require(from_env).await?;
    if from.own {
        return Err(CloneRefusal::OwnEnv { env: from.name });
    }
    let Some(door) = from.door.clone() else {
        return Err(CloneRefusal::NoDoor { env: from.name });
    };
    let name = match req.name {
        Some(n) if !n.trim().is_empty() => Some(check_tenant_name(n)?),
        _ => None,
    };
    let source = |error| CloneRefusal::Source {
        env: from.name.clone(),
        error,
    };
    let client =
        StepDoor::parse(&door.address, crate::step_door::plain_http_allowed()).map_err(source)?;
    let row = client
        .clone_out(&door.space, req.token, req.tenant, &own.name)
        .await
        .map_err(source)?;
    let out = CloneOut::from_json(&row).map_err(|why| {
        source(StepDoorError::Answer {
            address: door.address.clone(),
            why: format!("a clone that cannot be read: {why}"),
        })
    })?;
    if out.env != from.name {
        return Err(CloneRefusal::WrongSource {
            given: out.env,
            expected: from.name,
        });
    }
    if out.read_by != actor {
        return Err(CloneRefusal::NotSamePerson {
            env: from.name,
            there: out.read_by,
            here: actor.to_string(),
        });
    }
    // A version staging does not hold comes down with the clone, through
    // the promotion gate, read from production as the same person, and is
    // recorded as a clone (`promotion::arrive_with_clone`).
    let mut arrived = None;
    let local = match version_here(pool, &out.source_id).await? {
        Some(local) => local,
        None => {
            let p = crate::promotion::arrive_with_clone(
                pool,
                pools,
                &from,
                &out.version,
                &crate::promotion::SourceCredential::Person {
                    token: req.token.to_string(),
                },
                &format!("with the clone of {}: {reason}", out.tenant),
                actor,
                at,
            )
            .await
            .map_err(|e| CloneRefusal::Version {
                version: out.version.clone(),
                from: from.name.clone(),
                refusal: Box::new(e),
            })?;
            let Some(local) = version_here(pool, &out.source_id).await? else {
                return Err(CloneRefusal::Data(format!(
                    "{} arrived from `{}` and is not stored here under source {}",
                    out.version, from.name, out.source_id
                )));
            };
            arrived = Some(p.report.version.clone());
            local
        }
    };
    let secret = own_key(pool, keys.as_ref(), home, at).await?;
    let plain = secret
        .open(&out.sealed, &aad(&out.tenant, &out.source_id, &own.name))
        .map_err(|e: RecipientError| CloneRefusal::Bundle(e.to_string()))?;
    let mut members = unpack(&plain)?;

    let versions = crate::versions::Versions::on(pool);
    let artifact = versions
        .get(&local)
        .await
        .map_err(version_refusal)?
        .ok_or_else(|| CloneRefusal::Data(format!("{local} is not stored")))?;
    // The version's name here: the identity is the same, checked above.
    let mut manifest: Value = members
        .get("manifest.json")
        .and_then(|b| serde_json::from_slice(b).ok())
        .ok_or_else(|| CloneRefusal::Bundle("it has no manifest".to_string()))?;
    manifest["artifact"]["version"] = json!(local);
    members.insert(
        "manifest.json".to_string(),
        serde_json::to_vec_pretty(&manifest).expect("JSON"),
    );

    let id = crate::tenants::mint_space_id();
    let tenant_name = name.unwrap_or_else(|| default_name(&id));
    crate::tenants::ensure_table(pool).await?;
    crate::tenants::reserve(pool, &id, &tenant_name, actor, at, None).await?;
    // **Its own database, when production's tenant has one** (§3.4): made
    // on staging's own tenant cluster with staging's credential, as a new
    // tenant here gets one. Production's database is never named.
    let database = match (out.own_database, tenants) {
        (false, _) => Ok(Storage::Shared),
        (true, Some(t)) if t.gives_databases() => t
            .clone_database(&id)
            .await
            .map(Storage::Own)
            .map_err(|why| {
                CloneRefusal::Data(format!("the clone's own database could not be made: {why}"))
            }),
        (true, _) => Ok(Storage::NotGiven),
    };
    let landed = match database {
        Ok(database) => land(
            pool,
            pools,
            keys,
            &id,
            &local,
            &artifact,
            &members,
            matches!(database, Storage::Own(_)),
            actor,
            at,
        )
        .await
        .map(|r| (r, database)),
        Err(e) => Err(e),
    };
    match landed {
        Ok(((opens_as, rows), database)) => {
            let record = CloneRecord {
                space: id,
                name: tenant_name,
                from_env: from.name,
                from_tenant: out.tenant,
                version: local,
                source_id: out.source_id,
                opens_as,
                rows,
                actor: actor.to_string(),
                reason,
                at,
            };
            ensure_clones(pool).await?;
            pool.rows(&format!(
                "INSERT INTO \"$clones\" (\"space\", \"name\", \"from_env\", \"from_tenant\", \
                 \"version\", \"source_id\", \"opens_as\", \"rows\", \"actor\", \
                 \"reason\", \"at\") VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
                sql_lit(&record.space),
                sql_lit(&record.name),
                sql_lit(&record.from_env),
                sql_lit(&record.from_tenant),
                sql_lit(&record.version),
                sql_lit(&record.source_id),
                sql_lit(&record.opens_as),
                record.rows,
                sql_lit(&record.actor),
                sql_lit(&record.reason),
                record.at
            ))
            .await?;
            Ok(Cloned {
                record,
                report: out.report,
                not_cloned: out.not_cloned,
                arrived,
                database,
                own_bucket: out.own_bucket,
            })
        }
        Err(e) => {
            let _ = crate::tenants::fail(pool, &id, &format!("clone: {e}"), at).await;
            Err(e)
        }
    }
}

/// The new tenant's custody, members, pin and rows. Answers the member a
/// staging admin opens it as, and the rows imported.
#[allow(clippy::too_many_arguments)]
async fn land(
    pool: &Pool,
    pools: &dyn PoolResolver,
    keys: &Arc<dyn KeyProvider>,
    id: &str,
    version: &str,
    artifact: &crate::versions::Artifact,
    members: &BTreeMap<String, Vec<u8>>,
    custody_made: bool,
    actor: &str,
    at: u64,
) -> Result<(String, u64), CloneRefusal> {
    let sid = SpaceId(id.to_string());
    // An own database's provisioning minted custody with it.
    if !custody_made {
        keys.create_space(&sid)
            .map_err(|e| CloneRefusal::Custody(e.to_string()))?;
    }
    let space_pool = pools
        .resolve(&sid)
        .await
        .map_err(|e| CloneRefusal::Data(format!("the new tenant's database: {e}")))?;

    // **The clone's database holds its version's schema** before a row is
    // written. A database of its own was made empty, or on whatever this
    // deployment serves, so it is brought to the clone's version as a
    // tenant's database is. The deployment's shared database is never
    // changed here (the promotion gate is what applies a version to it),
    // so one that lacks the version's tables is refused, naming the fix.
    let mut program = crate::schema_apply::Program::compile(&artifact.source)
        .map_err(|e| CloneRefusal::Data(format!("{version} does not compile here: {e}")))?;
    program.version = version.to_string();
    if custody_made {
        crate::schema_apply::bring_forward_from(&space_pool, Some(pool), &program, actor, CLONE_IN)
            .await
            .map_err(|e| {
                CloneRefusal::Data(format!(
                    "the clone's own database could not be brought to {version}: {e}"
                ))
            })?;
    } else {
        match crate::schema_apply::schema_state(&space_pool, &program.schema)
            .await
            .map_err(CloneRefusal::Data)?
        {
            crate::schema_apply::SchemaState::Matches { .. } => {}
            _ => {
                return Err(CloneRefusal::Data(format!(
                    "this deployment's database does not hold the tables of {version}, the \
                     clone's version, and a clone never changes it. Apply {version} here \
                     first: `ordo promote {version} --reason \"...\"`, or `ordo evolve --to \
                     {version} --run`, then clone again"
                )))
            }
        }
    }

    // The members: kept ids and roles, masked names, no sign-in link.
    let body = members
        .get("data/members.jsonl")
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let mut people: Vec<(String, String)> = Vec::new();
    let mut owner: Option<String> = None;
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let m: Value = serde_json::from_str(line)
            .map_err(|e| CloneRefusal::Bundle(format!("a member does not parse: {e}")))?;
        let mid = m["id"].as_str().unwrap_or_default().to_string();
        if !ordo_api::well_formed_space(&mid) {
            return Err(CloneRefusal::Bundle(format!("`{mid}` is not a member id")));
        }
        let text = |k: &str, d: &str| m[k].as_str().unwrap_or(d).to_string();
        let kind = text("kind", "principal");
        let role = m["role"].as_str().map(sql_lit).unwrap_or("NULL".into());
        let status = text("status", "active");
        if owner.is_none()
            && kind == "principal"
            && status == "active"
            && m["role"].as_str() == Some("owner")
        {
            owner = Some(mid.clone());
        }
        space_pool
            .rows(&format!(
                "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \
                 \"displayName\", \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
                 VALUES ({}, {}, 1, {}, {}, {}, {}, {role}, {}, {})",
                sql_lit(id),
                sql_lit(&mid),
                sql_lit(&format!("{CLONE_PRINCIPAL_PREFIX}{mid}")),
                sql_lit(&text("displayName", "Member")),
                sql_lit(&kind),
                m["memberCount"].as_i64().unwrap_or(1),
                sql_lit(&status),
                sql_lit(MASKED_DATE),
            ))
            .await?;
        people.push((mid.clone(), mid));
    }
    let opens_as = owner.ok_or_else(|| {
        CloneRefusal::Bundle("the tenant has no active owner to open it as".to_string())
    })?;

    crate::versions::Versions::on(pool)
        .pin(id, version, actor, at)
        .await
        .map_err(|e| CloneRefusal::Data(format!("pinning the clone to {version}: {e}")))?;

    let compiled = crate::Fixture::from_source(&artifact.source, "")
        .map_err(|e| CloneRefusal::Data(format!("{version} does not compile here: {e}")))?;
    let mut fixture = compiled.in_space(id);
    fixture.keys = keys.clone();
    let fixture = Arc::new(fixture);
    let exec = ordo_exec::Executor {
        pool: space_pool,
        schema: fixture.schema.clone(),
        registry: fixture.registry.clone(),
        keys: keys.clone(),
        space: sid,
    };
    let mut source = ordo_exec::import::MemorySource::new();
    for (k, v) in members {
        source.put(k, v.clone());
    }
    // The import's name, recorded in `$importmap`. Its batch leases are
    // keyed by the space as well, so two clones never meet there.
    let import_name = format!("clone_{id}");
    let report = ordo_exec::import::import_space(
        &exec,
        &ordo_exec::import::ImportRequest {
            name: &import_name,
            fixture,
            artifact,
            // The founding owner the staging admin opens it as: access rows
            // are restored as the space's owner (`run_restore_on`).
            caller: &opens_as,
            batch: ordo_exec::import::BATCH,
            people: &people,
            blobs: None,
        },
        &source,
    )
    .await
    .map_err(|e| CloneRefusal::Data(format!("the import refused: {e}")))?;
    crate::tenants::activate(pool, id, at).await?;
    Ok((opens_as, report.rows() + people.len() as u64))
}

/// Every clone this deployment holds, newest first.
pub async fn clones(pool: &Pool) -> Result<Vec<CloneRecord>, CloneRefusal> {
    match pool.rows(FROM_NAME_MIGRATION).await {
        Ok(_) => {}
        Err(e) if e.is_missing_table() => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    }
    let sql = "SELECT \"space\", \"name\", \"from_env\", \"from_tenant\", \
               \"version\", \"source_id\", \"opens_as\", \"rows\"::text, \"actor\", \"reason\", \
               \"at\"::text FROM \"$clones\" ORDER BY \"at\" DESC, \"space\"";
    let rows = match pool.rows(sql).await {
        Ok(rows) => rows,
        Err(e) if e.is_missing_table() => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let g = |i: usize| r.get(i).cloned().flatten();
            Some(CloneRecord {
                space: g(0)?,
                name: g(1)?,
                from_env: g(2)?,
                from_tenant: g(3)?,
                version: g(4)?,
                source_id: g(5)?,
                opens_as: g(6)?,
                rows: g(7)?.parse().ok()?,
                actor: g(8)?,
                reason: g(9)?,
                at: g(10)?.parse().ok()?,
            })
        })
        .collect())
}

// ── who opens a clone ────────────────────────────────────────────────────

/// **Who opens a cloned tenant** (owner ruling 2026-11-23, Q12): any
/// staging deployment admin, or a person `clone.readers` names. The door
/// asks [`CloneAccess::opens_as`], which answers the cloned member the
/// person acts as: the clone's founding owner. A cloned member's own
/// sign-in reaches nothing: its link was never carried.
///
/// Held in memory, because the door's check is synchronous. Loaded at boot
/// and after every clone ([`CloneAccess::refresh`]).
#[derive(Debug, Default)]
pub struct CloneAccess {
    spaces: RwLock<BTreeMap<String, String>>,
    readers: RwLock<BTreeSet<String>>,
}

impl CloneAccess {
    pub fn new() -> Arc<CloneAccess> {
        Arc::new(CloneAccess::default())
    }

    /// Read the clones and `clone.readers` again.
    pub async fn refresh(&self, pool: &Pool) -> Result<(), CloneRefusal> {
        let spaces: BTreeMap<String, String> = clones(pool)
            .await?
            .into_iter()
            .map(|c| (c.space, c.opens_as))
            .collect();
        let readers = settings(pool).await?.readers;
        *self.spaces.write().unwrap_or_else(|p| p.into_inner()) = spaces;
        *self.readers.write().unwrap_or_else(|p| p.into_inner()) = readers;
        Ok(())
    }

    /// Whether `space` is a clone here.
    pub fn is_clone(&self, space: &str) -> bool {
        self.spaces
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(space)
    }

    /// Whether `person` is named in `clone.readers`: such a person may
    /// start a rehearsal on staging (Q12, `crate::rehearsals`).
    pub fn may_rehearse(&self, person: &str) -> bool {
        self.readers
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains(person)
    }

    /// The member `person` acts as in `space`: `Some` when `space` is a
    /// clone and the person is a deployment admin (`admin`) or named in
    /// `clone.readers`.
    pub fn opens_as(&self, space: &str, person: &str, admin: bool) -> Option<String> {
        let member = self
            .spaces
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(space)
            .cloned()?;
        let allowed = admin
            || self
                .readers
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .contains(person);
        allowed.then_some(member)
    }
}

// ── the door ─────────────────────────────────────────────────────────────

/// What the clone ops need: the deploy door's state, the deployment's
/// keys, and who opens a clone.
pub struct CloneOps {
    pub apply: Arc<crate::apply_op::ApplyOps>,
    pub keys: Arc<dyn KeyProvider>,
    pub access: Arc<CloneAccess>,
    /// What gives a clone a database of its own, when production's tenant
    /// has one. `None`: the clone shares this deployment's database.
    pub tenants: Option<Arc<crate::tenant_ops::TenantOps>>,
}

fn text<'a>(bound: &'a BoundParams, k: &str) -> Option<&'a str> {
    bound.0.get(k).and_then(|v| v.as_str())
}

fn param(name: &str, max: u32, required: bool) -> ParamSpec {
    ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    }
}

/// The clone ops' specs. Owner only, on the deployment's own space.
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
        spec(CLONE_KEY, OpKind::Read, vec![], &["key", "env"]),
        spec(
            SET_CLONE_KEY,
            OpKind::Write,
            vec![
                param("env", 128, true),
                param("key", 64, true),
                param("reason", 4096, true),
            ],
            &["env", "key"],
        ),
        spec(
            CLONE_OUT,
            OpKind::Write,
            vec![param("tenant", 128, true), param("env", 128, true)],
            &[
                "env",
                "tenant",
                "version",
                "sourceId",
                "sealed",
                "report",
                "notCloned",
                "readBy",
                "ownDatabase",
                "ownBucket",
            ],
        ),
        spec(
            CLONE_IN,
            OpKind::Write,
            vec![
                param("tenant", 128, true),
                param("from", 128, false),
                // The caller's own token for the environment cloned from,
                // from the same sign-in. Sent there once, never stored.
                param("fromToken", 8 * 1024, true),
                param("name", 128, false),
                param("reason", 4096, true),
            ],
            &[
                "clone",
                "report",
                "notCloned",
                "arrived",
                "storage",
                "ownBucket",
            ],
        ),
        spec(
            CLONES,
            OpKind::Read,
            vec![],
            &["clones", "mask", "readers", "sinks"],
        ),
        spec(
            SINKS,
            OpKind::Read,
            vec![
                param("tenant", 128, false),
                ParamSpec {
                    name: "limit".to_string(),
                    ty: ParamTy::Int,
                    required: false,
                    bounds: None,
                },
            ],
            &["entries"],
        ),
        spec(
            SET_CLONE_SETTING,
            OpKind::Write,
            vec![
                param("setting", 64, true),
                param("value", 4096, true),
                param("reason", 4096, true),
            ],
            &["setting", "mask", "readers", "sinks"],
        ),
    ]
}

impl CloneOps {
    /// Run one clone op. Bound to the deployment's own space.
    pub async fn execute(
        &self,
        name: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let ops = &self.apply;
        ops.home_only(space)?;
        let now = crate::runtime::now_secs();
        let row = match name {
            CLONE_KEY => {
                let key = own_key(&ops.pool, self.keys.as_ref(), &ops.home, now)
                    .await
                    .map_err(|e| e.to_exec_error("the clone key could not be read"))?;
                let env = Environments::on(&ops.pool)
                    .own()
                    .await
                    .map_err(|e| e.to_exec_error("the environment could not be read"))?;
                json!({ "key": key.public().hex(), "env": env.name })
            }
            SET_CLONE_KEY => {
                let env = text(bound, "env").unwrap_or_default();
                let reason = text(bound, "reason").unwrap_or_default();
                let key = set_recipient(
                    &ops.pool,
                    env,
                    text(bound, "key").unwrap_or_default(),
                    caller,
                    reason,
                    now,
                )
                .await
                .map_err(|e| e.to_exec_error("the clone key was not recorded"))?;
                ops.record(
                    caller,
                    SET_CLONE_KEY,
                    "clone-key",
                    format!(
                        "environment={} key={} by={caller} reason={}",
                        env.trim(),
                        key.hex(),
                        reason.trim()
                    ),
                );
                json!({ "env": env.trim(), "key": key.hex() })
            }
            CLONE_OUT => {
                let tenant = text(bound, "tenant").unwrap_or_default();
                let env = text(bound, "env").unwrap_or_default();
                let out = clone_out(
                    &ops.pool,
                    ops.pools.as_ref(),
                    self.keys.as_ref(),
                    tenant,
                    env,
                    caller,
                )
                .await
                .map_err(|e| e.to_exec_error("the tenant was not cloned"))?;
                ops.record(
                    caller,
                    CLONE_OUT,
                    "cloned-out",
                    format!(
                        "tenant={} version={} to={} rows={} masked={} mask={} by={caller}",
                        out.tenant,
                        out.version,
                        env.trim(),
                        out.report.total_rows(),
                        out.report.masked.len(),
                        if out.report.mask_off { "off" } else { "deny" },
                    ),
                );
                out.to_json()
            }
            CLONE_IN => {
                let reason = text(bound, "reason").unwrap_or_default();
                let done = clone_in(
                    &ops.pool,
                    ops.pools.as_ref(),
                    &self.keys,
                    &ops.home,
                    self.tenants.as_deref(),
                    CloneInRequest {
                        from: text(bound, "from").unwrap_or_default(),
                        tenant: text(bound, "tenant").unwrap_or_default(),
                        token: text(bound, "fromToken").unwrap_or_default(),
                        name: text(bound, "name"),
                        reason,
                    },
                    caller,
                    now,
                )
                .await
                .map_err(|e| e.to_exec_error("the tenant was not cloned"))?;
                // The door opens the clone to admins from now on.
                let _ = self.access.refresh(&ops.pool).await;
                let r = &done.record;
                ops.record(
                    caller,
                    CLONE_IN,
                    "cloned-in",
                    format!(
                        "space={} from={} tenant={} version={} arrived={} rows={} by={caller} \
                         reason={}",
                        r.space,
                        r.from_env,
                        r.from_tenant,
                        r.version,
                        done.arrived.is_some(),
                        r.rows,
                        reason.trim()
                    ),
                );
                json!({
                    "clone": r.to_json(), "report": done.report.to_json(),
                    "notCloned": done.not_cloned, "arrived": done.arrived,
                    "storage": done.database.to_json(), "ownBucket": done.own_bucket,
                })
            }
            CLONES => {
                let list = clones(&ops.pool)
                    .await
                    .map_err(|e| e.to_exec_error("the clones could not be read"))?;
                let s = settings(&ops.pool)
                    .await
                    .map_err(|e| e.to_exec_error("the clone settings could not be read"))?;
                json!({
                    "clones": list.iter().map(CloneRecord::to_json).collect::<Vec<_>>(),
                    "mask": s.mask.value(),
                    "readers": s.readers.iter().collect::<Vec<_>>(),
                    "sinks": s.sinks.word(),
                })
            }
            SINKS => {
                let limit = bound
                    .0
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(100)
                    .clamp(1, i64::from(crate::sinks::READ_MAX)) as u32;
                // A tenant by id, or by name.
                let space = match text(bound, "tenant").map(str::trim) {
                    None | Some("") => None,
                    Some(t) => Some(match find_tenant(&ops.pool, t).await {
                        Ok(found) => found.id,
                        Err(CloneRefusal::NoTenant { .. }) => t.to_string(),
                        Err(e) => return Err(e.to_exec_error("the sinks could not be read")),
                    }),
                };
                let list = crate::sinks::entries(&ops.pool, space.as_deref(), limit)
                    .await
                    .map_err(|e| {
                        CloneRefusal::Store(e).to_exec_error("the sinks could not be read")
                    })?;
                json!({
                    "entries": list.iter().map(crate::sinks::SinkEntry::to_json).collect::<Vec<_>>(),
                })
            }
            SET_CLONE_SETTING => {
                let setting = text(bound, "setting").unwrap_or_default();
                let value = text(bound, "value").unwrap_or_default();
                let reason = text(bound, "reason").unwrap_or_default();
                let s = set_setting(&ops.pool, setting, value, caller, reason, now)
                    .await
                    .map_err(|e| e.to_exec_error("the clone setting was not set"))?;
                let _ = self.access.refresh(&ops.pool).await;
                ops.record(
                    caller,
                    SET_CLONE_SETTING,
                    "clone-setting",
                    format!(
                        "setting={} value={} by={caller} reason={}",
                        setting.trim(),
                        value.trim(),
                        reason.trim()
                    ),
                );
                json!({
                    "setting": setting.trim(), "mask": s.mask.value(),
                    "readers": s.readers.iter().collect::<Vec<_>>(),
                    "sinks": s.sinks.word(),
                })
            }
            other => return Err(ExecError::internal(format!("`{other}` is not a clone op"))),
        };
        Ok(ExecResult {
            position: None,
            rows: vec![row],
            next_cursor: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_setting_reads_deny_off_or_safe_fields() {
        assert_eq!(Mask::parse("").unwrap(), Mask::default());
        assert_eq!(Mask::parse("deny").unwrap().value(), "deny");
        assert_eq!(Mask::parse("off").unwrap(), Mask::Off);
        let m = Mask::parse("notes.tags, notes.pinned").unwrap();
        assert_eq!(m.value(), "notes.pinned,notes.tags");
        let e = Mask::parse("notes").unwrap_err();
        assert!(e.to_string().contains("entity.field"), "{e}");
        assert!(parse_readers("p_ada, p_bob").unwrap().contains("p_bob"));
        assert!(parse_readers("p ada").is_err());
    }

    #[test]
    fn a_clone_opens_to_an_admin_or_a_named_reader_alone() {
        let a = CloneAccess::default();
        a.spaces
            .write()
            .unwrap()
            .insert("spc_c".into(), "mem_owner".into());
        a.readers.write().unwrap().insert("p_reader".into());
        assert_eq!(
            a.opens_as("spc_c", "p_admin", true).as_deref(),
            Some("mem_owner")
        );
        assert_eq!(
            a.opens_as("spc_c", "p_reader", false).as_deref(),
            Some("mem_owner")
        );
        assert_eq!(a.opens_as("spc_c", "p_other", false), None);
        assert_eq!(a.opens_as("spc_live", "p_admin", true), None);
    }

    #[test]
    fn a_masked_value_keeps_its_type_and_none_of_its_data() {
        let schema = ordo_schema::build(
            &ordo_syntax::parse_program(
                "entity people {\n  name: text\n  code: text(4) unique\n  age: int\n  \
                 born: date\n  seen: datetime\n  vip: bool\n  prefs: json\n  nick: text?\n}\n",
            )
            .unwrap(),
        )
        .unwrap();
        let e = schema.entities.iter().find(|e| e.name == "people").unwrap();
        let f = |n: &str| e.fields.iter().find(|f| f.name == n).unwrap();
        let v = |n: &str, val: Value| masked_value(e, f(n), "ppl_1", &val);
        let name = v("name", json!("Ada Lovelace"));
        assert!(name.as_str().unwrap().starts_with("masked-"));
        assert_ne!(
            v("name", json!("x")),
            masked_value(e, f("name"), "ppl_2", &json!("x"))
        );
        assert!(v("code", json!("ABCD")).as_str().unwrap().len() <= 4);
        assert_eq!(v("age", json!(36)), json!(0));
        assert_eq!(v("born", json!("1815-12-10")), json!(MASKED_DATE));
        assert_eq!(
            v("seen", json!("2026-01-01T00:00:00Z")),
            json!(MASKED_DATETIME)
        );
        assert_eq!(v("vip", json!(true)), json!(false));
        let prefs = v(
            "prefs",
            json!({"email": "ada@example.com", "tags": ["a", 2]}),
        );
        assert!(!prefs.to_string().contains("ada"), "{prefs}");
        assert_eq!(
            v("prefs", json!(["work", "home"])),
            json!(["masked", "masked"])
        );
        assert_eq!(v("nick", json!("Countess")), Value::Null);
    }
}
