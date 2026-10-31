//! Sign-in with an emailed code (`plan-auth-federated.md` §7, step 5).
//!
//! A person types their address, Ordo mails them a code, and they type the
//! code into the page that asked. It is the method `email` in
//! `ORDO_AUTH_METHODS`, and it needs the mail channel (`ORDO_MAIL_KIND`):
//! Ordo's own mail setting sends the codes when the app uses built-in sign-in
//! (owner, 2026-10-08).
//!
//! # The three moments
//!
//! 1. **`begin`** (`POST /auth/begin {method: "email", identifier}`):
//!    normalise the address, file a ceremony, queue one message, answer
//!    `{challenge, binder}`. Nothing is looked up, so the answer and the work
//!    are the same for every address (E11). The code does not exist yet.
//! 2. **The `mail.send` worker** calls [`EmailCodeProvider`]'s
//!    [`Composer::compose`]. That is where the deployment decides whether a
//!    message is owed (§7.5), applies the limits (E7 to E10), and only then
//!    mints the code, keeps its MAC and writes the message. A code is never
//!    in the outbox and never in a response.
//! 3. **`complete`** (`kind: "email_code"`, with the challenge, the binder and
//!    the code): one attempt is counted, the code is compared in constant
//!    time, and a match consumes the ceremony. Wrong, expired, consumed and
//!    another browser's ceremony are the one answer (E12).
//!
//! # What is stored
//!
//! The ceremony row in `$authceremony` (connection `email`) holds the MAC of
//! the address, the MAC of the code, the attempt count, and the address
//! **sealed under a key derived from the challenge**. The challenge itself is
//! never stored, only its SHA-256 (the handle), so a reader of the table can
//! neither read the address nor use a code. `complete` presents the
//! challenge, which opens the address.
//!
//! The counters are rows in `$authrate`, keyed by MACs of the address being
//! mailed, never by a client's network address (owner, 2026-10-06).
//!
//! # Delivery is at least once, and that is safe
//!
//! A crash between the relay's acceptance and the outbox delete sends the row
//! again. The second compose mints a **new** code and replaces the first, and
//! keeps the attempt count, so a duplicate delivery gives no extra guesses and
//! the ceremony still signs in at most once. A ceremony is limited once,
//! however many times its row is composed.
//!
//! # Every number is a setting
//!
//! [`EmailCodePolicy`] holds them, each with the plan's default.

use std::sync::{Arc, Mutex};

use ordo_exec::{sql_lit, Pool, PoolError, TextRow};
use ordo_identity::directory::{Directory, Provisioning};
use ordo_identity::provider::{
    AuthProvider, BeginRequest, ChallengeOrRedirect, CompleteRequest, EmailEvidence,
    SubjectAttributes, VerifiedEmail,
};
use ordo_identity::{
    AuthError, AuthMethodKind, AuthenticatedSubject, AuthenticatorId, CredentialStoreError,
    DirectoryError, DirectoryStore, Timestamp,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mail::{Composer, MailAddress, MailError, MailJob, MailQueue, Message, QueuedMail};
use crate::passkeys::{Source, SyncPg};

/// The method's name in `ORDO_AUTH_METHODS`, and the ceremony rows'
/// `connection`.
pub const METHOD: &str = "email";

/// The word an outbox enqueue refused for a full pool carries. The door
/// answers it as `server_busy`; any other outbox failure stays
/// `mail-outbox-unavailable`.
pub(crate) const MAIL_OUTBOX_BUSY: &str = "mail-outbox-busy";

/// The outbox kind its messages are queued under.
pub const MAIL_KIND: &str = "auth.email-code";

/// The one phase an email ceremony has. Its code is absent until the worker
/// sends the message.
const PHASE: &str = "code";

/// The subject line. No name, no tenant, no account state (§7.1).
pub const SUBJECT: &str = "Your sign-in code";

/// The subject of a code that confirms an address a signed-in person is
/// adding to their account ([`crate::contact_email`]).
pub const CONFIRM_SUBJECT: &str = "Confirm your email address";

const RATE: &str = "$authrate";

// ───────────────────────────── settings ───────────────────────────────

/// Code length, digits.
pub const DIGITS_ENV: &str = "ORDO_AUTH_EMAIL_CODE_DIGITS";
/// How long a ceremony and its code live, from `begin`.
pub const LIFE_ENV: &str = "ORDO_AUTH_EMAIL_CODE_SECS";
/// Wrong codes allowed per ceremony.
pub const ATTEMPTS_ENV: &str = "ORDO_AUTH_EMAIL_CODE_ATTEMPTS";
/// Messages an hour to one address (E7).
pub const PER_HOUR_ENV: &str = "ORDO_AUTH_EMAIL_PER_HOUR";
/// Messages a day to one address (E7).
pub const PER_DAY_ENV: &str = "ORDO_AUTH_EMAIL_PER_DAY";
/// Messages an hour on a returning device's own lane (E8). 0 turns the lane
/// off.
pub const DEVICE_PER_HOUR_ENV: &str = "ORDO_AUTH_EMAIL_DEVICE_PER_HOUR";
/// How long a device marker is honoured, in days (E8).
pub const DEVICE_DAYS_ENV: &str = "ORDO_AUTH_EMAIL_DEVICE_DAYS";
/// Sign-in messages an hour for the whole deployment (E9).
pub const DEPLOYMENT_PER_HOUR_ENV: &str = "ORDO_AUTH_EMAIL_DEPLOYMENT_PER_HOUR";

/// The app developer's numbers. Each is a setting with the plan's default
/// (§7.4; owner, 2026-09-29: *"we do not enforce an app developer's choices
/// but we can have it as default"*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailCodePolicy {
    /// Digits in a code. Default 8, from 6 to 10 (E1).
    pub digits: u32,
    /// Seconds a ceremony lives from `begin`. Default 600, from 120 to 900
    /// (E2).
    pub life_secs: u64,
    /// Wrong codes before the ceremony ends. Default 5, from 1 to 10 (E3).
    pub attempts: u32,
    /// Messages an hour to one address. Default 5 (E7).
    pub per_hour: u64,
    /// Messages a day to one address. Default 15 (E7).
    pub per_day: u64,
    /// Messages an hour on a returning device's lane. Default 3; 0 is no
    /// lane (E8).
    pub device_per_hour: u64,
    /// Days a device marker is honoured. Default 30 (E8).
    pub device_days: u64,
    /// Messages an hour for the deployment. Default 1000 (E9).
    pub deployment_per_hour: u64,
}

