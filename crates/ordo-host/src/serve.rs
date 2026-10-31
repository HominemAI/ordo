//! 8o — the deployment surface, and the load path that produces it.
//!
//! # Why this router is not in `ordo-api`
//!
//! `ordo-api`'s router is deliberately **generated from the registry**: one
//! path (`/op/{name}`), no per-op code, and — the load-bearing half — no
//! dependency on any sibling crate. Its whole boundary is five traits.
//!
//! Every surface here breaks one of those two properties on purpose:
//!
//! - The **catalog** is compiler output. It renders manifests: guard
//!   provenance, cost class, the FIELDS reached through composition and
//!   through every inlined guard (9l), the key release a run needs, the
//!   `unguarded` bypass disclosure. `ordo-api` cannot see any of that
//!   without depending on `ordo-schema`/`ordo-sema`/`ordo-plan`, and a
//!   `Catalog` trait added to its boundary would be a sixth trait with
//!   exactly one implementor, invented to keep a route in the wrong crate.
//! - **`/health/ready`** must answer for the *deployment's* dependencies —
//!   this pool, this KeyProvider, this configuration, these workers.
//!   `ordo-api` has no pool and no KeyProvider, and by design should never
//!   learn what one is.
//! - **`/metrics`** is per-process deployment telemetry, not a served op.
//!
//! None of them is dispatch. So they live in a second router that a
//! deployment merges alongside the generated one:
//!
//! ```ignore
//! let app = ordo_api::router(api_state).merge(ordo_host::platform_router(platform));
//! ```
//!
//! `ordo-api` keeps its one path and its zero sibling dependencies; the
//! crate that assembles a deployment owns the surfaces that describe one.
//!
//! # Access
//!
//! The catalog is scoped per op by the same checker the door uses, so an
//! anonymous asker gets an empty catalog rather than a list of what exists.
//! `/health/deep`, `/metrics` and `/security/alerts` are gated on reserved
//! op names ([`HEALTH_DEEP_OP`], [`METRICS_OP`], [`ALERTS_OP`]) that
//! [`crate::deployment_engine`] grants to owners only: deep health names the
//! deployment's KeyProvider and database, metrics carry principal ids across
//! every space, and the alert surface names which principals look like they
//! are probing. All three are checked against the deployment's own space
//! ([`authorize`]) rather than a space the caller names, because each is a
//! deployment-wide disclosure and not an ordinary per-space answer.
//! `/health/live` and `/health/ready` are open, because a probe that needs
//! a credential is a probe that fails during exactly the incident it
//! exists for. Owner-ruled 2026-09-20 (17t): readiness answers *"can this
//! platform accept and run a deployment"*. Since the deploy audit (F12) an
//! anonymous asker gets the status alone (the HTTP code and `status`); the
//! detail an operator standing one up needs — a check's name (`database`,
//! `custody`, `config`, `workers`, `deploy_path`), which `config`
//! requirement is unmet, the tenants held closed and the credential labels
//! — goes to the deployment owner's token, as `/health/deep` does. Never a
//! credential's value, a hostname, or a port in either case. `/health/deploy`
//! follows the same line: anonymous gets `settled` only; node ids, the
//! version and the pending work are the owner's. `/health/deep` keeps the
//! deeper disclosure — the KeyProvider and the database themselves.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use ordo_api::{AccessChecker, AccessDecision, ApiError, ErrorCode, TokenGate};
use serde_json::{json, Value};

use crate::metrics::Metrics;

/// The reserved op name `/health/deep` is checked against. Not a registered
/// op — a name in the same namespace, so the deployment's ordinary access
/// model decides who may see the deployment's own internals rather than a
/// second, parallel notion of "operator".
pub const HEALTH_DEEP_OP: &str = "platform.health.deep";
/// The reserved op name `/metrics` is checked against. Separate from
/// [`HEALTH_DEEP_OP`] because they are different disclosures: deep health
/// names dependencies, metrics name principals.
pub const METRICS_OP: &str = "platform.metrics";
/// The reserved op name `/security/alerts` is checked against.
///
/// A third name rather than a reuse of [`METRICS_OP`] because it is a third
/// disclosure: the metrics endpoint hands over every series, and this one
/// hands over the platform's *judgement* about which principals look like
/// they are probing. Naming a suspect is a different act from publishing a
/// counter, and a deployment that wants the second without the first must
/// be able to say so.
pub const ALERTS_OP: &str = "platform.security.alerts";
/// The reserved op name `/security/events` is checked against — and it is
/// checked **in the space being read**, not in the deployment's own.
///
/// This is the one route on this router that is not an operator surface, and
/// the distinction is the whole of 7d(b). `security-logging-design.md` §6.1
/// routes a space's own denials to that space's stream *precisely so its
/// owner can see them*, and until this route existed no owner could read
/// them at all: `/security/alerts` serves the deny-volume monitor's
/// firings, which is the platform's judgement about who looks like they are
/// probing, not the record.
///
/// A fourth name rather than a reuse of [`ALERTS_OP`] because the two are
/// different disclosures to different people. Alerts are deployment-wide and
/// name suspects across every tenant; this is one space's record, readable
/// by that space. A deployment that installs one in a tenant profile has not
/// installed the other.
pub const SECEVENTS_OP: &str = "platform.security.events";
/// The reserved op name `/usage` is checked against (17n M3). Beside
/// [`METRICS_OP`] for the same reason: `/usage?space=<any tenant>` names a
/// deployment-wide operator surface over every space this process serves,
/// not a per-space question the caller's own token answers for itself.
pub const USAGE_OP: &str = "platform.usage";

