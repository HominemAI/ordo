//! Who holds a pool's connections, for a pool that is asked to say.
//!
//! A pool that refuses a checkout as `server_busy` says only that every
//! connection is in use. For the door's store pool (one connection until
//! 2026-10-24, `ORDO_PG_STORE_POOL_SIZE` since) that is
//! not enough to act on: in CI on 2026-10-24 its one connection stayed in
//! use for about 2 minutes and every emailed-code sign-in failed, and the
//! log could not say which store held it or whether it was dialling or
//! waiting on a statement (board task `store-pool-held-blocks-signin`).
//!
//! A pool that tracks its holders ([`crate::Pool::track_holders`]) records
//! each checkout with the label of the handle that took it
//! ([`crate::Pool::labelled`]), what it is doing, and since when. When a
//! checkout is refused as busy, the pool hands the list to the report it was
//! given, at most once per [`REPORT_EVERY`], so a refusal names its cause.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The least time between two reports of one pool. A refusal per request
/// would otherwise be a report per request.
pub const REPORT_EVERY: Duration = Duration::from_secs(5);

/// What a holder is doing with its connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldPhase {
    /// Dialling a new connection: the old one was closed or reaped.
    Dialling,
    /// Holding a connection between statements: the holder is doing other
    /// work, or waiting on something that is not this connection.
    Holding,
    /// Waiting on a statement it sent: the answer has not come back.
    InStatement,
}

impl HoldPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            HoldPhase::Dialling => "dialling",
            HoldPhase::Holding => "holding a connection",
            HoldPhase::InStatement => "waiting on a statement",
        }
    }
}

/// One checkout in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// The label of the handle that took it, `unlabelled` when it had none.
    pub label: String,
    pub phase: HoldPhase,
    /// Since the permit was taken.
    pub held_for: Duration,
    /// The thread that took the permit: a store's own thread
    /// (`ordo-credentials`) or a runtime worker.
    pub thread: String,
    /// Where the connection in hand was dialled, which is the runtime its
    /// socket task runs on: a statement's answer arrives only while that
    /// runtime is polled. `None` while dialling.
    pub dialled_on: Option<String>,
    /// The statement in flight, its literals removed ([`redact_sql`]), and
    /// how long it has been waiting. `None` between statements.
    pub statement: Option<(String, Duration)>,
}

/// What a tracked pool calls when it refuses a checkout as busy: the pool's
/// name, the label of the refused handle, and every holder, longest first.
pub type BusyReport = Arc<dyn Fn(&str, &str, &[Holder]) + Send + Sync>;

