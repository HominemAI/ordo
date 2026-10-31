//! The second step, host side (`plan-second-factor.md`).
//!
//! `ordo_identity::second_step` holds the rule that says when a sign-in owes a
//! second step. This module holds what the host adds: the settings the app
//! developer sets, and the methods a person proves the step with. Each method
//! (a code from an authenticator app, a security key, a backup code) is a
//! [`SecondStepMethod`] registered on [`SecondFactors`], so the route that
//! completes a second step does not know how any of them checks.
//!
//! # Two rules that do not move
//!
//! A second factor never carries presence. Nothing in this module can put
//! presence in a token: the token a completed second step mints carries the
//! presence the **first** factor had, and none of its own.
//!
//! A second factor is not a way in alone. A method is asked only after a first
//! factor succeeded in the same ceremony.

use std::sync::Arc;

use ordo_crypto::{Anchor, KeyProvider, Purpose, SpaceId};
use ordo_identity::ceremony::{CeremonyRow, CeremonyStore};
use ordo_identity::second_step::{decide, DeploymentMode, Facts, Owes, PersonMode, TenantMode};
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use ordo_identity::{
    totp, AuthMethodKind, ChallengeId, PasskeyProvider, PrincipalId, SecondStepKind, Timestamp,
};
use serde_json::{json, Value};

use crate::second_factor_store::{Factor, SecondFactorStore};
use ordo_identity::StoreFault;

/// How the second step behaves. Every number is a default the app developer
/// can move (`docs/deploying.md`), never a constant of the system.
#[derive(Debug, Clone)]
pub struct SecondFactorConfig {
    /// `ORDO_AUTH_SECOND_FACTOR`: what the app developer sets for the
    /// deployment. A tenant can only tighten it.
    pub mode: DeploymentMode,
    /// `ORDO_AUTH_SECOND_STEP_ATTEMPTS`: wrong answers one second-step
    /// ceremony allows before it ends and the person signs in again.
    pub attempts_per_ceremony: u32,
    /// `ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER`: consecutive wrong answers, across
    /// ceremonies, that lock one factor until the person re-enrols it with a
    /// fresh first factor. Owner, 2026-09-29: 20 by default, configurable.
    pub lock_after: u32,
    /// `ORDO_AUTH_SECOND_STEP_SECS`: how long a half-signed-in state lasts,
    /// and how long an enrolment that has not been confirmed lasts.
    pub ceremony_secs: u64,
    /// `ORDO_AUTH_SECOND_FACTOR_MAX`: how many factors one person may hold.
    pub max_factors: usize,
    /// `ORDO_AUTH_REAUTH_SECS`: how long a fresh first factor stays good for
    /// changing a factor, after it was proved (`plan-second-factor.md` §7.1).
    pub reauth_secs: u64,
}

impl Default for SecondFactorConfig {
    fn default() -> SecondFactorConfig {
        SecondFactorConfig {
            mode: DeploymentMode::Enabled,
            attempts_per_ceremony: 5,
            lock_after: 20,
            ceremony_secs: 300,
            max_factors: 10,
            reauth_secs: 300,
        }
    }
}

/// Remove every second factor a person holds: each key's credential from
/// `$passkey`, then every row. Returns how many factors went.
pub fn remove_all_for(
    store: &dyn SecondFactorStore,
    passkey: Option<&Arc<PasskeyProvider>>,
    principal: &str,
) -> Result<usize, StoreFault> {
    if let Some(passkey) = passkey {
        for f in store.list(principal)? {
            if f.kind == SecondStepKind::SecurityKey.as_str() {
                if let Some(raw) = credential_of(&f) {
                    passkey
                        .remove_credential(&raw)
                        .map_err(crate::auth::store_failure::auth_fault)?;
                }
            }
        }
    }
    store.remove_all(principal)
}

/// What a recovery does to a person's second factors when it completes: every
/// one is removed (owner, 2026-09-29: *"but only after the reset"*, so a case
/// that is only opened, or contested, removes nothing).
pub struct RemoveSecondFactors {
    pub store: Arc<dyn SecondFactorStore>,
    pub passkey: Option<Arc<PasskeyProvider>>,
    pub recorder: Option<Arc<ordo_secevent::Recorder>>,
}

impl crate::recovery::Settlement for RemoveSecondFactors {
    fn name(&self) -> &'static str {
        "second-factors"
    }

    fn settle(&self, principal: &str, case: &str) -> Result<(), StoreFault> {
        let gone = remove_all_for(self.store.as_ref(), self.passkey.as_ref(), principal)?;
        if gone > 0 {
            if let Some(recorder) = &self.recorder {
                recorder.observe(
                    ordo_secevent::Observation {
                        space: None,
                        subject: ordo_secevent::Subject::member(principal),
                        op: "recovery.settle".to_string(),
                        kind: ordo_secevent::EventKind::SecondFactor {
                            act: "removed".into(),
                            detail: format!(
                                "principal={principal} count={gone} by=recovery case={case}"
                            ),
                        },
                        at: ordo_commandlog::Timestamp(crate::runtime::now_secs()),
                        correlation: ordo_secevent::correlation::mint(),
                    },
                    ordo_secevent::Relationship::None,
                );
            }
        }
        Ok(())
    }

    /// A contest removes every factor enrolled at or after `opened`, and a
    /// key's credential with it (owner ruling 2026-10-25). Completion removed
    /// every factor from before, so these are what whoever held the recovered
    /// passkey enrolled. The recovery service records each one.
    fn contest(
        &self,
        principal: &str,
        _case: &str,
        opened: u64,
    ) -> Result<Vec<(String, String)>, StoreFault> {
        let mut gone = Vec::new();
        for f in self.store.list(principal)? {
            if f.enrolled_at < opened {
                continue;
            }
            if f.kind == SecondStepKind::SecurityKey.as_str() {
                if let (Some(passkey), Some(raw)) = (&self.passkey, credential_of(&f)) {
                    passkey
                        .remove_credential(&raw)
                        .map_err(crate::auth::store_failure::auth_fault)?;
                }
            }
            if self.store.remove(&f.id, principal)?.is_some() {
                gone.push((f.kind.clone(), f.id.clone()));
            }
        }
        Ok(gone)
    }
}

