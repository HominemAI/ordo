//! P6 (slice) — the conformance harness.
//!
//! Implements the five stages of `composition-conformance.md` §2.4 for the
//! subset of the corpus the compiler can currently lower: **parse**,
//! **compile**, **execute** against a seeded Postgres, **compare** canonical
//! JSON, byte-equal or fail.
//!
//! # What a green run here does and does not mean
//!
//! It means: this source text compiled to this SQL, that SQL ran against a
//! database built from the fixture's own DDL, and the rows it returned are
//! byte-identical to the rows `composition-conformance.md` states — values
//! nobody in this repo authored.
//!
//! It does not mean the corpus passes. Most cases do not run at all: the
//! constructs they need are refused by name in `ordo-sema`. `cases_covered`
//! is the honest count and the tests print it.
//!
//! # Execution is `psql`, not an executor
//!
//! Statements reach Postgres through the `psql` binary, not through a driver
//! in this process. There is still no Postgres dependency in the workspace
//! and `ordo-exec` (F5) is still unbuilt — this harness proves the compiler
//! emits SQL a real server accepts and answers correctly, and proves nothing
//! whatsoever about a runtime executor.

use std::process::Command;

use ordo_plan::{plan, Plan};
use ordo_schema::{ddl_for, Schema};
use ordo_sema::{bind, Registry};
use ordo_syntax::decl::ScalarTy;
use ordo_syntax::parse_program;
use serde_json::{json, Map, Value};

pub mod access_profile;
pub mod golden;
pub mod membership;
pub mod routine;
pub mod write;
pub use golden::assert_golden;
pub use membership::{GrantPath, GroupCommand};
use write::sql_str;
pub use write::WriteLog;

pub const FIXTURE: &str = include_str!("../fixtures/market.ordo");
pub const SEED: &str = include_str!("../fixtures/seed.sql");

/// The `wide` fixture: a schema built to be hit rather than to be realistic.
/// It is the target of the frozen-wants corpus.
pub const WIDE: &str = include_str!("../fixtures/wide.ordo");
pub const WIDE_SEED: &str = include_str!("../fixtures/wide-seed.sql");

/// The corpus cases this slice lowers and executes end to end, with the
/// source text `composition-conformance.md` §4 states for each.
///
/// One table, two consumers: `tests/corpus.rs` runs them against seeded
/// Postgres and `tests/goldens.rs` pins their compiled SQL. A second copy
/// would let the goldened set and the executed set drift, and the drift
/// that matters is the silent one — a case goldened but no longer executed
/// still looks green.
pub fn covered_case_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("C001", "activeProducts[name, price]\nsort price"),
        ("C002", "cheapActive($cap: 5000)"),
        ("C003", "doneOrders($from: 2026-02-01)[placed, total]\nsort placed"),
        ("C004", "top3Orders(total < 15000)[placed, total]"),
        (
            "C005",
            "doneOrders($from: 2026-01-01, total > 5000)[placed, total, customer.name]\nsort -total",
        ),
        ("C006", "spendByCustomer($from: 2026-01-01)\nsort -spent"),
        ("C007", "activeProducts[label: name, price]\nsort price"),
        ("C008", "cheapActive($cap: 10000)[name, sale: price - 500]\nsort sale"),
        ("C009", "top3Orders[*]"),
        ("C010", "spendByCustomer($from: 2026-01-01)[*, -n]\nsort -spent"),
    ]
}

/// Connection details for the throwaway conformance cluster. Every field has
/// an environment override so the harness is not welded to one machine.
#[derive(Debug, Clone)]
pub struct Pg {
    pub host: String,
    pub port: String,
    pub user: String,
    pub db: String,
    pub psql: String,
}

impl Default for Pg {
    fn default() -> Self {
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Pg {
            host: env("ORDO_PG_HOST", "/tmp/ordo-pg"),
            port: env("ORDO_PG_PORT", "55432"),
            user: env("ORDO_PG_USER", "ordo"),
            db: env("ORDO_PG_DB", "ordo_conformance"),
            psql: env("ORDO_PSQL", "/opt/homebrew/opt/postgresql@17/bin/psql"),
        }
    }
}

/// Separators chosen because they cannot occur in the fixture's values and
/// need no escaping — parsing psql's aligned output would be a second parser
/// with its own bugs.
const FS: &str = "\u{1f}";
const RS: &str = "\u{1e}";
// Not NUL: a process argument cannot contain one.
const NULL_SENTINEL: &str = "\u{1d}NULL";

