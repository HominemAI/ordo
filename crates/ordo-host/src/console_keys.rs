//! The console key (`plan-console.md` 8.2, G14): how one console signs in
//! to this deployment as its owner, with no browser and no cookie.
//!
//! # What a console key is
//!
//! An Ed25519 public key, stored here as a credential of the deployment
//! owner who enrolled it, of kind `console`. The private half never leaves
//! the console, which keeps it sealed in its own database.
//!
//! - **Enrolled** with `std.deployment.addConsoleKey($publicKey, $name)`, a
//!   write that always needs the owner's own passkey touch (step-up).
//!   `ORDO_STEP_UP_EXEMPT_OPS` can never exempt it or `revokeConsoleKey`
//!   ([`NEVER_EXEMPT`]).
//! - **Used**: `POST /auth/begin {method: "console", key}` answers a
//!   challenge that stores nothing: it carries its own expiry and a MAC
//!   under a key derived from the token secret. `POST /auth/complete
//!   {method: "console", key,
//!   challenge, signature}` answers a session. The session lasts
//!   `ORDO_AUTH_CONSOLE_SESSION_SECS` (default 900) and is renewed only by
//!   signing again: `/auth/refresh` refuses it, so a revoked key stops at
//!   its next sign-in at the latest. A challenge is spent once its
//!   signature verifies, so only a live key's holder writes anything.
//!   Sign-in attempts are throttled per node ([`MAX_ATTEMPTS_PER_WINDOW`]).
//! - **Revoked** with `std.deployment.revokeConsoleKey($id)`. Every session
//!   the key signed in that has not yet expired is ended at once.
//! - **Listed** with `std.deployment.consoleKeys`: each key, its name, its
//!   owner and its last use.
//!
//! # What it can never do
//!
//! A console key session carries **no presence**. It is minted here with
//! `user_presence_at: None` and nothing else mints it, so it never
//! satisfies step-up. A write that needs a touch needs the owner's own
//! passkey, relayed through the console page (`plan-console.md` 8.3, G15):
//! `/auth/step-up` with the console session's bearer and a passkey
//! assertion of the same principal, made against a challenge bound to that
//! one session.
//!
//! # Where a console session may go
//!
//! A console key session reaches the door's ops in the deployment's own
//! space and, among the `/auth/*` routes, only [`AUTH_ROUTES_OPEN`]: the
//! relayed touch and its own logout. In a tenant's space it reaches only
//! [`TENANT_OPS_OPEN`], and only with the owner's touch in the last
//! [`TENANT_REACH_SECS`]. [`console_may`] is the one check;
//! the `/auth/*` routes call it from one layer (`auth::console::gate`) and
//! the door from `IdentityAccess::check`.
//!
//! # One client
//!
//! The keys are reached through [`ConsoleKeys`] and its store,
//! [`ConsoleKeyStore`], whose every failure is the stores' one typed fault
//! ([`StoreFault`]). The routes and the ops never touch the table.

use std::sync::{Arc, Mutex};

use ordo_api::{
    BoundParams, ErrorCode, ExecError, ExecResult, FieldSpec, NotSyncable, OpCost, OpKind, OpSpec,
    ParamSpec, ParamTy, RefineTier, Syncability,
};
use ordo_exec::{sql_lit, Pool};
use ordo_identity::webauthn::{b64url_decode, b64url_encode};
use ordo_identity::{PrincipalId, StoreFault, Timestamp, TokenClaims};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// `std.deployment.addConsoleKey($publicKey, $name)`.
pub const ADD: &str = "std.deployment.addConsoleKey";
/// `std.deployment.consoleKeys`.
pub const LIST: &str = "std.deployment.consoleKeys";
/// `std.deployment.revokeConsoleKey($id)`.
pub const REVOKE: &str = "std.deployment.revokeConsoleKey";
/// Every op this module serves.
pub const OPS: [&str; 3] = [ADD, LIST, REVOKE];
/// The ops `ORDO_STEP_UP_EXEMPT_OPS` can never exempt: who holds a console
/// key is decided with the owner's own touch, always.
pub const NEVER_EXEMPT: [&str; 2] = [ADD, REVOKE];

/// The `/auth/*` routes a console key session may call: the relayed touch
/// (G15) and its own logout. Every other `/auth/*` route refuses it.
pub const AUTH_ROUTES_OPEN: [&str; 3] = ["/auth/step-up/begin", "/auth/step-up", "/auth/logout"];

/// Console sign-in attempts (begins and completes) one node takes in a
/// window. A console signs in about once in 15 minutes.
pub const MAX_ATTEMPTS_PER_WINDOW: u32 = 30;
/// The throttle's window.
pub const ATTEMPT_WINDOW_SECS: u64 = 60;

/// The method word at `/auth/begin` and `/auth/complete`.
pub const METHOD: &str = "console";
/// What a console key session's `via` starts with.
pub const VIA_PREFIX: &str = "console:";

