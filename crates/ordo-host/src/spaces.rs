//! The host's per-space resolver: **many spaces, one process**.
//!
//! # What was single-space, and what was not
//!
//! Nothing below this crate was. Every table carries `"$space"`, every
//! generated statement is confined to one, the primary key is
//! `(space, id)`, no ref crosses a space, and keys are per space by
//! construction. The door has bound a space per request since SH-19: the
//! header or `$space` names it, the access check is made against it, and
//! `Gate::approved_space` refuses to hand the executor a space other than
//! the one that was authorized.
//!
//! What was single-space was the **wiring**. Two per-space things were
//! built once, at construction, and read off a constant:
//!
//! - one `ordo_exec::Executor`, whose `space` field is *"the isolation
//!   boundary rather than a label"*, and
//! - one `WriteLog`, whose chain head is per space by definition.
//!
//! So the executor asserted that the door's space equalled the constant
//! and refused otherwise, which was the honest form of a single-space
//! deployment and is now the thing this module deletes.
//!
//! # Resolved, not threaded
//!
//! The fix is deliberately not "pass the space down to the call sites".
//! `Executor::space` is worth having precisely because no caller chooses
//! it: a space that arrives as an argument is a space some future call site
//! can pass wrongly, and the failure that produces (plan one tenant's
//! statement, execute it in another's rows) is the one the field exists to
//! make unreachable. So the space stays a field and the host holds **N of
//! them**, resolved once per request from the binding the door already
//! made.
//!
//! [`Spaces::resolve`] is the only place that resolution happens.
//!
//! # Why the two caches live in different places
//!
//! A [`crate::deploy::SpaceView`] carries this version's schema and
//! registry, so it belongs to a version and is cached inside
//! [`crate::deploy::Loaded`], so a swap drops the cache with the version it
//! described. A [`WriteLog`] carries a chain head, which belongs to the
//! *space* and outlives every deploy: the chain is the space's audit
//! spine, and a swap must not fork it. So the logs are cached here, keyed
//! by space, for the life of the process.
//!
//! One consequence worth stating: the op path and the blob door must share
//! this resolver, not merely hold equivalent ones. Two resolvers would mean
//! two logs for one space, and two logs for one space is two chains: the
//! one thing a chain cannot be.
//!
//! # Which database
//!
//! The resolver also says where a space's rows are, through an
//! `ordo_exec::route::PoolResolver` held here beside the logs. Held here
//! for the logs' reason: a space's database belongs to the space and
//! outlives every deploy, so it cannot live in a version. And held once,
//! for the chain's reason: the op path and the blob door reach a space's
//! rows through this resolver, so they reach the same database, and a
//! space that is moved is moved for both at once.
//!
//! The production server always builds [`Spaces::routed`] over
//! `AttachedPools`, because every tenant has a database of its own (ruled
//! 2026-09-25, 2026-10-06). [`Spaces::new`] puts `SinglePool` over one
//! pool, which answers the same handle for every space; tests and the
//! operator tool that reach one database use it, and no deployment can
//! configure it. The executor never resolves: it is handed the pool this
//! module resolved, the way it is handed the space.
//!
//! # What "unprovisioned" is refused by
//!
//! Custody. [`crate::provision`] mints a space's keys before it writes a
//! row, so a space with no SMK is a space nothing has ever provisioned, and
//! resolving one is refused rather than served on a chain and a fixture
//! minted out of a header. The door refuses first in every ordinary case
//! (an unprovisioned space has no membership, so the access check denies
//! without disclosing whether it exists); this is the layer below saying
//! the same thing, and it is the layer that would still say it if a future
//! access checker were wrong.
//!
//! # A tenant's history is refused at the tenant's boundary (17aa)
//!
//! The owner, 2026-09-21: *"The system is the control plane and should
//! function without teeth into those. Dependence is on the control plane,
//! not the deployment underneath."*
//!
//! A chain that has moved backwards unannounced used to refuse the whole
//! process at `assemble`, which made the control plane depend on one
//! tenant's data state and removed the door through which
//! `ordo restore --record` explains the rewind. It is answered here now,
//! per space, and it is answered in exactly one function
//! ([`Spaces::continuity_of`]) that both the boot
//! ([`Spaces::reconcile`]) and the first request to name a space
//! ([`Spaces::resolve`]) call. So the space is quarantined, the deployment
//! keeps serving every other tenant and the control plane, and
//! `/health/ready` names what an operator has to repair.
//!
//! **This loosens nothing about the refusal itself.** A quarantined space
//! does not serve. The control plane staying up is what makes the repair
//! possible, never permission to serve data whose history is in question.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ordo_conformance::WriteLog;
use ordo_exec::route::{PoolResolver, SinglePool};
use ordo_exec::undo::{self, Continuity, Db};
use ordo_exec::witness::Witness;
use ordo_exec::Pool;

