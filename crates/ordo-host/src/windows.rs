//! **`Windows`: the one client for maintenance windows** (AGENTS.md rule
//! 9; owner 2026-11-04, board `program-versions-the-core-under-schema`
//! asks 19, 21, 22 and 24).
//!
//! A maintenance window is a block of time the app developer has agreed
//! with a tenant. Ordo stores and lists it, and it changes nothing by
//! itself (ask 24: "It's just a window - a maintenance block - that is
//! all"). Other features read it later. Nothing here knows about versions.
//!
//! Everything that reads or writes `$windows`, `$window_tenants` or
//! `$window_skips` goes through [`Windows`]. Its one error is
//! [`WindowRefusal`], and [`WindowRefusal::to_exec_error`] is the one place
//! it becomes an answer at the door.
//!
//! # What a window is
//!
//! - **When** ([`Schedule`]): a range with a start and an end, or a cron
//!   (standard 5 fields, UTC) with a length, which repeats it. UTC as
//!   every Ordo schedule is; a time zone is a later extension.
//! - **Who** ([`Target`]): every tenant, only the tenants listed, or every
//!   tenant except those listed. The rule is stored, not a list it
//!   expands to, so `except` covers a tenant created later.
//! - **A message**: any text, never rendered, at most
//!   [`MAX_MESSAGE_CHARS`] characters.
//! - **Skips**: a time range during which a cron window's times are
//!   suppressed, with an optional reason.
//!
//! The deployment owner sets them. The tenant has no say through Ordo: the
//! window is the app developer's contract with the tenant.
//!
//! # The list
//!
//! [`Windows::list`] answers every time from 12 months back to 12 months
//! ahead. A cron window is listed as each time the cron gives. A skipped
//! time is kept and marked with the skip that covers it, so a console can
//! show it struck through. A time is skipped when it starts inside a skip.
//! The list holds at most [`MAX_OCCURRENCES`] times and says when it was
//! cut; one cron window gives at most [`MAX_OCCURRENCES_PER_WINDOW`] in the
//! 24 months, which [`Windows::set`] checks.
//!
//! # Tenants
//!
//! A tenant named in a target, or as the list's filter, must exist and not
//! be reaped. `$window_tenants` has a `space` column, so erasing a tenant
//! deletes its rows there (`ordo_exec::erase`). An `only` window whose last
//! tenant was erased covers nobody and stays listed until it is deleted.

use std::collections::{BTreeMap, BTreeSet};

use ordo_api::{ErrorCode, ExecError};
use ordo_exec::{sql_lit, Pool, PoolError};
use ordo_fault::FaultClass;

/// The longest message a window may carry, in characters.
pub const MAX_MESSAGE_CHARS: usize = 2_000;
/// The longest reason a skip may carry, in characters.
pub const MAX_REASON_CHARS: usize = 500;
/// The longest window or skip id, in characters.
pub const MAX_ID_CHARS: usize = 64;
/// The most tenants one window may list.
pub const MAX_TENANTS: usize = 1_000;
/// How far back and ahead the list reaches, in months.
pub const LIST_MONTHS: i64 = 12;
/// The most times one cron window may give in the list's 24 months: about
/// once an hour. A cron that gives more is refused when it is set.
pub const MAX_OCCURRENCES_PER_WINDOW: usize = 20_000;
/// The most times one list answers. Past it the list is cut and says so.
pub const MAX_OCCURRENCES: usize = 100_000;

const DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$windows\" ( \
  \"id\" text PRIMARY KEY, \
  \"target\" text NOT NULL, \
  \"kind\" text NOT NULL, \
  \"starts_at\" bigint NULL, \
  \"ends_at\" bigint NULL, \
  \"cron\" text NULL, \
  \"length_secs\" bigint NULL, \
  \"message\" text NOT NULL, \
  \"set_by\" text NOT NULL, \
  \"set_at\" bigint NOT NULL \
); \
CREATE TABLE IF NOT EXISTS \"$window_tenants\" ( \
  \"window_id\" text NOT NULL, \
  \"space\" text NOT NULL, \
  PRIMARY KEY (\"window_id\", \"space\") \
); \
CREATE INDEX IF NOT EXISTS \"$window_tenants_space\" ON \"$window_tenants\" (\"space\"); \
CREATE TABLE IF NOT EXISTS \"$window_skips\" ( \
  \"id\" text PRIMARY KEY, \
  \"window_id\" text NOT NULL, \
  \"starts_at\" bigint NOT NULL, \
  \"ends_at\" bigint NOT NULL, \
  \"reason\" text NULL, \
  \"set_by\" text NOT NULL, \
  \"set_at\" bigint NOT NULL \
)";

// ── the shapes ─────────────────────────────────────────────────────────

/// Which tenants a window covers.
///
/// The rule is stored, so `All` and `Except` cover tenants created after
/// the window was set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Every tenant in the deployment.
    All,
    /// Only these tenants.
    Only(BTreeSet<String>),
    /// Every tenant but these.
    Except(BTreeSet<String>),
}

impl Target {
    /// `all`, `only` or `except`: the rule's name on the wire and in
    /// `$windows.target`.
    pub fn kind(&self) -> &'static str {
        match self {
            Target::All => "all",
            Target::Only(_) => "only",
            Target::Except(_) => "except",
        }
    }

    /// The tenants listed, empty for `All`.
    pub fn tenants(&self) -> Vec<&str> {
        match self {
            Target::All => vec![],
            Target::Only(t) | Target::Except(t) => t.iter().map(String::as_str).collect(),
        }
    }

    /// Whether the window covers `space`.
    pub fn covers(&self, space: &str) -> bool {
        match self {
            Target::All => true,
            Target::Only(t) => t.contains(space),
            Target::Except(t) => !t.contains(space),
        }
    }

    /// A target from its rule's name and the tenants listed. Checked:
    /// `all` lists none, `only` and `except` at least 1 and at most
    /// [`MAX_TENANTS`].
    pub fn parse(kind: &str, tenants: &[String]) -> Result<Target, WindowRefusal> {
        let set: BTreeSet<String> = tenants
            .iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if set.len() > MAX_TENANTS {
            return Err(WindowRefusal::Input(format!(
                "a window lists at most {MAX_TENANTS} tenants; this one lists {}. Use `except` \
                 with the tenants left out, or `all`",
                set.len()
            )));
        }
        match kind.trim() {
            "all" if set.is_empty() => Ok(Target::All),
            "all" => Err(WindowRefusal::Input(
                "target `all` covers every tenant and lists none; use `only` or `except` to \
                 list tenants"
                    .into(),
            )),
            "only" | "except" if set.is_empty() => Err(WindowRefusal::Input(format!(
                "target `{}` needs at least 1 tenant (`--only <tenant>` or `--except <tenant>`, `tenants` on the door)",
                kind.trim()
            ))),
            "only" => Ok(Target::Only(set)),
            "except" => Ok(Target::Except(set)),
            other => Err(WindowRefusal::Input(format!(
                "target `{other}` is not one; it is `all`, `only` or `except`"
            ))),
        }
    }
}

