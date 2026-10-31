//! `PgDirectoryStore`: the [`ordo_identity::directory::DirectoryStore`] a
//! deployment boots on.
//!
//! `ordo_identity::directory` states the trait and what an implementation
//! must guarantee about a minted principal id. What follows is only what
//! building the Postgres one decided.
//!
//! # The same shape as `$credential`, for the same reasons
//!
//! `ordo_host::credentials::PgCredentialStore` is the pattern: a table of
//! its own, a thread of its own with a current-thread runtime on it, and
//! synchronous trait methods that hand that thread a statement and block
//! on the answer. [`ordo_identity::directory::Directory`]'s methods are
//! synchronous and are called from inside a blocking task in the door's
//! `/auth/complete` handler, so the same bridge is needed here.
//!
//! # Two tables, not one
//!
//! `$principal` (`id` primary key, `kind`, `display`, `created_at`) holds
//! each minted identity once. `$authenticator` (`authenticator` primary
//! key, `principal`, `linked_at`) holds which authenticator names which
//! principal — the name the census and the operator docs already use for
//! this gap.
//!
//! A single denormalized table (one row per authenticator, carrying the
//! principal's kind and display alongside it) would also have worked, but
//! it would have made principal-id collision a **property of the
//! application code that never happens to draw the same id twice**
//! instead of a constraint the database enforces. With `$principal` as
//! its own table, [`PgDirectoryStore::create_principal`] commits a new id
//! with `INSERT ... ON CONFLICT ("id") DO NOTHING` and reads the row
//! count back: a collision cannot be minted, because the database refuses
//! the second writer of the same key rather than one of the two writers
//! silently winning.
//!
//! # No cache, for the reason `PgCredentialStore` gives one up
//!
//! Every read is a query. A link or an onboarding must be visible to the
//! very next login, including one served by a different replica, and a
//! cache would buy a round trip on a path that already spends tens of
//! milliseconds in argon2id before it gets here.
//!
//! # A restart must not quietly start over
//!
//! `open` refuses if either table is missing, the same posture
//! `PgCredentialStore::open` takes on `$credential`: an empty directory
//! is a deployment where nobody has signed up yet, and a directory this
//! process cannot read is a different fact that must not be reported the
//! same way. `ordo_host::runtime` refuses the boot when a password method
//! is configured and this store cannot be opened, rather than falling
//! back to the in-memory one.

use std::sync::mpsc::SyncSender;

use ordo_identity::directory::{random_principal_suffix, DirectoryStore, MINT_ATTEMPTS};
use ordo_identity::principal::{Principal, PrincipalId, PrincipalKind, Timestamp};
use ordo_identity::provider::AuthenticatorId;
use ordo_identity::{DirectoryStoreError, LinkMeta, Retired};

use ordo_exec::{sql_lit, Pool, PoolConfig, PoolError, TextRow};

/// Each minted principal, once.
pub const PRINCIPAL_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$principal" (
  "id"         text   NOT NULL,
  "kind"       text   NOT NULL,
  "display"    text   NOT NULL,
  "created_at" bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("id")
)"#;

/// Which authenticator names which principal.
///
/// `authenticator` is the primary key, which is what makes "an
/// authenticator names at most one principal" a constraint rather than a
/// check the caller remembers to run first. There is no foreign key to
/// `$principal`: both tables live in the same control database, but the
/// platform's other durable tables (`$credential`, `$sessionrevocation`)
/// take the same posture of plain columns with the check done in the
/// store, and a cross-table constraint here would buy nothing a
/// `principal()` read before every write does not already buy.
pub const AUTHENTICATOR_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$authenticator" (
  "authenticator" text   NOT NULL,
  "principal"     text   NOT NULL,
  "linked_at"     bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("authenticator")
)"#;

/// The columns recovery adds to `$authenticator`
/// (`plan-passkey-recovery.md` §7, §4.2). Each has a default that reads as an
/// ordinary link, so a row written before they existed is one. Added with
/// `ADD COLUMN IF NOT EXISTS` after the table, so a database that already
/// holds the table gains them at the next boot.
pub const AUTHENTICATOR_RECOVERY_DDL: [&str; 4] = [
    r#"ALTER TABLE "$authenticator" ADD COLUMN IF NOT EXISTS "presence_not_before" bigint NOT NULL DEFAULT 0"#,
    r#"ALTER TABLE "$authenticator" ADD COLUMN IF NOT EXISTS "recovered_case" text NOT NULL DEFAULT ''"#,
    r#"ALTER TABLE "$authenticator" ADD COLUMN IF NOT EXISTS "retired_case" text NOT NULL DEFAULT ''"#,
    r#"ALTER TABLE "$authenticator" ADD COLUMN IF NOT EXISTS "retired_until" bigint NOT NULL DEFAULT 0"#,
];

