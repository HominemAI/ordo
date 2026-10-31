//! The password give and the reset, and who may do either
//! (`plan-enrolment.md` §4.1 to §4.4, E6).
//!
//! # The give
//!
//! The owner: *"before the user exists the tenant creates that credential for
//! the user, then the user can own it."* `std.space.createPerson` creates the
//! person, admits them to the tenant, marks them **managed** by it, and hands
//! the administrator a one-time **ticket**. The person redeems it and chooses
//! their own password ([`CredentialOps::redeem_password`]). The administrator
//! never knows the password, so the person owns it from the first second and
//! not after a forced change.
//!
//! # Managed and independent
//!
//! A credential belongs to the deployment and unlocks every tenant its
//! holder is in, so one tenant resetting it would reach into the others.
//! The owner agreed (A1) that a tenant resets a password **only for a person
//! it created who belongs nowhere else**:
//!
//! | | managed | independent |
//! |---|---|---|
//! | created by | a tenant's owner, through a ticket | the person, by signing up or arriving through a provider |
//! | who may reset its credential | that tenant's owners, and the person | the person, and the deployment owner |
//! | what a tenant it belongs to may do | everything | suspend or remove the **membership**, nothing to the credential |
//!
//! A managed person admitted to a **second** tenant stops being managed at
//! that moment, on the record ([`ManagedStore::release_elsewhere`], called by
//! every admission), because from then on a reset by the first tenant would
//! reach into the second. The rule is checked again at the reset, against
//! every database the deployment serves from, and it refuses when any of them
//! cannot be read.
//!
//! One refusal covers "never managed by you", "released" and "belongs
//! elsewhere": a tenant that could tell them apart could learn where else a
//! person is a member.
//!
//! # What a reset does
//!
//! It removes the password credential and its link, ends **every** session
//! the person has (`SessionRevocations::end_all`), and issues a new ticket. A
//! reset that left the old sessions running would be one an attacker could
//! outlast. Passkeys are the person's own and are not touched.
//!
//! # The typed temporary password (A4)
//!
//! Off unless the deployment allows it. When it does, an owner may set a
//! password by hand for a person they manage. It is recorded, with who set it,
//! and marked so that the sign-in can require a change: [`ManagedStore::must_change`]
//! is the question the sign-in route asks. The route is `auth.rs`, which is
//! passkey sign-in's file (`plan-passkey-signin.md` §7), so enforcement at
//! sign-in waits for it.

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::{sql_lit, Pool};
use ordo_identity::credentials::CredentialStore;
use ordo_identity::directory::DirectoryStore;
use ordo_identity::principal::{PrincipalId, PrincipalKind, Timestamp};
use ordo_identity::provider::{AuthenticatorId, PasswordHasher};
use ordo_identity::session::SessionRevocations;
use serde_json::json;

use crate::member_ops::{MemberOps, ROLES};
use crate::tickets::{Purpose, TicketError, TicketSpec, Tickets, Want};

pub const CREATE_PERSON: &str = "std.space.createPerson";
pub const RESET_CREDENTIAL: &str = "std.space.resetCredential";
pub const SET_TEMPORARY_PASSWORD: &str = "std.space.setTemporaryPassword";

pub const OPS: [&str; 3] = [CREATE_PERSON, RESET_CREDENTIAL, SET_TEMPORARY_PASSWORD];

/// Who created an identity, while it is still only that tenant's to reset.
/// A row with `released_at` set is an independent identity that used to be
/// managed, kept because the record of when it changed is the point.
pub const MANAGED_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$managed" (
  "principal"   text   NOT NULL,
  "tenant"      text   NOT NULL,
  "created_by"  text   NOT NULL DEFAULT '',
  "created_at"  bigint NOT NULL DEFAULT 0,
  "released_at" bigint,
  PRIMARY KEY ("principal")
)"#;

/// A password somebody else typed, until its holder sets their own.
pub const TEMPORARY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$temporarypassword" (
  "principal" text   NOT NULL,
  "set_by"    text   NOT NULL DEFAULT '',
  "set_at"    bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("principal")
)"#;

