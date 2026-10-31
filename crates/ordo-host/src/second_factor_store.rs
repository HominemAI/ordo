//! Where second factors are kept (`plan-second-factor.md` §3).
//!
//! One row per factor in `$secondfactor`, deployment-scoped for the reason
//! `passkeys.rs` and `credentials.rs` are: sign-in must work when a tenant's
//! database is unreachable, and the credential is the deployment's.
//!
//! What is stored is what the method needs to check an answer and what the
//! throttle needs to count wrong ones. A code-generating factor's secret is
//! **sealed**, never stored in the clear and never hashed, because the verifier
//! has to compute the same HMAC the person's app does. A security key's row
//! holds the id of its credential in `$passkey`.
//!
//! The counters are decided by the database, one statement each, so two
//! replicas checking two answers at once cannot both win one time step and
//! cannot both miss a failure.

use std::collections::BTreeMap;
use std::sync::Mutex;

use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError};
use ordo_identity::second_step::PersonMode;

use crate::pgsync::{cell, PgSync, Source};
use ordo_identity::StoreFault;

/// The kind word a lock row carries. A backup code is not a factor; its row
/// exists so the person's wrong codes in a row have somewhere to be counted.
pub const LOCK_KIND: &str = "backup-code";

/// One second factor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Factor {
    /// `sf_<128 random bits, hex>`.
    pub id: String,
    pub principal: String,
    /// `totp`, `security-key` or `backup-code` (a lock row).
    pub kind: String,
    /// What the person called it.
    pub label: String,
    /// TOTP: the sealed secret, hex. Security key: the `$passkey` credential's
    /// authenticator id.
    pub secret: String,
    /// TOTP: the last accepted time step. Unused otherwise.
    pub last_counter: u64,
    /// Consecutive wrong answers. Reset by a right one.
    pub failures: u64,
    /// When the throttle locked it. Zero for a factor in use.
    pub disabled_at: u64,
    pub enrolled_at: u64,
    pub last_used_at: u64,
}

impl Factor {
    pub fn locked(&self) -> bool {
        self.disabled_at != 0
    }
}

/// The id of the lock row that counts a person's wrong backup codes.
pub fn lock_row_id(principal: &str) -> String {
    format!("lock_{principal}")
}

pub trait SecondFactorStore: Send + Sync {
    fn insert(&self, f: &Factor) -> Result<(), StoreFault>;

    /// Every factor of the person, locked ones included, lock rows excluded.
    fn list(&self, principal: &str) -> Result<Vec<Factor>, StoreFault>;

    fn get(&self, id: &str) -> Result<Option<Factor>, StoreFault>;

    /// Remove one of the person's factors. `None` when it was not theirs.
    fn remove(&self, id: &str, principal: &str) -> Result<Option<Factor>, StoreFault>;

    /// Remove every factor of the person, lock rows included. Returns how many
    /// factors went.
    fn remove_all(&self, principal: &str) -> Result<usize, StoreFault>;

    /// Accept a time step for a code-generating factor **if it is later than
    /// the last one accepted**, in one statement. Resets the failure count and
    /// stamps the use. `false` when another answer got the step first.
    fn accept_counter(&self, id: &str, step: u64, at: u64) -> Result<bool, StoreFault>;

    /// A right answer that has no counter: reset the failures, stamp the use.
    fn note_success(&self, id: &str, at: u64) -> Result<(), StoreFault>;

    /// A wrong answer: one more failure, and the factor locked when it reaches
    /// `lock_after`. Returns the count and whether it is now locked.
    fn note_failure(&self, id: &str, lock_after: u64, at: u64) -> Result<(u64, bool), StoreFault>;

    /// Make sure the person has a lock row to count wrong backup codes on.
    fn ensure_lock_row(&self, principal: &str, at: u64) -> Result<(), StoreFault>;

    /// Unlock and zero a row: a fresh set of backup codes does it for the lock
    /// row, a re-enrolment for a factor.
    fn clear_lock(&self, id: &str) -> Result<(), StoreFault>;

    /// What the person chose for their own sign-ins
    /// (`plan-second-factor.md` §5.4). The default for a person who chose
    /// nothing.
    fn person_mode(&self, principal: &str) -> Result<PersonMode, StoreFault>;

    fn set_person_mode(&self, principal: &str, mode: PersonMode, at: u64)
        -> Result<(), StoreFault>;

    /// What the person chose, or `None` when they never chose (no row).
    fn chosen_person_mode(&self, principal: &str) -> Result<Option<PersonMode>, StoreFault>;

    /// Forget the person's choice, so they are as a person who never chose.
    fn clear_person_mode(&self, principal: &str) -> Result<(), StoreFault>;
}

