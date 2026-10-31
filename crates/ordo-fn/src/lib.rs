//! The function host: runs one backend function call in a fresh WebAssembly
//! instance.
//!
//! - **The engine.** One JS engine module (Boa, `crates/ordo-fn-js`) is
//!   compiled once per process, booted once, and snapshotted with wizer. The
//!   snapshot is the module every call instantiates, so a call starts with the
//!   engine already built. Its memory comes from the snapshot copy-on-write.
//!   The compiled snapshot is kept on disk ([`HostConfig::engine_cache`],
//!   `src/cache.rs`), so a restart loads it instead of compiling.
//! - **A fresh instance per call** (ruled 2026-10-11). A new store, a new
//!   instance, the function's code evaluated in it, then all of it dropped.
//!   Nothing survives from one call to the next.
//! - **Limits** (plan §9): memory through a resource limiter, CPU as fuel, and
//!   wall time through epoch interruption plus a timeout around the network
//!   waits.
//! - **Six imports** and nothing else: `fetch` (through [`FnFetch`]), `take_reply`, `log`,
//!   `result`, `now_ms`, `random`. No file, socket, environment or secret
//!   value is reachable.
//!
//! The door (`ordo-api`) does sign-in, access, quotas, idempotency and pause
//! before a call gets here, and turns an [`FnError`] into an HTTP answer.

mod cache;
pub mod error;
pub mod seam;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::sync::{OnceCell, OwnedSemaphorePermit, Semaphore};
use wasmtime::{
    Caller, Config, Engine, Extern, Instance, InstancePre, Linker, Memory, Module, ResourceLimiter,
    Store, Trap, Val,
};
use wasmtime_wizer::{InstanceState, SnapshotVal, ValType, Wizer};

pub use cache::{CacheOutcome, CacheWrite};
pub use error::{BusyScope, FnError, Limit, Remedy};
pub use seam::{FetchError, FnFetch, FnFetchRequest, FnFetchResponse, HeaderValue};

/// The engine module, built by `scripts/build-fn-engine.sh`.
pub const ENGINE_WASM: &[u8] = include_bytes!("../engine/ordo_fn_js.wasm");

/// How often the epoch ticks. Wall time is enforced to this granularity.
const TICK: Duration = Duration::from_millis(10);

/// The limits one call runs under. Plan §9 gives the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub memory_bytes: usize,
    pub cpu: Duration,
    pub wall: Duration,
    /// Outbound calls per call.
    pub max_fetches: u32,
    /// Log kept per call; the rest is dropped and the log marked truncated.
    pub log_bytes: usize,
    /// The function's answer, as JSON text.
    pub output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 128 * 1024 * 1024,
            cpu: Duration::from_secs(10),
            wall: Duration::from_secs(60),
            max_fetches: 50,
            log_bytes: 64 * 1024,
            output_bytes: 1024 * 1024,
        }
    }
}

impl Limits {
    /// The ceilings a deployment owner sets by default: a function's own
    /// `limit` may ask for up to these.
    pub fn ceilings() -> Self {
        Self {
            memory_bytes: 512 * 1024 * 1024,
            cpu: Duration::from_secs(60),
            wall: Duration::from_secs(15 * 60),
            max_fetches: 500,
            log_bytes: 1024 * 1024,
            output_bytes: 16 * 1024 * 1024,
        }
    }

    /// These limits, each held to `ceiling`.
    pub fn within(&self, ceiling: &Limits) -> Limits {
        Limits {
            memory_bytes: self.memory_bytes.min(ceiling.memory_bytes),
            cpu: self.cpu.min(ceiling.cpu),
            wall: self.wall.min(ceiling.wall),
            max_fetches: self.max_fetches.min(ceiling.max_fetches),
            log_bytes: self.log_bytes.min(ceiling.log_bytes),
            output_bytes: self.output_bytes.min(ceiling.output_bytes),
        }
    }
}