use crate::deploy::{Deployment, Loaded, SpaceView};

/// Everything one request needs that carries its space.
///
/// Held together for [`crate::deploy::Loaded`]'s reason: a call that read
/// the version and the space binding at two moments could be handed one of
/// each from two different answers.
pub struct Bound {
    /// The live program, held for the whole call so a swap landing
    /// mid-request does not move the schema out from under it.
    pub live: Arc<Loaded>,
    /// That version, cut to this request's space.
    pub view: Arc<SpaceView>,
    /// This space's chain head. Shared across every request naming the
    /// space, because each op's group appends under the same head.
    pub log: Arc<Mutex<WriteLog>>,
}

/// Why a space could not be resolved.
///
/// Neither variant is the caller's doing. The caller has already been
/// authorized in the space it named, so there is nothing here to
/// enumerate; what there is to say is that the deployment agreed to serve a
/// space it cannot serve, which is the deployment disagreeing with itself.
#[derive(Debug)]
#[non_exhaustive]
pub enum SpaceRefusal {
    /// No key custody for this space. Either nothing provisioned it, or a
    /// provision was interrupted before it finished.
    NoCustody(String),
    /// The space's database could not be reached. Refused here, before
    /// anything is planned, so that an unreachable target fails a request
    /// at its start rather than partway through a write. `class` is the
    /// database client's own (`PoolError::class`): busy is a full pool,
    /// which reached the database and is not a fact about this space.
    Unroutable {
        space: String,
        reason: String,
        class: ordo_exec::FaultClass,
    },
    /// **The space's chain has moved backwards and nothing says so**
    /// (R2d-iii). Refused rather than served, because a silent restore is
    /// the one thing the audit spine must never allow, and the mechanism
    /// for that is refusal to serve rather than a log line.
    ///
    /// The refusal lifts the moment the restore is recorded: the next
    /// request re-asks, because a space is cached only once it has passed.
    Rewound {
        space: String,
        continuity: Continuity,
    },
    /// **The space's database is at a version other than the one this
    /// deployment serves**, and is being brought to it (25i).
    ///
    /// Its own variant, and not [`SpaceRefusal::Unroutable`], because the
    /// two are told to the caller differently: an unreachable database is a
    /// dependency that is down, and this is the app changing under the
    /// tenant. The reason (both versions, and any change that was refused)
    /// stays in the register and the log and is not carried to the caller.
    SchemaBehind { space: String, reason: String },
}

impl std::fmt::Display for SpaceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpaceRefusal::NoCustody(s) => write!(
                f,
                "no key custody for space `{s}`: this deployment serves no such space"
            ),
            SpaceRefusal::Unroutable { space, reason, .. } => write!(
                f,
                "space `{space}` is in a database this node cannot reach: {reason}"
            ),
            SpaceRefusal::Rewound { space, continuity } => {
                write!(f, "space `{space}` is not served: {continuity}")
            }
            SpaceRefusal::SchemaBehind { space, reason } => {
                write!(f, "space `{space}` is not served: {reason}")
            }
        }
    }
}

impl std::error::Error for SpaceRefusal {}

impl SpaceRefusal {
    /// What a caller can do about it: the database client's class for an
    /// unreachable database, and broken for every refusal that is a fact
    /// about the space.
    pub fn class(&self) -> ordo_exec::FaultClass {
        match self {
            SpaceRefusal::Unroutable { class, .. } => *class,
            _ => ordo_exec::FaultClass::Broken,
        }
    }

    /// The database was reached and this deployment's pool for it was full
    /// (25z). Read from the class, never from the reason's words.
    pub fn is_busy(&self) -> bool {
        self.class().is_busy()
    }
}

