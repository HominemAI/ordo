//! **Bringing a space into existence**: the one entry point, and the order
//! it runs in.
//!
//! Serving many spaces ([`crate::spaces`]) is inert without a way to create
//! the second one, and until this module there was none: `deployment_engine`
//! derived the door's memberships from the `members` table, `Pool::connect`
//! created the platform tables once for the whole database, and nothing
//! anywhere minted custody for a space the fixture's seed had not already
//! written. Two suites built a second space by hand (a `create_space` here,
//! a `set_membership` there, an `INSERT` somewhere else), and a hand-built
//! space is a space whose half-built states nobody has thought about.
//!
//! # The order, and the argument for it
//!
//! 1. **The name.** Refused first, because a space whose id the door cannot
//!    bind ([`ordo_api::well_formed_space`]) is a tenant with keys, rows and
//!    a chain that no request will ever reach. Nothing is written for a name
//!    that cannot be used.
//! 2. **Custody** ([`ordo_crypto::KeyProvider::create_space`]). Before any
//!    row, because a row in a space with no DEK is a row whose sealed fields
//!    cannot be written and whose blind indexes cannot be computed. There is
//!    no ordering in which rows may precede keys.
//! 3. **The founding member row.** Last, and it is what makes the space
//!    *exist*: [`crate::deployment_engine`] reads the `members` table to
//!    build the door's memberships and profiles, so a space with no member
//!    row grants nobody anything and is reachable by no one.
//!
//! # What an interruption leaves, and what a retry does
//!
//! Only one intermediate state is reachable, and it is inert. A crash
//! between (2) and (3) leaves custody with no member row: no principal is a
//! member, so the door denies every request naming the space, and
//! [`crate::spaces::Spaces::resolve`] would serve it but is never reached.
//! No data exists to be half-visible, because there are no rows.
//!
//! Re-running the same call completes it. Every step is idempotent by
//! construction rather than by a check this function makes:
//! `create_space` keeps the SMK a space already has (minting a second would
//! strand everything sealed under the first), and the founding row is an
//! `ON CONFLICT DO NOTHING` on the primary key the space shares with it.
//! So provisioning is safe to retry after an interruption, after a success,
//! and after a partial success, and none of the three needs to be told
//! apart.
//!
//! The direction is the mirror of [`crate::exit_ops`]: deletion destroys
//! custody *first*, so a half-deleted space is already unreadable; creation
//! writes it first, so a half-created space is not yet reachable. Both
//! orders put the irreversible act where an interruption is safe.
//!
//! # The one bootstrap, stated rather than hidden
//!
//! `members` is a platform entity, and the platform's membership surface
//! (`ordo_conformance::membership`) refuses a command whose actor is not
//! already a member of the space, per DL §3's *"membership commands are
//! administered from inside the space"*. The founding owner cannot satisfy
//! that: there is nobody inside the space yet. So this row is written
//! directly, and it is the **only** `members` row in a space's life that is,
//! every later one going through the attributed surface. For the same
//! reason it appends no command group: a group carries an actor, and the
//! actor this row would name is the row itself.
//!
//! **Owed, and named here rather than discovered:** the record that a space
//! was created belongs beside the record that one was destroyed, which
//! `exit_ops` writes to the security-event stream as a `CustodyAction`.
//! This function takes no recorder because it has no door in front of it
//! yet: who may create a space is the authorization increment's question,
//! and a creation record with no authenticated creator in it would record
//! the wrong half.

use ordo_crypto::{KeyProvider, SpaceId};
use ordo_exec::route::{target_config, DbCredentials};
use ordo_exec::spacedb::{self, AttachRefusal, DbAttachOutcome, DbAttachment};
use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError};

// ── the system space (checklist 17v) ─────────────────────────────────────

/// **The platform's own home, and its name is the platform's to choose.**
///
/// Owner-ruled 2026-09-20: *"System space should exist at boot because
/// nothing can really exist without it, it is where ordo configurations
/// live."* It extends the `system-space` ruling of 2026-09-12, which named
/// the third home: the control plane holds identity and access, tenant
/// spaces hold customer data, and platform-owned runtime content (a tool
/// catalogue, system agents, reference data, a deployment's own
/// configuration) had nowhere to live.
///
/// # How a reader tells this from a tenant, in code
///
/// A tenant's id arrives from outside the binary: `ORDO_SPACE`, or a
/// request header, or an operator's `ordo keys init --space`. This one is a
/// constant compiled into the binary and is read from nothing, so no typo
/// anywhere can select it and no operator can rename it. That is the whole
/// of the distinction [`crate::runtime`]'s boot rests on: the space it
/// creates without being asked is the one whose name it already held, and
/// every other space is still created only when somebody asks for it by
/// name.
///
/// # Where the id itself lives, and the `$` it does not carry
///
/// [`ordo_exec::erase`], because that is the module that must refuse to
/// erase it and `ordo-exec` cannot see `ordo-host`. Its neighbour there is
/// `SYSTEM_SPACE_PREFIX`, the `$` reserved for platform-owned spaces a
/// caller cannot name. This id carries no `$` for a reason worth stating
/// once: the boot's system space is *addressed*. It has a founding member
/// row, the access engine is built from that row, and
/// [`ordo_api::well_formed_space`] binds only alphanumerics, `_` and `-`,
/// so a `$` id could not be provisioned here and could not arrive in a
/// request header either. `ordo_exec::erase::is_system_space` answers for
/// both, which keeps "nobody erases it" one function rather than two.
///
/// # What it holds
///
/// Today: custody of its own, and a founding member row, which is what
/// makes it a space the door can serve rather than a row in a key table.
/// As the platform gains configuration it does not keep in environment
/// variables, such as a catalogue, deployment-wide settings and its own
/// reference data, this is where those rows go: they are rows the platform
/// is genuinely the author of. The 2026-09-12 ruling's test for
/// placement is authorship, and it still governs: a third party's content
/// belongs in the third party's own space, shared outward, never here.
///
/// # What it does not get
///
/// No exemption from anything else. It takes a chain, guards, versioning
/// and deploys exactly as a tenant does. Its distinguishing properties are
/// that nobody erases it and, once the universal read window exists,
/// everybody reads it.
pub const SYSTEM_SPACE: &str = ordo_exec::erase::SYSTEM_SPACE;

/// The founding member row's id in [`SYSTEM_SPACE`]. Fixed rather than
/// minted, so every retry and every restart addresses the same row instead
/// of adding a second owner beside the first.
pub const SYSTEM_MEMBER_ID: &str = "system-owner";

/// The principal the system space's founding row names.
///
/// `system:` is the [`ordo_commandlog::Actor::System`] prefix, which is how
/// every other platform-authored act in this codebase already attributes
/// itself ([`SYSTEM_ACTOR`] is the same shape). It is deliberately not a
/// principal any credential can authenticate as: the platform's own home
/// has a member row so that the door has something to resolve, and nobody
/// logs in as it.
pub const SYSTEM_PRINCIPAL: &str = "system:platform";

/// What the boot found, or did, about [`SYSTEM_SPACE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemSpace {
    /// This boot wrote the founding row. A first deployment, once.
    Created,
    /// It was already there: every boot after the first.
    Present,
    /// Custody exists and the founding row does not, because `members`
    /// does not exist yet, which is a database no `ordo apply` has run
    /// against.
    /// The space is inert in exactly the way [`provision_space`]'s module
    /// doc describes, and the next boot after the first apply completes it.
    CustodyOnly,
}

impl std::fmt::Display for SystemSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SystemSpace::Created => f.write_str("created at boot"),
            SystemSpace::Present => f.write_str("present"),
            SystemSpace::CustodyOnly => f.write_str(
                "custody only; the founding member row lands at the first boot after \
                 `ordo apply`",
            ),
        }
    }
}

