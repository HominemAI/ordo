//! Recovery codes: a person's piece of paper for the day every passkey is gone
//! (`plan-passkey-recovery.md` R4, `plan-second-factor.md` §6).
//!
//! # The standard
//!
//! NIST SP 800-63B's rules for look-up secrets, and the practice of the large
//! services (GitHub, Google), which the owner asked for on 2026-09-29: *"if
//! theres an industry standard way to add more validity to the One time backup
//! codes then lets add that"*.
//!
//! | Rule | Here |
//! |---|---|
//! | generation from an approved random bit generator | the operating system's, 120 bits a code |
//! | at least 20 bits of entropy (112 to be stored under a plain one-way hash) | 120 bits, so a plain SHA-256 is the approved form and no salt or slow hash is owed |
//! | used successfully once only | `used_at` is set by the one atomic statement that reads the code |
//! | delivered securely | in the response to the session that issued them, once, `Cache-Control: no-store`, never mailed, never shown again |
//! | rate limiting | a deployment-wide throttle on failures (below), because a wrong code names nobody |
//!
//! Twenty-four characters from a 32-symbol alphabet (Crockford base32, which
//! has no `I`, `L`, `O` or `U`), shown as six groups of four. Ten to a set.
//! Issuing a new set voids the old one, and so does a recovery completing.
//!
//! # One set, two uses
//!
//! The table has a `used_as` column for the second factor's use of the same
//! codes (`second-step`, `plan-second-factor.md` §6.4). R4 builds the use
//! `recovery`: a code alone opens a recovery case and nothing else, and what
//! comes back is a ticket for a **new passkey**. A code is not a way to sign
//! in.
//!
//! # What is stored
//!
//! The SHA-256 of the normalised code under a domain tag, never the code. The
//! plan asks for an HMAC under a deployment key so a table dump alone confirms
//! nothing. At 120 bits a dump confirms nothing either way, and a keyed hash
//! would tie every printed set to a key whose rotation would void them all
//! without anyone being told. It waits on the key service (18y), which is
//! where a deployment key that can rotate safely belongs.
//!
//! # The throttle
//!
//! A wrong code names no person, so failures cannot be counted against one.
//! They are counted for the deployment: a window of a minute, and past its
//! limit every redemption is answered `429` until the window rolls. At 120
//! bits a guess is worth nothing and the throttle only bounds the cost of
//! being guessed at. The per-person lock (twenty in a row) belongs to the
//! second step, where the person is named by the first factor.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::pgsync::{cell, PgSync, Source};
use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError};
use ordo_identity::StoreFault;

/// Characters a code is made of: Crockford base32, no `I`, `L`, `O`, `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// A code is twenty-four characters, five bits each.
pub const CODE_LEN: usize = 24;
/// Ten to a set by default (`ORDO_RECOVERY_CODES_PER_SET`).
pub const SET_SIZE: usize = 10;
/// How many wrong codes a deployment answers in a minute before it answers
/// `429` to every redemption for the rest of it.
pub const DEFAULT_FAILURES_PER_MINUTE: u32 = 30;

/// One fresh code, twenty-four characters, from the operating system's
/// random generator.
pub fn mint_code() -> String {
    // 15 bytes are 120 bits, which is exactly twenty-four five-bit symbols.
    // `random_principal_suffix` is 16 bytes from the operating system's
    // generator, as hex. Fifteen of them are the 120 bits.
    let hex = ordo_identity::directory::random_principal_suffix();
    let mut bytes = [0u8; 15];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex from the generator");
    }
    let mut out = String::with_capacity(CODE_LEN);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    out
}

/// A code as a person reads it and types it: six groups of four.
pub fn display(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("-")
}

/// What a person typed, as the code it names: upper case, groups and spaces
/// dropped, and the characters Crockford treats as the same (`O` is `0`, `I`
/// and `L` are `1`). `None` for anything that is not twenty-four symbols.
pub fn normalize(input: &str) -> Option<String> {
    let mut out = String::with_capacity(CODE_LEN);
    for c in input.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_LEN).then_some(out)
}

