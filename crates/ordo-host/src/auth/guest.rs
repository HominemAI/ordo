//! Guest sign-in (`plan-auth-federated.md` §7A, step 8).
//!
//! Ordo provides the mechanism; whether an app shows it is the app
//! developer's choice (owner, federated ask 3). Off by default.
//!
//! - `POST /auth/complete {method: "guest", kind: "new", human?, space?}`
//!   makes a guest: a principal with no email and no provider, a 256-bit
//!   guest secret, and a session. The secret is returned once, as
//!   `guest_secret`. The page keeps it.
//! - `POST /auth/complete {method: "guest", kind: "resume", secret}` signs
//!   the guest in again. The authenticator is `guest:<SHA-256 of the
//!   secret>`, so the secret itself is never stored.
//! - A guest becomes a full account by a **claim** (step 7,
//!   [`super::claim`]): it opens the window with a `resume`, then completes
//!   Discord, an emailed code or a new passkey. It keeps its id, its seats
//!   and its data.
//!
//! # What a new guest goes through
//!
//! The same as any first sign-in (a login method is a login method):
//!
//! 1. The setting. Off refuses with `403 guests_off`, naming
//!    `ORDO_AUTH_GUEST` and the value `on`.
//! 2. The human check, when it is on (E15).
//! 3. The sign-up ceiling (`ORDO_PROVISION_SIGNUPS_PER_HOUR`). A new guest
//!    is counted whatever the self-serve mode, because nothing else bounds
//!    it: no limit reads a visitor's address.
//! 4. The deployment's provisioning, an invitation, admission to a named
//!    `space`, and the self-serve hook, all in `/auth/complete` as for anyone.
//!
//! A `resume` is not a sign-up. It is refused for a secret nobody holds, so
//! it can never mint a principal, and the directory refuses a deleted guest
//! (§2.7 D6). Switching guests off stops new guests only: a guest who exists
//! keeps its way in, as any account keeps its sign-in.
//!
//! # The setting
//!
//! `ORDO_AUTH_GUEST` is `off` (the default) or `on`, stored in
//! `$deploymentsetting` on the self-serve pattern: the environment seeds it
//! on a boot that finds nothing stored, `std.deployment.setGuest` changes it,
//! and the stored value wins after that.
//!
//! `ORDO_AUTH_GUEST_TTL_SECS` is how long a guest that claimed nothing and
//! that no tenant admitted is kept. Unset, it is the registration lifetime
//! (`ORDO_AUTH_REGISTRATION_TTL_SECS`, 90 days). See
//! [`crate::registrations::sweep_guests`].

use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_api::{EdgeCode, EdgeError};
use ordo_exec::{sql_lit, Pool};
use ordo_identity::federation::{handle_of, random_secret};
use ordo_identity::{
    AuthError, AuthMethodKind, AuthProvider, AuthenticatedSubject, AuthenticatorId, BeginRequest,
    ChallengeOrRedirect, CompleteRequest, CredentialStoreError, DirectoryStore, SubjectAttributes,
    Timestamp,
};
use serde_json::{json, Value};

use super::store_failure::IntoFault;
use super::AuthState;
use ordo_identity::StoreFault;

/// The method word, in `/auth/complete` and in the registry.
pub const METHOD: &str = "guest";
/// The setting that turns guest sign-in on.
pub const SETTING: &str = "ORDO_AUTH_GUEST";
/// How long an unclaimed guest no tenant admitted is kept.
pub const TTL_SETTING: &str = "ORDO_AUTH_GUEST_TTL_SECS";
/// The op that changes the stored setting.
pub const SET_GUEST: &str = "std.deployment.setGuest";
/// Every op of this module, for registration.
pub const OPS: [&str; 1] = [SET_GUEST];

pub(crate) const KEY: &str = "auth_guest";
const OP: &str = "auth.complete";
/// A guest secret is 32 bytes, base64url without padding.
const SECRET_LEN: usize = 43;

/// Whether a new guest may be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuestMode {
    #[default]
    Off,
    On,
}

impl GuestMode {
    pub fn word(self) -> &'static str {
        match self {
            GuestMode::Off => "off",
            GuestMode::On => "on",
        }
    }

    pub fn parse(s: &str) -> Result<GuestMode, String> {
        match s.trim() {
            "off" => Ok(GuestMode::Off),
            "on" => Ok(GuestMode::On),
            other => Err(format!(
                "`{other}` is not `on` or `off`; unset it for the default, `off`"
            )),
        }
    }
}

