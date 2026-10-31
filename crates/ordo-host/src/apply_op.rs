//! 18l — the door in front of the apply: `std.deployment.apply`.
//!
//! # What this module is, stated
//!
//! `schema_apply` (13b) is the apply itself: plan, assess, triage, execute,
//! record, over a pool and a compiled program, returning structured
//! outcomes. The owner's constraint on it was that the apply belongs to the
//! service, and the consequence named at the time was that 18l is a
//! **wrapper rather than a second implementation** — a hosted control plane
//! and a self-hoster's CLI cannot drift into two different migrators.
//!
//! So this module adds exactly three things `schema_apply` deliberately
//! does not have, and nothing else:
//!
//! - **The sequence one `ordo apply` is.** `schema_apply` exposes the
//!   pieces; the order they go in (the target's state, then the artifact,
//!   then the activation) was written out inside `ordo-cli`'s `apply`
//!   command and lived nowhere a server could call it. [`run`] is that
//!   sequence, moved here, and `ordo apply` now calls it. One body, two
//!   front doors.
//! - **A structured outcome.** [`Report`] is what the sequence did, as
//!   data: the version, the schema line, whether the artifact was stored,
//!   and what activation happened. It renders to the text an operator
//!   reads and serialises to the rows the op returns, so the direct path
//!   and the wire print the same thing because they print the same value.
//! - **Serialization.** Two applies at once must queue, not interleave.
//!   See [`ApplyLock`].
//!
//! # Who may call it
//!
//! A **deployment owner**, and the distinction from a space owner is the
//! whole gate. Three mechanisms stack, and each one is refusing something
//! the others do not:
//!
//! 1. `platform_ops::owner_only` lists this op, so
//!    `engine_from_members` grants it to [`Role::Owner`] alone. That
//!    refuses every manager, editor and viewer.
//! 2. `require_step_up` covers every platform op, which is what makes this
//!    one structurally human-only: `security-logging-design.md` §5 pins
//!    that a capability token can never satisfy step-up, so no delegated
//!    agent and no third-party client can apply a schema however its
//!    allowlist reads.
//! 3. [`ApplyOps::apply`] refuses unless the bound space is the
//!    deployment's **own** — [`ApplyOps::home`], the space the binary was
//!    configured with (`ORDO_SPACE`), which is the platform's home rather
//!    than a tenant a customer was given. That is the half neither of the
//!    others can express: the access profile is deployment-wide policy
//!    installed in *every* space, so an owner of a tenant space passes
//!    checks 1 and 2 and is still a space member.
//!
//! Stated the other way, because it is the sentence that matters: **being
//! the owner of a space you were given is not being the owner of the
//! deployment that serves it**, and without check 3 every tenant owner on
//! a shared deployment could replace its program.
//!
//! ## Why check 3 compares a name and does not ask `is_system_space`
//!
//! 17v gives the platform's own space a reserved `$` prefix and
//! `ordo_exec::erase::is_system_space` is the one answer to "is this space
//! the platform's own", so asking it here is the obvious spelling. It does
//! not work yet, and the reason is worth recording rather than
//! rediscovering: **`ordo_api::router::well_formed_space` refuses a `$`**.
//! Its alphabet is ASCII alphanumerics, `_` and `-`, so a `$`-prefixed
//! space id cannot be bound at the door at all — `missing_space`, 400,
//! before any op is reached. Until that alphabet admits the prefix, an op
//! gated on `is_system_space` would be an op nobody could call.
//!
//! So the check is `space == home`, which is exact today and is the
//! narrower statement in any case: one deployment has one home, and the
//! prefix question would admit any system space on a deployment that grows
//! a second one. When the door learns to bind a `$` id, this comparison
//! keeps working unchanged, because the configured space is the one that
//! gets the name.
//!
//! # What the caller is recorded as
//!
//! `schema_apply::Record::actor` is written into `$migrations` and into the
//! deployment's security stream. `ordo apply --as <actor>` asserts that
//! string: the module doc on `ordo-cli`'s operator surface says so plainly,
//! because the CLI holds database credentials and checks authorization
//! rather than identity.
//!
//! Over this door there is no `--as`, and refusing to take one is the
//! point. The actor is `$caller` — the principal `IdentityAccess::check`
//! proved from the bearer token, router-inserted, a reserved key no op
//! parameter can shadow (R-G13). An **attested** actor rather than an
//! asserted one, which is the difference between a `$migrations` row that
//! records who someone said they were and one that records who the door
//! decided they were.

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, ExecError, ExecResult};
use ordo_exec::{registry, Pool};
use ordo_schema::diff::Change;
use serde_json::{json, Value};

use crate::schema_apply::{self as sa, Program, SchemaState};

/// `std.deployment.apply` — apply a program to this deployment.
///
/// `std.deployment.*` rather than `std.space.*`, and the namespace is the
/// gate restated in the name. Every other platform op acts on the space it
/// is bound to; this one acts on the deployment that holds every space, and
/// a name that read as a space's would invite exactly the reading check 3
/// above exists to refuse.
pub const APPLY: &str = "std.deployment.apply";

/// The op string recorded in `$migrations` and on the security stream for
/// an apply that came through this door.
///
/// Distinct from `ordo apply`'s, deliberately: the two paths are the same
/// code and different acts, and an auditor reading the table must be able
/// to tell an attested apply from one made by whoever held the database
/// password.
pub const APPLY_OP_LABEL: &str = "std.deployment.apply";

// ── function code over the door ─────────────────────────────────────────

/// The most characters the door's apply takes as function code, all files
/// together. The op route's body is capped at 2 MB
/// (`ordo_api::OP_BODY_LIMIT`) and the program text may take half of it.
/// A larger bundle is applied with `ordo apply --db`, which reads the files
/// itself and takes each up to [`crate::functions::MAX_CODE_BYTES`].
pub const MAX_DOOR_CODE: usize = 1024 * 1024;

/// One function's code as the door's apply carries it: the hash the caller
/// read and the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeFile {
    pub hash: String,
    pub text: String,
}

/// The `code` parameter of `std.deployment.apply`: each `code "..."` path
/// the program names, to its file.
pub type CodeFiles = std::collections::BTreeMap<String, CodeFile>;

/// The `code` parameter's text for `files`, as `ordo apply --host` sends it.
pub fn code_text(files: &CodeFiles) -> String {
    let map: serde_json::Map<String, Value> = files
        .iter()
        .map(|(path, f)| (path.clone(), json!({ "hash": f.hash, "text": f.text })))
        .collect();
    Value::Object(map).to_string()
}

/// Reads the `code` parameter. Absent is no code, which is right for a
/// program with no functions.
pub fn parse_code(raw: Option<&str>) -> Result<CodeFiles, ApplyRefused> {
    let Some(raw) = raw else {
        return Ok(CodeFiles::new());
    };
    let shape = || {
        ApplyRefused::Code(
            "`code` is not a JSON object from each code path to {\"hash\", \"text\"}".into(),
        )
    };
    let value: Value = serde_json::from_str(raw).map_err(|_| shape())?;
    let map = value.as_object().ok_or_else(shape)?;
    let mut out = CodeFiles::new();
    for (path, f) in map {
        let (Some(hash), Some(text)) = (f["hash"].as_str(), f["text"].as_str()) else {
            return Err(shape());
        };
        out.insert(
            path.clone(),
            CodeFile {
                hash: hash.to_string(),
                text: text.to_string(),
            },
        );
    }
    Ok(out)
}

