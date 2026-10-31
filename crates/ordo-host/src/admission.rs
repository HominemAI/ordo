//! Admission policy, per tenant, under the deployment's ceiling
//! (`plan-enrolment.md` §2.2 and §2.3, E4).
//!
//! Two acts that used to be one word. **Enrolling an identity** is the
//! deployment's: a credential is verified and, if the deployment has not seen
//! it, a principal is minted. **Admitting a member** is the tenant's: a
//! principal is given a membership, with a role. This module is the second.
//!
//! # The ways in
//!
//! | Way | On a person who is not a member |
//! |---|---|
//! | open | they are admitted at the tenant's default role, **viewer** unless the app developer set another |
//! | invitation | they are admitted if they hold an invitation ticket for this tenant, at the role it names |
//! | provider | they are admitted if they arrived through a named outside provider, optionally only from named domains, at the role the rule names |
//! | first-comer | they are admitted as owner if and only if the tenant has no members (E5) |
//!
//! None of them is compulsory (P4) and any combination may be on. A tenant
//! with none on admits only people an owner adds by hand.
//!
//! # The ceiling
//!
//! `ORDO_AUTH_PROVISIONING` is the ceiling on which ways a tenant may switch
//! on. `invite-only` allows the invitation way and nothing else, so a
//! deployment that must never admit a stranger has a tenant that cannot open
//! itself wider. `auto-onboard` allows all four. The ceiling is checked when a
//! policy is **set** and again when it is **used**, so a deployment that
//! lowers its ceiling stops admitting through a policy written under the old
//! one.
//!
//! # One answer
//!
//! A person the tenant will not admit is refused with [`Outcome::Refused`]
//! whatever the reason: no way switched on, a way switched on that they do not
//! match, a ticket that is unknown, expired, spent or for another tenant. A
//! caller that could tell them apart could tell a tenant's policy from outside.
//!
//! # Who calls `on_arrival`
//!
//! The sign-in route, once a sign-in names the tenant it is for. Today
//! `/auth/complete` signs a person in to the deployment and names no tenant,
//! and `auth.rs` is passkey sign-in's file (`plan-passkey-signin.md` §7), so
//! this module is the function that route calls and not a route of its own.

use std::collections::BTreeSet;
use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::{sql_lit, Pool, PoolError};
use serde_json::json;

use ordo_identity::second_step::TenantMode;

use crate::member_ops::{MemberOps, ROLES};
use crate::second_factors::SecondStepLevels;
use crate::tickets::{Purpose, TicketError, Tickets, Want};

pub const SET_ADMISSION: &str = "std.space.setAdmission";
pub const ADMISSION: &str = "std.space.admission";

/// The default role for a person a tenant admits without being told which.
pub const DEFAULT_ROLE: &str = "viewer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Way {
    Open,
    Invitation,
    Provider,
    FirstComer,
}

impl Way {
    pub fn as_str(self) -> &'static str {
        match self {
            Way::Open => "open",
            Way::Invitation => "invitation",
            Way::Provider => "provider",
            Way::FirstComer => "first-comer",
        }
    }
}

/// Which ways a tenant on this deployment may switch on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ceiling {
    allowed: BTreeSet<Way>,
}

impl Ceiling {
    /// `ORDO_AUTH_PROVISIONING=invite-only`: invitations, and nothing wider.
    pub fn invitation_only() -> Ceiling {
        Ceiling {
            allowed: BTreeSet::from([Way::Invitation]),
        }
    }

    /// `ORDO_AUTH_PROVISIONING=auto-onboard`: every way.
    pub fn everything() -> Ceiling {
        Ceiling {
            allowed: BTreeSet::from([Way::Open, Way::Invitation, Way::Provider, Way::FirstComer]),
        }
    }

    /// The ceiling a deployment's existing provisioning switch names.
    pub fn from_auto_onboard(auto_onboard: bool) -> Ceiling {
        if auto_onboard {
            Ceiling::everything()
        } else {
            Ceiling::invitation_only()
        }
    }

    pub fn allows(&self, way: Way) -> bool {
        self.allowed.contains(&way)
    }

    /// The ways, for the answer that names them.
    pub fn words(&self) -> Vec<&'static str> {
        self.allowed.iter().map(|w| w.as_str()).collect()
    }
}

/// One rule of the provider way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRule {
    /// The provider as the authenticator id names it: `oidc:acme`,
    /// `saml:corp`.
    pub provider: String,
    /// When set, only a person whose address is in this domain.
    pub domain: Option<String>,
    pub role: String,
}