/// Bring [`SYSTEM_SPACE`] into existence if it is absent, at boot.
///
/// One call to [`provision_space`], never a second path: the order custody
/// before rows, the idempotence of both halves, and the retry semantics are
/// argued once in this module's doc and this reuses all of them. A boot on
/// a live deployment writes nothing and answers [`SystemSpace::Present`].
///
/// # The one fork, and why it is not a refusal
///
/// `members` is created by a *program's* DDL, so a database nobody has run
/// `ordo apply` against does not have it, and that is the database a first
/// deployment is pointed at. Refusing there would make the ordinary first
/// deploy impossible, which is the same argument `assemble`'s
/// `NoActiveVersion` arm already makes about the registry. So custody is
/// minted either way (it is what the platform cannot exist without, and it
/// needs no table a program owns) and the founding row waits for a
/// directory to write it into. The question is asked with `to_regclass`
/// rather than by matching an error string, for
/// [`crate::deployment_engine_pooled`]'s reason: a directory that does not
/// exist is an un-applied deployment, and a directory that exists and will
/// not read is a failure and stays one.
pub async fn ensure_system_space(
    pool: &Pool,
    keys: &dyn KeyProvider,
) -> Result<SystemSpace, ProvisionRefusal> {
    if !directory_exists(pool).await? {
        keys.create_space(&SpaceId(SYSTEM_SPACE.to_string()))
            .map_err(ProvisionRefusal::Custody)?;
        return Ok(SystemSpace::CustodyOnly);
    }
    let owner = FoundingOwner {
        member_id: SYSTEM_MEMBER_ID,
        principal: SYSTEM_PRINCIPAL,
        display_name: "Ordo platform",
    };
    Ok(
        match provision_space(pool, keys, SYSTEM_SPACE, &owner).await? {
            Provisioned::Created { .. } => SystemSpace::Created,
            Provisioned::AlreadyExists { .. } => SystemSpace::Present,
        },
    )
}

/// The deployment owners the environment names (`plan-provisioning-on-signup.md`
/// §3, P0): a comma-separated list of principal ids.
pub const DEPLOYMENT_OWNERS_ENV: &str = "ORDO_DEPLOYMENT_OWNERS";

/// Parse the value of [`DEPLOYMENT_OWNERS_ENV`].
///
/// Entries are trimmed and blanks skipped, so a trailing comma is harmless.
/// An id the door could never address is refused with the entry named,
/// because it would otherwise become a `members` row nobody could reach and
/// the deployment would still have no owner. A repeated id is kept once.
pub fn parse_deployment_owners(text: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        if !ordo_api::well_formed_space(entry) {
            return Err(format!(
                "`{entry}` is not a principal id the door can address: letters, digits, `_` \
                 and `-`"
            ));
        }
        if !out.iter().any(|o| o == entry) {
            out.push(entry.to_string());
        }
    }
    Ok(out)
}

/// The deployment owners this door reads from the environment
/// (`plan-identity-service.md` step 8; owner, 2026-10-30).
///
/// A door without the identity service reads [`DEPLOYMENT_OWNERS_ENV`] as
/// it always has: a malformed id refuses the boot. A door with
/// `ORDO_IDENTITY_URL` set reads nothing from it: deployment people live
/// in the identity service, and the value is never parsed, so it can never
/// refuse a boot. [`ignored_deployment_owners_note`] is what such a boot
/// logs.
pub fn deployment_owners_from(get: &dyn Fn(&str) -> Option<String>) -> Result<Vec<String>, String> {
    if identity_service_set(get) {
        return Ok(Vec::new());
    }
    match get(DEPLOYMENT_OWNERS_ENV) {
        Some(v) => parse_deployment_owners(&v),
        None => Ok(Vec::new()),
    }
}

/// The boot-log line of a door with the identity service whose
/// environment still names [`DEPLOYMENT_OWNERS_ENV`]. `None` when there is
/// nothing to say.
pub fn ignored_deployment_owners_note(get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let set = get(DEPLOYMENT_OWNERS_ENV).is_some_and(|v| !v.trim().is_empty());
    (set && identity_service_set(get)).then(|| {
        format!(
            "{DEPLOYMENT_OWNERS_ENV} is set and ignored: {} is set, so deployment people \
             live in the identity service. Invite people with `ordo-identity-service \
             invite`, then remove the {DEPLOYMENT_OWNERS_ENV} line",
            crate::identity_keys::URL_ENV
        )
    })
}

fn identity_service_set(get: &dyn Fn(&str) -> Option<String>) -> bool {
    get(crate::identity_keys::URL_ENV).is_some_and(|v| !v.trim().is_empty())
}

/// What [`ensure_deployment_owners`] did for one named principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerOutcome {
    /// An owner row was written by this call.
    Added,
    /// The principal is already an active owner. Nothing was written.
    AlreadyOwner,
    /// The principal already holds a row that is not an active owner: a
    /// lower role, or suspended. Left alone, because a role change or a
    /// suspension is somebody's act and a boot does not undo it. Changing
    /// it is `std.space.setRole` or `std.space.reinstate`.
    Kept { role: String, status: String },
    /// Nothing could be written yet: the home space has no `members`
    /// directory (no `ordo apply` has reached it) or no custody (the name is
    /// not a provisioned tenant). The next boot, or the first deploy, writes
    /// the row.
    Waiting(&'static str),
}

/// Make each principal the environment names an owner of the deployment's
/// own space (P0).
///
/// **Adds, never removes.** Removing a deployment owner is
/// `std.space.remove`, and the variable is edited to match. **Idempotent:**
/// an existing row for the principal is never duplicated, so every restart
/// writes nothing after the first. A principal that has not signed in yet is
/// written anyway; the row waits for the person, who is found by principal
/// id when they arrive.
///
/// The act is recorded on the deployment's stream as
/// `Enrolment { act: "deployment-owner" }`, mirrored to the home space's
/// stream, and only when a row was written.
pub async fn ensure_deployment_owners(
    pool: &Pool,
    keys: &dyn KeyProvider,
    home: &str,
    owners: &[String],
    stream: Option<&ordo_secevent::Recorder>,
    now: u64,
) -> Result<Vec<(String, OwnerOutcome)>, ProvisionRefusal> {
    if owners.is_empty() {
        return Ok(Vec::new());
    }
    let waiting = if !directory_exists(pool).await? {
        Some("the members directory does not exist yet (no `ordo apply` has reached it)")
    } else if keys.current_epoch(&SpaceId(home.to_string())).is_err() {
        Some("the deployment's own space has no key custody")
    } else {
        None
    };
    let mut out = Vec::with_capacity(owners.len());
    for principal in owners {
        if let Some(why) = waiting {
            out.push((principal.clone(), OwnerOutcome::Waiting(why)));
            continue;
        }
        let added =
            crate::member_ops::MemberOps::admit_row(pool, home, principal, "owner", principal)
                .await
                .map_err(|e| {
                    ProvisionRefusal::Malformed(format!("the owner row: {}", e.message))
                })?;
        let outcome = if added.is_some() {
            if let Some(stream) = stream {
                crate::enrolment::record(
                    stream,
                    &crate::enrolment::Act {
                        act: "deployment-owner",
                        detail: format!("principal={principal} role=owner way=environment"),
                        by: None,
                        tenant: Some(home),
                        deployment: true,
                        at: now,
                    },
                );
            }
            OwnerOutcome::Added
        } else {
            let rows = pool
                .rows(&format!(
                    "SELECT \"role\", \"status\" FROM \"members\" \
                     WHERE \"$space\" = {} AND \"principal\" = {}",
                    sql_lit(home),
                    sql_lit(principal)
                ))
                .await
                .map_err(ProvisionRefusal::Store)?;
            let cell = |i: usize| {
                rows.first()
                    .and_then(|r| r.get(i))
                    .and_then(|c| c.clone())
                    .unwrap_or_default()
            };
            let (role, status) = (cell(0), cell(1));
            if role == "owner" && status == "active" {
                OwnerOutcome::AlreadyOwner
            } else {
                OwnerOutcome::Kept { role, status }
            }
        };
        out.push((principal.clone(), outcome));
    }
    Ok(out)
}