/// Binds each function's code into `program`, as the local apply does:
/// every declared `code` path must be carried, and each text must hash to
/// the hash the caller read. Answers the texts to store, by hash.
fn bind_code(
    program: &mut Program,
    code: &CodeFiles,
) -> Result<std::collections::BTreeMap<String, Vec<u8>>, ApplyRefused> {
    let mut hashes = std::collections::BTreeMap::new();
    let mut store = std::collections::BTreeMap::new();
    for f in &program.schema.functions {
        let Some(file) = code.get(&f.code) else {
            return Err(ApplyRefused::Code(format!(
                "function `{}`: its code `{}` was not sent. `ordo apply --host` reads it \
                 from beside the program; check the file is there",
                f.name.name, f.code
            )));
        };
        let bytes = file.text.as_bytes();
        if bytes.len() > crate::functions::MAX_CODE_BYTES {
            return Err(ApplyRefused::Code(format!(
                "function `{}`: `{}` is {} bytes, over the limit of {}",
                f.name.name,
                f.code,
                bytes.len(),
                crate::functions::MAX_CODE_BYTES
            )));
        }
        let hash = ordo_plan::artifact::code_hash(bytes);
        if hash != file.hash {
            return Err(ApplyRefused::Code(format!(
                "function `{}`: `{}` hashes to {hash} at the door and the caller read {}. \
                 The file changed or was cut short on the way; apply again",
                f.name.name, f.code, file.hash
            )));
        }
        hashes.insert(f.code.clone(), hash.clone());
        store.insert(hash, bytes.to_vec());
    }
    program.bind_code(&hashes).map_err(ApplyRefused::Code)?;
    Ok(store)
}

// ── the lock ────────────────────────────────────────────────────────────

/// The advisory-lock key two applies contend on.
///
/// **Derived from the `$migrations` table's name, not chosen**, so the lock
/// is named by the thing it guards: `$migrations` is the record every apply
/// writes and the table whose ordering a half-interleaved pair of applies
/// would make meaningless. FNV-1a over the name, at compile time, so the
/// construction is readable and the value is the same in every process
/// without a round trip to ask Postgres for an OID — which would not exist
/// on a first apply anyway, because the table is created by the apply.
pub const APPLY_LOCK: i64 = fnv1a(b"$migrations");

const fn fnv1a(bytes: &[u8]) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    // Into the signed range `pg_advisory_lock(bigint)` takes. The top bit
    // is dropped rather than wrapped, because a key that reads as negative
    // in `pg_locks` is a key an operator cannot search for.
    (hash & 0x7fff_ffff_ffff_ffff) as i64
}

/// How long a second apply waits for the first before it refuses.
///
/// A bound rather than an unbounded wait, and rather than an immediate
/// refusal. Unbounded would hold a connection and a request open behind an
/// apply that is itself stuck; immediate would turn two deploys that
/// arrived a second apart into a failed pipeline for no reason. Thirty
/// seconds is longer than every class-A apply the triage will send (they
/// are one transaction plus concurrent index builds) and shorter than any
/// HTTP client's patience.
pub const LOCK_WAIT_MS: u64 = 30_000;

/// The held lock. Dropping it ends the transaction and releases.
///
/// An advisory lock at **transaction** scope on a dedicated connection,
/// rather than a session lock or a row in a table. The reasons, in order:
///
/// - *Transaction scope, not session*: the connection is a pooled one, and
///   a session lock left behind by a task that was cancelled mid-apply
///   would be a lock nothing releases until the process restarts. A
///   transaction's locks go when the transaction does, and the transaction
///   goes when this guard drops — including on a panic, and including when
///   the socket closes under it.
/// - *An advisory lock, not a row*: a `$migrations` row marking "in
///   progress" is durable, which sounds like a virtue and is the defect. A
///   crashed apply would leave one, and every later apply would refuse
///   until a human cleared it by hand. The failure mode of the advisory
///   lock is that a crashed apply releases; the failure mode of the row is
///   that a crashed apply wedges the deployment.
/// - *The lock takes nothing else*: this transaction reads no table and
///   writes none, so it cannot deadlock against the DDL the apply is about
///   to run on other connections, and it holds no snapshot open.
pub struct ApplyLock {
    tx: Option<ordo_exec::TxConn>,
}

impl ApplyLock {
    /// Take the lock, waiting up to [`LOCK_WAIT_MS`].
    ///
    /// `Ok(None)` is *somebody else is applying*, which is a refusal the
    /// caller names rather than an error.
    pub async fn take(pool: &Pool) -> Result<Option<ApplyLock>, String> {
        let tx = pool
            .begin()
            .await
            .map_err(|e| format!("taking the apply lock: {e}"))?;
        // `SET LOCAL`, so both die with this transaction and neither leaks
        // onto the next borrower of this pooled connection.
        //
        // The statement bound is lifted here and only here (20a). Every
        // pooled connection carries `statement_timeout` from its dial, and
        // waiting for this lock is a statement that is *supposed* to take
        // as long as `lock_timeout` allows. Two bounds of the same size
        // racing on one statement is a coin flip, and the wrong side of it
        // reports `statement_timeout` for a lock somebody else is holding,
        // which is a driver failure where the caller should have read
        // `apply_in_progress`. The wait stays bounded by `lock_timeout`,
        // which is the bound that knows what it is waiting for.
        tx.rows(&format!(
            "SET LOCAL lock_timeout = '{LOCK_WAIT_MS}ms'; SET LOCAL statement_timeout = 0"
        ))
        .await
        .map_err(|e| format!("taking the apply lock: {e}"))?;
        match tx
            .rows(&format!("SELECT pg_advisory_xact_lock({APPLY_LOCK})"))
            .await
        {
            Ok(_) => Ok(Some(ApplyLock { tx: Some(tx) })),
            // A `lock_timeout` expiry is the one error that means "held",
            // and it is told apart by its type (`PoolError::LockTimeout`,
            // SQLSTATE `55P03` `lock_not_available`), never its text.
            // Anything else is a real failure and stays one.
            Err(e) if e.is_lock_timeout() => Ok(None),
            Err(e) => Err(format!("taking the apply lock: {e}")),
        }
    }

    /// Release now rather than at drop, reporting a failure to end the
    /// transaction. Callers that do not care may simply drop the guard.
    pub async fn release(mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.rollback().await;
        }
    }
}

// ── the outcome ─────────────────────────────────────────────────────────

/// What the schema half of an apply did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaOutcome {
    /// An empty target: the whole schema was created.
    Created {
        tables: usize,
        migration: i64,
        event: String,
    },
    /// The live schema already holds everything the program asserts.
    /// `pending` counts the drops the gate tolerates.
    Matches { pending: usize },
    /// The database was behind by safe additive changes only, and the apply
    /// made them itself (`ORDO_APPLY_SAFE_CHANGES`, on by default), the way
    /// `ordo migrate --apply` does, with the same `$migrations` record.
    Migrated {
        changes: Vec<(String, String)>,
        migration: i64,
        event: String,
    },
    /// The live schema lacks something the program asserts. The artifact is
    /// stored; nothing is activated over it.
    Behind { changes: Vec<(String, String)> },
}

