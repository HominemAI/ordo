//! The holder's door for other Ordo deployments
//! (`plan-federation-grants.md` 2, 3, 5, 7, step G3), against a real
//! identity service and a real door over Postgres.
//!
//! `notes` is the holder: its file grants `note_viewer` (on behalf of a
//! person), `billing as itself`, and `provider ids` (every deployment the
//! service vouches for, on behalf of a person). `viewer` and `billing` are
//! consumers tied to those grants by `ORDO_CONSUMER_<NAME>_CLIENT`;
//! `stranger` is registered at the service and tied to nothing. Each
//! consumer is a test-built one: it signs its client assertions with the
//! G2 helpers (`ordo_identity::peer_token`), since the real consumer is G4.
//!
//! What it proves:
//!
//! - on behalf of a person, an op in the grant runs as that person's member
//!   here, under this deployment's own access rules, and the call is
//!   recorded with the person and the consumer;
//! - as itself, an op in a grant marked `as itself` runs, in the
//!   deployment's own space only;
//! - each refusal names the consumer, the op and the fix: an op outside the
//!   grant, a call as itself under a grant without `as itself`, an unknown
//!   consumer, another audience, an expired token, a loop in the chain;
//! - `grant provider ids` covers a consumer with no grant of its own, on
//!   behalf of a person, for its ops only;
//! - a token the service did not sign gets the door's uniform denial.
//!
//! A regression looks like a consumer reaching an op no grant opens, a
//! call as itself under a person-only grant, a call that loops, or a
//! refusal that does not say what to change.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_peer_`.

mod oidc_support;
mod refine_support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use ordo_api::AccessChecker;
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::PoolConfig;
use ordo_host::auth::AuthState;
use ordo_host::consumer_grants as cg;
use ordo_host::identity_keys::{IdentityKeyStore, IdentityKeys, IdentitySettings, ServiceUrl};
use ordo_host::metrics::Metrics;
use ordo_host::peer_door::PeerDoor;
use ordo_host::principal_directory::PgDirectoryStore;
use ordo_host::{IdentityAccess, DEFAULT_SPACE};
use ordo_identity::clock::Clock;
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::federation::form_encode;
use ordo_identity::peer_token::{
    self, ASSERTION_TYPE, CHAIN_PARAM, GRANT_CLIENT_CREDENTIALS, GRANT_TOKEN_EXCHANGE,
    TOKEN_TYPE_ID_TOKEN,
};
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    Argon2idHasher, AuthenticatorId, CompleteRequest, PrincipalKind, ProviderRegistry, Timestamp,
    TokenKey,
};
use ordo_identity_service::config::{DATABASE_URL_ENV, DATA_DIR_ENV, ORIGIN_ENV};
use ordo_identity_service::deployments::{Registration, SignUp};
use ordo_identity_service::people::Actor;
use ordo_identity_service::{BootOptions, IdentityConfig, Service};
use ordo_secevent::{EventKind, RecordedDecision};
use refine_support::{call_as, door_over};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const NOW: u64 = 1_800_000_000;
const MASTER: [u8; 32] = [21; 32];
const VERIFIER: &str = "a-verifier-of-at-least-forty-three-characters-0123456789";
const HOST: Actor<'static> = Actor {
    id: "unix:test",
    client: "host",
};

/// The holder's program: the conformance fixture, one entity and its ops,
/// and the grants.
const HOLDER: &str = r#"
entity journal {
  title: text
}

op addEntry($title: text) =
  set journal { title: $title }

op archiveEntry($id: id) =
  del journal { id: $id }

grant note_viewer {
  std.journal.list
  addEntry
}

grant billing as itself {
  std.journal.list
}

grant provider ids {
  std.journal.get
}
"#;

// ───────────────────────── the identity service ─────────────────────────

struct Ids {
    svc: Service,
    /// Its public origin and the `iss` of its tokens.
    issuer: String,
    /// Where the door fetches its keys: plain loopback.
    url: String,
}

fn callback(issuer: &str, handle: &str) -> String {
    format!("{issuer}/{handle}/auth/callback/ids")
}

fn key_of(handle: &str) -> SigningKey {
    peer_token::consumer_key(&MASTER, handle)
}