/// Settings for the host as a whole.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Calls running at once for one tenant (plan §9: 10).
    pub per_space: usize,
    /// Calls running at once in this process (plan §9: 100).
    pub per_process: usize,
    /// Fuel that stands for one millisecond of CPU. Measured on the Boa
    /// engine on 2026-10-31 (Apple M-series): 5 to 8 million per ms, so the
    /// default is 6 million. See `tests/run.rs`, `fuel_rate_is_measured`.
    pub fuel_per_cpu_ms: u64,
    /// What a function's `limit` may reach.
    pub ceilings: Limits,
    /// Where the compiled engine is kept between starts (`src/cache.rs`).
    /// `None` compiles at every start. The engine is shared per process, so
    /// the first host made decides.
    pub engine_cache: Option<PathBuf>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            per_space: 10,
            per_process: 100,
            fuel_per_cpu_ms: 6_000_000,
            ceilings: Limits::ceilings(),
            engine_cache: None,
        }
    }
}

/// How this process got its engine: loaded from the cache or compiled.
#[derive(Debug, Clone)]
pub struct EngineStart {
    /// From the start of the build to an engine ready to instantiate.
    pub took: Duration,
    pub cache: CacheOutcome,
    pub write: CacheWrite,
}

/// One call to run.
pub struct FnCall {
    pub space: String,
    pub function: String,
    /// The program version the code belongs to.
    pub version: String,
    /// Stable across retries; the function may use it as an idempotency key.
    pub run_id: String,
    /// The bundled ES module.
    pub code: Arc<[u8]>,
    pub input: Value,
    /// Hosts the function may reach, exactly as declared.
    pub egress: Vec<String>,
    /// Secret names the function may use.
    pub secrets: Vec<String>,
    pub limits: Limits,
    pub fetch: Arc<dyn FnFetch>,
    /// Removes live secret values from log lines and error messages.
    pub scrub: Arc<dyn Scrub>,
}

/// Removes every live secret value of a tenant from text the function
/// wrote. The egress kernel already replaces a value an outside API echoes
/// back, so this is the second line, for logs and thrown messages.
pub trait Scrub: Send + Sync {
    fn scrub(&self, space: &str, text: &str) -> String;
}

/// Scrubs nothing: for a host with no secret store.
pub struct NoScrub;

impl Scrub for NoScrub {
    fn scrub(&self, _space: &str, text: &str) -> String {
        text.to_owned()
    }
}

/// One log line the function wrote, already scrubbed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// 0 debug, 1 info, 2 warn, 3 error.
    pub level: u8,
    pub text: String,
}

/// What a call did, for `$fnruns` and metrics. Never the input or output.
#[derive(Debug, Clone, Default)]
pub struct RunReport {
    pub wall: Duration,
    /// Making the fresh instance from the snapshot.
    pub instantiate: Duration,
    pub fuel: u64,
    pub cpu_ms: u64,
    pub peak_memory: usize,
    pub fetches: u32,
    pub hosts: Vec<String>,
    pub bytes_out: u64,
    pub bytes_in: u64,
    pub logs: Vec<LogLine>,
    pub logs_truncated: bool,
}

/// A finished call.
#[derive(Debug)]
pub struct FnRun {
    pub outcome: Result<Value, FnError>,
    pub report: RunReport,
}

impl FnRun {
    /// The outcome class `$fnruns` records.
    pub fn class(&self) -> &str {
        match &self.outcome {
            Ok(_) => "ok",
            Err(e) => e.code(),
        }
    }
}

/// The compiled, snapshotted engine. One per process.
struct FnEngine {
    engine: Engine,
    pre: InstancePre<Guest>,
    stop: Arc<AtomicBool>,
    start: EngineStart,
}

impl Drop for FnEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

static SHARED: OnceCell<Arc<FnEngine>> = OnceCell::const_new();

impl FnEngine {
    async fn shared(cache: Option<&Path>) -> Result<Arc<FnEngine>, FnError> {
        SHARED
            .get_or_try_init(|| async { FnEngine::build(cache).await.map(Arc::new) })
            .await
            .cloned()
    }