/// What the activation half did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    Switched {
        from: Option<String>,
        to: String,
    },
    AlreadyActive {
        version: String,
    },
    /// `--no-activate`: stored, and the deployment still points where it
    /// did.
    Staged,
}

/// Whether the artifact was written or was already there byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    Written,
    AlreadyPresent,
}

/// What one apply did, as data.
///
/// The value both front doors render, which is what makes "the CLI over the
/// wire prints what the direct path prints" a property of the code rather
/// than of two format strings somebody keeps in step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub version: String,
    /// Ops the artifact records an error for that bind correctly when a
    /// caller is supplied. Not a reason to refuse; said out loud anyway.
    pub notes: Vec<String>,
    pub schema: SchemaOutcome,
    pub stored: Stored,
    pub activation: Activation,
    /// What the apply did to each tenant's own database.
    pub tenants: TenantsReport,
}

/// What an apply did to the databases tenants are served from, apart from
/// the deployment's own.
///
/// A count for each outcome and a line for each database that was not
/// brought forward, so the answer stays short however many people have a
/// tenant. A database named under `held` keeps its tenant closed. The
/// catch-up retries it when the reason may pass (the database was
/// unreachable, or slow), and leaves it when the change was refused by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TenantsReport {
    /// Databases that were behind and now record this version.
    pub advanced: usize,
    /// Databases that already recorded it.
    pub current: usize,
    /// `(target, reason)` for each database that was not brought forward.
    pub held: Vec<(String, String)>,
}

/// Why an apply did not happen.
///
/// Each variant carries the **class** the direct path prints, so the two
/// paths name a refusal identically; [`ApplyRefused::class`] is that name
/// and [`ApplyRefused::code`] is the wire code it travels as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyRefused {
    /// The registry names a serving version and none of its tables exist.
    Ghost { serving: String },
    /// Ops in this program do not compile, so no node could load the
    /// artifact.
    Broken { ops: Vec<String> },
    /// The program text does not compile at all: a parse error, a
    /// semantic one, a DDL that cannot be rendered.
    ///
    /// Distinct from `Internal`, and the distinction is who can fix it.
    /// This is the caller's text; `Internal` is the deployment's own
    /// failure. Over the wire that is the difference between a 409 the
    /// operator acts on and a 500 that pages somebody.
    DoesNotCompile(String),
    /// The load gate would refuse this artifact over this database.
    /// Stored, not activated.
    NotActivated {
        version: String,
        changes: Vec<(String, String)>,
    },
    /// Another apply holds the deployment.
    InProgress,
    /// An activation named a version the registry does not hold
    /// (`std.deployment.activate`, `ordo rollback`).
    UnknownVersion(String),
    /// A function's code is missing from the apply, does not match the
    /// hash the caller read, or is over the size limit. The caller's to fix.
    Code(String),
    /// A step before the decision failed: a count, a DDL rendering, the
    /// registry.
    Internal(String),
}

impl ApplyRefused {
    /// The greppable class, identical on both paths.
    pub fn class(&self) -> &'static str {
        match self {
            ApplyRefused::Ghost { .. } => "serving_schema_drifted",
            ApplyRefused::Broken { .. } => "ops_do_not_compile",
            ApplyRefused::DoesNotCompile(_) => "program_does_not_compile",
            ApplyRefused::NotActivated { .. } => "not_activated",
            ApplyRefused::InProgress => "apply_in_progress",
            ApplyRefused::UnknownVersion(_) => "unknown_version",
            ApplyRefused::Code(_) => "function_code_refused",
            ApplyRefused::Internal(_) => "internal",
        }
    }

    /// The wire code this refusal travels as.
    pub fn code(&self) -> ErrorCode {
        match self {
            // The class the registry-over-no-tables case already had a
            // name for on the wire. Same fact, reached from the deploy
            // path rather than from the export.
            ApplyRefused::Ghost { .. } => ErrorCode::SchemaDrift,
            ApplyRefused::Broken { .. }
            | ApplyRefused::DoesNotCompile(_)
            | ApplyRefused::Code(_) => ErrorCode::ApplyRefused,
            ApplyRefused::NotActivated { .. } => ErrorCode::NotActivated,
            ApplyRefused::InProgress => ErrorCode::ApplyInProgress,
            // A version that is not there is the same fact a missing row
            // is anywhere else, and the executor may say it.
            ApplyRefused::UnknownVersion(_) => ErrorCode::NotFound,
            ApplyRefused::Internal(_) => ErrorCode::Internal,
        }
    }
}

impl std::fmt::Display for ApplyRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyRefused::Ghost { serving } => write!(
                f,
                "serving_schema_drifted — the registry says {serving} is serving, and none \
                 of its tables exist. This is not an empty target: restore the database, or \
                 clear `$artifacts` and `$activations` if the registry is what is wrong."
            ),
            ApplyRefused::Broken { ops } => write!(
                f,
                "{} op(s) in this program do not compile, so no node could load the \
                 artifact:\n{}",
                ops.len(),
                ops.join("\n")
            ),
            ApplyRefused::NotActivated { version, changes } => {
                write!(
                    f,
                    "not activated: no node could load {version} over this database. \
                     {} change(s) the load gate would refuse on:",
                    changes.len()
                )?;
                for (code, text) in changes {
                    write!(f, "\n  {code:<3} {text}")?;
                }
                Ok(())
            }
            ApplyRefused::DoesNotCompile(e) => {
                write!(f, "program_does_not_compile — {e}")
            }
            ApplyRefused::InProgress => write!(
                f,
                "apply_in_progress — another apply holds this deployment. Applies serialize \
                 on `$migrations`, so two of them queue rather than interleave; this one \
                 waited {}ms and gave the connection back. Retry.",
                LOCK_WAIT_MS
            ),
            ApplyRefused::UnknownVersion(v) => write!(
                f,
                "unknown_version: no program is stored under version `{v}`. The stored \
                 versions are listed by `std.deployment.versions` (`ordo deploys`); apply \
                 the program to store it first."
            ),
            ApplyRefused::Code(e) => write!(f, "function_code_refused: {e}"),
            ApplyRefused::Internal(e) => write!(f, "{e}"),
        }
    }
}

// ── the sequence ────────────────────────────────────────────────────────

/// Compile `source`, then apply it: the whole of one `ordo apply`.
///
/// `activate: false` is `--no-activate` — store the artifact and leave the
/// deployment pointing where it does.
///
/// `code` is each function's code (see [`CodeFiles`]). It is checked and
/// bound into the manifest before the version is taken, and stored by hash
/// before the version that names it, so a node that loads the version
/// always finds its code. A secret with no value does not block this: the
/// function answers `secret_not_set` until `ordo secret set` runs.
pub async fn run(
    pool: &Pool,
    pools: &dyn ordo_exec::route::PoolResolver,
    source: &str,
    code: &CodeFiles,
    actor: &str,
    op: &str,
    activate: bool,
) -> Result<Report, ApplyRefused> {
    let (mut program, broken, notes) = compile_checked(source)?;
    if !broken.is_empty() {
        return Err(ApplyRefused::Broken { ops: broken });
    }
    let store = bind_code(&mut program, code)?;
    for bytes in store.values() {
        crate::functions::store_code(pool, bytes)
            .await
            .map_err(|e| ApplyRefused::Internal(format!("storing function code: {e}")))?;
    }
    run_program(pool, pools, &program, notes, actor, op, activate).await
}

