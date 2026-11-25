//! **The signed rehearsal** (`plan-versions.md` §3.2, §4 row 12b).
//! Provisional, as the design is (ruled 2026-11-02, D2).
//!
//! # The path
//!
//! 1. **Staging runs the stored plan on clones** of production tenants
//!    ([`rehearse`]). Each named tenant must be a clone (`$clones`), pinned
//!    to the plan's from-version. The step runs on each clone's database,
//!    and the clone is pinned to the to-version.
//! 2. **When every clone runs green, staging signs an attestation**: the
//!    source identities of both versions, the plan digest and its options,
//!    the to-version's build identity, the Ordo release, the result, the
//!    clones, and what is not reproducible (Q14). Stored in `$rehearsals`.
//! 3. **Production accepts it** ([`accept`]): it reads the attestation from
//!    staging's door as the person accepting, with their own token for
//!    staging (the Q11 way, as a promotion reads its source). It checks the
//!    signature, the release (`promotion.release_mismatch`), the age
//!    (`promotion.attestation_max_age`), that it holds both versions with
//!    the same source identities, and re-renders the plan here: the digest
//!    must be the one staging signed. Then it stores the plan and records
//!    the acceptance in `$rehearsals_accepted`.
//! 4. **Production runs only a signed digest** ([`require_rehearsed`]):
//!    every stored step on a deployment with staging before it in its chain
//!    is refused unless an accepted attestation names that step and its
//!    digest. `promotion.skip_staging` on turns the rule off, as it takes
//!    staging out of the promotion path.
//! 5. **Break-glass** ([`break_glass`], §3.2 item 6): when staging does not
//!    answer, the deployment owner lets the stored plan of one step run
//!    without a rehearsal, with a reason, for one hour. The stream marks
//!    it (`break-glass`, and `break-glass-refused` for each refused try),
//!    and `ordo rehearsals list` shows it.
//!
//! # The key (owner ruling 2026-11-24, Q11: no key to set up)
//!
//! Staging's signing key is made on first use, an Ed25519 seed sealed
//! under the deployment's own keys in its own space ([`own_key`]), the way
//! the clone key is. Nobody sets it up. Production learns it from staging's
//! door, read as the person accepting, and records it the first time
//! (`$rehearsal_keys`). A later attestation under another key is refused
//! until the person names the new key (`--trust-key`).
//!
//! # Who may rehearse (owner ruling 2026-11-23, Q12)
//!
//! Any staging deployment admin, or a person staging allows
//! (`clone.readers`). The door lets a `clone.readers` person call
//! [`REHEARSE`] in the deployment's own space and nothing else there
//! (`IdentityAccess`, beside the clone rule).
//!
//! # Not reproducible (owner ruling 2026-11-23, Q14)
//!
//! A backfill function that declares secrets or hosts may give staging and
//! production different values. The plan's receipt marks it
//! (`reproducible: false`); the attestation lists each such backfill under
//! `notReproducible`, and production's acceptance says so.
//!
//! [`RehearsalRefusal`] is the one error; each variant says what to do.

use std::sync::Arc;

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_crypto::{Anchor, KeyProvider, Purpose, SpaceId};
use ordo_exec::route::PoolResolver;
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::{json, Value};

use crate::environments::{self as envs_mod, Environment, EnvironmentRefusal, Environments};
use crate::evolve::{self, EvolveRefusal, Plans, StepContext, StepRun, StoredPlan};
use crate::step_door::{StepDoor, StepDoorError};
use crate::versions::Versions;

/// Run a stored plan on staging's clones and sign the result.
pub const REHEARSE: &str = "std.deployment.rehearse";
/// The signed rehearsals this deployment holds, and its signing key.
pub const REHEARSALS: &str = "std.deployment.rehearsals";
/// Read staging's attestation for one step, check it, and accept it.
pub const ACCEPT: &str = "std.deployment.acceptRehearsal";

/// **Break glass** (§3.2 item 6): staging is down, so a named deployment
/// owner lets one stored step run without its rehearsal, for
/// [`BREAK_GLASS_SECS`], with a recorded reason.
pub const BREAK_GLASS: &str = "std.deployment.breakGlassRehearsal";

/// Every rehearsal op.
pub const OPS: [&str; 4] = [REHEARSE, REHEARSALS, ACCEPT, BREAK_GLASS];

/// How long a break-glass lets its step run: 1 hour. Fixed, and short: the
/// plan names no length, so the conservative one (owner question).
pub const BREAK_GLASS_SECS: u64 = 3600;

/// The attestation's format.
pub const FORMAT: &str = "ordo.rehearsal.v1";

/// `refuse` (the default) or `warn`: an attestation made under another
/// Ordo release.
pub const RELEASE_MISMATCH: &str = "promotion.release_mismatch";
/// How old an attestation may be when it is accepted.
pub const MAX_AGE: &str = "promotion.attestation_max_age";
/// 7 days (provisional, §7).
pub const DEFAULT_MAX_AGE_SECS: u64 = 7 * 24 * 3600;

const KEY_ENTITY: &str = "$rehearsal_key";
const KEY_FIELD: &str = "seed";

const KEY_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$rehearsal_key\" ( \
  \"id\" integer PRIMARY KEY, \
  \"public\" text NOT NULL, \
  \"sealed\" bytea NOT NULL, \
  \"at\" bigint NOT NULL \
)";

const SIGNED_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$rehearsals\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"from_version\" text NOT NULL, \
  \"to_version\" text NOT NULL, \
  \"digest\" text NOT NULL, \
  \"body\" text NOT NULL, \
  \"signature\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

const ACCEPTED_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$rehearsals_accepted\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"from_version\" text NOT NULL, \
  \"to_version\" text NOT NULL, \
  \"digest\" text NOT NULL, \
  \"env\" text NOT NULL, \
  \"key\" text NOT NULL, \
  \"body\" text NOT NULL, \
  \"signature\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

const BREAK_GLASS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$rehearsal_break_glass\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"from_version\" text NOT NULL, \
  \"to_version\" text NOT NULL, \
  \"digest\" text NOT NULL, \
  \"env\" text NOT NULL, \
  \"reason\" text NOT NULL, \
  \"staging_said\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"at\" bigint NOT NULL, \
  \"until\" bigint NOT NULL \
)";

const KEYS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$rehearsal_keys\" ( \
  \"env\" text PRIMARY KEY, \
  \"public\" text NOT NULL, \
  \"actor\" text NOT NULL, \
  \"at\" bigint NOT NULL \
)";

