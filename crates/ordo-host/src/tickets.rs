//! Enrolment tickets (`plan-enrolment.md` §4.1, E2).
//!
//! A ticket is a one-time secret that somebody with the authority hands to a
//! person who does not yet hold a credential. The person redeems it to do one
//! thing: set a first password, register a first passkey, or join a tenant.
//! One mechanism serves three flows, and none of them is compulsory.
//!
//! # What is stored
//!
//! The SHA-256 of the ticket, never the ticket. A leaked table holds nothing
//! anybody can redeem. The ticket is 256 random bits, so a fast hash is the
//! right one: there is no low-entropy secret here to slow an attacker down
//! over.
//!
//! # One answer
//!
//! A ticket that never existed, one that has expired, one already redeemed
//! and one that is malformed are the same [`TicketError::Refused`]. A caller
//! who could tell them apart could tell a ticket that was real from one that
//! was not.
//!
//! # Single use is the store's, not the caller's
//!
//! [`TicketStore::take`] marks a ticket redeemed and returns it in one atomic
//! step. A read followed by a write would let two requests holding the same
//! ticket both win. The Postgres store does it in one `UPDATE ... WHERE
//! "redeemed_at" IS NULL RETURNING`, so two replicas cannot both redeem.
//!
//! # The seam passkey sign-in calls
//!
//! `plan-passkey-signin.md` §7 names `redeem(ticket, now) ->
//! Result<Redeemed, TicketError>`. A function with no store cannot redeem
//! anything, so the store is the first argument here ([`redeem`]) and
//! [`Tickets::redeem`] holds it. `Redeemed` carries the three fields that
//! plan names, and two more (`space`, `role`) that an invitation needs.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ordo_identity::directory::random_principal_suffix;
use ordo_identity::principal::PrincipalId;
use ordo_secevent::Recorder;
use sha2::{Digest, Sha256};

use crate::pgsync::{cell, PgSync, Source};
use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError};
use ordo_identity::StoreFault;

/// The word every ticket starts with. It makes a ticket recognisable in a
/// log scrub and cheap to refuse when it is not one.
pub const PREFIX: &str = "tkt_";
/// A ticket is the prefix and 64 hex characters: 256 random bits.
const BODY_LEN: usize = 64;
/// Three days, when the issuer names no lifetime.
pub const DEFAULT_TTL_SECS: u64 = 3 * 24 * 3600;
/// Thirty days, the longest a ticket may live. A ticket is a bearer secret
/// and the longer it lives the longer it can be found in somebody's mail.
pub const MAX_TTL_SECS: u64 = 30 * 24 * 3600;

/// What redeeming a ticket lets the holder do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Set a first password (or, after a reset, a new one).
    Password,
    /// Register a first passkey.
    Passkey,
    /// Join a tenant, at the role the ticket names.
    Invitation,
    /// Accept a share: become a guest in the tenant the ticket names, with
    /// a window over the rows and ops its payload lists
    /// (`plan-sharing.md` §3.3).
    Share,
    /// Register a **passkey** to recover a person who has lost every way in
    /// (`plan-passkey-recovery.md` §4.1). Redeemable only by completing a
    /// passkey registration ceremony at `/auth/recover/complete`. No
    /// endpoint that takes a password asks for this purpose, so a recovery
    /// cannot end in one.
    Recovery,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Password => "password",
            Purpose::Passkey => "passkey",
            Purpose::Invitation => "invitation",
            Purpose::Share => "share",
            Purpose::Recovery => "recovery",
        }
    }

    fn parse(s: &str) -> Option<Purpose> {
        match s {
            "password" => Some(Purpose::Password),
            "passkey" => Some(Purpose::Passkey),
            "invitation" => Some(Purpose::Invitation),
            "share" => Some(Purpose::Share),
            "recovery" => Some(Purpose::Recovery),
            _ => None,
        }
    }
}

/// What the issuer decides.
#[derive(Debug, Clone)]
pub struct TicketSpec {
    pub purpose: Purpose,
    /// The person the ticket is for, when they already exist as a principal
    /// (an administrator created them). `None` for a person who will be
    /// minted when they redeem: an invitation to a stranger.
    pub principal: Option<PrincipalId>,
    /// A name to show the person. Free text.
    pub display: String,
    /// The tenant an invitation admits to. `None` for the other purposes.
    pub space: Option<String>,
    /// The role an invitation admits at.
    pub role: Option<String>,
    /// Who issued it: a principal id, or a word for the platform.
    pub issued_by: String,
    /// How long it lives. [`DEFAULT_TTL_SECS`] when zero, and never more
    /// than [`MAX_TTL_SECS`].
    pub ttl_secs: u64,
}