/// The namespace stamped onto every throwaway test database, so two runs
/// against the same cluster — two `cargo test` invocations, or two agents —
/// never touch the same database. See `docs/corpus-rulings.md`.
///
/// `ORDO_TEST_DB_SUFFIX` pins it, for a scripted before/after comparison
/// that wants the SAME name on both sides. Left unset (the ordinary case),
/// it is minted once per test binary — this process's id plus a start-time
/// nanosecond stamp — which is unique among anything else touching the
/// cluster right now: two `cargo test` processes never share a pid, and two
/// runs of the same binary a moment apart never share a nanosecond.
fn db_suffix() -> &'static str {
    static SUFFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SUFFIX.get_or_init(|| {
        std::env::var("ORDO_TEST_DB_SUFFIX").unwrap_or_else(|_| {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            format!("{}_{nanos:x}", std::process::id())
        })
    })
}

/// This run's actual name for a test's own database, stem plus [`db_suffix`].
///
/// Pure and deterministic within one process: two call sites that pass the
/// same `stem` (a `Pg` built by [`Pg::named`], and a `PoolConfig` built
/// alongside it for the same throwaway database, say) always agree on the
/// name without either one having to hand the other a value — which is the
/// point, since `ordo-exec`'s `PoolConfig` cannot depend on this crate.
///
/// The stem stays the readable part of any failure — `relation "tasks"
/// does not exist in ordo_rt_sup_e2e_84213_a91fb02c` still names the suite.
pub fn test_db_name(stem: &str) -> String {
    format!("{stem}_{}", db_suffix())
}

/// Drop `stem`'s own siblings left by a run whose process has since exited
/// — an ordinary finish (nothing here drops its own database on the way
/// out; see [`Pg::named`]'s doc) or a panic (which unwinds the panicking
/// test's thread, not the process — the database outlives it exactly as an
/// ordinary finish would, and is reaped the same way). Scoped to `stem` so
/// it costs one query per creation, not a scan of the whole cluster.
/// `scripts/reap-test-dbs.sh` sweeps everything, for a stem this process
/// never asks for again.
fn reap_orphans(stem: &str) {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    // `!` escapes `_` and `%`, which a stem like `ordo_rt_sup_e2e` is full
    // of and which LIKE would otherwise read as wildcards.
    let escaped = stem
        .replace('!', "!!")
        .replace('_', "!_")
        .replace('%', "!%");
    let Ok(rows) = admin.query(&format!(
        "SELECT datname FROM pg_database WHERE datname LIKE '{escaped}!_%' ESCAPE '!'"
    )) else {
        return;
    };
    let prefix = format!("{stem}_");
    for row in rows {
        let Some(Some(name)) = row.into_iter().next() else {
            continue;
        };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(pid_str) = rest.split('_').next() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        // Never our own run, even mid-flight, and never a pid still alive —
        // `ps -p` is the portable "is this process running" check with no
        // `unsafe` (the workspace forbids it) libc `kill(pid, 0)` needs.
        if pid == std::process::id() || pid_alive(pid) {
            continue;
        }
        let _ = admin.exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
    }
}

fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        // Can't tell — e.g. no `ps` in a minimal container — so leave it
        // alone rather than risk dropping a live run's database.
        .map(|o| o.status.success())
        .unwrap_or(true)
}

impl Pg {
    /// A database of this suite's own, created if it is not already there,
    /// under the name given. [`Pg::scoped`] is the same thing namespaced
    /// per run, which is where this is going; see its doc for why the two
    /// exist and what converting a suite means.
    ///
    /// Test binaries run CONCURRENTLY under `cargo test`, and [`Fixture::reset`]
    /// opens with `DROP SCHEMA public CASCADE` — so two suites pointed at one
    /// database tear down each other's tables mid-run. The failure surfaces in
    /// whichever suite lost the race, as `relation "tasks" does not exist`,
    /// which reads exactly like a bug in that suite and is why this was
    /// diagnosed as a flake four separate times before being fixed. Naming
    /// collided across RUNS the same way, and did real damage on 2026-09-20:
    /// one agent's in-flight migration sat in a database another agent's
    /// suite, sharing the fixed name, then failed against — which is what
    /// [`Pg::scoped`] closes.
    ///
    /// `..Pg::default()` with an explicit `db` says the same thing; this exists
    /// so a new suite gets the isolation by asking for a database at all,
    /// rather than by remembering why it must.
    pub fn named(db: &str) -> Pg {
        Pg::create(db.to_string())
    }

