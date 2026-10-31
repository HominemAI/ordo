//! The layer beneath [`crate::access::AccessEngine`]: bearer strings in,
//! verified [`TokenClaims`] out.
//!
//! The engine's doc says transport validity — signature, expiry — is "the
//! token layer's job". This is that job, and nothing more: it decides
//! whether a string was minted by this deployment and has not expired. It
//! decides no authorization whatsoever; a perfectly valid token for a
//! suspended member still opens nothing, because the engine is what
//! answers that.
//!
//! Format: `base64url(payload) . base64url(MAC)` where the MAC is
//! `HMAC-SHA256(key, domain ‖ len-prefixed payload)`. The payload is a
//! compact, ordered, `|`-separated record rather than JSON — a token is
//! read on every request, and a hand-rolled field split cannot be tricked
//! by a duplicate key the way a lenient JSON parser can.
//!
//! # Why every failure is one error
//!
//! [`TokenError`] has a single opaque variant on purpose. A verifier that
//! distinguishes "bad signature" from "expired" from "malformed" hands an
//! attacker a decision procedure: forge, observe which complaint changes,
//! and learn where the boundary is. Operators get detail from the audit
//! path, which sees the claims; callers get "no".
//!
//! # What is deliberately absent
//!
//! No asymmetric signing (one deployment verifies its own tokens; a
//! multi-issuer federation is a different design, not a bigger enum), and
//! no revocation list *in here*: a token is a statement, and whether the
//! session it names has been ended is a fact about the deployment rather
//! than about the string. [`crate::session::SessionRevocations`] holds that
//! fact and the door asks it; this module's job ends at "was this minted
//! here and is it still inside its expiry".
//!
//! What did change is that a token can now **name** its session
//! ([`crate::session::SessionClaim`]), which is what gives logout and
//! refresh something to refer to. Before that field, two logins by one
//! person produced two indistinguishable tokens and "end this one" had no
//! referent.

use crate::access::TokenClaims;
use crate::delegation::DelegationClaim;
use crate::principal::{
    CapabilityGrant, ClientId, DelegationId, GrantId, PrincipalId, SpaceId, Timestamp,
};
use crate::second_step::SecondStepKind;
use crate::session::{SessionClaim, SessionId};

const DOMAIN: &[u8] = b"ordo.token.v1";

/// The deployment's token key. Not `Debug` and not `Clone`: it exists to
/// be handed to [`mint`] and [`verify`], and a key that prints is a key in
/// a log.
pub struct TokenKey([u8; 32]);

impl TokenKey {
    /// Derive from deployment secret material — hashed, so short secrets
    /// still fill the key rather than padding with zeros.
    pub fn from_secret(secret: &[u8]) -> TokenKey {
        TokenKey(hmac(b"ordo.token.key.v1", &[secret]))
    }

    /// Read the deployment's token key from `ORDO_TOKEN_KEY` — 64 hex
    /// characters.
    ///
    /// **Unset is a refusal, never a default**, on the exact posture
    /// `ordo_commandlog::sign::Ed25519Signer::from_env` and
    /// `ordo_crypto::LocalKeyProvider::from_env` already take. A defaulted
    /// token key means every deployment built from this source can mint
    /// tokens every other one accepts, which is not a weak deployment: it
    /// is one authentication boundary shared by strangers.
    ///
    /// Hex, and exactly 64 characters, for the same reason the checkpoint
    /// key is: a passphrase-shaped secret invites a passphrase, and there
    /// is no KDF between this and the MAC. The error says how to make one.
    pub fn from_env() -> Result<TokenKey, TokenKeyError> {
        Ok(TokenKey::from_secret(&TokenKey::secret_from_env()?))
    }