/// Why a second-step setting did not load: the variable and the rule it broke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub var: &'static str,
    pub detail: String,
}

impl SecondFactorConfig {
    /// Read the settings from the environment, refusing what is unknown or
    /// out of range. **Each is refused rather than clamped**: a setting that
    /// quietly became a different value is a security setting nobody chose.
    pub fn from_env() -> Result<SecondFactorConfig, ConfigError> {
        SecondFactorConfig::from_lookup(&|k| std::env::var(k).ok())
    }

    /// The settings, read through `get` so a test can supply them.
    pub fn from_lookup(
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<SecondFactorConfig, ConfigError> {
        let d = SecondFactorConfig::default();
        let raw = |var: &'static str| {
            get(var)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let number = |var: &'static str, default: u64, floor: u64| -> Result<u64, ConfigError> {
            let Some(text) = raw(var) else {
                return Ok(default);
            };
            let n: u64 = text.parse().map_err(|_| ConfigError {
                var,
                detail: format!("`{text}` is not a number"),
            })?;
            if n < floor {
                return Err(ConfigError {
                    var,
                    detail: format!("{n} is below the floor of {floor}"),
                });
            }
            Ok(n)
        };
        let mode = match raw("ORDO_AUTH_SECOND_FACTOR") {
            None => d.mode,
            Some(word) => DeploymentMode::parse(&word).ok_or_else(|| ConfigError {
                var: "ORDO_AUTH_SECOND_FACTOR",
                detail: format!(
                    "`{word}` is not one of off, enabled, required, required-for-non-passkey"
                ),
            })?,
        };
        Ok(SecondFactorConfig {
            mode,
            attempts_per_ceremony: number(
                "ORDO_AUTH_SECOND_STEP_ATTEMPTS",
                d.attempts_per_ceremony as u64,
                1,
            )? as u32,
            lock_after: number("ORDO_AUTH_SECOND_FACTOR_LOCK_AFTER", d.lock_after as u64, 1)?
                as u32,
            ceremony_secs: number("ORDO_AUTH_SECOND_STEP_SECS", d.ceremony_secs, 30)?,
            max_factors: number("ORDO_AUTH_SECOND_FACTOR_MAX", d.max_factors as u64, 1)? as usize,
            reauth_secs: number("ORDO_AUTH_REAUTH_SECS", d.reauth_secs, 30)?,
        })
    }

    /// One line for the startup log.
    pub fn describe(&self) -> String {
        format!(
            "{}; {} wrong answers end a step, {} in a row lock a factor",
            self.mode.as_str(),
            self.attempts_per_ceremony,
            self.lock_after
        )
    }
}

/// What each tenant requires, held where the door and the sign-in gate can read
/// it without a query (`plan-second-factor.md` §5.4).
///
/// A tenant's level lives on its admission policy (`$admissionpolicy`). This is
/// the copy the hot paths read: written in this process when an owner sets the
/// level, and re-read on a timer so another replica learns of it. **The door
/// holds this one object and never a copy**, so a level set here is the level
/// the very next request meets.
pub struct SecondStepLevels {
    deployment: DeploymentMode,
    tenants: std::sync::RwLock<std::collections::BTreeMap<String, TenantMode>>,
    /// Whose two-step claims count, and where (`plan-auth-federated.md`
    /// §8.2). The same object the access engine reads.
    upstream: Option<Arc<ordo_identity::UpstreamAcceptance>>,
}

impl SecondStepLevels {
    pub fn new(deployment: DeploymentMode) -> SecondStepLevels {
        SecondStepLevels {
            deployment,
            tenants: std::sync::RwLock::new(std::collections::BTreeMap::new()),
            upstream: None,
        }
    }

    /// Let a provider's accepted two-step claim stand in for the second step
    /// (`plan-second-factor.md` §5.3, federated ask 11). Without it, none
    /// does.
    pub fn with_upstream(
        mut self,
        upstream: Option<Arc<ordo_identity::UpstreamAcceptance>>,
    ) -> SecondStepLevels {
        self.upstream = upstream;
        self
    }

    /// Which level a two-step claim from connection `via` satisfies, for a
    /// sign-in or a call in `space`. The app developer's `on` satisfies every
    /// level. A tenant's acceptance satisfies that tenant's own level and no
    /// other.
    pub fn upstream_level(&self, via: &str, space: Option<&str>) -> Option<UpstreamLevel> {
        let a = self.upstream.as_ref()?;
        if a.deployment_accepts(via) {
            return Some(UpstreamLevel::Deployment);
        }
        let space = space?;
        a.tenant_accepts(&ordo_identity::SpaceId(space.to_string()), via)
            .then_some(UpstreamLevel::Tenant)
    }

