//! When a sealed value last opened, per key epoch.
//!
//! **What it reads.** One entry per space and key epoch, kept by the metered
//! key provider ([`crate::metrics::MeteredKeys`]) from the answer of each
//! `open_at` and `open_chunk`: the time of the last open that succeeded
//! under that epoch, and, since that success, the time of the first and the
//! last open that failed with the key present (`KeyError::Seal`, "unreadable,
//! possibly tampered"). A missing epoch (a deliberately erased key) is not a
//! failure here. Nothing is read from the sealed values themselves.
//!
//! It is a debugging aid (owner, 2026-10-23: "a way to see that last
//! successful decrypt could be useful for debug"). It reads, for example,
//! "worked until 14:02, failing since 14:03 after the restart", which is
//! what a wrong master key mounted after a restart looks like.
//!
//! **Cost.** A read updates the entry in memory and writes nothing. The
//! entries that changed are written to `$decrypt_seen` in the control
//! database at most once per interval (`ORDO_DECRYPT_SEEN_SECS`, default
//! 60) and once more at graceful shutdown, and are loaded at boot, so the
//! time survives a restart. A crash loses at most one interval. The table
//! is a report and never an authority: nothing routes or refuses on it.
//! The row is per space and epoch, not per node, because a node's name can
//! change across a restart and the question is about the key. With several
//! nodes the later write of `failing_since` wins and `last_success` is the
//! greatest seen.
//!
//! It also rate-limits the alert: one security event per space, entity,
//! field and epoch per [`ALERT_WINDOW_SECS`], so a corrupted table does not
//! flood the stream. The counter is not limited.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use ordo_exec::{sql_lit, Pool, PoolError};

/// The setting for how often changed entries are written.
pub const FLUSH_SETTING: &str = "ORDO_DECRYPT_SEEN_SECS";
pub const DEFAULT_FLUSH_SECS: u64 = 60;

/// One alert per space, entity, field and epoch in this many seconds.
pub const ALERT_WINDOW_SECS: u64 = 300;

pub const TABLE: &str = "$decrypt_seen";

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$decrypt_seen\" ( \
  \"space\" text NOT NULL, \
  \"epoch\" integer NOT NULL, \
  \"last_ok\" bigint NOT NULL, \
  \"failing_since\" bigint NOT NULL, \
  \"last_fail\" bigint NOT NULL, \
  PRIMARY KEY (\"space\", \"epoch\") \
)";

/// What is known about one space's key epoch. Seconds since the epoch of
/// the clock; `0` is "never".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Seen {
    pub last_success: u64,
    /// The first failure since the last success, or `0` when the last
    /// open succeeded.
    pub failing_since: u64,
    pub last_failure: u64,
}

#[derive(Default)]
struct State {
    seen: BTreeMap<(String, u8), (Seen, bool)>,
    alerted: BTreeMap<(String, String, String, u8), u64>,
}

#[derive(Default)]
pub struct DecryptLedger {
    state: Mutex<State>,
}

impl DecryptLedger {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// An open under `epoch` succeeded.
    pub fn succeeded(&self, space: &str, epoch: u8, now: u64) {
        let mut st = self.lock();
        let (s, dirty) = st.seen.entry((space.to_string(), epoch)).or_default();
        s.last_success = now;
        s.failing_since = 0;
        *dirty = true;
    }

    /// An open under `epoch` failed with its key present.
    pub fn failed(&self, space: &str, epoch: u8, now: u64) {
        let mut st = self.lock();
        let (s, dirty) = st.seen.entry((space.to_string(), epoch)).or_default();
        if s.failing_since == 0 {
            s.failing_since = now;
        }
        s.last_failure = now;
        *dirty = true;
    }

    /// May an alert for this space, entity, field and epoch be recorded
    /// now. True at most once per [`ALERT_WINDOW_SECS`].
    pub fn alert_due(&self, space: &str, entity: &str, field: &str, epoch: u8, now: u64) -> bool {
        let mut st = self.lock();
        if st.alerted.len() > 4096 {
            st.alerted
                .retain(|_, at| now.saturating_sub(*at) < ALERT_WINDOW_SECS);
        }
        let key = (
            space.to_string(),
            entity.to_string(),
            field.to_string(),
            epoch,
        );
        match st.alerted.get(&key) {
            Some(at) if now.saturating_sub(*at) < ALERT_WINDOW_SECS => false,
            _ => {
                st.alerted.insert(key, now);
                true
            }
        }
    }