/// The process-local store, for tests and for nothing a deployment runs.
#[derive(Default)]
pub struct MemorySecondFactorStore {
    rows: Mutex<BTreeMap<String, Factor>>,
    modes: Mutex<BTreeMap<String, PersonMode>>,
}

impl MemorySecondFactorStore {
    pub fn new() -> MemorySecondFactorStore {
        MemorySecondFactorStore::default()
    }

    /// Every row, lock rows included. For a test that asks what was stored.
    pub fn all(&self) -> Vec<Factor> {
        self.rows
            .lock()
            .expect("factor store")
            .values()
            .cloned()
            .collect()
    }
}

impl SecondFactorStore for MemorySecondFactorStore {
    fn insert(&self, f: &Factor) -> Result<(), StoreFault> {
        let mut rows = self.rows.lock().expect("factor store");
        if rows.contains_key(&f.id) {
            return Err("that factor id is taken".into());
        }
        rows.insert(f.id.clone(), f.clone());
        Ok(())
    }

    fn list(&self, principal: &str) -> Result<Vec<Factor>, StoreFault> {
        let mut out: Vec<Factor> = self
            .rows
            .lock()
            .expect("factor store")
            .values()
            .filter(|f| f.principal == principal && f.kind != LOCK_KIND)
            .cloned()
            .collect();
        out.sort_by_key(|f| (f.enrolled_at, f.id.clone()));
        Ok(out)
    }

    fn get(&self, id: &str) -> Result<Option<Factor>, StoreFault> {
        Ok(self.rows.lock().expect("factor store").get(id).cloned())
    }

    fn remove(&self, id: &str, principal: &str) -> Result<Option<Factor>, StoreFault> {
        let mut rows = self.rows.lock().expect("factor store");
        if rows
            .get(id)
            .is_some_and(|f| f.principal == principal && f.kind != LOCK_KIND)
        {
            return Ok(rows.remove(id));
        }
        Ok(None)
    }

    fn remove_all(&self, principal: &str) -> Result<usize, StoreFault> {
        let mut rows = self.rows.lock().expect("factor store");
        let held = rows
            .values()
            .filter(|f| f.principal == principal && f.kind != LOCK_KIND)
            .count();
        rows.retain(|_, f| f.principal != principal);
        Ok(held)
    }

