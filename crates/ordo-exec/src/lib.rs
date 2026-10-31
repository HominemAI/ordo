//! F5's first real piece: the connection pool and the transaction seam.
//!
//! `tokio-postgres` deliberately ships no pool, so this one is explicit
//! rather than automanaged, and every sizing decision is written down:
//!
//! - **Fixed size, chosen once** ([`PoolConfig::size`], default
//!   `min(available_parallelism, 8)`, floor 2). A Postgres backend is a
//!   process; more connections than cores buys queueing inside the server
//!   instead of in front of it, where we can see it. The pool IS the
//!   platform's DB-side concurrency bound — the API layer above inherits
//!   it naturally, because a request cannot execute without a checkout.
//! - **Bounded acquire** ([`PoolConfig::acquire_timeout`], default 2s):
//!   waiters queue FIFO on a semaphore and time out into a typed error.
//!   Backpressure surfaces as a fast failure the door can convert to a
//!   retryable response — never an unbounded internal queue.
//! - **Connections on demand** ([`PoolLimits::idle_timeout`], default 30s):
//!   [`Pool::connect`] dials one connection, so a misconfigured database
//!   fails at boot, and the rest dial when a checkout finds none idle. A
//!   connection nobody has used for the idle period is closed, so a pool
//!   that is not being used holds nothing. `size` is the ceiling, never a
//!   minimum. This is what lets a deployment with a database per tenant
//!   hold connections in proportion to the tenants in use, not the tenants
//!   it has.
//! - **Health at checkout**: a connection found closed is replaced then,
//!   transparently; the async socket driver for each connection runs on
//!   its own spawned task (tokio's runtime threads are the pool's threads
//!   — no extra thread pool exists to tune).
//! - **Transactions own their connection**: [`Pool::begin`] checks one
//!   out for the duration. Committing returns it; **dropping an
//!   uncommitted [`TxConn`] discards the connection entirely**, which
//!   aborts the transaction server-side — the one rollback path that
//!   cannot be forgotten, misordered, or half-run. The pool re-dials on a
//!   later checkout.
//!
//! Everything speaks the **simple (text) protocol**: the compiler binds
//! literals, plans carry no free parameters, and text rows decode by the
//! plan's declared output types — the same discipline the psql harness
//! proved, now over a real driver.

pub mod blob;
pub mod blobgc;
pub mod bridge;
pub mod bucket;
pub mod cadence;
pub mod commandlog;
pub mod creds;
pub mod dburl;
pub mod erase;
pub mod export;
pub mod fleet;
pub mod holders;
pub mod import;
pub mod incident;
pub mod jobs;
pub mod keystore;
pub mod metrics;
pub mod migrate;
pub mod nfc;
pub mod nodes;
mod pool_error;
pub mod prune;
pub mod registry;
pub mod reseal;
pub mod route;
pub mod sealed;
pub mod secevent;
pub mod shape;
pub mod spacedb;
pub mod triggers;
pub mod undo;
pub mod usage;
pub mod webhook;
pub mod witness;

pub use pool_error::{
    invalid_input_text, Cause, DbDetail, FaultClass, PoolError, SATURATED_MESSAGE,
};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_postgres::{Client, NoTls, SimpleQueryMessage};

/// One row of a text-protocol result: column values in output order,
/// `None` for SQL NULL.
pub type TextRow = Vec<Option<String>>;

#[derive(Clone)]
pub struct PoolConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub db: String,
    /// The password, when the server asks for one.
    ///
    /// **`tokio-postgres` reads no environment at all** — there is no
    /// `env::var` anywhere in its `Config` — so `PGPASSWORD` reaches
    /// `psql`-shaped clients and reaches this pool never. Until this field
    /// existed, any Postgres that asked for a password was a Postgres this
    /// platform could not dial, and it did not show locally because the
    /// conformance cluster runs on a Unix socket with `trust`.
    ///
    /// `None` is legitimate — peer auth, a socket, an IAM sidecar — so it
    /// cannot be a refusal. See `ORDO_PG_PASSWORD` in
    /// `ordo_host::runtime` for the posture and for why a connect failure
    /// that names authentication prints the variable's name.
    pub password: Option<String>,
    /// The DB-side concurrency bound. See the module doc for why this is
    /// a deliberate number and not "as many as ask".
    pub size: usize,
    /// How long a checkout may queue before failing typed.
    pub acquire_timeout: Duration,
}

impl std::fmt::Debug for PoolConfig {
    /// Written out rather than derived so the password cannot reach a log
    /// line, a panic message or an error that formats its config.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("db", &self.db)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("size", &self.size)
            .field("acquire_timeout", &self.acquire_timeout)
            .finish()
    }
}

impl PoolConfig {
    /// Environment-driven config, same variables as the conformance
    /// harness (`ORDO_PG_HOST`/`PORT`/`USER`/`DB`).
    pub fn from_env() -> PoolConfig {
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        PoolConfig {
            host: env("ORDO_PG_HOST", "/tmp/ordo-pg"),
            port: env("ORDO_PG_PORT", "55432").parse().unwrap_or(55432),
            user: env("ORDO_PG_USER", "ordo"),
            db: env("ORDO_PG_DB", "ordo_conformance"),
            // `ORDO_PG_PASSWORD_FILE` or `ORDO_PG_PASSWORD`. The file is
            // the form an orchestrator's secret store mounts and the form
            // the image docs recommend: an environment variable is
            // readable from `/proc/<pid>/environ` and printed by every
            // `docker inspect`. Read through `ordo_env::secret`, the one reader of a secret
            // setting. A server refuses an unreadable file or both forms
            // at boot (`ordo_host::runtime`); here either reads as none.
            password: ordo_env::secret("ORDO_PG_PASSWORD").ok().flatten(),
            // `ORDO_PG_POOL_SIZE` when it is set, and `min(cores, 8)`
            // otherwise.
            //
            // **It was not read here until 2026-09-25**, which made this
            // function's name a half-truth: it read six environment
            // variables and computed the seventh. `ordo_host::runtime`
            // read the variable separately (`parsed("ORDO_PG_POOL_SIZE",
            // PoolConfig::from_env().size)`), so a deployment honoured it
            // and every other caller silently did not — which is why a
            // test suite setting it on a spawned server saw its pools
            // shrink while suites building pools directly kept dialling
            // eight apiece.
            //
            // Clamped to the same floor of 2 the default uses: the boot
            // refuses a per-space reserve that is not smaller than the
            // pool, so one connection is a size no deployment accepts.
            size: std::env::var("ORDO_PG_POOL_SIZE")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or_else(|| {
                    std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(4)
                        .clamp(2, 8)
                })
                .max(2),
            acquire_timeout: Duration::from_secs(2),
        }
    }

    /// The dialer, for a caller that needs a connection the pool does not
    /// own — [`crate::jobs::listen`]'s LISTEN socket, and the supervisor's
    /// leadership lock. Both hold **session** state, and a session-scoped
    /// thing on a pooled connection is a leak: `TxConn::commit` returns its
    /// client to the idle list the moment a query finishes.
    pub fn connect_config(&self) -> tokio_postgres::Config {
        self.pg()
    }

    fn pg(&self) -> tokio_postgres::Config {
        self.pg_bounded(None)
    }

    /// [`PoolConfig::pg`] with a `statement_timeout` pinned in the session
    /// options (20a).
    ///
    /// In the options rather than in a `SET` at checkout for two reasons:
    /// a connection carries it from the moment it is dialled, including the
    /// re-dial that replaces a dead one mid-request, and it costs no round
    /// trip on a path whose whole job is to be cheap. It is a session GUC,
    /// so a transaction that wants a different bound can still `SET LOCAL`
    /// over it.
    fn pg_bounded(&self, statement_timeout: Option<Duration>) -> tokio_postgres::Config {
        self.pg_named(statement_timeout, None)
    }

    /// [`PoolConfig::pg_bounded`] with an `application_name`, so the
    /// connection can be told apart in `pg_stat_activity`.
    fn pg_named(
        &self,
        statement_timeout: Option<Duration>,
        application_name: Option<&str>,
    ) -> tokio_postgres::Config {
        let mut c = tokio_postgres::Config::new();
        if let Some(name) = application_name {
            c.application_name(name);
        }
        if let Some(p) = &self.password {
            c.password(p);
        }
        c.host(&self.host)
            .port(self.port)
            .user(&self.user)
            .dbname(&self.db)
            // Pinned session rendering, matching the harness: date text
            // is a property of the platform, not of server defaults.
            .options(format!(
                // `standard_conforming_strings` is pinned, not assumed: with
                // it off, a backslash escapes the quote that ends a literal
                // and our doubling of `'` stops being sufficient. Correctness
                // of escaping must not depend on a server setting.
                //
                // `ordo.regex_max_len` is what the `matches` guard reads
                // ([`REGEX_MAX_LEN_ENV`]); a session without it gets the
                // default in the guard itself.
                "-c datestyle=ISO,YMD -c timezone=UTC -c standard_conforming_strings=on \
                 -c {}={}{}",
                ordo_plan::REGEX_MAX_LEN_GUC,
                regex_max_len(),
                match statement_timeout {
                    // Milliseconds is `statement_timeout`'s own unit when it
                    // is given a bare number, so there is no unit suffix to
                    // get wrong.
                    Some(d) => format!(" -c statement_timeout={}", d.as_millis()),
                    None => String::new(),
                }
            ))
            // A target that drops packets has to fail a dial in bounded time,
            // not after the operating system's TCP timeout. The bound is the
            // checkout's own: a dial happens at checkout for a dead
            // connection, so a slower dial would outlast the wait the caller
            // was promised anyway.
            .connect_timeout(self.acquire_timeout);
        if let Some(p) = &self.password {
            c.password(p);
        }
        c
    }
}

/// How the pool's bound is **divided**, as against [`PoolConfig`], which
/// says how large it is (20a).
///
/// # Why this is not a field of `PoolConfig`
///
/// `PoolConfig` is built with a struct literal at some thirty call sites
/// across the workspace, most of them tests. A required field there is
/// thirty edits that teach nothing, and an `Option` field there would still
/// have to be spelled at every one of them. This is its own value with a
/// [`Default`], read from the environment by [`Pool::connect`], so a
/// deployment that sets nothing inherits the policy and a caller that wants
/// a different one passes it to [`Pool::connect_with`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolLimits {
    /// Connections held out of **any single space's** reach, so that one
    /// space queueing slow work cannot leave another space with nothing.
    /// `None` computes it from the pool's size; see [`PoolLimits::space_max`].
    pub space_reserve: Option<usize>,
    /// How long **one statement** may run before the server cancels it.
    ///
    /// Set as a session GUC in the dial options, so every pooled connection
    /// carries it from the moment it exists and no checkout can forget to
    /// apply it. `None` is no bound, which is what a session-scoped
    /// connection outside the pool gets ([`PoolConfig::connect_config`]) and
    /// what an operator sets for a migration window.
    ///
    /// The bound reaches everything that runs on a pooled connection,
    /// including a writer blocked in the chain-append lock and a
    /// maintenance pass over a large table. That is the point of it and it
    /// is also its cost: a deployment whose migration legitimately needs
    /// longer raises the knob for that window rather than discovering the
    /// bound halfway through.
    pub statement_timeout: Option<Duration>,
    /// How long a connection may sit idle before the pool closes it. `None`
    /// keeps every connection it has dialled for the pool's life.
    ///
    /// There is no minimum held open: a pool that has been quiet for this
    /// long holds no connection at all, and the next checkout dials one. A
    /// database per tenant makes that the setting that decides what an
    /// idle tenant costs the server.
    pub idle_timeout: Option<Duration>,
    /// [`WRITE_RETRIES_ENV`]: how many more times a serializable write that
    /// lost a race with another write is tried before it is refused as
    /// `serialization_conflict`. `0` tries once.
    pub write_retries: u32,
    /// [`WRITE_RETRY_MAX_ENV`]: the longest pause between two tries of a
    /// write. The pause before try `n` is a random time up to
    /// `min(this, 4 ms * 2^n)`.
    pub write_retry_max: Duration,
}

/// How many more times a write that lost a serialization race is tried.
pub const WRITE_RETRIES_ENV: &str = "ORDO_WRITE_RETRIES";
/// The longest pause between two tries of a write, in milliseconds.
pub const WRITE_RETRY_MAX_ENV: &str = "ORDO_WRITE_RETRY_MAX_MS";

/// The longest `matches` pattern a statement runs, in characters. A longer
/// one, and one holding a backreference, is refused before the regex engine
/// sees it (`ordo_plan`'s `matches` guard), as `invalid_param`.
pub const REGEX_MAX_LEN_ENV: &str = "ORDO_REGEX_MAX_LEN";
/// The most [`REGEX_MAX_LEN_ENV`] may be set to.
pub const REGEX_MAX_LEN_MAX: usize = 10_000;