impl Default for EmailCodePolicy {
    fn default() -> EmailCodePolicy {
        EmailCodePolicy {
            digits: 8,
            life_secs: 600,
            attempts: 5,
            per_hour: 5,
            per_day: 15,
            device_per_hour: 3,
            device_days: 30,
            deployment_per_hour: 1000,
        }
    }
}

impl EmailCodePolicy {
    /// Read the settings through `var`. Unset is the default. A value
    /// outside its range refuses the boot, naming the setting: it is not
    /// clamped.
    pub fn from_vars(
        var: &dyn Fn(&str) -> Option<String>,
    ) -> Result<EmailCodePolicy, (&'static str, String)> {
        let ranged = |k: &'static str, default: u64, lo: u64, hi: u64| match var(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            None => Ok(default),
            Some(v) => match v.parse::<u64>() {
                Ok(n) if (lo..=hi).contains(&n) => Ok(n),
                _ => Err((
                    k,
                    format!("`{v}` is not a whole number from {lo} to {hi} (default {default})"),
                )),
            },
        };
        let d = EmailCodePolicy::default();
        let p = EmailCodePolicy {
            digits: ranged(DIGITS_ENV, d.digits.into(), 6, 10)? as u32,
            life_secs: ranged(LIFE_ENV, d.life_secs, 120, 900)?,
            attempts: ranged(ATTEMPTS_ENV, d.attempts.into(), 1, 10)? as u32,
            per_hour: ranged(PER_HOUR_ENV, d.per_hour, 1, 1_000)?,
            per_day: ranged(PER_DAY_ENV, d.per_day, 1, 10_000)?,
            device_per_hour: ranged(DEVICE_PER_HOUR_ENV, d.device_per_hour, 0, 1_000)?,
            device_days: ranged(DEVICE_DAYS_ENV, d.device_days, 1, 400)?,
            deployment_per_hour: ranged(
                DEPLOYMENT_PER_HOUR_ENV,
                d.deployment_per_hour,
                1,
                10_000_000,
            )?,
        };
        if p.per_day < p.per_hour {
            return Err((
                PER_DAY_ENV,
                format!(
                    "{} a day is fewer than {} an hour, so the hourly limit could never be reached",
                    p.per_day, p.per_hour
                ),
            ));
        }
        Ok(p)
    }

    /// One line for the startup log.
    pub fn describe(&self) -> String {
        format!(
            "email codes: {} digits, {}s, {} attempts; {} an hour and {} a day per address, \
             {} an hour per returning device, {} an hour for the deployment",
            self.digits,
            self.life_secs,
            self.attempts,
            self.per_hour,
            self.per_day,
            self.device_per_hour,
            self.deployment_per_hour
        )
    }
}

/// Normalise an address as §7.3 says: trim, lowercase the whole address,
/// exactly one `@`, an ASCII domain. Dots and `+tags` are left alone. The
/// result must also pass the mail channel's strict grammar, so whatever is
/// accepted here can be written into a header exactly (E13).
pub fn normalise(address: &str) -> Option<String> {
    let a = address.trim().to_ascii_lowercase();
    if a.matches('@').count() != 1 || !a.is_ascii() {
        return None;
    }
    MailAddress::parse(&a).ok().map(|_| a)
}

// ───────────────────────────── keys ───────────────────────────────────

/// `k_otp`, derived from the deployment's token secret, and everything made
/// with it. No `Debug`.
struct Keys {
    otp: [u8; 32],
}

impl Keys {
    fn from_secret(secret: &[u8]) -> Keys {
        Keys {
            otp: ordo_crypto::hmac_sha256(secret, b"ordo/auth/email-otp/v1"),
        }
    }

    fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut msg = Vec::new();
        for p in parts {
            msg.extend_from_slice(&(p.len() as u32).to_be_bytes());
            msg.extend_from_slice(p);
        }
        ordo_crypto::hmac_sha256(&self.otp, &msg)
    }

    /// `HMAC(k_otp, address)`: what the ceremony and the counters are keyed
    /// by.
    fn address(&self, address: &str) -> String {
        ordo_crypto::hex(&self.mac(&[b"address", address.as_bytes()]))
    }

    /// `HMAC(k_otp, handle ‖ code)` (E1).
    fn code(&self, handle: &str, code: &str) -> String {
        ordo_crypto::hex(&self.mac(&[b"code", handle.as_bytes(), code.as_bytes()]))
    }

    /// The key a ceremony's address is sealed under: only the holder of the
    /// challenge can derive it.
    fn seal(&self, challenge: &str) -> [u8; 32] {
        self.mac(&[b"seal", challenge.as_bytes()])
    }

    /// A device marker as stored: bound to the address it was given for.
    fn device(&self, address_mac: &str, marker: &str) -> String {
        ordo_crypto::hex(&self.mac(&[b"device", address_mac.as_bytes(), marker.as_bytes()]))
    }
}

fn sha256_hex(s: &str) -> String {
    ordo_crypto::hex(&Sha256::digest(s.as_bytes()))
}