/// A tenant's admission policy. Nothing on is the default, so a tenant that
/// never chose admits by hand and by invitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub open: bool,
    pub default_role: String,
    pub providers: Vec<ProviderRule>,
    pub first_comer: bool,
    /// What the tenant requires of its people's sign-ins
    /// (`plan-second-factor.md` §5.4). `inherit` is whatever the app developer
    /// set for the deployment; a tenant can only tighten it.
    pub second_factor: TenantMode,
    /// The connections whose two-step claim this tenant accepts as presence,
    /// for its own data and nothing else (`plan-auth-federated.md` §8.2).
    /// Empty is the default. Each needs the app developer's `tenant` or `on`.
    pub presence_from: Vec<String>,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            open: false,
            default_role: DEFAULT_ROLE.to_string(),
            providers: Vec::new(),
            first_comer: false,
            second_factor: TenantMode::Inherit,
            presence_from: Vec::new(),
        }
    }
}

impl Policy {
    /// The ways this policy switches on.
    pub fn ways(&self) -> Vec<Way> {
        let mut out = Vec::new();
        if self.open {
            out.push(Way::Open);
        }
        if !self.providers.is_empty() {
            out.push(Way::Provider);
        }
        if self.first_comer {
            out.push(Way::FirstComer);
        }
        out
    }

    /// Is every way this policy switches on one the ceiling allows?
    pub fn within(&self, ceiling: &Ceiling) -> Result<(), String> {
        for way in self.ways() {
            if !ceiling.allows(way) {
                return Err(format!(
                    "this deployment's ceiling does not allow the `{}` way in; the ways it \
                     allows are: {}",
                    way.as_str(),
                    ceiling.words().join(", ")
                ));
            }
        }
        Ok(())
    }
}

/// Parse `provider[@domain]=role, ...`. Empty text is no rules.
pub fn parse_providers(text: &str) -> Result<Vec<ProviderRule>, String> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (who, role) = part
            .split_once('=')
            .ok_or_else(|| format!("`{part}` is not `provider[@domain]=role`"))?;
        let role = role.trim();
        if !ROLES.contains(&role) {
            return Err(format!("`{role}` is not a role"));
        }
        let (provider, domain) = match who.split_once('@') {
            Some((p, d)) if !d.trim().is_empty() => (p.trim(), Some(d.trim().to_ascii_lowercase())),
            Some((p, _)) => (p.trim(), None),
            None => (who.trim(), None),
        };
        if provider.is_empty() {
            return Err(format!("`{part}` names no provider"));
        }
        out.push(ProviderRule {
            provider: provider.to_string(),
            domain,
            role: role.to_string(),
        });
    }
    Ok(out)
}

/// A newcomer's invitation, spent: the role it admits at, and when it was
/// spent, which is what giving it back checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentInvitation {
    pub role: String,
    pub spent_at: u64,
}

/// What a newcomer's invitation must be: an invitation, to this tenant, that
/// names nobody. A newcomer has no principal yet, so the holder is the empty
/// id, which no named ticket matches and every unnamed one does. The store
/// applies it in the same statement that spends the ticket.
fn newcomer_want(space: &str) -> Want<'_> {
    Want {
        purpose: Some(Purpose::Invitation),
        space: Some(space),
        holder: Some(""),
    }
}