    pub fn deployment(&self) -> DeploymentMode {
        self.deployment
    }

    pub fn tenant(&self, space: &str) -> TenantMode {
        self.tenants
            .read()
            .expect("second step levels")
            .get(space)
            .copied()
            .unwrap_or_default()
    }

    pub fn set(&self, space: &str, mode: TenantMode) {
        let mut t = self.tenants.write().expect("second step levels");
        if mode == TenantMode::Inherit {
            t.remove(space);
        } else {
            t.insert(space.to_string(), mode);
        }
    }

    /// Re-read every tenant's level from `$admissionpolicy`. Returns how many
    /// tenants require anything.
    pub async fn load(&self, pool: &ordo_exec::Pool) -> Result<usize, StoreFault> {
        let rows = pool
            .rows(
                r#"SELECT "space", "second_factor" FROM "$admissionpolicy"
                   WHERE "second_factor" <> 'inherit'"#,
            )
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the tenants' second-step levels could not be read",
                )
            })?;
        let mut fresh = std::collections::BTreeMap::new();
        for r in &rows {
            let (Some(Some(space)), Some(Some(word))) = (r.first(), r.get(1)) else {
                continue;
            };
            if let Some(mode) = TenantMode::parse(word) {
                fresh.insert(space.clone(), mode);
            }
        }
        let n = fresh.len();
        *self.tenants.write().expect("second step levels") = fresh;
        Ok(n)
    }

    /// Whether the door refuses a session for `space` because it did not take a
    /// second step the deployment or the tenant requires.
    ///
    /// Only a human session is asked: a capability token and a delegated token
    /// are not sessions and are ended by withdrawing what they were minted
    /// against. **Passkey stands in for "presence in the token"** until the
    /// token names how it signed in (`plan-auth-federated.md` F5): a session
    /// with presence signed in with a passkey. The one wrong reading is a
    /// quarantined passkey's session, which has none and is refused under a
    /// `required-for-non-passkey` level. That is the safe direction.
    ///
    /// A session whose provider's two-step claim was accepted at sign-in
    /// meets the level that accepts it here too: the app developer's `on`
    /// meets every level, a tenant's acceptance meets that tenant's.
    pub fn refuses(&self, claims: &ordo_identity::TokenClaims, space: &str) -> bool {
        let Some(session) = claims.session.as_ref() else {
            return false;
        };
        if session.second_step.is_some() || self.deployment == DeploymentMode::Off {
            return false;
        }
        let upstream = session
            .upstream_presence
            .as_ref()
            .and_then(|u| self.upstream_level(u.via(), Some(space)));
        if upstream == Some(UpstreamLevel::Deployment) {
            return false;
        }
        let tenant = if upstream == Some(UpstreamLevel::Tenant) {
            TenantMode::Inherit
        } else {
            self.tenant(space)
        };
        if self.deployment == DeploymentMode::Required || tenant == TenantMode::Required {
            return true;
        }
        let non_passkey = self.deployment == DeploymentMode::RequiredForNonPasskey
            || tenant == TenantMode::RequiredForNonPasskey;
        non_passkey && claims.user_presence_at.is_none()
    }
}

/// The level a provider's two-step claim stands in for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamLevel {
    /// The app developer's `ORDO_AUTH_<NAME>_PRESENCE=on`: every level.
    Deployment,
    /// A tenant's `presenceFrom`: that tenant's level alone.
    Tenant,
}

impl UpstreamLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            UpstreamLevel::Deployment => "deployment",
            UpstreamLevel::Tenant => "tenant",
        }
    }
}

/// What a method decided about one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    /// The answer was wrong. The method has counted it against whatever it
    /// counts against, and says whether that locked the factor just now.
    Rejected {
        locked: bool,
    },
}

/// One way of proving a second step.
pub trait SecondStepMethod: Send + Sync {
    fn kind(&self) -> SecondStepKind;

    /// Whether the person holds one of this kind, **a locked one included**.
    /// A factor the throttle locked still counts: if it did not, twenty wrong
    /// codes would turn a person with a second step into a person without one,
    /// and a password alone would sign in. A locked factor refuses every
    /// answer instead ([`Verdict::Rejected`]).
    fn holds(&self, principal: &str) -> Result<bool, StoreFault>;

    /// Start a challenge the answer must be bound to, for a method that has
    /// one (a security key). `None` for a method whose answer is a code.
    fn begin(&self, _principal: &str, _now: u64) -> Result<Option<Value>, StoreFault> {
        Ok(None)
    }

    /// Check an answer. `body` is the request as sent.
    fn verify(&self, principal: &str, body: &Value, now: u64) -> Result<Verdict, StoreFault>;
}

/// The settings and the methods.
pub struct SecondFactors {
    levels: Arc<SecondStepLevels>,
    store: Option<Arc<dyn SecondFactorStore>>,
    config: SecondFactorConfig,
    methods: Vec<Arc<dyn SecondStepMethod>>,
    totp: Option<Arc<TotpMethod>>,
    backup: Option<Arc<BackupCodeMethod>>,
    key: Option<Arc<SecurityKeyMethod>>,
}