/// The hash a code is stored under.
pub fn hash_of(code: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"ordo.recoverycode.v1\0");
    h.update(code.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Where codes are kept.
pub trait CodeStore: Send + Sync {
    /// Replace the person's whole set, used codes included, with these
    /// hashes. One statement, so a failure leaves the old set.
    fn replace_set(&self, principal: &str, hashes: &[String], at: u64) -> Result<(), StoreFault>;

    /// Whose an unused code is.
    fn find(&self, hash: &str) -> Result<Option<String>, StoreFault>;

    /// Mark an unused code used, as `used_as`, **and return whose it was**,
    /// atomically. Of two requests with one code, one gets `Some`.
    fn consume(&self, hash: &str, used_as: &str, at: u64) -> Result<Option<String>, StoreFault>;

    /// Mark an unused code used **only if it is `principal`'s**, atomically.
    /// The second step names the person by their first factor, so a code that
    /// belongs to somebody else is not spent by it. `true` when it was.
    fn consume_for(
        &self,
        principal: &str,
        hash: &str,
        used_as: &str,
        at: u64,
    ) -> Result<bool, StoreFault>;

    /// How many unused codes the person holds.
    fn remaining(&self, principal: &str) -> Result<usize, StoreFault>;

    /// Delete the person's unused codes. Returns how many.
    fn void(&self, principal: &str) -> Result<usize, StoreFault>;
}

/// The process-local store, for tests and for nothing a deployment runs.
#[derive(Default)]
pub struct MemoryCodeStore {
    // hash -> (principal, used_as when used)
    rows: Mutex<BTreeMap<String, (String, Option<String>)>>,
}

impl MemoryCodeStore {
    pub fn new() -> MemoryCodeStore {
        MemoryCodeStore::default()
    }

    /// Every hash held, used or not. For a test that asks whether a code was
    /// stored as itself.
    pub fn hashes(&self) -> Vec<String> {
        self.rows
            .lock()
            .expect("code store")
            .keys()
            .cloned()
            .collect()
    }

    /// How a code was used, if it was.
    pub fn used_as(&self, hash: &str) -> Option<String> {
        self.rows
            .lock()
            .expect("code store")
            .get(hash)
            .and_then(|(_, u)| u.clone())
    }
}

impl CodeStore for MemoryCodeStore {
    fn replace_set(&self, principal: &str, hashes: &[String], _at: u64) -> Result<(), StoreFault> {
        let mut rows = self.rows.lock().expect("code store");
        rows.retain(|_, (p, _)| p != principal);
        for h in hashes {
            rows.insert(h.clone(), (principal.to_string(), None));
        }
        Ok(())
    }

    fn find(&self, hash: &str) -> Result<Option<String>, StoreFault> {
        Ok(self
            .rows
            .lock()
            .expect("code store")
            .get(hash)
            .filter(|(_, used)| used.is_none())
            .map(|(p, _)| p.clone()))
    }

    fn consume(&self, hash: &str, used_as: &str, _at: u64) -> Result<Option<String>, StoreFault> {
        let mut rows = self.rows.lock().expect("code store");
        match rows.get_mut(hash) {
            Some((p, used)) if used.is_none() => {
                *used = Some(used_as.to_string());
                Ok(Some(p.clone()))
            }
            _ => Ok(None),
        }
    }

    fn consume_for(
        &self,
        principal: &str,
        hash: &str,
        used_as: &str,
        _at: u64,
    ) -> Result<bool, StoreFault> {
        let mut rows = self.rows.lock().expect("code store");
        match rows.get_mut(hash) {
            Some((p, used)) if used.is_none() && p == principal => {
                *used = Some(used_as.to_string());
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn remaining(&self, principal: &str) -> Result<usize, StoreFault> {
        Ok(self
            .rows
            .lock()
            .expect("code store")
            .values()
            .filter(|(p, used)| p == principal && used.is_none())
            .count())
    }

    fn void(&self, principal: &str) -> Result<usize, StoreFault> {
        let mut rows = self.rows.lock().expect("code store");
        let before = rows.len();
        rows.retain(|_, (p, used)| !(p == principal && used.is_none()));
        Ok(before - rows.len())
    }
}

/// The code table. `hash` is the key, so redeeming is a lookup by something
/// the holder proves they know and nobody can list. `used_as` is `''` for an
/// unused code and `second-step` or `recovery` for a used one.
pub const CODE_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$recoverycode" (
  "principal" text   NOT NULL,
  "hash"      text   NOT NULL,
  "issued_at" bigint NOT NULL,
  "used_at"   bigint,
  "used_as"   text   NOT NULL DEFAULT '',
  PRIMARY KEY ("hash")
)"#;

pub const CODE_INDEX_DDL: &str = r#"CREATE INDEX IF NOT EXISTS "$recoverycode$principal"
    ON "$recoverycode" ("principal")"#;

/// Create `$recoverycode`, retry-once for the reason the other platform
/// tables do.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [CODE_DDL, CODE_INDEX_DDL] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// The Postgres code store.
pub struct PgCodeStore {
    db: PgSync,
}

impl PgCodeStore {
    /// Dial a connection of its own. Refuses when `$recoverycode` is absent.
    pub fn open(cfg: PoolConfig) -> Result<PgCodeStore, StoreFault> {
        PgCodeStore::start(Source::Dial(cfg))
    }

    /// On a connection somebody else owns. Never the request pool.
    pub fn open_on(pool: Pool) -> Result<PgCodeStore, StoreFault> {
        PgCodeStore::start(Source::Shared(pool))
    }

    fn start(source: Source) -> Result<PgCodeStore, StoreFault> {
        let db = PgSync::start(source, "recoverycode")?;
        db.sql(r#"SELECT 1 FROM "$recoverycode" LIMIT 1"#)?;
        Ok(PgCodeStore { db })
    }
}

impl CodeStore for PgCodeStore {
    fn replace_set(&self, principal: &str, hashes: &[String], at: u64) -> Result<(), StoreFault> {
        let values = hashes
            .iter()
            .map(|h| format!("({}, {}, {at})", sql_lit(principal), sql_lit(h)))
            .collect::<Vec<_>>()
            .join(", ");
        // One statement: the old set goes and the new one arrives, or
        // neither does.
        self.db.sql(&format!(
            r#"WITH d AS (DELETE FROM "$recoverycode" WHERE "principal" = {} RETURNING 1)
               INSERT INTO "$recoverycode" ("principal", "hash", "issued_at") VALUES {values}"#,
            sql_lit(principal)
        ))?;
        Ok(())
    }

    fn find(&self, hash: &str) -> Result<Option<String>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT "principal" FROM "$recoverycode" WHERE "hash" = {} AND "used_at" IS NULL"#,
            sql_lit(hash)
        ))?;
        Ok(rows.first().and_then(|r| cell(r, 0)))
    }

    fn consume(&self, hash: &str, used_as: &str, at: u64) -> Result<Option<String>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"UPDATE "$recoverycode" SET "used_at" = {at}, "used_as" = {}
               WHERE "hash" = {} AND "used_at" IS NULL
               RETURNING "principal""#,
            sql_lit(used_as),
            sql_lit(hash)
        ))?;
        Ok(rows.first().and_then(|r| cell(r, 0)))
    }

    fn consume_for(
        &self,
        principal: &str,
        hash: &str,
        used_as: &str,
        at: u64,
    ) -> Result<bool, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"UPDATE "$recoverycode" SET "used_at" = {at}, "used_as" = {}
               WHERE "hash" = {} AND "principal" = {} AND "used_at" IS NULL
               RETURNING 1"#,
            sql_lit(used_as),
            sql_lit(hash),
            sql_lit(principal)
        ))?;
        Ok(!rows.is_empty())
    }

    fn remaining(&self, principal: &str) -> Result<usize, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT COUNT(*) FROM "$recoverycode" WHERE "principal" = {} AND "used_at" IS NULL"#,
            sql_lit(principal)
        ))?;
        Ok(rows
            .first()
            .and_then(|r| cell(r, 0))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    fn void(&self, principal: &str) -> Result<usize, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"WITH d AS (DELETE FROM "$recoverycode"
                          WHERE "principal" = {} AND "used_at" IS NULL RETURNING 1)
               SELECT COUNT(*) FROM d"#,
            sql_lit(principal)
        ))?;
        Ok(rows
            .first()
            .and_then(|r| cell(r, 0))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }
}

