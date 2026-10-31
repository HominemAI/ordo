//! `std.space.create`: **a tenant comes into being through a call at the
//! door** (checklist 17aa piece 3).
//!
//! # What this replaces
//!
//! Until now the only way a space came into existence was somebody with
//! the database password running [`crate::provision::provision_space`] —
//! which no production path called at all, except the boot creating the
//! platform's own home. A tenant therefore had to be arranged *before* a
//! deployment could serve it, which is the last reason the CLI needed the
//! database on day zero, and it put the act that creates a customer
//! outside every record the deployment keeps.
//!
//! # The three checks behind it, which are `std.deployment.apply`'s
//!
//! Creating a tenant is an act **on the deployment**, not inside a space,
//! and the gate is the one `apply_op` already argues for in full:
//!
//! 1. **Owner**, through [`crate::platform_ops::owner_only`].
//! 2. **Step-up**, which every platform op requires, so no capability
//!    token can reach it.
//! 3. **The bound space is the deployment's own** ([`TenantOps::home`]),
//!    because the first two are deployment-wide policy installed in every
//!    space, so a *tenant's* owner passes both. Owning a space you were
//!    given is not owning the deployment that serves it, and minting the
//!    next tenant is the deployment's act.
//!
//! # What the caller supplies and what the platform mints
//!
//! This is the whole of the id ruling in one table, and it is
//! `bootstrap.md` §2's split with the slug removed:
//!
//! | | |
//! |---|---|
//! | the space id | **minted** ([`crate::tenants::mint_space_id`]) |
//! | the owner's role | **fixed** to owner; not a parameter |
//! | `name` | the caller's, and an attribute nothing keys on |
//! | `owner` | the caller's: the principal who will hold the space |
//! | the creator | **derived** from the door's proved principal |
//!
//! There is no `slug`, and its absence is the ruling: *"names are
//! irrelevant, everything should be by id."* A deployment-unique handle
//! would be a second identity with a uniqueness rule of its own, and the
//! first support call about two customers wanting `acme` would be a
//! deployment arbitrating names.
//!
//! # Why the door can serve the tenant immediately
//!
//! The access engine is built from the `members` table once, at boot, and
//! `crate::directory`'s own doc says refreshing a live one is not
//! attempted. A minted tenant would therefore be unreachable until a
//! restart, which would make this op a way to *schedule* a tenant rather
//! than create one. So the mint ends by announcing itself on the deploy
//! bus, and every replica rebuilds its door exactly as it does after an
//! `ordo apply` — the same mechanism, not a second one, for the reason
//! [`crate::runtime::refresh_access`] gives: the engine is rebuilt whole
//! and installed under a lock, because an engine mutated in place under an
//! in-flight request is the half-swap `crate::deploy` exists to describe.

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_crypto::KeyProvider;
use ordo_exec::Pool;
use serde_json::json;

use crate::deploy::Deployment;
use crate::provision::{FoundingOwner, HostAllowlist, ProvisionRefusal, Provisioned};
use crate::tenants;
use ordo_exec::spacedb::{DbAttachment, DbCustody};
use ordo_exec::PoolConfig;

/// The op the door dispatches to [`TenantOps::create`].
pub const CREATE: &str = "std.space.create";

/// The longest human name this op will record.
///
/// Bounded at the door in the door's own vocabulary, like every other
/// platform op's text. Generous for a company name and far short of a
/// place to hide a payload: a `name` is read back to a human in a list.
pub const MAX_NAME_LEN: u32 = 256;

/// What `std.space.create` needs, and nothing else.
///
/// The `KeyProvider` is the ingredient `ApplyOps` does not carry and this
/// op cannot do without: custody is minted before any row, so a struct
/// with a pool and no provider could write a founding member into a space
/// whose sealed fields can never be written.
pub struct TenantOps {
    pub pool: Pool,
    pub keys: Arc<dyn KeyProvider>,
    /// `RuntimeConfig::space` — the deployment's own space, and the only
    /// one this op answers in. See the module doc, check 3.
    pub home: String,
    /// What a tenant needs to be given a database of its own (18d T8).
    /// `None` is a deployment wired without it, and then the routing
    /// parameters are refused rather than ignored: silently putting a
    /// tenant on the shared database after the caller asked for its own
    /// is the failure this whole increment exists to prevent.
    routing: Option<Routing>,
    /// One own-tenant provision at a time in this process (P1). The apply
    /// lock already serialises provisioning deployment-wide, and every waiter
    /// for it holds a pooled connection; a burst of sign-ups would spend the
    /// control pool on waiters. Waiting here holds none. The latch is still
    /// the registry index, which is what holds across processes.
    own_gate: tokio::sync::Mutex<()>,
    /// Rebuilds the door on this replica (25e). Set once, by the runtime,
    /// after the door exists; `None` in a test that wires no door, and then
    /// a new tenant is served when the announcement reaches this replica.
    refresh: std::sync::OnceLock<crate::member_ops::Refresh>,
    /// The own tenants a task in this process is making now, so a resume
    /// or a second sign-in does not start a second task for one.
    in_flight: std::sync::Mutex<std::collections::HashSet<String>>,
    /// The intervals and caps of setting up an own tenant. Set once by the
    /// runtime from the environment; the defaults otherwise.
    setup: std::sync::OnceLock<crate::tenant_setup::SetupSettings>,
    /// This server's name on a setup lease. Random per process, so a
    /// restarted server waits out its old lease like any other.
    holder: String,
    /// The own tenants being set up, as the door reads them.
    not_ready: std::sync::OnceLock<Arc<crate::tenant_setup::NotReady>>,
}

/// What [`TenantOps::provision_own`] found or made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnTenant {
    /// This call provisioned it and it is `active`. `reachable` is false
    /// when announcing it to the other replicas failed: this node serves it
    /// and the rest do at their next restart.
    Created { space: String, reachable: bool },
    /// The principal already had one, `active`. Nothing was made.
    Existing { space: String },
    /// The principal already has one being built, by another request or
    /// another replica. Nothing was made.
    InProgress { space: String },
}

impl OwnTenant {
    /// The tenant's id, whichever way it came about.
    pub fn space(&self) -> &str {
        match self {
            OwnTenant::Created { space, .. }
            | OwnTenant::Existing { space }
            | OwnTenant::InProgress { space } => space,
        }
    }
}