fn render_providers(rules: &[ProviderRule]) -> String {
    rules
        .iter()
        .map(|r| match &r.domain {
            Some(d) => format!("{}@{}={}", r.provider, d, r.role),
            None => format!("{}={}", r.provider, r.role),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The policy table: one row per tenant that chose anything.
pub const POLICY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$admissionpolicy" (
  "space"        text    NOT NULL,
  "open"         boolean NOT NULL DEFAULT false,
  "default_role" text    NOT NULL DEFAULT 'viewer',
  "providers"    text    NOT NULL DEFAULT '',
  "first_comer"  boolean NOT NULL DEFAULT false,
  "updated_by"   text    NOT NULL DEFAULT '',
  "updated_at"   bigint  NOT NULL DEFAULT 0,
  "second_factor" text   NOT NULL DEFAULT 'inherit',
  "presence_from" text   NOT NULL DEFAULT '',
  PRIMARY KEY ("space")
)"#;

/// A table made before a tenant could require a second step gains the column,
/// with every existing row inheriting the deployment's setting.
pub const POLICY_SECOND_FACTOR_DDL: &str = r#"ALTER TABLE "$admissionpolicy"
  ADD COLUMN IF NOT EXISTS "second_factor" text NOT NULL DEFAULT 'inherit'"#;

/// A table made before a tenant could accept a provider's two-step claim
/// gains the column, with every existing row accepting none.
pub const POLICY_PRESENCE_FROM_DDL: &str = r#"ALTER TABLE "$admissionpolicy"
  ADD COLUMN IF NOT EXISTS "presence_from" text NOT NULL DEFAULT ''"#;

/// The latch that closes the first-comer way for a tenant, once and for
/// good. A row here is the fact that somebody claimed the space.
pub const CLAIM_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$firstcomer" (
  "space"      text   NOT NULL,
  "claimed_by" text   NOT NULL DEFAULT '',
  "claimed_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("space")
)"#;

/// Deployment-wide settings that are not environment: one row each, so the
/// console and the op that will call it have one place to write.
pub const SETTING_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$deploymentsetting" (
  "key"        text   NOT NULL,
  "value"      text   NOT NULL,
  "updated_by" text   NOT NULL DEFAULT '',
  "updated_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("key")
)"#;

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [
        POLICY_DDL,
        POLICY_SECOND_FACTOR_DDL,
        POLICY_PRESENCE_FROM_DDL,
        CLAIM_DDL,
        SETTING_DDL,
    ] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// Where policies are kept: the control database, because a policy is the
/// deployment's record about a tenant and not one of the tenant's rows.
#[derive(Clone)]
pub struct PolicyStore {
    pool: Pool,
    /// The tenants' second-step levels the door and the sign-in gate read, and
    /// the deployment's setting a tenant's choice is checked against.
    levels: Option<Arc<SecondStepLevels>>,
}

impl PolicyStore {
    pub fn new(pool: Pool) -> PolicyStore {
        PolicyStore { pool, levels: None }
    }

    /// Share the second-step levels: a level an owner sets is the level the
    /// next request meets, and is refused when the deployment's setting cannot
    /// honour it.
    pub fn with_levels(mut self, levels: Arc<SecondStepLevels>) -> PolicyStore {
        self.levels = Some(levels);
        self
    }

    pub async fn get(&self, space: &str) -> Result<Policy, ordo_identity::StoreFault> {
        let rows = self
            .pool
            .rows(&format!(
                r#"SELECT "open", "default_role", "providers", "first_comer", "second_factor",
                          "presence_from"
                   FROM "$admissionpolicy" WHERE "space" = {}"#,
                sql_lit(space)
            ))
            .await
            .map_err(|e| {
                crate::auth::store_failure::pool_fault(e, "the admission policy could not be read")
            })?;
        let Some(r) = rows.first() else {
            return Ok(Policy::default());
        };
        let text = |i: usize| r.get(i).and_then(|c| c.clone()).unwrap_or_default();
        let flag = |i: usize| matches!(text(i).as_str(), "t" | "true");
        Ok(Policy {
            open: flag(0),
            default_role: match text(1) {
                r if ROLES.contains(&r.as_str()) => r,
                _ => DEFAULT_ROLE.to_string(),
            },
            providers: parse_providers(&text(2)).unwrap_or_default(),
            first_comer: flag(3),
            second_factor: TenantMode::parse(&text(4)).unwrap_or_default(),
            presence_from: parse_presence_from(&text(5)).unwrap_or_default(),
        })
    }

    pub async fn put(
        &self,
        space: &str,
        p: &Policy,
        by: &str,
        at: u64,
    ) -> Result<(), ordo_identity::StoreFault> {
        self.pool
            .rows(&format!(
                r#"INSERT INTO "$admissionpolicy"
                   ("space","open","default_role","providers","first_comer","updated_by","updated_at","second_factor","presence_from")
                   VALUES ({s}, {o}, {r}, {p}, {f}, {b}, {at}, {sf}, {pf})
                   ON CONFLICT ("space") DO UPDATE SET
                     "open" = EXCLUDED."open", "default_role" = EXCLUDED."default_role",
                     "providers" = EXCLUDED."providers", "first_comer" = EXCLUDED."first_comer",
                     "updated_by" = EXCLUDED."updated_by", "updated_at" = EXCLUDED."updated_at",
                     "second_factor" = EXCLUDED."second_factor",
                     "presence_from" = EXCLUDED."presence_from""#,
                s = sql_lit(space),
                o = p.open,
                r = sql_lit(&p.default_role),
                p = sql_lit(&render_providers(&p.providers)),
                f = p.first_comer,
                b = sql_lit(by),
                sf = sql_lit(p.second_factor.as_str()),
                pf = sql_lit(&p.presence_from.join(",")),
            ))
            .await
            .map(|_| ())
            .map_err(|e| crate::auth::store_failure::pool_fault(e, "the admission policy could not be written"))
    }
}