impl Ids {
    /// A service on a fresh database with `notes`, `viewer`, `billing`,
    /// `stranger` and `hal` registered, open; all but `notes` with a
    /// consumer key.
    async fn start(name: &str) -> Ids {
        let c = PoolConfig::from_env();
        let db = format!("ordo_peer_ids_{name}");
        ordo_exec::drop_database(&c, &db).await.expect("drop");
        ordo_exec::create_database(&c, &db).await.expect("create");
        let mut url = format!(
            "host={} port={} user={} dbname={db}",
            c.host, c.port, c.user
        );
        if let Some(p) = &c.password {
            url.push_str(&format!(" password={p}"));
        }
        let dir = std::env::temp_dir().join(format!("ordo-peer-{name}-{}", std::process::id()));
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
        let svc = Service::boot(
            cfg,
            BootOptions {
                clock: Clock::manual(NOW),
                hasher: Arc::new(Argon2idHasher::weak_for_tests()),
            },
        )
        .await
        .expect("boot");
        for handle in ["notes", "viewer", "billing", "stranger", "hal"] {
            svc.register_deployment(
                HOST,
                &Registration {
                    handle: handle.into(),
                    client_origins: vec![issuer.clone()],
                    return_urls: vec![callback(&issuer, handle)],
                    sign_up: SignUp::Open,
                    consumer_key: (handle != "notes")
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

    /// `POST /oidc/token` with a form body.
    async fn token(&self, pairs: &[(&str, &str)]) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("POST")
            .uri("/oidc/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(form_encode(pairs)))
            .unwrap();
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
        let code =
            oidc_support::param(&location, "code").unwrap_or_else(|| panic!("a code: {location}"));
        let (s, v) = self
            .token(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect),
                ("client_id", client),
                ("code_verifier", VERIFIER),
            ])
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["id_token"].as_str().unwrap().to_string()
    }

    /// A fresh client assertion for `client`, signed with its consumer key:
    /// what a consumer sends (built here; the real consumer is G4).
    fn assertion(&self, client: &str, jti: &str) -> String {
        peer_token::client_assertion(
            client,
            &format!("{}/oidc/token", self.issuer),
            &key_of(client),
            NOW,
            NOW + 60,
            jti,
        )
    }

    /// A token for `audience` on behalf of the person `id_token` names,
    /// with the consumer's incoming `chain`.
    async fn on_behalf(&self, client: &str, id_token: &str, audience: &str, chain: &str) -> String {
        let jti = format!(
            "{client}-{audience}-{chain}-{}",
            &id_token[id_token.len() - 8..]
        );
        let a = self.assertion(client, &jti);
        let mut form = vec![
            ("grant_type", GRANT_TOKEN_EXCHANGE),
            ("client_assertion_type", ASSERTION_TYPE),
            ("client_assertion", a.as_str()),
            ("subject_token", id_token),
            ("subject_token_type", TOKEN_TYPE_ID_TOKEN),
            ("audience", audience),
        ];
        if !chain.is_empty() {
            form.push((CHAIN_PARAM, chain));
        }
        let (s, v) = self.token(&form).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["access_token"].as_str().unwrap().to_string()
    }

    /// A token for `audience` for `client` as itself.
    async fn as_itself(&self, client: &str, audience: &str) -> String {
        let a = self.assertion(client, &format!("{client}-{audience}-itself"));
        let (s, v) = self
            .token(&[
                ("grant_type", GRANT_CLIENT_CREDENTIALS),
                ("client_assertion_type", ASSERTION_TYPE),
                ("client_assertion", &a),
                ("audience", audience),
            ])
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["access_token"].as_str().unwrap().to_string()
    }
}

// ─────────────────────────────── the holder ───────────────────────────────

struct Holder {
    state: ordo_api::ApiState,
    door: Arc<IdentityAccess>,
    clock: Arc<AtomicU64>,
    pool: ordo_exec::Pool,
}

fn holder_fixture() -> Fixture {
    Fixture::from_source(&format!("{FIXTURE}\n{HOLDER}"), SEED).expect("the holder compiles")
}

impl Holder {
    /// `notes`'s door over its own database, signing people in through
    /// the connection `ids` to the service, with `ada` (the service's
    /// person id) a member here as `mem_ada`.
    async fn start(name: &str, ids: &Ids, ada: &str, sign_up: Provisioning) -> Holder {
        let db = format!("ordo_peer_holder_{name}");
        let f = Arc::new(holder_fixture());
        let (mut state, f, pool) = door_over(f, &db, 50, PoolConfig::from_env(), |k| k).await;
        let pg = Pg::named(&db);
        ordo_host::principal_directory::ensure_table(&pool)
            .await
            .expect("a directory");

        // The grants of the serving version, expanded as an apply does.
        let ops: Vec<String> = ordo_host::build_specs(&f)
            .into_iter()
            .map(|s| s.name)
            .collect();
        let grants: Vec<cg::Expanded> = f
            .schema
            .grants
            .iter()
            .map(|g| cg::expand(g, &ops).expect("expands").grant)
            .collect();
        assert_eq!(grants.len(), 3);
        cg::store(&pool, "1.0.0", &grants).await.expect("stored");
        ordo_exec::registry::ensure_tables(&pool)
            .await
            .expect("registry");
        // Version 1.0.0 serving, as an apply leaves it.
        pool.rows(
            "INSERT INTO \"$artifacts\" (\"version\", \"artifact\", \"source\", \"ddl\", \
             \"applied_at\", \"actor\") VALUES ('1.0.0', '{}', '', '', 0, 'test'); \
             INSERT INTO \"$activations\" (\"version\", \"at\", \"actor\") \
             VALUES ('1.0.0', 0, 'test')",
        )
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
            Directory::with_store(sign_up, store),
            TokenKey::from_secret(b"peer-door-holder"),
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

        let clock = Arc::new(AtomicU64::new(NOW));
        let read = clock.clone();
        let now: Arc<dyn Fn() -> u64 + Send + Sync> = Arc::new(move || read.load(Ordering::SeqCst));
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
            ("ORDO_CONSUMER_BILLING_CLIENT".into(), "billing".into()),
        ]);
        let peer = PeerDoor::new(
            keys.clone(),
            Some(auth),
            Some(pool.clone()),
            Arc::new(move |k: &str| vars.get(k).cloned()),
            now.clone(),
        );
        assert!(peer.settings().is_ok(), "{:?}", peer.settings());
        let engine = ordo_host::deployment_engine(&f, &pg).expect("an engine");
        let read = clock.clone();
        let door = Arc::new(
            IdentityAccess::new(
                engine,
                TokenKey::from_secret(b"peer-door-holder"),
                Box::new(move || read.load(Ordering::SeqCst)),
                Arc::new(Metrics::new()),
            )
            .with_console_home(DEFAULT_SPACE)
            .with_identity_keys(keys)
            .with_peer(Arc::new(peer)),
        );
        state.access = door.clone();
        Holder {
            state,
            door,
            clock,
            pool,
        }
    }

