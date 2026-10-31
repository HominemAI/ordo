//! Backend functions in the host (`plan-backend-functions.md` increment 1).
//!
//! - **The code store.** `ordo apply` reads each function's code file and
//!   stores it in `$fncode` by its SHA-256. The program's manifest carries
//!   the hash, so one version of the program is one version of its code, and
//!   an old version's code is still there on a rollback.
//! - **The table.** The serving version's functions, each with its code read
//!   back by hash, its limits and its door spec. Rebuilt when the version
//!   changes.
//! - **The runner.** [`HostFunctions`] is the door's
//!   [`ordo_api::functions::FunctionRunner`]: the pause gate, one run in a
//!   fresh instance ([`ordo_fn::FnHost`]), one `$fnruns` row, and the one
//!   mapping from [`ordo_fn::FnError`] to the door's [`ExecError`].
//!
//! **Who it runs as.** A page call runs as the signed-in person: the door's
//! access check resolved them, and the run is recorded as
//! `function:<name>@<version>` on behalf of that person. In increment 1 a
//! function reaches no ops, so the delegation has nothing to carry yet;
//! increment 4 (`ctx.op`) is where it bounds the ops it calls.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use ordo_api::functions::{FnInvocation, FunctionRunner};
use ordo_api::spec::{
    ExecError, NotSyncable, OpCost, OpKind, OpSpec, ParamSpec, RefineTier, Syncability,
};
use ordo_api::ErrorCode;
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_fn::{FnCall, FnError, FnFetch, FnHost, HostConfig, Limits, Scrub};
use ordo_schema::Schema;
use ordo_syntax::FunctionDecl;
use serde_json::Value;

use crate::spaces::Spaces;

/// The code store, and one row per call (§10). Both in the control
/// database. `IF NOT EXISTS`, as every table the host makes itself.
pub const FUNCTIONS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$fncode\" ( \
  \"hash\" text PRIMARY KEY, \
  \"code\" text NOT NULL, \
  \"size\" bigint NOT NULL, \
  \"stored_at\" bigint NOT NULL \
); \
CREATE TABLE IF NOT EXISTS \"$fnruns\" ( \
  \"seq\" bigserial PRIMARY KEY, \
  \"function\" text NOT NULL, \
  \"version\" text NOT NULL, \
  \"space\" text NOT NULL, \
  \"wake\" text NOT NULL, \
  \"principal\" text NOT NULL, \
  \"started_at\" bigint NOT NULL, \
  \"wall_ms\" bigint NOT NULL, \
  \"cpu_ms\" bigint NOT NULL, \
  \"peak_memory\" bigint NOT NULL, \
  \"fetches\" bigint NOT NULL, \
  \"hosts\" text NOT NULL, \
  \"bytes_out\" bigint NOT NULL, \
  \"bytes_in\" bigint NOT NULL, \
  \"outcome\" text NOT NULL, \
  \"message\" text NOT NULL, \
  \"logs\" text NOT NULL \
); \
CREATE INDEX IF NOT EXISTS \"$fnruns_space\" ON \"$fnruns\" (\"space\", \"seq\"); \
CREATE INDEX IF NOT EXISTS \"$fnruns_started\" ON \"$fnruns\" (\"started_at\")";

/// The largest code file `ordo apply` stores (§9).
pub const MAX_CODE_BYTES: usize = 20 * 1024 * 1024;

pub async fn ensure_tables(pool: &Pool) -> Result<(), PoolError> {
    pool.rows(FUNCTIONS_DDL).await.map(|_| ())
}

/// Stores one code file by its hash. Idempotent: the same bytes under the
/// same hash are already there.
pub async fn store_code(pool: &Pool, code: &[u8]) -> Result<String, String> {
    if code.len() > MAX_CODE_BYTES {
        return Err(format!(
            "a function's code is {} bytes, over the limit of {MAX_CODE_BYTES}",
            code.len()
        ));
    }
    let text = std::str::from_utf8(code).map_err(|_| {
        "a function's code is not UTF-8 text: bundle it as one ES module".to_string()
    })?;
    let hash = ordo_plan::artifact::code_hash(code);
    ensure_tables(pool).await.map_err(|e| e.to_string())?;
    pool.rows(&format!(
        "INSERT INTO \"$fncode\" (\"hash\", \"code\", \"size\", \"stored_at\") \
         VALUES ({}, {}, {}, {}) ON CONFLICT (\"hash\") DO NOTHING",
        sql_lit(&hash),
        sql_lit(text),
        code.len(),
        now_secs()
    ))
    .await
    .map_err(|e| e.to_string())?;
    Ok(hash)
}

