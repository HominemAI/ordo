//! Which database a space's rows are in: **the per-space attachment**.
//!
//! `plan-space-routing.md` §2, and `definition-language.md` §2's pin
//! (1), which names databases beside buckets: *"attaching or changing a
//! space's BYO bucket (or BYO database) is itself a granted operation"*,
//! one that needs the space owner's consent, a step-up and a command-log
//! record, so that storage custody never changes silently. This module is
//! the table and the two operations on it. Nothing reads it yet: the resolver that does is S3,
//! and the op that writes it is S5's provisioning.
//!
//! # The same three answers the bucket attachment gives
//!
//! [`crate::bucket`] argues each of these and the arguments carry over
//! unchanged, so they are stated rather than repeated:
//!
//! - **The credential is a reference, never a value.** A secret here would
//!   be a secret in every backup and replica of the control database, and
//!   rotating it would be a schema-level act.
//! - **The custody choice is stored rather than inferred**, because a
//!   consent page has to state which one was taken ([`DbCustody`]).
//! - **A repeat is keyed on the target's identity**, so a retry is a no-op
//!   that reports success and not a second custody event.
//!
//! # Where this differs from a bucket, and why
//!
//! **A different target for a space that has one is refused**, where a
//! bucket attachment replaces it. Replacing a bucket converts custody going
//! forward and the objects already stored stay readable through the
//! references that name them. Replacing a database moves nothing: the
//! space's rows, its chain and its blob references stay in the old
//! database, and every request after the swap reads an empty one. That is
//! the live move `plan-space-routing.md` Q-2 excludes from this plan and
//! the owner ruled must be possible later (`routing-migration`,
//! 2026-09-12). Until the move is built, [`AttachRefusal::WouldMove`] is
//! the place that says so, and it is the one place the move will have to
//! change.
//!
//! **This table lives in the control database** (§3): it is the answer to
//! where a space's database is, so it cannot be inside that database.

use ordo_crypto::custody::SpaceId;

use crate::{sql_lit, Pool, PoolError};

pub const SPACEDB_TABLE: &str = "$spacedb";

/// One row per space. A space with no row is on the deployment's default
/// database, which is every space until something attaches one.
///
/// There is **no secret column**, and the absence is asserted by a test that
/// reads this string.
pub const SPACEDB_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$spacedb\" ( \
  \"$space\" text NOT NULL PRIMARY KEY, \
  \"host\" text NOT NULL, \
  \"port\" integer NOT NULL, \
  \"user\" text NOT NULL, \
  \"db\" text NOT NULL, \
  \"credential\" text NOT NULL, \
  \"custody\" text NOT NULL, \
  \"attached_at\" bigint NOT NULL, \
  \"attached_by\" text NOT NULL \
)";

/// Who runs the database a space's rows are in, stated so a consent page
/// can state it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbCustody {
    /// A database the platform runs, on the platform's own cluster: a
    /// dedicated tier for one tenant, with the platform holding every copy.
    PlatformManaged,
    /// The tenant's own database, under their own perimeter. The platform
    /// reads and writes it through the credential it was given, and does
    /// not reach its backups, its replicas or its write-ahead log.
    OwnPerimeter,
}