/// Everything the deployment surface answers from. Cheap to clone — every
/// field is a handle.
#[derive(Clone)]
pub struct PlatformState {
    pub access: Arc<dyn AccessChecker>,
    /// The live program — fixture, specs, manifests and the shape report
    /// that vouched for them, all of one version. Every one of those used to
    /// be a field here, snapshotted at boot; a swap moved the registry and
    /// left the rest describing the version before it. Asking the
    /// deployment is what keeps `/health/deep` and the catalog about the
    /// program that is actually serving.
    pub deployment: Arc<crate::deploy::Deployment>,
    pub pool: ordo_exec::Pool,
    pub keys: Arc<dyn ordo_crypto::KeyProvider>,
    pub space: String,
    pub metrics: Arc<Metrics>,
    /// Where a bearer token comes from, when a deployment has wired one.
    ///
    /// `None` is a configuration a deployment can be in and the routes say
    /// so by name (`auth_unconfigured`) rather than by 404, for the reason
    /// the drain's `wired: false` exists: a surface missing because nobody
    /// wired it must not look like one that does not exist.
    pub auth: Option<crate::auth::SharedAuth>,
    /// What the door that is serving was built **without** (18d T5).
    ///
    /// A closure rather than the `SwappableAccess` itself, because this
    /// module holds an `Arc<dyn AccessChecker>` and the unread report
    /// belongs to the one implementation that rebuilds. A trait method
    /// would put a question about routed directories on every access
    /// checker, including the test doubles that have no directory.
    ///
    /// `None` is an assembly with no rebuildable door, which is every
    /// in-process harness; it reports nothing here rather than pretending
    /// to an empty answer.
    #[allow(clippy::type_complexity)]
    pub access_unread: Option<Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>>,
    /// The spaces the door serves from their last good member list through a
    /// passing failure, named beside `access_unread` on `/health/ready`.
    #[allow(clippy::type_complexity)]
    pub access_kept_stale: Option<Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>>,
    /// The security-event drain's own health, when one is running.
    ///
    /// `None` means **no drainer is wired**, and `/health/deep` says so in
    /// those words rather than omitting the section. A deployment whose
    /// security stream is accumulating in memory and reaching no database
    /// is in a specific and serious condition — a restart loses every
    /// unwritten denial — and "the field is absent" is not a way to
    /// communicate it.
    pub security: Option<Arc<crate::secdrain::DrainHealth>>,
    /// The supervisor's per-worker census (R3), on the same terms as
    /// `security` above: **not running is a first-class value**, reported
    /// with its reason rather than by absence.
    ///
    /// Held as the supervisor itself so the same handle answers both the
    /// per-worker census and the `supervisor` section (leadership and
    /// orphaned listener cursors).
    pub supervisor: Option<Arc<crate::supervisor::Supervisor>>,
    /// The blob store's label, when one is attached (`crate::runtime`'s
    /// `blob_label`) — `None` for a deployment with none.
    ///
    /// Held as the label rather than the store itself: `/health/ready`
    /// never touches storage (see [`health_ready`]'s doc), it only asks
    /// whether *this program's* declared fields have somewhere to put
    /// their bytes, which is a question about configuration, not about
    /// reachability.
    pub blob: Option<String>,
    /// The space resolver, for the quarantine register `/health/ready`
    /// reports (17aa). `None` in a process that serves no spaces through a
    /// resolver at all (a probe-only embedding), which readiness states
    /// rather than reads as "nothing is closed".
    pub spaces: Option<Arc<crate::spaces::Spaces>>,
    /// Every label the tenant credential provider holds, names only, never
    /// a value (18d T3 / 18e U4, §9.7 C-M3). `/health/ready` prints these
    /// so a fleet-wide move (18e, out of scope here) can check every node's
    /// labels before it copies anything, without dialling any of them.
    /// `None` is a process assembled without `runtime::assemble` (a
    /// probe-only embedding); an empty `Vec` is an assembled process with
    /// no credential provider configured, which is every deployment before
    /// this field existed and remains a legitimate answer.
    pub credential_labels: Option<Vec<String>>,
    /// How a space's own database is found, for `/security/events` (7d).
    ///
    /// **This is not a convenience.** S4 routes each space's security
    /// entries to that space's own database and only the deployment's to the
    /// control database, so reading a routed tenant's stream from
    /// [`Self::pool`] finds **nothing** — and for a security surface
    /// "nothing" renders as *your space has recorded no denials*, which is
    /// the most dangerous answer it could give. A clean bill of health for a
    /// stream nobody looked at is worse than an error.
    ///
    /// `None` is a process assembled without a resolver, and the route says
    /// so by name rather than answering an empty list, for the reason the
    /// drain's `wired: false` exists: a surface missing because nobody wired
    /// it must not look like one with nothing in it.
    pub pools: Option<Arc<dyn ordo_exec::route::PoolResolver>>,
}

impl PlatformState {
    /// Assemble the deployment surface **after** the shape gate has passed.
    ///
    /// The gate is still by type and is now enforced one layer down: a
    /// [`crate::deploy::Deployment`] booted from a
    /// [`crate::deploy::Verified`] carries a report, and the only
    /// constructor for a `Verified` runs the gate. Taking the deployment
    /// rather than a loose `ShapeReport` is what stops the report and the
    /// registry from being about different versions.
    ///
    /// A [`crate::deploy::Deployment::pinned`] one carries no report, and
    /// readiness says so rather than pretending: see [`health_ready`].
    pub fn new(
        access: Arc<dyn AccessChecker>,
        deployment: Arc<crate::deploy::Deployment>,
        pool: ordo_exec::Pool,
        keys: Arc<dyn ordo_crypto::KeyProvider>,
        space: impl Into<String>,
        metrics: Arc<Metrics>,
    ) -> PlatformState {
        PlatformState {
            access,
            deployment,
            pool,
            keys,
            space: space.into(),
            metrics,
            security: None,
            supervisor: None,
            auth: None,
            blob: None,
            spaces: None,
            access_unread: None,
            access_kept_stale: None,
            credential_labels: None,
            pools: None,
        }
    }

    /// Let `/health/ready` see what the serving door was built without
    /// (18d T5). The **same** `SwappableAccess` the door resolves
    /// through, for `with_spaces`' reason: two answers to "which tenants
    /// did the door skip" is two opinions about which tenants serve.
    pub fn with_access_unread(
        mut self,
        access: Arc<crate::runtime::SwappableAccess>,
    ) -> PlatformState {
        let again = access.clone();
        self.access_kept_stale = Some(Arc::new(move || again.kept_stale()));
        self.access_unread = Some(Arc::new(move || access.unread()));
        self
    }

    /// Attach the space resolver, so `/health/ready` reports the spaces
    /// this deployment is holding closed (17aa). The **same** `Arc` the
    /// door resolves through, never a second resolver: what readiness
    /// reports has to be what the door found, and two resolvers over one
    /// deployment are two opinions about which tenants serve.
    pub fn with_spaces(mut self, spaces: Arc<crate::spaces::Spaces>) -> PlatformState {
        self.spaces = Some(spaces);
        self
    }

    /// Wire authentication. Without this the deployment verifies tokens it
    /// has no way to issue, which is the state every deployment was in
    /// before `auth` existed.
    pub fn with_auth(mut self, auth: crate::auth::SharedAuth) -> PlatformState {
        self.auth = Some(auth);
        self
    }

    /// Attach the running supervisor's census, so `/health/deep` answers
    /// for the loops as well as for the door.
    pub fn with_supervisor(
        mut self,
        supervisor: Arc<crate::supervisor::Supervisor>,
    ) -> PlatformState {
        self.supervisor = Some(supervisor);
        self
    }

    /// Attach the running security drain's health, so `/health/deep`
    /// answers for the audit record as well as for the database.
    pub fn with_security_drain(
        mut self,
        health: Arc<crate::secdrain::DrainHealth>,
    ) -> PlatformState {
        self.security = Some(health);
        self
    }

    /// Attach the blob store's label, when a deployment configured one, so
    /// `/health/ready` can tell a program that needs a store and has one
    /// from a program that needs one and does not. A deployment that
    /// attached nothing leaves this `None`, which is correct exactly when
    /// no loaded program declares a non-inline `blob` field — see
    /// [`crate::runtime::blob_needed`]'s ruling for why that is the only
    /// case a store is required at all.
    pub fn with_blob_label(mut self, label: Option<String>) -> PlatformState {
        self.blob = label;
        self
    }

    /// Attach the tenant credential provider's labels, so `/health/ready`
    /// can print them (§9.7 C-M3). The provider itself is never held here:
    /// this is names read once at boot, not a handle a health check could
    /// be tempted to dial through.
    pub fn with_credential_labels(mut self, labels: Vec<String>) -> PlatformState {
        self.credential_labels = Some(labels);
        self
    }

    /// Attach the resolver `/security/events` reads a space's stream
    /// through. **The door's own**, never a second one, for the reason
    /// [`Self::with_spaces`] already gives: two resolvers over one
    /// deployment can disagree, and then what a surface reports is not what
    /// the deployment does.
    pub fn with_pools(mut self, pools: Arc<dyn ordo_exec::route::PoolResolver>) -> PlatformState {
        self.pools = Some(pools);
        self
    }
}