/// Does the directory table a founding row goes into exist yet?
async fn directory_exists(pool: &Pool) -> Result<bool, ProvisionRefusal> {
    let rows = pool
        .rows(r#"SELECT to_regclass('"members"') IS NOT NULL"#)
        .await
        .map_err(ProvisionRefusal::Store)?;
    Ok(matches!(
        rows.first().and_then(|r| r[0].as_deref()),
        Some("t") | Some("true")
    ))
}

/// Who the space belongs to, at the moment it comes into existence.
///
/// A space with no owner is a space nobody can administer, and the roles
/// are not a hierarchy (nothing is implied by absence), so the founding
/// membership is `Role::Owner` and is not a parameter.
pub struct FoundingOwner<'a> {
    /// The `members` row id. Supplied rather than minted, so that a retry
    /// after an interruption addresses the same row instead of adding a
    /// second owner beside the first.
    pub member_id: &'a str,
    /// The identity-layer principal this member is. What a token proves.
    pub principal: &'a str,
    /// What a roster screen shows. Free text, quoted like any value.
    pub display_name: &'a str,
}

/// Why a space was not provisioned.
#[derive(Debug)]
#[non_exhaustive]
pub enum ProvisionRefusal {
    /// A space id, or a member id, the door could never bind or address.
    Malformed(String),
    /// Key custody could not be minted. Nothing was written.
    Custody(ordo_crypto::KeyError),
    /// The founding row could not be written. Custody exists; a retry
    /// completes the space.
    Store(PoolError),
    /// The space already holds rows on the default database. Attaching it
    /// elsewhere would leave them behind, which is a move under another
    /// name, and moving a live space is not built. Nothing was written.
    Occupied { space: String, rows: i64 },
    /// The space is already attached to a different database. Nothing was
    /// written.
    Attach(AttachRefusal),
    /// The target database could not be created or prepared. Custody may
    /// exist and the database may exist; a retry completes both.
    Target(PoolError),
    /// **The attachment names the control database itself** (18d R-X2).
    ///
    /// A tenant pointed at the control plane is a tenant whose rows live
    /// among identity, grants, `$artifacts` and every other tenant's
    /// attachment. Refused by the target's identity rather than by its
    /// name, because `localhost` and `127.0.0.1` and the pod's own DNS
    /// name are three spellings of one database and a check on the string
    /// would catch one of them.
    ControlDatabase { space: String },
    /// **The database already holds a tenant** (18d D-D).
    ///
    /// Ruled 2026-09-24: *"1 tenant 1 database. same for buckets.
    /// database/schema not cluster."*
    ///
    /// **One refusal, and custody does not enter into it.** This was built
    /// first as two — a second tenant into someone's own perimeter, and an
    /// `OwnPerimeter` claim over an occupied database — on the reading that
    /// sharing was a problem only where a tenant had been told the
    /// perimeter was theirs. The ruling is broader than that question: a
    /// database holds one tenant, whatever the custody column says, so
    /// there is one condition to check and `DbCustody` is not part of it.
    ///
    /// **The cluster is not the unit; the database is.** Many tenants on
    /// one Postgres server is the ordinary case and stays supported — what
    /// may not be shared is the database. That is what keeps
    /// `ORDO_PG_ATTACH_HOSTS` a list of hosts rather than a list of
    /// tenants, and what keeps the cost of a tenant a database rather than
    /// a server.
    DatabaseOccupied { space: String, holder: String },
    /// **The attachment's host is not one this deployment may dial**
    /// (18d R-X3).
    ///
    /// The host is an address chosen by data, which is the class of thing
    /// `ordo-egress` exists to refuse for HTTP. A Postgres dial does not
    /// pass through that kernel, so the allowlist is applied here,
    /// before the row is written and before any dial.
    ///
    /// **One reason, never several.** `ordo_egress::refusal`'s argument
    /// applies unchanged: a distinguishable reason is an oracle, and a
    /// caller who learns *why* a host was refused learns the shape of the
    /// allowlist one attempt at a time.
    HostNotAllowed { host: String },
    /// **A schema apply has the routing lock and did not give it up in
    /// time** (18d R-S5).
    ///
    /// Not a rejection of anything the caller asked for: the attach waited
    /// `apply_op::LOCK_WAIT_MS` for a deploy to finish and the deploy is
    /// still going. Nothing was written, and the identical call lands once
    /// the apply completes.
    RoutingBusy { space: String },
}

impl std::fmt::Display for ProvisionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionRefusal::Malformed(m) => f.write_str(m),
            ProvisionRefusal::Custody(e) => write!(f, "key custody: {e}"),
            ProvisionRefusal::Store(e) => write!(f, "the founding member row: {e}"),
            ProvisionRefusal::Occupied { space, rows } => write!(
                f,
                "space `{space}` already holds {rows} row(s) on the default database, and \
                 attaching it elsewhere would leave them behind: moving a live space is not built"
            ),
            ProvisionRefusal::Attach(e) => write!(f, "the attachment: {e}"),
            ProvisionRefusal::Target(e) => write!(f, "the space's database: {e}"),
            ProvisionRefusal::ControlDatabase { space } => write!(
                f,
                "`{space}` cannot be attached to this deployment's own control database: its \
                 rows would live among identity, grants and every other tenant's attachment"
            ),
            ProvisionRefusal::DatabaseOccupied { space, holder } => write!(
                f,
                "`{space}` cannot be attached to a database `{holder}` is already in: a \
                 database holds one tenant. Another database on the same server is fine; \
                 the same database is not"
            ),
            ProvisionRefusal::RoutingBusy { space } => write!(
                f,
                "`{space}` could not be attached because a schema apply is in flight and \
                 holds the routing lock. Nothing was created, and the identical call lands \
                 once the apply finishes"
            ),
            ProvisionRefusal::HostNotAllowed { host } => write!(
                f,
                "`{host}` is not a host this deployment may attach. Set \
                 ORDO_TENANT_ATTACH_HOSTS to the hosts an operator permits"
            ),
        }
    }
}

impl std::error::Error for ProvisionRefusal {}

/// What provisioning did, for a log line that distinguishes the two.
///
/// Both arms are a success. They are told apart because "the space already
/// existed" is the ordinary answer to a retry and must not read as a space
/// having just been created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provisioned {
    /// The founding row was written by this call.
    Created { space: String },
    /// It was already there. Custody was confirmed either way.
    AlreadyExists { space: String },
}

/// Provision one space: custody, then the founding owner.
///
/// Safe to call twice, and safe to call again after a crash at any point
/// inside it. See the module doc for the order and what each interruption
/// leaves behind.
///
/// The door does not see the new space until the access engine is next
/// built from the `members` table. Refreshing a *live* engine is the
/// authorization increment's work and deliberately not attempted here: an
/// engine swapped under an in-flight request is the half-swap
/// [`crate::deploy`] exists to describe, and it wants the same treatment
/// rather than a second one invented in passing.
pub async fn provision_space(
    pool: &Pool,
    keys: &dyn KeyProvider,
    space: &str,
    owner: &FoundingOwner<'_>,
) -> Result<Provisioned, ProvisionRefusal> {
    if !ordo_api::well_formed_space(space) {
        return Err(ProvisionRefusal::Malformed(format!(
            "`{space}` is not a space id this door can bind, so nothing that \
             named it could ever be reached"
        )));
    }
    // The same test, for the same reason, one level down: a row id with a
    // quote or a space in it is a row no `$blobref`, no ref and no guard
    // could address by literal.
    if !ordo_api::well_formed_space(owner.member_id) {
        return Err(ProvisionRefusal::Malformed(format!(
            "`{}` is not a well-formed members row id",
            owner.member_id
        )));
    }

    keys.create_space(&SpaceId(space.to_string()))
        .map_err(ProvisionRefusal::Custody)?;

    let written = write_founding_row(pool, space, owner)
        .await
        .map_err(ProvisionRefusal::Store)?;

    Ok(if written {
        Provisioned::Created {
            space: space.to_string(),
        }
    } else {
        Provisioned::AlreadyExists {
            space: space.to_string(),
        }
    })
}