/// How a person arrived.
#[derive(Debug, Clone, Default)]
pub struct Arrival<'a> {
    /// An invitation ticket the person presents.
    pub ticket: Option<&'a str>,
    /// The outside provider they signed in through, and the domain of the
    /// address it vouched for, when it vouched for one.
    pub provider: Option<Via<'a>>,
}

#[derive(Debug, Clone)]
pub struct Via<'a> {
    pub provider: &'a str,
    pub domain: Option<&'a str>,
}

/// What an arrival came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// They already were a member. Nothing changed.
    AlreadyMember { member: String },
    Admitted {
        member: String,
        role: String,
        way: Way,
    },
    /// The one refusal.
    Refused,
}

pub struct Admission {
    pub members: Arc<MemberOps>,
    pub store: PolicyStore,
    pub tickets: Option<Arc<Tickets>>,
    pub ceiling: Ceiling,
    /// The deployment's own space, the only one the deployment-wide
    /// settings answer in. `None` where nothing is the home. It has no
    /// admission default of its own: first-comer is off there too, until
    /// somebody turns it on (owner, 2026-09-29: *setup belongs on the server
    /// side*).
    pub home: Option<String>,
}

impl Admission {
    /// A person the deployment knows has arrived at a tenant. Admit them if
    /// the tenant's policy says so, under the ceiling, and otherwise refuse.
    pub async fn on_arrival(
        &self,
        principal: &str,
        space: &str,
        arrival: &Arrival<'_>,
    ) -> Result<Outcome, ExecError> {
        if let Some(member) = self.members.member_of(space, principal).await? {
            return Ok(Outcome::AlreadyMember { member });
        }
        // 27ar: a paused tenant admits nobody new. The one refusal, so a
        // person who is not a member learns nothing of the pause, and asked
        // before any ticket is spent, so the invitation still works after a
        // resume.
        if let Ok(Some(t)) = crate::tenants::lookup(&self.members.control, space).await {
            if t.state == crate::tenants::State::Paused {
                return Ok(Outcome::Refused);
            }
        }
        let policy = self.store.get(space).await.map_err(ExecError::from)?;

        // The order is the order of trust. A ticket is somebody deciding
        // about this person; a provider rule is somebody deciding about a
        // provider; open is nobody deciding.
        let mut choice: Option<(Way, String)> = None;

        if let (Some(ticket), Some(tickets)) = (arrival.ticket, &self.tickets) {
            if self.ceiling.allows(Way::Invitation) {
                // The ticket must be an invitation, to this tenant, for
                // this person or for whoever holds it. One that is not is
                // refused **without being spent**.
                let want = Want {
                    purpose: Some(Purpose::Invitation),
                    space: Some(space),
                    holder: Some(principal),
                };
                match tickets.redeem_for(ticket, &want) {
                    Ok(r) => {
                        let role = r
                            .role
                            .filter(|role| ROLES.contains(&role.as_str()))
                            .unwrap_or_else(|| policy.default_role.clone());
                        choice = Some((Way::Invitation, role));
                    }
                    // Spent, unknown, expired, for somebody else or for
                    // another tenant: the one refusal, unless another way
                    // still admits them.
                    Err(TicketError::Refused) => {}
                    Err(TicketError::Unavailable(why)) => {
                        return Err(ExecError::from(why));
                    }
                }
            }
        }
        if choice.is_none() && self.ceiling.allows(Way::Provider) {
            if let Some(via) = &arrival.provider {
                let hit = policy.providers.iter().find(|r| {
                    r.provider == via.provider
                        && r.domain
                            .as_deref()
                            .is_none_or(|d| via.domain.is_some_and(|v| v.eq_ignore_ascii_case(d)))
                });
                if let Some(rule) = hit {
                    choice = Some((Way::Provider, rule.role.clone()));
                }
            }
        }
        if choice.is_none() && policy.open && self.ceiling.allows(Way::Open) {
            choice = Some((Way::Open, policy.default_role.clone()));
        }
        // The deployment's own space never admits a first-comer: deployment
        // people come from the identity service (`plan-identity-service.md`
        // 6, owner 2026-10-30), even where an old policy row says otherwise.
        if choice.is_none()
            && policy.first_comer
            && self.ceiling.allows(Way::FirstComer)
            && self.home.as_deref() != Some(space)
        {
            return self.claim(principal, space).await;
        }
        let Some((way, role)) = choice else {
            return Ok(Outcome::Refused);
        };
        self.admit(principal, space, &role, way).await
    }