/// A ticket as stored. `hash` is the only handle on it.
#[derive(Debug, Clone)]
pub struct TicketRecord {
    pub hash: String,
    pub purpose: Purpose,
    pub principal: Option<PrincipalId>,
    pub display: String,
    pub space: Option<String>,
    pub role: Option<String>,
    pub issued_by: String,
    pub issued_at: u64,
    pub expires_at: u64,
    /// What the ticket carries beyond who it is for: for a share, the
    /// intent (ops, rows, expiry) as JSON. `None` for every other purpose.
    /// Set through [`Tickets::issue_with_payload`], which leaves
    /// [`TicketSpec`] as it was for every caller that never had one.
    pub payload: Option<String>,
}

/// What a redemption insists on before it consumes anything.
///
/// A ticket presented for the wrong purpose, at the wrong tenant, or by
/// somebody it does not name is **not consumed**: otherwise anybody who saw
/// a ticket could spend it, and the person it was for would find it gone.
#[derive(Debug, Clone, Default)]
pub struct Want<'a> {
    pub purpose: Option<Purpose>,
    pub space: Option<&'a str>,
    /// The principal presenting it. A ticket that names a principal is for
    /// that principal alone. A ticket that names none is for whoever holds
    /// it.
    pub holder: Option<&'a str>,
}

impl Want<'_> {
    fn met_by(&self, rec: &TicketRecord) -> bool {
        self.purpose.is_none_or(|p| p == rec.purpose)
            && self.space.is_none_or(|s| rec.space.as_deref() == Some(s))
            && match (&rec.principal, self.holder) {
                (Some(named), Some(holder)) => named.0 == holder,
                _ => true,
            }
    }
}

/// What issuing returns: the ticket, once, and when it stops working.
#[derive(Debug, Clone)]
pub struct Issued {
    pub ticket: String,
    pub expires_at: u64,
}

/// What redeeming yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redeemed {
    pub principal: Option<PrincipalId>,
    pub purpose: Purpose,
    pub display: String,
    pub space: Option<String>,
    pub role: Option<String>,
    /// The ticket's payload; see [`TicketRecord::payload`].
    pub payload: Option<String>,
}

/// Why a redemption did not yield a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TicketError {
    /// Unknown, expired, already redeemed or malformed. One answer.
    Refused,
    /// The store could not answer, with its typed class. Fail closed, and
    /// say so to the deployment owner.
    Unavailable(StoreFault),
}

impl std::fmt::Display for TicketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TicketError::Refused => f.write_str("that ticket cannot be used"),
            TicketError::Unavailable(why) => write!(f, "tickets are unavailable: {why}"),
        }
    }
}

impl std::error::Error for TicketError {}

/// Where tickets are kept.
pub trait TicketStore: Send + Sync {
    fn insert(&self, rec: &TicketRecord) -> Result<(), StoreFault>;

