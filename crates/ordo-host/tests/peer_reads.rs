//! Peer entities served through the holder's door
//! (`plan-federation-grants.md` §4, step G5), against a real identity
//! service and 2 real deployments.
//!
//! `notes` is the holder: a door on a loopback socket over its own
//! database, holding `memo` and `secret`, granting `note_viewer`
//! `std.memo.list` on behalf of a person, tied to the consumer `viewer`.
//! Every call to its door that reads `memo` is counted.
//!
//! `viewer` is the consumer: its own database, `peer notes` with
//! `ORDO_PEER_NOTES_URL` set, externals over `notes.memo`, and a native
//! `bookmark` whose `note` refs one. Its reads run through its own
//! executor ([`FixtureExecutor`]), as the door runs them.
//!
//! What it proves:
//!
//! - a read of a peer entity is one call to the holder's `std.memo.list`,
//!   with the filter, sort, `take` and fields sent whole, and the holder's
//!   answer is the read's answer;
//! - 100 native rows joined to the peer through a ref make 1 call;
//! - `$caller` on a peer entity is the holder's: a guard `owner ==
//!   $caller` is decided by the holder for the person, and the call is
//!   made for that person past their 10-minute ID token;
//! - a holder that does not answer in `ORDO_PEER_TIMEOUT_SECS` refuses the
//!   op with `peer_unavailable`, naming the setting;
//! - a read the holder does not grant is refused with `peer_refused`,
//!   naming the grant line;
//!
//! A regression looks like one call per row, a filter run here over every
//! holder row, a `$caller` read through the peer's system tables, or a
//! holder outage that hangs or answers 500.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_peerr_`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use ordo_api::{
    router, ApiState, BoundParams, ErrorCode, ExecOutcome, MemoryBus, MemoryReplayCache,
    OpExecutor, OpSpec, PageKey,
};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_crypto::SpaceId;
use ordo_exec::route::PoolResolver;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::auth::AuthState;
use ordo_host::consumer_grants as cg;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::identity_keys::{IdentityKeyStore, IdentityKeys, IdentitySettings, ServiceUrl};
use ordo_host::metrics::Metrics;
use ordo_host::peer_client::PeerClient;
use ordo_host::peer_door::PeerDoor;
use ordo_host::peer_ops::PeerOps;
use ordo_host::peer_reads::DoorReads;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::schema_apply::Program;
use ordo_host::{FixtureExecutor, IdentityAccess, DEFAULT_SPACE};
use ordo_identity::clock::Clock;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::peer_token;
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    Argon2idHasher, AuthenticatorId, CompleteRequest, PrincipalKind, ProviderRegistry, Timestamp,
    TokenKey,
};
use ordo_identity_service::config::{DATABASE_URL_ENV, DATA_DIR_ENV, ORIGIN_ENV};
use ordo_identity_service::deployments::{Registration, SignUp};
use ordo_identity_service::{BootOptions, IdentityConfig, Service};
use serde_json::{json, Value};
use tower::ServiceExt;

const NOW: u64 = 1_800_000_000;
const MASTER: [u8; 32] = [44; 32];

/// The holder's program: 2 entities, one granted on behalf of a person.
const HOLDER: &str = r#"
entity memo {
  title: text
  owner: text
  rank: int
}

entity secret {
  body: text
}

entity diary {
  body: text sealed
  std sealed
}

op addDiary($body: text) =
  set diary { body: $body }

grant note_viewer {
  std.memo.list
  std.diary.list
}
"#;

/// The consumer's program, after the conformance fixture.
const CONSUMER: &str = r#"
peer notes {
  std.memo.list
}

external entries from notes.memo shared {
  id: text <- id
  title: text <- title
  owner: text <- owner
  rank: int <- rank
}

external mine from notes.memo shared {
  id: text <- id
  title: text <- title
  owner: text <- owner
  access read owner == $caller
}

external secrets from notes.secret shared {
  id: text <- id
  body: text <- body
}

entity bookmark {
  note -> entries
  label: text
}