/// Why a code did not redeem.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodeError {
    /// Not a code, not one this deployment issued, or already used. One
    /// answer, so a caller learns nothing about which strings were real.
    Refused,
    /// The deployment has answered too many wrong codes this minute.
    Throttled {
        retry_after_secs: u64,
    },
    /// The person has a live recovery case, so a code opens no second one.
    LiveCase,
    Unavailable(StoreFault),
}

/// The failure throttle: a window and a count.
struct Throttle {
    window_start: u64,
    failures: u32,
}

/// Recovery codes: issue a set, find and consume one, count what is left.
pub struct RecoveryCodes {
    store: Arc<dyn CodeStore>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
    failures_per_minute: u32,
    set_size: usize,
    throttle: Mutex<Throttle>,
}

impl RecoveryCodes {
    pub fn new(store: Arc<dyn CodeStore>) -> RecoveryCodes {
        RecoveryCodes {
            store,
            now: Box::new(crate::runtime::now_secs),
            failures_per_minute: DEFAULT_FAILURES_PER_MINUTE,
            set_size: SET_SIZE,
            throttle: Mutex::new(Throttle {
                window_start: 0,
                failures: 0,
            }),
        }
    }

    pub fn with_clock(mut self, now: Box<dyn Fn() -> u64 + Send + Sync>) -> RecoveryCodes {
        self.now = now;
        self
    }