fn random_hex() -> String {
    let mut b = [0u8; 32];
    ordo_crypto::fill_random(&mut b);
    ordo_crypto::hex(&b)
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// `digits` decimal digits from the OS generator, by rejection sampling, so
/// every code is equally likely (E1).
pub fn mint_code(digits: u32) -> String {
    let modulus = 10u64.pow(digits);
    // The largest multiple of `modulus` a u64 holds; a draw at or above it
    // is thrown away so the remainder has no bias.
    let zone = u64::MAX - (u64::MAX % modulus);
    loop {
        let mut b = [0u8; 8];
        ordo_crypto::fill_random(&mut b);
        let n = u64::from_be_bytes(b);
        if n < zone {
            return format!("{:0width$}", n % modulus, width = digits as usize);
        }
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ───────────────────────────── the store ──────────────────────────────

/// One open email ceremony, as its row's payload holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// `HMAC(k_otp, address)`.
    pub address: String,
    /// The address, sealed under the challenge's key. Hex.
    pub sealed: String,
    /// The returning-device marker `begin` presented, as stored, if any.
    pub device: Option<String>,
    /// `HMAC(k_otp, handle ‖ code)`, once the worker has sent a code.
    pub code: Option<String>,
    /// Attempts made at `complete`.
    pub tries: u32,
    /// Whether this ceremony's message was counted against the limits.
    pub counted: bool,
    /// How many messages this ceremony has sent.
    pub sends: u32,
    /// The wall clock at `begin`, in nanoseconds: what orders two `begin`s
    /// for one address inside one second (E5).
    pub order: u64,
    /// `begin` carried an invitation a newcomer may sign up with. Under
    /// invite-only, the worker mails an unknown address only when this is
    /// set (owner ruling 2026-10-09, federated ask 9). `complete` checks
    /// and spends the invitation again; this only decides the message.
    pub invited: bool,
    /// The signed-in person adding this address to their account
    /// ([`crate::contact_email`]). `None` is a sign-in. A ceremony for a
    /// person never signs anybody in, and a sign-in ceremony never confirms
    /// an address.
    pub principal: Option<String>,
}

impl Pending {
    fn to_json(&self) -> String {
        json!({
            "a": self.address, "s": self.sealed, "d": self.device, "c": self.code,
            "t": self.tries, "k": self.counted, "n": self.sends, "o": self.order,
            "i": self.invited, "p": self.principal,
        })
        .to_string()
    }

    fn from_json(s: &str) -> Option<Pending> {
        let v: Value = serde_json::from_str(s).ok()?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Pending {
            address: text("a")?,
            sealed: text("s")?,
            device: text("d"),
            code: text("c"),
            tries: v.get("t").and_then(Value::as_u64).unwrap_or(0) as u32,
            counted: v.get("k").and_then(Value::as_bool).unwrap_or(false),
            sends: v.get("n").and_then(Value::as_u64).unwrap_or(0) as u32,
            order: v.get("o").and_then(Value::as_u64).unwrap_or(0),
            invited: v.get("i").and_then(Value::as_bool).unwrap_or(false),
            principal: text("p"),
        })
    }
}

/// The store could not answer. Names a table, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailStoreError(pub ordo_identity::StoreFault);

/// Where email ceremonies and their counters live.
///
/// Every operation that decides something is one statement, so two replicas
/// cannot both win it.
pub trait EmailCodeStore: Send + Sync {
    /// File a new ceremony.
    fn open(
        &self,
        handle: &str,
        binder: &str,
        pending: &Pending,
        now: u64,
        expires_at: u64,
    ) -> Result<(), EmailStoreError>;
    /// The ceremony, if it is open at `now`.
    fn load(&self, handle: &str, now: u64) -> Result<Option<Pending>, EmailStoreError>;
    /// Give the ceremony a new code MAC, mark it counted, add one send, and
    /// **remove every older email ceremony for the same address** (E5).
    /// `false`, and nothing changed, when the ceremony is gone or a newer one
    /// for the same address exists: a slow message for an old `begin` never
    /// ends the ceremony the page is now waiting on. Older and newer are by
    /// `(minted_at, order, handle)`, so two `begin`s in one second still
    /// order.
    fn arm(&self, handle: &str, code: &str, now: u64) -> Result<bool, EmailStoreError>;
    /// Count one attempt on the ceremony with this handle **and this
    /// binder**, open at `now`, and answer it as it now is, with the exact
    /// payload text, so a success can consume that version and no other.
    fn attempt(
        &self,
        handle: &str,
        binder: &str,
        now: u64,
    ) -> Result<Option<(Pending, String)>, EmailStoreError>;
    /// Delete the ceremony if its payload is still `version`. One winner.
    fn consume(&self, handle: &str, version: &str) -> Result<bool, EmailStoreError>;
    /// Delete the ceremony.
    fn discard(&self, handle: &str) -> Result<(), EmailStoreError>;
    /// Add one to `key` if it is below `limit`. `true` when counted.
    fn count(&self, key: &str, limit: u64, expires_at: u64) -> Result<bool, EmailStoreError>;
    /// Take back one count from `key`.
    fn uncount(&self, key: &str) -> Result<(), EmailStoreError>;
    /// Keep `key` until `expires_at` (a device marker).
    fn remember(&self, key: &str, expires_at: u64) -> Result<(), EmailStoreError>;
    /// Is `key` kept at `now`?
    fn known(&self, key: &str, now: u64) -> Result<bool, EmailStoreError>;
    /// Delete counters and markers that expired. Ceremonies are swept with
    /// the ceremony table.
    fn sweep(&self, now: u64) -> Result<u64, EmailStoreError>;
}

/// `$authrate`: the email counters and device markers.
pub const RATE_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$authrate" (
  "key"         text   NOT NULL,
  "n"           bigint NOT NULL,
  "expires_at"  bigint NOT NULL,
  PRIMARY KEY ("key")
)"#;

/// Create `$authrate`. The ceremony table is `ceremonies::ensure_table`'s.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    if pool.rows(RATE_DDL).await.is_err() {
        pool.rows(RATE_DDL).await?;
    }
    Ok(())
}

/// Process-local. **Tests only.**
#[derive(Default)]
pub struct MemoryEmailCodeStore {
    /// handle -> (binder, payload, minted_at, expires_at)
    ceremonies: Mutex<std::collections::BTreeMap<String, (String, String, u64, u64)>>,
    rates: Mutex<std::collections::BTreeMap<String, (u64, u64)>>,
}

impl MemoryEmailCodeStore {
    pub fn new() -> MemoryEmailCodeStore {
        MemoryEmailCodeStore::default()
    }

    /// Open ceremonies. Test observation.
    pub fn open_ceremonies(&self) -> usize {
        self.ceremonies.lock().expect("email store").len()
    }
}

impl EmailCodeStore for MemoryEmailCodeStore {
    fn open(
        &self,
        handle: &str,
        binder: &str,
        pending: &Pending,
        now: u64,
        expires_at: u64,
    ) -> Result<(), EmailStoreError> {
        self.ceremonies.lock().expect("email store").insert(
            handle.to_string(),
            (binder.to_string(), pending.to_json(), now, expires_at),
        );
        Ok(())
    }

