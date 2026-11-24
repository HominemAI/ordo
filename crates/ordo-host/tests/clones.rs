//! Staging clones named production tenants (`plan-versions.md` §3.4, §4
//! row 12), with the owner's ruling of 2026-11-23 on Q12.
//!
//! Real servers: production and staging are each a door on a loopback
//! socket over its own database, and a fake identity service signs one
//! person's tokens for both. Staging asks production's door for the clone as
//! the person, with their own token for production. Production masks the
//! tenant there and seals it to staging's key; staging opens it and imports
//! it under its own keys.
//!
//! What it proves:
//!
//! - a clone of a tenant on 0.1.0 serves in staging on 0.1.0, and no real
//!   personal data reached staging's database: not in the rows, not in the
//!   sealed values, not in the members, not in the command log;
//! - a cloned member cannot sign in to the clone; a staging admin opens it,
//!   and so does a person `clone.readers` names;
//! - `clone.mask` names safe fields, and turning masking off is an opt-in
//!   that still never carries a sign-in link;
//! - each refusal names its fix: no clone key recorded on production, no
//!   door for production on staging, another person, a bundle sealed to
//!   another key;
//! - a version staging does not hold comes down with the clone through the
//!   promotion gate, recorded as a clone, and a version edited after it was
//!   stored is refused on its way;
//! - nothing a clone does reaches a person or a service: a function's call
//!   and a routine's mail land in the sink, which the door reads back;
//!   with `clone.sinks` on (the default) so does every staging tenant's,
//!   `clones` narrows it, and production sinks nothing;
//! - a staging database without the clone version's tables is refused,
//!   naming `ordo promote`;
//! - a tenant with a database of its own is cloned onto a database of its
//!   own in staging, made by staging.
//!
//! A regression looks like a production value in staging's database, a
//! cloned member's sign-in opening the clone, or a refusal that does not
//! name the command that fixes it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use axum::http::StatusCode;
use ed25519_dalek::SigningKey;
use ordo_api::{router, ApiState, MemoryBus, MemoryReplayCache, PageKey};
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::{Anchor, KeyProvider, LocalKeyProvider, MemoryKeyStore, Purpose, SpaceId};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::clones::{self, CloneAccess, CloneInRequest, CloneOps, CloneRefusal};
use ordo_host::environments::{Door, Environments};
use ordo_host::grants::PgGrantRegistry;
use ordo_host::identity_keys::{IdentityKeyStore, IdentityKeys, IdentitySettings, ServiceUrl};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::provision::{self, FoundingOwner};
use ordo_host::tenants;
use ordo_host::versions::Versions;
use ordo_host::FixtureExecutor;
use ordo_identity::deployment_token::{self, DeploymentClaims, Kind, ROLE_ADMIN};
use ordo_identity::principal::{PrincipalId, Timestamp as IStamp};
use ordo_identity::{TokenKey, TokenSpec};
use serde_json::{json, Value};

/// The person cloning: an admin of both deployments.
const ADA: &str = "p_ada";
/// Another admin.
const BOB: &str = "p_bob";
const ISS: &str = "https://id.clones.example";
const HOME: &str = "ordo_home";
const SECRET: &[u8] = b"clones-test-app-tokens";

/// What production holds that must never reach staging.
const REAL: [&str; 6] = [
    "Ada Lovelace",
    "Charles Babbage",
    "Dear diary",
    "ada@example.com",
    "p_real_ada",
    "Analytical engine plans",
];

const PROGRAM: &str = "domain notes @ 0.1.0\n\n\
    entity notes {\n  title: text\n  body: text sealed\n  tags: json = []\n  \
    created: datetime\n  author -> members\n}\n\n\
    op allNotes unbounded =\n  notes[title, body, tags, created]\n";

/// The same program at another version: what staging may not hold.
fn program_at(version: &str) -> String {
    PROGRAM.replace("@ 0.1.0", &format!("@ {version}"))
}

fn keys_for(secret: &[u8]) -> Arc<dyn KeyProvider> {
    let k: Arc<dyn KeyProvider> = Arc::new(LocalKeyProvider::new(
        secret,
        Box::new(MemoryKeyStore::new()),
    ));
    k.create_space(&SpaceId(HOME.to_string()))
        .expect("home custody");
    k
}

async fn fresh(stem: &str) -> (Pool, Pg) {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 4,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    (pool, pg)
}

async fn apply(pool: &Pool, source: &str) {
    apply_op::run(
        pool,
        &SinglePool::new(pool.clone()),
        source,
        &apply_op::CodeFiles::new(),
        ADA,
        "t",
        true,
    )
    .await
    .expect("applied");
}

/// A deployment of the chain `registered`, being `own`, with the program.
async fn deployment(stem: &str, registered: &[&str], own: &str, source: &str) -> (Pool, Pg) {
    let (pool, pg) = fresh(stem).await;
    // The program first: a version stored before the chain was registered
    // counts as arrived, so each side may pin it without a promotion,
    // which is not what these tests are about.
    apply(&pool, source).await;
    let envs = Environments::on(&pool);
    let later = ordo_host::runtime::now_secs() + 60;
    for (i, name) in registered.iter().enumerate() {
        envs.register(name, Some(i as u32 + 1), ADA, "the chain", later)
            .await
            .expect("registered");
    }
    if own != "production" {
        envs.set_own(own, "this is it").await.expect("own");
    }
    (pool, pg)
}