op topEntries =
  entries(rank >= 2, owner == "mem_ada")[id, title, rank]
  sort -rank
  take 3

op myEntries =
  mine[id, title]
  sort title
  take 50

op marks =
  bookmark[label, title: note.title]
  sort label
  take 100

op marksNotFirst =
  bookmark(note.title != "memo 001")[label]
  sort label
  take 100

op allSecrets =
  secrets[id, body]
  take 10

external diaries from notes.diary shared {
  id: text <- id
  body: text <- body
}

op allDiaries =
  diaries[id, body]
  sort id
  take 10
"#;

fn key_of(handle: &str) -> SigningKey {
    peer_token::consumer_key(&MASTER, handle)
}

// ───────────────────────── the identity service ─────────────────────────

struct Ids {
    svc: Service,
    clock: Clock,
    issuer: String,
    url: String,
}

impl Ids {
    /// A service with `notes` and `viewer` registered, open, `viewer` with
    /// its consumer key.
    async fn start(name: &str) -> Ids {
        let c = PoolConfig::from_env();
        let db = format!("ordo_peerr_ids_{name}");
        ordo_exec::drop_database(&c, &db).await.expect("drop");
        ordo_exec::create_database(&c, &db).await.expect("create");
        let mut url = format!(
            "host={} port={} user={} dbname={db}",
            c.host, c.port, c.user
        );
        if let Some(p) = &c.password {
            url.push_str(&format!(" password={p}"));
        }
        let dir = std::env::temp_dir().join(format!("ordo-peerr-{name}-{}", std::process::id()));
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
        for handle in ["notes", "viewer"] {
            svc.register_deployment(
                ordo_identity_service::people::Actor {
                    id: "unix:test",
                    client: "host",
                },
                &Registration {
                    handle: handle.into(),
                    client_origins: vec![issuer.clone()],
                    return_urls: vec![format!("{issuer}/{handle}/auth/callback/ids")],
                    sign_up: SignUp::Open,
                    consumer_key: (handle == "viewer")
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
            clock,
            issuer,
            url: format!("http://127.0.0.1:{port}"),
        }
    }

    async fn browser(&self, path: &str, body: Value) -> (StatusCode, Value, Option<String>) {
        let mut req = Request::builder()
            .method("POST")
            .uri(path)
            .header("origin", &self.issuer)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [198, 51, 100, 9],
                40000,
            ))));
        let res = self.svc.router().oneshot(req).await.unwrap();
        let status = res.status();
        let set = res
            .headers()
            .get("set-cookie")
            .map(|v| v.to_str().unwrap().to_string());
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            set,
        )
    }

    /// Open sign-up into `deployment`: the person's id at the service.
    async fn sign_up(&self, deployment: &str, name: &str, seed: u8) -> String {
        let (s, opts, _) = self
            .browser("/sign-up/begin", json!({ "deployment": deployment }))
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
        let (s, done, _) = self
            .browser(
                "/sign-up/finish",
                json!({
                    "challenge": challenge,
                    "clientDataJSON": b64url_encode(&client_data_json),
                    "attestationObject": b64url_encode(&attestation_object),
                    "deployment": deployment,
                    "displayName": name,
                }),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{done}");
        done["person"].as_str().unwrap().to_string()
    }
}

// ─────────────────────────────── a deployment ───────────────────────────────

/// A person here: a principal linked to the identity service's `sub`, and
/// a member of the space.
async fn person(pg: &Pg, pool: &Pool, sub: &str, member: &str, name: &str) -> String {
    ordo_host::principal_directory::ensure_table(pool)
        .await
        .expect("a directory");
    let store = Arc::new(
        PgDirectoryStore::open(PoolConfig {
            db: pg.db.clone(),
            size: 2,
            ..PoolConfig::from_env()
        })
        .expect("directory store"),
    );
    let auth = AuthState::new(
        ProviderRegistry::new(),
        Directory::with_store(Provisioning::AutoOnboard, store),
        TokenKey::from_secret(b"peer-reads"),
    );
    let principal = {
        let mut dir = auth.lock_directory();
        let p = dir
            .create_principal(PrincipalKind::Human, name, Timestamp(NOW))
            .expect("a principal");
        dir.link_first(
            &p,
            &AuthenticatorId(format!("oidc:ids:{sub}")),
            Timestamp(NOW),
        )
        .expect("linked");
        p
    };
    pg.exec(&format!(
        "INSERT INTO members (\"$space\", id, principal, \"displayName\", kind, \
         \"memberCount\", role, status, joined) VALUES ('{DEFAULT_SPACE}', '{member}', \
         '{}', '{name}', 'principal', 1, 'manager', 'active', '2026-11-26')",
        principal.0
    ))
    .expect("a member");
    principal.0
}

