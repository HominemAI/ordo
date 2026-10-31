//! `ordo identity` — the deployment owner's commands on a person's credential
//! (`plan-passkey-recovery.md` R3).
//!
//! # `ordo identity recover`
//!
//! Opens a recovery case for a person who has lost every way in, from the
//! host, with the database credentials this tool already holds for its other
//! commands. It prints the recovery ticket once. The person then registers a
//! new passkey with it at `/auth/recover/begin` and `/auth/recover/complete`,
//! which is the only thing it can be spent on.
//!
//! This is the way back in when the deployment owner's own last passkey is
//! gone, because every other path needs a person who can sign in. It is the
//! act of whoever controls the host, and is recorded as one: under the
//! subject `platform cli`, with the name given to `--as` in the record, the
//! evidence class, and the reference the deployment owner keeps in their own
//! ticketing system.
//!
//! **No delay**, by default (`RecoveryPolicy::host_delay_secs`). Whoever holds
//! the database and the master key is already sovereign over the deployment,
//! and a delay enforced by software they control would restrain nobody. What
//! stays is what follows the recovery: the new passkey is quarantined from
//! step-up, every session ends, and the retired passkeys can contest.
//!
//! **What the host cannot do.** It cannot mirror the act into a tenant's own
//! stream: which tenants a person belongs to is answered by the running
//! server's door, and this command has the database and not the door. The act
//! is in the deployment's stream, which the deployment owner reads. A
//! recovery made through the door (`std.identity.recover`) mirrors into each
//! tenant.

use std::sync::Arc;

use ordo_exec::{Pool, PoolConfig};
use ordo_host::recovery::{Open, OpenError, Path, PgRecoveryStore, Recovery, RecoveryPolicy};
use ordo_host::tickets::{PgTicketStore, Tickets};
use ordo_identity::{DirectoryStore, PrincipalId};

use crate::operator::{Flags, Outcome};

/// The classes of evidence a recovery may name. A closed list: no free text
/// enters the stream. The same list the door's ops take.
pub use ordo_host::identity_ops::EVIDENCE;

pub fn run(sub: &str, flags: Flags, cfg: PoolConfig) -> Outcome {
    match sub {
        "recover" => recover(flags, cfg),
        other => Outcome::usage(format!(
            "unknown `ordo identity` command `{other}` (recover)\n"
        )),
    }
}