/// Compile `source` and split the artifact's op errors (see
/// [`triage_errors`]): the program, the ops that refuse an apply, and the
/// notes that do not. One body for [`run`] and [`preview`], so a preview
/// refuses exactly what an apply would.
fn compile_checked(source: &str) -> Result<(Program, Vec<String>, Vec<String>), ApplyRefused> {
    // `DoesNotCompile`, never `Internal`: this is the caller's text, and a
    // 500 for a program with a typo in it is a page for somebody who can
    // do nothing about it.
    let program = Program::compile(source).map_err(ApplyRefused::DoesNotCompile)?;
    let registry = crate::Fixture::from_source(source, "")
        .map_err(ApplyRefused::DoesNotCompile)?
        .registry;
    let (broken, notes) = triage_errors(&program.doc, &program.schema, &registry);
    Ok((program, broken, notes))
}

/// The same sequence over an already-compiled program, for a caller that
/// compiled it for other reasons too (which `ordo apply` does: it builds
/// the artifact it stores from the same document).
pub async fn run_program(
    pool: &Pool,
    pools: &dyn ordo_exec::route::PoolResolver,
    program: &Program,
    notes: Vec<String>,
    actor: &str,
    op: &str,
    activate: bool,
) -> Result<Report, ApplyRefused> {
    // **The lock, before anything reads the schema.** Taken here rather
    // than around the DDL alone, because what must not interleave is the
    // *decision*: two applies that both read an empty target and then both
    // create it are two correct executions of a sequence that raced at its
    // first read.
    let lock = match ApplyLock::take(pool).await {
        Ok(Some(lock)) => lock,
        Ok(None) => return Err(ApplyRefused::InProgress),
        Err(e) => return Err(ApplyRefused::Internal(e)),
    };
    let out = sequence(&lock, pool, pools, program, notes, actor, op, activate).await;
    lock.release().await;
    out
}

#[allow(clippy::too_many_arguments)]
async fn sequence(
    // **Proof this runs under the apply lock** (18d R-S5). Taken by the
    // caller one line before this, and passed rather than re-taken: it is
    // `pg_advisory_xact_lock` on a dedicated connection, so a second take
    // here would block on the first forever.
    lock: &ApplyLock,
    pool: &Pool,
    pools: &dyn ordo_exec::route::PoolResolver,
    program: &Program,
    notes: Vec<String>,
    actor: &str,
    op: &str,
    activate: bool,
) -> Result<Report, ApplyRefused> {
    registry::ensure_tables(pool)
        .await
        .map_err(|e| ApplyRefused::Internal(format!("registry tables: {e}")))?;

    // The schema, before the artifact: a first apply creates it, a match
    // proceeds, and a database that is behind stores but does not activate.
    let mut behind: Vec<(String, String)> = Vec::new();
    let schema = match sa::schema_state(pool, &program.schema)
        .await
        .map_err(ApplyRefused::Internal)?
    {
        SchemaState::Ghost { serving } => return Err(ApplyRefused::Ghost { serving }),
        SchemaState::Empty => {
            let c = sa::create_schema(pool, program, actor, op)
                .await
                .map_err(ApplyRefused::Internal)?;
            SchemaOutcome::Created {
                tables: c.tables,
                migration: c.migration,
                event: c.event,
            }
        }
        SchemaState::Matches { pending } => {
            // A program that only stops declaring a field applies no DDL,
            // so no `$migrations` row says this database is at the new
            // version. Recorded here, on the same terms a tenant's
            // database gets it, or a space routed to this database would
            // read as behind the version it is serving.
            if let Err(e) = sa::record_version_if_behind(pool, program, actor, op).await {
                eprintln!("apply: the deployment's own database could not record its version: {e}");
            }
            SchemaOutcome::Matches { pending }
        }
        SchemaState::Behind(changes) => {
            match sa::apply_safe(pool, program, actor, op)
                .await
                .map_err(ApplyRefused::Internal)?
            {
                sa::SafeApply::Applied {
                    changes,
                    migration,
                    event,
                } => SchemaOutcome::Migrated {
                    changes,
                    migration,
                    event,
                },
                sa::SafeApply::NotSafe => {
                    behind = changes.iter().map(named).collect();
                    SchemaOutcome::Behind {
                        changes: behind.clone(),
                    }
                }
            }
        }
    };

    let artifact = registry::Artifact {
        version: program.version.clone(),
        artifact: ordo_plan::artifact::to_bytes(&program.doc),
        source: program.source.clone(),
        ddl: program.ddl.clone(),
        applied_at: ordo_commandlog::Timestamp(now_secs()),
        actor: actor.to_string(),
    };
    let stored = match registry::store(pool, &artifact).await {
        Ok(registry::Applied::Stored(_)) => Stored::Written,
        Ok(registry::Applied::AlreadyStored(_)) => Stored::AlreadyPresent,
        Err(e) => return Err(ApplyRefused::Internal(e.to_string())),
    };

    // **Every tenant's own database** (R-S3, and 25i).
    //
    // The deployment's own database was applied above, by the schema step.
    // What follows brings each other database to the same version, once the
    // deployment's own has accepted it and the version is about to serve:
    // a staged apply (`--no-activate`) leaves them alone, because a
    // database that records a version nothing serves is a tenant held
    // closed for nothing.
    //
    // **A tenant that could not be brought forward does not hold back the
    // activation**, which is the change 25i makes to R-S3's gate. With one
    // tenant per person, one unreachable or refused database would stop
    // every deploy for everybody. The tenant is closed instead, alone, by
    // the schema-behind quarantine (R-S4), with the reason on
    // `/health/ready` and in this report, and the catch-up
    // (`tenant_forward`) retries it. An activation is still gated on the
    // deployment's own database, which is refused above when it is behind.
    //
    // A deployment with no attached target walks a one-element list and
    // this costs a `targets()` call, which is why it is unconditional.
    let fleet = if behind.is_empty() && activate {
        sa::apply_everywhere_except_default(pools, pool, program, actor, op, lock)
            .await
            .map_err(ApplyRefused::Internal)?
    } else {
        sa::Forwarded::default()
    };
    // **The fleet view** (R-S2), written from what each target reported
    // about itself. A write failure here does not fail the apply. This is a
    // cache and never the authority (R-S2).
    if let Err(e) = ordo_exec::fleet::ensure_table(pool).await {
        eprintln!("apply: the fleet view's table could not be ensured: {e}");
    } else {
        let at = crate::runtime::now_secs() as i64;

        // **The control database is one of the targets** (R-S3), and the
        // walk above deliberately excludes it: `sequence` applies it
        // through the schema step further up rather than through
        // `apply_everywhere`. So it is confirmed here, from its own
        // `$migrations` like every other target.
        let mut confirmations: Vec<(String, String)> = Vec::new();
        match ordo_exec::spacedb::recorded_version(pool).await {
            Ok(ordo_exec::spacedb::TargetVersion::At(v)) => {
                confirmations.push((ordo_exec::route::DEFAULT_LABEL.to_string(), v))
            }
            Ok(ordo_exec::spacedb::TargetVersion::Unknown) => confirmations.push((
                ordo_exec::route::DEFAULT_LABEL.to_string(),
                ordo_exec::fleet::UNKNOWN.to_string(),
            )),
            Err(e) => eprintln!("apply: the control database's own version could not be read: {e}"),
        }
        confirmations.extend(
            fleet
                .applied
                .iter()
                .filter_map(|t| t.confirmed.clone().map(|v| (t.label.clone(), v))),
        );

        for (label, version) in confirmations {
            if let Err(e) = ordo_exec::fleet::confirm(pool, &label, &version, at).await {
                eprintln!("apply: the fleet view did not record `{label}`: {e}");
            }
        }
    }
    let tenants = TenantsReport {
        advanced: fleet.advanced(),
        current: fleet.current(),
        held: fleet
            .failures()
            .iter()
            .filter_map(|t| {
                t.outcome
                    .as_ref()
                    .err()
                    .map(|e| (t.label.clone(), e.to_string()))
            })
            .collect(),
    };
    for (target, why) in &tenants.held {
        eprintln!(
            "apply {}: tenant database {target} was not brought forward and stays closed: {why}",
            program.version
        );
    }

    // Behind and asked to activate is the refusal. Behind and staged is a
    // report — that is what `--no-activate` is for.
    if !behind.is_empty() && activate {
        return Err(ApplyRefused::NotActivated {
            version: program.version.clone(),
            changes: behind,
        });
    }
    let activation = if !activate {
        Activation::Staged
    } else {
        match registry::activate(
            pool,
            &program.version,
            actor,
            ordo_commandlog::Timestamp(now_secs()),
        )
        .await
        {
            Ok(registry::Activated::Switched { from, to }) => Activation::Switched { from, to },
            Ok(registry::Activated::AlreadyActive(version)) => {
                Activation::AlreadyActive { version }
            }
            Err(e) => return Err(ApplyRefused::Internal(e.to_string())),
        }
    };

    Ok(Report {
        version: program.version.clone(),
        notes,
        schema,
        stored,
        activation,
        tenants,
    })
}