    /// How many codes one set holds.
    pub fn with_set_size(mut self, n: usize) -> RecoveryCodes {
        self.set_size = n;
        self
    }

    pub fn with_failure_limit(mut self, per_minute: u32) -> RecoveryCodes {
        self.failures_per_minute = per_minute;
        self
    }

    /// Issue a new set for `principal`, voiding the old one. The codes are
    /// returned once, in the form a person reads them, and stored only as
    /// hashes.
    pub fn issue(&self, principal: &str) -> Result<Vec<String>, StoreFault> {
        let codes: Vec<String> = (0..self.set_size).map(|_| mint_code()).collect();
        let hashes: Vec<String> = codes.iter().map(|c| hash_of(c)).collect();
        self.store.replace_set(principal, &hashes, (self.now)())?;
        Ok(codes.iter().map(|c| display(c)).collect())
    }

    fn check_throttle(&self) -> Result<(), CodeError> {
        let now = (self.now)();
        let mut t = self.throttle.lock().expect("throttle");
        if now.saturating_sub(t.window_start) >= 60 {
            t.window_start = now;
            t.failures = 0;
        }
        if t.failures >= self.failures_per_minute {
            return Err(CodeError::Throttled {
                retry_after_secs: 60 - now.saturating_sub(t.window_start).min(59),
            });
        }
        Ok(())
    }

    fn note_failure(&self) {
        let now = (self.now)();
        let mut t = self.throttle.lock().expect("throttle");
        if now.saturating_sub(t.window_start) >= 60 {
            t.window_start = now;
            t.failures = 0;
        }
        t.failures += 1;
    }

    /// Whose an unused code is, without using it. A wrong code counts against
    /// the throttle.
    pub fn peek(&self, typed: &str) -> Result<String, CodeError> {
        self.check_throttle()?;
        let found = match normalize(typed) {
            Some(code) => self
                .store
                .find(&hash_of(&code))
                .map_err(CodeError::Unavailable)?,
            None => None,
        };
        match found {
            Some(p) => Ok(p),
            None => {
                self.note_failure();
                Err(CodeError::Refused)
            }
        }
    }

    /// Use a code, once, as `used_as`. Returns whose it was and how many the
    /// person has left.
    pub fn consume(&self, typed: &str, used_as: &str) -> Result<(String, usize), CodeError> {
        let Some(code) = normalize(typed) else {
            self.note_failure();
            return Err(CodeError::Refused);
        };
        let taken = self
            .store
            .consume(&hash_of(&code), used_as, (self.now)())
            .map_err(CodeError::Unavailable)?;
        match taken {
            Some(p) => {
                let left = self.store.remaining(&p).map_err(CodeError::Unavailable)?;
                Ok((p, left))
            }
            None => {
                self.note_failure();
                Err(CodeError::Refused)
            }
        }
    }