    /// The raw 32 bytes behind [`TokenKey::from_env`].
    ///
    /// Exists because a server that rebuilds its door on a program swap
    /// needs a second `TokenKey` and `TokenKey` is deliberately not
    /// `Clone`. Re-reading the environment there would be a second
    /// configuration read, which `plan-runtime.md` §1.7 rules out: the
    /// environment is read once, into a validated struct.
    ///
    /// `ORDO_TOKEN_KEY_FILE` names a file holding the same 64 hex
    /// characters, the form the console's supervisor passes
    /// (`ordo_env::secret`).
    pub fn secret_from_env() -> Result<[u8; 32], TokenKeyError> {
        let hex = ordo_env::secret("ORDO_TOKEN_KEY")
            .map_err(|e| TokenKeyError::File(e.to_string()))?
            .ok_or(TokenKeyError::Missing)?;
        unhex32(hex.trim()).ok_or(TokenKeyError::Malformed)
    }
}

/// Why `ORDO_TOKEN_KEY` did not produce a key. Two cases, because the
/// remedies differ: one is "set it", the other is "you set something that
/// is not a key".
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenKeyError {
    Missing,
    Malformed,
    /// `ORDO_TOKEN_KEY_FILE` could not be read, or both forms are set.
    File(String),
}

impl std::fmt::Display for TokenKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenKeyError::Missing => f.write_str(
                "ORDO_TOKEN_KEY is not set (nor ORDO_TOKEN_KEY_FILE) — refusing to mint tokens \
                 under a default key",
            ),
            TokenKeyError::Malformed => {
                f.write_str("ORDO_TOKEN_KEY must be 64 hex characters (openssl rand -hex 32)")
            }
            TokenKeyError::File(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for TokenKeyError {}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Verification failed. One variant, deliberately — see the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenError;

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid token")
    }
}

/// What a token asserts, before verification. Mint takes this; verify
/// returns the [`TokenClaims`] the engine consumes plus the expiry that
/// was enforced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSpec {
    pub principal: PrincipalId,
    pub expires: Timestamp,
    pub client: Option<ClientId>,
    pub user_presence_at: Option<Timestamp>,
    /// Present for synthetic callers only. A capability grant IS the whole
    /// authority of such a token — the engine never falls through to
    /// membership for one, which is why the two cannot be mixed here
    /// either.
    pub capability: Option<CapabilityGrant>,
    /// Present when this token is minted **against a delegation** — the
    /// consent that lets its principal act as somebody else (DL §8).
    ///
    /// Orthogonal to `capability`, because DL calls the two distinct: a
    /// token may carry an allowlist, a delegation, both, or neither. The
    /// claim carries the delegation's own expiry so a lapsed one is refused
    /// from the token alone; **revocation** is the door's registry lookup
    /// and is deliberately not expressible here.
    pub delegation: Option<DelegationClaim>,
    /// Which sign-in this token belongs to, and when that sign-in happened
    /// (`crate::session::SessionClaim`).
    ///
    /// `Some` for every token a login ceremony produces. `None` for a
    /// capability token and for a token minted against a delegation:
    /// neither is a session, neither is logged out, and both are ended by
    /// withdrawing the grant or the consent they were minted against.
    ///
    /// `started_at` rides here rather than in a table because it is the
    /// ceiling `crate::session::refresh` enforces, and a claim under the
    /// MAC is a claim the holder cannot move.
    pub session: Option<SessionClaim>,
}

/// Mint a bearer token.
pub fn mint(key: &TokenKey, spec: &TokenSpec) -> String {
    let payload = encode(spec);
    let mac = hmac(
        &key.0,
        &[
            DOMAIN,
            &(payload.len() as u64).to_be_bytes(),
            payload.as_bytes(),
        ],
    );
    format!("{}.{}", b64(payload.as_bytes()), b64(&mac))
}