/// Why a rehearsal, or its acceptance, was refused, and what to do.
#[derive(Debug)]
pub enum RehearsalRefusal {
    /// Rehearsals run in staging. Say which environment this deployment is.
    NotStaging {
        own: String,
    },
    /// This deployment has no staging before it, so there is nothing to
    /// accept and nothing is required.
    NoStaging {
        own: String,
    },
    /// No tenant was named. Name the clones.
    NoTenants,
    /// The tenant is not a clone here. Clone it first.
    NotAClone {
        tenant: String,
    },
    /// The clone is pinned to another version than the step starts from.
    /// Clone the tenant again.
    WrongPin {
        clone: String,
        pinned: Option<String>,
        from: String,
    },
    /// The step failed on a clone. Nothing was signed.
    Failed {
        clone: String,
        refusal: Box<EvolveRefusal>,
    },
    /// A backfill stopped at its budget on a clone. Run the rehearsal
    /// again: it resumes from the cursor.
    Unfinished {
        clone: String,
    },
    /// The step before is not where staging answers. Record its door.
    NoDoor {
        env: String,
    },
    /// Reading from staging failed.
    Source {
        env: String,
        error: StepDoorError,
    },
    /// Staging's door answers as another environment.
    WrongSource {
        given: String,
        expected: String,
    },
    /// Staging read the attestation as another person.
    NotSamePerson {
        env: String,
        there: String,
        here: String,
    },
    /// Staging signed no rehearsal of this step.
    NotSigned {
        env: String,
        from: String,
        to: String,
    },
    /// The signature does not verify.
    BadSignature {
        env: String,
        why: String,
    },
    /// Staging signs with another key than the one recorded here.
    KeyChanged {
        env: String,
        recorded: String,
        given: String,
    },
    /// Signed under another Ordo release, and `promotion.release_mismatch`
    /// is `refuse`.
    Release {
        signed: String,
        here: String,
    },
    /// Older than `promotion.attestation_max_age`.
    TooOld {
        age: u64,
        max: u64,
    },
    /// A version this deployment holds has another source identity than
    /// the one staging rehearsed, or is not held here.
    Source2 {
        version: String,
        why: String,
    },
    /// The plan renders here to another digest than the one staging signed.
    Digest {
        from: String,
        to: String,
        signed: String,
        here: String,
    },
    /// Break-glass while staging answers: staging is not down.
    StagingAnswers {
        env: String,
        address: String,
        status: u16,
    },
    /// Break-glass names a step this deployment stored no plan for.
    NoPlan {
        from: String,
        to: String,
    },
    /// A stored plan or version could not be read, rendered or stored.
    Evolve(Box<EvolveRefusal>),
    /// The signing key could not be made or opened.
    Custody(String),
    Environment(EnvironmentRefusal),
    Input(String),
    Store(PoolError),
}

impl From<PoolError> for RehearsalRefusal {
    fn from(e: PoolError) -> Self {
        RehearsalRefusal::Store(e)
    }
}

impl From<EnvironmentRefusal> for RehearsalRefusal {
    fn from(e: EnvironmentRefusal) -> Self {
        RehearsalRefusal::Environment(e)
    }
}

impl From<EvolveRefusal> for RehearsalRefusal {
    fn from(e: EvolveRefusal) -> Self {
        RehearsalRefusal::Evolve(Box::new(e))
    }
}

impl From<crate::versions::VersionRefusal> for RehearsalRefusal {
    fn from(e: crate::versions::VersionRefusal) -> Self {
        RehearsalRefusal::Evolve(Box::new(EvolveRefusal::Version(e)))
    }
}

fn rehearse_command(from: &str, to: &str) -> String {
    format!(
        "`ordo rehearsals rehearse --from {from} --to {to} --clone <clone> --reason \"...\" \
         --host <staging>`"
    )
}

impl std::fmt::Display for RehearsalRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RehearsalRefusal::NotStaging { own } => write!(
                f,
                "this deployment is `{own}`, and a rehearsal runs in staging, on clones of \
                 production tenants. Run it on staging's door. If this deployment is staging, \
                 say so: `ordo environments set-own staging --reason \"...\"`"
            ),
            RehearsalRefusal::NoStaging { own } => write!(
                f,
                "this deployment is `{own}`, and no staging comes before it in its chain, so \
                 there is no rehearsal to accept and none is required. To require one, \
                 register staging before it: `{}`",
                envs_mod::register_command(envs_mod::STAGING)
            ),
            RehearsalRefusal::NoTenants => write!(
                f,
                "name at least one clone to rehearse on: `--clone <clone>`. `ordo clones list` \
                 names them"
            ),
            RehearsalRefusal::NotAClone { tenant } => write!(
                f,
                "`{tenant}` is not a clone on this deployment, and a rehearsal runs on clones \
                 alone. Clone a production tenant first: `ordo clones clone <tenant> --reason \
                 \"...\"`; `ordo clones list` names the clones"
            ),
            RehearsalRefusal::WrongPin {
                clone,
                pinned,
                from,
            } => write!(
                f,
                "the clone `{clone}` is on {}, and the step starts from `{from}`. Clone the \
                 production tenant again for a clone on `{from}`: `ordo clones clone <tenant> \
                 --reason \"...\"`",
                pinned
                    .as_deref()
                    .map(|p| format!("`{p}`"))
                    .unwrap_or_else(|| "no version".into())
            ),
            RehearsalRefusal::Failed { clone, refusal } => write!(
                f,
                "the step failed on the clone `{clone}`, so nothing was signed: {refusal}"
            ),
            RehearsalRefusal::Unfinished { clone } => write!(
                f,
                "a backfill on the clone `{clone}` stopped at its budget (backfill.budget), so \
                 nothing was signed yet. Run the rehearsal again: it resumes where it stopped"
            ),
            RehearsalRefusal::NoDoor { env } => write!(
                f,
                "this deployment does not know where `{env}` answers, so it cannot read the \
                 rehearsal there. Record it: `{}`",
                envs_mod::door_command(env)
            ),
            RehearsalRefusal::Source { env, error } => write!(f, "reading from `{env}`: {error}"),
            RehearsalRefusal::WrongSource { given, expected } => write!(
                f,
                "the door recorded for `{expected}` answers as `{given}`. Record `{expected}`'s \
                 own door: `{}`",
                envs_mod::door_command(expected)
            ),
            RehearsalRefusal::NotSamePerson { env, there, here } => write!(
                f,
                "`{env}` read the rehearsal as `{there}`, and this deployment serves you as \
                 `{here}`. Accepting is one person's act: sign in to both through the same \
                 identity service (`ordo login`), as an admin of both, and pass that \
                 sign-in's token for `{env}`"
            ),
            RehearsalRefusal::NotSigned { env, from, to } => write!(
                f,
                "`{env}` signed no rehearsal of the step from `{from}` to `{to}`. Rehearse it \
                 on a clone there first: {}",
                rehearse_command(from, to)
            ),
            RehearsalRefusal::BadSignature { env, why } => write!(
                f,
                "the rehearsal `{env}` answered does not verify ({why}), so it was not \
                 accepted. Rehearse the step again on `{env}`, then accept it"
            ),
            RehearsalRefusal::KeyChanged {
                env,
                recorded,
                given,
            } => write!(
                f,
                "`{env}` signs with key {given}, and this deployment recorded {recorded} for \
                 it. A new key is expected after `{env}` lost its database or master key. If \
                 that is so, accept naming the new key: `--trust-key {given}`"
            ),
            RehearsalRefusal::Release { signed, here } => write!(
                f,
                "staging rehearsed under Ordo {signed}, and this deployment runs Ordo {here}. \
                 Rehearse again under {here}, or accept a different release with a warning: \
                 `ordo environments set release-mismatch warn --reason \"...\"` \
                 ({RELEASE_MISMATCH})"
            ),
            RehearsalRefusal::TooOld { age, max } => write!(
                f,
                "the rehearsal is {age} seconds old, and {MAX_AGE} is {max} seconds. Rehearse \
                 the step again on staging, or set a longer age: `ordo environments set \
                 attestation-max-age <n>d --reason \"...\"`"
            ),
            RehearsalRefusal::Source2 { version, why } => write!(
                f,
                "`{version}` {why}. Promote the version staging rehearsed: `ordo promote \
                 {version} --reason \"...\"`"
            ),
            RehearsalRefusal::Digest {
                from,
                to,
                signed,
                here,
            } => write!(
                f,
                "the step from `{from}` to `{to}` renders here to {here}, and staging signed \
                 {signed}. Production runs only the plan staging rehearsed: render it on \
                 staging with the same options, rehearse it, and accept it again"
            ),
            RehearsalRefusal::StagingAnswers {
                env,
                address,
                status,
            } => write!(
                f,
                "`{env}` answers at {address} ({status}), so it is not down, and break-glass \
                 is for when staging is down. Rehearse the step there and accept it here: {}, \
                 then `ordo rehearsals accept`. To promote with no staging at all, set \
                 promotion.skip_staging: `ordo environments set skip-staging on --reason \
                 \"...\"`",
                rehearse_command("<from>", "<to>")
            ),
            RehearsalRefusal::NoPlan { from, to } => write!(
                f,
                "this deployment stored no plan for the step from `{from}` to `{to}`, and \
                 break-glass lets one stored plan run. Store it first: `ordo evolve --from \
                 {from} --to {to}`"
            ),
            RehearsalRefusal::Evolve(e) => write!(f, "{e}"),
            RehearsalRefusal::Custody(why) => write!(
                f,
                "the rehearsal signing key {why}. It is sealed under this deployment's master \
                 key: restore that key"
            ),
            RehearsalRefusal::Environment(e) => write!(f, "{e}"),
            RehearsalRefusal::Input(why) => write!(f, "{why}"),
            RehearsalRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RehearsalRefusal {}

