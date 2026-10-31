//! A provider's two-step claim as presence (`plan-auth-federated.md` §8.2,
//! step 10, F14).
//!
//! # The ruling
//!
//! Owner, 2026-09-29: *"This should be up to the app dev to decide what is
//! safe or not. 2FA from a third party is often a standard... forcing internal
//! auth in addition would be a nuisace."* Amended the same day: *"A tenant
//! should also decide what it is what they need for their tenant."*
//!
//! # What this module holds
//!
//! - [`UpstreamPresence`]: the claim a session carries when an outside
//!   provider vouched, moments ago, that the person passed two steps there.
//!   It is its own type and never a [`crate::passkey::UserPresence`]. It
//!   rides inside the session claim, so a capability token or a delegated
//!   token, which have no session, can never carry one.
//! - [`PresenceRule`]: what the app developer set for one connection. Off by
//!   default.
//! - [`UpstreamAcceptance`]: the one object the access engine and the
//!   second step ask whether a claim from a connection counts for an op in
//!   a space. The deployment's `on` reaches every op. A tenant's acceptance
//!   reaches that tenant's own space and never an act on the deployment.
//!
//! # Any connection, for a tenant
//!
//! Owner, 2026-10-09, federated ask 10: *"Open"*. A tenant may accept any
//! connection's claim for its own data, whatever the app developer set. So
//! the deployment setting has two values, `off` and `on`. The `tenant`
//! value step 10 had meant "a tenant may accept it", which every connection
//! now is, and it was removed.
//!
//! # What counts
//!
//! A claim mints only when every one of these holds:
//!
//! 1. the sign-in asked for it, so the request carried `max_age=0`;
//! 2. the ID token carries `auth_time`, no older than the window and not in
//!    the future past the connection's clock skew;
//! 3. its `amr` holds one of the connection's two-step words (`mfa` by
//!    default), or its `acr` is one the connection names.
//!
//! A single-factor claim (`amr: ["pwd"]`) and a stale one mint nothing. A
//! minted claim counts only where [`UpstreamAcceptance::accepts`] says so.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use crate::principal::{SpaceId, Timestamp};
use crate::provider::UpstreamAssurance;

/// The window a claim must be fresh within, by default: the engine's own
/// step-up window. The setting may shorten it, never lengthen it.
pub const DEFAULT_MAX_AGE_SECS: u64 = crate::access::DEFAULT_STEP_UP_MAX_AGE_SECS;
/// The shortest window the setting takes.
pub const MIN_MAX_AGE_SECS: u64 = 30;
/// The `amr` word that counts when a connection names none: RFC 8176's
/// "multiple-factor authentication".
pub const DEFAULT_AMR: &str = "mfa";

/// A provider's word, kept: at most 64 bytes of `[A-Za-z0-9:._-]`. Anything
/// else is not a word this module compares or records.
fn word_ok(w: &str) -> bool {
    !w.is_empty()
        && w.len() <= 64
        && w.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":._-".contains(&b))
}

/// A connection name as a token writes it: 1 to 32 of `a-z`, `0-9` and `-`.
fn connection_ok(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= 32
        && c.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An outside provider's word that a person passed two steps there, moments
/// before this session began. Carried in [`crate::session::SessionClaim`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamPresence {
    at: Timestamp,
    via: String,
}

impl UpstreamPresence {
    /// A claim from connection `via`, made at `at`. `None` for a connection
    /// name no token could carry. The host builds one only from a token it
    /// minted or a ceremony row it filed; [`PresenceRule::judge`] is the one
    /// place a provider's answer becomes one.
    pub fn new(at: Timestamp, via: &str) -> Option<UpstreamPresence> {
        connection_ok(via).then(|| UpstreamPresence {
            at,
            via: via.to_string(),
        })
    }

    /// When the person passed the provider's two steps (`auth_time`).
    pub fn at(&self) -> Timestamp {
        self.at
    }