/// What asking the continuity question about one space found (17aa).
///
/// The same answer whether the question was asked at boot
/// ([`Spaces::reconcile`]) or by the first request to name the space
/// ([`Spaces::resolve`]), because it is one question asked in one place.
#[derive(Debug, Clone)]
pub enum Reconciled {
    /// The chain agrees with every record kept of it, or contradicts one a
    /// recorded restore accounts for. The space serves.
    ///
    /// **What reconciliation settles on its own is exactly this**: a
    /// difference somebody already wrote down. It reads the marker, checks
    /// that the chain reaches it, and lets the space serve. It never
    /// writes a marker, and it never decides that a rewind looks plausible
    /// enough to wave through: accepting a rewound chain because it looks
    /// reasonable is the act the whole restore mechanism exists to prevent.
    Served(Continuity),
    /// The chain moved backwards and nothing says so. The space is held
    /// closed until an operator records the restore through the door,
    /// which is up, because this is a fact about one tenant.
    Quarantined {
        space: String,
        continuity: Continuity,
    },
    /// The question could not be asked: the space's chain or the witness
    /// could not be read. *"I could not check"* is not *"it checks out"*,
    /// so the space is held closed with the read error as the reason.
    /// `class` is the read error's own: a full pool is busy, and holds
    /// nothing closed (25z).
    Unreadable {
        space: String,
        why: String,
        class: ordo_exec::FaultClass,
    },
    /// **The space's database is not at the version this deployment is
    /// serving** (18d R-S4). Its tables are some other artifact's, so the
    /// serving program references columns that may not be there, and the
    /// failure would otherwise arrive per statement, late, and
    /// differently for each request.
    ///
    /// **Version unknown is not behind.** A database with a schema and no
    /// `$migrations` row predates R-S1, and nothing here guesses what it
    /// holds: a guess written down becomes indistinguishable from a
    /// version that was actually applied. Such a target serves, and an
    /// operator resolves it once.
    ///
    /// **A deployment with no active program does not quarantine
    /// anybody.** `Deployment::empty` carries `(none)` as its version,
    /// which is not a version any target can be at or behind, and a
    /// deployment waiting for its first `ordo apply` is the ordinary
    /// first-boot state rather than a fleet that has drifted.
    SchemaBehind {
        space: String,
        /// What the target says it is at.
        at: String,
        /// What this deployment is serving.
        serving: String,
    },
    /// **This deployment holds no custody for the space** (17ac). Nothing
    /// provisioned it, or a provision was interrupted before it finished.
    ///
    /// Asked here and not only on the request path so that the answer
    /// reaches `/health/ready` by the same route every other closed space
    /// does. Until 17ac this was a **boot refusal**: a deployment whose
    /// `ORDO_SPACE` named an unprovisioned tenant exited before binding,
    /// which is the shape 17aa removed everywhere else — one tenant's
    /// state stopping the whole process.
    ///
    /// The property that refusal existed to hold is kept in full, and it
    /// was never the refusal that held it: `current_epoch` reads and never
    /// creates, so a typo in `ORDO_SPACE` still mints nothing. What
    /// changes is that the typo now closes one space and names it, instead
    /// of stopping a deployment that may be serving other tenants
    /// perfectly well.
    Unprovisioned { space: String },
}

impl Reconciled {
    /// Does this verdict let the space serve?
    pub fn serves(&self) -> bool {
        matches!(self, Reconciled::Served(_))
    }

    /// One line an operator reads: what was found, and what they must do
    /// about it when the answer is that the space is closed.
    pub fn why(&self) -> String {
        match self {
            Reconciled::Served(c) => c.to_string(),
            Reconciled::Quarantined { continuity, .. } => continuity.to_string(),
            Reconciled::Unreadable { why, .. } => {
                format!("this space's continuity could not be read: {why}")
            }
            Reconciled::SchemaBehind { at, serving, .. } => format!(
                "schema behind: this space's database is at `{at}` and this deployment \
                 serves `{serving}`. It is held closed until it is brought to that version"
            ),
            // Carries the remedy, because this is the one closed state an
            // operator fixes with a command rather than an investigation.
            Reconciled::Unprovisioned { space } => format!(
                "no key custody on this deployment: nothing has created the tenant `{space}`, \
                 or a provision was interrupted. run:  ordo keys init --space {space} --as \
                 <you>"
            ),
        }
    }
}