    fn load(&self, handle: &str, now: u64) -> Result<Option<Pending>, EmailStoreError> {
        Ok(self
            .ceremonies
            .lock()
            .expect("email store")
            .get(handle)
            .filter(|(_, _, _, exp)| *exp > now)
            .and_then(|(_, p, _, _)| Pending::from_json(p)))
    }

    fn arm(&self, handle: &str, code: &str, now: u64) -> Result<bool, EmailStoreError> {
        let mut rows = self.ceremonies.lock().expect("email store");
        let Some((_, payload, minted, exp)) = rows.get(handle).cloned() else {
            return Ok(false);
        };
        if exp <= now {
            return Ok(false);
        }
        let Some(mut p) = Pending::from_json(&payload) else {
            return Ok(false);
        };
        let me = (minted, p.order, handle.to_string());
        let key = |h: &str, pl: &str, m: u64| {
            Pending::from_json(pl)
                .filter(|o| o.address == p.address)
                .map(|o| (m, o.order, h.to_string()))
        };
        let newer = rows
            .iter()
            .any(|(h, (_, pl, m, _))| key(h, pl, *m).is_some_and(|k| k > me));
        if newer {
            return Ok(false);
        }
        p.code = Some(code.to_string());
        p.counted = true;
        p.sends += 1;
        rows.retain(|h, (_, pl, m, _)| !key(h, pl, *m).is_some_and(|k| k < me));
        if let Some(row) = rows.get_mut(handle) {
            row.1 = p.to_json();
        }
        Ok(true)
    }

    fn attempt(
        &self,
        handle: &str,
        binder: &str,
        now: u64,
    ) -> Result<Option<(Pending, String)>, EmailStoreError> {
        let mut rows = self.ceremonies.lock().expect("email store");
        let Some((b, payload, _, exp)) = rows.get_mut(handle) else {
            return Ok(None);
        };
        if b != binder || *exp <= now {
            return Ok(None);
        }
        let Some(mut p) = Pending::from_json(payload) else {
            return Ok(None);
        };
        p.tries += 1;
        *payload = p.to_json();
        Ok(Some((p, payload.clone())))
    }

    fn consume(&self, handle: &str, version: &str) -> Result<bool, EmailStoreError> {
        let mut rows = self.ceremonies.lock().expect("email store");
        if rows.get(handle).is_some_and(|(_, p, _, _)| p == version) {
            rows.remove(handle);
            return Ok(true);
        }
        Ok(false)
    }

    fn discard(&self, handle: &str) -> Result<(), EmailStoreError> {
        self.ceremonies.lock().expect("email store").remove(handle);
        Ok(())
    }

    fn count(&self, key: &str, limit: u64, expires_at: u64) -> Result<bool, EmailStoreError> {
        let mut rates = self.rates.lock().expect("email store");
        let e = rates.entry(key.to_string()).or_insert((0, expires_at));
        if e.0 >= limit {
            return Ok(false);
        }
        e.0 += 1;
        Ok(true)
    }

    fn uncount(&self, key: &str) -> Result<(), EmailStoreError> {
        if let Some(e) = self.rates.lock().expect("email store").get_mut(key) {
            e.0 = e.0.saturating_sub(1);
        }
        Ok(())
    }

    fn remember(&self, key: &str, expires_at: u64) -> Result<(), EmailStoreError> {
        self.rates
            .lock()
            .expect("email store")
            .insert(key.to_string(), (0, expires_at));
        Ok(())
    }

    fn known(&self, key: &str, now: u64) -> Result<bool, EmailStoreError> {
        Ok(self
            .rates
            .lock()
            .expect("email store")
            .get(key)
            .is_some_and(|(_, exp)| *exp > now))
    }

    fn sweep(&self, now: u64) -> Result<u64, EmailStoreError> {
        let mut rates = self.rates.lock().expect("email store");
        let before = rates.len();
        rates.retain(|_, (_, exp)| *exp > now);
        Ok((before - rates.len()) as u64)
    }
}

/// The Postgres store: ceremonies in `$authceremony`, counters in
/// `$authrate`.
pub struct PgEmailCodeStore {
    db: SyncPg,
}