impl SecondFactors {
    pub fn new(config: SecondFactorConfig) -> SecondFactors {
        SecondFactors {
            levels: Arc::new(SecondStepLevels::new(config.mode)),
            store: None,
            config,
            methods: Vec::new(),
            totp: None,
            backup: None,
            key: None,
        }
    }

    /// Share the tenants' levels with the door and the admission op, which hold
    /// the same object.
    pub fn with_levels(mut self, levels: Arc<SecondStepLevels>) -> SecondFactors {
        self.levels = levels;
        self
    }

    /// Give the service the store a person's own level lives in.
    pub fn with_store(mut self, store: Arc<dyn SecondFactorStore>) -> SecondFactors {
        self.store = Some(store);
        self
    }

    pub fn levels(&self) -> &Arc<SecondStepLevels> {
        &self.levels
    }

    /// Every factor the person holds, locked ones included.
    pub fn factors_of(&self, principal: &str) -> Result<Vec<Factor>, StoreFault> {
        match &self.store {
            Some(s) => s.list(principal),
            None => Ok(Vec::new()),
        }
    }

    /// Remove one of the person's factors, and a key's credential with it.
    /// `None` when it was not theirs.
    pub fn remove_factor(&self, principal: &str, id: &str) -> Result<Option<Factor>, StoreFault> {
        let Some(store) = &self.store else {
            return Err("no store is wired for second factors".into());
        };
        let removed = store.remove(id, principal)?;
        if let (Some(f), Some(key)) = (&removed, &self.key) {
            if f.kind == SecondStepKind::SecurityKey.as_str() {
                key.remove_credential(f);
            }
        }
        Ok(removed)
    }

    /// Remove every factor the person holds, and every key's credential, and
    /// the lock rows. What a reset or a recovery does once it completes.
    pub fn remove_all(&self, principal: &str) -> Result<usize, StoreFault> {
        match &self.store {
            Some(s) => remove_all_for(
                s.as_ref(),
                self.key.as_ref().map(|k| k.passkey()),
                principal,
            ),
            None => Ok(0),
        }
    }

    /// What the person chose for their own sign-ins. The default for a person
    /// who chose nothing, and where no store is wired.
    pub fn person_mode(&self, principal: &str) -> Result<PersonMode, StoreFault> {
        match &self.store {
            Some(s) => s.person_mode(principal),
            None => Ok(PersonMode::default()),
        }
    }

    pub fn set_person_mode(
        &self,
        principal: &str,
        mode: PersonMode,
        at: u64,
    ) -> Result<(), StoreFault> {
        match &self.store {
            Some(s) => s.set_person_mode(principal, mode, at),
            None => Err("no store is wired for a person's second-step level".into()),
        }
    }

    pub fn with_method(mut self, method: Arc<dyn SecondStepMethod>) -> SecondFactors {
        self.methods.push(method);
        self
    }

    /// Register the code-generating method, keeping the concrete handle the
    /// enrolment routes need.
    pub fn with_totp(mut self, totp: Arc<TotpMethod>) -> SecondFactors {
        self.methods.push(totp.clone());
        self.totp = Some(totp);
        self
    }

    pub fn totp(&self) -> Option<&Arc<TotpMethod>> {
        self.totp.as_ref()
    }

    /// Register the backup-code method, keeping the concrete handle the issue
    /// route needs to clear a person's lock when a fresh set is made.
    pub fn with_backup(mut self, backup: Arc<BackupCodeMethod>) -> SecondFactors {
        self.methods.push(backup.clone());
        self.backup = Some(backup);
        self
    }

    pub fn backup(&self) -> Option<&Arc<BackupCodeMethod>> {
        self.backup.as_ref()
    }

    /// Register the security-key method, keeping the concrete handle the
    /// enrolment routes need.
    pub fn with_key(mut self, key: Arc<SecurityKeyMethod>) -> SecondFactors {
        self.methods.push(key.clone());
        self.key = Some(key);
        self
    }

    pub fn key(&self) -> Option<&Arc<SecurityKeyMethod>> {
        self.key.as_ref()
    }

    pub fn config(&self) -> &SecondFactorConfig {
        &self.config
    }