/// The resolver: one deployment, N spaces.
pub struct Spaces {
    deployment: Arc<Deployment>,
    pools: Arc<dyn PoolResolver>,
    logs: Mutex<HashMap<String, Arc<Mutex<WriteLog>>>>,
    /// The control database's record of every signed head. A handle rather
    /// than something derived from `pools`, because the witness is not a
    /// space's database: it is the deployment's, and the host binary does
    /// not construct an `AttachedPools` it could be read from.
    witness: Witness,
    /// Spaces whose continuity has been checked and passed, for the life of
    /// the process.
    ///
    /// **Passing verdicts only.** A cached refusal would keep refusing after
    /// `ordo restore --record` had fixed it, and every replica would have to
    /// be restarted to serve again, which turns a recorded restore into an
    /// outage. A cached pass can go stale the other way, and what catches
    /// that is stated where the check runs.
    continuous: Mutex<HashSet<String>>,
    /// **Spaces this deployment is holding closed, and why** — a report,
    /// never a decision (17aa).
    ///
    /// Nothing reads this to refuse: [`Spaces::resolve`] re-asks the
    /// continuity question every time for a space that has not passed,
    /// which is what makes a recorded restore lift without a restart. What
    /// reads it is `/health/ready`, so an operator learns which tenant is
    /// closed from the deployment rather than from that tenant's customers.
    /// An entry is written when the question is answered "closed" and
    /// removed the moment it is answered "serves".
    ///
    /// Ordered, so two replicas' bodies list the same spaces in the same
    /// order and a diff between them is a real difference.
    quarantined: Mutex<BTreeMap<String, String>>,
    /// **The spaces held closed because their database is behind** (25i):
    /// the subset of [`Self::quarantined`] the catch-up
    /// ([`crate::tenant_forward`]) works from. Written where the schema
    /// comparison answers "behind" and cleared with the quarantine, so
    /// nothing else has to be walked to find them.
    behind: Mutex<BTreeSet<String>>,
    /// **A change that was refused for a space's database, and for which
    /// serving version** (25i), so the reason `/health/ready` gives says what
    /// was refused and the catch-up does not ask again for the same version.
    /// `(version, reason)`, cleared when the space serves.
    refusals: Mutex<BTreeMap<String, (String, String)>>,
    /// How many continuity checks have run, so a test can assert "once per
    /// process for a serving space" rather than infer it.
    checks: AtomicUsize,
    /// **The tenants this deployment holds paused, and how** (27ar).
    ///
    /// Read on every op call by the pause gate (`crate::pause::gate`), so it
    /// is memory and never a query. Loaded from the registry at boot,
    /// written here at once by a pause or resume on this replica, and
    /// reloaded from the registry when another replica announces one and on
    /// the refresh timer. Separate from [`Self::quarantined`] on purpose: a
    /// pause is a decision somebody made, not a fault `/health/ready`
    /// should report.
    paused: Mutex<BTreeMap<String, crate::pause::PauseRecord>>,
    /// What a paused tenant answers, per reason: the app developer's setting.
    pause_answers: Mutex<crate::pause::PauseAnswers>,
}

impl Spaces {
    /// A resolver that routes every space to the pool the deployment booted
    /// with. The default, and what a deployment that attaches nothing gets.
    pub fn new(deployment: Arc<Deployment>) -> Arc<Spaces> {
        let pool = deployment.current().exec.pool.clone();
        let pools = Arc::new(SinglePool::new(pool.clone()));
        Spaces::routed(deployment, pools, Witness::new(pool))
    }

    /// A resolver that routes each space through `pools`, with `witness`
    /// over the control database.
    pub fn routed(
        deployment: Arc<Deployment>,
        pools: Arc<dyn PoolResolver>,
        witness: Witness,
    ) -> Arc<Spaces> {
        Arc::new(Spaces {
            deployment,
            pools,
            logs: Mutex::new(HashMap::new()),
            witness,
            continuous: Mutex::new(HashSet::new()),
            quarantined: Mutex::new(BTreeMap::new()),
            behind: Mutex::new(BTreeSet::new()),
            refusals: Mutex::new(BTreeMap::new()),
            checks: AtomicUsize::new(0),
            paused: Mutex::new(BTreeMap::new()),
            pause_answers: Mutex::new(crate::pause::PauseAnswers::default()),
        })
    }

    /// Whether this tenant is paused, and how. `None` serves.
    pub fn paused_record(&self, space: &str) -> Option<crate::pause::PauseRecord> {
        self.paused
            .lock()
            .expect("pause register is not poisoned")
            .get(space)
            .cloned()
    }

    /// What a paused tenant answers, per reason.
    pub fn pause_answers(&self) -> crate::pause::PauseAnswers {
        self.pause_answers
            .lock()
            .expect("pause answers are not poisoned")
            .clone()
    }

    /// Set what a paused tenant answers, at boot.
    pub fn set_pause_answers(&self, answers: crate::pause::PauseAnswers) {
        *self
            .pause_answers
            .lock()
            .expect("pause answers are not poisoned") = answers;
    }

    /// Hold a tenant paused on this replica.
    pub fn hold_paused(&self, record: crate::pause::PauseRecord) {
        self.paused
            .lock()
            .expect("pause register is not poisoned")
            .insert(record.space.clone(), record);
    }

    /// Lift a pause on this replica.
    pub fn lift_paused(&self, space: &str) {
        self.paused
            .lock()
            .expect("pause register is not poisoned")
            .remove(space);
    }

    /// Replace the whole register with what the registry says now, so a
    /// resume another replica made is lifted here as well as a pause.
    pub fn replace_paused(&self, records: Vec<crate::pause::PauseRecord>) {
        let mut held = self.paused.lock().expect("pause register is not poisoned");
        held.clear();
        for r in records {
            held.insert(r.space.clone(), r);
        }
    }

