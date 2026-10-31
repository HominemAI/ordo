//! The JWS verifier OIDC sign-in uses: compact serialisation, four
//! algorithms, keys from a JWKS (`plan-auth-federated.md` §4.3, §9.1).
//!
//! **Pure.** Every function here is a function of bytes, keys and nothing
//! else. No clock, no socket, no cache: [`crate::oidc`] owns those. That is
//! what lets RFC 7515's own vectors test it.
//!
//! # What it accepts, and nothing more
//!
//! | Pin | Rule | Where |
//! |---|---|---|
//! | O7 | `alg` is one of `RS256`, `PS256`, `ES256`, `EdDSA`, and in the caller's allow-list. `none` and every `HS*` are refused by name. The key's `kty` and curve must match the algorithm family. A JWK that carries `alg` must carry this one, and one that carries `use` must say `sig`. An RSA modulus under 2048 bits is refused | [`Alg::parse`], [`verify_with_key`] |
//! | O8 | The key is chosen by `kid` from the JWKS the caller holds; a token without one is verified only when the set holds exactly one key usable for its `alg`. `jku`, `jwk`, `x5u` and `x5c` in a token's header are never read | [`verify_with_jwks`] |
//! | O17 | The payload is decoded only after its signature verified, from the bytes the signature covered, and is parsed once. A duplicate member name in the header or the payload is a refusal | [`Verified::claims`], [`strict_json`] |
//!
//! Also refused: a `crit` header (no extension is understood, so none may be
//! required), an encrypted token (`enc`), a nested token (`cty`), and any
//! serialisation but three dot-separated base64url segments.
//!
//! Every refusal is a fixed word ([`JoseError`]), safe for the security
//! stream: no byte of the token travels in one.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Why a token or a key was refused. A fixed word from this module's own
/// vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoseError(pub &'static str);

impl std::fmt::Display for JoseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for JoseError {}

/// The signature algorithms this verifier knows. A closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Alg {
    /// RSASSA-PKCS1-v1_5 with SHA-256.
    RS256,
    /// RSASSA-PSS with SHA-256 and MGF1 with SHA-256.
    PS256,
    /// ECDSA over P-256 with SHA-256.
    ES256,
    /// Ed25519 (RFC 8037).
    EdDSA,
}

/// Every algorithm the verifier knows, the default allow-list.
pub const ALL_ALGS: [Alg; 4] = [Alg::RS256, Alg::PS256, Alg::ES256, Alg::EdDSA];

impl Alg {
    pub fn as_str(self) -> &'static str {
        match self {
            Alg::RS256 => "RS256",
            Alg::PS256 => "PS256",
            Alg::ES256 => "ES256",
            Alg::EdDSA => "EdDSA",
        }
    }

    /// The header's `alg`. `none` and the `HS*` family are refused by name,
    /// before anything else is looked at: a verifier that held an RSA public
    /// key and accepted `HS256` would check an HMAC keyed with that public
    /// key, which anybody can compute.
    pub fn parse(word: &str) -> Result<Alg, JoseError> {
        match word {
            "RS256" => Ok(Alg::RS256),
            "PS256" => Ok(Alg::PS256),
            "ES256" => Ok(Alg::ES256),
            "EdDSA" => Ok(Alg::EdDSA),
            w if w.eq_ignore_ascii_case("none") => Err(JoseError("alg-none")),
            w if w.len() >= 2 && w[..2].eq_ignore_ascii_case("HS") => Err(JoseError("alg-hmac")),
            _ => Err(JoseError("alg-unknown")),
        }
    }
}

// ───────────────────────────── strict JSON ─────────────────────────────

/// Parse JSON, refusing a duplicate member name in any object at any depth
/// (O17). `serde_json` alone keeps the last of two equal names, and a parser
/// elsewhere that keeps the first would read another claim from the same
/// bytes.
pub fn strict_json(bytes: &[u8]) -> Result<Value, JoseError> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let v = serde::Deserialize::deserialize(&mut de)
        .map(|Strict(v)| v)
        .map_err(|e| {
            if e.to_string().contains(DUPLICATE) {
                JoseError("duplicate-member")
            } else {
                JoseError("json")
            }
        })?;
    de.end().map_err(|_| JoseError("json"))?;
    Ok(v)
}

const DUPLICATE: &str = "ordo-jose-duplicate-member";

struct Strict(Value);

