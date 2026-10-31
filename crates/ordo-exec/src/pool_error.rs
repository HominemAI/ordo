//! The database client's one typed error (AGENTS.md rule 9, owner
//! 2026-10-23; `docs/audit-service-interfaces.md`, "1. Database").
//!
//! Every layer above the pool converts from [`PoolError`] and decides by its
//! type: [`PoolError::class`] says what a caller can do (busy, retryable,
//! refused, broken), the narrower questions ([`PoolError::is_busy`],
//! [`PoolError::is_missing_table`], ...) are methods here, and the driver's
//! own error is kept as `source()` wherever there was one. Nothing above
//! reads the words to decide; `ordo-host/tests/db_error_text_scan.rs` keeps
//! it that way.
//!
//! The `Display` text keeps its `<class>: <message>` shape, because the
//! write protocol's refusals still travel as text to the door, but no
//! decision is made from it.

use std::fmt;
use std::sync::Arc;

pub use ordo_fault::FaultClass;

use crate::{route, WRITE_RETRIES_ENV};

/// What [`PoolError::Saturated`] says, word for word.
///
/// Class-prefixed for the write protocol's convention, so `classify` in
/// `ordo-host` reads it back as `server_busy` (reported fix 25z). `ordo_api`
/// holds the same string as `SERVER_BUSY_MESSAGE`, and a test in
/// `ordo-host` holds the two equal. Nothing decides by comparing to it:
/// busy is [`PoolError::is_busy`].
pub const SATURATED_MESSAGE: &str = "server_busy: every database connection this deployment \
     holds is in use. Nothing ran, and the same request can be sent again shortly";

/// SQLSTATE 53300 `too_many_connections`: the database server is at its
/// `max_connections`. Busy, the same as a full pool.
pub const FULL_DATABASE: &str = "53300";
/// SQLSTATE 57P03 `cannot_connect_now`: the server is starting up. Busy.
pub const STARTING_DATABASE: &str = "57P03";

/// The cause a database error carries.
pub type Cause = Arc<dyn std::error::Error + Send + Sync + 'static>;

/// What a database error says, and the driver error under it when there
/// was one.
///
/// Reads as its text (`Deref<Target = str>`, `Display`), so a caller that
/// only logs it is unchanged; [`DbDetail::cause`] and `source()` on the
/// [`PoolError`] that holds it give the driver error back, with its
/// SQLSTATE, for a caller that decides.
#[derive(Clone, Default)]
pub struct DbDetail {
    text: String,
    cause: Option<Cause>,
}