    /// Every tenant held paused here, by id.
    pub fn paused_spaces(&self) -> Vec<String> {
        self.paused
            .lock()
            .expect("pause register is not poisoned")
            .keys()
            .cloned()
            .collect()
    }

    /// How many continuity checks have run. For tests.
    pub fn continuity_checks(&self) -> usize {
        self.checks.load(Ordering::Relaxed)
    }

    /// The pool resolver, for a surface that reaches a space's rows without
    /// going through [`Spaces::resolve`] (export, erasure, a chain proof).
    /// Handing out this `Arc` rather than building a second resolver is what
    /// keeps every surface reading a space from the one database it is in.
    pub fn pools(&self) -> &Arc<dyn PoolResolver> {
        &self.pools
    }

    /// The pool a space's rows are reached through.
    ///
    /// Routing only: this does not ask whether the space exists, which
    /// [`Spaces::resolve`] asks first. A caller using this directly has
    /// bound its space some other way (the blob door's read path reads the
    /// reference row it was handed a handle for).
    pub async fn pool_of(&self, space: &str) -> Result<Pool, SpaceRefusal> {
        let id = ordo_crypto::SpaceId(space.to_string());
        self.pools
            .resolve(&id)
            .await
            // Charged to the space here, at the one resolution every
            // request-path checkout passes through (20a). The resolver
            // answers *which database*; this says *whose share of it*, so
            // a space queueing slow work cannot hold every connection on
            // the target and cannot saturate it anonymously. A caller that
            // reaches a pool without coming through here is charged to no
            // space and bounded by the pool alone, which is what a
            // boot-time migration and an operator command should be.
            .map(|pool| pool.for_space(&id))
            .map_err(|e| SpaceRefusal::Unroutable {
                space: space.to_string(),
                reason: e.to_string(),
                class: e.class(),
            })
    }

    /// The deployment behind this resolver, for the questions that have no
    /// space in them: which version is live, what the catalog publishes,
    /// what the schema says.
    pub fn deployment(&self) -> &Arc<Deployment> {
        &self.deployment
    }

    /// Resolve the space the door bound.
    ///
    /// Reads the live version once, cuts it to the space on the space's own
    /// pool, and pairs it with the space's chain. The custody check is first
    /// because it is the one that says whether the space exists: a view and
    /// a log for a space that does not exist are two pieces of state minted
    /// from a string a request supplied. Routing is second, and refuses
    /// before anything is planned.
    pub async fn resolve(&self, space: &str) -> Result<Bound, SpaceRefusal> {
        let live = self.deployment.current();
        // Custody is what provisioning writes first, so it is what
        // existence is asked about here. `current_epoch` reads; it never
        // creates, which is what makes a typo unable to mint a tenant.
        //
        // Asked per request rather than cached beside the log, so that a
        // space whose keys were destroyed (`std.space.delete`) stops
        // resolving at once rather than when its first sealed field is
        // touched. That costs one store lookup, which is a map read for
        // today's only `WrappedKeyStore`; a provider whose store is remote
        // is where this wants a short-lived cache, and it should be added
        // there rather than here, because the staleness window is the
        // provider's to state.
        if live
            .exec
            .keys
            .current_epoch(&ordo_crypto::SpaceId(space.to_string()))
            .is_err()
        {
            return Err(self.note(SpaceRefusal::NoCustody(space.to_string())));
        }
        let pool = match self.pool_of(space).await {
            Ok(p) => p,
            Err(e) => return Err(self.note(e)),
        };

        // **The version, before the continuity read** (R-S4). A database
        // whose tables are some other artifact's cannot usefully be asked
        // about its chain, and the cheaper question is the one that
        // decides whether this space serves at all.
        if let Some(behind) = self.schema_behind(space, &pool).await {
            let why = self.behind_reason(space, &behind);
            self.note_behind(space, why.clone());
            return Err(SpaceRefusal::SchemaBehind {
                space: space.to_string(),
                reason: why,
            });
        }

        // **Continuity, on this space's own pool and against the control
        // database's record** (R2d-iii). After routing, because it reads the
        // space's chain and has to read it where the chain is; before
        // `Bound`, because the point is to refuse rather than to serve and
        // complain afterwards.
        //
        // Once per process for a space that passes. A space restored under a
        // running process is caught by the checkpoint worker's next tick for
        // `ORDO_SPACE`, by `ordo verify --control-db` for any space, and at
        // the next boot. A check per request would put two table reads on
        // every call to answer a question whose answer changes only when
        // somebody restores a database.
        if !self.is_continuous(space) {
            match self.continuity_of(space, &pool).await {
                Reconciled::Served(_) => {}
                Reconciled::Quarantined { space, continuity } => {
                    // Already noted by `continuity_of`, the register's
                    // other writer. Routed through `note` anyway so every
                    // failing exit from `resolve` goes through one place:
                    // a refusal added later that forgot to register would
                    // be a tenant failing every request while readiness
                    // reports nothing, which is the defect R-F1 closes.
                    return Err(self.note(SpaceRefusal::Rewound { space, continuity }));
                }
                // "I could not check" is not "it checks out", and it is not
                // a silent restore either: the read failed, and the
                // unreachable-database refusal is the honest name for that.
                // Except a full pool, which is not a fact about this space
                // and is not registered (25z): the request is answered
                // `server_busy` and the next one asks again.
                Reconciled::Unreadable { space, why, class } if class.is_busy() => {
                    return Err(SpaceRefusal::Unroutable {
                        space,
                        reason: why,
                        class,
                    });
                }
                Reconciled::Unreadable { space, why, class } => {
                    return Err(self.note(SpaceRefusal::Unroutable {
                        space,
                        reason: why,
                        class,
                    }));
                }
                // Unreachable from here: `continuity_of` never answers it,
                // because the custody question is asked above this match
                // and again in `reconcile`, and both refuse before a
                // continuity read is attempted. Named rather than wildcarded
                // so that a future verdict added to `Reconciled` has to be
                // decided here instead of silently serving.
                Reconciled::Unprovisioned { space } => {
                    return Err(self.note(SpaceRefusal::NoCustody(space)));
                }
                // Unreachable from here: `schema_behind` is asked above
                // and `continuity_of` never answers it. Named rather than
                // wildcarded so a verdict added later has to be decided
                // here instead of silently serving.
                Reconciled::SchemaBehind { space, at, serving } => {
                    return Err(SpaceRefusal::SchemaBehind {
                        space,
                        reason: format!("schema behind: at `{at}`, serving `{serving}`"),
                    });
                }
            }
        }

        // The resolve succeeded, so whatever held this space closed is
        // over. Lifted here rather than only in `continuity_of`, which is
        // what makes the register self-healing for **every** reason it
        // can hold rather than the rewind alone: a cluster that comes
        // back lifts its own tenants on their next request, with no
        // restart and no operator action (R-F1).
        self.lift_quarantine(space);
        Ok(Bound {
            view: live.in_space(space, pool),
            log: self.log_of(space),
            live,
        })
    }