/// [`REGEX_MAX_LEN_ENV`], read once: [`ordo_plan::REGEX_MAX_LEN_DEFAULT`]
/// when unset, clamped to 1 to [`REGEX_MAX_LEN_MAX`]. `ordo_host::runtime`
/// refuses a value out of range at boot, naming it.
pub fn regex_max_len() -> usize {
    static LEN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *LEN.get_or_init(|| {
        std::env::var(REGEX_MAX_LEN_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .map_or(ordo_plan::REGEX_MAX_LEN_DEFAULT, |n| {
                n.clamp(1, REGEX_MAX_LEN_MAX)
            })
    })
}

impl Default for PoolLimits {
    /// A bound rather than none: an unbounded statement is the half of
    /// 20a that costs nothing to fix and hangs a deployment when it is
    /// missing. Thirty seconds is far past any request the platform means
    /// to serve and far short of a request anybody is still waiting for.
    fn default() -> PoolLimits {
        PoolLimits {
            space_reserve: None,
            statement_timeout: Some(Duration::from_secs(30)),
            idle_timeout: Some(PoolLimits::DEFAULT_IDLE),
            write_retries: PoolLimits::DEFAULT_WRITE_RETRIES,
            write_retry_max: PoolLimits::DEFAULT_WRITE_RETRY_MAX,
        }
    }
}

impl PoolLimits {
    /// Five more tries after the first. Small and finite by design
    /// (`implementation.md`: "retry budget is finite (small, with
    /// jitter)"): an unbounded retry under contention is a livelock that
    /// looks like a hang. The client SDK tries again on top of this.
    pub const DEFAULT_WRITE_RETRIES: u32 = 5;
    /// The most retries a deployment may set. Past this a write waits
    /// seconds on one row, which is a design to change rather than a
    /// number to raise.
    pub const MAX_WRITE_RETRIES: u32 = 20;
    /// A quarter second. With five retries the pause windows are 8, 16,
    /// 32, 64 and 128 ms, so this ceiling only bites when the retries are
    /// raised. The worst total pause is about 250 ms, where the old fixed
    /// backoff gave about 90 ms: too little for twenty writers on one row
    /// to spread out (coverage run 39197274294).
    pub const DEFAULT_WRITE_RETRY_MAX: Duration = Duration::from_millis(250);
    /// The longest ceiling a deployment may set, in milliseconds.
    pub const MAX_WRITE_RETRY_MAX_MS: u64 = 10_000;
    /// The idle period when nothing sets it: thirty seconds. Long enough
    /// that a burst of requests reuses the connections it just opened, short
    /// enough that a tenant who stopped using the app stops costing the
    /// server anything within the minute.
    pub const DEFAULT_IDLE: Duration = Duration::from_secs(30);

    /// Environment-driven limits: `ORDO_PG_SPACE_RESERVE`,
    /// `ORDO_PG_STATEMENT_TIMEOUT_MS`, where `0` means no statement bound,
    /// and `ORDO_PG_IDLE_SECS`, where `0` means idle connections are kept.
    ///
    /// An unparseable value is the same as an absent one rather than a boot
    /// refusal, because this is the library's reader and a server's boot
    /// refusals belong to `ordo_host::runtime`, which validates the whole
    /// environment in one place and before the first dial.
    pub fn from_env() -> PoolLimits {
        let statement_timeout = match std::env::var("ORDO_PG_STATEMENT_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            Some(0) => None,
            Some(ms) => Some(Duration::from_millis(ms)),
            None => PoolLimits::default().statement_timeout,
        };
        PoolLimits {
            space_reserve: std::env::var("ORDO_PG_SPACE_RESERVE")
                .ok()
                .and_then(|v| v.parse().ok()),
            statement_timeout,
            idle_timeout: match std::env::var("ORDO_PG_IDLE_SECS")
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
            {
                Some(0) => None,
                Some(secs) => Some(Duration::from_secs(secs)),
                None => PoolLimits::default().idle_timeout,
            },
            // Out of range is clamped here; `ordo_host::runtime` refuses
            // it at boot naming the range.
            write_retries: std::env::var(WRITE_RETRIES_ENV)
                .ok()
                .and_then(|v| v.trim().parse::<u32>().ok())
                .map_or(PoolLimits::DEFAULT_WRITE_RETRIES, |n| {
                    n.min(PoolLimits::MAX_WRITE_RETRIES)
                }),
            write_retry_max: std::env::var(WRITE_RETRY_MAX_ENV)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map_or(PoolLimits::DEFAULT_WRITE_RETRY_MAX, |ms| {
                    Duration::from_millis(ms.clamp(1, PoolLimits::MAX_WRITE_RETRY_MAX_MS))
                }),
        }
    }

    /// The most connections one space may hold at once, out of `size`.
    ///
    /// `size - reserve`, with `reserve` defaulting to a quarter of the pool
    /// and at least one. The floor is `min(size, 2)`: a pool of two or three
    /// connections cannot both serve a tenant and reserve anything, and a
    /// cap of one there would halve a single-tenant deployment's concurrency
    /// to buy an isolation guarantee that pool is too small to make.
    ///
    /// **What this bounds is one space, not every space.** Two spaces each
    /// at their cap can still reach the pool's own bound, and a third then
    /// waits and takes `Saturated`. That is deliberate: dividing the pool
    /// k ways as k moves needs a fair queue with feedback, and the failure
    /// §2.7 of `audit-topology-gaps.md` describes is one tenant's scan
    /// taking **every** connection, which a fixed cap ends outright. The
    /// reserve is the dial an operator raises when the tenant count per
    /// target goes up.
    pub fn space_max(&self, size: usize) -> usize {
        let reserve = self.space_reserve.unwrap_or_else(|| (size / 4).max(1));
        size.saturating_sub(reserve).max(size.min(2))
    }
}

/// A transaction's isolation level, spelled as Postgres spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isolation {
    ReadCommitted,
    /// One snapshot for the whole transaction. Used read-only, where it
    /// cannot fail to serialize.
    RepeatableRead,
    Serializable,
}

/// What class of work a transaction is, from `implementation.md`'s
/// "Isolation level per operation class" table. The table is encoded here
/// rather than at each call site so that a new caller picks a *class* and
/// inherits the pin, instead of picking a level and inventing one.
///
/// | class | isolation | why (the doc's words) |
/// |---|---|---|
/// | [`Read`](OpClass::Read) | READ COMMITTED, read-only | replica-eligible; cross-query consistency comes from log-positioned reads, not the level |
/// | [`SnapshotRead`](OpClass::SnapshotRead) | REPEATABLE READ, read-only | a read that reports the log position it was read at: the rows and the position must share one snapshot, which READ COMMITTED gives each statement separately |
/// | [`AddressedWrite`](OpClass::AddressedWrite) | READ COMMITTED | the conflict detector is `rev` (CAS) and the unique constraint, both exact at any level |
/// | [`ConvergeOrCreate`](OpClass::ConvergeOrCreate) | READ COMMITTED | atomic in one `INSERT … ON CONFLICT` statement |
/// | [`BulkConditional`](OpClass::BulkConditional) | **SERIALIZABLE** | the class whose condition the spec says evaluates atomically; only SERIALIZABLE buys that sentence |
/// | [`MultiStatementGroup`](OpClass::MultiStatementGroup) | **SERIALIZABLE** | a group is atomic across its statements by definition |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpClass {
    Read,
    /// A read whose rows and command-log position are taken in one
    /// snapshot, so the position is the rows' version and not a later one.
    /// `implementation.md`'s "log-positioned reads" rule, which a client
    /// replica orders its rows by (`plan-client-cache.md` C1).
    SnapshotRead,
    AddressedWrite,
    ConvergeOrCreate,
    BulkConditional,
    MultiStatementGroup,
}

impl OpClass {
    pub fn isolation(self) -> Isolation {
        match self {
            OpClass::Read | OpClass::AddressedWrite | OpClass::ConvergeOrCreate => {
                Isolation::ReadCommitted
            }
            OpClass::SnapshotRead => Isolation::RepeatableRead,
            OpClass::BulkConditional | OpClass::MultiStatementGroup => Isolation::Serializable,
        }
    }

    /// Reads take a read-only transaction. Not a micro-optimization: a
    /// read-only transaction is the thing a deployment can route to a
    /// replica, and it is the level at which "nothing on the read path pays
    /// for SERIALIZABLE" is enforced rather than asserted.
    pub fn read_only(self) -> bool {
        matches!(self, OpClass::Read | OpClass::SnapshotRead)
    }

    /// Only serializable work can produce a `40001`, so only serializable
    /// work is worth retrying.
    pub fn retryable(self) -> bool {
        self.isolation() == Isolation::Serializable
    }

    fn begin_stmt(self) -> &'static str {
        match (self.isolation(), self.read_only()) {
            (Isolation::Serializable, _) => "BEGIN ISOLATION LEVEL SERIALIZABLE",
            (Isolation::ReadCommitted, true) => "BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY",
            (Isolation::ReadCommitted, false) => "BEGIN ISOLATION LEVEL READ COMMITTED",
            (Isolation::RepeatableRead, true) => "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY",
            (Isolation::RepeatableRead, false) => "BEGIN ISOLATION LEVEL REPEATABLE READ",
        }
    }
}

/// How large the per-space share table may grow before a checkout for a
/// space it does not hold sweeps the idle entries out of it.
const SPACE_TABLE_PRUNE_AT: usize = 1024;

/// One checkout's admission: the pool's permit, and the space's share of
/// the pool when the handle is charged to one.
///
/// Held for as long as the connection is, and released by dropping, which
/// is why both live in one value that the transaction owns rather than in
/// two the caller has to keep together.
pub struct Permit {
    _global: OwnedSemaphorePermit,
    _space: Option<OwnedSemaphorePermit>,
    /// This checkout's entry in the pool's record of its holders, on a
    /// pool that keeps one ([`Pool::track_holders`]).
    hold: Option<holders::Hold>,
    /// Where the connection this checkout holds was dialled
    /// ([`holders::here`]): the runtime its socket task runs on. It goes
    /// back to the idle list with the connection.
    origin: Arc<str>,
}

impl Permit {
    /// Mark a statement sent, on a pool that tracks its holders.
    fn statement(&self, sql: &str) {
        if let Some(h) = &self.hold {
            h.statement(sql);
        }
    }

    /// Mark the statement answered.
    fn answered(&self) {
        if let Some(h) = &self.hold {
            h.answered();
        }
    }
}

/// An idle connection: the client, when it was returned, and where it was
/// dialled ([`Permit::origin`]).
type Idle = (Client, Instant, Arc<str>);

struct Shared {
    cfg: PoolConfig,
    /// Idle connections with the instant each was returned, oldest first.
    /// Checkout takes from the back, so the connections a busy pool keeps
    /// are the ones it just used and the ones nothing has wanted age out.
    idle: Mutex<Vec<Idle>>,
    permits: Arc<Semaphore>,
    /// The most connections one tagged space may hold at once
    /// ([`PoolLimits::space_max`]), resolved once at [`Pool::connect`].
    space_max: usize,
    /// One semaphore of [`Shared::space_max`] permits per space that has
    /// checked a connection out through a tagged handle.
    ///
    /// A `std` mutex rather than tokio's: the critical section is a map
    /// lookup with no `await` in it, and the permit itself is waited on
    /// outside the lock.
    spaces: std::sync::Mutex<HashMap<String, Arc<Semaphore>>>,
    /// How the bound is divided, kept for the re-dial at checkout: a
    /// replacement connection must carry the same statement bound as the
    /// one it replaces.
    limits: PoolLimits,
    /// Who holds this pool's connections, once [`Pool::track_holders`]
    /// has asked for it.
    holders: std::sync::OnceLock<Arc<holders::Holders>>,
    /// The `application_name` this pool's connections are dialled with,
    /// once [`Pool::track_holders`] has named the pool, so a Postgres
    /// snapshot can tell its backends from every other.
    application_name: std::sync::OnceLock<String>,
}

impl Shared {
    /// Return a healthy connection to the idle list.
    async fn give_back(&self, client: Client, origin: Arc<str>) {
        if !client.is_closed() {
            self.idle
                .lock()
                .await
                .push((client, Instant::now(), origin));
        }
    }

    /// Close every idle connection that has been idle for `period`, and
    /// every one whose socket has died.
    async fn reap(&self, period: Duration) {
        let mut idle = self.idle.lock().await;
        idle.retain(|(c, since, _)| !c.is_closed() && since.elapsed() < period);
    }
}

/// Close a pool's idle connections on a timer, until the pool is gone.
///
/// Holds the pool weakly, so the task never keeps a dropped pool's sockets
/// open and ends when the last handle goes. Ticks at a quarter of the idle
/// period, so a connection is closed between one and 1.25 periods after its
/// last use.
fn spawn_reaper(shared: &Arc<Shared>, period: Duration) {
    let weak = Arc::downgrade(shared);
    let tick = (period / 4).max(Duration::from_millis(50));
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tick).await;
            let Some(shared) = weak.upgrade() else { break };
            shared.reap(period).await;
        }
    });
}

/// The pool. Cloneable handle; all clones share the same connections and
/// the same bound.
#[derive(Clone)]
pub struct Pool {
    shared: Arc<Shared>,
    /// The space this handle's checkouts are **charged to** (20a), set by
    /// [`Pool::for_space`]. Per handle for the reason the observer is: it
    /// is a property of who holds the handle rather than of the sockets,
    /// and the host resolves a space per request onto a handle it then
    /// caches.
    ///
    /// `None` is a checkout charged to no space: a boot-time migration, an
    /// operator command, a loop walking every target. Those are bounded by
    /// the pool's own size and by nothing else, which is what they were
    /// before this field existed.
    space: Option<Arc<str>>,
    /// Where the chain-append lock's timings go (5f). Per **handle**, not
    /// per `Shared`: an observer is a policy of whoever holds the handle,
    /// not a property of the sockets, and keeping it here means reading it
    /// costs no lock on the write path's hottest line.
    observer: Option<Arc<dyn crate::metrics::AppendObserver>>,
    /// Set on a handle made by [`Pool::sweeping`]: a handle for a walker
    /// that touches each tenant once. It keeps no connection after a
    /// checkout (the socket closes instead of returning to the idle list),
    /// and every checkout takes a permit from this gate before a slot of
    /// the pool, holding it until its connection is dialled, so the walkers
    /// of one node together dial at most the gate's size at once.
    sweep: Option<Arc<Semaphore>>,
    /// How long a sweeping handle waits on `sweep` before giving up with
    /// [`PoolError::DialWaitExceeded`].
    sweep_wait: Duration,
    /// Who this handle's checkouts are recorded as on a pool that tracks
    /// its holders ([`Pool::labelled`]).
    label: Option<Arc<str>>,
}

async fn dial(cfg: &PoolConfig, statement_timeout: Option<Duration>) -> Result<Client, PoolError> {
    dial_named(cfg, statement_timeout, None).await
}

/// [`dial`], with an `application_name` when one is given.
async fn dial_named(
    cfg: &PoolConfig,
    statement_timeout: Option<Duration>,
    application_name: Option<&str>,
) -> Result<Client, PoolError> {
    let (client, conn) = cfg
        .pg_named(statement_timeout, application_name)
        .connect(NoTls)
        .await
        .map_err(driver_error)?;
    // The socket driver: one lightweight task per connection on the
    // shared runtime. It ends when the client drops.
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(client)
}

/// The database `CREATE DATABASE` is issued from. Every Postgres cluster has
/// one by this name, and a new database cannot be created from inside itself.
pub const MAINTENANCE_DB: &str = "postgres";

