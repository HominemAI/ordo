//! The sweep of registrations no tenant admitted (`plan-enrolment.md` §2.1,
//! owner ruling F3, E4).
//!
//! The owner: *"it should be registration and they stay there for some x
//! amount of time and then get nuked... this is really up to the app dev."*
//!
//! An identity enrolled at sign-in and admitted to no tenant is kept for
//! `ORDO_AUTH_REGISTRATION_TTL_SECS` and then removed, with its authenticator
//! links, its passkey rows, its password credential and its unredeemed
//! tickets. Admission anywhere before the sweep keeps it.
//!
//! # The default
//!
//! **The default is 90 days**, ruled by the owner on 2026-09-29
//! (`plan-enrolment.md` §6 question 6). The variable overrides it and the
//! startup census reports the value in use. `0` and anything that is not a
//! number refuse the boot.
//!
//! # Never on incomplete knowledge
//!
//! Members live in each tenant's own database. Who is admitted anywhere is
//! the union of `members.principal` over **every** database the deployment
//! serves from. A database that cannot be read makes that union incomplete,
//! and deleting an identity because a database it may be admitted in was
//! down would be the one mistake this cannot take back. So one unreadable
//! database, or one that is reachable and has no `members` table, is treated
//! by the table's absence only when the table truly does not exist, and any
//! read failure ends the pass with nothing removed.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use ordo_exec::route::PoolResolver;
use ordo_exec::{sql_lit, Pool};
use ordo_secevent::Recorder;

use crate::supervisor::{BoxFut, Cadence, Progress, Worker, WorkerCtx, WorkerError};

/// How long a sign-up nobody admits is kept when the deployment names no
/// lifetime: 90 days, the owner's ruling of 2026-09-29.
pub const DEFAULT_TTL_SECS: u64 = 90 * 24 * 60 * 60;

/// `ORDO_AUTH_REGISTRATION_TTL_SECS`, or [`DEFAULT_TTL_SECS`] when the
/// deployment did not set it.
pub fn ttl_from_env() -> Result<u64, String> {
    match std::env::var("ORDO_AUTH_REGISTRATION_TTL_SECS") {
        Err(_) => Ok(DEFAULT_TTL_SECS),
        Ok(v) if v.trim().is_empty() => Ok(DEFAULT_TTL_SECS),
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(format!(
                "`{v}` is not a number of seconds above zero; unset it for the default of {} \
                 days",
                DEFAULT_TTL_SECS / 86_400
            )),
        },
    }
}

/// The census line for the lifetime in use.
pub fn describe_ttl(ttl_secs: u64) -> String {
    let source = if ttl_secs == DEFAULT_TTL_SECS {
        "the default"
    } else {
        "set by ORDO_AUTH_REGISTRATION_TTL_SECS"
    };
    format!(
        "a sign-up no tenant admits is kept {ttl_secs}s ({} days), {source}",
        ttl_secs / 86_400
    )
}

/// What one pass did.
#[derive(Debug, Default)]
pub struct Report {
    /// The principals removed.
    pub removed: Vec<String>,
    /// Why nothing was considered, when the pass ended early.
    pub skipped: Option<String>,
}

pub async fn table_exists(pool: &Pool, name: &str) -> Result<bool, String> {
    let rows = pool
        .rows(&format!(
            "SELECT to_regclass({}) IS NOT NULL",
            sql_lit(&format!("\"{name}\""))
        ))
        .await
        .map_err(|e| e.to_string())?;
    Ok(matches!(
        rows.first().and_then(|r| r[0].as_deref()),
        Some("t") | Some("true")
    ))
}

