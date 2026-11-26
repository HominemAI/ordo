//! The consumer's half of federation through the door
//! (`plan-federation-grants.md` 4, 5 and 7, step G4), against a real
//! identity service and 2 real deployments.
//!
//! `notes` is the holder: a door on a loopback socket over its own
//! database, granting `note_viewer as itself { std.journal.list  addEntry }`
//! and tied to the consumer `viewer` by `ORDO_CONSUMER_NOTE_VIEWER_CLIENT`.
//! `viewer` is the consumer: its own database and deploy door, with the
//! one client ([`ordo_host::peer_client`]) signing with its consumer key.
//!
//! What it proves:
//!
//! - the consumer's key is registered at the service through the
//!   consumer's own door op, with a token for its own admin; before that,
//!   its apply is refused naming the op that registers it;
//! - the consumer's apply passes when the holder grants every op its
//!   `peer` block lists, and the holder records what it calls;
//! - an op the holder does not grant refuses the consumer's apply by name,
//!   and nothing is stored; so does a deployment the holder grants nothing;
//! - the holder's plan names the consumer a removal breaks ("note_viewer
//!   calls addEntry"), and warns rather than refuses;
//! - a call through the client succeeds end to end, as itself and on
//!   behalf of a person; the holder still refuses what it does not grant,
//!   and a chain passed on that loops back.
//!
//! A regression looks like an apply that stores a version calling an op
//! the holder does not grant, a holder change that breaks a consumer
//! silently, or a client call that drops the person or the chain.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_peerc_`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use ordo_api::{router, ApiState, BoundParams, MemoryBus, MemoryReplayCache, OpExecutor, PageKey};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::ApplyOps;
use ordo_host::auth::AuthState;
use ordo_host::consumer_grants as cg;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::identity_keys::{IdentityKeyStore, IdentityKeys, IdentitySettings, ServiceUrl};
use ordo_host::metrics::Metrics;
use ordo_host::peer_client::{Acting, Method, PeerClient, PeerClientError};
use ordo_host::peer_door::PeerDoor;
use ordo_host::peer_ops::{self, PeerOps};
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::schema_apply::Program;
use ordo_host::versions::Versions;
use ordo_host::{FixtureExecutor, IdentityAccess, DEFAULT_SPACE};
use ordo_identity::clock::Clock;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::federation::form_encode;
use ordo_identity::peer_token;
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    Argon2idHasher, AuthenticatorId, CompleteRequest, PrincipalKind, ProviderRegistry, Timestamp,
    TokenKey,
};
use ordo_identity_service::config::{DATABASE_URL_ENV, DATA_DIR_ENV, ORIGIN_ENV};
use ordo_identity_service::deployments::{Registration, SignUp};
use ordo_identity_service::people::{self, Actor};
use ordo_identity_service::sessions::{self, NewSession, SessionKind};
use ordo_identity_service::{BootOptions, IdentityConfig, Service};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const NOW: u64 = 1_800_000_000;
const MASTER: [u8; 32] = [33; 32];
const VERIFIER: &str = "a-verifier-of-at-least-forty-three-characters-0123456789";
const HOST: Actor<'static> = Actor {
    id: "unix:test",
    client: "host",
};
/// The consumer's own space.
const HOME: &str = "home";

/// The holder's program: the conformance fixture, one entity and its ops,
/// and the grant `viewer` is tied to.
const HOLDER: &str = r#"
entity journal {
  title: text
}

op addEntry($title: text) =
  set journal { title: $title }

op archiveEntry($id: id) =
  del journal { id: $id }

grant note_viewer as itself {
  std.journal.list
  addEntry
}
"#;

/// The same, at its next version, no longer granting `addEntry`.
const HOLDER_NEXT: &str = r#"
entity journal {
  title: text
}

op addEntry($title: text) =
  set journal { title: $title }

op archiveEntry($id: id) =
  del journal { id: $id }

grant note_viewer as itself {
  std.journal.list
}
"#;

/// The consumer's program at `version`, calling `ops` of `notes`.
fn consumer_source(version: &str, ops: &[&str]) -> String {
    format!(
        "domain viewer @ {version}\n\nentity bookmark {{\n  title: text\n}}\n\n\
         op bookmarks unbounded =\n  bookmark[title]\n\npeer notes {{\n{}\n}}\n",
        ops.iter()
            .map(|o| format!("  {o}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn key_of(handle: &str) -> SigningKey {
    peer_token::consumer_key(&MASTER, handle)
}

fn param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name)
            .then(|| ordo_identity::federation::percent_decode(v))
            .flatten()
    })
}

// ───────────────────────── the identity service ─────────────────────────

struct Ids {
    svc: Service,
    issuer: String,
    url: String,
    /// The one clock of the service, the holder and the consumer.
    clock: Clock,
}

fn callback(issuer: &str, handle: &str) -> String {
    format!("{issuer}/{handle}/auth/callback/ids")
}

