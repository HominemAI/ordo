//! F6's hot swap — `implementation.md`'s *"compiled ops are data … each
//! node swaps atomically … rollback = re-point the registry at the prior
//! version hash"* — with the load gate kept in front of it.
//!
//! [`ordo_exec::registry`] is the storage half. This is the loading half:
//! turning a stored artifact into something that serves, refusing the ones
//! that must not, and replacing the live one without a restart.
//!
//! # The defect this module exists to close
//!
//! `ordo_api::RegistryHandle::swap` takes any `Arc<dyn OpRegistry>`. Nothing
//! asked where it came from. So the load gate — which is otherwise
//! enforced *by type*, since [`crate::serve::PlatformState::new`] cannot be
//! called without a [`ShapeReport`] and only [`crate::shape::verify_registry`]
//! produces one — could be walked around completely by booting a verified
//! registry and then swapping in an unverified one. The gate would still be
//! in the boot path, and the deployment would be serving a registry no gate
//! had ever seen.
//!
//! Worse, `PlatformState.shape` was a **boot-time snapshot**. After any swap
//! it described a registry that was no longer serving, so `/health/deep`
//! published shape coverage for the wrong program and `/health/ready`
//! consulted a report that had stopped being about anything.
//!
//! And the program lived in **three** places that no one moved together:
//! the `RegistryHandle`'s specs, `FixtureExecutor`'s `Fixture`, and
//! `ordo_exec::Executor`'s own `schema` and `registry` fields. A swap of the
//! first alone would have left the door dispatching new specs into an
//! executor still planning the old program — the worst kind of half-swap,
//! because both halves are individually valid.
//!
//! # The shape of the fix
//!
//! [`Loaded`] is *everything a request must read from one version*: the
//! fixture, the executor built over it, the specs the door dispatches on,
//! the manifests the catalog renders, and the shape report that vouched for
//! all of them. [`Deployment`] holds one `Loaded` behind one lock and **is
//! itself the `OpRegistry`**, so the door's `RegistryHandle` is installed
//! once at boot and never swapped again: replacing the inner `Loaded`
//! replaces what the door sees, what the executor plans against, and what
//! the catalog publishes, in one write. `implementation.md`'s *"the catalog
//! refreshes shapes and republishes atomically with the swap"* is then a
//! consequence of where the fields live rather than a sequence someone has
//! to get right.
//!
//! A request that has already read the handle finishes on the version it
//! read — `current()` clones an `Arc`, so an in-flight call holds its own
//! version alive until it is done. No drain, no restart. Rollback is
//! [`ordo_exec::registry::activate`] naming an earlier version, which
//! arrives here as an ordinary swap.
//!
//! [`Verified`] is the gate as a type: [`Deployment::swap`] takes one and
//! nothing else, and the only way to obtain one is [`verify_candidate`],
//! which runs the shape gate and the live-schema comparison and returns
//! nothing when either refuses.
//!
//! # What is deliberately still `pinned`
//!
//! [`Deployment::pinned`] builds a deployment from a fixture with **no**
//! shape report — the in-process doors and test harnesses that stand up a
//! router without a database gate. It is not a hole: a pinned deployment
//! can still only ever *swap* to a [`Verified`], so the gate governs every
//! transition even from an ungated start. What a pinned deployment gives up
//! is the claim to have been gated at boot, and `/health/deep` reads that
//! honestly — its `shape_verification` is `null` for a pinned deployment,
//! "the gate did not run here", rather than looking like a gate that ran
//! and found nothing. `/health/ready` (17t) no longer asks the report at
//! all: gate coverage is `/health/deep`'s question, not readiness's.

use std::sync::{Arc, RwLock};

use ordo_api::{OpRegistry, OpSpec, RegistryHandle, SpecError};
use ordo_conformance::Fixture;
use ordo_exec::registry::{self as artifacts, RegistryRefusal};
use ordo_exec::shape::Drift;
use ordo_exec::{Pool, PoolConfig};
use serde_json::Value;

use crate::shape::{ShapeRefusal, ShapeReport};

/// Everything a request must read from **one** version of the program.
///
/// Held together rather than assembled per caller, because the failure of
/// the previous arrangement was not that any one field was wrong — it was
/// that three owners of the same program could disagree about which version
/// they held.
pub struct Loaded {
    /// The artifact registry's version hash, or a name the embedder chose
    /// for a [`Deployment::pinned`] program.
    pub version: String,
    /// The fixture as loaded, bound to whatever space built it. A request's
    /// fixture comes from [`Loaded::in_space`]; this one is the template
    /// every per-space view is cut from, and the schema every
    /// deployment-wide question is answered off.
    pub fixture: Arc<Fixture>,
    /// The pooled executor, built over this version's schema and registry,
    /// and bound to the space that loaded it. A request's executor comes
    /// from [`Loaded::in_space`]; this one is the template.
    pub exec: ordo_exec::Executor,
    /// What the door dispatches on.
    pub specs: Arc<dyn OpRegistry>,
    /// What the catalog renders, computed once per version.
    pub manifests: Arc<Vec<(String, Value)>>,
    /// Proof the load gate ran **for this version**. `None` only for
    /// [`Deployment::pinned`].
    pub report: Option<Arc<ShapeReport>>,
    /// One registered op name, so readiness can prove the registry still
    /// answers without knowing anything about the program.
    pub canary_op: String,
    /// One [`SpaceView`] per space this version has served, minted on first
    /// use.
    ///
    /// **Inside `Loaded` rather than beside it**, because a view carries
    /// this version's schema and registry: a cache that outlived a swap
    /// would hand a request an executor planning the previous program,
    /// which is the half-swap this module's doc exists to describe. A swap
    /// replaces the `Loaded` and the cache goes with it, so the per-space
    /// cut is re-taken once per space per version and can never name a
    /// version that is no longer serving.
    views: RwLock<std::collections::HashMap<String, Arc<SpaceView>>>,
}