/// How long a console key session lasts.
pub const SESSION_SECS_ENV: &str = "ORDO_AUTH_CONSOLE_SESSION_SECS";
/// 15 minutes (`plan-console.md` 8.2).
pub const DEFAULT_SESSION_SECS: u64 = 15 * 60;
/// The shortest a deployment owner may set.
pub const MIN_SESSION_SECS: u64 = 60;
/// The longest: an hour, the length of an ordinary token.
pub const MAX_SESSION_SECS: u64 = 60 * 60;

/// How long a sign-in challenge lives. A console answers in milliseconds.
pub const CHALLENGE_SECS: u64 = 60;
/// Live keys one owner may hold: the key in use and the one replacing it
/// during a rotation.
pub const MAX_LIVE_PER_OWNER: usize = 2;
/// The longest name a key may carry.
pub const MAX_NAME: usize = 64;

/// What a console signs: this prefix, then the challenge's bytes. The
/// prefix keeps a console key's signature from meaning anything anywhere
/// else.
pub const SIGNED_PREFIX: &[u8] = b"ordo-console-key/v1\0";

/// What a challenge's MAC is over, before the key id, so it means nothing
/// anywhere else.
const CHALLENGE_DOMAIN: &[u8] = b"ordo-console-challenge/v1\0";
/// A challenge: 16 random bytes, the expiry (8 bytes, big-endian), the MAC.
const CHALLENGE_LEN: usize = 16 + 8 + 32;

/// The session length from the environment. Outside the range is a boot
/// refusal naming the setting and the range.
pub fn session_secs_from(get: &dyn Fn(&str) -> Option<String>) -> Result<u64, String> {
    let Some(text) = get(SESSION_SECS_ENV) else {
        return Ok(DEFAULT_SESSION_SECS);
    };
    match text.trim().parse::<u64>() {
        Ok(n) if (MIN_SESSION_SECS..=MAX_SESSION_SECS).contains(&n) => Ok(n),
        _ => Err(format!(
            "is `{text}`; set a whole number of seconds from {MIN_SESSION_SECS} to \
             {MAX_SESSION_SECS}, or unset it for {DEFAULT_SESSION_SECS}"
        )),
    }
}

/// One stored key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRow {
    pub id: String,
    pub principal: String,
    pub name: String,
    /// The 32-byte Ed25519 public key, base64url.
    pub public_key: String,
    pub created_at: u64,
    pub last_used_at: Option<u64>,
    pub revoked_at: Option<u64>,
    pub revoked_by: Option<String>,
}

impl KeyRow {
    pub fn live(&self) -> bool {
        self.revoked_at.is_none()
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "principal": self.principal,
            "name": self.name,
            "publicKey": self.public_key,
            "createdAt": self.created_at,
            "lastUsedAt": self.last_used_at,
            "revokedAt": self.revoked_at,
            "revokedBy": self.revoked_by,
        })
    }
}

const KEY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$consolekey" (
  "id"           text   PRIMARY KEY,
  "principal"    text   NOT NULL,
  "name"         text   NOT NULL,
  "public_key"   text   NOT NULL UNIQUE,
  "created_at"   bigint NOT NULL,
  "last_used_at" bigint,
  "revoked_at"   bigint,
  "revoked_by"   text
)"#;

const SESSION_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$consolekeysession" (
  "key_id"     text   NOT NULL,
  "session"    text   NOT NULL,
  "expires_at" bigint NOT NULL,
  PRIMARY KEY ("key_id", "session")
)"#;

const SPENT_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$consolekeyspent" (
  "handle"     text   PRIMARY KEY,
  "expires_at" bigint NOT NULL
)"#;

const COLUMNS: &str =
    r#""id","principal","name","public_key","created_at","last_used_at","revoked_at","revoked_by""#;

/// Runs in the memory store as a sign-in's session row is written. A test
/// uses it to land a revocation between the live check and the write.
pub type SignInHook = Box<dyn Fn(&mut Vec<KeyRow>) + Send + Sync>;

#[derive(Default)]
struct Memory {
    keys: Vec<KeyRow>,
    sessions: Vec<(String, String, u64)>,
    spent: Vec<(String, u64)>,
    on_sign_in: Option<SignInHook>,
}

enum Backend {
    Pg(Pool),
    Memory(Mutex<Memory>),
}

/// Where console keys are kept: the control database, `$consolekey`, and
/// the sessions each signed in, `$consolekeysession`, so a revocation can
/// end them.
#[derive(Clone)]
pub struct ConsoleKeyStore(Arc<Backend>);

fn fault(e: ordo_exec::PoolError) -> StoreFault {
    crate::auth::store_failure::pool_fault(e, "the console keys could not be read or written")
}

fn row_of(r: &ordo_exec::TextRow) -> Option<KeyRow> {
    let text = |i: usize| r.get(i).and_then(|c| c.clone());
    let num = |i: usize| text(i).and_then(|v| v.parse::<u64>().ok());
    Some(KeyRow {
        id: text(0)?,
        principal: text(1)?,
        name: text(2)?,
        public_key: text(3)?,
        created_at: num(4)?,
        last_used_at: num(5),
        revoked_at: num(6),
        revoked_by: text(7),
    })
}