impl RehearsalRefusal {
    /// A greppable class.
    pub fn class(&self) -> &'static str {
        match self {
            RehearsalRefusal::NotStaging { .. } => "rehearsal_not_staging",
            RehearsalRefusal::NoStaging { .. } => "rehearsal_no_staging",
            RehearsalRefusal::NoTenants => "rehearsal_no_tenants",
            RehearsalRefusal::NotAClone { .. } => "rehearsal_not_a_clone",
            RehearsalRefusal::WrongPin { .. } => "rehearsal_clone_version",
            RehearsalRefusal::Failed { .. } => "rehearsal_failed",
            RehearsalRefusal::Unfinished { .. } => "rehearsal_unfinished",
            RehearsalRefusal::NoDoor { .. } => "rehearsal_no_door",
            RehearsalRefusal::Source { .. } => "rehearsal_unreadable",
            RehearsalRefusal::WrongSource { .. } => "rehearsal_wrong_source",
            RehearsalRefusal::NotSamePerson { .. } => "rehearsal_not_same_person",
            RehearsalRefusal::NotSigned { .. } => "rehearsal_not_signed",
            RehearsalRefusal::BadSignature { .. } => "rehearsal_bad_signature",
            RehearsalRefusal::KeyChanged { .. } => "rehearsal_key_changed",
            RehearsalRefusal::Release { .. } => "rehearsal_release_mismatch",
            RehearsalRefusal::TooOld { .. } => "rehearsal_too_old",
            RehearsalRefusal::Source2 { .. } => "rehearsal_version_differs",
            RehearsalRefusal::Digest { .. } => "rehearsal_digest_differs",
            RehearsalRefusal::StagingAnswers { .. } => "break_glass_staging_answers",
            RehearsalRefusal::NoPlan { .. } => "break_glass_no_plan",
            RehearsalRefusal::Evolve(e) => e.class(),
            RehearsalRefusal::Custody(_) => "rehearsal_key_custody",
            RehearsalRefusal::Environment(_) => "environment_refused",
            RehearsalRefusal::Input(_) => "invalid_param",
            RehearsalRefusal::Store(_) => "store_failed",
        }
    }

    /// **The one place a `RehearsalRefusal` becomes a door answer.**
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {}: {self}", self.class());
        match self {
            RehearsalRefusal::Environment(e) => e.to_exec_error(what),
            RehearsalRefusal::Evolve(e) => e.to_exec_error(what),
            RehearsalRefusal::Failed { refusal, .. } if refusal.transient() => ExecError::of_fault(
                ordo_fault::FaultClass::Retryable,
                ErrorCode::SpaceUnavailable,
                message,
            ),
            RehearsalRefusal::Source { error, .. } if error.retryable() => {
                ExecError::new(ErrorCode::SpaceUnavailable, message)
            }
            RehearsalRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
            RehearsalRefusal::Input(_) | RehearsalRefusal::NoTenants => {
                ExecError::new(ErrorCode::InvalidParam, message)
            }
            _ => ExecError::new(ErrorCode::ApplyRefused, message),
        }
    }
}

// ── hex ──────────────────────────────────────────────────────────────────

fn custody(e: impl std::fmt::Display) -> RehearsalRefusal {
    RehearsalRefusal::Custody(e.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

// ── the key ──────────────────────────────────────────────────────────────

/// **This deployment's rehearsal signing key**, made on first use (Q11: no
/// key to set up). The seed is sealed under the deployment's own keys, in
/// its own space, and never leaves it.
pub async fn own_key(
    pool: &Pool,
    keys: &dyn KeyProvider,
    home: &str,
    at: u64,
) -> Result<SigningKey, RehearsalRefusal> {
    pool.rows(KEY_DDL).await?;
    let home_id = SpaceId(home.to_string());
    let read = || async {
        pool.rows("SELECT \"public\", \"sealed\" FROM \"$rehearsal_key\" WHERE \"id\" = 1")
            .await
    };
    if read().await?.is_empty() {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed)
            .map_err(|_| RehearsalRefusal::Custody("could not be made: no randomness".into()))?;
        let fresh = SigningKey::from_bytes(&seed);
        let lease = keys
            .release(&home_id, Purpose::Write, "rehearsal-key")
            .map_err(custody)?;
        let sealed = keys
            .seal_at(&lease, KEY_ENTITY, KEY_FIELD, Anchor::Row("1"), &seed)
            .map_err(custody)?;
        pool.rows(&format!(
            "INSERT INTO \"$rehearsal_key\" (\"id\", \"public\", \"sealed\", \"at\") \
             VALUES (1, {}, {}, {at}) ON CONFLICT DO NOTHING",
            sql_lit(&hex(fresh.verifying_key().as_bytes())),
            ordo_crypto::sealed::bytea_lit(&sealed)
        ))
        .await?;
    }
    // Read back: a replica that made one at the same moment may have won.
    let rows = read().await?;
    let row = rows
        .first()
        .ok_or_else(|| RehearsalRefusal::Custody("was not stored".into()))?;
    let public = row.first().cloned().flatten().unwrap_or_default();
    let sealed = row
        .get(1)
        .cloned()
        .flatten()
        .and_then(|h| ordo_crypto::sealed::unhex(&h))
        .ok_or_else(|| RehearsalRefusal::Custody("is not readable".into()))?;
    let lease = keys
        .release(&home_id, Purpose::Read, "rehearsal-key")
        .map_err(custody)?;
    let seed = keys
        .open_at(&lease, KEY_ENTITY, KEY_FIELD, Anchor::Row("1"), &sealed)
        .map_err(custody)?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| RehearsalRefusal::Custody("is not 32 bytes".into()))?;
    let key = SigningKey::from_bytes(&seed);
    if hex(key.verifying_key().as_bytes()) != public {
        return Err(RehearsalRefusal::Custody(
            "does not match its public half".into(),
        ));
    }
    Ok(key)
}

// ── what is signed ───────────────────────────────────────────────────────

/// One signed rehearsal: the attestation's canonical text and its
/// signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// The attestation, canonical JSON. What the signature covers.
    pub body: String,
    /// Ed25519 over `body`, hex.
    pub signature: String,
}

impl Signed {
    pub fn attestation(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }

    pub fn to_json(&self) -> Value {
        json!({ "attestation": self.attestation(), "body": self.body, "signature": self.signature })
    }