fn named(c: &Change) -> (String, String) {
    (c.code().to_string(), c.to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The caller the describer did not have.
///
/// A real string, so a guard or a filter comparing against it lowers to
/// real SQL and the bind is the same shape a request's would be; it matches
/// no member, and nothing is executed here. Deliberately the same value
/// `crate::shape`'s load gate poses, because the two must not disagree
/// about which ops are usable.
const APPLY_PROBE_CALLER: &str = "$shape-probe";

/// Split the artifact's recorded errors into the ones that refuse an apply
/// and the ones that are only the describer's blind spot.
///
/// The test is the second bind: an op that fails `bind_op_generic` and
/// succeeds under a supplied caller was never broken.
pub fn triage_errors(
    doc: &Value,
    schema: &ordo_schema::Schema,
    registry: &ordo_sema::Registry,
) -> (Vec<String>, Vec<String>) {
    let mut broken = Vec::new();
    let mut describer_only = Vec::new();
    for op in doc["ops"].as_array().into_iter().flatten() {
        let Some(e) = op["error"].as_object() else {
            continue;
        };
        let name = op["op"].as_str().unwrap_or("");
        let code = e["code"].as_str().unwrap_or("");
        let message = e["message"].as_str().unwrap_or("");
        let binds_with_a_caller = registry
            .get(name)
            .and_then(|o| o.read_pipeline().cloned())
            .is_some_and(|pl| {
                ordo_sema::bind_as(
                    schema,
                    registry,
                    &pl,
                    &[],
                    Some(APPLY_PROBE_CALLER),
                    ordo_sema::Clock::CORPUS,
                )
                .is_ok()
            });
        if binds_with_a_caller {
            describer_only.push(format!(
                "`{name}` is recorded as [{code}] in the artifact and binds correctly when a \
                 caller is supplied — the describer binds without one, so its `plan.sql` is \
                 absent rather than wrong. Not a reason to refuse this deploy."
            ));
        } else {
            broken.push(format!("  {name} [{code}] {message}"));
        }
    }
    (broken, describer_only)
}

// ── activation, history and preview (G1, G2, G3) ────────────────────────

/// Point the deployment at a version it already stores: a program
/// rollback, or the activation of a staged apply. `std.deployment.activate`
/// and `ordo rollback` both call this.
///
/// The same registry act as [`run`]'s activation, under the same lock, so
/// an activation and an apply queue rather than interleave. And the same
/// gate: a version whose tables the database does not hold is refused
/// rather than activated, because no node could load it. A rollback to a
/// version that only lacks something the newer one added passes, since
/// the gate tolerates extra columns.
pub async fn activate(pool: &Pool, version: &str, actor: &str) -> Result<Activation, ApplyRefused> {
    let lock = match ApplyLock::take(pool).await {
        Ok(Some(lock)) => lock,
        Ok(None) => return Err(ApplyRefused::InProgress),
        Err(e) => return Err(ApplyRefused::Internal(e)),
    };
    let out = activate_locked(pool, version, actor).await;
    lock.release().await;
    out
}

async fn activate_locked(
    pool: &Pool,
    version: &str,
    actor: &str,
) -> Result<Activation, ApplyRefused> {
    registry::ensure_tables(pool)
        .await
        .map_err(|e| ApplyRefused::Internal(format!("registry tables: {e}")))?;
    let stored = registry::get(pool, version)
        .await
        .map_err(|e| ApplyRefused::Internal(e.to_string()))?
        .ok_or_else(|| ApplyRefused::UnknownVersion(version.to_string()))?;
    let schema = crate::Fixture::from_source(&stored.source, "")
        .map_err(|e| {
            ApplyRefused::Internal(format!(
                "the stored program {version} no longer compiles with this build: {e}"
            ))
        })?
        .schema;
    match sa::schema_state(pool, &schema)
        .await
        .map_err(ApplyRefused::Internal)?
    {
        SchemaState::Matches { .. } => {}
        SchemaState::Ghost { serving } => return Err(ApplyRefused::Ghost { serving }),
        SchemaState::Behind(changes) => {
            return Err(ApplyRefused::NotActivated {
                version: version.to_string(),
                changes: changes.iter().map(named).collect(),
            })
        }
        SchemaState::Empty => {
            return Err(ApplyRefused::NotActivated {
                version: version.to_string(),
                changes: vec![(
                    String::new(),
                    "none of this version's tables exist; apply the program to create them"
                        .to_string(),
                )],
            })
        }
    }
    match registry::activate(pool, version, actor, ordo_commandlog::Timestamp(now_secs())).await {
        Ok(registry::Activated::Switched { from, to }) => Ok(Activation::Switched { from, to }),
        Ok(registry::Activated::AlreadyActive(version)) => {
            Ok(Activation::AlreadyActive { version })
        }
        Err(registry::RegistryRefusal::UnknownVersion(v)) => Err(ApplyRefused::UnknownVersion(v)),
        Err(e) => Err(ApplyRefused::Internal(e.to_string())),
    }
}

/// What is stored, what serves, and every re-point, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub active: Option<String>,
    /// `(version, applied at, by whom)`, newest first.
    pub stored: Vec<(String, u64, String)>,
    pub activations: Vec<registry::Activation>,
}

/// The deployment's program history: `std.deployment.versions` and `ordo
/// deploys`. `limit` bounds the activation log. A database no apply has
/// reached has no registry yet, and answers an empty history.
pub async fn history(pool: &Pool, limit: usize) -> Result<History, ordo_exec::PoolError> {
    let empty_when_new = |r: Result<History, ordo_exec::PoolError>| match r {
        Err(e) if e.is_missing_table() => Ok(History::default()),
        other => other,
    };
    empty_when_new(
        async {
            Ok(History {
                active: registry::active_version(pool).await?,
                stored: registry::stored(pool)
                    .await?
                    .into_iter()
                    .map(|(v, at, by)| (v, at.0, by))
                    .collect(),
                activations: registry::activations(pool, limit).await?,
            })
        }
        .await,
    )
}

impl History {
    /// The op's result row.
    pub fn to_json(&self) -> Value {
        json!({
            "active": self.active,
            "stored": self.stored.iter().map(|(v, at, by)| json!({
                "version": v,
                "appliedAt": at,
                "actor": by,
                "serving": Some(v) == self.active.as_ref(),
            })).collect::<Vec<_>>(),
            "activations": self.activations.iter().map(|a| json!({
                "seq": a.seq,
                "version": a.version,
                "at": a.at.0,
                "actor": a.actor,
            })).collect::<Vec<_>>(),
        })
    }
}

/// What an apply of a program would do, worked out and not done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Would {
    /// The apply would refuse: ops that do not compile, or a registry over
    /// no tables.
    Refuse(ApplyRefused),
    /// An empty target: the schema would be created.
    Create { entities: usize },
    /// The database already holds the program's schema.
    Match { pending: usize },
    /// The database is behind by safe changes only, and the apply would
    /// make them itself (`ORDO_APPLY_SAFE_CHANGES`).
    Migrate,
    /// The database is behind by changes that need the app developer's
    /// say: the apply would store the program and not activate it.
    Hold,
}