impl PgEmailCodeStore {
    /// On a connection the deployment already holds. Refuses when either
    /// table does not exist.
    pub fn open_on(pool: Pool) -> Result<PgEmailCodeStore, EmailStoreError> {
        let db = SyncPg::start(Source::Shared(pool), "ordo-email-codes")
            .map_err(|e| EmailStoreError(e.into()))?;
        let s = PgEmailCodeStore { db };
        s.sql(r#"SELECT 1 FROM "$authceremony" LIMIT 1"#)?;
        s.sql(r#"SELECT 1 FROM "$authrate" LIMIT 1"#)?;
        Ok(s)
    }

    fn sql(&self, sql: &str) -> Result<Vec<TextRow>, EmailStoreError> {
        self.db.sql(sql).map_err(|e| {
            let table = if sql.contains(RATE) {
                RATE
            } else {
                "$authceremony"
            };
            EmailStoreError(crate::passkeys::scrub_fault(table, e))
        })
    }

    fn first(rows: &[TextRow]) -> Option<String> {
        rows.first().and_then(|r| r.first()).and_then(|c| c.clone())
    }
}

/// The payload as jsonb, for the statements that change one field of it.
const J: &str = r#"("payload")::jsonb"#;

impl EmailCodeStore for PgEmailCodeStore {
    fn open(
        &self,
        handle: &str,
        binder: &str,
        pending: &Pending,
        now: u64,
        expires_at: u64,
    ) -> Result<(), EmailStoreError> {
        self.sql(&format!(
            r#"INSERT INTO "$authceremony"
                 ("handle", "connection", "phase", "binder", "payload", "minted_at", "expires_at")
               VALUES ({}, {}, {}, {}, {}, {now}, {expires_at})"#,
            sql_lit(handle),
            sql_lit(METHOD),
            sql_lit(PHASE),
            sql_lit(binder),
            sql_lit(&pending.to_json()),
        ))?;
        Ok(())
    }

    fn load(&self, handle: &str, now: u64) -> Result<Option<Pending>, EmailStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "payload" FROM "$authceremony"
               WHERE "handle" = {} AND "connection" = {} AND "phase" = {} AND "expires_at" > {now}"#,
            sql_lit(handle),
            sql_lit(METHOD),
            sql_lit(PHASE),
        ))?;
        Ok(Self::first(&rows).and_then(|p| Pending::from_json(&p)))
    }

    fn arm(&self, handle: &str, code: &str, now: u64) -> Result<bool, EmailStoreError> {
        let rows = self.sql(&format!(
            r#"WITH me AS (
                 SELECT "minted_at", COALESCE(({J}->>'o')::bigint, 0) AS o, "handle",
                        {J}->>'a' AS a FROM "$authceremony"
                 WHERE "handle" = {h} AND "connection" = {m} AND "phase" = {p}
                   AND "expires_at" > {now}
               ), newer AS (
                 SELECT 1 FROM "$authceremony" o, me
                 WHERE o."connection" = {m} AND (o."payload")::jsonb->>'a' = me.a
                   AND (o."minted_at", COALESCE(((o."payload")::jsonb->>'o')::bigint, 0), o."handle")
                     > (me."minted_at", me.o, me."handle")
               ), armed AS (
                 UPDATE "$authceremony" SET "payload" = ({J} || jsonb_build_object(
                     'c', {code}::text, 'k', true, 'n', COALESCE(({J}->>'n')::int, 0) + 1))::text
                 WHERE "handle" = {h} AND "connection" = {m} AND "phase" = {p}
                   AND "expires_at" > {now} AND NOT EXISTS (SELECT 1 FROM newer)
                 RETURNING 1
               ), gone AS (
                 DELETE FROM "$authceremony" o USING me
                 WHERE o."connection" = {m} AND (o."payload")::jsonb->>'a' = me.a
                   AND (o."minted_at", COALESCE(((o."payload")::jsonb->>'o')::bigint, 0), o."handle")
                     < (me."minted_at", me.o, me."handle")
                   AND EXISTS (SELECT 1 FROM armed)
                 RETURNING 1
               ) SELECT count(*) FROM armed"#,
            code = sql_lit(code),
            h = sql_lit(handle),
            m = sql_lit(METHOD),
            p = sql_lit(PHASE),
        ))?;
        Ok(Self::first(&rows).as_deref() == Some("1"))
    }

    fn attempt(
        &self,
        handle: &str,
        binder: &str,
        now: u64,
    ) -> Result<Option<(Pending, String)>, EmailStoreError> {
        let rows = self.sql(&format!(
            r#"UPDATE "$authceremony" SET "payload" = ({J} || jsonb_build_object(
                   't', COALESCE(({J}->>'t')::int, 0) + 1))::text
               WHERE "handle" = {} AND "connection" = {} AND "phase" = {} AND "binder" = {}
                 AND "expires_at" > {now}
               RETURNING "payload""#,
            sql_lit(handle),
            sql_lit(METHOD),
            sql_lit(PHASE),
            sql_lit(binder),
        ))?;
        Ok(Self::first(&rows).and_then(|v| Pending::from_json(&v).map(|p| (p, v))))
    }

    fn consume(&self, handle: &str, version: &str) -> Result<bool, EmailStoreError> {
        let rows = self.sql(&format!(
            r#"DELETE FROM "$authceremony"
               WHERE "handle" = {} AND "connection" = {} AND "payload" = {}
               RETURNING 1"#,
            sql_lit(handle),
            sql_lit(METHOD),
            sql_lit(version),
        ))?;
        Ok(!rows.is_empty())
    }

    fn discard(&self, handle: &str) -> Result<(), EmailStoreError> {
        self.sql(&format!(
            r#"DELETE FROM "$authceremony" WHERE "handle" = {} AND "connection" = {}"#,
            sql_lit(handle),
            sql_lit(METHOD),
        ))?;
        Ok(())
    }

    fn count(&self, key: &str, limit: u64, expires_at: u64) -> Result<bool, EmailStoreError> {
        let rows = self.sql(&format!(
            r#"INSERT INTO "$authrate" ("key", "n", "expires_at") VALUES ({k}, 1, {expires_at})
               ON CONFLICT ("key") DO UPDATE SET "n" = "$authrate"."n" + 1
                 WHERE "$authrate"."n" < {limit}
               RETURNING "n""#,
            k = sql_lit(key),
        ))?;
        Ok(!rows.is_empty())
    }

    fn uncount(&self, key: &str) -> Result<(), EmailStoreError> {
        self.sql(&format!(
            r#"UPDATE "$authrate" SET "n" = "n" - 1 WHERE "key" = {} AND "n" > 0"#,
            sql_lit(key)
        ))?;
        Ok(())
    }

    fn remember(&self, key: &str, expires_at: u64) -> Result<(), EmailStoreError> {
        self.sql(&format!(
            r#"INSERT INTO "$authrate" ("key", "n", "expires_at") VALUES ({k}, 0, {expires_at})
               ON CONFLICT ("key") DO UPDATE SET "expires_at" = EXCLUDED."expires_at""#,
            k = sql_lit(key),
        ))?;
        Ok(())
    }

    fn known(&self, key: &str, now: u64) -> Result<bool, EmailStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT 1 FROM "$authrate" WHERE "key" = {} AND "expires_at" > {now}"#,
            sql_lit(key)
        ))?;
        Ok(!rows.is_empty())
    }

    fn sweep(&self, now: u64) -> Result<u64, EmailStoreError> {
        let rows = self.sql(&format!(
            r#"WITH gone AS (DELETE FROM "$authrate" WHERE "expires_at" <= {now} RETURNING 1)
               SELECT count(*) FROM gone"#
        ))?;
        Ok(Self::first(&rows).and_then(|c| c.parse().ok()).unwrap_or(0))
    }
}

// ───────────────────────────── the provider ───────────────────────────

/// What `begin` hands the page. The binder never travels in a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Begun {
    pub challenge: String,
    pub binder: String,
}

/// The emailed-code method: `begin`, the composer the mail worker calls, and
/// `complete`.
pub struct EmailCodeProvider {
    keys: Keys,
    policy: EmailCodePolicy,
    store: Arc<dyn EmailCodeStore>,
    queue: Arc<dyn MailQueue>,
    directory: Arc<dyn DirectoryStore>,
    provisioning: Provisioning,
    recorder: Option<Arc<ordo_secevent::Recorder>>,
}