impl<'de> serde::Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Strict, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> serde::de::Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JSON")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_string()))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if out.contains_key(&key) {
                return Err(serde::de::Error::custom(DUPLICATE));
            }
            let Strict(v) = map.next_value()?;
            out.insert(key, v);
        }
        Ok(Value::Object(out))
    }
}

// ─────────────────────────────── keys ───────────────────────────────

/// The public key a JWK names, in the shape its verifier takes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Material {
    /// Modulus and exponent, big-endian, no leading zero bytes.
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    /// SEC1 uncompressed point, `04 || x || y`.
    P256(Vec<u8>),
    Ed25519([u8; 32]),
}

/// One verification key from a JWKS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jwk {
    kid: Option<String>,
    alg: Option<String>,
    use_: Option<String>,
    key: Material,
}

fn b64(s: &str) -> Result<Vec<u8>, JoseError> {
    use base64::Engine as _;
    // Unpadded, strict: a padded or non-canonical segment is not this
    // serialisation.
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| JoseError("base64"))
}

fn strip_zeros(mut v: Vec<u8>) -> Vec<u8> {
    let lead = v.iter().take_while(|b| **b == 0).count();
    v.drain(..lead);
    v
}

impl Jwk {
    /// Parse one JWK. Keys of a type this verifier cannot use (`oct`, an
    /// EC curve other than P-256, X25519) are refused here, which the JWKS
    /// parser turns into "skipped": a symmetric key published beside the
    /// signing keys is never a candidate.
    pub fn parse(v: &Value) -> Result<Jwk, JoseError> {
        let o = v.as_object().ok_or(JoseError("jwk-not-an-object"))?;
        let text = |k: &str| -> Result<Option<String>, JoseError> {
            match o.get(k) {
                None => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(JoseError("jwk-member-not-a-string")),
            }
        };
        let field = |k: &str| -> Result<Vec<u8>, JoseError> {
            b64(&text(k)?.ok_or(JoseError("jwk-member-missing"))?)
        };
        let key = match text("kty")?.as_deref() {
            Some("RSA") => {
                let n = strip_zeros(field("n")?);
                let e = strip_zeros(field("e")?);
                if n.is_empty() || e.is_empty() {
                    return Err(JoseError("jwk-rsa-empty"));
                }
                Material::Rsa { n, e }
            }
            Some("EC") => {
                if text("crv")?.as_deref() != Some("P-256") {
                    return Err(JoseError("jwk-curve"));
                }
                let (x, y) = (field("x")?, field("y")?);
                if x.len() != 32 || y.len() != 32 {
                    return Err(JoseError("jwk-ec-coordinates"));
                }
                let mut sec1 = Vec::with_capacity(65);
                sec1.push(4);
                sec1.extend_from_slice(&x);
                sec1.extend_from_slice(&y);
                p256::ecdsa::VerifyingKey::from_sec1_bytes(&sec1)
                    .map_err(|_| JoseError("jwk-ec-point"))?;
                Material::P256(sec1)
            }
            Some("OKP") => {
                if text("crv")?.as_deref() != Some("Ed25519") {
                    return Err(JoseError("jwk-curve"));
                }
                let x: [u8; 32] = field("x")?
                    .try_into()
                    .map_err(|_| JoseError("jwk-okp-length"))?;
                Material::Ed25519(x)
            }
            _ => return Err(JoseError("jwk-kty")),
        };
        Ok(Jwk {
            kid: text("kid")?,
            alg: text("alg")?,
            use_: text("use")?,
            key,
        })
    }

    pub fn kid(&self) -> Option<&str> {
        self.kid.as_deref()
    }

    /// O7: this key may verify `alg`. Family, curve, the JWK's own `alg`
    /// and `use`, and the RSA floor.
    fn admits(&self, alg: Alg) -> Result<(), JoseError> {
        if let Some(own) = &self.alg {
            if own != alg.as_str() {
                return Err(JoseError("jwk-alg-mismatch"));
            }
        }
        if let Some(u) = &self.use_ {
            if u != "sig" {
                return Err(JoseError("jwk-use-not-sig"));
            }
        }
        match (&self.key, alg) {
            (Material::Rsa { n, .. }, Alg::RS256 | Alg::PS256) => {
                // Bits of the modulus, leading zeros already stripped.
                let bits = n.len() * 8 - n[0].leading_zeros() as usize;
                if bits < 2048 {
                    return Err(JoseError("rsa-key-too-small"));
                }
                Ok(())
            }
            (Material::P256(_), Alg::ES256) | (Material::Ed25519(_), Alg::EdDSA) => Ok(()),
            _ => Err(JoseError("key-type-mismatch")),
        }
    }