fn recover(flags: Flags, cfg: PoolConfig) -> Outcome {
    if let Err(e) = flags.only(&["--principal", "--as", "--evidence", "--ref", "--db"]) {
        return Outcome::usage(e);
    }
    let principal = match flags.require("--principal") {
        Ok(p) => p.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let actor = match flags.require("--as") {
        Ok(a) => a.to_string(),
        Err(e) => return Outcome::usage(e),
    };
    let evidence = flags
        .named_value("--evidence")
        .unwrap_or("host-break-glass")
        .to_string();
    if !EVIDENCE.contains(&evidence.as_str()) {
        return Outcome::usage(format!(
            "`--evidence {evidence}` is not one of: {}\n",
            EVIDENCE.join(", ")
        ));
    }
    let reference = flags.named_value("--ref").unwrap_or("").to_string();
    if reference.len() > 120
        || !reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/-".contains(c))
    {
        return Outcome::usage(
            "`--ref` is an opaque reference of at most 120 characters from A-Z a-z 0-9 . _ : / -\n"
                .into(),
        );
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return Outcome::failed(format!("runtime: {e}\n")),
    };
    // The stores own a thread and a runtime each; the tables are made and
    // the record is written on this one.
    let pool = match rt.block_on(Pool::connect(PoolConfig {
        size: 1,
        ..cfg.clone()
    })) {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("cannot reach the database: {e}\n")),
    };
    if let Err(e) = rt.block_on(async {
        ordo_host::tickets::ensure_table(&pool).await?;
        ordo_host::recovery::ensure_table(&pool).await
    }) {
        return Outcome::failed(format!("cannot create the recovery tables: {e}\n"));
    }
    let directory = match ordo_host::principal_directory::PgDirectoryStore::open(cfg.clone()) {
        Ok(d) => d,
        Err(e) => return Outcome::failed(format!("the directory cannot be read: {e}\n")),
    };
    match directory.principal(&PrincipalId(principal.clone())) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Outcome::failed(format!("no principal `{principal}` in this deployment\n"))
        }
        Err(e) => return Outcome::failed(format!("the directory cannot be read: {e}\n")),
    }
    let recorder: Arc<ordo_secevent::Recorder> = Arc::default();
    let tickets = match PgTicketStore::open(cfg.clone()) {
        Ok(t) => Arc::new(Tickets::new(Arc::new(t)).with_recorder(recorder.clone())),
        Err(e) => return Outcome::failed(format!("the ticket table cannot be read: {e}\n")),
    };
    let cases = match PgRecoveryStore::open(cfg) {
        Ok(c) => c,
        Err(e) => return Outcome::failed(format!("the recovery table cannot be read: {e}\n")),
    };
    // The deployment's own settings, read the way the server reads them, so
    // the host and the door agree on the quarantine, the window and the paths.
    let policy = match RecoveryPolicy::from_env() {
        Ok(p) => p,
        Err(e) => return Outcome::failed(format!("{}: {}\n", e.var, e.detail)),
    };
    let recovery = Recovery::new(Arc::new(cases), tickets, policy)
        .with_recorder(recorder.clone())
        .with_directory(Arc::new(directory));
    let opened = match recovery.open(&Open {
        principal: &principal,
        path: Path::Host,
        opened_by: &actor,
        authority: "host",
        evidence: &evidence,
        reference: &reference,
        space: "",
        extra_delay_secs: 0,
        approvals: 0,
    }) {
        Ok(o) => o,
        Err(OpenError::Live) => {
            return Outcome::failed(format!(
                "`{principal}` already has a recovery in progress (open, matured, or inside its \
                 contest window). It has to be contested or to run its course first.\n"
            ))
        }
        Err(OpenError::Disabled) => {
            return Outcome::failed(
                "this deployment has switched the host path off (ORDO_RECOVERY_PATHS)\n".into(),
            )
        }
        Err(OpenError::OutsideProvider) => {
            return Outcome::failed(format!(
                "{}\n",
                ordo_host::recovery::OUTSIDE_PROVIDER_SENTENCE
            ))
        }
        Err(OpenError::Unavailable(e)) => {
            return Outcome::failed(format!("the recovery could not be opened: {e}\n"))
        }
        Err(e) => return Outcome::failed(format!("the recovery could not be opened: {e:?}\n")),
    };
    let at = ordo_commandlog::Timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    let recorded = rt.block_on(record(&pool, &recorder, at));
    let mut out = format!(
        "recovery case {} opened for {principal}\n  path: host, evidence: {evidence}, recorded: {recorded}\n",
        opened.case
    );
    match (&opened.ticket, opened.ticket_expires_at) {
        (Some(ticket), Some(expires)) => out.push_str(&format!(
            "\n  recovery ticket (shown once, valid until {expires}):\n\n    {ticket}\n\n  \
             The person spends it by registering a new passkey. Nothing else accepts it.\n  \
             Their new passkey will not satisfy step-up until the quarantine ends, every \
             session they\n  had ends when they finish, and the passkeys they had can \
             contest for the window.\n"
        )),
        _ => out.push_str(&format!(
            "\n  the case matures at {}; collect its ticket after that.\n",
            opened.matures_at
        )),
    }
    Outcome::ok(out)
}

/// Append what the recorder holds to the deployment's security stream, in one
/// transaction the way the drain does it. Best effort, said so in the output:
/// the case is opened whether or not the record could be written.
async fn record(
    pool: &Pool,
    recorder: &ordo_secevent::Recorder,
    at: ordo_commandlog::Timestamp,
) -> String {
    let entries = recorder.drain_all(at);
    let tx = match pool.begin().await {
        Ok(t) => t,
        Err(e) => return format!("NOT RECORDED ({e}); the case is still open"),
    };
    let mut n = 0;
    for e in &entries {
        if let Err(err) = tx.append_security_entry(e).await {
            return format!("NOT RECORDED ({err}); the case is still open");
        }
        n += 1;
    }
    match tx.commit().await {
        Ok(()) => format!("{n} entries"),
        Err(e) => format!("NOT RECORDED ({e}); the case is still open"),
    }
}

// ── ordo verify --identity ───────────────────────────────────────────────

/// What the deployment's security stream says about how links came to be.
#[derive(Default)]
struct Seen {
    /// `AuthenticatorLinked { added }`: the authenticator, by id.
    linked: std::collections::BTreeSet<String>,
    /// `Recovery` `completed` with `added=`: the authenticator, by id.
    recovered: std::collections::BTreeSet<String>,
    /// `Enrolment` `credential-set` and `temporary-password-set`: the
    /// principal a password was given to, whose `password:` link they explain.
    password_given: std::collections::BTreeSet<String>,
    /// `PrincipalOnboarded`: the rendered subject of each, which names the
    /// principal onboarded.
    onboarded: Vec<String>,
}

fn token<'a>(detail: &'a str, key: &str) -> Option<&'a str> {
    detail
        .split_whitespace()
        .find_map(|t| t.strip_prefix(key)?.strip_prefix('='))
}

