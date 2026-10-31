//! The WebAuthn wire formats, and nothing else.
//!
//! This module is the parser and the verifier: CTAP2 canonical CBOR, the
//! COSE public key, `authenticatorData`, `clientDataJSON`, and the two
//! signature algorithms this deployment offers. It holds no state, knows
//! nothing about principals, and decides no authorization — [`crate::passkey`]
//! is the provider that uses it.
//!
//! # Why this is written here rather than pulled in
//!
//! `security-logging-design.md` §2 names `webauthn-rs` in an implementation
//! parenthetical (no marker — the `[settled]` clause in that row is the RP-id
//! rule, not the library). Taking it would add **223** transitive crates and a
//! native OpenSSL link to a workspace whose largest crate has eleven, which
//! buys registration-attestation formats this deployment deliberately does not
//! verify (see `AttestationFormat` below). What is actually load-bearing —
//! challenge binding, RP-id binding, origin binding, the user-presence flag,
//! and one ECDSA/EdDSA verify — is small, closed, and testable, so it is here
//! where a reviewer can read all of it.
//!
//! # What is deliberately absent
//!
//! - **Attestation statement verification.** The only accepted `fmt` is
//!   `none`. Attestation identifies the *authenticator model*; verifying it
//!   is how an RP enforces a hardware allowlist, and it is also a
//!   device-fingerprinting surface. Ordo has no hardware policy, so demanding
//!   a statement it would not read is theatre. Enterprise attestation is a
//!   deployment policy decision with its own metadata-service dependency, and
//!   it belongs to whoever wants it.
//! - **RS256 (`alg: -257`) and every other COSE algorithm.** An RP receives
//!   only what it advertises in `pubKeyCredParams`; this one advertises
//!   ES256 and EdDSA, so anything else arriving is a client that ignored the
//!   options and is refused rather than accommodated.
//! - **Indefinite-length CBOR.** CTAP2 canonical CBOR forbids it. A parser
//!   that accepts it accepts encodings the authenticator did not produce.

use sha2::{Digest, Sha256};

/// Structural and cryptographic refusals from the WebAuthn layer.
///
/// These are loud and specific on purpose. Unlike a password identifier,
/// nothing here is an enumeration oracle: the challenge is server-minted and
/// the credential id is a random value the caller had to already hold, so a
/// caller learns nothing about *which accounts exist* from any of them. What
/// they do tell an operator is whether they are looking at a client bug, a
/// misconfigured origin, or an attack — three responses, three names.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebAuthnError {
    /// `clientDataJSON` was not a JSON object, or a field was absent or of
    /// the wrong shape.
    MalformedClientData,
    /// `clientData.type` was not the one this ceremony expects. A
    /// registration response replayed into the assertion endpoint lands here
    /// — the type field exists precisely so the two cannot be crossed.
    ClientDataTypeMismatch { expected: &'static str },
    /// `clientData.challenge` is not the challenge this ceremony minted.
    ChallengeMismatch,
    /// `clientData.origin` is not one of the deployment's registered origins.
    /// A phishing page on another host produces a valid signature over its
    /// *own* origin; this is the check that makes that signature useless.
    OriginNotAllowed,
    /// The client says this was a cross-origin (iframed) ceremony. Refused:
    /// an embedded RP cannot know what its embedder is showing the human.
    CrossOrigin,
    /// `authenticatorData` was truncated or carried trailing bytes.
    MalformedAuthenticatorData,
    /// The RP-id hash in `authenticatorData` is not this deployment's. The
    /// assertion is genuine and belongs to somebody else.
    RpIdMismatch,
    /// The user-presence (UP) flag is clear. **This is the step-up gate's
    /// entire physical basis** — without it nothing was touched and no human
    /// was there, so a presence assertion cannot be minted from it.
    UserPresenceMissing,
    /// User verification (UV) was required and the flag is clear — a PIN or
    /// biometric was demanded and not performed.
    UserVerificationMissing,
    /// The response carried no attested credential data (`AT` clear) on a
    /// registration, so there is no public key to enroll.
    NoAttestedCredential,
    /// Attestation format other than `none` — see the module doc.
    UnsupportedAttestation { fmt: String },
    /// A COSE algorithm this deployment does not advertise.
    UnsupportedAlgorithm { alg: i64 },
    /// The COSE key was structurally wrong for its stated algorithm — the
    /// wrong curve, or a coordinate of the wrong length.
    MalformedCoseKey,
    /// CBOR that is not CTAP2-canonical: indefinite lengths, trailing bytes,
    /// truncation, or a nesting depth no real message has.
    MalformedCbor,
    /// The signature did not verify over `authData ‖ SHA-256(clientDataJSON)`.
    BadSignature,
    /// The authenticator's signature counter did not advance. A counter that
    /// goes backwards is the one signal WebAuthn gives that a credential has
    /// been **cloned**, and it is a distinct name because it demands a
    /// distinct response: this is not a retry to shrug at.
    SignCountRegression { stored: u32, presented: u32 },
    /// Base64url that does not decode.
    MalformedBase64,
}