/// The deployment's own surface. Merge alongside [`ordo_api::router()`].
pub fn platform_router(state: PlatformState) -> Router {
    let auth = state.auth.clone();
    Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/health/deep", get(health_deep))
        .route("/health/deploy", get(health_deploy))
        .route("/metrics", get(metrics_route))
        .route("/security/alerts", get(alerts_route))
        .route("/security/events", get(secevents_route))
        .route("/usage", get(usage_route))
        // Every `/auth/...` route and the WebAuthn well-known document, from
        // the one list the in-process router also uses (defect 25ax).
        .merge(crate::auth::auth_routes(auth))
        // Both renderings at one well-known endpoint (F6), plus the short
        // alias a human types. JSON by default, prose on `?format=prose`
        // or an `Accept: text/plain`.
        .route("/.well-known/ordo-catalog", get(catalog_route))
        .route("/catalog", get(catalog_route))
        .route("/.well-known/ordo-interface", get(interface_route))
        .route("/interface", get(interface_route))
        .with_state(state)
}

// ── health ───────────────────────────────────────────────────────────────

/// Process liveness. Touches nothing: a probe that consults a dependency
/// restarts the process when the *dependency* is sick, which turns one
/// outage into two.
async fn health_live() -> Response {
    (StatusCode::OK, Json(json!({ "status": "live" }))).into_response()
}

