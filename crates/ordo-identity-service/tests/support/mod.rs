//! The harness the identity service's Postgres tests share: a fresh
//! database per test, a manual clock, the router in-process, and a software
//! authenticator for passkey ceremonies.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ordo_exec::PoolConfig;
use ordo_identity::clock::Clock;
use ordo_identity::deployment_token::Expect;
use ordo_identity::jose::Jwks;
use ordo_identity::software_authenticator::{TestAuthenticator, TestKey};
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use ordo_identity::{Argon2idHasher, CompleteRequest};
use ordo_identity_service::config::*;
use ordo_identity_service::people::{self, Actor};
use ordo_identity_service::sessions::{self, NewSession, Opened, SessionKind};
use ordo_identity_service::{BootOptions, IdentityConfig, IdentityError, Service};
use serde_json::{json, Value};
use tower::ServiceExt;

pub const START: u64 = 1_800_000_000;
pub const ORIGIN: &str = "http://localhost:8082";
pub const CONSOLE: &str = "http://localhost:8081";
pub const AUD: &str = "ordo-deployment:main";

pub fn cluster() -> PoolConfig {
    PoolConfig::from_env()
}

pub fn url_for(db: &str) -> String {
    let c = cluster();
    let mut url = format!(
        "host={} port={} user={} dbname={db}",
        c.host, c.port, c.user
    );
    if let Some(p) = &c.password {
        url.push_str(&format!(" password={p}"));
    }
    url
}

pub async fn fresh_db(name: &str) -> String {
    let db = format!("ordo_identity_t_{}", name.replace('-', "_"));
    let c = cluster();
    ordo_exec::drop_database(&c, &db).await.expect("drop");
    ordo_exec::create_database(&c, &db).await.expect("create");
    url_for(&db)
}

pub struct H {
    pub svc: Service,
    pub clock: Clock,
    pub vars: HashMap<String, String>,
}

pub fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ordo-identity-t-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

pub async fn boot_with(name: &str, extra: &[(&str, &str)]) -> H {
    let url = fresh_db(name).await;
    let dir = temp(name);
    let mut vars = HashMap::from([
        (DATABASE_URL_ENV.to_string(), url),
        (ORIGIN_ENV.to_string(), ORIGIN.to_string()),
        (DATA_DIR_ENV.to_string(), dir.display().to_string()),
        (CLIENT_ORIGINS_ENV.to_string(), CONSOLE.to_string()),
    ]);
    for (k, v) in extra {
        vars.insert(k.to_string(), v.to_string());
    }
    let clock = Clock::manual(START);
    let svc = reboot(&vars, &clock).await.expect("boot");
    H { svc, clock, vars }
}

pub async fn reboot(
    vars: &HashMap<String, String>,
    clock: &Clock,
) -> Result<Service, IdentityError> {
    let cfg = IdentityConfig::from_vars(&|k| vars.get(k).cloned())?;
    Service::boot(
        cfg,
        BootOptions {
            clock: clock.clone(),
            hasher: Arc::new(Argon2idHasher::weak_for_tests()),
        },
    )
    .await
}

/// A software authenticator for the service's relying party: RP ID
/// `localhost`, origin [`ORIGIN`].
pub fn authenticator(seed: u8, id: &[u8]) -> TestAuthenticator {
    TestAuthenticator {
        key: TestKey::es256(seed),
        credential_id: id.to_vec(),
        rp_id: "localhost".to_string(),
        origin: ORIGIN.to_string(),
        ..TestAuthenticator::es256()
    }
}

/// The browser's answer to a creation ceremony, as JSON.
pub fn registration(auth: &TestAuthenticator, challenge: &str) -> Value {
    let CompleteRequest::PasskeyRegistration {
        client_data_json,
        attestation_object,
        ..
    } = auth.register(challenge)
    else {
        unreachable!()
    };
    json!({
        "challenge": challenge,
        "clientDataJSON": b64url_encode(&client_data_json),
        "attestationObject": b64url_encode(&attestation_object),
    })
}

