//! The promotion gate and the environment changes (`plan-versions.md` §4
//! rows 4a and 4b), with the owner's rulings of 2026-11-24.
//!
//! Real servers: each deployment of the chain is a door on a loopback
//! socket over its own database, and a fake identity service publishes the
//! key that signs one person's tokens for each of them. The person promotes
//! from dev to staging: the staging door reads the version from dev's door
//! itself, with the person's own token for dev (Q11, no machine key).
//!
//! Each gate rule has a refusal test that tries the forbidden thing:
//!
//! - nothing is promoted into the first step; no door recorded for the step
//!   before; a door that answers as another environment;
//! - dev read a different person; dev refused the token; dev unreachable;
//!   dev has no such version;
//! - dev has not activated the version (read from dev, never handed over);
//! - dev's stored copy differs from what it recorded, naming the file;
//! - the target's own checks: a function secret, an external entity and a
//!   `reader` line, each before anything is stored;
//! - staging and production pin only what arrived through the gate;
//!   history stored before the first registration still counts;
//! - `ordo apply` goes to the first step: a later step is refused, naming
//!   the first step's address; a deployment that registered nothing applies;
//! - `promotion.skip_staging` is named when the step before is staging,
//!   and set, promotes from dev.
//!
//! A regression looks like a version reaching staging or production around
//! the gate, a source the caller handed over being trusted, a refusal that
//! does not name the fix, or a promotion with no record.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::http::StatusCode;
use ed25519_dalek::SigningKey;
use ordo_api::{router, ApiState, BoundParams, ErrorCode, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, CodeFile, CodeFiles};
use ordo_host::deploy_ops;
use ordo_host::environments::{self, Door, Environments};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::identity_keys::{IdentityKeyStore, IdentityKeys, IdentitySettings, ServiceUrl};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::promotion::{self, PromoteRequest, PromotionRefusal, SourceCredential};
use ordo_host::tenants;
use ordo_host::versions::Versions;
use ordo_host::FixtureExecutor;
use ordo_identity::deployment_token::{self, DeploymentClaims, Kind, ROLE_ADMIN};
use ordo_identity::TokenKey;
use serde_json::{json, Value};

/// The person promoting: one subject at the identity service, an admin of
/// every deployment here.
const ADA: &str = "p_ada";
/// Another admin.
const BOB: &str = "p_bob";
const ISS: &str = "https://id.promotion.example";
const HOME: &str = "ordo_home";
const CODE_PATH: &str = "fn/hello.js";
const SECRET: &[u8] = b"promotion-test-app-tokens";

fn source(version: &str, extra: &str) -> String {
    format!(
        "domain tasks @ {version}\n\nentity task {{\n  title: text\n}}\n\n\
         op openTasks unbounded =\n  task[title]\n\n\
         secret hookKey\n\n\
         function hello in tasks {{\n  call($t: text(10)) -> json by member\n  \
         code \"{CODE_PATH}\"\n  secrets hookKey\n}}\n{extra}"
    )
}

fn code(text: &str) -> CodeFiles {
    [(
        CODE_PATH.to_string(),
        CodeFile {
            hash: ordo_plan::artifact::code_hash(text.as_bytes()),
            text: text.to_string(),
        },
    )]
    .into()
}

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    all_at_once(&pool).await;
    pool
}

async fn apply_with(
    pool: &Pool,
    version: &str,
    extra: &str,
    activate: bool,
) -> Result<(), apply_op::ApplyRefused> {
    apply_op::run(
        pool,
        &SinglePool::new(pool.clone()),
        &source(version, extra),
        &code("export default {};"),
        ADA,
        "t",
        activate,
    )
    .await
    .map(|_| ())
}

async fn apply(pool: &Pool, version: &str, activate: bool) -> Result<(), apply_op::ApplyRefused> {
    apply_with(pool, version, "", activate).await
}

/// A deployment of the chain: `registered` in order, being `own`.
async fn deployment(stem: &str, registered: &[&str], own: &str) -> Pool {
    let pool = fresh(stem).await;
    let envs = Environments::on(&pool);
    for (i, name) in registered.iter().enumerate() {
        envs.register(name, Some(i as u32 + 1), ADA, "the chain", 100)
            .await
            .expect("registered");
    }
    if own != "production" {
        envs.set_own(own, "this is it").await.expect("own");
    }
    pool
}