/// Where a version stands across the running nodes: whether each serves it
/// and has finished bringing the tenants' databases to it. `ordo apply
/// --host` waits on this, and an app developer can ask it before a page
/// calls an op the new version added.
///
/// `?version=<v>` asks about one version; without it, the active one. It
/// reads `$nodes` ([`ordo_exec::nodes`]), the rows each node writes. A
/// database a sweep left behind is counted and not named: its label is a
/// host and a port, which this public surface does not print. `ordo apply`
/// against the database names them.
async fn health_deploy(
    State(state): State<PlatformState>,
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    // Node ids, the version and what each node is doing are the deployment
    // owner's (audit F12). An anonymous asker gets whether it has settled.
    let owner = authorize(&state, &headers, HEALTH_DEEP_OP).await.is_some();
    let version = match query.get("version").filter(|v| !v.is_empty()) {
        Some(v) => v.clone(),
        None => ordo_exec::registry::active_version(&state.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default(),
    };
    let nodes = match ordo_exec::nodes::live(&state.pool).await {
        Ok(n) => n,
        Err(e) => {
            // The reason is the operator's: a pool error can name a host.
            eprintln!("health/deploy: the nodes could not be read: {e}");
            return ApiError::by_fault(
                e.class(),
                ApiError::server_busy(),
                ApiError::new(ErrorCode::SpaceUnavailable, "the nodes could not be read"),
            )
            .into_response();
        }
    };
    let body = if owner {
        deploy_status(&version, &nodes)
    } else {
        deploy_status_anonymous(&version, &nodes)
    };
    (StatusCode::OK, Json(body)).into_response()
}

/// The body of `GET /health/deploy` for an asker who is not the deployment
/// owner: whether every live node has settled on `version`, and nothing
/// that names a node or a version.
pub fn deploy_status_anonymous(version: &str, nodes: &[ordo_exec::nodes::Node]) -> Value {
    let settled = ordo_exec::nodes::progress(nodes, version).settled();
    json!({
        "status": if settled { "settled" } else { "pending" },
        "settled": settled,
    })
}

/// The body of `GET /health/deploy`. Public so the CLI and the tests read
/// the same shape the door writes.
pub fn deploy_status(version: &str, nodes: &[ordo_exec::nodes::Node]) -> Value {
    let progress = ordo_exec::nodes::progress(nodes, version);
    json!({
        "version": version,
        "settled": progress.settled(),
        "held": progress.held.len(),
        "pending": progress.pending.iter().map(|(node, doing)| json!({
            "node": node,
            "doing": doing,
        })).collect::<Vec<_>>(),
        "nodes": nodes.iter().map(|n| json!({
            "node": n.node,
            "serving": n.serving,
            "caughtUp": n.caught_up,
            "held": n.held.len(),
            "retrying": n.retrying,
            "sweepMs": n.sweep_ms,
            "seenSecsAgo": n.age_secs,
        })).collect::<Vec<_>>(),
    })
}

/// Readiness — owner-ruled 2026-09-20 (17t, `corpus-rulings.md`): *"Being a
/// baas in a way, a 503 is not an appropriate status. Readiness is not a
/// measure of is a deployment running, it is a measure of are we ready to
/// apply and run deployments."*
///
/// A platform exists to **receive** programs, so a correctly configured
/// deployment with none applied yet is ready by definition — a
/// [`crate::deploy::Deployment::empty`] one, which this handler must answer
/// 200 for, is the ordinary first minute of a fresh deployment, not an
/// outage. What readiness asks instead is the conjunction of what must be
/// true for a deploy to succeed and for traffic to be served once one
/// lands:
///
/// - **`database`** — reachable. A pooled round trip (one `SELECT 1`), not
///   a dial: saturation is also not-ready, and this is the only probe that
///   can see it.
/// - **`custody`** — openable for this deployment's own space. See the
///   `blind_index` probe below for why this is the cheapest question that
///   actually proves it, at the cost of one in-process unwrap (no lease,
///   no DEK release, no network round trip beyond what the provider itself
///   dials).
/// - **`config`** — required configuration for the program that is
///   *actually loaded* is present. Everything `runtime::assemble` requires
///   (the token and page keys, the master key, `ORDO_PG_*`) already refused
///   to boot without, so it cannot be missing here — except a blob store,
///   which a **swap** can make necessary after boot in a way the boot-time
///   check never sees again. So this asks one thing: does the live
///   program declare a non-inline `blob` field, and if so, is one
///   attached. An in-memory schema scan, no I/O, cheap enough for every
///   probe.
/// - **`workers`** — the workers this process must run are running, not
///   quarantined. Atomic loads over an in-memory census
///   ([`crate::supervisor::Supervisor::quarantined`]); a process with no
///   supervisor (the dev embedding, the conformance harness) has nothing
///   required, so it passes.
/// - **`deploy_path`** — the database role this deployment connects as can
///   create tables, which is what a first `ordo apply` needs to do. One
///   more pooled round trip (`has_database_privilege`), skipped when
///   `database` already failed rather than spent on a connection that is
///   already down.
///
/// - **`continuity`** — the *control plane's own* history is accounted
///   for. A tenant whose chain moved backwards is quarantined and named in
///   the body's `quarantined` array without touching this check or the
///   status code: under 17aa a tenant's data state never decides whether
///   the deployment is ready, because the deployment staying up is what
///   lets an operator record the restore that lifts the quarantine. An
///   in-memory read of the resolver's register, no I/O.
///
/// Whether a **program** is active — the fact the old readiness answered
/// with its status code — is reported in the body as `program_active`
/// instead, alongside a `checks` object naming which check failed and why,
/// in `/health/deep`'s style. Never a credential's value, a host or a
/// port: see this module's doc for the line the detail stays behind.
/// `/health/deep` keeps the shape report and its owner gate exactly as
/// they were; this handler no longer reads either.
async fn health_ready(State(state): State<PlatformState>, headers: HeaderMap) -> Response {
    let owner = authorize(&state, &headers, HEALTH_DEEP_OP).await.is_some();
    // One read of the live version, so every check below is about the same
    // program even if a swap lands mid-probe.
    let live = state.deployment.current();
    let mut checks = serde_json::Map::new();
    let mut failed: Vec<&str> = Vec::new();

    fn ok_check(checks: &mut serde_json::Map<String, Value>, name: &'static str, detail: String) {
        checks.insert(name.to_string(), json!({ "ok": true, "detail": detail }));
    }
    fn fail_check(
        checks: &mut serde_json::Map<String, Value>,
        failed: &mut Vec<&'static str>,
        name: &'static str,
        detail: String,
    ) {
        checks.insert(name.to_string(), json!({ "ok": false, "detail": detail }));
        failed.push(name);
    }

    // ── database ─────────────────────────────────────────────────────────
    let db_ok = state.pool.rows("SELECT 1").await.is_ok();
    if db_ok {
        ok_check(&mut checks, "database", "reachable".to_string());
    } else {
        fail_check(
            &mut checks,
            &mut failed,
            "database",
            "a pooled round trip failed: the database is unreachable".to_string(),
        );
    }

    // ── custody ──────────────────────────────────────────────────────────
    //
    // KeyProvider reachable **and holding the right master**.
    //
    // This was `current_epoch`, and `current_epoch` is a *presence* check:
    // it takes the maximum epoch of the store's DEK rows and unwraps
    // nothing. So did `release`. A replica booted with a mistyped
    // `ORDO_MASTER_KEY` passed both, reported **ready**, and then failed
    // every read of every sealed field — the worst available shape, because
    // a load balancer had already sent it traffic.
    //
    // `blind_index` is the cheapest question that actually unwraps: it
    // derives the per-field index key from the space's SMK, so a wrong
    // master is `KeyError::Seal` here rather than at the first customer
    // read. It takes no lease and releases no DEK, which is the property
    // readiness needs. The entity and field are reserved names, so the
    // digest it computes addresses nothing.
    // `ordo-crypto`'s `a_wrong_master_is_invisible_to_the_presence_checks`
    // pins the distinction this rests on.
    // **Asked about the control plane's own space, never about a tenant**
    // (17ac). It used to probe `state.space`, which is `ORDO_SPACE`, so a
    // deployment pointed at a tenant nobody had provisioned answered
    // `not_ready` and a load balancer took it out of rotation — one
    // tenant's state deciding whether the whole deployment is ready, which
    // is the shape 17aa removed from the boot and 17ac removes here. The
    // system space is created unconditionally at boot, so it is the space
    // a deployment can always be asked this of, and it is the one whose
    // custody failing genuinely means the deployment cannot serve anyone.
    //
    // A tenant whose custody is missing is not silently dropped: it is
    // quarantined and named in `quarantined` below, with the command that
    // fixes it. That is the honest split, and it is `continuity`'s split
    // exactly: this check answers for the control plane, the register
    // answers for tenants.
    //
    // **The question itself lives in `ordo_crypto::probe`**, so this and
    // `std.space.create`'s preflight ask the identical one. Two copies of
    // "is custody healthy" is two answers waiting to disagree, and the
    // reasoning above is exactly the sort that gets reproduced badly.
    let home = ordo_crypto::SpaceId(crate::provision::SYSTEM_SPACE.to_string());
    if ordo_crypto::probe(state.keys.as_ref(), &home).is_ok() {
        ok_check(
            &mut checks,
            "custody",
            "open for the control plane's own space".to_string(),
        );
    } else {
        fail_check(
            &mut checks,
            &mut failed,
            "custody",
            "the key provider could not open custody for the control plane's own space".to_string(),
        );
    }

    // ── config ───────────────────────────────────────────────────────────
    //
    // Everything `runtime::assemble` treats as required already refused to
    // boot without — this asks the one thing a **swap** can change after
    // boot: whether the program that is actually live declares a
    // non-inline `blob` field with nowhere to put its bytes. See
    // `crate::runtime::blob_needed`'s doc for the three-way rule this reads.
    match crate::runtime::blob_needed(&live.fixture.schema) {
        Some(field) if state.blob.is_none() => fail_check(
            &mut checks,
            &mut failed,
            "config",
            format!(
                "`{field}` is a stored blob field and no blob store is configured \
                 (ORDO_BLOB_STORE)"
            ),
        ),
        Some(field) => ok_check(
            &mut checks,
            "config",
            format!("blob store configured for `{field}`"),
        ),
        None => ok_check(
            &mut checks,
            "config",
            "no additional configuration required by the active program".to_string(),
        ),
    }

    // ── workers ──────────────────────────────────────────────────────────
    match state.supervisor.as_deref() {
        Some(sup) => {
            let quarantined = sup.quarantined();
            if quarantined.is_empty() {
                ok_check(
                    &mut checks,
                    "workers",
                    "all registered workers running".to_string(),
                );
            } else {
                fail_check(
                    &mut checks,
                    &mut failed,
                    "workers",
                    format!("quarantined: {}", quarantined.join(", ")),
                );
            }
        }
        // `null` on `/health/deep`'s own terms: no supervisor in this
        // process is a different fact from every worker being stopped, and
        // there is nothing required here to be not-running.
        None => ok_check(
            &mut checks,
            "workers",
            "no supervisor in this process".to_string(),
        ),
    }

    // ── deploy_path ──────────────────────────────────────────────────────
    //
    // Whether the role this deployment connects as could actually create
    // the tables a first `ordo apply` needs. Skipped when `database`
    // already failed, so an already-down connection is not charged a
    // second round trip for an answer its first one already gave.
    if db_ok {
        match state
            .pool
            .rows("SELECT has_database_privilege(current_user, current_database(), 'CREATE')")
            .await
        {
            Ok(rows)
                if rows
                    .first()
                    .and_then(|r| r.first())
                    .and_then(|v| v.as_deref())
                    == Some("t") =>
            {
                ok_check(
                    &mut checks,
                    "deploy_path",
                    "can create schema objects".to_string(),
                );
            }
            Ok(_) => fail_check(
                &mut checks,
                &mut failed,
                "deploy_path",
                "this role has no CREATE privilege, so a deploy could not create tables"
                    .to_string(),
            ),
            Err(_) => fail_check(
                &mut checks,
                &mut failed,
                "deploy_path",
                "could not evaluate this role's privileges".to_string(),
            ),
        }
    } else {
        fail_check(
            &mut checks,
            &mut failed,
            "deploy_path",
            "the database is unreachable, so nothing can be applied to it".to_string(),
        );
    }

    // ── continuity ───────────────────────────────────────────────────────
    //
    // **The control plane's own history, and nothing else's** (17aa). A
    // tenant whose chain moved backwards is that tenant's fact: it is
    // quarantined, it is named in `quarantined` below, and it does not make
    // this deployment unready, because the deployment can still receive a
    // program, serve every other tenant, and take the `ordo restore
    // --record` that lifts the quarantine. Readiness that failed on it
    // would be the process-wide refusal 17aa removed, wearing a status
    // code.
    //
    // The system space is the one whose answer *is* a readiness answer:
    // it is where the platform keeps what it knows about itself, and a
    // deployment whose own history it cannot account for is not ready to
    // apply and run deployments. Its id is a constant compiled into this
    // binary, so nothing a caller or an operator types selects it.
    // **One list, assembled here from every source that can close a
    // space** (18d T5 / R-F1). Two sources today:
    //
    // - the resolver's quarantine register, which a failing resolve
    //   writes and a later successful one lifts, and
    // - the door's own report of tenants whose members it could not read
    //   when it was last built.
    //
    // They were separate until T5, with the second reaching only a line
    // printed at boot, so a tenant whose database was down at door-build
    // time failed every request while `/health/ready` said nothing. An
    // operator's first evidence was a customer.
    //
    // Merged rather than concatenated: a space can be in both at once (the
    // door could not read it, and then a request for it failed to
    // resolve), and naming it twice would make one broken tenant look like
    // two. The register wins, because it is the more recent fact: the door
    // is built at boot and on deploy, while the register is written on
    // every request.
    let mut by_space: std::collections::BTreeMap<String, String> = state
        .access_unread
        .as_ref()
        .map(|f| f().into_iter().collect())
        .unwrap_or_default();
    for (space, reason) in state
        .spaces
        .as_deref()
        .map(|s| s.quarantined())
        .unwrap_or_default()
    {
        by_space.insert(space, reason);
    }
    let held: Vec<(String, String)> = by_space.into_iter().collect();
    match state.spaces.as_deref() {
        None => ok_check(
            &mut checks,
            "continuity",
            "no space resolver in this process".to_string(),
        ),
        Some(_) => match held
            .iter()
            .find(|(space, _)| space == crate::provision::SYSTEM_SPACE)
        {
            Some((_, why)) => fail_check(
                &mut checks,
                &mut failed,
                "continuity",
                format!(
                    "the control plane's own space `{}` is held closed: {why}",
                    crate::provision::SYSTEM_SPACE
                ),
            ),
            None if held.is_empty() => ok_check(
                &mut checks,
                "continuity",
                "every space this deployment has read accounts for its history".to_string(),
            ),
            // Ready, and saying so in the same breath as naming what is
            // closed. The count is here rather than only in `quarantined`
            // so a probe that reads `checks` alone still sees it.
            None => ok_check(
                &mut checks,
                "continuity",
                format!(
                    "the control plane accounts for its history; {} tenant space(s) held \
                     closed, named in `quarantined`",
                    held.len()
                ),
            ),
        },
    }

    // Whether a program is active is a fact, not a gate: a deployment with
    // none applied is exactly as ready as one that is serving, and the old
    // behaviour of answering 503 until this was true is the bug 17t files.
    let program_active = !live.canary_op.is_empty() && live.specs.get(&live.canary_op).is_some();

    let status = if failed.is_empty() {
        "ready"
    } else {
        "not_ready"
    };
    // Anonymous probes get the status alone (audit F12): check names, the
    // tenants held closed and the credential labels are the deployment
    // owner's. The HTTP status code still tells a load balancer.
    if !owner {
        return ordo_api::error::status_report(failed.is_empty(), json!({ "status": status }));
    }
    ordo_api::error::status_report(
        failed.is_empty(),
        json!({
            "status": status,
            "program_active": program_active,
            "failed": failed,
            "checks": checks,
            // 17aa. A fact about which tenants this deployment is holding
            // closed, in `program_active`'s category rather than in
            // `checks`: it does not gate readiness, and an empty array is
            // the ordinary answer. Each entry carries the space and what
            // reconciliation found, which is what an operator needs to
            // decide between recording a restore and restoring the
            // missing suffix.
            "quarantined": held
                .iter()
                .map(|(space, reason)| json!({ "space": space, "reason": reason }))
                .collect::<Vec<_>>(),
            // §9.7 C-M3: the labels this node's tenant credential provider
            // holds, names only and never a value, so a fleet-wide move can
            // check every node's labels before it copies anything. `null`
            // for a process assembled without `runtime::assemble`; an empty
            // array for an assembled process with no provider configured,
            // which are two different facts and are kept distinguishable
            // rather than both reading as "no labels".
            "credential_labels": state.credential_labels,
            // Spaces served from their last good member list because their
            // database could not be read for a passing reason, beside the
            // unread list (owner, 2026-10-27).
            "kept_stale": state
                .access_kept_stale
                .as_ref()
                .map(|f| f())
                .unwrap_or_default()
                .iter()
                .map(|(space, note)| json!({ "space": space, "note": note }))
                .collect::<Vec<_>>(),
        }),
    )
}

/// Per-dependency detail — **access-controlled**, since it names the
/// deployment's KeyProvider and database.
async fn health_deep(State(state): State<PlatformState>, headers: HeaderMap) -> Response {
    let Some(principal) = authorize(&state, &headers, HEALTH_DEEP_OP).await else {
        return refusal(&state, &headers, &state.space, HEALTH_DEEP_OP);
    };
    let live = state.deployment.current();
    let db_ok = state.pool.rows("SELECT 1").await.is_ok();
    let epoch = state
        .keys
        .current_epoch(&ordo_crypto::SpaceId(state.space.clone()))
        .ok();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let security = security_stream(&state, now);
    // A drain that has been failing for longer than its grace, or one that
    // was never wired at all, colours the deployment degraded — here and
    // NOT on `/health/ready`. See `crate::secdrain`'s module doc: pulling a
    // node that is serving correct answers out of rotation because its
    // audit writer is stuck converts a logging fault into an outage, and
    // when the cause is the shared database it does so fleet-wide.
    let audit_ok = state.security.as_ref().is_some_and(|h| !h.degraded(now));
    // Owner, 2026-10-21: a stuck tenant setup is a person who cannot use
    // the app, so it colours the deployment degraded too. `/health/ready`
    // stays untouched for the same reason as the drain above.
    let tenant_setup = crate::tenants::setup_report(&state.pool, now).await;
    let setup_ok = tenant_setup["stuck"]
        .as_array()
        .is_none_or(|s| s.is_empty());
    (
        StatusCode::OK,
        Json(json!({
            "status": if db_ok && epoch.is_some() && audit_ok && setup_ok { "ok" } else { "degraded" },
            "security_stream": security,
            // R3. `null` is a process with no supervisor — the dev
            // embedding and the conformance harness — and it is a different
            // fact from "every worker is stopped", which is what an empty
            // object would say.
            "workers": state.supervisor.as_ref().map(|s| s.health()),
            // Leadership and the orphaned listener cursors (§3.5, §3.7),
            // `null` on the same terms as `workers`.
            "supervisor": state.supervisor.as_ref().map(|s| s.overview()),
            "asked_by": principal,
            "space": state.space,
            // P3: how many people hold a tenant of their own, `null` when
            // the registry cannot be read.
            "own_tenants": crate::self_serve::count_own_tenants(&state.pool).await,
            // The self-serve mode and its two limits (owner ruling
            // 2026-10-05). `null` when the settings cannot be read.
            "self_serve": crate::self_serve::readback(&state.pool).await,
            // Owner, 2026-10-21: own tenants being set up, and each one
            // reported stuck. `null` when the registry cannot be read.
            "tenant_setup": tenant_setup,
            // How long sessions last: the deployment's values and limits,
            // the app developer's layer, and what is in force where a tenant
            // set nothing. A tenant's own is read with
            // `std.space.sessionPolicy`. `null` with no sign-in.
            "session_policy": match state.auth.as_ref().and_then(|a| a.policy.clone()) {
                Some(p) => p.readback().await,
                None => Value::Null,
            },
            "database": {
                "reachable": db_ok,
                // Envelope-tier: what kind of dependency and where, never a
                // credential.
                "driver": "tokio-postgres (simple/text protocol)",
            },
            // When a sealed value last opened under each key epoch, and
            // the first failure since (owner, 2026-10-23). Read from the
            // metered provider's answers, kept in memory and flushed to the
            // control database, so it survives a restart.
            "sealed_decrypts": state.metrics.decrypts().report(),
            // G16: what started this deployment: the console's revision,
            // who applied it, the binary and the settings' fingerprint.
            // `null` for a process that never recorded one.
            "boot": crate::boot_record::current_json(),
            "key_provider": {
                // The concrete provider a deployment selected is exactly
                // what F0 wants disclosed to an operator and to nobody else.
                "provider": provider_name(state.keys.as_ref()),
                "reachable": epoch.is_some(),
                "current_epoch": epoch,
            },
            "registry": {
                "version": live.version,
                "ops": live.manifests.len(),
            },
            // 5f: whether the high-cardinality families carry a `space`
            // label. Published because a lock-wait series with no `space`
            // on it has two readings — "per-space is off" and "one space
            // wrote" — and an operator diagnosing contention must not have
            // to guess which.
            "metrics": {
                "per_space_series": state.metrics.per_space_series(),
            },
            // `null` rather than an empty object when the deployment was
            // pinned rather than loaded: "the gate did not run here" and
            // "the gate ran and found nothing" must not render the same.
            "shape_verification": live.report.as_deref().map(|r| json!({
                "summary": r.summary(),
                "verified": r.verified,
                "no_result_shape": r.no_result_shape,
                "undescribable": r.undescribable.iter().map(|u| json!({
                    "op": u.op, "reason": u.reason,
                })).collect::<Vec<_>>(),
                // Observed by the gate, refused by the deploy path. A
                // deployment that is serving with drift got there by being
                // booted directly rather than through `deploy`, and this is
                // where an operator finds out.
                "live_schema_drift": r.drift.iter().map(|d| json!({
                    "table": d.table, "column": d.column, "detail": d.detail,
                })).collect::<Vec<_>>(),
            })),
        })),
    )
        .into_response()
}

/// The security-event drain, as `/health/deep` reports it.
///
/// Every field answers a question an operator actually asks when the audit
/// record matters: is anything being written, is anything being lost, how
/// long has it been broken, and is retention reclaiming or is the stream
/// growing forever because a key is unset.
///
/// `wired: false` is a first-class answer and not an absence. A deployment
/// with no drainer accumulates security events in process memory and loses
/// every one of them on restart — the events lost being exactly the ones a
/// review would want — so the surface says it in a field rather than by
/// having nothing to say.
fn security_stream(state: &PlatformState, now: u64) -> Value {
    let Some(h) = state.security.as_ref() else {
        return json!({
            "wired": false,
            "detail": "no drainer is running: security events accumulate in this \
                       process's bounded outbox, reach no durable storage, and are \
                       lost on restart",
        });
    };
    json!({
        "wired": true,
        "appended": h.appended(),
        "append_failures": h.failures(),
        "pruned": h.pruned(),
        "failing_for_secs": h.failing_for(now),
        "degraded": h.degraded(now),
        "degraded_after_secs": crate::secdrain::DrainHealth::DEGRADED_SECS,
        "last_error": h.last_error(),
        "retention": h.retention_note(),
        // What a verifier sets as `ORDO_SECURITY_CHECKPOINT_KEY_ID` and
        // `ORDO_SECURITY_CHECKPOINT_PUBKEY`: the set key, or the one made at
        // first boot. Public by design; it cannot sign.
        "checkpoint_key": h.checkpoint_key().map(|(key_id, public_key, source)| json!({
            "key_id": key_id,
            "public_key": public_key,
            "source": source,
        })),
    })
}

fn provider_name(keys: &dyn ordo_crypto::KeyProvider) -> String {
    // The type name is the honest answer and needs no registry of names to
    // drift from the providers themselves.
    let full = std::any::type_name_of_val(keys);
    full.rsplit("::").next().unwrap_or(full).to_string()
}

// ── metrics ──────────────────────────────────────────────────────────────

/// Prometheus text exposition, access-controlled: the series carry
/// principal ids and op names across every space this process served.
async fn metrics_route(State(state): State<PlatformState>, headers: HeaderMap) -> Response {
    if authorize(&state, &headers, METRICS_OP).await.is_none() {
        return refusal(&state, &headers, &state.space, METRICS_OP);
    }
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        state.metrics.render_prometheus(),
    )
        .into_response()
}