    /// **Verify the signature** with the key the attestation names. Answers
    /// the key, hex.
    pub fn verify(&self) -> Result<String, String> {
        let a = self.attestation();
        if a["format"].as_str() != Some(FORMAT) {
            return Err(format!("not a `{FORMAT}` attestation"));
        }
        let key_hex = a["key"].as_str().ok_or("it names no key")?.to_string();
        let key: [u8; 32] = unhex(&key_hex)
            .and_then(|b| b.try_into().ok())
            .ok_or("its key is not 32 bytes of hex")?;
        let key = VerifyingKey::from_bytes(&key).map_err(|_| "its key is not a key")?;
        let sig = unhex(&self.signature)
            .and_then(|b| ed25519_dalek::Signature::from_slice(&b).ok())
            .ok_or("its signature is not 64 bytes of hex")?;
        // Canonical: the text signed is the text a re-serialisation gives.
        if ordo_plan::artifact::to_bytes(&a) != self.body {
            return Err("its text is not canonical".into());
        }
        key.verify_strict(self.body.as_bytes(), &sig)
            .map_err(|_| "the signature does not match its text".to_string())?;
        Ok(key_hex)
    }
}

/// **Q14**: each backfill in a plan's receipt that is not reproducible: a
/// function that declares secrets or hosts.
pub fn not_reproducible(receipt: &Value) -> Vec<Value> {
    receipt["backfills"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter(|b| b["reproducible"].as_bool() == Some(false))
                .map(|b| {
                    json!({
                        "entity": b["entity"], "field": b["field"],
                        "function": b["function"]["name"],
                        "secrets": b["function"]["secrets"],
                        "hosts": b["function"]["hosts"],
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── staging: rehearse ────────────────────────────────────────────────────

/// What a rehearsal asks for.
pub struct RehearseRequest<'a> {
    pub from: &'a str,
    pub to: &'a str,
    /// Clones, each by space id or name.
    pub tenants: &'a [String],
    pub reason: &'a str,
}

/// What a rehearsal did.
#[derive(Debug)]
pub struct Rehearsed {
    pub signed: Signed,
    /// Each clone and what the step did on its database.
    pub ran: Vec<(String, usize, bool)>,
}

/// The source identity of a stored version.
async fn source_id_of(pool: &Pool, version: &str) -> Result<Option<String>, RehearsalRefusal> {
    match Versions::on(pool).get(version).await? {
        Some(a) => Ok(Some(evolve::compiled(&a)?.source_id)),
        None => Ok(None),
    }
}

/// **Rehearse a stored plan on staging's clones**, and sign it when every
/// clone runs green. The caller holds no lock: this takes the apply lock.
#[allow(clippy::too_many_arguments)]
pub async fn rehearse(
    pool: &Pool,
    pools: &dyn PoolResolver,
    keys: &dyn KeyProvider,
    home: &str,
    ctx: &StepContext,
    req: RehearseRequest<'_>,
    actor: &str,
    at: u64,
) -> Result<Rehearsed, RehearsalRefusal> {
    crate::versions::check_reason(req.reason)
        .map_err(|e| RehearsalRefusal::Input(e.to_string()))?;
    let own = Environments::on(pool).own().await?;
    if own.name != envs_mod::STAGING {
        return Err(RehearsalRefusal::NotStaging { own: own.name });
    }
    let v = Versions::on(pool);
    let from = v.resolve(req.from).await?;
    let to = v.resolve(req.to).await?;
    let Some(plan) = Plans::on(pool).latest(&from, &to).await? else {
        return Err(EvolveRefusal::NoPath { from, to }.into());
    };
    if req.tenants.iter().all(|t| t.trim().is_empty()) {
        return Err(RehearsalRefusal::NoTenants);
    }
    let clones = crate::clones::clones(pool)
        .await
        .map_err(|e| RehearsalRefusal::Input(e.to_string()))?;
    let mut spaces = Vec::new();
    for t in req
        .tenants
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
    {
        let Some(c) = clones.iter().find(|c| c.space == t || c.name == t) else {
            return Err(RehearsalRefusal::NotAClone {
                tenant: t.to_string(),
            });
        };
        let pinned = v.pin_of(&c.space).await?.and_then(|p| p.version);
        if pinned.as_deref() != Some(plan.from.as_str()) {
            return Err(RehearsalRefusal::WrongPin {
                clone: c.space.clone(),
                pinned,
                from: plan.from.clone(),
            });
        }
        if !spaces.contains(&c.space) {
            spaces.push(c.space.clone());
        }
    }
    let lock = match crate::apply_op::ApplyLock::take(pool).await {
        Ok(Some(l)) => l,
        Ok(None) => return Err(EvolveRefusal::Busy.into()),
        Err(e) => return Err(EvolveRefusal::Store(e.to_string()).into()),
    };
    let mut ran = Vec::new();
    for space in &spaces {
        let failed = |refusal| RehearsalRefusal::Failed {
            clone: space.clone(),
            refusal: Box::new(refusal),
        };
        let target = pools
            .resolve(&ordo_crypto::SpaceId(space.clone()))
            .await
            .map_err(|e| {
                failed(EvolveRefusal::Run {
                    database: space.clone(),
                    from: plan.from.clone(),
                    to: plan.to.clone(),
                    class: "unreachable",
                    why: e.to_string(),
                    transient: true,
                })
            })?;
        let r = evolve::run_step_with(pool, &target, space, &plan, actor, ctx)
            .await
            .map_err(failed)?;
        if matches!(r, StepRun::Backfilling { .. }) {
            return Err(RehearsalRefusal::Unfinished {
                clone: space.clone(),
            });
        }
        v.pin(space, &plan.to, actor, at).await?;
        ran.push((space.clone(), r.statements(), r == StepRun::Current));
    }
    drop(lock);

    let key = own_key(pool, keys, home, at).await?;
    let (Some(a), Some(b)) = (v.get(&plan.from).await?, v.get(&plan.to).await?) else {
        return Err(crate::versions::VersionRefusal::UnknownVersion(plan.from.clone()).into());
    };
    let receipt: Value = serde_json::from_str(&plan.receipt).unwrap_or(Value::Null);
    let release = crate::release_check::release();
    let body = json!({
        "format": FORMAT,
        "env": own.name,
        "from": plan.from,
        "to": plan.to,
        "fromSource": evolve::compiled(&a)?.source_id,
        "toSource": evolve::compiled(&b)?.source_id,
        "digest": plan.digest,
        "options": plan.options,
        "build": ordo_plan::artifact::build_identity_of(&b.artifact, &b.ddl, release),
        "release": release,
        "result": "green",
        "clones": spaces,
        "notReproducible": not_reproducible(&receipt),
        "actor": actor,
        "reason": req.reason.trim(),
        "at": at,
        "key": hex(key.verifying_key().as_bytes()),
    });
    let text = ordo_plan::artifact::to_bytes(&body);
    let signed = Signed {
        signature: hex(&key.sign(text.as_bytes()).to_bytes()),
        body: text,
    };
    pool.rows(SIGNED_DDL).await?;
    pool.rows(&format!(
        "INSERT INTO \"$rehearsals\" (\"from_version\", \"to_version\", \"digest\", \"body\", \
         \"signature\", \"at\") VALUES ({}, {}, {}, {}, {}, {at})",
        sql_lit(&plan.from),
        sql_lit(&plan.to),
        sql_lit(&plan.digest),
        sql_lit(&signed.body),
        sql_lit(&signed.signature)
    ))
    .await?;
    Ok(Rehearsed { signed, ran })
}

/// The signed rehearsals this deployment holds, newest first, for one step
/// when `from` and `to` are given.
pub async fn signed(
    pool: &Pool,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<Signed>, RehearsalRefusal> {
    let mut filter = Vec::new();
    if let Some(f) = from {
        filter.push(format!("\"from_version\" = {}", sql_lit(f)));
    }
    if let Some(t) = to {
        filter.push(format!("\"to_version\" = {}", sql_lit(t)));
    }
    let filter = if filter.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filter.join(" AND "))
    };
    let sql = format!(
        "SELECT \"body\", \"signature\" FROM \"$rehearsals\" {filter} ORDER BY \"seq\" DESC \
         LIMIT 100"
    );
    match pool.rows(&sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(Signed {
                    body: r.first().cloned().flatten()?,
                    signature: r.get(1).cloned().flatten()?,
                })
            })
            .collect()),
        Err(e) if e.is_missing_table() => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

// ── production: accept ───────────────────────────────────────────────────

/// The rehearsal settings this deployment holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RehearsalSettings {
    /// `promotion.release_mismatch` is `warn`.
    pub release_warn: bool,
    /// `promotion.attestation_max_age`, seconds.
    pub max_age: u64,
}

impl Default for RehearsalSettings {
    fn default() -> Self {
        RehearsalSettings {
            release_warn: false,
            max_age: DEFAULT_MAX_AGE_SECS,
        }
    }
}

/// Each setting's latest value, its default when never set.
pub async fn settings(pool: &Pool) -> Result<RehearsalSettings, RehearsalRefusal> {
    let latest = |name: &'static str| {
        format!(
            "SELECT \"value\" FROM \"$promotion_settings\" WHERE \"setting\" = {} \
             ORDER BY \"seq\" DESC LIMIT 1",
            sql_lit(name)
        )
    };
    let read = |sql: String| async move {
        match pool.rows(&sql).await {
            Ok(rows) => Ok(rows.first().and_then(|r| r.first().cloned().flatten())),
            Err(e) if e.is_missing_table() => Ok(None),
            Err(e) => Err(RehearsalRefusal::Store(e)),
        }
    };
    let mut s = RehearsalSettings::default();
    if let Some(v) = read(latest(RELEASE_MISMATCH)).await? {
        s.release_warn = v == "warn";
    }
    if let Some(v) = read(latest(MAX_AGE)).await? {
        if let Ok(n) = parse_age(&v) {
            s.max_age = n;
        }
    }
    Ok(s)
}

/// `7d`, `36h`, `90m` or seconds.
pub fn parse_age(value: &str) -> Result<u64, String> {
    let v = value.trim();
    let (n, unit) = match v.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&v[..i], c),
        _ => (v, 's'),
    };
    let n: u64 = n
        .parse()
        .map_err(|_| format!("{MAX_AGE} takes a count with d, h, m or s, like 7d, not `{v}`"))?;
    let mult = match unit {
        'd' => 86_400,
        'h' => 3_600,
        'm' => 60,
        's' => 1,
        _ => {
            return Err(format!(
                "{MAX_AGE} takes a count with d, h, m or s, like 7d, not `{v}`"
            ))
        }
    };
    if n == 0 {
        return Err(format!("{MAX_AGE} must be more than 0"));
    }
    Ok(n.saturating_mul(mult))
}