/// Verify a bearer token at `now`. The MAC is checked **before** the
/// payload is parsed — an unauthenticated payload is attacker input, and
/// parsing it first would run our parser on their bytes for free.
pub fn verify(key: &TokenKey, token: &str, now: Timestamp) -> Result<TokenClaims, TokenError> {
    let (p_b64, m_b64) = token.split_once('.').ok_or(TokenError)?;
    let payload = unb64(p_b64)?;
    let mac = unb64(m_b64)?;
    let expect = hmac(
        &key.0,
        &[DOMAIN, &(payload.len() as u64).to_be_bytes(), &payload],
    );
    if !ct_eq(&mac, &expect) {
        return Err(TokenError);
    }
    let text = String::from_utf8(payload).map_err(|_| TokenError)?;
    let spec = decode(&text)?;
    // The expiry instant itself denies, matching the grant rule in
    // `AccessEngine` — one boundary convention across the platform.
    if now.0 >= spec.expires.0 {
        return Err(TokenError);
    }
    Ok(TokenClaims {
        principal: spec.principal,
        capability: spec.capability,
        client: spec.client,
        user_presence_at: spec.user_presence_at,
        delegation: spec.delegation,
        session: spec.session,
    })
}

// ── payload encoding ─────────────────────────────────────────────────────
//
// `v6|<principal>|<exp>|<client?>|<presence?>|<grant?>|<delegation?>|<session?>`
// where a grant is `<id>~<space>~<exp>~<b64url of the per-op pins>`, a
// delegation is `<id>~<space>~<delegator>~<exp>`, and a session is
// `<id>~<started_at>~<second step?>~<via?>~<upstream at?>~<upstream via?>`. Fields are
// positional and the count is fixed, so a
// token with an extra or missing field is malformed rather than partially
// understood.
//
// The version word moved from `v1` to `v2` when the delegation field
// arrived, from `v2` to `v3` when the session field did, and from `v3` to `v4`
// when the session gained the second factor that proved it, from `v4`
// to `v5` when it gained the way in (`via`), and from `v5` to `v6` when it
// gained a provider's two-step claim (`upstream_presence`), and an older
// token is refused outright rather than read as a narrower one with the new
// field missing. That is the safe direction and it costs nothing durable: a
// token is a short-lived bearer credential, not stored state, so a format
// change is a re-login rather than a migration. Accepting both widths would
// mean a forged truncation could present itself as "no delegation claimed"
// or, now, as "no session, so nothing to look up in the deny list", which
// is precisely the claim a logged-out holder would like to make. This
// file's whole discipline is that a malformed token is refused rather than
// partially understood.
//
// The session id is written plainly for the reason the delegator is: it is
// hex from `SessionId::mint`, so it cannot contain `~` or `|`.
//
// The pins are base64url'd rather than written inline because a pinned
// row-scope shape carries **guard text** — `project.projectMembers(member ==
// $caller)` — which contains spaces, dots, parentheses and `$`, and there is
// no ASCII delimiter that is safe by inspection. `ordo_identity::pins`
// length-prefixes every element inside that blob for the same reason, so the
// two layers are unambiguous independently: the outer split cannot be
// confused by the inner content, and the inner read cannot be confused by
// anything.

fn encode(s: &TokenSpec) -> String {
    let grant = match &s.capability {
        None => String::new(),
        // The grant id leads, because a token that cannot name which grant
        // it carries cannot be revoked — the registry would have nothing to
        // match it against but `(principal, client, ops)`, which collides on
        // the ordinary case of consenting twice to the same scope.
        Some(g) => format!(
            "{}~{}~{}~{}",
            g.id().0,
            g.space().0,
            g.expires().0,
            b64(crate::pins::encode_pins(g.pins()).as_bytes()),
        ),
    };
    // The delegator is written plainly rather than base64'd: a principal id
    // is a typed id (`usr_…`), which cannot contain `~` or `|`, so nothing
    // here can be confused by its content the way a guard walk could.
    let delegation = match &s.delegation {
        None => String::new(),
        Some(d) => format!("{}~{}~{}~{}", d.id.0, d.space.0, d.delegator.0, d.expires.0),
    };
    let session = match &s.session {
        None => String::new(),
        Some(x) => format!(
            "{}~{}~{}~{}~{}~{}",
            x.id.0,
            x.started_at.0,
            x.second_step.map(|k| k.as_str()).unwrap_or(""),
            x.via.as_deref().unwrap_or(""),
            x.upstream_presence
                .as_ref()
                .map(|u| u.at().0.to_string())
                .unwrap_or_default(),
            x.upstream_presence.as_ref().map(|u| u.via()).unwrap_or("")
        ),
    };
    format!(
        "v6|{}|{}|{}|{}|{}|{}|{}",
        s.principal.0,
        s.expires.0,
        s.client.as_ref().map(|c| c.0.as_str()).unwrap_or(""),
        s.user_presence_at
            .map(|t| t.0.to_string())
            .unwrap_or_default(),
        grant,
        delegation,
        session
    )
}