    /// Mark the ticket redeemed **and return it**, atomically, if and only
    /// if it exists, is unredeemed, has not expired at `now` and meets
    /// `want`.
    fn take(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault>;

    /// The same question as [`TicketStore::take`], **answered without
    /// spending the ticket**. For a ceremony that must know who a ticket is
    /// for before it begins (a passkey registration lists the person's
    /// existing credentials) and spends the ticket only when it completes.
    fn peek(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault>;

    /// Undo one spend: make the ticket redeemable again **only if** it is
    /// still the spend made at `spent_at`, and not a decline. For a caller
    /// that spent a ticket and then could not finish what it was spent for.
    /// `true` when it was given back.
    fn give_back(&self, _hash: &str, _spent_at: u64) -> Result<bool, StoreFault> {
        Err("this ticket store cannot give a ticket back".into())
    }

    /// The tickets of `purpose` for `space` that can still be redeemed at
    /// `now`, oldest first. A store that cannot list says so, and the ops
    /// that need it refuse rather than answer an empty list.
    fn list_open(
        &self,
        _space: &str,
        _purpose: Purpose,
        _now: u64,
    ) -> Result<Vec<TicketRecord>, StoreFault> {
        Err("this ticket store cannot list tickets".into())
    }

    /// Withdraw the unredeemed ticket of `purpose` for `space` whose hash
    /// begins with `id`. Returns how many it withdrew.
    fn revoke_open(&self, _space: &str, _purpose: Purpose, _id: &str) -> Result<u64, StoreFault> {
        Err("this ticket store cannot withdraw tickets".into())
    }

    /// Spend the ticket **as declined**: the same atomic step as
    /// [`TicketStore::take`], with the decline kept beside it so the issuer
    /// can see the ticket was refused rather than used. Returns the ticket
    /// as it was.
    fn decline(
        &self,
        _hash: &str,
        _now: u64,
        _want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault> {
        Err("this ticket store cannot record a decline".into())
    }

    /// The declined tickets of `purpose` for `space`, with when each was
    /// declined, oldest first. A declined ticket is kept until the instant
    /// it would have expired, and then swept like any other.
    fn list_declined(
        &self,
        _space: &str,
        _purpose: Purpose,
        _now: u64,
    ) -> Result<Vec<(TicketRecord, u64)>, StoreFault> {
        Err("this ticket store cannot list tickets".into())
    }

    /// Remove every ticket whose life has ended. Returns how many of them
    /// were never redeemed, which is the count the record names.
    fn sweep(&self, now: u64) -> Result<u64, StoreFault>;
}

/// How many characters of a ticket's hash name it in a listing and in a
/// revocation. Sixteen hex characters are 64 bits: enough that two open
/// tickets of one tenant never share one, and nothing anybody can redeem.
pub const ID_LEN: usize = 16;

/// The handle a listing shows for a ticket. It is not the ticket.
pub fn id_of(rec: &TicketRecord) -> &str {
    &rec.hash[..ID_LEN.min(rec.hash.len())]
}

/// The hash a ticket is stored under.
pub fn hash_of(ticket: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"ordo.ticket.v1\0");
    h.update(ticket.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn well_formed(ticket: &str) -> bool {
    ticket
        .strip_prefix(PREFIX)
        .is_some_and(|b| b.len() == BODY_LEN && b.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn mint_ticket() -> String {
    format!(
        "{PREFIX}{}{}",
        random_principal_suffix(),
        random_principal_suffix()
    )
}

/// Redeem a ticket against a store, insisting on nothing about who holds it.
pub fn redeem(store: &dyn TicketStore, ticket: &str, now: u64) -> Result<Redeemed, TicketError> {
    redeem_for(store, ticket, now, &Want::default())
}

/// Redeem a ticket that must meet `want`.
pub fn redeem_for(
    store: &dyn TicketStore,
    ticket: &str,
    now: u64,
    want: &Want<'_>,
) -> Result<Redeemed, TicketError> {
    if !well_formed(ticket) {
        return Err(TicketError::Refused);
    }
    match store.take(&hash_of(ticket), now, want) {
        Ok(Some(r)) => Ok(Redeemed {
            principal: r.principal,
            purpose: r.purpose,
            display: r.display,
            space: r.space,
            role: r.role,
            payload: r.payload,
        }),
        Ok(None) => Err(TicketError::Refused),
        Err(e) => Err(TicketError::Unavailable(e)),
    }
}

/// Tickets, with a clock and, when there is one, the record.
pub struct Tickets {
    store: Arc<dyn TicketStore>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
    stream: Option<Arc<Recorder>>,
}

impl Tickets {
    pub fn new(store: Arc<dyn TicketStore>) -> Tickets {
        Tickets {
            store,
            now: Box::new(crate::runtime::now_secs),
            stream: None,
        }
    }

    pub fn with_clock(mut self, now: Box<dyn Fn() -> u64 + Send + Sync>) -> Tickets {
        self.now = now;
        self
    }

    pub fn with_recorder(mut self, stream: Arc<Recorder>) -> Tickets {
        self.stream = Some(stream);
        self
    }

    /// Mint a ticket and store its hash. The ticket is in the answer once
    /// and nowhere else.
    pub fn issue(&self, spec: TicketSpec) -> Result<Issued, StoreFault> {
        self.issue_with_payload(spec, None)
    }

    /// [`Tickets::issue`] for a ticket that carries a payload. The payload
    /// is stored beside the hash and comes back on a peek or a redemption;
    /// it is never in the answer to the issuer beyond what they sent.
    pub fn issue_with_payload(
        &self,
        spec: TicketSpec,
        payload: Option<String>,
    ) -> Result<Issued, StoreFault> {
        let now = (self.now)();
        let ttl = match spec.ttl_secs {
            0 => DEFAULT_TTL_SECS,
            t => t.min(MAX_TTL_SECS),
        };
        let ticket = mint_ticket();
        let rec = TicketRecord {
            hash: hash_of(&ticket),
            purpose: spec.purpose,
            principal: spec.principal,
            display: spec.display,
            space: spec.space,
            role: spec.role,
            issued_by: spec.issued_by,
            issued_at: now,
            expires_at: now + ttl,
            payload,
        };
        self.store.insert(&rec)?;
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "ticket-issued",
                    detail: format!(
                        "purpose={} ticket={} for={} space={} by={}",
                        rec.purpose.as_str(),
                        &rec.hash[..8],
                        rec.principal.as_ref().map(|p| p.0.as_str()).unwrap_or("-"),
                        rec.space.as_deref().unwrap_or("-"),
                        rec.issued_by,
                    ),
                    by: None,
                    tenant: rec.space.as_deref(),
                    deployment: true,
                    at: now,
                },
            );
        }
        Ok(Issued {
            ticket,
            expires_at: rec.expires_at,
        })
    }

    /// Who a ticket is for and what it allows, **without spending it**. One
    /// that does not meet `want` is the same refusal as one that does not
    /// exist.
    pub fn peek(&self, ticket: &str, want: &Want<'_>) -> Result<TicketRecord, TicketError> {
        if !well_formed(ticket) {
            return Err(TicketError::Refused);
        }
        match self.store.peek(&hash_of(ticket), (self.now)(), want) {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(TicketError::Refused),
            Err(e) => Err(TicketError::Unavailable(e)),
        }
    }

    /// Redeem, once.
    pub fn redeem(&self, ticket: &str) -> Result<Redeemed, TicketError> {
        self.redeem_for(ticket, &Want::default())
    }

    /// Redeem, once, a ticket that must meet `want`. One that does not is
    /// refused and left for its holder.
    pub fn redeem_for(&self, ticket: &str, want: &Want<'_>) -> Result<Redeemed, TicketError> {
        self.spend_for(ticket, want).map(|(out, _)| out)
    }

    /// [`Tickets::redeem_for`], also answering when the ticket was spent,
    /// which is what [`Tickets::give_back`] needs to undo this spend and
    /// no other.
    pub fn spend_for(&self, ticket: &str, want: &Want<'_>) -> Result<(Redeemed, u64), TicketError> {
        let now = (self.now)();
        let out = redeem_for(self.store.as_ref(), ticket, now, want)?;
        if let Some(stream) = &self.stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "ticket-redeemed",
                    detail: format!(
                        "purpose={} ticket={} for={} space={}",
                        out.purpose.as_str(),
                        &hash_of(ticket)[..8],
                        out.principal.as_ref().map(|p| p.0.as_str()).unwrap_or("-"),
                        out.space.as_deref().unwrap_or("-"),
                    ),
                    by: None,
                    tenant: None,
                    deployment: true,
                    at: now,
                },
            );
        }
        Ok((out, now))
    }