/// The pieces `provision_space_on` needs that `provision_space` does not.
///
/// **The deployment is held whole, and the version is read per call.** It
/// is `Deployment` that swaps its artifact, not the `Arc` that is swapped,
/// so `current()` on a handle taken at boot answers with what is serving
/// now. That matters here rather than being a detail: a tenant created
/// after an `apply` must receive the schema the deployment is serving at
/// that moment, and a version captured at boot would give it the previous
/// one — which is R-S4's quarantine, reached by construction on the first
/// tenant created after a deploy.
/// Which cluster a tenant's database is created in when nothing else is
/// said: **level 3 of four** (18d, ruled 2026-09-25).
///
/// *"there exist a default cluster which databases are created in. 1 tenant
/// one database ... the settings are simply to decide what DB cluster to
/// store the database in. Either whatever the deployment provides as
/// default, or what the deployment provides for that tenant."*
///
/// The four levels are the console's own database, the deployment's own,
/// this one, and a per-tenant cluster. **Each defaults to the one above**,
/// because *"someone deploying could use the same cluster for all 3 if they
/// chose"* — so a deployment that configures nothing runs its control plane
/// and every tenant database on one server, and that has to stay a no-op to
/// set up.
///
/// **What is never shared is the database.** D-D's *"database/schema not
/// cluster"* and this ruling are the same statement from two directions,
/// and `provision::same_database` already enforces it by comparing host,
/// port *and* database: a tenant database beside the control database on
/// one server passes, the control database itself is refused.
#[derive(Debug, Clone)]
pub struct TenantCluster {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// A **label**, never a secret. [`ordo_exec::route::OWN_CREDENTIAL`]
    /// when the deployment's own credential dials it, which is the default
    /// and is what makes the zero-configuration case work.
    pub credential: String,
}

impl TenantCluster {
    /// `ORDO_PG_TENANT_HOST` / `_PORT` / `_USER` / `_CREDENTIAL`, each
    /// falling back to the deployment's own connection.
    ///
    /// **Per field, not all-or-nothing.** An operator pointing tenants at
    /// another server usually changes the host and keeps the port, and a
    /// setting that demanded all four would be four chances to mistype the
    /// three that were not the point. The fallback is the deployment's own
    /// value, which is the level above.
    pub fn from_env(control: &PoolConfig) -> TenantCluster {
        let var = |n: &str| std::env::var(n).ok().filter(|v| !v.trim().is_empty());
        TenantCluster {
            host: var("ORDO_PG_TENANT_HOST").unwrap_or_else(|| control.host.clone()),
            port: var("ORDO_PG_TENANT_PORT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(control.port),
            user: var("ORDO_PG_TENANT_USER").unwrap_or_else(|| control.user.clone()),
            credential: var("ORDO_PG_TENANT_CREDENTIAL")
                .unwrap_or_else(|| ordo_exec::route::OWN_CREDENTIAL.to_string()),
        }
    }
}

/// The database name a tenant's space id becomes.
///
/// **Minted, never the caller's** (ruled 2026-09-25). T8 took the name as
/// `$db` and refused the call without it; under the ruling the database is
/// neither optional nor the caller's to name, so it is derived from the id
/// the platform already minted.
///
/// The space id is its own name. `mint_space_id` produces `spc_` plus 26
/// Crockford base32 characters, which is 30 bytes of `[a-z0-9_]` starting
/// with a letter — a legal Postgres identifier with room to spare under the
/// 63-byte limit, and already globally unique, so two tenants cannot
/// collide on a database name any more than they can on an id. Prefixing it
/// with something like `ordo_` would add bytes and a second thing to agree
/// on for no gain.
pub fn database_for(space: &str) -> String {
    space.to_string()
}

pub struct Routing {
    pub serving: Arc<Deployment>,
    pub credentials: Arc<dyn ordo_exec::route::DbCredentials>,
    /// The shape every target pool is built from: sizes and timeouts, not
    /// an address. The address comes from the attachment.
    pub template: PoolConfig,
    /// R-X3, read from the environment once at boot. See
    /// [`crate::provision::HostAllowlist`].
    pub hosts: HostAllowlist,
    /// Level 3: where a tenant's database goes when the call names no
    /// cluster of its own. See [`TenantCluster`].
    pub default_cluster: TenantCluster,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

impl TenantOps {
    pub fn new(pool: Pool, keys: Arc<dyn KeyProvider>, home: &str) -> Arc<TenantOps> {
        Arc::new(TenantOps {
            pool,
            keys,
            home: home.to_string(),
            routing: None,
            own_gate: tokio::sync::Mutex::new(()),
            refresh: std::sync::OnceLock::new(),
            in_flight: std::sync::Mutex::new(std::collections::HashSet::new()),
            setup: std::sync::OnceLock::new(),
            holder: format!("setup-{}", tenants::mint_space_id()),
            not_ready: std::sync::OnceLock::new(),
        })
    }

    /// The same op, able to put a tenant on a database of its own.
    pub fn with_routing(
        pool: Pool,
        keys: Arc<dyn KeyProvider>,
        home: &str,
        routing: Routing,
    ) -> Arc<TenantOps> {
        Arc::new(TenantOps {
            pool,
            keys,
            home: home.to_string(),
            routing: Some(routing),
            own_gate: tokio::sync::Mutex::new(()),
            refresh: std::sync::OnceLock::new(),
            in_flight: std::sync::Mutex::new(std::collections::HashSet::new()),
            setup: std::sync::OnceLock::new(),
            holder: format!("setup-{}", tenants::mint_space_id()),
            not_ready: std::sync::OnceLock::new(),
        })
    }

    /// Give this server the setup settings. Once; a second call is ignored.
    pub fn set_setup(&self, settings: crate::tenant_setup::SetupSettings) {
        let _ = self.setup.set(settings);
    }

    /// The setup settings in force.
    pub fn setup(&self) -> crate::tenant_setup::SetupSettings {
        self.setup.get().copied().unwrap_or_default()
    }

    /// The own tenants being set up, as this server's door reads them.
    pub fn not_ready(&self) -> Arc<crate::tenant_setup::NotReady> {
        self.not_ready
            .get_or_init(|| {
                Arc::new(crate::tenant_setup::NotReady::new(
                    self.setup().not_ready_retry_secs,
                ))
            })
            .clone()
    }

    /// This server's name on a setup lease.
    pub fn holder(&self) -> &str {
        &self.holder
    }

    /// Rebuild this replica's door before a new tenant is answered (25e), so
    /// the caller's first call to it is served. A second call is ignored.
    pub fn refresh_with(&self, hook: crate::member_ops::Refresh) {
        let _ = self.refresh.set(hook);
    }

    /// Make this replica serve a tenant just made, and tell every other
    /// replica. A failed rebuild is left to the announcement and the
    /// refresh tick: the tenant exists either way.
    async fn serve_new(&self, id: &str) -> Result<(), String> {
        if let Some(refresh) = self.refresh.get() {
            if let Err(e) = refresh(id).await {
                eprintln!("tenant {id}: this replica's door was not rebuilt ({e}); it follows the announcement");
            }
        }
        crate::deploy::announce_members(&self.pool, id).await
    }