/// Staging, when it comes before this deployment in its chain and
/// `promotion.skip_staging` is off. `None`: no rehearsal is required.
async fn staging_before(
    pool: &Pool,
) -> Result<Option<(Environment, Environment)>, RehearsalRefusal> {
    let envs = Environments::on(pool);
    let chain = envs.chain().await?;
    let Some(own) = chain.iter().find(|e| e.own).cloned() else {
        return Ok(None);
    };
    if own.name == envs_mod::STAGING {
        return Ok(None);
    }
    let Some(staging) = chain
        .iter()
        .find(|e| e.name == envs_mod::STAGING && e.position < own.position)
        .cloned()
    else {
        return Ok(None);
    };
    if envs.promotion_settings().await?.skip_staging {
        return Ok(None);
    }
    Ok(Some((own, staging)))
}

/// **Whether this deployment requires a rehearsal**: staging comes before
/// it in its chain and `promotion.skip_staging` is off.
pub async fn required(pool: &Pool) -> Result<bool, RehearsalRefusal> {
    Ok(staging_before(pool).await?.is_some())
}

/// What an acceptance asks for.
pub struct AcceptRequest<'a> {
    pub from: &'a str,
    pub to: &'a str,
    /// The person's own token for staging, from the same sign-in.
    pub token: &'a str,
    /// The key the person names after staging's key changed.
    pub trust_key: Option<&'a str>,
}

/// What an acceptance did.
#[derive(Debug)]
pub struct Accepted {
    pub signed: Signed,
    pub env: String,
    /// The key was recorded here for the first time, or replaced.
    pub key_recorded: bool,
    /// The plan was stored here by this acceptance.
    pub plan_stored: bool,
    pub plan: StoredPlan,
    /// Warnings: another release under `warn`, a backfill not reproducible.
    pub warnings: Vec<String>,
}

impl Accepted {
    pub fn to_json(&self) -> Value {
        let a = self.signed.attestation();
        json!({
            "env": self.env, "from": self.plan.from, "to": self.plan.to,
            "digest": self.plan.digest, "key": a["key"], "keyRecorded": self.key_recorded,
            "planStored": self.plan_stored, "clones": a["clones"],
            "notReproducible": a["notReproducible"], "release": a["release"],
            "rehearsedAt": a["at"], "warnings": self.warnings,
        })
    }
}