/// Production's tenant `acme` on 0.1.0: Ada owns it, Charles is a member,
/// and 2 notes hold real words, one sealed.
async fn seed_tenant(pool: &Pool, keys: &Arc<dyn KeyProvider>) -> String {
    let id = tenants::mint_space_id();
    tenants::reserve(pool, &id, "acme", ADA, 100, None)
        .await
        .unwrap();
    provision::provision_space(
        pool,
        keys.as_ref(),
        &id,
        &FoundingOwner {
            member_id: "mem_ada",
            principal: "p_real_ada",
            display_name: "Ada Lovelace",
        },
    )
    .await
    .expect("provisioned");
    pool.rows(&format!(
        "INSERT INTO \"members\" (\"$space\", \"id\", \"rev\", \"principal\", \"displayName\", \
         \"kind\", \"memberCount\", \"role\", \"status\", \"joined\") VALUES \
         ('{id}', 'mem_charles', 1, 'p_real_charles', 'Charles Babbage', 'principal', 1, \
         'editor', 'active', '2024-03-01')"
    ))
    .await
    .unwrap();
    Versions::on(pool)
        .pin(&id, "0.1.0", ADA, 100)
        .await
        .expect("pinned");
    tenants::activate(pool, &id, 100).await.unwrap();
    let sid = SpaceId(id.clone());
    let lease = keys.release(&sid, Purpose::Write, "seed").unwrap();
    let seal = |row: &str, v: &str| {
        ordo_crypto::sealed::bytea_lit(
            &keys
                .seal_at(&lease, "notes", "body", Anchor::Row(row), v.as_bytes())
                .unwrap(),
        )
    };
    pool.rows(&format!(
        "INSERT INTO \"notes\" (\"$space\", \"id\", \"rev\", \"title\", \"body\", \"tags\", \
         \"created\", \"author\") VALUES \
         ('{id}', 'not_1', 1, 'Analytical engine plans', {b1}, '[\"ada@example.com\", \"work\"]', \
          '2025-06-01 10:00:00+00', 'mem_ada'), \
         ('{id}', 'not_2', 1, 'Dear diary', {b2}, '[]', '2025-06-02 11:00:00+00', 'mem_charles')",
        b1 = seal("not_1", "Dear diary, the engine computes Bernoulli numbers"),
        b2 = seal("not_2", "Charles Babbage came to tea"),
    ))
    .await
    .expect("notes");
    id
}

// ── the identity service ─────────────────────────────────────────────────

struct Issuer {
    url: String,
    key: SigningKey,
}

impl Issuer {
    async fn start() -> Issuer {
        let key = SigningKey::from_bytes(&[0x5c; 32]);
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
            jti: format!("j_{who}_{handle}_{now}"),
        };
        deployment_token::sign(
            &c,
            &deployment_token::key_id(&self.key.verifying_key()),
            &self.key,
        )
    }
}

/// An app session's token on staging, signed with staging's own key.
fn app_token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: IStamp(9_000_000_000),
            client: None,
            user_presence_at: Some(IStamp(ordo_host::runtime::now_secs())),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

// ── a deployment's door ──────────────────────────────────────────────────

/// The deployment over `pool`, its door on a loopback port, trusting the
/// issuer's tokens for `handle`, serving the notes program. Answers the
/// address and its stream.
async fn serve(
    db: &Pg,
    pool: &Pool,
    handle: &str,
    issuer: &Issuer,
    keys: Arc<dyn KeyProvider>,
    clone_access: Arc<CloneAccess>,
) -> (String, Arc<ordo_secevent::Recorder>) {
    // The app's ops read the deployment's own database, under its own keys,
    // so what the door answers in a clone is what staging holds.
    let mut program = Fixture::from_source(PROGRAM, "").expect("the program");
    keys.create_space(&ordo_conformance::fixture_space())
        .expect("fixture custody");
    program.keys = keys.clone();
    let f = Arc::new(program);
    let dir_pg = db.clone();
    let dir_pool = pool.clone();
    let grants = Arc::new(PgGrantRegistry::new(dir_pool.clone()));
    grants.ensure_table().await.expect("platform table");
    let engine = Arc::new(ordo_host::engine_from_members(&f, &[]).expect("engine"));
    // Served under the version the database records, so its tenants fit.
    let serving = Versions::on(pool)
        .active_version()
        .await
        .expect("registry")
        .expect("an active version");
    let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        serving,
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
    let apply_ops = apply_op::ApplyOps::recording(
        pool.clone(),
        Arc::new(SinglePool::new(pool.clone())),
        HOME,
        Some(stream.clone()),
    );
    let ops = Arc::new(
        PlatformOps::new(
            engine.clone(),
            grants,
            Arc::new(ordo_host::runtime::now_secs),
        )
        .with_apply(apply_ops.clone())
        .with_clones(Arc::new(CloneOps {
            apply: apply_ops,
            keys,
            access: clone_access.clone(),
            tenants: None,
        })),
    );
    let id_keys = Arc::new(IdentityKeys::new(
        IdentitySettings {
            url: ServiceUrl::parse(&issuer.url, false).unwrap(),
            issuer: ISS.into(),
            audience: deployment_token::audience(handle),
        },
        IdentityKeyStore::memory(),
        Arc::new(ordo_host::runtime::now_secs),
    ));
    id_keys.boot().await.expect("the keys");
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
            .with_identity_keys(id_keys)
            .with_clones(clone_access),
        ),
        quota: Arc::new(ordo_api::NoQuota),
        bus: Arc::new(MemoryBus::new()),
        replay: Arc::new(MemoryReplayCache::new()),
        attest: Arc::new(ordo_api::attest::NoAttestation),
        page_key: PageKey::from_secret(b"clones-test"),
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

