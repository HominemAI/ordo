//! The OIDC provider against an in-memory issuer. One test per pin the plan
//! names (O3 to O6, O11), each written so that removing its check makes it
//! fail; the verifier's own pins (O7, O8, O17) are tested in `jose.rs`.

use super::*;
use crate::ceremony::MemoryCeremonyStore;
use crate::federation::{FederationRequest, FederationResponse};
use crate::jose::testing::Signer;
use crate::jose::ALL_ALGS;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

const ISSUER: &str = "https://idp.example.com";
const CLIENT: &str = "client-1";
const NOW: u64 = 1_800_000_000;

/// An issuer in memory. Answers discovery, the JWKS and the token endpoint.
struct Issuer {
    discovery: Mutex<Value>,
    jwks: Mutex<Option<Value>>,
    id_token: Mutex<String>,
    jwks_fetches: AtomicUsize,
    discovery_fetches: AtomicUsize,
    /// `Cache-Control: max-age` on every answer, when set.
    max_age: Mutex<Option<u64>>,
    /// Discovery answers a connection failure.
    discovery_down: Mutex<bool>,
    token_forms: Mutex<Vec<String>>,
    /// The token endpoint answers this status (and whether the egress
    /// kernel would retry it) instead of a token, when set.
    token_status: Mutex<Option<(u16, bool)>>,
    hosts_asked: Mutex<Vec<Vec<String>>>,
}

impl Issuer {
    fn new(jwks: Value) -> Arc<Issuer> {
        Arc::new(Issuer {
            discovery: Mutex::new(json!({
                "issuer": ISSUER,
                "authorization_endpoint": "https://idp.example.com/authorize",
                "token_endpoint": "https://tokens.example.com/token",
                "jwks_uri": "https://keys.example.com/jwks",
                "response_types_supported": ["code", "id_token"],
                "code_challenge_methods_supported": ["S256"],
            })),
            jwks: Mutex::new(Some(jwks)),
            id_token: Mutex::new(String::new()),
            jwks_fetches: AtomicUsize::new(0),
            discovery_fetches: AtomicUsize::new(0),
            max_age: Mutex::new(None),
            discovery_down: Mutex::new(false),
            token_forms: Mutex::new(Vec::new()),
            token_status: Mutex::new(None),
            hosts_asked: Mutex::new(Vec::new()),
        })
    }
}

struct Wire(Arc<Issuer>);

impl FederationTransport for Wire {
    fn fetch(&self, req: &FederationRequest<'_>) -> Result<FederationResponse, ProviderError> {
        let i = &self.0;
        let body = match req.url {
            u if u.ends_with("/.well-known/openid-configuration") => {
                i.discovery_fetches.fetch_add(1, Ordering::SeqCst);
                if *i.discovery_down.lock().unwrap() {
                    return Err(ProviderError::unreachable("connect"));
                }
                i.discovery.lock().unwrap().to_string()
            }
            "https://keys.example.com/jwks" => {
                i.jwks_fetches.fetch_add(1, Ordering::SeqCst);
                match &*i.jwks.lock().unwrap() {
                    Some(v) => v.to_string(),
                    None => return Err(ProviderError::unreachable("connect")),
                }
            }
            "https://tokens.example.com/token" => {
                if let Some((status, retryable)) = *i.token_status.lock().unwrap() {
                    return Err(ProviderError::answered(status, retryable));
                }
                let mut form = form_encode(&req.form);
                if let Some(s) = req.client_secret {
                    form.push_str(&format!("&client_secret={}", s.reveal_to_wire()));
                }
                i.token_forms.lock().unwrap().push(form);
                json!({"access_token": "at", "token_type": "Bearer",
                       "id_token": *i.id_token.lock().unwrap()})
                .to_string()
            }
            _ => return Err(ProviderError::unreachable("not-allowed")),
        };
        Ok(FederationResponse::new(body.into_bytes()).with_max_age(*i.max_age.lock().unwrap()))
    }
}

