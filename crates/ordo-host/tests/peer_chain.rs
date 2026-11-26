//! Peer calls carry the incoming chain, and a write's guard reads a peer
//! through its door (`plan-federation-grants.md` §4, step G5b), against a
//! real identity service and 2 real deployments.
//!
//! `north` and `south` run one program: a native `widget` whose `link` refs
//! the other deployment's `widget` through the external `mate`, with read
//! and write guards that walk into it (Ada's own rows,
//! and `listed or link.shown`, `link.unlocked`). Each is both a holder (a door on a
//! loopback socket, granting the other `std.widget.list` on behalf of a
//! person) and a consumer (`peer other`, `ORDO_PEER_OTHER_URL` set to the
//! other's door). Ada is a member of both.
//!
//! What it proves:
//!
//! - 2 deployments whose guards read each other are refused as a loop by
//!   name at once, far under `ORDO_PEER_TIMEOUT_SECS`: every peer call
//!   passes on the chain of the call it serves, and the holder already on
//!   the chain refuses it;
//! - a write whose guard reads a peer through its door runs, the guard
//!   decided on the holder's answer: a create and an update the guard
//!   allows are written, and the ones it refuses are refused by name, never
//!   a 500.
//!
//! A regression looks like a read that hangs until the timeout and answers
//! `peer_unavailable`, or a write that answers `internal`.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_peerc_`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use ordo_api::{
    router, ApiState, BoundParams, ErrorCode, ExecOutcome, MemoryBus, MemoryReplayCache,
    OpExecutor, OpSpec, PageKey,
};
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::auth::AuthState;
use ordo_host::consumer_grants as cg;
use ordo_host::grants::PgGrantRegistry;
use ordo_host::identity_keys::{IdentityKeyStore, IdentityKeys, IdentitySettings, ServiceUrl};
use ordo_host::metrics::Metrics;
use ordo_host::peer_client::PeerClient;
use ordo_host::peer_door::PeerDoor;
use ordo_host::peer_reads::DoorReads;
use ordo_host::platform_ops::{self, PlatformExecutor, PlatformOps};
use ordo_host::principal_directory::PgDirectoryStore;
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
use serde_json::{json, Map, Value};
use tower::ServiceExt;

const NOW: u64 = 1_800_000_000;
const MASTER: [u8; 32] = [45; 32];

/// Both deployments' program, after the conformance fixture.
const PROGRAM: &str = r#"
peer other

external mate from other.widget shared {
  id: text <- id
  title: text <- title
  shown: bool <- shown
  unlocked: bool <- unlocked
}

entity widget {
  title: text
  owner: text
  listed: bool
  shown: bool
  unlocked: bool
  link -> mate?
  access read   owner == $caller and (listed or link.shown)
  access write  owner == $caller and link.unlocked
}

grant mates {
  std.widget.list
}

op items =
  widget[id, title]
  sort title
  take 50

op addItem($title: text, $link: id) =
  set widget { title: $title, owner: $caller, listed: true, shown: true, unlocked: true, link: $link }

op renameItem($item: id, $title: text) =
  set widget(id == $item) { title: $title }
"#;

/// A holder only: its guards read nobody.
const HOLDER: &str = r#"
entity widget {
  title: text
  owner: text
  listed: bool
  shown: bool
  unlocked: bool
  access read   owner == $caller
  access write  owner == $caller
}

grant mates {
  std.widget.list
}
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
    /// A service with `north` and `south` registered, open, each with its
    /// consumer key.
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
        for handle in ["north", "south"] {
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
                    consumer_key: Some(peer_token::public_text(&key_of(handle).verifying_key())),
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

    async fn browser(&self, path: &str, body: Value) -> (axum::http::StatusCode, Value) {
        let mut req = axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("origin", &self.issuer)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [198, 51, 100, 9],
                40000,
            ))));
        let res = self.svc.router().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Ada signs up through `north` and is made a member of `south`: her id
    /// at the service.
    async fn ada(&self) -> String {
        let (s, opts) = self
            .browser("/sign-up/begin", json!({ "deployment": "north" }))
            .await;
        assert_eq!(s, axum::http::StatusCode::OK, "{opts}");
        let challenge = opts["publicKey"]["challenge"].as_str().unwrap().to_string();
        let auth = TestAuthenticator {
            key: TestKey::es256(81),
            credential_id: vec![81; 16],
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
        let (s, done) = self
            .browser(
                "/sign-up/finish",
                json!({
                    "challenge": challenge,
                    "clientDataJSON": b64url_encode(&client_data_json),
                    "attestationObject": b64url_encode(&attestation_object),
                    "deployment": "north",
                    "displayName": "Ada",
                }),
            )
            .await;
        assert_eq!(s, axum::http::StatusCode::OK, "{done}");
        let ada = done["person"].as_str().unwrap().to_string();
        self.svc
            .add_member(
                ordo_identity_service::people::Actor {
                    id: "unix:test",
                    client: "host",
                },
                "south",
                &ada,
            )
            .await
            .expect("a member of south");
        ada
    }
}