/// When a window is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schedule {
    /// Once, from `start` to `end`, in seconds since 1970 UTC.
    Range { start: i64, end: i64 },
    /// Each time `cron` gives, for `length` seconds.
    Cron { cron: String, length: i64 },
}

/// One window, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: String,
    pub target: Target,
    pub schedule: Schedule,
    pub message: String,
    pub set_by: String,
    pub set_at: u64,
}

/// What [`Windows::set`] is given: a window without who set it and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSpec {
    /// `None` mints one. A given id replaces the window stored under it.
    pub id: Option<String>,
    pub target: Target,
    pub schedule: Schedule,
    pub message: String,
}

/// A time range during which a cron window's times are suppressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub id: String,
    pub window: String,
    pub start: i64,
    pub end: i64,
    pub reason: Option<String>,
    pub set_by: String,
    pub set_at: u64,
}

/// One time a window is in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub window: String,
    pub start: i64,
    pub end: i64,
    /// The window's rule: `all`, `only` or `except`.
    pub target: &'static str,
    /// The skip that suppresses it, when one does.
    pub skipped_by: Option<String>,
}

/// What [`Windows::list`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The span listed: 12 months back to 12 months ahead.
    pub from: i64,
    pub to: i64,
    /// The tenant asked about, when one was.
    pub tenant: Option<String>,
    /// Every window listed, by id.
    pub windows: Vec<Window>,
    /// Every skip of those windows.
    pub skips: Vec<Skip>,
    /// Each time in the span, by start.
    pub occurrences: Vec<Occurrence>,
    /// The list reached [`MAX_OCCURRENCES`] and stopped there.
    pub truncated: bool,
}

// ── the error ──────────────────────────────────────────────────────────

/// The one error of [`Windows`] (rule 9). Each variant keeps its cause and
/// says what the caller can do ([`WindowRefusal::remedy`]).
#[derive(Debug)]
#[non_exhaustive]
pub enum WindowRefusal {
    /// A value that is not one: a target, a time, a cron, a length, a
    /// message or reason over its cap. Fix the input.
    Input(String),
    /// Tenants named that do not exist or were reaped. Fix the input.
    UnknownTenants(Vec<String>),
    /// No window, or no skip, under this id. Fix the input.
    NotFound { what: &'static str, id: String },
    /// The database. Its class says whether a retry can help.
    Store(PoolError),
}

/// What a caller can do about a [`WindowRefusal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    Retry,
    Refuse,
    FixInput,
}

impl WindowRefusal {
    pub fn remedy(&self) -> Remedy {
        match self {
            WindowRefusal::Input(_)
            | WindowRefusal::UnknownTenants(_)
            | WindowRefusal::NotFound { .. } => Remedy::FixInput,
            WindowRefusal::Store(e) => match e.class() {
                FaultClass::Busy | FaultClass::Retryable | FaultClass::Broken => Remedy::Retry,
                _ => Remedy::Refuse,
            },
        }
    }

    /// **The one place a `WindowRefusal` becomes a door answer.** `what`
    /// says what was being done, and leads the message.
    pub fn to_exec_error(&self, what: &str) -> ExecError {
        let message = format!("{what}: {self}");
        match self {
            WindowRefusal::Input(_) => ExecError::new(ErrorCode::InvalidParam, message),
            WindowRefusal::UnknownTenants(_) | WindowRefusal::NotFound { .. } => {
                ExecError::new(ErrorCode::NotFound, message)
            }
            WindowRefusal::Store(e) => {
                ExecError::of_fault(e.class(), ErrorCode::SpaceUnavailable, message)
            }
        }
    }
}

impl std::fmt::Display for WindowRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowRefusal::Input(why) => write!(f, "window refused: {why}"),
            WindowRefusal::UnknownTenants(ids) => write!(
                f,
                "no tenant {}: {}. `ordo space list` names the tenants there are",
                if ids.len() == 1 { "is" } else { "are" },
                ids.join(", ")
            ),
            WindowRefusal::NotFound { what, id } => write!(
                f,
                "no {what} is stored under `{id}`; `ordo windows list` names them"
            ),
            WindowRefusal::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WindowRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WindowRefusal::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<PoolError> for WindowRefusal {
    fn from(e: PoolError) -> WindowRefusal {
        WindowRefusal::Store(e)
    }
}

// ── the checks ─────────────────────────────────────────────────────────

/// A message as stored: any text, at most [`MAX_MESSAGE_CHARS`] characters,
/// not blank. Kept as written, never rendered.
pub fn check_message(text: &str) -> Result<String, WindowRefusal> {
    let n = text.chars().count();
    if text.trim().is_empty() {
        return Err(WindowRefusal::Input(format!(
            "a window carries a message of 1 to {MAX_MESSAGE_CHARS} characters; this one is \
             empty (`--message \"...\"`)"
        )));
    }
    if n > MAX_MESSAGE_CHARS {
        return Err(WindowRefusal::Input(format!(
            "a message is at most {MAX_MESSAGE_CHARS} characters; this one is {n}"
        )));
    }
    Ok(text.to_string())
}

/// An id a person gave: 1 to [`MAX_ID_CHARS`] of `a-z`, `0-9`, `_`, `-`.
pub fn check_id(text: &str) -> Result<String, WindowRefusal> {
    let id = text.trim();
    if id.is_empty()
        || id.chars().count() > MAX_ID_CHARS
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(WindowRefusal::Input(format!(
            "`{}` is not an id: 1 to {MAX_ID_CHARS} of a-z, 0-9, `_` and `-`",
            id.chars().take(MAX_ID_CHARS).collect::<String>()
        )));
    }
    Ok(id.to_string())
}

fn mint(prefix: &str) -> String {
    let mut bytes = [0u8; 8];
    ordo_crypto::fill_random(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}_{hex}")
}