    /// Where this tenant's database goes, which is **always somewhere**
    /// (ruled 2026-09-25).
    ///
    /// *"1 tenant one database ... the settings are simply to decide what
    /// DB cluster to store the database in."* So this never returns
    /// "nowhere": the name is minted from the space id, and the only
    /// question is the cluster. No cluster named means the deployment's
    /// default tenant cluster (level 3), which itself defaults to the
    /// deployment's own.
    ///
    /// **What T8 had and this does not.** `$db` named the database and the
    /// call was refused without it, with `$dbHost` and the rest required
    /// beside it. The database is not the caller's to name, so `$db` is
    /// gone. What remains is a per-tenant **cluster** (level 4), which is
    /// still a whole address because a cluster this deployment has not been
    /// configured with is one it has to be told how to reach.
    fn target_for(&self, space: &str, bound: &BoundParams) -> Result<DbAttachment, ExecError> {
        let routing = self.routing.as_ref().ok_or_else(|| {
            ExecError::new(
                ErrorCode::SpaceUnavailable,
                "this server was assembled without a deployment to give a tenant's \
                 database its schema, and every tenant is a database of its own. Nothing \
                 was created",
            )
        })?;
        let text = |n: &str| {
            bound
                .0
                .get(n)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        };

        // Level 4, or level 3. `$dbHost` is what decides, because a cluster
        // this deployment was not configured with is one the call has to
        // describe in full.
        let (host, port, user, credential, custody) = match text("dbHost") {
            None => {
                // Nothing may be half-stated into the default: a caller who
                // named a user and forgot the host asked for another
                // cluster and would otherwise silently get this one.
                let stray: Vec<&str> = ["dbUser", "dbCredential"]
                    .into_iter()
                    .filter(|n| text(n).is_some())
                    .chain(
                        ["dbPort", "dbOwnPerimeter"]
                            .into_iter()
                            .filter(|n| bound.0.get(*n).is_some_and(|v| !v.is_null())),
                    )
                    .collect();
                if !stray.is_empty() {
                    return Err(ExecError::new(
                        ErrorCode::TenantRefused,
                        format!(
                            "`{}` was given without `$dbHost`. A cluster is a whole \
                             address, and completing a half-stated one from this \
                             deployment's own connection would put the tenant somewhere \
                             the caller did not ask for. Nothing was created",
                            stray.join("`, `")
                        ),
                    ));
                }
                let c = &routing.default_cluster;
                (
                    c.host.clone(),
                    c.port,
                    c.user.clone(),
                    c.credential.clone(),
                    // The deployment's own cluster is the platform's to
                    // run. A tenant's own perimeter is a claim about who
                    // runs the server, and nobody made it here.
                    DbCustody::PlatformManaged,
                )
            }
            Some(host) => {
                let need = |n: &str| {
                    text(n).ok_or_else(|| {
                        ExecError::new(
                            ErrorCode::TenantRefused,
                            format!(
                                "`${n}` is required with `$dbHost`: a cluster this \
                                 deployment was not configured with has to be described in \
                                 full, and a half-stated address would be completed from \
                                 this deployment's own connection"
                            ),
                        )
                    })
                };
                let user = need("dbUser")?.to_string();
                let credential = need("dbCredential")?.to_string();
                // **Answerable before anything is built.** A label this
                // node cannot answer would otherwise surface at the dial as
                // `space_unavailable`, whose message names no topology and
                // so cannot say what to fix. The label is the caller's own
                // word, so naming it back discloses nothing, and the
                // setting that installs it is what the deployment owner
                // needs to read.
                if let Err(why) = routing.credentials.password(&credential) {
                    return Err(ExecError::new(
                        ErrorCode::TenantRefused,
                        format!(
                            "`$dbCredential` names `{credential}`, which this deployment \
                             cannot answer ({why}). A cluster other than the deployment's \
                             own is dialled with a credential installed through \
                             ORDO_TENANT_CREDENTIALS. Nothing was created"
                        ),
                    ));
                }
                // **A bool, and required with `$dbHost`.** `bucket_ops`'
                // `platformSealed` makes this choice for the reason
                // repeated here: custody as text would put an enum check in
                // the executor, and an executor may not say
                // `invalid_param`, so a typo would reach the caller as a
                // 500. Required rather than defaulted because the two
                // positions answer differently to "what does deleting this
                // space do", and choosing for the owner is choosing a
                // disclosure on their behalf.
                let own_perimeter = bound
                    .0
                    .get("dbOwnPerimeter")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| {
                        ExecError::new(
                            ErrorCode::TenantRefused,
                            "`$dbOwnPerimeter` is required with `$dbHost`: it is the \
                             difference between a cluster the platform runs and one under \
                             the tenant's own perimeter, and those disclose different \
                             things about what deleting the space does",
                        )
                    })?;
                let port = match bound.0.get("dbPort").and_then(|v| v.as_i64()) {
                    Some(p) if (1..=65_535).contains(&p) => p as u16,
                    Some(p) => {
                        return Err(ExecError::new(
                            ErrorCode::TenantRefused,
                            format!("`{p}` is not a port"),
                        ))
                    }
                    None => routing.default_cluster.port,
                };
                (
                    host.to_string(),
                    port,
                    user,
                    credential,
                    if own_perimeter {
                        DbCustody::OwnPerimeter
                    } else {
                        DbCustody::PlatformManaged
                    },
                )
            }
        };

        Ok(DbAttachment {
            host,
            port,
            user,
            db: database_for(space),
            // A **label**, never a secret: see `ordo_exec::spacedb`'s module
            // doc. The command log is plaintext by design.
            credential,
            custody,
            // Both overwritten by `prepare_target`, which stamps the system
            // as the actor and the call's own clock as the time.
            attached_at: 0,
            attached_by: String::new(),
        })
    }

