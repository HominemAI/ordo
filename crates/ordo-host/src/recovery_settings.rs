//! What a contest puts back: a person's own sign-in settings (owner ruling
//! 2026-10-25, board `after-a-contest-what-comes-back`: a contest "resets
//! sign-in settings the thief changed during the case to their value when it
//! opened").
//!
//! Whoever holds a recovered passkey may change how the person signs in or is
//! reached, for example ask for a second step after a passkey too, or move the
//! contact email. A contest that removed the thief's factors and left those
//! would hand the owner back a sign-in the thief shaped.
//!
//! # One place
//!
//! [`RestoreSettings`] is a [`Settlement`]. When a case opens it notes each
//! setting in [`PersonSetting`] as the person has it. When the case is
//! contested it reads each one again; where the value differs from the note,
//! it writes the noted value back. A setting the person did not have is noted
//! as absent and goes back to absent. Each reset is returned to the recovery
//! service, which records it (`contest-reset`: kind, old and new value).
//!
//! # The settings
//!
//! | Kind | Stored in | Words in the record |
//! |---|---|---|
//! | `second-step-level` | `$secondfactorpolicy` | `when-enrolled`, `also-for-passkeys`, `none` for a person who never chose |
//! | `contact-email` | `$contactemail` | `none`, or `address(<source>)` |
//!
//! The record never carries an address. The note holds the address, in a table
//! of the control database beside `$contactemail`, until it is pruned.
//!
//! Remember-me and the session lifetimes are the app developer's and the
//! tenant's settings, not a person's, so a recovered passkey cannot change
//! them. A new setting a person can change is added by implementing
//! [`PersonSetting`] and listing it where the service is wired.
//!
//! # Cases that opened before this existed
//!
//! A case with no note resets nothing: there is no value to go back to.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ordo_exec::sql_lit;
use ordo_identity::StoreFault;

use crate::contact_email::{ContactEmails, Stored};
use crate::pgsync::{cell, PgSync, Source};
use crate::recovery::{SettingReset, Settlement};
use crate::second_factor_store::SecondFactorStore;
use ordo_identity::second_step::PersonMode;

/// One setting a signed-in person can change about how they sign in or are
/// reached.
pub trait PersonSetting: Send + Sync {
    /// The word for the record.
    fn kind(&self) -> &'static str;
    /// The person's value now, in a form [`write`](PersonSetting::write)
    /// takes back. `None` when the person has none.
    fn read(&self, principal: &str) -> Result<Option<String>, StoreFault>;
    /// Put the value back, or remove it (`None`).
    fn write(&self, principal: &str, value: Option<&str>) -> Result<(), StoreFault>;
    /// The value as words for the record: never a secret, never an address.
    fn show(&self, value: Option<&str>) -> String;
}

/// The person's own second-step level (`when-enrolled`, `also-for-passkeys`).
pub struct SecondStepLevel(pub Arc<dyn SecondFactorStore>);

impl PersonSetting for SecondStepLevel {
    fn kind(&self) -> &'static str {
        "second-step-level"
    }

    fn read(&self, principal: &str) -> Result<Option<String>, StoreFault> {
        Ok(self
            .0
            .chosen_person_mode(principal)?
            .map(|m| m.as_str().to_string()))
    }

    fn write(&self, principal: &str, value: Option<&str>) -> Result<(), StoreFault> {
        match value.and_then(PersonMode::parse) {
            Some(mode) => self
                .0
                .set_person_mode(principal, mode, crate::runtime::now_secs()),
            None => self.0.clear_person_mode(principal),
        }
    }

    fn show(&self, value: Option<&str>) -> String {
        value.unwrap_or("none").to_string()
    }
}

/// The verified address the person added.
pub struct ContactEmailSetting {
    pub emails: Arc<ContactEmails>,
    /// The runtime the store is on. A settlement is synchronous and runs on a
    /// blocking thread, so it drives the async store through this.
    pub runtime: tokio::runtime::Handle,
}

/// The stored form: address, source, when verified, as JSON.
fn encode(s: &Stored) -> String {
    serde_json::json!({"a": s.address, "s": s.source, "v": s.verified_at}).to_string()
}

fn decode(text: &str) -> Option<Stored> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    Some(Stored {
        address: v.get("a")?.as_str()?.to_string(),
        source: v.get("s")?.as_str()?.to_string(),
        verified_at: v.get("v")?.as_u64()?,
    })
}

impl ContactEmailSetting {
    /// Drive the async store from a synchronous settlement. A case is opened
    /// from a blocking thread by the routes and from a runtime thread by
    /// other callers, so it steps off the runtime thread first.
    fn block_on<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        tokio::task::block_in_place(|| self.runtime.block_on(f))
    }
}

impl PersonSetting for ContactEmailSetting {
    fn kind(&self) -> &'static str {
        "contact-email"
    }

    fn read(&self, principal: &str) -> Result<Option<String>, StoreFault> {
        Ok(self
            .block_on(self.emails.added_address(principal))?
            .as_ref()
            .map(encode))
    }

    fn write(&self, principal: &str, value: Option<&str>) -> Result<(), StoreFault> {
        let to = value.and_then(decode);
        self.block_on(self.emails.restore(principal, to.as_ref()))
    }

    fn show(&self, value: Option<&str>) -> String {
        match value.and_then(decode) {
            Some(s) => format!("address({})", s.source),
            None => "none".to_string(),
        }
    }
}