/// Whether `name` is a database name this platform will create.
///
/// Lowercase letters, digits and underscores, starting with a letter or an
/// underscore, at most 63 bytes (Postgres truncates longer names silently,
/// and a silently different name is a different database). Narrower than
/// Postgres allows, because the name is quoted into a statement and a
/// narrow alphabet is what makes that quoting trivially safe.
pub fn valid_database_name(name: &str) -> bool {
    name.len() <= 63
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Reach the cluster `cfg` points at and say whether `name` is already
/// standing there, **without creating anything**.
///
/// `bootstrap.md` §2 item 26 asks preflight to *"connect and run a probe
/// transaction"* rather than merely parse the target, and this is that
/// probe. It dials exactly the connection [`create_database`] will dial —
/// the cluster's [`MAINTENANCE_DB`], with the target's own credentials —
/// so a target that passes here is one whose host resolves, whose port
/// answers, and whose role can log in. It answers the catalogue question
/// on the same round trip, because that is the difference between a plan
/// that says *"this will create a database"* and one that says *"this will
/// adopt the one already there"*, and an operator reading a dry run is
/// entitled to know which.
///
/// **What it does not prove**, said plainly rather than implied: that the
/// role may `CREATE DATABASE`. Postgres will not tell us without trying,
/// and trying is the write this exists to avoid. A credential that can
/// connect and not create therefore still fails in phase 3, which is the
/// residue `bootstrap.md` §4 is about. What this removes is the far
/// commoner case of a host, a port or a password that was simply wrong,
/// which used to cost a registry row and a `failed` tenant.
pub async fn probe(cfg: &PoolConfig, name: &str) -> Result<bool, PoolError> {
    let client = dial(
        &PoolConfig {
            db: MAINTENANCE_DB.to_string(),
            ..cfg.clone()
        },
        // The same absence of a statement timeout the other maintenance
        // dials take: one round trip on a connection closed immediately
        // afterwards is not a tenant's runaway query.
        None,
    )
    .await?;
    let rows = client
        .query("SELECT 1 FROM pg_database WHERE datname = $1", &[&name])
        .await
        .map_err(driver_error)?;
    Ok(!rows.is_empty())
}

/// Create database `name` on the cluster `cfg` points at, and say whether
/// this call created it.
///
/// **"Already exists" is success**, reported as `Ok(false)`, because this is
/// a step in a provisioning sequence a retry has to be able to complete, and
/// `CREATE DATABASE` cannot run inside a transaction, so there is no
/// rolling it back with the steps around it. A concurrent create can also
/// lose on the catalog's unique index rather than on the existence check,
/// and that is the same answer.
///
/// Dials the cluster's [`MAINTENANCE_DB`] once and closes; it does not use a
/// pool, because a pool creates the platform tables in the database it
/// opens and the maintenance database must not receive them.
/// Drop a database this platform made, for `std.space.reap` (11-15).
///
/// **`WITH (FORCE)`**, which terminates whatever is still connected. A
/// database being reaped belongs to a tenant that never became `active`,
/// so anything holding a session on it is this deployment's own pool from
/// the failed provision, not a caller doing work. Without it a reap races
/// the connection it is trying to clean up and fails intermittently, which
/// is the worst behaviour for the operation whose whole job is to make a
/// mess go away.
///
/// **`IF EXISTS`**, so a reap is idempotent: a create that died before
/// `CREATE DATABASE` leaves a registry row naming a database that was
/// never made, and finishing the cleanup for it must succeed rather than
/// report a problem that is already resolved.
///
/// Returns whether a database was actually dropped, which is what lets the
/// caller record what the compensation did rather than what it attempted.
///
/// The name is validated the same way [`create_database`] validates it.
/// This interpolates into DDL, where a bind parameter is not available, so
/// the check is the boundary.
pub async fn drop_database(cfg: &PoolConfig, name: &str) -> Result<bool, PoolError> {
    if !valid_database_name(name) {
        return Err(PoolError::db(format!(
            "`{name}` is not a database name this platform creates, so it is not one it drops"
        )));
    }
    let client = dial(
        &PoolConfig {
            db: MAINTENANCE_DB.to_string(),
            ..cfg.clone()
        },
        None,
    )
    .await?;
    let before = client
        .simple_query(&format!(
            "SELECT 1 FROM pg_database WHERE datname = {}",
            sql_lit(name)
        ))
        .await
        .map_err(driver_error)?
        .into_iter()
        .filter(|m| matches!(m, tokio_postgres::SimpleQueryMessage::Row(_)))
        .count();
    client
        .simple_query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .await
        .map_err(driver_error)?;
    Ok(before > 0)
}

/// The advisory-lock namespace that serialises [`Pool::ensure_platform_tables`]
/// across every pool booting on one database ("ORDT"), beside
/// `commandlog::APPEND_LOCK_NS` and `secevent::SECEVENT_LOCK_NS`.
pub const PLATFORM_DDL_LOCK_NS: i32 = 0x4F52_4454;

pub async fn create_database(cfg: &PoolConfig, name: &str) -> Result<bool, PoolError> {
    if !valid_database_name(name) {
        return Err(PoolError::db(format!(
            "`{name}` is not a database name this platform creates"
        )));
    }
    // No statement bound: this dials once, issues one `CREATE DATABASE`
    // and closes, and a provisioning step cancelled halfway is a step a
    // retry has to be able to finish rather than a tenant's runaway query.
    let client = dial(
        &PoolConfig {
            db: MAINTENANCE_DB.to_string(),
            ..cfg.clone()
        },
        None,
    )
    .await?;
    match client
        .simple_query(&format!("CREATE DATABASE \"{name}\""))
        .await
    {
        Ok(_) => Ok(true),
        Err(e)
            if e.code() == Some(&tokio_postgres::error::SqlState::DUPLICATE_DATABASE)
                || e.code() == Some(&tokio_postgres::error::SqlState::UNIQUE_VIOLATION) =>
        {
            Ok(false)
        }
        Err(e) => Err(driver_error(e)),
    }
}

/// The driver's error with its causes. `tokio_postgres::Error` prints only
/// its kind ("invalid configuration"); the reason ("password missing") is in
/// its source chain, and a refusal without the reason cannot be acted on.
fn described(e: &tokio_postgres::Error) -> String {
    let mut out = e.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(c) = cause {
        out.push_str(": ");
        out.push_str(&c.to_string());
        cause = c.source();
    }
    out
}

/// A driver error as the pool's own, typed by its SQLSTATE, with the driver
/// error kept as the cause and its causes spelled out in the text
/// ([`described`]).
pub(crate) fn driver_error(e: tokio_postgres::Error) -> PoolError {
    let text = described(&e);
    PoolError::from_driver(e, text)
}

/// A query answering `t` when the guarded statement `stmt` would change
/// nothing, or `None` when its shape is not one of the platform's guards.
fn already_in_place(stmt: &str) -> Option<String> {
    fn quoted(s: &str) -> Vec<&str> {
        s.split('"').skip(1).step_by(2).collect()
    }
    fn literal(s: &str) -> String {
        s.replace('\'', "''")
    }
    fn regclass(name: &str) -> String {
        format!("to_regclass('\"{}\"')", literal(name))
    }
    let words = stmt.split_whitespace().collect::<Vec<_>>().join(" ");
    let upper = words.to_uppercase();
    let names = quoted(&words);
    let first = *names.first()?;
    if upper.starts_with("CREATE TABLE IF NOT EXISTS ")
        || upper.starts_with("CREATE INDEX IF NOT EXISTS ")
        || upper.starts_with("CREATE UNIQUE INDEX IF NOT EXISTS ")
    {
        return Some(format!(
            "SELECT CASE WHEN {} IS NOT NULL THEN 't' ELSE 'f' END",
            regclass(first)
        ));
    }
    if upper.starts_with("ALTER TABLE ") && upper.contains(" ADD COLUMN IF NOT EXISTS ") {
        // Every added column is named right after its `IF NOT EXISTS`.
        let mut columns = Vec::new();
        let mut rest = words.as_str();
        let marker = "ADD COLUMN IF NOT EXISTS ";
        while let Some(i) = rest.to_uppercase().find(marker) {
            rest = &rest[i + marker.len()..];
            columns.push(quoted(rest).first().copied()?);
        }
        if columns.is_empty() {
            return None;
        }
        let all = columns
            .iter()
            .map(|c| {
                format!(
                    "EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid = {} \
                     AND attname = '{}' AND NOT attisdropped)",
                    regclass(first),
                    literal(c)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        return Some(format!("SELECT CASE WHEN {all} THEN 't' ELSE 'f' END"));
    }
    if upper.starts_with("ALTER TABLE ") && upper.contains(" DROP CONSTRAINT IF EXISTS ") {
        let constraint = *names.get(1)?;
        return Some(format!(
            "SELECT CASE WHEN NOT EXISTS (SELECT 1 FROM pg_constraint \
             WHERE conrelid = {} AND conname = '{}') THEN 't' ELSE 'f' END",
            regclass(first),
            literal(constraint)
        ));
    }
    None
}

impl Pool {
    /// Create the platform's own tables once, at startup.
    ///
    /// Not from inside the request path: `CREATE TABLE IF NOT EXISTS`
    /// takes a lock even as a no-op, so running it inside one transaction
    /// blocks every other transaction that runs the same guard.
    pub async fn ensure_platform_tables(&self) -> Result<(), PoolError> {
        // `CREATE TABLE IF NOT EXISTS` is not race-free: two replicas
        // booting together can both pass the existence check and one then
        // fails on the catalog's unique index. The guard is advisory, not
        // atomic, a known Postgres wart that every deployment scaling
        // horizontally meets on its first simultaneous rollout.
        //
        // So every boot takes one transaction-scoped advisory lock and runs
        // the whole list inside it: boots take turns, and the second finds
        // every table made. A retry once was not enough: three pools racing
        // on a fresh database lost twice (CI b15227ea, 2026-10-24,
        // `$commandlog`'s row type).
        //
        // **Each list entry runs in a transaction of its own, and only the
        // statements that would change something run at all.** Every
        // `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` takes `ACCESS
        // EXCLUSIVE` even when the column is there, and one transaction
        // holding that on `$sealedpayload` while it waited for `$blobref`
        // deadlocked with a blob write that holds them in the other order.
        // A dial on a running deployment (the write path's first
        // connection to a tenant) therefore probes the catalog first, with
        // plain reads, and takes no table lock when the shape is already
        // there. When something is missing, the lock is held for one entry
        // and released at its commit, so no transaction waits on a second
        // table while holding the first.
        for ddl in [
            REPLAY_DDL,
            crate::jobs::JOBS_DDL,
            crate::commandlog::COMMANDLOG_DDL,
            crate::secevent::SECEVENT_DDL,
            crate::triggers::TRIGGERS_DDL,
            ordo_crypto::SEALED_PAYLOAD_DDL,
            // The blob reference store and its tombstone table, beside the
            // sealed payload store for the same reason: `ordo_crypto` owns
            // the shape because two crates that cannot see each other both
            // create it, and this is the one place a running deployment
            // does.
            ordo_crypto::BLOB_REF_DDL,
            ordo_crypto::BLOB_TOMB_DDL,
            // 17n M1b: `$usage_replica` and `$usage`, the metering
            // ledger's durable half. Control-database tables, like every
            // other row in this list.
            crate::usage::USAGE_DDL,
            // The bulk importer's old-id to new-id mapping
            // (`docs/plan-import.md` §4). A control-database table with a
            // `$space` column, so `erase_space`'s information_schema sweep
            // finds it without being told.
            crate::import::IMPORTMAP_DDL,
        ] {
            let pending = self.pending_statements(ddl).await?;
            if pending.is_empty() {
                continue;
            }
            let tx = self.begin().await?;
            tx.rows(&format!(
                "SELECT pg_advisory_xact_lock({PLATFORM_DDL_LOCK_NS}, 0)"
            ))
            .await?;
            for stmt in &pending {
                tx.rows(stmt).await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }

    /// The statements of `ddl` that would change the database, in order.
    ///
    /// Each statement of a platform DDL list is a `CREATE TABLE`, `CREATE
    /// INDEX`, `ADD COLUMN` or `DROP CONSTRAINT` with an `IF [NOT] EXISTS`
    /// guard; this asks the catalog, with reads that lock nothing, whether
    /// the guard would be a no-op. A statement it does not recognise is
    /// kept. Once one statement is pending, every later one is too, because
    /// it may act on what the earlier one makes.
    async fn pending_statements(&self, ddl: &str) -> Result<Vec<String>, PoolError> {
        let mut pending = Vec::new();
        for stmt in ddl.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            if pending.is_empty() {
                if let Some(probe) = already_in_place(stmt) {
                    let rows = self.rows(&probe).await?;
                    let done = rows
                        .first()
                        .and_then(|r| r.first())
                        .and_then(|c| c.as_deref())
                        == Some("t");
                    if done {
                        continue;
                    }
                }
            }
            pending.push(stmt.to_string());
        }
        Ok(pending)
    }

    /// Dial one connection now, so a bad config fails at boot, and the rest
    /// on demand. Divides the bound by [`PoolLimits::from_env`].
    pub async fn connect(cfg: PoolConfig) -> Result<Pool, PoolError> {
        Pool::connect_with(cfg, PoolLimits::from_env()).await
    }

    /// [`Pool::connect`] with the division stated rather than read from the
    /// environment, for `ordo_host::runtime`, which validates the whole
    /// environment itself, and for a test that wants a cap it chose.
    pub async fn connect_with(cfg: PoolConfig, limits: PoolLimits) -> Result<Pool, PoolError> {
        let pool = Pool::open(cfg, limits).await?;
        pool.ensure_platform_tables().await?;
        Ok(pool)
    }

    /// A pool on a database that is **not a deployment's**, so it gets none
    /// of the platform's tables. The console's own database (`ordo-console`,
    /// `plan-console.md` 5.2) is the one caller: it runs its own migrations,
    /// and a deployment's tables there would be rows nothing reads.
    pub async fn connect_own(cfg: PoolConfig, limits: PoolLimits) -> Result<Pool, PoolError> {
        Pool::open(cfg, limits).await
    }

    /// A pool on a deployment database **whose platform tables someone else
    /// already made**, for a reader on a hot path that must take no table
    /// lock. The door's read of a tenant's members
    /// (`ordo_host::directory`) is the caller: it dials on every rebuild,
    /// and a rebuild follows every announced write.
    ///
    /// [`Pool::ensure_platform_tables`] holds an `ACCESS EXCLUSIVE` lock on
    /// each table it alters until it commits, even when there is nothing
    /// to add. Run beside writes, it holds `$sealedpayload` and waits for
    /// `$blobref`, while a blob write holds `$blobref` and waits for
    /// `$sealedpayload`: a deadlock, and the door then denied the whole
    /// space (`concurrent_writes_stay_consistent`, 2026-10-27).
    pub async fn connect_without_ddl(cfg: PoolConfig) -> Result<Pool, PoolError> {
        Pool::open(cfg, PoolLimits::from_env()).await
    }

    async fn open(cfg: PoolConfig, limits: PoolLimits) -> Result<Pool, PoolError> {
        // One connection now, so that an unreachable database or a wrong
        // password fails here and not on the first request. The rest dial
        // when a checkout finds none idle.
        let first = dial(&cfg, limits.statement_timeout).await?;
        let space_max = limits.space_max(cfg.size);
        let shared = Arc::new(Shared {
            permits: Arc::new(Semaphore::new(cfg.size)),
            idle: Mutex::new(vec![(first, Instant::now(), Arc::from(holders::here()))]),
            space_max,
            spaces: std::sync::Mutex::new(HashMap::new()),
            limits,
            cfg,
            holders: std::sync::OnceLock::new(),
            application_name: std::sync::OnceLock::new(),
        });
        if let Some(period) = limits.idle_timeout {
            spawn_reaper(&shared, period);
        }
        Ok(Pool {
            shared,
            observer: None,
            space: None,
            sweep: None,
            sweep_wait: route::TENANT_DIAL_WAIT_DEFAULT,
            label: None,
        })
    }

    /// A handle onto the same connections that reports the chain-append
    /// lock's timings to `observer` (5f).
    ///
    /// A new handle rather than a mutation, because `Pool` is `Clone` and
    /// shared: mutating one clone's observer through an `Arc` would need a
    /// lock on the append path, and the whole point of instrumenting the
    /// lock is not to add a second one.
    pub fn observing(&self, observer: Arc<dyn crate::metrics::AppendObserver>) -> Pool {
        Pool {
            shared: self.shared.clone(),
            observer: Some(observer),
            space: self.space.clone(),
            sweep: self.sweep.clone(),
            sweep_wait: self.sweep_wait,
            label: self.label.clone(),
        }
    }

    /// A handle onto the same connections whose checkouts are **charged to
    /// `space`**: bounded by that space's share of the pool
    /// ([`PoolLimits::space_max`]) and reported under its name (20a).
    ///
    /// A new handle rather than an argument to [`Pool::begin`], for the
    /// reason [`crate::Executor`] carries its space as a field: a space
    /// passed per call is a space a call site can get wrong, and every
    /// request-path caller here already sits below one resolution
    /// (`ordo_host::spaces::Spaces::pool_of`) that knows the answer once.
    ///
    /// Two handles for two spaces are two handles to
    /// [`Pool::same_handle`], because the cached executor above this has to
    /// be re-cut when the space changes.
    pub fn for_space(&self, space: &ordo_crypto::SpaceId) -> Pool {
        Pool {
            shared: self.shared.clone(),
            observer: self.observer.clone(),
            space: Some(Arc::from(space.0.as_str())),
            sweep: self.sweep.clone(),
            sweep_wait: self.sweep_wait,
            label: self.label.clone(),
        }
    }

    /// A handle onto the same pool for a walker that touches each tenant
    /// once (a boot pass, a catch-up sweep, a deploy).
    ///
    /// **Why it exists.** A pool keeps the connection it used until
    /// `ORDO_PG_IDLE_SECS` passes, which is right for a tenant being served
    /// and wrong for a walk over every tenant: a walk of 121 tenants left 121
    /// connections open together. A sweeping handle closes its connection at
    /// the end of each checkout, and takes a permit from `gate` before each
    /// checkout's slot of the pool, until it has its connection, so however
    /// many walkers run, at most the gate's size dial at once, and a walker
    /// waiting on the gate holds no slot. The rest of the pool, and every
    /// other handle, is unchanged.
    pub fn sweeping(&self, gate: Arc<Semaphore>) -> Pool {
        self.sweeping_for(gate, route::TENANT_DIAL_WAIT_DEFAULT)
    }

    /// [`Pool::sweeping`], giving up on the gate after `wait` with
    /// [`PoolError::DialWaitExceeded`] ([`route::TENANT_DIAL_WAIT_MS_ENV`]).
    pub fn sweeping_for(&self, gate: Arc<Semaphore>, wait: Duration) -> Pool {
        Pool {
            shared: self.shared.clone(),
            observer: self.observer.clone(),
            space: self.space.clone(),
            sweep: Some(gate),
            sweep_wait: wait,
            label: self.label.clone(),
        }
    }

    /// Keep a record of who holds this pool's connections, and on every
    /// checkout refused as busy hand it to `report`, at most once per
    /// [`holders::REPORT_EVERY`]. For a small pool every caller depends on,
    /// such as the door's store pool, so that a refusal
    /// names the store holding the connection, for how long, and whether it
    /// is dialling or holding it. Set once; a second call changes nothing.
    ///
    /// The pool's connections are dialled from then on with
    /// `application_name` `ordo: <name>`, and the idle ones are closed so
    /// the next checkout dials one that carries it: a snapshot of
    /// `pg_stat_activity` taken at a refusal can then pick this pool's
    /// backends out ([`incident`]).
    pub fn track_holders(&self, name: &str, report: holders::BusyReport) {
        let _ = self
            .shared
            .holders
            .set(Arc::new(holders::Holders::new(name.to_string(), report)));
        let _ = self
            .shared
            .application_name
            .set(incident::application_name(name));
        if let Ok(mut idle) = self.shared.idle.try_lock() {
            idle.clear();
        }
    }

    /// The `application_name` this pool's connections carry, when
    /// [`Pool::track_holders`] named it.
    pub fn application_name(&self) -> Option<&str> {
        self.shared.application_name.get().map(String::as_str)
    }

    /// A handle whose checkouts are recorded as `label` on a pool that
    /// tracks its holders. The same connections and bound.
    pub fn labelled(&self, label: &str) -> Pool {
        Pool {
            label: Some(Arc::from(label)),
            ..self.clone()
        }
    }

    /// Who holds this pool's connections now, longest first. Empty on a
    /// pool that does not track its holders.
    pub fn holders(&self) -> Vec<holders::Holder> {
        self.shared
            .holders
            .get()
            .map(|h| h.snapshot())
            .unwrap_or_default()
    }

    /// Close every connection this pool is holding idle. Checkouts still
    /// work: the next one dials again.
    pub async fn close_idle(&self) {
        self.shared.idle.lock().await.clear();
    }

    /// Return `client` to the idle list, unless this is a sweeping handle,
    /// which lets the socket close.
    async fn give_back(&self, client: Client, permit: &Permit) {
        if self.sweep.is_none() {
            self.shared.give_back(client, permit.origin.clone()).await;
        }
    }

    /// The space this handle's checkouts are charged to, if any.
    pub fn charged_space(&self) -> Option<&str> {
        self.space.as_deref()
    }

    /// Whether `other` is this handle: the same connections, the same
    /// space, and the same observer.
    ///
    /// Both halves, because an observer is a policy of the handle rather
    /// than of the sockets (see [`Pool::observing`]), so two handles over
    /// one set of connections that report to different places are two
    /// handles. A cached per-space executor asks this to learn whether the
    /// resolver still answers what it answered when the executor was cut.
    pub fn same_handle(&self, other: &Pool) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
            && self.space == other.space
            && self.sweep.is_some() == other.sweep.is_some()
            && match (&self.observer, &other.observer) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }

    /// The configuration this pool dials with.
    ///
    /// For a caller that needs a connection of its own to the same database
    /// rather than one of the pool's, which is what a `LISTEN` session is:
    /// session state that a pooled connection would lose the moment it went
    /// back to the idle list. The password it carries is redacted from the
    /// config's `Debug`.
    pub fn config(&self) -> PoolConfig {
        self.shared.cfg.clone()
    }

    /// The observer this handle reports to, if any.
    pub(crate) fn observer(&self) -> Option<&dyn crate::metrics::AppendObserver> {
        self.observer.as_deref()
    }

    /// The semaphore holding `space`'s share, minted on first checkout.
    ///
    /// The table is pruned when it grows past [`SPACE_TABLE_PRUNE_AT`],
    /// dropping the entries no checkout holds and none is waiting on. A
    /// deployment serving tens of thousands of spaces would otherwise keep
    /// one semaphore per space it has ever served, and the live population
    /// is bounded by the pool's own size rather than by the tenant count.
    fn space_share(&self, space: &str) -> Arc<Semaphore> {
        let mut table = self.shared.spaces.lock().unwrap_or_else(|p| p.into_inner());
        if table.len() >= SPACE_TABLE_PRUNE_AT && !table.contains_key(space) {
            let max = self.shared.space_max;
            table.retain(|_, s| Arc::strong_count(s) > 1 || s.available_permits() != max);
        }
        table
            .entry(space.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.shared.space_max)))
            .clone()
    }

    /// Admission for one checkout: the space's share first, then the pool's
    /// own bound, both inside one [`PoolConfig::acquire_timeout`].
    ///
    /// **The order is the whole mechanism.** A checkout that took the pool's
    /// permit first and then waited on its space's share would be holding a
    /// connection it cannot use while a space that has one waits behind it,
    /// which is the starvation this exists to end wearing a different
    /// shape. Waiting on the space's share costs the pool nothing.
    ///
    /// One budget rather than two, because `acquire_timeout` is a promise
    /// made to the caller: a request must not be able to wait twice as long
    /// as the deployment declared because it queued in two places.
    async fn admit(&self, started: Instant) -> Result<Permit, PoolError> {
        let budget = self.shared.cfg.acquire_timeout;
        let space_permit = match self.space.as_deref() {
            Some(space) if self.shared.space_max < self.shared.cfg.size => {
                let share = self.space_share(space);
                match share.clone().try_acquire_owned() {
                    Ok(p) => Some(p),
                    // At its cap. Everything past here is this space
                    // queueing behind itself, which is the point.
                    Err(_) => {
                        if let Some(o) = self.observer() {
                            o.pool_capped(space);
                        }
                        Some(
                            tokio::time::timeout(
                                budget.saturating_sub(started.elapsed()),
                                share.acquire_owned(),
                            )
                            .await
                            .map_err(|_| self.saturated())?
                            .expect("space share semaphore never closes"),
                        )
                    }
                }
            }
            _ => None,
        };
        let global = tokio::time::timeout(
            budget.saturating_sub(started.elapsed()),
            self.shared.permits.clone().acquire_owned(),
        )
        .await
        .map_err(|_| self.saturated())?
        .expect("pool semaphore never closes");
        Ok(Permit {
            _global: global,
            _space: space_permit,
            origin: Arc::from(""),
            hold: self
                .shared
                .holders
                .get()
                .map(|h| h.enter(self.label.as_ref())),
        })
    }

    /// Report the refusal under the space that was refused, then be the
    /// refusal.
    fn saturated(&self) -> PoolError {
        if let (Some(o), Some(space)) = (self.observer(), self.space.as_deref()) {
            o.pool_saturated(space);
        }
        if let Some(h) = self.shared.holders.get() {
            h.refused(self.label.as_ref());
        }
        PoolError::Saturated
    }

    async fn acquire(&self) -> Result<(Client, Permit), PoolError> {
        // **A sweeping handle queues on the node's gate before it takes a
        // slot of this pool**, and gives the gate back once it has its
        // connection. The other order held a slot of this tenant's pool
        // while waiting behind every other walker's dials, so a request or
        // another walker on this tenant was refused `Saturated` for a queue
        // that had nothing to do with it. This order is safe: the gate is
        // held for at most the acquire budget and one dial, and nothing
        // waits on the gate while holding a slot. The wait on the gate is
        // outside the acquire budget, because a walker is background work
        // that should wait its turn. It has its own limit,
        // `ORDO_PG_TENANT_DIAL_WAIT_MS`, after which the step fails.
        let gate = match &self.sweep {
            Some(g) => Some(route::wait_for_dial_gate(g, self.sweep_wait).await?),
            None => None,
        };
        let started = Instant::now();
        let permit = self.admit(started).await?;
        if let (Some(o), Some(space)) = (self.observer(), self.space.as_deref()) {
            o.pool_wait(space, started.elapsed().as_secs_f64());
        }
        let mut permit = permit;
        let existing = self.shared.idle.lock().await.pop();
        let client = match existing {
            Some((c, _, origin)) if !c.is_closed() => {
                permit.origin = origin;
                c
            }
            // Health at checkout: a dead (or discarded) connection is
            // replaced here, where the caller is already paying latency.
            // Its socket task is spawned on the runtime this runs on, which
            // is recorded with it.
            _ => {
                let c = dial_named(
                    &self.shared.cfg,
                    self.shared.limits.statement_timeout,
                    self.application_name(),
                )
                .await?;
                permit.origin = Arc::from(holders::here());
                c
            }
        };
        drop(gate);
        if let Some(h) = &permit.hold {
            h.holding(&permit.origin);
        }
        Ok((client, permit))
    }

    /// One-shot text-protocol query on a pooled connection.
    pub async fn rows(&self, sql: &str) -> Result<Vec<TextRow>, PoolError> {
        let (client, permit) = self.acquire().await?;
        permit.statement(sql);
        let out = simple_rows(&client, sql).await;
        permit.answered();
        self.give_back(client, &permit).await;
        out
    }

    /// `statements` in order on ONE pooled connection, each as its own
    /// query. The rows of the last one come back.
    ///
    /// For a statement that may not run inside a transaction block and
    /// still needs a session setting in force: `CREATE INDEX CONCURRENTLY`
    /// under `SET lock_timeout`. Several statements in one [`Pool::rows`]
    /// string run as one implicit transaction, which that statement
    /// refuses, and a `SET` sent through a second `rows` call lands on
    /// whichever connection the pool hands out next.
    ///
    /// The connection returns to the pool only when every statement
    /// succeeded, so the caller's last statement is the `RESET` that undoes
    /// the first. A failure drops the connection instead, and with it the
    /// setting: a session setting never outlives this call on a pooled
    /// socket.
    pub async fn rows_in_session(&self, statements: &[&str]) -> Result<Vec<TextRow>, PoolError> {
        let (client, permit) = self.acquire().await?;
        let mut out = Vec::new();
        for sql in statements {
            // `?` drops `client`: its socket closes and the pool dials a
            // fresh one at the next checkout.
            permit.statement(sql);
            out = simple_rows(&client, sql).await?;
            permit.answered();
        }
        self.give_back(client, &permit).await;
        Ok(out)
    }

    /// Begin a transaction on a dedicated connection. Statements run
    /// through the returned guard; [`TxConn::commit`] returns the
    /// connection to the pool, drop-without-commit discards it (the
    /// server aborts the open transaction when the socket closes).
    ///
    /// This is the addressed-write level (READ COMMITTED), stated rather
    /// than inherited: a bare `BEGIN` picks up whatever
    /// `default_transaction_isolation` happens to be, and an isolation
    /// level the spec pins must not depend on a server setting — the same
    /// discipline `standard_conforming_strings` gets in [`PoolConfig::pg`].
    pub async fn begin(&self) -> Result<TxConn, PoolError> {
        self.begin_for(OpClass::AddressedWrite).await
    }

    /// Begin at the level `implementation.md` pins for this class of work.
    pub async fn begin_for(&self, class: OpClass) -> Result<TxConn, PoolError> {
        let (client, permit) = self.acquire().await?;
        permit.statement(class.begin_stmt());
        simple_rows(&client, class.begin_stmt()).await?;
        permit.answered();
        Ok(TxConn {
            client: Some(client),
            pool: self.clone(),
            _permit: permit,
        })
    }

    /// Run a unit of work at its pinned isolation, retrying the **whole
    /// unit** on a serialization failure.
    ///
    /// The boundary is deliberate. `implementation.md` names three
    /// properties that make re-executing a command group safe, and each one
    /// is a property of this shape rather than a hope:
    ///
    /// - **Triggers and invalidations fire on commit.** They are enqueued
    ///   inside the transaction ([`TxConn::enqueue`], `pg_notify`), so an
    ///   aborted attempt fires nothing and leaves nothing to compensate.
    /// - **The idempotency record is written in the same transaction as the
    ///   group.** A retried attempt re-reads the journal from scratch inside
    ///   its own transaction, so it cannot consume its key twice.
    /// - **The chain append happens once, for the attempt that commits.**
    ///   It is inside the transaction too, so an aborted attempt's link
    ///   rolls back with the effect it described.
    ///
    /// `body` therefore receives a *fresh* transaction each attempt and
    /// must build everything it needs inside it. It returns the transaction
    /// back so that **commit is ours**: SSI reports most conflicts at
    /// `COMMIT`, and a helper that let the body commit would miss exactly
    /// the failures it exists to retry.
    pub async fn retrying<F, Fut, T>(&self, class: OpClass, mut body: F) -> Result<T, PoolError>
    where
        F: FnMut(TxConn) -> Fut,
        Fut: std::future::Future<Output = Result<(T, TxConn), PoolError>>,
    {
        let attempts = self.write_attempts();
        let space = self.space.as_deref().unwrap_or("").to_string();
        let mut last = None;
        for attempt in 1..=attempts {
            let tx = self.begin_for(class).await?;
            match body(tx).await {
                Ok((value, tx)) => match tx.commit().await {
                    Ok(()) => return Ok(value),
                    Err(e) if class.retryable() && e.is_retryable() => last = Some(e),
                    Err(e) => return Err(e),
                },
                // The transaction is dropped by `body`'s error path, which
                // closes its socket and aborts it server-side.
                Err(e) if class.retryable() && e.is_retryable() => last = Some(e),
                Err(e) => return Err(e),
            }
            self.lost_race(&space, attempt, attempts).await;
        }
        Err(PoolError::exhausted(attempts, last))
    }

    /// The limits this pool was opened with.
    pub fn limits(&self) -> PoolLimits {
        self.shared.limits
    }

    /// Every try a serializable write gets: the first, plus
    /// [`PoolLimits::write_retries`].
    pub fn write_attempts(&self) -> u32 {
        self.shared.limits.write_retries.saturating_add(1)
    }

    /// Try `attempt` of `attempts` lost a serialization race: report it,
    /// and pause before the next try when there is one.
    async fn lost_race(&self, space: &str, attempt: u32, attempts: u32) {
        if let Some(o) = self.observer() {
            o.write_conflict(space);
        }
        if attempt < attempts {
            backoff(attempt, self.shared.limits.write_retry_max).await;
        }
    }
}

/// The first pause window, doubled on each try after it. It is the scale of
/// one short write transaction, so the first retry spreads writers over a
/// few of them.
const BACKOFF_UNIT: Duration = Duration::from_millis(4);

/// The pause after try `attempt` lost: exponential backoff with full
/// jitter, a random time from zero to `min(max, 4 ms * 2^attempt)`.
///
/// Without the jitter two conflicting writers re-collide in lockstep, which
/// turns a retry budget into a countdown to failure rather than a way of
/// succeeding. The draw is from the operating system's random source. The
/// clock's sub-millisecond noise used before is the same for writers in one
/// process that lost the same race at the same instant, which is exactly
/// the case jitter is for (coverage run 39197274294).
async fn backoff(attempt: u32, max: Duration) {
    tokio::time::sleep(backoff_pause(attempt, max, random_u64())).await;
}

/// [`backoff`]'s arithmetic, with the random draw passed in.
fn backoff_pause(attempt: u32, max: Duration, draw: u64) -> Duration {
    let window = BACKOFF_UNIT
        .saturating_mul(1u32 << attempt.min(16))
        .min(max);
    let micros = u64::try_from(window.as_micros()).unwrap_or(u64::MAX);
    Duration::from_micros(draw % micros.saturating_add(1))
}

/// A random number from the operating system. When that fails, the clock's
/// noise, which still differs between processes.
fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    match getrandom::getrandom(&mut b) {
        Ok(()) => u64::from_le_bytes(b),
        Err(_) => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::from(d.subsec_nanos()))
            .unwrap_or(0),
    }
}