/// Check a schedule: an end after its start; a cron that parses and fires;
/// a length that is positive and shorter than the cron's shortest gap, so
/// two times never overlap; and no more than
/// [`MAX_OCCURRENCES_PER_WINDOW`] times in the list's 24 months.
pub fn check_schedule(s: &Schedule, now: i64) -> Result<(), WindowRefusal> {
    match s {
        Schedule::Range { start, end } => {
            if end <= start {
                return Err(WindowRefusal::Input(format!(
                    "a window ends after it starts; this one starts {} and ends {}",
                    format_utc(*start),
                    format_utc(*end)
                )));
            }
            Ok(())
        }
        Schedule::Cron { cron, length } => {
            let c = Cron::parse(cron)?;
            if *length <= 0 {
                return Err(WindowRefusal::Input(format!(
                    "a cron window's length is more than 0; this one is {length} seconds \
                     (`--length 2h`, `lengthSecs` on the door)"
                )));
            }
            let Some(gap) = c.shortest_gap() else {
                return Err(WindowRefusal::Input(format!(
                    "the cron `{cron}` gives no time in 8 years; check its day and month"
                )));
            };
            if *length >= gap {
                return Err(WindowRefusal::Input(format!(
                    "a cron window's length is shorter than the shortest gap between its \
                     times, so two never overlap. `{cron}` is {} apart at the closest; this \
                     length is {}",
                    format_length(gap),
                    format_length(*length)
                )));
            }
            let (from, to) = span(now);
            let n = c.count(from, to);
            if n > MAX_OCCURRENCES_PER_WINDOW {
                return Err(WindowRefusal::Input(format!(
                    "a cron window gives at most {MAX_OCCURRENCES_PER_WINDOW} times in the 24 \
                     months the list covers, about once an hour; `{cron}` gives {n}"
                )));
            }
            Ok(())
        }
    }
}

// ── the client ─────────────────────────────────────────────────────────

/// The client. Borrowed over the control database's pool, where `$spaces`
/// is.
#[derive(Clone, Copy)]
pub struct Windows<'a> {
    pool: &'a Pool,
}