impl DbCustody {
    pub fn as_str(self) -> &'static str {
        match self {
            DbCustody::PlatformManaged => "platform-managed",
            DbCustody::OwnPerimeter => "own-perimeter",
        }
    }

    pub fn parse(s: &str) -> Option<DbCustody> {
        match s {
            "platform-managed" => Some(DbCustody::PlatformManaged),
            "own-perimeter" => Some(DbCustody::OwnPerimeter),
            _ => None,
        }
    }

    /// The sentence a consent page prints, written beside the mechanism for
    /// [`crate::bucket::BucketCustody::disclosure`]'s reason.
    ///
    /// Both say what deleting the space does, because that is where the two
    /// positions differ. Sealed values are unreadable after erasure in
    /// either, since the keys that open them are the platform's to destroy.
    /// What differs is everything that is not sealed, and who can check the
    /// audit chain.
    pub fn disclosure(self) -> &'static str {
        match self {
            DbCustody::PlatformManaged => {
                "Your space's rows are kept in a database the platform runs for you. \
                 Deleting this space destroys the platform's keys, so every sealed value \
                 becomes permanently unreadable, and the platform deletes the space's \
                 rows from that database. The platform can verify this space's audit \
                 chain without asking you."
            }
            DbCustody::OwnPerimeter => {
                "Your space's rows are kept in your own database, under your own \
                 perimeter. Deleting this space destroys the platform's keys, so every \
                 sealed value becomes permanently unreadable, including in your \
                 backups. Values that are not sealed stay wherever your database has \
                 copied them (backups, replicas, its write-ahead log), and removing \
                 those is yours to do. Verifying this space's audit chain needs read \
                 access to your database."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbAttachment {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub db: String,
    /// A **label**, never a secret. See the module doc.
    pub credential: String,
    pub custody: DbCustody,
    pub attached_at: i64,
    pub attached_by: String,
}

impl DbAttachment {
    /// The identity a repeat is keyed on: everything that says where the
    /// rows are and on what terms, and nothing that says when or who.
    ///
    /// Two attachments with the same identity are one target, which is
    /// also the key the resolver (S3) pools by: N spaces on one target cost
    /// one pool.
    pub fn identity(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.host,
            self.port,
            self.user,
            self.db,
            self.credential,
            self.custody.as_str()
        )
    }
}

/// What an attach did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbAttachOutcome {
    /// No attachment existed; this space now has one.
    Attached,
    /// The identical attachment was already stored. Nothing changed, and a
    /// retry is not a custody event.
    Unchanged,
}

/// Why an attach was refused.
#[derive(Debug)]
#[non_exhaustive]
pub enum AttachRefusal {
    /// The space is already attached to a different database, and moving
    /// a live space between databases is not built. See the module doc.
    WouldMove {
        current: String,
    },
    Pool(PoolError),
}