    async fn build(cache_dir: Option<&Path>) -> Result<FnEngine, FnError> {
        let started = Instant::now();
        let engine_err = |what: &str, e: wasmtime::Error| FnError::Engine {
            message: format!("{what}: {e:#}"),
        };
        let mut cfg = Config::new();
        cfg.consume_fuel(true);
        cfg.epoch_interruption(true);
        cfg.memory_init_cow(true);
        let engine = Engine::new(&cfg).map_err(|e| engine_err("engine config", e))?;
        let mut linker = Linker::<Guest>::new(&engine);
        define_imports(&mut linker).map_err(|e| engine_err("imports", e))?;

        let cache = cache_dir.map(|d| cache::EngineCache::new(d, &engine, ENGINE_WASM));
        let found = match &cache {
            Some(c) => c.load(&engine),
            None => Err(CacheOutcome::Off),
        };
        let (module, outcome, write) = match found {
            Ok(m) => {
                let path = cache
                    .as_ref()
                    .map(|c| c.path().to_owned())
                    .unwrap_or_default();
                (m, CacheOutcome::Hit(path), CacheWrite::Skipped)
            }
            Err(outcome) => {
                let mut m = snapshot_engine(&engine, &linker).await?;
                let write = match &cache {
                    Some(c) => c.store(&m),
                    None => CacheWrite::Skipped,
                };
                // Run from the file just written, as a restart would: a
                // module mapped from a file shares its memory image
                // copy-on-write on every platform (8 µs an instance on
                // macOS, against about 90 µs copied).
                if let (Some(c), CacheWrite::Written(_)) = (&cache, &write) {
                    if let Ok(loaded) = c.load(&engine) {
                        m = loaded;
                    }
                }
                (m, outcome, write)
            }
        };
        let pre = linker
            .instantiate_pre(&module)
            .map_err(|e| engine_err("pre-instantiate", e))?;

        let stop = Arc::new(AtomicBool::new(false));
        {
            let engine = engine.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("ordo-fn-epoch".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(TICK);
                        engine.increment_epoch();
                    }
                })
                .map_err(|e| FnError::Engine {
                    message: format!("epoch thread: {e}"),
                })?;
        }
        let start = EngineStart {
            took: started.elapsed(),
            cache: outcome,
            write,
        };
        Ok(FnEngine {
            engine,
            pre,
            stop,
            start,
        })
    }
}

/// Compiles the engine, boots it once, and compiles the snapshot: the
/// module every call starts from. About a second; the cache skips it.
async fn snapshot_engine(engine: &Engine, linker: &Linker<Guest>) -> Result<Module, FnError> {
    let engine_err = |what: &str, e: wasmtime::Error| FnError::Engine {
        message: format!("{what}: {e:#}"),
    };
    let mut wizer = Wizer::new();
    wizer.init_func("ordo_init");
    let (cx, instrumented) = wizer
        .instrument(ENGINE_WASM)
        .map_err(|e| engine_err("instrument", e))?;
    let module = Module::new(engine, &instrumented).map_err(|e| engine_err("compile engine", e))?;
    let mut store = Store::new(engine, Guest::boot());
    store.limiter(|g| &mut g.limiter);
    store
        .set_fuel(u64::MAX / 2)
        .map_err(|e| engine_err("fuel", e))?;
    store.set_epoch_deadline(u64::MAX / 2);
    let instance = linker
        .instantiate_async(&mut store, &module)
        .await
        .map_err(|e| engine_err("instantiate engine", e))?;
    let init = instance
        .get_typed_func::<(), ()>(&mut store, "ordo_init")
        .map_err(|e| engine_err("ordo_init", e))?;
    init.call_async(&mut store, ())
        .await
        .map_err(|e| engine_err("engine boot", e))?;
    let snapshot = wizer
        .snapshot(
            &cx,
            &mut Snap {
                store: &mut store,
                instance,
            },
        )
        .await
        .map_err(|e| engine_err("snapshot", e))?;
    drop(store);
    Module::new(engine, &snapshot).map_err(|e| engine_err("compile snapshot", e))
}