/// One op on a door: status and body.
async fn call(
    address: &str,
    method: &'static str,
    op: &str,
    space: &str,
    body: Option<Value>,
    token: &str,
) -> (u16, Value) {
    use futures::StreamExt;
    let endpoint = ordo_blob::http::Endpoint::parse(address).unwrap();
    let mut headers = vec![
        ("authorization".into(), format!("Bearer {token}")),
        (ordo_api::router::SPACE_HEADER.into(), space.to_string()),
    ];
    let body = body.map(|mut b| {
        b["$space"] = json!(space);
        headers.push(("content-type".into(), "application/json".into()));
        let text = b.to_string().into_bytes();
        let len = text.len() as u64;
        let stream: ordo_blob::ByteStream =
            Box::pin(futures::stream::once(async move { Ok(text) }));
        (len, stream)
    });
    let r = ordo_blob::http::send(
        &endpoint,
        ordo_blob::http::Request {
            method,
            path: format!("/op/{op}"),
            headers,
            body,
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

async fn post(address: &str, op: &str, body: Value, token: &str) -> (u16, Value) {
    call(address, "POST", op, HOME, Some(body), token).await
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

/// Every byte staging holds for `space`: its rows, its members, its
/// command log, as text. What must not hold a production value.
async fn everything_in(pool: &Pool, space: &str) -> String {
    let mut out = String::new();
    for table in ["notes", "members"] {
        for r in pool
            .rows(&format!(
                "SELECT row_to_json(t)::text FROM \"{table}\" t WHERE \"$space\" = '{space}'"
            ))
            .await
            .unwrap()
        {
            out.push_str(&r[0].clone().unwrap_or_default());
        }
    }
    for r in pool
        .rows(&format!(
            "SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = '{space}'"
        ))
        .await
        .unwrap_or_default()
    {
        let bytes = ordo_crypto::sealed::unhex(&r[0].clone().unwrap_or_default()).unwrap();
        out.push_str(&String::from_utf8_lossy(&bytes));
    }
    out
}

/// Production and staging, served, and staging's key recorded on
/// production. Answers both and their doors.
struct Pair {
    issuer: Issuer,
    prod: Pool,
    prod_address: String,
    prod_stream: Arc<ordo_secevent::Recorder>,
    tenant: String,
    staging: Pool,
    staging_pg: Pg,
    staging_keys: Arc<dyn KeyProvider>,
    staging_address: String,
    staging_stream: Arc<ordo_secevent::Recorder>,
    access: Arc<CloneAccess>,
}

async fn pair(stem: &str, staging_source: &str) -> Pair {
    let issuer = Issuer::start().await;
    let (prod, prod_pg) = deployment(
        &format!("ordo_clone_{stem}_p"),
        &["staging"],
        "production",
        PROGRAM,
    )
    .await;
    let prod_keys = keys_for(b"production master key");
    let tenant = seed_tenant(&prod, &prod_keys).await;
    let (prod_address, prod_stream) = serve(
        &prod_pg,
        &prod,
        "production",
        &issuer,
        prod_keys,
        CloneAccess::new(),
    )
    .await;

    let (staging, staging_pg) = deployment(
        &format!("ordo_clone_{stem}_s"),
        &["staging"],
        "staging",
        staging_source,
    )
    .await;
    Environments::on(&staging)
        .set_door(
            "production",
            Some(Door {
                address: prod_address.clone(),
                space: HOME.to_string(),
                handle: Some("production".to_string()),
            }),
            "where production answers",
        )
        .await
        .expect("door");
    let staging_keys = keys_for(b"staging master key");
    let access = CloneAccess::new();
    let (staging_address, staging_stream) = serve(
        &staging_pg,
        &staging,
        "staging",
        &issuer,
        staging_keys.clone(),
        access.clone(),
    )
    .await;

    // Staging's clone key, recorded on production through both doors.
    let (status, body) = call(
        &staging_address,
        "GET",
        clones::CLONE_KEY,
        HOME,
        None,
        &issuer.token(ADA, "staging"),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let key = body["rows"][0]["key"].as_str().expect("a key").to_string();
    let (status, body) = post(
        &prod_address,
        clones::SET_CLONE_KEY,
        json!({"env": "staging", "key": key, "reason": "staging receives clones"}),
        &issuer.token(ADA, "production"),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    Pair {
        issuer,
        prod,
        prod_address,
        prod_stream,
        tenant,
        staging,
        staging_pg,
        staging_keys,
        staging_address,
        staging_stream,
        access,
    }
}

async fn clone_as(p: &Pair, who: &str, from_token: String) -> Result<clones::Cloned, CloneRefusal> {
    clones::clone_in(
        &p.staging,
        &SinglePool::new(p.staging.clone()),
        &p.staging_keys,
        HOME,
        None,
        CloneInRequest {
            from: "production",
            tenant: "acme",
            token: &from_token,
            name: None,
            reason: "rehearse 0.2.0 on real shapes",
        },
        who,
        2_000,
    )
    .await
}

// ── the clone ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_clone_serves_in_staging_on_its_version_with_no_real_personal_data() {
    let p = pair("c1", PROGRAM).await;
    let here = p.issuer.token(ADA, "staging");

    // The person clones through staging's door, with their own token for
    // production.
    let (status, body) = post(
        &p.staging_address,
        clones::CLONE_IN,
        json!({
            "tenant": "acme", "fromToken": p.issuer.token(ADA, "production"),
            "reason": "rehearse 0.2.0 on real shapes",
        }),
        &here,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let row = &body["rows"][0];
    let space = row["clone"]["space"].as_str().expect("a space").to_string();
    assert_ne!(space, p.tenant, "a new tenant, not production's id");
    assert_eq!(row["clone"]["name"], "acme-clone");
    assert_eq!(row["clone"]["fromTenant"], p.tenant.as_str());
    assert_eq!(row["clone"]["version"], "0.1.0");
    assert_eq!(row["clone"]["opensAs"], "mem_ada");
    let masked: Vec<&str> = row["report"]["masked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for f in ["notes.title", "notes.body", "notes.tags", "notes.created"] {
        assert!(masked.contains(&f), "{f} masked: {masked:?}");
    }
    assert_eq!(row["report"]["members"], 2);

    // It serves on its version.
    let t = tenants::lookup(&p.staging, &space).await.unwrap().unwrap();
    assert_eq!(t.state, tenants::State::Active);
    let pin = Versions::on(&p.staging)
        .pin_of(&space)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pin.version.as_deref(), Some("0.1.0"));

    // The rows arrived, masked, the refs kept.
    let rows = p
        .staging
        .rows(&format!(
            "SELECT \"id\", \"title\", \"tags\"::text, \"created\"::text, \"author\" \
             FROM \"notes\" WHERE \"$space\" = '{space}' ORDER BY \"author\""
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(r[1].as_deref().unwrap().starts_with("masked-"), "{r:?}");
        assert!(r[3].as_deref().unwrap().starts_with("2000-01-01"), "{r:?}");
    }
    assert_eq!(rows[0][2].as_deref(), Some("[\"masked\", \"masked\"]"));
    assert_eq!(rows[0][4].as_deref(), Some("mem_ada"));
    assert_eq!(rows[1][4].as_deref(), Some("mem_charles"));

    // The sealed body was sealed again under staging's own keys, for the new
    // tenant, and holds a placeholder.
    let sealed = p
        .staging
        .rows(&format!(
            "SELECT \"id\", \"body\" FROM \"notes\" WHERE \"$space\" = '{space}'"
        ))
        .await
        .unwrap();
    let sid = SpaceId(space.clone());
    let lease = p.staging_keys.release(&sid, Purpose::Read, "t").unwrap();
    for r in &sealed {
        let id = r[0].clone().unwrap();
        let bytes = ordo_crypto::sealed::unhex(&r[1].clone().unwrap()).unwrap();
        let plain = p
            .staging_keys
            .open_at(&lease, "notes", "body", Anchor::Row(&id), &bytes)
            .expect("opens under staging's keys");
        assert!(String::from_utf8(plain).unwrap().starts_with("masked-"));
    }

    // The members: kept ids and roles, no names, no sign-in link.
    let members = p
        .staging
        .rows(&format!(
            "SELECT \"id\", \"principal\", \"displayName\", \"role\" FROM \"members\" \
             WHERE \"$space\" = '{space}' ORDER BY \"id\""
        ))
        .await
        .unwrap();
    assert_eq!(members.len(), 2);
    for m in &members {
        let id = m[0].clone().unwrap();
        assert_eq!(m[1].as_deref(), Some(format!("$clone:{id}").as_str()));
    }
    assert_eq!(members[0][3].as_deref(), Some("owner"));

    // Nothing real anywhere staging holds for the clone.
    let all = everything_in(&p.staging, &space).await;
    for real in REAL {
        assert!(!all.contains(real), "`{real}` reached staging: {all}");
    }

    // Recorded on both sides.
    let record = clones::clones(&p.staging).await.unwrap();
    assert_eq!(record.len(), 1);
    assert_eq!(record[0].actor, ADA, "the person is named");
    assert_eq!(record[0].from_env, "production");
    assert!(acts(&p.prod_stream).contains(&"cloned-out".to_string()));
    assert!(acts(&p.staging_stream).contains(&"cloned-in".to_string()));

    // ── who opens it (Q12) ──
    // A staging admin, through staging's door, acting as the founding owner.
    let (status, body) = call(
        &p.staging_address,
        "GET",
        "allNotes",
        &space,
        None,
        &p.issuer.token(BOB, "staging"),
    )
    .await;
    assert_eq!(status, 200, "a staging admin opens the clone: {body}");
    // The notes it serves are the clone's, masked, the sealed body opened
    // under staging's keys.
    let served = body["rows"].as_array().expect("rows");
    assert_eq!(served.len(), 2, "{body}");
    for n in served {
        assert!(n["title"].as_str().unwrap().starts_with("masked-"), "{n}");
        assert!(n["body"].as_str().unwrap().starts_with("masked-"), "{n}");
    }
    for real in REAL {
        assert!(!body.to_string().contains(real), "`{real}` served");
    }

    // A cloned member's own sign-in opens nothing: their production
    // principal signed in to staging is no member of the clone.
    let (status, _) = call(
        &p.staging_address,
        "GET",
        "allNotes",
        &space,
        None,
        &app_token("p_real_ada"),
    )
    .await;
    assert_eq!(status, 403, "a cloned member cannot sign in");
    // Nor does the link written in its place.
    let (status, _) = call(
        &p.staging_address,
        "GET",
        "allNotes",
        &space,
        None,
        &app_token("$clone:mem_ada"),
    )
    .await;
    assert_eq!(status, 403);
    // The door staging builds from its members after the clone holds no
    // cloned member as anyone a sign-in names: not the production
    // principal, not the link written in its place, not the member's id.
    let f = Fixture::from_source(PROGRAM, "").unwrap();
    let engine = ordo_host::deployment_engine(&f, &p.staging_pg).expect("engine");
    let sid = ordo_identity::principal::SpaceId(space.clone());
    for who in ["p_real_ada", "$clone:mem_ada", "mem_ada", "mem_charles"] {
        assert_eq!(
            engine.member_id_of(&sid, &PrincipalId(who.into())),
            None,
            "`{who}` is a member of the clone"
        );
    }

    // A person staging allows: `clone.readers`.
    let (status, _) = call(
        &p.staging_address,
        "GET",
        "allNotes",
        &space,
        None,
        &app_token("p_tester"),
    )
    .await;
    assert_eq!(status, 403, "not yet allowed");
    let (status, body) = post(
        &p.staging_address,
        clones::SET_CLONE_SETTING,
        json!({"setting": "clone.readers", "value": "p_tester", "reason": "QA reads clones"}),
        &here,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(
        &p.staging_address,
        "GET",
        "allNotes",
        &space,
        None,
        &app_token("p_tester"),
    )
    .await;
    assert_eq!(status, 200, "an allowed person opens the clone: {body}");
    // A clone reader reaches the app's ops alone, never a platform op.
    let (status, _) = post(
        &p.staging_address,
        ordo_host::exit_ops::DELETE,
        json!({"confirm": space}),
        &app_token("p_tester"),
    )
    .await;
    assert_ne!(status, 200);
    // A live tenant is not opened by a deployment admin.
    assert!(p.access.opens_as(&space, BOB, true).is_some());
    assert!(p.access.opens_as("spc_not_a_clone", BOB, true).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn clone_mask_names_safe_fields_and_masking_off_still_carries_no_sign_in() {
    let p = pair("c2", PROGRAM).await;
    let prod_token = || p.issuer.token(ADA, "production");
    let set = |value: &'static str| {
        let address = p.prod_address.clone();
        let token = prod_token();
        async move {
            post(
                &address,
                clones::SET_CLONE_SETTING,
                json!({"setting": "clone.mask", "value": value, "reason": "titles are safe"}),
                &token,
            )
            .await
        }
    };

    // A safe field is carried as it is; every other field is masked.
    let (status, body) = set("notes.title").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["rows"][0]["mask"], "notes.title");
    let done = clone_as(&p, ADA, prod_token()).await.expect("cloned");
    assert_eq!(done.report.kept, ["notes.title"]);
    let all = everything_in(&p.staging, &done.record.space).await;
    assert!(all.contains("Analytical engine plans"), "the safe field");
    assert!(!all.contains("ada@example.com"), "tags still masked");
    assert!(!all.contains("Ada Lovelace"), "names still masked");

    // Masking off is an opt-in by name, and a sign-in link is never carried.
    let (status, _) = set("off").await;
    assert_eq!(status, 200);
    let done = clone_as(&p, ADA, prod_token()).await.expect("cloned");
    assert!(done.report.mask_off);
    let all = everything_in(&p.staging, &done.record.space).await;
    assert!(
        all.contains("ada@example.com"),
        "masking off carries the data: {all}"
    );
    assert!(!all.contains("p_real_ada"), "never a sign-in link");
    assert!(!all.contains("Ada Lovelace"), "never a member's name");

    // A value that is not a setting names the forms it takes.
    let (status, body) = set("everything").await;
    assert_eq!(status, 400, "{body}");
    assert!(body.to_string().contains("entity.field"), "{body}");
}

// ── refusals ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn each_refusal_names_its_fix() {
    let p = pair("c3", PROGRAM).await;
    let ada_prod = || p.issuer.token(ADA, "production");

    // Production read another person: a clone is one person's act.
    let e = clone_as(&p, ADA, p.issuer.token(BOB, "production"))
        .await
        .unwrap_err();
    assert!(matches!(e, CloneRefusal::NotSamePerson { .. }), "{e}");
    assert!(e.to_string().contains("ordo login"), "{e}");

    // Production refuses a token that is not for it.
    let e = clone_as(&p, ADA, p.issuer.token(ADA, "staging"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("refused your sign-in"), "{e}");

    // No such tenant there.
    let e = clones::clone_in(
        &p.staging,
        &SinglePool::new(p.staging.clone()),
        &p.staging_keys,
        HOME,
        None,
        CloneInRequest {
            from: "production",
            tenant: "nobody",
            token: &ada_prod(),
            name: None,
            reason: "x",
        },
        ADA,
        1,
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("ordo tenants"), "{e}");

    // A clone into the deployment it comes from.
    let e = clones::clone_in(
        &p.staging,
        &SinglePool::new(p.staging.clone()),
        &p.staging_keys,
        HOME,
        None,
        CloneInRequest {
            from: "staging",
            tenant: "acme",
            token: &ada_prod(),
            name: None,
            reason: "x",
        },
        ADA,
        1,
    )
    .await
    .unwrap_err();
    assert!(matches!(e, CloneRefusal::OwnEnv { .. }), "{e}");

    // Production holds another key for staging: the bundle does not open.
    let other = ordo_crypto::recipient::RecipientSecret::generate();
    clones::set_recipient(&p.prod, "staging", &other.public().hex(), ADA, "wrong", 1)
        .await
        .unwrap();
    let e = clone_as(&p, ADA, ada_prod()).await.unwrap_err();
    assert!(matches!(e, CloneRefusal::Bundle(_)), "{e}");
    assert!(e.to_string().contains("does not open"), "{e}");

    // Production holds no key for staging: the refusal names both commands.
    p.prod
        .rows("DELETE FROM \"$clone_recipients\"")
        .await
        .unwrap();
    let e = clone_as(&p, ADA, ada_prod()).await.unwrap_err();
    assert!(e.to_string().contains("ordo clones key"), "{e}");
    assert!(e.to_string().contains("ordo clones set-key staging"), "{e}");

    // A key that is not one.
    let e = clones::set_recipient(&p.prod, "staging", "abc", ADA, "x", 1)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("64 hex digits"), "{e}");
    // An environment production does not know.
    let e = clones::set_recipient(&p.prod, "qa", &other.public().hex(), ADA, "x", 1)
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("ordo environments register qa"),
        "{e}"
    );

    // Staging does not know where production answers.
    Environments::on(&p.staging)
        .set_door("production", None, "forgotten")
        .await
        .unwrap();
    let e = clone_as(&p, ADA, ada_prod()).await.unwrap_err();
    assert!(matches!(e, CloneRefusal::NoDoor { .. }), "{e}");
    assert!(
        e.to_string().contains("ordo environments door production"),
        "{e}"
    );

    // Nothing was made in staging by any refusal.
    assert!(clones::clones(&p.staging).await.unwrap().is_empty());
    let _ = &p.prod_address;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_staging_does_not_hold_comes_down_with_the_clone_through_the_gate() {
    // Staging holds 0.2.0 alone; the tenant serves 0.1.0 in production.
    let p = pair("c4", &program_at("0.2.0")).await;
    let active_before = Versions::on(&p.staging).active_version().await.unwrap();
    let done = clone_as(&p, ADA, p.issuer.token(ADA, "production"))
        .await
        .expect("cloned, its version with it");
    assert_eq!(done.arrived.as_deref(), Some("0.1.0"), "it came down");
    assert_eq!(done.record.version, "0.1.0");
    let pin = Versions::on(&p.staging)
        .pin_of(&done.record.space)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        pin.version.as_deref(),
        Some("0.1.0"),
        "the clone serves on it"
    );
    // Stored, never activated: staging still serves what it served.
    assert_eq!(
        Versions::on(&p.staging).active_version().await.unwrap(),
        active_before
    );
    // Recorded through the gate, as a clone, by the person.
    let promoted = ordo_host::promotion::promotions(&p.staging, 10)
        .await
        .unwrap();
    let arrival = promoted
        .iter()
        .find(|r| r.version == "0.1.0")
        .expect("recorded in $promotions");
    assert_eq!(arrival.via, "clone");
    assert_eq!(arrival.from_env, "production");
    assert_eq!(arrival.to_env, "staging");
    assert_eq!(arrival.actor, ADA);
    // A second clone finds it here and brings nothing down.
    let again = clone_as(&p, ADA, p.issuer.token(ADA, "production"))
        .await
        .expect("cloned again");
    assert_eq!(again.arrived, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_edited_after_it_was_stored_is_refused_on_its_way_down() {
    let p = pair("c5", &program_at("0.2.0")).await;
    // Production's stored copy of 0.1.0 is changed by hand: its identity
    // no longer matches what production recorded.
    p.prod
        .rows(
            "UPDATE \"$artifacts\" SET \"source\" = \"source\" || E'\\n# edited by hand\\n' \
             WHERE \"version\" = '0.1.0'",
        )
        .await
        .unwrap();
    let e = clone_as(&p, ADA, p.issuer.token(ADA, "production"))
        .await
        .unwrap_err();
    assert!(matches!(e, CloneRefusal::Version { .. }), "{e}");
    assert!(e.to_string().contains("definition file"), "{e}");
    assert!(e.to_string().contains("Apply the version again"), "{e}");
    // Nothing stored, nothing cloned.
    assert!(clones::clones(&p.staging).await.unwrap().is_empty());
    assert!(Versions::on(&p.staging)
        .get("0.1.0")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_database_without_the_versions_tables_is_refused_naming_promote() {
    // Staging serves a program with no `notes`. The first clone brings
    // 0.1.0 down through the gate, which gives the shared database its
    // tables. Then the table is gone (restored from an older backup, say):
    // a clone never changes the shared database, so the next is refused,
    // naming the commands that apply the version.
    let p = pair(
        "c8",
        "domain notes @ 0.2.0\n\nentity pages {\n  title: text\n}\n\n\
         op allPages unbounded =\n  pages[title]\n",
    )
    .await;
    clone_as(&p, ADA, p.issuer.token(ADA, "production"))
        .await
        .expect("the first clone");
    p.staging
        .rows("DROP TABLE \"notes\"")
        .await
        .expect("dropped");
    let e = clone_as(&p, ADA, p.issuer.token(ADA, "production"))
        .await
        .unwrap_err();
    let text = e.to_string();
    assert!(text.contains("does not hold the tables of 0.1.0"), "{text}");
    assert!(text.contains("ordo promote 0.1.0"), "{text}");
    assert!(text.contains("ordo evolve --to 0.1.0 --run"), "{text}");
    // Only the first clone is recorded.
    assert_eq!(clones::clones(&p.staging).await.unwrap().len(), 1);
}

// ── sinks ────────────────────────────────────────────────────────────────

/// The secret store a function's egress reads. Opening a value fails, so a
/// call that reached the real kernel says so.
struct NoValues;

#[async_trait::async_trait]
impl ordo_egress::SecretSource for NoValues {
    fn bound_hosts(&self, name: &str) -> Option<Vec<String>> {
        (name == "llmKey").then(|| vec!["api.example.com".to_string()])
    }
    async fn open(
        &self,
        _space: &str,
        name: &str,
    ) -> Result<Option<ordo_egress::SecretBytes>, ordo_egress::FetchError> {
        Err(ordo_egress::FetchError::Store {
            detail: format!("`{name}` was opened"),
        })
    }
}

fn llm_call(url: &str) -> ordo_egress::FnFetchRequest {
    ordo_egress::FnFetchRequest {
        method: "POST".into(),
        url: url.into(),
        headers: vec![
            (
                "x-api-key".into(),
                ordo_egress::HeaderValue::Secret {
                    name: "llmKey".into(),
                    template: "{}".into(),
                },
            ),
            (
                "content-type".into(),
                ordo_egress::HeaderValue::Plain("application/json".into()),
            ),
        ],
        body: br#"{"text":"masked-1a2b"}"#.to_vec(),
        egress: vec!["api.example.com".into(), "other.example.com".into()],
        secrets: vec!["llmKey".into()],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_a_clone_sends_leaves_staging_and_the_sink_shows_it() {
    use ordo_egress::FnFetch;
    let p = pair("c6", PROGRAM).await;
    let done = clone_as(&p, ADA, p.issuer.token(ADA, "production"))
        .await
        .expect("cloned");
    let space = done.record.space.clone();
    let fetch = ordo_host::sinks::SinkFetch::new(
        ordo_egress::FnEgress::new(
            Arc::new(NoValues),
            ordo_egress::TlsRoots::webpki(),
            ordo_egress::DenyList::default(),
        ),
        p.staging.clone(),
    );

    // ── egress ──
    // A clone's call is recorded and answered; no secret is opened and
    // nothing is dialled.
    let answer = fetch
        .fetch(&space, llm_call("https://api.example.com/v1/messages"))
        .await
        .expect("the sink answers");
    assert_eq!(answer.status, 200);
    assert_eq!(answer.body, b"{}");
    assert!(answer
        .headers
        .iter()
        .any(|(k, v)| k == ordo_host::sinks::SINK_HEADER && v == "recorded"));
    // A call the real kernel refuses is refused here too: a host off the
    // function's list, a secret not bound to the host.
    let mut off = llm_call("https://evil.example.net/x");
    off.headers.truncate(1);
    let e = fetch.fetch(&space, off).await.unwrap_err();
    assert_eq!(e.kind(), "host_not_allowed", "{e}");
    let e = fetch
        .fetch(&space, llm_call("https://other.example.com/x"))
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "secret_host_mismatch", "{e}");
    // `clone.sinks` is `on` by default: every tenant of a staging
    // deployment is sunk, a clone or not (D17, "in staging").
    let answer = fetch
        .fetch(
            "spc_not_a_clone",
            llm_call("https://api.example.com/v1/messages"),
        )
        .await
        .expect("staging's own tenant is sunk too");
    assert_eq!(answer.status, 200);
    // On production a tenant's call goes out through the kernel: it opens
    // the secret, which this store refuses.
    let prod_fetch = ordo_host::sinks::SinkFetch::new(
        ordo_egress::FnEgress::new(
            Arc::new(NoValues),
            ordo_egress::TlsRoots::webpki(),
            ordo_egress::DenyList::default(),
        ),
        p.prod.clone(),
    );
    let e = prod_fetch
        .fetch(&p.tenant, llm_call("https://api.example.com/v1/messages"))
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "secret_store_unavailable", "{e}");
    // `off` is no value: a clone's sends always go to the sink. The
    // refusal names the values it takes.
    let staging_admin = p.issuer.token(ADA, "staging");
    let (status, body) = post(
        &p.staging_address,
        clones::SET_CLONE_SETTING,
        json!({"setting": "clone.sinks", "value": "off", "reason": "try"}),
        &staging_admin,
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body.to_string().contains("`clones`"), "{body}");
    // `clones` narrows it to the clones: staging's own tenant goes out
    // through the kernel, and the clone is still sunk.
    let (status, body) = post(
        &p.staging_address,
        clones::SET_CLONE_SETTING,
        json!({"setting": "clone.sinks", "value": "clones", "reason": "QA calls a sandbox"}),
        &staging_admin,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["rows"][0]["sinks"], "clones");
    let e = fetch
        .fetch(
            "spc_not_a_clone",
            llm_call("https://api.example.com/v1/messages"),
        )
        .await
        .unwrap_err();
    assert_eq!(e.kind(), "secret_store_unavailable", "{e}");

    // ── mail ──
    // A routine's notice in a clone is recorded, and nothing is queued,
    // even with the deployment's mail on.
    let now = ordo_host::runtime::now_secs();
    let told = ordo_host::routines::tell(
        &p.staging,
        true,
        "fireReminders",
        &space,
        &["$clone:mem_ada".to_string()],
        "A reminder on one of your notes is due",
        now,
    )
    .await;
    assert_eq!(told, 1);
    let queued = p
        .staging
        .rows("SELECT count(*) FROM \"$mailoutbox\"")
        .await
        .map(|r| r[0][0].clone().unwrap_or_default())
        .unwrap_or_else(|_| "0".into());
    assert_eq!(queued, "0", "no mail was queued");
    // With `clones`, staging's own tenant is not sunk: with mail off,
    // nothing is told.
    let told = ordo_host::routines::tell(
        &p.staging,
        false,
        "fireReminders",
        "spc_not_a_clone",
        &["p_someone".to_string()],
        "x",
        now,
    )
    .await;
    assert_eq!(told, 0);
    // Back to `on`: staging's own tenant's notice goes to the sink.
    let (status, body) = post(
        &p.staging_address,
        clones::SET_CLONE_SETTING,
        json!({"setting": "sinks", "value": "on", "reason": "all of staging"}),
        &staging_admin,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let told = ordo_host::routines::tell(
        &p.staging,
        false,
        "fireReminders",
        "spc_not_a_clone",
        &["p_someone".to_string()],
        "x",
        now,
    )
    .await;
    assert_eq!(told, 1);
    // On production, nothing goes to the sink.
    let told = ordo_host::routines::tell(
        &p.prod,
        false,
        "fireReminders",
        &p.tenant,
        &["p_someone".to_string()],
        "x",
        now,
    )
    .await;
    assert_eq!(told, 0);

    // ── the door reads it back, to a deployment admin alone ──
    let (status, body) = call(
        &p.staging_address,
        "GET",
        &format!("{}?tenant={space}", clones::SINKS),
        HOME,
        None,
        &p.issuer.token(ADA, "staging"),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let entries = body["rows"][0]["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2, "{body}");
    let mail = &entries[0];
    assert_eq!(mail["kind"], "mail");
    assert_eq!(mail["target"], "$clone:mem_ada");
    assert_eq!(mail["summary"], "A reminder on one of your notes is due");
    let call_entry = &entries[1];
    assert_eq!(call_entry["kind"], "egress");
    assert_eq!(
        call_entry["target"],
        "POST https://api.example.com/v1/messages"
    );
    assert_eq!(call_entry["summary"], "headers: x-api-key, content-type");
    assert!(call_entry["body"].as_str().unwrap().contains("masked-1a2b"));
    // By the clone's name too.
    let (status, body) = call(
        &p.staging_address,
        "GET",
        &format!("{}?tenant=acme-clone", clones::SINKS),
        HOME,
        None,
        &p.issuer.token(ADA, "staging"),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["rows"][0]["entries"].as_array().unwrap().len(), 2);
    // A person who is no deployment admin reads nothing.
    let (status, _) = call(
        &p.staging_address,
        "GET",
        clones::SINKS,
        HOME,
        None,
        &app_token("p_tester"),
    )
    .await;
    assert_ne!(status, 200);
}

// ── storage of its own ───────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_with_its_own_database_is_cloned_onto_one_staging_makes() {
    let p = pair("c7", PROGRAM).await;
    // Production's tenant sits on a database of its own.
    tenants::at_phase(
        &p.prod,
        &p.tenant,
        tenants::Phase::Founded,
        "production-tenants/acme",
        100,
    )
    .await
    .unwrap();

    // Without tenant databases to give, the clone shares staging's and
    // the answer says so.
    let shared = clone_as(&p, ADA, p.issuer.token(ADA, "production"))
        .await
        .expect("cloned");
    assert_eq!(shared.database, clones::Storage::NotGiven);

    // Staging as the runtime assembles it: every tenant a database of its
    // own, on its own cluster, with its own credential.
    let f = Arc::new({
        let mut f = Fixture::from_source(PROGRAM, "").unwrap();
        f.keys = p.staging_keys.clone();
        f
    });
    let serving = ordo_host::deploy::Deployment::pinned_with_platform_ops(
        "0.1.0",
        f.clone(),
        ordo_exec::Executor {
            pool: p.staging.clone(),
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_conformance::fixture_space(),
        },
        platform_ops::specs(),
    )
    .unwrap();
    let own = ordo_exec::PoolConfig::from_env().password.clone();
    let creds = ordo_host::runtime::tenant_credentials(None, own);
    let template = ordo_exec::PoolConfig {
        size: 2,
        ..ordo_exec::PoolConfig::from_env()
    };
    // The deployment's own space, which provisioning asks the keys about.
    let _ = p
        .staging_keys
        .create_space(&SpaceId(ordo_host::provision::SYSTEM_SPACE.to_string()));
    let routing = ordo_host::tenant_ops::TenantOps::with_routing(
        p.staging.clone(),
        p.staging_keys.clone(),
        HOME,
        ordo_host::tenant_ops::Routing {
            serving,
            credentials: creds.clone(),
            template: template.clone(),
            hosts: ordo_host::provision::HostAllowlist::of(&ordo_exec::PoolConfig::from_env().host),
            default_cluster: ordo_host::tenant_ops::TenantCluster::from_env(
                &ordo_exec::PoolConfig::from_env(),
            ),
            stream: None,
        },
    );
    let resolver =
        ordo_exec::route::AttachedPools::new(p.staging.clone(), p.staging.clone(), template, creds);
    let token = p.issuer.token(ADA, "production");
    let done = clones::clone_in(
        &p.staging,
        &resolver,
        &p.staging_keys,
        HOME,
        Some(routing.as_ref()),
        CloneInRequest {
            from: "production",
            tenant: "acme",
            token: &token,
            name: Some("acme-own"),
            reason: "rehearse on its own database",
        },
        ADA,
        3_000,
    )
    .await
    .expect("cloned onto its own database");
    let space = done.record.space.clone();
    assert!(
        matches!(done.database, clones::Storage::Own(_)),
        "{:?}",
        done.database
    );

    // Staging attached a database named for the clone, on its own cluster.
    let attached = ordo_exec::spacedb::lookup(&p.staging, &SpaceId(space.clone()))
        .await
        .unwrap()
        .expect("an attachment");
    assert_eq!(attached.db, space, "the database is the clone's own");
    // The rows are there, and not in staging's shared database.
    let own_pool = Pool::connect(PoolConfig {
        db: space.clone(),
        size: 1,
        ..PoolConfig::from_env()
    })
    .await
    .unwrap();
    let n = own_pool
        .rows(&format!(
            "SELECT count(*) FROM \"notes\" WHERE \"$space\" = '{space}'"
        ))
        .await
        .unwrap();
    assert_eq!(n[0][0].as_deref(), Some("2"));
    let all = everything_in(&own_pool, &space).await;
    for real in REAL {
        assert!(!all.contains(real), "`{real}` reached the clone's database");
    }
    assert_eq!(everything_in(&p.staging, &space).await, "");
    drop(own_pool);
    let _ = p
        .staging_pg
        .exec(&format!("DROP DATABASE IF EXISTS \"{space}\" WITH (FORCE)"));
}