    /// `std.space.create`.
    ///
    /// `caller` is the principal the door proved. It is recorded as the
    /// creator, and it is **not** made the owner: a deployment owner who
    /// mints a tenant for a customer is the ordinary case, and conflating
    /// the two would make every tenant's founding owner the operator.
    pub async fn create(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        // Check 3. `not_found` rather than `access_denied`, for the two
        // reasons `ApplyOps::apply` states: an executor may not say the
        // door's word, and this op genuinely does not exist in a tenant's
        // space — no membership change there will ever make it answer.
        if space != self.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "creating a tenant is the deployment's act rather than a space's: this op \
                 answers only when it is bound to the deployment's own space, and owning a \
                 space you were given is not owning the deployment that serves it",
            ));
        }

        let name = bound
            .0
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecError::internal("`$name` did not reach the platform op"))?
            .trim();
        if name.is_empty() {
            return Err(ExecError::new(
                ErrorCode::TenantRefused,
                "`name` is the human reference for a tenant whose id is unreadable by design, \
                 so it may not be blank",
            ));
        }
        let owner = bound
            .0
            .get("owner")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecError::internal("`$owner` did not reach the platform op"))?
            .trim();
        if owner.is_empty() {
            return Err(ExecError::new(
                ErrorCode::TenantRefused,
                "`owner` names the principal who will hold this tenant, and a space with no \
                 owner is the one state nothing can repair from inside",
            ));
        }

        let id = tenants::mint_space_id();
        // **The member id is the principal, and it is not minted here.**
        //
        // Until enrolment E1 this was forced: `crate::engine_from_members`
        // keyed memberships by `members.id` and never read the `principal`
        // column, so a minted id would have left the owner named here with
        // no membership. E1 keys the door by the `principal` column, so the
        // row is reached through `principal` whatever `id` holds. The id is
        // still the principal's own string, which keeps the retry property
        // `provision_space` wants from a caller-supplied member id (a retry
        // addresses the same row rather than adding a second owner beside
        // the first). Giving the founding row a minted `mem_...` id is a
        // later, separate change.
        let founding = FoundingOwner {
            member_id: owner,
            principal: owner,
            // `bootstrap.md` §2 item 7 says a display name is derived by
            // the identity service and never caller-supplied, *"a
            // caller-supplied name is a caller writing a platform field"*.
            // There is no directory lookup to derive it from yet, so the
            // principal stands in: it is true, it is the deployment's own
            // value rather than the caller's, and it is replaced by the
            // lookup when identity gains one.
            display_name: owner,
        };

        // **Every tenant gets a database** (ruled 2026-09-25). There is
        // no branch here any more: the create that put a tenant in the
        // control database alongside every other tenant is gone, and with
        // it `provision::provision_space`, which now has no caller outside
        // the boot that creates the deployment's own home space.
        //
        // **The system chooses at provisioning** — ruled 2026-09-13
        // (`db-attach-gate`): *"during provisioning the system decides, and
        // for changes that would require an agreement - you cannot have an
        // agreement for a space that does not exist and you cannot have a
        // space without a place for storage."* So the cluster is a
        // parameter of creation rather than a separate owner-consented
        // attach op; there is no owner yet to consent, and the attachment
        // records `attached_by = system:provisioning` to say so. Changing
        // it afterwards is a two-party act and is 18e.
        // **The registry row comes first** (`bootstrap.md` §4.4: *"Nothing
        // external is provisioned before the registry row exists, and
        // nothing external is used before its handle is recorded on that
        // row"*). Until 11-15 this was written **last**, after the custody
        // keys and the database, so a crash in between left both with
        // nothing naming them and no way to find them. Every orphan is now
        // reachable from one table.
        //
        // `provisioning`, so it is not servable. In practice it is also
        // unreachable: the door builds memberships from `members`, and the
        // founding row does not exist yet.
        // **Everything decidable without touching a resource is decided
        // first**, so a bad parameter or an unconfigured deployment leaves
        // no registry row behind. The row is evidence that something was
        // built; a call refused before anything was built has nothing to
        // be evidence of.
        let attachment = self.target_for(&id, bound)?;
        if !ordo_api::well_formed_space(founding.member_id) {
            return Err(ExecError::new(
                ErrorCode::TenantRefused,
                "`owner` becomes this tenant's founding member id, so it must be one the \
                 door can address: letters, digits, `_` and `-`. Nothing was created",
            ));
        }

        let routing = self
            .routing
            .as_ref()
            .expect("target_for refuses without it");
        let now = crate::runtime::now_secs();
        let target = crate::provision::Target {
            attachment: &attachment,
            credentials: routing.credentials.as_ref(),
            template: &routing.template,
            hosts: &routing.hosts,
        };

        // **Phase 0, and it is the whole reason the two halves of
        // provisioning are separately callable** (`bootstrap.md` §1.2:
        // *"Preflight is the whole product … every decidable failure … is
        // rejected in phase 0, before the registry row exists"*).
        //
        // Until 11-15 every one of these refusals — a host this deployment
        // may not dial, the control database, a database another tenant
        // already sits on, a cluster that does not answer — was discovered
        // **after** `reserve`, so an operator who mistyped a host was told
        // so *and* left a `failed` tenant behind. That row then sat in
        // `tenants::troubled`, which §4.1 makes the deployment's one
        // detection query for provisions that need a human. A typo is not a
        // stuck provision, and a detection query that reports typos is one
        // an operator learns to scroll past.
        //
        // The lock this returns is held across the registry write below, so
        // R-S5's window does not reopen in the gap. See
        // `provision::Preflight`.
        let pre = match crate::provision::preflight(
            &self.pool,
            self.keys.as_ref(),
            &routing.serving,
            &id,
            &target,
            now,
        )
        .await
        {
            Ok(pre) => pre,
            Err(refusal) => return Err(refusal_to_error(refusal)),
        };

        // **`dryRun` (`bootstrap.md` §2 item 45).** Phase 0 ran and nothing
        // else will: *"It writes nothing and reserves nothing, so two dry
        // runs of the same slug both succeed."* Dropping `pre` here is the
        // whole of abandoning the plan — it releases the routing lock and
        // there is nothing else to undo, which is the property the split
        // was built to have.
        //
        // The plan is what a caller cannot work out from their own
        // parameters: which cluster the deployment resolved them to, the
        // minted database name, the schema version it would receive, and
        // whether that database is already standing. The **space id is not
        // in it**, because no id was reserved and returning one would read
        // as a handle to something.
        if bound.0.get("dryRun").and_then(|v| v.as_bool()) == Some(true) {
            let plan = json!({
                "target": pre.label(),
                "database": pre.database(),
                "version": pre.version(),
                "databaseStanding": pre.standing(),
                "occupancy": pre.occupancy(),
            });
            return Ok(ExecResult {
                position: None,
                rows: vec![json!({
                    // Null rather than a minted id: nothing was reserved.
                    "space": serde_json::Value::Null,
                    "name": name,
                    "owner": owner,
                    // Null rather than `false`: `false` would say a tenant
                    // exists and cannot be reached, and no tenant exists.
                    "reachable": serde_json::Value::Null,
                    "plan": plan,
                })],
                next_cursor: None,
            });
        }

        // **Now the registry row, before any resource** (`bootstrap.md`
        // §4.4: *"Nothing external is provisioned before the registry row
        // exists, and nothing external is used before its handle is
        // recorded on that row"*). Until 11-15 this was written **last**,
        // after the custody keys and the database, so a crash in between
        // left both with nothing naming them. Every orphan is now
        // reachable from one table.
        //
        // `provisioning`, so it is not servable. In practice it is also
        // unreachable: the door builds memberships from `members`, and the
        // founding row does not exist yet.
        //
        // The target goes on in the same breath, because it is the field
        // that makes a leaked database findable: if the process dies
        // inside `create_database`, the row already names where to look.
        let label = pre.label();
        // G12: the deployment owner's act leaves a record, whatever it came
        // to, once the registry row names the tenant. In the deployment's
        // own stream and mirrored to the tenant's, as a pause is.
        let record = |result: &str| {
            if let Some(stream) = routing.stream.as_deref() {
                crate::enrolment::record(
                    stream,
                    &crate::enrolment::Act {
                        act: "tenant-created",
                        detail: format!("tenant={id} owner={owner} result={result}"),
                        by: Some(caller),
                        tenant: Some(&id),
                        deployment: true,
                        at: crate::runtime::now_secs(),
                    },
                );
            }
        };
        let reserved = tenants::reserve(&self.pool, &id, name, caller, now, None).await;
        let targeted = match reserved {
            Ok(_) => {
                tenants::at_phase(&self.pool, &id, tenants::Phase::Targeted, &label, now).await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = targeted {
            return Err(ExecError::new(
                ErrorCode::SpaceUnavailable,
                format!("the tenant registry could not be written ({e}); nothing was created"),
            ));
        }
        // Phases 2 and 3, against the plan phase 0 made. `pre` moves in,
        // so there is no path from here that could run the checks a second
        // time and provision against a different answer.
        let provisioned = crate::provision::provision_prepared(
            &self.pool,
            self.keys.as_ref(),
            &id,
            &founding,
            &target,
            pre,
            routing.stream.as_deref(),
            crate::runtime::now_secs(),
        )
        .await;

        match provisioned {
            Ok(Provisioned::Created { .. }) => {}
            // Unreachable by construction: the id was minted a line ago
            // and 128 bits do not collide. Treated as an error rather than
            // ignored, because if it ever does happen the alternative is
            // handing a caller a tenant that is somebody else's.
            Ok(Provisioned::AlreadyExists { .. }) => {
                return Err(ExecError::internal(
                    "a minted tenant id was already provisioned; nothing was created",
                ))
            }
            Err(refusal) => {
                // **The row is marked, not deleted.** Whatever was made
                // before the refusal — custody keys, a database — is still
                // there, and the row is the only thing that names it.
                // Deleting it here would be the leak this increment
                // exists to stop, arriving through the tidy-up path.
                // `std.space.reap` releases the resources and lands it in
                // `reaped`.
                let _ = tenants::fail(&self.pool, &id, &refusal.to_string(), now).await;
                record("failed");
                // **And the answer says a row was left**, which every
                // message from `refusal_to_error` used to deny in the same
                // breath as being wrong about it. Since phase 0 became a
                // phase of its own, a refusal that reaches this arm has
                // already passed every decidable check, so something was
                // being built and the registry row is what names it. A
                // caller told "nothing was created" here would have no
                // reason to look for the tenant an operator now has to
                // reap.
                let e = refusal_to_error(refusal);
                return Err(ExecError::new(
                    e.code,
                    format!(
                        "{}. Tenant `{id}` was reserved before this failed and is recorded as \
                         `failed`; releasing what it built is `std.space.reap`",
                        e.message
                    ),
                ));
            }
        }

        // **Activation is the last write**, and the only one that makes
        // the space callable. A failure here leaves a fully built tenant
        // in `provisioning`, which the reconciler will time out and an
        // operator can then reap or complete — a state that is visible and
        // recoverable, unlike the resource nobody named.
        match tenants::activate(&self.pool, &id, crate::runtime::now_secs()).await {
            Ok(true) => record("created"),
            Ok(false) => {
                record("not-activated");
                return Err(ExecError::internal(format!(
                    "tenant `{id}` was built and could not be activated because its registry \
                     row is no longer `provisioning`; it is owned by `{owner}`"
                )));
            }
            Err(e) => {
                record("not-activated");
                return Err(ExecError::new(
                    ErrorCode::SpaceUnavailable,
                    format!(
                        "tenant `{id}` was built and not activated ({e}); it is owned by \
                         `{owner}` and does not serve until the control database is writable"
                    ),
                ));
            }
        }

        // The door this replica is holding was built before this tenant
        // existed, and so was every other replica's. Rebuilt here before
        // the answer, so a caller's next call to this replica is served
        // (25e), and announced, so every other replica follows within its
        // rebuild time. A caller on another replica can still be refused
        // for that moment, as `member_ops` says of its own commands.
        let announced = self.serve_new(&id).await;

        Ok(ExecResult {
            position: None,
            rows: vec![json!({
                "space": id,
                "name": name,
                "owner": owner,
                // Honest about the one thing a caller cannot otherwise
                // tell: whether the tenant is reachable *now* or at the
                // next restart.
                "reachable": match &announced {
                    Ok(()) => json!(true),
                    Err(e) => json!({"at": "next restart", "why": e}),
                },
                // A plan is what a call *would* do. This one did it, so
                // there is nothing to describe in the conditional.
                "plan": serde_json::Value::Null,
            })],
            next_cursor: None,
        })
    }

    /// Built-in provisioning: give `principal` a tenant of their own
    /// (`plan-provisioning-on-signup.md` §2.4).
    ///
    /// The same phases [`TenantOps::create`] runs, entered by the platform's
    /// own setting rather than by a deployment owner's call. The differences
    /// are the plan's table: the owner is the principal, the creator
    /// recorded is the same principal, the name is `display` (else the
    /// principal id), the cluster is the deployment's default and never a
    /// caller's, there is no `dryRun`, and no space is bound, so check 3
    /// (bound to the home space) does not apply. Whether the caller may
    /// provision at all, presence included, is the caller's decision.
    ///
    /// **One person, one own tenant.** The registry row carries `own_of` and
    /// a partial unique index makes a second live row for one principal
    /// impossible. A call that finds one, or loses the race to one, answers
    /// with it and makes nothing. A `failed` or `reaped` one does not count,
    /// so the next call provisions again.
    ///
    /// On failure after the row exists the row is `failed` and the error
    /// says so; releasing what was built is `std.space.reap`. The act is
    /// recorded as `Enrolment { act: "own-tenant" }`.
    pub async fn provision_own(
        &self,
        principal: &str,
        display: &str,
    ) -> Result<OwnTenant, ExecError> {
        check_principal(principal)?;
        let started = std::time::Instant::now();
        let _one_at_a_time = self.own_gate.lock().await;
        let queued = started.elapsed();
        if let Some(t) = tenants::own_of(&self.pool, principal)
            .await
            .map_err(registry_unavailable)?
        {
            return Ok(existing(t));
        }

        let name = own_name(principal, display);
        let id = tenants::mint_space_id();

        // The default cluster, always: no parameter reaches `target_for`.
        let attachment = self.target_for(&id, &BoundParams(serde_json::Map::new()))?;
        let routing = self
            .routing
            .as_ref()
            .expect("target_for refuses without it");
        let now = crate::runtime::now_secs();
        let target = crate::provision::Target {
            attachment: &attachment,
            credentials: routing.credentials.as_ref(),
            template: &routing.template,
            hosts: &routing.hosts,
        };
        let pre = crate::provision::preflight(
            &self.pool,
            self.keys.as_ref(),
            &routing.serving,
            &id,
            &target,
            now,
        )
        .await
        .map_err(refusal_to_error)?;

        // Preflight may have waited on another replica's provision, so the
        // latch is looked at again before the row is written. The index is
        // still what decides.
        if let Some(t) = tenants::own_of(&self.pool, principal)
            .await
            .map_err(registry_unavailable)?
        {
            return Ok(existing(t));
        }
        let reserved = tenants::reserve(&self.pool, &id, &name, principal, now, Some(principal))
            .await
            .map_err(registry_unavailable)?;
        if !reserved {
            // The latch held: another request won between the look and the
            // write. Answer with the winner's row.
            return match tenants::own_of(&self.pool, principal)
                .await
                .map_err(registry_unavailable)?
            {
                Some(t) => Ok(existing(t)),
                None => Err(ExecError::new(
                    ErrorCode::SpaceUnavailable,
                    "another request was provisioning this principal's tenant and it did not \
                     complete. Nothing was created here; asking again provisions",
                )),
            };
        }
        self.build_own(&id, principal, &name, &target, pre, started, queued, false)
            .await
    }

    /// Built-in provisioning **without waiting for it** (owner, 2026-10-21:
    /// *"The process of provisioning should not be a waiting call."*).
    ///
    /// Writes the registry row, which is the latch that keeps one person to
    /// one own tenant, and answers at once with `InProgress`. The tenant is
    /// then made by [`TenantOps::finish_own`] on a task of its own. A caller
    /// learns when it is ready from `GET /auth/provisioning`.
    ///
    /// A principal who already has a row is answered with it. When that row
    /// is still `provisioning` and no task in this process is making it (the
    /// process that was making it stopped), it is made again here: every
    /// step of a provision can be run twice for one tenant id.
    pub async fn start_own(
        self: &Arc<Self>,
        principal: &str,
        display: &str,
    ) -> Result<OwnTenant, ExecError> {
        check_principal(principal)?;
        if let Some(t) = tenants::own_of(&self.pool, principal)
            .await
            .map_err(registry_unavailable)?
        {
            if t.state == tenants::State::Provisioning {
                self.spawn_finish(t.id.clone(), principal.to_string(), t.name.clone());
            }
            return Ok(existing(t));
        }
        let name = own_name(principal, display);
        let id = tenants::mint_space_id();
        // Refused here, before any row is written, when this deployment
        // cannot give a tenant a database.
        self.target_for(&id, &BoundParams(serde_json::Map::new()))?;
        let now = crate::runtime::now_secs();
        let reserved = tenants::reserve(&self.pool, &id, &name, principal, now, Some(principal))
            .await
            .map_err(registry_unavailable)?;
        if !reserved {
            // Two sign-ups for one person at the same moment: the index let
            // one row in, and this answers with it.
            return match tenants::own_of(&self.pool, principal)
                .await
                .map_err(registry_unavailable)?
            {
                Some(t) => Ok(existing(t)),
                None => Err(ExecError::new(
                    ErrorCode::SpaceUnavailable,
                    "another request was provisioning this principal's tenant and it did not \
                     complete. Nothing was created here; asking again provisions",
                )),
            };
        }
        // The person's first call to it is answered `tenant_not_ready` on
        // this server at once, and on the others from their next sweep.
        self.not_ready().hold(&id, principal);
        self.spawn_finish(id.clone(), principal.to_string(), name);
        Ok(OwnTenant::InProgress { space: id })
    }

    /// Make the tenant a row already names, on a task of its own, when this
    /// server takes its setup lease.
    pub fn spawn_finish(self: &Arc<Self>, id: String, principal: String, name: String) {
        let ops = self.clone();
        tokio::spawn(async move {
            ops.start_setup(id, principal, name).await;
        });
    }

    /// **Take the setup lease on `id` and, when this server won it, make the
    /// tenant on a task of its own** (owner, 2026-10-21). Answers whether
    /// this server took it. A task in this process already making it, or
    /// another server's live lease, answers `false` and starts nothing.
    pub async fn start_setup(
        self: &Arc<Self>,
        id: String,
        principal: String,
        name: String,
    ) -> bool {
        let Some(claim) = InFlight::claim(self, &id) else {
            return false;
        };
        let lease = self.setup().lease_secs;
        let now = crate::runtime::now_secs();
        match tenants::claim_setup(&self.pool, &id, &self.holder, now, lease).await {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                eprintln!("tenant {id}: setup lease not taken: {e}");
                return false;
            }
        }
        self.not_ready().hold(&id, &principal);
        let ops = self.clone();
        tokio::spawn(async move {
            let _claim = claim;
            ops.run_setup(&id, &principal, &name).await;
        });
        true
    }

    /// One try at a setup whose lease this server holds. The lease is
    /// renewed while it runs. A try that fails leaves the row
    /// `provisioning` and sets when the next may start: nothing is marked
    /// failed and nothing is deleted.
    async fn run_setup(&self, id: &str, principal: &str, name: &str) {
        let settings = self.setup();
        let renew = async {
            let every = std::time::Duration::from_secs((settings.lease_secs / 3).max(1));
            loop {
                tokio::time::sleep(every).await;
                let _ = tenants::claim_setup(
                    &self.pool,
                    id,
                    &self.holder,
                    crate::runtime::now_secs(),
                    settings.lease_secs,
                )
                .await;
            }
        };
        let result = tokio::select! {
            r = self.finish_own(id, principal, name) => r,
            () = renew => return,
        };
        match result {
            Ok(OwnTenant::InProgress { .. }) => {}
            Ok(_) => self.not_ready().lift(id),
            Err(e) => {
                let failed = tenants::lookup(&self.pool, id)
                    .await
                    .ok()
                    .flatten()
                    .map_or(0, |t| t.attempts)
                    + 1;
                let wait = settings.backoff(failed);
                let at = crate::runtime::now_secs().saturating_add(wait);
                let _ = tenants::retry_later(&self.pool, id, &e.message, at).await;
                eprintln!(
                    "tenant {id}: setting up failed (try {failed}); trying again in {wait} s: {}",
                    e.message
                );
                return;
            }
        }
        let _ = tenants::release_setup(&self.pool, id, &self.holder).await;
    }

    /// **The setup sweep** (owner, 2026-10-21). Every own tenant still being
    /// set up is looked at: one whose lease ran out and whose next try is
    /// due is taken and started, one past `ORDO_PROVISION_STUCK_SECS` is
    /// reported stuck once, and this server's
    /// [`crate::tenant_setup::NotReady`] map is replaced with what the
    /// registry says.
    pub async fn sweep_setups(
        self: &Arc<Self>,
        now: u64,
    ) -> Result<crate::tenant_setup::Swept, ordo_exec::PoolError> {
        let settings = self.setup();
        let rows = tenants::setting_up(&self.pool).await?;
        let mut swept = crate::tenant_setup::Swept::default();
        let mut held = std::collections::HashMap::new();
        for t in rows {
            let Some(principal) = t.own_of.clone() else {
                continue;
            };
            swept.in_progress += 1;
            held.insert(t.id.clone(), principal.clone());
            if now.saturating_sub(t.created_at) >= settings.stuck_secs {
                swept.stuck += 1;
                if t.stuck_at == 0 && tenants::mark_stuck(&self.pool, &t.id, now).await? {
                    self.report_stuck(&t, &principal, now);
                }
            }
            if self.routing.is_some()
                && t.lease_until <= now
                && t.retry_at <= now
                && self
                    .start_setup(t.id.clone(), principal, t.name.clone())
                    .await
            {
                swept.started += 1;
            }
        }
        self.not_ready().replace(held);
        Ok(swept)
    }

    /// Tell the app developer and the deployment owner a setup is stuck:
    /// the security-event stream, at deployment scope and in the tenant's
    /// own, and the log.
    fn report_stuck(&self, t: &tenants::Tenant, principal: &str, now: u64) {
        let secs = now.saturating_sub(t.created_at);
        eprintln!(
            "tenant {}: setting up is stuck after {secs} s ({} failed tries, last step `{}`). \
             Still trying. Stuck means longer than {} ({} s)",
            t.id,
            t.attempts,
            t.phase.as_str(),
            crate::tenant_setup::STUCK_ENV,
            self.setup().stuck_secs,
        );
        if let Some(stream) = self.routing.as_ref().and_then(|r| r.stream.as_deref()) {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "own-tenant",
                    detail: format!(
                        "principal={principal} space={} result=stuck seconds={secs} \
                         attempts={} step={}",
                        t.id,
                        t.attempts,
                        t.phase.as_str()
                    ),
                    by: Some(principal),
                    tenant: Some(&t.id),
                    deployment: true,
                    at: now,
                },
            );
        }
    }

    /// Resume every own tenant left `provisioning`: what a process that
    /// stopped mid-provision left behind. Called at boot. Returns how many
    /// this server took; one whose lease another server holds is left to it.
    pub async fn resume_own(self: &Arc<Self>) -> usize {
        if self.routing.is_none() {
            return 0;
        }
        let Ok(all) = tenants::setting_up(&self.pool).await else {
            return 0;
        };
        let mut started = 0;
        for t in all {
            let Some(principal) = t.own_of.clone() else {
                continue;
            };
            let id = t.id.clone();
            if self.start_setup(t.id, principal, t.name).await {
                println!("tenant {id}: resuming its provisioning");
                started += 1;
            }
        }
        started
    }

    /// Make the own tenant whose row `id` already holds: preflight, the
    /// database and its schema, the founding member, activation and the
    /// door. Safe to run again for one id, and safe to run beside another
    /// run for the same id: the apply lock puts the two in turn, and every
    /// step finds what the first one made.
    ///
    /// A refusal leaves the row `provisioning` and answers the error; the
    /// caller decides when to try again ([`TenantOps::start_setup`]). The
    /// person keeps their one tenant whatever happens (owner, 2026-10-21).
    pub async fn finish_own(
        &self,
        id: &str,
        principal: &str,
        name: &str,
    ) -> Result<OwnTenant, ExecError> {
        let started = std::time::Instant::now();
        let _one_at_a_time = self.own_gate.lock().await;
        let queued = started.elapsed();
        match tenants::lookup(&self.pool, id)
            .await
            .map_err(registry_unavailable)?
        {
            Some(t) if t.state == tenants::State::Provisioning => {}
            // Made, failed or reaped while this waited: nothing to do.
            Some(t) => return Ok(existing(t)),
            None => {
                return Err(ExecError::internal(format!(
                    "tenant `{id}` has no registry row to provision"
                )))
            }
        }
        let attachment = self.target_for(id, &BoundParams(serde_json::Map::new()))?;
        let routing = self
            .routing
            .as_ref()
            .expect("target_for refuses without it");
        let target = crate::provision::Target {
            attachment: &attachment,
            credentials: routing.credentials.as_ref(),
            template: &routing.template,
            hosts: &routing.hosts,
        };
        let pre = match crate::provision::preflight(
            &self.pool,
            self.keys.as_ref(),
            &routing.serving,
            id,
            &target,
            crate::runtime::now_secs(),
        )
        .await
        {
            Ok(pre) => pre,
            Err(refusal) => return Err(refusal_to_error(refusal)),
        };
        self.build_own(id, principal, name, &target, pre, started, queued, true)
            .await
    }

    /// The part of an own tenant's provision after its row is reserved and
    /// its preflight passed. Shared by the waiting path and the job.
    #[allow(clippy::too_many_arguments)]
    async fn build_own(
        &self,
        id: &str,
        principal: &str,
        name: &str,
        target: &crate::provision::Target<'_>,
        pre: crate::provision::Preflight,
        started: std::time::Instant,
        queued: std::time::Duration,
        keep_trying: bool,
    ) -> Result<OwnTenant, ExecError> {
        let preflighted = started.elapsed();
        let routing = self
            .routing
            .as_ref()
            .expect("target_for refuses without it");
        let now = crate::runtime::now_secs();
        let founding = FoundingOwner {
            member_id: principal,
            principal,
            display_name: name,
        };
        let label = pre.label();
        tenants::at_phase(&self.pool, id, tenants::Phase::Targeted, &label, now)
            .await
            .map_err(registry_unavailable)?;

        let provisioned = crate::provision::provision_prepared(
            &self.pool,
            self.keys.as_ref(),
            id,
            &founding,
            target,
            pre,
            routing.stream.as_deref(),
            crate::runtime::now_secs(),
        )
        .await;
        let built = started.elapsed();
        let record = |result: &str| {
            if let Some(stream) = routing.stream.as_deref() {
                crate::enrolment::record(
                    stream,
                    &crate::enrolment::Act {
                        act: "own-tenant",
                        detail: format!("principal={principal} space={id} result={result}"),
                        by: Some(principal),
                        tenant: Some(id),
                        deployment: true,
                        at: crate::runtime::now_secs(),
                    },
                );
            }
        };
        match provisioned {
            // `AlreadyExists` is a run that found the founding member a
            // stopped run had written: the provision is being finished.
            Ok(Provisioned::Created { .. }) | Ok(Provisioned::AlreadyExists { .. }) => {}
            Err(refusal) if keep_trying => {
                // The setup sweep tries again; the row stays `provisioning`.
                record("retrying");
                return Err(refusal_to_error(refusal));
            }
            Err(refusal) => {
                let _ = tenants::fail(&self.pool, id, &refusal.to_string(), now).await;
                record("failed");
                let e = refusal_to_error(refusal);
                return Err(ExecError::new(
                    e.code,
                    format!(
                        "{}. Tenant `{id}` was reserved before this failed and is recorded as \
                         `failed`; releasing what it built is `std.space.reap`",
                        e.message
                    ),
                ));
            }
        }
        match tenants::activate(&self.pool, id, crate::runtime::now_secs()).await {
            Ok(true) => {}
            Ok(false) => {
                // Another run for this id activated it first.
                if let Ok(Some(t)) = tenants::lookup(&self.pool, id).await {
                    if t.state == tenants::State::Active {
                        return Ok(OwnTenant::Existing { space: t.id });
                    }
                }
                record("failed");
                return Err(ExecError::internal(format!(
                    "tenant `{id}` was built and could not be activated because its registry \
                     row is no longer `provisioning`"
                )));
            }
            Err(e) => {
                record("failed");
                return Err(ExecError::new(
                    ErrorCode::SpaceUnavailable,
                    format!(
                        "tenant `{id}` was built and not activated ({e}); it does not serve \
                         until the control database is writable"
                    ),
                ));
            }
        }
        record("created");
        let activated = started.elapsed();
        // The person's first call follows at once: see `serve_new`.
        let announced = self.serve_new(id).await;
        let ms = |d: std::time::Duration| d.as_millis();
        println!(
            "tenant {id}: provisioned in {} ms (waited {} ms for another sign-up, preflight {} \
             ms, database and schema {} ms, activate {} ms, door {} ms)",
            ms(started.elapsed()),
            ms(queued),
            ms(preflighted - queued),
            ms(built - preflighted),
            ms(activated - built),
            ms(started.elapsed() - activated),
        );
        Ok(OwnTenant::Created {
            space: id.to_string(),
            reachable: announced.is_ok(),
        })
    }
}

