//! Which database a space's rows are in: **the pool resolver**.
//!
//! The seam `plan-space-routing.md` §1 designs, in the shape
//! `ordo_blob::StoreResolver` already set for bytes: one trait asked per
//! space. The production server uses [`AttachedPools`], which reads the
//! per-space attachment; [`SinglePool`] answers the same pool for every
//! space and is for tests and single-database tools only.
//!
//! # Resolved above the executor, never inside it
//!
//! [`crate::Executor`] carries a pool and a space as fields and is handed
//! both. The resolver runs where a request is bound to a space (the host's
//! `Spaces::resolve`), so the write protocol, the revert applier and the
//! chain append never ask which database they are in. That is what lets
//! one code path serve a space on the shared database and a space on its
//! own.
//!
//! # A handle rather than an `Arc` of one
//!
//! `resolve` yields a [`Pool`] where the plan sketched `Arc<Pool>`, because a
//! `Pool` already is a shared handle: every clone holds the same connections
//! and the same bound. What a caller compares is the handle, through
//! [`Pool::same_handle`], and that is how a cached per-space executor learns
//! its space was moved: the resolver hands back a different handle, and the
//! executor is cut again rather than kept.
//!
//! # Asynchronous, because the answer can be in a database
//!
//! [`AttachedPools`] reads `$spacedb` and dials target pools, both of which
//! wait on the network. The trait returns a boxed future ([`Resolving`])
//! rather than using `async fn`, because it is used as `dyn` and a native
//! `async fn` in a trait cannot be.
//!
//! # What this module does not decide
//!
//! Which platform tables a target owes (S4), how a space comes to have a
//! target (S5), and how the loops that walk every space learn about targets
//! (S6).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ordo_crypto::SpaceId;

use crate::spacedb::{self, DbAttachment};
use crate::{Pool, PoolConfig, PoolError};

/// What [`PoolResolver::resolve`] returns.
pub type Resolving<'a> = Pin<Box<dyn Future<Output = Result<Pool, PoolError>> + Send + 'a>>;

/// One database the deployment's spaces live in, as
/// [`PoolResolver::targets`] reports it.
/// The label meaning **this deployment's own credential** (18d
/// `db-always-a-database`).
///
/// Every tenant has a database, and a deployment that configures nothing
/// puts them on its own cluster. Something has to say which credential
/// dials them, and it is the deployment's own — the one `ORDO_PG_PASSWORD`
/// or `ORDO_PG_PASSWORD_FILE` already holds.
///
/// **A reserved label rather than an empty string**, because
/// `DbAttachment::credential` is a column an operator reads and `""` there
/// would look like a row somebody failed to fill in. `$` prefixes every
/// platform-owned name in this tree (`$spacedb`, `$space`, `$artifacts`),
/// so it is unmistakable and cannot collide with a label an operator
/// installed, which `Installed` and the credential directory both key on
/// ordinary names.
pub const OWN_CREDENTIAL: &str = "$deployment";

/// A credential provider that additionally answers [`OWN_CREDENTIAL`].
///
/// Wraps whatever the operator configured and adds one label, so a
/// deployment with no credential directory at all can still give every
/// tenant a database on its own cluster. Everything else delegates, and a
/// label the inner provider does not hold is refused there exactly as
/// before — this widens the vocabulary by one name rather than loosening
/// the check.
pub struct WithOwnCredential {
    inner: Arc<dyn DbCredentials>,
    own: Option<String>,
}

impl WithOwnCredential {
    /// `own` is the deployment's own password, or `None` where it
    /// authenticates without one (a local socket, peer auth, an IAM
    /// sidecar) — which is legitimate and is why the type is `Option`
    /// rather than a string that might be empty.
    pub fn wrapping(inner: Arc<dyn DbCredentials>, own: Option<String>) -> Arc<dyn DbCredentials> {
        Arc::new(WithOwnCredential { inner, own })
    }
}