/// What the environment says at boot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GuestConfig {
    /// `ORDO_AUTH_GUEST`. `None` seeds nothing.
    pub seed: Option<GuestMode>,
    /// `ORDO_AUTH_GUEST_TTL_SECS`. `None` is the registration lifetime.
    pub ttl_secs: Option<u64>,
}

impl GuestConfig {
    /// Read the two variables. A refusal names the variable.
    pub fn from_vars(
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<GuestConfig, (&'static str, String)> {
        let set = |k: &str| env(k).filter(|v| !v.trim().is_empty());
        let seed = set(SETTING)
            .map(|v| GuestMode::parse(&v))
            .transpose()
            .map_err(|e| (SETTING, e))?;
        let ttl_secs = match set(TTL_SETTING) {
            None => None,
            Some(raw) => match raw.trim().parse::<u64>() {
                Ok(n) if n > 0 => Some(n),
                _ => {
                    return Err((
                        TTL_SETTING,
                        format!(
                            "`{raw}` is not a number of seconds above zero; unset it to keep \
                             guests as long as any registration"
                        ),
                    ))
                }
            },
        };
        Ok(GuestConfig { seed, ttl_secs })
    }
}

/// The stored setting, over `$deploymentsetting`.
#[derive(Clone)]
pub struct GuestStore {
    pool: Pool,
}

impl GuestStore {
    pub fn new(pool: Pool) -> GuestStore {
        GuestStore { pool }
    }

    /// Make the settings table if it is missing.
    pub async fn ensure_table(&self) -> Result<(), StoreFault> {
        self.pool
            .rows(crate::admission::SETTING_DDL)
            .await
            .map(|_| ())
            .map_err(IntoFault::fault)
    }

    /// The setting in force. A stored value that no longer parses reads as
    /// `off`, the safe direction.
    pub async fn get(&self) -> Result<GuestMode, StoreFault> {
        Ok(self
            .stored()
            .await?
            .map(|v| GuestMode::parse(&v).unwrap_or_default())
            .unwrap_or_default())
    }

    async fn stored(&self) -> Result<Option<String>, StoreFault> {
        let rows = self
            .pool
            .rows(&format!(
                r#"SELECT "value" FROM "$deploymentsetting" WHERE "key" = {}"#,
                sql_lit(KEY)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(e, "the guest setting could not be read")
            })?;
        Ok(rows.first().and_then(|r| r[0].clone()))
    }

    /// Write the setting.
    pub async fn set(&self, mode: GuestMode, by: &str, at: u64) -> Result<(), StoreFault> {
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$deploymentsetting" ("key", "value", "updated_by", "updated_at")
                   VALUES ({}, {}, {}, {at})
                   ON CONFLICT ("key") DO UPDATE SET "value" = EXCLUDED."value",
                     "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at""#,
                sql_lit(KEY),
                sql_lit(mode.word()),
                sql_lit(by),
            ))
            .await
            .map(|_| ())
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(e, "the guest setting could not be written")
            })
    }

    /// What a boot does with the environment: seed when nothing is stored,
    /// and name a disagreement, which loses. Returns the lines to print.
    pub async fn boot(&self, env: &GuestConfig, at: u64) -> Result<Vec<String>, StoreFault> {
        self.ensure_table().await?;
        let Some(wanted) = env.seed else {
            return Ok(Vec::new());
        };
        match self.stored().await? {
            None => {
                self.set(wanted, "environment", at).await?;
                Ok(Vec::new())
            }
            Some(now) if now != wanted.word() => Ok(vec![format!(
                "{SETTING} disagrees with the stored setting, which is `{now}` and wins; change \
                 it with {SET_GUEST}"
            )]),
            Some(_) => Ok(Vec::new()),
        }
    }
}

/// Where `/auth/complete` reads the setting from.
pub struct Guests {
    source: Source,
}

enum Source {
    Fixed(GuestMode),
    Stored(GuestStore),
}

impl Guests {
    /// A setting that does not change: a test, or an assembly with no store.
    pub fn fixed(mode: GuestMode) -> Guests {
        Guests {
            source: Source::Fixed(mode),
        }
    }