fn decode(text: &str) -> Result<TokenSpec, TokenError> {
    let f: Vec<&str> = text.split('|').collect();
    if f.len() != 8 || f[0] != "v6" {
        return Err(TokenError);
    }
    let capability = if f[5].is_empty() {
        None
    } else {
        let g: Vec<&str> = f[5].split('~').collect();
        if g.len() != 4 {
            return Err(TokenError);
        }
        let blob = String::from_utf8(unb64(g[3])?).map_err(|_| TokenError)?;
        // A pin that does not decode is not a pin. Refusing the whole token
        // is the only sound response: the alternative is a grant with an
        // empty allowlist, which reads as "consented to nothing" and would
        // therefore deny — same outcome, reached by an accident rather than
        // by a rule.
        let pins = crate::pins::decode_pins(&blob).ok_or(TokenError)?;
        Some(CapabilityGrant::new(
            GrantId(g[0].to_string()),
            SpaceId(g[1].to_string()),
            pins,
            Timestamp(g[2].parse().map_err(|_| TokenError)?),
        ))
    };
    // A delegation that does not parse is not a delegation, and the whole
    // token is refused rather than read as "acting only as itself". The
    // downgrade would be the dangerous reading: the caller believes it is
    // Ada, the platform believes it is the agent, and the difference lands
    // in the audit chain as an attribution nobody intended.
    let delegation = if f[6].is_empty() {
        None
    } else {
        let d: Vec<&str> = f[6].split('~').collect();
        if d.len() != 4 || d.iter().take(3).any(|p| p.is_empty()) {
            return Err(TokenError);
        }
        Some(DelegationClaim {
            id: DelegationId(d[0].to_string()),
            space: SpaceId(d[1].to_string()),
            delegator: PrincipalId(d[2].to_string()),
            expires: Timestamp(d[3].parse().map_err(|_| TokenError)?),
        })
    };
    // A session that does not parse is not a session, and the whole token
    // is refused rather than read as "no session". The downgrade is the
    // dangerous reading here too: a token with an unreadable session field
    // would be a token the deny list cannot match, which is a logout that
    // silently stops working.
    let session = if f[7].is_empty() {
        None
    } else {
        let x: Vec<&str> = f[7].split('~').collect();
        if x.len() != 6 || x[0].is_empty() {
            return Err(TokenError);
        }
        // A provider's two-step claim is both halves or neither. Half of
        // one, or a connection name no token was minted with, is not a
        // token this deployment wrote.
        let upstream_presence = match (x[4], x[5]) {
            ("", "") => None,
            (at, via) => Some(
                crate::upstream::UpstreamPresence::new(
                    Timestamp(at.parse().map_err(|_| TokenError)?),
                    via,
                )
                .ok_or(TokenError)?,
            ),
        };
        // The way in is a word this deployment wrote: a method word or
        // `kind:connection`. Anything else is not a token it minted.
        let via = if x[3].is_empty() {
            None
        } else if x[3].len() <= 80
            && x[3]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b':')
        {
            Some(x[3].to_string())
        } else {
            return Err(TokenError);
        };
        // An unknown second-step word is refused, not read as "none": the
        // downgrade would be a session that looks as if it skipped a step it
        // took, or the other way round.
        let second_step = if x[2].is_empty() {
            None
        } else {
            Some(SecondStepKind::parse(x[2]).ok_or(TokenError)?)
        };
        Some(SessionClaim {
            id: SessionId(x[0].to_string()),
            started_at: Timestamp(x[1].parse().map_err(|_| TokenError)?),
            second_step,
            via,
            upstream_presence,
        })
    };
    Ok(TokenSpec {
        principal: PrincipalId(f[1].to_string()),
        expires: Timestamp(f[2].parse().map_err(|_| TokenError)?),
        client: if f[3].is_empty() {
            None
        } else {
            Some(ClientId(f[3].to_string()))
        },
        user_presence_at: if f[4].is_empty() {
            None
        } else {
            Some(Timestamp(f[4].parse().map_err(|_| TokenError)?))
        },
        capability,
        delegation,
        session,
    })
}