async fn simple_rows(client: &Client, sql: &str) -> Result<Vec<TextRow>, PoolError> {
    let msgs = client.simple_query(sql).await.map_err(|e| {
        // The driver's Display is "db error"; the server's message is in
        // the source. Swallowing it turns every failure into a shrug. The
        // SQLSTATE picks the variant (`PoolError::from_driver`), and the
        // driver error itself is kept as the cause.
        let detail = std::error::Error::source(&e)
            .map(|s| s.to_string())
            .unwrap_or_else(|| e.to_string());
        PoolError::from_driver(e, detail)
    })?;
    Ok(msgs
        .into_iter()
        .filter_map(|m| match m {
            SimpleQueryMessage::Row(r) => Some(
                (0..r.len())
                    .map(|i| r.get(i).map(String::from))
                    .collect::<TextRow>(),
            ),
            _ => None,
        })
        .collect())
}

/// An open transaction owning its connection.
pub struct TxConn {
    client: Option<Client>,
    pool: Pool,
    _permit: Permit,
}

impl TxConn {
    pub async fn rows(&self, sql: &str) -> Result<Vec<TextRow>, PoolError> {
        self._permit.statement(sql);
        let out = simple_rows(self.client.as_ref().expect("live until consumed"), sql).await;
        self._permit.answered();
        out
    }