    fn verify(&self, alg: Alg, message: &[u8], sig: &[u8]) -> Result<(), JoseError> {
        let bad = JoseError("signature");
        match (&self.key, alg) {
            (Material::Rsa { n, e }, Alg::RS256 | Alg::PS256) => {
                let params = if alg == Alg::RS256 {
                    &ring::signature::RSA_PKCS1_2048_8192_SHA256
                } else {
                    &ring::signature::RSA_PSS_2048_8192_SHA256
                };
                ring::signature::RsaPublicKeyComponents { n, e }
                    .verify(params, message, sig)
                    .map_err(|_| bad)
            }
            (Material::P256(sec1), Alg::ES256) => {
                use p256::ecdsa::signature::Verifier as _;
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(sec1).map_err(|_| bad)?;
                // JWS carries `r || s`, 64 bytes, not DER (RFC 7518 §3.4).
                if sig.len() != 64 {
                    return Err(bad);
                }
                let sig = p256::ecdsa::Signature::from_slice(sig).map_err(|_| bad)?;
                key.verify(message, &sig).map_err(|_| bad)
            }
            (Material::Ed25519(x), Alg::EdDSA) => {
                let key = ed25519_dalek::VerifyingKey::from_bytes(x).map_err(|_| bad)?;
                let sig = ed25519_dalek::Signature::from_slice(sig).map_err(|_| bad)?;
                key.verify_strict(message, &sig).map_err(|_| bad)
            }
            _ => Err(JoseError("key-type-mismatch")),
        }
    }
}

/// A JWKS: the keys a connection's issuer publishes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Jwks {
    keys: Vec<Jwk>,
}

impl Jwks {
    /// Parse `{"keys": [...]}`. A key this verifier cannot use is skipped,
    /// not fatal: an issuer's set may hold an encryption key or a curve
    /// beside the signing keys. A set with no usable key is a refusal.
    pub fn parse(bytes: &[u8]) -> Result<Jwks, JoseError> {
        let v = strict_json(bytes)?;
        let list = v
            .get("keys")
            .and_then(Value::as_array)
            .ok_or(JoseError("jwks-without-keys"))?;
        let keys: Vec<Jwk> = list.iter().filter_map(|k| Jwk::parse(k).ok()).collect();
        if keys.is_empty() {
            return Err(JoseError("jwks-no-usable-key"));
        }
        Ok(Jwks { keys })
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Has the set a key under `kid`?
    pub fn has(&self, kid: &str) -> bool {
        self.keys.iter().any(|k| k.kid.as_deref() == Some(kid))
    }

    /// The one key usable for `alg`, for a token without a `kid`.
    fn sole_key_for(&self, alg: Alg) -> Result<&Jwk, JoseError> {
        let mut usable = self.keys.iter().filter(|k| k.admits(alg).is_ok());
        match (usable.next(), usable.next()) {
            (Some(k), None) => Ok(k),
            _ => Err(JoseError("kid-missing")),
        }
    }

    /// The one key under `kid`. Two keys under one `kid` are ambiguous and
    /// refused, so the choice of key never depends on the set's order.
    fn find(&self, kid: &str) -> Result<&Jwk, JoseError> {
        let mut found = self.keys.iter().filter(|k| k.kid.as_deref() == Some(kid));
        let first = found.next().ok_or(JoseError("kid-unknown"))?;
        if found.next().is_some() {
            return Err(JoseError("kid-ambiguous"));
        }
        Ok(first)
    }
}

// ─────────────────────────────── tokens ──────────────────────────────

/// The header members this verifier reads. Nothing else in a header is
/// looked at, `jku`, `jwk`, `x5u` and `x5c` included (O8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub alg: Alg,
    pub kid: Option<String>,
}

/// A token whose signature verified. The payload is reachable only through
/// this type, which only [`verify_with_key`] makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub header: Header,
    payload: Vec<u8>,
}