    /// **Is this space's database at the version this deployment serves?**
    /// (18d R-S4 / R-S6.)
    ///
    /// Asked per space rather than per target, which costs a cached-pool
    /// read and buys the thing the register needs: quarantine is a fact
    /// about a **tenant**, and a target holding four tenants closes four
    /// of them by name rather than one line about a host.
    ///
    /// **A version comparison, not a shape diff.** The shape gate answers
    /// "does this database match the DDL", which is the right question at
    /// boot for the deployment's own database and a heavy one to pose
    /// against every target on every resolve. What R-S4 refuses on is
    /// narrower and already recorded: the target's own `$migrations` row,
    /// which R-S1 made authoritative. A database that says it is at the
    /// serving version and is not is schema drift, which is the boot
    /// gate's question rather than this one's.
    ///
    /// Answers `None` when there is nothing to compare: no active
    /// program, or a target that predates the version row.
    async fn schema_behind(&self, space: &str, pool: &Pool) -> Option<Reconciled> {
        let serving = self.deployment.current().version.clone();
        // **A deployment that cannot name its own version has no basis to
        // call anything behind**, and there are two ways to be in that
        // state rather than the one this first checked for.
        //
        // `Deployment::empty` carries `(none)`: a deployment waiting for
        // its first apply is not a fleet that has drifted. An embedded or
        // pinned deployment can carry the **empty string**, which is the
        // same absence spelled differently, and treating it as a version
        // makes every target with a real recorded version look behind an
        // unnamed one.
        //
        // That second case is not hypothetical: it closed three restore
        // suites the day this landed, because their harness provisions a
        // target from one pinned deployment and resolves through another.
        // The refusal read *"serves ``"*, which is the tell: a comparison
        // against nothing at all.
        if serving.is_empty() || serving == "(none)" {
            return None;
        }
        match ordo_exec::spacedb::recorded_version(pool).await {
            Ok(ordo_exec::spacedb::TargetVersion::At(at)) if at != serving => {
                Some(Reconciled::SchemaBehind {
                    space: space.to_string(),
                    at,
                    serving,
                })
            }
            // At the serving version, or version unknown. Unknown serves:
            // see the variant for why nothing here guesses.
            Ok(_) => None,
            // The question could not be asked. That is the unreachable
            // case, which `pool_of` and `continuity_of` already own, and
            // answering "behind" from a failed read would be inventing a
            // verdict out of an error.
            Err(_) => None,
        }
    }