// ─────────────────────────────── a deployment ───────────────────────────────

/// Ada here: a principal linked to the identity service's `sub`, and the
/// member `mem_ada` of the space.
async fn ada_here(pg: &Pg, pool: &Pool, sub: &str) {
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
        TokenKey::from_secret(b"peer-chain"),
    );
    let principal = {
        let mut dir = auth.lock_directory();
        let p = dir
            .create_principal(PrincipalKind::Human, "Ada", Timestamp(NOW))
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
         \"memberCount\", role, status, joined) VALUES ('{DEFAULT_SPACE}', 'mem_ada', \
         '{}', 'Ada', 'principal', 1, 'manager', 'active', '2026-11-26')",
        principal.0
    ))
    .expect("a member");
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

/// A row of `widget`.
struct Item {
    id: &'static str,
    title: &'static str,
    public: bool,
    shown: bool,
    open: bool,
    link: Option<&'static str>,
}

/// A row only its link can show: its guard asks the other deployment.
fn hidden(id: &'static str, title: &'static str, link: &'static str) -> Item {
    Item {
        id,
        title,
        public: false,
        shown: true,
        open: true,
        link: Some(link),
    }
}

/// A public row, `open` or not, linking `link`.
fn public(id: &'static str, title: &'static str, open: bool, link: Option<&'static str>) -> Item {
    Item {
        id,
        title,
        public: true,
        shown: true,
        open,
        link,
    }
}

/// One deployment: its door on `listener`, its peer `other` at
/// `other_door`.
struct Node {
    exec: Arc<FixtureExecutor>,
    specs: Vec<OpSpec>,
    pg: Pg,
    /// The typed-id prefix of `mate`: `<prefix>_<id>` names the other's row.
    mate: String,
}