    /// Make a ticket spent at `spent_at` redeemable again, when what it was
    /// spent for could not be finished. Only that spend is undone: a ticket
    /// declined, swept, or spent at another moment is left as it is.
    pub fn give_back(&self, ticket: &str, spent_at: u64) -> Result<bool, TicketError> {
        if !well_formed(ticket) {
            return Ok(false);
        }
        self.store
            .give_back(&hash_of(ticket), spent_at)
            .map_err(TicketError::Unavailable)
    }

    /// Decline a ticket that meets `want`, once. The ticket is spent, so it
    /// can no longer be redeemed, and the store keeps that it was declined
    /// until it would have expired. A ticket that does not meet `want` is
    /// the one refusal and is left whole for its holder.
    pub fn decline(&self, ticket: &str, want: &Want<'_>) -> Result<TicketRecord, TicketError> {
        if !well_formed(ticket) {
            return Err(TicketError::Refused);
        }
        let now = (self.now)();
        match self.store.decline(&hash_of(ticket), now, want) {
            Ok(Some(r)) => Ok(r),
            Ok(None) => Err(TicketError::Refused),
            Err(e) => Err(TicketError::Unavailable(e)),
        }
    }

    /// The declined tickets of `purpose` for `space`, with when each was
    /// declined.
    pub fn list_declined(
        &self,
        space: &str,
        purpose: Purpose,
    ) -> Result<Vec<(TicketRecord, u64)>, StoreFault> {
        self.store.list_declined(space, purpose, (self.now)())
    }

    /// The redeemable tickets of `purpose` for `space`, oldest first.
    pub fn list_open(
        &self,
        space: &str,
        purpose: Purpose,
    ) -> Result<Vec<TicketRecord>, StoreFault> {
        self.store.list_open(space, purpose, (self.now)())
    }

