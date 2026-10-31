//! §10 T4 of `plan-space-routing.md`: the routing switch's own done-when.
//!
//! T0 through T3 wired a resolver, a door that reads each tenant's members
//! from its own database, one resolver shared by the door, the cache, the
//! drain and the exit ops, and a credential provider that reads a mounted
//! directory. Every one of those increments was proved with the switch
//! left off: `ORDO_PG_ROUTING` unset, `$spacedb` empty, every space still
//! on the one database it was on before any of this existed. That is the
//! ordering the plan states out loud — "the wiring lands and is proved not
//! to change anything before the switch that changes something exists" —
//! and it means none of T0 through T3's tests can be the proof that
//! turning the switch on and attaching a tenant actually moves that
//! tenant's data. This file is that proof, in the plan's own words: **one
//! tenant provisioned onto a second real database has its writes, reads,
//! security events, idempotency records and `?live` notifications all
//! land there, while a tenant on the default is untouched.**
//!
//! # Why one file proves five subsystems at once
//!
//! `one_resolver.rs` pins T2's construction property (one resolver, four
//! consumers) as a source scan, because before T4 there is no attached
//! database for a behavioural test to observe — `$spacedb` is empty on
//! every deployment that can be booted. That file's own doc says what
//! should replace it once an attach exists: "attach a tenant to a second
//! database, exercise an op, a replayed idempotency key, a security
//! event... and assert all four landed in the target rather than the
//! control database." This is that test, with `?live` added because T4's
//! entry in §10 names it as the fifth surface the switch has to move.
//!
//! `replay_routing.rs`, `secdrain_routing.rs` and `invalidation_routing.rs`
//! already pin that `PgReplayCache`, `SecurityDrain` and
//! `PgInvalidationBus` each route correctly **in isolation**, built and
//! driven by hand. What none of them do is go through a door: a real
//! token, a real guard decision, a real write dispatched by `POST /op/…`.
//! A resolver can be wired correctly and a door can still read the wrong
//! engine, or an op handler can still reach a pool of its own — and every
//! one of those bugs is invisible to a test that calls the routed
//! component directly instead of asking the deployment to serve a
//! request. This file drives the actual HTTP surface, the way T4's own
//! done-when is stated: an *end-to-end* test.
//!
//! # Two things this test insists on beyond its siblings
//!
//! **A real [`AttachedPools`]**, never a hand-written [`PoolResolver`]
//! like `spaces.rs`'s own `TwinApart`. A hand-written resolver can only
//! prove that the code downstream of a resolver behaves given a correct
//! answer; it says nothing about whether `AttachedPools` itself — the
//! thing T4 actually wires in — produces one.
//!
//! **The credential resolved through [`ordo_exec::creds::Dir`]**, T3's
//! provider, reading a real temporary directory, rather than through
//! [`ordo_exec::route::Installed`] the way `replay_routing.rs` and its
//! neighbours do for their own narrower purposes. `Installed` is a map an
//! operator never touches; `Dir` is the thing §9.1 R-C3 describes an
//! operator actually mounting. Exercising `Installed` here would leave the
//! label-to-file read, the mode check and the empty-file-is-passwordless
//! reading — the whole of T3 — off the path this test claims to cover.
//! The conformance Postgres in this environment is passwordless over its
//! local socket, so the file is empty, the same shape
//! `an_empty_credential_file_is_passwordless` (`ordo-exec/src/creds.rs`)
//! pins, written at `0o600` because `Dir::open` refuses anything looser.
//!
//! # What each assertion actually checks, and what would pass without it
//!
//! Five subsystems, each asserted twice: that the twin's fact is **in**
//! the target, and that it is **not in** the control database. The second
//! half is the one that matters. A resolver that quietly fell back to the
//! default on every read and write would pass every positive assertion
//! below and be indistinguishable from a correct one until an operator
//! went looking at the wrong tenant's database — which is exactly the
//! failure `directory.rs`'s own doc calls "unobservable" before routing,
//! and the reason this file exists.
//!
//! - **Writes.** `POST /op/addTask` in the twin's space, then a direct
//!   count against `tasks` in both databases. Without the negative half a
//!   resolver that always answers with the control pool would still show
//!   the row (in the wrong place) and the positive assertion alone would
//!   pass.
//! - **Reads.** `GET /op/openTasks` in the twin's space finds the row; the
//!   same op in the deployment's own space does not. Since the row is
//!   proved (by direct SQL) to exist nowhere but the target, a read that
//!   found it can only have reached the target — which is what makes the
//!   read assertion meaningful without a second, symmetrical SQL check of
//!   its own.
//! - **Idempotency records.** The same write repeated under the same
//!   `Idempotency-Key` is answered without a second row appearing, and the
//!   `$idempotency` entry itself is counted in both databases. Without the
//!   row-count check, a cache that replayed correctly but wrote its record
//!   to the control database would pass silently — and `plan-space-routing.md`
//!   §3 is explicit that an idempotency record is tenant data under the
//!   tenant's own custody, not a detail the control database is entitled to.
//! - **Security events.** A second member of the twin, role `viewer`,
//!   attempts a write and is refused, and the denial is counted in
//!   `$secevents` by scope in both databases. The refusal has to be a
//!   **door**-level one: `access_guard`, an entity guard's own refusal, is
//!   raised by the executor and never reaches `ordo_secevent::Recorder` at
//!   all — a first version of this test denied a row guard instead and
//!   found zero rows everywhere, not merely in the wrong database, because
//!   nothing was ever recorded. `engine_from_members` never grants a
//!   write op to `Role::Viewer`, so `AccessEngine::check` itself denies
//!   it, which is what `IdentityAccess::check` actually records denials
//!   from. `SecurityDrain::shutdown` is called before either count,
//!   because a fresh denial sits in an open collapse window until a tick
//!   closes it or a shutdown force-closes it — checking before that would
//!   read zero rows regardless of whether routing is correct, and the
//!   assertion would pass for the wrong reason.
//! - **`?live` notifications.** A subscriber on the twin's space, through
//!   the deployment's own routed bus, hears an `invalidate` frame after the
//!   write. The negative half is not "a subscriber on the deployment's own
//!   space hears nothing" — [`ordo_api::InvalidationEvent`] carries only an
//!   op name, never a space, precisely because the signal is a pure
//!   staleness push under "refetch-through-scope semantics" (`ordo-api/src/
//!   spec.rs`'s own doc), so every subscriber to `openTasks` on one shared
//!   bus is told regardless of which space wrote. What routing changes is
//!   not who is told; it is whether the write is heard **at all**. So the
//!   negative half is `invalidation_routing.rs`'s own comparison, driven
//!   here through the door instead of the bus's bare API: a second,
//!   unrouted bus that only ever listens to the control database's
//!   `LISTEN` session hears nothing from the twin's write, which is what a
//!   bus that never dials the target sounds like in production.
//!
//! A closing request against the deployment's own space, after every twin
//! assertion above, stands in for "and a tenant on the default is
//! untouched" the way the plan states it: not merely that the twin's rows
//! are absent from the control database, but that the control database
//! keeps answering ordinary requests throughout.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_host::provision::HostAllowlist;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request};
use futures::StreamExt;
use ordo_api::InvalidationBus;
use ordo_conformance::{Fixture, Pg};
use ordo_exec::creds::Dir;
use ordo_exec::route::{AttachedPools, PoolResolver};
use ordo_exec::spacedb::{DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::deploy::Deployment;
use ordo_host::provision::{provision_space_on, FoundingOwner, Target};
use ordo_host::secdrain::{DrainConfig, SecurityDrain};
use ordo_host::spaces::Spaces;
use ordo_host::{FixtureExecutor, IdentityAccess};
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::Value;
use tower::util::ServiceExt;

/// The deployment's own space: the tenant that must stay untouched.
const HERE: &str = ordo_host::DEFAULT_SPACE;
/// A member the shipped seed gives `HERE`, a projectMember of `proj_1`
/// (`canWrite`) and `proj_3` (read-only), which the `openTasks` reads below
/// use to prove the deployment's own space keeps serving.
const HERE_MEMBER: &str = "mem_1";

/// The tenant this test provisions onto a database of its own.
const TWIN: &str = "spc_twin_t4";
const TWIN_OWNER: &str = "mem_twin_t4_owner";
/// A second member of the twin, role `viewer` — the door-level denial the
/// security-event assertion needs. `engine_from_members`
/// (`ordo-host/src/lib.rs`) never grants a write op to `Role::Viewer`, so a
/// viewer attempting one is refused by [`ordo_identity::AccessEngine::check`]
/// itself, which is what `IdentityAccess::check` records denials from. An
/// entity guard's own refusal (`access_guard`) would not do: it is raised by
/// the executor and never reaches the security-event recorder at all — a
/// first version of this test used exactly that and found zero rows
/// everywhere, not merely in the wrong database, because nothing was ever
/// recorded.
const TWIN_VIEWER: &str = "mem_twin_t4_viewer";

/// The label the twin's attachment names and the only entry the
/// credential directory holds.
const CREDENTIAL_LABEL: &str = "t4-target";

const SECRET: &[u8] = b"attached-tenant-e2e-test-secret";
const NOW: u64 = 1_758_000_000;

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    }
}