impl<'a> Windows<'a> {
    pub fn on(pool: &'a Pool) -> Windows<'a> {
        Windows { pool }
    }

    /// Create the three tables. Idempotent; retried once, because `IF NOT
    /// EXISTS` is not race-free between two callers.
    pub async fn ensure(&self) -> Result<(), WindowRefusal> {
        if self.pool.rows(DDL).await.is_err() {
            self.pool.rows(DDL).await?;
        }
        Ok(())
    }

    /// **Set a window**: create it, or replace the one stored under its id.
    /// Checked first. Replacing a cron window with a range drops its skips,
    /// which a range has no use for. Answers the window as stored.
    pub async fn set(
        &self,
        spec: &WindowSpec,
        actor: &str,
        now: i64,
    ) -> Result<Window, WindowRefusal> {
        let id = match &spec.id {
            Some(id) => check_id(id)?,
            None => mint("win"),
        };
        let message = check_message(&spec.message)?;
        check_schedule(&spec.schedule, now)?;
        self.ensure().await?;
        self.refuse_unknown(&spec.target.tenants()).await?;
        let (kind, start, end, cron, length) = match &spec.schedule {
            Schedule::Range { start, end } => (
                "range",
                start.to_string(),
                end.to_string(),
                "NULL".into(),
                "NULL".into(),
            ),
            Schedule::Cron { cron, length } => (
                "cron",
                "NULL".to_string(),
                "NULL".to_string(),
                sql_lit(cron.trim()),
                length.to_string(),
            ),
        };
        let tx = self.pool.begin().await?;
        tx.rows(&format!(
            "INSERT INTO \"$windows\" (\"id\", \"target\", \"kind\", \"starts_at\", \"ends_at\", \
               \"cron\", \"length_secs\", \"message\", \"set_by\", \"set_at\") \
             VALUES ({id_lit}, {target}, '{kind}', {start}, {end}, {cron}, {length}, {msg}, \
               {by}, {now}) \
             ON CONFLICT (\"id\") DO UPDATE SET \"target\" = EXCLUDED.\"target\", \
               \"kind\" = EXCLUDED.\"kind\", \"starts_at\" = EXCLUDED.\"starts_at\", \
               \"ends_at\" = EXCLUDED.\"ends_at\", \"cron\" = EXCLUDED.\"cron\", \
               \"length_secs\" = EXCLUDED.\"length_secs\", \"message\" = EXCLUDED.\"message\", \
               \"set_by\" = EXCLUDED.\"set_by\", \"set_at\" = EXCLUDED.\"set_at\"",
            id_lit = sql_lit(&id),
            target = sql_lit(spec.target.kind()),
            msg = sql_lit(&message),
            by = sql_lit(actor),
        ))
        .await?;
        tx.rows(&format!(
            "DELETE FROM \"$window_tenants\" WHERE \"window_id\" = {}",
            sql_lit(&id)
        ))
        .await?;
        let tenants = spec.target.tenants();
        if !tenants.is_empty() {
            tx.rows(&format!(
                "INSERT INTO \"$window_tenants\" (\"window_id\", \"space\") VALUES {}",
                tenants
                    .iter()
                    .map(|t| format!("({}, {})", sql_lit(&id), sql_lit(t)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .await?;
        }
        if kind == "range" {
            tx.rows(&format!(
                "DELETE FROM \"$window_skips\" WHERE \"window_id\" = {}",
                sql_lit(&id)
            ))
            .await?;
        }
        tx.commit().await?;
        self.get(&id)
            .await?
            .ok_or(WindowRefusal::NotFound { what: "window", id })
    }

    /// **Delete a window** and its skips. Refused for an id not stored.
    pub async fn delete(&self, id: &str) -> Result<(), WindowRefusal> {
        let id = id.trim();
        self.ensure().await?;
        let tx = self.pool.begin().await?;
        let gone = tx
            .rows(&format!(
                "DELETE FROM \"$windows\" WHERE \"id\" = {} RETURNING \"id\"",
                sql_lit(id)
            ))
            .await?;
        if gone.is_empty() {
            tx.rollback().await?;
            return Err(WindowRefusal::NotFound {
                what: "window",
                id: id.to_string(),
            });
        }
        for table in ["$window_tenants", "$window_skips"] {
            tx.rows(&format!(
                "DELETE FROM \"{table}\" WHERE \"window_id\" = {}",
                sql_lit(id)
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// **Skip a cron window's times** from `start` to `end`: create the
    /// skip, or replace the one stored under its id. A range window is
    /// refused: it is deleted or set again instead.
    #[allow(clippy::too_many_arguments)]
    pub async fn skip(
        &self,
        window: &str,
        id: Option<&str>,
        start: i64,
        end: i64,
        reason: Option<&str>,
        actor: &str,
        now: i64,
    ) -> Result<Skip, WindowRefusal> {
        let id = match id {
            Some(id) => check_id(id)?,
            None => mint("skp"),
        };
        let reason = match reason.map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) if r.chars().count() > MAX_REASON_CHARS => {
                return Err(WindowRefusal::Input(format!(
                    "a skip's reason is at most {MAX_REASON_CHARS} characters; this one is {}",
                    r.chars().count()
                )))
            }
            other => other.map(str::to_string),
        };
        if end <= start {
            return Err(WindowRefusal::Input(format!(
                "a skip ends after it starts; this one starts {} and ends {}",
                format_utc(start),
                format_utc(end)
            )));
        }
        let Some(w) = self.get(window.trim()).await? else {
            return Err(WindowRefusal::NotFound {
                what: "window",
                id: window.trim().to_string(),
            });
        };
        if matches!(w.schedule, Schedule::Range { .. }) {
            return Err(WindowRefusal::Input(format!(
                "`{}` is a range, not a cron, so it has no times to skip. `ordo windows \
                 delete {}` removes it, and `ordo windows set --id {}` changes it",
                w.id, w.id, w.id
            )));
        }
        self.pool
            .rows(&format!(
                "INSERT INTO \"$window_skips\" (\"id\", \"window_id\", \"starts_at\", \"ends_at\", \
                   \"reason\", \"set_by\", \"set_at\") \
                 VALUES ({}, {}, {start}, {end}, {}, {}, {now}) \
                 ON CONFLICT (\"id\") DO UPDATE SET \"window_id\" = EXCLUDED.\"window_id\", \
                   \"starts_at\" = EXCLUDED.\"starts_at\", \"ends_at\" = EXCLUDED.\"ends_at\", \
                   \"reason\" = EXCLUDED.\"reason\", \"set_by\" = EXCLUDED.\"set_by\", \
                   \"set_at\" = EXCLUDED.\"set_at\"",
                sql_lit(&id),
                sql_lit(&w.id),
                reason.as_deref().map(sql_lit).unwrap_or_else(|| "NULL".into()),
                sql_lit(actor),
            ))
            .await?;
        Ok(Skip {
            id,
            window: w.id,
            start,
            end,
            reason,
            set_by: actor.to_string(),
            set_at: u64::try_from(now).unwrap_or(0),
        })
    }

    /// **Delete a skip.** Refused for an id not stored.
    pub async fn delete_skip(&self, id: &str) -> Result<(), WindowRefusal> {
        let id = id.trim();
        self.ensure().await?;
        let gone = self
            .pool
            .rows(&format!(
                "DELETE FROM \"$window_skips\" WHERE \"id\" = {} RETURNING \"id\"",
                sql_lit(id)
            ))
            .await?;
        if gone.is_empty() {
            return Err(WindowRefusal::NotFound {
                what: "skip",
                id: id.to_string(),
            });
        }
        Ok(())
    }

    /// One window. `None` when none is stored under `id`.
    pub async fn get(&self, id: &str) -> Result<Option<Window>, WindowRefusal> {
        Ok(self.read(Some(id)).await?.into_iter().next())
    }

    /// **Every time from 12 months back to 12 months ahead**, for one
    /// tenant (the windows that cover it) or for the whole deployment.
    /// See the module doc for what is listed and the caps.
    pub async fn list(&self, tenant: Option<&str>, now: i64) -> Result<Listing, WindowRefusal> {
        let tenant = tenant.map(str::trim).filter(|t| !t.is_empty());
        if let Some(t) = tenant {
            self.refuse_unknown(&[t]).await?;
        }
        let (from, to) = span(now);
        let windows: Vec<Window> = self
            .read(None)
            .await?
            .into_iter()
            .filter(|w| tenant.is_none_or(|t| w.target.covers(t)))
            .collect();
        let skips = self.skips().await?;
        let mut by_window: BTreeMap<&str, Vec<&Skip>> = BTreeMap::new();
        for s in &skips {
            by_window.entry(s.window.as_str()).or_default().push(s);
        }
        let mut occurrences = Vec::new();
        let mut truncated = false;
        for w in &windows {
            let starts: Vec<(i64, i64)> = match &w.schedule {
                Schedule::Range { start, end } => {
                    if *start < to && *end > from {
                        vec![(*start, *end)]
                    } else {
                        vec![]
                    }
                }
                Schedule::Cron { cron, length } => match Cron::parse(cron) {
                    // Times that started before the span and run into it
                    // are listed too.
                    Ok(c) => c
                        .expand(from - length, to, MAX_OCCURRENCES_PER_WINDOW)
                        .into_iter()
                        .map(|s| (s, s + length))
                        .collect(),
                    // Checked when it was set; a cron this build reads
                    // differently lists nothing rather than failing the list.
                    Err(_) => vec![],
                },
            };
            let mine = by_window.get(w.id.as_str());
            for (start, end) in starts {
                if occurrences.len() >= MAX_OCCURRENCES {
                    truncated = true;
                    break;
                }
                let skipped_by = mine.and_then(|ss| {
                    ss.iter()
                        .find(|s| start >= s.start && start < s.end)
                        .map(|s| s.id.clone())
                });
                occurrences.push(Occurrence {
                    window: w.id.clone(),
                    start,
                    end,
                    target: w.target.kind(),
                    skipped_by,
                });
            }
        }
        occurrences.sort_by(|a, b| (a.start, &a.window).cmp(&(b.start, &b.window)));
        let ids: BTreeSet<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        let skips = skips
            .iter()
            .filter(|s| ids.contains(s.window.as_str()))
            .cloned()
            .collect();
        Ok(Listing {
            from,
            to,
            tenant: tenant.map(str::to_string),
            windows,
            skips,
            occurrences,
            truncated,
        })
    }

    /// Refuse tenants that do not exist or were reaped, naming each.
    async fn refuse_unknown(&self, ids: &[&str]) -> Result<(), WindowRefusal> {
        if ids.is_empty() {
            return Ok(());
        }
        let list = ids
            .iter()
            .map(|t| sql_lit(t))
            .collect::<Vec<_>>()
            .join(", ");
        let found: BTreeSet<String> = match self
            .pool
            .rows(&format!(
                "SELECT \"$space\" FROM \"$spaces\" WHERE \"$space\" IN ({list}) \
                 AND \"state\" <> 'reaped'"
            ))
            .await
        {
            Ok(rows) => rows
                .into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .collect(),
            // No tenant registry: no tenant exists.
            Err(e) if e.is_missing_table() => BTreeSet::new(),
            Err(e) => return Err(e.into()),
        };
        let missing: Vec<String> = ids
            .iter()
            .filter(|t| !found.contains(**t))
            .map(|t| t.to_string())
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(WindowRefusal::UnknownTenants(missing))
        }
    }

    async fn read(&self, id: Option<&str>) -> Result<Vec<Window>, WindowRefusal> {
        let filter = id
            .map(|i| format!("WHERE w.\"id\" = {}", sql_lit(i)))
            .unwrap_or_default();
        let rows = match self
            .pool
            .rows(&format!(
                "SELECT w.\"id\", w.\"target\", w.\"kind\", w.\"starts_at\", w.\"ends_at\", \
                   w.\"cron\", w.\"length_secs\", w.\"message\", w.\"set_by\", w.\"set_at\", \
                   coalesce((SELECT string_agg(t.\"space\", ',' ORDER BY t.\"space\") \
                     FROM \"$window_tenants\" t WHERE t.\"window_id\" = w.\"id\"), '') \
                 FROM \"$windows\" w {filter} ORDER BY w.\"id\""
            ))
            .await
        {
            Ok(rows) => rows,
            Err(e) if e.is_missing_table() => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let get = |i: usize| r.get(i).cloned().flatten();
                let num = |i: usize| get(i).and_then(|v| v.parse::<i64>().ok());
                let tenants: BTreeSet<String> = get(10)
                    .unwrap_or_default()
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                let target = match get(1)?.as_str() {
                    "only" => Target::Only(tenants),
                    "except" => Target::Except(tenants),
                    _ => Target::All,
                };
                let schedule = match get(2)?.as_str() {
                    "cron" => Schedule::Cron {
                        cron: get(5)?,
                        length: num(6)?,
                    },
                    _ => Schedule::Range {
                        start: num(3)?,
                        end: num(4)?,
                    },
                };
                Some(Window {
                    id: get(0)?,
                    target,
                    schedule,
                    message: get(7)?,
                    set_by: get(8)?,
                    set_at: get(9)?.parse().ok()?,
                })
            })
            .collect())
    }

    async fn skips(&self) -> Result<Vec<Skip>, WindowRefusal> {
        let rows = match self
            .pool
            .rows(
                "SELECT \"id\", \"window_id\", \"starts_at\", \"ends_at\", \"reason\", \
                   \"set_by\", \"set_at\" FROM \"$window_skips\" ORDER BY \"starts_at\", \"id\"",
            )
            .await
        {
            Ok(rows) => rows,
            Err(e) if e.is_missing_table() => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let get = |i: usize| r.get(i).cloned().flatten();
                Some(Skip {
                    id: get(0)?,
                    window: get(1)?,
                    start: get(2)?.parse().ok()?,
                    end: get(3)?.parse().ok()?,
                    reason: get(4),
                    set_by: get(5)?,
                    set_at: get(6)?.parse().ok()?,
                })
            })
            .collect())
    }
}

// ── the wire shape ─────────────────────────────────────────────────────

impl Window {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "id": self.id,
            "target": self.target.kind(),
            "tenants": self.target.tenants(),
            "message": self.message,
            "setBy": self.set_by,
            "setAt": self.set_at,
        });
        match &self.schedule {
            Schedule::Range { start, end } => {
                v["starts"] = format_utc(*start).into();
                v["ends"] = format_utc(*end).into();
            }
            Schedule::Cron { cron, length } => {
                v["cron"] = cron.clone().into();
                v["lengthSecs"] = (*length).into();
            }
        }
        v
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Window> {
        let tenants: Vec<String> = v["tenants"]
            .as_array()?
            .iter()
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect();
        let schedule = match v["cron"].as_str() {
            Some(cron) => Schedule::Cron {
                cron: cron.to_string(),
                length: v["lengthSecs"].as_i64()?,
            },
            None => Schedule::Range {
                start: parse_utc(v["starts"].as_str()?).ok()?,
                end: parse_utc(v["ends"].as_str()?).ok()?,
            },
        };
        Some(Window {
            id: v["id"].as_str()?.to_string(),
            target: match v["target"].as_str()? {
                "only" => Target::Only(tenants.into_iter().collect()),
                "except" => Target::Except(tenants.into_iter().collect()),
                _ => Target::All,
            },
            schedule,
            message: v["message"].as_str()?.to_string(),
            set_by: v["setBy"].as_str()?.to_string(),
            set_at: v["setAt"].as_u64()?,
        })
    }
}