    /// Is `ticket` an invitation a newcomer may sign up with at `space`?
    /// The role it admits at when it is, **without spending it**.
    ///
    /// A newcomer has no principal yet, so only an invitation that names
    /// nobody can be theirs. It must also be unspent, unexpired, for this
    /// tenant, under a ceiling that allows invitations, and the tenant must
    /// not be paused. Every "no" is `None`: the caller answers the one
    /// refusal whatever the reason (owner ruling 2026-10-09, federated
    /// ask 9).
    pub async fn newcomer_invitation(
        &self,
        space: &str,
        ticket: &str,
    ) -> Result<Option<String>, ExecError> {
        let Some(tickets) = &self.tickets else {
            return Ok(None);
        };
        if !self.ceiling.allows(Way::Invitation) {
            return Ok(None);
        }
        if let Ok(Some(t)) = crate::tenants::lookup(&self.members.control, space).await {
            if t.state == crate::tenants::State::Paused {
                return Ok(None);
            }
        }
        let rec = match tickets.peek(ticket, &newcomer_want(space)) {
            Ok(rec) => rec,
            Err(TicketError::Refused) => return Ok(None),
            Err(TicketError::Unavailable(why)) => return Err(ExecError::from(why)),
        };
        if rec.principal.is_some() {
            return Ok(None);
        }
        let policy = self.store.get(space).await.map_err(ExecError::from)?;
        Ok(Some(
            rec.role
                .filter(|role| ROLES.contains(&role.as_str()))
                .unwrap_or(policy.default_role),
        ))
    }

    /// Check and **spend** a newcomer's invitation, before they are minted.
    /// The role it admits at, or `None` for the one refusal.
    ///
    /// The spend is one statement in the ticket store, so of two sign-ups
    /// racing with one invitation exactly one gets the role. The other is
    /// refused here, before anything is minted for them.
    pub async fn take_newcomer_invitation(
        &self,
        space: &str,
        ticket: &str,
    ) -> Result<Option<SpentInvitation>, ExecError> {
        let Some(role) = self.newcomer_invitation(space, ticket).await? else {
            return Ok(None);
        };
        let Some(tickets) = &self.tickets else {
            return Ok(None);
        };
        match tickets.spend_for(ticket, &newcomer_want(space)) {
            Ok((_, spent_at)) => Ok(Some(SpentInvitation { role, spent_at })),
            Err(TicketError::Refused) => Ok(None),
            Err(TicketError::Unavailable(why)) => Err(ExecError::from(why)),
        }
    }

    /// Give back an invitation [`Admission::take_newcomer_invitation`] spent,
    /// when the newcomer could not be minted or admitted after all. Only
    /// that spend is undone, so the invitation is usable again and nothing
    /// else changes. `false` when there was nothing to give back.
    pub fn give_back_invitation(
        &self,
        ticket: &str,
        spent: &SpentInvitation,
    ) -> Result<bool, ExecError> {
        let Some(tickets) = &self.tickets else {
            return Ok(false);
        };
        tickets
            .give_back(ticket, spent.spent_at)
            .map_err(crate::auth::store_failure::exec_unavailable)
    }

    /// Admit a newcomer whose invitation [`Admission::take_newcomer_invitation`]
    /// already spent, at the role it named.
    pub async fn admit_invited(
        &self,
        principal: &str,
        space: &str,
        role: &str,
    ) -> Result<Outcome, ExecError> {
        if let Some(member) = self.members.member_of(space, principal).await? {
            return Ok(Outcome::AlreadyMember { member });
        }
        self.admit(principal, space, role, Way::Invitation).await
    }