impl EmailCodeProvider {
    /// `secret` is the deployment's token secret; `k_otp` is derived from
    /// it. `directory` and `provisioning` are the deployment's own, so the
    /// worker decides whether a message is owed as a sign-in would.
    pub fn new(
        secret: &[u8],
        policy: EmailCodePolicy,
        store: Arc<dyn EmailCodeStore>,
        queue: Arc<dyn MailQueue>,
        directory: Arc<dyn DirectoryStore>,
        provisioning: Provisioning,
    ) -> EmailCodeProvider {
        EmailCodeProvider {
            keys: Keys::from_secret(secret),
            policy,
            store,
            queue,
            directory,
            provisioning,
            recorder: None,
        }
    }

    /// Record why a message was not sent in the security stream.
    pub fn with_recorder(mut self, recorder: Arc<ordo_secevent::Recorder>) -> EmailCodeProvider {
        self.recorder = Some(recorder);
        self
    }

    pub fn policy(&self) -> &EmailCodePolicy {
        &self.policy
    }

    /// The store, for the sweep.
    pub fn store(&self) -> Arc<dyn EmailCodeStore> {
        self.store.clone()
    }

    /// Start a ceremony for `identifier`. Constant work whatever the address
    /// (E11): normalise, two MACs, a seal, one insert, one enqueue. Whether
    /// the address is known is first looked at by the worker.
    ///
    /// `device` is a marker an earlier sign-in from this browser was given
    /// (E8). It is filed with the ceremony and checked by the worker.
    pub fn start(
        &self,
        identifier: &str,
        device: Option<&str>,
        now: u64,
    ) -> Result<Begun, AuthError> {
        self.start_invited(identifier, device, false, now)
    }

    /// [`EmailCodeProvider::start`], noting whether `begin` carried a valid
    /// invitation ([`Pending::invited`]). The work and the answer are the
    /// same either way.
    pub fn start_invited(
        &self,
        identifier: &str,
        device: Option<&str>,
        invited: bool,
        now: u64,
    ) -> Result<Begun, AuthError> {
        self.open_ceremony(identifier, device, invited, None, now)
    }

    /// Start confirming `identifier` as `principal`'s own address
    /// ([`crate::contact_email`]). The same ceremony, limits and settings as
    /// a sign-in. The message says it confirms an address, and only
    /// [`EmailCodeProvider::complete_contact`] for the same person accepts
    /// its code.
    pub fn start_contact(
        &self,
        principal: &str,
        identifier: &str,
        now: u64,
    ) -> Result<Begun, AuthError> {
        self.open_ceremony(identifier, None, false, Some(principal.to_string()), now)
    }