impl DbDetail {
    /// Text, and the error that caused it.
    pub fn caused_by(
        text: impl Into<String>,
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> DbDetail {
        DbDetail {
            text: text.into(),
            cause: Some(Arc::new(cause)),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn into_string(self) -> String {
        self.text
    }

    /// The driver error under this one, if there was one.
    pub fn cause(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.cause.as_deref()
    }

    /// The five-character SQLSTATE the server answered, if the cause is a
    /// server error.
    pub fn sqlstate(&self) -> Option<&str> {
        self.cause()?
            .downcast_ref::<tokio_postgres::Error>()?
            .code()
            .map(|c| c.code())
    }
}

impl From<String> for DbDetail {
    fn from(text: String) -> DbDetail {
        DbDetail { text, cause: None }
    }
}

impl From<&str> for DbDetail {
    fn from(text: &str) -> DbDetail {
        DbDetail::from(text.to_string())
    }
}

impl std::ops::Deref for DbDetail {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl AsRef<str> for DbDetail {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

impl PartialEq for DbDetail {
    fn eq(&self, other: &DbDetail) -> bool {
        self.text == other.text
    }
}

impl Eq for DbDetail {}

impl fmt::Debug for DbDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.text, f)
    }
}

impl fmt::Display for DbDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<DbDetail> for String {
    fn from(d: DbDetail) -> String {
        d.text
    }
}

/// The database client's error.
///
/// `#[non_exhaustive]`: a caller outside this crate decides by
/// [`PoolError::class`] or one of the `is_*` methods, and a variant added
/// later reaches its wildcard arm instead of failing to compile it.
#[derive(Debug)]
#[non_exhaustive]
pub enum PoolError {
    /// The acquire timeout elapsed — the deployment is at its declared
    /// concurrency bound, or the **space's share** of it. Retryable by
    /// design. It reaches a caller as `server_busy`, a 503
    /// ([`SATURATED_MESSAGE`]).
    Saturated,
    /// **The arithmetic, not the server.** `/ 0` is a typed runtime error
    /// uniform with `mod(a, 0)` (DL §2, Division), and `int`/`money`
    /// overflow is a typed runtime error rather than wraparound (DL §2,
    /// Numeric semantics). Both arrive from Postgres as ordinary query
    /// failures, and the only thing separating them from a genuine server
    /// fault is the SQLSTATE — so it is read here, once, rather than by
    /// every caller matching on English error text.
    ///
    /// The name is class-prefixed for the write protocol's convention
    /// (`<class>: <message>`), so the layer that maps engine classes onto
    /// the wire inventory sees the same shape it already parses.
    ///
    /// Nothing here is `nullif`: silently answering NULL for a division by
    /// zero is the folklore DL §2 explicitly refuses.
    Arithmetic(DbDetail),
    /// **The data, not the server.** A declared row invariant (`check`)
    /// refused the write — SQLSTATE 23514, read here once like the
    /// arithmetic family, for the same reason: the only thing separating
    /// "your data violated `used <= seats`" from a genuine fault is the
    /// SQLSTATE, and a caller told `internal` cannot tell them apart.
    /// The message carries the constraint name, which carries the
    /// condition — that is the whole diagnostic channel Postgres offers.
    CheckViolation(DbDetail),
    /// **Retryable.** Postgres could not maintain the serializable
    /// ordering (SQLSTATE `40001`) or broke a deadlock (`40P01`). Neither
    /// says anything went wrong — they say *this attempt* did not happen,
    /// and the work must be re-executed from scratch.
    ///
    /// Typed separately from [`PoolError::Db`] for the same reason
    /// `Arithmetic` is: the retry decision cannot be made by matching
    /// English error text, and a serialization failure landing in the
    /// generic bucket is an internal error the caller cannot act on.
    Serialization(DbDetail),
    /// **The clock, not the server.** One statement ran past the
    /// deployment's [`crate::PoolLimits::statement_timeout`] and Postgres
    /// cancelled it (SQLSTATE 57014), so the transaction it was in is
    /// dead and nothing it was doing landed.
    ///
    /// Typed rather than left in [`PoolError::Db`] for the reason the
    /// arithmetic family is typed: the only thing separating "your query
    /// was too slow for this deployment" from a broken server is the
    /// SQLSTATE, and a caller told `internal` cannot tell them apart or
    /// act. This one they can act on, by asking for less.
    ///
    /// Class-prefixed for the write protocol's convention, and the class
    /// is in `ordo_api::ErrorCode`, so it reaches a caller as itself.
    StatementTimeout(DbDetail),
    /// **Another session's lock, not the server.** A statement waited
    /// `lock_timeout` for a lock another session holds and Postgres gave
    /// up on it (SQLSTATE 55P03, `lock_not_available`). The transaction it
    /// was in is dead and nothing it was doing landed.
    ///
    /// Typed for the reason the other three are: the schema apply retries
    /// exactly this and nothing else (`plan-evolution-plans.md` §3.7), and
    /// a retry decision made by matching English error text is the thing
    /// this enum exists to avoid. Only a statement that set `lock_timeout`
    /// can see it; the request path sets none.
    LockTimeout(DbDetail),
    /// **The caller's value, not the server.** A data exception (SQLSTATE
    /// class 22) other than the arithmetic pair: an invalid regular
    /// expression for `matches` (2201B, also what a pattern past
    /// [`crate::REGEX_MAX_LEN_ENV`] or holding a backreference meets), text
    /// that is not the type it was cast to (22P02), a value too long for
    /// its column (22001), and the like. It reaches a caller as
    /// `invalid_param` with a clean sentence ([`invalid_input_text`]); the
    /// server's own words stay in the cause.
    InvalidInput(DbDetail),
    /// The retry budget for a serializable operation ran out. This is
    /// `implementation.md`'s **retryable conflict**, flagged there as
    /// `serialization_conflict` [proposed] because the wire taxonomy in
    /// `api-engine-design.md` §4 has no code for it yet. It is emphatically
    /// not an internal error: the caller may try again.
    ///
    /// `attempts` is every try, the first included, so the setting that
    /// allowed them, [`WRITE_RETRIES_ENV`], was `attempts - 1`. The message
    /// names it. `last` is the conflict the final try lost to, and is this
    /// error's `source()`.
    RetryExhausted { attempts: u32, last: Box<PoolError> },
    /// **The node's tenant dial gate did not free in time.** A walker
    /// (boot pass, catch-up sweep, deploy) waited
    /// [`route::TENANT_DIAL_WAIT_MS_ENV`] for a permit of the gate set by
    /// [`route::TENANT_DIAL_CONCURRENCY_ENV`] and gave up, so this tenant's
    /// step did not run. Nothing was changed. Retryable: the walker holds
    /// the tenant and the next pass tries it again. The value is the wait
    /// that ran out, in milliseconds.
    DialWaitExceeded(u64),
    /// Dial or protocol failure, or a row this platform cannot read. The
    /// driver error, when there was one, is the detail's cause.
    Db(DbDetail),
}

impl PoolError {
    /// A [`PoolError::Db`] from anything that reads as its text.
    pub fn db(detail: impl Into<DbDetail>) -> PoolError {
        PoolError::Db(detail.into())
    }

    /// A driver error, typed by its SQLSTATE, with the driver error kept as
    /// the cause. `text` is what the error says; the driver's own `Display`
    /// is only its kind ("db error"), so the caller builds the words.
    pub(crate) fn from_driver(e: tokio_postgres::Error, text: String) -> PoolError {
        let code = e.code().map(|c| c.code().to_string());
        if let Some(c) = code.as_deref().filter(|c| invalid_input(c)) {
            let clean = invalid_input_text(c, e.as_db_error().map(|d| d.message()));
            return PoolError::InvalidInput(DbDetail::caused_by(clean, e));
        }
        // The severity is not the caller's to read: the typed variants
        // below say what happened in their own class.
        let text = match text.strip_prefix("ERROR: ") {
            Some(rest) => rest.to_string(),
            None => text,
        };
        let detail = DbDetail::caused_by(text, e);
        match code.as_deref() {
            // 22012 division_by_zero (`/ 0`, `mod(a, 0)`) and 22003
            // numeric_value_out_of_range (int/money overflow). DL §2 calls
            // both typed runtime errors; typing them here is what keeps
            // them out of the `internal` bucket, where a caller cannot tell
            // "you divided by zero" from "the server is broken".
            Some("22012" | "22003") => PoolError::Arithmetic(detail),
            // 23514 check_violation — a declared row invariant refused the
            // write. The server did not break and the caller's data is the
            // problem, so this must never wear `internal`. The constraint
            // name in the message is the diagnostic: it carries the
            // condition (`teams$chk0$used<=seats`), because the name is the
            // only channel Postgres gives us wide enough to say WHICH check.
            Some("23514") => PoolError::CheckViolation(detail),
            // 40001 serialization_failure and 40P01 deadlock_detected. Both
            // mean "this attempt did not happen", and both are the
            // transaction's whole answer — the connection is fine, the
            // work is not done, and re-running it is correct. The pin in
            // `implementation.md` is only real because this line exists:
            // without it a SERIALIZABLE transaction fails as `db:` and the
            // caller has no way to tell a conflict from a broken server.
            Some("40001" | "40P01") => PoolError::Serialization(detail),
            // 57014 query_canceled: the statement ran past the bound the
            // deployment set on every pooled connection (20a). Read here
            // beside the other three for the same reason. It is the
            // caller's to act on, and a 500 would tell them the server
            // broke. A cancel this platform did not ask for arrives the
            // same way, and reads the same to a caller: the statement did
            // not finish and nothing it did landed.
            Some("57014") => PoolError::StatementTimeout(detail),
            // 55P03 lock_not_available: the statement set `lock_timeout`
            // and another session held the lock past it. Two callers set
            // it: the schema apply, which retries on this variant alone
            // (`plan-evolution-plans.md` §3.7), and the apply op's lock,
            // which reads it as `apply_in_progress`.
            Some("55P03") => PoolError::LockTimeout(detail),
            _ => PoolError::Db(detail),
        }
    }

    /// The retry budget ran out; `last` is the conflict the final try lost
    /// to. `None` only if no try ran, which a budget of at least one
    /// prevents.
    pub(crate) fn exhausted(attempts: u32, last: Option<PoolError>) -> PoolError {
        PoolError::RetryExhausted {
            attempts,
            last: Box::new(
                last.unwrap_or_else(|| PoolError::Serialization("no attempt ran".into())),
            ),
        }
    }

    /// What a caller can do about this error. Read from the variant, and
    /// for [`PoolError::Db`] from the server's SQLSTATE: an integrity or
    /// data error (classes 22 and 23) is the request's, refused.
    ///
    /// `Busy` is a full pool, or a database server that is full or starting
    /// ([`FULL_DATABASE`], [`STARTING_DATABASE`]). [`Self::is_busy`] reads
    /// this, so the two always agree.
    pub fn class(&self) -> FaultClass {
        match self {
            PoolError::Saturated => FaultClass::Busy,
            PoolError::Serialization(_)
            | PoolError::LockTimeout(_)
            | PoolError::RetryExhausted { .. }
            | PoolError::DialWaitExceeded(_) => FaultClass::Retryable,
            PoolError::Arithmetic(_)
            | PoolError::CheckViolation(_)
            | PoolError::InvalidInput(_)
            | PoolError::StatementTimeout(_) => FaultClass::Refused,
            PoolError::Db(d) => match d.sqlstate() {
                // 53300 too_many_connections and 57P03 cannot_connect_now:
                // the database itself is full or still starting. Busy, as a
                // full pool is: nothing ran, and the same request lands once
                // connections free (reported fix pool-exhaustion-reads-as-
                // denied). Broken would make the door deny instead.
                Some(FULL_DATABASE | STARTING_DATABASE) => FaultClass::Busy,
                Some(s) if s.starts_with("22") || s.starts_with("23") => FaultClass::Refused,
                _ => FaultClass::Broken,
            },
        }
    }

    /// Is this failure one that passes by itself: a full pool, a deadlock or
    /// serialization conflict, a lock wait or a statement that ran past its
    /// time bound? The door's rebuild keeps a space's last good member list
    /// through these and through nothing else (owner, 2026-10-27).
    pub fn is_passing(&self) -> bool {
        self.class().may_retry() || matches!(self, PoolError::StatementTimeout(_))
    }

    /// Is re-running the whole unit of work, now, the right response? A
    /// serialization conflict or a deadlock: what [`crate::Pool::retrying`]
    /// and the write loops retry. Narrower than [`FaultClass::Retryable`],
    /// which also holds the waits a caller retries later.
    pub fn is_retryable(&self) -> bool {
        matches!(self, PoolError::Serialization(_))
    }

    /// Was the deployment busy rather than broken? Every connection was in
    /// use (this pool's, or the database server's), nothing ran, and the
    /// same request can be sent again shortly.
    /// The one place this is decided: a store above keeps the answer in its
    /// own typed error (`ordo_fault::StoreFault`) instead of matching the
    /// words of [`SATURATED_MESSAGE`].
    pub fn is_busy(&self) -> bool {
        self.class() == FaultClass::Busy
    }

    /// Another session held a lock past this statement's `lock_timeout`.
    pub fn is_lock_timeout(&self) -> bool {
        matches!(self, PoolError::LockTimeout(_))
    }

    /// The SQLSTATE the server answered, when it answered one.
    pub fn sqlstate(&self) -> Option<&str> {
        self.detail()?.sqlstate()
    }

    /// A table the statement named does not exist (SQLSTATE 42P01): the
    /// deployment has not created its platform tables, or not this one yet.
    pub fn is_missing_table(&self) -> bool {
        self.sqlstate() == Some("42P01")
    }

    /// A column the statement named does not exist (SQLSTATE 42703): the
    /// table predates the column.
    pub fn is_missing_column(&self) -> bool {
        self.sqlstate() == Some("42703")
    }

    /// A foreign key refused the statement (SQLSTATE 23503): other rows
    /// still refer to the one being removed, or the one referred to is not
    /// there.
    pub fn is_foreign_key_violation(&self) -> bool {
        self.sqlstate() == Some("23503")
    }

    /// A unique constraint refused the statement (SQLSTATE 23505): the row
    /// it would write already exists under a key that must be unique.
    pub fn is_unique_violation(&self) -> bool {
        self.sqlstate() == Some("23505")
    }

    /// The server refused the credentials (SQLSTATE 28P01 or 28000).
    pub fn is_auth_failure(&self) -> bool {
        matches!(self.sqlstate(), Some("28P01" | "28000"))
    }

    /// This error as a store's fault: its class, its words, and itself as
    /// the cause. For a store whose words may name what it was asked
    /// (an identifier), build the fault by hand and keep the cause only
    /// when it names none.
    pub fn into_fault(self) -> ordo_fault::StoreFault {
        let text = self.to_string();
        ordo_fault::StoreFault::caused_by(self.class(), text, self)
    }

    /// The detail the variant carries, if it carries one.
    pub fn detail(&self) -> Option<&DbDetail> {
        match self {
            PoolError::Arithmetic(d)
            | PoolError::CheckViolation(d)
            | PoolError::Serialization(d)
            | PoolError::StatementTimeout(d)
            | PoolError::LockTimeout(d)
            | PoolError::InvalidInput(d)
            | PoolError::Db(d) => Some(d),
            PoolError::RetryExhausted { last, .. } => last.detail(),
            PoolError::Saturated | PoolError::DialWaitExceeded(_) => None,
        }
    }
}

impl std::error::Error for PoolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PoolError::RetryExhausted { last, .. } => Some(last.as_ref()),
            other => other
                .detail()?
                .cause()
                .map(|c| c as &(dyn std::error::Error + 'static)),
        }
    }
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PoolError::Saturated => f.write_str(SATURATED_MESSAGE),
            PoolError::Arithmetic(m) => write!(f, "arithmetic_error: {m}"),
            PoolError::CheckViolation(m) => write!(f, "check_violation: {m}"),
            PoolError::Serialization(m) => write!(f, "serialization_conflict: {m}"),
            PoolError::StatementTimeout(m) => write!(f, "statement_timeout: {m}"),
            PoolError::LockTimeout(m) => write!(f, "lock_timeout: {m}"),
            PoolError::InvalidInput(m) => write!(f, "invalid_param: {m}"),
            PoolError::RetryExhausted { attempts, last } => write!(
                f,
                "serialization_conflict: other writes to the same rows won {attempts} times \
                 in a row, so nothing was applied and the request can be sent again. The \
                 retries are {WRITE_RETRIES_ENV}={}; last: {last}",
                attempts.saturating_sub(1)
            ),
            PoolError::DialWaitExceeded(ms) => write!(
                f,
                "dial_wait_exceeded: waited {ms} ms for a free slot to dial this tenant's \
                 database and none came, so nothing was done for it and it can be tried \
                 again. The wait is {}={ms}; the slots are {}",
                route::TENANT_DIAL_WAIT_MS_ENV,
                route::TENANT_DIAL_CONCURRENCY_ENV
            ),
            PoolError::Db(m) => write!(f, "db: {m}"),
        }
    }
}