    /// A person signed in through `space`'s own connection
    /// (`plan-tenant-providers.md` §6): admit them at the connection's role.
    /// It is the provider way, so the ceiling must allow it, and a paused
    /// tenant admits nobody new. Every "no" is the one refusal.
    pub async fn admit_by_connection(
        &self,
        principal: &str,
        space: &str,
        role: &str,
    ) -> Result<Outcome, ExecError> {
        if let Some(member) = self.members.member_of(space, principal).await? {
            return Ok(Outcome::AlreadyMember { member });
        }
        if !self.ceiling.allows(Way::Provider) || !ROLES.contains(&role) {
            return Ok(Outcome::Refused);
        }
        if let Ok(Some(t)) = crate::tenants::lookup(&self.members.control, space).await {
            if t.state == crate::tenants::State::Paused {
                return Ok(Outcome::Refused);
            }
        }
        self.admit(principal, space, role, Way::Provider).await
    }

    /// The first-comer way: the first person to arrive at a tenant with no
    /// members owns it, and the way closes for good.
    ///
    /// **The latch is what makes "first" mean one.** Two people arriving at
    /// the same moment both see an empty tenant. Each tries to insert the
    /// tenant's row in `$firstcomer`, the database lets one succeed, and only
    /// that one goes on to be admitted. The loser is refused like anybody
    /// else. If the winner's admission then fails, the latch is released so
    /// the tenant is not left unclaimable.
    async fn claim(&self, principal: &str, space: &str) -> Result<Outcome, ExecError> {
        if self.members.count_members(space).await? > 0 {
            return Ok(Outcome::Refused);
        }
        let won = self
            .store
            .pool
            .rows(&format!(
                r#"INSERT INTO "$firstcomer" ("space", "claimed_by", "claimed_at")
                   VALUES ({}, {}, {}) ON CONFLICT DO NOTHING RETURNING "space""#,
                sql_lit(space),
                sql_lit(principal),
                self.members.at()
            ))
            .await
            .map_err(|e| {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, e.to_string())
            })?;
        if won.is_empty() {
            return Ok(Outcome::Refused);
        }
        let out = self.admit(principal, space, "owner", Way::FirstComer).await;
        if !matches!(out, Ok(Outcome::Admitted { .. })) {
            let _ = self
                .store
                .pool
                .rows(&format!(
                    r#"DELETE FROM "$firstcomer" WHERE "space" = {}"#,
                    sql_lit(space)
                ))
                .await;
        }
        out
    }

    /// Write the membership, record it and rebuild the door.
    async fn admit(
        &self,
        principal: &str,
        space: &str,
        role: &str,
        way: Way,
    ) -> Result<Outcome, ExecError> {
        let pool = self.members.pool_of(space).await?;
        // The name the person gave, from the directory; the id only when the
        // directory holds none.
        let display = self.members.display_name_of(principal);
        match MemberOps::admit_row(&pool, space, principal, role, &display).await? {
            Some(a) => {
                self.members.note(
                    space,
                    principal,
                    "member-admitted",
                    format!(
                        "member={} principal={} role={} way={}",
                        a.member,
                        a.principal,
                        a.role,
                        way.as_str()
                    ),
                );
                self.members.admitted(space, &a.principal).await;
                self.members.rebuilt(space).await;
                Ok(Outcome::Admitted {
                    member: a.member,
                    role: a.role,
                    way,
                })
            }
            // Admitted by somebody else between the read and the write.
            None => match self.members.member_of(space, principal).await? {
                Some(member) => Ok(Outcome::AlreadyMember { member }),
                None => Ok(Outcome::Refused),
            },
        }
    }

    /// Change what a tenant switches on. Only the fields supplied change.
    /// Refused, by name, when the result is wider than the ceiling.
    pub async fn set_policy(
        &self,
        space: &str,
        change: &PolicyChange,
        by: &str,
        at: u64,
    ) -> Result<Policy, ExecError> {
        let mut p = self.store.get(space).await.map_err(ExecError::from)?;
        if let Some(v) = change.open {
            p.open = v;
        }
        if let Some(r) = &change.default_role {
            p.default_role = r.clone();
        }
        if let Some(rules) = &change.providers {
            p.providers = rules.clone();
        }
        if let Some(v) = change.first_comer {
            if v && self.home.as_deref() == Some(space) {
                return Err(ExecError::new(
                    ErrorCode::TenantRefused,
                    HOME_FIRST_COMER_REFUSED,
                ));
            }
            p.first_comer = v;
        }
        if let Some(mode) = change.second_factor {
            // Tightening only, and by name: a tenant cannot require what the
            // app developer turned off or loosen what they require.
            if let Some(levels) = &self.store.levels {
                mode.within(levels.deployment())
                    .map_err(|e| ExecError::new(ErrorCode::TenantRefused, e))?;
            }
            p.second_factor = mode;
        }
        if let Some(list) = &change.presence_from {
            // Any connection of this deployment, whatever the app developer
            // set for it (federated ask 10, owner 2026-10-09: "Open"). A name
            // that is no connection is refused, naming the ones there are, so
            // a misspelling does not leave the tenant believing it accepted
            // something.
            let acceptance = crate::presence::upstream();
            for c in list {
                // A tenant's own connection, by its `key`
                // (`plan-tenant-providers.md` §7). Another tenant's key is
                // refused like any unknown name.
                let own = ordo_identity::upstream::tenant_key_of(
                    c,
                    &ordo_identity::SpaceId(space.to_string()),
                );
                if !own && !acceptance.as_ref().is_some_and(|a| a.knows(c)) {
                    let known: Vec<String> = acceptance
                        .as_ref()
                        .map(|a| a.connections().map(str::to_string).collect())
                        .unwrap_or_default();
                    return Err(ExecError::new(
                        ErrorCode::TenantRefused,
                        format!(
                            "`{c}` in `presenceFrom` is not a sign-in connection of this \
                             deployment; the connections are: {}",
                            if known.is_empty() {
                                "none".to_string()
                            } else {
                                known.join(", ")
                            }
                        ),
                    ));
                }
            }
            p.presence_from = list.clone();
        }
        p.within(&self.ceiling)
            .map_err(|e| ExecError::new(ErrorCode::TenantRefused, e))?;
        self.store
            .put(space, &p, by, at)
            .await
            .map_err(ExecError::from)?;
        if let Some(levels) = &self.store.levels {
            levels.set(space, p.second_factor);
        }
        if let Some(acceptance) = crate::presence::upstream() {
            acceptance.set_tenant(
                &ordo_identity::SpaceId(space.to_string()),
                p.presence_from.iter().cloned().collect(),
            );
        }
        self.members.note(
            space,
            by,
            "admission-policy-set",
            format!(
                "open={} role={} providers={} first_comer={} second_factor={} presence_from={}",
                p.open,
                p.default_role,
                p.providers.len(),
                p.first_comer,
                p.second_factor.as_str(),
                p.presence_from.join(",")
            ),
        );
        Ok(p)
    }

    /// Run one of the two platform ops of this module.
    pub async fn execute(
        &self,
        op: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        let policy = match op {
            SET_ADMISSION => {
                let text = |n: &str| bound.0.get(n).and_then(|v| v.as_str());
                let flag = |n: &str| bound.0.get(n).and_then(|v| v.as_bool());
                let change = PolicyChange {
                    open: flag("open"),
                    default_role: match text("role").map(str::trim) {
                        Some(r) if ROLES.contains(&r) => Some(r.to_string()),
                        Some(_) => {
                            return Err(ExecError::new(
                                ErrorCode::TenantRefused,
                                "a default role is one of owner, manager, editor or viewer",
                            ))
                        }
                        None => None,
                    },
                    providers: match text("providers") {
                        Some(t) => Some(
                            parse_providers(t)
                                .map_err(|e| ExecError::new(ErrorCode::TenantRefused, e))?,
                        ),
                        None => None,
                    },
                    first_comer: flag("firstComer"),
                    second_factor: match text("secondFactor").map(str::trim) {
                        Some(w) => Some(TenantMode::parse(w).ok_or_else(|| {
                            ExecError::new(
                                ErrorCode::TenantRefused,
                                "`secondFactor` is inherit, required or required-for-non-passkey",
                            )
                        })?),
                        None => None,
                    },
                    presence_from: match text("presenceFrom") {
                        Some(t) => Some(
                            parse_presence_from(t)
                                .map_err(|e| ExecError::new(ErrorCode::TenantRefused, e))?,
                        ),
                        None => None,
                    },
                };
                self.set_policy(space, &change, caller, self.members.at())
                    .await?
            }
            ADMISSION => self.store.get(space).await.map_err(ExecError::from)?,
            other => {
                return Err(ExecError::internal(format!(
                    "`{other}` is not an admission op"
                )))
            }
        };
        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "open": policy.open,
                "role": policy.default_role,
                "providers": render_providers(&policy.providers),
                "firstComer": policy.first_comer,
                "secondFactor": policy.second_factor.as_str(),
                "presenceFrom": policy.presence_from.join(","),
                "ceiling": self.ceiling.words().join(","),
            })],
            next_cursor: None,
        })
    }
}