/// Where the notes are kept: per case, one value per setting kind. A `None`
/// value is a setting the person did not have.
pub trait SnapshotStore: Send + Sync {
    fn put(
        &self,
        case: &str,
        principal: &str,
        kind: &str,
        value: Option<&str>,
        at: u64,
    ) -> Result<(), StoreFault>;
    /// `None`: no note. `Some(None)`: noted as absent.
    fn get(&self, case: &str, kind: &str) -> Result<Option<Option<String>>, StoreFault>;
    /// Drop notes taken before `before`.
    fn prune(&self, before: u64) -> Result<(), StoreFault>;
}

/// A noted value (null: the person had none) and when it was taken.
type Note = (Option<String>, u64);

/// The process-local store, for tests.
#[derive(Default)]
pub struct MemorySnapshots {
    rows: Mutex<BTreeMap<(String, String), Note>>,
}

impl MemorySnapshots {
    pub fn new() -> MemorySnapshots {
        MemorySnapshots::default()
    }
}

impl SnapshotStore for MemorySnapshots {
    fn put(
        &self,
        case: &str,
        _principal: &str,
        kind: &str,
        value: Option<&str>,
        at: u64,
    ) -> Result<(), StoreFault> {
        self.rows.lock().expect("snapshots").insert(
            (case.to_string(), kind.to_string()),
            (value.map(str::to_string), at),
        );
        Ok(())
    }

    fn get(&self, case: &str, kind: &str) -> Result<Option<Option<String>>, StoreFault> {
        Ok(self
            .rows
            .lock()
            .expect("snapshots")
            .get(&(case.to_string(), kind.to_string()))
            .map(|(v, _)| v.clone()))
    }

    fn prune(&self, before: u64) -> Result<(), StoreFault> {
        self.rows
            .lock()
            .expect("snapshots")
            .retain(|_, (_, at)| *at >= before);
        Ok(())
    }
}

/// The Postgres store: `$recoverysnapshot`.
pub struct PgSnapshots {
    db: PgSync,
}

impl PgSnapshots {
    /// On a connection somebody else owns. Never the request pool.
    pub fn open_on(pool: ordo_exec::Pool) -> Result<PgSnapshots, StoreFault> {
        let db = PgSync::start(Source::Shared(pool), "recoverysnapshot")?;
        db.sql(r#"SELECT 1 FROM "$recoverysnapshot" LIMIT 1"#)?;
        Ok(PgSnapshots { db })
    }
}

impl SnapshotStore for PgSnapshots {
    fn put(
        &self,
        case: &str,
        principal: &str,
        kind: &str,
        value: Option<&str>,
        at: u64,
    ) -> Result<(), StoreFault> {
        let value = value.map(sql_lit).unwrap_or_else(|| "NULL".to_string());
        self.db.sql(&format!(
            r#"INSERT INTO "$recoverysnapshot" ("case", "kind", "principal", "value", "taken_at")
               VALUES ({}, {}, {}, {value}, {at})
               ON CONFLICT ("case", "kind") DO NOTHING"#,
            sql_lit(case),
            sql_lit(kind),
            sql_lit(principal),
        ))?;
        Ok(())
    }

    fn get(&self, case: &str, kind: &str) -> Result<Option<Option<String>>, StoreFault> {
        let rows = self.db.sql(&format!(
            r#"SELECT "value" FROM "$recoverysnapshot" WHERE "case" = {} AND "kind" = {}"#,
            sql_lit(case),
            sql_lit(kind),
        ))?;
        Ok(rows.first().map(|r| cell(r, 0)))
    }

    fn prune(&self, before: u64) -> Result<(), StoreFault> {
        self.db.sql(&format!(
            r#"DELETE FROM "$recoverysnapshot" WHERE "taken_at" < {before}"#
        ))?;
        Ok(())
    }
}

/// The settlement: note the settings when a case opens, put back the ones
/// changed when it is contested.
pub struct RestoreSettings {
    pub snapshots: Arc<dyn SnapshotStore>,
    pub settings: Vec<Arc<dyn PersonSetting>>,
    /// How long a note is kept. Longer than a case can last: its delay and
    /// its contest window.
    pub keep_secs: u64,
}

impl Settlement for RestoreSettings {
    fn name(&self) -> &'static str {
        "restore-settings"
    }

    /// Nothing happens when a recovery completes: the settings the thief
    /// changes come after it.
    fn settle(&self, _principal: &str, _case: &str) -> Result<(), StoreFault> {
        Ok(())
    }

    fn opened(&self, principal: &str, case: &str, at: u64) -> Result<(), StoreFault> {
        self.snapshots.prune(at.saturating_sub(self.keep_secs))?;
        for s in &self.settings {
            let now = s.read(principal)?;
            self.snapshots
                .put(case, principal, s.kind(), now.as_deref(), at)?;
        }
        Ok(())
    }

    fn reset(
        &self,
        principal: &str,
        case: &str,
        _opened: u64,
    ) -> Result<Vec<SettingReset>, StoreFault> {
        let mut done = Vec::new();
        for s in &self.settings {
            let Some(was) = self.snapshots.get(case, s.kind())? else {
                continue;
            };
            let now = s.read(principal)?;
            if now == was {
                continue;
            }
            s.write(principal, was.as_deref())?;
            done.push(SettingReset {
                kind: s.kind().to_string(),
                old: s.show(now.as_deref()),
                new: s.show(was.as_deref()),
            });
        }
        Ok(done)
    }
}