    /// The stored setting, read at each new guest.
    pub fn stored(store: GuestStore) -> Guests {
        Guests {
            source: Source::Stored(store),
        }
    }

    /// The setting in force.
    pub async fn mode(&self) -> Result<GuestMode, StoreFault> {
        match &self.source {
            Source::Fixed(m) => Ok(*m),
            Source::Stored(s) => s.get().await,
        }
    }
}

/// The authenticator a guest secret names.
pub fn authenticator_of(secret: &str) -> AuthenticatorId {
    AuthenticatorId(format!("{METHOD}:{}", handle_of(secret)))
}

/// Does this authenticator belong to a guest?
pub fn is_guest(authenticator: &AuthenticatorId) -> bool {
    authenticator
        .0
        .strip_prefix(METHOD)
        .is_some_and(|rest| rest.starts_with(':'))
}

fn subject(authenticator: AuthenticatorId) -> AuthenticatedSubject {
    AuthenticatedSubject {
        authenticator,
        method: AuthMethodKind::Guest,
        attributes: SubjectAttributes::default(),
        presence: None,
        upstream_presence: None,
    }
}

/// The registry's guest method. A `resume` answers a secret the directory
/// already holds and refuses every other, so it never makes a principal. A
/// `GuestNew` answers only a secret nobody holds; it is built by [`request`]
/// from what [`new_guest`] minted, after the setting, the human check and the
/// ceiling, and never from a request body.
pub struct GuestProvider {
    directory: Arc<dyn DirectoryStore>,
}

impl GuestProvider {
    pub fn new(directory: Arc<dyn DirectoryStore>) -> GuestProvider {
        GuestProvider { directory }
    }
}

impl AuthProvider for GuestProvider {
    fn kind(&self) -> AuthMethodKind {
        AuthMethodKind::Guest
    }

    /// A guest has no `begin`: `new` and `resume` are both one request.
    fn begin(
        &self,
        _req: &BeginRequest,
        _now: Timestamp,
    ) -> Result<ChallengeOrRedirect, AuthError> {
        Err(AuthError::MethodMismatch {
            provider: AuthMethodKind::Guest,
        })
    }

    fn complete(
        &self,
        resp: &CompleteRequest,
        _now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        let (secret, new) = match resp {
            CompleteRequest::GuestResume { secret } => (secret, false),
            CompleteRequest::GuestNew { secret } => (secret, true),
            _ => {
                return Err(AuthError::MethodMismatch {
                    provider: AuthMethodKind::Guest,
                })
            }
        };
        if secret.len() != SECRET_LEN {
            return Err(AuthError::BadCredentials);
        }
        let id = authenticator_of(secret);
        match self.directory.principal_for(&id) {
            Ok(known) if known.is_some() != new => Ok(subject(id)),
            Ok(_) => Err(AuthError::BadCredentials),
            Err(e) => Err(AuthError::StoreUnavailable(CredentialStoreError(e.0))),
        }
    }
}

fn guests_off(auth: &AuthState, now: Timestamp) -> Response {
    auth.record_refusal(OP, "guests-off: set ORDO_AUTH_GUEST=on to allow them", now);
    EdgeError::new(
        EdgeCode::GuestsOff,
        "guest sign-in is switched off on this deployment; set ORDO_AUTH_GUEST=on (or std.deployment.setGuest) to allow it",
    )
    .with("setting", SETTING)
    .with("value", "on")
    .into_response()
}

/// The ceremony for a new guest's secret, or `None` when there is none.
pub(super) fn request(secret: Option<&str>) -> Option<CompleteRequest> {
    secret.map(|s| CompleteRequest::GuestNew {
        secret: s.to_string(),
    })
}