impl std::fmt::Display for WebAuthnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebAuthnError::MalformedClientData => f.write_str("malformed clientDataJSON"),
            WebAuthnError::ClientDataTypeMismatch { expected } => {
                write!(f, "clientData.type is not `{expected}`")
            }
            WebAuthnError::ChallengeMismatch => {
                f.write_str("clientData.challenge is not this ceremony's challenge")
            }
            WebAuthnError::OriginNotAllowed => {
                f.write_str("clientData.origin is not a registered origin of this deployment")
            }
            WebAuthnError::CrossOrigin => f.write_str("cross-origin ceremonies are refused"),
            WebAuthnError::MalformedAuthenticatorData => f.write_str("malformed authenticatorData"),
            WebAuthnError::RpIdMismatch => {
                f.write_str("the assertion was made for a different relying party")
            }
            WebAuthnError::UserPresenceMissing => {
                f.write_str("the user-presence flag is clear — nobody was there")
            }
            WebAuthnError::UserVerificationMissing => {
                f.write_str("user verification was required and not performed")
            }
            WebAuthnError::NoAttestedCredential => {
                f.write_str("registration response carried no attested credential data")
            }
            WebAuthnError::UnsupportedAttestation { fmt } => {
                write!(
                    f,
                    "attestation format `{fmt}` is not accepted (only `none`)"
                )
            }
            WebAuthnError::UnsupportedAlgorithm { alg } => {
                write!(f, "COSE algorithm {alg} is not offered by this deployment")
            }
            WebAuthnError::MalformedCoseKey => f.write_str("malformed COSE public key"),
            WebAuthnError::MalformedCbor => f.write_str("malformed or non-canonical CBOR"),
            WebAuthnError::BadSignature => f.write_str("signature did not verify"),
            WebAuthnError::SignCountRegression { stored, presented } => write!(
                f,
                "signature counter went backwards ({presented} after {stored}) — \
                 this credential may have been cloned"
            ),
            WebAuthnError::MalformedBase64 => f.write_str("malformed base64url"),
        }
    }
}

impl std::error::Error for WebAuthnError {}

// ─────────────────────────────── base64url ───────────────────────────────

/// WebAuthn's base64url is unpadded, but padded input is accepted rather than
/// refused: the padding carries no information and a client that adds it is
/// wrong about the encoding, not about the bytes.
pub fn b64url_decode(s: &str) -> Result<Vec<u8>, WebAuthnError> {
    use base64::Engine as _;
    let trimmed = s.trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .map_err(|_| WebAuthnError::MalformedBase64)
}

pub fn b64url_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// ──────────────────────────────── CBOR ───────────────────────────────────

/// Definite-length CBOR only, over a borrowed slice. Every read is bounds
/// checked and every length is compared against what remains *before* it is
/// used, so a declared length larger than the message cannot allocate.
struct Cbor<'a> {
    b: &'a [u8],
    i: usize,
}

/// Nesting deeper than this is not a message any authenticator produces; the
/// cap exists so a hostile `attestationObject` cannot recurse the skipper
/// into the stack guard.
const MAX_DEPTH: usize = 8;

const MAJOR_UINT: u8 = 0;
const MAJOR_NINT: u8 = 1;
const MAJOR_BYTES: u8 = 2;
const MAJOR_TEXT: u8 = 3;
const MAJOR_ARRAY: u8 = 4;
const MAJOR_MAP: u8 = 5;
const MAJOR_SIMPLE: u8 = 7;