impl DbCredentials for WithOwnCredential {
    fn password(&self, label: &str) -> Result<Option<String>, String> {
        if label == OWN_CREDENTIAL {
            // The deployment's own password, when it holds one, so an owner
            // states the database password once. A deployment that holds
            // none falls back to a `$deployment` entry in the credential
            // directory when there is one (an owner who keeps the password
            // only there), and is passwordless when there is not.
            if self.own.is_some() {
                return Ok(self.own.clone());
            }
            return Ok(self.inner.password(label).ok().flatten());
        }
        self.inner.password(label)
    }
}

/// How a target is named on every operator-facing surface: `default`, or
/// `host:port/db`.
///
/// One function rather than a `format!` at each site, because
/// `ordo_exec::fleet` keys its rows on this string (18d R-S2). Two spellings
/// of one database would be two rows in the fleet view describing the same
/// target, and whichever the operator read first would look authoritative.
/// **Never a credential**, which is what keeps this printable.
pub fn target_label(host: &str, port: u16, db: &str) -> String {
    format!("{host}:{port}/{db}")
}

/// The label for the deployment's own database.
pub const DEFAULT_LABEL: &str = "default";

pub struct Target {
    /// Where it is, for a log line or an operator: `default`, or
    /// `host:port/db`. Never a credential.
    pub label: String,
    /// Its pool, or why it could not be reached.
    pub pool: Result<Pool, String>,
}

/// What [`PoolResolver::targets`] returns.
pub type Targets<'a> = Pin<Box<dyn Future<Output = Result<Vec<Target>, PoolError>> + Send + 'a>>;

/// Which pool a space's rows are reached through.
pub trait PoolResolver: Send + Sync {
    /// The pool for `space`.
    ///
    /// Fallible although the default never fails, because an attached
    /// target can be unreachable, and the place to learn that is here,
    /// before a request has planned anything, rather than partway through a
    /// write.
    ///
    /// Routing is not existence. This answers for a space nothing has
    /// provisioned exactly as it answers for any other; whether the
    /// deployment serves the space is custody's question, asked separately
    /// and first.
    fn resolve<'a>(&'a self, space: &'a SpaceId) -> Resolving<'a>;

    /// Every database this resolver can route a space to, each once.
    ///
    /// For the work that walks every database rather than one space's:
    /// security-stream retention now, and the supervisor's blob collection
    /// and job claiming when it exists. A database that cannot be reached is
    /// listed with the reason rather than left out, because a loop that
    /// skipped it silently would report a clean pass over a database it
    /// never visited. `Err` is for failing to produce the list at all.
    fn targets<'a>(&'a self) -> Targets<'a>;

    /// How many databases a walker may work on at once
    /// ([`TENANT_DIAL_CONCURRENCY_ENV`]). The tenant catch-up runs this many
    /// databases forward together.
    fn walk_concurrency(&self) -> usize {
        tenant_dial_concurrency()
    }

    /// The deployment's own database, without opening any other.
    ///
    /// [`PoolResolver::targets`] dials every attached database to hand back
    /// its pool, which is the wrong price for a caller that wants only the
    /// first. The default reads the first target.
    fn default_pool<'a>(&'a self) -> Resolving<'a> {
        Box::pin(async move {
            match self.targets().await?.into_iter().next() {
                Some(t) => t.pool.map_err(PoolError::db),
                None => Err(PoolError::Db("the resolver lists no database".into())),
            }
        })
    }
}

/// One pool for every space.
///
/// **Not a deployment's choice.** Every tenant has a database of its own
/// (ruled 2026-09-25, 2026-10-06), so the production server always resolves
/// through [`AttachedPools`]. This stays for what reaches exactly one
/// database by construction: the conformance harness, tests that stand up
/// one database, and the operator tool's direct `apply`. It hands back the
/// same handle for every space, including one no request has named before.
pub struct SinglePool(Pool);

impl SinglePool {
    pub fn new(pool: Pool) -> Self {
        SinglePool(pool)
    }
}

impl PoolResolver for SinglePool {
    fn resolve<'a>(&'a self, _space: &'a SpaceId) -> Resolving<'a> {
        let pool = self.0.clone();
        Box::pin(async move { Ok(pool) })
    }