fn executor_for(f: &Arc<Fixture>, pool: &Pool) -> ordo_exec::Executor {
    ordo_exec::Executor {
        pool: pool.clone(),
        schema: f.schema.clone(),
        registry: f.registry.clone(),
        keys: f.keys.clone(),
        space: ordo_conformance::fixture_space(),
    }
}

// ─────────────────────────────── the holder ───────────────────────────────

/// The `$refine` of each call to the holder's door that reads `memo`.
type Seen = Arc<Mutex<Vec<Value>>>;

struct Holder {
    address: String,
    memo_calls: Seen,
    /// Its own executor, for its own writes, and its database.
    executor: Arc<dyn OpExecutor>,
    specs: Vec<OpSpec>,
    pg: Pg,
}

async fn count_memo(
    axum::extract::State(seen): axum::extract::State<Seen>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.uri().path().ends_with("/op/std.memo.list") {
        let refine = req
            .uri()
            .query()
            .unwrap_or_default()
            .split('&')
            .find_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                let k = ordo_identity::federation::percent_decode(k)?;
                (k == "$refine")
                    .then(|| ordo_identity::federation::percent_decode(&v.replace('+', " ")))
                    .flatten()
            })
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        seen.lock().unwrap().push(refine);
    }
    next.run(req).await
}