impl<'a> Cbor<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cbor { b, i: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], WebAuthnError> {
        let end = self.i.checked_add(n).ok_or(WebAuthnError::MalformedCbor)?;
        let out = self
            .b
            .get(self.i..end)
            .ok_or(WebAuthnError::MalformedCbor)?;
        self.i = end;
        Ok(out)
    }

    /// `(major type, argument)`. Indefinite length (additional info 31) is a
    /// refusal, not a mode.
    fn head(&mut self) -> Result<(u8, u64), WebAuthnError> {
        let b = *self.take(1)?.first().ok_or(WebAuthnError::MalformedCbor)?;
        let major = b >> 5;
        let info = b & 0x1f;
        let arg = match info {
            0..=23 => u64::from(info),
            24 => u64::from(u8::from_be_bytes(
                self.take(1)?
                    .try_into()
                    .map_err(|_| WebAuthnError::MalformedCbor)?,
            )),
            25 => u64::from(u16::from_be_bytes(
                self.take(2)?
                    .try_into()
                    .map_err(|_| WebAuthnError::MalformedCbor)?,
            )),
            26 => u64::from(u32::from_be_bytes(
                self.take(4)?
                    .try_into()
                    .map_err(|_| WebAuthnError::MalformedCbor)?,
            )),
            27 => u64::from_be_bytes(
                self.take(8)?
                    .try_into()
                    .map_err(|_| WebAuthnError::MalformedCbor)?,
            ),
            _ => return Err(WebAuthnError::MalformedCbor),
        };
        Ok((major, arg))
    }

    fn len_arg(arg: u64) -> Result<usize, WebAuthnError> {
        usize::try_from(arg).map_err(|_| WebAuthnError::MalformedCbor)
    }

    fn bytes(&mut self) -> Result<&'a [u8], WebAuthnError> {
        let (major, arg) = self.head()?;
        if major != MAJOR_BYTES {
            return Err(WebAuthnError::MalformedCbor);
        }
        self.take(Self::len_arg(arg)?)
    }

    fn text(&mut self) -> Result<&'a str, WebAuthnError> {
        let (major, arg) = self.head()?;
        if major != MAJOR_TEXT {
            return Err(WebAuthnError::MalformedCbor);
        }
        std::str::from_utf8(self.take(Self::len_arg(arg)?)?)
            .map_err(|_| WebAuthnError::MalformedCbor)
    }

    fn map_len(&mut self) -> Result<usize, WebAuthnError> {
        let (major, arg) = self.head()?;
        if major != MAJOR_MAP {
            return Err(WebAuthnError::MalformedCbor);
        }
        Self::len_arg(arg)
    }

    /// A COSE label: a small signed integer. Both `0..` and negative labels
    /// occur (`-1` is `crv`, `-2` is `x`).
    fn int(&mut self) -> Result<i64, WebAuthnError> {
        let (major, arg) = self.head()?;
        match major {
            MAJOR_UINT => i64::try_from(arg).map_err(|_| WebAuthnError::MalformedCbor),
            MAJOR_NINT => i64::try_from(arg)
                .map(|n| -1 - n)
                .map_err(|_| WebAuthnError::MalformedCbor),
            _ => Err(WebAuthnError::MalformedCbor),
        }
    }

    fn skip(&mut self, depth: usize) -> Result<(), WebAuthnError> {
        if depth > MAX_DEPTH {
            return Err(WebAuthnError::MalformedCbor);
        }
        let (major, arg) = self.head()?;
        match major {
            MAJOR_UINT | MAJOR_NINT | MAJOR_SIMPLE => Ok(()),
            MAJOR_BYTES | MAJOR_TEXT => self.take(Self::len_arg(arg)?).map(|_| ()),
            MAJOR_ARRAY => {
                for _ in 0..Self::len_arg(arg)? {
                    self.skip(depth + 1)?;
                }
                Ok(())
            }
            MAJOR_MAP => {
                for _ in 0..Self::len_arg(arg)? {
                    self.skip(depth + 1)?;
                    self.skip(depth + 1)?;
                }
                Ok(())
            }
            // Tags (6) have no place in a CTAP2 message.
            _ => Err(WebAuthnError::MalformedCbor),
        }
    }
}

