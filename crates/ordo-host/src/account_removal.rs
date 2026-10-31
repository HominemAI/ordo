//! A deleted account, marked and kept, and when its personal data is removed
//! (owner ruling 2026-10-06, person-can-leave ask 11).
//!
//! The owner: *"account should be marked deleted but not removed ... and
//! then dev can decide if they get removed automatically, and they'll have
//! the ability to decide how that information is presented always."*
//!
//! # What deleting an account does now
//!
//! `POST /auth/account/delete` ([`crate::auth::account`]) ends every session
//! and **marks** the identity deleted (`$principal.deleted_at`). A marked
//! identity never signs in again: the directory refuses it on sign-in,
//! step-up and recovery (`ordo_identity::DirectoryError::Deleted`). Its
//! links stay, so its sign-in identifiers name it and cannot be used for
//! anyone else. Its data stays. The act is `identity-deleted` on the
//! deployment's stream.
//!
//! The person's `left` seats in every tenant become `deleted`
//! ([`mark_seats`]), each through the write protocol. Their name stays; the
//! app reads the status and decides how the seat is shown.
//!
//! # When the data is removed: the app developer's setting
//!
//! `ORDO_ACCOUNT_REMOVAL`, read by [`Removal::from_env`]:
//!
//! | Value | What happens |
//! |---|---|
//! | `never` (the default) | the data stays |
//! | `now` | removed in the same request |
//! | `<n>d`, or `<n>` | removed by the `accounts.removal` worker once `n` days have passed |
//!
//! Removal is [`crate::identity_removal::remove`], the path built in 27aq
//! R6, recorded as `identity-removed`.
//!
//! # The setting in force decides, at each pass
//!
//! Accounts already deleted follow the current setting (owner ruling
//! 2026-10-06, person-can-leave ask 12). Nothing is scheduled when an account
//! is deleted. The worker reads `$principal.deleted_at` and works out each
//! account's due time from the setting at that pass: `deleted_at + n days`
//! under `<n>d`, every deleted account under `now`, none under `never`. So
//! shortening the days, or changing to `now`, takes hold at the next boot
//! for accounts already waiting, and an account deleted under `never` is
//! removed once the setting becomes `<n>d` and the time has passed. Removal
//! deletes the `$principal` row, so a pass that runs twice finds nothing the
//! second time.

use std::sync::Arc;
use std::time::Duration;

use ordo_exec::{sql_lit, Pool};
use ordo_secevent::Recorder;

use crate::release_ops::ReleaseOps;
use crate::supervisor::{BoxFut, Cadence, Progress, Worker, WorkerCtx, WorkerError};

/// The app developer's setting.
pub const REMOVAL_ENV: &str = "ORDO_ACCOUNT_REMOVAL";

const DAY_SECS: u64 = 86_400;

/// Whether, and when, a deleted account's personal data is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Removal {
    /// Kept. The default the owner ruled.
    #[default]
    Never,
    /// Removed in the request that deletes the account.
    Now,
    /// Removed by the worker once this many days have passed.
    AfterDays(u64),
}

impl Removal {
    /// Read [`REMOVAL_ENV`]. A value that is not understood refuses the boot
    /// rather than being read as the default.
    pub fn from_env() -> Result<Removal, String> {
        Removal::parse(std::env::var(REMOVAL_ENV).ok().as_deref())
    }

    /// [`Removal::from_env`] over a given value.
    pub fn parse(value: Option<&str>) -> Result<Removal, String> {
        let Some(v) = value
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
        else {
            return Ok(Removal::Never);
        };
        match v.as_str() {
            "never" => Ok(Removal::Never),
            "now" => Ok(Removal::Now),
            other => match other.strip_suffix('d').unwrap_or(other).parse::<u64>() {
                Ok(n) if n > 0 => Ok(Removal::AfterDays(n)),
                _ => Err(format!(
                    "`{other}` is `never`, `now`, or a number of days above zero such as `30d`"
                )),
            },
        }
    }