impl Ids {
    /// A service with `notes`, `viewer` and `stranger` registered, open.
    /// `stranger` has a consumer key; `viewer` registers its own.
    async fn start(name: &str) -> Ids {
        let c = PoolConfig::from_env();
        let db = format!("ordo_peerc_ids_{name}");
        ordo_exec::drop_database(&c, &db).await.expect("drop");
        ordo_exec::create_database(&c, &db).await.expect("create");
        let mut url = format!(
            "host={} port={} user={} dbname={db}",
            c.host, c.port, c.user
        );
        if let Some(p) = &c.password {
            url.push_str(&format!(" password={p}"));
        }
        let dir = std::env::temp_dir().join(format!("ordo-peerc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().unwrap().port();
        let issuer = format!("http://localhost:{port}");
        let vars: HashMap<&str, String> = HashMap::from([
            (DATABASE_URL_ENV, url),
            (ORIGIN_ENV, issuer.clone()),
            (DATA_DIR_ENV, dir.display().to_string()),
        ]);
        let cfg = IdentityConfig::from_vars(&|k| vars.get(k).cloned()).expect("settings");
        let clock = Clock::manual(NOW);
        let svc = Service::boot(
            cfg,
            BootOptions {
                clock: clock.clone(),
                hasher: Arc::new(Argon2idHasher::weak_for_tests()),
            },
        )
        .await
        .expect("boot");
        for handle in ["notes", "viewer", "stranger"] {
            svc.register_deployment(
                HOST,
                &Registration {
                    handle: handle.into(),
                    client_origins: vec![issuer.clone()],
                    return_urls: vec![callback(&issuer, handle)],
                    sign_up: SignUp::Open,
                    consumer_key: (handle == "stranger")
                        .then(|| peer_token::public_text(&key_of(handle).verifying_key())),
                },
            )
            .await
            .expect("registered");
        }
        let app = svc
            .router()
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ids {
            svc,
            issuer,
            url: format!("http://127.0.0.1:{port}"),
            clock,
        }
    }

    async fn browser(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Option<String>, Value, Option<String>) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", &self.issuer);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        let mut req = match body {
            Some(v) => req
                .header("content-type", "application/json")
                .body(Body::from(v.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [198, 51, 100, 9],
                40000,
            ))));
        let res = self.svc.router().oneshot(req).await.unwrap();
        let status = res.status();
        let header = |n: &str| {
            res.headers()
                .get(n)
                .map(|v| v.to_str().unwrap().to_string())
        };
        let (location, set) = (header("location"), header("set-cookie"));
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, location, v, set)
    }

    /// Open sign-up into `deployment`: the session cookie and the person.
    async fn sign_up(&self, deployment: &str, name: &str, seed: u8) -> (String, String) {
        let (s, _, opts, _) = self
            .browser(
                "POST",
                "/sign-up/begin",
                None,
                Some(json!({ "deployment": deployment })),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{opts}");
        let challenge = opts["publicKey"]["challenge"].as_str().unwrap().to_string();
        let auth = TestAuthenticator {
            key: TestKey::es256(seed),
            credential_id: vec![seed; 16],
            rp_id: "localhost".to_string(),
            origin: self.issuer.clone(),
            ..TestAuthenticator::es256()
        };
        let CompleteRequest::PasskeyRegistration {
            client_data_json,
            attestation_object,
            ..
        } = auth.register(&challenge)
        else {
            unreachable!()
        };
        let body = json!({
            "challenge": challenge,
            "clientDataJSON": b64url_encode(&client_data_json),
            "attestationObject": b64url_encode(&attestation_object),
            "deployment": deployment,
            "displayName": name,
        });
        let (s, _, done, set) = self
            .browser("POST", "/sign-up/finish", None, Some(body))
            .await;
        assert_eq!(s, StatusCode::OK, "{done}");
        let cookie = set.unwrap().split(';').next().unwrap().to_string();
        (cookie, done["person"].as_str().unwrap().to_string())
    }

    /// The ID token `client` holds for the browser `cookie`, through the
    /// authorization code flow.
    async fn id_token(&self, client: &str, cookie: &str) -> String {
        let challenge = b64url_encode(&Sha256::digest(VERIFIER.as_bytes()));
        let redirect = callback(&self.issuer, client);
        let query = form_encode(&[
            ("response_type", "code"),
            ("client_id", client),
            ("redirect_uri", &redirect),
            ("scope", "openid"),
            ("nonce", "n-1"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ]);
        let (_, location, _, _) = self
            .browser(
                "GET",
                &format!("/oidc/authorize?{query}"),
                Some(cookie),
                None,
            )
            .await;
        let location = location.expect("a redirect");
        let code = param(&location, "code").unwrap_or_else(|| panic!("a code: {location}"));
        let req = Request::builder()
            .method("POST")
            .uri("/oidc/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(form_encode(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect),
                ("client_id", client),
                ("code_verifier", VERIFIER),
            ])))
            .unwrap();
        let res = self.svc.router().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        v["id_token"].as_str().unwrap().to_string()
    }

    /// The person signed in at `cookie` joins `deployment`.
    async fn join(&self, cookie: &str, deployment: &str) {
        let (s, _, v, _) = self
            .browser(
                "POST",
                "/sign-up/join",
                Some(cookie),
                Some(json!({ "deployment": deployment })),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }

    /// A token for an admin of `deployment`, touched a moment ago: what
    /// the deployment's own admin holds.
    async fn admin_token(&self, deployment: &str) -> String {
        let pool = self.svc.store().pool();
        let tx = pool.begin().await.unwrap();
        let by = Actor {
            id: "boot",
            client: "",
        };
        let p = people::add(&tx, self.svc.handle(), self.svc.keys(), "Ann", by, NOW)
            .await
            .unwrap();
        let s = sessions::open(
            &tx,
            self.svc.handle(),
            self.svc.keys(),
            self.svc.lifetimes(),
            NewSession {
                subject: &p.id,
                kind: SessionKind::Browser,
                client: "console",
                touched_at: Some(NOW),
                must_enrol: false,
            },
            NOW,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        self.svc.add_member(HOST, deployment, &p.id).await.unwrap();
        let cookie = format!("ordo-identity={}", s.secret);
        let (st, _, v, _) = self
            .browser(
                "POST",
                "/token",
                Some(&cookie),
                Some(json!({ "aud": format!("ordo-deployment:{deployment}") })),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        v["token"].as_str().unwrap().to_string()
    }
}

// ─────────────────────────────── the holder ───────────────────────────────

struct Holder {
    address: String,
    pool: Pool,
}

impl Holder {
    /// `notes`'s door on a loopback socket, over its own database, with
    /// `ada` (the service's person id) a member of its own space.
    async fn start(name: &str, ids: &Ids, ada: &str) -> Holder {
        Holder::start_with(name, ids, ada, HOLDER).await
    }

    /// [`Holder::start`], with `holder` as its program after the fixture.
    async fn start_with(name: &str, ids: &Ids, ada: &str, holder: &str) -> Holder {
        let source = format!("{FIXTURE}\n{holder}");
        let f = Arc::new(Fixture::from_source(&source, SEED).expect("the holder compiles"));
        let pg = Pg::scoped(&format!("ordo_peerc_holder_{name}"));
        f.reset(&pg).expect("seeded");
        let pool = Pool::connect(PoolConfig {
            db: pg.db.clone(),
            size: 3,
            ..PoolConfig::from_env()
        })
        .await
        .expect("a pool");
        ordo_host::principal_directory::ensure_table(&pool)
            .await
            .expect("a directory");

        // Version 1.0.0 serving, with its document and expanded grants, as
        // an apply leaves them.
        let program = Program::compile(&source).expect("compiles");
        let ops: Vec<String> = cg::program_ops(&program.doc);
        let grants: Vec<cg::Expanded> = f
            .schema
            .grants
            .iter()
            .map(|g| cg::expand(g, &ops).expect("expands").grant)
            .collect();
        cg::store(&pool, "1.0.0", &grants).await.expect("stored");
        ordo_exec::registry::ensure_tables(&pool)
            .await
            .expect("registry");
        pool.rows(&format!(
            "INSERT INTO \"$artifacts\" (\"version\", \"artifact\", \"source\", \"ddl\", \
             \"applied_at\", \"actor\") VALUES ('1.0.0', {}, {}, '', 0, 'test'); \
             INSERT INTO \"$activations\" (\"version\", \"at\", \"actor\") \
             VALUES ('1.0.0', 0, 'test')",
            ordo_exec::sql_lit(&ordo_plan::artifact::to_bytes(&program.doc)),
            ordo_exec::sql_lit(&source),
        ))
        .await
        .expect("active");

        // Ada: linked to the connection, a member of the space.
        let store = Arc::new(
            PgDirectoryStore::open(PoolConfig {
                db: pg.db.clone(),
                size: 3,
                ..PoolConfig::from_env()
            })
            .expect("directory store"),
        );
        let auth = Arc::new(AuthState::new(
            ProviderRegistry::new(),
            Directory::with_store(Provisioning::AutoOnboard, store),
            TokenKey::from_secret(b"peer-consumer-holder"),
        ));
        let principal = {
            let mut dir = auth.lock_directory();
            let p = dir
                .create_principal(PrincipalKind::Human, "Ada", Timestamp(NOW))
                .expect("a principal");
            dir.link_first(
                &p,
                &AuthenticatorId(format!("oidc:ids:{ada}")),
                Timestamp(NOW),
            )
            .expect("linked");
            p
        };
        pg.exec(&format!(
            "INSERT INTO members (\"$space\", id, principal, \"displayName\", kind, \
             \"memberCount\", role, status, joined) VALUES ('{DEFAULT_SPACE}', 'mem_ada', \
             '{}', 'Ada', 'principal', 1, 'manager', 'active', '2026-11-26'); \
             INSERT INTO journal (\"$space\", id, title) VALUES ('{DEFAULT_SPACE}', 'j_1', \
             'first');",
            principal.0
        ))
        .expect("seed");

        let now: Arc<dyn Fn() -> u64 + Send + Sync> = {
            let clock = ids.clock.clone();
            Arc::new(move || clock.now())
        };
        let keys = Arc::new(IdentityKeys::new(
            IdentitySettings {
                url: ServiceUrl::parse(&ids.url, false).unwrap(),
                issuer: ids.issuer.clone(),
                audience: ordo_identity::deployment_token::audience("notes"),
            },
            IdentityKeyStore::memory(),
            now.clone(),
        ));
        keys.boot().await.expect("the service's keys");
        let vars = holder_vars(ids);
        let peer = Arc::new(PeerDoor::new(
            keys.clone(),
            Some(auth),
            Some(pool.clone()),
            Arc::new(move |k: &str| vars.get(k).cloned()),
            now.clone(),
        ));
        assert!(peer.settings().is_ok(), "{:?}", peer.settings());
        let engine = ordo_host::deployment_engine(&f, &pg).expect("an engine");
        let door = Arc::new(
            IdentityAccess::new(
                engine,
                TokenKey::from_secret(b"peer-consumer-holder"),
                Box::new(|| NOW),
                Arc::new(Metrics::new()),
            )
            .with_console_home(DEFAULT_SPACE)
            .with_identity_keys(keys)
            .with_peer(peer.clone()),
        );

        // The platform ops: the two another deployment calls.
        let peer_ops = PeerOps::new(pool.clone(), DEFAULT_SPACE, now.clone());
        let _ = peer_ops.door.set(peer);
        let registry = Arc::new(PgGrantRegistry::new(pool.clone()));
        registry.ensure_table().await.expect("platform table");
        let ops = Arc::new(
            PlatformOps::new(
                Arc::new(ordo_host::engine_from_members(&f, &[]).expect("engine")),
                registry,
                now.clone(),
            )
            .with_peer(peer_ops),
        );
        let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
            "peer-holder",
            f.clone(),
            ordo_exec::Executor {
                pool: pool.clone(),
                schema: f.schema.clone(),
                registry: f.registry.clone(),
                keys: f.keys.clone(),
                space: ordo_conformance::fixture_space(),
            },
            platform_ops::specs(),
        )
        .expect("platform specs");
        let executor: Arc<dyn OpExecutor> = Arc::new(PlatformExecutor::new(
            Arc::new(FixtureExecutor {
                spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
                pg: pg.clone(),
                page_size: 50,
            }),
            ops,
        ));
        let state = ApiState {
            registry: deployment.handle(),
            executor,
            access: door,
            quota: Arc::new(ordo_api::NoQuota),
            bus: Arc::new(MemoryBus::new()),
            replay: Arc::new(MemoryReplayCache::new()),
            attest: Arc::new(ordo_api::attest::NoAttestation),
            page_key: PageKey::from_secret(b"peer-consumer"),
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
        Holder {
            address: format!("http://{addr}"),
            pool,
        }
    }
}

fn holder_vars(ids: &Ids) -> HashMap<String, String> {
    HashMap::from([
        ("ORDO_AUTH_METHODS".into(), "passkey,ids".into()),
        ("ORDO_AUTH_IDS_KIND".into(), "oidc".into()),
        ("ORDO_AUTH_IDS_ISSUER".into(), ids.issuer.clone()),
        ("ORDO_IDENTITY_ORIGIN".into(), ids.issuer.clone()),
        ("ORDO_IDENTITY_DEPLOYMENT".into(), "notes".into()),
        ("ORDO_CONSUMER_NOTE_VIEWER_CLIENT".into(), "viewer".into()),
    ])
}

// ─────────────────────────────── the consumer ───────────────────────────────

/// The settings a consumer node reads.
fn consumer_vars(ids: &Ids, holder: &Holder, handle: &str) -> HashMap<String, String> {
    HashMap::from([
        ("ORDO_IDENTITY_URL".into(), ids.url.clone()),
        ("ORDO_IDENTITY_ORIGIN".into(), ids.issuer.clone()),
        ("ORDO_IDENTITY_DEPLOYMENT".into(), handle.into()),
        ("ORDO_PEER_NOTES_URL".into(), holder.address.clone()),
        ("ORDO_PEER_NOTES_SPACE".into(), DEFAULT_SPACE.into()),
    ])
}

/// The consumer key of label version `version`: the first is `key_of`.
fn key_version_of(handle: &str, version: u32) -> SigningKey {
    let mut seed = MASTER;
    seed[0] = seed[0].wrapping_add(version as u8 - 1);
    peer_token::consumer_key(&seed, handle)
}

/// One node of the consumer: a client that keeps its key's version in
/// `pool`, as the runtime wires it. Several nodes over one pool are
/// several nodes of one deployment.
async fn node(
    ids: &Ids,
    holder: &Holder,
    handle: &str,
    pool: &Pool,
    now: &Arc<dyn Fn() -> u64 + Send + Sync>,
) -> Arc<PeerClient> {
    let vars = consumer_vars(ids, holder, handle);
    let client = Arc::new(PeerClient::new(
        key_of(handle),
        Arc::new(move |k: &str| vars.get(k).cloned()),
        now.clone(),
    ));
    let name = handle.to_string();
    client
        .with_custody(
            Arc::new(ordo_host::peer_key::KeyStore::new(pool.clone())),
            Arc::new(move |v| Ok(key_version_of(&name, v))),
        )
        .await
        .expect("the key's version is read");
    client
}

struct Consumer {
    apply: Arc<ApplyOps>,
    peer: Arc<PeerOps>,
    client: Arc<PeerClient>,
    pool: Pool,
}

impl Consumer {
    /// The deployment `handle` over its own database, its client reaching
    /// the holder at `holder` through `ids`.
    async fn start(name: &str, handle: &str, ids: &Ids, holder: &Holder) -> Consumer {
        let pg = Pg::scoped(&format!("ordo_peerc_{handle}_{name}"));
        let pool = Pool::connect(PoolConfig {
            db: pg.db.clone(),
            size: 3,
            ..PoolConfig::from_env()
        })
        .await
        .expect("a pool");
        ordo_host::tenants::ensure_table(&pool)
            .await
            .expect("$spaces");
        ordo_host::rollouts::Rollouts::on(&pool)
            .set_setting("rollout.strategy", Some("all"), "test", "at once", 0)
            .await
            .expect("rollout.strategy all");
        let now: Arc<dyn Fn() -> u64 + Send + Sync> = {
            let clock = ids.clock.clone();
            Arc::new(move || clock.now())
        };
        let client = node(ids, holder, handle, &pool, &now).await;
        let apply = ApplyOps::new(pool.clone(), Arc::new(SinglePool::new(pool.clone())), HOME);
        let _ = apply.peer_client.set(client.clone());
        let peer = PeerOps::new(pool.clone(), HOME, now);
        let _ = peer.client.set(client.clone());
        Consumer {
            apply,
            peer,
            client,
            pool,
        }
    }

    async fn apply(&self, source: &str) -> Result<Value, String> {
        let b = BoundParams(json!({ "program": source }).as_object().unwrap().clone());
        self.apply
            .apply(HOME, &b, "ann")
            .await
            .map(|r| r.rows[0].clone())
            .map_err(|e| e.message)
    }

    async fn stored(&self) -> Vec<String> {
        let v = Versions::on(&self.pool);
        v.ensure().await.expect("tables");
        v.stored().await.unwrap().into_iter().map(|s| s.0).collect()
    }
}

async fn stage(name: &str) -> (Ids, Holder, Consumer, String) {
    let ids = Ids::start(name).await;
    let (cookie, ada) = ids.sign_up("viewer", "Ada", 81).await;
    let holder = Holder::start(name, &ids, &ada).await;
    let viewer = Consumer::start(name, "viewer", &ids, &holder).await;
    (ids, holder, viewer, cookie)
}

/// Register `viewer`'s key through its own door op, with its admin's token.
async fn register(ids: &Ids, viewer: &Consumer) {
    let token = ids.admin_token("viewer").await;
    let b = BoundParams(json!({ "token": token }).as_object().unwrap().clone());
    let out = viewer
        .peer
        .execute(peer_ops::REGISTER_KEY, HOME, &b, "ann")
        .await
        .expect("registered");
    assert_eq!(out.rows[0]["handle"], "viewer");
    assert_eq!(
        out.rows[0]["consumerKey"],
        peer_token::public_text(&key_of("viewer").verifying_key())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_consumer_registers_its_key_and_its_apply_is_checked_against_the_holder() {
    let (ids, holder, viewer, _cookie) = stage("apply").await;

    // Before its key is registered, the service does not take it: the
    // apply is refused naming the op that registers it.
    let e = viewer
        .apply(&consumer_source("1.0.0", &["std.journal.list"]))
        .await
        .expect_err("no key yet");
    assert!(e.contains("peer_unreachable in `peer notes`"), "{e}");
    assert!(e.contains("std.peer.registerKey"), "{e}");
    // A token for another deployment's admin does not register it.
    let other = ids.admin_token("notes").await;
    let b = BoundParams(json!({ "token": other }).as_object().unwrap().clone());
    let refused = viewer
        .peer
        .execute(peer_ops::REGISTER_KEY, HOME, &b, "ann")
        .await
        .expect_err("not viewer's admin");
    assert!(
        refused
            .message
            .contains("did not register the consumer key"),
        "{}",
        refused.message
    );
    register(&ids, &viewer).await;

    // An op the holder does not grant: refused by name, nothing stored, and
    // nothing recorded with the holder.
    let e = viewer
        .apply(&consumer_source(
            "1.0.0",
            &["std.journal.list", "archiveEntry"],
        ))
        .await
        .expect_err("archiveEntry is not granted");
    assert!(e.starts_with("peer_refused: 1 problem"), "{e}");
    assert!(
        e.contains("peer_op_not_granted in `peer notes`: `archiveEntry` is not granted"),
        "{e}"
    );
    assert!(e.contains("remove the line `archiveEntry`"), "{e}");
    assert!(viewer.stored().await.is_empty());
    assert!(peer_ops::recorded(&holder.pool).await.unwrap().is_empty());

    // Granted: the apply passes, shows how each op is granted, and the
    // holder records what this version calls.
    let report = viewer
        .apply(&consumer_source("1.0.0", &["std.journal.list", "addEntry"]))
        .await
        .expect("granted");
    let notes = report["notes"].to_string();
    assert!(
        notes.contains("peer        notes: 2 op(s) granted: std.journal.list (read"),
        "{notes}"
    );
    assert!(notes.contains("addEntry (write"), "{notes}");
    assert_eq!(viewer.stored().await, ["1.0.0"]);
    let uses = peer_ops::recorded(&holder.pool).await.unwrap();
    assert_eq!(uses.len(), 1, "{uses:?}");
    assert_eq!(
        (uses[0].consumer.as_str(), uses[0].version.as_str()),
        ("viewer", "1.0.0")
    );
    assert_eq!(uses[0].ops, ["std.journal.list", "addEntry"]);

    // The holder's plan of a version that stops granting `addEntry` names
    // the consumer it breaks, and warns: nothing is refused.
    let next = Program::compile(&format!("{FIXTURE}\n{HOLDER_NEXT}")).expect("compiles");
    let vars = holder_vars(&ids);
    let plan = cg::plan_with(&holder.pool, &next.schema, &next.doc, &|k| {
        vars.get(k).cloned()
    })
    .await
    .expect("planned");
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    let warned: Vec<&String> = plan
        .notes
        .iter()
        .filter(|n| n.starts_with("warning     breaks"))
        .collect();
    assert_eq!(warned.len(), 1, "{:?}", plan.notes);
    assert!(
        warned[0].contains("note_viewer calls addEntry (consumer `viewer`, its version 1.0.0)"),
        "{}",
        warned[0]
    );
    assert!(warned[0].contains("This apply goes on"), "{}", warned[0]);
    assert!(
        warned[0].contains("keep `addEntry` in `grant note_viewer as itself`"),
        "{}",
        warned[0]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_the_holder_grants_nothing_is_refused_by_name() {
    let (ids, holder, _viewer, _cookie) = stage("stranger").await;
    let stranger = Consumer::start("stranger", "stranger", &ids, &holder).await;
    let e = stranger
        .apply(&consumer_source("1.0.0", &["std.journal.list"]))
        .await
        .expect_err("not a consumer of notes");
    assert!(e.contains("peer_unreachable in `peer notes`"), "{e}");
    assert!(
        e.contains("`stranger` is not a consumer of this deployment"),
        "{e}"
    );
    assert!(e.contains("ORDO_CONSUMER_<NAME>_CLIENT=stranger"), "{e}");
    assert!(stranger.stored().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_through_the_client_runs_as_itself_and_on_behalf_of_a_person() {
    let (ids, _holder, viewer, cookie) = stage("calls").await;
    register(&ids, &viewer).await;
    let c = &viewer.client;

    // As itself: a write the grant opens.
    let got = c
        .call(
            "notes",
            "addEntry",
            Method::Write,
            &json!({ "title": "from viewer" }),
            Acting::AsItself,
            &[],
        )
        .await
        .expect("written as itself");
    assert!(got["rows"].is_array(), "{got}");

    // On behalf of Ada: a read, under the holder's own rules for her,
    // seeing the row written above.
    let id_token = ids.id_token("viewer", &cookie).await;
    let got = c
        .call(
            "notes",
            "std.journal.list",
            Method::Read,
            &json!({}),
            Acting::OnBehalf {
                id_token: &id_token,
            },
            &[],
        )
        .await
        .expect("read on behalf of Ada");
    let titles: Vec<&str> = got["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["title"].as_str())
        .collect();
    assert!(
        titles.contains(&"first") && titles.contains(&"from viewer"),
        "{got}"
    );

    // The grant tells the consumer each op's kind, which decides the method.
    let grant = c.grant("notes").await.expect("the grant");
    assert_eq!(grant.consumer, "viewer");
    assert_eq!(grant.kind("std.journal.list"), Some(Method::Read));
    assert_eq!(grant.kind("addEntry"), Some(Method::Write));
    assert_eq!(grant.kind("archiveEntry"), None, "not granted, not shown");

    // The holder still decides: an op its grant does not open.
    let e = c
        .call(
            "notes",
            "archiveEntry",
            Method::Delete,
            &json!({ "id": "j_1" }),
            Acting::OnBehalf {
                id_token: &id_token,
            },
            &[],
        )
        .await
        .expect_err("not granted");
    let PeerClientError::HolderRefused {
        status, message, ..
    } = &e
    else {
        panic!("{e}")
    };
    assert_eq!(*status, 403, "{e}");
    assert!(message.contains("add the line `archiveEntry`"), "{e}");

    // The chain of the call being served is passed on: one that already
    // came through `notes` loops, and `notes` refuses it.
    let e = c
        .call(
            "notes",
            "std.journal.list",
            Method::Read,
            &json!({}),
            Acting::AsItself,
            &["notes".to_string()],
        )
        .await
        .expect_err("a loop");
    assert!(
        e.to_string()
            .contains("already passed through `notes` (chain notes → viewer → notes)"),
        "{e}"
    );
}

/// `grant provider ids` (step G6): every deployment the identity service
/// vouches for, on behalf of a person, and never as itself.
fn holder_with_provider() -> String {
    format!("{HOLDER}\ngrant provider ids {{\n  std.journal.list\n}}\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_grant_covers_an_unnamed_consumer_for_a_person_and_never_as_itself() {
    let ids = Ids::start("provider").await;
    let (cookie, ada) = ids.sign_up("viewer", "Ada", 82).await;
    ids.join(&cookie, "stranger").await;
    let source = holder_with_provider();
    let holder = Holder::start_with("provider", &ids, &ada, &source).await;
    // `stranger` is registered at the service and named in no grant.
    let stranger = Consumer::start("provider", "stranger", &ids, &holder).await;

    // Its apply reads the provider grant: an op outside it is refused by
    // name, and nothing is stored.
    let e = stranger
        .apply(&consumer_source("1.0.0", &["std.journal.list", "addEntry"]))
        .await
        .expect_err("addEntry is `note_viewer`'s");
    assert!(
        e.contains("peer_op_not_granted in `peer notes`: `addEntry` is not granted"),
        "{e}"
    );
    assert!(e.contains("(`stranger`)"), "{e}");
    assert!(stranger.stored().await.is_empty());

    // What it grants passes, shown as the provider grant, and the holder
    // records the use.
    let report = stranger
        .apply(&consumer_source("1.0.0", &["std.journal.list"]))
        .await
        .expect("granted by the provider grant");
    let notes = report["notes"].to_string();
    assert!(
        notes.contains("std.journal.list (read, grant provider ids (on behalf of a person))"),
        "{notes}"
    );
    assert_eq!(stranger.stored().await, ["1.0.0"]);
    let uses = peer_ops::recorded(&holder.pool).await.unwrap();
    assert_eq!(uses.len(), 1, "{uses:?}");
    assert_eq!(uses[0].consumer, "stranger");

    // On behalf of Ada: the read runs under the holder's own rules for her.
    let c = &stranger.client;
    let id_token = ids.id_token("stranger", &cookie).await;
    let got = c
        .call(
            "notes",
            "std.journal.list",
            Method::Read,
            &json!({}),
            Acting::OnBehalf {
                id_token: &id_token,
            },
            &[],
        )
        .await
        .expect("read on behalf of Ada");
    assert_eq!(got["rows"][0]["title"], "first", "{got}");

    // As itself: refused, naming the setting that would tie a grant.
    let e = c
        .call(
            "notes",
            "std.journal.list",
            Method::Read,
            &json!({}),
            Acting::AsItself,
            &[],
        )
        .await
        .expect_err("a provider grant is never as itself");
    let PeerClientError::HolderRefused {
        status, message, ..
    } = &e
    else {
        panic!("{e}")
    };
    assert_eq!(*status, 403, "{e}");
    assert!(
        message.contains("`stranger` is not a consumer of this deployment"),
        "{message}"
    );
    assert!(
        message.contains("a `grant provider` covers calls on behalf of a person only"),
        "{message}"
    );
    assert!(
        message.contains("ORDO_CONSUMER_<NAME>_CLIENT=stranger"),
        "{message}"
    );

    // The holder's plan shows the provider grant and whom it reaches; one
    // that drops it names the consumer it breaks, and warns.
    let vars = holder_vars(&ids);
    let program = Program::compile(&format!("{FIXTURE}\n{source}")).expect("compiles");
    let plan = cg::plan_with(&holder.pool, &program.schema, &program.doc, &|k| {
        vars.get(k).cloned()
    })
    .await
    .expect("planned");
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    assert!(
        plan.notes.iter().any(
            |n| n.starts_with("grant       provider ids: 1 op(s), every deployment registered")
        ),
        "{:#?}",
        plan.notes
    );
    assert!(
        plan.notes
            .iter()
            .any(|n| n.contains("provider ids: any deployment registered at")
                && n.contains("a call as itself needs a grant of its own")),
        "{:#?}",
        plan.notes
    );
    let next = Program::compile(&format!("{FIXTURE}\n{HOLDER}")).expect("compiles");
    let plan = cg::plan_with(&holder.pool, &next.schema, &next.doc, &|k| {
        vars.get(k).cloned()
    })
    .await
    .expect("planned");
    let warned: Vec<&String> = plan
        .notes
        .iter()
        .filter(|n| n.starts_with("warning     breaks"))
        .collect();
    assert_eq!(warned.len(), 1, "{:?}", plan.notes);
    assert!(
        warned[0].contains("breaks stranger: stranger calls std.journal.list"),
        "{}",
        warned[0]
    );
}

// ─────────────────────────── the key's lifecycle (G9) ───────────────────────────

fn empty() -> BoundParams {
    BoundParams(serde_json::Map::new())
}

async fn status(viewer: &Consumer) -> Value {
    viewer
        .peer
        .execute(peer_ops::KEY_STATUS, HOME, &empty(), "ann")
        .await
        .expect("the key's status")
        .rows[0]
        .clone()
}

async fn rotate(
    ids: &Ids,
    viewer: &Consumer,
    deployment: &str,
    overlap: u64,
) -> Result<Value, String> {
    let token = ids.admin_token(deployment).await;
    let b = BoundParams(
        json!({ "token": token, "overlapSecs": overlap })
            .as_object()
            .unwrap()
            .clone(),
    );
    viewer
        .peer
        .execute(peer_ops::ROTATE_KEY, HOME, &b, "ann")
        .await
        .map(|r| r.rows[0].clone())
        .map_err(|e| e.message)
}

/// A read as itself through `client`: the grant opens `std.journal.list`.
async fn read_as_itself(client: &PeerClient) -> Result<Value, PeerClientError> {
    client
        .call(
            "notes",
            "std.journal.list",
            Method::Read,
            &json!({}),
            Acting::AsItself,
            &[],
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_key_is_registered_once_and_the_plan_says_so_across_restarts() {
    let (ids, holder, viewer, _cookie) = stage("keyonce").await;
    let source = consumer_source("1.0.0", &["std.journal.list"]);

    // A file with no `peer` block says nothing of the key.
    assert_eq!(
        viewer
            .apply
            .peer_key_note("domain viewer @ 1.0.0\n\nentity bookmark {\n  title: text\n}\n")
            .await,
        None
    );

    // Before: not registered, in the plan and in the status op. What
    // `ordo apply` then does is register it with the token it holds.
    let note = viewer.apply.peer_key_note(&source).await.expect("a note");
    assert!(note.contains("consumer key v1"), "{note}");
    assert!(note.contains("is not registered"), "{note}");
    assert!(note.contains("ordo apply"), "{note}");
    let before = status(&viewer).await;
    assert_eq!(before["registered"], false, "{before}");
    assert_eq!(before["version"], 1, "{before}");
    register(&ids, &viewer).await;

    // After: registered, the plan says so, and the apply goes through.
    let note = viewer.apply.peer_key_note(&source).await.expect("a note");
    assert!(
        note.contains("is registered at the identity service"),
        "{note}"
    );
    let after = status(&viewer).await;
    assert_eq!(after["registered"], true, "{after}");
    assert_eq!(after["consumerKey"], viewer.client.public_key());
    viewer.apply(&source).await.expect("applies");

    // A restart: a new node over the same database knows it, and signs
    // with the same key, with nothing registered again.
    let now: Arc<dyn Fn() -> u64 + Send + Sync> = {
        let clock = ids.clock.clone();
        Arc::new(move || clock.now())
    };
    let vars = consumer_vars(&ids, &holder, "viewer");
    let restarted = Arc::new(PeerClient::new(
        key_of("viewer"),
        Arc::new(move |k: &str| vars.get(k).cloned()),
        now,
    ));
    restarted
        .with_custody(
            Arc::new(ordo_host::peer_key::KeyStore::new(viewer.pool.clone())),
            Arc::new(|v| Ok(key_version_of("viewer", v))),
        )
        .await
        .expect("reads the version");
    let state = restarted.key_state().await.expect("the state");
    assert!(state.registered, "{state:?}");
    assert_eq!(state.public, viewer.client.public_key());
    read_as_itself(&restarted).await.expect("signs as before");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rotated_key_signs_the_old_one_works_for_the_overlap_and_is_refused_after() {
    let (ids, holder, viewer, _cookie) = stage("rotate").await;
    register(&ids, &viewer).await;
    let now: Arc<dyn Fn() -> u64 + Send + Sync> = {
        let clock = ids.clock.clone();
        Arc::new(move || clock.now())
    };
    // A second node of the deployment, not yet told of the rotation by
    // anything but the stored version.
    let other = node(&ids, &holder, "viewer", &viewer.pool, &now).await;
    // A node that never looks at the stored version again signs with the
    // key it had: what an old node does.
    let vars = consumer_vars(&ids, &holder, "viewer");
    let stale = PeerClient::new(
        key_of("viewer"),
        Arc::new(move |k: &str| vars.get(k).cloned()),
        now.clone(),
    );
    let v1 = viewer.client.public_key();
    read_as_itself(&stale).await.expect("v1 signs");

    // A token for another deployment's admin does not rotate it, and
    // nothing moves.
    let e = rotate(&ids, &viewer, "notes", 30)
        .await
        .expect_err("not viewer's admin");
    assert!(e.contains("did not register the consumer key"), "{e}");
    let kept = status(&viewer).await;
    assert_eq!(kept["version"], 1, "{kept}");
    assert_eq!(viewer.client.public_key(), v1);

    // Rotated: version 2, a new key, the old one named.
    let done = rotate(&ids, &viewer, "viewer", 30).await.expect("rotated");
    assert_eq!(done["version"], 2, "{done}");
    assert_eq!(done["previousConsumerKey"], v1, "{done}");
    assert_ne!(done["consumerKey"], v1, "{done}");
    assert_eq!(done["registered"], true, "{done}");
    let now_key = viewer.client.public_key();
    assert_eq!(done["consumerKey"], now_key);
    let s = status(&viewer).await;
    assert_eq!(
        (s["version"].clone(), s["registered"].clone()),
        (json!(2), json!(true))
    );

    // Inside the overlap both keys are taken: the rotated node, the node
    // that reads the stored version (no further than 15 seconds on), and
    // the node still on v1.
    read_as_itself(&viewer.client).await.expect("v2 signs");
    ids.clock.advance(20);
    read_as_itself(&other)
        .await
        .expect("the other node took v2");
    assert_eq!(other.public_key(), now_key);
    read_as_itself(&stale)
        .await
        .expect("v1 still signs in the overlap");

    // After it, v1 is refused by name, and v2 goes on.
    ids.clock.advance(20);
    let e = read_as_itself(&stale)
        .await
        .expect_err("v1 is dropped after the overlap");
    let PeerClientError::KeyNotRegistered { handle, message } = &e else {
        panic!("{e}")
    };
    assert_eq!(handle, "viewer", "{e}");
    assert!(message.contains("viewer"), "{e}");
    assert!(e.to_string().contains("std.peer.registerKey"), "{e}");
    read_as_itself(&viewer.client).await.expect("v2 goes on");
    read_as_itself(&other).await.expect("v2 goes on");

    // Persisted: a restart derives v2 from the stored version.
    let restarted = node(&ids, &holder, "viewer", &viewer.pool, &now).await;
    assert_eq!(restarted.key_version(), 2);
    assert_eq!(restarted.public_key(), now_key);
}

#[tokio::test(flavor = "multi_thread")]
async fn rotating_with_no_overlap_drops_the_old_key_at_once() {
    let (ids, holder, viewer, _cookie) = stage("rotate0").await;
    register(&ids, &viewer).await;
    let now: Arc<dyn Fn() -> u64 + Send + Sync> = {
        let clock = ids.clock.clone();
        Arc::new(move || clock.now())
    };
    let vars = consumer_vars(&ids, &holder, "viewer");
    let stale = PeerClient::new(
        key_of("viewer"),
        Arc::new(move |k: &str| vars.get(k).cloned()),
        now,
    );
    rotate(&ids, &viewer, "viewer", 0).await.expect("rotated");
    let e = read_as_itself(&stale)
        .await
        .expect_err("no overlap was asked for");
    assert!(matches!(e, PeerClientError::KeyNotRegistered { .. }), "{e}");
    read_as_itself(&viewer.client).await.expect("v2 signs");
}