/// One version of the program, cut to **one space**.
///
/// The two halves of a request that carry a space, held together for the
/// reason [`Loaded`] holds a version together: `ordo_exec::Executor::space`
/// is the space every read's statements are confined to, and
/// `Fixture::space` is the space the write protocol stamps and seals under.
/// Two owners of the same binding is two things to set, and setting one of
/// them is a read confined to one tenant writing rows into another.
///
/// Nothing here is a parameter a caller passes. The view is resolved once,
/// from the space the door bound, and every call made through it is made
/// with the executor's own field, which is what keeps
/// `ordo_exec::Executor`'s "the isolation boundary rather than a label"
/// true with more than one of them in the process.
pub struct SpaceView {
    /// The space both halves below are bound to.
    pub space: ordo_crypto::SpaceId,
    /// This version's fixture, rebound. The write protocol reads its space.
    pub fixture: Arc<Fixture>,
    /// This version's executor, rebound. Reads plan against its space.
    pub exec: ordo_exec::Executor,
}

impl Loaded {
    /// This version bound to `space`, minted on first use and then shared.
    ///
    /// Cheap after the first call and deliberately not cheap on it: a cut
    /// clones the schema and the registry, which is the cost of having the
    /// binding be a field rather than an argument. Once per space per
    /// version is a cost a deployment can state; once per request would not
    /// be.
    ///
    /// **`pool` is the space's database, and the view is kept only while it
    /// stays the same.** The resolver above this ([`crate::spaces`]) says
    /// which database a space's rows are in; this cuts an executor on that
    /// pool and hands the cut back for as long as the resolver keeps
    /// answering with the same handle. A space the resolver moves gets a new
    /// cut on its next request, which is the property a move needs from this
    /// cache. What a move needs beyond it (a request in flight still holds
    /// the old view and finishes on the old database) is the migration's own
    /// consistency argument, and it is not made here.
    pub fn in_space(&self, space: &str, pool: Pool) -> Arc<SpaceView> {
        if let Some(v) = self
            .views
            .read()
            .expect("space view cache is not poisoned")
            .get(space)
            .filter(|v| v.exec.pool.same_handle(&pool))
        {
            return v.clone();
        }
        let mut views = self
            .views
            .write()
            .expect("space view cache is not poisoned");
        // Re-checked under the write lock: two requests naming a new space
        // at once must share one view, not one each, because a second view
        // is a second executor for one tenant and the cache would then be
        // decoration rather than the thing that bounds the cutting cost.
        if let Some(v) = views.get(space).filter(|v| v.exec.pool.same_handle(&pool)) {
            return v.clone();
        }
        let sid = ordo_crypto::SpaceId(space.to_string());
        let view = Arc::new(SpaceView {
            fixture: Arc::new(self.fixture.in_space(space)),
            exec: ordo_exec::Executor {
                pool,
                schema: self.exec.schema.clone(),
                registry: self.exec.registry.clone(),
                keys: self.exec.keys.clone(),
                space: sid.clone(),
            },
            space: sid,
        });
        views.insert(space.to_string(), view.clone());
        view
    }
}

impl Loaded {
    /// Does this program seal through `keys`?
    ///
    /// **By identity, not by type name.** The failure being guarded against
    /// (R2a) is a program that compiled its own provider — and a
    /// `provider_name` comparison would also accept a second,
    /// differently-configured provider of the same kind, which is precisely
    /// the bug. `Arc::ptr_eq` accepts exactly one thing: the provider the
    /// caller supplied.
    ///
    /// The write path is why this exists: `Executor::keys` covers reads,
    /// and the Stage-5 write protocol seals through `Fixture::keys`.
    pub fn custody_is(&self, keys: &Arc<dyn ordo_crypto::KeyProvider>) -> bool {
        Arc::ptr_eq(&self.fixture.keys, keys)
    }
}

/// A [`Loaded`] whose shape gate passed and whose live database is the shape
/// it asserts. [`Deployment::swap`] takes one of these and nothing else, and
/// [`verify_candidate`] is its only constructor.
pub struct Verified(Loaded);