    fn accept_counter(&self, id: &str, step: u64, at: u64) -> Result<bool, StoreFault> {
        let mut rows = self.rows.lock().expect("factor store");
        match rows.get_mut(id) {
            Some(f) if f.last_counter < step => {
                f.last_counter = step;
                f.failures = 0;
                f.last_used_at = at;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn note_success(&self, id: &str, at: u64) -> Result<(), StoreFault> {
        if let Some(f) = self.rows.lock().expect("factor store").get_mut(id) {
            f.failures = 0;
            f.last_used_at = at;
        }
        Ok(())
    }

    fn note_failure(&self, id: &str, lock_after: u64, at: u64) -> Result<(u64, bool), StoreFault> {
        let mut rows = self.rows.lock().expect("factor store");
        let Some(f) = rows.get_mut(id) else {
            return Ok((0, false));
        };
        f.failures += 1;
        if f.failures >= lock_after && f.disabled_at == 0 {
            f.disabled_at = at.max(1);
        }
        Ok((f.failures, f.disabled_at != 0))
    }

    fn ensure_lock_row(&self, principal: &str, at: u64) -> Result<(), StoreFault> {
        let id = lock_row_id(principal);
        self.rows
            .lock()
            .expect("factor store")
            .entry(id.clone())
            .or_insert_with(|| Factor {
                id,
                principal: principal.to_string(),
                kind: LOCK_KIND.to_string(),
                label: String::new(),
                secret: String::new(),
                last_counter: 0,
                failures: 0,
                disabled_at: 0,
                enrolled_at: at,
                last_used_at: 0,
            });
        Ok(())
    }

    fn clear_lock(&self, id: &str) -> Result<(), StoreFault> {
        if let Some(f) = self.rows.lock().expect("factor store").get_mut(id) {
            f.failures = 0;
            f.disabled_at = 0;
        }
        Ok(())
    }

    fn person_mode(&self, principal: &str) -> Result<PersonMode, StoreFault> {
        Ok(self
            .modes
            .lock()
            .expect("factor store")
            .get(principal)
            .copied()
            .unwrap_or_default())
    }

    fn set_person_mode(
        &self,
        principal: &str,
        mode: PersonMode,
        _at: u64,
    ) -> Result<(), StoreFault> {
        self.modes
            .lock()
            .expect("factor store")
            .insert(principal.to_string(), mode);
        Ok(())
    }

    fn chosen_person_mode(&self, principal: &str) -> Result<Option<PersonMode>, StoreFault> {
        Ok(self
            .modes
            .lock()
            .expect("factor store")
            .get(principal)
            .copied())
    }

    fn clear_person_mode(&self, principal: &str) -> Result<(), StoreFault> {
        self.modes.lock().expect("factor store").remove(principal);
        Ok(())
    }
}

pub const FACTOR_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$secondfactor" (
  "id"           text   NOT NULL,
  "principal"    text   NOT NULL,
  "kind"         text   NOT NULL,
  "label"        text   NOT NULL,
  "secret"       text   NOT NULL,
  "last_counter" bigint NOT NULL DEFAULT 0,
  "failures"     bigint NOT NULL DEFAULT 0,
  "disabled_at"  bigint NOT NULL DEFAULT 0,
  "enrolled_at"  bigint NOT NULL,
  "last_used_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("id")
)"#;

/// What each person chose for their own sign-ins. One row per person who
/// chose; the default is what a person who never chose gets.
pub const POLICY_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$secondfactorpolicy" (
  "principal"  text   NOT NULL,
  "mode"       text   NOT NULL,
  "updated_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("principal")
)"#;

pub const FACTOR_INDEX_DDL: &str = r#"CREATE INDEX IF NOT EXISTS "$secondfactor$principal"
    ON "$secondfactor" ("principal")"#;

/// Create `$secondfactor`, retry-once for the reason the other platform
/// tables do.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [FACTOR_DDL, FACTOR_INDEX_DDL, POLICY_DDL] {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// The Postgres factor store.
pub struct PgSecondFactorStore {
    db: PgSync,
}

const COLUMNS: &str = r#""id", "principal", "kind", "label", "secret", "last_counter", "failures", "disabled_at", "enrolled_at", "last_used_at""#;

fn factor_of(row: &ordo_exec::TextRow) -> Option<Factor> {
    let n = |i: usize| cell(row, i).and_then(|s| s.parse::<u64>().ok());
    Some(Factor {
        id: cell(row, 0)?,
        principal: cell(row, 1)?,
        kind: cell(row, 2)?,
        label: cell(row, 3)?,
        secret: cell(row, 4)?,
        last_counter: n(5)?,
        failures: n(6)?,
        disabled_at: n(7)?,
        enrolled_at: n(8)?,
        last_used_at: n(9)?,
    })
}

impl PgSecondFactorStore {
    /// Dial a connection of its own. Refuses when `$secondfactor` is absent.
    pub fn open(cfg: PoolConfig) -> Result<PgSecondFactorStore, StoreFault> {
        PgSecondFactorStore::start(Source::Dial(cfg))
    }

    /// On a connection somebody else owns. Never the request pool.
    pub fn open_on(pool: Pool) -> Result<PgSecondFactorStore, StoreFault> {
        PgSecondFactorStore::start(Source::Shared(pool))
    }

    fn start(source: Source) -> Result<PgSecondFactorStore, StoreFault> {
        let db = PgSync::start(source, "secondfactor")?;
        db.sql(r#"SELECT 1 FROM "$secondfactor" LIMIT 1"#)?;
        Ok(PgSecondFactorStore { db })
    }
}

impl SecondFactorStore for PgSecondFactorStore {
    fn insert(&self, f: &Factor) -> Result<(), StoreFault> {
        self.db.sql(&format!(
            r#"INSERT INTO "$secondfactor" ({COLUMNS}) VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {})"#,
            sql_lit(&f.id),
            sql_lit(&f.principal),
            sql_lit(&f.kind),
            sql_lit(&f.label),
            sql_lit(&f.secret),
            f.last_counter,
            f.failures,
            f.disabled_at,
            f.enrolled_at,
            f.last_used_at,
        ))?;
        Ok(())
    }

