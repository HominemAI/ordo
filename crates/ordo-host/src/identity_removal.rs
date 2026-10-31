//! Removing an identity, one code path (checklist 27aq R6,
//! `plan-member-release.md` §5.6).
//!
//! Three acts end a person's existence in the deployment's directory: their
//! sessions end, what names them in the control database is deleted, and the
//! act is recorded. Tenant deletion does them for the people a tenant made
//! (reasons `tenant-provider` and `never-signed-in`). A person deleting their
//! own account is marked deleted and kept (owner ruling 2026-10-06); their
//! data comes here only when the app developer's removal setting says so
//! (`crate::account_removal`: reason `self` at once, `deleted-account-due`
//! from the worker). Every removal calls here, so none can come to disagree
//! about what "removed" means.
//!
//! # What stays, and why that is the erasure
//!
//! The chain's actor ids stay. `user:usr_…` was minted at random and, with
//! `$principal` gone, it maps to nobody (owner ruling 2026-10-05, ask 10).
//! The security stream keeps the record that the removal happened. Neither is
//! rewritten.

use std::sync::Arc;

use ordo_exec::{sql_lit, Pool};
use ordo_identity::principal::{PrincipalId, Timestamp};
use ordo_identity::session::SessionRevocations;
use ordo_secevent::Recorder;

use crate::registrations::{remove_identity, table_exists, IdentityTables};

/// Tables that keep something about a person beside the ones
/// [`remove_identity`] clears, with the columns that name a person. Each
/// is deleted from by every column listed when the deployment has the
/// table.
///
/// The columns are named here rather than assumed to be `principal`,
/// because two of these tables name a person another way: a delegation
/// has a `delegator` and an `agent`, and a recovery approval has an
/// `approver`. An earlier shape of this list deleted by `principal` alone
/// and skipped a table without that column in silence, so a deleted
/// person's delegations and approvals stayed (audit 2026-10-05d).
///
const PERSONAL_TABLES: [(&str, &[&str]); 5] = [
    ("$recoverycode", &["principal"]),
    ("$recovery", &["principal"]),
    ("$recoveryapproval", &["approver"]),
    ("$secondfactor", &["principal"]),
    ("$delegation", &["delegator", "agent"]),
];

/// End every session `person` holds. `None` revocations is a deployment
/// with no session deny list, where there is nothing to end.
pub async fn end_sessions(
    revocations: Option<Arc<dyn SessionRevocations>>,
    person: &str,
    at: u64,
    session_max_secs: u64,
) -> Result<(), String> {
    let Some(store) = revocations else {
        return Ok(());
    };
    let who = PrincipalId(person.to_string());
    let until = Timestamp(at.saturating_add(session_max_secs));
    tokio::task::spawn_blocking(move || store.end_all(&who, Timestamp(at), until))
        .await
        .map_err(|_| "ending the sessions did not finish".to_string())?
        .map_err(|e| e.to_string())
}

/// Delete everything in the control database that names `person`: links,
/// passkeys, password, unredeemed tickets, managed and sign-in records, the
/// principal, and the personal tables above. Returns how many authenticator
/// links the person had.
pub async fn remove(control: &Pool, person: &str) -> Result<usize, String> {
    let tables = IdentityTables::probe(control).await?;
    let links = remove_identity(control, &tables, person).await?;
    for (table, columns) in PERSONAL_TABLES {
        if !table_exists(control, table).await? {
            continue;
        }
        for column in columns {
            // A listed column the table lacks is a wiring error, and it is
            // answered as one: the account is not reported deleted while a
            // row that names the person may still be there.
            let has = control
                .rows(&format!(
                    "SELECT 1 FROM information_schema.columns \
                     WHERE table_name = {} AND column_name = {}",
                    sql_lit(table),
                    sql_lit(column)
                ))
                .await
                .map_err(|e| e.to_string())?;
            if has.is_empty() {
                return Err(format!(
                    "removing an identity: `{table}` has no `{column}` column to delete by"
                ));
            }
            control
                .rows(&format!(
                    r#"DELETE FROM "{table}" WHERE "{column}" = {}"#,
                    sql_lit(person)
                ))
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(links)
}

/// The record, at deployment scope: `identity-removed` or `identity-released`
/// with the reason, the person and who acted. Returns the correlation id the
/// entry carries, which is what a receipt names.
pub fn record(
    stream: Option<&Recorder>,
    act: &str,
    person: &str,
    reason: &str,
    tenant: Option<&str>,
    by: &str,
    at: u64,
) -> String {
    let correlation = ordo_secevent::correlation::mint();
    if let Some(stream) = stream {
        stream.observe(
            ordo_secevent::Observation {
                space: None,
                subject: ordo_secevent::Subject::member(by),
                op: format!("enrol.{act}"),
                kind: ordo_secevent::EventKind::Enrolment {
                    act: act.to_string(),
                    detail: match tenant {
                        Some(t) => format!("principal={person} reason={reason} tenant={t}"),
                        None => format!("principal={person} reason={reason}"),
                    },
                },
                at: ordo_commandlog::Timestamp(at),
                correlation: correlation.clone(),
            },
            ordo_secevent::Relationship::None,
        );
    }
    correlation.0
}