    /// Commit and return the connection to the pool.
    pub async fn commit(mut self) -> Result<(), PoolError> {
        let client = self.client.take().expect("live until consumed");
        simple_rows(&client, "COMMIT").await?;
        self.pool.give_back(client, &self._permit).await;
        Ok(())
    }

    /// Explicit rollback — same connection-return path as commit. The
    /// implicit path (drop) also aborts, by discarding the connection.
    pub async fn rollback(mut self) -> Result<(), PoolError> {
        let client = self.client.take().expect("live until consumed");
        simple_rows(&client, "ROLLBACK").await?;
        self.pool.give_back(client, &self._permit).await;
        Ok(())
    }
}

// Drop: the client (if still held) is simply dropped — its socket closes,
// the server aborts the transaction, and the permit frees a slot the next
// checkout will re-dial. No async work is smuggled into Drop.

// ── the read executor ────────────────────────────────────────────────────

/// Compile-and-run: the read half of F5. A call source (`openTasks`,
/// `customerByEmail($e: "…")`) binds against the schema with the caller in
/// the guard slot, plans to SQL, executes on a pooled connection, and
/// decodes to canonical JSON by the plan's declared output types.
///
/// The venue gate lives here, at the one place a plan becomes work. A plan
/// that returns a sealed value is served when its host work is bounded: the
/// rows are fetched as stored, one read release is taken for the page, and
/// the page's envelopes open here ([`Executor::venue`], `open_sealed`). A
/// plan whose host work grows with the data is refused, naming the sealed
/// fields. No caller path reaches the database around this gate.
pub struct Executor {
    pub pool: Pool,
    pub schema: ordo_schema::Schema,
    pub registry: ordo_sema::Registry,
    /// Key custody, injected. Nothing here constructs a provider: F0's
    /// done-when is that a provider returning plaintext cannot be selected
    /// outside unit tests, and the only way to mean that is for the
    /// executor to be handed one.
    pub keys: std::sync::Arc<dyn ordo_crypto::KeyProvider>,
    /// The space this executor serves. Keys are per space, so this is the
    /// isolation boundary rather than a label.
    pub space: ordo_crypto::SpaceId,
}

impl Executor {
    /// The space every statement this executor plans is confined to.
    ///
    /// It comes from the field the door set, never from the query and never
    /// re-derived: "a re-derived space is the cross-space read the check
    /// exists to prevent" (`engine-plan.md` F4). Folded to a literal because
    /// this executor runs planned SQL as written rather than binding
    /// parameters.
    pub fn plan_space(&self) -> ordo_plan::Space {
        ordo_plan::Space::Bound(self.space.0.clone())
    }

    /// The chain this executor's writes join: its own space's, from the
    /// field, never from an argument, for the reason [`Executor::plan_space`]
    /// reads the field.
    fn chain_space(&self) -> ordo_commandlog::SpaceId {
        ordo_commandlog::SpaceId(self.space.0.clone())
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum ExecFail {
    /// Compilation refused it — the message carries the closed error class.
    Compile(String),
    /// The plan's venue is not servable by this executor: an `unbounded-host`
    /// read, a whole read with no proven bound, or an anonymous sealed read.
    /// The message names the sealed field.
    Venue(String),
    Pool(PoolError),
}

impl ExecFail {
    /// The database's error, when that is what failed.
    pub fn pool_error(&self) -> Option<&PoolError> {
        match self {
            ExecFail::Pool(e) => Some(e),
            ExecFail::Compile(_) | ExecFail::Venue(_) => None,
        }
    }

    /// Was the pool full? Read from [`PoolError::is_busy`], never the text.
    pub fn is_busy(&self) -> bool {
        self.pool_error().is_some_and(PoolError::is_busy)
    }
}

impl std::error::Error for ExecFail {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.pool_error()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

impl std::fmt::Display for ExecFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecFail::Compile(m) | ExecFail::Venue(m) => f.write_str(m),
            ExecFail::Pool(e) => write!(f, "{e}"),
        }
    }
}

impl Executor {
    /// Run a read as a caller. `None` is anonymous — every guard false.
    pub async fn read_as(
        &self,
        call: &str,
        caller: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, ExecFail> {
        let pl = ordo_syntax::parse_pipeline(call)
            .map_err(|e| ExecFail::Compile(format!("parse: {} at {}", e.message, e.span)))?;
        let q = ordo_sema::bind_as(
            &self.schema,
            &self.registry,
            &pl,
            &[],
            caller,
            // The request's own execution instant. This path is what a
            // caller reaches, and it was pinned to the conformance corpus's
            // date — so `since 30d` was a window anchored to 2026-05-01 and
            // widening by a day every day, in the one feature whose whole
            // point is not being stale.
            ordo_sema::Clock::now(),
        )
        .map_err(|e| ExecFail::Compile(format!("bind: {e}")))?;
        // The venue gate, at the only point that matters. This path answers
        // the whole result at once, so only a bound proven from the plan
        // itself (`take n`, a unique address) bounds its host work.
        self.venue(&q, call, caller, Read::Whole)?;
        let mut q = q;
        ordo_plan::blind::resolve(&mut q, self.keys.as_ref(), &self.space, &[])
            .map_err(|e| ExecFail::Compile(e.to_string()))?;
        // The anchor's id rides every row (`ui-gen.md` §1, C4e). Attached as a
        // real projection BEFORE planning, so the paged path, this unpaged
        // path, the SQL and the manifest all see one set of outputs. A read
        // whose rows have no identity (grouped, a measure, `distinct`, a
        // many-side join) gets nothing, which is the same answer
        // `syncability` gives the door.
        let _ = ordo_plan::artifact::project_row_key(&mut q);
        let plan = ordo_plan::plan(&q, &self.plan_space())
            .map_err(|e| ExecFail::Compile(format!("plan: {e}")))?;
        let mut rows = self.pool.rows(&plan.sql).await.map_err(ExecFail::Pool)?;
        let correlation = ordo_secevent::correlation::mint();
        open_sealed(
            &mut rows,
            &plan,
            self.keys.as_ref(),
            &self.space,
            &correlation.0,
        )?;
        Ok(canonical(&rows, &plan.outputs))
    }

    /// The venue gate: which reads this executor serves.
    ///
    /// - `sql` and `blind-index` plans are served as they always were.
    /// - A `bounded-host` plan is served. Its rows are fetched by plain SQL,
    ///   with the guard deciding which, and the envelopes they carry open
    ///   here under one read release per page (`open_sealed`). Read
    ///   [`Read::Whole`], it is served only when the plan proves its own
    ///   bound; the credit a page gives does not hold for a whole read.
    /// - An `unbounded-host` plan is refused. The refusal names the sealed
    ///   fields and the fix.
    /// - An anonymous caller is refused any plan that would open a sealed
    ///   value. Every guard is false for them, so the rows they could reach
    ///   are unguarded ones, and an unguarded sealed field is still sealed.
    ///
    /// The class is the deploy-time one, with the schema, so the gate and
    /// the catalog agree on what a read costs.
    fn venue(
        &self,
        q: &ordo_sema::ir::Query,
        call: &str,
        caller: Option<&str>,
        read: Read,
    ) -> Result<(), ExecFail> {
        use ordo_plan::cost::{classify_with, row_bound, CostClass, RowBound};
        let class = classify_with(q, Some(&self.schema));
        if matches!(class, CostClass::Sql | CostClass::BlindIndex) {
            return Ok(());
        }
        let fields = sealed_fields(q);
        if caller.is_none() {
            return Err(ExecFail::Venue(format!(
                "sealed_unsupported: `{call}` returns sealed {fields}, and an anonymous \
                 caller never opens a sealed value; call it as a signed-in member"
            )));
        }
        let proven = matches!(row_bound(q, &self.schema), RowBound::AtMost(_));
        match (class, read) {
            (CostClass::BoundedHost, Read::Paged) => Ok(()),
            (CostClass::BoundedHost, Read::Whole) if proven => Ok(()),
            (CostClass::BoundedHost, Read::Whole) => Err(ExecFail::Venue(format!(
                "sealed_unsupported: `{call}` returns sealed {fields}, and only a page bounds \
                 how many it opens; read it a page at a time, or bound it with `take n`"
            ))),
            _ => Err(ExecFail::Venue(format!(
                "sealed_unsupported: `{call}` returns sealed {fields} and plans as {}: the \
                 payloads it opens grow with the data; bound it with `take n`, or make each \
                 opened row a row of the page (no group, distinct or many-side join over it)",
                class.as_str()
            ))),
        }
    }
}

/// How a read is answered, which decides what bounds its host work.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Read {
    /// One page of a total order: the page bounds the payloads it opens.
    Paged,
    /// The whole result at once.
    Whole,
}

/// The sealed fields a plan returns, as `` `entity.field` ``, for a refusal
/// to name. Every level of the plan is read, so a sealed value carried up
/// through a sub-select or a join is named too.
fn sealed_fields(q: &ordo_sema::ir::Query) -> String {
    let out = sealed_field_list(q);
    match out.len() {
        0 => "a field".to_string(),
        1 => format!("field {}", out[0]),
        _ => format!("fields {}", out.join(", ")),
    }
}