/// When the person deleted the account (owner ruling 2026-10-06: an account
/// is marked deleted and kept). Zero is a live account. Added after the
/// table, so a database that already holds it gains the column at boot.
pub const PRINCIPAL_DELETED_DDL: &str =
    r#"ALTER TABLE "$principal" ADD COLUMN IF NOT EXISTS "deleted_at" bigint NOT NULL DEFAULT 0"#;

/// The sweep an unlink or a last-authenticator count runs by principal.
pub const AUTHENTICATOR_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS "$authenticator$principal"
    ON "$authenticator" ("principal")"#;

/// The first completed sign-in of each principal, written by
/// [`DirectoryStore::note_sign_in`] on the one path every sign-in takes
/// (`ordo_identity::sign_in`). Tenant deletion reads it to tell a person who
/// used their identity from one nobody ever held (`plan-enrolment.md` E7).
pub const SIGNEDIN_DDL: &str = r#"CREATE TABLE IF NOT EXISTS "$signedin" (
  "principal" text   NOT NULL,
  "first_at"  bigint NOT NULL DEFAULT 0,
  PRIMARY KEY ("principal")
)"#;

/// Create `$principal` and `$authenticator`.
///
/// Called at boot beside `$credential` and `$sessionrevocation`. Retry-once
/// for the reason `Pool::ensure_platform_tables` does it: `CREATE TABLE IF
/// NOT EXISTS` is not race-free between two processes booting together.
pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    for ddl in [PRINCIPAL_DDL, PRINCIPAL_DELETED_DDL, AUTHENTICATOR_DDL]
        .into_iter()
        .chain(AUTHENTICATOR_RECOVERY_DDL)
        .chain([AUTHENTICATOR_INDEX_DDL, SIGNEDIN_DDL])
    {
        if pool.rows(ddl).await.is_err() {
            pool.rows(ddl).await?;
        }
    }
    Ok(())
}

/// One statement handed to the store's own thread, and where its answer
/// goes.
struct Job {
    sql: String,
    reply: SyncSender<Result<Vec<TextRow>, PoolError>>,
}

/// The Postgres directory store.
pub struct PgDirectoryStore {
    /// `Option` so [`Drop`] can close the channel *before* joining the
    /// thread, the same ordering `PgCredentialStore` uses and for the
    /// same reason: joining while still holding a sender is a deadlock.
    jobs: Option<tokio::sync::mpsc::UnboundedSender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
    id_source: Box<dyn Fn() -> String + Send + Sync>,
}

/// Where the store's connection comes from. See
/// [`crate::credentials::PgCredentialStore::open_on`].
enum Source {
    Dial(PoolConfig),
    Shared(Pool),
}

impl PgDirectoryStore {
    /// Dial and stand the store up.
    ///
    /// **Synchronous, and safe to call from inside a runtime**, because the
    /// dial happens on a thread this function creates rather than on the
    /// caller's.
    ///
    /// Refuses when `$principal` or `$authenticator` does not exist. See
    /// the module doc.
    pub fn open(cfg: PoolConfig) -> Result<PgDirectoryStore, DirectoryStoreError> {
        PgDirectoryStore::open_with_id_source(cfg, Box::new(random_principal_suffix))
    }

    /// The same, with the id draw as an input, for a test that wants to
    /// force a collision rather than hope for one.
    pub fn open_with_id_source(
        cfg: PoolConfig,
        id_source: Box<dyn Fn() -> String + Send + Sync>,
    ) -> Result<PgDirectoryStore, DirectoryStoreError> {
        // One connection: every statement is serialized through the job
        // loop anyway. Keeps this store's footprint off the pool the
        // request path shares, the same choice `PgCredentialStore` makes.
        PgDirectoryStore::start(Source::Dial(PoolConfig { size: 1, ..cfg }), id_source)
    }