impl TransportFactory for Arc<Issuer> {
    fn for_hosts(&self, hosts: &[String]) -> Arc<dyn FederationTransport> {
        self.hosts_asked.lock().unwrap().push(hosts.to_vec());
        Arc::new(Wire(self.clone()))
    }
}

fn connection(name: &str) -> OidcConnection {
    OidcConnection {
        name: name.into(),
        issuer: ISSUER.into(),
        client_id: CLIENT.into(),
        client_secret: ClientSecret::new("s3cret".into()),
        scope: "openid email profile".into(),
        endpoints: None,
        use_email: true,
        display_claims: PLAIN_DISPLAY_CLAIMS.iter().map(|s| s.to_string()).collect(),
        avatar_claim: Some("picture".into()),
        skew_secs: DEFAULT_SKEW_SECS,
        algs: ALL_ALGS.to_vec(),
        cache_secs: DEFAULT_CACHE_SECS,
        stale_grace_secs: JWKS_STALE_GRACE_SECS,
        redirect_uri: format!("https://api.example.com/auth/callback/{name}"),
        returns: crate::origin::OriginList::parse("https://app.example.com/in", true, false)
            .expect("a return list"),
        presence: Default::default(),
    }
}

struct H {
    p: OidcProvider,
    issuer: Arc<Issuer>,
    key: Signer,
}

fn harness() -> H {
    harness_on(connection("corp"))
}

fn harness_on(conn: OidcConnection) -> H {
    let key = Signer::es256(1);
    let issuer = Issuer::new(json!({"keys": [key.jwk("k1")]}));
    let p = OidcProvider::new(
        conn,
        Arc::new(MemoryCeremonyStore::new()),
        Arc::new(issuer.clone()),
    )
    .expect("a valid connection");
    H { p, issuer, key }
}

fn param(url: &str, key: &str) -> String {
    let q = url.split_once('?').unwrap().1;
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| crate::federation::percent_decode(v).unwrap())
        .unwrap()
}

/// `(state, nonce, binder)` from a `begin` at `now`.
fn begin(h: &H, now: u64) -> (String, String, String) {
    let Ok(ChallengeOrRedirect::Redirect { url, binder }) = h.p.begin(
        &BeginRequest {
            identifier: None,
            return_to: Some("https://app.example.com/in".into()),
            presence: false,
        },
        Timestamp(now),
    ) else {
        panic!("a redirect")
    };
    (param(&url, "state"), param(&url, "nonce"), binder)
}

fn claims(nonce: &str) -> Value {
    json!({"iss": ISSUER, "aud": CLIENT, "sub": "248289761001", "nonce": nonce,
           "iat": NOW, "exp": NOW + 600, "name": "Nelly Example",
           "email": "nelly@example.com", "email_verified": true,
           "picture": "https://idp.example.com/p/nelly.png"})
}

/// Run a whole sign-in with the claims `edit` makes from the good ones.
fn sign_in_with(h: &H, edit: impl FnOnce(&mut Value)) -> Result<AuthenticatedSubject, AuthError> {
    let (state, nonce, binder) = begin(h, NOW);
    let mut c = claims(&nonce);
    edit(&mut c);
    *h.issuer.id_token.lock().unwrap() = h.key.sign("k1", &c);
    let out = h.p.callback(
        &OidcCallback {
            state: &state,
            code: Some("code-1"),
            ..Default::default()
        },
        Timestamp(NOW),
    );
    let handoff = out.result?;
    h.p.complete(
        &CompleteRequest::Handoff { handoff, binder },
        Timestamp(NOW + 1),
    )
}

#[test]
fn a_token_endpoint_outage_or_rate_limit_is_unavailable_not_refused() {
    let h = harness();
    for (status, retryable, want) in [
        (
            503u16,
            true,
            AuthError::UpstreamUnavailable("provider-busy"),
        ),
        (429, true, AuthError::UpstreamUnavailable("provider-busy")),
        (
            400,
            false,
            AuthError::UpstreamRefused("token-exchange-rejected"),
        ),
    ] {
        *h.issuer.token_status.lock().unwrap() = Some((status, retryable));
        let err = sign_in_with(&h, |_| {}).expect_err("no token");
        assert_eq!(err, want, "{status}");
    }
}