impl std::fmt::Display for AttachRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachRefusal::WouldMove { current } => write!(
                f,
                "this space is already in database `{current}`, and repointing it would \
                 leave its rows and its chain behind: moving a live space between \
                 databases is not built"
            ),
            AttachRefusal::Pool(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AttachRefusal {}

impl From<PoolError> for AttachRefusal {
    fn from(e: PoolError) -> Self {
        AttachRefusal::Pool(e)
    }
}

/// Apply the deployment's schema to a new target, once, and say whether this
/// call applied it.
///
/// The schema's DDL is plain `CREATE TABLE`, which fails when run twice, so
/// a retry has to know whether it already ran. It is sent as one
/// simple-protocol query, which Postgres runs as one implicit transaction:
/// it applied completely or not at all. So the presence of `members`, which
/// every schema declares as a platform entity, answers whether it ran.
///
/// **This form leaves the target with no record of which version its tables
/// are**, which is what [`apply_schema_at_version`] exists to fix. It stays
/// for the one caller that writes that record itself, on its own connection,
/// so that a *failed* apply also leaves a row saying it failed
/// (`ordo-host`'s `schema_apply::create_schema`). A fresh target has no such
/// caller standing over it, so it uses the recording form.
pub async fn apply_schema(pool: &Pool, ddl: &str) -> Result<bool, PoolError> {
    apply(pool, ddl, None).await
}

/// Apply the schema to a new target **and record which version it is**, in
/// one act that either does both or does neither.
///
/// # Why the record rides inside the same query, and why that is still
/// all-or-nothing
///
/// `plan-space-routing.md` R-S1: a target holding the tables without the row
/// that names their version is the exact state this function exists to
/// prevent, because the next apply would then have to infer the version from
/// the live shape, and a shape does not distinguish two versions that happen
/// to declare the same tables.
///
/// So the `$migrations` table and its one row are appended to the schema's
/// own DDL and the whole thing is sent as **one** simple-protocol query.
/// Postgres executes a multi-statement Query message as a single implicit
/// transaction, and an error in any statement rolls back every statement in
/// the message. The property [`apply_schema`]'s doc claims therefore covers
/// the row too, and it rests on three things that are true here and are
/// checked rather than assumed:
///
/// - **Nothing in the string opens or closes a transaction, and nothing in
///   it is `CONCURRENTLY`.** An explicit `BEGIN` or `COMMIT` inside a
///   multi-statement Query message ends the implicit one and splits the
///   message into separately-committed pieces, and a `CREATE INDEX
///   CONCURRENTLY` cannot run inside a transaction block at all.
///   `ordo_schema::ddl_for` emits only `CREATE TABLE`, `ALTER TABLE ... ADD
///   CONSTRAINT` and the non-concurrent `CREATE INDEX` (its `concurrently`
///   flag is `false` on this path; the concurrent form belongs to 13b's
///   apply, which runs each such index on its own connection on purpose),
///   and the two statements appended here are a `CREATE TABLE IF NOT
///   EXISTS` and an `INSERT`. That is checked rather than assumed:
///   [`breaks_the_implicit_transaction`] reads the DDL this call was
///   actually handed, so a schema that ever grew such a statement is
///   refused here rather than quietly giving up the guarantee.
/// - **`Pool::rows` really is the simple protocol.** It sends the string
///   through `simple_query`, one Query message, not a prepared statement per
///   line. An extended-protocol path would execute the statements one at a
///   time, each in its own implicit transaction, and the guarantee would be
///   gone without a word of this doc changing.
///
/// Returns `false`, having written nothing, when `members` is already
/// present: the same retry guard [`apply_schema`] uses. A target in that
/// state is either one this call already finished or a **pre-R-S1**
/// database, and the two are told apart by [`recorded_version`] rather than
/// by writing a row that would guess.
///
/// `at` is **milliseconds** since the epoch, which is the unit every
/// `$migrations` row already carries.
pub async fn apply_schema_at_version(
    pool: &Pool,
    ddl: &str,
    version: &str,
    actor: &str,
    changes: &str,
    at: u64,
) -> Result<bool, PoolError> {
    let row = format!(
        "{migrations};\n\
         INSERT INTO \"$migrations\" \
           (\"kind\", \"from_version\", \"to_version\", \"actor\", \"at\", \
            \"changes\", \"ddl\", \"phase\", \"note\") \
         VALUES ({kind}, NULL, {to}, {actor}, {at}, {changes}, {ddl}, {phase}, {note});\n",
        migrations = crate::migrate::MIGRATIONS_DDL,
        kind = sql_lit(crate::migrate::kind::CREATE),
        to = sql_lit(version),
        actor = sql_lit(actor),
        at = at,
        changes = sql_lit(changes),
        ddl = sql_lit(ddl),
        phase = sql_lit(crate::migrate::phase::APPLIED),
        note = sql_lit(
            "created with the database, inside the apply that created the tables \
             (plan-space-routing R-S1)"
        ),
    );
    apply(pool, ddl, Some(&row)).await
}

/// The one body both forms share: the retry guard, then one query.
///
/// `trailing` is appended to the DDL **inside the same string**, which is
/// the whole of [`apply_schema_at_version`]'s argument. The `from_version`
/// the row carries is NULL because there is no previous version on a
/// database that did not exist a moment ago.
async fn apply(pool: &Pool, ddl: &str, trailing: Option<&str>) -> Result<bool, PoolError> {
    let present = pool
        .rows("SELECT to_regclass('public.members') IS NOT NULL")
        .await?;
    if present
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        == Some("t")
    {
        return Ok(false);
    }
    let Some(trailing) = trailing else {
        pool.rows(ddl).await?;
        return Ok(true);
    };
    if let Some(word) = breaks_the_implicit_transaction(ddl) {
        return Err(PoolError::db(format!(
            "this schema's DDL contains `{word}`, which would split the apply into \
             separately-committed pieces and could leave the target holding the tables \
             with no record of which version they are"
        )));
    }
    let mut sql = ddl.trim_end().to_string();
    // A schema that already terminates its last statement is the ordinary
    // case; one that does not would otherwise splice into the `CREATE TABLE`
    // that follows it and change both statements' meaning.
    if !sql.is_empty() && !sql.ends_with(';') {
        sql.push(';');
    }
    sql.push('\n');
    sql.push_str(trailing);
    pool.rows(&sql).await?;
    Ok(true)
}

/// The word that would cost this apply its all-or-nothing property, if the
/// DDL holds one.
///
/// Two families, and both are fatal for the same reason. Transaction control
/// (`BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`) ends the implicit
/// transaction Postgres wraps a multi-statement Query message in, so what
/// followed would commit separately from what came before. `CONCURRENTLY`
/// cannot run inside a transaction block at all, so the server would reject
/// the whole message; naming it here turns that into a sentence about the
/// guarantee rather than a syntax error from the driver.
///
/// **Quoted identifiers and string literals are skipped**, because an entity
/// named `commit` renders as `"commit"` and a default value may say anything
/// at all. A check that failed on those would refuse to provision a tenant
/// over a word in their data model, which is a worse failure than the one it
/// is guarding against and far more likely. What is left after skipping them
/// is keywords, punctuation and unquoted names, where these words can only
/// be what they look like.
///
/// The check is here rather than in a test because the DDL it reads is the
/// serving artifact's, which changes with every deploy. A test would assert
/// it of today's fixture; this asserts it of the string actually sent.
fn breaks_the_implicit_transaction(ddl: &str) -> Option<&'static str> {
    const FATAL: [&str; 5] = ["begin", "commit", "rollback", "savepoint", "concurrently"];
    for word in bare_words(ddl) {
        if let Some(found) = FATAL.iter().find(|f| word.eq_ignore_ascii_case(f)) {
            return Some(found);
        }
    }
    None
}