impl Skip {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "window": self.window,
            "starts": format_utc(self.start),
            "ends": format_utc(self.end),
            "reason": self.reason,
            "setBy": self.set_by,
            "setAt": self.set_at,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Skip> {
        Some(Skip {
            id: v["id"].as_str()?.to_string(),
            window: v["window"].as_str()?.to_string(),
            start: parse_utc(v["starts"].as_str()?).ok()?,
            end: parse_utc(v["ends"].as_str()?).ok()?,
            reason: v["reason"].as_str().map(str::to_string),
            set_by: v["setBy"].as_str()?.to_string(),
            set_at: v["setAt"].as_u64()?,
        })
    }
}

impl Listing {
    /// `std.deployment.windows`' result row.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "from": format_utc(self.from),
            "to": format_utc(self.to),
            "tenant": self.tenant,
            "windows": self.windows.iter().map(Window::to_json).collect::<Vec<_>>(),
            "skips": self.skips.iter().map(Skip::to_json).collect::<Vec<_>>(),
            "occurrences": self.occurrences.iter().map(|o| serde_json::json!({
                "window": o.window,
                "starts": format_utc(o.start),
                "ends": format_utc(o.end),
                "target": o.target,
                "skippedBy": o.skipped_by,
            })).collect::<Vec<_>>(),
            "truncated": self.truncated,
        })
    }

    /// The row read back, so `ordo windows list --host` prints what the
    /// direct path prints.
    pub fn from_json(v: &serde_json::Value) -> Result<Listing, String> {
        let bad = |what: &str| format!("the door's windows list has no readable `{what}`");
        let time = |k: &str, o: &serde_json::Value| {
            o[k].as_str()
                .and_then(|s| parse_utc(s).ok())
                .ok_or_else(|| bad(k))
        };
        let occurrences = v["occurrences"]
            .as_array()
            .ok_or_else(|| bad("occurrences"))?
            .iter()
            .map(|o| {
                Ok(Occurrence {
                    window: o["window"]
                        .as_str()
                        .ok_or_else(|| bad("window"))?
                        .to_string(),
                    start: time("starts", o)?,
                    end: time("ends", o)?,
                    target: match o["target"].as_str() {
                        Some("only") => "only",
                        Some("except") => "except",
                        _ => "all",
                    },
                    skipped_by: o["skippedBy"].as_str().map(str::to_string),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Listing {
            from: time("from", v)?,
            to: time("to", v)?,
            tenant: v["tenant"].as_str().map(str::to_string),
            windows: v["windows"]
                .as_array()
                .ok_or_else(|| bad("windows"))?
                .iter()
                .map(|w| Window::from_json(w).ok_or_else(|| bad("windows")))
                .collect::<Result<_, _>>()?,
            skips: v["skips"]
                .as_array()
                .ok_or_else(|| bad("skips"))?
                .iter()
                .map(|s| Skip::from_json(s).ok_or_else(|| bad("skips")))
                .collect::<Result<_, _>>()?,
            occurrences,
            truncated: v["truncated"].as_bool().unwrap_or(false),
        })
    }

    /// What `ordo windows list` prints, on both paths.
    pub fn render(&self) -> String {
        let mut out = format!(
            "WINDOWS{}  ({} to {}, UTC)\n",
            self.tenant
                .as_deref()
                .map(|t| format!(" covering {t}"))
                .unwrap_or_default(),
            format_utc(self.from),
            format_utc(self.to)
        );
        if self.windows.is_empty() {
            out.push_str("  none\n");
        }
        for w in &self.windows {
            let who = match &w.target {
                Target::All => "all tenants".to_string(),
                Target::Only(t) => format!("only {}", join_or_none(t)),
                Target::Except(t) => format!("all except {}", join_or_none(t)),
            };
            let when = match &w.schedule {
                Schedule::Range { start, end } => {
                    format!("{} to {}", format_utc(*start), format_utc(*end))
                }
                Schedule::Cron { cron, length } => {
                    format!("cron `{cron}` for {}", format_length(*length))
                }
            };
            out.push_str(&format!("  {}  {when}  {who}  set by {}\n", w.id, w.set_by));
            for line in w.message.lines().take(1) {
                let line: String = line.chars().take(80).collect();
                out.push_str(&format!("      \"{line}\"\n"));
            }
        }
        if !self.skips.is_empty() {
            out.push_str("\nSKIPS\n");
            for s in &self.skips {
                out.push_str(&format!(
                    "  {}  {}  {} to {}{}\n",
                    s.id,
                    s.window,
                    format_utc(s.start),
                    format_utc(s.end),
                    s.reason
                        .as_deref()
                        .map(|r| format!("  \"{r}\""))
                        .unwrap_or_default()
                ));
            }
        }
        out.push_str(&format!(
            "\nTIMES ({}{})\n",
            self.occurrences.len(),
            if self.truncated {
                format!(", cut at {MAX_OCCURRENCES}")
            } else {
                String::new()
            }
        ));
        for o in &self.occurrences {
            out.push_str(&format!(
                "  {} to {}  {}  {}{}\n",
                format_utc(o.start),
                format_utc(o.end),
                o.window,
                o.target,
                o.skipped_by
                    .as_deref()
                    .map(|s| format!("  skipped by {s}"))
                    .unwrap_or_default()
            ));
        }
        out
    }
}

fn join_or_none(t: &BTreeSet<String>) -> String {
    if t.is_empty() {
        "nobody".into()
    } else {
        t.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

// ── time ───────────────────────────────────────────────────────────────

/// The list's span around `now`: the same moment 12 months back and 12
/// months ahead, the day clamped to the month's last.
pub fn span(now: i64) -> (i64, i64) {
    (add_months(now, -LIST_MONTHS), add_months(now, LIST_MONTHS))
}

fn add_months(t: i64, months: i64) -> i64 {
    let day = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(day);
    let total = y * 12 + (m as i64 - 1) + months;
    let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let nd = d.min(days_in_month(ny, nm));
    days_from_civil(ny, nm, nd) * 86_400 + secs
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Howard Hinnant's `days_from_civil`. The workspace has no date library,
/// and `ordo-blob` and `ordo-sema` carry the same few lines.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Seconds as `YYYY-MM-DDTHH:MM:SSZ`, the door's datetime.
pub fn format_utc(t: i64) -> String {
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

/// `YYYY-MM-DDTHH:MM[:SS]Z`, UTC with the `Z`, as seconds since 1970.
/// The door sends `:SS`; a person may leave it out.
pub fn parse_utc(text: &str) -> Result<i64, WindowRefusal> {
    let bad = || {
        WindowRefusal::Input(format!(
            "`{}` is not a UTC time: `YYYY-MM-DDTHH:MMZ` or `YYYY-MM-DDTHH:MM:SSZ`",
            text.chars().take(40).collect::<String>()
        ))
    };
    let t = text.trim().strip_suffix('Z').ok_or_else(bad)?;
    let (date, time) = t.split_once('T').ok_or_else(bad)?;
    let num = |s: &str, len: usize| -> Result<i64, WindowRefusal> {
        if s.len() != len || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        s.parse().map_err(|_| bad())
    };
    let mut d = date.split('-');
    let (y, m, day) = match (d.next(), d.next(), d.next(), d.next()) {
        (Some(y), Some(m), Some(day), None) => (num(y, 4)?, num(m, 2)?, num(day, 2)?),
        _ => return Err(bad()),
    };
    let mut c = time.split(':');
    let (h, mi, s) = match (c.next(), c.next(), c.next(), c.next()) {
        (Some(h), Some(mi), None, None) => (num(h, 2)?, num(mi, 2)?, 0),
        (Some(h), Some(mi), Some(s), None) => (num(h, 2)?, num(mi, 2)?, num(s, 2)?),
        _ => return Err(bad()),
    };
    if !(1..=12).contains(&m)
        || day < 1
        || day > days_in_month(y, m as u32) as i64
        || h > 23
        || mi > 59
        || s > 59
    {
        return Err(bad());
    }
    Ok(days_from_civil(y, m as u32, day as u32) * 86_400 + h * 3600 + mi * 60 + s)
}

/// A length a person wrote: whole seconds, or a number then `s`, `m`, `h`
/// or `d`, joined: `90m`, `1h30m`.
pub fn parse_length(text: &str) -> Result<i64, WindowRefusal> {
    let bad = || {
        WindowRefusal::Input(format!(
            "`{}` is not a length: seconds, or like `90m`, `2h`, `1h30m` (units s m h d)",
            text.chars().take(40).collect::<String>()
        ))
    };
    let t = text.trim();
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        return t.parse().map_err(|_| bad());
    }
    let (mut total, mut n, mut any) = (0i64, String::new(), false);
    for ch in t.chars() {
        if ch.is_ascii_digit() {
            n.push(ch);
            continue;
        }
        let unit = match ch {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            _ => return Err(bad()),
        };
        let v: i64 = n.parse().map_err(|_| bad())?;
        total = total
            .checked_add(v.checked_mul(unit).ok_or_else(bad)?)
            .ok_or_else(bad)?;
        n.clear();
        any = true;
    }
    if !n.is_empty() || !any {
        return Err(bad());
    }
    Ok(total)
}

/// Seconds as a person reads a length: `1h30m`, `45s`.
pub fn format_length(secs: i64) -> String {
    if secs <= 0 {
        return format!("{secs}s");
    }
    let mut out = String::new();
    let mut rest = secs;
    for (unit, name) in [(86_400, "d"), (3600, "h"), (60, "m"), (1, "s")] {
        if rest >= unit {
            out.push_str(&format!("{}{name}", rest / unit));
            rest %= unit;
        }
    }
    out
}

// ── cron ───────────────────────────────────────────────────────────────

/// A standard 5-field cron, in UTC: minute, hour, day of month, month, day
/// of week. Each field takes `*`, a number, `a-b`, a list joined by `,`,
/// and a step `/n` after `*`, a range or a number. Months take `jan` to
/// `dec`, days of the week `sun` to `sat`, and 7 is Sunday as 0 is. When
/// both day fields are restricted, a day matches either, as cron does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: Vec<u32>,
    hours: Vec<u32>,
    dom: u64,
    months: u64,
    dow: u64,
    dom_any: bool,
    dow_any: bool,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

impl Cron {
    pub fn parse(text: &str) -> Result<Cron, WindowRefusal> {
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [mi, h, dom, mo, dow] = fields.as_slice() else {
            return Err(WindowRefusal::Input(format!(
                "a cron has 5 fields (minute hour day-of-month month day-of-week, UTC); \
                 `{}` has {}",
                text.trim(),
                fields.len()
            )));
        };
        let bits = |f: &str, name: &str, lo: u32, hi: u32, names: &[&str], base: u32| {
            field(f, lo, hi, names, base)
                .map_err(|why| WindowRefusal::Input(format!("the cron's {name} field `{f}` {why}")))
        };
        let list = |b: u64| (0..64).filter(|i| b & (1 << i) != 0).collect::<Vec<u32>>();
        let mut dow_bits = bits(dow, "day-of-week", 0, 7, &DAYS, 0)?;
        if dow_bits & (1 << 7) != 0 {
            dow_bits = (dow_bits & !(1 << 7)) | 1;
        }
        Ok(Cron {
            minutes: list(bits(mi, "minute", 0, 59, &[], 0)?),
            hours: list(bits(h, "hour", 0, 23, &[], 0)?),
            dom: bits(dom, "day-of-month", 1, 31, &[], 0)?,
            months: bits(mo, "month", 1, 12, &MONTHS, 1)?,
            dow: dow_bits,
            dom_any: dom.starts_with('*'),
            dow_any: dow.starts_with('*'),
        })
    }

    fn day_matches(&self, day: i64) -> bool {
        let (_, m, d) = civil_from_days(day);
        if self.months & (1 << m) == 0 {
            return false;
        }
        // 1970-01-01 was a Thursday.
        let wd = (day + 4).rem_euclid(7) as u32;
        let dom = self.dom & (1 << d) != 0;
        let dow = self.dow & (1 << wd) != 0;
        if self.dom_any || self.dow_any {
            dom && dow
        } else {
            dom || dow
        }
    }

    /// The minutes of a matching day it fires at, in order.
    fn day_minutes(&self) -> Vec<i64> {
        let mut out = Vec::with_capacity(self.hours.len() * self.minutes.len());
        for h in &self.hours {
            for m in &self.minutes {
                out.push(i64::from(*h) * 60 + i64::from(*m));
            }
        }
        out
    }

    /// Each time from `from` (inclusive) to `to` (exclusive), at most
    /// `cap` of them.
    pub fn expand(&self, from: i64, to: i64, cap: usize) -> Vec<i64> {
        let mut out = Vec::new();
        let minutes = self.day_minutes();
        let mut day = from.div_euclid(86_400);
        let last = to.div_euclid(86_400);
        while day <= last {
            if self.day_matches(day) {
                for m in &minutes {
                    let t = day * 86_400 + m * 60;
                    if t >= from && t < to {
                        if out.len() >= cap {
                            return out;
                        }
                        out.push(t);
                    }
                }
            }
            day += 1;
        }
        out
    }

    /// How many times from `from` to `to`, counted by day.
    pub fn count(&self, from: i64, to: i64) -> usize {
        let per_day = self.day_minutes().len();
        let (first, last) = (from.div_euclid(86_400), to.div_euclid(86_400));
        (first..=last).filter(|d| self.day_matches(*d)).count() * per_day
    }

    /// The shortest gap between two of its times, in seconds, over 8 years
    /// from 2024, which hold two leap days. `None` when it gives no time
    /// there.
    ///
    /// Every matching day has the same minutes, so the gap is the shorter
    /// of the closest two minutes in a day, and the closest two matching
    /// days less the day's span.
    pub fn shortest_gap(&self) -> Option<i64> {
        let minutes = self.day_minutes();
        let (first, last) = (*minutes.first()?, *minutes.last()?);
        let within = minutes.windows(2).map(|w| w[1] - w[0]).min();
        let start = days_from_civil(2024, 1, 1);
        let mut prev: Option<i64> = None;
        let mut closest: Option<i64> = None;
        for day in start..start + 8 * 366 {
            if self.day_matches(day) {
                if let Some(p) = prev {
                    let g = day - p;
                    closest = Some(closest.map_or(g, |c| c.min(g)));
                }
                prev = Some(day);
            }
        }
        prev?;
        let across = closest.map(|d| d * 1440 - last + first);
        [within, across].into_iter().flatten().min().map(|m| m * 60)
    }
}

/// One cron field as a bit set over `lo..=hi`.
fn field(f: &str, lo: u32, hi: u32, names: &[&str], base: u32) -> Result<u64, String> {
    let value = |s: &str| -> Result<u32, String> {
        let lower = s.to_ascii_lowercase();
        if let Some(i) = names.iter().position(|n| *n == lower) {
            return Ok(i as u32 + base);
        }
        let n: u32 = s
            .parse()
            .map_err(|_| format!("has `{s}`, which is not a number from {lo} to {hi}"))?;
        if n < lo || n > hi {
            return Err(format!("has {n}, outside {lo} to {hi}"));
        }
        Ok(n)
    };
    let mut bits = 0u64;
    for item in f.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((r, s)) => {
                let s: u32 =
                    s.parse().ok().filter(|s| *s > 0).ok_or_else(|| {
                        format!("has the step `/{s}`, which is not a number above 0")
                    })?;
                (r, s)
            }
            None => (item, 1),
        };
        let (a, b) = if range == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range.split_once('-') {
            (value(a)?, value(b)?)
        } else if item.contains('/') {
            (value(range)?, hi)
        } else {
            let v = value(range)?;
            (v, v)
        };
        if a > b {
            return Err(format!("has the range {a}-{b}, which runs backwards"));
        }
        let mut v = a;
        while v <= b {
            bits |= 1 << v;
            v += step;
        }
    }
    if bits == 0 {
        return Err("is empty".into());
    }
    Ok(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_times_read_and_print_back() {
        let t = parse_utc("2026-11-04T02:00Z").unwrap();
        assert_eq!(format_utc(t), "2026-11-04T02:00:00Z");
        assert_eq!(parse_utc("2026-11-04T02:00:00Z").unwrap(), t);
        assert_eq!(parse_utc("1970-01-01T00:00:00Z").unwrap(), 0);
        assert!(parse_utc("2026-02-30T00:00Z").is_err());
        assert!(parse_utc("2026-11-04 02:00").is_err());
        assert!(parse_utc("2026-11-04T02:00+01:00").is_err());
    }

    #[test]
    fn lengths_read_and_print_back() {
        assert_eq!(parse_length("90m").unwrap(), 5400);
        assert_eq!(parse_length("1h30m").unwrap(), 5400);
        assert_eq!(parse_length("7200").unwrap(), 7200);
        assert!(parse_length("2 hours").is_err());
        assert!(parse_length("h").is_err());
        assert_eq!(format_length(5400), "1h30m");
    }

    #[test]
    fn the_span_is_twelve_months_each_way() {
        let now = parse_utc("2028-02-29T12:00Z").unwrap();
        let (from, to) = span(now);
        assert_eq!(format_utc(from), "2027-02-28T12:00:00Z");
        assert_eq!(format_utc(to), "2029-02-28T12:00:00Z");
    }

    #[test]
    fn a_cron_expands_in_utc() {
        // Sundays at 02:00.
        let c = Cron::parse("0 2 * * sun").unwrap();
        let from = parse_utc("2026-11-01T00:00Z").unwrap();
        let to = parse_utc("2026-11-16T00:00Z").unwrap();
        let times: Vec<String> = c
            .expand(from, to, 100)
            .into_iter()
            .map(format_utc)
            .collect();
        assert_eq!(
            times,
            vec![
                "2026-11-01T02:00:00Z",
                "2026-11-08T02:00:00Z",
                "2026-11-15T02:00:00Z"
            ]
        );
        assert_eq!(c.count(from, to), 3);
        assert_eq!(c.shortest_gap(), Some(7 * 86_400));
        // 7 is Sunday too.
        assert_eq!(Cron::parse("0 2 * * 7").unwrap(), c);
    }

    #[test]
    fn steps_lists_and_both_day_fields() {
        let c = Cron::parse("*/15 1-2 * * *").unwrap();
        assert_eq!(c.day_minutes().len(), 8);
        assert_eq!(c.shortest_gap(), Some(15 * 60));
        // The 1st or any Monday, as cron reads two restricted day fields.
        let c = Cron::parse("0 0 1 * mon").unwrap();
        let from = parse_utc("2026-06-01T00:00Z").unwrap();
        let to = parse_utc("2026-07-01T00:00Z").unwrap();
        // June 2026: the 1st is a Monday, then the 8th, 15th, 22nd, 29th.
        assert_eq!(c.expand(from, to, 100).len(), 5);
        // Across days: 23:00 and 01:00 are 2 hours apart.
        assert_eq!(
            Cron::parse("0 1,23 * * *").unwrap().shortest_gap(),
            Some(7200)
        );
    }

    #[test]
    fn a_bad_cron_says_which_field() {
        let e = Cron::parse("0 25 * * *").unwrap_err().to_string();
        assert!(e.contains("hour") && e.contains("25"), "{e}");
        let e = Cron::parse("0 2 * *").unwrap_err().to_string();
        assert!(e.contains("5 fields"), "{e}");
        let e = Cron::parse("*/0 * * * *").unwrap_err().to_string();
        assert!(e.contains("step"), "{e}");
        assert!(Cron::parse("0 0 31 apr *")
            .unwrap()
            .shortest_gap()
            .is_none());
    }

    #[test]
    fn a_schedule_is_checked() {
        let now = parse_utc("2026-11-04T00:00Z").unwrap();
        let range = |a: &str, b: &str| Schedule::Range {
            start: parse_utc(a).unwrap(),
            end: parse_utc(b).unwrap(),
        };
        assert!(check_schedule(&range("2026-11-20T02:00Z", "2026-11-20T04:00Z"), now).is_ok());
        let e = check_schedule(&range("2026-11-20T04:00Z", "2026-11-20T02:00Z"), now)
            .unwrap_err()
            .to_string();
        assert!(e.contains("ends after it starts"), "{e}");
        let cron = |c: &str, l: i64| Schedule::Cron {
            cron: c.into(),
            length: l,
        };
        assert!(check_schedule(&cron("0 2 * * sun", 7200), now).is_ok());
        let e = check_schedule(&cron("0 2 * * *", 86_400), now)
            .unwrap_err()
            .to_string();
        assert!(e.contains("shortest gap"), "{e}");
        let e = check_schedule(&cron("0 2 * * *", 0), now)
            .unwrap_err()
            .to_string();
        assert!(e.contains("more than 0"), "{e}");
        let e = check_schedule(&cron("*/5 * * * *", 60), now)
            .unwrap_err()
            .to_string();
        assert!(e.contains("20000"), "{e}");
        let e = check_schedule(&cron("0 0 31 apr *", 60), now)
            .unwrap_err()
            .to_string();
        assert!(e.contains("no time"), "{e}");
    }

    #[test]
    fn a_target_is_one_of_three_rules() {
        let t = |k: &str, ids: &[&str]| {
            Target::parse(k, &ids.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(t("all", &[]).unwrap(), Target::All);
        assert!(t("all", &["spc_a"]).is_err());
        assert!(t("only", &[]).is_err());
        let except = t("except", &["spc_a"]).unwrap();
        assert!(!except.covers("spc_a"));
        // A tenant made later is covered: the rule is stored.
        assert!(except.covers("spc_later"));
        assert!(t("only", &["spc_a"]).unwrap().covers("spc_a"));
        assert!(!t("only", &["spc_a"]).unwrap().covers("spc_b"));
        let e = t("whitelist", &[]).unwrap_err().to_string();
        assert!(e.contains("`all`, `only` or `except`"), "{e}");
    }

    #[test]
    fn a_message_is_capped_and_kept_as_written() {
        assert_eq!(
            check_message("**Upgrade**\nback at 4").unwrap(),
            "**Upgrade**\nback at 4"
        );
        let e = check_message(&"x".repeat(2_001)).unwrap_err().to_string();
        assert!(e.contains("at most 2000"), "{e}");
        assert!(check_message("  ").is_err());
    }
}