/// **Accept staging's rehearsal of one step**: read it from staging's door
/// as the person accepting, check it, re-render the plan here, store it,
/// and record the acceptance.
pub async fn accept(
    pool: &Pool,
    req: AcceptRequest<'_>,
    actor: &str,
    at: u64,
) -> Result<Accepted, RehearsalRefusal> {
    let Some((_own, staging)) = staging_before(pool).await? else {
        let own = Environments::on(pool).own().await?;
        return Err(RehearsalRefusal::NoStaging { own: own.name });
    };
    let v = Versions::on(pool);
    let resolve = |name: &str| {
        let name = name.trim().to_string();
        async move {
            v.resolve(&name)
                .await
                .map_err(|_| RehearsalRefusal::Source2 {
                    version: name.clone(),
                    why: "is not stored on this deployment".into(),
                })
        }
    };
    let from = resolve(req.from).await?;
    let to = resolve(req.to).await?;
    let Some(door) = &staging.door else {
        return Err(RehearsalRefusal::NoDoor { env: staging.name });
    };
    let source = |error| RehearsalRefusal::Source {
        env: staging.name.clone(),
        error,
    };
    let client =
        StepDoor::parse(&door.address, crate::step_door::plain_http_allowed()).map_err(source)?;
    let row = client
        .rehearsals(&door.space, req.token, &from, &to)
        .await
        .map_err(source)?;
    let given_env = row["env"].as_str().unwrap_or_default().to_string();
    if given_env != staging.name {
        return Err(RehearsalRefusal::WrongSource {
            given: given_env,
            expected: staging.name.clone(),
        });
    }
    let read_by = row["readBy"].as_str().unwrap_or_default();
    if read_by != actor {
        return Err(RehearsalRefusal::NotSamePerson {
            env: staging.name.clone(),
            there: read_by.to_string(),
            here: actor.to_string(),
        });
    }
    // The newest signed rehearsal of the step.
    let newest = row["rehearsals"].as_array().and_then(|all| {
        all.iter().find_map(|s| {
            Some(Signed {
                body: s["body"].as_str()?.to_string(),
                signature: s["signature"].as_str()?.to_string(),
            })
        })
    });
    let Some(signed) = newest else {
        return Err(RehearsalRefusal::NotSigned {
            env: staging.name.clone(),
            from,
            to,
        });
    };
    let key = signed
        .verify()
        .map_err(|why| RehearsalRefusal::BadSignature {
            env: staging.name.clone(),
            why,
        })?;
    let a = signed.attestation();
    let text = |k: &str| a[k].as_str().unwrap_or_default().to_string();
    if text("env") != staging.name || text("from") != from || text("to") != to {
        return Err(RehearsalRefusal::BadSignature {
            env: staging.name.clone(),
            why: format!(
                "it names `{}` from `{}` to `{}`, not this step",
                text("env"),
                text("from"),
                text("to")
            ),
        });
    }

    // The key: recorded the first time, refused when it changes unless the
    // person names the new one.
    pool.rows(KEYS_DDL).await?;
    let recorded = pool
        .rows(&format!(
            "SELECT \"public\" FROM \"$rehearsal_keys\" WHERE \"env\" = {}",
            sql_lit(&staging.name)
        ))
        .await?
        .first()
        .and_then(|r| r.first().cloned().flatten());
    let trusted = req.trust_key.map(str::trim).filter(|k| !k.is_empty());
    let key_recorded = match &recorded {
        Some(r) if *r == key => false,
        Some(r) if trusted != Some(key.as_str()) => {
            return Err(RehearsalRefusal::KeyChanged {
                env: staging.name.clone(),
                recorded: r.clone(),
                given: key,
            })
        }
        _ => true,
    };

    // Release and age.
    let s = settings(pool).await?;
    let mut warnings = Vec::new();
    let here = crate::release_check::release();
    let signed_release = text("release");
    if signed_release != here {
        if !s.release_warn {
            return Err(RehearsalRefusal::Release {
                signed: signed_release,
                here: here.to_string(),
            });
        }
        warnings.push(format!(
            "rehearsed under Ordo {signed_release}, accepted under {here} ({RELEASE_MISMATCH} \
             is warn)"
        ));
    }
    let age = at.saturating_sub(a["at"].as_u64().unwrap_or(0));
    if age > s.max_age {
        return Err(RehearsalRefusal::TooOld {
            age,
            max: s.max_age,
        });
    }

    // The same versions, and the same plan rendered here.
    for (version, field) in [(&from, "fromSource"), (&to, "toSource")] {
        let mine = source_id_of(pool, version).await?;
        if mine.as_deref() != Some(text(field).as_str()) {
            return Err(RehearsalRefusal::Source2 {
                version: version.clone(),
                why: format!(
                    "has source identity {} here, and staging rehearsed {}",
                    mine.unwrap_or_else(|| "none".into()),
                    text(field)
                ),
            });
        }
    }
    let options = evolve::parse_options(&text("options"))?;
    let rendered = evolve::render(pool, &from, &to, &options, at).await?;
    if rendered.digest != text("digest") {
        return Err(RehearsalRefusal::Digest {
            from,
            to,
            signed: text("digest"),
            here: rendered.digest,
        });
    }
    let (plan, plan_stored) = Plans::on(pool).store(&rendered, actor, at).await?;
    for nr in a["notReproducible"].as_array().into_iter().flatten() {
        warnings.push(format!(
            "the backfill of {}.{} by function `{}` uses secrets {} and hosts {}: staging's \
             run is not a reproduction of production's (Q14)",
            nr["entity"].as_str().unwrap_or("?"),
            nr["field"].as_str().unwrap_or("?"),
            nr["function"].as_str().unwrap_or("?"),
            nr["secrets"],
            nr["hosts"]
        ));
    }

    if key_recorded {
        pool.rows(&format!(
            "INSERT INTO \"$rehearsal_keys\" (\"env\", \"public\", \"actor\", \"at\") \
             VALUES ({e}, {k}, {a}, {at}) ON CONFLICT (\"env\") DO UPDATE SET \
             \"public\" = {k}, \"actor\" = {a}, \"at\" = {at}",
            e = sql_lit(&staging.name),
            k = sql_lit(&key),
            a = sql_lit(actor),
        ))
        .await?;
    }
    pool.rows(ACCEPTED_DDL).await?;
    pool.rows(&format!(
        "INSERT INTO \"$rehearsals_accepted\" (\"from_version\", \"to_version\", \"digest\", \
         \"env\", \"key\", \"body\", \"signature\", \"actor\", \"at\") VALUES \
         ({}, {}, {}, {}, {}, {}, {}, {}, {at})",
        sql_lit(&from),
        sql_lit(&to),
        sql_lit(&plan.digest),
        sql_lit(&staging.name),
        sql_lit(&key),
        sql_lit(&signed.body),
        sql_lit(&signed.signature),
        sql_lit(actor),
    ))
    .await?;
    Ok(Accepted {
        signed,
        env: staging.name,
        key_recorded,
        plan_stored,
        plan,
        warnings,
    })
}

/// The digests accepted here for one step.
async fn accepted_digests(pool: &Pool, from: &str, to: &str) -> Result<Vec<String>, PoolError> {
    let sql = format!(
        "SELECT DISTINCT \"digest\" FROM \"$rehearsals_accepted\" WHERE \"from_version\" = {} \
         AND \"to_version\" = {}",
        sql_lit(from),
        sql_lit(to)
    );
    match pool.rows(&sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect()),
        Err(e) if e.is_missing_table() => Ok(vec![]),
        Err(e) => Err(e),
    }
}

/// The rehearsals accepted here, newest first.
pub async fn accepted(pool: &Pool) -> Result<Vec<Value>, PoolError> {
    let sql = "SELECT \"from_version\", \"to_version\", \"digest\", \"env\", \"key\", \
               \"actor\", \"at\"::text FROM \"$rehearsals_accepted\" ORDER BY \"seq\" DESC \
               LIMIT 100";
    match pool.rows(sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .map(|r| {
                let g = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
                json!({
                    "from": g(0), "to": g(1), "digest": g(2), "env": g(3), "key": g(4),
                    "actor": g(5), "at": g(6).parse::<u64>().unwrap_or(0),
                })
            })
            .collect()),
        Err(e) if e.is_missing_table() => Ok(vec![]),
        Err(e) => Err(e),
    }
}

// ── break-glass ──────────────────────────────────────────────────────────

/// What a break-glass did.
#[derive(Debug)]
pub struct BrokeGlass {
    pub env: String,
    pub plan: StoredPlan,
    pub reason: String,
    /// Why staging counts as down: the probe's own words.
    pub staging_said: String,
    pub at: u64,
    pub until: u64,
}

impl BrokeGlass {
    pub fn to_json(&self) -> Value {
        json!({
            "env": self.env, "from": self.plan.from, "to": self.plan.to,
            "digest": self.plan.digest, "reason": self.reason,
            "stagingSaid": self.staging_said, "at": self.at, "until": self.until,
        })
    }
}