/// Every principal any tenant has admitted, over every database served.
///
/// **A seat that was left still counts as an admission.** A left seat holds
/// `left:<principal>:<seat>` (`ordo_conformance::Fixture::release_seat`),
/// so the person is read out of it. A person who was admitted and later
/// left every tenant is not a registration no tenant admitted: their
/// account is kept until they delete it, and what happens then is the app
/// developer's removal setting (owner ruling 2026-10-06). Before this, such
/// a person was removed 90 days after they signed up, which could be the
/// day after they left (audit 2026-10-06).
async fn admitted_anywhere(pools: &dyn PoolResolver) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    let targets = pools.targets().await.map_err(|e| e.to_string())?;
    for t in targets {
        let pool = t
            .pool
            .map_err(|e| format!("`{}` could not be reached: {e}", t.label))?;
        if !table_exists(&pool, "members")
            .await
            .map_err(|e| format!("`{}`: {e}", t.label))?
        {
            continue;
        }
        let rows = pool
            .rows(r#"SELECT DISTINCT "principal" FROM "members" WHERE "kind" = 'principal'"#)
            .await
            .map_err(|e| format!("`{}`: {e}", t.label))?;
        out.extend(
            rows.into_iter()
                .filter_map(|r| r[0].clone())
                .map(|p| person_of_seat(&p).to_string()),
        );
    }
    Ok(out)
}

/// The person a `members.principal` value names: the value itself for a
/// live seat, and the id inside `left:<principal>:<seat>` for one that
/// left. A principal id never holds a colon, so the first one ends it. A
/// left value with nobody inside (`left::<seat>`, a seat that named no
/// principal) is returned as it is, which matches no principal.
pub fn person_of_seat(value: &str) -> &str {
    match value.strip_prefix("left:") {
        Some(rest) => match rest.split_once(':') {
            Some((principal, _seat)) if !principal.is_empty() => principal,
            _ => value,
        },
        None => value,
    }
}

/// Which of the tables an identity's removal touches this deployment has.
/// Probed once per pass, because a deployment without passkeys or tickets
/// has no such table and that is not a failure.
pub struct IdentityTables {
    passkeys: bool,
    credentials: bool,
    tickets: bool,
    profiles: bool,
}

impl IdentityTables {
    pub async fn probe(control: &Pool) -> Result<IdentityTables, String> {
        Ok(IdentityTables {
            passkeys: table_exists(control, "$passkey").await?,
            credentials: table_exists(control, "$credential").await?,
            tickets: table_exists(control, "$ticket").await?,
            profiles: table_exists(control, "$identityattr").await?,
        })
    }
}