    /// The same, **on a connection somebody else owns**: the one shared by
    /// the deployment's three single-connection stores. See
    /// [`crate::credentials::PgCredentialStore::open_on`] for why they may
    /// share with each other and may not share the request pool.
    pub fn open_on(pool: Pool) -> Result<PgDirectoryStore, DirectoryStoreError> {
        PgDirectoryStore::start(Source::Shared(pool), Box::new(random_principal_suffix))
    }

    /// The one thread, runtime and job loop both constructors use.
    fn start(
        source: Source,
        id_source: Box<dyn Fn() -> String + Send + Sync>,
    ) -> Result<PgDirectoryStore, DirectoryStoreError> {
        let (jobs, mut rx) = tokio::sync::mpsc::unbounded_channel::<Job>();
        let (ready, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);

        let thread = std::thread::Builder::new()
            .name("ordo-directory".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready.send(Err(e.to_string()));
                        return;
                    }
                };
                rt.block_on(async move {
                    let pool = match source {
                        Source::Shared(p) => p.labelled("directory"),
                        Source::Dial(cfg) => match Pool::connect(cfg).await {
                            Ok(p) => p,
                            Err(e) => {
                                let _ = ready.send(Err(e.to_string()));
                                return;
                            }
                        },
                    };
                    let _ = ready.send(Ok(()));
                    while let Some(job) = rx.recv().await {
                        let out = pool.rows(&job.sql).await;
                        let _ = job.reply.send(out);
                    }
                });
            })
            .map_err(|e| DirectoryStoreError(format!("directory store thread: {e}").into()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(DirectoryStoreError(e.into())),
            Err(_) => return Err(DirectoryStoreError("directory store thread died".into())),
        }

        let store = PgDirectoryStore {
            jobs: Some(jobs),
            thread: Some(thread),
            id_source,
        };
        // Prove both tables are there before anybody can mistake their
        // absence for an empty deployment. One query each, once, at open.
        store.sql(r#"SELECT 1 FROM "$principal" LIMIT 1"#)?;
        store.sql(r#"SELECT 1 FROM "$authenticator" LIMIT 1"#)?;
        Ok(store)
    }

    /// Run one statement on the store's own connection and wait for it.
    fn sql(&self, sql: &str) -> Result<Vec<TextRow>, DirectoryStoreError> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.jobs
            .as_ref()
            .ok_or_else(|| DirectoryStoreError("directory store is closed".into()))?
            .send(Job {
                sql: sql.to_string(),
                reply,
            })
            .map_err(|_| DirectoryStoreError("directory store is closed".into()))?;
        match ordo_exec::bridge::wait(&rx) {
            Ok(Ok(rows)) => Ok(rows),
            Ok(Err(e)) => Err(store_error(e)),
            Err(_) => Err(DirectoryStoreError("directory store is closed".into())),
        }
    }
}

/// `$principal`/`$authenticator` missing is a different fact from "nobody
/// has signed up", and an operator who conflates them looks for the wrong
/// thing.
///
/// **The text is scrubbed of everything but the table.** A `PoolError` can
/// carry the statement that failed, and a statement here carries a
/// principal or authenticator id. Neither is a secret, but both are the
/// input to the enumeration this whole path is careful about, and an
/// error that travels to a log with one in it has published it.
fn store_error(e: PoolError) -> DirectoryStoreError {
    // A full pool names no table and no statement, and the sign-in routes
    // answer it as `server_busy`, so its fixed words are kept whole.
    if e.is_busy() {
        return DirectoryStoreError(crate::passkeys::scrub_fault("$principal", e));
    }
    let text = e.to_string();
    for table in ["$principal", "$authenticator"] {
        // Missing is told by SQLSTATE; the text only names which table.
        if e.is_missing_table() && text.contains(table) {
            return DirectoryStoreError(
                format!(
                    "`{table}` does not exist: the deployment has not created its platform tables"
                )
                .into(),
            );
        }
    }
    DirectoryStoreError("the directory could not be read".into())
}