impl Holder {
    /// `notes`'s door over its own database. Ada (`ada`, `mem_ada`) and Bob
    /// (`bob`, `mem_bob`) are members. 120 memos: `m001` to `m120`, Ada's
    /// but each 5th, which is Bob's; `rank` is the number mod 7.
    async fn start(name: &str, ids: &Ids, ada: &str, bob: &str) -> Holder {
        let source = format!("{FIXTURE}\n{HOLDER}");
        let f = Arc::new(Fixture::from_source(&source, SEED).expect("the holder compiles"));
        let pg = Pg::scoped(&format!("ordo_peerr_holder_{name}"));
        f.reset(&pg).expect("seeded");
        let pool = Pool::connect(PoolConfig {
            db: pg.db.clone(),
            size: 4,
            ..PoolConfig::from_env()
        })
        .await
        .expect("a pool");

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

        person(&pg, &pool, ada, "mem_ada", "Ada").await;
        person(&pg, &pool, bob, "mem_bob", "Bob").await;
        let mut rows = Vec::new();
        for i in 1..=120 {
            let owner = if i % 5 == 0 { "mem_bob" } else { "mem_ada" };
            rows.push(format!(
                "('{DEFAULT_SPACE}', 'm{i:03}', 'memo {i:03}', '{owner}', {})",
                i % 7
            ));
        }
        pg.exec(&format!(
            "INSERT INTO memo (\"$space\", id, title, owner, rank) VALUES {}; \
             INSERT INTO secret (\"$space\", id, body) VALUES ('{DEFAULT_SPACE}', 's1', 'x');",
            rows.join(", ")
        ))
        .expect("memos");

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
        let vars: HashMap<String, String> = HashMap::from([
            ("ORDO_AUTH_METHODS".into(), "passkey,ids".into()),
            ("ORDO_AUTH_IDS_KIND".into(), "oidc".into()),
            ("ORDO_AUTH_IDS_ISSUER".into(), ids.issuer.clone()),
            ("ORDO_IDENTITY_ORIGIN".into(), ids.issuer.clone()),
            ("ORDO_IDENTITY_DEPLOYMENT".into(), "notes".into()),
            ("ORDO_CONSUMER_NOTE_VIEWER_CLIENT".into(), "viewer".into()),
        ]);
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
            TokenKey::from_secret(b"peer-reads-holder"),
        ));
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
                TokenKey::from_secret(b"peer-reads-holder"),
                Box::new(|| NOW),
                Arc::new(Metrics::new()),
            )
            .with_console_home(DEFAULT_SPACE)
            .with_identity_keys(keys)
            .with_peer(peer.clone()),
        );
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
            executor_for(&f, &pool),
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
            executor: executor.clone(),
            access: door,
            quota: Arc::new(ordo_api::NoQuota),
            bus: Arc::new(MemoryBus::new()),
            replay: Arc::new(MemoryReplayCache::new()),
            attest: Arc::new(ordo_api::attest::NoAttestation),
            page_key: PageKey::from_secret(b"peer-reads"),
            sync: Default::default(),
            live_streams: Default::default(),
            refine_limits: Default::default(),
            public: Default::default(),
        };
        let memo_calls: Seen = Arc::default();
        let app = router(state).layer(axum::middleware::from_fn_with_state(
            memo_calls.clone(),
            count_memo,
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Holder {
            address: format!("http://{addr}"),
            memo_calls,
            executor,
            specs: ordo_host::build_specs(&f),
            pg,
        }
    }

    /// Run the holder's own write `op` as Ada, in its own space.
    async fn write(&self, op: &str, params: Value) {
        let spec = self
            .specs
            .iter()
            .find(|s| s.name == op)
            .unwrap_or_else(|| panic!("no op {op}"));
        let mut bound = params.as_object().unwrap().clone();
        bound.insert("$caller".into(), json!("mem_ada"));
        match self
            .executor
            .execute(spec, DEFAULT_SPACE, BoundParams(bound), None, None, None)
            .await
        {
            Ok(ExecOutcome::Ran(_)) => {}
            other => panic!("{op}: {other:?}"),
        }
    }

    fn calls(&self) -> usize {
        self.memo_calls.lock().unwrap().len()
    }

    /// The `$refine` the holder was sent, call by call.
    fn refines(&self) -> Vec<Value> {
        self.memo_calls.lock().unwrap().clone()
    }
}

// ─────────────────────────────── the consumer ───────────────────────────────

struct Consumer {
    exec: FixtureExecutor,
    specs: Vec<OpSpec>,
    /// The same program and database through `Executor::read_as`, which
    /// has no client to another deployment's door.
    plain: ordo_exec::Executor,
    /// The deployment's own database, held for as long as the consumer.
    _pg: Pg,
}

/// A tenant of `viewer` kept in a database of its own (step G7), Ada its
/// member as `mem_twin`.
const TWIN: &str = "spc_twin";

/// [`TWIN`] in its own database, every other space in the deployment's.
struct TwinApart {
    default: Pool,
    twin: Pool,
}

impl PoolResolver for TwinApart {
    fn resolve<'a>(&'a self, space: &'a SpaceId) -> ordo_exec::route::Resolving<'a> {
        let pool = if space.0 == TWIN {
            self.twin.clone()
        } else {
            self.default.clone()
        };
        Box::pin(async move { Ok(pool) })
    }

    fn targets<'a>(&'a self) -> ordo_exec::route::Targets<'a> {
        let (default, twin) = (self.default.clone(), self.twin.clone());
        Box::pin(async move {
            Ok(vec![
                ordo_exec::route::Target {
                    label: "default".into(),
                    pool: Ok(default),
                },
                ordo_exec::route::Target {
                    label: "twin".into(),
                    pool: Ok(twin),
                },
            ])
        })
    }
}

impl Consumer {
    /// `viewer` over its own database, its peer `notes` at `door`, Ada a
    /// member as `mem_ann` and 100 bookmarks, one on each of `m001` to
    /// `m100`.
    async fn start(name: &str, ids: &Ids, door: &str, ada: &str, extra: &[(&str, &str)]) -> Self {
        Consumer::start_with(name, ids, door, ada, extra, false).await
    }