impl ConsoleKeyStore {
    /// Over the control database.
    pub fn pg(pool: Pool) -> ConsoleKeyStore {
        ConsoleKeyStore(Arc::new(Backend::Pg(pool)))
    }

    /// In memory. For tests, and for nothing a deployment runs.
    pub fn memory() -> ConsoleKeyStore {
        ConsoleKeyStore(Arc::new(Backend::Memory(Mutex::default())))
    }

    /// In memory, with `hook` run as each sign-in's session row is written.
    /// For the test of a revocation racing a sign-in.
    pub fn memory_with_sign_in_hook(hook: SignInHook) -> ConsoleKeyStore {
        ConsoleKeyStore(Arc::new(Backend::Memory(Mutex::new(Memory {
            on_sign_in: Some(hook),
            ..Memory::default()
        }))))
    }

    fn mem(&self) -> Option<std::sync::MutexGuard<'_, Memory>> {
        match self.0.as_ref() {
            Backend::Memory(m) => Some(m.lock().expect("console key memory")),
            Backend::Pg(_) => None,
        }
    }

    /// Make the tables if they are missing.
    pub async fn ensure_tables(&self) -> Result<(), StoreFault> {
        if let Backend::Pg(pool) = self.0.as_ref() {
            for ddl in [KEY_DDL, SESSION_DDL, SPENT_DDL] {
                pool.rows(ddl).await.map_err(fault)?;
            }
        }
        Ok(())
    }

    /// Store a new key, unless its owner already holds
    /// [`MAX_LIVE_PER_OWNER`] live keys or the public key is already
    /// stored. `false` is "not stored", decided in the one statement.
    pub async fn insert(&self, row: &KeyRow) -> Result<bool, StoreFault> {
        if let Some(mut m) = self.mem() {
            let live = m
                .keys
                .iter()
                .filter(|k| k.principal == row.principal && k.live())
                .count();
            if live >= MAX_LIVE_PER_OWNER || m.keys.iter().any(|k| k.public_key == row.public_key) {
                return Ok(false);
            }
            m.keys.push(row.clone());
            return Ok(true);
        }
        let Backend::Pg(pool) = self.0.as_ref() else {
            unreachable!("memory handled above")
        };
        let rows = pool
            .rows(&format!(
                r#"INSERT INTO "$consolekey" ("id","principal","name","public_key","created_at")
                   SELECT {id}, {p}, {n}, {k}, {at}
                   WHERE (SELECT count(*) FROM "$consolekey"
                          WHERE "principal" = {p} AND "revoked_at" IS NULL) < {max}
                   ON CONFLICT DO NOTHING RETURNING "id""#,
                id = sql_lit(&row.id),
                p = sql_lit(&row.principal),
                n = sql_lit(&row.name),
                k = sql_lit(&row.public_key),
                at = row.created_at,
                max = MAX_LIVE_PER_OWNER,
            ))
            .await
            .map_err(fault)?;
        Ok(!rows.is_empty())
    }

    /// One key, live or not.
    pub async fn get(&self, id: &str) -> Result<Option<KeyRow>, StoreFault> {
        if let Some(m) = self.mem() {
            return Ok(m.keys.iter().find(|k| k.id == id).cloned());
        }
        let Backend::Pg(pool) = self.0.as_ref() else {
            unreachable!("memory handled above")
        };
        let rows = pool
            .rows(&format!(
                r#"SELECT {COLUMNS} FROM "$consolekey" WHERE "id" = {}"#,
                sql_lit(id)
            ))
            .await
            .map_err(fault)?;
        Ok(rows.first().and_then(row_of))
    }

    /// Every key, oldest first.
    pub async fn list(&self) -> Result<Vec<KeyRow>, StoreFault> {
        if let Some(m) = self.mem() {
            return Ok(m.keys.clone());
        }
        let Backend::Pg(pool) = self.0.as_ref() else {
            unreachable!("memory handled above")
        };
        let rows = pool
            .rows(&format!(
                r#"SELECT {COLUMNS} FROM "$consolekey" ORDER BY "created_at", "id""#
            ))
            .await
            .map_err(fault)?;
        Ok(rows.iter().filter_map(row_of).collect())
    }

    /// Revoke a live key. Answers the key as it now stands, or `None` when
    /// there is no live key with that id.
    pub async fn revoke(&self, id: &str, by: &str, at: u64) -> Result<Option<KeyRow>, StoreFault> {
        if let Some(mut m) = self.mem() {
            let Some(k) = m.keys.iter_mut().find(|k| k.id == id && k.live()) else {
                return Ok(None);
            };
            k.revoked_at = Some(at);
            k.revoked_by = Some(by.to_string());
            return Ok(Some(k.clone()));
        }
        let Backend::Pg(pool) = self.0.as_ref() else {
            unreachable!("memory handled above")
        };
        let rows = pool
            .rows(&format!(
                r#"UPDATE "$consolekey" SET "revoked_at" = {at}, "revoked_by" = {}
                   WHERE "id" = {} AND "revoked_at" IS NULL RETURNING {COLUMNS}"#,
                sql_lit(by),
                sql_lit(id)
            ))
            .await
            .map_err(fault)?;
        Ok(rows.first().and_then(row_of))
    }

    /// A sign-in with the key: its last use, and the session it opened, so
    /// a revocation can end it. Sessions already expired are dropped.
    pub async fn signed_in(
        &self,
        id: &str,
        session: &str,
        expires_at: u64,
        at: u64,
    ) -> Result<(), StoreFault> {
        if let Some(mut m) = self.mem() {
            let m = &mut *m;
            if let Some(hook) = &m.on_sign_in {
                hook(&mut m.keys);
            }
            if let Some(k) = m.keys.iter_mut().find(|k| k.id == id) {
                k.last_used_at = Some(at);
            }
            m.sessions.retain(|(_, _, until)| *until > at);
            m.sessions
                .push((id.to_string(), session.to_string(), expires_at));
            return Ok(());
        }
        let Backend::Pg(pool) = self.0.as_ref() else {
            unreachable!("memory handled above")
        };
        for sql in [
            format!(
                r#"UPDATE "$consolekey" SET "last_used_at" = {at} WHERE "id" = {}"#,
                sql_lit(id)
            ),
            format!(r#"DELETE FROM "$consolekeysession" WHERE "expires_at" <= {at}"#),
            format!(
                r#"INSERT INTO "$consolekeysession" ("key_id","session","expires_at")
                   VALUES ({}, {}, {expires_at}) ON CONFLICT DO NOTHING"#,
                sql_lit(id),
                sql_lit(session)
            ),
        ] {
            pool.rows(&sql).await.map_err(fault)?;
        }
        Ok(())
    }

    /// Spend a challenge: `true` the first time `handle` is spent, `false`
    /// after. Spent challenges past their expiry are dropped. Called only
    /// once a live key's signature over the challenge has verified.
    pub async fn spend(&self, handle: &str, expires_at: u64, now: u64) -> Result<bool, StoreFault> {
        if let Some(mut m) = self.mem() {
            m.spent.retain(|(_, until)| *until > now);
            if m.spent.iter().any(|(h, _)| h == handle) {
                return Ok(false);
            }
            m.spent.push((handle.to_string(), expires_at));
            return Ok(true);
        }
        let Backend::Pg(pool) = self.0.as_ref() else {
            unreachable!("memory handled above")
        };
        pool.rows(&format!(
            r#"DELETE FROM "$consolekeyspent" WHERE "expires_at" <= {now}"#
        ))
        .await
        .map_err(fault)?;
        let rows = pool
            .rows(&format!(
                r#"INSERT INTO "$consolekeyspent" ("handle","expires_at")
                   VALUES ({}, {expires_at}) ON CONFLICT DO NOTHING RETURNING "handle""#,
                sql_lit(handle)
            ))
            .await
            .map_err(fault)?;
        Ok(!rows.is_empty())
    }

    /// The sessions a key opened that have not expired at `now`, with when
    /// each does.
    pub async fn sessions_of(&self, id: &str, now: u64) -> Result<Vec<(String, u64)>, StoreFault> {
        if let Some(m) = self.mem() {
            return Ok(m
                .sessions
                .iter()
                .filter(|(k, _, until)| k == id && *until > now)
                .map(|(_, s, until)| (s.clone(), *until))
                .collect());
        }
        let Backend::Pg(pool) = self.0.as_ref() else {
            unreachable!("memory handled above")
        };
        let rows = pool
            .rows(&format!(
                r#"SELECT "session", "expires_at" FROM "$consolekeysession"
                   WHERE "key_id" = {} AND "expires_at" > {now}"#,
                sql_lit(id)
            ))
            .await
            .map_err(fault)?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let s = r.first()?.clone()?;
                let until = r.get(1)?.as_deref()?.parse().ok()?;
                Some((s, until))
            })
            .collect())
    }
}