/// wizer reads the booted instance's state through this.
struct Snap<'a> {
    store: &'a mut Store<Guest>,
    instance: Instance,
}

impl InstanceState for Snap<'_> {
    async fn global_get(&mut self, name: &str, _: ValType) -> SnapshotVal {
        let global = self
            .instance
            .get_global(&mut *self.store, name)
            .expect("wizer names an exported global");
        match global.get(&mut *self.store) {
            Val::I32(x) => SnapshotVal::I32(x),
            Val::I64(x) => SnapshotVal::I64(x),
            Val::F32(x) => SnapshotVal::F32(x),
            Val::F64(x) => SnapshotVal::F64(x),
            Val::V128(x) => SnapshotVal::V128(x.as_u128()),
            _ => panic!("the engine has a global wizer cannot snapshot"),
        }
    }

    async fn memory_contents(&mut self, name: &str, contents: impl FnOnce(&[u8]) + Send) {
        let memory = self
            .instance
            .get_memory(&mut *self.store, name)
            .expect("wizer names an exported memory");
        contents(memory.data(&*self.store))
    }
}

/// Runs function calls. Cheap to make: the engine is shared per process.
pub struct FnHost {
    engine: Arc<FnEngine>,
    config: HostConfig,
    process: Arc<Semaphore>,
    spaces: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl FnHost {
    /// The first host in a process compiles and snapshots the engine, which
    /// takes seconds. Every later one reuses it.
    pub async fn new(config: HostConfig) -> Result<FnHost, FnError> {
        let engine = FnEngine::shared(config.engine_cache.as_deref()).await?;
        Ok(FnHost::with(engine, config))
    }

    /// A host with an engine of its own, built now and never shared. For
    /// tests and measurements of the engine start; a server uses [`FnHost::new`].
    #[doc(hidden)]
    pub async fn unshared(config: HostConfig) -> Result<FnHost, FnError> {
        let engine = FnEngine::build(config.engine_cache.as_deref()).await?;
        Ok(FnHost::with(Arc::new(engine), config))
    }

    fn with(engine: Arc<FnEngine>, config: HostConfig) -> FnHost {
        FnHost {
            engine,
            process: Arc::new(Semaphore::new(config.per_process)),
            spaces: Mutex::new(HashMap::new()),
            config,
        }
    }

    /// How this process's engine was built or loaded.
    pub fn engine_start(&self) -> &EngineStart {
        &self.engine.start
    }

    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    fn permits(
        &self,
        space: &str,
    ) -> Result<(OwnedSemaphorePermit, OwnedSemaphorePermit), FnError> {
        let process = self
            .process
            .clone()
            .try_acquire_owned()
            .map_err(|_| FnError::Busy {
                scope: BusyScope::Process,
            })?;
        let sem = {
            let mut spaces = self.spaces.lock().unwrap_or_else(|p| p.into_inner());
            spaces
                .entry(space.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(self.config.per_space)))
                .clone()
        };
        let space = sem.try_acquire_owned().map_err(|_| FnError::Busy {
            scope: BusyScope::Tenant,
        })?;
        Ok((process, space))
    }