    /// **Write a failing resolve into the quarantine register, and hand
    /// the refusal back unchanged** (18d R-F1).
    ///
    /// Until T5 the register had one writer, `continuity_of`, which
    /// answers the rewind question alone. So a tenant whose cluster was
    /// down failed every request while `/health/ready` reported nothing,
    /// and an operator's first evidence was a customer. This is one more
    /// writer rather than a second register, so the list stays one list.
    ///
    /// **What stops it growing without bound**, which is the question any
    /// register keyed by a caller-influenced string has to answer.
    /// `resolve` is reached only after the door authorized this caller
    /// **in this space**, and the door's engine is built from `members`
    /// rows, so a name nobody is a member of is refused before it gets
    /// here. The register can therefore only hold spaces the deployment
    /// already knows about. `Spaces::reconcile` makes the same argument
    /// for its own caller, which is the boot.
    fn note(&self, refusal: SpaceRefusal) -> SpaceRefusal {
        let space = match &refusal {
            SpaceRefusal::NoCustody(s) => s.clone(),
            SpaceRefusal::Unroutable { space, .. } => space.clone(),
            SpaceRefusal::Rewound { space, .. } => space.clone(),
            // Written by the site that made it, which knows the reason and
            // that it belongs in the catch-up's list as well.
            SpaceRefusal::SchemaBehind { .. } => return refusal,
        };
        self.note_quarantine(&space, refusal.to_string());
        refusal
    }

    /// **Ask the continuity question about one space, on its own pool,
    /// without binding anything** (17aa).
    ///
    /// This is what the boot runs, and it is the same code [`resolve`]
    /// runs, so a space the boot held closed and a space the first request
    /// finds closed are closed for one reason with one wording. It answers
    /// rather than refuses: whether a verdict stops a process or stops a
    /// space is the caller's to decide, and under 17aa nothing stops the
    /// process.
    ///
    /// [`resolve`]: Spaces::resolve
    pub async fn reconcile(&self, space: &str) -> Reconciled {
        // Custody first, and for [`resolve`]'s reason in the same order:
        // it is what provisioning writes first, so it is what existence is
        // asked about. A space with no custody has no chain to ask the
        // continuity question of, and asking anyway would answer
        // "continuous" about a tenant that does not exist.
        //
        // This is the one place besides [`continuity_of`] that writes the
        // quarantine register, and it is bounded for the same reason the
        // other is: the caller is the boot, walking names the deployment
        // itself holds, never a name that arrived in a request. A register
        // written from request-supplied space names would grow with every
        // probe and would publish those names on `/health/ready`.
        if self
            .deployment
            .current()
            .exec
            .keys
            .current_epoch(&ordo_crypto::SpaceId(space.to_string()))
            .is_err()
        {
            let verdict = Reconciled::Unprovisioned {
                space: space.to_string(),
            };
            self.note_quarantine(space, verdict.why());
            return verdict;
        }
        match self.pool_of(space).await {
            Ok(pool) => {
                if let Some(behind) = self.schema_behind(space, &pool).await {
                    self.note_behind(space, self.behind_reason(space, &behind));
                    return behind;
                }
                self.continuity_of(space, &pool).await
            }
            Err(e) => Reconciled::Unreadable {
                space: space.to_string(),
                why: e.to_string(),
                class: e.class(),
            },
        }
    }

    /// The one place the continuity question is answered.
    ///
    /// One of the two places the quarantine register is written; the
    /// other is [`Spaces::reconcile`], for the custody question it asks
    /// before this one is reachable (17ac).
    async fn continuity_of(&self, space: &str, pool: &Pool) -> Reconciled {
        let id = ordo_commandlog::SpaceId(space.to_string());
        self.checks.fetch_add(1, Ordering::Relaxed);
        let verdict = match undo::continuity(Db::Pool(pool), &self.witness, &id).await {
            Ok(c @ Continuity::Rewound { .. }) => Reconciled::Quarantined {
                space: space.to_string(),
                continuity: c,
            },
            Ok(c) => Reconciled::Served(c),
            Err(e) => Reconciled::Unreadable {
                space: space.to_string(),
                why: e.to_string(),
                class: e.class(),
            },
        };
        if verdict.serves() {
            self.mark_continuous(space);
            self.lift_quarantine(space);
        } else if !matches!(&verdict, Reconciled::Unreadable { class, .. } if class.is_busy()) {
            // A full pool is not a fact about this space (25z). The check
            // did not run, the next request asks it again, and holding the
            // tenant closed in `/health/ready` for a busy moment would
            // report an outage that is not one.
            self.note_quarantine(space, verdict.why());
        }
        verdict
    }