fn drop_database(name: &str) {
    let _ = Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
    .exec(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"));
}

fn on(db: &str, credential: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.into(),
        credential: credential.into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 0,
        attached_by: "test:t4".into(),
    }
}

/// A directory holding exactly [`CREDENTIAL_LABEL`] at `0o600`, carrying
/// whatever password this environment's Postgres actually wants.
///
/// **The contents come from `ORDO_PG_PASSWORD`, and that is the fix for a
/// real failure rather than a precaution.** This wrote an empty file
/// originally, on the true observation that the conformance Postgres on
/// a developer machine authenticates over a local socket with no
/// password. On CI it is a service container over TCP with
/// `POSTGRES_PASSWORD` set, so provisioning the twin's database failed
/// with `invalid configuration: password missing` and the whole suite
/// went red.
///
/// It is worth naming what that was: an assumption about the
/// **environment** baked into a test, which passes everywhere it is
/// usually run and fails in the one place that differs. It was invisible
/// for three days while a billing outage kept the suite on one laptop,
/// and the first CI run after the self-hosted runner landed found it in
/// four minutes.
///
/// Empty when the variable is unset, which keeps the local socket path
/// working and keeps `an_empty_credential_file_is_passwordless`
/// (`ordo-exec/src/creds.rs`) the thing this leans on there. Removed on
/// drop, so a failed run leaves no credential material on disk.
struct CredDir(PathBuf);