    fn list(&self, principal: &str) -> Result<Vec<Factor>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT {COLUMNS} FROM "$secondfactor"
               WHERE "principal" = {} AND "kind" <> {}
               ORDER BY "enrolled_at", "id""#,
            sql_lit(principal),
            sql_lit(LOCK_KIND),
        ))?;
        Ok(rows.iter().filter_map(factor_of).collect())
    }

    fn get(&self, id: &str) -> Result<Option<Factor>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT {COLUMNS} FROM "$secondfactor" WHERE "id" = {}"#,
            sql_lit(id)
        ))?;
        Ok(rows.first().and_then(factor_of))
    }

    fn remove(&self, id: &str, principal: &str) -> Result<Option<Factor>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"DELETE FROM "$secondfactor"
               WHERE "id" = {} AND "principal" = {} AND "kind" <> {}
               RETURNING {COLUMNS}"#,
            sql_lit(id),
            sql_lit(principal),
            sql_lit(LOCK_KIND),
        ))?;
        Ok(rows.first().and_then(factor_of))
    }

    fn remove_all(&self, principal: &str) -> Result<usize, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"WITH d AS (DELETE FROM "$secondfactor" WHERE "principal" = {} RETURNING "kind")
               SELECT COUNT(*) FROM d WHERE "kind" <> {}"#,
            sql_lit(principal),
            sql_lit(LOCK_KIND),
        ))?;
        Ok(rows
            .first()
            .and_then(|r| cell(r, 0))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    fn accept_counter(&self, id: &str, step: u64, at: u64) -> Result<bool, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"UPDATE "$secondfactor"
               SET "last_counter" = {step}, "failures" = 0, "last_used_at" = {at}
               WHERE "id" = {} AND "last_counter" < {step}
               RETURNING 1"#,
            sql_lit(id)
        ))?;
        Ok(!rows.is_empty())
    }

    fn note_success(&self, id: &str, at: u64) -> Result<(), StoreFault> {
        self.db.sql(&format!(
            r#"UPDATE "$secondfactor" SET "failures" = 0, "last_used_at" = {at} WHERE "id" = {}"#,
            sql_lit(id)
        ))?;
        Ok(())
    }

    fn note_failure(&self, id: &str, lock_after: u64, at: u64) -> Result<(u64, bool), StoreFault> {
        let rows = self.db.sql(&format!(
            r#"UPDATE "$secondfactor"
               SET "failures" = "failures" + 1,
                   "disabled_at" = CASE WHEN "disabled_at" = 0 AND "failures" + 1 >= {lock_after}
                                        THEN {} ELSE "disabled_at" END
               WHERE "id" = {}
               RETURNING "failures", "disabled_at""#,
            at.max(1),
            sql_lit(id)
        ))?;
        let Some(row) = rows.first() else {
            return Ok((0, false));
        };
        let failures = cell(row, 0).and_then(|s| s.parse().ok()).unwrap_or(0);
        let locked = cell(row, 1)
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|d| d != 0);
        Ok((failures, locked))
    }

    fn ensure_lock_row(&self, principal: &str, at: u64) -> Result<(), StoreFault> {
        self.db.sql(&format!(
            r#"INSERT INTO "$secondfactor" ({COLUMNS})
               VALUES ({}, {}, {}, '', '', 0, 0, 0, {at}, 0)
               ON CONFLICT ("id") DO NOTHING"#,
            sql_lit(&lock_row_id(principal)),
            sql_lit(principal),
            sql_lit(LOCK_KIND),
        ))?;
        Ok(())
    }

    fn clear_lock(&self, id: &str) -> Result<(), StoreFault> {
        self.db.sql(&format!(
            r#"UPDATE "$secondfactor" SET "failures" = 0, "disabled_at" = 0 WHERE "id" = {}"#,
            sql_lit(id)
        ))?;
        Ok(())
    }

    fn person_mode(&self, principal: &str) -> Result<PersonMode, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT "mode" FROM "$secondfactorpolicy" WHERE "principal" = {}"#,
            sql_lit(principal)
        ))?;
        Ok(rows
            .first()
            .and_then(|r| cell(r, 0))
            .and_then(|m| PersonMode::parse(&m))
            .unwrap_or_default())
    }

    fn set_person_mode(
        &self,
        principal: &str,
        mode: PersonMode,
        at: u64,
    ) -> Result<(), StoreFault> {
        self.db.sql(&format!(
            r#"INSERT INTO "$secondfactorpolicy" ("principal", "mode", "updated_at")
               VALUES ({}, {}, {at})
               ON CONFLICT ("principal") DO UPDATE SET
                 "mode" = EXCLUDED."mode", "updated_at" = EXCLUDED."updated_at""#,
            sql_lit(principal),
            sql_lit(mode.as_str()),
        ))?;
        Ok(())
    }
    fn chosen_person_mode(&self, principal: &str) -> Result<Option<PersonMode>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT "mode" FROM "$secondfactorpolicy" WHERE "principal" = {}"#,
            sql_lit(principal)
        ))?;
        Ok(rows
            .first()
            .and_then(|r| cell(r, 0))
            .and_then(|m| PersonMode::parse(&m)))
    }

    fn clear_person_mode(&self, principal: &str) -> Result<(), StoreFault> {
        self.db.sql(&format!(
            r#"DELETE FROM "$secondfactorpolicy" WHERE "principal" = {}"#,
            sql_lit(principal)
        ))?;
        Ok(())
    }
}