    /// Whether the person holds a second factor, a locked one included. Backup
    /// codes stand in for a factor and are not one.
    pub fn holds_any(&self, principal: &str) -> Result<bool, StoreFault> {
        for m in &self.methods {
            if m.kind() != SecondStepKind::BackupCode && m.holds(principal)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn method(&self, kind: SecondStepKind) -> Option<&Arc<dyn SecondStepMethod>> {
        self.methods.iter().find(|m| m.kind() == kind)
    }

    /// What a sign-in by `first` owes, for `principal`. `tenant` is the level
    /// the tenant named at sign-in has chosen, and `person` the person's own.
    pub fn owes(
        &self,
        principal: &str,
        first: AuthMethodKind,
        tenant: TenantMode,
        person: PersonMode,
    ) -> Result<Owes, StoreFault> {
        if self.config.mode == DeploymentMode::Off {
            return Ok(Owes::Nothing);
        }
        let mut enrolled = Vec::new();
        let mut has_backup_codes = false;
        for m in &self.methods {
            if !m.holds(principal)? {
                continue;
            }
            match m.kind() {
                SecondStepKind::BackupCode => has_backup_codes = true,
                other => enrolled.push(other),
            }
        }
        Ok(decide(&Facts {
            deployment: self.config.mode,
            tenant,
            person,
            first,
            enrolled: &enrolled,
            has_backup_codes,
        }))
    }
}

// ── sealing ──────────────────────────────────────────────────────────────

/// Seals a code-generating factor's secret under the deployment's key custody.
///
/// **The plan was wrong** that `KeyProvider` takes a purpose string: `Purpose`
/// is a closed set (`read`, `write`, `reseal`) and every release names a
/// space. The deployment's own space, `provision::SYSTEM_SPACE`, is the
/// deployment-scoped home the platform already keeps custody for, so the
/// secret is sealed there, at the row of its factor, and it opens nowhere else.
pub struct Sealer {
    keys: Arc<dyn KeyProvider>,
}

impl Sealer {
    pub fn new(keys: Arc<dyn KeyProvider>) -> Sealer {
        Sealer { keys }
    }

    fn space() -> SpaceId {
        SpaceId(crate::provision::SYSTEM_SPACE.to_string())
    }

    /// The sealed secret as hex, bound to the factor `id`.
    pub fn seal(&self, id: &str, plain: &[u8]) -> Result<String, StoreFault> {
        let lease = self
            .keys
            .release(
                &Sealer::space(),
                Purpose::Write,
                &format!("second-factor:{id}"),
            )
            .map_err(|_| StoreFault::unavailable("the key custody could not release a key"))?;
        let sealed = self
            .keys
            .seal_at(&lease, "$secondfactor", "secret", Anchor::Row(id), plain)
            .map_err(|_| StoreFault::unavailable("the secret could not be sealed"))?;
        Ok(sealed.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn open(&self, id: &str, hex: &str) -> Result<Vec<u8>, StoreFault> {
        let bytes: Option<Vec<u8>> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok())
            .collect();
        let bytes = bytes.ok_or("a sealed secret is not hex")?;
        let lease = self
            .keys
            .release(
                &Sealer::space(),
                Purpose::Read,
                &format!("second-factor:{id}"),
            )
            .map_err(|_| StoreFault::unavailable("the key custody could not release a key"))?;
        self.keys
            .open_at(&lease, "$secondfactor", "secret", Anchor::Row(id), &bytes)
            .map_err(|_| StoreFault::unavailable("the secret could not be opened"))
    }
}

// ── code-generating tokens ───────────────────────────────────────────────

/// A code from an authenticator app or a code-generating token (RFC 6238).
pub struct TotpMethod {
    store: Arc<dyn SecondFactorStore>,
    sealer: Sealer,
    config: SecondFactorConfig,
}

/// A begun enrolment, for the person's app.
pub struct BegunTotp {
    pub factor: String,
    pub secret_base32: String,
    pub uri: String,
    pub expires_at: u64,
}

/// Why a confirmation did not enrol.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfirmError {
    /// Nothing to confirm: unknown, expired, another person's, or spent.
    Refused(&'static str),
    /// A wrong code. The enrolment stays open until its attempts run out.
    Wrong,
    /// A store did not answer: the word for the stream, and whether it was
    /// busy (`crate::auth::StoreFailure`).
    Unavailable(&'static str, crate::auth::StoreFailure),
}

/// A confirmed enrolment.
pub struct Enrolled {
    pub factor: Factor,
}

const ENROL_CONNECTION: &str = "totp-enrol";
const ENROL_PHASE: &str = "pending";

fn sha_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"ordo.totpenrol.v1\0");
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

impl TotpMethod {
    pub fn new(
        store: Arc<dyn SecondFactorStore>,
        sealer: Sealer,
        config: SecondFactorConfig,
    ) -> TotpMethod {
        TotpMethod {
            store,
            sealer,
            config,
        }
    }

    fn held(&self, principal: &str) -> Result<Vec<Factor>, StoreFault> {
        Ok(self
            .store
            .list(principal)?
            .into_iter()
            .filter(|f| f.kind == SecondStepKind::Totp.as_str())
            .collect())
    }

    /// Start an enrolment: a fresh secret, sealed into a ceremony row that
    /// expires. **Nothing is enrolled** until a code from the person's app
    /// proves it holds the secret ([`TotpMethod::confirm_enrol`]).
    pub fn begin_enrol(
        &self,
        ceremonies: &dyn CeremonyStore,
        principal: &str,
        label: &str,
        issuer: &str,
        account: &str,
        now: u64,
    ) -> Result<BegunTotp, StoreFault> {
        if self.store.list(principal)?.len() >= self.config.max_factors {
            return Err("the person holds as many factors as are allowed".into());
        }
        let id = format!("sf_{}", ordo_identity::directory::random_principal_suffix());
        let secret = totp::generate_secret();
        let sealed = self.sealer.seal(&id, &secret)?;
        let expires_at = now.saturating_add(self.config.ceremony_secs);
        let payload = json!({
            "principal": principal, "label": label, "secret": sealed,
            "attempts_left": self.config.attempts_per_ceremony,
            "minted_at": now, "expires_at": expires_at,
        })
        .to_string();
        ceremonies
            .put(&CeremonyRow {
                handle: sha_hex(&id),
                connection: ENROL_CONNECTION.into(),
                phase: ENROL_PHASE.into(),
                binder: String::new(),
                payload,
                minted_at: Timestamp(now),
                expires_at: Timestamp(expires_at),
            })
            .map_err(|e| e.0)?;
        Ok(BegunTotp {
            factor: id,
            secret_base32: totp::base32(&secret),
            uri: totp::otpauth_uri(issuer, account, &secret),
            expires_at,
        })
    }

    /// Finish an enrolment: one code from the person's app, against the secret
    /// the begin sealed. Only a right code writes the factor, and it stores the
    /// step it proved so the same code cannot then be used to sign in.
    pub fn confirm_enrol(
        &self,
        ceremonies: &dyn CeremonyStore,
        principal: &str,
        factor: &str,
        code: &str,
        now: u64,
    ) -> Result<Enrolled, ConfirmError> {
        let key = sha_hex(factor);
        let taken = ceremonies
            .take(&key, ENROL_CONNECTION, ENROL_PHASE, Timestamp(now))
            .map_err(|e| ConfirmError::Unavailable("ceremony-store-unavailable", e.into()))?;
        let Some(text) = taken else {
            return Err(ConfirmError::Refused("enrolment-unknown"));
        };
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| ConfirmError::Refused("enrolment-malformed"))?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64());
        let (Some(owner), Some(label), Some(sealed), Some(attempts), Some(minted), Some(expires)) = (
            s("principal"),
            s("label"),
            s("secret"),
            n("attempts_left"),
            n("minted_at"),
            n("expires_at"),
        ) else {
            return Err(ConfirmError::Refused("enrolment-malformed"));
        };
        let put_back = |attempts_left: u64| {
            let mut v2 = v.clone();
            v2["attempts_left"] = json!(attempts_left);
            let _ = ceremonies.put(&CeremonyRow {
                handle: key.clone(),
                connection: ENROL_CONNECTION.into(),
                phase: ENROL_PHASE.into(),
                binder: String::new(),
                payload: v2.to_string(),
                minted_at: Timestamp(minted),
                expires_at: Timestamp(expires),
            });
        };
        if owner != principal {
            // Another person's enrolment is not theirs to confirm, and not
            // theirs to spend.
            put_back(attempts);
            return Err(ConfirmError::Refused("enrolment-not-theirs"));
        }
        let secret = self
            .sealer
            .open(factor, &sealed)
            .map_err(|e| ConfirmError::Unavailable("key-custody-unavailable", e.into()))?;
        match totp::matches(&secret, code, now, 0) {
            Some(step) => {
                let f = Factor {
                    id: factor.to_string(),
                    principal: principal.to_string(),
                    kind: SecondStepKind::Totp.as_str().to_string(),
                    label,
                    secret: sealed,
                    last_counter: step,
                    failures: 0,
                    disabled_at: 0,
                    enrolled_at: now,
                    last_used_at: 0,
                };
                self.store.insert(&f).map_err(|e| {
                    ConfirmError::Unavailable("second-factor-store-unavailable", e.into())
                })?;
                Ok(Enrolled { factor: f })
            }
            None => {
                if attempts > 1 {
                    put_back(attempts - 1);
                }
                Err(ConfirmError::Wrong)
            }
        }
    }
}

impl SecondStepMethod for TotpMethod {
    fn kind(&self) -> SecondStepKind {
        SecondStepKind::Totp
    }