    /// Withdraw one redeemable ticket by the id a listing showed. An id of
    /// the wrong shape withdraws nothing.
    pub fn revoke_open(&self, space: &str, purpose: Purpose, id: &str) -> Result<u64, StoreFault> {
        if id.len() != ID_LEN || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Ok(0);
        }
        self.store
            .revoke_open(space, purpose, &id.to_ascii_lowercase())
    }

    /// Remove dead tickets and record how many expired unused.
    pub fn sweep(&self) -> Result<u64, StoreFault> {
        let now = (self.now)();
        let unused = self.store.sweep(now)?;
        if unused > 0 {
            if let Some(stream) = &self.stream {
                crate::enrolment::record(
                    stream,
                    &crate::enrolment::Act {
                        act: "ticket-expired",
                        detail: format!("unused={unused}"),
                        by: None,
                        tenant: None,
                        deployment: true,
                        at: now,
                    },
                );
            }
        }
        Ok(unused)
    }
}

/// The process-local store, for tests and for nothing a deployment runs.
#[derive(Default)]
pub struct MemoryTicketStore {
    rows: Mutex<BTreeMap<String, (TicketRecord, bool)>>,
    /// The hashes of declined tickets, and when each was declined.
    declined: Mutex<BTreeMap<String, u64>>,
}

impl MemoryTicketStore {
    pub fn new() -> MemoryTicketStore {
        MemoryTicketStore::default()
    }
}

impl TicketStore for MemoryTicketStore {
    fn insert(&self, rec: &TicketRecord) -> Result<(), StoreFault> {
        self.rows
            .lock()
            .expect("ticket store")
            .insert(rec.hash.clone(), (rec.clone(), false));
        Ok(())
    }