pub async fn ensure_table(pool: &Pool) -> Result<(), ordo_exec::PoolError> {
    for ddl in [MANAGED_DDL, TEMPORARY_DDL] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// The managed/independent state, in the control database.
#[derive(Clone)]
pub struct ManagedStore {
    pool: Pool,
}

impl ManagedStore {
    pub fn new(pool: Pool) -> ManagedStore {
        ManagedStore { pool }
    }

    pub async fn mark(
        &self,
        principal: &str,
        tenant: &str,
        by: &str,
        at: u64,
    ) -> Result<(), ordo_identity::StoreFault> {
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$managed" ("principal", "tenant", "created_by", "created_at")
                   VALUES ({}, {}, {}, {at}) ON CONFLICT DO NOTHING"#,
                sql_lit(principal),
                sql_lit(tenant),
                sql_lit(by)
            ))
            .await
            .map(|_| ())
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(e, "the managed table could not be written")
            })
    }

    /// The tenant that manages this person, while it does.
    pub async fn managed_by(
        &self,
        principal: &str,
    ) -> Result<Option<String>, ordo_identity::StoreFault> {
        let rows = self
            .pool
            .rows(&format!(
                r#"SELECT "tenant" FROM "$managed"
                   WHERE "principal" = {} AND "released_at" IS NULL"#,
                sql_lit(principal)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(e, "the managed table could not be read")
            })?;
        Ok(rows.first().and_then(|r| r[0].clone()))
    }

    /// `principal` was just admitted to `admitted_in`. If a different tenant
    /// manages them, they are independent from now on. Returns that tenant.
    pub async fn release_elsewhere(
        &self,
        principal: &str,
        admitted_in: &str,
        at: u64,
    ) -> Result<Option<String>, ordo_identity::StoreFault> {
        let rows = self
            .pool
            .rows(&format!(
                r#"UPDATE "$managed" SET "released_at" = {at}
                   WHERE "principal" = {} AND "released_at" IS NULL AND "tenant" <> {}
                   RETURNING "tenant""#,
                sql_lit(principal),
                sql_lit(admitted_in)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(e, "the managed table could not be written")
            })?;
        Ok(rows.first().and_then(|r| r[0].clone()))
    }

    pub async fn set_temporary(
        &self,
        principal: &str,
        by: &str,
        at: u64,
    ) -> Result<(), ordo_identity::StoreFault> {
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$temporarypassword" ("principal", "set_by", "set_at")
                   VALUES ({}, {}, {at})
                   ON CONFLICT ("principal") DO UPDATE SET
                     "set_by" = EXCLUDED."set_by", "set_at" = EXCLUDED."set_at""#,
                sql_lit(principal),
                sql_lit(by)
            ))
            .await
            .map(|_| ())
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the temporary password table could not be written",
                )
            })
    }

    /// Must this person choose a new password before anything else? The
    /// question the sign-in route asks.
    pub async fn must_change(&self, principal: &str) -> Result<bool, ordo_identity::StoreFault> {
        Ok(!self
            .pool
            .rows(&format!(
                r#"SELECT 1 FROM "$temporarypassword" WHERE "principal" = {}"#,
                sql_lit(principal)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the temporary password table could not be read",
                )
            })?
            .is_empty())
    }

    pub async fn clear_temporary(&self, principal: &str) -> Result<(), ordo_identity::StoreFault> {
        self.pool
            .rows(&format!(
                r#"DELETE FROM "$temporarypassword" WHERE "principal" = {}"#,
                sql_lit(principal)
            ))
            .await
            .map(|_| ())
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(
                    e,
                    "the temporary password table could not be written",
                )
            })
    }
}

/// Why a redemption did not set a password.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RedeemError {
    /// The one answer: unknown, spent, expired, malformed, or not a
    /// password ticket.
    Refused,
    /// The identifier is somebody else's. Told to a person who holds a
    /// valid ticket, because they must choose another.
    IdentifierTaken,
    /// A store did not answer, with its typed class.
    Unavailable(ordo_identity::StoreFault),
}

/// A store error as a redemption's `Unavailable`, by its typed class.
fn redeem_unavailable(e: impl crate::auth::store_failure::IntoFault) -> RedeemError {
    RedeemError::Unavailable(e.fault())
}

/// What a reset does to a person's second factors once it completes. Filled
/// in after the platform ops are built, because the second step's store is
/// built after them, and shared for the reason `RecoveryHandle` is. Empty, a
/// completed reset touches no second factor.
pub type RedeemHook = Arc<
    std::sync::OnceLock<
        Arc<dyn Fn(&str) -> Result<usize, ordo_identity::StoreFault> + Send + Sync>,
    >,