    fn targets<'a>(&'a self) -> Targets<'a> {
        let pool = self.0.clone();
        Box::pin(async move {
            Ok(vec![Target {
                label: "default".to_string(),
                pool: Ok(pool),
            }])
        })
    }
}

/// Where an attachment's credential label becomes something a dial can use.
///
/// A `$spacedb` row names a label and never a secret (see
/// [`crate::spacedb`]); this is the other half, installed on the node out of
/// band. A label this node does not hold is refused before any dial rather
/// than tried without a password, because a dial that happened to succeed
/// unauthenticated would reach a database in a way nobody chose.
pub trait DbCredentials: Send + Sync {
    /// `Ok(Some(password))`; `Ok(None)` for a label installed with no
    /// password (peer, trust or certificate authentication); `Err` for a
    /// label this node does not hold.
    fn password(&self, label: &str) -> Result<Option<String>, String>;
}

/// Credentials installed as a map, from whatever configuration the
/// deployment reads.
#[derive(Default)]
pub struct Installed(HashMap<String, Option<String>>);

impl Installed {
    pub fn new() -> Self {
        Self::default()
    }

    /// A label that authenticates without a password.
    pub fn passwordless(mut self, label: impl Into<String>) -> Self {
        self.0.insert(label.into(), None);
        self
    }

    pub fn with_password(mut self, label: impl Into<String>, password: impl Into<String>) -> Self {
        self.0.insert(label.into(), Some(password.into()));
        self
    }

    /// A label that authenticates the way [`crate::PoolConfig::from_env`]
    /// does: with `ORDO_PG_PASSWORD` when it is set, without a password when
    /// it is not. For databases on the same server as the deployment's own,
    /// such as a test's attached targets, which a local socket admits
    /// without a password and a CI server over TCP does not.
    pub fn as_configured(self, label: impl Into<String>) -> Self {
        match crate::PoolConfig::from_env().password {
            Some(password) => self.with_password(label, password),
            None => self.passwordless(label),
        }
    }
}

impl DbCredentials for Installed {
    fn password(&self, label: &str) -> Result<Option<String>, String> {
        self.0.get(label).cloned().ok_or_else(|| {
            format!("no database credential is installed under `{label}` on this node")
        })
    }
}

/// The credential provider of a deployment that configured none.
///
/// Every label is refused, and the refusal names the setting that installs
/// one. Wrapped in [`WithOwnCredential`] it still answers
/// [`OWN_CREDENTIAL`], which is all a deployment that puts its tenants on
/// its own cluster needs.
pub struct Unconfigured;

impl DbCredentials for Unconfigured {
    fn password(&self, label: &str) -> Result<Option<String>, String> {
        Err(format!(
            "no credential provider is configured, so nothing answers the label `{label}`: \
             set ORDO_TENANT_CREDENTIALS=dir and ORDO_TENANT_CREDENTIALS_DIR"
        ))
    }
}