/// A principal becomes its tenant's founding member id.
fn check_principal(principal: &str) -> Result<(), ExecError> {
    if ordo_api::well_formed_space(principal) {
        return Ok(());
    }
    Err(ExecError::new(
        ErrorCode::TenantRefused,
        "a principal becomes its tenant's founding member id, so it must be one the \
         door can address: letters, digits, `_` and `-`. Nothing was created",
    ))
}

fn registry_unavailable(e: ordo_exec::PoolError) -> ExecError {
    ExecError::new(
        ErrorCode::SpaceUnavailable,
        format!("the tenant registry could not be read or written ({e})"),
    )
}

/// What an own-tenant row found in the registry answers.
fn existing(t: tenants::Tenant) -> OwnTenant {
    match t.state {
        // A paused own tenant is still theirs: answered with it (27ar).
        tenants::State::Active | tenants::State::Paused => OwnTenant::Existing { space: t.id },
        _ => OwnTenant::InProgress { space: t.id },
    }
}

/// The tenant's name: the display name, else the principal id.
fn own_name(principal: &str, display: &str) -> String {
    let display = display.trim();
    if display.is_empty() {
        principal
    } else {
        display
    }
    .chars()
    .take(MAX_NAME_LEN as usize)
    .collect()
}

/// A tenant this process is making now. Released when dropped.
struct InFlight {
    ops: Arc<TenantOps>,
    id: String,
}