    /// [`Consumer::start`], with [`TWIN`] in a database of its own when
    /// `twin` is set.
    async fn start_with(
        name: &str,
        ids: &Ids,
        door: &str,
        ada: &str,
        extra: &[(&str, &str)],
        twin: bool,
    ) -> Self {
        let source = format!("{FIXTURE}\n{CONSUMER}");
        let mut f = Fixture::from_source(&source, SEED).expect("the consumer compiles");
        let mut vars: HashMap<String, String> = HashMap::from([
            ("ORDO_IDENTITY_URL".into(), ids.url.clone()),
            ("ORDO_IDENTITY_ORIGIN".into(), ids.issuer.clone()),
            ("ORDO_IDENTITY_DEPLOYMENT".into(), "viewer".into()),
            ("ORDO_AUTH_METHODS".into(), "passkey,ids".into()),
            ("ORDO_AUTH_IDS_KIND".into(), "oidc".into()),
            ("ORDO_AUTH_IDS_ISSUER".into(), ids.issuer.clone()),
            ("ORDO_PEER_NOTES_SPACE".into(), DEFAULT_SPACE.into()),
            ("ORDO_PEER_NOTES_URL".into(), door.into()),
        ]);
        for (k, v) in extra {
            vars.insert(k.to_string(), v.to_string());
        }
        let lookup = vars.clone();
        ordo_exec::peer::resolve_with(&mut f.schema, |k| lookup.get(k).cloned())
            .expect("the peer resolves");
        let f = Arc::new(f);
        let pg = Pg::scoped(&format!("ordo_peerr_viewer_{name}"));
        f.reset(&pg).expect("seeded");
        let pool = Pool::connect(PoolConfig {
            db: pg.db.clone(),
            size: 4,
            ..PoolConfig::from_env()
        })
        .await
        .expect("a pool");
        let principal = person(&pg, &pool, ada, "mem_ann", "Ada").await;
        let marks: Vec<String> = (1..=100)
            .map(|i| format!("('{DEFAULT_SPACE}', 'b{i:03}', 'entr_m{i:03}', 'mark {i:03}')"))
            .collect();
        pg.exec(&format!(
            "INSERT INTO bookmark (\"$space\", id, note, label) VALUES {}",
            marks.join(", ")
        ))
        .expect("bookmarks");

        let now: Arc<dyn Fn() -> u64 + Send + Sync> = {
            let clock = ids.clock.clone();
            Arc::new(move || clock.now())
        };
        let settings: ordo_host::peer_client::Settings =
            Arc::new(move |k: &str| vars.get(k).cloned());
        let client = Arc::new(PeerClient::new(key_of("viewer"), settings.clone(), now));
        let deployment =
            ordo_host::deploy::Deployment::pinned("viewer", f.clone(), executor_for(&f, &pool));
        deployment.set_peer_reads(Arc::new(
            DoorReads::new(client, settings).with_directory(pool.clone()),
        ));
        let spaces = if twin {
            // The tenant: custody and its owner through the one entry
            // point, its rows in a database of its own. Ada's sign-in link
            // stays in the deployment's database, as a deployment keeps it.
            ordo_host::provision::provision_space(
                &pool,
                f.keys.as_ref(),
                TWIN,
                &ordo_host::provision::FoundingOwner {
                    member_id: "mem_twin",
                    principal: &principal,
                    display_name: "Ada",
                },
            )
            .await
            .expect("the tenant provisions");
            let twin_pg = Pg::scoped(&format!("ordo_peerr_twin_{name}"));
            f.reset(&twin_pg).expect("the tenant's database");
            let twin_pool = Pool::connect(PoolConfig {
                db: twin_pg.db.clone(),
                size: 3,
                ..PoolConfig::from_env()
            })
            .await
            .expect("a pool on the tenant's database");
            twin_pg
                .exec(&format!(
                    "INSERT INTO members (\"$space\", id, principal, \"displayName\", kind, \
                     \"memberCount\", role, status, joined) VALUES ('{TWIN}', 'mem_twin', \
                     '{principal}', 'Ada', 'principal', 1, 'manager', 'active', '2026-11-26')"
                ))
                .expect("Ada in the tenant");
            ordo_host::spaces::Spaces::routed(
                deployment,
                Arc::new(TwinApart {
                    default: pool.clone(),
                    twin: twin_pool,
                }),
                ordo_exec::witness::Witness::new(pool.clone()),
            )
        } else {
            ordo_host::spaces::Spaces::new(deployment)
        };
        Consumer {
            exec: FixtureExecutor {
                spaces,
                pg: pg.clone(),
                page_size: 100,
            },
            specs: ordo_host::build_specs(&f),
            plain: executor_for(&f, &pool),
            _pg: pg,
        }
    }

