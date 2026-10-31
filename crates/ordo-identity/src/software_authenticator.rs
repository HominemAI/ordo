//! A software authenticator: a real key that signs real assertions.
//!
//! Compiled for this crate's own tests and, behind the `test-support`
//! feature, for the host's. There is no browser and no authenticator in a
//! `cargo test`, so nothing here runs the *ceremony*: no user gesture, no
//! secure enclave, no platform prompt. What it does is stand a real
//! authenticator's **output** in front of the verifier: a genuine P-256 or
//! Ed25519 key signing a genuine `authData ‖ SHA-256(clientDataJSON)` inside
//! genuine CTAP2 CBOR. [`TestAuthenticator`] holds a private key and nothing
//! else. Every byte it emits is one a real authenticator emits, and every
//! negative case is produced by making it misbehave rather than by reaching
//! into the provider.
//!
//! **Never compiled into a deployment.** `ordo-host` enables the feature for
//! its tests only, in `[dev-dependencies]`.

use crate::provider::{AuthenticatorId, ChallengeId, CompleteRequest};
use crate::webauthn::{b64url_encode, rp_id_hash, signed_message};

/// The relying party the software authenticator talks to unless a test says
/// otherwise.
pub const DEFAULT_RP_ID: &str = "example.com";
/// The origin it reports unless a test says otherwise.
pub const DEFAULT_ORIGIN: &str = "https://app.example.com";

pub const FLAG_UP: u8 = 0b0000_0001;
pub const FLAG_UV: u8 = 0b0000_0100;
pub const FLAG_BE: u8 = 0b0000_1000;
pub const FLAG_BS: u8 = 0b0001_0000;
pub const FLAG_AT: u8 = 0b0100_0000;

pub fn cbor_head(major: u8, arg: u64) -> Vec<u8> {
    let mut v = Vec::new();
    if arg < 24 {
        v.push((major << 5) | arg as u8);
    } else if arg <= u64::from(u8::MAX) {
        v.push((major << 5) | 24);
        v.push(arg as u8);
    } else if arg <= u64::from(u16::MAX) {
        v.push((major << 5) | 25);
        v.extend_from_slice(&(arg as u16).to_be_bytes());
    } else {
        v.push((major << 5) | 26);
        v.extend_from_slice(&(arg as u32).to_be_bytes());
    }
    v
}

pub fn cbor_uint(n: u64) -> Vec<u8> {
    cbor_head(0, n)
}

/// CBOR negative integers encode `-1 - n`, which is why COSE's `-1` label is
/// the byte `0x20`.
pub fn cbor_nint(n: i64) -> Vec<u8> {
    assert!(n < 0);
    cbor_head(1, (-1 - n) as u64)
}

pub fn cbor_int(n: i64) -> Vec<u8> {
    if n < 0 {
        cbor_nint(n)
    } else {
        cbor_uint(n as u64)
    }
}

pub fn cbor_bstr(b: &[u8]) -> Vec<u8> {
    let mut v = cbor_head(2, b.len() as u64);
    v.extend_from_slice(b);
    v
}

pub fn cbor_tstr(s: &str) -> Vec<u8> {
    let mut v = cbor_head(3, s.len() as u64);
    v.extend_from_slice(s.as_bytes());
    v
}

pub fn cbor_map(n: u64) -> Vec<u8> {
    cbor_head(5, n)
}

pub enum TestKey {
    Es256(p256::ecdsa::SigningKey),
    Ed25519(ed25519_dalek::SigningKey),
}

impl TestKey {
    /// Deterministic keys: a test that regenerates its key on every run
    /// cannot show you the bytes that failed.
    pub fn es256(seed: u8) -> TestKey {
        TestKey::Es256(
            p256::ecdsa::SigningKey::from_slice(&[seed.max(1); 32]).expect("valid P-256 scalar"),
        )
    }

    pub fn ed25519(seed: u8) -> TestKey {
        TestKey::Ed25519(ed25519_dalek::SigningKey::from_bytes(&[seed.max(1); 32]))
    }

    /// The COSE_Key an authenticator puts in `attestedCredentialData`.
    pub fn cose(&self) -> Vec<u8> {
        match self {
            TestKey::Es256(sk) => {
                use p256::elliptic_curve::sec1::ToEncodedPoint as _;
                let vk = sk.verifying_key();
                let point = vk.as_affine().to_encoded_point(false);
                let mut v = cbor_map(5);
                v.extend(cbor_int(1)); // kty
                v.extend(cbor_int(2)); //   EC2
                v.extend(cbor_int(3)); // alg
                v.extend(cbor_int(-7)); //  ES256
                v.extend(cbor_int(-1)); // crv
                v.extend(cbor_int(1)); //   P-256
                v.extend(cbor_int(-2)); // x
                v.extend(cbor_bstr(point.x().expect("uncompressed point has x")));
                v.extend(cbor_int(-3)); // y
                v.extend(cbor_bstr(point.y().expect("uncompressed point has y")));
                v
            }
            TestKey::Ed25519(sk) => {
                let mut v = cbor_map(4);
                v.extend(cbor_int(1)); // kty
                v.extend(cbor_int(1)); //   OKP
                v.extend(cbor_int(3)); // alg
                v.extend(cbor_int(-8)); //  EdDSA
                v.extend(cbor_int(-1)); // crv
                v.extend(cbor_int(6)); //   Ed25519
                v.extend(cbor_int(-2)); // x
                v.extend(cbor_bstr(sk.verifying_key().as_bytes()));
                v
            }
        }
    }

    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        match self {
            TestKey::Es256(sk) => {
                use p256::ecdsa::signature::Signer as _;
                let sig: p256::ecdsa::Signature = sk.sign(msg);
                sig.to_der().as_bytes().to_vec()
            }
            TestKey::Ed25519(sk) => {
                use ed25519_dalek::Signer as _;
                sk.sign(msg).to_bytes().to_vec()
            }
        }
    }
}