#[test]
fn a_round_trip_yields_the_subject_and_its_profile() {
    let h = harness();
    let s = sign_in_with(&h, |_| {}).expect("signed in");
    assert_eq!(s.authenticator.0, "oidc:corp:248289761001");
    assert_eq!(s.method, AuthMethodKind::Oidc);
    assert!(s.presence.is_none());
    assert_eq!(s.attributes.display.as_deref(), Some("Nelly Example"));
    assert_eq!(
        s.attributes.email.as_ref().map(|e| e.address()),
        Some("nelly@example.com")
    );
    let forms = h.issuer.token_forms.lock().unwrap();
    assert!(forms[0].contains("code_verifier=") && forms[0].contains("client_secret=s3cret"));
    assert!(
        forms[0].contains("redirect_uri=https%3A%2F%2Fapi.example.com%2Fauth%2Fcallback%2Fcorp")
    );
    // Discovery was asked of the issuer's host alone; the endpoints it named
    // got their own allowlist.
    let asked = h.issuer.hosts_asked.lock().unwrap();
    assert_eq!(asked[0], vec!["idp.example.com".to_string()]);
    assert_eq!(
        asked[1],
        vec![
            "keys.example.com".to_string(),
            "tokens.example.com".to_string()
        ]
    );
}

#[test]
fn begin_asks_for_a_code_with_pkce_and_a_nonce() {
    let h = harness();
    let Ok(ChallengeOrRedirect::Redirect { url, .. }) = h.p.begin(
        &BeginRequest {
            identifier: None,
            return_to: Some("https://app.example.com/in".into()),
            presence: false,
        },
        Timestamp(NOW),
    ) else {
        panic!()
    };
    assert!(url.starts_with("https://idp.example.com/authorize?response_type=code&"));
    assert_eq!(param(&url, "code_challenge_method"), "S256");
    assert_eq!(param(&url, "scope"), "openid email profile");
    assert!(param(&url, "nonce").len() >= 43);
}

#[test]
fn o3_the_nonce_must_be_this_ceremonys_exactly() {
    let h = harness();
    for bad in [json!("another-sessions-nonce"), json!(null)] {
        let r = sign_in_with(&h, |c| c["nonce"] = bad.clone());
        assert_eq!(r.err(), Some(AuthError::UpstreamRefused("id-token-nonce")));
    }
    let r = sign_in_with(&h, |c| {
        c.as_object_mut().unwrap().remove("nonce");
    });
    assert_eq!(r.err(), Some(AuthError::UpstreamRefused("id-token-nonce")));
}

#[test]
fn o4_the_issuer_must_match_byte_for_byte() {
    let h = harness();
    for bad in [
        "https://idp.example.com/",
        "https://IDP.example.com",
        "https://evil.example.com",
    ] {
        let r = sign_in_with(&h, |c| c["iss"] = json!(bad));
        assert_eq!(
            r.err(),
            Some(AuthError::UpstreamRefused("id-token-issuer")),
            "{bad}"
        );
    }
}

#[test]
fn o5_the_audience_must_name_this_client_and_azp_settles_several() {
    let h = harness();
    let r = sign_in_with(&h, |c| c["aud"] = json!("another-client"));
    assert_eq!(
        r.err(),
        Some(AuthError::UpstreamRefused("id-token-audience"))
    );
    let r = sign_in_with(&h, |c| c["aud"] = json!([CLIENT, "another-client"]));
    assert_eq!(r.err(), Some(AuthError::UpstreamRefused("id-token-azp")));
    let r = sign_in_with(&h, |c| {
        c["aud"] = json!([CLIENT, "another-client"]);
        c["azp"] = json!("another-client");
    });
    assert_eq!(r.err(), Some(AuthError::UpstreamRefused("id-token-azp")));
    assert!(sign_in_with(&h, |c| {
        c["aud"] = json!([CLIENT, "another-client"]);
        c["azp"] = json!(CLIENT);
    })
    .is_ok());
    assert!(sign_in_with(&h, |c| c["aud"] = json!([CLIENT])).is_ok());
}