/// Why a console sign-in did not go through. The caller is told the one
/// answer either way; the stream is told the word.
#[derive(Debug)]
pub enum SignInRefusal {
    /// The key, the challenge or the signature: `reason` says which.
    Refused(&'static str),
    /// A store could not answer. Not the caller's fault, and retried.
    Store(StoreFault),
}

impl From<StoreFault> for SignInRefusal {
    fn from(f: StoreFault) -> SignInRefusal {
        SignInRefusal::Store(f)
    }
}

/// A console sign-in that went through.
#[derive(Debug, Clone)]
pub struct SignedIn {
    pub principal: PrincipalId,
    pub key: String,
}

/// The console keys: the one client the sign-in routes and the ops use.
pub struct ConsoleKeys {
    pub store: ConsoleKeyStore,
    /// The MAC key a challenge is minted under. Every replica derives the
    /// same one from the token secret ([`ConsoleKeys::challenge_key_from`]),
    /// so a challenge from one replica completes at another.
    challenge_key: [u8; 32],
    /// [`SESSION_SECS_ENV`].
    pub session_secs: u64,
    /// The deployment's own space, answered to a console that signed in.
    pub home: String,
    /// Attempts in the current window: (window start, count).
    attempts: Mutex<(u64, u32)>,
    /// The setting that closes console key sign-in, when it is set:
    /// `ORDO_IDENTITY_URL`. With it the console signs people in at the
    /// identity service and calls the door as them, so a console key left
    /// at the door must not sign in (`plan-identity-service.md` 6, G14).
    pub closed_by: Option<&'static str>,
}

fn handle_of(challenge: &[u8]) -> String {
    b64url_encode(&Sha256::digest(challenge))
}

fn challenge_mac(key: &[u8; 32], console_key: &str, nonce: &[u8], expires: &[u8]) -> [u8; 32] {
    let mut msg = CHALLENGE_DOMAIN.to_vec();
    msg.extend_from_slice(console_key.as_bytes());
    msg.push(0);
    msg.extend_from_slice(nonce);
    msg.extend_from_slice(expires);
    ordo_crypto::hmac_sha256(key, &msg)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Why a sign-in attempt was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Throttled {
    pub retry_after_secs: u64,
}

/// A key id: `ck-` and 16 random bytes in hex.
fn mint_id() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("the operating system CSPRNG is unavailable");
    format!(
        "ck-{}",
        b.iter().map(|x| format!("{x:02x}")).collect::<String>()
    )
}

/// Whether text is a key id this module could have minted.
pub fn well_formed_id(id: &str) -> bool {
    id.len() == 35
        && id.starts_with("ck-")
        && id[3..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b"abcdef").contains(&b))
}