async fn stored(pool: &Pool) -> Vec<String> {
    let v = Versions::on(pool);
    v.ensure().await.expect("tables");
    v.stored().await.unwrap().into_iter().map(|s| s.0).collect()
}

async fn set_secret(pool: &Pool, name: &str) {
    pool.rows(ordo_host::secrets::DDL).await.expect("$secrets");
    pool.rows(&format!(
        "INSERT INTO \"$secrets\" (\"name\", \"value\", \"version\", \"set_by\", \"set_at\") \
         VALUES ('{name}', '\\x00'::bytea, 1, 'ada', 1) ON CONFLICT DO NOTHING"
    ))
    .await
    .expect("secret set");
}

// ── the identity service ─────────────────────────────────────────────────

/// The fake identity service: one signing key, published on loopback.
struct Issuer {
    url: String,
    key: SigningKey,
}

impl Issuer {
    async fn start() -> Issuer {
        let key = SigningKey::from_bytes(&[0x4b; 32]);
        let kid = deployment_token::key_id(&key.verifying_key());
        let body =
            json!({"keys": [deployment_token::public_jwk(&kid, &key.verifying_key())]}).to_string();
        let app = axum::Router::new().route(
            ordo_host::identity_keys::JWKS_PATH,
            axum::routing::get(move || {
                let body = body.clone();
                async move { (StatusCode::OK, body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Issuer { url, key }
    }

    /// `who`'s token for the deployment `handle`, touched a moment ago.
    fn token(&self, who: &str, handle: &str) -> String {
        let now = ordo_host::runtime::now_secs();
        let c = DeploymentClaims {
            iss: ISS.into(),
            aud: deployment_token::audience(handle),
            sub: who.into(),
            kind: Kind::Person,
            role: ROLE_ADMIN.into(),
            client: "cli".into(),
            sid: format!("s_{who}"),
            touched_at: Some(now.saturating_sub(5)),
            grants: Vec::new(),
            iat: now,
            exp: now + 600,
            jti: format!("j_{who}_{handle}"),
        };
        deployment_token::sign(
            &c,
            &deployment_token::key_id(&self.key.verifying_key()),
            &self.key,
        )
    }
}

// ── a deployment's door ──────────────────────────────────────────────────

/// The deployment over `pool`, its door on a loopback port, trusting the
/// issuer's tokens for `handle`. Answers the address and its stream.
async fn serve(
    pool: &Pool,
    handle: &str,
    issuer: &Issuer,
) -> (String, Arc<ordo_secevent::Recorder>) {
    let f = Arc::new(Fixture::load().expect("fixture"));
    // One directory database per door: a stem is one database per process.
    static DOORS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = DOORS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir_pg = Pg::scoped(&format!("ordo_promo_dir{n}"));
    f.reset(&dir_pg).expect("seeded");
    let dir_pool = Pool::connect(PoolConfig {
        db: dir_pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("directory pool");
    let grants = Arc::new(PgGrantRegistry::new(dir_pool.clone()));
    grants.ensure_table().await.expect("platform table");
    let engine = Arc::new(ordo_host::engine_from_members(&f, &[]).expect("engine"));
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "promotion-test",
        f.clone(),
        ordo_exec::Executor {
            pool: dir_pool.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .expect("platform specs");
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = Arc::new(
        PlatformOps::new(
            engine.clone(),
            grants,
            Arc::new(ordo_host::runtime::now_secs),
        )
        .with_apply(apply_op::ApplyOps::recording(
            pool.clone(),
            Arc::new(SinglePool::new(pool.clone())),
            HOME,
            Some(stream.clone()),
        )),
    );
    let keys = Arc::new(IdentityKeys::new(
        IdentitySettings {
            url: ServiceUrl::parse(&issuer.url, false).unwrap(),
            issuer: ISS.into(),
            audience: deployment_token::audience(handle),
        },
        IdentityKeyStore::memory(),
        Arc::new(ordo_host::runtime::now_secs),
    ));
    keys.boot().await.expect("the keys");
    let state = ApiState {
        registry: deployment.handle(),
        executor: Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: dir_pg.clone(),
                page_size: 50,
            }),
            ops,
        )),
        access: Arc::new(
            ordo_host::IdentityAccess::sharing(
                engine,
                TokenKey::from_secret(SECRET),
                Box::new(ordo_host::runtime::now_secs),
                Arc::new(ordo_host::metrics::Metrics::new()),
            )
            .with_console_home(HOME)
            .with_identity_keys(keys),
        ),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"promotion-test"),
        sync: Default::default(),
        live_streams: Default::default(),
        refine_limits: Default::default(),
        public: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    (format!("http://{addr}"), stream)
}

/// A write op on a door, as a person: status and body.
async fn post(address: &str, op: &str, mut body: Value, token: &str) -> (u16, Value) {
    use futures::StreamExt;
    body["$space"] = json!(HOME);
    let text = body.to_string().into_bytes();
    let len = text.len() as u64;
    let stream: ordo_blob::ByteStream = Box::pin(futures::stream::once(async move { Ok(text) }));
    let endpoint = ordo_blob::http::Endpoint::parse(address).unwrap();
    let r = ordo_blob::http::send(
        &endpoint,
        ordo_blob::http::Request {
            method: "POST",
            path: format!("/op/{op}"),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("authorization".into(), format!("Bearer {token}")),
                (ordo_api::router::SPACE_HEADER.into(), HOME.into()),
            ],
            body: Some((len, stream)),
        },
    )
    .await
    .expect("the door answers");
    let status = r.status;
    let mut out = Vec::new();
    let mut s = r.body;
    while let Some(c) = s.next().await {
        out.extend_from_slice(&c.unwrap());
    }
    (status, serde_json::from_slice(&out).unwrap_or(Value::Null))
}

fn door_at(address: &str) -> Door {
    Door {
        address: address.to_string(),
        space: HOME.to_string(),
        handle: Some("dev".to_string()),
    }
}

/// Dev, served: `dev` first in its chain, 0.1.0 activated, 0.2.0 stored
/// only.
async fn dev(stem: &str, issuer: &Issuer) -> (Pool, String) {
    let pool = deployment(stem, &["dev"], "dev").await;
    apply(&pool, "0.1.0", true).await.expect("0.1.0");
    apply(&pool, "0.2.0", false).await.expect("0.2.0 staged");
    let (address, _) = serve(&pool, "dev", issuer).await;
    (pool, address)
}

/// Staging, its step before dev at `dev_address`.
async fn staging(stem: &str, dev_address: &str) -> Pool {
    let pool = deployment(stem, &["dev", "staging"], "staging").await;
    Environments::on(&pool)
        .set_door("dev", Some(door_at(dev_address)), "where dev answers")
        .await
        .expect("door");
    pool
}

/// Promote `version` into `to` as `caller`, reading with `from_token`.
async fn promote_as(
    to: &Pool,
    version: &str,
    caller: &str,
    from_token: String,
) -> Result<promotion::Promoted, PromotionRefusal> {
    promotion::promote(
        to,
        &SinglePool::new(to.clone()),
        PromoteRequest {
            version,
            credential: &SourceCredential::Person { token: from_token },
            reason: "ready for staging",
            activate: true,
        },
        caller,
        1_000,
    )
    .await
}

fn acts(stream: &ordo_secevent::Recorder) -> Vec<String> {
    stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect()
}

// ── the person promotes ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn the_person_promotes_from_dev_to_staging_through_both_doors() {
    let issuer = Issuer::start().await;
    let (_dev, dev_address) = dev("ordo_promo_d1", &issuer).await;
    let staging = deployment("ordo_promo_s1", &["dev", "staging"], "staging").await;
    set_secret(&staging, "hookKey").await;
    let (staging_address, stream) = serve(&staging, "staging", &issuer).await;
    let here = issuer.token(ADA, "staging");

    // Where dev answers, recorded on staging through its door.
    let (status, body) = post(
        &staging_address,
        deploy_ops::SET_ENVIRONMENT_DOOR,
        json!({
            "name": "dev", "address": dev_address, "space": HOME, "handle": "dev",
            "reason": "where dev answers",
        }),
        &here,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["rows"][0]["door"]["address"], dev_address.as_str());

    // The person promotes: their staging token, and their own dev token
    // for staging to read dev with.
    let (status, body) = post(
        &staging_address,
        deploy_ops::PROMOTE,
        json!({
            "version": "0.1.0", "fromToken": issuer.token(ADA, "dev"),
            "reason": "ready for staging",
        }),
        &here,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["from"], "dev");
    assert_eq!(row["to"], "staging");
    assert_eq!(row["promoted"], "0.1.0");

    let v = Versions::on(&staging);
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    let there = v.identity("0.1.0").await.unwrap().unwrap().1;
    assert_eq!(row["sourceId"], there.as_str());
    let records = promotion::promotions(&staging, 10).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].from_env, "dev");
    assert_eq!(records[0].actor, ADA, "the person is named");
    assert_eq!(acts(&stream), ["environment-door", "promoted"]);