#[test]
fn o6_expiry_and_issue_time_hold_within_the_skew() {
    let h = harness();
    // Expired more than the skew ago.
    let r = sign_in_with(&h, |c| c["exp"] = json!(NOW - DEFAULT_SKEW_SECS));
    assert_eq!(
        r.err(),
        Some(AuthError::UpstreamRefused("id-token-expired"))
    );
    // Expired, but inside the skew.
    assert!(sign_in_with(&h, |c| c["exp"] = json!(NOW - DEFAULT_SKEW_SECS + 1)).is_ok());
    // Issued in the future, past the skew.
    let r = sign_in_with(&h, |c| c["iat"] = json!(NOW + DEFAULT_SKEW_SECS + 1));
    assert_eq!(
        r.err(),
        Some(AuthError::UpstreamRefused("id-token-issued-in-the-future"))
    );
    assert!(sign_in_with(&h, |c| c["iat"] = json!(NOW + DEFAULT_SKEW_SECS)).is_ok());
    let r = sign_in_with(&h, |c| c["nbf"] = json!(NOW + DEFAULT_SKEW_SECS + 1));
    assert_eq!(
        r.err(),
        Some(AuthError::UpstreamRefused("id-token-not-yet-valid"))
    );
    let r = sign_in_with(&h, |c| {
        c.as_object_mut().unwrap().remove("exp");
    });
    assert_eq!(
        r.err(),
        Some(AuthError::UpstreamRefused("id-token-exp-missing"))
    );
}

#[test]
fn o11_email_counts_only_when_email_verified_is_the_boolean_true() {
    let h = harness();
    for flag in [json!(false), json!("true"), json!(1), json!(null)] {
        let s = sign_in_with(&h, |c| c["email_verified"] = flag.clone()).expect("signed in");
        assert!(s.attributes.email.is_none(), "{flag}");
    }
    let s = sign_in_with(&h, |c| {
        c.as_object_mut().unwrap().remove("email_verified");
    })
    .expect("signed in");
    assert!(s.attributes.email.is_none());
    // A connection that does not use email keeps none, verified or not.
    let mut conn = connection("ms");
    conn.use_email = false;
    let attrs = attributes_of(&conn, claims("n").as_object().unwrap());
    assert!(attrs.email.is_none());
}

#[test]
fn o1_a_token_in_the_front_channel_is_refused() {
    let h = harness();
    let (state, _, _) = begin(&h, NOW);
    let out = h.p.callback(
        &OidcCallback {
            state: &state,
            code: Some("c"),
            front_channel_token: true,
            ..Default::default()
        },
        Timestamp(NOW),
    );
    assert_eq!(
        out.result,
        Err(AuthError::UpstreamRefused("front-channel-token"))
    );
    assert!(h.issuer.token_forms.lock().unwrap().is_empty());
}

#[test]
fn o14_the_iss_parameter_must_match_and_is_required_when_advertised() {
    let h = harness();
    let (state, _, _) = begin(&h, NOW);
    let out = h.p.callback(
        &OidcCallback {
            state: &state,
            code: Some("c"),
            iss: Some("https://evil.example.com"),
            ..Default::default()
        },
        Timestamp(NOW),
    );
    assert_eq!(out.result, Err(AuthError::UpstreamRefused("iss-parameter")));

    let key = Signer::es256(1);
    let issuer = Issuer::new(json!({"keys": [key.jwk("k1")]}));
    issuer.discovery.lock().unwrap()["authorization_response_iss_parameter_supported"] =
        json!(true);
    let h = H {
        p: OidcProvider::new(
            connection("corp"),
            Arc::new(MemoryCeremonyStore::new()),
            Arc::new(issuer.clone()),
        )
        .unwrap(),
        issuer,
        key,
    };
    let (state, _, _) = begin(&h, NOW);
    let out = h.p.callback(
        &OidcCallback {
            state: &state,
            code: Some("c"),
            ..Default::default()
        },
        Timestamp(NOW),
    );
    assert_eq!(
        out.result,
        Err(AuthError::UpstreamRefused("iss-parameter-missing"))
    );
}