    fn holds(&self, principal: &str) -> Result<bool, StoreFault> {
        Ok(!self.held(principal)?.is_empty())
    }

    /// Every unlocked code-generating factor the person holds is tried. A code
    /// that matches a step later than the last one accepted wins, and a code
    /// that was already used does not. A wrong answer counts against each
    /// factor tried, and the one that reaches the limit locks.
    fn verify(&self, principal: &str, body: &Value, now: u64) -> Result<Verdict, StoreFault> {
        let code = body.get("code").and_then(|c| c.as_str()).unwrap_or("");
        let live: Vec<Factor> = self
            .held(principal)?
            .into_iter()
            .filter(|f| !f.locked())
            .collect();
        for f in &live {
            let secret = self.sealer.open(&f.id, &f.secret)?;
            if let Some(step) = totp::matches(&secret, code, now, f.last_counter) {
                if self.store.accept_counter(&f.id, step, now)? {
                    return Ok(Verdict::Accepted);
                }
            }
        }
        let mut locked = false;
        for f in &live {
            let (n, is_locked) =
                self.store
                    .note_failure(&f.id, self.config.lock_after as u64, now)?;
            locked |= is_locked && n >= self.config.lock_after as u64;
        }
        Ok(Verdict::Rejected { locked })
    }
}

// ── backup codes ─────────────────────────────────────────────────────────

/// One of the person's one-time backup codes, used as a second step
/// (`plan-second-factor.md` §6.4, use A).
///
/// It is the same set recovery uses (`recovery_codes`). What differs is what a
/// code may do. Here it stands in for a lost second factor and is asked only
/// after a first factor succeeded in the same ceremony: one factor plus one
/// factor, which is what NIST SP 800-63B counts as two. Alone, a code opens a
/// recovery and nothing else (`/auth/recover/code`), and it is not a way in.
///
/// A code is spent as the person's own: another person's code is not accepted
/// and not consumed. Wrong codes are counted **per person** on a lock row in
/// `$secondfactor`, and at the limit the person's code redemption is locked
/// until a fresh set is issued.
pub struct BackupCodeMethod {
    codes: Arc<crate::recovery_codes::RecoveryCodes>,
    store: Arc<dyn SecondFactorStore>,
    lock_after: u64,
}

impl BackupCodeMethod {
    pub fn new(
        codes: Arc<crate::recovery_codes::RecoveryCodes>,
        store: Arc<dyn SecondFactorStore>,
        config: &SecondFactorConfig,
    ) -> BackupCodeMethod {
        BackupCodeMethod {
            codes,
            store,
            lock_after: config.lock_after as u64,
        }
    }