/// `/auth/complete` for `{method: "guest", kind: "new"}`: the new guest's
/// secret, which the answer carries once. `Ok(None)` for any other body.
/// Runs the setting, the human check and the ceiling, in that order, before
/// anything is minted.
pub(super) async fn new_guest(
    auth: &AuthState,
    body: &Value,
    now: Timestamp,
) -> Result<Option<String>, Box<Response>> {
    let is_new = body.get("method").and_then(Value::as_str) == Some(METHOD)
        && body.get("kind").and_then(Value::as_str) == Some("new");
    if !is_new {
        return Ok(None);
    }
    let mode = match auth.guests() {
        None => GuestMode::Off,
        Some(g) => match g.mode().await {
            Ok(m) => m,
            Err(e) => {
                auth.record_refusal(OP, "guest-setting-unreadable", now);
                return Err(Box::new(super::StoreFailure::from(e).into_response()));
            }
        },
    };
    if mode == GuestMode::Off {
        return Err(Box::new(guests_off(auth, now)));
    }
    auth.check_human(body, now).await?;
    if let Some(signup) = auth.signup() {
        match signup.admit_guest(now.0).await {
            Ok(()) => {}
            Err(crate::self_serve::Refusal::DeploymentFull) => {
                auth.record_refusal(OP, &super::signup::limit_reason(), now);
                return Err(Box::new(super::signup::limit_reached()));
            }
            Err(crate::self_serve::Refusal::Unavailable(failure)) => {
                auth.record_refusal(OP, "signup-limit-unreadable", now);
                return Err(Box::new(failure.into_response()));
            }
        }
    }
    Ok(Some(random_secret()))
}

/// The ops' state: the store and the deployment's own space, the only one
/// the setting answers in.
pub struct GuestOps {
    pub store: GuestStore,
    pub home: String,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

impl GuestOps {
    /// `std.deployment.setGuest($mode?)`. Without `mode` it answers the
    /// setting and changes nothing.
    pub async fn execute(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if space != self.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "guest sign-in is the deployment's setting and this op answers only in the \
                 deployment's own space",
            ));
        }
        let mode = bound
            .0
            .get("mode")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(GuestMode::parse)
            .transpose()
            .map_err(|e| ExecError::new(ErrorCode::TenantRefused, e))?;
        let now = (self.now)();
        let unavailable = |e: StoreFault| ExecError::from(e);
        if let Some(m) = mode {
            self.store.set(m, caller, now).await.map_err(unavailable)?;
            if let Some(stream) = &self.stream {
                crate::enrolment::record(
                    stream,
                    &crate::enrolment::Act {
                        act: "guest-set",
                        detail: format!("mode={}", m.word()),
                        by: Some(caller),
                        tenant: Some(space),
                        deployment: true,
                        at: now,
                    },
                );
            }
        }
        let m = self.store.get().await.map_err(unavailable)?;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({"mode": m.word()})],
            next_cursor: None,
        })
    }
}

/// The boot line when guests are on and the human check is off: one script
/// can then spend the whole sign-up ceiling (§7A).
pub fn unguarded_line(mode: GuestMode, human_check: bool) -> Option<String> {
    (mode == GuestMode::On && !human_check).then(|| {
        format!(
            "{SETTING}=on with no human check: a script can make guests up to the sign-up \
             ceiling; set ORDO_AUTH_HUMAN_CHECK=turnstile to require one"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_takes_on_and_off_and_names_itself_otherwise() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            GuestConfig::from_vars(&env(&[])),
            Ok(GuestConfig::default())
        );
        assert_eq!(
            GuestConfig::from_vars(&env(&[(SETTING, "on"), (TTL_SETTING, "3600")])),
            Ok(GuestConfig {
                seed: Some(GuestMode::On),
                ttl_secs: Some(3600)
            })
        );
        assert_eq!(
            GuestConfig::from_vars(&env(&[(SETTING, "yes")]))
                .unwrap_err()
                .0,
            SETTING
        );
        assert_eq!(
            GuestConfig::from_vars(&env(&[(TTL_SETTING, "0")]))
                .unwrap_err()
                .0,
            TTL_SETTING
        );
    }

    #[test]
    fn a_guest_authenticator_holds_a_hash_and_never_the_secret() {
        let secret = random_secret();
        assert_eq!(secret.len(), SECRET_LEN);
        let id = authenticator_of(&secret);
        assert!(is_guest(&id));
        assert!(!id.0.contains(&secret));
        assert!(!is_guest(&AuthenticatorId("guestbook:x".into())));
        assert!(!id.is_outside_provider());
        assert_eq!(id.via(AuthMethodKind::Guest), "guest");
    }

    #[test]
    fn the_boot_line_shows_only_when_guests_are_on_without_a_check() {
        assert!(unguarded_line(GuestMode::On, false).is_some());
        assert!(unguarded_line(GuestMode::On, true).is_none());
        assert!(unguarded_line(GuestMode::Off, false).is_none());
    }
}