/// One code file by hash.
pub async fn load_code(pool: &Pool, hash: &str) -> Result<Option<Vec<u8>>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"code\" FROM \"$fncode\" WHERE \"hash\" = {}",
            sql_lit(hash)
        ))
        .await?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next().flatten())
        .map(String::into_bytes))
}

/// One function the serving version declares.
#[derive(Clone)]
pub struct FnEntry {
    pub decl: FunctionDecl,
    /// The program version it belongs to.
    pub version: String,
    pub code_hash: String,
    pub code: Arc<[u8]>,
    pub spec: OpSpec,
}

impl FnEntry {
    /// Its limits: the declaration's, over the defaults.
    pub fn limits(&self) -> Limits {
        let mut l = Limits::default();
        if let Some(ms) = self.decl.limits.wall_ms {
            l.wall = Duration::from_millis(ms);
        }
        if let Some(ms) = self.decl.limits.cpu_ms {
            l.cpu = Duration::from_millis(ms);
        }
        if let Some(b) = self.decl.limits.memory_bytes {
            l.memory_bytes = b as usize;
        }
        l
    }
}

/// The serving version's functions, by name.
#[derive(Clone, Default)]
pub struct FnTable {
    pub by_name: BTreeMap<String, FnEntry>,
}

impl FnTable {
    /// Builds the table from the compiled schema, the stored artifact
    /// document (which holds each code hash) and the code by hash.
    pub fn build(
        schema: &Schema,
        doc: &Value,
        version: &str,
        code: &BTreeMap<String, Vec<u8>>,
    ) -> Result<FnTable, String> {
        let mut by_name = BTreeMap::new();
        for f in &schema.functions {
            let hash = code_hash_in(doc, &f.name.name).ok_or_else(|| {
                format!(
                    "function `{}` has no code hash in version {version}: apply it with \
                     `ordo apply`, which reads its code file",
                    f.name.name
                )
            })?;
            let bytes = code.get(&hash).ok_or_else(|| {
                format!(
                    "function `{}`: code {hash} is not in `$fncode`",
                    f.name.name
                )
            })?;
            by_name.insert(
                f.name.name.clone(),
                FnEntry {
                    spec: door_spec(f, version, &hash),
                    decl: f.clone(),
                    version: version.to_string(),
                    code_hash: hash,
                    code: Arc::from(bytes.as_slice()),
                },
            );
        }
        Ok(FnTable { by_name })
    }

    /// [`Self::build`], reading the code from `$fncode`.
    pub async fn load(
        pool: &Pool,
        schema: &Schema,
        doc: &Value,
        version: &str,
    ) -> Result<FnTable, String> {
        if schema.functions.is_empty() {
            return Ok(FnTable::default());
        }
        ensure_tables(pool).await.map_err(|e| e.to_string())?;
        let mut code = BTreeMap::new();
        for f in &schema.functions {
            if let Some(h) = code_hash_in(doc, &f.name.name) {
                if let Some(bytes) = load_code(pool, &h).await.map_err(|e| e.to_string())? {
                    code.insert(h, bytes);
                }
            }
        }
        FnTable::build(schema, doc, version, &code)
    }
}

fn code_hash_in(doc: &Value, name: &str) -> Option<String> {
    doc["functions"]
        .as_array()?
        .iter()
        .find(|f| f["name"] == name)?["code_hash"]
        .as_str()
        .map(str::to_string)
}

/// The door's view of a function: a write (so `POST`), typed like an op.
fn door_spec(f: &FunctionDecl, version: &str, code_hash: &str) -> OpSpec {
    let params = f
        .params
        .iter()
        .filter_map(|p| {
            Some(ParamSpec {
                name: p.name.name.clone(),
                ty: crate::param_ty_of(p)?,
                required: p.default.is_none(),
                bounds: None,
            })
        })
        .collect();
    let short = code_hash.trim_start_matches("sha256:");
    OpSpec {
        name: f.name.name.clone(),
        version_hash: format!("{version}.{}", &short[..short.len().min(16)]),
        kind: OpKind::Write,
        params,
        output_fields: Vec::new(),
        live_capable: false,
        sync: Syncability::NotSyncable(NotSyncable::NoAnchor),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    }
}

/// What scrubs one call's log lines and error messages: a scrubber made
/// for the call, from the values of the secrets the function declares.
#[async_trait::async_trait]
pub trait CallScrub: Send + Sync {
    async fn for_call(&self, space: &str, secrets: &[String]) -> Arc<dyn Scrub>;
}

/// Scrubs nothing. For a host that holds no secrets, and for tests.
pub struct NoCallScrub;

#[async_trait::async_trait]
impl CallScrub for NoCallScrub {
    async fn for_call(&self, _space: &str, _secrets: &[String]) -> Arc<dyn Scrub> {
        Arc::new(ordo_fn::NoScrub)
    }
}