/// Every unquoted word in a SQL string, with `"identifiers"` and
/// `'literals'` dropped whole.
///
/// The doubled-quote escapes both kinds use (`""` inside an identifier,
/// `''` inside a literal) close and immediately reopen the quoted run, so
/// treating each quote as a toggle skips them correctly without a special
/// case.
fn bare_words(sql: &str) -> Vec<&str> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut word_start) = (0usize, None::<usize>);
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None if c == b'"' || c == b'\'' => {
                if let Some(s) = word_start.take() {
                    out.push(&sql[s..i]);
                }
                quote = Some(c);
            }
            None if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' => {
                word_start.get_or_insert(i);
            }
            None => {
                if let Some(s) = word_start.take() {
                    out.push(&sql[s..i]);
                }
            }
        }
        i += 1;
    }
    if let Some(s) = word_start {
        out.push(&sql[s..]);
    }
    out
}

/// What a target's own `$migrations` says its tables are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetVersion {
    /// The version the target's most recent apply brought it to. The
    /// target's own row, which R-S2 makes the authority: the control
    /// database's fleet view is a cache of these and never the other way
    /// round.
    At(String),
    /// **Version unknown.** The target has a schema and no record of which
    /// one, which is every database provisioned before R-S1 landed.
    ///
    /// This is not an error and it is not repaired here. A row written now
    /// would name whichever version happens to be serving, which is a guess
    /// about a database whose tables were created by some earlier artifact;
    /// once written it would be indistinguishable from a version that was
    /// actually applied, and every later decision would trust it. R-S4
    /// reports this state to an operator, who resolves it once with the
    /// knowledge this process does not have.
    Unknown,
}

/// Read the version a target records for itself.
///
/// Absent table and empty table are one answer, because they are one
/// situation: nothing here has ever written a version. A caller that needs
/// to know whether the target has a *schema* asks about `members`; this
/// function answers only what the target says about its version.
pub async fn recorded_version(pool: &Pool) -> Result<TargetVersion, PoolError> {
    let present = pool
        .rows("SELECT to_regclass('public.\"$migrations\"') IS NOT NULL")
        .await?;
    if present
        .first()
        .and_then(|r| r.first())
        .and_then(|c| c.as_deref())
        != Some("t")
    {
        return Ok(TargetVersion::Unknown);
    }
    let rows = pool
        .rows(
            "SELECT \"to_version\" FROM \"$migrations\" WHERE \"phase\" <> 'failed' \
             ORDER BY \"seq\" DESC LIMIT 1",
        )
        .await?;
    Ok(
        match rows.first().and_then(|r| r.first()).and_then(|c| c.clone()) {
            Some(v) => TargetVersion::At(v),
            None => TargetVersion::Unknown,
        },
    )
}

pub async fn ensure_table(pool: &Pool) -> Result<(), PoolError> {
    // Retried once for `Pool::ensure_platform_tables`' reason: `CREATE
    // TABLE IF NOT EXISTS` is not race-free between two booting replicas.
    if pool.rows(SPACEDB_DDL).await.is_err() {
        pool.rows(SPACEDB_DDL).await?;
    }
    Ok(())
}

/// The space's attachment, or `None` for a space on the default database.
///
/// A stored custody this build cannot read is an error rather than a
/// default. A bucket attachment falls back to the stronger position; here
/// either fallback would print a consent sentence that may be false.
pub async fn lookup(pool: &Pool, space: &SpaceId) -> Result<Option<DbAttachment>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT {COLUMNS} FROM \"{SPACEDB_TABLE}\" WHERE \"$space\" = {}",
            sql_lit(&space.0)
        ))
        .await?;
    rows.first().map(|row| parse_row(row, space)).transpose()
}