/// Hand-written because a `Fixture` and an `ordo_exec::Executor` are not
/// `Debug` and should not become so — one holds a key provider. What a
/// reader of a failed `expect` wants is which version and how much of it
/// the gate covered, which is exactly this.
impl std::fmt::Debug for Verified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Verified({}: {})",
            self.0.version,
            self.report().summary()
        )
    }
}

impl Verified {
    /// Build one from a fixture the caller already has and a report the gate
    /// already produced.
    ///
    /// The gate is still the only way in: [`ShapeReport`] is taken **by
    /// value**, and the only function that returns one is
    /// [`crate::shape::verify_registry`]. This exists for the embedder whose
    /// program did not come out of the artifact registry — `ordo dev`, a
    /// single-program self-host, the deployment tests — so that "I ran the
    /// gate" and "my source came from Postgres" stay separate claims.
    ///
    /// Drift is refused here rather than at the two call sites, so every
    /// path that installs a version applies the same rule.
    pub fn from_gate(
        version: impl Into<String>,
        fixture: Arc<Fixture>,
        exec: ordo_exec::Executor,
        report: ShapeReport,
    ) -> Result<Verified, LoadRefusal> {
        let version = version.into();
        if !report.drift.is_empty() {
            return Err(LoadRefusal::SchemaDrift {
                version,
                drift: report.drift,
            });
        }
        let specs = ordo_api::MapRegistry::new(crate::build_specs(&fixture)).map_err(|error| {
            LoadRefusal::Specs {
                version: version.clone(),
                error,
            }
        })?;
        let manifests = crate::manifests(&fixture);
        let canary_op = manifests
            .first()
            .map(|(n, _)| n.clone())
            .unwrap_or_default();
        Ok(Verified(Loaded {
            version,
            fixture,
            exec,
            specs: Arc::new(specs),
            manifests: Arc::new(manifests),
            report: Some(Arc::new(report)),
            canary_op,
            views: RwLock::new(std::collections::HashMap::new()),
        }))
    }

    /// Does this candidate seal through `keys`? See [`Loaded::custody_is`].
    ///
    /// Checked here by [`verify_candidate`] and again by the binary on
    /// every boot *and every swap*: a swapped-in candidate carries a
    /// freshly compiled fixture, so a boot-time-only check would not catch
    /// a regression that reached the tree after the first load.
    pub fn custody_is(&self, keys: &Arc<dyn ordo_crypto::KeyProvider>) -> bool {
        self.0.custody_is(keys)
    }

    /// The schema this candidate would install.
    ///
    /// For the checks a node makes *before* swapping — whether it can serve
    /// what the candidate declares. A node that installed first and
    /// discovered second would be a node whose refusal is an outage rather
    /// than an alert, which is the one thing the deploy channel's policy
    /// forbids.
    pub fn schema(&self) -> &ordo_schema::Schema {
        &self.0.fixture.schema
    }

    /// The version this candidate would install, for a log line before the
    /// swap rather than after it.
    pub fn version(&self) -> &str {
        &self.0.version
    }

    /// What the gate saw. A caller that wants to print coverage before
    /// swapping does not have to swap first to see it.
    pub fn report(&self) -> &ShapeReport {
        self.0.report.as_deref().expect("a Verified always has one")
    }
}

/// Why a stored artifact did not become a serving registry. Every variant
/// is fatal for that version: the deployment keeps serving what it had.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadRefusal {
    /// Nothing has ever been activated in this database. A fresh install,
    /// not a fault — named separately so a node can say *"point `ordo
    /// apply` at me"* rather than *"load failed"*.
    NoActiveVersion,
    /// The stored source does not compile under this binary.
    ///
    /// The realistic cause is not a corrupt row: it is a platform upgrade
    /// that changed the language under an artifact deployed by an older
    /// binary. Refusing loudly is the only safe answer, because the
    /// alternative is a node that silently serves an older version than the
    /// registry says it does.
    Compile {
        version: String,
        error: String,
    },
    /// A spec the door refuses to register — [`ordo_api::OpSpec::validate`].
    Specs {
        version: String,
        error: SpecError,
    },
    /// The program compiled an op whose name the **platform** serves (8r).
    ///
    /// Refused rather than shadowed. `std.` is a reserved namespace and
    /// `std.space.*` is the platform's half of it; a domain that could
    /// register `std.space.callersOf` would own the answer to *"who can
    /// call this"* for its own space, which is the audit surface answering
    /// to the thing it audits.
    ReservedOpName {
        version: String,
        op: String,
    },
    /// The load gate refused.
    Shape {
        version: String,
        refusal: ShapeRefusal,
    },
    /// The live database is not the shape this artifact asserts.
    ///
    /// This is the refusal the empty shadow cannot produce on its own
    /// (see [`crate::shape`]'s module doc), and it is the one that matters
    /// most on a swap: an artifact whose migration has not been applied
    /// verifies perfectly against its own DDL and then fails on the first
    /// request. Refused here rather than reported, because a swap is
    /// precisely the moment the deployment is about to start depending on
    /// the live schema being right.
    SchemaDrift {
        version: String,
        drift: Vec<Drift>,
    },
    Registry(RegistryRefusal),
    Store(ordo_exec::PoolError),
}