// ────────────────────────────── COSE keys ────────────────────────────────

/// The two algorithms this deployment advertises in `pubKeyCredParams`, by
/// their COSE identifiers. An authenticator picks from what it is offered, so
/// this list is also the exhaustive set of what can ever arrive.
pub const ALG_ES256: i64 = -7;
pub const ALG_EDDSA: i64 = -8;

/// A credential's public key, already validated into the exact shape its
/// algorithm needs. Stored per credential; nothing else survives the
/// registration ceremony.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoseKey {
    /// ECDSA over P-256 with SHA-256. Uncompressed SEC1 point, 65 bytes.
    Es256 { sec1: Vec<u8> },
    /// Ed25519. Raw 32-byte public key.
    Eddsa { key: [u8; 32] },
}

impl CoseKey {
    pub fn algorithm(&self) -> i64 {
        match self {
            CoseKey::Es256 { .. } => ALG_ES256,
            CoseKey::Eddsa { .. } => ALG_EDDSA,
        }
    }

    /// The key as stored: the 65-byte SEC1 point for ES256, the 32-byte
    /// public key for EdDSA. The algorithm is stored beside it, so the bytes
    /// carry no tag of their own.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            CoseKey::Es256 { sec1 } => sec1.clone(),
            CoseKey::Eddsa { key } => key.to_vec(),
        }
    }

    /// The inverse of [`CoseKey::to_bytes`], **re-running the checks the
    /// registration ran**: an ES256 point must be 65 bytes, uncompressed and
    /// on the curve, and an EdDSA key must be 32 bytes and decode. A row
    /// edited by hand, or damaged, refuses to load rather than loading a key
    /// that verifies nothing or, worse, something else.
    pub fn from_bytes(alg: i64, bytes: &[u8]) -> Result<CoseKey, WebAuthnError> {
        match alg {
            ALG_ES256 => {
                if bytes.len() != 65 || bytes[0] != 0x04 {
                    return Err(WebAuthnError::MalformedCoseKey);
                }
                p256::ecdsa::VerifyingKey::from_sec1_bytes(bytes)
                    .map_err(|_| WebAuthnError::MalformedCoseKey)?;
                Ok(CoseKey::Es256 {
                    sec1: bytes.to_vec(),
                })
            }
            ALG_EDDSA => {
                let key: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| WebAuthnError::MalformedCoseKey)?;
                ed25519_dalek::VerifyingKey::from_bytes(&key)
                    .map_err(|_| WebAuthnError::MalformedCoseKey)?;
                Ok(CoseKey::Eddsa { key })
            }
            other => Err(WebAuthnError::UnsupportedAlgorithm { alg: other }),
        }
    }

    /// Verify `signature` over `message`. The caller has already built
    /// `message` as `authData ‖ SHA-256(clientDataJSON)`; this only does the
    /// mathematics, and every failure is one [`WebAuthnError::BadSignature`]
    /// because a verifier that distinguishes "wrong encoding" from "wrong
    /// key" hands out a decision procedure.
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), WebAuthnError> {
        match self {
            CoseKey::Es256 { sec1 } => {
                use p256::ecdsa::signature::Verifier as _;
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(sec1)
                    .map_err(|_| WebAuthnError::BadSignature)?;
                // WebAuthn ES256 signatures are ASN.1 DER, not fixed-width.
                let sig = p256::ecdsa::DerSignature::try_from(signature)
                    .map_err(|_| WebAuthnError::BadSignature)?;
                key.verify(message, &sig)
                    .map_err(|_| WebAuthnError::BadSignature)
            }
            CoseKey::Eddsa { key } => {
                use ed25519_dalek::Verifier as _;
                let vk = ed25519_dalek::VerifyingKey::from_bytes(key)
                    .map_err(|_| WebAuthnError::BadSignature)?;
                let sig: [u8; 64] = signature
                    .try_into()
                    .map_err(|_| WebAuthnError::BadSignature)?;
                vk.verify(message, &ed25519_dalek::Signature::from_bytes(&sig))
                    .map_err(|_| WebAuthnError::BadSignature)
            }
        }
    }
}