impl ConsoleKeys {
    pub fn new(
        store: ConsoleKeyStore,
        challenge_key: [u8; 32],
        session_secs: u64,
        home: &str,
    ) -> ConsoleKeys {
        ConsoleKeys {
            store,
            challenge_key,
            session_secs,
            home: home.to_string(),
            attempts: Mutex::new((0, 0)),
            closed_by: None,
        }
    }

    /// Close console key sign-in while `setting` is set
    /// ([`ConsoleKeys::closed_by`]).
    pub fn closed_by(mut self, setting: &'static str) -> ConsoleKeys {
        self.closed_by = Some(setting);
        self
    }

    /// The refusal a closed sign-in answers, naming the setting.
    pub fn closed_message(setting: &str) -> String {
        format!(
            "console key sign-in is closed because {setting} is set: the console signs \
             people in at the identity service and calls the door as them. Sign in at \
             the identity service instead; a console key works only with {setting} unset"
        )
    }

    /// The refusal of `std.deployment.addConsoleKey` while console key
    /// sign-in is closed, naming the setting.
    pub fn closed_add_message(setting: &str) -> String {
        format!(
            "no console key is made because {setting} is set: the console signs people in \
             at the identity service and calls the door as them, so a console key would \
             never sign in. Make a machine key at the identity service instead \
             (`ordo people machine-key add`)"
        )
    }

    /// The challenge MAC key, derived from the deployment's token secret so
    /// every replica holds the same one and it means nothing else.
    pub fn challenge_key_from(token_secret: &[u8]) -> [u8; 32] {
        ordo_crypto::hmac_sha256(token_secret, b"ordo-console-challenge-key/v1")
    }

    /// Count one sign-in attempt against this node's window.
    pub fn attempt(&self, now: u64) -> Result<(), Throttled> {
        let mut a = self.attempts.lock().expect("console key attempts");
        if now >= a.0.saturating_add(ATTEMPT_WINDOW_SECS) {
            *a = (now, 0);
        }
        if a.1 >= MAX_ATTEMPTS_PER_WINDOW {
            return Err(Throttled {
                retry_after_secs: a.0.saturating_add(ATTEMPT_WINDOW_SECS).saturating_sub(now),
            });
        }
        a.1 += 1;
        Ok(())
    }

    /// A sign-in challenge for `key`. Minted for any well-formed id, known
    /// or not, so the answer says nothing about which keys exist. Nothing
    /// is stored: the challenge carries its expiry and its MAC.
    pub fn begin(&self, key: &str, now: u64) -> String {
        let mut nonce = [0u8; 16];
        getrandom::getrandom(&mut nonce)
            .expect("the operating system CSPRNG is unavailable; refusing to mint a challenge");
        let expires = now.saturating_add(CHALLENGE_SECS).to_be_bytes();
        let mac = challenge_mac(&self.challenge_key, key, &nonce, &expires);
        let mut bytes = Vec::with_capacity(CHALLENGE_LEN);
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&expires);
        bytes.extend_from_slice(&mac);
        b64url_encode(&bytes)
    }

    /// Check a signed challenge: minted here for this key, not expired,
    /// signed by the live key, and not spent before. It is spent only once
    /// the signature verifies, so nobody but a key's holder writes a row.
    pub async fn complete(
        &self,
        key: &str,
        challenge: &str,
        signature: &str,
        now: u64,
    ) -> Result<SignedIn, SignInRefusal> {
        let Some(bytes) = b64url_decode(challenge)
            .ok()
            .filter(|b| b.len() == CHALLENGE_LEN)
        else {
            return Err(SignInRefusal::Refused("malformed-request"));
        };
        let (nonce, rest) = bytes.split_at(16);
        let (expires, mac) = rest.split_at(8);
        if !ct_eq(
            &challenge_mac(&self.challenge_key, key, nonce, expires),
            mac,
        ) {
            return Err(SignInRefusal::Refused("challenge-for-another-key"));
        }
        let expires_at = u64::from_be_bytes(expires.try_into().expect("8 bytes"));
        if expires_at <= now {
            return Err(SignInRefusal::Refused("challenge-expired"));
        }
        let Some(row) = self.store.get(key).await? else {
            return Err(SignInRefusal::Refused("unknown-console-key"));
        };
        if !row.live() {
            return Err(SignInRefusal::Refused("console-key-revoked"));
        }
        if !verify(&row.public_key, &bytes, signature) {
            return Err(SignInRefusal::Refused("bad-signature"));
        }
        if !self
            .store
            .spend(&handle_of(&bytes), expires_at, now)
            .await?
        {
            return Err(SignInRefusal::Refused("challenge-spent"));
        }
        Ok(SignedIn {
            principal: PrincipalId(row.principal),
            key: row.id,
        })
    }
}