    /// Runs one call in a fresh instance. Never panics on the function's
    /// account: every failure is an [`FnError`] in the outcome.
    pub async fn call(&self, call: FnCall) -> FnRun {
        let started = Instant::now();
        let _permits = match self.permits(&call.space) {
            Ok(p) => p,
            Err(e) => {
                return FnRun {
                    outcome: Err(e),
                    report: RunReport::default(),
                }
            }
        };
        let limits = call.limits.within(&self.config.ceilings);
        let wall = limits.wall;
        let fuel_per_ms = self.config.fuel_per_cpu_ms.max(1);
        let fuel = (limits.cpu.as_millis() as u64).saturating_mul(fuel_per_ms);
        let scrub = call.scrub.clone();
        let space = call.space.clone();

        let mut store = Store::new(&self.engine.engine, Guest::for_call(&call, &limits));
        store.limiter(|g| &mut g.limiter);
        let mut report = RunReport::default();
        if let Err(e) = store.set_fuel(fuel) {
            return FnRun {
                outcome: Err(FnError::Engine {
                    message: format!("fuel: {e:#}"),
                }),
                report,
            };
        }
        store.set_epoch_deadline(wall.as_millis() as u64 / TICK.as_millis() as u64 + 1);
        store.epoch_deadline_trap();

        let run = run_in(&self.engine.pre, &mut store, &call, &mut report);
        let outcome = match tokio::time::timeout(wall, run).await {
            Ok(r) => r,
            Err(_) => Err(FnError::Timeout {
                limit: Limit::Wall,
                after: wall,
            }),
        };
        let used = fuel.saturating_sub(store.get_fuel().unwrap_or(0));
        let outcome = outcome.map_err(|e| match e {
            // A fuel trap and a wall trap look alike from inside; the fuel
            // count says which it was.
            FnError::Timeout { .. } if used >= fuel => FnError::Timeout {
                limit: Limit::Cpu,
                after: limits.cpu,
            },
            FnError::Threw { message, stack } => FnError::Threw {
                message: scrub.scrub(&space, &message),
                stack: scrub.scrub(&space, &stack),
            },
            other => other,
        });
        let guest = store.into_data();
        report.wall = started.elapsed();
        report.fuel = used;
        report.cpu_ms = used / fuel_per_ms;
        report.peak_memory = guest.limiter.peak;
        report.fetches = guest.fetches;
        report.hosts = guest.hosts;
        report.bytes_out = guest.bytes_out;
        report.bytes_in = guest.bytes_in;
        report.logs = guest.logs;
        report.logs_truncated = guest.logs_truncated;
        FnRun { outcome, report }
    }
}

async fn run_in(
    pre: &InstancePre<Guest>,
    store: &mut Store<Guest>,
    call: &FnCall,
    report: &mut RunReport,
) -> Result<Value, FnError> {
    let t = Instant::now();
    let instance = pre
        .instantiate_async(&mut *store)
        .await
        .map_err(|e| classify(store, e))?;
    report.instantiate = t.elapsed();

    let env = serde_json::json!({
        "input": call.input,
        "ctx": {
            "tenant": call.space,
            "function": call.function,
            "version": call.version,
            "runId": call.run_id,
        }
    })
    .to_string();
    let memory = instance
        .get_memory(&mut *store, "memory")
        .ok_or_else(|| FnError::Engine {
            message: "the engine exports no memory".into(),
        })?;
    let code_ptr = place(store, &instance, &memory, &call.code).await?;
    let env_ptr = place(store, &instance, &memory, env.as_bytes()).await?;
    let run = instance
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut *store, "ordo_run")
        .map_err(|e| FnError::Engine {
            message: format!("ordo_run: {e:#}"),
        })?;
    run.call_async(
        &mut *store,
        (code_ptr, call.code.len() as i32, env_ptr, env.len() as i32),
    )
    .await
    .map_err(|e| classify(store, e))?;

    let guest = store.data_mut();
    if guest.output_over {
        return Err(FnError::BadOutput {
            reason: format!(
                "the answer is over the limit of {} bytes",
                guest.limits_output
            ),
        });
    }
    let raw = guest.result.take().ok_or_else(|| FnError::Engine {
        message: "the engine returned no answer".into(),
    })?;
    let answer: Value = serde_json::from_slice(&raw).map_err(|e| FnError::BadOutput {
        reason: format!("not JSON: {e}"),
    })?;
    if let Some(v) = answer.get("ok") {
        return Ok(v.clone());
    }
    let err = answer.get("error").cloned().unwrap_or(Value::Null);
    let text = |k: &str| err.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let code = text("code");
    // Only a refusal the host itself issued in this call may set the
    // outcome class; a function cannot claim one by throwing `{code}`.
    if !code.is_empty() && guest.issued.contains(&code) {
        return Err(FnError::Fetch {
            code,
            message: text("message"),
        });
    }
    Err(FnError::Threw {
        message: text("message"),
        stack: text("stack"),
    })
}

