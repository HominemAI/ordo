//! Ordo apps sign their users in through Ordo's own identity service (owner,
//! 2026-11-17, ask 3): "Each deployment connects to it as an OIDC provider
//! under one connection name, so one person has one subject both
//! deployments recognise."
//!
//! The issuer here is a REAL identity service, in-process on its own
//! Postgres database, behind a TLS listener on loopback that presents the
//! test CA's `localhost` leaf (`oidc_support`). The app reaches it through
//! its egress kernel, certificate checks on, with Ordo's own OIDC client
//! configured as a public client (`CLIENT_AUTH=none`). The browser's part
//! (open sign-up with a passkey, following redirects) is driven in-process
//! with the software authenticator, as the service's own tests drive it: a
//! browser passkey ceremony is not practical in a Rust test.
//!
//! What it proves:
//!
//! - an app completes sign-in for an openly signed-up member, and the
//!   person becomes `oidc:<connection>:<sub>`, `sub` their id at the
//!   service.
//!
//! The same person read across 2 deployments is `peer_reads.rs`: one
//! deployment reads another through its door, for the person signed in.
//!
//! A regression looks like a sign-in that fails against the service, a
//! subject that differs between two apps, or one person's rows shown to
//! another.
//!
//! Needs the conformance cluster. Its own databases, prefixed
//! `ordo_idsvc_`.

mod oauth2_support;
mod oidc_support;

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use oauth2_support::{redeem, send};
use oidc_support::{param, CA, PUBLIC, RETURN};
use ordo_egress::{DenyList, TlsRoots};
use ordo_exec::PoolConfig;
use ordo_host::auth::AuthState;
use ordo_host::federation::MemoryIdentityAttrs;
use ordo_host::oidc::EgressFactory;
use ordo_identity::clock::Clock;
use ordo_identity::directory::{Directory, DirectoryStore, Provisioning};
use ordo_identity::jose::Alg;
use ordo_identity::oidc::{OidcConnection, OidcProvider, DEFAULT_SKEW_SECS};
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::b64url_encode;
use ordo_identity::{
    Argon2idHasher, AuthenticatorId, CompleteRequest, MemoryCeremonyStore,
    MemorySessionRevocations, ProviderRegistry, TokenKey,
};
use ordo_identity_service::config::{DATABASE_URL_ENV, DATA_DIR_ENV, ORIGIN_ENV};
use ordo_identity_service::deployments::{Registration, SignUp};
use ordo_identity_service::people::Actor;
use ordo_identity_service::{BootOptions, IdentityConfig, Service};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;

const NOW: u64 = 1_800_000_000;
const HOST: Actor<'static> = Actor {
    id: "unix:test",
    client: "host",
};

// ───────────────────────── the identity service ─────────────────────────

/// A real identity service, its issuer `https://localhost:<port>`.
struct Ids {
    svc: Service,
    issuer: String,
}

/// Every address `localhost` resolves to, on one port.
async fn localhost_listeners() -> (u16, Vec<TcpListener>) {
    let answers: Vec<std::net::SocketAddr> = tokio::net::lookup_host("localhost:0")
        .await
        .expect("localhost resolves")
        .collect();
    let first = *answers.first().expect("localhost answers an address");
    let tcp = TcpListener::bind((first.ip(), 0)).await.expect("bind");
    let port = tcp.local_addr().unwrap().port();
    let mut listeners = vec![tcp];
    for other in answers.iter().skip(1) {
        if other.ip() == first.ip() {
            continue;
        }
        if let Ok(l) = TcpListener::bind((other.ip(), port)).await {
            listeners.push(l);
        }
    }
    (port, listeners)
}