impl Drop for PgDirectoryStore {
    fn drop(&mut self) {
        // Dropping the sender ends the job loop, which ends `block_on`,
        // which ends the thread. Joining here means a dropped store has
        // released its connection by the time drop returns, which is what
        // makes a restart test a restart rather than two live stores.
        self.jobs.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn row_text(row: &TextRow, i: usize) -> Option<String> {
    row.get(i).and_then(|c| c.clone())
}

/// The four recovery columns starting at `i`, then `linked_at`, read as
/// [`LinkMeta`].
fn meta_at(row: &TextRow, i: usize) -> LinkMeta {
    let num = |k: usize| {
        row_text(row, k)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let text = |k: usize| row_text(row, k).filter(|s| !s.is_empty());
    LinkMeta {
        presence_not_before: num(i),
        recovered_case: text(i + 1),
        retired: text(i + 2).map(|case| Retired {
            case,
            until: num(i + 3),
        }),
        linked_at: num(i + 4),
    }
}

fn meta_of(row: &TextRow) -> LinkMeta {
    meta_at(row, 0)
}

impl DirectoryStore for PgDirectoryStore {
    fn principal(&self, id: &PrincipalId) -> Result<Option<Principal>, DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "kind", "display", "created_at" FROM "$principal" WHERE "id" = {}"#,
            sql_lit(&id.0)
        ))?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let kind_text = row_text(row, 0).unwrap_or_default();
        let Some(kind) = PrincipalKind::from_prefix(&kind_text) else {
            return Err(DirectoryStoreError(
                "`$principal` holds a kind this build does not recognise".into(),
            ));
        };
        let display = row_text(row, 1).unwrap_or_default();
        let created_at: u64 = row_text(row, 2).and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok(Some(Principal {
            id: id.clone(),
            kind,
            display,
            created: Timestamp(created_at),
        }))
    }