    /// Every entry, in space then epoch order.
    pub fn snapshot(&self) -> Vec<(String, u8, Seen)> {
        self.lock()
            .seen
            .iter()
            .map(|((sp, ep), (s, _))| (sp.clone(), *ep, *s))
            .collect()
    }

    /// The `/health/deep` document.
    pub fn report(&self) -> serde_json::Value {
        let at = |t: u64| (t > 0).then_some(t);
        serde_json::json!({
            "reads": "the last time a sealed value opened under this key epoch, \
                      and the first and last time one failed to open with its key \
                      present, since that success",
            "epochs": self.snapshot().into_iter().map(|(space, epoch, s)| serde_json::json!({
                "space": space,
                "epoch": epoch,
                "last_success_at": at(s.last_success),
                "failing_since": at(s.failing_since),
                "last_failure_at": at(s.last_failure),
            })).collect::<Vec<_>>(),
        })
    }

    pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
        if pool.rows(DDL).await.is_err() {
            pool.rows(DDL).await?;
        }
        Ok(())
    }

    /// Read the stored entries into memory, so the times from before a
    /// restart show. An absent table is no entries.
    pub async fn load(&self, pool: &Pool) -> Result<(), PoolError> {
        let rows = match pool
            .rows(&format!(
                "SELECT \"space\", \"epoch\", \"last_ok\", \"failing_since\", \"last_fail\" \
                 FROM \"{TABLE}\""
            ))
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_missing_table() => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut st = self.lock();
        for row in rows {
            let cell = |i: usize| row.get(i).and_then(|c| c.as_deref()).unwrap_or_default();
            let (Ok(epoch), Ok(ok), Ok(fs), Ok(lf)) = (
                cell(1).parse::<u8>(),
                cell(2).parse::<u64>(),
                cell(3).parse::<u64>(),
                cell(4).parse::<u64>(),
            ) else {
                continue;
            };
            let entry = st.seen.entry((cell(0).to_string(), epoch)).or_default();
            if entry.0 == Seen::default() {
                entry.0 = Seen {
                    last_success: ok,
                    failing_since: fs,
                    last_failure: lf,
                };
            } else {
                entry.0.last_success = entry.0.last_success.max(ok);
            }
        }
        Ok(())
    }

    /// Write the entries that changed since the last flush. A failure is a
    /// log line and leaves them changed, to be written next time.
    pub async fn flush(&self, pool: &Pool) {
        let changed: Vec<(String, u8, Seen)> = {
            let mut st = self.lock();
            st.seen
                .iter_mut()
                .filter(|(_, (_, dirty))| *dirty)
                .map(|((sp, ep), (s, dirty))| {
                    *dirty = false;
                    (sp.clone(), *ep, *s)
                })
                .collect()
        };
        for (space, epoch, s) in changed {
            let sql = format!(
                "INSERT INTO \"{TABLE}\" (\"space\", \"epoch\", \"last_ok\", \"failing_since\", \
                   \"last_fail\") VALUES ({}, {epoch}, {}, {}, {}) \
                 ON CONFLICT (\"space\", \"epoch\") DO UPDATE SET \
                   \"last_ok\" = GREATEST(\"{TABLE}\".\"last_ok\", EXCLUDED.\"last_ok\"), \
                   \"failing_since\" = EXCLUDED.\"failing_since\", \
                   \"last_fail\" = GREATEST(\"{TABLE}\".\"last_fail\", EXCLUDED.\"last_fail\")",
                sql_lit(&space),
                s.last_success,
                s.failing_since,
                s.last_failure,
            );
            let mut out = pool.rows(&sql).await;
            if out.is_err() && Self::ensure_table(pool).await.is_ok() {
                out = pool.rows(&sql).await;
            }
            if let Err(e) = out {
                eprintln!("decrypt times: not written for space `{space}` epoch {epoch} ({e})");
                if let Some(entry) = self.lock().seen.get_mut(&(space, epoch)) {
                    entry.1 = true;
                }
            }
        }
    }

    /// Flush every `every` until the task is aborted.
    pub fn spawn_flusher(
        self: &std::sync::Arc<Self>,
        pool: Pool,
        every: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                me.flush(&pool).await;
            }
        })
    }
}