/// Copies `bytes` into the guest through its allocator.
async fn place(
    store: &mut Store<Guest>,
    instance: &Instance,
    memory: &Memory,
    bytes: &[u8],
) -> Result<i32, FnError> {
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut *store, "ordo_alloc")
        .map_err(|e| FnError::Engine {
            message: format!("ordo_alloc: {e:#}"),
        })?;
    let ptr = alloc
        .call_async(&mut *store, bytes.len() as i32)
        .await
        .map_err(|e| classify(store, e))?;
    memory
        .write(&mut *store, ptr as u32 as usize, bytes)
        .map_err(|e| FnError::Engine {
            message: format!("write input: {e}"),
        })?;
    Ok(ptr)
}

/// Turns a trap into the limit or failure behind it.
fn classify(store: &Store<Guest>, e: wasmtime::Error) -> FnError {
    if store.data().limiter.hit {
        return FnError::OutOfMemory {
            limit_bytes: store.data().limiter.limit,
        };
    }
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => FnError::Timeout {
            limit: Limit::Cpu,
            after: Duration::ZERO,
        },
        Some(Trap::Interrupt) => FnError::Timeout {
            limit: Limit::Wall,
            after: Duration::ZERO,
        },
        Some(Trap::StackOverflow) => FnError::Threw {
            message: "stack overflow".into(),
            stack: String::new(),
        },
        _ => FnError::Engine {
            message: format!("{e:#}"),
        },
    }
}

/// Memory limit, and the peak a call reached.
struct MemLimiter {
    limit: usize,
    peak: usize,
    hit: bool,
}

impl ResourceLimiter for MemLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.limit {
            self.hit = true;
            return Ok(false);
        }
        self.peak = self.peak.max(desired);
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= 1_000_000)
    }
}

/// The state one instance's imports see.
struct Guest {
    limiter: MemLimiter,
    space: String,
    egress: Vec<String>,
    secrets: Vec<String>,
    fetch: Option<Arc<dyn FnFetch>>,
    scrub: Option<Arc<dyn Scrub>>,
    max_fetches: u32,
    fetches: u32,
    hosts: Vec<String>,
    bytes_out: u64,
    bytes_in: u64,
    reply: Vec<u8>,
    /// Refusal codes this host issued during the call.
    issued: Vec<String>,
    result: Option<Vec<u8>>,
    limits_output: usize,
    output_over: bool,
    logs: Vec<LogLine>,
    log_bytes: usize,
    log_limit: usize,
    logs_truncated: bool,
}

impl Guest {
    /// The state the engine boots in, before the snapshot. It can reach
    /// nothing.
    fn boot() -> Guest {
        Guest {
            limiter: MemLimiter {
                limit: 512 * 1024 * 1024,
                peak: 0,
                hit: false,
            },
            space: String::new(),
            egress: Vec::new(),
            secrets: Vec::new(),
            fetch: None,
            scrub: None,
            max_fetches: 0,
            fetches: 0,
            hosts: Vec::new(),
            bytes_out: 0,
            bytes_in: 0,
            reply: Vec::new(),
            issued: Vec::new(),
            result: None,
            limits_output: 0,
            output_over: false,
            logs: Vec::new(),
            log_bytes: 0,
            log_limit: 64 * 1024,
            logs_truncated: false,
        }
    }