/// 7b's operator surface: the mandatory deny-volume rule, evaluated now.
///
/// Text, not JSON, and deliberately: what an operator needs from a
/// mandatory alert is the rule, whether their deployment actually set it,
/// what this process can and cannot see, and who is over the line. Each
/// request is one observation of the counters, so a monitoring system that
/// polls this endpoint on a tick gets the sliding window it expects.
///
/// **What this endpoint is not.** It does not page anyone, it does not
/// aggregate across replicas, and it forgets everything on restart. The
/// body says so in its own header lines, because an alert surface that
/// looks complete and is not is worse than none — see
/// `ordo_secevent::alert` for the full list of what a deployment still has
/// to supply.
async fn alerts_route(State(state): State<PlatformState>, headers: HeaderMap) -> Response {
    if authorize(&state, &headers, ALERTS_OP).await.is_none() {
        return refusal(&state, &headers, &state.space, ALERTS_OP);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=utf-8")],
        state.metrics.render_alerts(now),
    )
        .into_response()
}

// ── the space's own security record (7d(b)) ───────────────────────────────

/// How many entries one page of `/security/events` returns at most.
///
/// A page rather than a stream, because a space's record grows without bound
/// and an owner opening a dashboard must not pull a year of denials into one
/// response. `from` is a **full-stream index**, not an offset into what
/// happens to be retained, so a reader paging forward is not silently moved
/// by a prune that runs between two of their requests.
pub const SECEVENTS_PAGE_MAX: usize = 200;