    /// The connection that vouched.
    pub fn via(&self) -> &str {
        &self.via
    }
}

/// What the app developer set for one connection
/// (`ORDO_AUTH_<NAME>_PRESENCE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PresenceMode {
    /// The claim counts only in a tenant that accepts it, for that tenant's
    /// own data. The default.
    #[default]
    Off,
    /// The claim counts everywhere, acts on the deployment included.
    On,
}

impl PresenceMode {
    pub fn parse(w: &str) -> Option<PresenceMode> {
        match w {
            "off" => Some(PresenceMode::Off),
            "on" => Some(PresenceMode::On),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PresenceMode::Off => "off",
            PresenceMode::On => "on",
        }
    }
}

/// One connection's rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceRule {
    pub mode: PresenceMode,
    /// `amr` words that mean two steps. [`DEFAULT_AMR`] by default.
    pub amr: Vec<String>,
    /// `acr` values that mean two steps. None by default.
    pub acr: Vec<String>,
    /// How old `auth_time` may be: [`MIN_MAX_AGE_SECS`] to
    /// [`DEFAULT_MAX_AGE_SECS`].
    pub max_age_secs: u64,
}

impl Default for PresenceRule {
    fn default() -> PresenceRule {
        PresenceRule {
            mode: PresenceMode::Off,
            amr: vec![DEFAULT_AMR.to_string()],
            acr: Vec::new(),
            max_age_secs: DEFAULT_MAX_AGE_SECS,
        }
    }
}

/// Why a claim minted no presence. A fixed word, for the security stream.
pub type NotMinted = &'static str;

impl PresenceRule {
    /// Read a comma list of provider words. An empty entry is skipped; a word
    /// with other characters is refused.
    pub fn parse_words(list: &str) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        for w in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            if !word_ok(w) {
                return Err(format!(
                    "`{w}` is not a provider word: 1 to 64 of letters, digits and `:._-`"
                ));
            }
            if !out.iter().any(|x| x == w) {
                out.push(w.to_string());
            }
        }
        Ok(out)
    }

    /// Does this provider answer make a two-step claim, from connection `via`?
    /// The mode is not read: under `off` a tenant may still accept the claim.
    ///
    /// `asked` is whether the sign-in sent `max_age=0`. `skew` is the
    /// connection's clock skew, the most `auth_time` may sit in the future.
    pub fn judge(
        &self,
        via: &str,
        asked: bool,
        assurance: Option<&UpstreamAssurance>,
        now: Timestamp,
        skew: u64,
    ) -> Result<UpstreamPresence, NotMinted> {
        if !asked {
            return Err("presence-not-asked");
        }
        let a = assurance.ok_or("presence-no-claims")?;
        let at = a.auth_time.ok_or("presence-no-auth-time")?;
        if at > now.0.saturating_add(skew) {
            return Err("presence-auth-time-in-the-future");
        }
        if now.0.saturating_sub(at) > self.max_age_secs {
            return Err("presence-stale");
        }
        let two_steps = a.amr.iter().any(|m| self.amr.iter().any(|w| w == m))
            || a.acr
                .as_deref()
                .is_some_and(|c| self.acr.iter().any(|w| w == c));
        if !two_steps {
            return Err("presence-single-factor");
        }
        // A future `auth_time` inside the skew is read as now: the engine
        // refuses a presence from the future as a clock lie.
        UpstreamPresence::new(Timestamp(at.min(now.0)), via).ok_or("presence-bad-connection")
    }
}

// ── a tenant's own connections (`plan-tenant-providers.md` §4, §7) ───────

/// How many hex digits of the tenant's hash a tenant connection's key
/// starts with, after the leading `t`.
const TENANT_HASH_HEX: usize = 20;
/// How many random hex digits follow the `-`.
pub const TENANT_KEY_RANDOM_HEX: usize = 10;