/// Resolution through `$spacedb`: each attached space on its target's pool,
/// every other space on the default.
///
/// # Pools are per target, not per space
///
/// A pool is keyed by [`DbAttachment::identity`], so a hundred spaces
/// attached to one database share one pool. That is the property the sizing
/// in `plan-space-routing.md` rests on: every per-database cost scales with
/// the number of distinct targets, and so does this.
///
/// # What is cached, and for how long
///
/// **An attachment is cached for the life of the process.** That is sound
/// because an attachment does not change: `spacedb::attach` refuses a
/// different target for an attached space, so the answer a node cached is
/// the answer that stands. When the live move is built, this cache is one of
/// the things it has to reach.
///
/// **A space with no row is cached for `miss_ttl`** (30 seconds by default).
/// That is almost every space, and asking the control database for every one
/// of them on every request would put a round trip in front of every request
/// in the deployment. The window it opens is a space attached *after* a node
/// cached its miss, which that node would keep serving from the default
/// database until the miss expired. What closes the window is ordering
/// rather than the TTL: a space's attachment is written before its founding
/// member, so no request can be authorized in a space before its answer here
/// is final. S5 builds provisioning in that order and has to keep it. The
/// TTL bounds the damage if anything ever attaches out of order, and
/// [`AttachedPools::forget`] lets the node that attached drop its own miss at
/// once.
///
/// # Failure is at resolve
///
/// A target's pool is dialled on first use, every connection eagerly
/// (`Pool::connect`), under the checkout's own timeout. A target that cannot
/// be reached fails here, before the request has planned anything, and the
/// failure is not cached: the next request dials again.
pub struct AttachedPools {
    control: Pool,
    default: Pool,
    template: PoolConfig,
    credentials: Arc<dyn DbCredentials>,
    miss_ttl: Duration,
    routes: Mutex<HashMap<String, Route>>,
    targets: Mutex<HashMap<String, Arc<tokio::sync::OnceCell<Pool>>>>,
    /// **How long to wait before dialling a target that just failed**
    /// (18d R-F3), keyed by target identity.
    ///
    /// A failed dial deliberately leaves [`Self::targets`]' cell empty so
    /// the next request tries again, which is right for recovery and
    /// wrong for a cluster that is simply down: every request, for every
    /// tenant on that target, pays a fresh dial and waits out the
    /// checkout timeout. With a handful of tenants on one downed cluster
    /// that is the deployment spending its connection budget on a
    /// database that is not there.
    ///
    /// So a failure arms a backoff and a success disarms it. The cell
    /// stays the mechanism for *sharing* a live pool; this is the
    /// mechanism for *not* re-dialling a dead one. Keyed by target rather
    /// than by space, because the thing that is down is the target and
    /// every tenant on it should wait behind one timer rather than each
    /// starting its own.
    backoff: Mutex<HashMap<String, Backoff>>,
    /// How many times this node has actually attempted a dial.
    ///
    /// The observable R-F3 is about: a down target must cost a dial per
    /// backoff interval rather than a dial per request, and "how many
    /// requests failed" cannot tell those apart. `target_count` counts
    /// pools that exist, which a failing target never has, so it answers
    /// zero either way.
    dials: std::sync::atomic::AtomicUsize,
    /// **How many tenant databases the walkers of this node dial at once**
    /// ([`TENANT_DIAL_CONCURRENCY_ENV`]). [`PoolResolver::targets`] and the
    /// handles it returns take a permit before every dial, so a boot pass,
    /// a sweep after new tenants or a deploy opens a few connections at a
    /// time and none stays open after its tenant is done, instead of one
    /// per tenant together.
    dial_gate: Arc<tokio::sync::Semaphore>,
    /// The size `dial_gate` was made with, kept so a walker that does its
    /// own work per database can run that many at once and no more.
    dial_cap: usize,
    /// How long a walker waits on `dial_gate`
    /// ([`TENANT_DIAL_WAIT_MS_ENV`]) before its step fails.
    dial_wait: Duration,
}

/// How long a walker waits for a free dial slot before its step for that
/// tenant fails.
pub const TENANT_DIAL_WAIT_MS_ENV: &str = "ORDO_PG_TENANT_DIAL_WAIT_MS";
/// The default for [`TENANT_DIAL_WAIT_MS_ENV`]: 60 seconds.
pub const TENANT_DIAL_WAIT_DEFAULT: Duration = Duration::from_millis(60_000);
/// The most [`TENANT_DIAL_WAIT_MS_ENV`] accepts: one hour.
pub const TENANT_DIAL_WAIT_MAX_MS: u64 = 3_600_000;