    /// Read `op` as `caller`: the rows, or the refusal.
    async fn read(&self, op: &str, caller: &str) -> Result<Vec<Value>, ordo_api::ExecError> {
        self.read_in(op, DEFAULT_SPACE, caller).await
    }

    /// Read `op` in `space` as `caller`.
    async fn read_in(
        &self,
        op: &str,
        space: &str,
        caller: &str,
    ) -> Result<Vec<Value>, ordo_api::ExecError> {
        let spec = self
            .specs
            .iter()
            .find(|s| s.name == op)
            .unwrap_or_else(|| panic!("no op {op}"));
        let bound = BoundParams(json!({ "$caller": caller }).as_object().unwrap().clone());
        match self
            .exec
            .execute(spec, space, bound, None, None, None)
            .await?
        {
            ExecOutcome::Ran(r) => Ok(r.rows),
            other => panic!("{other:?}"),
        }
    }
}

async fn stage(name: &str) -> (Ids, Holder, Consumer) {
    let ids = Ids::start(name).await;
    let ada = ids.sign_up("viewer", "Ada", 91).await;
    let bob = ids.sign_up("notes", "Bob", 92).await;
    let holder = Holder::start(name, &ids, &ada, &bob).await;
    let viewer = Consumer::start(name, &ids, &holder.address, &ada, &[]).await;
    (ids, holder, viewer)
}