#[test]
fn o15_discovery_must_name_the_configured_issuer_and_https_endpoints() {
    let ok = json!({"issuer": ISSUER, "authorization_endpoint": "https://a/x",
                    "token_endpoint": "https://t/x", "jwks_uri": "https://k/x"});
    assert!(parse_discovery(ok.to_string().as_bytes(), ISSUER).is_ok());
    assert_eq!(
        parse_discovery(ok.to_string().as_bytes(), "https://idp.example.com/"),
        Err("discovery-issuer-mismatch")
    );
    let mut plain = ok.clone();
    plain["token_endpoint"] = json!("http://t/x");
    assert_eq!(
        parse_discovery(plain.to_string().as_bytes(), ISSUER),
        Err("discovery-endpoint-not-https")
    );
    let mut no_s256 = ok.clone();
    no_s256["code_challenge_methods_supported"] = json!(["plain"]);
    assert_eq!(
        parse_discovery(no_s256.to_string().as_bytes(), ISSUER),
        Err("discovery-no-s256")
    );
    // A poisoned document never loads: begin is unavailable, and asks again
    // only after the retry floor.
    let h = harness();
    h.issuer.discovery.lock().unwrap()["issuer"] = json!("https://evil.example.com");
    let req = BeginRequest {
        identifier: None,
        return_to: Some("https://app.example.com/in".into()),
        presence: false,
    };
    assert_eq!(
        h.p.begin(&req, Timestamp(NOW)).err(),
        Some(AuthError::UpstreamUnavailable("discovery-issuer-mismatch"))
    );
    assert_eq!(
        h.p.begin(&req, Timestamp(NOW + 1)).err(),
        Some(AuthError::UpstreamUnavailable("discovery-issuer-mismatch")),
        "the last reason, without a second request inside the floor"
    );
    assert_eq!(h.issuer.discovery_fetches.load(Ordering::SeqCst), 1);
    // Corrected at the provider: loads after the floor, with no restart.
    h.issuer.discovery.lock().unwrap()["issuer"] = json!(ISSUER);
    assert!(h
        .p
        .begin(&req, Timestamp(NOW + JWKS_REFETCH_FLOOR_SECS))
        .is_ok());
}

/// Discovery is re-read on the cache's schedule. A refresh that fails keeps
/// the last good copy and the callback says so; a refresh that succeeds
/// moves the endpoints without a restart.
#[test]
fn discovery_refreshes_on_the_cache_schedule_and_keeps_the_last_good_copy() {
    let h = harness();
    assert!(sign_in_with(&h, |_| {}).is_ok());
    assert!(sign_in_with(&h, |_| {}).is_ok());
    assert_eq!(h.issuer.discovery_fetches.load(Ordering::SeqCst), 1);

    let run = |at: u64| {
        let (state, nonce, _) = begin(&h, at);
        let mut c = claims(&nonce);
        c["iat"] = json!(at);
        c["exp"] = json!(at + 600);
        *h.issuer.id_token.lock().unwrap() = h.key.sign("k1", &c);
        h.p.callback(
            &OidcCallback {
                state: &state,
                code: Some("c"),
                ..Default::default()
            },
            Timestamp(at),
        )
    };
    // Past freshness with the issuer's discovery down: the last good copy.
    *h.issuer.discovery_down.lock().unwrap() = true;
    let later = NOW + DEFAULT_CACHE_SECS + 10;
    let out = run(later);
    assert!(out.result.is_ok(), "{:?}", out.result);
    assert!(out.stale_discovery);
    assert_eq!(h.issuer.discovery_fetches.load(Ordering::SeqCst), 2);
    // Back up, and the document now names another token endpoint.
    *h.issuer.discovery_down.lock().unwrap() = false;
    h.issuer.discovery.lock().unwrap()["token_endpoint"] = json!("https://keys.example.com/jwks");
    let out = run(later + JWKS_REFETCH_FLOOR_SECS);
    assert_eq!(h.issuer.discovery_fetches.load(Ordering::SeqCst), 3);
    assert!(!out.stale_discovery);
    // The exchange went to the new endpoint (here the JWKS URL, which
    // answers no ID token): the endpoints moved without a restart.
    assert_eq!(
        out.result,
        Err(AuthError::UpstreamRefused(
            "token-response-without-id-token"
        ))
    );
}