impl Node {
    #[allow(clippy::too_many_arguments)]
    async fn start(
        test: &str,
        me: &str,
        other: &str,
        listener: tokio::net::TcpListener,
        other_door: &str,
        ids: &Ids,
        ada: &str,
        program: &str,
        items: &[Item],
    ) -> Node {
        let source = format!("{FIXTURE}\n{program}");
        let mut f = Fixture::from_source(&source, SEED).expect("the program compiles");
        let vars: HashMap<String, String> = HashMap::from([
            ("ORDO_IDENTITY_URL".into(), ids.url.clone()),
            ("ORDO_IDENTITY_ORIGIN".into(), ids.issuer.clone()),
            ("ORDO_IDENTITY_DEPLOYMENT".into(), me.into()),
            ("ORDO_AUTH_METHODS".into(), "passkey,ids".into()),
            ("ORDO_AUTH_IDS_KIND".into(), "oidc".into()),
            ("ORDO_AUTH_IDS_ISSUER".into(), ids.issuer.clone()),
            ("ORDO_CONSUMER_MATES_CLIENT".into(), other.into()),
            ("ORDO_PEER_OTHER_URL".into(), other_door.into()),
            ("ORDO_PEER_OTHER_DEPLOYMENT".into(), other.into()),
            ("ORDO_PEER_OTHER_SPACE".into(), DEFAULT_SPACE.into()),
        ]);
        let lookup = vars.clone();
        ordo_exec::peer::resolve_with(&mut f.schema, "public", |k| lookup.get(k).cloned())
            .expect("a door peer needs no schema setting");
        let mate = f
            .schema
            .externals
            .iter()
            .find(|e| e.name == "mate")
            .map(|e| e.key.clone())
            .unwrap_or_default();
        let f = Arc::new(f);
        let pg = Pg::scoped(&format!("ordo_peerc_{me}_{test}"));
        f.reset(&pg).expect("seeded");
        let pool = Pool::connect(PoolConfig {
            db: pg.db.clone(),
            size: 4,
            ..PoolConfig::from_env()
        })
        .await
        .expect("a pool");

        // The grant `mates`, tied to the other deployment, in the version
        // serving.
        // The artifact as the file wrote it: no peer setting reaches it.
        let doc =
            ordo_plan::artifact::program_artifact(&f.schema, &f.registry).expect("an artifact");
        let ops: Vec<String> = cg::program_ops(&doc);
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
            ordo_exec::sql_lit(&ordo_plan::artifact::to_bytes(&doc)),
            ordo_exec::sql_lit(&source),
        ))
        .await
        .expect("active");

        ada_here(&pg, &pool, ada).await;
        for i in items {
            let (column, link) = match i.link {
                Some(l) => (", link", format!(", '{mate}_{l}'")),
                None => ("", String::new()),
            };
            pg.exec(&format!(
                "INSERT INTO widget (\"$space\", id, title, owner, listed, shown, unlocked{column}) \
                 VALUES ('{DEFAULT_SPACE}', '{}', '{}', 'mem_ada', {}, {}, {}{link})",
                i.id, i.title, i.public, i.shown, i.open
            ))
            .expect("an item");
        }

        let now: Arc<dyn Fn() -> u64 + Send + Sync> = {
            let clock = ids.clock.clone();
            Arc::new(move || clock.now())
        };
        let settings: ordo_host::peer_client::Settings = {
            let vars = vars.clone();
            Arc::new(move |k: &str| vars.get(k).cloned())
        };

        // The holder's half: the door for the other deployment.
        let keys = Arc::new(IdentityKeys::new(
            IdentitySettings {
                url: ServiceUrl::parse(&ids.url, false).unwrap(),
                issuer: ids.issuer.clone(),
                audience: ordo_identity::deployment_token::audience(me),
            },
            IdentityKeyStore::memory(),
            now.clone(),
        ));
        keys.boot().await.expect("the service's keys");
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
            TokenKey::from_secret(b"peer-chain-door"),
        ));
        let peer = Arc::new(PeerDoor::new(
            keys.clone(),
            Some(auth),
            Some(pool.clone()),
            settings.clone(),
            now.clone(),
        ));
        assert!(peer.settings().is_ok(), "{:?}", peer.settings());
        let engine = ordo_host::deployment_engine(&f, &pg).expect("an engine");
        let door = Arc::new(
            IdentityAccess::new(
                engine,
                TokenKey::from_secret(b"peer-chain-door"),
                Box::new(|| NOW),
                Arc::new(Metrics::new()),
            )
            .with_console_home(DEFAULT_SPACE)
            .with_identity_keys(keys)
            .with_peer(peer),
        );
        let registry = Arc::new(PgGrantRegistry::new(pool.clone()));
        registry.ensure_table().await.expect("platform table");
        let ops = Arc::new(PlatformOps::new(
            Arc::new(ordo_host::engine_from_members(&f, &[]).expect("engine")),
            registry,
            now.clone(),
        ));
        let deployment = ordo_host::deploy::Deployment::pinned_with_platform_ops(
            me,
            f.clone(),
            executor_for(&f, &pool),
            platform_ops::specs(),
        )
        .expect("platform specs");

        // The consumer's half: reads of the other through its door.
        let client = Arc::new(PeerClient::new(key_of(me), settings.clone(), now));
        deployment.set_peer_reads(Arc::new(DoorReads::new(client, settings)));

        let exec = Arc::new(FixtureExecutor {
            spaces: ordo_host::spaces::Spaces::new(deployment.clone()),
            pg: pg.clone(),
            page_size: 50,
        });
        let executor: Arc<dyn OpExecutor> = Arc::new(PlatformExecutor::new(exec.clone(), ops));
        let state = ApiState {
            registry: deployment.handle(),
            executor,
            access: door,
            quota: Arc::new(ordo_api::NoQuota),
            bus: Arc::new(MemoryBus::new()),
            replay: Arc::new(MemoryReplayCache::new()),
            attest: Arc::new(ordo_api::attest::NoAttestation),
            page_key: PageKey::from_secret(b"peer-chain"),
            sync: Default::default(),
            live_streams: Default::default(),
            refine_limits: Default::default(),
            public: Default::default(),
        };
        let app = router(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Node {
            exec,
            specs: ordo_host::build_specs(&f),
            pg,
            mate,
        }
    }

    /// Run `op` as Ada with `params`: the rows, or the refusal.
    async fn call(&self, op: &str, params: Value) -> Result<Vec<Value>, ordo_api::ExecError> {
        let spec = self
            .specs
            .iter()
            .find(|s| s.name == op)
            .unwrap_or_else(|| panic!("no op {op}"));
        let mut bound: Map<String, Value> = params.as_object().cloned().unwrap_or_default();
        bound.insert("$caller".into(), json!("mem_ada"));
        match self
            .exec
            .execute(spec, DEFAULT_SPACE, BoundParams(bound), None, None, None)
            .await?
        {
            ExecOutcome::Ran(r) => Ok(r.rows),
            other => panic!("{other:?}"),
        }
    }

    fn titles(&self) -> Vec<String> {
        let mut t: Vec<String> = self
            .pg
            .query(&format!(
                "SELECT title FROM widget WHERE \"$space\" = '{DEFAULT_SPACE}' ORDER BY title"
            ))
            .expect("titles")
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect();
        t.sort();
        t
    }
}