    /// [`Pg::named`], **namespaced per run**: the database is
    /// `<db>_<pid>_<nanos>` ([`test_db_name`]), so two `cargo test`
    /// invocations — or two agents — never touch the same one.
    ///
    /// # Why this is a second constructor rather than `named`'s behaviour
    ///
    /// Namespacing is the right default and it is not yet the default,
    /// because **95 call sites across 63 files spell the database name a
    /// second time**: a `PoolConfig`, a `psql` invocation, a helper taking
    /// `db: &str` and building a connection string from it. Every one of
    /// those opens the *bare stem* — a different database from the one the
    /// fixture just seeded — and fails as `relation "…" does not exist`.
    /// Switching `named` wholesale breaks 69 tests across 12 suites at once,
    /// which is a migration and not a change.
    ///
    /// So suites move over one at a time, and a converted suite is one where
    /// the name is spelled **once**: read it off this `Pg` (`pg.db`) or off
    /// the pool (`pool.config().db`), never re-typed. `scripts/reap-test-dbs.sh`
    /// and [`reap_orphans`] work on whatever has moved, because both match
    /// the `<stem>_<pid>_<hex>` shape and ignore everything else.
    ///
    /// Nothing here drops the database on the way out, on success OR on a
    /// panic — `Pg` is cloned too freely to carry a drop-guard safely, and a
    /// panic only unwinds the panicking test's thread, not the process. It
    /// becomes an ordinary orphan once this process exits, and
    /// [`reap_orphans`] sweeps it — under this same `db` stem — the next
    /// time anything asks for a database under that stem and finds this
    /// run's pid no longer alive.
    pub fn scoped(db: &str) -> Pg {
        reap_orphans(db);
        Pg::create(test_db_name(db))
    }

    fn create(name: String) -> Pg {
        let admin = Pg {
            db: "postgres".into(),
            ..Pg::default()
        };
        // Already-exists is the ordinary case, not a failure.
        let _ = admin.exec(&format!("CREATE DATABASE {name}"));
        Pg {
            db: name,
            ..Pg::default()
        }
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new(&self.psql)
            .args([
                "-h", &self.host, "-p", &self.port, "-U", &self.user, "-d", &self.db,
            ])
            .args(["-v", "ON_ERROR_STOP=1", "-q"])
            // VERBOSITY=verbose puts the SQLSTATE on the error line, so the
            // harness can classify by code the way the pool does — never by
            // matching English.
            .args(["-v", "VERBOSITY=verbose"])
            .args(args)
            // Pinned so date rendering and time zone are properties of the
            // harness, not of whoever's shell started it.
            .env(
                "PGOPTIONS",
                "-c datestyle=ISO,YMD -c timezone=UTC -c standard_conforming_strings=on",
            )
            .env("PGTZ", "UTC")
            .output()
            .map_err(|e| {
                format!(
                    "could not run `{}`: {e}\n\
                     The execution tier needs a Postgres cluster. Start one with \
                     `scripts/conformance-pg.sh start` and re-run.",
                    self.psql
                )
            })?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            // The harness's one SQLSTATE seam, mirroring the pool's: 23514
            // check_violation is the data refusing a declared row invariant,
            // and it must carry its class — a raw psql dump reads as the
            // harness breaking, which is exactly the `internal` confusion
            // the class exists to prevent. The constraint name on the error
            // line is the diagnostic; keep the whole message.
            if stderr.contains("ERROR:  23514") {
                return Err(format!("check_violation: {}", stderr.trim()));
            }
            return Err(format!("psql failed ({}):\n{stderr}", out.status));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    pub fn exec(&self, sql: &str) -> Result<(), String> {
        self.run(&["-c", sql]).map(|_| ())
    }

    /// Run a planned statement and return its rows as raw column strings, in
    /// the server's returned order — no wrapping aggregate, because wrapping
    /// an ordered query in one is exactly how a harness loses the ordering it
    /// is trying to test.
    pub fn query(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        let raw = self.run(&[
            "-t",
            "-A",
            "-F",
            FS,
            "-R",
            RS,
            "-P",
            &format!("null={NULL_SENTINEL}"),
            "-c",
            sql,
        ])?;
        let raw = raw.trim_end_matches('\n');
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        Ok(raw
            .split(RS)
            .filter(|r| !r.is_empty())
            .map(|r| {
                r.split(FS)
                    .map(|c| {
                        if c == NULL_SENTINEL {
                            None
                        } else {
                            Some(c.to_string())
                        }
                    })
                    .collect()
            })
            .collect())
    }
}

/// The compiled fixture: schema graph, DDL, and op registry.
pub struct Fixture {
    pub schema: Schema,
    pub registry: Registry,
    pub ddl: String,
    /// Key custody, INJECTED rather than constructed at call sites. This
    /// is the seam F0's done-when asks for: swapping a real provider in is
    /// a field assignment, and nothing downstream can reach a key without
    /// going through it.
    pub keys: std::sync::Arc<dyn ordo_crypto::KeyProvider>,
    /// The space every statement this fixture plans is confined to, and the
    /// space its custody derives keys under. There is no default anywhere
    /// below this: `plan` takes a space, the seed stamps one, and the
    /// write path scopes by one.
    space: ordo_crypto::SpaceId,
    seed: &'static str,
}

/// The space the fixture's custody lives in by default.
pub fn fixture_space() -> ordo_crypto::SpaceId {
    ordo_crypto::SpaceId(FIXTURE_SPACE.into())
}

/// The conformance corpus's space. The fixture's DATA is single-space —
/// the corpus's wants are stated for one tenant — but the schema is not,
/// and `tests/spaces.rs` seeds a twin to prove it.
pub const FIXTURE_SPACE: &str = "conformance";

impl Fixture {
    /// This fixture with a different schema and everything else shared.
    ///
    /// Exists for the door's rule-6 view (`write.rs`), which runs one call
    /// against a schema whose acl link entities carry no guards. The registry
    /// and the DDL are unchanged on purpose: the ops are the same ops and the
    /// tables are the same tables — only what a guard consults differs, and
    /// only for that call.
    pub(crate) fn with_schema(&self, schema: ordo_schema::Schema) -> Fixture {
        Fixture {
            schema,
            registry: self.registry.clone(),
            ddl: self.ddl.clone(),
            keys: self.keys.clone(),
            space: self.space.clone(),
            seed: self.seed,
        }
    }