/// Is this session one a console key signed in?
pub fn is_console_session(claims: &TokenClaims) -> bool {
    claims
        .session
        .as_ref()
        .and_then(|s| s.via.as_deref())
        .is_some_and(|v| v.starts_with(VIA_PREFIX))
}

/// The ops a console session may call in a tenant's own space, outside the
/// deployment's. The one list; nothing else lets a console session leave
/// the home space.
///
/// - Step 5 (`plan-console.md` 4.4): the tenant's exit and bucket ops,
///   owner-only there.
/// - Step 9 (4.7, 4.8): the tenant's links, shares and security record.
/// - Step 8 (4.5, People): the G5 reads, the groups and `callersOf`, and
///   the writes the People screen makes: invite, list and revoke
///   invitations; change a role, suspend, reinstate, remove; make and
///   delete a group, add to and take out of one; read and change the
///   access profile and a member's own access; read and set admission; a
///   tenant's recovery of a member it created, its approval, and its
///   recovery settings.
///
/// The door still asks whether the deployment owner holds a role there
/// that may call the op: the owner role for the exit, bucket, membership
/// and admission ops, manager or owner for the rest. A tenant where the
/// owner holds no role answers nothing. Owner succession
/// (`std.space.succeedOwner`) answers in the deployment's own space, so it
/// is not listed here.
pub const TENANT_OPS_OPEN: [&str; 35] = [
    crate::exit_ops::EXPORT,
    crate::exit_ops::DELETE,
    crate::bucket_ops::ATTACH_BUCKET,
    crate::bucket_ops::DETACH_BUCKET,
    crate::people_ops::MEMBERS,
    crate::people_ops::GRANTS,
    crate::people_ops::GROUP_MEMBERS,
    crate::people_ops::CALLABLE_BY,
    crate::profile_ops::GROUPS,
    crate::platform_ops::CALLERS_OF,
    crate::invite_ops::INVITE,
    crate::member_ops::SET_ROLE,
    crate::member_ops::REMOVE,
    crate::profile_ops::ADD_TO_GROUP,
    crate::share_links::LINKS,
    crate::share_links::REVOKE_LINK,
    crate::share_ops::SHARES,
    crate::share_ops::REVOKE_SHARE,
    crate::serve::SECEVENTS_OP,
    // Step 8, session 2: the rest of People.
    crate::member_ops::SUSPEND,
    crate::member_ops::REINSTATE,
    crate::profile_ops::CREATE_GROUP,
    crate::profile_ops::DELETE_GROUP,
    crate::profile_ops::REMOVE_FROM_GROUP,
    crate::profile_ops::PROFILE,
    crate::profile_ops::SET_OP_ACCESS,
    crate::profile_ops::RESET_OP_ACCESS,
    crate::profile_ops::SET_MEMBER_ACCESS,
    crate::invite_ops::INVITATIONS,
    crate::invite_ops::REVOKE_INVITATION,
    crate::admission::ADMISSION,
    crate::admission::SET_ADMISSION,
    crate::identity_ops::RECOVER_MEMBER,
    crate::identity_ops::APPROVE_MEMBER_RECOVERY,
    crate::identity_ops::SET_RECOVERY_POLICY,
];

/// How recent the owner's relayed touch must be for a console session to
/// reach a tenant's space at all: the 300 seconds a touch keeps a console
/// session present.
pub const TENANT_REACH_SECS: u64 = 300;

/// Where a request is going, for [`console_may`].
#[derive(Debug, Clone, Copy)]
pub enum Reach<'a> {
    /// An `/auth/*` route, by its path.
    Auth(&'a str),
    /// A door op in a space. `home` is the deployment's own space, `None`
    /// when the door was not told it. `now` is the door's clock, in
    /// seconds.
    Space {
        space: &'a str,
        home: Option<&'a str>,
        op: &'a str,
        now: u64,
    },
}