/// `GET /security/events?space=&from=&limit=` — 7d(b), the owner's view of
/// their own space's security record.
///
/// # Who may read it
///
/// The caller, checked against **the space named in the query**
/// ([`authorize_in`]), on [`SECEVENTS_OP`]. This is the one route on this
/// router that is not a deployment-wide disclosure, and getting that gate
/// wrong is the way this feature ships looking done: an operator-only
/// version of it would return data, pass a naive test, and still leave
/// §6.1's members-only routing pointless because the members it routes to
/// could not read the result.
///
/// # What it refuses, and why each refusal is the same shape
///
/// A space that does not exist, a space the caller has no membership in, and
/// a space whose id is not well formed all answer `access_denied`. That is
/// the door's own rule — a distinguishable refusal is an oracle a caller maps
/// one attempt at a time — and it matters more here than usual, because the
/// thing being probed is *which spaces exist*.
///
/// **`$deployment` is refused by name.** The deployment scope's storage key
/// is not a valid space id, so it cannot arrive through
/// [`ordo_api::well_formed_space`] — but it is checked explicitly anyway,
/// because if it ever did arrive, the caller would be handed the one stream
/// §6.1 keeps from tenants on purpose: the cross-space sweep, where a
/// principal probing a hundred spaces is visible. A guard that depends on
/// another function's alphabet for a disclosure of that size should be its
/// own line.
async fn secevents_route(
    State(state): State<PlatformState>,
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(space) = query.get("space").filter(|s| !s.is_empty()) else {
        return secevents_bad_request("`space` is required");
    };
    // Before the access check, because "that is not a space id" is not a
    // fact about this caller's permissions and must not consume one.
    if space == ordo_secevent::DEPLOYMENT_KEY || !ordo_api::well_formed_space(space) {
        return denied();
    }
    if authorize_in(&state, &headers, space, SECEVENTS_OP)
        .await
        .is_none()
    {
        return refusal(&state, &headers, space, SECEVENTS_OP);
    }
    let from = match query.get("from").map(|s| s.parse::<usize>()) {
        None => 0,
        Some(Ok(n)) => n,
        Some(Err(_)) => return secevents_bad_request("`from` must be a whole number"),
    };
    let limit = match query.get("limit").map(|s| s.parse::<usize>()) {
        None => SECEVENTS_PAGE_MAX,
        Some(Ok(n)) if n >= 1 => n.min(SECEVENTS_PAGE_MAX),
        Some(_) => return secevents_bad_request("`limit` must be 1 or more"),
    };

    // **The space's own database, not the control database.** S4 routes each
    // space's entries to the database it is attached to; reading a routed
    // tenant's stream from `state.pool` finds nothing, and for this surface
    // "nothing" reads as "your space has recorded no denials". A clean bill
    // of health for a stream nobody looked at is the worst answer here, so an
    // unwired resolver says so instead of answering.
    let Some(pools) = state.pools.as_ref() else {
        return ApiError::new(
            ErrorCode::SpaceUnavailable,
            "this process has no space resolver, so a space's own security \
             record cannot be located. Nothing was read",
        )
        .into_response();
    };
    let pool = match pools
        .resolve(&ordo_crypto::SpaceId(space.to_string()))
        .await
    {
        Ok(pool) => pool,
        Err(e) => {
            // The reason is the operator's, not the caller's: a resolve
            // failure names a host and often a user.
            eprintln!("security/events: `{space}` could not be resolved: {e}");
            return ApiError::new(
                ErrorCode::SpaceUnavailable,
                "this space's own database could not be reached, so its \
                 security record could not be read. Nothing was read",
            )
            .into_response();
        }
    };

    let scope = ordo_secevent::Scope::Space(ordo_commandlog::SpaceId(space.to_string()));
    let store = ordo_exec::secevent::PgSecurityStore::new(pool, tokio::runtime::Handle::current());
    let base = match store.try_base_index(&scope).await {
        Ok(base) => base,
        Err(e) => return secevents_unreadable(&e.to_string()),
    };
    // `from` is a full-stream index and the store reads offsets into what is
    // retained, so a request for a position already pruned reads from the
    // start of the tail rather than silently skipping into the middle of it.
    let offset = from.saturating_sub(base);
    let entries = match store.try_entries(&scope, offset, limit).await {
        Ok(entries) => entries,
        Err(e) => return secevents_unreadable(&e.to_string()),
    };
    let next = base + offset + entries.len();
    let rows: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "index": base + offset + i,
                // The canonical rendering, the same string a command group
                // is attributed with, so an owner can join a denial to the
                // write it was about without a translation table.
                "subject": e.entry.subject.render(),
                "subjectKind": e.entry.subject.kind_str(),
                "op": e.entry.op,
                "kind": e.entry.kind.label(),
                "firstAt": e.entry.first_at.0,
                "lastAt": e.entry.last_at.0,
                // A collapsed window is one entry covering `count`
                // occurrences, so a reader who assumed one row per event
                // would under-report a scanner by orders of magnitude.
                "count": e.entry.count,
                // A bounded sample, not the whole set: the entry carries
                // at most `CORRELATION_SAMPLE` of them by design.
                "correlations": e.entry.correlations.iter().map(|c| c.0.clone())
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "space": space,
            // What a prune has taken. Reported on every page for the reason
            // `x-ordo-verifiable-from` exists on the command log: a reader
            // must be able to tell "there were no denials" from "the denials
            // there were have aged out".
            "verifiableFrom": base,
            "from": base + offset,
            "next": next,
            "events": rows,
        })),
    )
        .into_response()
}