/// Forget a space's attachment, for `std.space.reap` (11-15).
///
/// **The one exception to `AttachRefusal::WouldMove`'s rule that nothing
/// changes an attachment.** C-M2 says `WouldMove` is the single place a
/// move has to be argued with, and this does not move anything: it removes
/// the row for a space that never became `active`, on its way to
/// `reaped`. A reap of an `active` space is refused by the op, not here.
///
/// Idempotent, because a reap must be able to finish a cleanup that a
/// previous attempt started.
pub async fn detach(pool: &Pool, space: &SpaceId) -> Result<bool, PoolError> {
    let rows = pool
        .rows(&format!(
            "DELETE FROM \"{SPACEDB_TABLE}\" WHERE \"$space\" = {} RETURNING \"$space\"",
            sql_lit(&space.0)
        ))
        .await?;
    Ok(!rows.is_empty())
}

/// Every attachment, in space order, for a caller that has to visit every
/// target: the door's directory, and the loops that walk every database.
pub async fn all(pool: &Pool) -> Result<Vec<(SpaceId, DbAttachment)>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"$space\", {COLUMNS} FROM \"{SPACEDB_TABLE}\" ORDER BY \"$space\""
        ))
        .await?;
    rows.iter()
        .map(|row| {
            let space = SpaceId(row.first().and_then(|c| c.clone()).unwrap_or_default());
            let a = parse_row(&row[1..], &space)?;
            Ok((space, a))
        })
        .collect()
}

/// Who else is already in this database (18d R-X1).
///
/// **Occupancy is a recorded fact, not a sentence.** The 2026-09-21 ruling
/// on D-S8 put the split here: *"the platform records and exposes the fact
/// and the developer writes the sentence."* So this returns spaces and
/// their custody and writes no prose, and [`DbCustody::disclosure`] stays
/// what it was rather than growing a clause it cannot fill in.
///
/// **Why the target and not the space is the key.** A pool is per target,
/// not per tenant (R-X1), so every space this returns shares one pool, one
/// credential, and one database in which the only separation is the
/// `$space` column. That is the shared tier's isolation whatever the
/// custody column says, and a caller that wants to state the difference
/// needs the list rather than the count.
///
/// Matched on host, port and database, the three columns that say where the
/// rows are. Not on `credential`: two spaces reaching one database under
/// two credential labels are still two spaces in one database.
pub async fn occupants(
    pool: &Pool,
    host: &str,
    port: u16,
    db: &str,
) -> Result<Vec<(SpaceId, DbAttachment)>, PoolError> {
    Ok(all(pool)
        .await?
        .into_iter()
        .filter(|(_, a)| a.host == host && a.port == port && a.db == db)
        .collect())
}

/// The attachment's columns, in the order [`parse_row`] reads them.
const COLUMNS: &str = "\"host\", \"port\", \"user\", \"db\", \"credential\", \"custody\", \
                       \"attached_at\", \"attached_by\"";

/// One row of [`COLUMNS`], read the one way both readers read it.
fn parse_row(row: &[Option<String>], space: &SpaceId) -> Result<DbAttachment, PoolError> {
    let cell = |i: usize| row.get(i).and_then(|c| c.as_deref()).unwrap_or_default();
    let custody = DbCustody::parse(cell(5)).ok_or_else(|| {
        PoolError::db(format!(
            "`{SPACEDB_TABLE}` names custody `{}` for space `{}`, which this build does not know",
            cell(5),
            space.0
        ))
    })?;
    let port = cell(1).parse().map_err(|_| {
        PoolError::db(format!(
            "`{SPACEDB_TABLE}` port is not a port: `{}`",
            cell(1)
        ))
    })?;
    Ok(DbAttachment {
        host: cell(0).to_string(),
        port,
        user: cell(2).to_string(),
        db: cell(3).to_string(),
        credential: cell(4).to_string(),
        custody,
        attached_at: cell(6).parse().unwrap_or(0),
        attached_by: cell(7).to_string(),
    })
}