impl Verified {
    /// The payload bytes the signature covered, decoded.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// The payload as a JSON object of claims, parsed once from the verified
    /// bytes with duplicate names refused (O17).
    pub fn claims(&self) -> Result<Map<String, Value>, JoseError> {
        match strict_json(&self.payload)? {
            Value::Object(m) => Ok(m),
            _ => Err(JoseError("payload-not-an-object")),
        }
    }
}

/// The three segments and the decoded header, with every header rule
/// applied. Nothing in the payload is decoded here.
struct Parts<'a> {
    header: Header,
    signing_input: &'a [u8],
    payload_b64: &'a str,
    signature: Vec<u8>,
}

/// The largest token accepted. An ID token is a few kilobytes.
pub const TOKEN_CAP: usize = 16 * 1024;

fn parts<'a>(token: &'a str, allowed: &[Alg]) -> Result<Parts<'a>, JoseError> {
    if token.len() > TOKEN_CAP {
        return Err(JoseError("token-too-large"));
    }
    let mut segs = token.split('.');
    let (Some(h), Some(p), Some(s), None) = (segs.next(), segs.next(), segs.next(), segs.next())
    else {
        return Err(JoseError("not-compact-jws"));
    };
    if h.is_empty() {
        return Err(JoseError("not-compact-jws"));
    }
    let header = match strict_json(&b64(h)?)? {
        Value::Object(m) => m,
        _ => return Err(JoseError("header-not-an-object")),
    };
    // Refusals that hold whatever the algorithm.
    if header.contains_key("crit") {
        return Err(JoseError("crit-header"));
    }
    if header.contains_key("enc") {
        return Err(JoseError("encrypted-token"));
    }
    if header.contains_key("cty") {
        return Err(JoseError("nested-token"));
    }
    let alg = Alg::parse(
        header
            .get("alg")
            .and_then(Value::as_str)
            .ok_or(JoseError("alg-missing"))?,
    )?;
    // An empty signature is refused after the `alg` word, so an unsecured
    // token is refused as `alg-none` by name.
    if s.is_empty() {
        return Err(JoseError("not-compact-jws"));
    }
    if !allowed.contains(&alg) {
        return Err(JoseError("alg-not-allowed"));
    }
    let kid = match header.get("kid") {
        None => None,
        Some(Value::String(k)) => Some(k.clone()),
        Some(_) => return Err(JoseError("kid-not-a-string")),
    };
    Ok(Parts {
        header: Header { alg, kid },
        signing_input: &token.as_bytes()[..h.len() + 1 + p.len()],
        payload_b64: p,
        signature: b64(s)?,
    })
}

/// The header of a token, checked but unverified: what a caller needs to
/// know which key to fetch. Nothing it returns may be trusted for anything
/// else.
pub fn peek_header(token: &str, allowed: &[Alg]) -> Result<Header, JoseError> {
    parts(token, allowed).map(|p| p.header)
}

/// Verify a compact JWS under one given key. The payload is decoded only
/// once the signature over its encoded bytes has verified.
pub fn verify_with_key(token: &str, key: &Jwk, allowed: &[Alg]) -> Result<Verified, JoseError> {
    let p = parts(token, allowed)?;
    key.admits(p.header.alg)?;
    key.verify(p.header.alg, p.signing_input, &p.signature)?;
    let payload = b64(p.payload_b64)?;
    Ok(Verified {
        header: p.header,
        payload,
    })
}

/// Verify a compact JWS with the key its `kid` names in `jwks` (O8). The key
/// is never taken from the token.
///
/// A token without a `kid` is verified only when the set holds **exactly
/// one** key usable for the token's `alg` (family, curve, the JWK's own `alg`
/// and `use`, the RSA floor). With two or more there is no choosing, and it
/// is refused as `kid-missing`.
pub fn verify_with_jwks(token: &str, jwks: &Jwks, allowed: &[Alg]) -> Result<Verified, JoseError> {
    let header = peek_header(token, allowed)?;
    let key = match header.kid.as_deref() {
        Some(kid) => jwks.find(kid)?,
        None => jwks.sole_key_for(header.alg)?,
    };
    verify_with_key(token, key, allowed)
}

/// The algorithms named in a comma-separated list, for a setting. Refuses a
/// word it does not know, and `none` and `HS*` by name.
pub fn parse_alg_list(text: &str) -> Result<Vec<Alg>, JoseError> {
    let mut out = BTreeSet::new();
    for w in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        out.insert(Alg::parse(w)?);
    }
    if out.is_empty() {
        return Err(JoseError("alg-list-empty"));
    }
    Ok(out.into_iter().collect())
}