fn secevents_bad_request(message: &str) -> Response {
    ApiError::new(ErrorCode::InvalidParam, message).into_response()
}

/// A read that failed is never an empty page. Same rule the stream's
/// verifier follows: "I could not check" and "there is nothing" must not be
/// one value, and here they would differ only in whether an owner thinks
/// their space is quiet.
fn secevents_unreadable(detail: &str) -> Response {
    eprintln!("security/events: the stream could not be read: {detail}");
    ApiError::new(
        ErrorCode::SpaceUnavailable,
        "this space's security record could not be read. Nothing was read, \
         which is not the same as nothing having happened",
    )
    .into_response()
}

// ── usage ────────────────────────────────────────────────────────────────

/// `GET /usage?space=&from=&to=`, 17n M3 — the wire twin of `ordo usage`.
/// Both render through `ordo_exec::usage::render_between`, so a caller
/// reading either surface for the same closed period reads the same bytes,
/// gaps (18d R-O5) included.
///
/// Access-controlled like [`metrics_route`]: [`authorize`] checks the
/// caller against **the deployment's own space**, never the `space` query
/// parameter — `space=` names *whose* usage to read, which is a question
/// this operator surface answers about every tenant the process serves,
/// the same shape `/metrics` already has for principals and op names.
async fn usage_route(
    State(state): State<PlatformState>,
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if authorize(&state, &headers, USAGE_OP).await.is_none() {
        return refusal(&state, &headers, &state.space, USAGE_OP);
    }
    let Some(space) = query.get("space").filter(|s| !s.is_empty()) else {
        return usage_bad_request("`space` is required");
    };
    let Some(from) = query.get("from").and_then(|s| s.parse::<u64>().ok()) else {
        return usage_bad_request("`from` must be a whole number of seconds");
    };
    let Some(to) = query.get("to").and_then(|s| s.parse::<u64>().ok()) else {
        return usage_bad_request("`to` must be a whole number of seconds");
    };
    let out = match ordo_exec::usage::render_between(&state.pool, space, from, to).await {
        Ok(out) => out,
        // A full pool is the one store failure a caller can act on
        // (reported fix 25z).
        Err(e) if e.is_busy() => return ordo_api::ApiError::server_busy().into_response(),
        Err(e) => return ApiError::new(ErrorCode::Internal, e.to_string()).into_response(),
    };
    (
        StatusCode::OK,
        [("content-type", "application/x-ndjson")],
        out,
    )
        .into_response()
}

fn usage_bad_request(message: &str) -> Response {
    ApiError::new(ErrorCode::InvalidParam, message).into_response()
}

// ── catalog ──────────────────────────────────────────────────────────────