    pub fn load() -> Result<Fixture, String> {
        Fixture::compile_source(FIXTURE, SEED)
    }

    /// The `wide` fixture and its seed.
    pub fn load_wide() -> Result<Fixture, String> {
        Fixture::compile_source(WIDE, WIDE_SEED)
    }

    /// A fixture over source text a suite composes, seeded from an existing
    /// seed file.
    ///
    /// `Registry::extend_from` already lets a suite register extra OPS
    /// against the shipped fixture, which is how the guards corpus adds
    /// §3.4's library without moving the numbers other suites assert about
    /// `market.ordo`. An extra ENTITY has no such door: the schema, and
    /// therefore the DDL, come from one source text. This is that door,
    /// and it exists for the same reason — a suite that needs a shape the
    /// shipped fixture does not have should not have to change the shipped
    /// fixture and every golden derived from it.
    ///
    /// The seed stays `&'static`: it is stored and re-rendered per space.
    /// An entity the seed knows nothing about simply starts empty, which
    /// is what a create test wants.
    pub fn from_source(src: &str, seed: &'static str) -> Result<Fixture, String> {
        Fixture::compile_source(src, seed)
    }

    fn compile_source(src: &str, seed: &'static str) -> Result<Fixture, String> {
        let program =
            parse_program(src).map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
        let keys = ordo_schema::KeyTable::from_env().map_err(|e| format!("keys: {e}"))?;
        let schema =
            ordo_schema::build_with_keys(&program, &keys).map_err(|e| format!("schema: {e}"))?;
        let ddl = ddl_for(&schema).map_err(|e| format!("ddl: {e}"))?;
        let mut registry = Registry::from_program(&program);
        // `acl` derives its admin ops the way it derives its link entity —
        // from the declaration, not from the fixture. They pass the same
        // registration gate as everything else.
        registry.extend_from(&ordo_schema::acl_admin_ops(&schema));
        // The std library, derived from the same schema by the same
        // mechanism, and gated by the same call below.
        registry.extend_from(&ordo_schema::std_ops(&schema));
        ordo_plan::cost::check_registration(&schema, &registry)
            .map_err(|e| format!("register: {e}"))?;
        Ok(Fixture {
            schema,
            registry,
            ddl,
            keys: std::sync::Arc::new(ordo_crypto::TestProvider::with_space(FIXTURE_SPACE)),
            space: fixture_space(),
            seed,
        })
    }

    /// The same compiled fixture, reading and writing a DIFFERENT space.
    ///
    /// One schema, one registry, one key provider (which derives per space
    /// already), and a different tenancy value. This is what makes the
    /// isolation suite an honest test rather than a second fixture that
    /// might differ in some other way: the ONLY difference between the two
    /// fixtures is the space, so anything one sees and the other does not
    /// is the space doing its job.
    pub fn in_space(&self, space: &str) -> Fixture {
        Fixture {
            schema: self.schema.clone(),
            registry: self.registry.clone(),
            ddl: self.ddl.clone(),
            keys: self.keys.clone(),
            space: ordo_crypto::SpaceId(space.to_string()),
            seed: self.seed,
        }
    }

    /// The space this fixture is bound to.
    pub fn space(&self) -> &ordo_crypto::SpaceId {
        &self.space
    }

    /// The planner's view of it. Every statement the harness plans folds
    /// this in as a literal — the harness executes SQL through `psql`, so
    /// there is nothing to bind a slot with, and the space is known at
    /// plan time exactly as the door knows it at request time.
    pub fn plan_space(&self) -> ordo_plan::Space {
        ordo_plan::Space::Bound(self.space.0.clone())
    }

    /// Stage 1 — parse a case body and bind it. Exposed so tests can assert
    /// on properties of the IR (such as the ordering guarantee) rather than
    /// by regexing SQL, per §2.4's rule about compile assertions.
    pub fn bind_case(&self, source: &str) -> Result<ordo_sema::Query, String> {
        let pipeline = ordo_syntax::parse_pipeline(source)
            .map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
        bind(
            &self.schema,
            &self.registry,
            &pipeline,
            ordo_sema::Clock::CORPUS,
        )
        .map_err(|e| format!("bind: {e}"))
    }