    /// The word the answer and the stream carry.
    pub fn word(&self) -> &'static str {
        match self {
            Removal::Never => "never",
            Removal::Now => "now",
            Removal::AfterDays(_) => "after",
        }
    }

    /// When an account deleted at `at` is due for removal, if ever.
    pub fn due(&self, at: u64) -> Option<u64> {
        match self {
            Removal::Never => None,
            Removal::Now => Some(at),
            Removal::AfterDays(n) => Some(at.saturating_add(n.saturating_mul(DAY_SECS))),
        }
    }

    /// The census line.
    pub fn describe(&self) -> String {
        match self {
            Removal::Never => "a deleted account's data is kept (the default)".to_string(),
            Removal::Now => {
                format!("a deleted account's data is removed at once, set by {REMOVAL_ENV}")
            }
            Removal::AfterDays(n) => {
                format!("a deleted account's data is removed after {n} days, set by {REMOVAL_ENV}")
            }
        }
    }
}

/// Remove the data of `principal` now, and record it. Returns the
/// correlation of the `identity-removed` entry.
pub async fn remove_now(
    control: &Pool,
    stream: Option<&Recorder>,
    principal: &str,
    reason: &str,
    at: u64,
) -> Result<String, String> {
    crate::identity_removal::remove(control, principal).await?;
    Ok(crate::identity_removal::record(
        stream,
        "identity-removed",
        principal,
        reason,
        None,
        principal,
        at,
    ))
}

/// What [`mark_seats`] did.
#[derive(Debug, Default)]
pub struct Seats {
    /// Seats moved from `left` to `deleted`.
    pub marked: usize,
    /// Databases or tenants that could not be read or written. Named so the
    /// answer says where the mark is missing; nothing else depends on it.
    pub unreached: Vec<String>,
}

/// Mark every `left` seat of `principal`, in every tenant, `deleted`.
///
/// A left seat holds `left:<principal>:<seat>` (see
/// `ordo_conformance::Fixture::release_seat`), so the person's seats are
/// found over every database the deployment serves from, the way the
/// registration sweep reads who is admitted. Each mark is one converge
/// through the write protocol into its tenant's chain
/// (`Fixture::run_seat_deleted_on`). A seat already marked is skipped, so a
/// second pass changes nothing.
pub async fn mark_seats(release: &ReleaseOps, principal: &str) -> Seats {
    let mut out = Seats::default();
    let prefix = format!("left:{principal}:");
    let targets = match release.members.pools.targets().await {
        Ok(t) => t,
        Err(e) => {
            out.unreached
                .push(format!("the databases could not be listed: {e}"));
            return out;
        }
    };
    let mut found: Vec<(String, String)> = Vec::new();
    for t in targets {
        let pool = match t.pool {
            Ok(p) => p,
            Err(_) => {
                out.unreached.push(t.label.clone());
                continue;
            }
        };
        match crate::registrations::table_exists(&pool, "members").await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => {
                out.unreached.push(t.label.clone());
                continue;
            }
        }
        match pool
            .rows(&format!(
                r#"SELECT "$space", "id" FROM "members"
                   WHERE "kind" = 'principal' AND "status" = 'left'
                     AND starts_with("principal", {})
                   ORDER BY "$space", "id""#,
                sql_lit(&prefix)
            ))
            .await
        {
            Ok(rows) => found.extend(
                rows.into_iter()
                    .filter_map(|r| Some((r.first()?.clone()?, r.get(1)?.clone()?))),
            ),
            Err(_) => out.unreached.push(t.label.clone()),
        }
    }
    for (space, seat) in found {
        let bound = match release.spaces.resolve(&space).await {
            Ok(b) => b,
            Err(_) => {
                out.unreached.push(space);
                continue;
            }
        };
        let f = bound.view.fixture.clone();
        let seat2 = seat.clone();
        let written = bound
            .view
            .exec
            .platform_write(bound.log.clone(), move |db, log| {
                f.run_seat_deleted_on(db, log, &seat2)
            })
            .await;
        match written {
            Ok(v) if !v.is_null() && v.as_array().is_some_and(|a| !a.is_empty()) => {
                out.marked += 1;
                release.members.note(
                    &space,
                    &seat,
                    "member-deleted",
                    format!("member={seat} reason=account-deleted"),
                );
            }
            Ok(_) => {}
            Err(_) => out.unreached.push(space),
        }
    }
    out
}