impl Ids {
    /// Boot a service on a fresh database, serve it over TLS, and register
    /// each `(handle, connection name)` as an open deployment whose return
    /// URL is that connection's callback at the app.
    async fn start(name: &str, apps: &[(&str, &str)]) -> Ids {
        let c = PoolConfig::from_env();
        let db = format!("ordo_idsvc_service_{name}");
        ordo_exec::drop_database(&c, &db).await.expect("drop");
        ordo_exec::create_database(&c, &db).await.expect("create");
        let mut url = format!(
            "host={} port={} user={} dbname={db}",
            c.host, c.port, c.user
        );
        if let Some(p) = &c.password {
            url.push_str(&format!(" password={p}"));
        }
        let dir = std::env::temp_dir().join(format!("ordo-idsvc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        let (port, listeners) = localhost_listeners().await;
        let issuer = format!("https://localhost:{port}");
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
        for (handle, connection) in apps {
            svc.register_deployment(
                HOST,
                &Registration {
                    handle: handle.to_string(),
                    // An app registers its callback alone: its pages are
                    // no client origin of the service.
                    client_origins: Vec::new(),
                    return_urls: vec![format!("{PUBLIC}/auth/callback/{connection}")],
                    sign_up: SignUp::Open,
                    consumer_key: None,
                },
            )
            .await
            .expect("registered");
        }

        // The service on plain loopback, and TLS in front of it.
        let plain = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let inner = plain.local_addr().unwrap();
        let app = svc
            .router()
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move {
            let _ = axum::serve(plain, app).await;
        });
        let acceptor = oidc_support::acceptor();
        for tcp in listeners {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                while let Ok((sock, _)) = tcp.accept().await {
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move {
                        let Ok(mut tls) = acceptor.accept(sock).await else {
                            return;
                        };
                        let Ok(mut out) = tokio::net::TcpStream::connect(inner).await else {
                            return;
                        };
                        let _ = tokio::io::copy_bidirectional(&mut tls, &mut out).await;
                    });
                }
            });
        }
        Ids { svc, issuer }
    }

    /// A browser call to the service: status, `Location`, JSON body,
    /// `Set-Cookie`.
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

    /// A passkey for this service: RP ID `localhost`, its origin.
    fn authenticator(&self, seed: u8) -> TestAuthenticator {
        TestAuthenticator {
            key: TestKey::es256(seed),
            credential_id: vec![seed; 16],
            rp_id: "localhost".to_string(),
            origin: self.issuer.clone(),
            ..TestAuthenticator::es256()
        }
    }

    /// Open sign-up into `deployment` on the service's page: the session
    /// cookie and the person's id there.
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
        let CompleteRequest::PasskeyRegistration {
            client_data_json,
            attestation_object,
            ..
        } = self.authenticator(seed).register(&challenge)
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

    /// The app's connection to this service, by discovery, as
    /// `ORDO_AUTH_<NAME>_KIND=oidc`, `_ISSUER`, `_CLIENT_ID=<handle>`,
    /// `_CLIENT_AUTH=none` build it.
    fn connection(&self, name: &str, handle: &str) -> OidcConnection {
        OidcConnection {
            name: name.into(),
            issuer: self.issuer.clone(),
            client_id: handle.into(),
            client_secret: None,
            scope: "openid profile".into(),
            endpoints: None,
            use_email: false,
            display_claims: vec!["name".into()],
            avatar_claim: None,
            skew_secs: DEFAULT_SKEW_SECS,
            algs: vec![Alg::EdDSA],
            cache_secs: ordo_identity::oidc::DEFAULT_CACHE_SECS,
            stale_grace_secs: ordo_identity::oidc::JWKS_STALE_GRACE_SECS,
            redirect_uri: format!("{PUBLIC}/auth/callback/{name}"),
            returns: ordo_host::federation::parse_return_urls(RETURN, false)
                .expect("a return list"),
            presence: Default::default(),
        }
    }
}

// ─────────────────────────────── the app ───────────────────────────────

struct App {
    router: axum::Router,
    auth: Arc<AuthState>,
    key: TokenKey,
    name: String,
}