/// The cache life is the setting, shortened by the provider's `max-age`,
/// never lengthened, never below the floor.
#[test]
fn the_cache_follows_max_age_inside_the_setting() {
    let fetches_after = |max_age: Option<u64>, cache_secs: u64, at: u64| {
        let key = Signer::es256(1);
        let issuer = Issuer::new(json!({"keys": [key.jwk("k1")]}));
        *issuer.max_age.lock().unwrap() = max_age;
        let mut conn = connection("corp");
        conn.cache_secs = cache_secs;
        let h = H {
            p: OidcProvider::new(
                conn,
                Arc::new(MemoryCeremonyStore::new()),
                Arc::new(issuer.clone()),
            )
            .unwrap(),
            issuer,
            key,
        };
        assert!(sign_in_with(&h, |_| {}).is_ok());
        let (state, nonce, _) = begin(&h, at);
        let mut c = claims(&nonce);
        c["iat"] = json!(at);
        c["exp"] = json!(at + 600);
        *h.issuer.id_token.lock().unwrap() = h.key.sign("k1", &c);
        let q = OidcCallback {
            state: &state,
            code: Some("c"),
            ..Default::default()
        };
        assert!(h.p.callback(&q, Timestamp(at)).result.is_ok());
        h.issuer.jwks_fetches.load(Ordering::SeqCst)
    };
    // `max-age=600` under the hour: refetched at 601 s, not at 599 s.
    assert_eq!(fetches_after(Some(600), 3600, NOW + 599), 1);
    assert_eq!(fetches_after(Some(600), 3600, NOW + 601), 2);
    // A day's `max-age` is capped by the setting.
    assert_eq!(fetches_after(Some(86_400), 3600, NOW + 3601), 2);
    // `no-store` cannot push below the floor.
    assert_eq!(fetches_after(Some(0), 3600, NOW + MIN_CACHE_SECS - 1), 1);
    // The setting alone.
    assert_eq!(fetches_after(None, 900, NOW + 901), 2);
    let mut c = connection("x");
    c.cache_secs = MIN_CACHE_SECS - 1;
    assert_eq!(c.check(), Err(OidcConnectionError::CacheOutOfRange));
}

#[test]
fn a_token_without_a_kid_verifies_against_a_sole_usable_key() {
    let h = harness();
    let (state, nonce, binder) = begin(&h, NOW);
    let header = json!({"alg": "ES256", "typ": "JWT"});
    *h.issuer.id_token.lock().unwrap() = h.key.sign_raw(
        crate::jose::Alg::ES256,
        header.to_string().as_bytes(),
        claims(&nonce).to_string().as_bytes(),
    );
    let q = OidcCallback {
        state: &state,
        code: Some("c"),
        ..Default::default()
    };
    let handoff = h.p.callback(&q, Timestamp(NOW)).result.expect("one key");
    assert!(h
        .p
        .complete(
            &CompleteRequest::Handoff { handoff, binder },
            Timestamp(NOW)
        )
        .is_ok());
    // With two P-256 keys published there is no choosing.
    *h.issuer.jwks.lock().unwrap() =
        Some(json!({"keys": [h.key.jwk("k1"), Signer::es256(5).jwk("k5")]}));
    let (state, nonce, _) = begin(&h, NOW + JWKS_REFETCH_FLOOR_SECS + DEFAULT_CACHE_SECS);
    let at = NOW + JWKS_REFETCH_FLOOR_SECS + DEFAULT_CACHE_SECS;
    let mut c = claims(&nonce);
    c["iat"] = json!(at);
    c["exp"] = json!(at + 600);
    *h.issuer.id_token.lock().unwrap() = h.key.sign_raw(
        crate::jose::Alg::ES256,
        header.to_string().as_bytes(),
        c.to_string().as_bytes(),
    );
    let q = OidcCallback {
        state: &state,
        code: Some("c"),
        ..Default::default()
    };
    assert_eq!(
        h.p.callback(&q, Timestamp(at)).result,
        Err(AuthError::UpstreamRefused("kid-missing"))
    );
}

