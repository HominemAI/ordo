//! A stand-in OIDC issuer: a real TLS listener on loopback that serves a
//! discovery document, a JWKS and a token endpoint, and signs real ID tokens.
//!
//! **Nothing here calls Google or Microsoft.** The listener presents the
//! `localhost` leaf signed by the test CA (`fixture/`), the issuer is
//! `https://localhost:<port>`, and the provider reaches it through the
//! deployment's own [`EgressFactory`] with that CA as its only anchor and a
//! deny list that permits loopback (`ordo-egress`'s `test-pin` feature).
//! Certificate validation stays on. Like `oauth2_support`, the listener binds
//! every address `localhost` resolves to on one port.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use ordo_egress::{DenyList, TlsRoots};
use ordo_host::oidc::EgressFactory;
use ordo_identity::federation::{percent_decode, ClientSecret};
use ordo_identity::jose::testing::Signer;
use ordo_identity::jose::ALL_ALGS;
use ordo_identity::oidc::{OidcConnection, OidcProvider, DEFAULT_SKEW_SECS};
use ordo_identity::CeremonyStore;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const CA: &[u8] = include_bytes!("../fixture/ca.der");
const LEAF: &[u8] = include_bytes!("../fixture/localhost.der");
const LEAF_KEY: &[u8] = include_bytes!("../fixture/localhost-key.der");
/// The RSA key the issuer signs with first, as Google does (RS256).
const RSA_KEY: &[u8] = include_bytes!("../../../ordo-identity/tests/fixture/rsa-2048.pk8");

pub const CLIENT_SECRET: &str = "oidc-client-secret-do-not-print";
pub const CLIENT_ID: &str = "ordo-client-7";
pub const RETURN: &str = "https://app.example.com/signed-in";
pub const PUBLIC: &str = "https://api.example.com";

/// What the issuer holds and does. Tests change it between sign-ins.
pub struct IdpState {
    /// The published keys, by kid, and which one signs.
    pub keys: Vec<(String, Arc<Signer>)>,
    pub signing: String,
    /// The discovery document's `issuer`, normally the real one.
    pub advertised_issuer: Option<String>,
    /// Codes the person has authorized: the claims, the nonce of the
    /// authorize request, and the redirect URI it named.
    codes: HashMap<String, (Value, String)>,
    pub jwks_fetches: usize,
    pub token_requests: Vec<HashMap<String, String>>,
    pub lines: Vec<String>,
}

pub struct FakeIdp {
    pub port: u16,
    pub state: Arc<Mutex<IdpState>>,
}

impl FakeIdp {
    pub async fn start() -> FakeIdp {
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
        let state = Arc::new(Mutex::new(IdpState {
            keys: vec![("rsa-1".into(), Arc::new(Signer::rsa(RSA_KEY)))],
            signing: "rsa-1".into(),
            advertised_issuer: None,
            codes: HashMap::new(),
            jwks_fetches: 0,
            token_requests: Vec::new(),
            lines: Vec::new(),
        }));
        let issuer = format!("https://localhost:{port}");
        let acceptor = acceptor();
        for tcp in listeners {
            let (acceptor, st, iss) = (acceptor.clone(), state.clone(), issuer.clone());
            tokio::spawn(async move {
                loop {
                    let Ok((sock, _)) = tcp.accept().await else {
                        return;
                    };
                    let (acceptor, st, iss) = (acceptor.clone(), st.clone(), iss.clone());
                    tokio::spawn(async move {
                        let Ok(mut tls) = acceptor.accept(sock).await else {
                            return;
                        };
                        let Ok((head, body)) = read_request(&mut tls).await else {
                            return;
                        };
                        let response = answer(&head, &body, &iss, &st);
                        let _ = tls.write_all(response.as_bytes()).await;
                        let _ = tls.flush().await;
                        let _ = tls.shutdown().await;
                    });
                }
            });
        }
        FakeIdp { port, state }
    }

    pub fn issuer(&self) -> String {
        format!("https://localhost:{}", self.port)
    }

    /// The person signs in at the issuer, in answer to the authorize URL
    /// `begin` produced. From then on `code` exchanges for an ID token with
    /// `claims`, plus the issuer, audience, nonce and times the issuer adds.
    pub fn authorize(&self, authorize_url: &str, code: &str, claims: Value) {
        let nonce = param(authorize_url, "nonce").expect("a nonce in the authorize URL");
        self.state
            .lock()
            .unwrap()
            .codes
            .insert(code.to_string(), (claims, nonce));
    }

    /// Publish `signer` under `kid`, sign with it from now on, and drop
    /// every other key.
    pub fn rotate_to(&self, kid: &str, signer: Signer) {
        let mut st = self.state.lock().unwrap();
        st.keys = vec![(kid.to_string(), Arc::new(signer))];
        st.signing = kid.to_string();
    }

    /// A connection to this issuer, by discovery.
    pub fn connection(&self, name: &str) -> OidcConnection {
        OidcConnection {
            name: name.into(),
            issuer: self.issuer(),
            client_id: CLIENT_ID.into(),
            client_secret: ClientSecret::new(CLIENT_SECRET.into()),
            scope: "openid email profile".into(),
            endpoints: None,
            use_email: true,
            display_claims: vec!["name".into(), "preferred_username".into()],
            avatar_claim: None,
            skew_secs: DEFAULT_SKEW_SECS,
            algs: ALL_ALGS.to_vec(),
            cache_secs: ordo_identity::oidc::DEFAULT_CACHE_SECS,
            stale_grace_secs: ordo_identity::oidc::JWKS_STALE_GRACE_SECS,
            redirect_uri: format!("{PUBLIC}/auth/callback/{name}"),
            returns: ordo_host::federation::parse_return_urls(RETURN, false)
                .expect("a return list"),
            presence: Default::default(),
        }
    }