    /// Which spaces this deployment is holding closed, and why. Ordered by
    /// space id. Read by `/health/ready`; see the field's own note for why
    /// nothing refuses from it.
    pub fn quarantined(&self) -> Vec<(String, String)> {
        self.quarantined
            .lock()
            .expect("quarantine register is not poisoned")
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn note_quarantine(&self, space: &str, why: String) {
        // A space held for another reason is not the catch-up's to work on.
        self.behind
            .lock()
            .expect("behind register is not poisoned")
            .remove(space);
        self.quarantined
            .lock()
            .expect("quarantine register is not poisoned")
            .insert(space.to_string(), why);
    }

    /// Hold a space closed because its database is behind, and tell the
    /// catch-up.
    fn note_behind(&self, space: &str, why: String) {
        self.note_quarantine(space, why);
        self.behind
            .lock()
            .expect("behind register is not poisoned")
            .insert(space.to_string());
    }

    fn lift_quarantine(&self, space: &str) {
        self.behind
            .lock()
            .expect("behind register is not poisoned")
            .remove(space);
        self.refusals
            .lock()
            .expect("refusal register is not poisoned")
            .remove(space);
        self.quarantined
            .lock()
            .expect("quarantine register is not poisoned")
            .remove(space);
    }

    /// The reason to publish for a space that is behind: the comparison's
    /// own words, and the change that was refused when bringing it forward
    /// was tried for the version now serving.
    fn behind_reason(&self, space: &str, behind: &Reconciled) -> String {
        let base = behind.why();
        let serving = self.deployment.current().version.clone();
        match self
            .refusals
            .lock()
            .expect("refusal register is not poisoned")
            .get(space)
        {
            Some((version, why)) if *version == serving => {
                format!("{base}. Bringing it forward was refused: {why}")
            }
            _ => base,
        }
    }

    /// The spaces held closed because their database is behind the version
    /// being served. What the catch-up works from.
    pub fn awaiting_schema(&self) -> Vec<String> {
        self.behind
            .lock()
            .expect("behind register is not poisoned")
            .iter()
            .cloned()
            .collect()
    }

    /// Record that bringing this space's database to `version` was refused,
    /// and why. The reason joins the one `/health/ready` publishes.
    pub fn note_refused(&self, space: &str, version: &str, why: &str) {
        self.refusals
            .lock()
            .expect("refusal register is not poisoned")
            .insert(space.to_string(), (version.to_string(), why.to_string()));
        let mut held = self
            .quarantined
            .lock()
            .expect("quarantine register is not poisoned");
        if let Some(reason) = held.get_mut(space) {
            if !reason.contains("Bringing it forward was refused") {
                reason.push_str(&format!(". Bringing it forward was refused: {why}"));
            }
        }
    }

    /// The refusal already recorded for this space at this version, so the
    /// catch-up does not ask the database the same question again.
    pub fn refused_at(&self, space: &str, version: &str) -> Option<String> {
        self.refusals
            .lock()
            .expect("refusal register is not poisoned")
            .get(space)
            .filter(|(v, _)| v == version)
            .map(|(_, why)| why.clone())
    }

    fn is_continuous(&self, space: &str) -> bool {
        self.continuous
            .lock()
            .expect("continuity registry is not poisoned")
            .contains(space)
    }

    fn mark_continuous(&self, space: &str) {
        self.continuous
            .lock()
            .expect("continuity registry is not poisoned")
            .insert(space.to_string());
    }

    /// This space's write log, minted on first use.
    ///
    /// Public so that a caller holding the resolver can read the chain a
    /// request appended to, which is what makes "each tenant writes to its
    /// own chain" an assertable claim rather than an arrangement of the
    /// wiring. It hands back the same `Arc` every request for the space
    /// uses, deliberately: a second log for one space is a second chain,
    /// and handing out a fresh one here is the way that would happen.
    pub fn log_of(&self, space: &str) -> Arc<Mutex<WriteLog>> {
        self.logs
            .lock()
            .expect("write-log registry is not poisoned")
            .entry(space.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(serving_log())))
            .clone()
    }
}

/// A write log for a deployment rather than a suite.
///
/// `WriteLog::new` mints ids and times from one counter so that a suite
/// replays exactly. A deployment restarts, and a counter that restarts mints
/// correlations the durable chain already holds and row ids a table already
/// holds. So each log draws a random prefix once, and stamps commands with
/// the wall clock in seconds, which is the unit the undo horizon is compared
/// in. Sixty-four bits per log, and a log is one space in one process.
fn serving_log() -> WriteLog {
    let mut bytes = [0u8; 8];
    ordo_crypto::fill_random(&mut bytes);
    let mut prefix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    prefix.push('_');
    WriteLog::serving(
        prefix,
        Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }),
    )
}