/// Replaces each held value with `[secret]`.
pub struct ValueScrub {
    values: Vec<String>,
}

impl ValueScrub {
    /// Values shorter than 4 bytes are not held: replacing them would
    /// shred ordinary text and hide nothing worth hiding.
    pub fn new(values: Vec<String>) -> ValueScrub {
        let mut values: Vec<String> = values.into_iter().filter(|v| v.len() >= 4).collect();
        // Longest first, so a value that contains another is replaced whole.
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        ValueScrub { values }
    }
}

impl Scrub for ValueScrub {
    fn scrub(&self, _space: &str, text: &str) -> String {
        let mut out = text.to_owned();
        for v in &self.values {
            if out.contains(v.as_str()) {
                out = out.replace(v.as_str(), "[secret]");
            }
        }
        out
    }
}

#[cfg(test)]
mod scrub_tests {
    use super::ValueScrub;
    use ordo_fn::Scrub;

    #[test]
    fn every_held_value_is_replaced_and_short_ones_are_not_held() {
        let s = ValueScrub::new(vec![
            "sk-abcdef".into(),
            "sk-abcdef-long".into(),
            "ab".into(),
        ]);
        assert_eq!(
            s.scrub("acme", "a sk-abcdef-long and sk-abcdef, ab"),
            "a [secret] and [secret], ab"
        );
    }
}

/// The door's function runner.
pub struct HostFunctions {
    /// Made on the first call: compiling the engine takes about a second,
    /// and a deployment that declares no function never pays it.
    host: tokio::sync::OnceCell<FnHost>,
    config: HostConfig,
    table: RwLock<Arc<FnTable>>,
    spaces: Arc<Spaces>,
    control: Pool,
    fetch: Arc<dyn FnFetch>,
    scrub: Arc<dyn CallScrub>,
}

impl HostFunctions {
    pub fn new(
        config: HostConfig,
        table: FnTable,
        spaces: Arc<Spaces>,
        control: Pool,
        fetch: Arc<dyn FnFetch>,
        scrub: Arc<dyn CallScrub>,
    ) -> HostFunctions {
        HostFunctions {
            host: tokio::sync::OnceCell::new(),
            config,
            table: RwLock::new(Arc::new(table)),
            spaces,
            control,
            fetch,
            scrub,
        }
    }

    /// Serve another version's functions. Calls in flight finish on the
    /// entry they started with.
    pub fn replace(&self, table: FnTable) {
        *self.table.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(table);
    }

    /// Serve `version`'s functions: its manifest from the registry, its code
    /// from `$fncode`. On a failure the table is emptied, so no function of
    /// another version answers for this one, and the reason is answered.
    pub async fn reload(
        &self,
        pool: &Pool,
        version: &str,
        schema: &Schema,
    ) -> Result<usize, String> {
        if schema.functions.is_empty() {
            self.replace(FnTable::default());
            return Ok(0);
        }
        let loaded = async {
            let stored = ordo_exec::registry::get(pool, version)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("version {version} is not in the registry"))?;
            let doc: Value = serde_json::from_str(&stored.artifact).map_err(|e| e.to_string())?;
            FnTable::load(pool, schema, &doc, version).await
        }
        .await;
        match loaded {
            Ok(t) => {
                let n = t.by_name.len();
                self.replace(t);
                Ok(n)
            }
            Err(e) => {
                self.replace(FnTable::default());
                Err(e)
            }
        }
    }

    fn entry(&self, name: &str) -> Option<FnEntry> {
        self.table
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .by_name
            .get(name)
            .cloned()
    }
}

impl FunctionRunner for HostFunctions {
    fn spec(&self, name: &str) -> Option<OpSpec> {
        self.entry(name).map(|e| e.spec)
    }