impl CredDir {
    fn open(label: &str) -> CredDir {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "ordo-host-t4-creds-{}-{}",
            std::process::id(),
            label
        ));
        std::fs::create_dir_all(&p).expect("credential directory");
        let file = p.join(label);
        // `PoolConfig::from_env`'s own variable, so the credential this
        // test installs and the one the pool would use are read from one
        // place and cannot drift apart.
        let password = std::env::var("ORDO_PG_PASSWORD").unwrap_or_default();
        std::fs::write(&file, &password).expect("the credential file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("0600");
        }
        CredDir(p)
    }
}

impl Drop for CredDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The twin's own fixture, beyond the founding member `provision_space_on`
/// writes: a project `TWIN_OWNER` can write to, for the write assertion,
/// and a second, viewer-role member, for the security-event assertion's
/// denial. `ON CONFLICT DO NOTHING` because this database is dropped and
/// recreated at the top of the harness, but a panic partway through a
/// previous run can still leave it applied.
async fn seed_twin(target: &Pool) {
    target
        .rows(&format!(
            "INSERT INTO \"members\" (\"$space\", \"id\", \"principal\", \"displayName\", \
             \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") \
             VALUES ('{TWIN}', '{TWIN_VIEWER}', '{TWIN_VIEWER}', 'The Viewer', \
                     'principal', 1, 'viewer', 'active', DATE '2026-01-01') \
             ON CONFLICT DO NOTHING"
        ))
        .await
        .expect("the twin's viewer member");
    target
        .rows(&format!(
            "INSERT INTO \"projects\" (\"$space\", \"id\", \"name\", \"owner\") \
             VALUES ('{TWIN}', 'proj_twint4', 'Twin Project', '{TWIN_OWNER}') \
             ON CONFLICT DO NOTHING"
        ))
        .await
        .expect("the twin's project");
    target
        .rows(&format!(
            "INSERT INTO \"projectMembers\" (\"$space\", \"id\", \"project\", \"member\", \
             \"canWrite\") \
             VALUES ('{TWIN}', 'pm_twint4', 'proj_twint4', '{TWIN_OWNER}', true) \
             ON CONFLICT DO NOTHING"
        ))
        .await
        .expect("the twin's project membership");
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(NOW + 3_600),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