/// What a `setAdmission` changes. `None` leaves a field alone.
#[derive(Debug, Clone, Default)]
pub struct PolicyChange {
    pub open: Option<bool>,
    pub default_role: Option<String>,
    pub providers: Option<Vec<ProviderRule>>,
    pub first_comer: Option<bool>,
    pub second_factor: Option<TenantMode>,
    pub presence_from: Option<Vec<String>>,
}

/// A comma list of connection names: 1 to 32 of a-z, 0-9 and `-` each.
/// Empty is none.
pub fn parse_presence_from(list: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for c in list.split(',').map(str::trim).filter(|c| !c.is_empty()) {
        if !ordo_identity::oauth2::valid_connection_name(c) {
            return Err(format!(
                "`{c}` in `presenceFrom` is not a connection name: 1 to 32 of a-z, 0-9 and -"
            ));
        }
        if !out.iter().any(|x| x == c) {
            out.push(c.to_string());
        }
    }
    Ok(out)
}

/// Re-read every tenant's `presence_from` into `acceptance`.
pub async fn load_presence_from(
    pool: &Pool,
    acceptance: &ordo_identity::UpstreamAcceptance,
) -> Result<usize, ordo_identity::StoreFault> {
    let rows = pool
        .rows(r#"SELECT "space", "presence_from" FROM "$admissionpolicy" WHERE "presence_from" <> ''"#)
        .await
        .map_err(|e| crate::auth::store_failure::pool_fault(e, "the tenants' accepted two-step claims could not be read"))?;
    let mut fresh = std::collections::BTreeMap::new();
    for r in &rows {
        let (Some(Some(space)), Some(Some(list))) = (r.first(), r.get(1)) else {
            continue;
        };
        if let Ok(names) = parse_presence_from(list) {
            if !names.is_empty() {
                fresh.insert(
                    ordo_identity::SpaceId(space.clone()),
                    names.into_iter().collect(),
                );
            }
        }
    }
    let n = fresh.len();
    acceptance.replace_tenants(fresh);
    Ok(n)
}

// ── first-comer, without a claim code ────────────────────────────────────

/// The setting that admits a first-comer: a tenant's `firstComer`, set
/// with `std.space.setAdmission`, off by default, and allowed only under
/// `ORDO_AUTH_PROVISIONING=auto-onboard`.
pub const FIRST_COMER_SETTING: &str =
    "`firstComer=true` (std.space.setAdmission), under ORDO_AUTH_PROVISIONING=auto-onboard";

/// What a sign-in that still carries a claim code is told when it is not
/// admitted. The same words whatever the tenant's policy, so they say
/// nothing about it (owner, 2026-10-30: the claim code is removed).
pub const CLAIM_CODE_GONE: &str = "claim codes are no longer read. The first person to \
     arrive at an empty tenant owns it only when its owner set `firstComer=true` \
     (std.space.setAdmission), which is off by default and allowed under \
     ORDO_AUTH_PROVISIONING=auto-onboard. The deployment's own space never admits a \
     first-comer";

/// The refusal of `firstComer=true` in the deployment's own space.
pub const HOME_FIRST_COMER_REFUSED: &str = "the deployment's own space never admits a \
     first-comer: deployment people come from the identity service \
     (`ordo-identity-service invite`). `firstComer` may be true in an app tenant";

/// The boot-log line of a door whose environment still sets
/// `ORDO_AUTH_CLAIM_CODE` (or its `_FILE` form). Never a refusal.
pub fn ignored_claim_code_note(get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let set = |k: &str| get(k).is_some_and(|v| !v.trim().is_empty());
    (set(CLAIM_CODE_ENV) || set(&format!("{CLAIM_CODE_ENV}_FILE"))).then(|| {
        format!(
            "{CLAIM_CODE_ENV} is set and ignored: claim codes were removed. First-comer \
             admission is a tenant's {FIRST_COMER_SETTING}, off by default. Remove the \
             {CLAIM_CODE_ENV} line"
        )
    })
}

/// The removed setting, named only to say it is ignored.
pub const CLAIM_CODE_ENV: &str = "ORDO_AUTH_CLAIM_CODE";