#[test]
fn o9_an_unknown_kid_refetches_once_a_minute_and_a_removed_key_stops() {
    let h = harness();
    assert!(sign_in_with(&h, |_| {}).is_ok());
    assert_eq!(h.issuer.jwks_fetches.load(Ordering::SeqCst), 1);
    // A second sign-in inside the hour uses the cache.
    assert!(sign_in_with(&h, |_| {}).is_ok());
    assert_eq!(h.issuer.jwks_fetches.load(Ordering::SeqCst), 1);

    // The issuer rotates to k2. A token under k2 at the same instant cannot
    // refetch (the floor), so it is refused; a minute later it can.
    let k2 = Signer::es256(2);
    *h.issuer.jwks.lock().unwrap() = Some(json!({"keys": [k2.jwk("k2")]}));
    let run = |at: u64, key: &Signer, kid: &str| {
        let (state, nonce, binder) = begin(&h, at);
        let mut c = claims(&nonce);
        c["iat"] = json!(at);
        c["exp"] = json!(at + 600);
        *h.issuer.id_token.lock().unwrap() = key.sign(kid, &c);
        let handoff =
            h.p.callback(
                &OidcCallback {
                    state: &state,
                    code: Some("c"),
                    ..Default::default()
                },
                Timestamp(at),
            )
            .result?;
        h.p.complete(&CompleteRequest::Handoff { handoff, binder }, Timestamp(at))
    };
    assert_eq!(
        run(NOW + 10, &k2, "k2").err(),
        Some(AuthError::UpstreamRefused("kid-unknown"))
    );
    assert_eq!(h.issuer.jwks_fetches.load(Ordering::SeqCst), 1);
    assert!(run(NOW + 61, &k2, "k2").is_ok());
    assert_eq!(h.issuer.jwks_fetches.load(Ordering::SeqCst), 2);
    // k1 left the set at that fetch, so it no longer verifies.
    assert_eq!(
        run(NOW + 200, &h.key, "k1").err(),
        Some(AuthError::UpstreamRefused("kid-unknown"))
    );
}

#[test]
fn o10_a_warm_cache_rides_out_an_outage_for_a_day_and_a_cold_one_does_not() {
    let h = harness();
    assert!(sign_in_with(&h, |_| {}).is_ok());
    *h.issuer.jwks.lock().unwrap() = None;
    let run = |at: u64| {
        let (state, nonce, _) = begin(&h, at);
        let mut c = claims(&nonce);
        c["iat"] = json!(at);
        c["exp"] = json!(at + 600);
        *h.issuer.id_token.lock().unwrap() = h.key.sign("k1", &c);
        h.p.callback(
            &OidcCallback {
                state: &state,
                code: Some("c"),
                ..Default::default()
            },
            Timestamp(at),
        )
    };
    let stale = run(NOW + DEFAULT_CACHE_SECS + 5);
    assert!(stale.result.is_ok());
    assert!(stale.stale_keys, "the route records a stale verification");
    let gone = run(NOW + DEFAULT_CACHE_SECS + JWKS_STALE_GRACE_SECS);
    assert_eq!(
        gone.result,
        Err(AuthError::UpstreamUnavailable("jwks-expired"))
    );

    let cold = harness();
    *cold.issuer.jwks.lock().unwrap() = None;
    assert_eq!(
        sign_in_with(&cold, |_| {}).err(),
        Some(AuthError::UpstreamUnavailable("connect"))
    );
}