/// The founding member row, written directly into whichever database the
/// space's rows live in, and whether this call wrote it.
///
/// `ON CONFLICT DO NOTHING` over the primary key `(space, id)`, which is
/// what makes the retry quiet, and `RETURNING` so the two successes can
/// still be told apart: the clause yields a row only when one was actually
/// inserted. `rev` is 1 because this row was created, not converged;
/// `joined` is the day custody was minted, which is the only date this
/// function is in a position to know.
async fn write_founding_row(
    pool: &Pool,
    space: &str,
    owner: &FoundingOwner<'_>,
) -> Result<bool, PoolError> {
    let sql = format!(
        "INSERT INTO \"members\" \
         (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \"kind\", \
          \"memberCount\", \"role\", \"status\", \"joined\") \
         VALUES ({space}, {id}, 1, {principal}, {display}, 'principal', 1, 'owner', \
                 'active', CURRENT_DATE) \
         ON CONFLICT DO NOTHING RETURNING \"id\"",
        space = sql_lit(space),
        id = sql_lit(owner.member_id),
        principal = sql_lit(owner.principal),
        display = sql_lit(owner.display_name),
    );
    Ok(!pool.rows(&sql).await?.is_empty())
}

// ── provisioning onto a database of its own (plan-space-routing S5) ─────

/// Who chooses a new space's database: the system. Owner-ruled 2026-09-13
/// (`db-attach-gate`): at provisioning there is no owner yet to consent, and
/// no space without a place to store it. Stored as `attached_by` and named
/// on the deployment stream, so the record says the platform chose.
pub const SYSTEM_ACTOR: &str = "system:provisioning";

/// Where a new space's rows will live, and what is needed to put them there.
///
/// **There is no schema here.** Until R-S1 this struct carried the DDL the
/// new database would receive, which made the schema a caller's choice: two
/// call sites could hand the same deployment two different schemas, and a
/// caller holding a stale `Schema` could give a tenant a database the
/// running program cannot read. What a target receives is a property of the
/// deployment and not of whoever asked, so [`prepare_target`] reads it from
/// [`crate::deploy::Deployment::current`] and this struct says only where
/// the rows go.
pub struct Target<'a> {
    /// The database, as the system chose it. `attached_by` and
    /// `attached_at` are overwritten: the chooser is [`SYSTEM_ACTOR`] and
    /// the time is the call's.
    pub attachment: &'a DbAttachment,
    /// Supplies the password for the attachment's credential label.
    pub credentials: &'a dyn DbCredentials,
    /// Everything a target pool needs that the attachment does not say.
    pub template: &'a PoolConfig,
    /// The hosts this deployment may attach (R-X3). See [`HostAllowlist`]
    /// for why it arrives here rather than being read where it is checked.
    pub hosts: &'a HostAllowlist,
}

/// Everything phase 0 decided, carried into the phase that writes.
///
/// `bootstrap.md` §1.2 makes preflight a **phase of its own** — *"every
/// decidable failure … is rejected in phase 0, before the registry row
/// exists"* — and until 11-15 this code had no such seam. Every refusal
/// below lived inside [`prepare_target`], which ran *after*
/// [`crate::tenants::reserve`], so an operator who mistyped a host got a
/// `failed` registry row rather than an answer. That row is a lie in the
/// one place the platform cannot afford one: `tenants::troubled` is §4.1's
/// single detection query, and a mistyped parameter is not a stuck
/// provision. A caller who passes phase 0 and then fails has hit an
/// infrastructure fault; a caller who fails phase 0 has made a mistake,
/// and mistakes leave nothing behind.
///
/// **It carries the apply lock**, which is the whole reason this is a value
/// and not two independent calls. R-S5 wants *one* lock held across the
/// occupancy read and the attach, so that a tenant attached while an apply
/// is walking `targets()` cannot receive a schema the walk has already
/// passed. Taking it in phase 0 and dropping it before phase 1 would open
/// exactly that window. So the lock is taken here, rides through the
/// registry write, and is released when this value is dropped — which also
/// means dropping a `Preflight` is the whole of abandoning a dry run.
///
/// The one statement phase 0 issues is `spacedb::ensure_table` on the
/// control plane's own attachment table. It is idempotent, it belongs to
/// the deployment rather than to the space being planned, and it leaves
/// nothing any reap would have to find — but it is a write, and saying
/// "phase 0 writes nothing" without that sentence would be false.
pub struct Preflight {
    /// Held, not used. Dropping it ends the transaction, which releases.
    _lock: crate::apply_op::ApplyLock,
    sid: SpaceId,
    want: DbAttachment,
    db: String,
    cfg: ordo_exec::PoolConfig,
    artifact: std::sync::Arc<crate::deploy::Loaded>,
    /// Whether the target database was already standing when phase 0
    /// looked. `true` means this provision will adopt a database rather
    /// than create one, which is either a retry finishing or an operator
    /// pointing at something that exists.
    standing: bool,
    /// How many spaces already sit on the target. Zero for the ordinary
    /// case; a non-zero count that is not this space is what
    /// [`ProvisionRefusal::DatabaseOccupied`] already refused above.
    occupancy: usize,
}

impl Preflight {
    /// The resolved target, in the spelling the routing layer will use for
    /// it. Credential **labels** only: there is no secret in a `Preflight`
    /// and nothing here may ever be rendered into a plan that reaches a
    /// caller if that changes.
    pub fn label(&self) -> String {
        ordo_exec::route::target_label(&self.want.host, self.want.port, &self.want.db)
    }

    /// The database this provision would create or adopt.
    pub fn database(&self) -> &str {
        &self.db
    }

    /// The artifact version whose schema would be applied to it.
    pub fn version(&self) -> &str {
        &self.artifact.version
    }

    /// Whether the database is already standing. See the field.
    pub fn standing(&self) -> bool {
        self.standing
    }

    /// How many spaces already sit on the target.
    pub fn occupancy(&self) -> usize {
        self.occupancy
    }
}

/// What preparing a target did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// This call attached the space. The one outcome that is a custody
    /// event, and the one that is recorded.
    Attached { space: String, db: String },
    /// The identical attachment was already there: a retry, and not a
    /// custody event.
    AlreadyAttached { space: String, db: String },
}