/// An app's sign-in with one OIDC connection `name` to the service, as
/// client `handle`, its people in `store`.
fn app(ids: &Ids, name: &str, handle: &str, store: Arc<dyn DirectoryStore>, secret: &[u8]) -> App {
    let ceremonies = Arc::new(MemoryCeremonyStore::new());
    let factory = Arc::new(EgressFactory::new(
        TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
        DenyList::categories_only().permitting_loopback_for_tests(),
    ));
    let provider = Arc::new(
        OidcProvider::new(ids.connection(name, handle), ceremonies.clone(), factory)
            .expect("a valid connection"),
    );
    let mut registry = ProviderRegistry::new();
    registry
        .register(name, Box::new(provider.clone()))
        .expect("registered");
    let mut state = AuthState::new(
        registry,
        Directory::with_store(Provisioning::AutoOnboard, store),
        TokenKey::from_secret(secret),
    )
    .with_revocations(Arc::new(MemorySessionRevocations::new()))
    .with_ceremonies(ceremonies)
    .with_oidc(provider)
    .with_identity_attrs(Arc::new(MemoryIdentityAttrs::default()));
    state.now = Box::new(|| NOW);
    let auth = Arc::new(state);
    App {
        router: ordo_host::auth::router(Some(auth.clone())),
        auth,
        key: TokenKey::from_secret(secret),
        name: name.into(),
    }
}

/// The whole sign-in, as a browser holding the service's `cookie` follows
/// it: the app's begin, the service's authorize, the app's callback, the
/// redemption. Answers the app's principal.
async fn sign_in(app: &App, ids: &Ids, cookie: Option<&str>) -> (StatusCode, Value) {
    let (status, _, begun) = send(
        &app.router,
        "POST",
        "/auth/begin",
        Some(json!({"method": app.name, "return_to": RETURN})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let url = begun["redirect"].as_str().unwrap().to_string();
    let binder = begun["binder"].as_str().unwrap().to_string();
    let path = url
        .strip_prefix(&ids.issuer)
        .unwrap_or_else(|| panic!("the service's authorize endpoint: {url}"));
    assert!(path.starts_with("/oidc/authorize?"), "{path}");
    let (status, location, _, _) = ids.browser("GET", path, cookie, None).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let location = location.expect("a location");
    let Some(query) = location.strip_prefix(&format!("{PUBLIC}/auth/callback/{}?", app.name))
    else {
        return (StatusCode::UNAUTHORIZED, json!({ "location": location }));
    };
    let (status, back, _) = send(
        &app.router,
        "GET",
        &format!("/auth/callback/{}?{query}", app.name),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let back = back.expect("a location");
    let handoff = param(&back, "handoff").unwrap_or_else(|| panic!("a handoff: {back}"));
    redeem(&app.router, &app.name, &handoff, &binder, json!({})).await
}

fn principal_of(app: &App, done: &Value) -> String {
    let token = done["token"].as_str().expect("a token");
    ordo_identity::verify(&app.key, token, ordo_identity::Timestamp(NOW))
        .expect("a token the app minted")
        .principal
        .0
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_signs_in_an_openly_signed_up_member_through_the_identity_service() {
    let ids = Ids::start("one_app", &[("notes", "accounts")]).await;
    let store = Arc::new(ordo_identity::MemoryDirectoryStore::new());
    let notes = app(
        &ids,
        "accounts",
        "notes",
        store,
        b"identity-service-oidc-one",
    );

    // Not signed in at the service: its page, for sign-up into notes.
    let (status, done) = sign_in(&notes, &ids, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let page = done["location"].as_str().unwrap();
    assert!(page.starts_with("/sign-in?deployment=notes&"), "{page}");
    assert_eq!(param(page, "signUp").as_deref(), Some("open"));

    let (cookie, dana) = ids.sign_up("notes", "Dana", 51).await;
    let (status, done) = sign_in(&notes, &ids, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["via"], "oidc:accounts");
    let principal = principal_of(&notes, &done);
    let dir = notes.auth.directory.lock().unwrap();
    assert_eq!(
        dir.principal_for(&AuthenticatorId(format!("oidc:accounts:{dana}")))
            .unwrap()
            .map(|p| p.0),
        Some(principal.clone()),
        "the person is `oidc:<connection>:<sub>`, `sub` their id at the service"
    );
    let p = dir
        .principal(&ordo_identity::PrincipalId(principal))
        .unwrap()
        .unwrap();
    assert_eq!(p.display, "Dana", "the name from the ID token");
}