// COSE_Key labels (RFC 8152 §7.1 and §13).
const COSE_KTY: i64 = 1;
const COSE_ALG: i64 = 3;
const COSE_CRV: i64 = -1;
const COSE_X: i64 = -2;
const COSE_Y: i64 = -3;
const KTY_OKP: i64 = 1;
const KTY_EC2: i64 = 2;
const CRV_P256: i64 = 1;
const CRV_ED25519: i64 = 6;

/// One COSE map value, read without yet knowing what it means.
///
/// The two-pass shape matters: label `-1` is `crv` for an EC2 or OKP key and
/// the RSA *modulus* for an RSA one, so a parser that decides what `-1` is
/// before it has read `alg` answers "malformed CBOR" to a message that is
/// merely an algorithm we do not offer. Reading generically first is what lets
/// [`WebAuthnError::UnsupportedAlgorithm`] name the real problem.
enum CoseValue<'a> {
    Int(i64),
    Bytes(&'a [u8]),
    Other,
}

fn parse_cose_key<'a>(c: &mut Cbor<'a>) -> Result<CoseKey, WebAuthnError> {
    let n = c.map_len()?;
    let mut entries: Vec<(i64, CoseValue<'a>)> = Vec::new();
    for _ in 0..n {
        let label = c.int()?;
        // Peek the value's major type without consuming it, so the generic
        // read stays one pass over the bytes.
        let value = match c.b.get(c.i).map(|b| b >> 5) {
            Some(MAJOR_UINT) | Some(MAJOR_NINT) => CoseValue::Int(c.int()?),
            Some(MAJOR_BYTES) => CoseValue::Bytes(c.bytes()?),
            Some(_) => {
                c.skip(0)?;
                CoseValue::Other
            }
            None => return Err(WebAuthnError::MalformedCbor),
        };
        entries.push((label, value));
    }
    let get_int = |label: i64| -> Option<i64> {
        entries.iter().find_map(|(l, v)| match (l, v) {
            (l, CoseValue::Int(i)) if *l == label => Some(*i),
            _ => None,
        })
    };
    let get_bytes = |label: i64| -> Option<&'a [u8]> {
        entries.iter().find_map(|(l, v)| match (l, v) {
            (l, CoseValue::Bytes(b)) if *l == label => Some(*b),
            _ => None,
        })
    };
    let (kty, crv) = (get_int(COSE_KTY), get_int(COSE_CRV));
    let (x, y) = (get_bytes(COSE_X), get_bytes(COSE_Y));
    let alg = get_int(COSE_ALG).ok_or(WebAuthnError::MalformedCoseKey)?;
    match alg {
        ALG_ES256 => {
            if kty != Some(KTY_EC2) || crv != Some(CRV_P256) {
                return Err(WebAuthnError::MalformedCoseKey);
            }
            let (x, y) = (
                x.ok_or(WebAuthnError::MalformedCoseKey)?,
                y.ok_or(WebAuthnError::MalformedCoseKey)?,
            );
            // Fixed 32-byte coordinates: a short `x` left-padded by the
            // authenticator would otherwise become a different point.
            if x.len() != 32 || y.len() != 32 {
                return Err(WebAuthnError::MalformedCoseKey);
            }
            let mut sec1 = Vec::with_capacity(65);
            sec1.push(0x04);
            sec1.extend_from_slice(x);
            sec1.extend_from_slice(y);
            // Reject a point that is not on the curve here rather than at
            // first assertion, so a bad enrollment fails where it happened.
            p256::ecdsa::VerifyingKey::from_sec1_bytes(&sec1)
                .map_err(|_| WebAuthnError::MalformedCoseKey)?;
            Ok(CoseKey::Es256 { sec1 })
        }
        ALG_EDDSA => {
            if kty != Some(KTY_OKP) || crv != Some(CRV_ED25519) {
                return Err(WebAuthnError::MalformedCoseKey);
            }
            let x = x.ok_or(WebAuthnError::MalformedCoseKey)?;
            let key: [u8; 32] = x.try_into().map_err(|_| WebAuthnError::MalformedCoseKey)?;
            ed25519_dalek::VerifyingKey::from_bytes(&key)
                .map_err(|_| WebAuthnError::MalformedCoseKey)?;
            Ok(CoseKey::Eddsa { key })
        }
        other => Err(WebAuthnError::UnsupportedAlgorithm { alg: other }),
    }
}