fn lower_hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The prefix every key of `space`'s own connections starts with:
/// `t<20 hex of sha256(space)>-`. It binds a key to its tenant, so a claim
/// carried under that key can be kept inside the tenant.
pub fn tenant_key_prefix(space: &SpaceId) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(space.0.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("t{}-", &hex[..TENANT_HASH_HEX])
}

/// A key for a new connection of `space`: the prefix and `random`, which
/// must be [`TENANT_KEY_RANDOM_HEX`] lowercase hex digits. 32 characters,
/// a valid connection name.
pub fn tenant_key(space: &SpaceId, random: &str) -> Option<String> {
    lower_hex(random, TENANT_KEY_RANDOM_HEX)
        .then(|| format!("{}{random}", tenant_key_prefix(space)))
}

/// Is `via` the key of a tenant's own connection? Shape only.
pub fn is_tenant_key(via: &str) -> bool {
    match via.strip_prefix('t').and_then(|rest| rest.split_once('-')) {
        Some((hash, random)) => {
            lower_hex(hash, TENANT_HASH_HEX) && lower_hex(random, TENANT_KEY_RANDOM_HEX)
        }
        None => false,
    }
}

/// Is `via` a key of `space`'s own connections?
pub fn tenant_key_of(via: &str, space: &SpaceId) -> bool {
    is_tenant_key(via) && via.starts_with(&tenant_key_prefix(space))
}

/// Which connections' claims count where. Shared by every engine a
/// deployment builds, and by the admission op that writes a tenant's choice,
/// so a choice made is the choice the next request meets.
#[derive(Debug, Default)]
pub struct UpstreamAcceptance {
    modes: BTreeMap<String, PresenceMode>,
    deployment_space: Option<SpaceId>,
    /// Ops a tenant's acceptance never reaches: exact names, or a prefix
    /// ending `.*`.
    deployment_ops: Vec<String>,
    max_age_secs: u64,
    tenants: RwLock<BTreeMap<SpaceId, BTreeSet<String>>>,
}

fn op_matches(pattern: &str, op: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => op.len() > prefix.len() + 1 && op.starts_with(&format!("{prefix}.")),
        None => pattern == op,
    }
}

impl UpstreamAcceptance {
    /// Nothing counts anywhere, within the default window.
    pub fn new() -> UpstreamAcceptance {
        UpstreamAcceptance {
            max_age_secs: DEFAULT_MAX_AGE_SECS,
            ..UpstreamAcceptance::default()
        }
    }

    /// Connection `name` under `mode`.
    pub fn with_connection(mut self, name: &str, mode: PresenceMode) -> UpstreamAcceptance {
        self.modes.insert(name.to_string(), mode);
        self
    }

    /// The deployment's own space. No tenant's acceptance reaches an op in it.
    pub fn with_deployment_space(mut self, space: SpaceId) -> UpstreamAcceptance {
        self.deployment_space = Some(space);
        self
    }

    /// Ops that act on the deployment: no tenant's acceptance reaches them,
    /// whatever space they are called in.
    pub fn with_deployment_ops(mut self, ops: &[&str]) -> UpstreamAcceptance {
        self.deployment_ops = ops.iter().map(|s| s.to_string()).collect();
        self
    }

    /// How old a claim may be when it is used. The engine's own window
    /// still applies: the shorter wins.
    pub fn with_max_age(mut self, secs: u64) -> UpstreamAcceptance {
        self.max_age_secs = secs;
        self
    }

    pub fn max_age_secs(&self) -> u64 {
        self.max_age_secs
    }

    /// The app developer's setting for a connection. `Off` for one never
    /// named.
    pub fn mode(&self, connection: &str) -> PresenceMode {
        self.modes.get(connection).copied().unwrap_or_default()
    }

    /// Is `op` an act on the deployment?
    pub fn deployment_op(&self, op: &str) -> bool {
        self.deployment_ops.iter().any(|p| op_matches(p, op))
    }

    /// What a tenant accepts.
    pub fn tenant(&self, space: &SpaceId) -> BTreeSet<String> {
        self.tenants
            .read()
            .expect("upstream acceptance")
            .get(space)
            .cloned()
            .unwrap_or_default()
    }