>;

pub struct CredentialOps {
    pub members: Arc<MemberOps>,
    pub directory: Arc<dyn DirectoryStore>,
    pub credentials: Arc<dyn CredentialStore>,
    pub hasher: Arc<dyn PasswordHasher>,
    pub tickets: Arc<Tickets>,
    pub managed: ManagedStore,
    pub revocations: Arc<dyn SessionRevocations>,
    /// `ORDO_SESSION_MAX_SECS`: how long an epoch has to be remembered.
    pub session_max_secs: u64,
    /// `ORDO_AUTH_TEMPORARY_PASSWORDS=allow`.
    pub allow_temporary: bool,
    /// The control database, for the credential and link rows.
    pub control: Pool,
    /// Run when a person redeems a password ticket. A reset removes every
    /// second factor **once it completes** (owner, 2026-09-29), and it
    /// completes here: when the person chooses a password, not when an
    /// administrator asks for the reset.
    pub after_redeem: RedeemHook,
}

fn refused(msg: &str) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, msg)
}

/// A store error as an op's: busy is `server_busy`, anything else
/// `space_unavailable` with the store's sentence.
fn unavailable(e: impl crate::auth::store_failure::IntoFault) -> ExecError {
    crate::auth::store_failure::exec_unavailable(e)
}

fn text<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The one sentence for every case in which a tenant may not touch a
/// person's credential.
const NOT_YOURS: &str = "a tenant may reset a credential only for a person it created who belongs \
                         nowhere else, and this is not such a person";

/// The person a member row names, once this tenant is shown to have the power
/// over their credential: it created them, and they belong nowhere else. One
/// refusal covers never managed, released, and belongs elsewhere, so a
/// tenant is not told which. Shared by the password reset and by a tenant's
/// recovery of a member (`identity_ops`).
pub(crate) async fn authorise_managed(
    members: &MemberOps,
    managed: &ManagedStore,
    space: &str,
    member: &str,
) -> Result<String, ExecError> {
    let pool = members.pool_of(space).await?;
    let rows = pool
        .rows(&format!(
            r#"SELECT "principal" FROM "members"
               WHERE "$space" = {} AND "id" = {} AND "kind" = 'principal'"#,
            sql_lit(space),
            sql_lit(member)
        ))
        .await
        .map_err(unavailable)?;
    let principal = rows
        .first()
        .and_then(|r| r[0].clone())
        .ok_or_else(|| ExecError::new(ErrorCode::NotFound, "this space has no such member"))?;
    let managed_by = managed
        .managed_by(&principal)
        .await
        .map_err(ExecError::from)?;
    if managed_by.as_deref() != Some(space) {
        return Err(refused(NOT_YOURS));
    }
    // And again, against every database: a person admitted elsewhere without
    // the release having run (a row written by hand, an import) is still not
    // this tenant's to reset.
    if belongs_elsewhere(members, space, &principal).await? {
        return Err(refused(NOT_YOURS));
    }
    Ok(principal)
}