    fn call<'a>(&'a self, call: FnInvocation) -> BoxFuture<'a, Result<Value, ExecError>> {
        Box::pin(async move {
            let entry = self
                .entry(&call.name)
                .ok_or_else(|| ExecError::internal("the function left the program mid-call"))?;
            // §9: a paused tenant refuses with its pause answer, unless the
            // function says `nopause`. Below the door, as for an op.
            if !entry.decl.nopause {
                crate::pause::gate(&self.spaces, &entry.decl.name.name, &call.space)?;
            }
            let run_id = call.idempotency_key.clone().unwrap_or_else(fresh_run_id);
            let started_at = now_secs();
            let host = self
                .host
                .get_or_try_init(|| FnHost::new(self.config.clone()))
                .await
                .map_err(exec_error)?;
            let secrets: Vec<String> = entry.decl.secrets.iter().map(|s| s.name.clone()).collect();
            let scrub = self.scrub.for_call(&call.space, &secrets).await;
            let run = host
                .call(FnCall {
                    space: call.space.clone(),
                    function: entry.decl.name.name.clone(),
                    version: entry.version.clone(),
                    run_id,
                    code: entry.code.clone(),
                    input: Value::Object(call.args.clone()),
                    egress: entry.decl.egress.iter().map(|h| h.name.clone()).collect(),
                    secrets,
                    limits: entry.limits(),
                    fetch: self.fetch.clone(),
                    scrub,
                })
                .await;
            let principal = match &call.agent {
                Some(agent) => format!("{} via {agent}", call.principal),
                None => call.principal.clone(),
            };
            // The run record is operational: a failure to write it is
            // reported in the log and never turns a finished call into a
            // failed one.
            if let Err(e) = record_run(
                &self.control,
                &entry,
                &call.space,
                &principal,
                started_at,
                &run,
            )
            .await
            {
                eprintln!(
                    "ordo: $fnruns write failed for {}: {e}",
                    entry.decl.name.name
                );
            }
            run.outcome.map_err(exec_error)
        })
    }
}

/// The one mapping from a function's outcome to the door's error.
pub fn exec_error(e: FnError) -> ExecError {
    let code = match &e {
        FnError::Threw { .. } | FnError::BadOutput { .. } => ErrorCode::FunctionError,
        FnError::Timeout { .. } => ErrorCode::FunctionTimeout,
        FnError::OutOfMemory { .. } => ErrorCode::FunctionOom,
        FnError::Busy { .. } => ErrorCode::FunctionBusy,
        FnError::Fetch { .. } => ErrorCode::EgressRefused,
        FnError::BadInput { .. } => ErrorCode::InvalidParam,
        FnError::Engine { .. } => ErrorCode::Internal,
    };
    ExecError::new(code, e.to_string())
}

/// One `$fnruns` row. Never the input, the answer or a body.
pub async fn record_run(
    pool: &Pool,
    entry: &FnEntry,
    space: &str,
    principal: &str,
    started_at: u64,
    run: &ordo_fn::FnRun,
) -> Result<(), PoolError> {
    let r = &run.report;
    let message = match &run.outcome {
        Ok(_) => String::new(),
        Err(e) => e.to_string(),
    };
    let logs = serde_json::to_string(
        &r.logs
            .iter()
            .map(|l| serde_json::json!({ "level": l.level, "text": l.text }))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());
    pool.rows(&format!(
        "INSERT INTO \"$fnruns\" (\"function\", \"version\", \"space\", \"wake\", \"principal\", \
           \"started_at\", \"wall_ms\", \"cpu_ms\", \"peak_memory\", \"fetches\", \"hosts\", \
           \"bytes_out\", \"bytes_in\", \"outcome\", \"message\", \"logs\") \
         VALUES ({}, {}, {}, 'call', {}, {started_at}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
        sql_lit(&entry.decl.name.name),
        sql_lit(&entry.version),
        sql_lit(space),
        sql_lit(&format!(
            "function:{}@{} on behalf of {principal}",
            entry.decl.name.name, entry.version
        )),
        r.wall.as_millis(),
        r.cpu_ms,
        r.peak_memory,
        r.fetches,
        sql_lit(&r.hosts.join(",")),
        r.bytes_out,
        r.bytes_in,
        sql_lit(run.class()),
        sql_lit(&message),
        sql_lit(&logs),
    ))
    .await
    .map(|_| ())
}

/// One `$fnruns` row, read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub function: String,
    pub version: String,
    pub space: String,
    pub principal: String,
    pub outcome: String,
    pub fetches: u64,
    pub hosts: String,
    pub logs: String,
}

/// Every run in `space`, oldest first.
pub async fn runs(pool: &Pool, space: &str) -> Result<Vec<RunRow>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"function\", \"version\", \"space\", \"principal\", \"outcome\", \
               \"fetches\", \"hosts\", \"logs\" \
             FROM \"$fnruns\" WHERE \"space\" = {} ORDER BY \"seq\"",
            sql_lit(space)
        ))
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let cell = |i: usize| r.get(i).cloned().flatten().unwrap_or_default();
            RunRow {
                function: cell(0),
                version: cell(1),
                space: cell(2),
                principal: cell(3),
                outcome: cell(4),
                fetches: cell(5).parse().unwrap_or(0),
                hosts: cell(6),
                logs: cell(7),
            }
        })
        .collect())
}

/// A run id for a call that sent no `Idempotency-Key`.
fn fresh_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("run-{nanos:x}-{}", N.fetch_add(1, Ordering::Relaxed))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