    fn take(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault> {
        let mut rows = self.rows.lock().expect("ticket store");
        match rows.get_mut(hash) {
            Some((rec, redeemed)) if !*redeemed && rec.expires_at > now && want.met_by(rec) => {
                *redeemed = true;
                Ok(Some(rec.clone()))
            }
            _ => Ok(None),
        }
    }

    fn give_back(&self, hash: &str, _spent_at: u64) -> Result<bool, StoreFault> {
        if self
            .declined
            .lock()
            .expect("ticket store")
            .contains_key(hash)
        {
            return Ok(false);
        }
        let mut rows = self.rows.lock().expect("ticket store");
        Ok(match rows.get_mut(hash) {
            Some((_, redeemed)) if *redeemed => {
                *redeemed = false;
                true
            }
            _ => false,
        })
    }

    fn peek(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault> {
        let rows = self.rows.lock().expect("ticket store");
        Ok(match rows.get(hash) {
            Some((rec, redeemed)) if !*redeemed && rec.expires_at > now && want.met_by(rec) => {
                Some(rec.clone())
            }
            _ => None,
        })
    }

    fn list_open(
        &self,
        space: &str,
        purpose: Purpose,
        now: u64,
    ) -> Result<Vec<TicketRecord>, StoreFault> {
        let rows = self.rows.lock().expect("ticket store");
        let mut out: Vec<TicketRecord> = rows
            .values()
            .filter(|(rec, redeemed)| {
                !*redeemed
                    && rec.expires_at > now
                    && rec.purpose == purpose
                    && rec.space.as_deref() == Some(space)
            })
            .map(|(rec, _)| rec.clone())
            .collect();
        out.sort_by_key(|r| r.issued_at);
        Ok(out)
    }

    fn decline(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault> {
        let out = self.take(hash, now, want)?;
        if out.is_some() {
            self.declined
                .lock()
                .expect("ticket store")
                .insert(hash.to_string(), now);
        }
        Ok(out)
    }

    fn list_declined(
        &self,
        space: &str,
        purpose: Purpose,
        now: u64,
    ) -> Result<Vec<(TicketRecord, u64)>, StoreFault> {
        let rows = self.rows.lock().expect("ticket store");
        let declined = self.declined.lock().expect("ticket store");
        let mut out: Vec<(TicketRecord, u64)> = declined
            .iter()
            .filter_map(|(hash, at)| {
                let (rec, _) = rows.get(hash)?;
                (rec.expires_at > now
                    && rec.purpose == purpose
                    && rec.space.as_deref() == Some(space))
                .then(|| (rec.clone(), *at))
            })
            .collect();
        out.sort_by_key(|(r, at)| (*at, r.issued_at));
        Ok(out)
    }

    fn revoke_open(&self, space: &str, purpose: Purpose, id: &str) -> Result<u64, StoreFault> {
        let mut rows = self.rows.lock().expect("ticket store");
        let before = rows.len();
        rows.retain(|hash, (rec, redeemed)| {
            *redeemed
                || rec.purpose != purpose
                || rec.space.as_deref() != Some(space)
                || !hash.starts_with(id)
        });
        Ok((before - rows.len()) as u64)
    }

    fn sweep(&self, now: u64) -> Result<u64, StoreFault> {
        let mut rows = self.rows.lock().expect("ticket store");
        let mut unused = 0;
        rows.retain(|_, (rec, redeemed)| {
            let dead = rec.expires_at <= now;
            if dead && !*redeemed {
                unused += 1;
            }
            !dead
        });
        // A decline is kept as long as its ticket is, and no longer.
        self.declined
            .lock()
            .expect("ticket store")
            .retain(|hash, _| rows.contains_key(hash));
        Ok(unused)
    }
}

/// The ticket table. `hash` is the key, so a redemption is a lookup by
/// something the holder proves they know and nobody can list.
pub const TICKET_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$ticket" (
  "hash"        text   NOT NULL,
  "purpose"     text   NOT NULL,
  "principal"   text   NOT NULL DEFAULT '',
  "display"     text   NOT NULL DEFAULT '',
  "space"       text   NOT NULL DEFAULT '',
  "role"        text   NOT NULL DEFAULT '',
  "issued_by"   text   NOT NULL DEFAULT '',
  "issued_at"   bigint NOT NULL DEFAULT 0,
  "expires_at"  bigint NOT NULL,
  "redeemed_at" bigint,
  "payload"     text   NOT NULL DEFAULT '',
  "declined_at" bigint,
  PRIMARY KEY ("hash")
)"#;

/// A table created before `payload` existed gains the column. Nothing has
/// deployed, so this is for a development database that outlived a build,
/// not a migration anybody is owed.
const TICKET_PAYLOAD_DDL: &str =
    r#"ALTER TABLE "$ticket" ADD COLUMN IF NOT EXISTS "payload" text NOT NULL DEFAULT ''"#;

/// A table created before a decline was kept gains the column, for
/// [`TICKET_PAYLOAD_DDL`]'s reason.
const TICKET_DECLINED_DDL: &str =
    r#"ALTER TABLE "$ticket" ADD COLUMN IF NOT EXISTS "declined_at" bigint"#;

/// Create `$ticket`, retry-once for the reason the other platform tables
/// do.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(TICKET_DDL).await.is_err() {
        pool.rows(TICKET_DDL).await?;
    }
    if pool.rows(TICKET_PAYLOAD_DDL).await.is_err() {
        pool.rows(TICKET_PAYLOAD_DDL).await?;
    }
    if pool.rows(TICKET_DECLINED_DDL).await.is_err() {
        pool.rows(TICKET_DECLINED_DDL).await?;
    }
    Ok(())
}

/// The Postgres ticket store.
pub struct PgTicketStore {
    db: PgSync,
}

impl PgTicketStore {
    /// Dial a connection of its own. Refuses when `$ticket` is absent.
    pub fn open(cfg: PoolConfig) -> Result<PgTicketStore, StoreFault> {
        PgTicketStore::start(Source::Dial(cfg))
    }

    /// On a connection somebody else owns. Never the request pool.
    pub fn open_on(pool: Pool) -> Result<PgTicketStore, StoreFault> {
        PgTicketStore::start(Source::Shared(pool))
    }