/// `ordo verify --identity`: every link in `$authenticator` reconciled against
/// the deployment's signed security stream (`plan-passkey-recovery.md` R8, T3).
///
/// Nothing that can write the directory table can be stopped from writing a
/// row, and a recovery design cannot change that. What it can do is make the
/// row **detectable**: every way a link comes to exist writes an event to the
/// stream, and the stream is signed. So a link with no event behind it is one
/// somebody wrote by hand, and this names it. The rules for what explains a
/// link are the ways one is made:
///
/// - an `AuthenticatorLinked` event naming it (sign-up, a ticket, a link
///   ceremony);
/// - a `Recovery` event `completed` that names it as `added`;
/// - for a `password:` link, an enrolment act that gave the person a password;
/// - `PrincipalOnboarded` explains **one** link of the principal it names, the
///   first, and no more, so a passkey somebody attached to an onboarded
///   person's account is still reported.
///
/// The stream's own chain is verified beside it (`security_report`), because a
/// reconciliation against a stream that has been rewritten proves nothing.
/// **A stream trimmed by retention shows the links it covered as unexplained**,
/// and the report says from which position the stream is retained.
pub async fn reconcile(pool: &Pool) -> Result<(String, bool), String> {
    let table = pool
        .rows(r#"SELECT to_regclass('"$authenticator"') IS NOT NULL"#)
        .await
        .map_err(|e| e.to_string())?;
    if !matches!(
        table
            .first()
            .and_then(|r| r.first())
            .and_then(|c| c.as_deref()),
        Some("t") | Some("true")
    ) {
        return Ok((
            "identity  this deployment has no directory table, so no link exists to reconcile\n"
                .to_string(),
            false,
        ));
    }
    let links: Vec<(String, String, u64)> = pool
        .rows(
            r#"SELECT "authenticator", "principal", "linked_at" FROM "$authenticator"
               ORDER BY "principal", "linked_at", "authenticator""#,
        )
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|r| {
            Some((
                r.first()?.clone()?,
                r.get(1)?.clone()?,
                r.get(2)?.as_deref()?.parse().ok()?,
            ))
        })
        .collect();

    let store =
        ordo_exec::secevent::PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    let scope = ordo_secevent::Scope::Deployment;
    let base = store
        .try_base_index(&scope)
        .await
        .map_err(|e| e.to_string())?;
    let mut seen = Seen::default();
    let mut read = 0usize;
    loop {
        let batch = store
            .try_entries(&scope, read, 500)
            .await
            .map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        read += batch.len();
        for c in batch {
            use ordo_secevent::EventKind as K;
            match c.entry.kind {
                K::AuthenticatorLinked { added, .. } => {
                    seen.linked.insert(added);
                }
                K::Recovery { act, detail } if act == "completed" => {
                    if let Some(a) = token(&detail, "added") {
                        seen.recovered.insert(a.to_string());
                    }
                }
                K::Enrolment { act, detail }
                    if act == "credential-set" || act == "temporary-password-set" =>
                {
                    if let Some(p) = token(&detail, "principal") {
                        seen.password_given.insert(p.to_string());
                    }
                }
                K::PrincipalOnboarded => seen.onboarded.push(c.entry.subject.render()),
                _ => {}
            }
        }
    }

    // What no specific record explains, principal by principal.
    let mut unexplained: Vec<&(String, String, u64)> = Vec::new();
    let mut by_principal: std::collections::BTreeMap<&str, Vec<&(String, String, u64)>> =
        Default::default();
    for l in &links {
        let (id, principal, _) = l;
        let specific = seen.linked.contains(id)
            || seen.recovered.contains(id)
            || (id.starts_with("password:") && seen.password_given.contains(principal));
        if !specific {
            by_principal.entry(principal.as_str()).or_default().push(l);
        }
    }
    for (principal, mut rest) in by_principal {
        // `PrincipalOnboarded` explains the first, and only the first.
        if seen.onboarded.iter().any(|s| s.contains(principal)) && !rest.is_empty() {
            rest.remove(0);
        }
        unexplained.extend(rest);
    }

    let mut out = format!(
        "identity  {} link{} in `$authenticator` reconciled against {} entr{} of the deployment's \
         security stream\n",
        links.len(),
        if links.len() == 1 { "" } else { "s" },
        read,
        if read == 1 { "y" } else { "ies" },
    );
    if base > 0 {
        out.push_str(&format!(
            "retained  the stream is kept from position {base}, so a link made before it shows \
             as unexplained\n"
        ));
    }
    if unexplained.is_empty() {
        out.push_str("VERIFIED  every link has a record in the stream\n");
        return Ok((out, false));
    }
    out.push_str(&format!(
        "FAILED    {} link{} with no record in the stream, so somebody wrote {} by hand:\n",
        unexplained.len(),
        if unexplained.len() == 1 { "" } else { "s" },
        if unexplained.len() == 1 { "it" } else { "them" },
    ));
    for (id, principal, at) in unexplained {
        out.push_str(&format!(
            "          {id}  principal {principal}  linked_at {at}\n"
        ));
    }
    Ok((out, true))
}