    fn for_call(call: &FnCall, limits: &Limits) -> Guest {
        let mut g = Guest::boot();
        g.limiter.limit = limits.memory_bytes;
        g.space = call.space.clone();
        g.egress = call.egress.iter().map(|h| h.to_ascii_lowercase()).collect();
        g.secrets = call.secrets.clone();
        g.fetch = Some(call.fetch.clone());
        g.scrub = Some(call.scrub.clone());
        g.max_fetches = limits.max_fetches;
        g.limits_output = limits.output_bytes;
        g.log_limit = limits.log_bytes;
        g
    }

    fn push_log(&mut self, level: u8, text: String) {
        if self.logs_truncated {
            return;
        }
        if self.log_bytes + text.len() > self.log_limit {
            self.logs_truncated = true;
            self.logs.push(LogLine {
                level: 2,
                text: "[log truncated: the per-call limit was reached]".into(),
            });
            return;
        }
        self.log_bytes += text.len();
        self.logs.push(LogLine { level, text });
    }
}

fn guest_bytes(caller: &mut Caller<'_, Guest>, ptr: i32, len: i32) -> Option<Vec<u8>> {
    let mem = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return None,
    };
    let (ptr, len) = (ptr as u32 as usize, len as u32 as usize);
    mem.data(&*caller)
        .get(ptr..ptr.checked_add(len)?)
        .map(<[u8]>::to_vec)
}

fn define_imports(linker: &mut Linker<Guest>) -> wasmtime::Result<()> {
    linker.func_wrap_async(
        "ordo",
        "fetch",
        |mut caller: Caller<'_, Guest>, (ptr, len): (i32, i32)| {
            Box::new(async move {
                let req = guest_bytes(&mut caller, ptr, len).unwrap_or_default();
                let reply = guest_fetch(caller.data_mut(), &req).await;
                let g = caller.data_mut();
                let (bytes, refused) = match reply {
                    Ok(b) => (b, false),
                    Err(e) => {
                        g.issued.push(e.code.clone());
                        let b = serde_json::json!({ "code": e.code, "message": e.message })
                            .to_string()
                            .into_bytes();
                        (b, true)
                    }
                };
                let n = bytes.len() as i32;
                g.reply = bytes;
                if refused {
                    -n
                } else {
                    n
                }
            })
        },
    )?;
    linker.func_wrap(
        "ordo",
        "take_reply",
        |mut caller: Caller<'_, Guest>, ptr: i32| -> wasmtime::Result<()> {
            let reply = std::mem::take(&mut caller.data_mut().reply);
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => wasmtime::bail!("no memory"),
            };
            mem.write(&mut caller, ptr as u32 as usize, &reply)?;
            Ok(())
        },
    )?;
    linker.func_wrap(
        "ordo",
        "log",
        |mut caller: Caller<'_, Guest>, level: i32, ptr: i32, len: i32| {
            let bytes = guest_bytes(&mut caller, ptr, len).unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let g = caller.data_mut();
            let text = match &g.scrub {
                Some(f) => f.scrub(&g.space, &text),
                None => text,
            };
            g.push_log(level.clamp(0, 3) as u8, text);
        },
    )?;
    linker.func_wrap(
        "ordo",
        "result",
        |mut caller: Caller<'_, Guest>, ptr: i32, len: i32| {
            let limit = caller.data().limits_output;
            // The answer is `{"ok":...}` around the function's own JSON.
            if len as u32 as usize > limit + 16 {
                caller.data_mut().output_over = true;
                return;
            }
            let bytes = guest_bytes(&mut caller, ptr, len);
            caller.data_mut().result = bytes;
        },
    )?;
    linker.func_wrap("ordo", "now_ms", || -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0)
    })?;
    linker.func_wrap(
        "ordo",
        "random",
        |mut caller: Caller<'_, Guest>, ptr: i32, len: i32| -> wasmtime::Result<()> {
            let mut buf = vec![0u8; len as u32 as usize];
            fill_random(&mut buf);
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => wasmtime::bail!("no memory"),
            };
            mem.write(&mut caller, ptr as u32 as usize, &buf)?;
            Ok(())
        },
    )?;
    Ok(())
}