impl std::fmt::Display for LoadRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadRefusal::NoActiveVersion => write!(
                f,
                "this database has no active artifact — run `ordo apply` against it before \
                 serving from it"
            ),
            LoadRefusal::Compile { version, error } => write!(
                f,
                "version `{version}` no longer compiles under this binary, so it will not be \
                 served: {error}"
            ),
            LoadRefusal::Specs { version, error } => write!(
                f,
                "version `{version}` produced a spec the door refuses to register: {error}"
            ),
            LoadRefusal::ReservedOpName { version, op } => write!(
                f,
                "version `{version}` registers `{op}`, which is a platform op this build \
                 serves — the `std.space.*` namespace belongs to the platform, and a program \
                 that could shadow it would answer its own audit questions"
            ),
            LoadRefusal::Shape { version, refusal } => {
                write!(f, "version `{version}`: {refusal}")
            }
            LoadRefusal::SchemaDrift { version, drift } => write!(
                f,
                "version `{version}` asserts a schema the live database does not have, so its \
                 ops would fail on the first request rather than at load:\n{}",
                drift
                    .iter()
                    .map(|d| format!("  {d}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            LoadRefusal::Registry(e) => write!(f, "{e}"),
            LoadRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadRefusal {}

impl From<ordo_exec::PoolError> for LoadRefusal {
    fn from(e: ordo_exec::PoolError) -> LoadRefusal {
        LoadRefusal::Store(e)
    }
}

impl From<RegistryRefusal> for LoadRefusal {
    fn from(e: RegistryRefusal) -> LoadRefusal {
        LoadRefusal::Registry(e)
    }
}

/// Compile a stored artifact, gate it, and hand back something swappable.
///
/// Runs, in this order and for these reasons:
///
/// 1. **Compile.** A version that does not compile cannot be posed to the
///    database at all, so nothing below would mean anything.
/// 2. **Register the specs.** `OpSpec::validate` is registration's own
///    refusal, and it is cheap; a registry the door would reject should not
///    cost a shadow build to find out about.
/// 3. **The load gate**, which also observes live-schema drift on the same
///    shadow.
/// 4. **Drift**, refused last of the three because a mismatch is the more
///    specific complaint: a database that disagrees about a *type* is worth
///    naming as a type disagreement even though it is also a drifted
///    schema.
///
/// `pool` is the pool the loaded executor will use; `cfg` is what the gate
/// dials its own short-lived verification pool from. They are separate
/// arguments because verification must not hand a `search_path`-carrying
/// connection back to the serving pool (see [`crate::shape`]).
///
/// **`keys` and `space` are inputs to loading, not properties of the
/// compilation** (R2a; ruled 2026-09-07). `Fixture::compile_source` mints a
/// `TestProvider` over the conformance space, because a compiler is a pure
/// function of source text and has no business deciding whose keys the
/// program seals under. Threading `keys` into the `Executor` alone covers
/// only the READ path: `write_leased` hands the *fixture* to the Stage-5
/// protocol, whose every seal, blind-index digest and sealed-handle mint is
/// `Fixture::keys`. So the fixture's custody and its tenancy are assigned
/// here, before it is shared, and asserted below.
pub async fn verify_candidate(
    stored: &artifacts::Artifact,
    pool: &Pool,
    cfg: &PoolConfig,
    keys: Arc<dyn ordo_crypto::KeyProvider>,
    space: ordo_crypto::SpaceId,
) -> Result<Verified, LoadRefusal> {
    let compiled = Fixture::from_source(&stored.source, "").map_err(|e| LoadRefusal::Compile {
        version: stored.version.clone(),
        error: e,
    })?;
    // **The places the stored amounts are held at** (25aa). A `money(p)`
    // column is `bigint` minor units whatever `p` is, so the shadow below
    // cannot see that the amounts were rescaled for another version. Once a
    // places change has been applied, `$places` says where they are, and a
    // version declaring other places would read every amount 10^n off.
    let held = ordo_exec::migrate::held_places(pool).await?;
    let drift = places_drift(&compiled.schema, &held);
    if !drift.is_empty() {
        return Err(LoadRefusal::SchemaDrift {
            version: stored.version.clone(),
            drift,
        });
    }
    // `stored.ddl` — the DDL captured when this version was applied — not
    // a fresh `ddl_for(&compiled.schema)`: gating against a freshly
    // regenerated DDL would hide a compiler change in DDL generation
    // between the apply and this load, which is precisely the drift this
    // gate exists to catch.
    gate_fixture(
        compiled,
        pool,
        cfg,
        keys,
        space,
        &stored.version,
        &stored.ddl,
    )
    .await
}

/// Every money or decimal field whose declared places differ from the
/// places `$places` records its stored amounts at. Empty when no places
/// change was ever applied, or when every one agrees.
pub fn places_drift(
    schema: &ordo_schema::Schema,
    held: &std::collections::BTreeMap<(String, String), u8>,
) -> Vec<Drift> {
    let mut out = Vec::new();
    for e in &schema.entities {
        for f in e.declared() {
            let Some((money, declared)) = ordo_schema::diff::declared_places(f) else {
                continue;
            };
            let Some(&at) = held.get(&(e.name.clone(), f.name.clone())) else {
                continue;
            };
            if at != declared {
                let ty = if money { "money" } else { "decimal" };
                out.push(Drift {
                    table: e.name.clone(),
                    column: Some(f.column()),
                    object: None,
                    detail: format!(
                        "places: the artifact says {ty}({declared}), the stored amounts are \
                         held at {ty}({at}) (`$places`); activate the version that declares \
                         {ty}({at})"
                    ),
                });
            }
        }
    }
    out
}

/// `ordo dev` (`plan-dev-loop.md` D3), a single-program self-host: a
/// fixture that did not come from the artifact registry, gated exactly the
/// same way as one that did. Unlike [`verify_candidate`], the DDL gated
/// against is freshly derived from `compiled.schema` — there is no prior
/// applied DDL to gate against instead, because `ordo dev` never stores an
/// artifact (§4.3), and the DDL it just reset the database to IS this
/// schema's own `ddl_for`, computed moments before this call.
///
/// `version` is the caller's: `ordo dev` derives it from a content digest
/// of the compiled source (`dev.rs`'s `dev_version`), so an unrelated
/// re-save of unchanged content reports [`Swapped::AlreadyServing`] rather
/// than installing an identical `Loaded` under a fresh label every time —
/// a fixed constant here would make every save after the first do exactly
/// that.
pub async fn verify_embedded(
    compiled: Fixture,
    pool: &Pool,
    cfg: &PoolConfig,
    keys: Arc<dyn ordo_crypto::KeyProvider>,
    space: ordo_crypto::SpaceId,
    version: &str,
) -> Result<Verified, LoadRefusal> {
    let ddl = ordo_schema::ddl_for(&compiled.schema).map_err(|e| LoadRefusal::Compile {
        version: version.to_string(),
        error: e.to_string(),
    })?;
    gate_fixture(compiled, pool, cfg, keys, space, version, &ddl).await
}

/// The shared gating logic behind [`verify_candidate`] (the registry path)
/// and [`verify_embedded`] (`ordo dev`'s path): assign custody and tenancy,
/// validate specs, run the shape gate, and mint a [`Verified`]. Neither
/// caller can reach a `Verified` any other way — see [`Verified::from_gate`]'s
/// own doc on why that separation is the point.
async fn gate_fixture(
    compiled: Fixture,
    pool: &Pool,
    cfg: &PoolConfig,
    keys: Arc<dyn ordo_crypto::KeyProvider>,
    space: ordo_crypto::SpaceId,
    version: &str,
    ddl: &str,
) -> Result<Verified, LoadRefusal> {
    // Tenancy first (`in_space` carries custody across), then custody, so
    // the ordering of the two assignments cannot silently matter.
    let mut fixture = compiled.in_space(&space.0);
    fixture.keys = keys.clone();
    let fixture = Arc::new(fixture);

    // Registration's own refusal, before a shadow build pays for finding
    // out: a registry the door would reject should not cost a schema.
    ordo_api::MapRegistry::new(crate::build_specs(&fixture)).map_err(|error| {
        LoadRefusal::Specs {
            version: version.to_string(),
            error,
        }
    })?;

    let report = crate::shape::verify_registry(&fixture, cfg, ddl)
        .await
        .map_err(|refusal| LoadRefusal::Shape {
            version: version.to_string(),
            refusal,
        })?;

    let exec = ordo_exec::Executor {
        pool: pool.clone(),
        schema: fixture.schema.clone(),
        registry: fixture.registry.clone(),
        keys: keys.clone(),
        space: space.clone(),
    };
    let verified = Verified::from_gate(version, fixture, exec, report)?;

    // The seam, guarded where it is made. A future refactor that
    // reintroduces a `Fixture::from_source` after the assignment above —
    // or a `with_schema`/`in_space` that drops the field — fails here
    // rather than shipping a deployment that reads through the operator's
    // custody and writes under the repository's published test constant.
    //
    // By identity, never by type name: a second, differently-configured
    // `LocalKeyProvider` is exactly the shape of the bug being guarded
    // against, and it would pass a `provider_name` check.
    assert!(
        verified.custody_is(&keys),
        "loaded program `{version}` does not seal through the supplied KeyProvider \
         (custody is an input to loading; see deploy::gate_fixture)"
    );
    assert_eq!(
        verified.0.fixture.space(),
        &space,
        "loaded program `{version}` was not confined to the space it was loaded for"
    );
    Ok(verified)
}

/// Load and gate whatever version the registry currently points at.
pub async fn load_active(
    pool: &Pool,
    cfg: &PoolConfig,
    keys: Arc<dyn ordo_crypto::KeyProvider>,
    space: ordo_crypto::SpaceId,
) -> Result<Verified, LoadRefusal> {
    let Some(stored) = artifacts::active(pool).await? else {
        return Err(LoadRefusal::NoActiveVersion);
    };
    verify_candidate(&stored, pool, cfg, keys, space).await
}

/// Load and gate one named version — what a node does when it is told a
/// specific version became active, and what a rollback resolves to.
pub async fn load_version(
    pool: &Pool,
    cfg: &PoolConfig,
    version: &str,
    keys: Arc<dyn ordo_crypto::KeyProvider>,
    space: ordo_crypto::SpaceId,
) -> Result<Verified, LoadRefusal> {
    let Some(stored) = artifacts::get(pool, version).await? else {
        return Err(LoadRefusal::Registry(RegistryRefusal::UnknownVersion(
            version.to_string(),
        )));
    };
    verify_candidate(&stored, pool, cfg, keys, space).await
}

/// What a swap did. Both arms are worth logging: a node that was already on
/// the version it was nudged toward is the ordinary case when several
/// replicas hear one `NOTIFY`, and it must not read as a swap that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Swapped {
    Installed {
        from: String,
        to: String,
    },
    AlreadyServing(String),
    /// The version compiled a name the platform serves. Carries the
    /// refusal's text; the old version is still installed.
    Refused(String),
}

/// The live program. One lock, one `Loaded`, and the `OpRegistry` the door
/// dispatches through.
pub struct Deployment {
    current: RwLock<Arc<Loaded>>,
    /// Ops the **platform** serves that no program compiled (8r) — see
    /// [`crate::platform_ops`].
    ///
    /// Outside the lock, deliberately. A platform op is not part of a
    /// program version: it does not arrive with a deploy, it does not leave
    /// with a rollback, and its behaviour is this binary's rather than an
    /// artifact's. Putting it in `Loaded` would make it *look* swappable,
    /// and something that looks swappable eventually gets swapped.
    platform: Vec<OpSpec>,
}

impl Deployment {
    /// Boot on a gated version.
    pub fn boot(verified: Verified) -> Arc<Deployment> {
        Arc::new(Deployment {
            current: RwLock::new(Arc::new(verified.0)),
            platform: Vec::new(),
        })
    }

    /// Boot on a gated version **and serve these platform ops**.
    ///
    /// Fallible for one reason: a program that compiled an op with the same
    /// name as a platform op is refused outright rather than shadowed. The
    /// shadow would be the worse outcome by far — `std.space.callersOf` is
    /// an audit surface, and a domain able to register something the door
    /// dispatches under that name could answer the question *"who can call
    /// this"* with whatever it liked. Refused at load, like 8y's
    /// platform-entity write: a check the request path can forget is not a
    /// check.
    pub fn boot_with_platform_ops(
        verified: Verified,
        platform: Vec<OpSpec>,
    ) -> Result<Arc<Deployment>, LoadRefusal> {
        Ok(Arc::new(Deployment {
            platform: Self::vet_platform(&verified.0, platform)?,
            current: RwLock::new(Arc::new(verified.0)),
        }))
    }

    /// Every platform spec, checked against the program's names and against
    /// its own registration rules.
    ///
    /// The collision check runs against **every** name this build reserves,
    /// not only the ones this deployment registers. A deployment that
    /// leaves the exit ops unwired (no `ORDO_EXIT_BUNDLE_DIR`) still mounts
    /// `crate::platform_ops::PlatformExecutor`, which routes on
    /// `is_platform_op` and would take a program's `std.space.delete`
    /// (derived from an entity named `space`) away from the program's
    /// executor. And a program that loads today must not start refusing the
    /// day an operator sets a variable, so the reservation cannot depend on
    /// configuration.
    fn vet_platform(loaded: &Loaded, platform: Vec<OpSpec>) -> Result<Vec<OpSpec>, LoadRefusal> {
        for spec in &platform {
            // Platform ops face `OpSpec::validate` like any other — a
            // `$`-prefixed parameter is exactly as dangerous when the
            // platform declares it.
            spec.validate().map_err(|error| LoadRefusal::Specs {
                version: loaded.version.clone(),
                error,
            })?;
        }
        Self::vet_names(loaded, &platform)?;
        Ok(platform)
    }

    /// The names of the platform ops this deployment serves, for the
    /// startup log.
    pub fn platform_names(&self) -> Vec<String> {
        self.platform.iter().map(|s| s.name.clone()).collect()
    }

    /// The reserved-name half of [`Deployment::vet_platform`], which a swap
    /// repeats against the incoming version.
    fn vet_names(loaded: &Loaded, platform: &[OpSpec]) -> Result<(), LoadRefusal> {
        let reserved = crate::platform_ops::specs();
        for name in reserved
            .iter()
            .map(|s| &s.name)
            .chain(platform.iter().map(|s| &s.name))
        {
            if loaded.specs.get(name).is_some() {
                return Err(LoadRefusal::ReservedOpName {
                    version: loaded.version.clone(),
                    op: name.clone(),
                });
            }
        }
        Ok(())
    }

    /// **No program at all.** A database nobody has run `ordo apply`
    /// against yet.
    ///
    /// `load_active` returning [`LoadRefusal::NoActiveVersion`] is not a
    /// boot failure, and refusing to start on it would make `ordo apply`
    /// impossible to run against a fresh deployment through the ordinary
    /// path: you would need a database with a program in it before you
    /// could start the server the program is deployed to. So the binary
    /// boots into this state, says so once in the startup log, and
    /// subscribes for the deploy notification that will [`swap`] a real
    /// version in.
    ///
    /// Distinct from [`pinned`], which is the shape that must **not**
    /// appear in a binary. `pinned` carries a program with no gate behind
    /// it; this carries no program, and every surface reads it correctly
    /// without being told: `/health/live` answers 200 because the process
    /// is alive, `/health/ready` answers 200 too — owner-ruled 2026-09-20
    /// (17t): readiness is "are we ready to apply and run deployments", not
    /// "is a program running", and a deployment nobody has run `ordo apply`
    /// against yet is exactly the ordinary state that question is "yes"
    /// for — with `program_active: false` in the body naming the one fact
    /// that *is* about this `Loaded`, and `/health/deep` still gates on the
    /// access engine and still has no report to show. Nothing has to
    /// special-case "empty" for readiness to answer correctly; it does have
    /// to say, honestly, that no program is active.
    ///
    /// [`pinned`]: Deployment::pinned
    /// [`swap`]: Deployment::swap
    pub fn empty(
        space: ordo_crypto::SpaceId,
        pool: Pool,
        keys: Arc<dyn ordo_crypto::KeyProvider>,
    ) -> Result<Arc<Deployment>, LoadRefusal> {
        Deployment::empty_with_platform_ops(space, pool, keys, Vec::new())
    }

    /// [`Deployment::empty`], serving these platform ops. The binary boots
    /// this way on a database with no active version, so the first
    /// `ordo apply` swaps a program in beside platform ops that were
    /// already there: `swap` replaces the `Loaded` and never touches the
    /// platform list.
    pub fn empty_with_platform_ops(
        space: ordo_crypto::SpaceId,
        pool: Pool,
        keys: Arc<dyn ordo_crypto::KeyProvider>,
        platform: Vec<OpSpec>,
    ) -> Result<Arc<Deployment>, LoadRefusal> {
        let mut fixture = Fixture::from_source("", "").map_err(|error| LoadRefusal::Compile {
            version: "(none)".into(),
            error,
        })?;
        fixture.keys = keys.clone();
        let fixture = Arc::new(fixture.in_space(&space.0));
        let loaded = Loaded {
            version: String::new(),
            exec: ordo_exec::Executor {
                pool,
                schema: fixture.schema.clone(),
                registry: fixture.registry.clone(),
                keys,
                space,
            },
            specs: Arc::new(
                ordo_api::MapRegistry::new(Vec::new()).expect("an empty registry validates"),
            ),
            manifests: Arc::new(Vec::new()),
            report: None,
            canary_op: String::new(),
            fixture,
            views: RwLock::new(std::collections::HashMap::new()),
        };
        // Vetted like a boot's, so a malformed platform spec is refused
        // here too; no program means no name to collide with.
        let platform = Self::vet_platform(&loaded, platform)?;
        Ok(Arc::new(Deployment {
            current: RwLock::new(Arc::new(loaded)),
            platform,
        }))
    }

    /// A deployment over a fixture that was not loaded from a registry —
    /// in-process doors, embeddings, and the test harnesses that stand up a
    /// router directly. It carries no shape report and says so; it can
    /// still only ever swap to a [`Verified`].
    pub fn pinned(
        version: impl Into<String>,
        fixture: Arc<Fixture>,
        exec: ordo_exec::Executor,
    ) -> Arc<Deployment> {
        let specs = ordo_api::MapRegistry::new(crate::build_specs(&fixture))
            .expect("a fixture's own specs validate");
        let manifests = crate::manifests(&fixture);
        let canary_op = manifests
            .first()
            .map(|(n, _)| n.clone())
            .unwrap_or_default();
        Arc::new(Deployment {
            current: RwLock::new(Arc::new(Loaded {
                version: version.into(),
                fixture,
                exec,
                specs: Arc::new(specs),
                manifests: Arc::new(manifests),
                report: None,
                canary_op,
                views: RwLock::new(std::collections::HashMap::new()),
            })),
            platform: Vec::new(),
        })
    }

    /// [`Deployment::pinned`] plus platform ops, with the same collision
    /// refusal [`Deployment::boot_with_platform_ops`] applies. In-process
    /// doors and test harnesses stand up `std.space.*` through this.
    pub fn pinned_with_platform_ops(
        version: impl Into<String>,
        fixture: Arc<Fixture>,
        exec: ordo_exec::Executor,
        platform: Vec<OpSpec>,
    ) -> Result<Arc<Deployment>, LoadRefusal> {
        let d = Deployment::pinned(version, fixture, exec);
        let platform = Deployment::vet_platform(&d.current(), platform)?;
        Ok(Arc::new(Deployment {
            current: RwLock::new(d.current()),
            platform,
        }))
    }

    /// The catalog's source: this version's manifests, plus the platform's.
    ///
    /// Merged here rather than at the route so that *"the catalog
    /// republishes atomically with the swap"* keeps holding — the program
    /// half still comes through the one pointer a swap moves, and the
    /// platform half does not move at all, which is what it means for it
    /// not to be part of a version.
    pub fn manifests(&self) -> Vec<(String, Value)> {
        let mut out = self.current().manifests.as_ref().clone();
        out.extend(
            crate::platform_ops::manifests()
                .into_iter()
                .filter(|(name, _)| self.platform.iter().any(|s| s.name == *name)),
        );
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The version serving right now. An `Arc` clone: a request that takes
    /// one finishes on it however many swaps land while it runs.
    pub fn current(&self) -> Arc<Loaded> {
        self.current.read().expect("deployment lock").clone()
    }

    /// The handle the door holds. Installed **once**: because `Deployment`
    /// is itself the registry, the handle never has to be swapped, and the
    /// window in which the door could see new specs beside an old executor
    /// does not exist.
    pub fn handle(self: &Arc<Self>) -> Arc<RegistryHandle> {
        Arc::new(RegistryHandle::new(self.clone()))
    }

    /// Install a gated version. Atomic for every reader: one write, under
    /// one lock, to the one pointer everything reads through.
    ///
    /// A deployment that serves platform ops vets the incoming version's
    /// names first, on the terms the boot applied: a version that compiled a
    /// reserved `std.space.*` name is [`Swapped::Refused`] and the node keeps
    /// serving the old one. Without this the boot's refusal would be a
    /// check a deploy could walk around.
    pub fn swap(&self, next: Verified) -> Swapped {
        if !self.platform.is_empty() {
            if let Err(refusal) = Self::vet_names(&next.0, &self.platform) {
                return Swapped::Refused(refusal.to_string());
            }
        }
        let mut slot = self.current.write().expect("deployment lock");
        if slot.version == next.0.version {
            return Swapped::AlreadyServing(slot.version.clone());
        }
        let from = slot.version.clone();
        let to = next.0.version.clone();
        *slot = Arc::new(next.0);
        Swapped::Installed { from, to }
    }
}

impl OpRegistry for Deployment {
    fn get(&self, name: &str) -> Option<OpSpec> {
        // Platform ops first. Collisions are refused at load, so the order
        // decides nothing today — it is the defensive one, so that if a
        // future path ever installs a program without the vet, the audit
        // surface is the thing that survives rather than the thing that is
        // shadowed.
        if let Some(spec) = self.platform.iter().find(|s| s.name == name) {
            return Some(spec.clone());
        }
        self.current().specs.get(name)
    }
}

/// The notifications a node acts on: a new version, and a new tenant.
///
/// Separated from the swap itself so a caller decides its own policy for a
/// refused load. The one this platform recommends is *keep serving*: a node
/// that cannot load the new version must not stop serving the old one, and
/// the refusal is an operator alert, not an outage.
///
/// **Two channels, one socket.** A `LISTEN` session is a dedicated
/// connection ([`ordo_exec::jobs::Listener`]'s own doc says why it cannot
/// be a pooled one), so a second listener for the second channel would
/// double the connections every replica holds idle to learn about events
/// that arrive a handful of times a day. The receiver carries
/// `Notification::channel`, so one task tells them apart.
pub async fn listen_for_changes(
    cfg: &PoolConfig,
) -> Result<
    (
        ordo_exec::jobs::Listener,
        tokio::sync::mpsc::UnboundedReceiver<ordo_exec::jobs::Notification>,
    ),
    ordo_exec::PoolError,
> {
    ordo_exec::jobs::listen(
        cfg,
        &[artifacts::DEPLOY_CHANNEL, ordo_exec::jobs::MEMBERS_CHANNEL],
    )
    .await
}

/// Tell every replica that a space now exists, so each rebuilds its door.
///
/// **Not in a transaction, deliberately**, which is the opposite of
/// `registry::activate`'s notify. That one rides its own transaction
/// because the notification must not outrun the activation row it
/// announces. Here the founding member row was committed by
/// [`crate::provision::provision_space`] before this is called, so there
/// is nothing left to outrun, and wrapping a bare `pg_notify` in a
/// transaction would only take a connection for longer.
///
/// The error is a `String` rather than a refusal type because the caller
/// does not retry and must not fail over it: the tenant exists either way.
/// What changes is *when* it becomes reachable — now, or at the next
/// restart — and `std.space.create` reports which in its answer rather
/// than letting the caller assume.
pub async fn announce_members(pool: &Pool, space: &str) -> Result<(), String> {
    pool.rows(&format!(
        "SELECT pg_notify({}, {})",
        ordo_exec::sql_lit(ordo_exec::jobs::MEMBERS_CHANNEL),
        ordo_exec::sql_lit(space),
    ))
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}