async fn get_in(app: &axum::Router, path: &str, space: &str, who: &str) -> (u16, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", space)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post_in(
    app: &axum::Router,
    path: &str,
    space: &str,
    who: &str,
    idempotency_key: Option<&str>,
    body: Value,
) -> (u16, Value) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", space)
        .header("content-type", "application/json");
    if let Some(key) = idempotency_key {
        req = req.header("idempotency-key", key);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A live subscription over `openTasks`, proven open (200, `text/event-
/// stream`) before it is handed back.
async fn open_live(app: &axum::Router, space: &str, who: &str) -> axum::body::BodyDataStream {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/op/openTasks?live=1")
        .header("authorization", format!("Bearer {}", token(who)))
        .header("x-ordo-space", space)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        200,
        "the live subscription in `{space}` was refused"
    );
    assert_eq!(
        resp.headers()["content-type"],
        "text/event-stream",
        "`{space}`'s subscription did not upgrade to SSE"
    );
    resp.into_body().into_data_stream()
}

/// A row count, for a table this test knows exists in `pool`'s database.
async fn count(pool: &Pool, sql: &str) -> i64 {
    pool.rows(sql)
        .await
        .expect("count")
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next())
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// The same, but honest about a table that may never have been created in
/// `pool`'s database at all — `$idempotency` and `$secevents` are created
/// lazily, on first use, in whichever database a space's records land in.
/// A control database the twin never touched may hold neither table, and
/// that absence is itself the negative assertion rather than a query
/// error to work around.
async fn count_if_table_exists(pool: &Pool, table: &str, filter: &str) -> i64 {
    let exists = pool
        .rows(&format!(
            "SELECT to_regclass('public.\"{table}\"') IS NOT NULL"
        ))
        .await
        .expect("regclass check")
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next())
        .flatten()
        .is_some_and(|s| s == "t" || s == "true");
    if !exists {
        return 0;
    }
    count(
        pool,
        &format!("SELECT count(*) FROM \"{table}\" WHERE {filter}"),
    )
    .await
}

struct Harness {
    app: axum::Router,
    control: Pool,
    target: Pool,
    /// Held so the test can build a second, unrouted bus over the same
    /// control database for the `?live` assertion's negative half. See
    /// that assertion for why "a different space's subscriber" is not it.
    fixture: Arc<Fixture>,
    secdrain: Option<SecurityDrain>,
    _creds: CredDir,
}