/// One change the preview found, with what refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewChange {
    pub code: String,
    pub change: String,
    /// `A` to `D` (`plan-schema-evolution.md` §5).
    pub class: String,
    /// The refusal classes raised against it. A probe that found rows says
    /// so here by class and question; the rows' values are never carried.
    pub refused: Vec<String>,
}

/// A preview of an apply: `std.deployment.plan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub version: String,
    pub serving: Option<String>,
    /// The registry already holds this exact version.
    pub already_stored: bool,
    pub notes: Vec<String>,
    pub would: Would,
    pub changes: Vec<PreviewChange>,
    /// `ORDO_APPLY_SAFE_CHANGES`, as this node reads it.
    pub safe_changes: bool,
}

/// Compile `source` and work out what [`run`] would do with it over this
/// database, storing nothing and changing nothing: the same compile, the
/// same op triage, the same schema state and the same safe-change test the
/// apply makes. Takes no lock: it writes nothing, and an apply that lands
/// meanwhile is what the next preview reads.
pub async fn preview(pool: &Pool, source: &str) -> Result<Preview, ApplyRefused> {
    let (program, broken, notes) = compile_checked(source)?;
    let serving = registry::active_version(pool).await.ok().flatten();
    let already_stored = registry::get(pool, &program.version)
        .await
        .ok()
        .flatten()
        .is_some();
    let plan = sa::plan_with(pool, &program, &sa::Choices::default(), None)
        .await
        .map_err(ApplyRefused::Internal)?;
    let changes = plan
        .assessed
        .iter()
        .map(|a| PreviewChange {
            code: a.change.code().to_string(),
            change: a.change.to_string(),
            class: format!("{:?}", a.change.class()),
            refused: a.refused.clone(),
        })
        .collect();
    let safe_changes = sa::safe_changes_enabled();
    let would = if !broken.is_empty() {
        Would::Refuse(ApplyRefused::Broken { ops: broken })
    } else {
        match sa::schema_state(pool, &program.schema)
            .await
            .map_err(ApplyRefused::Internal)?
        {
            SchemaState::Ghost { serving } => Would::Refuse(ApplyRefused::Ghost { serving }),
            SchemaState::Empty => Would::Create {
                entities: program.schema.entities.len(),
            },
            SchemaState::Matches { pending } => Would::Match { pending },
            SchemaState::Behind(_) if safe_changes && sa::all_safe(&plan) => Would::Migrate,
            SchemaState::Behind(_) => Would::Hold,
        }
    };
    Ok(Preview {
        version: program.version,
        serving,
        already_stored,
        notes,
        would,
        changes,
        safe_changes,
    })
}

impl Preview {
    /// The op's result row.
    pub fn to_json(&self) -> Value {
        let would = match &self.would {
            Would::Refuse(r) => json!({
                "state": "refused",
                "class": r.class(),
                "code": r.code().as_str(),
                "message": r.to_string(),
            }),
            Would::Create { entities } => json!({ "state": "create", "entities": entities }),
            Would::Match { pending } => json!({ "state": "matches", "pending": pending }),
            Would::Migrate => json!({ "state": "migrate" }),
            Would::Hold => json!({ "state": "held" }),
        };
        json!({
            "version": self.version,
            "serving": self.serving,
            "alreadyStored": self.already_stored,
            "notes": self.notes,
            "would": would,
            "safeChanges": self.safe_changes,
            "safeChangesSetting": sa::SAFE_CHANGES_SETTING,
            "changes": self.changes.iter().map(|c| json!({
                "code": c.code,
                "change": c.change,
                "class": c.class,
                "refused": c.refused,
            })).collect::<Vec<_>>(),
        })
    }
}

// ── the wire shape ──────────────────────────────────────────────────────

impl Report {
    /// The op's result row.
    ///
    /// Hand-built rather than derived, because this is a **wire format**:
    /// the same argument `ordo-identity`'s token MAC and
    /// `ordo-commandlog`'s chain hashing make for their own bytes. A derive
    /// would make the shape a consequence of a field name somebody renames.
    pub fn to_json(&self) -> Value {
        let schema = match &self.schema {
            SchemaOutcome::Created {
                tables,
                migration,
                event,
            } => json!({
                "state": "created",
                "tables": tables,
                "migration": migration,
                "event": event,
            }),
            SchemaOutcome::Matches { pending } => json!({
                "state": "matches",
                "pending": pending,
            }),
            SchemaOutcome::Migrated {
                changes,
                migration,
                event,
            } => json!({
                "state": "migrated",
                "migration": migration,
                "event": event,
                "changes": changes.iter()
                    .map(|(c, t)| json!({ "code": c, "change": t }))
                    .collect::<Vec<_>>(),
            }),
            SchemaOutcome::Behind { changes } => json!({
                "state": "behind",
                "changes": changes.iter()
                    .map(|(c, t)| json!({ "code": c, "change": t }))
                    .collect::<Vec<_>>(),
            }),
        };
        let activation = match &self.activation {
            Activation::Switched { from, to } => json!({
                "state": "switched", "from": from, "to": to,
            }),
            Activation::AlreadyActive { version } => json!({
                "state": "alreadyActive", "version": version,
            }),
            Activation::Staged => json!({ "state": "staged" }),
        };
        json!({
            "version": self.version,
            "notes": self.notes,
            "schema": schema,
            "stored": match self.stored {
                Stored::Written => "written",
                Stored::AlreadyPresent => "alreadyPresent",
            },
            "activation": activation,
            "tenants": {
                "advanced": self.tenants.advanced,
                "current": self.tenants.current,
                "held": self.tenants.held.iter()
                    .map(|(t, r)| json!({ "target": t, "reason": r }))
                    .collect::<Vec<_>>(),
            },
        })
    }