impl InFlight {
    fn claim(ops: &Arc<TenantOps>, id: &str) -> Option<InFlight> {
        let mut held = ops.in_flight.lock().expect("in-flight lock");
        if !held.insert(id.to_string()) {
            return None;
        }
        Some(InFlight {
            ops: ops.clone(),
            id: id.to_string(),
        })
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Ok(mut held) = self.ops.in_flight.lock() {
            held.remove(&self.id);
        }
    }
}

/// A provisioning refusal in the executor's vocabulary.
///
/// `Malformed` **can** reach a caller, and only ever about the owner: the
/// space id was minted by this build and passes by construction, while the
/// owner is the caller's string and becomes the member id. So it is
/// `invalid_param` naming the rule, not an internal error — a principal
/// with a `:` or a space in it is something the caller can fix, and
/// nothing was written.
fn refusal_to_error(refusal: ProvisionRefusal) -> ExecError {
    match refusal {
        ProvisionRefusal::Malformed(_) => ExecError::new(
            ErrorCode::TenantRefused,
            format!(
                "`owner` becomes this tenant's founding member id, so it must be one the door \
                 can address: letters, digits, `_` and `-` ({refusal}). Nothing was created"
            ),
        ),
        // **18d §9.8's refusals reach the caller.** Each is a decision
        // about the target the caller named, so answering `internal` would
        // tell an operator who mistyped a host that the platform broke.
        // `TenantRefused` is 400 and is executor-raisable; `invalid_param`
        // is the door's word and an executor may not say it (§4.1).
        //
        // `HostNotAllowed` carries **one reason and no detail**, which is
        // `ordo_egress::refusal`'s argument: a distinguishable refusal is
        // an oracle that maps the allowlist one attempt at a time. The
        // others name what they refused, because they are about the
        // caller's own tenant and disclose nothing they did not supply.
        ProvisionRefusal::HostNotAllowed { .. } => ExecError::new(
            ErrorCode::TenantRefused,
            "that host is not one this deployment may attach a tenant to. Nothing was created",
        ),
        ProvisionRefusal::ControlDatabase { .. }
        | ProvisionRefusal::DatabaseOccupied { .. }
        | ProvisionRefusal::Occupied { .. } => ExecError::new(
            ErrorCode::TenantRefused,
            format!("{refusal}. Nothing was created"),
        ),
        // Unreachable from this op by construction — the id was minted a
        // moment ago, so no attachment can already stand for it — and kept
        // opaque anyway, because `AttachRefusal::WouldMove` formats the
        // standing target's host, port and database, and an unreachable
        // arm is exactly the one nobody re-reads before it fires.
        ProvisionRefusal::Attach(_) => ExecError::new(
            ErrorCode::TenantRefused,
            "that tenant is already attached to a different database. Nothing was created",
        ),
        // The target could not be reached or could not be written. Not the
        // caller's to fix and not a permanent answer, so 503 rather than
        // 400 or 500: a retry is the right next act.
        //
        // **The cause is not in the message**, which is the invariant
        // `space_unavailable` is documented with in
        // `docs/api-engine-design.md`: a dial error names a host, a port
        // and often a user, and that is deployment topology a caller has
        // no business reading — for a tenant on their own cluster it is
        // their own infrastructure described back to them by a third
        // party. The reason an operator needs is on `/health/ready`.
        // Waited for a deploy and it is still going. 503 rather than 400:
        // nothing about the call was wrong and the identical one lands.
        ProvisionRefusal::RoutingBusy { .. } => ExecError::new(
            ErrorCode::SpaceUnavailable,
            "a schema apply is in flight, so a tenant cannot be given a database right now. \
             Nothing was created, and the identical call lands once it finishes",
        ),
        ProvisionRefusal::Target(_) => ExecError::new(
            ErrorCode::SpaceUnavailable,
            "the tenant's database could not be prepared, and the identical call lands once \
             it is reachable",
        ),
        // **A custody provider that refuses is a dependency being down,
        // not a bug** (11-15). This was a 500 until phase 0 became a phase
        // of its own, which was defensible only while nothing here
        // distinguished the caller's mistakes from the infrastructure's:
        // `bootstrap.md` §4.3 lists the KMS first among the steps that
        // cannot be transactional, and §2 item 22 wants an unhealthy
        // provider refused in preflight — which this build cannot yet ask,
        // because `KeyProvider` has no health question. Until it does, the
        // failure lands in phase 2, and the honest answer for it is the
        // retryable one rather than "something went wrong here".
        ProvisionRefusal::Custody(_) => ExecError::new(
            ErrorCode::SpaceUnavailable,
            "the tenant's custody keys could not be minted, and the identical call lands \
             once the key provider answers",
        ),
        ProvisionRefusal::Store(ref e) => {
            ExecError::of_fault(e.class(), ErrorCode::Internal, refusal.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The op name is `bootstrap.md` §1's, deliberately: that document has
    /// specified this surface since before the code existed, and a second
    /// name for it would leave two designs for one act.
    #[test]
    fn the_op_is_the_one_the_design_already_named() {
        assert_eq!(CREATE, "std.space.create");
    }
}