    fn open_ceremony(
        &self,
        identifier: &str,
        device: Option<&str>,
        invited: bool,
        principal: Option<String>,
        now: u64,
    ) -> Result<Begun, AuthError> {
        let address = normalise(identifier).ok_or(AuthError::BadCredentials)?;
        let to = MailAddress::parse(&address).map_err(|_| AuthError::BadCredentials)?;
        let challenge = random_hex();
        let binder = random_hex();
        let handle = sha256_hex(&challenge);
        let address_mac = self.keys.address(&address);
        let device = device
            .filter(|m| m.len() == 64 && m.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(|m| self.keys.device(&address_mac, m));
        let sealed = ordo_crypto::seal_with(
            &self.keys.seal(&challenge),
            0,
            "$authceremony",
            "email-address",
            address.as_bytes(),
        );
        let pending = Pending {
            address: address_mac,
            sealed: ordo_crypto::hex(&sealed),
            device,
            code: None,
            tries: 0,
            counted: false,
            sends: 0,
            order: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
            invited,
            principal,
        };
        self.store
            .open(
                &handle,
                &sha256_hex(&binder),
                &pending,
                now,
                now + self.policy.life_secs,
            )
            .map_err(|e| AuthError::StoreUnavailable(CredentialStoreError(e.0)))?;
        let job = MailJob::new(MAIL_KIND, to, &handle).map_err(|_| AuthError::BadCredentials)?;
        self.queue.enqueue(&job, now).map_err(|e| match e {
            e if e.class().is_busy() => AuthError::UpstreamUnavailable(MAIL_OUTBOX_BUSY),
            _ => AuthError::UpstreamUnavailable("mail-outbox-unavailable"),
        })?;
        Ok(Begun { challenge, binder })
    }

    /// Give the browser that just signed in with `address` a marker for the
    /// returning-device lane (E8). `None` when the lane is off or the marker
    /// could not be kept; the sign-in stands either way.
    pub fn remember_device(&self, address: &str, now: u64) -> Option<String> {
        if self.policy.device_per_hour == 0 {
            return None;
        }
        let marker = random_hex();
        let key = self.keys.device(&self.keys.address(address), &marker);
        self.store
            .remember(
                &format!("email.device:{key}"),
                now + self.policy.device_days * 86_400,
            )
            .ok()
            .map(|_| marker)
    }

    fn record(&self, reason: &str, now: u64) {
        let Some(recorder) = &self.recorder else {
            return;
        };
        recorder.observe(
            ordo_secevent::Observation {
                space: None,
                subject: ordo_secevent::Subject::Unverified,
                op: "auth.email.send".to_string(),
                kind: ordo_secevent::EventKind::AuthenticationFailed {
                    reason: reason.to_string(),
                },
                at: ordo_commandlog::Timestamp(now),
                correlation: ordo_secevent::correlation::mint(),
            },
            ordo_secevent::Relationship::None,
        );
    }

    /// Count this ceremony's message against its limits, or say which one
    /// stopped it. A refusal takes back what it counted.
    fn allowance(&self, p: &Pending, now: u64) -> Result<Result<(), &'static str>, MailError> {
        let unavailable = |e: EmailStoreError| MailError::unavailable(e.0.to_string());
        let hour = now / 3600;
        let day = now / 86_400;
        let hour_end = (hour + 1) * 3600;
        let day_end = (day + 1) * 86_400;
        let mut lanes: Vec<(String, u64, u64, &'static str)> = Vec::new();
        let returning = match (&p.device, self.policy.device_per_hour) {
            (Some(d), n) if n > 0 => self
                .store
                .known(&format!("email.device:{d}"), now)
                .map_err(unavailable)?,
            _ => false,
        };
        if returning {
            let d = p.device.as_deref().unwrap_or_default();
            lanes.push((
                format!("email.lane:{d}:{hour}"),
                self.policy.device_per_hour,
                hour_end,
                "email-limit-device",
            ));
        } else {
            lanes.push((
                format!("email.to.h:{}:{hour}", p.address),
                self.policy.per_hour,
                hour_end,
                "email-limit-recipient",
            ));
            lanes.push((
                format!("email.to.d:{}:{day}", p.address),
                self.policy.per_day,
                day_end,
                "email-limit-recipient",
            ));
        }
        lanes.push((
            format!("email.all.h:{hour}"),
            self.policy.deployment_per_hour,
            hour_end,
            "email-limit-deployment",
        ));
        let mut taken: Vec<String> = Vec::new();
        for (key, limit, until, reason) in lanes {
            if self.store.count(&key, limit, until).map_err(unavailable)? {
                taken.push(key);
            } else {
                for k in &taken {
                    let _ = self.store.uncount(k);
                }
                return Ok(Err(reason));
            }
        }
        Ok(Ok(()))
    }

    /// The message, with the one substitution (E13).
    fn message(&self, to: MailAddress, code: &str) -> Result<Message, MailError> {
        let minutes = self.policy.life_secs.div_ceil(60);
        let text = format!(
            "Your sign-in code is {code}.\n\
             \n\
             Type it on the page where you asked for it. It works once, for {minutes} minutes.\n\
             \n\
             If you did not ask to sign in, you can ignore this message. The code is no use\n\
             without the browser that asked for it.\n"
        );
        Message::new(to, SUBJECT, &text)
    }

    /// The message that confirms an address a person is adding.
    fn confirm_message(&self, to: MailAddress, code: &str) -> Result<Message, MailError> {
        let minutes = self.policy.life_secs.div_ceil(60);
        let text = format!(
            "Your code to confirm this address is {code}.\n\
             \n\
             Type it on the page where you added this address. It works once, for {minutes} minutes.\n\
             \n\
             If you did not add this address to an account, you can ignore this message.\n\
             Nothing changes until the code is typed.\n"
        );
        Message::new(to, CONFIRM_SUBJECT, &text)
    }

    /// Check one code against the open ceremony and consume it on a match.
    /// `principal` is whose ceremony it must be: `None` for a sign-in, the
    /// person for a confirmation. Answers the address the ceremony was for.
    fn check_code(
        &self,
        challenge: &ordo_identity::ChallengeId,
        binder: &str,
        code: &str,
        principal: Option<&str>,
        now: u64,
    ) -> Result<String, AuthError> {
        let unavailable =
            |e: EmailStoreError| AuthError::StoreUnavailable(CredentialStoreError(e.0));
        let handle = sha256_hex(&challenge.0);
        let Some((p, version)) = self
            .store
            .attempt(&handle, &sha256_hex(binder), now)
            .map_err(unavailable)?
        else {
            return Err(AuthError::UnknownChallenge(challenge.clone()));
        };
        // A sign-in ceremony never confirms an address, and a confirmation
        // never signs anybody in. The attempt still counts.
        if p.principal.as_deref() != principal {
            return Err(AuthError::BadCredentials);
        }
        // E3: the last allowed failure ends the ceremony.
        let wrong = |p: &Pending| {
            if p.tries >= self.policy.attempts {
                let _ = self.store.discard(&handle);
            }
            Err(AuthError::BadCredentials)
        };
        if p.tries > self.policy.attempts {
            let _ = self.store.discard(&handle);
            return Err(AuthError::BadCredentials);
        }
        let Some(stored) = p.code.as_deref() else {
            return wrong(&p);
        };
        let given = code.trim();
        let shaped =
            given.len() == self.policy.digits as usize && given.bytes().all(|b| b.is_ascii_digit());
        // The MAC is computed whatever the shape, so a malformed code takes
        // as long as a wrong one.
        let mac = self.keys.code(&handle, given);
        if !(ct_eq(mac.as_bytes(), stored.as_bytes()) && shaped) {
            return wrong(&p);
        }
        if !self.store.consume(&handle, &version).map_err(unavailable)? {
            return Err(AuthError::UnknownChallenge(challenge.clone()));
        }
        from_hex(&p.sealed)
            .and_then(|s| {
                ordo_crypto::open_with(
                    &self.keys.seal(&challenge.0),
                    "$authceremony",
                    "email-address",
                    &s,
                )
                .ok()
            })
            .and_then(|b| String::from_utf8(b).ok())
            .filter(|a| self.keys.address(a) == p.address)
            .ok_or(AuthError::BadCredentials)
    }

    /// Finish confirming an address `principal` is adding. Answers the
    /// address. Wrong, expired, used, another browser's and another
    /// person's codes are the one refusal, as at sign-in.
    pub fn complete_contact(
        &self,
        principal: &str,
        challenge: &str,
        binder: &str,
        code: &str,
        now: u64,
    ) -> Result<String, AuthError> {
        self.check_code(
            &ordo_identity::ChallengeId(challenge.to_string()),
            binder,
            code,
            Some(principal),
            now,
        )
    }
}

impl Composer for EmailCodeProvider {
    /// Decide whether a message is owed (§7.5), apply the limits, mint the
    /// code, and write the message. Every "no" is `Ok(None)`: the row goes,
    /// nothing is sent, and the caller of `begin` was told the same either
    /// way (E10).
    fn compose(&self, job: &QueuedMail, now: u64) -> Result<Option<Message>, MailError> {
        let unavailable = |e: EmailStoreError| MailError::unavailable(e.0.to_string());
        let handle = job.payload.as_str();
        let Some(p) = self.store.load(handle, now).map_err(unavailable)? else {
            // Expired, consumed, or replaced by a newer code (E5).
            return Ok(None);
        };
        let address = job.to.as_str();
        if self.keys.address(address) != p.address {
            return Ok(None);
        }
        // A signed-in person confirming an address: nobody is signed in or
        // onboarded by it, so the directory is not asked. The limits are
        // the same.
        if p.principal.is_some() {
            if !p.counted {
                if let Err(reason) = self.allowance(&p, now)? {
                    self.record(reason, now);
                    let _ = self.store.discard(handle);
                    return Ok(None);
                }
            }
            let code = mint_code(self.policy.digits);
            if !self
                .store
                .arm(handle, &self.keys.code(handle, &code), now)
                .map_err(unavailable)?
            {
                return Ok(None);
            }
            return self.confirm_message(job.to.clone(), &code).map(Some);
        }
        let dir = Directory::with_store(self.provisioning, self.directory.clone());
        let id = AuthenticatorId(format!("{METHOD}:{address}"));
        match dir.principal_for(&id) {
            Ok(Some(principal)) => match dir.refuse_deleted(&principal) {
                Ok(()) => {}
                // D4, E16: a deleted account is mailed nothing.
                Err(DirectoryError::Deleted(_)) => {
                    self.record("identity-deleted", now);
                    let _ = self.store.discard(handle);
                    return Ok(None);
                }
                Err(_) => return Err(MailError::unavailable("directory")),
            },
            // §7.5 row 3: nobody could be onboarded from this address, so
            // a code could only be refused. Nothing is sent. A `begin` that
            // carried a valid invitation is the exception: the code is how
            // the invited newcomer signs up.
            Ok(None) if dir.provisioning() == Provisioning::InviteOnly && !p.invited => {
                self.record("email-not-admitted", now);
                let _ = self.store.discard(handle);
                return Ok(None);
            }
            Ok(None) => {}
            Err(_) => return Err(MailError::unavailable("directory")),
        }
        if !p.counted {
            if let Err(reason) = self.allowance(&p, now)? {
                self.record(reason, now);
                let _ = self.store.discard(handle);
                return Ok(None);
            }
        }
        let code = mint_code(self.policy.digits);
        if !self
            .store
            .arm(handle, &self.keys.code(handle, &code), now)
            .map_err(unavailable)?
        {
            return Ok(None);
        }
        self.message(job.to.clone(), &code).map(Some)
    }
}

impl AuthProvider for EmailCodeProvider {
    fn kind(&self) -> AuthMethodKind {
        AuthMethodKind::EmailOtp
    }