/// Signing, for tests in this crate and in others: a fake identity provider
/// has to mint the tokens a deployment verifies. Behind `test-support`; a
/// deployment never compiles it.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::*;
    use crate::webauthn::b64url_encode;

    /// A signing key, and the JWK that publishes its public half.
    pub enum Signer {
        Es256(p256::ecdsa::SigningKey),
        Ed25519(ed25519_dalek::SigningKey),
        Rsa(ring::signature::RsaKeyPair),
    }

    impl Signer {
        /// ES256 from 32 fixed bytes, so a test's keys are the same every run.
        pub fn es256(seed: u8) -> Signer {
            let mut bytes = [0x11u8; 32];
            bytes[0] = seed;
            Signer::Es256(p256::ecdsa::SigningKey::from_slice(&bytes).expect("a P-256 scalar"))
        }

        pub fn ed25519(seed: u8) -> Signer {
            Signer::Ed25519(ed25519_dalek::SigningKey::from_bytes(&[seed; 32]))
        }

        /// RSA from a PKCS#8 DER private key.
        pub fn rsa(pkcs8: &[u8]) -> Signer {
            Signer::Rsa(ring::signature::RsaKeyPair::from_pkcs8(pkcs8).expect("an RSA key"))
        }

        /// The algorithm this key signs with by default.
        pub fn alg(&self) -> Alg {
            match self {
                Signer::Es256(_) => Alg::ES256,
                Signer::Ed25519(_) => Alg::EdDSA,
                Signer::Rsa(_) => Alg::RS256,
            }
        }

        /// The public JWK, under `kid`.
        pub fn jwk(&self, kid: &str) -> Value {
            match self {
                Signer::Es256(k) => {
                    let point = k.verifying_key().to_encoded_point(false);
                    serde_json::json!({"kty": "EC", "crv": "P-256", "kid": kid, "use": "sig",
                        "x": b64url_encode(point.x().expect("x")),
                        "y": b64url_encode(point.y().expect("y"))})
                }
                Signer::Ed25519(k) => serde_json::json!({"kty": "OKP", "crv": "Ed25519",
                    "kid": kid, "x": b64url_encode(k.verifying_key().as_bytes())}),
                Signer::Rsa(k) => {
                    use ring::signature::KeyPair as _;
                    let public = k.public_key();
                    let (n, e) = rsa_components(public.as_ref());
                    serde_json::json!({"kty": "RSA", "kid": kid, "use": "sig",
                        "n": b64url_encode(&n), "e": b64url_encode(&e)})
                }
            }
        }

        /// Sign `header` and `payload`, both given as the exact bytes to
        /// encode, under `alg`.
        pub fn sign_raw(&self, alg: Alg, header: &[u8], payload: &[u8]) -> String {
            let input = format!("{}.{}", b64url_encode(header), b64url_encode(payload));
            let sig = match self {
                Signer::Es256(k) => {
                    use p256::ecdsa::signature::Signer as _;
                    let s: p256::ecdsa::Signature = k.sign(input.as_bytes());
                    s.to_bytes().to_vec()
                }
                Signer::Ed25519(k) => {
                    use ed25519_dalek::Signer as _;
                    k.sign(input.as_bytes()).to_bytes().to_vec()
                }
                Signer::Rsa(k) => {
                    let pad: &'static dyn ring::signature::RsaEncoding = match alg {
                        Alg::PS256 => &ring::signature::RSA_PSS_SHA256,
                        _ => &ring::signature::RSA_PKCS1_SHA256,
                    };
                    let mut sig = vec![0u8; k.public().modulus_len()];
                    k.sign(
                        pad,
                        &ring::rand::SystemRandom::new(),
                        input.as_bytes(),
                        &mut sig,
                    )
                    .expect("RSA signing");
                    sig
                }
            };
            format!("{input}.{}", b64url_encode(&sig))
        }

        /// A token with header `{"alg":…,"kid":…}` and these claims.
        pub fn sign(&self, kid: &str, claims: &Value) -> String {
            let header = serde_json::json!({"alg": self.alg().as_str(), "kid": kid, "typ": "JWT"});
            self.sign_raw(
                self.alg(),
                header.to_string().as_bytes(),
                claims.to_string().as_bytes(),
            )
        }
    }

    /// `n` and `e` from a DER `RSAPublicKey` (`SEQUENCE { INTEGER, INTEGER }`).
    fn rsa_components(der: &[u8]) -> (Vec<u8>, Vec<u8>) {
        fn read(der: &[u8]) -> (u8, &[u8], &[u8]) {
            let tag = der[0];
            let (len, head) = if der[1] < 0x80 {
                (der[1] as usize, 2)
            } else {
                let n = (der[1] & 0x7f) as usize;
                let mut len = 0usize;
                for b in &der[2..2 + n] {
                    len = (len << 8) | *b as usize;
                }
                (len, 2 + n)
            };
            (tag, &der[head..head + len], &der[head + len..])
        }
        let (_, seq, _) = read(der);
        let (_, n, rest) = read(seq);
        let (_, e, _) = read(rest);
        (strip_zeros(n.to_vec()), strip_zeros(e.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Signer;
    use super::*;
    use serde_json::json;

    fn jwks_of(keys: &[Value]) -> Jwks {
        Jwks::parse(json!({"keys": keys}).to_string().as_bytes()).expect("a JWKS")
    }

    #[test]
    fn each_algorithm_round_trips() {
        let rsa = Signer::rsa(include_bytes!("../tests/fixture/rsa-2048.pk8"));
        for s in [Signer::es256(1), Signer::ed25519(2), rsa] {
            let jwks = jwks_of(&[s.jwk("k1")]);
            let t = s.sign("k1", &json!({"sub": "x"}));
            let v = verify_with_jwks(&t, &jwks, &ALL_ALGS).expect("verifies");
            assert_eq!(v.claims().unwrap()["sub"], "x");
        }
    }

    #[test]
    fn ps256_verifies_under_an_rsa_key() {
        let s = Signer::rsa(include_bytes!("../tests/fixture/rsa-2048.pk8"));
        let jwks = jwks_of(&[s.jwk("k1")]);
        let t = s.sign_raw(
            Alg::PS256,
            br#"{"alg":"PS256","kid":"k1"}"#,
            br#"{"sub":"x"}"#,
        );
        assert!(verify_with_jwks(&t, &jwks, &ALL_ALGS).is_ok());
        assert_eq!(
            verify_with_jwks(&t, &jwks, &[Alg::RS256]).err(),
            Some(JoseError("alg-not-allowed"))
        );
    }

    /// O7: `none` and `HS*` are refused by name, and a key is used only for
    /// its own family. The classic confusion: an RSA public key's bytes used
    /// as an HMAC secret. There is no HMAC code here to fall into, and the
    /// header word is refused before a key is chosen.
    #[test]
    fn o7_the_algorithm_comes_from_the_allow_list_and_matches_the_key() {
        let s = Signer::es256(1);
        let jwks = jwks_of(&[s.jwk("k1")]);
        let body = br#"{"sub":"x"}"#;
        for (alg, word) in [
            ("none", "alg-none"),
            ("NONE", "alg-none"),
            ("HS256", "alg-hmac"),
            ("hs512", "alg-hmac"),
            ("RS1", "alg-unknown"),
        ] {
            let header = format!(r#"{{"alg":"{alg}","kid":"k1"}}"#);
            let t = s.sign_raw(Alg::ES256, header.as_bytes(), body);
            assert_eq!(
                verify_with_jwks(&t, &jwks, &ALL_ALGS).err(),
                Some(JoseError(word)),
                "{alg}"
            );
        }
        // An unsigned token, the classic shape.
        let unsigned = format!(
            "{}.{}.",
            crate::webauthn::b64url_encode(br#"{"alg":"none","kid":"k1"}"#),
            crate::webauthn::b64url_encode(body)
        );
        assert!(verify_with_jwks(&unsigned, &jwks, &ALL_ALGS).is_err());
        // An ES256 key named by a token that says EdDSA.
        let ed = Signer::ed25519(3);
        let t = ed.sign_raw(Alg::EdDSA, br#"{"alg":"EdDSA","kid":"k1"}"#, body);
        assert_eq!(
            verify_with_jwks(&t, &jwks, &ALL_ALGS).err(),
            Some(JoseError("key-type-mismatch"))
        );
        // A JWK pinned to another algorithm, or published for encryption.
        let mut pinned = s.jwk("k1");
        pinned["alg"] = json!("RS256");
        let t = s.sign("k1", &json!({"sub": "x"}));
        assert_eq!(
            verify_with_jwks(&t, &jwks_of(&[pinned]), &ALL_ALGS).err(),
            Some(JoseError("jwk-alg-mismatch"))
        );
        let mut enc = s.jwk("k1");
        enc["use"] = json!("enc");
        assert_eq!(
            verify_with_jwks(&t, &jwks_of(&[enc]), &ALL_ALGS).err(),
            Some(JoseError("jwk-use-not-sig"))
        );
    }

    #[test]
    fn o7_an_rsa_key_under_2048_bits_is_refused() {
        // A 1024-bit modulus. The rule fires before any signature is
        // checked, so the token needs no real RSA signature.
        let n = crate::webauthn::b64url_encode(&[0xC5; 128]);
        let jwks = jwks_of(&[json!({"kty": "RSA", "kid": "k1", "n": n, "e": "AQAB"})]);
        let t = Signer::es256(1).sign_raw(Alg::RS256, br#"{"alg":"RS256","kid":"k1"}"#, b"{}");
        assert_eq!(
            verify_with_jwks(&t, &jwks, &ALL_ALGS).err(),
            Some(JoseError("rsa-key-too-small"))
        );
    }

    /// O8: the key is the JWKS's, by `kid`. A header that carries its own
    /// key (`jwk`), or a URL to one (`jku`, `x5u`), changes nothing.
    #[test]
    fn o8_the_key_comes_from_the_jwks_by_kid_and_never_from_the_token() {
        let honest = Signer::es256(1);
        let attacker = Signer::es256(9);
        let jwks = jwks_of(&[honest.jwk("k1")]);
        let body = br#"{"sub":"victim"}"#;
        let embedded = json!({"alg": "ES256", "kid": "k1", "jwk": attacker.jwk("k1"),
            "jku": "https://evil.example/jwks", "x5u": "https://evil.example/cert"});
        let t = attacker.sign_raw(Alg::ES256, embedded.to_string().as_bytes(), body);
        assert_eq!(
            verify_with_jwks(&t, &jwks, &ALL_ALGS).err(),
            Some(JoseError("signature"))
        );
        let t = attacker.sign_raw(Alg::ES256, br#"{"alg":"ES256","kid":"k9"}"#, body);
        assert_eq!(
            verify_with_jwks(&t, &jwks, &ALL_ALGS).err(),
            Some(JoseError("kid-unknown"))
        );
        // No kid, and one usable key: that key, and only its signature.
        let t = honest.sign_raw(Alg::ES256, br#"{"alg":"ES256"}"#, body);
        assert!(verify_with_jwks(&t, &jwks, &ALL_ALGS).is_ok());
        let t = attacker.sign_raw(Alg::ES256, br#"{"alg":"ES256"}"#, body);
        assert_eq!(
            verify_with_jwks(&t, &jwks, &ALL_ALGS).err(),
            Some(JoseError("signature"))
        );
        // Two keys under one kid: ambiguous, whichever would verify.
        let twice = jwks_of(&[honest.jwk("k1"), attacker.jwk("k1")]);
        let t = honest.sign("k1", &json!({"sub": "x"}));
        assert_eq!(
            verify_with_jwks(&t, &twice, &ALL_ALGS).err(),
            Some(JoseError("kid-ambiguous"))
        );
    }

    /// O8 without a `kid`: accepted only when exactly one key in the set is
    /// usable for the token's algorithm.
    #[test]
    fn o8_a_token_without_a_kid_needs_exactly_one_usable_key() {
        let ec = Signer::es256(1);
        let ed = Signer::ed25519(2);
        let body = br#"{"sub":"x"}"#;
        let t = ec.sign_raw(Alg::ES256, br#"{"alg":"ES256"}"#, body);
        // One P-256 key beside an Ed25519 key: one usable for ES256.
        let mixed = jwks_of(&[ec.jwk("a"), ed.jwk("b")]);
        assert!(verify_with_jwks(&t, &mixed, &ALL_ALGS).is_ok());
        // Two P-256 keys: no choosing.
        let two = jwks_of(&[ec.jwk("a"), Signer::es256(3).jwk("b")]);
        assert_eq!(
            verify_with_jwks(&t, &two, &ALL_ALGS).err(),
            Some(JoseError("kid-missing"))
        );
        // A second P-256 key published for encryption is not usable, so one
        // remains.
        let mut enc = Signer::es256(3).jwk("b");
        enc["use"] = json!("enc");
        assert!(verify_with_jwks(&t, &jwks_of(&[ec.jwk("a"), enc]), &ALL_ALGS).is_ok());
        // No usable key at all.
        assert_eq!(
            verify_with_jwks(&t, &jwks_of(&[ed.jwk("b")]), &ALL_ALGS).err(),
            Some(JoseError("kid-missing"))
        );
    }

    /// O17: duplicate names in the payload or the header are refused, and
    /// the payload is read only from the bytes the signature covered.
    #[test]
    fn o17_a_duplicate_member_name_is_refused() {
        let s = Signer::es256(1);
        let jwks = jwks_of(&[s.jwk("k1")]);
        let t = s.sign_raw(
            Alg::ES256,
            br#"{"alg":"ES256","kid":"k1"}"#,
            br#"{"sub":"alice","sub":"admin"}"#,
        );
        let v = verify_with_jwks(&t, &jwks, &ALL_ALGS).expect("the signature is fine");
        assert_eq!(v.claims().err(), Some(JoseError("duplicate-member")));
        let t = s.sign_raw(
            Alg::ES256,
            br#"{"alg":"ES256","kid":"k1","kid":"k2"}"#,
            br#"{"sub":"x"}"#,
        );
        assert_eq!(
            verify_with_jwks(&t, &jwks, &ALL_ALGS).err(),
            Some(JoseError("duplicate-member"))
        );
        // Nested objects too.
        assert_eq!(
            strict_json(br#"{"a":{"b":1,"b":2}}"#).err(),
            Some(JoseError("duplicate-member"))
        );
        // A payload swapped after signing is a signature failure, never a
        // claim read.
        let good = s.sign("k1", &json!({"sub": "alice"}));
        let mut segs: Vec<&str> = good.split('.').collect();
        let forged = crate::webauthn::b64url_encode(br#"{"sub":"admin"}"#);
        segs[1] = &forged;
        assert_eq!(
            verify_with_jwks(&segs.join("."), &jwks, &ALL_ALGS).err(),
            Some(JoseError("signature"))
        );
    }

    #[test]
    fn crit_encryption_and_nesting_are_refused() {
        let s = Signer::es256(1);
        let jwks = jwks_of(&[s.jwk("k1")]);
        for (h, word) in [
            (
                r#"{"alg":"ES256","kid":"k1","crit":["exp"],"exp":1}"#,
                "crit-header",
            ),
            (
                r#"{"alg":"ES256","kid":"k1","enc":"A256GCM"}"#,
                "encrypted-token",
            ),
            (r#"{"alg":"ES256","kid":"k1","cty":"JWT"}"#, "nested-token"),
        ] {
            let t = s.sign_raw(Alg::ES256, h.as_bytes(), b"{}");
            assert_eq!(
                verify_with_jwks(&t, &jwks, &ALL_ALGS).err(),
                Some(JoseError(word)),
                "{h}"
            );
        }
        for bad in ["a.b", "a.b.c.d", "a.b.c.d.e", "", ".."] {
            assert!(verify_with_jwks(bad, &jwks, &ALL_ALGS).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_jwks_skips_keys_it_cannot_use_and_refuses_when_none_is_left() {
        let s = Signer::es256(1);
        let set = json!({"keys": [{"kty": "oct", "k": "c2VjcmV0", "kid": "h"}, s.jwk("k1")]});
        let jwks = Jwks::parse(set.to_string().as_bytes()).unwrap();
        assert_eq!(jwks.len(), 1);
        assert!(jwks.has("k1") && !jwks.has("h"));
        assert_eq!(
            Jwks::parse(br#"{"keys":[{"kty":"oct","k":"c2VjcmV0"}]}"#).err(),
            Some(JoseError("jwks-no-usable-key"))
        );
    }

    #[test]
    fn an_algorithm_list_refuses_none_and_hmac_by_name() {
        assert_eq!(
            parse_alg_list("RS256, ES256").unwrap(),
            vec![Alg::RS256, Alg::ES256]
        );
        assert_eq!(parse_alg_list("RS256,none"), Err(JoseError("alg-none")));
        assert_eq!(parse_alg_list("HS256"), Err(JoseError("alg-hmac")));
        assert_eq!(parse_alg_list(" , "), Err(JoseError("alg-list-empty")));
    }
}