/// Assembles the deployment the way `assemble_program` does, which since
/// 2026-10-06 is always: a real [`AttachedPools`] over a real
/// [`Dir`]-backed credential, the door built through
/// [`ordo_host::directory::routed_engine`] (T1), the idempotency cache
/// through [`ordo_host::replay::PgReplayCache::routed`] (S6), the security
/// drain through [`SecurityDrain::start_routed`] (S4) and the invalidation
/// bus through [`ordo_host::invalidation::PgInvalidationBus::connect_routed`]
/// (S7) — the same five call sites `runtime.rs` wires from `spaces.pools()`.
/// Hand-assembled rather than spawned as a binary, on `spaces.rs`'s own and
/// `deployment.rs`'s own precedent: both already reconstruct pieces of
/// `assemble_program` by hand to reach a state a subprocess cannot be
/// driven into quickly (a short drain tick here; the same is true there for
/// a metered fixture and a two-node bus).
async fn harness() -> Harness {
    const CONTROL: &str = "ordo_host_t4_control";
    const TARGET: &str = "ordo_host_t4_target";
    drop_database(TARGET);

    let f = Arc::new(Fixture::load().expect("the market fixture compiles"));
    let pg = Pg::named(CONTROL);
    f.reset(&pg).expect("the control database is seeded");
    let pool = Pool::connect(cfg(CONTROL)).await.expect("control pool");

    let deployment = Deployment::pinned(
        "attached-tenant-e2e",
        f.clone(),
        ordo_exec::Executor {
            pool: pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
    );

    // T3: the real provider, reading a real directory — see the module
    // doc for why `Installed` would leave T3 untested here.
    let creds = CredDir::open(CREDENTIAL_LABEL);
    let dir_creds: Arc<dyn ordo_exec::route::DbCredentials> =
        Arc::new(Dir::open(&creds.0).expect("the credential directory opens"));

    // S5: the twin, provisioned onto a database of its own — custody, the
    // database, the schema and the attachment, through the one entry
    // point production uses.
    let attachment = on(TARGET, CREDENTIAL_LABEL);
    let template = cfg(TARGET);
    let target_spec = Target {
        attachment: &attachment,
        credentials: dir_creds.as_ref(),
        template: &template,
        hosts: &HostAllowlist::of(&PoolConfig::from_env().host),
    };
    let owner = FoundingOwner {
        member_id: TWIN_OWNER,
        principal: TWIN_OWNER,
        display_name: "The twin's owner",
    };
    let provisioned = provision_space_on(
        &pool,
        f.keys.as_ref(),
        &deployment,
        TWIN,
        &owner,
        &target_spec,
        None,
        NOW,
    )
    .await
    .expect("the twin provisions onto its own database");
    assert_eq!(
        provisioned,
        ordo_host::provision::Provisioned::Created {
            space: TWIN.to_string()
        },
        "a freshly dropped target must provision as created, not as a retry \
         finding an existing tenant"
    );

    let target = Pool::connect(cfg(TARGET)).await.expect("target pool");
    seed_twin(&target).await;

    // T4: the switch, on. A real `AttachedPools` — not `spaces.rs`'s own
    // `TwinApart` — dialling through the same `Dir` the attach above used.
    let resolver: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        pool.clone(),
        pool.clone(),
        template.clone(),
        dir_creds.clone(),
    ));
    let witness = ordo_exec::witness::Witness::new(pool.clone());
    let spaces = Spaces::routed(deployment.clone(), resolver.clone(), witness);

    // T1: the door reads the twin's founding member from the twin's own
    // database, not from the control pool's `members` table.
    let directory = ordo_host::directory::routed_engine(&f, &pool, dir_creds.as_ref(), &template)
        .await
        .expect("the directory reads every tenant's own database");
    assert!(
        directory.unread.is_empty(),
        "a target this harness just provisioned must be readable: {:?}",
        directory.unread
    );

    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let recorder = Arc::new(ordo_secevent::Recorder::default());
    let access = Arc::new(IdentityAccess {
        engine: Arc::new(directory.engine),
        key: TokenKey::from_secret(SECRET),
        now: Box::new(|| NOW),
        stream: recorder.clone(),
        metrics: metrics.clone(),
        delegations: None,
        revocations: None,
        shares: None,
        second_step: Default::default(),
        console_home: Default::default(),
        deployment: Default::default(),
    });

    // S4: the same recorder the door denies into, drained on a short tick
    // so the security-event assertion does not wait out the production
    // default (`DEFAULT_TICK`, thirty seconds) — `deployment.rs`'s own
    // harness makes the identical trade for the identical reason.
    let secdrain = SecurityDrain::start_routed(
        recorder,
        pool.clone(),
        resolver.clone(),
        metrics.clone(),
        DrainConfig {
            tick: Duration::from_millis(50),
            ..Default::default()
        },
    )
    .await
    .expect("the security drain starts");

    // S6: the idempotency cache, routed.
    let replay = Arc::new(ordo_host::replay::PgReplayCache::routed(
        resolver.clone(),
        f.keys.clone(),
    ));

    // S7: a listener per database, so `?live=1` means something for the
    // twin as well as for the deployment's own space.
    let bus: Arc<dyn ordo_api::InvalidationBus> = Arc::new(
        ordo_host::invalidation::PgInvalidationBus::connect_routed(
            &f,
            resolver.clone(),
            Some(metrics.clone()),
        )
        .await
        .expect("the routed bus connects"),
    );

    let api = ordo_api::ApiState {
        registry: deployment.handle(),
        executor: Arc::new(FixtureExecutor {
            spaces: spaces.clone(),
            pg: pg.clone(),
            page_size: 50,
        }),
        access: access.clone(),
        quota: Arc::new(ordo_api::NoQuota),
        bus,
        replay,
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: ordo_api::PageKey::from_secret(b"attached-tenant-e2e-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };

    let platform = ordo_host::serve::PlatformState::new(
        access,
        deployment,
        pool.clone(),
        f.keys.clone(),
        ordo_host::deployment_space().expect("the default is a bindable space id"),
        metrics,
    )
    .with_spaces(spaces)
    .with_security_drain(secdrain.health());

    let app = ordo_api::router(api).merge(ordo_host::serve::platform_router(platform));

    Harness {
        app,
        control: pool,
        target,
        fixture: f,
        secdrain: Some(secdrain),
        _creds: creds,
    }
}

/// **T4's own done-when.** A tenant provisioned onto a second real database
/// has its writes, reads, security events, idempotency records and `?live`
/// notifications all land there, while the deployment's own space is
/// untouched. See the module doc for what each assertion checks and what
/// would pass without its negative half.
#[tokio::test(flavor = "multi_thread")]
async fn an_attached_tenant_is_served_from_its_own_cluster() {
    let mut h = harness().await;

    // Subscribed before anything is written, so a frame it receives
    // afterward is provably about the write below and not about catching
    // up on history.
    let mut twin_sse = open_live(&h.app, TWIN, TWIN_OWNER).await;
    let first = tokio::time::timeout(Duration::from_secs(10), twin_sse.next())
        .await
        .expect("the twin's first frame never arrived")
        .expect("the twin's stream ended")
        .expect("body error");
    assert!(
        !String::from_utf8_lossy(&first).contains("invalidate"),
        "the twin's first frame must be the ordinary envelope, not a stale invalidation"
    );

    // The `?live` assertion's negative half: a second bus, never wired
    // into the app, that only ever connects to the control database — see
    // the module doc for why this is the comparison rather than a second
    // subscriber on the deployment's own space.
    let control_only =
        ordo_host::invalidation::PgInvalidationBus::connect(&h.fixture, &h.control.config(), None)
            .await
            .expect("a single-database bus connects");
    let mut control_rx = control_only.subscribe("openTasks");

    // ── writes ───────────────────────────────────────────────────────
    let write_key = "t4-e2e-write";
    let write_body = serde_json::json!({ "project": "proj_twint4", "title": "T4 twin write" });
    let (status, body) = post_in(
        &h.app,
        "/op/addTask",
        TWIN,
        TWIN_OWNER,
        Some(write_key),
        write_body.clone(),
    )
    .await;
    assert_eq!(status, 200, "the twin's write was refused: {body}");

    let in_target = count(
        &h.target,
        &format!(
            "SELECT count(*) FROM \"tasks\" WHERE \"$space\" = '{TWIN}' \
             AND \"title\" = 'T4 twin write'"
        ),
    )
    .await;
    let in_control = count(
        &h.control,
        &format!(
            "SELECT count(*) FROM \"tasks\" WHERE \"$space\" = '{TWIN}' \
             AND \"title\" = 'T4 twin write'"
        ),
    )
    .await;
    assert_eq!(
        in_target, 1,
        "the twin's write must land in its own database"
    );
    assert_eq!(
        in_control, 0,
        "a resolver that fell back to the default would have written the twin's \
         row there instead, and this would still be 1"
    );

    // ── reads ────────────────────────────────────────────────────────
    let (status, body) = get_in(&h.app, "/op/openTasks", TWIN, TWIN_OWNER).await;
    assert_eq!(status, 200, "the twin's read was refused: {body}");
    assert!(
        body.to_string().contains("T4 twin write"),
        "the twin's own read did not find its own write, which the SQL check above \
         just proved exists nowhere but the target: {body}"
    );
    let (status, body) = get_in(&h.app, "/op/openTasks", HERE, HERE_MEMBER).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        !body.to_string().contains("T4 twin write"),
        "the deployment's own space read the twin's row: {body}"
    );

    // ── `?live` notifications ───────────────────────────────────────
    let mut heard_twin = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        let Ok(Some(Ok(chunk))) =
            tokio::time::timeout(Duration::from_secs(20), twin_sse.next()).await
        else {
            break;
        };
        if String::from_utf8_lossy(&chunk).contains("event: invalidate") {
            heard_twin = true;
            break;
        }
    }
    assert!(
        heard_twin,
        "the twin's own live subscriber never heard its own write — `?live=1` \
         is silent for an attached tenant"
    );

    let heard_unrouted = matches!(
        tokio::time::timeout(Duration::from_millis(500), control_rx.recv()).await,
        Ok(Ok(_))
    );
    assert!(
        !heard_unrouted,
        "a bus that only ever listens to the control database heard the twin's write \
         anyway, which means the twin's `NOTIFY` did not actually need routing to be heard"
    );

    // ── idempotency records ─────────────────────────────────────────
    let (status, body) = post_in(
        &h.app,
        "/op/addTask",
        TWIN,
        TWIN_OWNER,
        Some(write_key),
        write_body,
    )
    .await;
    assert_eq!(status, 200, "the replay must answer, not refuse: {body}");
    let after_replay = count(
        &h.target,
        &format!(
            "SELECT count(*) FROM \"tasks\" WHERE \"$space\" = '{TWIN}' \
             AND \"title\" = 'T4 twin write'"
        ),
    )
    .await;
    assert_eq!(
        after_replay, 1,
        "a second call under the same idempotency key must not execute a second time"
    );
    let idem_in_target = count(
        &h.target,
        &format!(
            "SELECT count(*) FROM \"$idempotency\" WHERE \"$space\" = '{TWIN}' \
             AND \"key\" = '{write_key}'"
        ),
    )
    .await;
    assert_eq!(
        idem_in_target, 1,
        "the twin's idempotency record must live in the twin's own database"
    );
    let idem_in_control = count_if_table_exists(
        &h.control,
        "$idempotency",
        &format!("\"$space\" = '{TWIN}'"),
    )
    .await;
    assert_eq!(
        idem_in_control, 0,
        "and not in the control database, where a fallback resolver would have written it"
    );

    // ── security events ─────────────────────────────────────────────
    //
    // A role refusal at the door, not a row-guard refusal at the
    // executor: see `TWIN_VIEWER`'s own doc for why `access_guard` would
    // not reach the recorder at all.
    let (status, body) = post_in(
        &h.app,
        "/op/addTask",
        TWIN,
        TWIN_VIEWER,
        None,
        serde_json::json!({ "project": "proj_twint4", "title": "a viewer should not write" }),
    )
    .await;
    assert_eq!(
        status, 403,
        "a viewer must be refused a write op, and refused by the door rather \
         than the executor: {body}"
    );

    // Force every open collapse window closed before reading `$secevents`:
    // a fresh denial otherwise sits unflushed until the next tick or up to
    // the collapse window's own length, and reading before that would find
    // zero rows regardless of whether routing is correct.
    h.secdrain
        .take()
        .expect("the drain is still held")
        .shutdown()
        .await
        .expect("the drain flushes on shutdown");

    let sec_in_target = count(
        &h.target,
        &format!("SELECT count(*) FROM \"$secevents\" WHERE \"scope\" = '{TWIN}'"),
    )
    .await;
    assert!(
        sec_in_target >= 1,
        "the twin's denial must be recorded in the twin's own database"
    );
    let sec_in_control =
        count_if_table_exists(&h.control, "$secevents", &format!("\"scope\" = '{TWIN}'")).await;
    assert_eq!(
        sec_in_control, 0,
        "and not in the control database, where a fallback resolver would have written it"
    );

    // ── and the deployment's own space is untouched throughout ──────
    let (status, body) = get_in(&h.app, "/op/openTasks", HERE, HERE_MEMBER).await;
    assert_eq!(
        status, 200,
        "the deployment's own space stopped serving while the twin was attached: {body}"
    );
}