/// Give a new space a database of its own: every step of provisioning up
/// to, and not including, the founding member.
///
/// # The order, and what each interruption leaves
///
/// 1. **The names**, the space's and the database's. Nothing is written for
///    a name that cannot be used.
/// 2. **The space holds nothing on the default database**, and **is not
///    attached elsewhere.** Both refusals come before anything is written, so
///    a refused call leaves nothing behind: no keys, and no empty database
///    nobody will use.
/// 3. **Custody.** Keys before any row, for [`provision_space`]'s reason.
/// 4. **The database.** `CREATE DATABASE` cannot run in a transaction, so
///    "already exists" counts as done.
/// 5. **The schema and the record of which version it is**, all or nothing
///    together, skipped if it already ran. See *the schema is the
///    deployment's* below.
/// 6. **The attachment**, written after the database is ready. So an
///    interruption anywhere above leaves a database no row points at, which
///    serves nothing, and a retry completes it; there is no state in which
///    the attachment names a database that is not ready.
/// 7. **The record**, on the deployment's security stream as a custody
///    action, and only when this call attached. The space's own stream does
///    not exist yet, and a retry is not a custody event.
///
/// # What comes after, and why it is not here
///
/// The founding member. Where a space's members live once it has a
/// database of its own is open (`membership-home` on the decisions doc):
/// the door reads members from the control database, and the space's rows
/// reference them through foreign keys that cannot cross databases. Until
/// that is ruled, a space prepared here has a database and no member, which
/// the door treats as it treats every space with no member: it denies.
///
/// The attachment precedes the member by construction, which is the
/// obligation S3 recorded: no request can be authorized in a space before
/// its route is final, so no node can have cached "default" for it.
///
/// # The schema is the deployment's, read here, and recorded with itself
///
/// `plan-space-routing.md` R-S1. `serving` is the live
/// [`crate::deploy::Deployment`] and the schema this function applies is
/// [`crate::deploy::Deployment::current`]`().fixture.ddl`, read **inside
/// this call**. The
/// caller passes a deployment rather than a schema, so there is no argument
/// through which a different schema could arrive: not a stale `Schema` a
/// caller has been holding since before a deploy, not a hand-built one from
/// a test, and not two call sites that disagree. A tenant's database gets
/// the shape the program serving right now expects, or the call does not
/// get that far.
///
/// Reading it here rather than at the call site also fixes *when*: the
/// version a target records is the one that was serving at the moment its
/// tables were created, which is the only moment at which that claim is
/// true. A caller that read `current()` first and called afterwards would
/// record a version a deploy may already have replaced.
///
/// The `$migrations` row goes in with the tables, in one implicit
/// transaction, so this function has no reachable state in which the target
/// holds the schema and no record of it
/// ([`spacedb::apply_schema_at_version`] argues the property in full). A
/// target that already holds `members` is left exactly as it is: if it was
/// created before R-S1 it has no such row, and it reads as version unknown
/// rather than being retrofitted with a guess.
/// The hosts an operator permits an attachment to name (18d R-X3).
///
/// Ruled 2026-09-21 (`db-allowlist`, D-E): *"per tenant configurations are
/// still configurations - configured at the deployment level. egress
/// filtering usually belongs on the infra level, but we can just add a easy
/// allow list."* So this is one list for both tiers, set on the deployment,
/// and it sits beside `ordo-egress` rather than inside it.
///
/// **Read from the environment once, at the edge, and passed down.** The
/// check itself takes the list as an argument and reads no environment, for
/// the same reason [`Target`] carries `credentials` and `template`: a
/// function that reaches for a process global cannot be tested next to
/// another test that wants a different answer, and the tests that matter
/// here are exactly the ones that need both answers at once.
#[derive(Debug, Clone, Default)]
pub struct HostAllowlist(Vec<String>);

impl HostAllowlist {
    /// `ORDO_TENANT_ATTACH_HOSTS` and `ORDO_PG_ATTACH_HOSTS`, comma
    /// separated, exact hostnames or literal addresses, **unioned**.
    ///
    /// **Unset means no attachment may name any host.** That is the
    /// refusing default every other address-shaped control in this tree
    /// takes: `ordo-egress` denies by default and is told what to allow,
    /// and a credential label this node does not hold is refused before a
    /// dial. An allowlist defaulting to open would be a control that
    /// protects only the deployments whose operator already knew to
    /// configure it, which is the set that needed it least.
    ///
    /// # One list for both tiers, and why the union rather than a fallback
    ///
    /// D-S6, ruled 2026-09-21 (`st-endpoint-allowlist`): the bucket
    /// endpoint allowlist *"should be tied in with the db allowlist setup.
    /// implication really is if i defined it on my platform it is
    /// allowed."* So there is one list of hosts the operator declared, and
    /// both `prepare_target` (a Postgres host) and
    /// `bucket_ops::BucketOps::attach` (an S3 endpoint's host) are checked
    /// against that one list through this one type. A second list would be
    /// a second thing to keep in step, and the first host an operator added
    /// to one and not the other would be a refusal nobody could explain
    /// from the refusal.
    ///
    /// `ORDO_TENANT_ATTACH_HOSTS` is the name that says "both tiers", and
    /// `ORDO_PG_ATTACH_HOSTS` is the spelling 18d T8 shipped and the
    /// decisions board already tells operators about. Both are read and the
    /// results are **unioned**, not ranked: with a precedence, an operator
    /// who added the new variable would silently turn the old one off, and
    /// the hosts that vanished would be the ones their database tenants
    /// were already attached to. A union grows and never removes, which is
    /// the only direction that is safe to do behind an operator's back.
    pub fn from_env() -> Self {
        HostAllowlist::of_both(
            &std::env::var("ORDO_TENANT_ATTACH_HOSTS").unwrap_or_default(),
            &std::env::var("ORDO_PG_ATTACH_HOSTS").unwrap_or_default(),
        )
    }

    /// The union, from two values the caller holds.
    ///
    /// Split out of [`HostAllowlist::from_env`] so the union itself is
    /// testable: the two variables cannot both be set and unset in one
    /// process, and a test that reached for `set_var` would be a test
    /// racing every other test in the binary.
    pub fn of_both(tenant: &str, pg: &str) -> Self {
        let mut list = HostAllowlist::of(tenant);
        list.0.extend(HostAllowlist::of(pg).0);
        list.0.sort();
        list.0.dedup();
        list
    }

    /// The same parse, from a value the caller holds.
    pub fn of(raw: &str) -> Self {
        HostAllowlist(raw.split(',').map(fold).filter(|h| !h.is_empty()).collect())
    }

    /// A host this deployment configured, which is allowed by definition.
    ///
    /// Ruled 2026-09-21 (`st-endpoint-allowlist`, D-S6): *"implication
    /// really is if i defined it on my platform it is allowed."* The
    /// allowlist exists to refuse an address **chosen by data**; a cluster
    /// the operator set in the deployment's own environment is not that, so
    /// it does not have to be named twice.
    ///
    /// Without this, `ORDO_PG_ATTACH_HOSTS` being unset would refuse a
    /// deployment its own default tenant cluster, and since every tenant
    /// now has a database that would mean no deployment could create a
    /// tenant until an operator named their own server back to them.
    pub fn allowing(mut self, host: &str) -> Self {
        let host = fold(host);
        if !host.is_empty() && !self.0.contains(&host) {
            self.0.push(host);
        }
        self
    }

    /// **One reason, never several.** `ordo_egress::refusal`'s argument
    /// applies unchanged: a distinguishable reason is an oracle, and a
    /// caller who learns *why* a host was refused maps the allowlist one
    /// attempt at a time. So this answers yes or no and the refusal above
    /// carries no detail about which rule declined it.
    pub fn allows(&self, host: &str) -> bool {
        let host = fold(host);
        self.0.iter().any(|allowed| allowed == &host)
    }
}

/// One spelling of a host, for comparing two.
///
/// **Case is folded for hostnames and kept for socket paths.** A hostname is
/// case-insensitive by definition, so `DB.Internal` and `db.internal` are
/// one host and a list that matched only the spelling the operator typed
/// would be a trap. A Postgres host beginning with `/` is not a hostname at
/// all: it is a **Unix socket directory**, which is a filesystem path, and
/// on a case-sensitive filesystem `/var/run/PG` and `/var/run/pg` are two
/// different directories. Folding both the same way would have been
/// consistent and wrong in one of the two cases.
fn fold(host: &str) -> String {
    let host = host.trim();
    if host.starts_with('/') {
        host.to_string()
    } else {
        host.to_ascii_lowercase()
    }
}