    /// Read a report back off the wire.
    ///
    /// Every unreadable field is an error rather than a default. A client
    /// that rendered a missing `schema` as "matches" would tell an operator
    /// their database is current on the strength of a parse failure.
    pub fn from_json(v: &Value) -> Result<Report, String> {
        let s = |k: &str| -> Result<String, String> {
            v[k].as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("the apply's result carried no `{k}`"))
        };
        let schema = &v["schema"];
        let schema = match schema["state"].as_str() {
            Some("created") => SchemaOutcome::Created {
                tables: usize::try_from(schema["tables"].as_u64().unwrap_or_default())
                    .unwrap_or_default(),
                migration: schema["migration"].as_i64().unwrap_or_default(),
                event: schema["event"].as_str().unwrap_or_default().to_string(),
            },
            Some("matches") => SchemaOutcome::Matches {
                pending: usize::try_from(schema["pending"].as_u64().unwrap_or_default())
                    .unwrap_or_default(),
            },
            Some("migrated") => SchemaOutcome::Migrated {
                migration: schema["migration"].as_i64().unwrap_or_default(),
                event: schema["event"].as_str().unwrap_or_default().to_string(),
                changes: schema["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        (
                            c["code"].as_str().unwrap_or_default().to_string(),
                            c["change"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect(),
            },
            Some("behind") => SchemaOutcome::Behind {
                changes: schema["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| {
                        (
                            c["code"].as_str().unwrap_or_default().to_string(),
                            c["change"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect(),
            },
            other => {
                return Err(format!(
                    "the apply's result named schema state {other:?}, which this client does \
                     not know; upgrade the client rather than guessing at what happened"
                ))
            }
        };
        let a = &v["activation"];
        let activation = match a["state"].as_str() {
            Some("switched") => Activation::Switched {
                from: a["from"].as_str().map(str::to_string),
                to: a["to"].as_str().unwrap_or_default().to_string(),
            },
            Some("alreadyActive") => Activation::AlreadyActive {
                version: a["version"].as_str().unwrap_or_default().to_string(),
            },
            Some("staged") => Activation::Staged,
            other => {
                return Err(format!(
                    "the apply's result named activation state {other:?}, which this client \
                     does not know"
                ))
            }
        };
        let stored = match v["stored"].as_str() {
            Some("written") => Stored::Written,
            Some("alreadyPresent") => Stored::AlreadyPresent,
            other => {
                return Err(format!(
                    "the apply's result named stored state {other:?}, which this client does \
                     not know"
                ))
            }
        };
        Ok(Report {
            version: s("version")?,
            notes: v["notes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect(),
            schema,
            stored,
            activation,
            // Absent from an older host's answer, which walked no tenants
            // in this shape: read as none rather than refused, because the
            // rest of the report is still true.
            tenants: TenantsReport {
                advanced: usize::try_from(v["tenants"]["advanced"].as_u64().unwrap_or_default())
                    .unwrap_or_default(),
                current: usize::try_from(v["tenants"]["current"].as_u64().unwrap_or_default())
                    .unwrap_or_default(),
                held: v["tenants"]["held"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|h| {
                        (
                            h["target"].as_str().unwrap_or_default().to_string(),
                            h["reason"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect(),
            },
        })
    }
}

// ── the handler ─────────────────────────────────────────────────────────

/// Everything the apply op needs that the access engine does not have.
///
/// `Option` on [`crate::platform_ops::PlatformOps`], on the terms its
/// module doc sets for every other wired surface: a deployment either hands
/// this over **and** registers the spec, or hands over neither.
pub struct ApplyOps {
    /// The control database: where `$artifacts`, `$activations` and
    /// `$migrations` are. The registry is about the deployment rather than
    /// about any one space, so it does not move when a space does — and a
    /// schema is the deployment's for the same reason.
    pub pool: Pool,
    /// **Every database this deployment serves from** (18d T7 R-S3).
    ///
    /// The apply walks this rather than applying to [`Self::pool`] alone,
    /// because `$artifacts` and `$activations` are deployment-wide: a
    /// program activated while one target is behind is a program whose
    /// tables that tenant's database does not have, and every request in
    /// that tenant then fails per statement, late, and differently each
    /// time. The walk is `schema_apply::apply_everywhere`; this is the
    /// same resolver `Spaces` holds, handed in rather than built here for
    /// T2's reason.
    pub pools: Arc<dyn ordo_exec::route::PoolResolver>,
    /// The deployment's own space: `RuntimeConfig::space`, the one the
    /// binary was configured with. Check 3 of the gate compares the bound
    /// space against this; see the module doc for why it is a name and not
    /// `is_system_space`.
    pub home: String,
    /// The deployment's security record. Each change through the door
    /// (an apply, an activation) is written to it as a `deploy` event
    /// (`plan-console.md` G12). `None` records nothing there; the registry
    /// rows still name the caller.
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

impl ApplyOps {
    pub fn new(
        pool: Pool,
        pools: Arc<dyn ordo_exec::route::PoolResolver>,
        home: &str,
    ) -> Arc<ApplyOps> {
        Self::recording(pool, pools, home, None)
    }

    /// [`ApplyOps::new`], writing each change to `stream`.
    pub fn recording(
        pool: Pool,
        pools: Arc<dyn ordo_exec::route::PoolResolver>,
        home: &str,
        stream: Option<Arc<ordo_secevent::Recorder>>,
    ) -> Arc<ApplyOps> {
        Arc::new(ApplyOps {
            pool,
            pools,
            home: home.to_string(),
            stream,
        })
    }

    /// Check 3 of the gate (see the module doc), for every deploy op.
    ///
    /// **`not_found`, and not `access_denied`**, for two reasons that
    /// agree. The mechanical one: `access_denied` is the *door's* word and
    /// an executor that said it would reach the caller as `internal`. The
    /// honest one: these ops do not exist in that space. No membership
    /// change in a tenant space will ever make the call succeed, which is
    /// what separates "there is nothing here" from "not for you".
    pub fn home_only(&self, space: &str) -> Result<(), ExecError> {
        if space != self.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "the deployment's program is the deployment's rather than a space's: this op \
                 answers only when it is bound to the deployment's own space, and owning a \
                 space you were given is not owning the deployment that serves it",
            ));
        }
        Ok(())
    }

    /// Write one deploy act to the security record, when there is one.
    pub fn record(&self, by: &str, op: &str, act: &str, detail: String) {
        let Some(stream) = &self.stream else {
            return;
        };
        stream.observe(
            ordo_secevent::Observation {
                space: None,
                subject: ordo_secevent::Subject::member(by),
                op: op.to_string(),
                kind: ordo_secevent::EventKind::Deploy {
                    act: act.to_string(),
                    detail,
                },
                at: ordo_commandlog::Timestamp(now_secs()),
                correlation: ordo_secevent::correlation::mint(),
            },
            ordo_secevent::Relationship::None,
        );
    }

    /// `std.deployment.apply`.
    ///
    /// `caller` is the principal the door proved, and it is what reaches
    /// `$migrations` as the actor. See the module doc: there is no `--as`
    /// here and there will not be one.
    pub async fn apply(
        &self,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        // Check 3 of the gate. See the module doc for why the profile and
        // the step-up requirement cannot express this one: they are
        // deployment-wide policy installed in every space, so a tenant
        // space's owner passes both and is still a space member.
        self.home_only(space)?;

        let source = bound
            .0
            .get("program")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ExecError::internal("`$program` did not reach the platform op"))?;
        // Absent is **true**, which is the direct path's default: `ordo
        // apply` activates unless `--no-activate` is given, and a door that
        // defaulted the other way would make a deploy over the wire a
        // different act from a deploy on the box.
        let activate = bound
            .0
            .get("activate")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let code = parse_code(bound.0.get("code").and_then(|v| v.as_str()))
            .map_err(|refusal| ExecError::new(refusal.code(), refusal.to_string()))?;

        match run(
            &self.pool,
            self.pools.as_ref(),
            source,
            &code,
            caller,
            APPLY_OP_LABEL,
            activate,
        )
        .await
        {
            Ok(report) => {
                let activation = match &report.activation {
                    Activation::Switched { from, .. } => format!(
                        "activation=switched was={}",
                        from.as_deref().unwrap_or("none")
                    ),
                    Activation::AlreadyActive { .. } => "activation=already-active".to_string(),
                    Activation::Staged => "activation=staged".to_string(),
                };
                self.record(
                    caller,
                    APPLY,
                    "applied",
                    format!("version={} {activation} by={caller}", report.version),
                );
                Ok(ExecResult {
                    position: None,
                    rows: vec![report.to_json()],
                    next_cursor: None,
                })
            }
            Err(refusal) => Err(ExecError::new(refusal.code(), refusal.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_FN: &str = "domain fx @ 0.1.0\n\nentity a {\n  n: int\n}\n\n\
                           op listA unbounded =\n  a[n]\n\n\
                           function hello in fx {\n  call($t: text(10)) -> json by member\n  \
                           code \"fn/hello.js\"\n}\n";

    fn file(text: &str) -> CodeFile {
        CodeFile {
            hash: ordo_plan::artifact::code_hash(text.as_bytes()),
            text: text.to_string(),
        }
    }

    #[test]
    fn the_doors_code_binds_as_the_local_apply_does() {
        let code: CodeFiles = [("fn/hello.js".to_string(), file("export default {};"))].into();
        let parsed = parse_code(Some(&code_text(&code))).expect("round trips");
        assert_eq!(parsed, code);
        let mut program = Program::compile(WITH_FN).expect("compiles");
        let before = program.version.clone();
        let store = bind_code(&mut program, &parsed).expect("binds");
        assert_eq!(store.len(), 1);
        assert_ne!(program.version, before, "the code hash is in the version");
    }

    #[test]
    fn missing_or_altered_code_is_refused_by_name() {
        let mut program = Program::compile(WITH_FN).expect("compiles");
        let missing = bind_code(&mut program, &CodeFiles::new()).unwrap_err();
        assert_eq!(missing.class(), "function_code_refused");
        assert!(missing.to_string().contains("fn/hello.js"), "{missing}");

        let mut altered = file("export default {};");
        altered.text.push(' ');
        let code: CodeFiles = [("fn/hello.js".to_string(), altered)].into();
        let refused = bind_code(&mut program, &code).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::ApplyRefused);
        assert!(refused.to_string().contains("apply again"), "{refused}");

        let shape = parse_code(Some("[1]")).unwrap_err();
        assert_eq!(shape.class(), "function_code_refused");
    }

    #[test]
    fn the_lock_key_is_the_migrations_table_and_is_positive() {
        assert_eq!(APPLY_LOCK, fnv1a(b"$migrations"));
        // A `const` block, because the value is a constant and clippy is
        // right that a runtime assertion over one proves nothing at run
        // time. This way it does not compile if the derivation ever turns
        // the top bit on.
        const {
            assert!(APPLY_LOCK > 0, "a negative key is one nobody can grep for");
        }
        assert_ne!(APPLY_LOCK, fnv1a(b"$artifacts"));
    }

    #[test]
    fn a_report_round_trips_through_the_wire_shape() {
        for report in [
            Report {
                version: "v_1".into(),
                notes: vec!["a note".into()],
                schema: SchemaOutcome::Created {
                    tables: 3,
                    migration: 7,
                    event: "event recorded on the deployment stream".into(),
                },
                stored: Stored::Written,
                activation: Activation::Switched {
                    from: None,
                    to: "v_1".into(),
                },
                tenants: TenantsReport {
                    advanced: 2,
                    current: 1,
                    held: vec![(
                        "h:5432/t_x".into(),
                        "not_class_a: it adds a required field".into(),
                    )],
                },
            },
            Report {
                version: "v_2".into(),
                notes: vec![],
                schema: SchemaOutcome::Matches { pending: 2 },
                stored: Stored::AlreadyPresent,
                activation: Activation::AlreadyActive {
                    version: "v_2".into(),
                },
                tenants: TenantsReport::default(),
            },
            Report {
                version: "v_3".into(),
                notes: vec![],
                schema: SchemaOutcome::Behind {
                    changes: vec![("A2".into(), "new field `x`".into())],
                },
                stored: Stored::Written,
                activation: Activation::Staged,
                tenants: TenantsReport::default(),
            },
        ] {
            let back = Report::from_json(&report.to_json()).expect("round trip");
            assert_eq!(back, report);
        }
    }

    #[test]
    fn an_unknown_state_is_refused_rather_than_defaulted() {
        let v = json!({
            "version": "v", "notes": [],
            "schema": { "state": "from-a-newer-server" },
            "stored": "written",
            "activation": { "state": "staged" },
        });
        assert!(Report::from_json(&v).is_err());
    }

    #[test]
    fn a_lock_timeout_is_told_apart_from_a_real_failure() {
        use ordo_exec::PoolError;
        assert!(
            PoolError::LockTimeout("canceling statement due to lock timeout".into())
                .is_lock_timeout()
        );
        // The words alone are not the class.
        assert!(!PoolError::db("canceling statement due to lock timeout").is_lock_timeout());
        assert!(!PoolError::db("connection refused").is_lock_timeout());
    }

    #[test]
    fn every_refusal_names_a_class_and_a_code() {
        let all = [
            ApplyRefused::Ghost {
                serving: "v".into(),
            },
            ApplyRefused::Broken { ops: vec![] },
            ApplyRefused::DoesNotCompile("parse_error".into()),
            ApplyRefused::NotActivated {
                version: "v".into(),
                changes: vec![],
            },
            ApplyRefused::InProgress,
            ApplyRefused::Internal("x".into()),
        ];
        for r in &all {
            assert!(!r.class().is_empty());
            assert!(!r.to_string().is_empty());
        }
        // The two the direct path prints by name must survive the wire as
        // themselves rather than collapsing into `internal`.
        assert_eq!(ApplyRefused::InProgress.code(), ErrorCode::ApplyInProgress);
        assert_eq!(
            ApplyRefused::NotActivated {
                version: "v".into(),
                changes: vec![]
            }
            .code(),
            ErrorCode::NotActivated
        );
    }
}