    /// Use a code as **`principal`'s** second step, once. `Some(left)` when it
    /// was theirs and unused, `None` otherwise: malformed, unknown, used, or
    /// another person's, all the same answer. It does not count against the
    /// deployment-wide throttle: the person is named by their first factor, so
    /// the failures are counted against them by the second step's own lock.
    pub fn consume_for(&self, principal: &str, typed: &str) -> Result<Option<usize>, StoreFault> {
        let Some(code) = normalize(typed) else {
            return Ok(None);
        };
        let won =
            self.store
                .consume_for(principal, &hash_of(&code), "second-step", (self.now)())?;
        if !won {
            return Ok(None);
        }
        Ok(Some(self.store.remaining(principal)?))
    }

    /// How many unused codes the person holds.
    pub fn remaining(&self, principal: &str) -> Result<usize, StoreFault> {
        self.store.remaining(principal)
    }

    /// Void the person's unused codes. Returns how many.
    pub fn void(&self, principal: &str) -> Result<usize, StoreFault> {
        self.store.void(principal)
    }
}

/// What a recovery does to a person's codes when it completes: the unused
/// ones are voided, and the person is offered a new set once the quarantine
/// ends (`plan-passkey-recovery.md` §8).
pub struct VoidCodes(pub Arc<RecoveryCodes>);

impl crate::recovery::Settlement for VoidCodes {
    fn name(&self) -> &'static str {
        "recovery-codes"
    }
    fn settle(&self, principal: &str, _case: &str) -> Result<(), StoreFault> {
        self.0.void(principal).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_code_is_twenty_four_symbols_from_the_alphabet() {
        for _ in 0..200 {
            let c = mint_code();
            assert_eq!(c.len(), CODE_LEN);
            assert!(c.bytes().all(|b| ALPHABET.contains(&b)), "{c}");
        }
        let a: std::collections::BTreeSet<String> = (0..200).map(|_| mint_code()).collect();
        assert_eq!(a.len(), 200, "no repeats in 200 draws");
    }

    #[test]
    fn a_code_is_shown_in_six_groups_and_typed_back_however_a_person_types_it() {
        let c = mint_code();
        let shown = display(&c);
        assert_eq!(shown.split('-').count(), 6);
        assert_eq!(normalize(&shown).as_deref(), Some(c.as_str()));
        assert_eq!(
            normalize(&shown.to_lowercase()).as_deref(),
            Some(c.as_str())
        );
        assert_eq!(
            normalize(&shown.replace('-', " ")).as_deref(),
            Some(c.as_str())
        );
        // Crockford's aliases: O is 0, I and L are 1.
        assert_eq!(
            normalize("OOOO-IIII-LLLL-0000-1111-1111").as_deref(),
            Some("000011111111000011111111")
        );
        // Too short, too long, and a symbol outside the alphabet.
        assert_eq!(normalize(&c[..23]), None);
        assert_eq!(normalize(&format!("{c}0")), None);
        assert_eq!(normalize("UUUU-UUUU-UUUU-UUUU-UUUU-UUUU"), None);
        assert_eq!(normalize(""), None);
    }

    /// A code is spent as its owner's second step and as nobody else's: a code
    /// that belongs to another person is not accepted and not consumed.
    #[test]
    fn a_second_step_spends_only_the_owners_code() {
        let store = Arc::new(MemoryCodeStore::new());
        let codes = RecoveryCodes::new(store.clone());
        let ana = codes.issue("prn_ana").expect("a set");
        let bea = codes.issue("prn_bea").expect("a set");
        // Bea's code, offered for Ana: refused, and still Bea's.
        assert_eq!(codes.consume_for("prn_ana", &bea[0]).expect("store"), None);
        assert_eq!(codes.remaining("prn_bea").expect("count"), 10);
        // Ana's own is spent once, as a second step.
        assert_eq!(
            codes.consume_for("prn_ana", &ana[0]).expect("store"),
            Some(9)
        );
        assert_eq!(codes.consume_for("prn_ana", &ana[0]).expect("store"), None);
        let hash = hash_of(&normalize(&ana[0]).expect("a code"));
        assert_eq!(store.used_as(&hash).as_deref(), Some("second-step"));
        // A malformed answer is the same refusal.
        assert_eq!(
            codes.consume_for("prn_ana", "not a code").expect("ok"),
            None
        );
    }

    #[test]
    fn what_is_stored_is_a_hash_and_not_the_code() {
        let c = mint_code();
        assert_ne!(hash_of(&c), c);
        assert_eq!(hash_of(&c).len(), 64);
        assert_eq!(hash_of(&c), hash_of(&c));
    }
}