/// Every `` `entity.field` `` any level of the plan hands to the host.
fn sealed_field_list(q: &ordo_sema::ir::Query) -> Vec<String> {
    use ordo_sema::ir::From;
    fn walk(q: &ordo_sema::ir::Query, out: &mut Vec<String>) {
        for (_, entity, field) in &q.host_open {
            let name = format!("`{entity}.{field}`");
            if !out.contains(&name) {
                out.push(name);
            }
        }
        for c in &q.ctes {
            walk(&c.query, out);
        }
        match &q.from {
            From::Sub { query, .. } => walk(query, out),
            From::Union { arms, .. } => arms.iter().for_each(|a| walk(a, out)),
            _ => {}
        }
        for j in &q.source_joins {
            if let From::Sub { query, .. } = &j.from {
                walk(query, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(q, &mut out);
    out
}

/// Open every sealed envelope the plan marked, in place.
///
/// This is the host half of a host-class plan: the database handed over
/// ciphertext, and the plaintext exists only here, for as long as the
/// response takes to build. Each envelope opens under the entity and field
/// it was sealed with, so a payload that somehow arrived from another
/// column refuses rather than decrypting into the wrong name — and a
/// refusal is a hard error, never a null, because a silently empty sealed
/// value reads as "no data" when it means "we could not open your data".
///
/// **One release per call, and none when nothing opens.** The caller hands
/// over one page of rows, so a page costs one `Purpose::Read` release for
/// this space under the request's correlation, and a metered provider
/// records it as one `KeyReleased` event. The release is taken at the first
/// envelope that actually has to open: a page the guard emptied, or whose
/// sealed cells are all null or empty, releases nothing. So a caller who may
/// read no row never causes a key to be released for them.
fn open_sealed(
    rows: &mut [TextRow],
    plan: &ordo_plan::Plan,
    keys: &dyn ordo_crypto::KeyProvider,
    space: &ordo_crypto::SpaceId,
    correlation: &str,
) -> Result<(), ExecFail> {
    if plan.host_open.is_empty() {
        return Ok(());
    }
    // Opening is a key release, and a READ one: the narrow lease that can
    // open a payload and cannot seal one. Always this executor's own space,
    // so no other tenant's key is ever asked for.
    let mut lease: Option<ordo_crypto::KeyLease> = None;
    for (name, entity, field) in &plan.host_open {
        let Some(i) = plan.outputs.iter().position(|(o, _)| o == name) else {
            continue;
        };
        for row in rows.iter_mut() {
            let Some(cell) = row.get_mut(i) else { continue };
            let Some(hex) = cell.as_deref() else { continue };
            // Postgres renders bytea as `\x…` in text mode.
            let hex = hex.strip_prefix("\\x").unwrap_or(hex);
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .filter_map(|k| u8::from_str_radix(hex.get(k..k + 2)?, 16).ok())
                .collect();
            // The planner packs the id of the row the envelope was read
            // from beside it, and the envelope opens only at that row.
            let Some((id, envelope)) = ordo_plan::split_anchored(&bytes) else {
                return Err(ExecFail::Compile(format!(
                    "sealed_open_failed: `{entity}.{field}` arrived without the row it was read from"
                )));
            };
            // An empty payload is an empty value, not a failed open: the
            // seed's `''::bytea` memo is a real, empty memo.
            if envelope.is_empty() {
                *cell = Some(String::new());
                continue;
            }
            if lease.is_none() {
                lease = Some(
                    keys.release(space, ordo_crypto::Purpose::Read, correlation)
                        .map_err(|e| ExecFail::Compile(format!("key release: {e}")))?,
                );
            }
            let lease = lease.as_ref().expect("taken just above");
            let plain = keys
                .open_at(lease, entity, field, ordo_crypto::Anchor::Row(id), envelope)
                .map_err(|e| {
                    if e.is_unreadable() {
                        // The key is present and the envelope will not
                        // open: unreadable, possibly tampered, and not
                        // retryable. Names the field, never the value.
                        ExecFail::Compile(format!(
                            "sealed_unreadable: `{entity}.{field}` is unreadable, possibly \
                             tampered: its key is present and the stored value did not open"
                        ))
                    } else {
                        ExecFail::Compile(format!(
                            "sealed_open_failed: `{entity}.{field}` did not open — its key \
                             is not available to this read"
                        ))
                    }
                })?;
            *cell = Some(String::from_utf8_lossy(&plain).into_owned());
        }
    }
    Ok(())
}

/// Decode text rows against the plan's declared output types: the wire
/// shape (`api-engine-design.md` §5).
///
/// - `int` and `duration` are JSON integers.
/// - `money(p)` is an exact decimal **string in major units** with exactly
///   `p` places: the column's `2999` is `"29.99"` at places 2 and `"2999"`
///   at places 0 (27ak). It used to be the JSON integer of minor units,
///   which disagreed with §5 and with the generated SDK, which has always
///   typed money as a string.
/// - `decimal` is Postgres's own spelling of the `numeric`, which carries
///   the column's scale.
/// - `bool` is a real boolean.
/// - `json` is the document itself, never a string holding it (28b).
/// - Everything else is its text.
///
/// The conformance harness keeps its own decoder over stored units
/// (`ordo_conformance::canonical`), because its cases assert what the
/// database holds.
pub fn canonical(
    rows: &[TextRow],
    outputs: &[(String, ordo_syntax::ScalarTy)],
) -> Vec<serde_json::Value> {
    use ordo_syntax::ScalarTy;
    rows.iter()
        .map(|r| {
            let mut obj = serde_json::Map::new();
            for (i, (name, ty)) in outputs.iter().enumerate() {
                let cell = r.get(i).and_then(|c| c.clone());
                let v = match (cell, ty) {
                    (None, _) => serde_json::Value::Null,
                    (Some(s), ScalarTy::Money(p)) => match s.parse::<i128>() {
                        Ok(minor) => {
                            serde_json::json!(ordo_syntax::numeric::major_spelling(minor, *p))
                        }
                        Err(_) => serde_json::json!(s),
                    },
                    (Some(s), ScalarTy::Int | ScalarTy::Duration(_)) => match s.parse::<i64>() {
                        Ok(n) => serde_json::json!(n),
                        Err(_) => serde_json::json!(s),
                    },
                    (Some(s), ScalarTy::Bool) => serde_json::json!(s == "t" || s == "true"),
                    // The stored document, as JSON (28b): Postgres's print
                    // of the `jsonb`, or a sealed field's opened text. An
                    // empty sealed payload is no document.
                    (Some(s), ScalarTy::Json) if s.is_empty() => serde_json::Value::Null,
                    (Some(s), ScalarTy::Json) => {
                        serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s))
                    }
                    (Some(s), _) => serde_json::json!(s),
                };
                obj.insert(name.clone(), v);
            }
            serde_json::Value::Object(obj)
        })
        .collect()
}

// ── the transactional write seam (AA-004's closure) ──────────────────────

/// An open transaction presented to the **synchronous** write protocol.
///
/// The protocol is a sequence of decisions interleaved with queries —
/// lock the row, evaluate its guards, image it, mutate it — and it is
/// only sound when all of that shares one snapshot. That shape is
/// inherently sequential, so rather than colouring the whole protocol
/// async, the executor runs it on a blocking thread and bridges each
/// query back onto the runtime through a [`tokio::runtime::Handle`].
///
/// `block_on` is legal here and nowhere else: this type is only ever
/// constructed inside [`tokio::task::spawn_blocking`], whose threads are
/// not runtime workers. Constructing one on a worker thread and calling
/// it would deadlock the runtime, which is why [`Executor::write_as`] is
/// the only way to get one.
pub struct BlockingTx {
    tx: TxConn,
    handle: tokio::runtime::Handle,
    /// Set when a statement failed with `40001`/`40P01`.
    ///
    /// The protocol's error channel is `Result<_, String>`, so a
    /// serialization failure reaches the caller as prose and is
    /// indistinguishable from "your guard refused you" — which would make
    /// the retry loop either sniff error text or never fire. The flag is
    /// how the *class* survives the string boundary: the protocol still
    /// aborts the way it aborts for any failure, and the layer that decides
    /// whether to retry reads a bool rather than a message.
    conflict: Arc<std::sync::atomic::AtomicBool>,
    /// Set when a statement was refused by a foreign key or a unique
    /// constraint (`PoolError::is_foreign_key_violation`,
    /// `is_unique_violation`), for the same reason as `conflict`: a release
    /// rule reads it to tell "rows still stand in the way" from an outage.
    constraint: std::sync::atomic::AtomicBool,
}

impl ordo_conformance::write::WriteDb for BlockingTx {
    fn rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        self.handle.block_on(self.tx.rows(sql)).map_err(|e| {
            if e.is_retryable() {
                self.conflict
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if e.is_foreign_key_violation() || e.is_unique_violation() {
                self.constraint
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            e.to_string()
        })
    }

    fn refused_by_constraint(&self) -> bool {
        self.constraint.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Executor {
    /// Run a write op as a caller, with the ENTIRE protocol —
    /// preconditions, `FOR UPDATE` locks, mutations — inside one
    /// transaction that commits on success and aborts on any refusal.
    pub async fn write_as(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        call: String,
        caller: String,
    ) -> Result<serde_json::Value, ExecFail> {
        self.write_leased(fixture, log, call, caller, None)
            .await
            .map(|o| match o {
                Some(v) => v,
                // Unreachable without a lease.
                None => serde_json::Value::Array(Vec::new()),
            })
    }

    /// The write with its replay lease. `Ok(None)` means the lease's effect
    /// had already committed — the work is done, and re-running it is
    /// exactly what the key exists to prevent.
    ///
    /// Runs at SERIALIZABLE, retried on `40001`. See
    /// [`Executor::write_attempt`] for what one attempt is and
    /// [`Pool::retrying`] for why the whole group is the retry boundary.
    pub async fn write_leased(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        call: String,
        caller: String,
        lease: Option<(String, String)>,
    ) -> Result<Option<serde_json::Value>, ExecFail> {
        self.write_retrying(fixture, log, call, caller, None, lease, None)
            .await
    }

    /// [`Executor::write_leased`], with the command group appended to this
    /// executor's own space's durable chain inside the write's transaction.
    ///
    /// What a deployment calls. The effect and its link commit together or
    /// not at all, so a write that committed is a write the chain holds.
    /// [`LoggedExecutor`] does the same for a caller holding one, with the
    /// chain's space as a separate field; this takes it from the executor,
    /// so the space the rows were written in and the chain they are recorded
    /// in cannot be two different values.
    pub async fn write_chained(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        call: String,
        caller: String,
        lease: Option<(String, String)>,
    ) -> Result<Option<serde_json::Value>, ExecFail> {
        self.write_chained_as(fixture, log, call, caller, None, lease)
            .await
    }

    /// [`Executor::write_chained`] for a call made by `agent` on behalf of
    /// `caller` under a delegation. `caller` is still the guard input; the
    /// command group names both (`agent:<agent> on behalf of
    /// user:<caller>`). `agent: None` is exactly `write_chained`.
    pub async fn write_chained_as(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        call: String,
        caller: String,
        agent: Option<String>,
        lease: Option<(String, String)>,
    ) -> Result<Option<serde_json::Value>, ExecFail> {
        self.write_retrying(
            fixture,
            log,
            call,
            caller,
            agent,
            lease,
            Some(self.chain_space()),
        )
        .await
    }

    /// **One import batch**: N rows, one transaction, one command group,
    /// one chain append.
    ///
    /// The whole of [`Executor::attempt`]'s machinery applies unchanged —
    /// SERIALIZABLE, the in-transaction lease, the invalidation enqueue,
    /// the append held until the last statement before commit. What is
    /// different is `work`: instead of one op call it runs N, against one
    /// `WriteLog` with a **batch open**, so every call extends the pending
    /// group rather than replacing it and the chain gets one entry for the
    /// batch (`ordo_conformance::WriteLog::open_batch`,
    /// `docs/plan-import.md` §3).
    ///
    /// `work` is `Fn + Clone` rather than `FnOnce` because the retry
    /// boundary is the whole group, re-executed from scratch: an attempt
    /// that hit `40001` built its group against a snapshot that no longer
    /// exists, and the next attempt has to build a new one rather than
    /// re-use it. Two spaces writing at the same instant share
    /// `$commandlog` and raise `40001` between them
    /// (`tests/spaces.rs::two_spaces_append_at_once`), so a batch that
    /// could not retry would refuse a good import because a different
    /// tenant wrote.
    pub async fn import_batch<W>(
        &self,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        lease: Option<(String, String)>,
        correlation: String,
        work: W,
    ) -> Result<Option<serde_json::Value>, ExecFail>
    where
        W: Fn(&BlockingTx, &mut ordo_conformance::WriteLog) -> Result<serde_json::Value, String>
            + Clone
            + Send
            + 'static,
    {
        log.lock()
            .expect("write log")
            .open_batch(correlation.clone());
        let attempts = self.pool.write_attempts();
        let mut last = None;
        for attempt in 1..=attempts {
            let w = work.clone();
            let out = self
                .attempt(
                    log.clone(),
                    lease.clone(),
                    Some(self.chain_space()),
                    move |db, wlog| w(db, wlog),
                )
                .await;
            match out {
                Err(ExecFail::Pool(e)) if e.is_retryable() => {
                    last = Some(e);
                    self.pool.lost_race(&self.space.0, attempt, attempts).await;
                }
                other => {
                    log.lock().expect("write log").close_batch();
                    return other;
                }
            }
        }
        log.lock().expect("write log").close_batch();
        Err(ExecFail::Pool(PoolError::exhausted(attempts, last)))
    }

    /// The retry loop around [`Executor::write_attempt`].
    ///
    /// `implementation.md` pins the boundary: "the retry boundary is the
    /// **whole command group**, re-executed from scratch". Not a statement,
    /// not the transaction with its work preserved — the group is rebuilt,
    /// because it was built *inside* the transaction that just failed and
    /// its `before` images describe a snapshot that no longer exists.
    /// Re-appending a group computed against a vanished snapshot would put
    /// a false before/after pair into the audit spine, which is a worse
    /// outcome than the conflict.
    ///
    /// Each attempt therefore drops its pending group, re-claims its lease
    /// inside its own fresh transaction, and re-runs the protocol.
    // The agent rides beside the caller (delegation, increment 2).
    #[allow(clippy::too_many_arguments)]
    async fn write_retrying(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        call: String,
        caller: String,
        agent: Option<String>,
        lease: Option<(String, String)>,
        append_to: Option<ordo_commandlog::SpaceId>,
    ) -> Result<Option<serde_json::Value>, ExecFail> {
        let attempts = self.pool.write_attempts();
        let mut last = None;
        for attempt in 1..=attempts {
            let out = self
                .write_attempt(
                    fixture.clone(),
                    log.clone(),
                    call.clone(),
                    caller.clone(),
                    agent.clone(),
                    lease.clone(),
                    append_to.clone(),
                )
                .await;
            match out {
                Err(ExecFail::Pool(e)) if e.is_retryable() => {
                    last = Some(e);
                    self.pool.lost_race(&self.space.0, attempt, attempts).await;
                }
                other => return other,
            }
        }
        Err(ExecFail::Pool(PoolError::exhausted(attempts, last)))
    }

    /// One attempt: one transaction, from `BEGIN` to `COMMIT` or nothing.
    ///
    /// `append_to`, when present, is the space whose chain this write's
    /// command group joins — **inside this transaction**, before the
    /// commit. That is the difference between 8f's ordering guarantee and
    /// the real one: an aborted attempt leaves no link because the `INSERT`
    /// rolled back with it, not because a later step remembered to drop it.
    // The agent rides beside the caller (delegation, increment 2).
    #[allow(clippy::too_many_arguments)]
    async fn write_attempt(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        call: String,
        caller: String,
        agent: Option<String>,
        lease: Option<(String, String)>,
        append_to: Option<ordo_commandlog::SpaceId>,
    ) -> Result<Option<serde_json::Value>, ExecFail> {
        self.attempt(log, lease, append_to, move |db, log| {
            log.act_for(agent);
            fixture.run_write_on(db, log, &call, Some(&caller))
        })
        .await
    }

    /// One attempt at whatever the write protocol was asked to do.
    ///
    /// The body used to be inlined in [`Executor::write_attempt`] and is
    /// parameterised over `work` because there are now two protocol entry
    /// points that need every one of these guarantees: an op call
    /// (`run_write_on`) and a blob upload
    /// (`run_platform_converge`). Copying the lease claim, the conflict
    /// re-typing, the invalidation enqueue and the in-transaction append
    /// into a second function would be two implementations of "a write is
    /// one transaction", and the second would be the one that drifts.
    async fn attempt<W>(
        &self,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        lease: Option<(String, String)>,
        append_to: Option<ordo_commandlog::SpaceId>,
        work: W,
    ) -> Result<Option<serde_json::Value>, ExecFail>
    where
        W: FnOnce(
                &BlockingTx,
                &mut ordo_conformance::WriteLog,
            ) -> Result<serde_json::Value, String>
            + Send
            + 'static,
    {
        // This attempt's own log: it mints from the shared sequence and holds
        // its own pending group, so the shared log is never held across the
        // protocol's database I/O (17f-dl). Two writers to one row used to
        // take the shared mutex and the row lock in opposite orders, and a
        // blocked `lock()` on the async side stalled the runtime with them.
        let mut scratch = log.lock().expect("write log").attempt();
        // SERIALIZABLE: the write protocol is preconditions, then locks,
        // then mutations — a command group with more than one statement,
        // which is the row `implementation.md`'s isolation table pins at
        // SERIALIZABLE, and the mixed read-then-write shape it names as
        // exactly the anomaly READ COMMITTED admits.
        let tx = self
            .pool
            .begin_for(OpClass::MultiStatementGroup)
            .await
            .map_err(ExecFail::Pool)?;
        // Read the journal INSIDE the transaction that will write the
        // effect: checking outside it would reintroduce the very window
        // the lease exists to close.
        let mut generation = 0i64;
        if let Some((key, fp)) = &lease {
            let (state, g) = tx.claim_lease(key, fp).await.map_err(ExecFail::Pool)?;
            generation = g;
            match state {
                LeaseState::Fresh => {}
                LeaseState::Applied => {
                    let _ = tx.rollback().await;
                    return Ok(None);
                }
                LeaseState::Mismatch => {
                    let _ = tx.rollback().await;
                    return Err(ExecFail::Compile(
                        "idempotency_mismatch: this key was used for a different request".into(),
                    ));
                }
            }
        }
        let handle = tokio::runtime::Handle::current();
        // The shared log, for the post-commit append.
        let log_after = log;
        let conflict = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let conflict_seen = conflict.clone();
        let (result, tx, mut scratch) = tokio::task::spawn_blocking(move || {
            let db = BlockingTx {
                tx,
                handle,
                conflict,
                constraint: std::sync::atomic::AtomicBool::new(false),
            };
            let out = work(&db, &mut scratch);
            (out, db.tx, scratch)
        })
        .await
        .map_err(|e| ExecFail::Compile(format!("write task: {e}")))?;
        // A refusal that was really a serialization failure is re-typed
        // here, before it reaches the caller as a compile-shaped message.
        // Recovering the class at this seam rather than at the door is the
        // difference between a retryable conflict and a caller being told
        // their perfectly good write is malformed.
        if result.is_err() && conflict_seen.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = tx.rollback().await;
            return Err(ExecFail::Pool(PoolError::Serialization(
                result.err().unwrap_or_default().into(),
            )));
        }
        match result {
            Ok(v) => {
                // The completion record joins the effect in this
                // transaction. Commit publishes both or neither.
                // Invalidation and deferred work ride the SAME transaction
                // as the effect, which is the whole of "fires only on
                // commit": a rollback below un-enqueues them, and a commit
                // publishes work and effect in one instant. The signal
                // names the entity, never the rows — subscribers refetch
                // through their own guarded read.
                // The summary names what an op wrote. A routine's run answers
                // a summary of its own (`{rows, more}`), so the command group
                // the write built is read too: without it a routine's writes
                // reached no live reader (notes v2's reminders, 2026-10-22).
                // Only when the summary named nothing: an op's own summary
                // already names its entities, and the group also holds rows
                // an op never answers for, such as an acl create's access
                // row, which no reader subscribes to.
                let mut entities = touched(&v);
                if entities.is_empty() {
                    if let Some(group) = scratch.pending() {
                        for c in &group.commands {
                            if !entities.contains(&c.entity) {
                                entities.push(c.entity.clone());
                            }
                        }
                    }
                }
                for entity in entities {
                    tx.enqueue("invalidate", &entity)
                        .await
                        .map_err(ExecFail::Pool)?;
                    tx.notify("ordo.invalidate", &entity)
                        .await
                        .map_err(ExecFail::Pool)?;
                }
                if let Some((key, _)) = &lease {
                    if !tx
                        .complete_lease(key, generation)
                        .await
                        .map_err(ExecFail::Pool)?
                    {
                        // Fenced: another holder took this lease over while
                        // we worked. Our effect must not commit — it would
                        // be work nobody can attribute, under a key someone
                        // else now owns.
                        let _ = tx.rollback().await;
                        return Err(ExecFail::Compile(format!(
                            "lease_fenced: `{key}` was taken over by a newer holder; \
                             this attempt's work was rolled back"
                        )));
                    }
                }
                // The durable append, INSIDE the transaction that produced
                // the effect and under the per-space chain lock. This is
                // the last statement before commit — the lock is held for
                // the hash and the insert and nothing else, which is
                // `implementation.md`'s "taken as late as possible".
                // No pending group means the protocol built no command: a
                // bulk `set`/`del` whose condition described zero rows. That
                // is "the ordinary vacuous no-op" (`definition-language.md`
                // G11), and a no-op changes nothing, so it records nothing:
                // `CommandGroup::validate` refuses an empty group, and a
                // group in the chain asserts that a write happened.
                let pending = scratch.pending().cloned();
                if let (Some(space), Some(group)) = (&append_to, pending) {
                    // The executor serves one space; a group whose commands
                    // name a different one must not join this chain. The
                    // group already refuses to be mixed-space
                    // (`validate`), so this is the other half: single-space
                    // AND the space we are entitled to write.
                    let group_space = group
                        .validate()
                        .map_err(|e| ExecFail::Compile(format!("log_append_failed: {e}")))?;
                    if group_space != space {
                        let _ = tx.rollback().await;
                        return Err(ExecFail::Compile(format!(
                            "log_append_failed: this executor logs to `{space}`, the group is \
                             for `{group_space}`"
                        )));
                    }
                    if let Err(e) = tx.append_group(&group).await {
                        // A chain that will not take the group means the
                        // effect must not land either: an unrecorded write
                        // is exactly the hole the log exists to close.
                        let _ = tx.rollback().await;
                        // A serialization conflict here is retryable and
                        // must surface as one. Two spaces take different
                        // append locks and write disjoint rows, and they
                        // still share `$commandlog`, so SSI raises `40001`
                        // between them. Reported as `Compile` it reaches
                        // the caller as a permanent failure and
                        // `write_retrying` never sees it, which refuses one
                        // tenant's good write because a different tenant
                        // wrote at the same instant. Found by
                        // `ordo-exec/tests/spaces.rs::two_spaces_append_at_once`.
                        return Err(match e {
                            ordo_commandlog::AppendError::Retryable(m) => {
                                ExecFail::Pool(PoolError::Serialization(m.into()))
                            }
                            other => ExecFail::Compile(format!("log_append_failed: {other}")),
                        });
                    }
                }
                tx.commit().await.map_err(ExecFail::Pool)?;
                // Committed — only NOW does the group enter the process's
                // in-memory mirror. The durable link above landed with the
                // commit itself; this one still rides the ordering, which
                // is all an in-memory store can offer (8f).
                log_after
                    .lock()
                    .expect("write log")
                    .commit_attempt(&mut scratch)
                    .map_err(ExecFail::Compile)?;
                Ok(Some(v))
            }
            // Every refusal aborts the whole group — a create whose acl
            // bootstrap fails leaves no orphan root behind, and no
            // completion is recorded for a key whose work did not happen.
            Err(e) => {
                let _ = tx.rollback().await;
                Err(ExecFail::Compile(e))
            }
        }
    }

    /// A blob upload, run as the converge it is.
    ///
    /// The whole of [`Executor::attempt`]'s machinery applies: SERIALIZABLE,
    /// the invalidation enqueue, the group held back until the transaction
    /// commits. What the door hands in is a token it minted from bytes the
    /// pipeline already sealed and stored, and the token is consumed here,
    /// which is why the signature takes it by value and is generic: a
    /// private type may implement a public trait as long as it is never
    /// named in a public signature.
    ///
    /// **One attempt, not the retry loop**, and the reason is the move. The
    /// retry boundary for an op call is the whole command group,
    /// re-executed from scratch, and re-executing means calling the
    /// protocol a second time. A platform value cannot be handed over
    /// twice: `run_platform_converge` consumes it, and the only way to
    /// produce another is another upload of the same bytes. Keeping a
    /// re-usable copy inside this crate to feed a second attempt would put
    /// "one upload, two converges" back inside the boundary that exists to
    /// forbid it. So a `40001` here surfaces as a retryable
    /// [`PoolError::Serialization`] and the caller decides; nothing is
    /// committed and nothing is appended.
    pub async fn attach_write<V>(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        value: V,
        caller: String,
        agent: Option<String>,
    ) -> Result<serde_json::Value, ExecFail>
    where
        V: ordo_conformance::write::PlatformValue + Send + 'static,
    {
        // Appended to this executor's own chain, always. Its one caller is
        // the blob door, and an upload is revertible only because the revert
        // applier finds its group in the durable chain.
        self.attempt(log, None, Some(self.chain_space()), move |db, log| {
            log.act_for(agent);
            fixture.run_platform_converge(db, log, value, &caller)
        })
        .await
        .map(|o| o.unwrap_or_else(|| serde_json::Value::Array(Vec::new())))
    }

    /// A platform command that **decides and writes in one transaction**:
    /// a member leaving a tenant (checklist 27aq) locks the seat, evaluates
    /// the program's release rule, and then writes through
    /// `Fixture::run_release_on`, all against the one [`BlockingTx`].
    ///
    /// The whole of [`Executor::attempt`]'s machinery applies: SERIALIZABLE,
    /// the invalidation enqueue for every entity the summary names, the
    /// group appended to this executor's own chain inside the transaction,
    /// and the in-memory mirror only after commit. `work` that returns an
    /// error rolls everything back and appends nothing; `work` that returns
    /// `Ok` must have staged a group, or the append refuses it, because an
    /// effect with no record is what the chain exists to rule out.
    ///
    /// `work` reads through the transaction as it likes. It writes entity
    /// rows only through the protocol's entry points, which is what
    /// `only_the_write_protocol_updates_entity_rows` holds every source file
    /// to. One attempt, no retry: a `40001` surfaces as a retryable
    /// [`PoolError::Serialization`] and the caller decides, as
    /// [`Executor::attach_write`] does.
    pub async fn platform_write<W>(
        &self,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        work: W,
    ) -> Result<serde_json::Value, ExecFail>
    where
        W: FnOnce(
                &dyn ordo_conformance::write::WriteDb,
                &mut ordo_conformance::WriteLog,
            ) -> Result<serde_json::Value, String>
            + Send
            + 'static,
    {
        self.attempt(log, None, Some(self.chain_space()), move |db, log| {
            work(db, log)
        })
        .await
        .map(|o| o.unwrap_or_else(|| serde_json::Value::Array(Vec::new())))
    }
}

/// An [`Executor`] whose writes also append to a **durable** chain, in the
/// transaction that produces them.
///
/// A wrapper rather than a field on `Executor`, deliberately: `Executor`'s
/// fields are constructed by literal in three crates, so growing one would
/// be a breaking change to callers who have not asked for a durable log —
/// and "durable audit log" is a deployment decision, not an executor
/// detail. Composition says the same thing without the churn: a deployment
/// that wants the chain wraps its executor, and one that does not keeps the
/// executor it has.
///
/// The reads are the executor's; only the write path differs.
pub struct LoggedExecutor {
    pub exec: Executor,
    /// The space whose chain these writes join. Command groups are
    /// single-space by construction and this is the one this executor is
    /// entitled to write — a group naming another space is refused rather
    /// than logged.
    pub space: ordo_commandlog::SpaceId,
}

impl LoggedExecutor {
    pub fn new(exec: Executor, space: impl Into<String>) -> LoggedExecutor {
        LoggedExecutor {
            exec,
            space: ordo_commandlog::SpaceId(space.into()),
        }
    }

    pub async fn write_as(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        call: String,
        caller: String,
    ) -> Result<serde_json::Value, ExecFail> {
        self.write_leased(fixture, log, call, caller, None)
            .await
            .map(|o| o.unwrap_or_else(|| serde_json::Value::Array(Vec::new())))
    }

    pub async fn write_leased(
        &self,
        fixture: std::sync::Arc<ordo_conformance::Fixture>,
        log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
        call: String,
        caller: String,
        lease: Option<(String, String)>,
    ) -> Result<Option<serde_json::Value>, ExecFail> {
        self.exec
            .write_retrying(
                fixture,
                log,
                call,
                caller,
                None,
                lease,
                Some(self.space.clone()),
            )
            .await
    }
}

// ── keyset pagination ────────────────────────────────────────────────────

/// One page: the decoded rows, and the key values that address the row
/// after the last one — `None` when the page was not full, which is the
/// only honest way to say "there is no next page" without a second query.
#[derive(Debug)]
pub struct Page {
    pub rows: Vec<serde_json::Value>,
    pub next: Option<Vec<Option<String>>>,
    /// The space's command-log position these rows were read at, taken in
    /// the same snapshot (`OpClass::SnapshotRead`). `None` when the position
    /// could not be read, which leaves the response pinned to the current
    /// head as before rather than failing a read that succeeded.
    pub position: Option<u64>,
}

impl Executor {
    /// Run one read statement, and read this space's command-log position in
    /// the same snapshot.
    ///
    /// Under READ COMMITTED each statement takes its own snapshot, so a
    /// position read after the rows is only an upper bound on their version:
    /// a write committing in between makes the answer claim a position its
    /// rows predate. One REPEATABLE READ, read-only transaction gives both
    /// statements one snapshot, and a read-only transaction at that level
    /// cannot fail to serialize.
    async fn rows_at_position(&self, sql: &str) -> Result<(Vec<TextRow>, Option<u64>), ExecFail> {
        let tx = self
            .pool
            .begin_for(OpClass::SnapshotRead)
            .await
            .map_err(ExecFail::Pool)?;
        let rows = match tx.rows(sql).await {
            Ok(rows) => rows,
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(ExecFail::Pool(e));
            }
        };
        let position = tx
            .rows(&format!(
                "SELECT COALESCE(max(\"idx\") + 1, 0) FROM \"$commandlog\" WHERE \"space\" = {}",
                sql_lit(&self.space.0)
            ))
            .await
            .ok()
            .and_then(|r| r.first().and_then(|row| row.first().cloned().flatten()))
            .and_then(|s| s.parse().ok());
        tx.commit().await.map_err(ExecFail::Pool)?;
        Ok((rows, position))
    }
}

impl Executor {
    /// Read one page of a total order. `after` is the previous page's
    /// cursor keys (the API layer authenticates and transports them; their
    /// meaning is the executor's, exactly as `page.rs` says).
    ///
    /// A page that opens sealed values releases under a correlation minted
    /// here. The door's path, [`Executor::read_page_pipeline`], is handed the
    /// request's own.
    pub async fn read_page_as(
        &self,
        call: &str,
        caller: Option<&str>,
        page_size: i64,
        after: Option<&[Option<String>]>,
    ) -> Result<Page, ExecFail> {
        let pl = ordo_syntax::parse_pipeline(call)
            .map_err(|e| ExecFail::Compile(format!("parse: {} at {}", e.message, e.span)))?;
        let correlation = ordo_secevent::correlation::mint();
        self.read_page_pipeline(&pl, &[], call, caller, page_size, after, &correlation.0)
            .await
    }

    /// [`Executor::read_page_as`] over a pipeline that is already built.
    ///
    /// The door's refinement arrives this way (`plan-refinement.md` section
    /// 4): the host builds the call, its conditions and its pick as an AST
    /// and hands it here, so no caller's value is ever spelled as source and
    /// re-parsed. `label` names the call in a refusal.
    ///
    /// `row_key` is the refinement's conditions on `$id`, the attached
    /// anchor id. The call cannot carry them, since the op's body cannot
    /// name an id that is attached after binding, so they are applied to
    /// the bound query here ([`ordo_plan::artifact::filter_row_key`]).
    ///
    /// A page that returns sealed values opens them under one
    /// `Purpose::Read` release, taken for this space under `correlation`,
    /// the request's own. A page that opens nothing releases nothing.
    #[allow(clippy::too_many_arguments)]
    pub async fn read_page_pipeline(
        &self,
        pl: &ordo_syntax::Pipeline,
        row_key: &[ordo_plan::artifact::RowKeyCond],
        label: &str,
        caller: Option<&str>,
        page_size: i64,
        after: Option<&[Option<String>]>,
        correlation: &str,
    ) -> Result<Page, ExecFail> {
        let call = label;
        let q = ordo_sema::bind_as(
            &self.schema,
            &self.registry,
            pl,
            &[],
            caller,
            // The request's own execution instant. This path is what a
            // caller reaches, and it was pinned to the conformance corpus's
            // date — so `since 30d` was a window anchored to 2026-05-01 and
            // widening by a day every day, in the one feature whose whole
            // point is not being stale.
            ordo_sema::Clock::now(),
        )
        .map_err(|e| ExecFail::Compile(format!("bind: {e}")))?;
        // The venue gate. A page bounds the payloads a projection opens, so a
        // `bounded-host` read is served here one page at a time.
        self.venue(&q, call, caller, Read::Paged)?;
        // A plan with no ordering at all is provably one row (a keyless
        // `reduce`) — the one legitimately empty tiebreak. It has no
        // cursor to seek by and needs none, so it answers whole rather
        // than failing at the door. A cursor presented against one is a
        // client error, not a shrug.
        // Same resolution the unpaged read does, and for the same reason:
        // the digest is computed here, after the door, behind a release.
        let mut q = q;
        ordo_plan::artifact::filter_row_key(&mut q, row_key)
            .map_err(|e| ExecFail::Compile(format!("bind: {e}")))?;
        ordo_plan::blind::resolve(&mut q, self.keys.as_ref(), &self.space, &[])
            .map_err(|e| ExecFail::Compile(e.to_string()))?;
        // The anchor's id rides every row (`ui-gen.md` §1, C4e). Attached as a
        // real projection BEFORE planning, so the paged path, this unpaged
        // path, the SQL and the manifest all see one set of outputs. A read
        // whose rows have no identity (grouped, a measure, `distinct`, a
        // many-side join) gets nothing, which is the same answer
        // `syncability` gives the door.
        let _ = ordo_plan::artifact::project_row_key(&mut q);
        let paged = match ordo_plan::plan_paged(&q, &self.plan_space(), page_size, after) {
            Ok(p) => p,
            Err(e) if e.code == "unpageable" => {
                if after.is_some() {
                    return Err(ExecFail::Compile(format!("invalid_cursor: {e}")));
                }
                // No page window here: the whole result is answered at once,
                // so host work needs a bound the plan itself proves.
                if !sealed_field_list(&q).is_empty() {
                    self.venue(&q, call, caller, Read::Whole)?;
                }
                let plan = ordo_plan::plan(&q, &self.plan_space())
                    .map_err(|e| ExecFail::Compile(format!("plan: {e}")))?;
                let (mut rows, position) = self.rows_at_position(&plan.sql).await?;
                open_sealed(
                    &mut rows,
                    &plan,
                    self.keys.as_ref(),
                    &self.space,
                    correlation,
                )?;
                return Ok(Page {
                    rows: canonical(&rows, &plan.outputs),
                    next: None,
                    position,
                });
            }
            Err(e) => return Err(ExecFail::Compile(format!("plan: {e}"))),
        };
        let (raw, position) = self.rows_at_position(&paged.sql).await?;
        // A full page means "there may be more" — except when the plan's own
        // `take n` already bounded the whole result and this page covered it.
        // Without that, `top3Orders` at page size 3 costs a second, empty
        // request to discover it is finished.
        // The count, not the ceiling: by the time a plan executes, `take
        // $n`'s parameter has folded to the literal the caller supplied
        // (`Binder::take_limit`), so an unfolded slot here means nothing
        // bound it and "this page covered the whole result" is not
        // something we may claim.
        let bounded_out = matches!(
            &q.limit,
            Some(ordo_sema::ir::Limit::Lit(n)) if raw.len() as i64 >= *n
        );
        let full = raw.len() as i64 == page_size && !bounded_out;

        // The cursor is minted from the LAST row's order keys — the ones
        // already projected, addressed by output position, plus any hidden
        // trailing columns for keys the projection never carried (W047).
        let next = if full {
            raw.last().map(|r| {
                let outs = paged.outputs.len();
                let hidden: Vec<Option<String>> = r[outs..].to_vec();
                let mut keys = Vec::new();
                let mut h = hidden.into_iter();
                for (i, name) in cursor_key_names(&q, &paged).into_iter().enumerate() {
                    match name {
                        Some(n) => keys.push(
                            paged
                                .outputs
                                .iter()
                                .position(|(o, _)| o == &n)
                                .and_then(|p| r.get(p).cloned())
                                .flatten(),
                        ),
                        None => keys.push(h.next().flatten()),
                    }
                    let _ = i;
                }
                keys
            })
        } else {
            None
        };
        // Hidden key columns are stripped before decoding: they are the
        // platform's bookkeeping, never part of the op's output shape.
        let mut trimmed: Vec<TextRow> = raw
            .into_iter()
            .map(|mut r| {
                r.truncate(paged.outputs.len());
                r
            })
            .collect();
        // The cursor was minted from the raw row above, so opening here
        // cannot disturb it — and a sealed value is never an order key,
        // because the database cannot sort what it cannot read.
        open_sealed(
            &mut trimmed,
            &ordo_plan::Plan {
                sql: String::new(),
                params: Vec::new(),
                outputs: paged.outputs.clone(),
                host_open: q.host_open.clone(),
            },
            self.keys.as_ref(),
            &self.space,
            correlation,
        )?;
        Ok(Page {
            rows: canonical(&trimmed, &paged.outputs),
            next,
            position,
        })
    }
}

/// For each order key, the output name carrying it, or `None` when it
/// rides as a hidden column. Mirrors `plan_paged`'s own decision.
fn cursor_key_names(q: &ordo_sema::ir::Query, paged: &ordo_plan::PagedPlan) -> Vec<Option<String>> {
    let _ = paged;
    ordo_plan::order_key_outputs(q)
}

// ── the durable replay journal (AA-004's third leg) ──────────────────────

/// The table recording which idempotency keys have a committed effect.
/// Created on demand; a deployment's migration story owns it for real.
pub const REPLAY_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$replay\" (\
  \"key\" text PRIMARY KEY, \
  \"fingerprint\" text NOT NULL, \
  \"generation\" bigint NOT NULL DEFAULT 1, \
  \"done\" boolean NOT NULL DEFAULT false\
)";

/// What the journal knew about a key, read inside the caller's transaction.
#[derive(Debug, PartialEq, Eq)]
pub enum LeaseState {
    /// No record: this attempt owns the work.
    Fresh,
    /// The effect already committed under an earlier attempt.
    Applied,
    /// The key exists with a different payload — the caller reused an
    /// idempotency key for a different request.
    Mismatch,
}

impl TxConn {
    /// Claim the key inside this transaction, taking a **fencing
    /// generation** with it (AA-005).
    ///
    /// A lease can outlive the process holding it: a node stalls, its
    /// reservation is taken over, and the original wakes up still believing
    /// it owns the work. Without a generation the late writer's completion
    /// is indistinguishable from the current holder's, and it can mark a
    /// key done for work the current holder is still doing — or, worse,
    /// overwrite the outcome that holder recorded.
    ///
    /// So a claim bumps the generation and returns it. Completion is
    /// conditional on that number ([`TxConn::complete_lease`]), which makes
    /// a superseded holder harmless: it cannot finish, and it finds out.
    pub async fn claim_lease(
        &self,
        key: &str,
        fingerprint: &str,
    ) -> Result<(LeaseState, i64), PoolError> {
        let rows = self
            .rows(&format!(
                "SELECT \"fingerprint\", \"generation\", \"done\" FROM \"$replay\" \
                 WHERE \"key\" = {} FOR UPDATE",
                sql_lit(key)
            ))
            .await?;
        let existing = rows.first().map(|r| {
            (
                r.first().cloned().flatten().unwrap_or_default(),
                r.get(1)
                    .cloned()
                    .flatten()
                    .and_then(|g| g.parse::<i64>().ok())
                    .unwrap_or(0),
                matches!(
                    r.get(2).cloned().flatten().as_deref(),
                    Some("t") | Some("true")
                ),
            )
        });
        match existing {
            Some((f, _, true)) if f == fingerprint => Ok((LeaseState::Applied, 0)),
            Some((_, _, true)) => Ok((LeaseState::Mismatch, 0)),
            // A reservation exists but is unfinished: its holder is stalled
            // or gone. Take it over by bumping the generation, which fences
            // that holder out of ever completing.
            Some((_, gen, false)) => {
                let next = gen + 1;
                self.rows(&format!(
                    "UPDATE \"$replay\" SET \"generation\" = {next}, \"fingerprint\" = {} \
                     WHERE \"key\" = {}",
                    sql_lit(fingerprint),
                    sql_lit(key)
                ))
                .await?;
                Ok((LeaseState::Fresh, next))
            }
            None => {
                self.rows(&format!(
                    "INSERT INTO \"$replay\" (\"key\", \"fingerprint\", \"generation\", \"done\") \
                     VALUES ({}, {}, 1, false)",
                    sql_lit(key),
                    sql_lit(fingerprint)
                ))
                .await?;
                Ok((LeaseState::Fresh, 1))
            }
        }
    }

    /// Mark the key done — **only if this holder is still the current
    /// one**. Answers `false` when a newer generation has taken the lease
    /// over, which is the fence: the caller must abort rather than commit
    /// an effect nobody will ever be able to attribute.
    ///
    /// This lands in the SAME transaction as the effect, so there is no
    /// window in which one exists without the other. A crash before commit
    /// loses both; a crash after loses neither.
    pub async fn complete_lease(&self, key: &str, generation: i64) -> Result<bool, PoolError> {
        let rows = self
            .rows(&format!(
                "UPDATE \"$replay\" SET \"done\" = true WHERE \"key\" = {} \
                 AND \"generation\" = {generation} RETURNING \"key\"",
                sql_lit(key)
            ))
            .await?;
        Ok(!rows.is_empty())
    }
}

/// The entities a write summary touched, de-duplicated. Derived from the
/// command group rather than guessed from the op's name, so a multi-row
/// group (a create and its acl bootstrap) invalidates both.
fn touched(summary: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = summary
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.get("entity").and_then(|e| e.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

/// The one blessed SQL string literal.
///
/// Public because `ordo plan --against` reads the grant registry's table to
/// count what a deploy revokes, and a second quoter in the CLI would be a
/// second escaping story — the thing `standard_conforming_strings` is pinned
/// to make impossible to get wrong once.
pub fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod backoff_tests {
    use super::*;

    /// The window doubles from 8 ms, stops at the ceiling, and the pause is
    /// anywhere from zero to the window.
    #[test]
    fn the_pause_is_full_jitter_under_the_ceiling() {
        let max = Duration::from_millis(250);
        for (attempt, window) in [(1, 8), (2, 16), (3, 32), (5, 128), (6, 250), (40, 250)] {
            let micros: u64 = window * 1000;
            // The largest draw that fits lands on the window itself; one
            // more wraps to zero.
            assert_eq!(
                backoff_pause(attempt, max, micros),
                Duration::from_millis(window),
                "attempt {attempt}"
            );
            assert_eq!(backoff_pause(attempt, max, micros + 1), Duration::ZERO);
            assert_eq!(backoff_pause(attempt, max, 0), Duration::ZERO);
        }
        assert!(backoff_pause(3, Duration::from_millis(1), u64::MAX) <= Duration::from_millis(1));
    }

    /// Two draws in a row differ: the source is not the clock, which two
    /// writers in one process read alike.
    #[test]
    fn the_draw_is_random() {
        let draws: std::collections::HashSet<u64> = (0..64).map(|_| random_u64()).collect();
        assert!(draws.len() > 60, "{}", draws.len());
    }
}