    /// Set what a tenant accepts. Empty removes it.
    pub fn set_tenant(&self, space: &SpaceId, connections: BTreeSet<String>) {
        let mut t = self.tenants.write().expect("upstream acceptance");
        if connections.is_empty() {
            t.remove(space);
        } else {
            t.insert(space.clone(), connections);
        }
    }

    /// Replace every tenant's choice at once, from a fresh read.
    pub fn replace_tenants(&self, all: BTreeMap<SpaceId, BTreeSet<String>>) {
        *self.tenants.write().expect("upstream acceptance") = all;
    }

    /// Is `name` a connection of this deployment?
    pub fn knows(&self, name: &str) -> bool {
        self.modes.contains_key(name)
    }

    /// The deployment's connections, by name.
    pub fn connections(&self) -> impl Iterator<Item = &str> {
        self.modes.keys().map(String::as_str)
    }

    /// Does the app developer's setting accept a claim from `via`
    /// everywhere?
    pub fn deployment_accepts(&self, via: &str) -> bool {
        // A tenant's own connection is never the deployment's
        // (`plan-tenant-providers.md` §7).
        !is_tenant_key(via) && self.mode(via) == PresenceMode::On
    }

    /// Does tenant `space` accept a claim from `via` for its own data? Never
    /// in the deployment's own space. Whatever the app developer set for
    /// the connection (ask 10). A tenant's own connection counts only in the
    /// tenant whose key it is, and only once that tenant lists its key.
    pub fn tenant_accepts(&self, space: &SpaceId, via: &str) -> bool {
        (!is_tenant_key(via) || tenant_key_of(via, space))
            && self.deployment_space.as_ref() != Some(space)
            && self
                .tenants
                .read()
                .expect("upstream acceptance")
                .get(space)
                .is_some_and(|s| s.contains(via))
    }