    /// The registry's `begin` has no way to hand back a binder, so
    /// `POST /auth/begin` calls [`EmailCodeProvider::start`] directly, as it
    /// does the passkey provider. This answers the one refusal.
    fn begin(
        &self,
        _req: &BeginRequest,
        _now: Timestamp,
    ) -> Result<ChallengeOrRedirect, AuthError> {
        Err(AuthError::MethodMismatch {
            provider: AuthMethodKind::EmailOtp,
        })
    }

    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        let CompleteRequest::EmailCode {
            challenge,
            binder,
            code,
        } = resp
        else {
            return Err(AuthError::MethodMismatch {
                provider: AuthMethodKind::EmailOtp,
            });
        };
        let address = self.check_code(challenge, binder, code, None, now.0)?;
        let email = VerifiedEmail::new(&address, EmailEvidence::EmailedCode);
        Ok(AuthenticatedSubject::without_presence(
            AuthenticatorId(format!("{METHOD}:{address}")),
            AuthMethodKind::EmailOtp,
        )
        .with_attributes(SubjectAttributes {
            email,
            ..SubjectAttributes::default()
        }))
    }
}

/// The provider as the registry holds it: the same `Arc` `begin` and the
/// mail worker hold.
pub struct Registered(pub Arc<EmailCodeProvider>);

impl AuthProvider for Registered {
    fn kind(&self) -> AuthMethodKind {
        self.0.kind()
    }

    fn begin(&self, req: &BeginRequest, now: Timestamp) -> Result<ChallengeOrRedirect, AuthError> {
        self.0.begin(req, now)
    }

    fn complete(
        &self,
        resp: &CompleteRequest,
        now: Timestamp,
    ) -> Result<AuthenticatedSubject, AuthError> {
        self.0.complete(resp, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_has_the_configured_digits_and_no_other_character() {
        for digits in [6, 8, 10] {
            for _ in 0..200 {
                let c = mint_code(digits);
                assert_eq!(c.len(), digits as usize);
                assert!(c.bytes().all(|b| b.is_ascii_digit()), "{c}");
            }
        }
    }

    #[test]
    fn normalising_lowercases_and_trims_and_folds_nothing_else() {
        assert_eq!(
            normalise("  Kim.Lee+news@Example.ORG ").as_deref(),
            Some("kim.lee+news@example.org")
        );
        for bad in [
            "kim",
            "kim@@example.org",
            "a@b@example.org",
            "kim@example.org\r\nBcc: x@y.z",
            "kïm@example.org",
            "",
        ] {
            assert_eq!(normalise(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_defaults_are_the_plans_and_a_value_out_of_range_names_its_setting() {
        let none = |_: &str| None;
        assert_eq!(
            EmailCodePolicy::from_vars(&none).unwrap(),
            EmailCodePolicy::default()
        );
        let d = EmailCodePolicy::default();
        assert_eq!(
            (
                d.digits,
                d.life_secs,
                d.attempts,
                d.per_hour,
                d.per_day,
                d.device_per_hour,
                d.deployment_per_hour
            ),
            (8, 600, 5, 5, 15, 3, 1000)
        );
        for (k, v) in [
            (DIGITS_ENV, "5"),
            (LIFE_ENV, "60"),
            (LIFE_ENV, "901"),
            (ATTEMPTS_ENV, "0"),
            (PER_HOUR_ENV, "x"),
            (DEPLOYMENT_PER_HOUR_ENV, "0"),
        ] {
            let var = |name: &str| (name == k).then(|| v.to_string());
            assert_eq!(EmailCodePolicy::from_vars(&var).err().unwrap().0, k);
        }
        let var = |name: &str| match name {
            PER_HOUR_ENV => Some("20".to_string()),
            PER_DAY_ENV => Some("10".to_string()),
            _ => None,
        };
        assert_eq!(
            EmailCodePolicy::from_vars(&var).err().unwrap().0,
            PER_DAY_ENV
        );
    }

    #[test]
    fn a_pending_row_round_trips() {
        let p = Pending {
            address: "a".into(),
            sealed: "s".into(),
            device: None,
            code: Some("c".into()),
            tries: 2,
            counted: true,
            sends: 1,
            order: 7,
            invited: true,
            principal: None,
        };
        assert_eq!(Pending::from_json(&p.to_json()), Some(p.clone()));
        let mine = Pending {
            principal: Some("p_kim".into()),
            ..p
        };
        assert_eq!(Pending::from_json(&mine.to_json()), Some(mine));
    }
}