fn titles(rows: &[Value], field: &str) -> Vec<String> {
    rows.iter()
        .map(|r| r[field].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_read_sends_its_filter_sort_and_take_whole_in_one_call() {
    let (_ids, holder, viewer) = stage("pushdown").await;
    let rows = viewer
        .read("topEntries", "mem_ann")
        .await
        .expect("read through the holder's door");
    // rank >= 2 and Ada's, highest rank first, then the id: rank 6 is
    // m006, m013, m027 … (m020 and m034 are Bob's).
    assert_eq!(
        titles(&rows, "title"),
        ["memo 006", "memo 013", "memo 027"],
        "{rows:?}"
    );
    assert_eq!(rows[0]["rank"], 6);
    assert_eq!(rows[0]["id"], "entr_m006");
    assert_eq!(holder.calls(), 1, "one call for the whole read");
    // Sent whole: the filter, the order with the id after it, the `take`,
    // the fields. The holder ran it as its own SQL.
    let sent = &holder.refines()[0];
    assert_eq!(sent["limit"], 3, "{sent}");
    assert_eq!(
        sent["sort"],
        json!([{ "field": "rank", "desc": true }, { "field": "id", "desc": false }]),
        "{sent}"
    );
    let conds = sent["where"].as_array().unwrap();
    assert!(
        conds.contains(&json!({ "field": "rank", "op": "ge", "value": 2 })),
        "{sent}"
    );
    assert!(
        conds.contains(&json!({ "field": "owner", "op": "eq", "value": "mem_ada" })),
        "{sent}"
    );
    assert_eq!(sent["pick"], json!(["id", "title", "owner", "rank"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_of_100_native_rows_to_the_peer_is_one_call() {
    let (_ids, holder, viewer) = stage("join").await;
    let rows = viewer.read("marks", "mem_ann").await.expect("the marks");
    assert_eq!(rows.len(), 100);
    assert_eq!(rows[0]["label"], "mark 001");
    assert_eq!(rows[0]["title"], "memo 001");
    assert_eq!(rows[99]["title"], "memo 100");
    assert_eq!(holder.calls(), 1, "one batched call for 100 refs");
    let sent = &holder.refines()[0];
    let refs = sent["where"][0]["value"].as_array().expect("an `in`");
    assert_eq!(sent["where"][0]["op"], "in", "{sent}");
    assert_eq!(refs.len(), 100);
    assert_eq!(sent["limit"], 100);

    // A condition through the ref that holds on null (`!=`) stays here:
    // sent to the holder, the row it left out would join as null and pass.
    let rows = viewer
        .read("marksNotFirst", "mem_ann")
        .await
        .expect("the marks");
    assert_eq!(rows.len(), 99, "{rows:?}");
    assert_eq!(rows[0]["label"], "mark 002");
    assert_eq!(holder.calls(), 2);
    let sent = &holder.refines()[1];
    assert_eq!(sent["where"].as_array().unwrap().len(), 1, "{sent}");
}

#[tokio::test(flavor = "multi_thread")]
async fn caller_on_a_peer_entity_is_the_holders_and_holds_past_the_id_token() {
    let (ids, holder, viewer) = stage("caller").await;
    let rows = viewer.read("myEntries", "mem_ann").await.expect("Ada's");
    // Ada is `mem_ada` in the holder: 96 of the 120 are hers; the read
    // takes 50, sorted by title.
    assert_eq!(rows.len(), 50, "{rows:?}");
    assert_eq!(rows[0]["title"], "memo 001");
    assert!(
        !titles(&rows, "title").contains(&"memo 005".to_string()),
        "memo 005 is Bob's"
    );
    assert_eq!(holder.calls(), 1);
    // `$caller` went to the holder as a condition it decides; this
    // deployment never read the holder's members.
    let sent = &holder.refines()[0];
    assert!(
        sent["where"]
            .as_array()
            .unwrap()
            .contains(&json!({ "field": "owner", "op": "caller" })),
        "{sent}"
    );
    assert_eq!(sent["limit"], 50, "{sent}");

    // 20 minutes on: Ada's ID token would have expired twice over. The
    // consumer's own session statement still buys a token for her.
    ids.clock.advance(20 * 60);
    let again = viewer
        .read("myEntries", "mem_ann")
        .await
        .expect("still Ada's");
    assert_eq!(again.len(), 50);

    // Anonymous: `$caller` matches nothing, and the holder is not asked.
    let none = viewer
        .read("myEntries", ordo_api::ANONYMOUS)
        .await
        .expect("nothing");
    assert!(none.is_empty(), "{none:?}");
    assert_eq!(holder.calls(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_the_holder_does_not_grant_is_refused_by_name() {
    let (_ids, _holder, viewer) = stage("ungranted").await;
    let e = viewer
        .read("allSecrets", "mem_ann")
        .await
        .expect_err("std.secret.list is not granted");
    assert_eq!(e.code, ErrorCode::PeerRefused, "{e:?}");
    assert!(
        e.message.contains("peer `notes` refused `std.secret.list`"),
        "{}",
        e.message
    );
    assert!(e.message.contains("std.secret.list"), "{}", e.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_holder_that_does_not_answer_refuses_the_op_after_the_timeout() {
    let ids = Ids::start("timeout").await;
    let ada = ids.sign_up("viewer", "Ada", 93).await;
    // A door that takes the connection and never answers.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = silent.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = silent.accept().await {
            held.push(s);
        }
    });
    let viewer = Consumer::start(
        "timeout",
        &ids,
        &format!("http://{addr}"),
        &ada,
        &[("ORDO_PEER_TIMEOUT_SECS", "1")],
    )
    .await;
    let started = std::time::Instant::now();
    let e = viewer
        .read("topEntries", "mem_ann")
        .await
        .expect_err("the holder is silent");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(e.code, ErrorCode::PeerUnavailable, "{e:?}");
    assert!(
        e.message.contains("ORDO_PEER_TIMEOUT_SECS"),
        "{}",
        e.message
    );
    assert!(e.message.contains("peer `notes`"), "{}", e.message);
}

/// G8 (G5 left it): `Executor::read_as` has no client to another
/// deployment's door. A read of a peer there is refused by name,
/// `peer_refused`, naming the op, the peer and the fix. Before, the
/// statement ran without the holder's rows and failed on the database as a
/// 500 that named a relation.
#[tokio::test(flavor = "multi_thread")]
async fn read_as_refuses_a_peer_read_by_name_and_never_runs_it() {
    let (_ids, holder, viewer) = stage("readas").await;
    let e = viewer
        .plain
        .read_as("topEntries", Some("mem_ann"))
        .await
        .expect_err("no client to the holder's door");
    let ordo_exec::ExecFail::Peer { code, message } = &e else {
        panic!("not a named refusal: {e:?}");
    };
    assert_eq!(*code, "peer_refused", "{message}");
    for want in [
        "`topEntries` reads peer `notes` through its door",
        "ORDO_IDENTITY_URL",
    ] {
        assert!(message.contains(want), "{want}: {message}");
    }
    assert!(!message.contains("relation"), "{message}");
    assert_eq!(holder.calls(), 0, "the holder was never asked");
    // A read of this deployment's own rows on the same path still answers.
    let own = viewer
        .plain
        .read_as("bookmark[label]\nsort label\ntake 2", Some("mem_ann"))
        .await
        .expect("a native read");
    assert_eq!(own.len(), 2, "{own:?}");
    // And the same op through the door answers.
    let rows = viewer.read("topEntries", "mem_ann").await.expect("served");
    assert_eq!(rows.len(), 3, "{rows:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_field_is_opened_by_the_holder_and_never_stored_plain() {
    let (_ids, holder, viewer) = stage("sealed").await;
    holder
        .write("addDiary", json!({ "body": "plain words" }))
        .await;

    // The consumer reads the plain value through the door: the holder
    // opened it in its own answer, under its own rules for Ada.
    let rows = viewer
        .read("allDiaries", "mem_ann")
        .await
        .expect("read through the holder's door");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["body"], "plain words", "{rows:?}");

    // The holder's table holds no plaintext, and no key left it: the
    // consumer was handed text, never a seal.
    let stored = holder
        .pg
        .query("SELECT body::text FROM diary")
        .expect("the holder's table");
    assert_eq!(stored.len(), 1, "{stored:?}");
    let cell = stored[0][0].clone().unwrap_or_default();
    assert!(!cell.is_empty(), "a sealed value is stored");
    assert!(!cell.contains("plain words"), "plaintext stored: {cell}");
    let anywhere = holder
        .pg
        .query("SELECT count(*) FROM diary WHERE diary::text LIKE '%plain words%'")
        .expect("the holder's row");
    assert_eq!(anywhere[0][0].as_deref(), Some("0"), "plaintext in the row");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_in_its_own_database_reaches_the_peer_through_the_door() {
    let ids = Ids::start("tenantdb").await;
    let ada = ids.sign_up("viewer", "Ada", 94).await;
    let bob = ids.sign_up("notes", "Bob", 95).await;
    let holder = Holder::start("tenantdb", &ids, &ada, &bob).await;
    let viewer = Consumer::start_with("tenantdb", &ids, &holder.address, &ada, &[], true).await;

    // From the tenant's own database: the holder decides `$caller` for
    // Ada, whose sign-in link is in the deployment's database.
    let rows = viewer
        .read_in("myEntries", TWIN, "mem_twin")
        .await
        .expect("the tenant reads through the door");
    assert_eq!(rows.len(), 50, "{rows:?}");
    assert_eq!(rows[0]["title"], "memo 001");
    assert!(!titles(&rows, "title").contains(&"memo 005".to_string()));
    // And from the deployment's own space, as before.
    let rows = viewer
        .read("topEntries", "mem_ann")
        .await
        .expect("the own space reads");
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(holder.calls(), 2);
}