// ─────────────────────── authenticator data ──────────────────────────────

const FLAG_UP: u8 = 0b0000_0001;
const FLAG_UV: u8 = 0b0000_0100;
/// Backup eligible: the credential may be synced (bit 3).
pub const FLAG_BE: u8 = 0b0000_1000;
/// Backup state: the credential is synced now (bit 4).
pub const FLAG_BS: u8 = 0b0001_0000;
const FLAG_AT: u8 = 0b0100_0000;
const FLAG_ED: u8 = 0b1000_0000;

/// The parsed `authenticatorData` header. The credential half is present only
/// on registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatorData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub sign_count: u32,
    pub credential: Option<AttestedCredential>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestedCredential {
    pub id: Vec<u8>,
    pub key: CoseKey,
}

impl AuthenticatorData {
    pub fn user_present(&self) -> bool {
        self.flags & FLAG_UP != 0
    }

    pub fn user_verified(&self) -> bool {
        self.flags & FLAG_UV != 0
    }

    /// The BE flag: the authenticator says this credential can be backed up
    /// or synced to other devices. Fixed for the life of the credential.
    pub fn backup_eligible(&self) -> bool {
        self.flags & FLAG_BE != 0
    }

    /// The BS flag: the credential is backed up right now. It can change
    /// between assertions, which is why a registration's value is only the
    /// value at registration.
    pub fn backed_up(&self) -> bool {
        self.flags & FLAG_BS != 0
    }

    /// Parse the fixed header, then the optional attested-credential and
    /// extension blocks. Trailing bytes are a refusal: `authData` is signed
    /// as a whole and anything left over was not accounted for by the
    /// structure we just agreed on.
    pub fn parse(bytes: &[u8]) -> Result<AuthenticatorData, WebAuthnError> {
        if bytes.len() < 37 {
            return Err(WebAuthnError::MalformedAuthenticatorData);
        }
        let mut rp_id_hash = [0u8; 32];
        rp_id_hash.copy_from_slice(&bytes[..32]);
        let flags = bytes[32];
        let sign_count = u32::from_be_bytes(
            bytes[33..37]
                .try_into()
                .map_err(|_| WebAuthnError::MalformedAuthenticatorData)?,
        );
        let rest = &bytes[37..];
        let (credential, rest) = if flags & FLAG_AT != 0 {
            // aaguid(16) ‖ credIdLen(2) ‖ credId ‖ COSE key
            if rest.len() < 18 {
                return Err(WebAuthnError::MalformedAuthenticatorData);
            }
            let id_len = usize::from(u16::from_be_bytes(
                rest[16..18]
                    .try_into()
                    .map_err(|_| WebAuthnError::MalformedAuthenticatorData)?,
            ));
            // The spec caps credential ids at 1023 bytes; anything longer is
            // a length field being used as an allocator.
            if id_len > 1023 {
                return Err(WebAuthnError::MalformedAuthenticatorData);
            }
            let key_start = 18usize
                .checked_add(id_len)
                .ok_or(WebAuthnError::MalformedAuthenticatorData)?;
            if rest.len() < key_start {
                return Err(WebAuthnError::MalformedAuthenticatorData);
            }
            let id = rest[18..key_start].to_vec();
            let mut c = Cbor::new(&rest[key_start..]);
            let key = parse_cose_key(&mut c)?;
            let consumed = key_start + c.i;
            (Some(AttestedCredential { id, key }), &rest[consumed..])
        } else {
            (None, rest)
        };
        let rest = if flags & FLAG_ED != 0 {
            // Extensions are a CBOR map we do not read; it must still parse,
            // because an unparseable tail means we mis-split signed bytes.
            let mut c = Cbor::new(rest);
            c.skip(0)?;
            &rest[c.i..]
        } else {
            rest
        };
        if !rest.is_empty() {
            return Err(WebAuthnError::MalformedAuthenticatorData);
        }
        Ok(AuthenticatorData {
            rp_id_hash,
            flags,
            sign_count,
            credential,
        })
    }
}

/// The `attestationObject` of a registration response, after the format check.
pub struct AttestationObject {
    pub auth_data: AuthenticatorData,
}