/// Bytes for `Math.random`, from the operating system, fresh per instance.
fn fill_random(buf: &mut [u8]) {
    if getrandom::getrandom(buf).is_err() {
        buf.fill(0);
    }
}

/// The wire shape of a guest request, as the prelude writes it.
#[derive(serde::Deserialize)]
struct WireRequest {
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<(String, WireHeader)>,
    #[serde(default)]
    body: String,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum WireHeader {
    Plain { plain: String },
    Secret { secret: String, template: String },
}

/// One `fetch` from the guest: checked against the declaration, then handed
/// to the egress seam. The reply is the JSON the prelude reads.
/// A fetch the host refused, or the kernel's refusal as the guest sees it.
struct Refused {
    code: String,
    message: String,
}

impl Refused {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Refused {
            code: code.into(),
            message: message.into(),
        }
    }
}

async fn guest_fetch(g: &mut Guest, raw: &[u8]) -> Result<Vec<u8>, Refused> {
    let req: WireRequest = serde_json::from_slice(raw)
        .map_err(|e| Refused::new("bad_request", format!("a malformed request: {e}")))?;
    if g.fetches >= g.max_fetches {
        return Err(Refused::new(
            "fetch_limit",
            format!("more than {} outbound calls in one call", g.max_fetches),
        ));
    }
    let host = https_host(&req.url).ok_or_else(|| {
        Refused::new(
            "bad_url",
            "only https://host/... URLs may be fetched".to_owned(),
        )
    })?;
    if !g.egress.contains(&host) {
        return Err(Refused::new(
            "host_not_allowed",
            format!("{host} is not in this function's egress list"),
        ));
    }
    let mut headers = Vec::with_capacity(req.headers.len());
    for (name, v) in req.headers {
        let v = match v {
            WireHeader::Plain { plain } => HeaderValue::Plain(plain),
            WireHeader::Secret { secret, template } => {
                if !g.secrets.contains(&secret) {
                    return Err(Refused::new(
                        "secret_not_listed",
                        format!("{secret} is not in this function's secrets list"),
                    ));
                }
                HeaderValue::Secret {
                    name: secret,
                    template,
                }
            }
        };
        headers.push((name, v));
    }
    let fetch = g
        .fetch
        .clone()
        .ok_or_else(|| Refused::new("host_not_allowed", "no egress while booting"))?;
    g.fetches += 1;
    if !g.hosts.contains(&host) {
        g.hosts.push(host);
    }
    let body = req.body.into_bytes();
    g.bytes_out += body.len() as u64;
    let space = g.space.clone();
    let resp = fetch
        .fetch(
            &space,
            FnFetchRequest {
                method: req.method,
                url: req.url,
                headers,
                body,
                egress: g.egress.clone(),
                secrets: g.secrets.clone(),
            },
        )
        .await
        .map_err(|e| Refused::new(e.kind(), e.to_string()))?;
    g.bytes_in += resp.body.len() as u64;
    Ok(serde_json::json!({
        "status": resp.status,
        "headers": resp.headers,
        "body": String::from_utf8_lossy(&resp.body),
    })
    .to_string()
    .into_bytes())
}

/// The lowercased host of an `https://` URL, or `None`. A URL with user
/// information (`user@host`) is refused: it is how a host gets disguised.
pub fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| {
        url.get(..8)
            .filter(|p| p.eq_ignore_ascii_case("https://"))
            .map(|_| &url[8..])
    })?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if !h.contains(']') && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::https_host;

    #[test]
    fn a_url_yields_its_host() {
        assert_eq!(
            https_host("https://API.example.com/v1/x?y").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(
            https_host("https://api.example.com:443/").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(https_host("http://api.example.com/"), None);
        assert_eq!(https_host("https://evil.com@api.example.com/"), None);
        assert_eq!(https_host("https:///x"), None);
    }
}