    /// The provider a deployment would build, over the egress kernel.
    pub fn provider(&self, name: &str, ceremonies: Arc<dyn CeremonyStore>) -> Arc<OidcProvider> {
        self.provider_with(
            name,
            ceremonies,
            DenyList::categories_only().permitting_loopback_for_tests(),
        )
    }

    /// A provider whose two-step claim counts under `rule` (step 10).
    pub fn provider_with_presence(
        &self,
        name: &str,
        ceremonies: Arc<dyn CeremonyStore>,
        rule: ordo_identity::PresenceRule,
    ) -> Arc<OidcProvider> {
        let factory = Arc::new(EgressFactory::new(
            TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
            DenyList::categories_only().permitting_loopback_for_tests(),
        ));
        let mut conn = self.connection(name);
        conn.presence = rule;
        Arc::new(OidcProvider::new(conn, ceremonies, factory).expect("a valid connection"))
    }

    /// The same, under a deny list of the caller's choosing.
    pub fn provider_with(
        &self,
        name: &str,
        ceremonies: Arc<dyn CeremonyStore>,
        deny: DenyList,
    ) -> Arc<OidcProvider> {
        let factory = Arc::new(EgressFactory::new(
            TlsRoots::custom(&[CA.to_vec()]).expect("anchor"),
            deny,
        ));
        Arc::new(
            OidcProvider::new(self.connection(name), ceremonies, factory)
                .expect("a valid connection"),
        )
    }
}

fn acceptor() -> TlsAcceptor {
    let cert = rustls::pki_types::CertificateDer::from(LEAF.to_vec());
    let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).expect("pkcs8 key");
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    TlsAcceptor::from(Arc::new(cfg))
}

async fn read_request<S>(s: &mut S) -> io::Result<(String, Vec<u8>)>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let end = loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::other("closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let want: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < want {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Ok((head, body))
}

fn form_of(body: &[u8]) -> HashMap<String, String> {
    String::from_utf8_lossy(body)
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| {
            (
                percent_decode(k).unwrap_or_default(),
                percent_decode(v).unwrap_or_default(),
            )
        })
        .collect()
}

fn json_response(status: &str, body: &Value) -> String {
    let text = body.to_string();
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    )
}

fn answer(head: &str, body: &[u8], issuer: &str, st: &Mutex<IdpState>) -> String {
    let line = head.lines().next().unwrap_or_default().to_string();
    let mut st = st.lock().unwrap();
    st.lines.push(line.clone());
    if line.starts_with("GET /.well-known/openid-configuration ") {
        let advertised = st
            .advertised_issuer
            .clone()
            .unwrap_or_else(|| issuer.to_string());
        return json_response(
            "200 OK",
            &json!({
                "issuer": advertised,
                "authorization_endpoint": format!("{issuer}/authorize"),
                "token_endpoint": format!("{issuer}/token"),
                "jwks_uri": format!("{issuer}/jwks"),
                "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"],
                "id_token_signing_alg_values_supported": ["RS256", "ES256"],
            }),
        );
    }
    if line.starts_with("GET /jwks ") {
        st.jwks_fetches += 1;
        let keys: Vec<Value> = st.keys.iter().map(|(kid, s)| s.jwk(kid)).collect();
        return json_response("200 OK", &json!({ "keys": keys }));
    }
    if line.starts_with("POST /token ") {
        let form = form_of(body);
        st.token_requests.push(form.clone());
        let ok = form.get("grant_type").map(String::as_str) == Some("authorization_code")
            && form.get("client_id").map(String::as_str) == Some(CLIENT_ID)
            && form.get("client_secret").map(String::as_str) == Some(CLIENT_SECRET)
            && form.get("code_verifier").is_some_and(|v| v.len() >= 43)
            && form
                .get("redirect_uri")
                .is_some_and(|r| r.starts_with(PUBLIC));
        let code = form.get("code").cloned().unwrap_or_default();
        let Some((claims, nonce)) = st.codes.remove(&code).filter(|_| ok) else {
            return json_response("400 Bad Request", &json!({"error": "invalid_grant"}));
        };
        let now = claims["iat"].as_u64().expect("the test sets iat");
        let mut full = json!({"iss": issuer, "aud": CLIENT_ID, "nonce": nonce,
                              "iat": now, "exp": now + 3600});
        for (k, v) in claims.as_object().expect("claims") {
            full[k] = v.clone();
        }
        let (kid, signer) = st
            .keys
            .iter()
            .find(|(k, _)| *k == st.signing)
            .cloned()
            .expect("the signing key is published");
        return json_response(
            "200 OK",
            &json!({"access_token": "at-never-used", "token_type": "Bearer",
                    "expires_in": 3599, "refresh_token": "rt-never-kept",
                    "id_token": signer.sign(&kid, &full)}),
        );
    }
    json_response("404 Not Found", &json!({}))
}

/// A query parameter of a URL, decoded.
pub fn param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| percent_decode(v))
}