    // The handed-over source of 4b is gone: the op takes no source.
    let (status, body) = post(
        &staging_address,
        deploy_ops::PROMOTE,
        json!({
            "version": "0.1.0", "reason": "x", "source": "domain x @ 9.9.9",
            "activated": true,
        }),
        &here,
    )
    .await;
    assert_ne!(status, 200, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn staging_refuses_a_version_dev_has_not_activated() {
    let issuer = Issuer::start().await;
    let (_dev, dev_address) = dev("ordo_promo_d2", &issuer).await;
    let staging = staging("ordo_promo_s2", &dev_address).await;
    set_secret(&staging, "hookKey").await;
    let e = promote_as(&staging, "0.2.0", ADA, issuer.token(ADA, "dev"))
        .await
        .expect_err("refused");
    assert!(matches!(e, PromotionRefusal::NotActivated { .. }), "{e}");
    let text = e.to_string();
    assert!(text.contains("`dev` has not activated 0.2.0"), "{text}");
    assert!(text.contains("Activate it there"), "{text}");
    assert!(stored(&staging).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_gate_reads_the_step_before_itself_and_refuses_what_is_wrong_there() {
    let issuer = Issuer::start().await;
    let (dev_pool, dev_address) = dev("ordo_promo_d3", &issuer).await;
    let staging = deployment("ordo_promo_s3", &["dev", "staging"], "staging").await;
    set_secret(&staging, "hookKey").await;
    let ada_dev = || issuer.token(ADA, "dev");

    // No door recorded for dev: the refusal names the command.
    let e = promote_as(&staging, "0.1.0", ADA, ada_dev())
        .await
        .unwrap_err();
    assert!(matches!(e, PromotionRefusal::NoDoor { .. }), "{e}");
    assert!(e.to_string().contains("ordo environments door dev"), "{e}");

    // Nothing is promoted into the first step.
    let e = promote_as(&dev_pool, "0.1.0", ADA, ada_dev())
        .await
        .unwrap_err();
    assert!(matches!(e, PromotionRefusal::NoStepBefore { .. }), "{e}");
    assert!(e.to_string().contains("ordo apply"), "{e}");

    // A door that answers as another environment.
    let other = fresh("ordo_promo_o3").await;
    apply(&other, "0.1.0", true).await.expect("applied");
    let (other_address, _) = serve(&other, "dev", &issuer).await;
    Environments::on(&staging)
        .set_door("dev", Some(door_at(&other_address)), "wrong one")
        .await
        .unwrap();
    let e = promote_as(&staging, "0.1.0", ADA, ada_dev())
        .await
        .unwrap_err();
    assert!(matches!(e, PromotionRefusal::WrongSource { .. }), "{e}");
    assert!(e.to_string().contains("answers as `production`"), "{e}");

    Environments::on(&staging)
        .set_door("dev", Some(door_at(&dev_address)), "the right one")
        .await
        .unwrap();
    // Dev read someone else: a promotion is one person's act.
    let e = promote_as(&staging, "0.1.0", ADA, issuer.token(BOB, "dev"))
        .await
        .unwrap_err();
    assert!(matches!(e, PromotionRefusal::NotSamePerson { .. }), "{e}");
    assert!(e.to_string().contains("`p_bob`"), "{e}");
    assert!(e.to_string().contains("ordo login"), "{e}");

    // Dev refuses a token that is not for it.
    let e = promote_as(&staging, "0.1.0", ADA, issuer.token(ADA, "staging"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &e,
            PromotionRefusal::Source {
                error: ordo_host::step_door::StepDoorError::Refused { .. },
                ..
            }
        ),
        "{e}"
    );
    assert!(e.to_string().contains("refused your sign-in"), "{e}");
    assert_eq!(e.to_exec_error("x").code, ErrorCode::ApplyRefused);

    // Dev has no such version.
    let e = promote_as(&staging, "9.9.9", ADA, ada_dev())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("has no such version"), "{e}");

    // Dev is down: retryable.
    Environments::on(&staging)
        .set_door("dev", Some(door_at("http://127.0.0.1:1")), "down")
        .await
        .unwrap();
    let e = promote_as(&staging, "0.1.0", ADA, ada_dev())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("could not be reached"), "{e}");
    assert_eq!(e.to_exec_error("x").code, ErrorCode::SpaceUnavailable);

    // A plain address to another host is refused when it is recorded.
    let e = Door::checked("http://dev.internal:4000", HOME, None).unwrap_err();
    assert!(e.to_string().contains("ORDO_PROMOTION_PLAIN_HTTP"), "{e}");
    assert!(stored(&staging).await.is_empty(), "nothing was stored");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_copy_on_the_step_before_is_refused_naming_the_file() {
    let issuer = Issuer::start().await;
    let (dev_pool, dev_address) = dev("ordo_promo_d4", &issuer).await;
    let staging = staging("ordo_promo_s4", &dev_address).await;
    set_secret(&staging, "hookKey").await;

    // The function's code changed in dev's store after it was applied.
    dev_pool
        .rows("UPDATE \"$fncode\" SET \"code\" = convert_to('export default 1;', 'UTF8')")
        .await
        .expect("tampered");
    let e = promote_as(&staging, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .unwrap_err();
    assert!(matches!(e, PromotionRefusal::Edited { .. }), "{e}");
    assert!(e.to_string().contains("`fn/hello.js`"), "{e}");
    assert!(e.to_string().contains("Apply the version again"), "{e}");

    // The definition file changed in dev's store.
    let (dev2, dev2_address) = dev("ordo_promo_d4b", &issuer).await;
    dev2.rows(
        "UPDATE \"$artifacts\" SET \"source\" = \"source\" || E'\\n// edited\\n' \
         WHERE \"version\" = '0.1.0'",
    )
    .await
    .expect("tampered");
    Environments::on(&staging)
        .set_door("dev", Some(door_at(&dev2_address)), "dev again")
        .await
        .unwrap();
    let e = promote_as(&staging, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("the definition file"), "{e}");
    assert!(stored(&staging).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_targets_own_checks_run_before_anything_is_stored() {
    let issuer = Issuer::start().await;
    let (_dev, dev_address) = dev("ordo_promo_d5", &issuer).await;
    let staging = staging("ordo_promo_s5", &dev_address).await;

    // A function secret with no value here.
    let e = promote_as(&staging, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .unwrap_err();
    assert!(matches!(e, PromotionRefusal::SecretsMissing(_)), "{e}");
    assert!(e.to_string().contains("hookKey"), "{e}");
    assert!(e.to_string().contains("ordo secret set"), "{e}");
    assert!(stored(&staging).await.is_empty());
    assert!(promotion::promotions(&staging, 10)
        .await
        .unwrap()
        .is_empty());

    set_secret(&staging, "hookKey").await;
    promote_as(&staging, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .expect("promoted once the secret is set");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_external_entity_that_does_not_check_out_on_the_target_is_refused() {
    let issuer = Issuer::start().await;
    let ext = "\nexternal patrons (key: pat) from crm.customers shared {\n  \
               id <- \"customer_id\"\n  name: text <- \"full_name\"\n}\n";
    let dev_pool = deployment("ordo_promo_d6", &["dev"], "dev").await;
    dev_pool
        .rows(
            "CREATE SCHEMA crm; CREATE TABLE crm.customers (customer_id bigint PRIMARY KEY, \
             full_name text NOT NULL)",
        )
        .await
        .expect("dev's table");
    apply_with(&dev_pool, "0.1.0", ext, true)
        .await
        .expect("dev applies it");
    let (dev_address, _) = serve(&dev_pool, "dev", &issuer).await;
    let staging = staging("ordo_promo_s6", &dev_address).await;
    set_secret(&staging, "hookKey").await;

    let e = promote_as(&staging, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .unwrap_err();
    assert!(matches!(e, PromotionRefusal::External(_)), "{e}");
    assert!(e.to_string().contains("crm.customers"), "{e}");
    assert!(stored(&staging).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_line_with_no_role_on_the_target_is_refused_naming_the_setting() {
    const VAR: &str = "ORDO_READER_PROMOVIEW_ROLE";
    const ROLE: &str = "ordo_promo_reader";
    let issuer = Issuer::start().await;
    let dev_pool = deployment("ordo_promo_d7", &["dev"], "dev").await;
    dev_pool
        .rows(&format!(
            "DO $$ BEGIN CREATE ROLE {ROLE} NOLOGIN; \
             EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;"
        ))
        .await
        .expect("role");
    // Dev grants the reader: its role is set there.
    std::env::set_var(VAR, ROLE);
    apply_with(&dev_pool, "0.1.0", "\nreader promoview reads task\n", true)
        .await
        .expect("dev applies it");
    let (dev_address, _) = serve(&dev_pool, "dev", &issuer).await;
    let staging = staging("ordo_promo_s7", &dev_address).await;
    set_secret(&staging, "hookKey").await;
    // Staging has no such setting.
    std::env::remove_var(VAR);
    let e = promote_as(&staging, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .unwrap_err();
    assert!(matches!(e, PromotionRefusal::Readers(_)), "{e}");
    assert!(e.to_string().contains(VAR), "{e}");
    assert!(stored(&staging).await.is_empty());
}

// ── what arrives through the gate ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn staging_and_production_pin_only_what_arrived_through_the_gate() {
    let issuer = Issuer::start().await;
    let (_dev, dev_address) = dev("ordo_promo_d8", &issuer).await;

    for own in ["staging", "production"] {
        // The deployment as it was: one version applied directly.
        let pool = fresh(&format!("ordo_promo_{own}8")).await;
        apply(&pool, "0.0.9", true).await.expect("history");
        // Registering gives it a step before. Registered later than the
        // history, so the history stays usable.
        let envs = Environments::on(&pool);
        envs.register("dev", Some(1), ADA, "where work starts", u32::MAX as u64)
            .await
            .expect("dev");
        if own == "staging" {
            envs.register("staging", Some(2), ADA, "rehearsal", u32::MAX as u64)
                .await
                .expect("staging");
            envs.set_own("staging", "this is staging").await.unwrap();
        } else {
            envs.register("staging", Some(2), ADA, "rehearsal", u32::MAX as u64)
                .await
                .expect("staging");
            envs.set_promotion_setting(
                environments::SKIP_STAGING,
                "on",
                ADA,
                "no staging host yet",
                1,
            )
            .await
            .unwrap();
        }
        envs.set_door("dev", Some(door_at(&dev_address)), "dev")
            .await
            .unwrap();
        set_secret(&pool, "hookKey").await;

        // A version applied straight to it is refused before it is stored.
        let e = apply(&pool, "0.1.0", true).await.unwrap_err();
        assert_eq!(e.class(), "not_promoted", "{own}: {e}");
        assert!(e.to_string().contains("ordo promote"), "{e}");
        assert!(e.to_string().contains("starts on `dev`"), "{e}");
        assert_eq!(stored(&pool).await.len(), 1, "{own}");

        // History from before the chain still activates.
        apply_op::activate(&pool, "0.0.9", ADA)
            .await
            .expect("history still activates");

        promote_as(&pool, "0.1.0", ADA, issuer.token(ADA, "dev"))
            .await
            .expect("through the gate");
        let v = Versions::on(&pool);
        assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
        apply_op::activate(&pool, "0.0.9", ADA).await.expect("back");
        apply_op::activate(&pool, "0.1.0", ADA)
            .await
            .expect("forward");
    }

    // A deployment that registered nothing is not held to the gate.
    let free = fresh("ordo_promo_free").await;
    apply(&free, "0.1.0", true).await.expect("applies freely");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pin_to_a_version_that_skipped_the_gate_is_refused() {
    let pool = fresh("ordo_promo_pin").await;
    apply(&pool, "0.0.9", true).await.expect("history");
    Environments::on(&pool)
        .register("dev", None, ADA, "where work starts", 1)
        .await
        .expect("dev");
    // Staged, not activated: stored, so a pin to it must still be refused.
    apply(&pool, "0.3.0", false)
        .await
        .expect("a staged apply stores");
    tenants::reserve(&pool, "spc_a", "spc_a", "dev:ada", 10, None)
        .await
        .expect("reserved");
    tenants::activate(&pool, "spc_a", 11).await.expect("active");
    let e = Versions::on(&pool)
        .pin("spc_a", "0.3.0", ADA, 100)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not_promoted"), "{e}");
    assert!(e.to_string().contains("ordo promote 0.3.0"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn skip_staging_is_named_and_set_promotes_past_staging_from_dev() {
    let issuer = Issuer::start().await;
    let (_dev, dev_address) = dev("ordo_promo_d9", &issuer).await;
    let prod = deployment("ordo_promo_p9", &["dev", "staging"], "production").await;
    set_secret(&prod, "hookKey").await;
    let envs = Environments::on(&prod);
    envs.set_door("dev", Some(door_at(&dev_address)), "dev")
        .await
        .unwrap();
    // Staging is recorded, and down.
    envs.set_door("staging", Some(door_at("http://127.0.0.1:1")), "staging")
        .await
        .unwrap();
    let e = promote_as(&prod, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("reading from `staging`"), "{e}");
    assert!(e.to_string().contains("promotion.skip_staging"), "{e}");
    assert!(
        e.to_string()
            .contains("ordo environments set skip-staging on"),
        "{e}"
    );

    // Set through the door, recorded.
    let (address, stream) = serve(&prod, "production", &issuer).await;
    let (status, body) = post(
        &address,
        deploy_ops::SET_PROMOTION_SETTING,
        json!({"setting": "promotion.skip_staging", "value": "on", "reason": "no staging yet"}),
        &issuer.token(ADA, "production"),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["rows"][0]["changed"], true);
    let (status, body) = post(
        &address,
        deploy_ops::SET_PROMOTION_SETTING,
        json!({"setting": "promotion.skip_staging", "value": "maybe", "reason": "x"}),
        &issuer.token(ADA, "production"),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(acts(&stream), ["promotion-setting"]);

    let done = promote_as(&prod, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .expect("from dev");
    assert_eq!(done.from, "dev");
    assert_eq!(done.to, "production");
}

// ── where `ordo apply` goes ──────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn ordo_apply_goes_to_the_first_step_of_the_chain() {
    // A deployment that registered nothing is its own first step.
    let alone = fresh("ordo_promo_alone").await;
    let chain = Environments::on(&alone).chain().await.unwrap();
    environments::check_apply_target(&chain).expect("a single deployment applies");

    // Dev is the first step.
    let dev = deployment("ordo_promo_dev10", &["dev"], "dev").await;
    let chain = Environments::on(&dev).chain().await.unwrap();
    environments::check_apply_target(&chain).expect("dev applies");

    // Staging is not: the refusal names dev's address when it is known.
    let staging = deployment("ordo_promo_s10", &["dev", "staging"], "staging").await;
    let chain = Environments::on(&staging).chain().await.unwrap();
    let e = environments::check_apply_target(&chain).unwrap_err();
    assert!(e.to_string().contains("--host <its address>"), "{e}");
    Environments::on(&staging)
        .set_door("dev", Some(door_at("https://dev.example.com")), "dev")
        .await
        .unwrap();
    let chain = Environments::on(&staging).chain().await.unwrap();
    let e = environments::check_apply_target(&chain).unwrap_err();
    let text = e.to_string();
    assert!(text.contains("first step of the chain, `dev`"), "{text}");
    assert!(text.contains("--host https://dev.example.com"), "{text}");
    assert!(text.contains("ordo promote"), "{text}");
}

// ── the chain's own changes ──────────────────────────────────────────────

fn ops(pool: &Pool, stream: Arc<ordo_secevent::Recorder>) -> Arc<apply_op::ApplyOps> {
    apply_op::ApplyOps::recording(
        pool.clone(),
        Arc::new(SinglePool::new(pool.clone())),
        "home",
        Some(stream),
    )
}

async fn run(
    ops: &apply_op::ApplyOps,
    name: &str,
    v: Value,
) -> Result<ordo_api::ExecResult, ordo_api::ExecError> {
    let b = BoundParams(v.as_object().unwrap().clone());
    deploy_ops::execute(ops, name, "home", &b, ADA).await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_changes_which_environment_this_deployment_is() {
    let pool = fresh("ordo_promo_own").await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let o = ops(&pool, stream.clone());
    for name in ["dev", "staging"] {
        run(
            &o,
            deploy_ops::REGISTER_ENVIRONMENT,
            json!({"name": name, "reason": "chain"}),
        )
        .await
        .expect("registered");
    }
    let r = run(
        &o,
        deploy_ops::SET_OWN_ENVIRONMENT,
        json!({"name": "staging", "reason": "this host is staging"}),
    )
    .await
    .expect("set");
    assert_eq!(r.rows[0]["own"], "staging");
    assert_eq!(r.rows[0]["changed"], true);
    assert_eq!(Environments::on(&pool).own().await.unwrap().name, "staging");
    let again = run(
        &o,
        deploy_ops::SET_OWN_ENVIRONMENT,
        json!({"name": "staging", "reason": "again"}),
    )
    .await
    .expect("unchanged");
    assert_eq!(again.rows[0]["changed"], false);

    let e = run(
        &o,
        deploy_ops::SET_OWN_ENVIRONMENT,
        json!({"name": "qa", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(
        e.message.contains("ordo environments register qa"),
        "{}",
        e.message
    );
    let e = run(
        &o,
        deploy_ops::SET_OWN_ENVIRONMENT,
        json!({"name": "dev", "reason": ""}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParam);

    // A door: an address with no space is refused, a space is kept.
    let e = run(
        &o,
        deploy_ops::SET_ENVIRONMENT_DOOR,
        json!({"name": "dev", "address": "https://dev.example.com", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParam);
    assert!(e.message.contains("--space"), "{}", e.message);
    let r = run(
        &o,
        deploy_ops::SET_ENVIRONMENT_DOOR,
        json!({"name": "dev", "address": "https://dev.example.com/", "space": "system",
               "handle": "dev", "reason": "where dev answers"}),
    )
    .await
    .expect("door");
    assert_eq!(r.rows[0]["door"]["address"], "https://dev.example.com");
    let listed = run(&o, deploy_ops::ENVIRONMENTS, json!({})).await.unwrap();
    assert_eq!(listed.rows[0]["chain"][0]["door"]["handle"], "dev");
    assert_eq!(listed.rows[0]["skipStaging"], false);
    let e = run(
        &o,
        deploy_ops::SET_ENVIRONMENT_DOOR,
        json!({"name": "qa", "address": "https://qa.example.com", "space": "system",
               "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);

    assert_eq!(
        acts(&stream),
        [
            "environment-registered",
            "environment-registered",
            "environment-own",
            "environment-door",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_is_removed_or_renamed_unless_it_is_owned_or_named() {
    let pool = deployment("ordo_promo_rm", &["dev", "staging"], "staging").await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let o = ops(&pool, stream.clone());

    // The deployment's own stays.
    let e = run(
        &o,
        deploy_ops::REMOVE_ENVIRONMENT,
        json!({"name": "staging", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::ApplyRefused);
    assert!(
        e.message.contains("ordo environments set-own"),
        "{}",
        e.message
    );

    // Rename, then remove one nothing names.
    let r = run(
        &o,
        deploy_ops::RENAME_ENVIRONMENT,
        json!({"name": "dev", "to": "develop", "reason": "clearer"}),
    )
    .await
    .expect("renamed");
    assert_eq!(r.rows[0]["chain"][0]["name"], "develop");
    let e = run(
        &o,
        deploy_ops::RENAME_ENVIRONMENT,
        json!({"name": "develop", "to": "staging", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Duplicate);
    let r = run(
        &o,
        deploy_ops::REMOVE_ENVIRONMENT,
        json!({"name": "develop", "reason": "not used"}),
    )
    .await
    .expect("removed");
    let names: Vec<_> = r.rows[0]["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["staging", "production"]);
    assert_eq!(r.rows[0]["chain"][0]["position"], 1);

    // A stored promotion names `staging` and `dev`: both stay.
    let issuer = Issuer::start().await;
    let (_dev, dev_address) = dev("ordo_promo_d11", &issuer).await;
    let staging = staging("ordo_promo_rm2", &dev_address).await;
    set_secret(&staging, "hookKey").await;
    promote_as(&staging, "0.1.0", ADA, issuer.token(ADA, "dev"))
        .await
        .expect("promoted");
    let o2 = ops(&staging, stream.clone());
    for (op, params) in [
        (
            deploy_ops::REMOVE_ENVIRONMENT,
            json!({"name": "dev", "reason": "x"}),
        ),
        (
            deploy_ops::RENAME_ENVIRONMENT,
            json!({"name": "dev", "to": "develop", "reason": "x"}),
        ),
    ] {
        let e = run(&o2, op, params).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ApplyRefused, "{op}");
        assert!(
            e.message.contains("stored promotion record"),
            "{}",
            e.message
        );
        assert!(
            e.message.contains("ordo environments register"),
            "names what to do: {}",
            e.message
        );
    }
    assert_eq!(
        acts(&stream),
        ["environment-renamed", "environment-removed"]
    );
}

/// `rollout.strategy` `all` on `pool`. With it unset, a deployment that is
/// not dev rolls an apply out a batch at a time (ruled 2026-11-25); these
/// tests apply and expect every tenant on the new version at once.
async fn all_at_once(pool: &ordo_exec::Pool) {
    ordo_host::rollouts::Rollouts::on(pool)
        .set_setting(
            "rollout.strategy",
            Some("all"),
            "test",
            "every tenant at once",
            0,
        )
        .await
        .expect("rollout.strategy all");
}