/// Every report as one line: `<pool>: busy for <who>; held by <label> for
/// <ms> ms (<phase>) [thread ..., dialled on ..., statement ...], ...`.
pub fn describe(pool: &str, refused: &str, holders: &[Holder]) -> String {
    let held = if holders.is_empty() {
        "no holder recorded".to_string()
    } else {
        holders
            .iter()
            .map(|h| {
                let mut detail = vec![format!("thread {}", h.thread)];
                if let Some(on) = &h.dialled_on {
                    detail.push(format!("connection dialled on {on}"));
                }
                if let Some((sql, waited)) = &h.statement {
                    detail.push(format!("for {} ms on `{sql}`", waited.as_millis()));
                }
                format!(
                    "{} for {} ms ({}) [{}]",
                    h.label,
                    h.held_for.as_millis(),
                    h.phase.as_str(),
                    detail.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("{pool}: server_busy for {refused}; held by {held}")
}

/// The longest statement head a holder or a Postgres snapshot shows.
pub const STATEMENT_HEAD: usize = 200;

/// `sql` with every literal a statement could carry a value in replaced,
/// cut to `max` characters.
///
/// The platform speaks the simple protocol, so a statement's values are in
/// its text: an address, a code's hash, a token's id. A diagnostic line
/// shows the statement's shape and never those. Quoted strings (`'...'`,
/// `E'...'`, dollar-quoted bodies) become `'?'`, and a number standing on
/// its own becomes `?`. Quoted identifiers (`"$credential"`) are kept,
/// because they name the table and not a value. An unterminated literal,
/// as a statement cut short by Postgres's own `track_activity_query_size`
/// leaves, is replaced to its end.
pub fn redact_sql(sql: &str, max: usize) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len().min(max * 2));
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            // A quoted identifier, kept whole.
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '"' {
                    if chars.get(i + 1) == Some(&'"') {
                        out.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if c == '\'' {
            // A string literal; `''` is a quote inside it.
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push_str("'?'");
        } else if c == '$' && dollar_tag(&chars, i).is_some() {
            let tag = dollar_tag(&chars, i).unwrap_or_default();
            i += tag.len();
            let body_end = find(&chars, i, &tag);
            i = body_end.map(|e| e + tag.len()).unwrap_or(chars.len());
            out.push_str("'?'");
        } else if c.is_ascii_digit()
            && !out
                .chars()
                .last()
                .is_some_and(|p| p.is_alphanumeric() || p == '_' || p == '$' || p == '?')
        {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            out.push('?');
        } else {
            out.push(c);
            i += 1;
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > max {
        let mut cut: String = collapsed.chars().take(max).collect();
        cut.push('…');
        cut
    } else {
        collapsed
    }
}

/// The tag of a dollar quote opening at `i` (`$$` or `$name$`), if one does.
fn dollar_tag(chars: &[char], i: usize) -> Option<String> {
    let mut j = i + 1;
    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    (chars.get(j) == Some(&'$')).then(|| chars[i..=j].iter().collect())
}

fn find(chars: &[char], from: usize, tag: &str) -> Option<usize> {
    let t: Vec<char> = tag.chars().collect();
    (from..chars.len().saturating_sub(t.len() - 1)).find(|&k| chars[k..k + t.len()] == t[..])
}

/// The thread this code runs on and the runtime it belongs to, for a log
/// line: `ordo-credentials (current-thread runtime)`.
pub fn here() -> String {
    let thread = std::thread::current()
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{:?}", std::thread::current().id()));
    let runtime = match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::CurrentThread) => "current-thread runtime",
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => "multi-thread runtime",
        Ok(_) => "runtime",
        Err(_) => "no runtime",
    };
    format!("{thread} ({runtime})")
}

struct Entry {
    label: Arc<str>,
    phase: HoldPhase,
    since: Instant,
    thread: String,
    dialled_on: Option<Arc<str>>,
    statement: Option<(String, Instant)>,
}

#[derive(Default)]
struct Table {
    next: u64,
    live: BTreeMap<u64, Entry>,
    last_report: Option<Instant>,
}

/// A pool's record of its holders.
pub(crate) struct Holders {
    name: String,
    report: BusyReport,
    table: Mutex<Table>,
}

impl Holders {
    pub(crate) fn new(name: String, report: BusyReport) -> Holders {
        Holders {
            name,
            report,
            table: Mutex::new(Table::default()),
        }
    }

    fn table(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Record a checkout that has its permit and has no connection yet.
    pub(crate) fn enter(self: &Arc<Self>, label: Option<&Arc<str>>) -> Hold {
        let mut t = self.table();
        let id = t.next;
        t.next += 1;
        t.live.insert(
            id,
            Entry {
                label: label.cloned().unwrap_or_else(|| Arc::from("unlabelled")),
                phase: HoldPhase::Dialling,
                since: Instant::now(),
                thread: here(),
                dialled_on: None,
                statement: None,
            },
        );
        Hold {
            holders: self.clone(),
            id,
        }
    }

    /// Every holder, longest held first.
    pub(crate) fn snapshot(&self) -> Vec<Holder> {
        let t = self.table();
        let mut out: Vec<Holder> = t
            .live
            .values()
            .map(|e| Holder {
                label: e.label.to_string(),
                phase: e.phase,
                held_for: e.since.elapsed(),
                thread: e.thread.clone(),
                dialled_on: e.dialled_on.as_ref().map(|d| d.to_string()),
                statement: e
                    .statement
                    .as_ref()
                    .map(|(sql, at)| (sql.clone(), at.elapsed())),
            })
            .collect();
        out.sort_by_key(|h| std::cmp::Reverse(h.held_for));
        out
    }

    /// A checkout by `refused` was turned away as busy: report the holders,
    /// unless this pool reported less than [`REPORT_EVERY`] ago.
    pub(crate) fn refused(&self, refused: Option<&Arc<str>>) {
        {
            let mut t = self.table();
            if t.last_report.is_some_and(|at| at.elapsed() < REPORT_EVERY) {
                return;
            }
            t.last_report = Some(Instant::now());
        }
        let holders = self.snapshot();
        let who = refused.map(|l| l.as_ref()).unwrap_or("unlabelled");
        (self.report)(&self.name, who, &holders);
    }
}

/// One checkout's entry, removed when the checkout ends.
pub(crate) struct Hold {
    holders: Arc<Holders>,
    id: u64,
}

impl Hold {
    /// The connection is in hand. `dialled_on` is where it was dialled
    /// ([`here`] at the dial).
    pub(crate) fn holding(&self, dialled_on: &Arc<str>) {
        if let Some(e) = self.holders.table().live.get_mut(&self.id) {
            e.phase = HoldPhase::Holding;
            e.dialled_on = Some(dialled_on.clone());
        }
    }

    /// A statement has been sent on the connection.
    pub(crate) fn statement(&self, sql: &str) {
        let head = redact_sql(sql, STATEMENT_HEAD);
        if let Some(e) = self.holders.table().live.get_mut(&self.id) {
            e.phase = HoldPhase::InStatement;
            e.statement = Some((head, Instant::now()));
        }
    }

    /// The statement's answer is in.
    pub(crate) fn answered(&self) {
        if let Some(e) = self.holders.table().live.get_mut(&self.id) {
            e.phase = HoldPhase::Holding;
            e.statement = None;
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.holders.table().live.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::redact_sql;

    #[test]
    fn a_statement_keeps_its_shape_and_loses_its_values() {
        let sql = r#"INSERT INTO "$credential" ("id", "secret", "at") VALUES ('kim@example.org', E'\\x00ff', 1761300000)"#;
        assert_eq!(
            redact_sql(sql, 200),
            r#"INSERT INTO "$credential" ("id", "secret", "at") VALUES ('?', E'?', ?)"#
        );
    }

    #[test]
    fn quotes_inside_a_literal_and_dollar_bodies_are_removed_whole() {
        assert_eq!(
            redact_sql("SELECT 'it''s a secret', $$body 'x'$$, $t$ more $t$", 200),
            "SELECT '?', '?', '?'"
        );
        // An unterminated literal, as a statement Postgres cut short leaves.
        assert_eq!(redact_sql("SELECT 'half of a sec", 200), "SELECT '?'");
    }

    #[test]
    fn identifiers_and_parameters_keep_their_digits() {
        assert_eq!(
            redact_sql(r#"SELECT t2.a1 FROM "tbl9" WHERE x = $1 LIMIT 10"#, 200),
            r#"SELECT t2.a1 FROM "tbl9" WHERE x = $1 LIMIT ?"#
        );
    }

    #[test]
    fn a_long_statement_is_cut_and_its_whitespace_folded() {
        let sql = format!("SELECT\n\n   {}", "a, ".repeat(100));
        let out = redact_sql(&sql, 20);
        assert_eq!(out.chars().count(), 21, "{out}");
        assert!(out.starts_with("SELECT a, a,"), "{out}");
        assert!(out.ends_with('…'), "{out}");
    }
}