/// Attach a space to a database, keyed on the target's identity.
///
/// One statement that inserts only where no row exists, then a read of the
/// row that stands. Two callers racing to attach the same space therefore
/// cannot both succeed with different targets: the loser's insert does
/// nothing, and it is told what won.
pub async fn attach(
    pool: &Pool,
    space: &SpaceId,
    want: &DbAttachment,
) -> Result<DbAttachOutcome, AttachRefusal> {
    let inserted = pool
        .rows(&format!(
            "INSERT INTO \"{SPACEDB_TABLE}\" \
             (\"$space\", \"host\", \"port\", \"user\", \"db\", \"credential\", \"custody\", \
              \"attached_at\", \"attached_by\") \
             VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}) \
             ON CONFLICT (\"$space\") DO NOTHING \
             RETURNING 1",
            sql_lit(&space.0),
            sql_lit(&want.host),
            want.port,
            sql_lit(&want.user),
            sql_lit(&want.db),
            sql_lit(&want.credential),
            sql_lit(want.custody.as_str()),
            want.attached_at,
            sql_lit(&want.attached_by),
        ))
        .await?;
    if !inserted.is_empty() {
        return Ok(DbAttachOutcome::Attached);
    }
    let standing = lookup(pool, space).await?.ok_or_else(|| {
        // The insert conflicted, so a row existed; a row that is gone a
        // moment later was removed by something this build has no path for.
        AttachRefusal::Pool(PoolError::db(format!(
            "`{SPACEDB_TABLE}` conflicted for space `{}` and then held no row",
            space.0
        )))
    })?;
    if standing.identity() == want.identity() {
        Ok(DbAttachOutcome::Unchanged)
    } else {
        Err(AttachRefusal::WouldMove {
            current: format!("{}:{}/{}", standing.host, standing.port, standing.db),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> DbAttachment {
        DbAttachment {
            host: "db-eu-1.internal".into(),
            port: 5432,
            user: "ordo_acme".into(),
            db: "acme".into(),
            credential: "acme-prod".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "mem_a".into(),
        }
    }

    /// The table holds no secret, and the absence is asserted rather than
    /// reviewed: this is the control database, so a secret here would be a
    /// secret in every backup of the one database every tenant depends on.
    #[test]
    fn the_attachment_table_has_no_secret_column() {
        let lower = SPACEDB_DDL.to_lowercase();
        for forbidden in [
            "secret",
            "password",
            "passwd",
            "token",
            "access_key",
            "dsn",
            "url",
        ] {
            assert!(
                !lower.contains(forbidden),
                "`{forbidden}` in the database attachment table: the credential is a \
                 REFERENCE, and a secret here is a secret in every backup"
            );
        }
        assert!(SPACEDB_DDL.contains("\"credential\" text"));
    }

    /// Both positions say what deleting the space does, and they differ on
    /// exactly the two things custody changes.
    #[test]
    fn a_custody_choice_states_its_disclosure() {
        let managed = DbCustody::PlatformManaged.disclosure();
        let own = DbCustody::OwnPerimeter.disclosure();
        for d in [managed, own] {
            assert!(
                d.contains("permanently unreadable"),
                "sealed values are unreadable after erasure in either position: {d}"
            );
        }
        assert!(managed.contains("deletes the space's rows"));
        assert!(managed.contains("without asking you"));
        assert!(own.contains("removing those is yours to do"));
        assert!(own.contains("needs read access to your database"));
        for c in [DbCustody::PlatformManaged, DbCustody::OwnPerimeter] {
            assert_eq!(DbCustody::parse(c.as_str()), Some(c));
        }
        assert_eq!(
            DbCustody::parse("platform-sealed"),
            None,
            "a bucket's word is not a database's"
        );
    }

    /// A retry is the same target; anything that says where the rows are is
    /// a different one.
    #[test]
    fn identity_ignores_when_and_who_and_nothing_else() {
        let retried = DbAttachment {
            attached_at: 999,
            attached_by: "mem_b".into(),
            ..base()
        };
        assert_eq!(base().identity(), retried.identity());
        for changed in [
            DbAttachment {
                host: "db-us-1.internal".into(),
                ..base()
            },
            DbAttachment {
                port: 6432,
                ..base()
            },
            DbAttachment {
                user: "ordo_other".into(),
                ..base()
            },
            DbAttachment {
                db: "acme2".into(),
                ..base()
            },
            DbAttachment {
                credential: "acme-staging".into(),
                ..base()
            },
            DbAttachment {
                custody: DbCustody::OwnPerimeter,
                ..base()
            },
        ] {
            assert_ne!(base().identity(), changed.identity());
        }
    }
}