/// Remove one identity from the control database: its authenticator links,
/// passkey rows, password, unredeemed tickets, managed and temporary-password
/// rows, its sign-in record, and the principal itself. Returns how many links
/// it had. **Only ever called for an identity the caller has decided to
/// destroy**; it decides nothing.
pub async fn remove_identity(
    control: &Pool,
    t: &IdentityTables,
    id: &str,
) -> Result<usize, String> {
    let links = control
        .rows(&format!(
            r#"SELECT "authenticator" FROM "$authenticator" WHERE "principal" = {}"#,
            sql_lit(id)
        ))
        .await
        .map_err(|e| e.to_string())?;
    for link in links.iter().filter_map(|r| r[0].clone()) {
        if t.passkeys && link.starts_with("passkey:") {
            control
                .rows(&format!(
                    r#"DELETE FROM "$passkey" WHERE "authenticator" = {}"#,
                    sql_lit(&link)
                ))
                .await
                .map_err(|e| e.to_string())?;
        }
        if t.credentials {
            if let Some(identifier) = link.strip_prefix("password:") {
                control
                    .rows(&format!(
                        r#"DELETE FROM "$credential" WHERE "identifier" = {}"#,
                        sql_lit(identifier)
                    ))
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    control
        .rows(&format!(
            r#"DELETE FROM "$authenticator" WHERE "principal" = {}"#,
            sql_lit(id)
        ))
        .await
        .map_err(|e| e.to_string())?;
    // What an outside provider vouched for goes with the links it describes,
    // so a swept registration leaves no Discord profile behind.
    if t.profiles {
        control
            .rows(&format!(
                r#"DELETE FROM "$identityattr" WHERE "principal" = {}"#,
                sql_lit(id)
            ))
            .await
            .map_err(|e| e.to_string())?;
    }
    if t.tickets {
        control
            .rows(&format!(
                r#"DELETE FROM "$ticket" WHERE "principal" = {} AND "redeemed_at" IS NULL"#,
                sql_lit(id)
            ))
            .await
            .map_err(|e| e.to_string())?;
    }
    // What E6 keeps beside a principal, if it is there: who manages them,
    // and a temporary password nobody replaced. And E7's sign-in record.
    for table in ["$managed", "$temporarypassword", "$signedin"] {
        if table_exists(control, table).await? {
            control
                .rows(&format!(
                    r#"DELETE FROM "{table}" WHERE "principal" = {}"#,
                    sql_lit(id)
                ))
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    control
        .rows(&format!(
            r#"DELETE FROM "$principal" WHERE "id" = {}"#,
            sql_lit(id)
        ))
        .await
        .map_err(|e| e.to_string())?;
    Ok(links.len())
}

/// One pass. `ttl_secs` is how long a registration is kept; `now` is the
/// clock. Removes each registration older than that which no tenant admitted.
///
/// A deleted account is not a registration and is never swept here: when
/// its data is removed is the app developer's account removal setting
/// ([`crate::account_removal`]), default never (owner ruling 2026-10-06).
pub async fn sweep(
    control: &Pool,
    pools: &dyn PoolResolver,
    ttl_secs: u64,
    now: u64,
    stream: Option<&Recorder>,
) -> Result<Report, String> {
    let cutoff = now.saturating_sub(ttl_secs);
    let rows = control
        .rows(&format!(
            r#"SELECT "id" FROM "$principal" WHERE "created_at" <= {cutoff}
               AND "deleted_at" = 0 ORDER BY "id""#
        ))
        .await
        .map_err(|e| e.to_string())?;
    let candidates: Vec<String> = rows.into_iter().filter_map(|r| r[0].clone()).collect();
    let mut report = Report::default();
    if candidates.is_empty() {
        return Ok(report);
    }

    let admitted = match admitted_anywhere(pools).await {
        Ok(a) => a,
        Err(why) => {
            report.skipped = Some(format!("who is admitted could not be read in full: {why}"));
            return Ok(report);
        }
    };

    let tables = IdentityTables::probe(control).await?;
    for id in candidates.into_iter().filter(|c| !admitted.contains(c)) {
        let links = remove_identity(control, &tables, &id).await?;
        if let Some(stream) = stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "identity-removed",
                    detail: format!(
                        "principal={id} reason=registration-expired ttl_secs={ttl_secs} \
                         links={}",
                        links
                    ),
                    by: None,
                    tenant: None,
                    deployment: true,
                    at: now,
                },
            );
        }
        report.removed.push(id);
    }
    Ok(report)
}

/// One pass over **guests that claimed nothing** (`plan-auth-federated.md`
/// §7A), under the guest lifetime `ORDO_AUTH_GUEST_TTL_SECS`. A guest is a
/// principal whose every link is a guest secret. One that claimed Discord, an
/// address or a passkey is an ordinary account from then on, and only
/// [`sweep`] applies to it. Admission anywhere keeps a guest, as it keeps
/// anyone, and a deleted guest is never swept here.
pub async fn sweep_guests(
    control: &Pool,
    pools: &dyn PoolResolver,
    guest_ttl_secs: u64,
    now: u64,
    stream: Option<&Recorder>,
) -> Result<Report, String> {
    let cutoff = now.saturating_sub(guest_ttl_secs);
    let rows = control
        .rows(&format!(
            r#"SELECT p."id" FROM "$principal" p
               WHERE p."created_at" <= {cutoff} AND p."deleted_at" = 0
                 AND EXISTS (SELECT 1 FROM "$authenticator" a
                             WHERE a."principal" = p."id" AND a."authenticator" LIKE 'guest:%')
                 AND NOT EXISTS (SELECT 1 FROM "$authenticator" a
                             WHERE a."principal" = p."id"
                               AND a."authenticator" NOT LIKE 'guest:%')
               ORDER BY p."id""#
        ))
        .await
        .map_err(|e| e.to_string())?;
    let candidates: Vec<String> = rows.into_iter().filter_map(|r| r[0].clone()).collect();
    let mut report = Report::default();
    if candidates.is_empty() {
        return Ok(report);
    }
    let admitted = match admitted_anywhere(pools).await {
        Ok(a) => a,
        Err(why) => {
            report.skipped = Some(format!("who is admitted could not be read in full: {why}"));
            return Ok(report);
        }
    };
    let tables = IdentityTables::probe(control).await?;
    for id in candidates.into_iter().filter(|c| !admitted.contains(c)) {
        let links = remove_identity(control, &tables, &id).await?;
        if let Some(stream) = stream {
            crate::enrolment::record(
                stream,
                &crate::enrolment::Act {
                    act: "identity-removed",
                    detail: format!(
                        "principal={id} reason=guest-expired ttl_secs={guest_ttl_secs} \
                         links={links}"
                    ),
                    by: None,
                    tenant: None,
                    deployment: true,
                    at: now,
                },
            );
        }
        report.removed.push(id);
    }
    Ok(report)
}

/// The supervised worker: one pass per tick, on the `access.refresh`
/// pattern. **One replica**, because two would each read the same rows and
/// each delete them, which is idempotent and is work the second one wasted.
pub struct RegistrationSweep {
    pub pools: Arc<dyn PoolResolver>,
    pub stream: Arc<Recorder>,
    pub ttl_secs: u64,
    /// `ORDO_AUTH_GUEST_TTL_SECS`: a guest's own lifetime, when set. `None`
    /// keeps a guest as long as any registration.
    pub guest_ttl_secs: Option<u64>,
    pub every: Duration,
}

impl RegistrationSweep {
    pub const NAME: &'static str = "registrations.sweep";

    /// Ten minutes, or the ttl when that is shorter.
    pub fn period_for(ttl_secs: u64) -> Duration {
        Duration::from_secs(600.min(ttl_secs.max(1)))
    }
}

impl Worker for RegistrationSweep {
    fn name(&self) -> &'static str {
        RegistrationSweep::NAME
    }
    fn cadence(&self) -> Cadence {
        Cadence::Tick(self.every)
    }
    fn requires_leadership(&self) -> bool {
        true
    }
    fn run<'a>(&'a self, ctx: &'a WorkerCtx) -> BoxFut<'a, Result<Progress, WorkerError>> {
        Box::pin(async move {
            let report = sweep(
                &ctx.pool,
                self.pools.as_ref(),
                self.ttl_secs,
                ctx.now(),
                Some(&self.stream),
            )
            .await
            .map_err(WorkerError)?;
            let mut report = report;
            if let Some(g) = self.guest_ttl_secs.filter(|g| *g < self.ttl_secs) {
                let guests = sweep_guests(
                    &ctx.pool,
                    self.pools.as_ref(),
                    g,
                    ctx.now(),
                    Some(&self.stream),
                )
                .await
                .map_err(WorkerError)?;
                report.removed.extend(guests.removed);
                report.skipped = report.skipped.or(guests.skipped);
            }
            if let Some(why) = report.skipped {
                // Not an error: the pass took the safe direction. Named
                // once per tick so an operator can see it is happening.
                eprintln!("registrations.sweep: nothing removed this pass, {why}");
            }
            Ok(if report.removed.is_empty() {
                Progress::idle()
            } else {
                Progress::did(report.removed.len() as u64)
            })
        })
    }
}