/// What one removal pass did.
#[derive(Debug, Default)]
pub struct Report {
    pub removed: Vec<String>,
}

/// How many removals one pass makes. The rest are the next pass's, which
/// `Progress::more` asks for at once.
pub const SWEEP_BATCH: i64 = 32;

/// One pass: remove every deleted account that is due under `removal`, the
/// setting in force, oldest deletion first, at most [`SWEEP_BATCH`]. The due
/// time is worked out here from `$principal.deleted_at`, so a changed
/// setting applies to accounts deleted earlier. `Never` removes no one. A
/// removal that fails ends the pass; the account stays marked and the next
/// pass tries again.
pub async fn sweep(
    control: &Pool,
    stream: Option<&Recorder>,
    removal: Removal,
    now: u64,
) -> Result<Report, String> {
    let mut report = Report::default();
    let cutoff = match removal {
        Removal::Never => return Ok(report),
        Removal::Now => now,
        Removal::AfterDays(n) => match now.checked_sub(n.saturating_mul(DAY_SECS)) {
            Some(c) => c,
            None => return Ok(report),
        },
    };
    if !crate::registrations::table_exists(control, "$principal").await? {
        return Ok(report);
    }
    let rows = control
        .rows(&format!(
            r#"SELECT "id" FROM "$principal"
               WHERE "deleted_at" > 0 AND "deleted_at" <= {cutoff}
               ORDER BY "deleted_at", "id" LIMIT {SWEEP_BATCH}"#
        ))
        .await
        .map_err(|e| e.to_string())?;
    for p in rows
        .into_iter()
        .filter_map(|r| r.first().cloned().flatten())
    {
        remove_now(control, stream, &p, "deleted-account-due", now).await?;
        report.removed.push(p);
    }
    Ok(report)
}

/// The supervised worker for `ORDO_ACCOUNT_REMOVAL=<n>d` and `now` (under
/// `now` its first pass at boot removes everyone still waiting; the delete
/// route removes at once). Under `never` it is not started. **One replica**,
/// for the registration sweep's reason: two would each remove the same
/// rows, which is idempotent and wasted.
pub struct AccountRemovalSweep {
    pub stream: Arc<Recorder>,
    pub every: Duration,
    /// The setting in force at boot.
    pub removal: Removal,
}

impl AccountRemovalSweep {
    pub const NAME: &'static str = "accounts.removal";
    pub const DEFAULT_SECS: u64 = 600;
}

impl Worker for AccountRemovalSweep {
    fn name(&self) -> &'static str {
        AccountRemovalSweep::NAME
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let report = sweep(&ctx.pool, Some(&self.stream), self.removal, ctx.now())
                .await
                .map_err(WorkerError)?;
            let n = report.removed.len();
            Ok(Progress {
                units: n as u64,
                more: n as i64 == SWEEP_BATCH,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_removal_setting_reads_never_now_and_days() {
        assert_eq!(Removal::parse(None), Ok(Removal::Never));
        assert_eq!(Removal::parse(Some(" ")), Ok(Removal::Never));
        assert_eq!(Removal::parse(Some("never")), Ok(Removal::Never));
        assert_eq!(Removal::parse(Some("NOW")), Ok(Removal::Now));
        assert_eq!(Removal::parse(Some("30d")), Ok(Removal::AfterDays(30)));
        assert_eq!(Removal::parse(Some("7")), Ok(Removal::AfterDays(7)));
        assert!(Removal::parse(Some("0d")).is_err());
        assert!(Removal::parse(Some("soon")).is_err());
        assert_eq!(Removal::AfterDays(2).due(100), Some(100 + 2 * DAY_SECS));
        assert_eq!(Removal::Never.due(100), None);
    }
}