/// Do these two name the same database?
///
/// **Not a string comparison on the host**, because `localhost`,
/// `127.0.0.1` and `::1` are three spellings of one server and a check
/// that compared them literally would catch whichever one the operator
/// happened not to type.
///
/// **What this still does not catch, stated rather than implied:** a DNS
/// name that resolves to the control database's address is not equal to
/// it as a string, so an attachment naming one would pass here. The
/// complete check is to dial and compare Postgres's own
/// `system_identifier`, which is unique per cluster and immune to every
/// spelling. That belongs after the dial and is filed rather than built,
/// because this check runs *before* anything is written and catching the
/// three common spellings there is worth more than catching every
/// spelling later.
fn same_database(a_host: &str, a_port: u16, a_db: &str, b: &PoolConfig) -> bool {
    fn norm(h: &str) -> String {
        let h = h.trim().trim_start_matches('[').trim_end_matches(']');
        match h.to_ascii_lowercase().as_str() {
            "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" => "loopback".to_string(),
            other => other.to_string(),
        }
    }
    norm(a_host) == norm(&b.host) && a_port == b.port && a_db == b.db
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_target(
    control: &Pool,
    keys: &dyn KeyProvider,
    serving: &crate::deploy::Deployment,
    space: &str,
    target: &Target<'_>,
    stream: Option<&ordo_secevent::Recorder>,
    now: u64,
) -> Result<Prepared, ProvisionRefusal> {
    let pre = preflight(control, keys, serving, space, target, now).await?;
    commit_target(control, keys, space, pre, stream, now).await
}

/// **Phase 0.** Decide everything decidable about a target and write
/// nothing that names the space. See [`Preflight`] for why this is a seam
/// and not an implementation detail.
///
/// Every refusal this can return is a caller's mistake or a deployment's
/// misconfiguration: a name that cannot be bound, a host the deployment
/// may not dial, the control database, a busy routing lock, a space that
/// already holds rows or is already attached elsewhere, a database another
/// tenant sits on, or a cluster that does not answer. None of them is an
/// infrastructure fault mid-provision, and none of them should cost a
/// registry row.
pub async fn preflight(
    control: &Pool,
    keys: &dyn KeyProvider,
    serving: &crate::deploy::Deployment,
    space: &str,
    target: &Target<'_>,
    now: u64,
) -> Result<Preflight, ProvisionRefusal> {
    if !ordo_api::well_formed_space(space) {
        return Err(ProvisionRefusal::Malformed(format!(
            "`{space}` is not a space id this door can bind, so nothing that \
             named it could ever be reached"
        )));
    }
    let db = target.attachment.db.clone();
    if !ordo_exec::valid_database_name(&db) {
        return Err(ProvisionRefusal::Malformed(format!(
            "`{db}` is not a database name this platform creates"
        )));
    }
    // **The host, before anything is written and before any dial**
    // (R-X3). An attachment's host is an address this deployment will
    // connect to, chosen by data, which is the class of thing
    // `ordo-egress` refuses for HTTP. A Postgres dial does not pass
    // through that kernel, so the allowlist is applied here.
    if !target.hosts.allows(&target.attachment.host) {
        return Err(ProvisionRefusal::HostNotAllowed {
            host: target.attachment.host.clone(),
        });
    }

    // **The control database is not a target** (R-X2). A tenant pointed
    // at the control plane is a tenant whose rows live among identity,
    // grants, `$artifacts` and every other tenant's attachment. Checked
    // before the occupancy read below, because that read would otherwise
    // run against the very database this is refusing.
    if same_database(
        &target.attachment.host,
        target.attachment.port,
        &target.attachment.db,
        &control.config(),
    ) {
        return Err(ProvisionRefusal::ControlDatabase {
            space: space.to_string(),
        });
    }

    // **Custody answers** (`bootstrap.md` §2 item 22: an unhealthy provider
    // is a *"preflight rejection before the registry row exists — this is
    // the check that keeps phase 2's non-transactionality from mattering in
    // the common case"*).
    //
    // Phase 2 mints this tenant's SMK, and it is the first step that cannot
    // be rolled back: a KMS is an external system with its own durability
    // and no part in a Postgres transaction. When it fails, the compensation
    // is a *scheduled destroy* of a key that may or may not exist, gated on
    // the space never having reached `active`. Asking one cheap question
    // first turns the ordinary case — a provider that is simply down, or a
    // process holding the wrong master — from that residue into an answer.
    //
    // **Asked about the deployment's own space, not the one being created.**
    // The tenant's keys are what this call is deciding whether to mint, so
    // there is nothing there to ask about yet. The system space is created
    // unconditionally at boot and is the one space always present, which is
    // the same reason `/health/deep` asks about it — and it is literally the
    // same question, `ordo_crypto::probe`, so the two cannot drift.
    //
    // It does **not** prove the provider will mint a *new* space's key: a
    // provider can be readable and refuse to create, and that failure still
    // lands in phase 2 and still marks the row `failed`. What this removes
    // is the commoner case, which is the whole of item 22's claim.
    if let Err(e) = ordo_crypto::probe(keys, &SpaceId(SYSTEM_SPACE.to_string())) {
        return Err(ProvisionRefusal::Custody(e));
    }

    // **The apply lock, before the first read and held to the end**
    // (R-S5). A tenant attached while an apply is walking `targets()`
    // receives whichever schema this read finds and is then never revisited
    // by a walk already past it.
    //
    // **`ApplyLock`, not a lock of this increment's own.** R-S5 asks for
    // *one* advisory lock taken by both sides, and the apply side already
    // had one: `apply_op::ApplyLock`, held across the fleet walk and the
    // activation. Its key is derived from `$migrations`, which is exactly
    // the record whose ordering a half-interleaved attach would ruin, and
    // its shape is better than a session lock — transaction scope on a
    // dedicated connection, so a cancelled attach releases it instead of
    // wedging the deployment until a restart.
    //
    // Taken after the three refusals above, because none of them can be
    // changed by a concurrent apply and making a mistyped host wait thirty
    // seconds for a deploy before being told it is a mistyped host would be
    // a worse door.
    //
    // Held by binding: dropping it ends the transaction, which releases, so
    // `let _ =` would release on the same line and leave a lock in the code
    // that locks nothing.
    //
    // **The cost, stated because it is not obvious:** the lock rides a
    // **pooled** connection, and the rest of this function reads the
    // control database on another. So an attach occupies **two** control
    // connections for its whole duration, and N attaches waiting on one
    // apply occupy 2N. That is the same shape `ApplyOps` has had since it
    // was written, and `ORDO_PG_SPACE_RESERVE` is what keeps a pool from
    // being consumed entirely by one space, but a control pool sized 2 can
    // serve exactly one attach at a time. Since 11-15 the window it is held
    // across also contains `tenants::reserve`, which is one more statement
    // on the *other* connection and does not change that count.
    let routing = match crate::apply_op::ApplyLock::take(control).await {
        Ok(Some(lock)) => lock,
        // Waited `LOCK_WAIT_MS` and an apply still has it. **A refusal only
        // after waiting**, which is what the plan asks for: *"the loser
        // waits rather than refusing, because an attach that refuses during
        // every deploy is an attach that looks broken."* Retryable, and
        // nothing has been written.
        Ok(None) => {
            return Err(ProvisionRefusal::RoutingBusy {
                space: space.to_string(),
            })
        }
        Err(e) => return Err(ProvisionRefusal::Target(PoolError::db(e))),
    };

    let sid = SpaceId(space.to_string());
    let want = DbAttachment {
        attached_by: SYSTEM_ACTOR.to_string(),
        attached_at: now as i64,
        ..target.attachment.clone()
    };

    // The one read of the serving artifact, taken before anything is
    // written. Every step below (the occupancy check, the DDL, and the
    // version recorded beside it) is then about one version rather than
    // about whatever was serving at each separate moment.
    let artifact = serving.current();
    let schema = &artifact.fixture.schema;

    let held = ordo_exec::erase::rows_held(control, &sid, schema)
        .await
        .map_err(ProvisionRefusal::Target)?;
    if held > 0 {
        return Err(ProvisionRefusal::Occupied {
            space: space.to_string(),
            rows: held,
        });
    }
    spacedb::ensure_table(control)
        .await
        .map_err(ProvisionRefusal::Target)?;
    if let Some(standing) = spacedb::lookup(control, &sid)
        .await
        .map_err(ProvisionRefusal::Target)?
    {
        if standing.identity() != want.identity() {
            return Err(ProvisionRefusal::Attach(AttachRefusal::WouldMove {
                current: format!("{}:{}/{}", standing.host, standing.port, standing.db),
            }));
        }
    }

    // **One tenant per database** (D-D, ruled 2026-09-24: *"1 tenant 1
    // database. same for buckets. database/schema not cluster."*).
    //
    // Placed after `ensure_table` so the read has a table, and before
    // `create_space` so a refusal leaves no key behind.
    //
    // Custody is deliberately not consulted. The question put to the owner
    // was the narrow one — whether a second tenant inside a declared *own
    // perimeter* is refused — and the answer given was the general rule, so
    // `PlatformManaged` and `OwnPerimeter` are refused alike.
    let sitting = spacedb::occupants(control, &want.host, want.port, &want.db)
        .await
        .map_err(ProvisionRefusal::Target)?;
    if let Some((other, _)) = sitting.iter().find(|(o, _)| o != &sid) {
        return Err(ProvisionRefusal::DatabaseOccupied {
            space: space.to_string(),
            holder: other.0.clone(),
        });
    }

    let cfg = target_config(&want, target.credentials, target.template)
        .map_err(ProvisionRefusal::Target)?;

    // **The cluster answers** (`bootstrap.md` §2 item 26: preflight
    // *"connects and runs a probe transaction; it does not merely parse"*).
    //
    // Last of the phase-0 checks because it is the only one that leaves the
    // process: a mistyped host is refused by the allowlist above without a
    // dial, and there is no point paying a connection timeout to discover
    // something already known. Before this existed, a wrong password or an
    // unreachable port was discovered by `create_database` — after the
    // registry row, after the custody key, and therefore as a `failed`
    // tenant rather than as an answer.
    let standing = ordo_exec::probe(&cfg, &db)
        .await
        .map_err(ProvisionRefusal::Target)?;

    Ok(Preflight {
        _lock: routing,
        sid,
        want,
        db,
        cfg,
        artifact,
        standing,
        occupancy: sitting.len(),
    })
}

/// **Phases 2 and 3.** Mint custody, make the database, apply the schema,
/// and record the attachment — every write [`preflight`] refused to make.
///
/// Takes the [`Preflight`] by value, which is how the type system says the
/// checks happened: there is no way to reach this code without having run
/// phase 0, and no way to run phase 0 twice and commit against the older
/// answer. It takes no `Target` either — everything this needs from one was
/// resolved in phase 0 and travels in the `Preflight`, so there is no
/// second reading of the caller's parameters to disagree with the first.
pub async fn commit_target(
    control: &Pool,
    keys: &dyn KeyProvider,
    space: &str,
    pre: Preflight,
    stream: Option<&ordo_secevent::Recorder>,
    now: u64,
) -> Result<Prepared, ProvisionRefusal> {
    let Preflight {
        _lock,
        sid,
        want,
        db,
        cfg,
        artifact,
        ..
    } = pre;
    let schema = &artifact.fixture.schema;

    keys.create_space(&sid).map_err(ProvisionRefusal::Custody)?;

    // Whether this call made the database or found one already standing.
    // That is the difference between a fresh attach and the completion of a
    // provision that was interrupted between `create_database` and the
    // attachment row, and T8 requires the record to say which.
    //
    // Asked again rather than read off `Preflight::standing`, because the
    // catalogue answer phase 0 took is a **plan**, and what the record must
    // carry is what this statement did.
    let created = ordo_exec::create_database(&cfg, &db)
        .await
        .map_err(ProvisionRefusal::Target)?;
    let pool = Pool::connect(cfg).await.map_err(ProvisionRefusal::Target)?;
    spacedb::apply_schema_at_version(
        &pool,
        &artifact.fixture.ddl,
        &artifact.version,
        SYSTEM_ACTOR,
        &created_changes(schema),
        // Milliseconds, because `$migrations`.`at` is milliseconds
        // wherever `schema_apply` writes it, and R-S2 puts one `$migrations`
        // in every database. A column holding two units would be read wrong
        // by whichever operator met the other one first. `now` is seconds
        // here, the unit the attachment row and the security event take.
        now.saturating_mul(1_000),
    )
    .await
    .map_err(ProvisionRefusal::Target)?;

    // **The fleet view learns about the new database** (R-S2), from the
    // database's own `$migrations` rather than from `artifact.version`. A
    // target provisioned and never applied to again would otherwise be
    // missing from the operator's fleet listing until the next deploy,
    // which is precisely the window in which somebody asks where a
    // just-created tenant went.
    //
    // Best effort, and it does not fail the attach: `$fleet` is a cache and
    // never the authority, so a missing row is a worse diagnostic rather
    // than a wrong placement. The attachment row itself is already written
    // by the call below and that is what routing reads.
    let observed = match spacedb::recorded_version(&pool).await {
        Ok(spacedb::TargetVersion::At(v)) => Some(v),
        Ok(spacedb::TargetVersion::Unknown) => Some(ordo_exec::fleet::UNKNOWN.to_string()),
        Err(_) => None,
    };

    let outcome = spacedb::attach(control, &sid, &want)
        .await
        .map_err(ProvisionRefusal::Attach)?;

    if let Some(version) = &observed {
        // The same label the resolver will give this database, from the
        // same function, so the attach and the next apply walk write one
        // row rather than two spellings of one target.
        let label = ordo_exec::route::target_label(&want.host, want.port, &want.db);
        if let Err(e) = ordo_exec::fleet::ensure_table(control).await {
            eprintln!("provision: the fleet view's table could not be ensured: {e}");
        } else if let Err(e) = ordo_exec::fleet::confirm(control, &label, version, now as i64).await
        {
            eprintln!("provision: the fleet view did not record `{label}`: {e}");
        }
    }

    // Occupancy read **after** the attach, so the number recorded is the one
    // that was true once this tenant was in, rather than the one that was
    // true just before. R-X1 makes occupancy part of what the tenant was
    // told, and a record saying `occupancy=0` for a database with a tenant
    // in it would be the record disagreeing with the disclosure.
    let occupancy = spacedb::occupants(control, &want.host, want.port, &want.db)
        .await
        .map_err(ProvisionRefusal::Target)?
        .len();
    Ok(match outcome {
        DbAttachOutcome::Attached => {
            if let Some(stream) = stream {
                stream.observe(
                    ordo_secevent::Observation {
                        space: Some(ordo_commandlog::SpaceId(space.to_string())),
                        subject: ordo_secevent::Subject::platform("provisioning"),
                        op: "provision".to_string(),
                        kind: ordo_secevent::EventKind::CustodyAction {
                            // **The detail goes in the action string, not in
                            // a field.** `CustodyAction` carries one string
                            // and is hash-chained under a tag
                            // (`ordo_secevent::chain`), so widening the
                            // variant would change the encoding of every
                            // event already written. What T8 asks the record
                            // to carry therefore goes here, in a form that
                            // splits: the target's identity is
                            // `DbAttachment::identity`'s six fields
                            // unchanged, and `credential` in it is a
                            // **label** rather than a secret, which is the
                            // property that module doc states and this
                            // relies on. The space's minted id is not
                            // repeated because the observation already
                            // carries it.
                            action: format!(
                                "space_attached_database {} occupancy={occupancy} outcome={}",
                                want.identity(),
                                if created {
                                    "attached"
                                } else {
                                    // The database was already there, so
                                    // this call finished a provision that
                                    // stopped after the database was made
                                    // and before the row was written.
                                    "completed-interrupted"
                                },
                            ),
                        },
                        at: ordo_commandlog::Timestamp(now),
                        correlation: ordo_secevent::correlation::adopt(&format!("prov_{space}")),
                    },
                    // No relationship: the space has no members yet, so the
                    // record is the deployment's, about this space.
                    ordo_secevent::Relationship::None,
                );
            }
            Prepared::Attached {
                space: space.to_string(),
                db,
            }
        }
        DbAttachOutcome::Unchanged => Prepared::AlreadyAttached {
            space: space.to_string(),
            db,
        },
    })
}

/// The `changes` line the `$migrations` row carries for a fresh target.
///
/// Word for word what `schema_apply::create_schema` writes for the same
/// event on the control database, because it *is* the same event: the whole
/// schema created at once on an empty database. An operator reading
/// `$migrations` should not have to know which of the two paths created a
/// database in order to read its first row, and a difference in wording
/// would read as a difference in what happened.
fn created_changes(schema: &ordo_schema::Schema) -> String {
    schema
        .entities
        .iter()
        .map(|e| format!("A1 — new entity `{}`", e.name))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Provision a space onto a database of its own, founding member included.
///
/// [`prepare_target`], then the founding member row **in the target**:
/// owner-ruled 2026-09-14 (`membership-home`), a space's members live in its
/// own database. The member comes last, which is the order the whole of this
/// module argues for and the one S3 needs: nobody can be authorized in the
/// space before its attachment is final.
///
/// Safe to retry at every point, for the reasons each half gives. The door
/// sees the new member when its engine is next built with
/// [`crate::directory::routed_engine`].
///
/// `serving` is [`prepare_target`]'s, for [`prepare_target`]'s reason, and
/// this function adds nothing to that argument: the founding member row it
/// writes afterwards names columns the `members` table has in every version
/// this platform has ever served, so it does not read the artifact itself.
#[allow(clippy::too_many_arguments)]
pub async fn provision_space_on(
    control: &Pool,
    keys: &dyn KeyProvider,
    serving: &crate::deploy::Deployment,
    space: &str,
    owner: &FoundingOwner<'_>,
    target: &Target<'_>,
    stream: Option<&ordo_secevent::Recorder>,
    now: u64,
) -> Result<Provisioned, ProvisionRefusal> {
    if !ordo_api::well_formed_space(owner.member_id) {
        return Err(ProvisionRefusal::Malformed(format!(
            "`{}` is not a well-formed members row id",
            owner.member_id
        )));
    }
    let pre = preflight(control, keys, serving, space, target, now).await?;
    provision_prepared(control, keys, space, owner, target, pre, stream, now).await
}

/// [`provision_space_on`] with phase 0 already run.
///
/// The shape `std.space.create` needs, and the reason this split exists at
/// all: the registry row goes in **between** the two halves.
/// `bootstrap.md` §4.4 wants the row before any external resource, and
/// §1.2 wants every decidable failure refused before the row. Those two
/// only fit together if preflight and provisioning are separately callable,
/// with the reserve in the gap.
#[allow(clippy::too_many_arguments)]
pub async fn provision_prepared(
    control: &Pool,
    keys: &dyn KeyProvider,
    space: &str,
    owner: &FoundingOwner<'_>,
    target: &Target<'_>,
    pre: Preflight,
    stream: Option<&ordo_secevent::Recorder>,
    now: u64,
) -> Result<Provisioned, ProvisionRefusal> {
    commit_target(control, keys, space, pre, stream, now).await?;
    let cfg = target_config(target.attachment, target.credentials, target.template)
        .map_err(ProvisionRefusal::Target)?;
    let pool = Pool::connect(cfg).await.map_err(ProvisionRefusal::Target)?;
    let written = write_founding_row(&pool, space, owner)
        .await
        .map_err(ProvisionRefusal::Store)?;
    Ok(if written {
        Provisioned::Created {
            space: space.to_string(),
        }
    } else {
        Provisioned::AlreadyExists {
            space: space.to_string(),
        }
    })
}

#[cfg(test)]
mod host_identity {
    use super::*;

    /// **The loopback spellings name one database** (18d R-X2).
    ///
    /// A unit test rather than only a door test, because `same_database` is
    /// reached through `prepare_target` only when the conformance cluster
    /// is on TCP. It is on a **Unix socket** by default
    /// (`ORDO_PG_HOST=/tmp/ordo-pg`), so the door-level test for this skips
    /// on a developer machine and runs only on CI. A check that is exercised
    /// on one of the two machines is a check nobody is watching.
    #[test]
    fn the_loopback_spellings_are_one_host() {
        let control = PoolConfig {
            host: "127.0.0.1".into(),
            port: 5432,
            db: "ordo".into(),
            ..PoolConfig::from_env()
        };
        for spelling in ["127.0.0.1", "localhost", "::1", "[::1]", "  LocalHost "] {
            assert!(
                same_database(spelling, 5432, "ordo", &control),
                "`{spelling}` is the control database and was not recognised"
            );
        }
    }

    /// And it does not collapse databases that genuinely differ, so the test
    /// above is not passing because everything matches everything.
    #[test]
    fn a_different_host_port_or_database_is_a_different_database() {
        let control = PoolConfig {
            host: "127.0.0.1".into(),
            port: 5432,
            db: "ordo".into(),
            ..PoolConfig::from_env()
        };
        assert!(!same_database("db.internal", 5432, "ordo", &control));
        assert!(!same_database("127.0.0.1", 5433, "ordo", &control));
        assert!(!same_database("127.0.0.1", 5432, "ordo_tenant", &control));
    }

    /// **One allowlist for both tiers, and adding the new name does not
    /// remove the old one** (D-S6: the bucket endpoint allowlist *"should be
    /// tied in with the db allowlist setup"*).
    ///
    /// The union rather than a precedence, because with a precedence an
    /// operator who set `ORDO_TENANT_ATTACH_HOSTS` would silently turn
    /// `ORDO_PG_ATTACH_HOSTS` off, and the hosts that vanished would be the
    /// ones their database tenants were already attached to.
    #[test]
    fn both_attach_host_variables_are_one_list() {
        let both = HostAllowlist::of_both("s3.acme.com", "db-a.internal");
        assert!(both.allows("s3.acme.com"));
        assert!(both.allows("db-a.internal"));
        // Either one alone is the whole list, so a deployment that set only
        // the older name keeps working and one that set only the newer name
        // does not need the older.
        assert!(HostAllowlist::of_both("", "db-a.internal").allows("db-a.internal"));
        assert!(HostAllowlist::of_both("s3.acme.com", "").allows("s3.acme.com"));
        // And the union is still a list: a host on neither is refused, so
        // the assertions above are not passing because everything is
        // allowed.
        assert!(!both.allows("s3.somewhere.else"));
        assert!(!HostAllowlist::of_both("", "").allows("s3.acme.com"));
        // One host named twice is one entry, so a duplicate in an
        // operator's configuration is not a duplicate in the list.
        assert_eq!(HostAllowlist::of_both("a.b, c.d", "c.d").0.len(), 2);
    }

    /// A socket directory is a path, and paths are case-sensitive.
    #[test]
    fn a_socket_directory_keeps_its_case_and_a_hostname_does_not() {
        assert!(HostAllowlist::of("DB.Internal").allows("db.internal"));
        assert!(HostAllowlist::of("/tmp/ordo-pg").allows("/tmp/ordo-pg"));
        assert!(!HostAllowlist::of("/tmp/Ordo-PG").allows("/tmp/ordo-pg"));
    }
}