// ── primitives ───────────────────────────────────────────────────────────

/// Constant time over the compared bytes: an early return on the first
/// differing byte leaks the length of a correct prefix, which is enough to
/// forge a MAC one byte at a time.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    for p in parts {
        inner.update(p);
    }
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let take = chunk.len() + 1;
        for i in 0..take {
            out.push(B64[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
        }
    }
    out
}

fn unb64(s: &str) -> Result<Vec<u8>, TokenError> {
    let val = |c: u8| -> Result<u32, TokenError> {
        B64.iter()
            .position(|x| *x == c)
            .map(|p| p as u32)
            .ok_or(TokenError)
    };
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        if chunk.len() == 1 {
            return Err(TokenError);
        }
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        // A short last chunk leaves bits no byte uses. They must be zero, so
        // one token has one spelling: `...A` and `...B` may not both verify.
        let unused = 24 - 8 * (chunk.len() as u32 - 1);
        if unused < 24 && n & ((1 << unused) - 1) != 0 {
            return Err(TokenError);
        }
        for i in 0..chunk.len() - 1 {
            out.push(((n >> (16 - 8 * i)) & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_part_has_one_spelling() {
        // 32 bytes end on a 2-character chunk whose last 4 bits are unused.
        let tag = b64(&[0u8; 32]);
        assert!(tag.ends_with('A'));
        assert_eq!(unb64(&tag).unwrap(), vec![0u8; 32]);
        let mut other = tag.clone();
        other.pop();
        other.push('B');
        assert!(unb64(&other).is_err(), "nonzero unused bits must refuse");
        for len in 0..8usize {
            let bytes: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(unb64(&b64(&bytes)).unwrap(), bytes);
        }
    }

    fn key() -> TokenKey {
        TokenKey::from_secret(b"deployment-secret")
    }

    fn spec() -> TokenSpec {
        TokenSpec {
            principal: PrincipalId("usr_ada".into()),
            expires: Timestamp(1000),
            client: None,
            user_presence_at: None,
            capability: None,
            delegation: None,
            session: None,
        }
    }

    #[test]
    fn a_minted_token_verifies_and_carries_its_claims() {
        let t = mint(&key(), &spec());
        let c = verify(&key(), &t, Timestamp(999)).expect("valid");
        assert_eq!(c.principal, PrincipalId("usr_ada".into()));
        assert!(c.capability.is_none());
    }

    #[test]
    fn expiry_denies_at_the_instant_itself() {
        let t = mint(&key(), &spec());
        assert!(verify(&key(), &t, Timestamp(999)).is_ok());
        assert_eq!(verify(&key(), &t, Timestamp(1000)), Err(TokenError));
        assert_eq!(verify(&key(), &t, Timestamp(1001)), Err(TokenError));
    }

    #[test]
    fn forgery_of_every_shape_is_the_same_refusal() {
        let k = key();
        let t = mint(&k, &spec());

        // Another deployment's key.
        let other = TokenKey::from_secret(b"someone-else");
        assert_eq!(verify(&other, &t, Timestamp(0)), Err(TokenError));

        // An edited payload keeping the original MAC — the attack the MAC
        // exists for: promote yourself, or push the expiry out.
        let (p, m) = t.split_once('.').unwrap();
        let mut claims = String::from_utf8(unb64(p).unwrap()).unwrap();
        claims = claims.replace("usr_ada", "usr_eve");
        let forged = format!("{}.{}", b64(claims.as_bytes()), m);
        assert_eq!(verify(&k, &forged, Timestamp(0)), Err(TokenError));

        // Structural nonsense, at every joint.
        for bad in [
            "",
            ".",
            "junk",
            "a.b",
            &t.replace('.', ""),
            &format!("{t}x"),
        ] {
            assert_eq!(verify(&k, bad, Timestamp(0)), Err(TokenError), "{bad}");
        }
        // A flipped bit anywhere.
        for i in [0, 5, t.len() - 1] {
            let mut b = t.clone().into_bytes();
            b[i] = if b[i] == b'A' { b'B' } else { b'A' };
            let bad = String::from_utf8(b).unwrap();
            assert_eq!(verify(&k, &bad, Timestamp(0)), Err(TokenError));
        }
    }

    /// The whole pinned shape rides in the token, guard text and all — so
    /// the door decides from the token alone, with no registry history.
    ///
    /// The guard walk here is deliberately full of the characters a
    /// delimiter-separated encoding would choke on: spaces, dots,
    /// parentheses, `$` and `==`. A token whose grant survives *this* is a
    /// token whose grant survives any legal guard.
    #[test]
    fn capability_and_presence_claims_survive_the_round_trip() {
        use crate::pins::{EntityPin, GuardPin, OpPin};
        use std::collections::{BTreeMap, BTreeSet};

        let listed = EntityPin {
            entity: "orders".into(),
            read: BTreeSet::from(["amount".to_string(), "buyer".to_string()]),
            written: BTreeSet::new(),
            structure: false,
            read_guard: GuardPin::Walk("project.projectMembers(member == $caller)".into()),
            write_guard: GuardPin::Every,
            caller_restricted: true,
        };
        let pins = BTreeMap::from([
            (
                "orders.get".to_string(),
                OpPin {
                    manifest_hash: crate::ManifestHash([7u8; 32]),
                    unguarded: false,
                    entities: vec![listed.clone()],
                },
            ),
            (
                "orders.list".to_string(),
                OpPin {
                    manifest_hash: crate::ManifestHash([9u8; 32]),
                    unguarded: true,
                    entities: Vec::new(),
                },
            ),
        ]);
        let grant = CapabilityGrant::new(
            GrantId("gr_test".into()),
            SpaceId("sp_1".into()),
            pins.clone(),
            Timestamp(900),
        );
        let s = TokenSpec {
            client: Some(ClientId("cli_web".into())),
            user_presence_at: Some(Timestamp(42)),
            capability: Some(grant),
            ..spec()
        };
        let c = verify(&key(), &mint(&key(), &s), Timestamp(1)).expect("valid");
        assert_eq!(c.client, Some(ClientId("cli_web".into())));
        assert_eq!(c.user_presence_at, Some(Timestamp(42)));
        let g = c.capability.expect("grant");
        assert_eq!(g.space(), &SpaceId("sp_1".into()));
        assert_eq!(
            g.ops().cloned().collect::<Vec<_>>(),
            vec!["orders.get".to_string(), "orders.list".to_string()]
        );
        assert_eq!(g.pins(), &pins, "the pinned shape survives byte for byte");
        assert_eq!(g.expires(), Timestamp(900));
    }

    /// The second factor that proved a sign-in rides in the session claim and
    /// survives the round trip, and a token from before the field existed is
    /// refused outright rather than read as one that skipped the step.
    #[test]
    fn the_second_step_claim_round_trips_and_a_v3_token_is_refused_outright() {
        use crate::second_step::SecondStepKind;
        let with = TokenSpec {
            session: Some(
                SessionClaim::starting_at(Timestamp(5)).with_second_step(SecondStepKind::Totp),
            ),
            ..spec()
        };
        let c = verify(&key(), &mint(&key(), &with), Timestamp(1)).expect("valid");
        assert_eq!(
            c.session.and_then(|s| s.second_step),
            Some(SecondStepKind::Totp)
        );
        let without = TokenSpec {
            session: Some(SessionClaim::starting_at(Timestamp(5))),
            ..spec()
        };
        let c = verify(&key(), &mint(&key(), &without), Timestamp(1)).expect("valid");
        assert_eq!(c.session.and_then(|s| s.second_step), None);

        // A v4 payload, MAC'd correctly under this key: the version word alone
        // refuses it.
        let forge = |payload: &str| {
            let mac = hmac(
                &key().0,
                &[
                    DOMAIN,
                    &(payload.len() as u64).to_be_bytes(),
                    payload.as_bytes(),
                ],
            );
            format!("{}.{}", b64(payload.as_bytes()), b64(&mac))
        };
        assert_eq!(
            verify(
                &key(),
                &forge("v4|usr_ada|1000|||||sess_a~5~"),
                Timestamp(1)
            ),
            Err(TokenError)
        );

        // Control: the same payload in the v6 shape verifies.
        assert!(verify(
            &key(),
            &forge("v6|usr_ada|1000|||||sess_a~5~~~~"),
            Timestamp(1)
        )
        .is_ok());
        // A v5 token, from before a session could carry a provider's
        // two-step claim, is refused outright.
        assert_eq!(
            verify(
                &key(),
                &forge("v5|usr_ada|1000|||||sess_a~5~~"),
                Timestamp(1)
            ),
            Err(TokenError)
        );

        // An unknown second-step word is refused, not read as none.
        assert_eq!(
            verify(
                &key(),
                &forge("v6|usr_ada|1000|||||sess_a~5~carrier-pigeon~~~"),
                Timestamp(1)
            ),
            Err(TokenError)
        );
        // A session field with the old width is refused, not read as no `via`.
        assert_eq!(
            verify(
                &key(),
                &forge("v6|usr_ada|1000|||||sess_a~5~~"),
                Timestamp(1)
            ),
            Err(TokenError)
        );
        // A `via` this deployment could not have written is refused.
        assert_eq!(
            verify(
                &key(),
                &forge("v6|usr_ada|1000|||||sess_a~5~~oauth2:Discord Inc~~"),
                Timestamp(1)
            ),
            Err(TokenError)
        );
        // Half a provider's claim, or one naming no connection a token could
        // carry, is refused rather than read as none.
        for half in [
            "v6|usr_ada|1000|||||sess_a~5~~~40~",
            "v6|usr_ada|1000|||||sess_a~5~~~~corp",
            "v6|usr_ada|1000|||||sess_a~5~~~forty~corp",
            "v6|usr_ada|1000|||||sess_a~5~~~40~Corp Inc",
        ] {
            assert_eq!(verify(&key(), &forge(half), Timestamp(1)), Err(TokenError));
        }
    }

    /// A provider's two-step claim rides in the session claim and survives
    /// the round trip, separate from the token's own presence.
    #[test]
    fn an_upstream_presence_claim_round_trips_inside_the_session() {
        let up = crate::upstream::UpstreamPresence::new(Timestamp(40), "corp");
        let with = TokenSpec {
            session: Some(
                SessionClaim::starting_at(Timestamp(5))
                    .with_via("oidc:corp".into())
                    .with_upstream_presence(up.clone()),
            ),
            ..spec()
        };
        let c = verify(&key(), &mint(&key(), &with), Timestamp(1)).expect("valid");
        assert_eq!(
            c.user_presence_at, None,
            "never read as a passkey's presence"
        );
        assert_eq!(c.session.and_then(|s| s.upstream_presence), up);
    }

    #[test]
    fn the_way_in_round_trips_under_the_mac() {
        let spec = TokenSpec {
            session: Some(
                SessionClaim::starting_at(Timestamp(5)).with_via("oauth2:discord".into()),
            ),
            ..spec()
        };
        let c = verify(&key(), &mint(&key(), &spec), Timestamp(1)).expect("valid");
        assert_eq!(
            c.session.and_then(|s| s.via).as_deref(),
            Some("oauth2:discord")
        );
    }

    #[test]
    fn base64_round_trips_every_length_class() {
        for n in 0..40usize {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(unb64(&b64(&bytes)).unwrap(), bytes, "length {n}");
        }
    }
}