/// The setting, clamped to its range. `ordo_host::runtime` refuses an
/// out-of-range value at boot naming the range and default; a library
/// caller gets the nearest allowed one.
pub fn tenant_dial_wait() -> Duration {
    std::env::var(TENANT_DIAL_WAIT_MS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(TENANT_DIAL_WAIT_DEFAULT, |n| {
            Duration::from_millis(n.clamp(1, TENANT_DIAL_WAIT_MAX_MS))
        })
}

/// Take a permit of the dial gate, or give up after `wait`.
pub(crate) async fn wait_for_dial_gate(
    gate: &Arc<tokio::sync::Semaphore>,
    wait: Duration,
) -> Result<tokio::sync::OwnedSemaphorePermit, PoolError> {
    match tokio::time::timeout(wait, gate.clone().acquire_owned()).await {
        Ok(p) => Ok(p.expect("dial gate never closes")),
        Err(_) => Err(PoolError::DialWaitExceeded(wait.as_millis() as u64)),
    }
}

/// How many tenant databases the background walkers dial at once.
pub const TENANT_DIAL_CONCURRENCY_ENV: &str = "ORDO_PG_TENANT_DIAL_CONCURRENCY";
/// The default for [`TENANT_DIAL_CONCURRENCY_ENV`].
pub const TENANT_DIAL_CONCURRENCY_DEFAULT: usize = 8;
/// The most [`TENANT_DIAL_CONCURRENCY_ENV`] accepts.
pub const TENANT_DIAL_CONCURRENCY_MAX: usize = 64;

/// The setting, clamped to its range. `ordo_host::runtime` refuses an
/// out-of-range value at boot naming the range; a library caller gets the
/// nearest allowed one.
pub fn tenant_dial_concurrency() -> usize {
    std::env::var(TENANT_DIAL_CONCURRENCY_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(TENANT_DIAL_CONCURRENCY_DEFAULT, |n| {
            n.clamp(1, TENANT_DIAL_CONCURRENCY_MAX)
        })
}

/// One target's failure state. See [`AttachedPools::backoff`].
struct Backoff {
    /// Refuse without dialling until this instant.
    until: Instant,
    /// How many dials in a row have failed. The delay doubles with it, so
    /// a target that is down for an hour is dialled a few dozen times
    /// rather than a few million.
    consecutive: u32,
    /// The last dial's own words, kept so the refusal an operator reads
    /// says what actually went wrong rather than only that we are
    /// waiting. This never reaches a caller: `ordo_host`'s
    /// `space_refusal` replaces it with an opaque sentence, per §9.1
    /// R-C4.4.
    last: String,
}

/// The first wait after a failure.
///
/// Short enough that a target which blipped is back in service almost
/// immediately, which is the case that matters most: a rolling restart of
/// a Postgres cluster is seconds, and a tenant should not be refused for
/// a minute over it.
const BACKOFF_FIRST: Duration = Duration::from_millis(250);

/// The longest wait, however long a target stays down.
///
/// A cap rather than unbounded doubling, because the point is to stop
/// spending dials, not to punish a target for a long outage. Thirty
/// seconds means a cluster that has been down for an hour costs about 120
/// dials rather than one per request, and a tenant whose cluster comes
/// back waits at most this long to find out.
const BACKOFF_MAX: Duration = Duration::from_secs(30);

#[derive(Clone)]
enum Route {
    Attached(DbAttachment),
    Default { until: Instant },
}

impl AttachedPools {
    pub const MISS_TTL: Duration = Duration::from_secs(30);

    /// `control` is where `$spacedb` is read. `default` serves every space
    /// with no row; in a deployment that keeps them together
    /// (`plan-space-routing.md` Q-1) it is the same pool. `template`
    /// supplies what an attachment does not say about a target's pool: its
    /// size and its checkout timeout.
    pub fn new(
        control: Pool,
        default: Pool,
        template: PoolConfig,
        credentials: Arc<dyn DbCredentials>,
    ) -> Self {
        AttachedPools {
            control,
            default,
            template,
            credentials,
            miss_ttl: AttachedPools::MISS_TTL,
            routes: Mutex::new(HashMap::new()),
            targets: Mutex::new(HashMap::new()),
            backoff: Mutex::new(HashMap::new()),
            dials: std::sync::atomic::AtomicUsize::new(0),
            dial_gate: Arc::new(tokio::sync::Semaphore::new(tenant_dial_concurrency())),
            dial_cap: tenant_dial_concurrency(),
            dial_wait: tenant_dial_wait(),
        }
    }

    /// Dial at most `n` tenant databases at once when walking them, in
    /// place of [`TENANT_DIAL_CONCURRENCY_ENV`].
    pub fn with_dial_concurrency(mut self, n: usize) -> Self {
        self.dial_gate = Arc::new(tokio::sync::Semaphore::new(n.max(1)));
        self.dial_cap = n.max(1);
        self
    }

    /// Give up on the dial gate after `wait`, in place of
    /// [`TENANT_DIAL_WAIT_MS_ENV`].
    pub fn with_dial_wait(mut self, wait: Duration) -> Self {
        self.dial_wait = wait;
        self
    }

    /// How long a space with no row is remembered as having none.
    pub fn with_miss_ttl(mut self, ttl: Duration) -> Self {
        self.miss_ttl = ttl;
        self
    }

    /// Drop what this node remembers about `space`, so the next resolve
    /// reads `$spacedb` again. For the node that has just attached it.
    pub fn forget(&self, space: &SpaceId) {
        self.routes
            .lock()
            .expect("route cache is not poisoned")
            .remove(&space.0);
    }

    /// How many dials this node has attempted, successful or not. See the
    /// field: this is what makes a backoff observable, because a target
    /// that never connects has no pool to count.
    pub fn dials(&self) -> usize {
        self.dials.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many target pools this node has dialled. Distinct targets, not
    /// spaces: the number the sizing is about.
    pub fn target_count(&self) -> usize {
        self.targets
            .lock()
            .expect("target pools are not poisoned")
            .values()
            .filter(|cell| cell.initialized())
            .count()
    }

    async fn route(&self, space: &SpaceId) -> Result<Option<DbAttachment>, PoolError> {
        let cached = self
            .routes
            .lock()
            .expect("route cache is not poisoned")
            .get(&space.0)
            .cloned();
        match cached {
            Some(Route::Attached(a)) => return Ok(Some(a)),
            Some(Route::Default { until }) if Instant::now() < until => return Ok(None),
            _ => {}
        }
        let found = spacedb::lookup(&self.control, space).await?;
        let route = match &found {
            Some(a) => Route::Attached(a.clone()),
            None => Route::Default {
                until: Instant::now() + self.miss_ttl,
            },
        };
        self.routes
            .lock()
            .expect("route cache is not poisoned")
            .insert(space.0.clone(), route);
        Ok(found)
    }

    async fn pool_for(&self, target: &DbAttachment) -> Result<Pool, PoolError> {
        self.pool_for_as(target, false).await
    }

    /// [`Self::pool_for`], for a walker when `sweep` is set: a dial waits
    /// for the node's gate and gives its first connection back at once, and
    /// the handle returned closes its connections after each checkout. A
    /// pool that already exists is shared as it is.
    async fn pool_for_as(&self, target: &DbAttachment, sweep: bool) -> Result<Pool, PoolError> {
        let cell = self
            .targets
            .lock()
            .expect("target pools are not poisoned")
            .entry(target.identity())
            .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
            .clone();
        // **Refuse without dialling while this target is in backoff**
        // (R-F3). Checked before the cell rather than inside it, because
        // the cell's initializer only runs when there is no pool, and
        // what has to be suppressed is the *dial*, not the sharing.
        let id = target.identity();
        if let Some(wait) = self.backing_off(&id) {
            return Err(wait);
        }
        // One dial per target even when many requests arrive for it at
        // once: the cell runs one initializer and the rest wait on it. A
        // failed dial leaves the cell empty, so the next request tries again
        // once the backoff above has elapsed.
        let dialled = cell
            .get_or_try_init(|| async {
                let _gate = if sweep {
                    Some(wait_for_dial_gate(&self.dial_gate, self.dial_wait).await?)
                } else {
                    None
                };
                self.dials
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let pool = Pool::connect(target_config(
                    target,
                    self.credentials.as_ref(),
                    &self.template,
                )?)
                .await?;
                if sweep {
                    pool.close_idle().await;
                }
                Ok(pool)
            })
            .await
            .cloned()
            .map(|p| {
                if sweep {
                    p.sweeping_for(self.dial_gate.clone(), self.dial_wait)
                } else {
                    p
                }
            });
        match &dialled {
            // A target that answers is a target with no history worth
            // keeping. Cleared rather than decayed, because a single
            // success means the thing is up, and holding a grudge would
            // refuse the next request after a recovery.
            Ok(_) => self.dial_succeeded(&id),
            // Giving up on the gate says nothing about the target, so it
            // earns no backoff.
            Err(PoolError::DialWaitExceeded(_)) => {}
            Err(e) => self.dial_failed(&id, e),
        }
        dialled
    }

    /// The refusal to answer with while a target is in backoff, or `None`
    /// when it is time to try again.
    fn backing_off(&self, id: &str) -> Option<PoolError> {
        let held = self.backoff.lock().expect("backoff is not poisoned");
        let state = held.get(id)?;
        if Instant::now() < state.until {
            return Some(PoolError::db(format!(
                "target `{id}` is not being dialled: {} consecutive failures, last was {}",
                state.consecutive, state.last
            )));
        }
        None
    }

    fn dial_succeeded(&self, id: &str) {
        self.backoff
            .lock()
            .expect("backoff is not poisoned")
            .remove(id);
    }

    fn dial_failed(&self, id: &str, e: &PoolError) {
        let mut held = self.backoff.lock().expect("backoff is not poisoned");
        let entry = held.entry(id.to_string()).or_insert(Backoff {
            until: Instant::now(),
            consecutive: 0,
            last: String::new(),
        });
        entry.consecutive = entry.consecutive.saturating_add(1);
        entry.last = e.to_string();
        // Doubling, capped. `checked_mul` rather than a shift so a long
        // outage saturates at the cap instead of wrapping to nothing,
        // which would turn the longest outage into no backoff at all.
        let delay = BACKOFF_FIRST
            .checked_mul(1u32 << entry.consecutive.saturating_sub(1).min(16))
            .unwrap_or(BACKOFF_MAX)
            .min(BACKOFF_MAX);
        entry.until = Instant::now() + delay;
    }
}

/// The pool configuration for an attached target: where the attachment
/// says, authenticated as its credential label says, and otherwise as
/// `template` says (its size and checkout timeout).
///
/// One function for every caller that dials a target, so the resolver and
/// provisioning cannot come to disagree about how a target is reached.
pub fn target_config(
    target: &DbAttachment,
    credentials: &dyn DbCredentials,
    template: &PoolConfig,
) -> Result<PoolConfig, PoolError> {
    Ok(PoolConfig {
        host: target.host.clone(),
        port: target.port,
        user: target.user.clone(),
        db: target.db.clone(),
        password: credentials
            .password(&target.credential)
            .map_err(PoolError::db)?,
        ..template.clone()
    })
}

impl PoolResolver for AttachedPools {
    fn walk_concurrency(&self) -> usize {
        self.dial_cap
    }

    fn default_pool<'a>(&'a self) -> Resolving<'a> {
        Box::pin(async move { Ok(self.default.clone()) })
    }

    fn resolve<'a>(&'a self, space: &'a SpaceId) -> Resolving<'a> {
        Box::pin(async move {
            match self.route(space).await? {
                Some(target) => self.pool_for(&target).await,
                None => Ok(self.default.clone()),
            }
        })
    }

    /// The default, then each distinct attached target in the order its
    /// first space sorts. Every target is dialled (or its cached pool
    /// reused), so a caller walking the list gets pools it can use, and a
    /// target that will not answer is reported, not skipped.
    ///
    /// **A walk holds no connection per tenant.** The dials wait on
    /// [`TENANT_DIAL_CONCURRENCY_ENV`] and give their connection back at
    /// once, and the pools returned are sweeping handles
    /// ([`Pool::sweeping`]): each checkout dials, uses and closes. So 121
    /// tenants cost a few connections at a time, never 121 together.
    fn targets<'a>(&'a self) -> Targets<'a> {
        Box::pin(async move {
            spacedb::ensure_table(&self.control).await?;
            let mut out = vec![Target {
                label: "default".to_string(),
                pool: Ok(self.default.clone()),
            }];
            let mut seen: Vec<String> = Vec::new();
            for (_, a) in spacedb::all(&self.control).await? {
                if seen.contains(&a.identity()) {
                    continue;
                }
                seen.push(a.identity());
                out.push(Target {
                    label: target_label(&a.host, a.port, &a.db),
                    pool: self.pool_for_as(&a, true).await.map_err(|e| e.to_string()),
                });
            }
            Ok(out)
        })
    }
}