/// A data exception the request caused (SQLSTATE class 22), other than
/// the arithmetic pair 22012 and 22003, which are `arithmetic_error`.
fn invalid_input(sqlstate: &str) -> bool {
    sqlstate.starts_with("22") && !matches!(sqlstate, "22012" | "22003")
}

/// What a caller reads for a data exception: what to fix, never the
/// server's severity or the `db:` prefix. A pattern's own fault says the
/// rules a `matches` pattern must keep; any other says what the server
/// refused, which names only the caller's own value.
pub fn invalid_input_text(sqlstate: &str, server: Option<&str>) -> String {
    if sqlstate == "2201B" {
        return format!(
            "the pattern is not one this deployment runs. A `matches` pattern must be a \
             valid regular expression, hold no backreference (`\\1` to `\\9`), and be at \
             most {} characters (the setting {}). Nothing ran; send a different pattern",
            crate::regex_max_len(),
            crate::REGEX_MAX_LEN_ENV
        );
    }
    match server.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => format!(
            "a value in this request is not one the database accepts ({m}). Nothing ran; \
             fix the value and send it again"
        ),
        None => "a value in this request is not one the database accepts. Nothing ran; fix \
                 the value and send it again"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_a_class() {
        assert_eq!(PoolError::Saturated.class(), FaultClass::Busy);
        assert_eq!(
            PoolError::Serialization("x".into()).class(),
            FaultClass::Retryable
        );
        assert_eq!(
            PoolError::LockTimeout("x".into()).class(),
            FaultClass::Retryable
        );
        assert_eq!(
            PoolError::DialWaitExceeded(5).class(),
            FaultClass::Retryable
        );
        assert_eq!(PoolError::exhausted(3, None).class(), FaultClass::Retryable);
        assert_eq!(
            PoolError::Arithmetic("x".into()).class(),
            FaultClass::Refused
        );
        assert_eq!(
            PoolError::CheckViolation("x".into()).class(),
            FaultClass::Refused
        );
        assert_eq!(
            PoolError::StatementTimeout("x".into()).class(),
            FaultClass::Refused
        );
        assert_eq!(PoolError::db("x").class(), FaultClass::Broken);
    }

    #[test]
    fn busy_is_the_full_pool_alone() {
        assert!(PoolError::Saturated.is_busy());
        assert!(PoolError::Saturated.class().is_busy());
        // Words that look busy are not busy: the type decides.
        assert!(!PoolError::db(SATURATED_MESSAGE).is_busy());
        assert!(!PoolError::db(SATURATED_MESSAGE).class().is_busy());
        assert!(!PoolError::DialWaitExceeded(1).is_busy());
    }

    #[test]
    fn the_cause_is_kept() {
        let io = std::io::Error::other("connection reset by peer");
        let e = PoolError::Db(DbDetail::caused_by("connection reset by peer", io));
        let source = std::error::Error::source(&e).expect("the cause is the source");
        assert_eq!(source.to_string(), "connection reset by peer");
        assert_eq!(e.to_string(), "db: connection reset by peer");
        // A cause that is not a server error has no SQLSTATE.
        assert_eq!(e.sqlstate(), None);
        assert!(!e.is_missing_table());

        let exhausted = PoolError::exhausted(2, Some(e));
        let last = std::error::Error::source(&exhausted).expect("the last try is the source");
        assert_eq!(last.to_string(), "db: connection reset by peer");
        assert!(exhausted
            .to_string()
            .ends_with("last: db: connection reset by peer"));
    }

    #[test]
    fn text_only_errors_have_no_cause() {
        let e = PoolError::db("malformed sink row");
        assert!(std::error::Error::source(&e).is_none());
        assert_eq!(e.to_string(), "db: malformed sink row");
    }
}