/// The one check of where a console key session may go: the relayed touch
/// and its logout among the `/auth/*` routes, and the door in the
/// deployment's own space. Any other token passes. The error is the reason
/// for the security record.
///
/// **One way out of the home space** (step 5): an op in
/// [`TENANT_OPS_OPEN`], in a session the owner touched for in the last
/// [`TENANT_REACH_SECS`]. The console key alone never leaves the home
/// space: a key with no touch behind it reaches no tenant.
pub fn console_may(claims: &TokenClaims, reach: Reach<'_>) -> Result<(), &'static str> {
    if !is_console_session(claims) {
        return Ok(());
    }
    match reach {
        Reach::Auth(path) if AUTH_ROUTES_OPEN.contains(&path) => Ok(()),
        Reach::Auth(_) => Err("console-session-not-for-this-route"),
        Reach::Space { space, home, .. } if home == Some(space) => Ok(()),
        Reach::Space { op, now, home, .. } if home.is_some() && TENANT_OPS_OPEN.contains(&op) => {
            let touched = claims
                .user_presence_at
                .is_some_and(|t| t.0 <= now && now - t.0 <= TENANT_REACH_SECS);
            if touched {
                Ok(())
            } else {
                Err("console-session-tenant-op-without-touch")
            }
        }
        Reach::Space { .. } => Err("console-session-outside-home-space"),
    }
}

/// Does `signature` (base64url) verify over [`SIGNED_PREFIX`] and the
/// challenge, under `public_key` (base64url)? Strict verification: no
/// malleable or small-order forms.
pub fn verify(public_key: &str, challenge: &[u8], signature: &str) -> bool {
    let Some(key) = b64url_decode(public_key)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
        .and_then(|b| ed25519_dalek::VerifyingKey::from_bytes(&b).ok())
    else {
        return false;
    };
    let Some(sig) = b64url_decode(signature)
        .ok()
        .and_then(|b| ed25519_dalek::Signature::from_slice(&b).ok())
    else {
        return false;
    };
    let mut msg = SIGNED_PREFIX.to_vec();
    msg.extend_from_slice(challenge);
    key.verify_strict(&msg, &sig).is_ok()
}

/// The three ops' specs.
pub fn specs() -> Vec<OpSpec> {
    let text = |name: &str, max: u32, required: bool| ParamSpec {
        name: name.to_string(),
        ty: ParamTy::Text { max_len: Some(max) },
        required,
        bounds: None,
    };
    let fields = [
        "id",
        "principal",
        "name",
        "publicKey",
        "createdAt",
        "lastUsedAt",
        "revokedAt",
        "revokedBy",
    ];
    let spec = |name: &str, kind: OpKind, params: Vec<ParamSpec>| OpSpec {
        name: name.to_string(),
        version_hash: crate::platform_ops::PLATFORM_VERSION.to_string(),
        kind,
        params,
        output_fields: fields.iter().map(|f| FieldSpec::new(*f)).collect(),
        live_capable: false,
        sync: Syncability::NotSyncable(if kind == OpKind::Read {
            NotSyncable::NoAnchor
        } else {
            NotSyncable::NotARead
        }),
        requires_idempotency_key: false,
        public: Default::default(),
        refinable: RefineTier::None,
        aggregate_only: false,
        mode_label: None,
        cost: OpCost::default(),
    };
    vec![
        spec(
            ADD,
            OpKind::Write,
            vec![
                text("publicKey", 64, true),
                text("name", MAX_NAME as u32, true),
            ],
        ),
        spec(LIST, OpKind::Read, vec![]),
        spec(REVOKE, OpKind::Write, vec![text("id", 64, true)]),
    ]
}

/// What the three ops need.
pub struct ConsoleKeyOps {
    pub keys: Arc<ConsoleKeys>,
    /// The deny list the door consults, so a revocation ends the key's
    /// sessions now rather than at their expiry. `None` ends none, and the
    /// answer says so.
    pub revocations: Option<Arc<dyn ordo_identity::session::SessionRevocations>>,
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub stream: Option<Arc<ordo_secevent::Recorder>>,
}

fn param<'a>(bound: &'a BoundParams, name: &str) -> Option<&'a str> {
    bound
        .0
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn refused(why: impl Into<String>) -> ExecError {
    ExecError::new(ErrorCode::TenantRefused, why)
}

/// Write a console key act to the deployment's security record.
pub fn record(
    stream: &ordo_secevent::Recorder,
    by: &str,
    op: &str,
    act: &str,
    detail: String,
    at: u64,
) {
    stream.observe(
        ordo_secevent::Observation {
            space: None,
            subject: ordo_secevent::Subject::member(by),
            op: op.to_string(),
            kind: ordo_secevent::EventKind::ConsoleKey {
                act: act.to_string(),
                detail,
            },
            at: ordo_commandlog::Timestamp(at),
            correlation: ordo_secevent::correlation::mint(),
        },
        ordo_secevent::Relationship::None,
    );
}