    async fn call(&self, token: &str, op: &str) -> (StatusCode, Value) {
        let (method, body) = if op.starts_with("std.") {
            ("GET", None)
        } else if op == "addEntry" {
            ("POST", Some(json!({ "title": "from a peer" })))
        } else {
            ("DELETE", Some(json!({ "id": "j_1" })))
        };
        call_as(&self.state, Some(token), method, &format!("/op/{op}"), body).await
    }

    /// Every security record: (subject, token kind and client, decision).
    fn record(&self) -> Vec<(String, String, Option<RecordedDecision>)> {
        self.door
            .take_events()
            .into_iter()
            .flat_map(|e| {
                let who = e.subject.render();
                let facts = e
                    .subject
                    .token()
                    .map(|t| format!("{} {}", t.kind, t.client))
                    .unwrap_or_default();
                match e.kind {
                    EventKind::Denied(ds) => ds
                        .into_iter()
                        .map(|d| (who.clone(), facts.clone(), Some(d)))
                        .collect(),
                    EventKind::Authenticated => vec![(who, facts, None)],
                    _ => Vec::new(),
                }
            })
            .collect()
    }
}

/// The error message of a refused call.
fn refused(got: &(StatusCode, Value)) -> String {
    assert_eq!(got.0, StatusCode::FORBIDDEN, "{}", got.1);
    assert_eq!(got.1["error"]["code"], "access_denied", "{}", got.1);
    got.1["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A service with Ada signed up to `viewer` and joined to `stranger`, and
/// the holder.
async fn stage(name: &str) -> (Ids, Holder, String, String) {
    stage_with(name, Provisioning::AutoOnboard).await
}

async fn stage_with(name: &str, sign_up: Provisioning) -> (Ids, Holder, String, String) {
    let ids = Ids::start(name).await;
    let (cookie, ada) = ids.sign_up("viewer", "Ada", 71).await;
    ids.join(&cookie, "stranger").await;
    let holder = Holder::start(name, &ids, &ada, sign_up).await;
    (ids, holder, cookie, ada)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_calls_on_behalf_of_a_person_within_its_grant_and_no_further() {
    let (ids, holder, cookie, _ada) = stage("behalf").await;
    let id_token = ids.id_token("viewer", &cookie).await;
    let t = ids.on_behalf("viewer", &id_token, "notes", "").await;

    // In the grant: runs as Ada's member here, under the holder's rules.
    let got = holder.call(&t, "std.journal.list").await;
    assert_eq!(got.0, StatusCode::OK, "{}", got.1);
    assert_eq!(got.1["rows"][0]["title"], "first", "{}", got.1);
    let got = holder.call(&t, "addEntry").await;
    assert_eq!(got.0, StatusCode::OK, "{}", got.1);

    // Outside the grant: named, with the line to add.
    let why = refused(&holder.call(&t, "archiveEntry").await);
    assert!(
        why.contains("consumer `viewer`, op `archiveEntry`"),
        "{why}"
    );
    assert!(
        why.contains("add the line `archiveEntry` to `grant note_viewer`"),
        "{why}"
    );

    // Recorded with the person and the consumer.
    let record = holder.record();
    let allowed: Vec<_> = record.iter().filter(|r| r.2.is_none()).collect();
    assert_eq!(allowed.len(), 2, "{record:?}");
    assert!(
        allowed.iter().all(|r| r.1 == "peer-person viewer"),
        "{record:?}"
    );
    assert!(
        record
            .iter()
            .any(|r| r.1 == "peer-person viewer"
                && r.2 == Some(RecordedDecision::ConsumerNotGranted)),
        "{record:?}"
    );

    // `grant provider ids` covers a deployment with no grant of its own, on
    // behalf of a person, for its own ops.
    let id_token = ids.id_token("stranger", &cookie).await;
    let t = ids.on_behalf("stranger", &id_token, "notes", "").await;
    let got = holder.call(&t, "std.journal.get?id=j_1").await;
    assert_eq!(got.0, StatusCode::OK, "{}", got.1);
    let why = refused(&holder.call(&t, "std.journal.list").await);
    assert!(
        why.contains("`grant provider ids` does not grant it"),
        "{why}"
    );

    // As itself, `stranger` is nobody's consumer here: named, with the
    // setting.
    let t = ids.as_itself("stranger", "notes").await;
    let why = refused(&holder.call(&t, "std.journal.list").await);
    assert!(
        why.contains("`stranger` is not a consumer of this deployment"),
        "{why}"
    );
    assert!(
        why.contains("ORDO_CONSUMER_<NAME>_CLIENT=stranger"),
        "{why}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_consumer_calls_as_itself_only_under_a_grant_marked_as_itself() {
    let (ids, holder, _cookie, _ada) = stage("itself").await;

    let t = ids.as_itself("billing", "notes").await;
    let got = holder.call(&t, "std.journal.list").await;
    assert_eq!(got.0, StatusCode::OK, "{}", got.1);
    assert!(
        holder
            .record()
            .iter()
            .any(|r| r.0.contains("consumer:billing") && r.1 == "peer-client billing"),
        "recorded as the consumer"
    );
    // As itself it reaches the deployment's own space only.
    assert!(matches!(
        holder.door.check(&t, "elsewhere", "std.journal.list"),
        ordo_api::AccessDecision::Deny
    ));
    let why = holder
        .door
        .peer_refused(&t, "elsewhere", "std.journal.list")
        .expect("named");
    assert!(
        why.contains("reaches only this deployment's own space"),
        "{why}"
    );
    // Not in its grant.
    let why = refused(&holder.call(&t, "addEntry").await);
    assert!(why.contains("consumer `billing`, op `addEntry`"), "{why}");

    // `viewer`'s grant has no `as itself`.
    let t = ids.as_itself("viewer", "notes").await;
    let why = refused(&holder.call(&t, "std.journal.list").await);
    assert!(
        why.contains("`grant note_viewer` allows calls on behalf of a person only"),
        "{why}"
    );
    assert!(
        why.contains("write `grant note_viewer as itself { … }`"),
        "{why}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_for_another_deployment_an_expired_one_a_loop_and_a_forgery_are_refused() {
    let (ids, holder, cookie, _ada) = stage("refusals").await;
    let id_token = ids.id_token("viewer", &cookie).await;

    // For another deployment.
    let t = ids.on_behalf("viewer", &id_token, "hal", "").await;
    let why = refused(&holder.call(&t, "std.journal.list").await);
    assert!(why.contains("the token is for `ordo-peer:hal`"), "{why}");
    assert!(why.contains("audience `notes`"), "{why}");

    // A loop: the call already came through `notes`.
    let t = ids.on_behalf("viewer", &id_token, "notes", "notes").await;
    let why = refused(&holder.call(&t, "std.journal.list").await);
    assert!(
        why.contains("already passed through `notes` (chain notes → viewer → notes)"),
        "{why}"
    );
    assert!(holder
        .record()
        .iter()
        .any(|r| r.2 == Some(RecordedDecision::PeerLoop)));

    // A chain that does not reach `notes` passes.
    let t = ids.on_behalf("viewer", &id_token, "notes", "hal").await;
    let got = holder.call(&t, "std.journal.list").await;
    assert_eq!(got.0, StatusCode::OK, "{}", got.1);

    // Expired: the same token, once its life is over.
    holder.clock.store(NOW + 3_600, Ordering::SeqCst);
    let why = refused(&holder.call(&t, "std.journal.list").await);
    assert!(why.contains("the token expired at"), "{why}");
    holder.clock.store(NOW, Ordering::SeqCst);

    // A forgery: the right shape, signed by somebody else. The uniform
    // denial, naming nothing.
    let forged = peer_token::sign_jwt(
        &json!({ "alg": "EdDSA", "kid": "x", "typ": peer_token::TYP_ON_BEHALF }),
        &json!({ "iss": ids.issuer, "sub": "p", "aud": "ordo-peer:notes",
                 "act": { "sub": "viewer" }, "client_id": "viewer", "kind": "person",
                 "iat": NOW, "exp": NOW + 60, "jti": "f" }),
        &SigningKey::from_bytes(&[9; 32]),
    );
    let got = holder.call(&forged, "std.journal.list").await;
    assert_eq!(got.0, StatusCode::FORBIDDEN, "{}", got.1);
    assert!(
        !got.1["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("consumer"),
        "{}",
        got.1
    );
    assert!(holder
        .door
        .peer_refused(&forged, DEFAULT_SPACE, "std.journal.list")
        .is_none());
}

/// The links in the holder's directory for `authenticator`.
async fn linked(holder: &Holder, authenticator: &str) -> usize {
    holder
        .pool
        .rows(&format!(
            "SELECT principal FROM \"$authenticator\" WHERE authenticator = {}",
            ordo_exec::sql_lit(authenticator)
        ))
        .await
        .expect("read")
        .len()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_new_to_the_holder_follows_its_sign_up_rules() {
    // Invite-only: Bo, unknown here, is refused by name, and nothing is
    // made for him.
    let (ids, holder, _cookie, _ada) = stage_with("invite", Provisioning::InviteOnly).await;
    let (bo_cookie, bo) = ids.sign_up("viewer", "Bo", 72).await;
    let id_token = ids.id_token("viewer", &bo_cookie).await;
    let t = ids.on_behalf("viewer", &id_token, "notes", "").await;
    let why = refused(&holder.call(&t, "std.journal.list").await);
    assert!(why.contains("sign-up is invite-only"), "{why}");
    assert_eq!(linked(&holder, &format!("oidc:ids:{bo}")).await, 0);

    // Open sign-up: Bo is made a person here, through the same link a
    // direct sign-in makes, with no membership and so no data: the
    // holder's own rules give him the uniform denial.
    let (ids, holder, _cookie, _ada) = stage_with("open", Provisioning::AutoOnboard).await;
    let (bo_cookie, bo) = ids.sign_up("viewer", "Bo", 73).await;
    let id_token = ids.id_token("viewer", &bo_cookie).await;
    let t = ids.on_behalf("viewer", &id_token, "notes", "").await;
    let got = holder.call(&t, "std.journal.list").await;
    assert_eq!(got.0, StatusCode::FORBIDDEN, "{}", got.1);
    assert!(holder
        .door
        .peer_refused(&t, DEFAULT_SPACE, "std.journal.list")
        .is_none());
    assert_eq!(linked(&holder, &format!("oidc:ids:{bo}")).await, 1);
}