/// The browser's answer to a request ceremony, as JSON.
pub fn assertion(auth: &TestAuthenticator, challenge: &str) -> Value {
    let CompleteRequest::PasskeyAssertion {
        credential_id,
        client_data_json,
        authenticator_data,
        signature,
        ..
    } = auth.assert(challenge)
    else {
        unreachable!()
    };
    json!({
        "challenge": challenge,
        "credentialId": b64url_encode(&credential_id),
        "clientDataJSON": b64url_encode(&client_data_json),
        "authenticatorData": b64url_encode(&authenticator_data),
        "signature": b64url_encode(&signature),
    })
}

/// The challenge in a ceremony's options.
pub fn challenge(options: &Value) -> String {
    options["publicKey"]["challenge"]
        .as_str()
        .unwrap_or_else(|| panic!("no challenge in {options}"))
        .to_string()
}

/// The secret in a `Set-Cookie`, for a test to send back.
pub fn cookie_of(headers: &axum::http::HeaderMap) -> String {
    let set = headers["set-cookie"].to_str().unwrap();
    set.split(';').next().unwrap().to_string()
}

/// The secret of an invite link: what follows `#`.
pub fn secret_of(link: &str) -> String {
    link.split_once('#').expect("a fragment").1.to_string()
}

impl H {
    pub fn now(&self) -> u64 {
        self.clock.now()
    }

    /// A person and a browser session for them, touched now.
    pub async fn person(&self, name: &str) -> (String, Opened) {
        let pool = self.svc.store().pool();
        let tx = pool.begin().await.unwrap();
        let by = Actor {
            id: "boot",
            client: "",
        };
        let p = people::add(
            &tx,
            self.svc.handle(),
            self.svc.keys(),
            name,
            by,
            self.now(),
        )
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
                touched_at: Some(self.now()),
                must_enrol: false,
            },
            self.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (p.id, s)
    }

    pub async fn call(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Value,
    ) -> (StatusCode, Value, axum::http::HeaderMap) {
        self.call_from(None, method, path, headers, body).await
    }

    /// A POST from the service's own page, from a socket at `peer`.
    pub async fn post_from(
        &self,
        peer: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value, axum::http::HeaderMap) {
        self.call_from(Some(peer), "POST", path, &[("origin", ORIGIN)], body)
            .await
    }

    /// A call whose socket peer is `peer` (an address), or none.
    pub async fn call_from(
        &self,
        peer: Option<&str>,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Value,
    ) -> (StatusCode, Value, axum::http::HeaderMap) {
        let mut req = Request::builder().method(method).uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = req
            .header("content-type", "application/json")
            .body(Body::from(if body.is_null() {
                String::new()
            } else {
                body.to_string()
            }))
            .unwrap();
        let mut req = req;
        if let Some(p) = peer {
            let sock = std::net::SocketAddr::new(p.parse().unwrap(), 40000);
            req.extensions_mut()
                .insert(axum::extract::ConnectInfo(sock));
        }
        let res = self.svc.router().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v, headers)
    }

    /// A POST from the service's own page.
    pub async fn post(
        &self,
        path: &str,
        cookie: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value, axum::http::HeaderMap) {
        let mut headers = vec![("origin", ORIGIN)];
        if let Some(c) = cookie {
            headers.push(("cookie", c));
        }
        self.call("POST", path, &headers, body).await
    }

    /// A call with a bearer token.
    pub async fn bearer(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let auth = format!("Bearer {token}");
        let (s, v, _) = self
            .call(method, path, &[("authorization", &auth)], body)
            .await;
        (s, v)
    }

    pub async fn jwks(&self) -> Jwks {
        let (status, v, headers) = self
            .call("GET", "/.well-known/jwks.json", &[], Value::Null)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["cache-control"], "max-age=600");
        Jwks::parse(v.to_string().as_bytes()).expect("a JWKS")
    }

    pub fn expect(&self) -> Expect<'static> {
        Expect {
            issuer: ORIGIN,
            audience: AUD,
            now: self.now(),
            leeway_secs: 0,
        }
    }
}

pub fn segments(token: &str) -> (String, String) {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "a compact JWS");
    let text = |s: &str| String::from_utf8(b64url_decode(s).unwrap()).unwrap();
    (text(parts[0]), text(parts[1]))
}