    /// Does a claim from `via` count for `op` in `space`?
    ///
    /// The deployment's `on` reaches everything. A tenant's acceptance
    /// reaches only an op that is not an act on the deployment, called in a
    /// space that is not the deployment's own.
    pub fn accepts(&self, space: &SpaceId, op: &str, via: &str) -> bool {
        self.deployment_accepts(via) || (!self.deployment_op(op) && self.tenant_accepts(space, via))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assurance(amr: &[&str], acr: Option<&str>, at: Option<u64>) -> UpstreamAssurance {
        UpstreamAssurance {
            amr: amr.iter().map(|s| s.to_string()).collect(),
            acr: acr.map(str::to_string),
            auth_time: at,
        }
    }

    fn on() -> PresenceRule {
        PresenceRule {
            mode: PresenceMode::On,
            ..PresenceRule::default()
        }
    }

    const NOW: Timestamp = Timestamp(10_000);

    /// Off by default, and off counts nowhere a tenant did not accept. The
    /// claim is still minted, so a tenant can accept it (ask 10).
    #[test]
    fn the_rule_is_off_by_default() {
        let r = PresenceRule::default();
        assert_eq!(r.mode, PresenceMode::Off);
        assert!(r
            .judge(
                "corp",
                true,
                Some(&assurance(&["mfa"], None, Some(NOW.0))),
                NOW,
                60
            )
            .is_ok());
        let a = UpstreamAcceptance::new();
        assert_eq!(a.mode("corp"), PresenceMode::Off);
        assert!(!a.accepts(&SpaceId("spc_acme".into()), "std.space.export", "corp"));
        assert_eq!(PresenceMode::parse("tenant"), None, "two values only");
    }

    #[test]
    fn a_fresh_mfa_claim_mints_and_every_gap_mints_nothing() {
        let r = on();
        let fresh = assurance(&["pwd", "mfa"], None, Some(NOW.0 - 10));
        let p = r
            .judge("corp", true, Some(&fresh), NOW, 60)
            .expect("minted");
        assert_eq!((p.at(), p.via()), (Timestamp(NOW.0 - 10), "corp"));
        assert_eq!(
            r.judge("corp", false, Some(&fresh), NOW, 60),
            Err("presence-not-asked")
        );
        assert_eq!(
            r.judge(
                "corp",
                true,
                Some(&assurance(&["pwd"], None, Some(NOW.0))),
                NOW,
                60
            ),
            Err("presence-single-factor")
        );
        assert_eq!(
            r.judge(
                "corp",
                true,
                Some(&assurance(&["mfa"], None, None)),
                NOW,
                60
            ),
            Err("presence-no-auth-time")
        );
        let stale = assurance(&["mfa"], None, Some(NOW.0 - DEFAULT_MAX_AGE_SECS - 1));
        assert_eq!(
            r.judge("corp", true, Some(&stale), NOW, 60),
            Err("presence-stale")
        );
        let future = assurance(&["mfa"], None, Some(NOW.0 + 61));
        assert_eq!(
            r.judge("corp", true, Some(&future), NOW, 60),
            Err("presence-auth-time-in-the-future")
        );
        assert_eq!(
            r.judge("corp", true, None, NOW, 60),
            Err("presence-no-claims")
        );
    }

    #[test]
    fn an_acr_counts_only_when_the_connection_names_it() {
        let gold = assurance(&[], Some("urn:corp:loa:2fa"), Some(NOW.0));
        assert_eq!(
            on().judge("corp", true, Some(&gold), NOW, 60),
            Err("presence-single-factor")
        );
        let named = PresenceRule {
            acr: vec!["urn:corp:loa:2fa".into()],
            ..on()
        };
        assert!(named.judge("corp", true, Some(&gold), NOW, 60).is_ok());
    }

    #[test]
    fn a_tenants_acceptance_reaches_its_own_space_and_no_deployment_act() {
        let a = UpstreamAcceptance::new()
            .with_connection("corp", PresenceMode::Off)
            .with_connection("everywhere", PresenceMode::On)
            .with_deployment_space(SpaceId("spc_home".into()))
            .with_deployment_ops(&["std.deployment.*", "std.space.delete"]);
        let mine = SpaceId("spc_acme".into());
        let other = SpaceId("spc_other".into());
        let home = SpaceId("spc_home".into());
        assert!(
            !a.accepts(&mine, "std.space.export", "corp"),
            "not yet accepted"
        );
        a.set_tenant(&mine, ["corp".to_string()].into());
        assert!(
            a.accepts(&mine, "std.space.export", "corp"),
            "a tenant accepts a connection the app developer left off"
        );
        assert!(a.tenant_accepts(&mine, "corp"));
        assert!(!a.deployment_accepts("corp"));
        assert!(!a.accepts(&mine, "std.space.delete", "corp"));
        assert!(!a.accepts(&mine, "std.deployment.apply", "corp"));
        assert!(!a.accepts(&home, "std.space.export", "corp"));
        assert!(!a.accepts(&other, "std.space.export", "corp"));
        assert!(
            !a.accepts(&other, "std.space.export", "corp"),
            "off stays off where no tenant accepted it"
        );
        a.set_tenant(&home, ["corp".to_string()].into());
        assert!(
            !a.tenant_accepts(&home, "corp"),
            "the deployment's own space is never a tenant's"
        );
        assert!(a.accepts(&home, "std.deployment.apply", "everywhere"));
        a.set_tenant(&mine, BTreeSet::new());
        assert!(!a.accepts(&mine, "std.space.export", "corp"));
    }

    /// The engine end: the claim satisfies step-up where it is accepted and
    /// fresh, and nowhere else, and never on a token that is not a session.
    #[test]
    fn the_engine_counts_a_claim_only_where_accepted_and_fresh() {
        use crate::access::{AccessEngine, DenyReason, TokenClaims};
        use crate::principal::{AccessProfile, PrincipalId, Role};
        use crate::session::SessionClaim;
        use std::sync::Arc;

        let space = SpaceId("spc_acme".into());
        let ana = PrincipalId("usr_ana".into());
        let mut engine = AccessEngine::new();
        engine.set_profile(
            &space,
            AccessProfile::new().allow(Role::Owner, "std.space.export"),
        );
        engine.set_membership(&space, &ana, Role::Owner);
        engine.require_step_up("std.space.export");
        let claims = |at: u64, via: &str| TokenClaims {
            principal: ana.clone(),
            capability: None,
            client: None,
            user_presence_at: None,
            delegation: None,
            session: Some(
                SessionClaim::starting_at(Timestamp(at))
                    .with_upstream_presence(UpstreamPresence::new(Timestamp(at), via)),
            ),
        };
        let fresh = claims(NOW.0 - 10, "corp");
        assert_eq!(
            engine.check(&fresh, &space, "std.space.export", NOW),
            crate::access::Decision::Deny {
                op: "std.space.export".into(),
                reason: DenyReason::StepUpRequired
            },
            "no acceptance installed: the claim is not read"
        );
        engine.accept_upstream(Arc::new(
            UpstreamAcceptance::new().with_connection("corp", PresenceMode::On),
        ));
        assert!(engine
            .check(&fresh, &space, "std.space.export", NOW)
            .is_allow());
        let stale = claims(NOW.0 - DEFAULT_MAX_AGE_SECS - 1, "corp");
        assert!(matches!(
            engine.check(&stale, &space, "std.space.export", NOW),
            crate::access::Decision::Deny {
                reason: DenyReason::StepUpStale { .. },
                ..
            }
        ));
        let other = claims(NOW.0 - 10, "elsewhere");
        assert!(!engine
            .check(&other, &space, "std.space.export", NOW)
            .is_allow());
    }

    #[test]
    fn a_tenant_connections_claim_counts_only_inside_its_tenant() {
        let acme = SpaceId("spc_acme".into());
        let other = SpaceId("spc_other".into());
        let home = SpaceId("spc_home".into());
        let key = tenant_key(&acme, "0123456789").expect("a key");
        assert_eq!(key.len(), 32);
        assert!(crate::oauth2::valid_connection_name(&key));
        assert!(is_tenant_key(&key) && tenant_key_of(&key, &acme));
        assert!(!tenant_key_of(&key, &other));
        assert!(tenant_key(&acme, "XYZ").is_none());
        let a = UpstreamAcceptance::new()
            .with_deployment_space(home.clone())
            .with_deployment_ops(&["std.deployment.*", "std.space.delete"])
            // A deployment setting of `on` under the same word does not
            // reach it: a tenant key is never read as a deployment
            // connection.
            .with_connection(&key, PresenceMode::On);
        assert!(
            !a.accepts(&acme, "std.space.export", &key),
            "not until the tenant lists it"
        );
        a.set_tenant(&acme, [key.clone()].into());
        // Another tenant listing a key that is not its own gains nothing.
        a.set_tenant(&other, [key.clone()].into());
        a.set_tenant(&home, [key.clone()].into());
        assert!(a.accepts(&acme, "std.space.export", &key));
        assert!(!a.accepts(&other, "std.space.export", &key));
        assert!(!a.accepts(&home, "std.space.export", &key));
        assert!(!a.accepts(&acme, "std.space.delete", &key));
        assert!(!a.accepts(&acme, "std.deployment.apply", &key));
        assert!(!is_tenant_key("corp") && !is_tenant_key("t-abc"));
    }

    #[test]
    fn provider_words_are_checked() {
        assert_eq!(
            PresenceRule::parse_words(" mfa, hwk ,,mfa").unwrap(),
            vec!["mfa".to_string(), "hwk".to_string()]
        );
        assert!(PresenceRule::parse_words("m f a").is_err());
        assert!(UpstreamPresence::new(Timestamp(1), "Corp").is_none());
    }
}