    fn start(source: Source) -> Result<PgTicketStore, StoreFault> {
        let db = PgSync::start(source, "ticket")?;
        db.sql(r#"SELECT 1 FROM "$ticket" LIMIT 1"#)?;
        Ok(PgTicketStore { db })
    }
}

fn opt(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

impl TicketStore for PgTicketStore {
    fn insert(&self, rec: &TicketRecord) -> Result<(), StoreFault> {
        self.db.sql(&format!(
            r#"INSERT INTO "$ticket"
               ("hash","purpose","principal","display","space","role","issued_by","issued_at","expires_at","payload")
               VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {})"#,
            sql_lit(&rec.hash),
            sql_lit(rec.purpose.as_str()),
            sql_lit(rec.principal.as_ref().map(|p| p.0.as_str()).unwrap_or("")),
            sql_lit(&rec.display),
            sql_lit(rec.space.as_deref().unwrap_or("")),
            sql_lit(rec.role.as_deref().unwrap_or("")),
            sql_lit(&rec.issued_by),
            rec.issued_at,
            rec.expires_at,
            sql_lit(rec.payload.as_deref().unwrap_or("")),
        ))?;
        Ok(())
    }

    fn give_back(&self, hash: &str, spent_at: u64) -> Result<bool, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"UPDATE "$ticket" SET "redeemed_at" = NULL
               WHERE "hash" = {} AND "redeemed_at" = {spent_at} AND "declined_at" IS NULL
               RETURNING 1"#,
            sql_lit(hash),
        ))?;
        Ok(!rows.is_empty())
    }

    fn take(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault> {
        let mut also = String::new();
        if let Some(p) = want.purpose {
            also.push_str(&format!(r#" AND "purpose" = {}"#, sql_lit(p.as_str())));
        }
        if let Some(sp) = want.space {
            also.push_str(&format!(r#" AND "space" = {}"#, sql_lit(sp)));
        }
        if let Some(h) = want.holder {
            also.push_str(&format!(
                r#" AND ("principal" = '' OR "principal" = {})"#,
                sql_lit(h)
            ));
        }
        let rows = self.db.sql(&format!(
            r#"UPDATE "$ticket" SET "redeemed_at" = {now}
               WHERE "hash" = {} AND "redeemed_at" IS NULL AND "expires_at" > {now}{also}
               RETURNING "purpose","principal","display","space","role","issued_by","issued_at","expires_at","payload""#,
            sql_lit(hash),
        ))?;
        let Some(r) = rows.first() else {
            return Ok(None);
        };
        let Some(purpose) = cell(r, 0).and_then(|p| Purpose::parse(&p)) else {
            return Err("the ticket table holds a purpose this build does not recognise".into());
        };
        let num = |i: usize| cell(r, i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        Ok(Some(TicketRecord {
            hash: hash.to_string(),
            purpose,
            principal: opt(cell(r, 1)).map(PrincipalId),
            display: cell(r, 2).unwrap_or_default(),
            space: opt(cell(r, 3)),
            role: opt(cell(r, 4)),
            issued_by: cell(r, 5).unwrap_or_default(),
            issued_at: num(6),
            expires_at: num(7),
            payload: opt(cell(r, 8)),
        }))
    }

    fn peek(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault> {
        let mut also = String::new();
        if let Some(p) = want.purpose {
            also.push_str(&format!(r#" AND "purpose" = {}"#, sql_lit(p.as_str())));
        }
        if let Some(sp) = want.space {
            also.push_str(&format!(r#" AND "space" = {}"#, sql_lit(sp)));
        }
        if let Some(h) = want.holder {
            also.push_str(&format!(
                r#" AND ("principal" = '' OR "principal" = {})"#,
                sql_lit(h)
            ));
        }
        let rows = self.db.sql(&format!(
            r#"SELECT "purpose","principal","display","space","role","issued_by","issued_at","expires_at","payload"
               FROM "$ticket"
               WHERE "hash" = {} AND "redeemed_at" IS NULL AND "expires_at" > {now}{also}"#,
            sql_lit(hash),
        ))?;
        let Some(r) = rows.first() else {
            return Ok(None);
        };
        let Some(purpose) = cell(r, 0).and_then(|p| Purpose::parse(&p)) else {
            return Err("the ticket table holds a purpose this build does not recognise".into());
        };
        let num = |i: usize| cell(r, i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        Ok(Some(TicketRecord {
            hash: hash.to_string(),
            purpose,
            principal: opt(cell(r, 1)).map(PrincipalId),
            display: cell(r, 2).unwrap_or_default(),
            space: opt(cell(r, 3)),
            role: opt(cell(r, 4)),
            issued_by: cell(r, 5).unwrap_or_default(),
            issued_at: num(6),
            expires_at: num(7),
            payload: opt(cell(r, 8)),
        }))
    }

    fn list_open(
        &self,
        space: &str,
        purpose: Purpose,
        now: u64,
    ) -> Result<Vec<TicketRecord>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT "hash","principal","display","space","role","issued_by","issued_at","expires_at","payload"
               FROM "$ticket"
               WHERE "space" = {} AND "purpose" = {} AND "redeemed_at" IS NULL AND "expires_at" > {now}
               ORDER BY "issued_at", "hash""#,
            sql_lit(space),
            sql_lit(purpose.as_str()),
        ))?;
        Ok(rows
            .iter()
            .map(|r| {
                let num = |i: usize| cell(r, i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                TicketRecord {
                    hash: cell(r, 0).unwrap_or_default(),
                    purpose,
                    principal: opt(cell(r, 1)).map(PrincipalId),
                    display: cell(r, 2).unwrap_or_default(),
                    space: opt(cell(r, 3)),
                    role: opt(cell(r, 4)),
                    issued_by: cell(r, 5).unwrap_or_default(),
                    issued_at: num(6),
                    expires_at: num(7),
                    payload: opt(cell(r, 8)),
                }
            })
            .collect())
    }

    fn decline(
        &self,
        hash: &str,
        now: u64,
        want: &Want<'_>,
    ) -> Result<Option<TicketRecord>, StoreFault> {
        let mut also = String::new();
        if let Some(p) = want.purpose {
            also.push_str(&format!(r#" AND "purpose" = {}"#, sql_lit(p.as_str())));
        }
        if let Some(sp) = want.space {
            also.push_str(&format!(r#" AND "space" = {}"#, sql_lit(sp)));
        }
        if let Some(h) = want.holder {
            also.push_str(&format!(
                r#" AND ("principal" = '' OR "principal" = {})"#,
                sql_lit(h)
            ));
        }
        let rows = self.db.sql(&format!(
            r#"UPDATE "$ticket" SET "redeemed_at" = {now}, "declined_at" = {now}
               WHERE "hash" = {} AND "redeemed_at" IS NULL AND "expires_at" > {now}{also}
               RETURNING "purpose","principal","display","space","role","issued_by","issued_at","expires_at","payload""#,
            sql_lit(hash),
        ))?;
        let Some(r) = rows.first() else {
            return Ok(None);
        };
        let Some(purpose) = cell(r, 0).and_then(|p| Purpose::parse(&p)) else {
            return Err("the ticket table holds a purpose this build does not recognise".into());
        };
        let num = |i: usize| cell(r, i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        Ok(Some(TicketRecord {
            hash: hash.to_string(),
            purpose,
            principal: opt(cell(r, 1)).map(PrincipalId),
            display: cell(r, 2).unwrap_or_default(),
            space: opt(cell(r, 3)),
            role: opt(cell(r, 4)),
            issued_by: cell(r, 5).unwrap_or_default(),
            issued_at: num(6),
            expires_at: num(7),
            payload: opt(cell(r, 8)),
        }))
    }

    fn list_declined(
        &self,
        space: &str,
        purpose: Purpose,
        now: u64,
    ) -> Result<Vec<(TicketRecord, u64)>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT "hash","principal","display","space","role","issued_by","issued_at","expires_at","payload","declined_at"
               FROM "$ticket"
               WHERE "space" = {} AND "purpose" = {} AND "declined_at" IS NOT NULL
                 AND "expires_at" > {now}
               ORDER BY "declined_at", "issued_at", "hash""#,
            sql_lit(space),
            sql_lit(purpose.as_str()),
        ))?;
        Ok(rows
            .iter()
            .map(|r| {
                let num = |i: usize| cell(r, i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                (
                    TicketRecord {
                        hash: cell(r, 0).unwrap_or_default(),
                        purpose,
                        principal: opt(cell(r, 1)).map(PrincipalId),
                        display: cell(r, 2).unwrap_or_default(),
                        space: opt(cell(r, 3)),
                        role: opt(cell(r, 4)),
                        issued_by: cell(r, 5).unwrap_or_default(),
                        issued_at: num(6),
                        expires_at: num(7),
                        payload: opt(cell(r, 8)),
                    },
                    num(9),
                )
            })
            .collect())
    }

    fn revoke_open(&self, space: &str, purpose: Purpose, id: &str) -> Result<u64, StoreFault> {
        // `id` is hex, checked by `Tickets::revoke_open`, so the pattern
        // has no wildcard of its own.
        let rows = self.db.sql(&format!(
            r#"DELETE FROM "$ticket"
               WHERE "space" = {} AND "purpose" = {} AND "redeemed_at" IS NULL
                 AND "hash" LIKE {}
               RETURNING "hash""#,
            sql_lit(space),
            sql_lit(purpose.as_str()),
            sql_lit(&format!("{id}%")),
        ))?;
        Ok(rows.len() as u64)
    }

    fn sweep(&self, now: u64) -> Result<u64, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"WITH d AS (DELETE FROM "$ticket" WHERE "expires_at" <= {now} RETURNING "redeemed_at")
               SELECT COUNT(*) FROM d WHERE "redeemed_at" IS NULL"#
        ))?;
        Ok(rows
            .first()
            .and_then(|r| cell(r, 0))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }
}