/// A configurable stand-in for a physical authenticator. Every field is here
/// so a test can make it lie about exactly one thing.
pub struct TestAuthenticator {
    pub key: TestKey,
    pub credential_id: Vec<u8>,
    /// The RP the authenticator *thinks* it is talking to.
    pub rp_id: String,
    /// The origin the browser reports in `clientDataJSON`.
    pub origin: String,
    pub user_present: bool,
    pub user_verified: bool,
    /// The BE flag: the credential says it may be synced.
    pub backup_eligible: bool,
    /// The BS flag: the credential says it is synced now.
    pub backed_up: bool,
    pub sign_count: u32,
}

impl TestAuthenticator {
    pub fn es256() -> TestAuthenticator {
        TestAuthenticator {
            key: TestKey::es256(7),
            credential_id: b"credential-one".to_vec(),
            rp_id: DEFAULT_RP_ID.to_string(),
            origin: DEFAULT_ORIGIN.to_string(),
            user_present: true,
            user_verified: true,
            backup_eligible: false,
            backed_up: false,
            sign_count: 0,
        }
    }

    pub fn ed25519() -> TestAuthenticator {
        TestAuthenticator {
            key: TestKey::ed25519(9),
            credential_id: b"credential-ed".to_vec(),
            ..TestAuthenticator::es256()
        }
    }

    pub fn authenticator_id(&self) -> AuthenticatorId {
        AuthenticatorId(format!("passkey:{}", b64url_encode(&self.credential_id)))
    }

    pub fn flags(&self, attested: bool) -> u8 {
        let mut f = 0u8;
        if self.user_present {
            f |= FLAG_UP;
        }
        if self.user_verified {
            f |= FLAG_UV;
        }
        if self.backup_eligible {
            f |= FLAG_BE;
        }
        if self.backed_up {
            f |= FLAG_BS;
        }
        if attested {
            f |= FLAG_AT;
        }
        f
    }

    pub fn auth_data(&self, attested: bool) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&rp_id_hash(&self.rp_id));
        d.push(self.flags(attested));
        d.extend_from_slice(&self.sign_count.to_be_bytes());
        if attested {
            d.extend_from_slice(&[0u8; 16]); // aaguid: all zeros, as passkeys report
            d.extend_from_slice(&(self.credential_id.len() as u16).to_be_bytes());
            d.extend_from_slice(&self.credential_id);
            d.extend(self.key.cose());
        }
        d
    }

    pub fn client_data(&self, ty: &str, challenge: &str, origin: &str, cross: bool) -> Vec<u8> {
        format!(
            r#"{{"type":"{ty}","challenge":"{challenge}","origin":"{origin}","crossOrigin":{cross}}}"#
        )
        .into_bytes()
    }

    pub fn attestation_object(&self, fmt: &str) -> Vec<u8> {
        let auth_data = self.auth_data(true);
        let mut v = cbor_map(3);
        v.extend(cbor_tstr("fmt"));
        v.extend(cbor_tstr(fmt));
        v.extend(cbor_tstr("attStmt"));
        v.extend(cbor_map(0));
        v.extend(cbor_tstr("authData"));
        v.extend(cbor_bstr(&auth_data));
        v
    }

    pub fn register(&self, challenge: &str) -> CompleteRequest {
        CompleteRequest::PasskeyRegistration {
            challenge: ChallengeId(challenge.to_string()),
            client_data_json: self.client_data("webauthn.create", challenge, &self.origin, false),
            attestation_object: self.attestation_object("none"),
        }
    }

    pub fn assert_at(&self, challenge: &str, origin: &str) -> CompleteRequest {
        let client_data_json = self.client_data("webauthn.get", challenge, origin, false);
        let authenticator_data = self.auth_data(false);
        let signature = self
            .key
            .sign(&signed_message(&authenticator_data, &client_data_json));
        CompleteRequest::PasskeyAssertion {
            challenge: ChallengeId(challenge.to_string()),
            credential_id: self.credential_id.clone(),
            client_data_json,
            authenticator_data,
            signature,
        }
    }

    pub fn assert(&self, challenge: &str) -> CompleteRequest {
        self.assert_at(challenge, &self.origin)
    }
}