impl AttestationObject {
    /// Parse and refuse anything but `fmt: "none"` (see the module doc).
    pub fn parse(bytes: &[u8]) -> Result<AttestationObject, WebAuthnError> {
        let mut c = Cbor::new(bytes);
        let n = c.map_len()?;
        let (mut fmt, mut auth_data) = (None, None);
        for _ in 0..n {
            match c.text()? {
                "fmt" => fmt = Some(c.text()?.to_string()),
                "authData" => auth_data = Some(c.bytes()?),
                _ => c.skip(0)?,
            }
        }
        if c.i != bytes.len() {
            return Err(WebAuthnError::MalformedCbor);
        }
        let fmt = fmt.ok_or(WebAuthnError::MalformedCbor)?;
        if fmt != "none" {
            return Err(WebAuthnError::UnsupportedAttestation { fmt });
        }
        let auth_data = AuthenticatorData::parse(auth_data.ok_or(WebAuthnError::MalformedCbor)?)?;
        Ok(AttestationObject { auth_data })
    }
}

// ──────────────────────────── client data ────────────────────────────────

/// The three fields of `CollectedClientData` an RP is obliged to check, plus
/// the cross-origin flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientData {
    pub ty: String,
    pub challenge: Vec<u8>,
    pub origin: String,
    pub cross_origin: bool,
}

pub const CLIENT_DATA_CREATE: &str = "webauthn.create";
pub const CLIENT_DATA_GET: &str = "webauthn.get";

impl ClientData {
    /// Parse the JSON the client collected. The bytes are kept by the caller
    /// — the signature covers the **bytes**, not this parse, so re-serializing
    /// would be a different message.
    pub fn parse(bytes: &[u8]) -> Result<ClientData, WebAuthnError> {
        let v: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| WebAuthnError::MalformedClientData)?;
        let obj = v.as_object().ok_or(WebAuthnError::MalformedClientData)?;
        let ty = obj
            .get("type")
            .and_then(|x| x.as_str())
            .ok_or(WebAuthnError::MalformedClientData)?
            .to_string();
        let challenge = obj
            .get("challenge")
            .and_then(|x| x.as_str())
            .ok_or(WebAuthnError::MalformedClientData)?;
        let origin = obj
            .get("origin")
            .and_then(|x| x.as_str())
            .ok_or(WebAuthnError::MalformedClientData)?
            .to_string();
        // Absent means false, per the spec's default.
        let cross_origin = match obj.get("crossOrigin") {
            None | Some(serde_json::Value::Null) => false,
            Some(x) => x.as_bool().ok_or(WebAuthnError::MalformedClientData)?,
        };
        Ok(ClientData {
            ty,
            challenge: b64url_decode(challenge)?,
            origin,
            cross_origin,
        })
    }

    /// The four checks every ceremony owes, in the order that names the most
    /// actionable failure first.
    pub fn check(
        &self,
        expected_type: &'static str,
        expected_challenge: &[u8],
        origin_allowed: &dyn Fn(&str) -> bool,
    ) -> Result<(), WebAuthnError> {
        if self.ty != expected_type {
            return Err(WebAuthnError::ClientDataTypeMismatch {
                expected: expected_type,
            });
        }
        // Constant-time: the challenge is the replay defence and comparing it
        // byte-by-byte with an early return leaks a correct prefix.
        if !ct_eq(&self.challenge, expected_challenge) {
            return Err(WebAuthnError::ChallengeMismatch);
        }
        if !origin_allowed(&self.origin) {
            return Err(WebAuthnError::OriginNotAllowed);
        }
        if self.cross_origin {
            return Err(WebAuthnError::CrossOrigin);
        }
        Ok(())
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `SHA-256(rp_id)` — what an authenticator puts in `authenticatorData` and
/// what the RP compares against. The RP id is an ASCII domain, so this is a
/// hash of its bytes with no normalization of our own invention.
pub fn rp_id_hash(rp_id: &str) -> [u8; 32] {
    Sha256::digest(rp_id.as_bytes()).into()
}

/// The message an authenticator signs: the raw `authenticatorData` followed
/// by the SHA-256 of the raw `clientDataJSON`.
pub fn signed_message(auth_data: &[u8], client_data_json: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(auth_data.len() + 32);
    msg.extend_from_slice(auth_data);
    msg.extend_from_slice(&Sha256::digest(client_data_json));
    msg
}