/// **Break glass** (§3.2 item 6): staging is down, so the deployment owner
/// lets the step's stored plan run here without a rehearsal, with a
/// recorded reason, for [`BREAK_GLASS_SECS`].
///
/// - Only when staging comes before this deployment and does not answer:
///   staging's recorded door is asked `/health/live`, and any answer below
///   500 refuses ([`RehearsalRefusal::StagingAnswers`]). A deployment that
///   does not know where staging answers cannot tell it is down
///   ([`RehearsalRefusal::NoDoor`]).
/// - One step, one digest: the plan this deployment stored for it. No plan,
///   no break-glass ([`RehearsalRefusal::NoPlan`]).
/// - A reason is required, and kept with the actor, what staging said and
///   the time it ends, in `$rehearsal_break_glass`.
///
/// Who: the deployment owner, as for every rehearsal op. A machine key is
/// refused at the door (`deployment_access`), and a delegated token is,
/// as for every platform write.
pub async fn break_glass(
    pool: &Pool,
    from: &str,
    to: &str,
    reason: &str,
    actor: &str,
    at: u64,
) -> Result<BrokeGlass, RehearsalRefusal> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(RehearsalRefusal::Input(
            "break-glass takes a reason, kept on the deployment's stream: `--reason \"...\"`"
                .into(),
        ));
    }
    let Some((_own, staging)) = staging_before(pool).await? else {
        let own = Environments::on(pool).own().await?;
        return Err(RehearsalRefusal::NoStaging { own: own.name });
    };
    let v = Versions::on(pool);
    let from = v.resolve(from.trim()).await?;
    let to = v.resolve(to.trim()).await?;
    let Some(plan) = Plans::on(pool).latest(&from, &to).await? else {
        return Err(RehearsalRefusal::NoPlan { from, to });
    };
    let Some(door) = &staging.door else {
        return Err(RehearsalRefusal::NoDoor { env: staging.name });
    };
    let client = StepDoor::parse(&door.address, crate::step_door::plain_http_allowed()).map_err(
        |error| RehearsalRefusal::Source {
            env: staging.name.clone(),
            error,
        },
    )?;
    let staging_said = match client.live().await {
        Ok(status) => {
            return Err(RehearsalRefusal::StagingAnswers {
                env: staging.name,
                address: client.address().to_string(),
                status,
            })
        }
        Err(e) => e.to_string(),
    };
    let until = at.saturating_add(BREAK_GLASS_SECS);
    pool.rows(BREAK_GLASS_DDL).await?;
    pool.rows(&format!(
        "INSERT INTO \"$rehearsal_break_glass\" (\"from_version\", \"to_version\", \"digest\", \
         \"env\", \"reason\", \"staging_said\", \"actor\", \"at\", \"until\") VALUES \
         ({}, {}, {}, {}, {}, {}, {}, {at}, {until})",
        sql_lit(&plan.from),
        sql_lit(&plan.to),
        sql_lit(&plan.digest),
        sql_lit(&staging.name),
        sql_lit(reason),
        sql_lit(&staging_said),
        sql_lit(actor),
    ))
    .await?;
    Ok(BrokeGlass {
        env: staging.name,
        plan,
        reason: reason.to_string(),
        staging_said,
        at,
        until,
    })
}

/// The digests a break-glass lets run for one step at `now`.
async fn broken_glass_digests(
    pool: &Pool,
    from: &str,
    to: &str,
    now: u64,
) -> Result<Vec<String>, PoolError> {
    let sql = format!(
        "SELECT DISTINCT \"digest\" FROM \"$rehearsal_break_glass\" WHERE \"from_version\" = {} \
         AND \"to_version\" = {} AND \"until\" >= {now}",
        sql_lit(from),
        sql_lit(to)
    );
    match pool.rows(&sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect()),
        Err(e) if e.is_missing_table() => Ok(vec![]),
        Err(e) => Err(e),
    }
}

/// Every break-glass here, newest first, each marked active or ended.
pub async fn broken_glass(pool: &Pool, now: u64) -> Result<Vec<Value>, PoolError> {
    let sql = "SELECT \"from_version\", \"to_version\", \"digest\", \"env\", \"reason\", \
               \"staging_said\", \"actor\", \"at\"::text, \"until\"::text \
               FROM \"$rehearsal_break_glass\" ORDER BY \"seq\" DESC LIMIT 100";
    match pool.rows(sql).await {
        Ok(rows) => Ok(rows
            .into_iter()
            .map(|r| {
                let g = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
                let until = g(8).parse::<u64>().unwrap_or(0);
                json!({
                    "from": g(0), "to": g(1), "digest": g(2), "env": g(3), "reason": g(4),
                    "stagingSaid": g(5), "actor": g(6),
                    "at": g(7).parse::<u64>().unwrap_or(0), "until": until,
                    "active": until >= now,
                })
            })
            .collect()),
        Err(e) if e.is_missing_table() => Ok(vec![]),
        Err(e) => Err(e),
    }
}

/// **The gate** (§3.2, increment 12b): on a deployment with staging before
/// it in its chain, a stored step runs only when staging rehearsed it and
/// signed its digest, and this deployment accepted that, or a break-glass
/// for that digest has not ended. Called by [`evolve::run_step_with`]
/// before a step changes anything.
pub async fn require_rehearsed(pool: &Pool, plan: &StoredPlan) -> Result<(), EvolveRefusal> {
    let required = staging_before(pool).await.map_err(|e| match e {
        RehearsalRefusal::Store(e) => EvolveRefusal::from(e),
        other => EvolveRefusal::Store(other.to_string()),
    })?;
    if required.is_none() {
        return Ok(());
    }
    let signed = accepted_digests(pool, &plan.from, &plan.to).await?;
    if signed.contains(&plan.digest) {
        return Ok(());
    }
    let now = crate::runtime::now_secs();
    if broken_glass_digests(pool, &plan.from, &plan.to, now)
        .await?
        .contains(&plan.digest)
    {
        return Ok(());
    }
    Err(EvolveRefusal::NotRehearsed {
        from: plan.from.clone(),
        to: plan.to.clone(),
        digest: plan.digest.clone(),
        signed,
    })
}

// ── the door ─────────────────────────────────────────────────────────────

fn text<'a>(bound: &'a BoundParams, k: &str) -> Option<&'a str> {
    bound.0.get(k).and_then(|v| v.as_str())
}

/// The rehearsal ops' specs. Owner only, on the deployment's own space; a
/// `clone.readers` person may also call [`REHEARSE`] (Q12).
pub fn specs() -> Vec<ordo_api::OpSpec> {
    use ordo_api::{
        FieldSpec, NotSyncable, OpCost, OpKind, OpSpec, ParamSpec, ParamTy, RefineTier, Syncability,
    };
    let param = |name: &str, max: u32, required: bool| ParamSpec {
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
        spec(
            REHEARSE,
            OpKind::Write,
            vec![
                param("from", 128, true),
                param("to", 128, true),
                // Clones by id or name, comma-separated.
                param("tenants", 4096, true),
                param("reason", 4096, true),
            ],
            &["rehearsal", "ran"],
        ),
        spec(
            REHEARSALS,
            OpKind::Read,
            vec![param("from", 128, false), param("to", 128, false)],
            &[
                "env",
                "key",
                "readBy",
                "rehearsals",
                "accepted",
                "brokenGlass",
            ],
        ),
        spec(
            ACCEPT,
            OpKind::Write,
            vec![
                param("from", 128, true),
                param("to", 128, true),
                // The caller's own token for staging, from the same
                // sign-in. Sent there once, never stored.
                param("fromToken", 8 * 1024, true),
                param("trustKey", 128, false),
            ],
            &[
                "env",
                "from",
                "to",
                "digest",
                "key",
                "keyRecorded",
                "planStored",
                "clones",
                "notReproducible",
                "release",
                "rehearsedAt",
                "warnings",
            ],
        ),
        spec(
            BREAK_GLASS,
            OpKind::Write,
            vec![
                param("from", 128, true),
                param("to", 128, true),
                param("reason", 4096, true),
            ],
            &[
                "env",
                "from",
                "to",
                "digest",
                "reason",
                "stagingSaid",
                "at",
                "until",
            ],
        ),
    ]
}