    fn principal_for(
        &self,
        authenticator: &AuthenticatorId,
    ) -> Result<Option<PrincipalId>, DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "principal" FROM "$authenticator" WHERE "authenticator" = {}"#,
            sql_lit(&authenticator.0)
        ))?;
        Ok(rows.first().and_then(|r| row_text(r, 0)).map(PrincipalId))
    }

    fn link_count(&self, principal: &PrincipalId) -> Result<usize, DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT COUNT(*) FROM "$authenticator" WHERE "principal" = {}"#,
            sql_lit(&principal.0)
        ))?;
        let n: i64 = rows
            .first()
            .and_then(|r| row_text(r, 0))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Ok(n.max(0) as usize)
    }

    fn authenticators_of(
        &self,
        principal: &PrincipalId,
    ) -> Result<Vec<AuthenticatorId>, DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "authenticator" FROM "$authenticator" WHERE "principal" = {}
               ORDER BY "authenticator""#,
            sql_lit(&principal.0)
        ))?;
        Ok(rows
            .iter()
            .filter_map(|r| row_text(r, 0))
            .map(AuthenticatorId)
            .collect())
    }

    fn create_principal(
        &self,
        kind: PrincipalKind,
        display: &str,
        at: Timestamp,
    ) -> Result<PrincipalId, DirectoryStoreError> {
        for _ in 0..MINT_ATTEMPTS {
            let id = PrincipalId(format!("{}_{}", kind.id_prefix(), (self.id_source)()));
            let rows = self.sql(&format!(
                r#"INSERT INTO "$principal" ("id", "kind", "display", "created_at")
                   VALUES ({}, {}, {}, {})
                   ON CONFLICT ("id") DO NOTHING
                   RETURNING "id""#,
                sql_lit(&id.0),
                sql_lit(kind.id_prefix()),
                sql_lit(display),
                at.0
            ))?;
            if !rows.is_empty() {
                return Ok(id);
            }
            // The draw collided with an existing row: retried below rather
            // than minted, which is the guarantee `DirectoryStore` states.
        }
        Err(DirectoryStoreError(
            "could not mint a principal id that does not already exist".into(),
        ))
    }

    fn insert_link(
        &self,
        authenticator: &AuthenticatorId,
        principal: &Principal,
        at: Timestamp,
    ) -> Result<(), DirectoryStoreError> {
        self.sql(&format!(
            r#"INSERT INTO "$authenticator" ("authenticator", "principal", "linked_at")
               VALUES ({}, {}, {})"#,
            sql_lit(&authenticator.0),
            sql_lit(&principal.id.0),
            at.0
        ))?;
        Ok(())
    }

    fn remove_link(&self, authenticator: &AuthenticatorId) -> Result<(), DirectoryStoreError> {
        self.sql(&format!(
            r#"DELETE FROM "$authenticator" WHERE "authenticator" = {}"#,
            sql_lit(&authenticator.0)
        ))?;
        Ok(())
    }

    fn link_meta(
        &self,
        authenticator: &AuthenticatorId,
    ) -> Result<Option<LinkMeta>, DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "presence_not_before", "recovered_case", "retired_case", "retired_until",
                      "linked_at"
               FROM "$authenticator" WHERE "authenticator" = {}"#,
            sql_lit(&authenticator.0)
        ))?;
        Ok(rows.first().map(meta_of))
    }

    fn links_of(
        &self,
        principal: &PrincipalId,
    ) -> Result<Vec<(AuthenticatorId, LinkMeta)>, DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "authenticator", "presence_not_before", "recovered_case", "retired_case",
                      "retired_until", "linked_at"
               FROM "$authenticator" WHERE "principal" = {} ORDER BY "authenticator""#,
            sql_lit(&principal.0)
        ))?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let id = row_text(r, 0)?;
                Some((AuthenticatorId(id), meta_at(r, 1)))
            })
            .collect())
    }

    fn insert_link_marked(
        &self,
        authenticator: &AuthenticatorId,
        principal: &Principal,
        at: Timestamp,
        presence_not_before: u64,
        case: &str,
    ) -> Result<(), DirectoryStoreError> {
        self.sql(&format!(
            r#"INSERT INTO "$authenticator"
               ("authenticator", "principal", "linked_at", "presence_not_before", "recovered_case")
               VALUES ({}, {}, {}, {presence_not_before}, {})"#,
            sql_lit(&authenticator.0),
            sql_lit(&principal.id.0),
            at.0,
            sql_lit(case)
        ))?;
        Ok(())
    }

    fn set_retired(
        &self,
        authenticator: &AuthenticatorId,
        retired: Option<&Retired>,
    ) -> Result<(), DirectoryStoreError> {
        let (case, until) = match retired {
            Some(r) => (r.case.as_str(), r.until),
            None => ("", 0),
        };
        self.sql(&format!(
            r#"UPDATE "$authenticator" SET "retired_case" = {}, "retired_until" = {until}
               WHERE "authenticator" = {}"#,
            sql_lit(case),
            sql_lit(&authenticator.0)
        ))?;
        Ok(())
    }

    fn retired_by(
        &self,
        now: u64,
    ) -> Result<Vec<(AuthenticatorId, PrincipalId, Retired)>, DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "authenticator", "principal", "retired_case", "retired_until"
               FROM "$authenticator" WHERE "retired_case" <> '' AND "retired_until" <= {now}"#
        ))?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some((
                    AuthenticatorId(row_text(r, 0)?),
                    PrincipalId(row_text(r, 1)?),
                    Retired {
                        case: row_text(r, 2)?,
                        until: row_text(r, 3).and_then(|s| s.parse().ok()).unwrap_or(0),
                    },
                ))
            })
            .collect())
    }

    fn note_sign_in(
        &self,
        principal: &PrincipalId,
        at: Timestamp,
    ) -> Result<(), DirectoryStoreError> {
        self.sql(&format!(
            r#"INSERT INTO "$signedin" ("principal", "first_at") VALUES ({}, {})
               ON CONFLICT ("principal") DO NOTHING"#,
            sql_lit(&principal.0),
            at.0
        ))?;
        Ok(())
    }

    fn deleted_at(&self, principal: &PrincipalId) -> Result<Option<u64>, DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"SELECT "deleted_at" FROM "$principal" WHERE "id" = {} AND "deleted_at" > 0"#,
            sql_lit(&principal.0)
        ))?;
        Ok(rows
            .first()
            .and_then(|r| row_text(r, 0))
            .and_then(|t| t.parse::<u64>().ok()))
    }

    /// One statement, and the first instant kept: a second mark changes
    /// nothing. A principal with no row is refused rather than reported
    /// marked.
    fn mark_deleted(
        &self,
        principal: &PrincipalId,
        at: Timestamp,
    ) -> Result<(), DirectoryStoreError> {
        let rows = self.sql(&format!(
            r#"UPDATE "$principal" SET "deleted_at" = CASE WHEN "deleted_at" > 0
                 THEN "deleted_at" ELSE {at} END
               WHERE "id" = {} RETURNING "id""#,
            sql_lit(&principal.0),
            at = at.0.max(1),
        ))?;
        if rows.is_empty() {
            return Err(DirectoryStoreError(
                "the directory holds no such principal to mark deleted".into(),
            ));
        }
        Ok(())
    }
}