    /// Stage 1+2 — parse a case body and compile it against the fixture.
    pub fn compile(&self, source: &str) -> Result<Plan, String> {
        let mut q = self.bind_case(source)?;
        self.resolve_blind(&mut q, &[])?;
        plan(&q, &self.plan_space()).map_err(|e| format!("plan: {e}"))
    }

    /// Resolve blind-index digest sites — the step the executor owns,
    /// after the access check and behind a key release.
    pub fn resolve_blind(
        &self,
        q: &mut ordo_sema::Query,
        params: &[(String, String)],
    ) -> Result<(), String> {
        ordo_plan::blind::resolve(q, self.keys.as_ref(), &self.space, params)
            .map_err(|e| e.to_string())
    }

    /// Drop and rebuild the database from the fixture's own generated DDL,
    /// then seed it. Recreated per run, per §2.4 stage 3.
    ///
    /// Once per test binary, not once per case: cases are reads, so they
    /// share the seeded database. Rebuilding per case raced six ways on one
    /// cluster and failed inside Postgres's own catalog.
    pub fn reset_once(&self, pg: &Pg) -> Result<(), String> {
        static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| self.reset(pg)).clone()
    }

    /// Once-guard for the wide fixture (its own database).
    pub fn reset_once_wide(&self, pg: &Pg) -> Result<(), String> {
        static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| self.reset(pg)).clone()
    }

    /// Unconditional rebuild — used by fixtures other than the default one,
    /// which have their own database and their own once-guard.
    pub fn reset(&self, pg: &Pg) -> Result<(), String> {
        pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")?;
        pg.exec(&self.ddl)?;
        // The sealed payload store is a PLATFORM table, not a generated
        // one, so it is not in the schema's DDL — but the write protocol
        // writes it on every sealed field, and the harness's `psql` path
        // never boots an `ordo_exec::Pool` to run
        // `ensure_platform_tables`. Same shape either way: the constant
        // is the one `ordo-exec` runs (`ordo_crypto::sealed`).
        pg.exec(ordo_crypto::SEALED_PAYLOAD_DDL)?;
        self.seed_into(pg)
    }

    /// Seed this fixture's space into an already-built schema.
    ///
    /// Split out from [`Self::reset`] so a suite can build the tables once
    /// and seed two spaces into them — the same rows, the same ids, under
    /// two tenancies.
    pub fn seed_into(&self, pg: &Pg) -> Result<(), String> {
        pg.exec(&self.rendered_seed()?)
    }

    /// The seed file with its two per-space placeholders resolved.
    ///
    /// - `@SPACE@` — the tenancy value stamped on every row.
    /// - `@BIDX(entity,field,value)@` — the blind-index digest of `value`
    ///   (empty = the reserved null sentinel) under THIS SPACE's index key.
    ///
    /// The digests are placeholders rather than hex because they are
    /// **per-space by construction**: `BIK = KDF(SMK(space), …)`, so the
    /// same email digests differently in two spaces and a seed file with
    /// literal bytes could only ever be right for one of them. Computing
    /// them here also retires the standing instruction to "regenerate
    /// rather than hand-edit" — there is nothing left in the file to edit
    /// wrongly.
    pub fn rendered_seed(&self) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = self.seed;
        while let Some(i) = rest.find("@BIDX(") {
            out.push_str(&rest[..i]);
            let tail = &rest[i + "@BIDX(".len()..];
            let close = tail
                .find(")@")
                .ok_or("seed: an unterminated @BIDX( placeholder")?;
            let args: Vec<&str> = tail[..close].split(',').collect();
            let [entity, field, value] = args.as_slice() else {
                return Err(format!(
                    "seed: @BIDX takes entity,field,value — got `{}`",
                    &tail[..close]
                ));
            };
            let v = if value.is_empty() {
                ordo_crypto::BlindValue::Null
            } else {
                ordo_crypto::BlindValue::Text(value)
            };
            let digest = self
                .keys
                .blind_index(&self.space, entity, field, v)
                .map_err(|e| format!("seed: blind index: {e}"))?;
            out.push_str(&format!("'\\x{}'::bytea", ordo_crypto::hex(&digest)));
            rest = &tail[close + 2..];
        }
        out.push_str(rest);
        Ok(out.replace("@SPACE@", &sql_str(&self.space.0)))
    }

    /// Run raw SQL and canonicalise by inspection rather than by declared
    /// type — the corpus's stage-two queries are hand-written and carry no
    /// Ordo output shape to decode against.
    pub fn run_sql(&self, pg: &Pg, sql: &str) -> Result<Value, String> {
        let rows = pg.query(sql)?;
        Ok(Value::Array(
            rows.into_iter()
                .map(|r| Value::Array(r.into_iter().map(infer).collect()))
                .collect(),
        ))
    }

    /// The guards suite's caller dimension (GC §1): run a case `as:` a
    /// principal — `Some(member_id)` binds `$caller`; `None` is anonymous
    /// and every guard evaluates false (G15).
    pub fn run_case_as(
        &self,
        pg: &Pg,
        source: &str,
        params: &[(&str, &str)],
        caller: Option<&str>,
    ) -> Result<Value, String> {
        let pipeline = ordo_syntax::parse_pipeline(source)
            .map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
        let mut bound = Vec::new();
        for (n, lit) in params {
            let e = ordo_syntax::parse_expr(lit)
                .map_err(|e| format!("param {n}: {} at {}", e.message, e.span))?;
            bound.push((n.to_string(), e));
        }
        let query = ordo_sema::bind_as(
            &self.schema,
            &self.registry,
            &pipeline,
            &bound,
            caller,
            ordo_sema::Clock::CORPUS,
        )
        .map_err(|e| format!("bind: {e}"))?;
        let mut query = query;
        self.resolve_blind(&mut query, &blind_params(&bound))?;
        let p = plan(&query, &self.plan_space()).map_err(|e| format!("plan: {e}"))?;
        if !p.params.is_empty() {
            return Err(format!("unbound parameters remain: {:?}", p.params));
        }
        let mut rows = pg.query(&p.sql)?;
        self.equality_parity(pg, &query, &p, &rows)?;
        self.open_host_values(&mut rows, &p)?;
        Ok(canonical(&rows, &p.outputs))
    }

    /// A `whole entity($id)` read: one nested JSON document, or Null when
    /// the anchor does not exist.
    pub fn run_whole(&self, pg: &Pg, source: &str) -> Result<Value, String> {
        self.run_whole_as(pg, source, None)
    }

    /// `whole`, read as a caller. The anchor's read guard is inlined, so a
    /// whole-document read discloses exactly what an ordinary read of that
    /// row would — no more, and never the whole graph to anyone.
    pub fn run_whole_as(
        &self,
        pg: &Pg,
        source: &str,
        caller: Option<&str>,
    ) -> Result<Value, String> {
        let (entity, anchor) = ordo_syntax::parse_whole(source)
            .map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
        let anchor_sql = match &anchor {
            ordo_syntax::Expr::StrLit { value, .. } => {
                format!("'{}'", value.replace('\'', "''"))
            }
            _ => return Err("the playground anchors `whole` with a string id literal".into()),
        };
        // Every row the statement reads is guarded for this caller: the
        // anchor, each signpost's target and each owned child. The planner
        // asks the binder for each guard, so what crosses the boundary is
        // still IR and never SQL text.
        let sql = ordo_plan::whole::plan_whole_as(
            &self.schema,
            &entity.name,
            &anchor_sql,
            caller,
            &self.plan_space(),
        )
        .map_err(|e| format!("plan: {e}"))?;
        let rows = pg.query(&sql)?;
        match rows.first().and_then(|r| r.first()).and_then(|c| c.clone()) {
            None => Ok(Value::Null),
            Some(json) => serde_json::from_str(&json).map_err(|e| format!("bad json: {e}")),
        }
    }

    /// A case that DECLARES local ops: `op x = …` blocks, optionally
    /// followed (after a blank line) by the pipeline to run. An ops-only
    /// case runs its last op, with the harness params forwarded.
    pub fn run_case_with_ops(
        &self,
        pg: &Pg,
        source: &str,
        params: &[(&str, &str)],
    ) -> Result<Value, String> {
        let (ops_part, pipe_part) = match source.rsplit_once("\n\n") {
            Some((a, b)) if !b.trim_start().starts_with("op ") => (a.to_string(), b.to_string()),
            _ => (source.to_string(), String::new()),
        };
        let program = ordo_syntax::parse_program(&ops_part)
            .map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
        let mut registry = self.registry.clone();
        registry.extend_from(&program);
        ordo_plan::cost::check_registration(&self.schema, &registry)
            .map_err(|e| format!("register: {e}"))?;

        let pipe = if pipe_part.trim().is_empty() {
            let last = program.ops.last().ok_or("no op and no pipeline")?;
            let args: Vec<String> = last
                .params
                .iter()
                .map(|p| format!("${0}: ${0}", p.name.name))
                .collect();
            if args.is_empty() {
                last.name.name.clone()
            } else {
                format!("{}({})", last.name.name, args.join(", "))
            }
        } else {
            pipe_part
        };

        let pipeline = ordo_syntax::parse_pipeline(&pipe)
            .map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
        let mut bound = Vec::new();
        for (n, lit) in params {
            let e = ordo_syntax::parse_expr(lit)
                .map_err(|e| format!("param {n}: {} at {}", e.message, e.span))?;
            bound.push((n.to_string(), e));
        }
        let query = ordo_sema::bind_with_params(
            &self.schema,
            &registry,
            &pipeline,
            &bound,
            ordo_sema::Clock::CORPUS,
        )
        .map_err(|e| format!("bind: {e}"))?;
        let mut query = query;
        self.resolve_blind(&mut query, &blind_params(&bound))?;
        let p = plan(&query, &self.plan_space()).map_err(|e| format!("plan: {e}"))?;
        if !p.params.is_empty() {
            return Err(format!("unbound parameters remain: {:?}", p.params));
        }
        let mut rows = pg.query(&p.sql)?;
        self.equality_parity(pg, &query, &p, &rows)?;
        self.open_host_values(&mut rows, &p)?;
        Ok(canonical(&rows, &p.outputs))
    }

    /// `compile` with harness-supplied top-level parameters, given as Ordo
    /// literal source text (`("p", "\"prod_3\"")`, `("d", "2026-04-01")`).
    pub fn compile_params(&self, source: &str, params: &[(&str, &str)]) -> Result<Plan, String> {
        let query = self.bind_params(source, params)?;
        plan(&query, &self.plan_space()).map_err(|e| format!("plan: {e}"))
    }

    /// `compile_params` up to the bound query, blind sites resolved.
    fn bind_params(
        &self,
        source: &str,
        params: &[(&str, &str)],
    ) -> Result<ordo_sema::Query, String> {
        let pipeline = ordo_syntax::parse_pipeline(source)
            .map_err(|e| format!("{}: {} at {}", e.code, e.message, e.span))?;
        let mut bound = Vec::new();
        for (n, lit) in params {
            let e = ordo_syntax::parse_expr(lit)
                .map_err(|e| format!("param {n}: {} at {}", e.message, e.span))?;
            bound.push((n.to_string(), e));
        }
        let query = ordo_sema::bind_with_params(
            &self.schema,
            &self.registry,
            &pipeline,
            &bound,
            ordo_sema::Clock::CORPUS,
        )
        .map_err(|e| format!("bind: {e}"))?;
        let mut query = query;
        self.resolve_blind(&mut query, &blind_params(&bound))?;
        Ok(query)
    }

    /// `run_case` with harness-supplied parameters.
    pub fn run_case_params(
        &self,
        pg: &Pg,
        source: &str,
        params: &[(&str, &str)],
    ) -> Result<Value, String> {
        let query = self.bind_params(source, params)?;
        let p = plan(&query, &self.plan_space()).map_err(|e| format!("plan: {e}"))?;
        if !p.params.is_empty() {
            return Err(format!("unbound parameters remain: {:?}", p.params));
        }
        let mut rows = pg.query(&p.sql)?;
        self.equality_parity(pg, &query, &p, &rows)?;
        self.open_host_values(&mut rows, &p)?;
        Ok(canonical(&rows, &p.outputs))
    }

    /// **Equality parity** (board `guard-equality-uses-no-index`): run the
    /// query compiled the other way and require the same rows, in the same
    /// order. `ran` is what `p`, the planner's default form, returned.
    ///
    /// Every case this harness executes passes through here, so every query
    /// the conformance suites cover is also a test that compiling `==` by
    /// the never-null rule (`ordo_plan::never_null`) returns exactly what the
    /// null-safe `IS NOT DISTINCT FROM` form returns, over that suite's own
    /// data. A query whose two forms are the same SQL costs nothing more.
    pub fn equality_parity(
        &self,
        pg: &Pg,
        query: &ordo_sema::Query,
        p: &Plan,
        ran: &[Vec<Option<String>>],
    ) -> Result<bool, String> {
        let space = self.plan_space();
        let legacy = ordo_plan::plan_legacy_equality(query, &space)
            .map_err(|e| format!("plan, null-safe equality: {e}"))?;
        let proven = ordo_plan::plan_proven_equality(query, &space)
            .map_err(|e| format!("plan, proven equality: {e}"))?;
        if legacy.sql == proven.sql {
            return Ok(false);
        }
        let other = if p.sql == proven.sql {
            &legacy
        } else {
            &proven
        };
        let rows = pg
            .query(&other.sql)
            .map_err(|e| format!("equality parity: the other form failed: {e}\n{}", other.sql))?;
        // Without an ORDER BY no order is promised, so compare as sets.
        let (mut rows, mut ran) = (rows, ran.to_vec());
        if !other.sql.contains("\nORDER BY ") {
            rows.sort();
            ran.sort();
        }
        if rows != ran {
            return Err(format!(
                "equality parity: the two forms of equality returned different rows\n\
                 null-safe:\n{}\nproven:\n{}\nrows {:?}\nagainst {:?}",
                legacy.sql, proven.sql, ran, rows
            ));
        }
        Ok(true)
    }

    /// Stages 3+4 — execute and canonicalise.
    pub fn run_case(&self, pg: &Pg, source: &str) -> Result<Value, String> {
        let mut query = self.bind_case(source)?;
        self.resolve_blind(&mut query, &[])?;
        let p = plan(&query, &self.plan_space()).map_err(|e| format!("plan: {e}"))?;
        if !p.params.is_empty() {
            return Err(format!(
                "case left parameters unbound: {:?} — corpus cases bind every parameter at the call site",
                p.params
            ));
        }
        let mut rows = pg.query(&p.sql)?;
        self.equality_parity(pg, &query, &p, &rows)?;
        self.open_host_values(&mut rows, &p)?;
        Ok(canonical(&rows, &p.outputs))
    }
}