/// The catalog, **scoped to the asker**: the same check the door runs, per
/// op, omitting rather than marking — a listed-but-forbidden op still tells
/// the asker it exists.
///
/// Two renderings of one source: `application/json` for SDK generators,
/// `text/plain` for an agent's context window. No token means no ops, and
/// that is an empty catalog rather than a 403: a refusal would itself be an
/// answer about the deployment.
async fn catalog_route(
    State(state): State<PlatformState>,
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let token = bearer(&headers).unwrap_or_default();
    let space = space_of(&state, &headers, &query);
    // A dead token lists nothing, as no token lists nothing: a withdrawn
    // delegation or a closed share grant must not go on reading the map of
    // what it could once call.
    let live = token.is_empty() || token_live(&state, &token).await;
    // The catalog is a rendering of the version that is serving, plus the
    // platform ops this build serves (8r). It republishes with the swap
    // because the program half reads through the same pointer the swap
    // moves — `implementation.md`'s "republishes atomically with the swap"
    // as a consequence of where the field lives — and the platform half
    // does not move, which is what it means for it not to be part of a
    // version.
    let allowed: Vec<Value> = state
        .deployment
        .manifests()
        .iter()
        .filter(|(name, _)| {
            live && matches!(
                state.access.check(&token, &space, name),
                AccessDecision::Allow { .. }
            )
        })
        .map(|(_, m)| m.clone())
        .collect();
    let catalog = Value::Array(allowed);

    let wants_prose = query.get("format").is_some_and(|f| f == "prose")
        || headers
            .get("accept")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/plain") && !a.contains("application/json"));
    if wants_prose {
        (
            StatusCode::OK,
            [("content-type", "text/plain; charset=utf-8")],
            crate::catalog_prose(&catalog),
        )
            .into_response()
    } else {
        (StatusCode::OK, Json(catalog)).into_response()
    }
}

// ── interface descriptor ─────────────────────────────────────────────────

/// The **Interface Descriptor** (B1), scoped to the asker exactly as the
/// catalog is — same filter, one line apart, so the two can never describe
/// different op sets.
///
/// It sits beside `/catalog` rather than inside it because the two answer
/// different questions of the same facts: the catalog is the **consent**
/// document (what does this op touch, what guards it, what does it cost),
/// and this is the **contract** document a code generator and a renderer
/// consume. A caller reading only one of them still gets a complete answer
/// to the question it asked.
///
/// No token means an empty descriptor — no ops, and therefore no entities —
/// rather than a 403, for `catalog_route`'s reason: a refusal is itself an
/// answer about the deployment. The document is still well-formed, so a
/// generator run without credentials fails on "nothing to generate" rather
/// than on a parse error.
///
/// The program half republishes with the swap, because it reads through the
/// same `Deployment` pointer the swap moves.
async fn interface_route(
    State(state): State<PlatformState>,
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let token = bearer(&headers).unwrap_or_default();
    let space = space_of(&state, &headers, &query);
    // The catalog's rule: a dead token describes nothing.
    let live = token.is_empty() || token_live(&state, &token).await;
    let loaded = state.deployment.current();
    let allowed: Vec<Value> = state
        .deployment
        .manifests()
        .into_iter()
        .filter(|(name, _)| {
            live && matches!(
                state.access.check(&token, &space, name),
                AccessDecision::Allow { .. }
            )
        })
        .map(|(_, m)| m)
        .collect();
    let doc = ordo_plan::interface::interface_document(
        &json!(loaded.fixture.schema.domain),
        &ordo_plan::artifact::entity_artifacts(&loaded.fixture.schema),
        &allowed,
    );
    (StatusCode::OK, Json(doc)).into_response()
}

// ── shared ───────────────────────────────────────────────────────────────

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

/// The space a platform request names. Same convention as the door's
/// `X-Ordo-Space`; the deployment's own space is the default, because a
/// health probe naming no space is asking about this deployment.
fn space_of(
    state: &PlatformState,
    headers: &HeaderMap,
    query: &std::collections::HashMap<String, String>,
) -> String {
    headers
        .get("x-ordo-space")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| query.get("$space").cloned())
        .unwrap_or_else(|| state.space.clone())
}

/// Run the deployment's own access check for a reserved platform op.
/// Returns the principal it resolved, or `None`: one answer for a missing
/// token, a bad token, and a token without the grant.
///
/// Checked against **the deployment's own space** (`state.space`), never
/// [`space_of`]. Every reserved platform op is a deployment-wide surface:
/// `/metrics` names principals and op names across every space this process
/// served, `/security/alerts` names which principals look like they are
/// probing, and `/health/deep` names the deployment's own KeyProvider and
/// database. So the question they answer has exactly one space, the
/// deployment's, and it is not the caller's to name. Resolving it from
/// `space_of` would let the owner of any one space put that space in
/// `x-ordo-space` and receive the disclosure meant for the operator: a
/// cross-tenant read with a header standing in for a key. `space_of`'s other
/// two callers (`catalog_route`, `interface_route`) answer an ordinary,
/// per-space question (which ops can this caller reach in the space they
/// named) and are right to take the caller's word for which space that is.
async fn authorize(state: &PlatformState, headers: &HeaderMap, op: &str) -> Option<String> {
    authorize_in(state, headers, &state.space, op).await
}

/// [`authorize`], against a space the **caller named** rather than the
/// deployment's own.
///
/// Every other route on this router asks the deployment's own space, because
/// every other route is an operator surface: `/metrics` hands over every
/// series, `/usage?space=` answers about every tenant the process serves.
/// Reusing that gate for `/security/events` would have made an owner-facing
/// record an operator-only one, and 7d(b) would have read as built while
/// remaining unbuilt — the route would exist, return data to operators, and
/// still leave no owner able to see their own space's denials.
///
/// So this binds the space under test. A tenant owner passes for their own
/// space and fails for every other, which is the property, and it is the
/// ordinary access model answering rather than a second notion of who counts
/// as an owner.
///
/// **The token gate runs first**, as it does at the op door. A token whose
/// delegation was withdrawn, or whose share grant closed, reaches none of
/// these surfaces: `check` alone answers from the token and the membership
/// table, and cannot see a revocation (found 2026-10-08, increment 2 of
/// `plan-delegation-surface.md`).
async fn authorize_in(
    state: &PlatformState,
    headers: &HeaderMap,
    space: &str,
    op: &str,
) -> Option<String> {
    let Some(token) = bearer(headers) else {
        state
            .access
            .record_refusal(None, Some(space), op, ordo_api::DoorRefusal::TokenMissing);
        return None;
    };
    if !token_live(state, &token).await {
        return None;
    }
    match state.access.check(&token, space, op) {
        AccessDecision::Allow { principal, .. } => Some(principal),
        AccessDecision::Deny => None,
    }
}

/// The answer a platform surface gives a caller [`authorize_in`] refused:
/// the door's `presence_required` when the realm says the same caller would
/// pass with a fresh touch (a deployment person, `plan-identity-service.md`
/// 4.3), otherwise the one [`denied`]. The op door asks the same question
/// of the same checker.
fn refusal(state: &PlatformState, headers: &HeaderMap, space: &str, op: &str) -> Response {
    match bearer(headers) {
        Some(token) if state.access.presence_required(&token, space, op) => {
            ApiError::presence_required(op).into_response()
        }
        _ => denied(),
    }
}

/// Does the token clear the door's token gate: no withdrawn delegation and
/// no closed share grant? The platform surfaces answer a dead token as they
/// answer no token, so the gate's own class is not needed here.
async fn token_live(state: &PlatformState, token: &str) -> bool {
    matches!(state.access.token_gate(token).await, TokenGate::Pass)
}

/// One refusal shape for every platform-surface denial, matching the door's
/// rule: no detail, and nothing that distinguishes "no such surface" from
/// "not for you".
fn denied() -> Response {
    ApiError::new(ErrorCode::AccessDenied, "not permitted").into_response()
}