/// Run one rehearsal op. Bound to the deployment's own space.
pub async fn execute(
    ops: &crate::clones::CloneOps,
    name: &str,
    space: &str,
    bound: &BoundParams,
    caller: &str,
) -> Result<ExecResult, ExecError> {
    let apply: &Arc<crate::apply_op::ApplyOps> = &ops.apply;
    apply.home_only(space)?;
    let now = crate::runtime::now_secs();
    let pool = &apply.pool;
    let row = match name {
        REHEARSE => {
            const WHAT: &str = "the plan was not rehearsed";
            let tenants: Vec<String> = text(bound, "tenants")
                .unwrap_or_default()
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            let reason = text(bound, "reason").unwrap_or_default();
            let ctx = apply.step_context().await?;
            let done = rehearse(
                pool,
                apply.pools.as_ref(),
                ops.keys.as_ref(),
                &apply.home,
                &ctx,
                RehearseRequest {
                    from: text(bound, "from").unwrap_or_default(),
                    to: text(bound, "to").unwrap_or_default(),
                    tenants: &tenants,
                    reason,
                },
                caller,
                now,
            )
            .await
            .map_err(|e| {
                apply.record(
                    caller,
                    REHEARSE,
                    "rehearsal-failed",
                    format!("class={} by={caller}", e.class()),
                );
                e.to_exec_error(WHAT)
            })?;
            let a = done.signed.attestation();
            apply.record(
                caller,
                REHEARSE,
                "rehearsed",
                format!(
                    "from={} to={} digest={} clones={} by={caller} reason={}",
                    a["from"].as_str().unwrap_or_default(),
                    a["to"].as_str().unwrap_or_default(),
                    a["digest"].as_str().unwrap_or_default(),
                    done.ran.len(),
                    reason.trim()
                ),
            );
            json!({
                "rehearsal": done.signed.to_json(),
                "ran": done.ran.iter().map(|(c, n, current)| json!({
                    "clone": c, "statements": n, "current": current,
                })).collect::<Vec<_>>(),
            })
        }
        REHEARSALS => {
            const WHAT: &str = "the rehearsals could not be read";
            let v = Versions::on(pool);
            let named = |k: &str| text(bound, k).map(str::trim).filter(|s| !s.is_empty());
            let from = match named("from") {
                Some(n) => Some(v.resolve(n).await.map_err(|e| e.to_exec_error(WHAT))?),
                None => None,
            };
            let to = match named("to") {
                Some(n) => Some(v.resolve(n).await.map_err(|e| e.to_exec_error(WHAT))?),
                None => None,
            };
            let list = signed(pool, from.as_deref(), to.as_deref())
                .await
                .map_err(|e| e.to_exec_error(WHAT))?;
            let own = Environments::on(pool)
                .own()
                .await
                .map_err(|e| e.to_exec_error(WHAT))?;
            // The key once it exists; reading never makes one.
            let key = pool
                .rows("SELECT \"public\" FROM \"$rehearsal_key\" WHERE \"id\" = 1")
                .await
                .ok()
                .and_then(|r| r.first().and_then(|r| r.first().cloned().flatten()));
            let acc = accepted(pool)
                .await
                .map_err(|e| RehearsalRefusal::Store(e).to_exec_error(WHAT))?;
            let glass = broken_glass(pool, now)
                .await
                .map_err(|e| RehearsalRefusal::Store(e).to_exec_error(WHAT))?;
            json!({
                "env": own.name, "key": key, "readBy": caller,
                "rehearsals": list.iter().map(Signed::to_json).collect::<Vec<_>>(),
                "accepted": acc, "brokenGlass": glass,
            })
        }
        BREAK_GLASS => {
            const WHAT: &str = "the glass was not broken";
            let from = text(bound, "from").unwrap_or_default();
            let to = text(bound, "to").unwrap_or_default();
            let reason = text(bound, "reason").unwrap_or_default();
            // Every try is on the stream, refused or not.
            let done = break_glass(pool, from, to, reason, caller, now)
                .await
                .map_err(|e| {
                    apply.record(
                        caller,
                        BREAK_GLASS,
                        "break-glass-refused",
                        format!(
                            "from={from} to={to} class={} by={caller} reason={}",
                            e.class(),
                            reason.trim()
                        ),
                    );
                    e.to_exec_error(WHAT)
                })?;
            apply.record(
                caller,
                BREAK_GLASS,
                "break-glass",
                format!(
                    "from={} to={} digest={} env={} until={} by={caller} staging_said={} \
                     reason={}",
                    done.plan.from,
                    done.plan.to,
                    done.plan.digest,
                    done.env,
                    done.until,
                    done.staging_said,
                    done.reason
                ),
            );
            done.to_json()
        }
        ACCEPT => {
            const WHAT: &str = "the rehearsal was not accepted";
            let done = accept(
                pool,
                AcceptRequest {
                    from: text(bound, "from").unwrap_or_default(),
                    to: text(bound, "to").unwrap_or_default(),
                    token: text(bound, "fromToken").unwrap_or_default(),
                    trust_key: text(bound, "trustKey"),
                },
                caller,
                now,
            )
            .await
            .map_err(|e| e.to_exec_error(WHAT))?;
            apply.record(
                caller,
                ACCEPT,
                "rehearsal-accepted",
                format!(
                    "from={} to={} digest={} env={} key={} key_recorded={} by={caller}",
                    done.plan.from,
                    done.plan.to,
                    done.plan.digest,
                    done.env,
                    done.signed.attestation()["key"]
                        .as_str()
                        .unwrap_or_default(),
                    done.key_recorded
                ),
            );
            done.to_json()
        }
        other => {
            return Err(ExecError::internal(format!(
                "`{other}` is not a rehearsal op"
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
    fn a_backfill_that_declares_secrets_or_hosts_is_marked_not_reproducible() {
        let receipt = json!({"backfills": [
            {"entity": "notes", "field": "summary", "reproducible": false,
             "function": {"name": "summarise", "secrets": ["llmKey"], "hosts": ["api.example.com"]}},
            {"entity": "notes", "field": "words", "reproducible": true},
        ]});
        let nr = not_reproducible(&receipt);
        assert_eq!(nr.len(), 1);
        assert_eq!(nr[0]["function"], "summarise");
        assert_eq!(nr[0]["secrets"], json!(["llmKey"]));
        assert!(not_reproducible(&json!({})).is_empty());
    }

    #[test]
    fn a_signature_verifies_and_an_edited_text_does_not() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let body = json!({"format": FORMAT, "digest": "sha256:aa", "key": hex(key.verifying_key().as_bytes())});
        let text = ordo_plan::artifact::to_bytes(&body);
        let s = Signed {
            signature: hex(&key.sign(text.as_bytes()).to_bytes()),
            body: text.clone(),
        };
        assert_eq!(s.verify().unwrap(), hex(key.verifying_key().as_bytes()));
        let edited = Signed {
            body: text.replace("sha256:aa", "sha256:bb"),
            ..s.clone()
        };
        assert!(edited.verify().unwrap_err().contains("does not match"));
        // Another key named in the text: the signature is not its.
        let other = SigningKey::from_bytes(&[8; 32]);
        let swapped = ordo_plan::artifact::to_bytes(&json!({
            "format": FORMAT, "digest": "sha256:aa", "key": hex(other.verifying_key().as_bytes())
        }));
        assert!(Signed { body: swapped, ..s }.verify().is_err());
    }

    #[test]
    fn the_age_setting_reads_days_hours_minutes_and_seconds() {
        assert_eq!(parse_age("7d").unwrap(), 7 * 86_400);
        assert_eq!(parse_age("36h").unwrap(), 36 * 3_600);
        assert_eq!(parse_age("90m").unwrap(), 5_400);
        assert_eq!(parse_age("1").unwrap(), 1);
        assert!(parse_age("0d").is_err());
        assert!(parse_age("a week").unwrap_err().contains(MAX_AGE));
    }
}