/// Canonical JSON per §2.4 stage 4: an array of row objects with sorted keys,
/// money and int as JSON integers, dates as ISO strings, refs as id strings,
/// null as `null`. `serde_json::Map` is a `BTreeMap` here, so key order is
/// sorted by construction rather than by discipline.
/// Open the plan's sealed outputs in place.
///
/// The harness must do this for the same reason the executor does: a
/// host-class plan hands back envelopes, and rendering an envelope as if
/// it were the value is exactly the failure the old blanket refusal
/// existed to prevent. Serving `\x01dd91…` in a `memo` column would look
/// like data and be ciphertext.
impl Fixture {
    pub fn open_host_values(
        &self,
        rows: &mut [Vec<Option<String>>],
        plan: &Plan,
    ) -> Result<(), String> {
        if plan.host_open.is_empty() {
            return Ok(());
        }
        for (name, entity, field) in &plan.host_open {
            let Some(i) = plan.outputs.iter().position(|(o, _)| o == name) else {
                continue;
            };
            for row in rows.iter_mut() {
                let Some(cell) = row.get_mut(i) else { continue };
                let Some(text) = cell.as_deref() else {
                    continue;
                };
                let hex = text.strip_prefix("\\x").unwrap_or(text);
                let bytes: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .filter_map(|k| u8::from_str_radix(hex.get(k..k + 2)?, 16).ok())
                    .collect();
                // The planner packs the id of the row the envelope was read
                // from beside it, and the envelope opens only at that row.
                let Some((id, envelope)) = ordo_plan::split_anchored(&bytes) else {
                    return Err(format!(
                        "sealed_open_failed: `{entity}.{field}` arrived without the row it was \
                         read from"
                    ));
                };
                if envelope.is_empty() {
                    *cell = Some(String::new());
                    continue;
                }
                // Opening is a key release too, and a READ one — the narrow
                // lease that can open and cannot seal.
                let lease = self
                    .keys
                    .release(&self.space, ordo_crypto::Purpose::Read, "read")
                    .map_err(|e| format!("key release: {e}"))?;
                let plain = self
                    .keys
                    .open_at(
                        &lease,
                        entity,
                        field,
                        ordo_crypto::Anchor::Row(id),
                        envelope,
                    )
                    .map_err(|_| format!("sealed_open_failed: `{entity}.{field}` did not open"))?;
                *cell = Some(String::from_utf8_lossy(&plain).into_owned());
            }
        }
        Ok(())
    }
}