    /// A fresh set unlocks the person's code redemption.
    pub fn clear_lock(&self, principal: &str) -> Result<(), StoreFault> {
        self.store
            .clear_lock(&crate::second_factor_store::lock_row_id(principal))
    }

    /// How many unused codes the person holds.
    pub fn remaining(&self, principal: &str) -> Result<usize, StoreFault> {
        self.codes.remaining(principal)
    }
}

impl SecondStepMethod for BackupCodeMethod {
    fn kind(&self) -> SecondStepKind {
        SecondStepKind::BackupCode
    }

    fn holds(&self, principal: &str) -> Result<bool, StoreFault> {
        Ok(self.codes.remaining(principal)? > 0)
    }

    fn verify(&self, principal: &str, body: &Value, now: u64) -> Result<Verdict, StoreFault> {
        let typed = body.get("code").and_then(|c| c.as_str()).unwrap_or("");
        let lock = crate::second_factor_store::lock_row_id(principal);
        self.store.ensure_lock_row(principal, now)?;
        if self.store.get(&lock)?.is_some_and(|f| f.locked()) {
            return Ok(Verdict::Rejected { locked: false });
        }
        match self.codes.consume_for(principal, typed)? {
            Some(_) => {
                self.store.note_success(&lock, now)?;
                Ok(Verdict::Accepted)
            }
            None => {
                let (n, locked) = self.store.note_failure(&lock, self.lock_after, now)?;
                Ok(Verdict::Rejected {
                    locked: locked && n >= self.lock_after,
                })
            }
        }
    }
}

// ── security keys ────────────────────────────────────────────────────────

/// A WebAuthn security key registered as a second factor
/// (`plan-second-factor.md` §4).
///
/// It is verified by the same code as a passkey and it is not one. Its
/// credential is stored with the role `second-factor`, so it cannot sign in
/// alone (the sign-in assertion refuses it as a bad credential) and it cannot
/// step up. It is linked in `$secondfactor` and never in `$authenticator`, so
/// the directory's link count, the last-authenticator rule and onboarding never
/// see it. Nothing it proves is presence: [`PasskeyProvider::complete_second_step`]
/// returns a proof with no presence in it.
///
/// Registered **without attestation** (owner, 2026-09-29): the person labels
/// the key, and the record says *security key* and nothing about a model or
/// whether it is hardware.
pub struct SecurityKeyMethod {
    store: Arc<dyn SecondFactorStore>,
    passkey: Arc<PasskeyProvider>,
    config: SecondFactorConfig,
}

/// A key registered as a second factor.
pub struct EnrolledKey {
    pub factor: Factor,
    /// Whether the key verified the user (a PIN or a biometric). Recorded,
    /// because many keys have none and are accepted anyway.
    pub user_verified: bool,
}

/// Why a key did not enrol.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyEnrolError {
    Refused(&'static str),
    /// As [`ConfirmError::Unavailable`].
    Unavailable(&'static str, crate::auth::StoreFailure),
}

fn credential_of(factor: &Factor) -> Option<Vec<u8>> {
    factor
        .secret
        .strip_prefix("passkey:")
        .and_then(|id| b64url_decode(id).ok())
}

impl SecurityKeyMethod {
    pub fn new(
        store: Arc<dyn SecondFactorStore>,
        passkey: Arc<PasskeyProvider>,
        config: SecondFactorConfig,
    ) -> SecurityKeyMethod {
        SecurityKeyMethod {
            store,
            passkey,
            config,
        }
    }

    fn held(&self, principal: &str) -> Result<Vec<Factor>, StoreFault> {
        Ok(self
            .store
            .list(principal)?
            .into_iter()
            .filter(|f| f.kind == SecondStepKind::SecurityKey.as_str())
            .collect())
    }

    /// Start registering a key: creation options for a credential that need not
    /// be discoverable, excluding the keys the person already holds.
    pub fn begin_enrol(
        &self,
        principal: &str,
        now: u64,
    ) -> Result<ordo_identity::CreationOptions, StoreFault> {
        if self.store.list(principal)?.len() >= self.config.max_factors {
            return Err("the person holds as many factors as are allowed".into());
        }
        let exclude: Vec<Vec<u8>> = self
            .held(principal)?
            .iter()
            .filter_map(credential_of)
            .collect();
        self.passkey
            .begin_second_factor_registration(
                Timestamp(now),
                &PrincipalId(principal.to_string()),
                &exclude,
            )
            .map_err(crate::auth::store_failure::auth_fault)
    }

    /// Finish registering a key for `principal`: verify the registration,
    /// store the credential with its second-factor role, and write the factor.
    /// A ceremony begun for another person is refused and stores nothing.
    pub fn complete_enrol(
        &self,
        principal: &str,
        challenge: &ChallengeId,
        client_data_json: &[u8],
        attestation_object: &[u8],
        label: &str,
        now: u64,
    ) -> Result<EnrolledKey, KeyEnrolError> {
        let (pending, begun_for) = self
            .passkey
            .verify_second_factor_registration(
                challenge,
                client_data_json,
                attestation_object,
                Timestamp(now),
            )
            .map_err(|e| match e {
                ordo_identity::AuthError::StoreUnavailable(c) => {
                    KeyEnrolError::Unavailable("credential-store-unavailable", c.into())
                }
                ordo_identity::AuthError::CredentialAlreadyEnrolled => {
                    KeyEnrolError::Refused("credential-already-enrolled")
                }
                _ => KeyEnrolError::Refused("registration-refused"),
            })?;
        if begun_for.0 != principal {
            return Err(KeyEnrolError::Refused("enrolment-not-theirs"));
        }
        self.passkey
            .store_registration(&pending)
            .map_err(|e| match e {
                ordo_identity::AuthError::StoreUnavailable(c) => {
                    KeyEnrolError::Unavailable("credential-store-unavailable", c.into())
                }
                _ => KeyEnrolError::Refused("credential-already-enrolled"),
            })?;
        let id = format!("sf_{}", ordo_identity::directory::random_principal_suffix());
        let factor = Factor {
            id,
            principal: principal.to_string(),
            kind: SecondStepKind::SecurityKey.as_str().to_string(),
            label: label.to_string(),
            secret: pending.subject().authenticator.0.clone(),
            last_counter: 0,
            failures: 0,
            disabled_at: 0,
            enrolled_at: now,
            last_used_at: 0,
        };
        if let Err(e) = self.store.insert(&factor) {
            // A credential nobody can answer with and nobody removes is what
            // the ordering is built to avoid.
            let _ = self
                .passkey
                .remove_credential(pending.stored().credential.id());
            return Err(KeyEnrolError::Unavailable(
                "second-factor-store-unavailable",
                e.into(),
            ));
        }
        Ok(EnrolledKey {
            factor,
            user_verified: pending.user_verified(),
        })
    }

    /// The passkey provider the keys are verified by.
    pub fn passkey(&self) -> &Arc<PasskeyProvider> {
        &self.passkey
    }

    /// Remove a key's credential along with its factor row.
    pub fn remove_credential(&self, factor: &Factor) {
        if let Some(raw) = credential_of(factor) {
            let _ = self.passkey.remove_credential(&raw);
        }
    }
}

impl SecondStepMethod for SecurityKeyMethod {
    fn kind(&self) -> SecondStepKind {
        SecondStepKind::SecurityKey
    }

    fn holds(&self, principal: &str) -> Result<bool, StoreFault> {
        Ok(!self.held(principal)?.is_empty())
    }

    /// The assertion options, narrowed to the person's own unlocked keys.
    fn begin(&self, principal: &str, now: u64) -> Result<Option<Value>, StoreFault> {
        let allow: Vec<Vec<u8>> = self
            .held(principal)?
            .iter()
            .filter(|f| !f.locked())
            .filter_map(credential_of)
            .collect();
        if allow.is_empty() {
            return Ok(None);
        }
        let options = self
            .passkey
            .begin_second_step(Timestamp(now), &PrincipalId(principal.to_string()), &allow)
            .map_err(crate::auth::store_failure::auth_fault)?;
        Ok(Some(json!({
            "challenge": options.challenge,
            "method": "passkey",
            "rp_id": options.rp_id,
            "user_verification": options.user_verification,
            "allow_credentials": options.allow_credentials,
            "timeout_ms": self.passkey.challenge_ttl_secs().saturating_mul(1000),
        })))
    }

    fn verify(&self, principal: &str, body: &Value, now: u64) -> Result<Verdict, StoreFault> {
        let bytes = |k: &str| {
            body.get(k)
                .and_then(|v| v.as_str())
                .and_then(|v| b64url_decode(v).ok())
        };
        let (Some(challenge), Some(credential), Some(cdj), Some(ad), Some(sig)) = (
            body.get("challenge").and_then(|v| v.as_str()),
            bytes("credential_id"),
            bytes("client_data_json"),
            bytes("authenticator_data"),
            bytes("signature"),
        ) else {
            return Ok(Verdict::Rejected { locked: false });
        };
        let live: Vec<Factor> = self
            .held(principal)?
            .into_iter()
            .filter(|f| !f.locked())
            .collect();
        let presented = format!("passkey:{}", b64url_encode(&credential));
        let proof = self.passkey.complete_second_step(
            &ChallengeId(challenge.to_string()),
            &credential,
            &cdj,
            &ad,
            &sig,
            Timestamp(now),
        );
        match proof {
            Ok(p) if p.principal.0 == principal => {
                if let Some(f) = live.iter().find(|f| f.secret == p.authenticator.0) {
                    self.store.note_success(&f.id, now)?;
                    return Ok(Verdict::Accepted);
                }
            }
            Err(ordo_identity::AuthError::StoreUnavailable(e)) => return Err(e.0),
            _ => {}
        }
        // A wrong answer counts against the key it named, or, when it named
        // none of theirs, against each key they hold.
        let named: Vec<&Factor> = live.iter().filter(|f| f.secret == presented).collect();
        let targets: Vec<&Factor> = if named.is_empty() {
            live.iter().collect()
        } else {
            named
        };
        let mut locked = false;
        for f in targets {
            let (n, is_locked) =
                self.store
                    .note_failure(&f.id, self.config.lock_after as u64, now)?;
            locked |= is_locked && n >= self.config.lock_after as u64;
        }
        Ok(Verdict::Rejected { locked })
    }
}