/// Does this person hold a membership in any space but this one? Fails
/// closed: a database that cannot be read is an answer of "yes".
async fn belongs_elsewhere(
    members: &MemberOps,
    space: &str,
    principal: &str,
) -> Result<bool, ExecError> {
    let targets = members.pools.targets().await.map_err(unavailable)?;
    for t in targets {
        let Ok(pool) = t.pool else {
            return Ok(true);
        };
        let there = pool
            .rows("SELECT to_regclass('\"members\"') IS NOT NULL")
            .await
            .map_err(unavailable)?;
        if !matches!(
            there.first().and_then(|r| r[0].as_deref()),
            Some("t") | Some("true")
        ) {
            continue;
        }
        let rows = pool
            .rows(&format!(
                r#"SELECT 1 FROM "members" WHERE "principal" = {} AND "$space" <> {} LIMIT 1"#,
                sql_lit(principal),
                sql_lit(space)
            ))
            .await
            .map_err(unavailable)?;
        if !rows.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

impl CredentialOps {
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        match op {
            CREATE_PERSON => self.create_person(space, bound, caller).await,
            RESET_CREDENTIAL => self.reset(space, bound, caller).await,
            SET_TEMPORARY_PASSWORD => self.temporary(space, bound, caller).await,
            other => Err(ExecError::internal(format!(
                "`{other}` is not a credential op"
            ))),
        }
    }

    // ── the give ─────────────────────────────────────────────────────────

    async fn create_person(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let display = text(bound, "display")
            .ok_or_else(|| ExecError::internal("`$display` did not reach the platform op"))?;
        let role = match text(bound, "role") {
            None => crate::admission::DEFAULT_ROLE,
            Some(r) if ROLES.contains(&r) => r,
            Some(_) => return Err(refused("a role is one of owner, manager, editor or viewer")),
        };
        let purpose = match text(bound, "purpose") {
            None | Some("password") => Purpose::Password,
            Some("passkey") => Purpose::Passkey,
            Some(_) => return Err(refused("a ticket is for a `password` or a `passkey`")),
        };
        let now = self.members.at();
        let principal = self
            .directory
            .create_principal(PrincipalKind::Human, display, Timestamp(now))
            .map_err(unavailable)?;
        self.managed
            .mark(&principal.0, space, caller, now)
            .await
            .map_err(ExecError::from)?;
        let pool = self.members.pool_of(space).await?;
        let admitted = MemberOps::admit_row(&pool, space, &principal.0, role, display)
            .await?
            .ok_or_else(|| {
                ExecError::internal("a principal minted a moment ago is already a member")
            })?;
        let issued = self
            .tickets
            .issue(TicketSpec {
                purpose,
                principal: Some(principal.clone()),
                display: display.to_string(),
                space: Some(space.to_string()),
                role: None,
                issued_by: caller.to_string(),
                ttl_secs: 0,
            })
            .map_err(unavailable)?;
        self.record(
            space,
            caller,
            "identity-enrolled",
            format!(
                "principal={} way=administrator managed_by={space} member={} role={role}",
                principal.0, admitted.member
            ),
        );
        self.members.note(
            space,
            caller,
            "member-admitted",
            format!(
                "member={} principal={} role={role} way=administrator",
                admitted.member, principal.0
            ),
        );
        let door = self.members.rebuilt(space).await;
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "principal": principal.0,
                "member": admitted.member,
                "role": role,
                // The only time the ticket exists in the clear.
                "ticket": issued.ticket,
                "expiresAt": issued.expires_at,
                "outcome": "created",
                "door": door,
            })],
            next_cursor: None,
        })
    }

    /// The person redeems a password ticket and chooses their own password.
    ///
    /// The identifier is checked **before** the ticket is spent, so a taken
    /// identifier costs the person nothing.
    pub async fn redeem_password(
        &self,
        ticket: &str,
        identifier: &str,
        password: &str,
    ) -> Result<PrincipalId, RedeemError> {
        if identifier.trim().is_empty() || password.is_empty() {
            return Err(RedeemError::Refused);
        }
        let authenticator = AuthenticatorId(format!("password:{identifier}"));
        let taken = self
            .credentials
            .hash_for(identifier)
            .map_err(redeem_unavailable)?
            .is_some()
            || self
                .directory
                .principal_for(&authenticator)
                .map_err(redeem_unavailable)?
                .is_some();
        if taken {
            return Err(RedeemError::IdentifierTaken);
        }
        let want = Want {
            purpose: Some(Purpose::Password),
            ..Default::default()
        };
        let redeemed = self
            .tickets
            .redeem_for(ticket, &want)
            .map_err(|e| match e {
                TicketError::Refused => RedeemError::Refused,
                TicketError::Unavailable(w) => RedeemError::Unavailable(w),
            })?;
        // A password ticket always names the person it is for.
        let Some(principal) = redeemed.principal else {
            return Err(RedeemError::Refused);
        };
        let known = self
            .directory
            .principal(&principal)
            .map_err(redeem_unavailable)?
            .ok_or(RedeemError::Refused)?;
        let hash = self.hasher.hash(password);
        self.credentials
            .put(identifier, &hash)
            .map_err(redeem_unavailable)?;
        self.directory
            .insert_link(&authenticator, &known, Timestamp(self.members.at()))
            .map_err(redeem_unavailable)?;
        // Choosing their own password ends any temporary one.
        let _ = self.managed.clear_temporary(&principal.0).await;
        // The reset is complete. Every second factor goes with it, so a factor
        // an attacker enrolled while they held the old password does not
        // outlive the credential that let them.
        if let Some(hook) = self.after_redeem.get() {
            let space = redeemed.space.as_deref().unwrap_or("");
            match hook(&principal.0) {
                Ok(0) => {}
                Ok(n) => self.record(
                    space,
                    &principal.0,
                    "second-factors-removed",
                    format!("principal={} count={n} by=credential-reset", principal.0),
                ),
                Err(_) => self.record(
                    space,
                    &principal.0,
                    "second-factors-not-removed",
                    format!("principal={} by=credential-reset", principal.0),
                ),
            }
        }
        self.record(
            redeemed.space.as_deref().unwrap_or(""),
            &principal.0,
            "credential-set",
            format!("principal={} method=password by=self", principal.0),
        );
        Ok(principal)
    }

    /// A person who knows their current password chooses a new one, which is
    /// what a temporary password's forced change is.
    ///
    /// The current password is checked against the stored hash, and an
    /// unknown identifier costs the same work as a wrong password, so this is
    /// no oracle for who has a password. On success the new password is
    /// stored, a temporary marker is cleared, and **every session the person
    /// has ends**, so a password chosen because the old one leaked does not
    /// leave the leaker signed in. The person signs in again with the new one.
    ///
    /// The one refusal covers an unknown identifier, a wrong password, an
    /// identifier that is not a password of a known person, and a new
    /// password that is empty or the same as the old one.
    pub async fn change_password(
        &self,
        identifier: &str,
        current: &str,
        new: &str,
    ) -> Result<PrincipalId, RedeemError> {
        if identifier.trim().is_empty() || current.is_empty() || new.is_empty() || new == current {
            return Err(RedeemError::Refused);
        }
        let stored = self
            .credentials
            .hash_for(identifier)
            .map_err(redeem_unavailable)?;
        let correct = match &stored {
            Some(hash) => self.hasher.verify(current, hash),
            None => {
                // The same work as a real check, so timing says nothing.
                let _ = self
                    .hasher
                    .verify(current, &self.hasher.hash("not-a-password"));
                false
            }
        };
        if !correct {
            return Err(RedeemError::Refused);
        }
        let authenticator = AuthenticatorId(format!("password:{identifier}"));
        let principal = self
            .directory
            .principal_for(&authenticator)
            .map_err(redeem_unavailable)?
            .ok_or(RedeemError::Refused)?;
        let hash = self.hasher.hash(new);
        self.credentials
            .put(identifier, &hash)
            .map_err(redeem_unavailable)?;
        self.managed
            .clear_temporary(&principal.0)
            .await
            .map_err(redeem_unavailable)?;
        self.end_sessions(&principal.0)
            .await
            .map_err(redeem_unavailable)?;
        self.record(
            "",
            &principal.0,
            "credential-set",
            format!(
                "principal={} method=password by=self change=true",
                principal.0
            ),
        );
        Ok(principal)
    }

    // ── authority ────────────────────────────────────────────────────────

    /// The person a member row names, once this tenant is shown to have the
    /// power over their credential.
    async fn authorised(&self, space: &str, member: &str) -> Result<String, ExecError> {
        authorise_managed(&self.members, &self.managed, space, member).await
    }

    /// Remove every password credential the person holds, and its links.
    /// Returns how many password authenticators went.
    async fn remove_passwords(&self, principal: &str) -> Result<usize, ExecError> {
        let links = self
            .control
            .rows(&format!(
                r#"SELECT "authenticator" FROM "$authenticator"
                   WHERE "principal" = {} AND "authenticator" LIKE 'password:%'"#,
                sql_lit(principal)
            ))
            .await
            .map_err(unavailable)?;
        let mut n = 0;
        for link in links.iter().filter_map(|r| r[0].clone()) {
            if let Some(identifier) = link.strip_prefix("password:") {
                self.credentials.remove(identifier).map_err(unavailable)?;
            }
            self.directory
                .remove_link(&AuthenticatorId(link))
                .map_err(unavailable)?;
            n += 1;
        }
        Ok(n)
    }

    /// End every session the person has, off the request's own thread: the
    /// store is synchronous and a Postgres one blocks on a round trip.
    async fn end_sessions(&self, principal: &str) -> Result<(), ExecError> {
        let now = self.members.at();
        let store = self.revocations.clone();
        let who = PrincipalId(principal.to_string());
        let until = Timestamp(now.saturating_add(self.session_max_secs));
        tokio::task::spawn_blocking(move || store.end_all(&who, Timestamp(now), until))
            .await
            .map_err(|_| ExecError::internal("ending the sessions did not finish"))?
            .map_err(unavailable)
    }

    // ── the reset ────────────────────────────────────────────────────────

    async fn reset(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let member = text(bound, "member")
            .ok_or_else(|| ExecError::internal("`$member` did not reach the platform op"))?;
        let principal = self.authorised(space, member).await?;
        let removed = self.remove_passwords(&principal).await?;
        self.end_sessions(&principal).await?;
        let _ = self.managed.clear_temporary(&principal).await;
        let issued = self
            .tickets
            .issue(TicketSpec {
                purpose: Purpose::Password,
                principal: Some(PrincipalId(principal.clone())),
                display: String::new(),
                space: Some(space.to_string()),
                role: None,
                issued_by: caller.to_string(),
                ttl_secs: 0,
            })
            .map_err(unavailable)?;
        self.record(
            space,
            caller,
            "credential-reset",
            format!(
                "principal={principal} member={member} by={caller} authority=created-by-tenant \
                 passwords_removed={removed} sessions=ended"
            ),
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "principal": principal,
                "member": member,
                "ticket": issued.ticket,
                "expiresAt": issued.expires_at,
                "outcome": "reset",
            })],
            next_cursor: None,
        })
    }

    // ── the typed temporary password ─────────────────────────────────────

    async fn temporary(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if !self.allow_temporary {
            return Err(refused(
                "this deployment does not allow an administrator to type a password for \
                 somebody. Set ORDO_AUTH_TEMPORARY_PASSWORDS=allow to permit it, or create \
                 the person, or reset, and pass on the ticket: the person then chooses \
                 their own",
            ));
        }
        let member = text(bound, "member")
            .ok_or_else(|| ExecError::internal("`$member` did not reach the platform op"))?;
        let identifier = text(bound, "identifier")
            .ok_or_else(|| ExecError::internal("`$identifier` did not reach the platform op"))?;
        let password = bound
            .0
            .get("password")
            .and_then(|v| v.as_str())
            .filter(|p| !p.is_empty())
            .ok_or_else(|| ExecError::internal("`$password` did not reach the platform op"))?;
        let principal = self.authorised(space, member).await?;
        let authenticator = AuthenticatorId(format!("password:{identifier}"));
        // The identifier must be this person's or nobody's.
        let owner = self
            .directory
            .principal_for(&authenticator)
            .map_err(unavailable)?;
        if owner.as_ref().is_some_and(|o| o.0 != principal) {
            return Err(refused("that identifier belongs to somebody else"));
        }
        let known = self
            .directory
            .principal(&PrincipalId(principal.clone()))
            .map_err(unavailable)?
            .ok_or_else(|| ExecError::new(ErrorCode::NotFound, "this space has no such member"))?;
        self.remove_passwords(&principal).await?;
        self.credentials
            .put(identifier, &self.hasher.hash(password))
            .map_err(unavailable)?;
        self.directory
            .insert_link(&authenticator, &known, Timestamp(self.members.at()))
            .map_err(unavailable)?;
        self.managed
            .set_temporary(&principal, caller, self.members.at())
            .await
            .map_err(ExecError::from)?;
        self.end_sessions(&principal).await?;
        self.record(
            space,
            caller,
            "temporary-password-set",
            format!(
                "principal={principal} member={member} by={caller} identifier={identifier} \
                 must_change=true"
            ),
        );
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "principal": principal,
                "member": member,
                "outcome": "temporary password set; the person must choose their own at first sign-in",
            })],
            next_cursor: None,
        })
    }

    /// An act on a credential: at deployment scope, mirrored to the tenant
    /// whose owner did it.
    fn record(&self, space: &str, by: &str, act: &str, detail: String) {
        if let Some(stream) = &self.members.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act,
                    detail,
                    by: Some(by),
                    tenant: (!space.is_empty()).then_some(space),
                    deployment: true,
                    at: self.members.at(),
                },
            );
        }
    }
}