/// The literal text of each bound parameter, for resolving a blind
/// comparison whose value arrived as a parameter.
fn blind_params(bound: &[(String, ordo_syntax::Expr)]) -> Vec<(String, String)> {
    bound
        .iter()
        .filter_map(|(n, e)| match e {
            ordo_syntax::Expr::StrLit { value, .. } => Some((n.clone(), value.clone())),
            _ => None,
        })
        .collect()
}

pub fn canonical(rows: &[Vec<Option<String>>], outputs: &[(String, ScalarTy)]) -> Value {
    let mut out = Vec::new();
    for r in rows {
        let mut obj = Map::new();
        for (i, (name, ty)) in outputs.iter().enumerate() {
            let raw = r.get(i).and_then(|c| c.clone());
            obj.insert(name.clone(), coerce(raw, *ty));
        }
        out.push(Value::Object(obj));
    }
    Value::Array(out)
}

/// Coerce a raw psql cell without a declared type.
///
/// Integers become JSON numbers and `t`/`f` become booleans, because that is
/// what the wants are written in. A text column holding "123" would be
/// misread — none exists in the `wide` seed, and the alternative is decoding
/// against a shape hand-written SQL does not have.
fn infer(raw: Option<String>) -> Value {
    match raw {
        None => Value::Null,
        Some(s) => {
            if let Ok(n) = s.parse::<i64>() {
                return json!(n);
            }
            match s.as_str() {
                "t" => json!(true),
                "f" => json!(false),
                _ => json!(s),
            }
        }
    }
}

fn coerce(raw: Option<String>, ty: ScalarTy) -> Value {
    let Some(s) = raw else { return Value::Null };
    match ty {
        ScalarTy::Int | ScalarTy::Money(_) | ScalarTy::Duration(_) => match s.parse::<i64>() {
            Ok(n) => json!(n),
            // A non-integral money value is a bug worth seeing in the diff
            // rather than a silent string.
            Err(_) => json!(s),
        },
        ScalarTy::Bool => json!(s == "t" || s == "true"),
        // A document is JSON in the result, as on the wire (28b).
        ScalarTy::Json => serde_json::from_str(&s).unwrap_or(json!(s)),
        _ => json!(s),
    }
}