#[test]
fn a_state_is_good_once_and_only_at_its_own_connection() {
    let h = harness();
    let (state, nonce, _) = begin(&h, NOW);
    *h.issuer.id_token.lock().unwrap() = h.key.sign("k1", &claims(&nonce));
    // Another connection over the same ceremony store does not find it.
    let other = OidcProvider::new(
        connection("other"),
        h.p.ceremonies.clone(),
        Arc::new(h.issuer.clone()),
    )
    .unwrap();
    let q = OidcCallback {
        state: &state,
        code: Some("c"),
        ..Default::default()
    };
    let out = other.callback(&q, Timestamp(NOW));
    assert_eq!(out.return_to, None);
    assert!(out.result.is_err());
    assert!(h.p.callback(&q, Timestamp(NOW)).result.is_ok());
    assert_eq!(h.p.callback(&q, Timestamp(NOW)).return_to, None, "one-shot");
}

#[test]
fn issuers_that_are_templates_or_multi_tenant_are_refused() {
    for bad in [
        "https://login.microsoftonline.com/common/v2.0",
        "https://login.microsoftonline.com/organizations/v2.0",
        "https://login.microsoftonline.com/{tenantid}/v2.0",
    ] {
        let mut c = connection("ms");
        c.issuer = bad.into();
        assert_eq!(c.check(), Err(OidcConnectionError::TemplateIssuer), "{bad}");
    }
    assert_eq!(
        microsoft_issuer("9188040D-6C67-4C5B-B112-36A304B66DAD").as_deref(),
        Some("https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0")
    );
    assert_eq!(microsoft_issuer("contoso.onmicrosoft.com"), None);
    assert_eq!(microsoft_issuer("common"), None);
    let mut c = connection("x");
    c.scope = "email profile".into();
    assert_eq!(c.check(), Err(OidcConnectionError::ScopeWithoutOpenid));
    c.scope = "openid".into();
    c.skew_secs = 121;
    assert_eq!(c.check(), Err(OidcConnectionError::SkewOutOfRange));
}

#[test]
fn the_fingerprint_is_the_issuer() {
    let a = connection("corp");
    let mut b = connection("corp");
    b.client_id = "other".into();
    assert_eq!(a.fingerprint(), b.fingerprint());
    b.issuer = "https://idp2.example.com".into();
    assert_ne!(a.fingerprint(), b.fingerprint());
}

/// The stale grace is the connection's `stale_grace_secs`
/// (`ORDO_AUTH_OIDC_STALE_GRACE_SECS`), not a fixed day (audit 2026-10-11,
/// finding 19).
#[test]
fn o10_the_stale_grace_is_the_connections_setting() {
    let mut conn = connection("corp");
    conn.stale_grace_secs = 60;
    let h = harness_on(conn);
    assert!(sign_in_with(&h, |_| {}).is_ok());
    *h.issuer.jwks.lock().unwrap() = None;
    let run = |at: u64| {
        let (state, nonce, _) = begin(&h, at);
        let mut c = claims(&nonce);
        c["iat"] = json!(at);
        c["exp"] = json!(at + 600);
        *h.issuer.id_token.lock().unwrap() = h.key.sign("k1", &c);
        h.p.callback(
            &OidcCallback {
                state: &state,
                code: Some("c"),
                ..Default::default()
            },
            Timestamp(at),
        )
    };
    assert!(run(NOW + DEFAULT_CACHE_SECS + 5).result.is_ok());
    assert_eq!(
        run(NOW + DEFAULT_CACHE_SECS + 60).result,
        Err(AuthError::UpstreamUnavailable("jwks-expired"))
    );
    let mut bad = connection("corp");
    bad.stale_grace_secs = MAX_STALE_GRACE_SECS + 1;
    assert_eq!(bad.check(), Err(OidcConnectionError::StaleGraceOutOfRange));
}