impl ConsoleKeyOps {
    pub async fn execute(
        &self,
        name: &str,
        space: &str,
        bound: &BoundParams,
        caller: &str,
    ) -> Result<ExecResult, ExecError> {
        if space != self.keys.home {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                "console keys are the deployment's and these ops answer only in the \
                 deployment's own space",
            ));
        }
        // With the identity service a new console key could never sign in,
        // so none is made (step 8). Listing and revoking stay, so a key
        // left from before can be seen and ended.
        if let (ADD, Some(setting)) = (name, self.keys.closed_by) {
            return Err(refused(ConsoleKeys::closed_add_message(setting)));
        }
        let now = (self.now)();
        let rows = match name {
            ADD => vec![self.add(bound, caller, now).await?],
            LIST => self
                .keys
                .store
                .list()
                .await
                .map_err(ExecError::from)?
                .iter()
                .map(KeyRow::json)
                .collect(),
            REVOKE => vec![self.revoke(bound, caller, now).await?],
            other => {
                return Err(ExecError::internal(format!(
                    "{other} is not a console key op"
                )))
            }
        };
        Ok(ExecResult {
            rows,
            next_cursor: None,
            position: None,
        })
    }

    async fn add(&self, bound: &BoundParams, caller: &str, now: u64) -> Result<Value, ExecError> {
        let public_key = param(bound, "publicKey").unwrap_or_default();
        let ok_key = b64url_decode(public_key)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
            .is_some_and(|b| ed25519_dalek::VerifyingKey::from_bytes(&b).is_ok());
        if !ok_key {
            return Err(refused(
                "publicKey must be a 32-byte Ed25519 public key in base64url, as the console \
                 makes it",
            ));
        }
        let name = param(bound, "name").unwrap_or_default();
        if name.is_empty()
            || name.chars().count() > MAX_NAME
            || name.chars().any(|c| c.is_control())
        {
            return Err(refused(format!(
                "name must be 1 to {MAX_NAME} printable characters"
            )));
        }
        let row = KeyRow {
            id: mint_id(),
            principal: caller.to_string(),
            name: name.to_string(),
            public_key: public_key.to_string(),
            created_at: now,
            last_used_at: None,
            revoked_at: None,
            revoked_by: None,
        };
        if !self
            .keys
            .store
            .insert(&row)
            .await
            .map_err(ExecError::from)?
        {
            return Err(refused(format!(
                "this owner already holds {MAX_LIVE_PER_OWNER} live console keys, or this \
                 public key is already enrolled; revoke one with {REVOKE} first"
            )));
        }
        if let Some(s) = &self.stream {
            record(
                s,
                caller,
                ADD,
                "added",
                format!("key={} principal={caller}", row.id),
                now,
            );
        }
        Ok(row.json())
    }

    async fn revoke(
        &self,
        bound: &BoundParams,
        caller: &str,
        now: u64,
    ) -> Result<Value, ExecError> {
        let id = param(bound, "id").unwrap_or_default();
        let Some(row) = self
            .keys
            .store
            .revoke(id, caller, now)
            .await
            .map_err(ExecError::from)?
        else {
            return Err(ExecError::new(
                ErrorCode::NotFound,
                format!("no live console key has the id `{id}`; {LIST} lists them"),
            ));
        };
        let sessions = self
            .keys
            .store
            .sessions_of(&row.id, now)
            .await
            .map_err(ExecError::from)?;
        let mut ended = 0u64;
        if let Some(rev) = &self.revocations {
            for (s, until) in &sessions {
                let id = ordo_identity::session::SessionId(s.clone());
                if rev.revoke(&id, Timestamp(*until)).is_ok() {
                    ended += 1;
                }
            }
        }
        if let Some(s) = &self.stream {
            record(
                s,
                caller,
                REVOKE,
                "revoked",
                format!(
                    "key={} principal={} sessions_ended={ended}",
                    row.id, row.principal
                ),
                now,
            );
        }
        let mut out = row.json();
        out["sessionsEnded"] = json!(ended);
        out["sessionsOpen"] = json!(sessions.len() as u64 - ended);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_length_is_bounded_and_names_its_setting() {
        assert_eq!(session_secs_from(&|_| None), Ok(DEFAULT_SESSION_SECS));
        assert_eq!(session_secs_from(&|_| Some("120".into())), Ok(120));
        for bad in ["0", "59", "3601", "soon"] {
            let e = session_secs_from(&|_| Some(bad.into())).unwrap_err();
            assert!(e.contains("from 60 to 3600"), "{e}");
        }
    }

    #[test]
    fn a_signature_verifies_only_over_the_prefixed_challenge() {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let pk = b64url_encode(sk.verifying_key().as_bytes());
        let challenge = [9u8; 32];
        let mut msg = SIGNED_PREFIX.to_vec();
        msg.extend_from_slice(&challenge);
        use ed25519_dalek::Signer as _;
        let good = b64url_encode(&sk.sign(&msg).to_bytes());
        let bare = b64url_encode(&sk.sign(&challenge).to_bytes());
        assert!(verify(&pk, &challenge, &good));
        assert!(!verify(&pk, &challenge, &bare), "the prefix is required");
        assert!(!verify(&pk, &[8u8; 32], &good), "bound to the challenge");
    }

    #[test]
    fn ids_are_minted_well_formed() {
        let id = mint_id();
        assert!(well_formed_id(&id), "{id}");
        assert!(!well_formed_id("ck-../../etc"));
    }
}