/// `north` and `south`, each with its items.
async fn stage(
    test: &str,
    north_items: &[Item],
    south: &str,
    south_items: &[Item],
) -> (Ids, Node, Node) {
    let ids = Ids::start(test).await;
    let ada = ids.ada().await;
    let n = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let s = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let n_door = format!("http://{}", n.local_addr().unwrap());
    let s_door = format!("http://{}", s.local_addr().unwrap());
    let north = Node::start(
        test,
        "north",
        "south",
        n,
        &s_door,
        &ids,
        &ada,
        PROGRAM,
        north_items,
    )
    .await;
    let south = Node::start(
        test,
        "south",
        "north",
        s,
        &n_door,
        &ids,
        &ada,
        south,
        south_items,
    )
    .await;
    (ids, north, south)
}

#[tokio::test(flavor = "multi_thread")]
async fn guards_that_read_each_other_are_refused_as_a_loop_at_once() {
    // north's a1 links south's s1, and s1 links a1, neither public:
    // reading a1 asks south for s1, whose guard asks north for a1.
    let (_ids, north, _south) = stage(
        "loop",
        &[hidden("a1", "north one", "s1")],
        PROGRAM,
        &[hidden("s1", "south one", "a1")],
    )
    .await;
    let started = Instant::now();
    let e = north
        .call("items", json!({}))
        .await
        .expect_err("the guards make a cycle");
    let took = started.elapsed();
    // The default ORDO_PEER_TIMEOUT_SECS is 10.
    assert!(took < Duration::from_secs(3), "took {took:?}: {e:?}");
    assert_eq!(e.code, ErrorCode::PeerRefused, "{e:?}");
    assert!(
        e.message.contains("already passed through `north`"),
        "{}",
        e.message
    );
    assert!(e.message.contains("north → south"), "{}", e.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_whose_guard_reads_a_peer_is_decided_on_the_holders_answer() {
    // south's guards read nobody: it is a holder only.
    let (_ids, north, _south) = stage(
        "write",
        &[
            public("a_open", "linked open", true, Some("s_open")),
            public("a_locked", "linked locked", true, Some("s_locked")),
        ],
        HOLDER,
        &[
            public("s_open", "open", true, None),
            public("s_locked", "locked", false, None),
        ],
    )
    .await;
    let mate = north.mate.clone();

    // A create the write guard allows: south says s_open is open.
    north
        .call(
            "addItem",
            json!({ "title": "new open", "link": format!("{mate}_s_open") }),
        )
        .await
        .expect("allowed: the holder's row is not locked");
    // A create it refuses: south says s_locked is not.
    let e = north
        .call(
            "addItem",
            json!({ "title": "new locked", "link": format!("{mate}_s_locked") }),
        )
        .await
        .expect_err("refused: the holder's row is locked");
    assert_eq!(e.code, ErrorCode::AccessGuard, "{e:?}");

    // An update the guard allows, and one it refuses.
    north
        .call(
            "renameItem",
            json!({ "item": "a_open", "title": "renamed open" }),
        )
        .await
        .expect("allowed: the row links an open row");
    let e = north
        .call(
            "renameItem",
            json!({ "item": "a_locked", "title": "renamed locked" }),
        )
        .await
        .expect_err("refused: the row links a locked row");
    // Ada can read the row, so the refusal is the write guard's.
    assert_eq!(e.code, ErrorCode::AccessGuard, "{e:?}");

    assert_eq!(
        north.titles(),
        ["linked locked", "new open", "renamed open"],
        "only the allowed writes landed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_whose_guard_reads_a_peer_that_refuses_is_refused_by_name() {
    // south's s1 links north's a1 back: the write's guard asks south about
    // s1, whose read guard asks north, and the chain comes back to north.
    let (_ids, north, _south) = stage(
        "write_loop",
        &[hidden("a1", "north one", "s1")],
        PROGRAM,
        &[hidden("s1", "south one", "a1")],
    )
    .await;
    let mate = north.mate.clone();
    let started = Instant::now();
    let e = north
        .call(
            "addItem",
            json!({ "title": "new", "link": format!("{mate}_s1") }),
        )
        .await
        .expect_err("the guards make a cycle");
    assert!(started.elapsed() < Duration::from_secs(3), "{e:?}");
    assert_eq!(e.code, ErrorCode::PeerRefused, "{e:?}");
    assert!(
        e.message.contains("already passed through `north`"),
        "{}",
        e.message
    );
    assert_eq!(north.titles(), ["north one"], "nothing was written");
}
