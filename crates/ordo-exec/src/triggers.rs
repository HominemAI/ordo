//! The trigger layer, §7 — the half that is a mechanism rather than a
//! spelling.
//!
//! 5d built the job queue, `$schedules`, LISTEN/NOTIFY and clock leadership,
//! and left them **substrate**: only `enqueue` had a production caller (cache
//! invalidation), nothing drained the queue, nothing wrote `$schedules`, and
//! no `routine:` principal ran anything. This is what sits on top: event
//! derivation from the command log, listener matching, the `$trigger`
//! context, at-least-once delivery with a dead-letter path, `debounce` /
//! `coalesce`, and the clock.
//!
//! The declaration form is **not** here and is not owed — see
//! `ordo_syntax::trigger` for the reading of §7 against §8's `[lean]` pin
//! that this module is built on. A listener is registered through
//! [`Listener`], which is a value, not a keyword.
//!
//! # The event bus is the command log, and that is not an accident
//!
//! §7: "The command log serves as the event bus today: ordered, attributed,
//! per-space — an implementation choice, not a permanent promise." Every
//! fact a listener may pattern on is already in the envelope of an appended
//! group, and none of them is a value:
//!
//! | §7 pattern position | command-log field |
//! |---|---|
//! | space | `$commandlog.space` |
//! | entity | `Command.entity` |
//! | kind | `Command.kind` narrowed by `CommandGroup.actor` |
//! | actor | `CommandGroup.actor` |
//!
//! So the derivation below reads nothing but the skeleton, which is what
//! makes the trigger layer blind by construction rather than by discipline.
//!
//! # At-least-once IS the floor rule
//!
//! §7 states the delivery guarantee and the pruning floor as one fact seen
//! from two sides: "the archive floor never passes the slowest registered
//! consumer, and a listener cursor is never evicted from that calculation …
//! At-least-once is not a separate promise from the floor rule — it *is* the
//! floor rule, seen from the routine's side."
//!
//! There is therefore no delivery bookkeeping here beyond a cursor.
//! [`prune::ConsumerKind::Listener`] already exists and is already
//! non-evictable; a listener registers one, reads forward from it, and
//! advances it only after its runs are durably enqueued. A listener that
//! stops making progress dead-letters (an operator alert, which loses
//! nothing); a listener that should stop pinning the floor is deregistered,
//! explicitly and attributably — `prune::deregister_consumer`, which is
//! already the only way a non-evictable kind leaves the calculation.

use std::collections::BTreeSet;

use ordo_commandlog::{Actor, ChainedGroup, CommandGroup, CommandKind, SpaceId};
use ordo_syntax::trigger::{EventKind, KindPos, ListenerPattern, Modifiers, Schedule};

use ordo_conformance::write::WriteDb;

use crate::{prune, sql_lit, Pool, PoolError, TxConn};

/// The batch, run and dead-letter tables.
///
/// Applied at startup alongside the rest, and for the reason `jobs.rs` gives
/// at length: `CREATE TABLE IF NOT EXISTS` takes a lock even when it does
/// nothing, so DDL inside an open transaction blocks every other transaction
/// running the same guard.
///
/// `$triggerruns` is the idempotence record, and its primary key is the
/// whole of §7's rule: "a run is keyed by **batch id × routine version** — a
/// debounced batch is one identity; redelivery is a no-op". `ON CONFLICT DO
/// NOTHING` on that key is what makes redelivery a no-op, and it is the
/// database enforcing it rather than a read-then-write a race could straddle.
pub const TRIGGERS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$triggerbatches\" ( \
  \"space\" text NOT NULL, \
  \"listener\" text NOT NULL, \
  \"batch\" text NOT NULL, \
  \"first_at\" bigint NOT NULL, \
  \"last_at\" bigint NOT NULL, \
  \"ids\" text NOT NULL, \
  \"events\" text NOT NULL, \
  PRIMARY KEY (\"space\", \"listener\") \
); \
CREATE TABLE IF NOT EXISTS \"$triggerruns\" ( \
  \"space\" text NOT NULL, \
  \"run_key\" text NOT NULL, \
  \"listener\" text NOT NULL, \
  \"payload\" text NOT NULL, \
  \"attempts\" int NOT NULL DEFAULT 0, \
  \"next_attempt\" bigint NOT NULL DEFAULT 0, \
  \"claimed\" boolean NOT NULL DEFAULT false, \
  PRIMARY KEY (\"space\", \"run_key\") \
); \
CREATE TABLE IF NOT EXISTS \"$deadletters\" ( \
  \"space\" text NOT NULL, \
  \"run_key\" text NOT NULL, \
  \"listener\" text NOT NULL, \
  \"payload\" text NOT NULL, \
  \"attempts\" int NOT NULL, \
  \"at\" bigint NOT NULL, \
  \"reason\" text NOT NULL, \
  PRIMARY KEY (\"space\", \"run_key\") \
)";

// ── event derivation ─────────────────────────────────────────────────────

/// One row's worth of history, classified.
///
/// A single command raises **several** kinds at once, because §7's
/// vocabulary is not a partition: `written` is the union of three, and
/// `ingested`/`fed` are narrowings of `created`/`written` by actor kind
/// rather than events of their own. A listener on `written` therefore sees
/// feed copies too, "which is what makes feeds locally reactive".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub space: SpaceId,
    pub entity: String,
    pub row_id: String,
    pub kinds: BTreeSet<EventKind>,
    /// The command-group id — `$trigger`'s "command-group id".
    pub group: String,
    /// The per-command id — `$trigger`'s "batched event ids".
    pub event_id: String,
    pub at: u64,
}

/// The prefix `ordo_commandlog::revert_correlation` puts on the inverse of a
/// group. Named here rather than spelled inline because the two ends must
/// agree, and this is the reading end.
const REVERT_PREFIX: &str = "rev-";

/// The prefix that makes a `source:` principal a **feed** rather than an
/// ordinary mail slot — §7's `source:feed-*`.
const FEED_PREFIX: &str = "feed-";

/// Which of §7's seven kinds this command raises.
///
/// The whole table, and nothing outside it:
///
/// - `created` / `converged` / `deleted` follow `CommandKind` exactly. The
///   write language's whole verb family already compiles down to those
///   three, so there is no fourth case to miss and a `subtree` cascade's
///   per-row deletes are `deleted` for free — they are per-row commands.
/// - `written` is on every command, always. It is the union, and §7 calls it
///   "the common case, one word".
/// - `ingested` is a **create** attributed to a `source:` principal.
/// - `fed` is any **write** attributed to `source:feed-*`. Note the two
///   asymmetries, both of them §7's: `ingested` is creates only while `fed`
///   is any write, and `fed` implies `ingested` on a create because
///   `source:feed-x` is a `source:` principal.
/// - `reverted` is decided by the **group**, not the command: a revert's
///   correlation is `rev-<original>` and nothing else produces that shape,
///   so "a group revert touching the entity" is answerable from the envelope
///   without a flag column anyone could forget to set.
pub fn classify(kind: CommandKind, actor: &Actor, correlation: &str) -> BTreeSet<EventKind> {
    let mut out = BTreeSet::new();
    out.insert(match kind {
        CommandKind::Create => EventKind::Created,
        CommandKind::Converge => EventKind::Converged,
        CommandKind::Delete => EventKind::Deleted,
    });
    out.insert(EventKind::Written);
    if let Actor::Source(name) = actor {
        if kind == CommandKind::Create {
            out.insert(EventKind::Ingested);
        }
        if name.starts_with(FEED_PREFIX) {
            out.insert(EventKind::Fed);
        }
    }
    if correlation.starts_with(REVERT_PREFIX) {
        out.insert(EventKind::Reverted);
    }
    out
}

/// Every event an appended group raises, in the group's own command order.
pub fn events_of(group: &CommandGroup) -> Vec<Event> {
    group
        .commands
        .iter()
        .map(|c| Event {
            space: c.space.clone(),
            entity: c.entity.clone(),
            row_id: c.row_id.clone(),
            kinds: classify(c.kind, &group.actor, &group.correlation.0),
            group: group.correlation.0.clone(),
            event_id: c.id.0.clone(),
            at: c.at.0,
        })
        .collect()
}

// ── listeners ────────────────────────────────────────────────────────────

/// A registered listener: a pattern, the routine it wakes, and the
/// modifiers.
///
/// `routine` and `version` are two fields rather than one string because
/// §7's run identity is "batch id × **routine version**" — a version bump is
/// a different run of the same batch, on purpose, and joining them here
/// would make that a string-splitting problem later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    /// The domain the pattern's first position is matched against. Entity
    /// names are unqualified in a compiled program; the domain comes from
    /// the program header.
    pub domain: String,
    pub pattern: ListenerPattern,
    pub routine: String,
    pub version: String,
    pub modifiers: Modifiers,
}

impl Listener {
    /// The name this listener registers its log cursor and its batches
    /// under. Stable across versions **on purpose**: a routine's version
    /// changes what a run does, not where in history it has got to, and
    /// resetting a cursor on deploy would replay the whole chain.
    pub fn key(&self) -> String {
        format!("{}:{}", self.routine, self.pattern.render())
    }

    /// Does this event wake this listener?
    ///
    /// All three positions, and the kind position matches if the pattern's
    /// kind is **among** the event's kinds — not equal to one of them.
    /// `ingested` and `fed` are narrowings, so an ingest raises `created`,
    /// `written` and `ingested` together and a listener on any of the three
    /// is woken by it.
    pub fn matches(&self, event: &Event) -> bool {
        self.pattern.domain.matches(&self.domain)
            && self.pattern.entity.matches(&event.entity)
            && match self.pattern.kind {
                KindPos::Any => true,
                KindPos::Kind(k) => event.kinds.contains(&k),
            }
    }
}

// ── `$trigger` ───────────────────────────────────────────────────────────

/// §7's **blind but addressable** run context.
///
/// "the run receives an opaque `$trigger` context — command-group id,
/// **affected row ids**, batched event ids, timestamp. All plaintext-skeleton
/// data (no field values), so trigger blindness is intact — but rules can
/// scope to what changed (`events(id in $trigger.ids and …)`) instead of
/// rescanning history."
///
/// There is deliberately no field-value accessor and no place to add one. A
/// routine that must see a value reads it in its first line, under a grant,
/// with its key release logged — that is §7's own answer, and it is why this
/// struct is as thin as it is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TriggerContext {
    /// The command-group id, when one wake-up came from one group. `None`
    /// for a debounced batch spanning several groups, and for every clock
    /// firing.
    pub group: Option<String>,
    /// The entity the events landed on. `None` for a clock firing.
    pub entity: Option<String>,
    /// Affected row ids. **Empty for a clock-fired run** — §7: "Clock-fired
    /// runs get an empty id set." Empty is not "unbounded": under R-G20 a
    /// clock run is bounded by guards passing, and a *listener* run with an
    /// empty id set can reach nothing at all, which is the correct answer
    /// for a listener that matched no rows.
    pub ids: Vec<String>,
    /// Batched event ids — the per-command ids the batch accumulated.
    pub events: Vec<String>,
    pub at: u64,
}

impl TriggerContext {
    /// The context a clock firing hands its run: no group, no entity, no
    /// ids. Spelled as a constructor so that "clock runs get an empty id
    /// set" is a thing the code says rather than a thing it happens to do.
    pub fn clock(at: u64) -> TriggerContext {
        TriggerContext {
            at,
            ..TriggerContext::default()
        }
    }
}

// ── batching: debounce and coalesce ──────────────────────────────────────

/// An accumulating batch, before it becomes a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub space: SpaceId,
    pub listener: String,
    pub batch: String,
    pub first_at: u64,
    pub last_at: u64,
    pub ids: Vec<String>,
    pub events: Vec<String>,
}

/// Fold an event into the space's open batch for this listener.
///
/// Called **inside the write transaction that produced the event**, for the
/// property `jobs.rs` opens with: an aborted write accumulates nothing
/// because the upsert rolls back with everything else, and a committed one
/// publishes work and effect in the same instant. There is no outbox and no
/// delivery bookkeeping because there is no window for one to cover.
///
/// The batch id is derived from `(space, listener, first_at)` of the batch
/// it opened — deterministic, so a retried transaction that re-opens the
/// same batch produces the same run key and §7's "redelivery is a no-op"
/// holds through a retry as well as through a redelivery.
pub async fn accumulate(
    tx: &TxConn,
    listener: &Listener,
    event: &Event,
    now: u64,
) -> Result<(), PoolError> {
    let key = listener.key();
    let batch = format!("b-{}-{}", key, now);
    tx.rows(&format!(
        "INSERT INTO \"$triggerbatches\" \
           (\"space\", \"listener\", \"batch\", \"first_at\", \"last_at\", \"ids\", \"events\") \
         VALUES ({space}, {lk}, {b}, {now}, {now}, {ids}, {evs}) \
         ON CONFLICT (\"space\", \"listener\") DO UPDATE SET \
           \"last_at\" = {now}, \
           \"ids\" = CASE WHEN position({id_probe} in \"$triggerbatches\".\"ids\") > 0 \
                          THEN \"$triggerbatches\".\"ids\" \
                          ELSE \"$triggerbatches\".\"ids\" || {id_app} END, \
           \"events\" = \"$triggerbatches\".\"events\" || {ev_app}",
        space = sql_lit(&event.space.0),
        lk = sql_lit(&key),
        b = sql_lit(&batch),
        ids = sql_lit(&pack(std::slice::from_ref(&event.row_id))),
        evs = sql_lit(&pack(std::slice::from_ref(&event.event_id))),
        // Row ids are a SET: a burst that touches one row ten times is one
        // row in `$trigger.ids`, because `$trigger.ids` is what the run
        // scopes to and a duplicate there is a repeated scan, not a second
        // row. Event ids are a LIST, because "batched event ids" is the
        // audit answer to "what was in this batch" and deduplicating it
        // would lose the count.
        id_probe = sql_lit(&pack(std::slice::from_ref(&event.row_id))),
        id_app = sql_lit(&pack(std::slice::from_ref(&event.row_id))),
        ev_app = sql_lit(&pack(std::slice::from_ref(&event.event_id))),
    ))
    .await
    .map(|_| ())
}

/// The separator for the packed id lists. `\u{1f}` (unit separator) for the
/// same reason the conformance harness uses it: it cannot occur in an
/// identifier, so containment testing a packed list is exact rather than a
/// prefix accident.
const SEP: char = '\u{1f}';

fn pack(items: &[String]) -> String {
    let mut out = String::new();
    for i in items {
        out.push(SEP);
        out.push_str(i);
    }
    out.push(SEP);
    out
}

fn unpack(s: &str) -> Vec<String> {
    s.split(SEP)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

/// Is this batch due to fire?
///
/// §7: "`debounce` is trailing-edge — the run fires 5 minutes after the last
/// event in the burst, with a declared max-wait".
///
/// Both clauses, and the second is not optional. A trailing edge alone means
/// a steady stream arriving faster than the window defers the run **forever**
/// — a listener that compiles and never fires, which is the failure
/// `unknown_event_kind` exists to prevent, reached through the other door.
/// `Modifiers` sets window and max-wait together precisely so this function
/// cannot be written with only the first half.
pub fn batch_due(m: &Modifiers, b: &Batch, now: u64) -> bool {
    match (m.debounce, m.max_wait) {
        // No window: the first event is the burst.
        (None, _) => true,
        (Some(window), max) => {
            let quiet = now.saturating_sub(b.last_at) >= window;
            let waited = max.is_some_and(|w| now.saturating_sub(b.first_at) >= w);
            quiet || waited
        }
    }
}

/// Close every due batch and enqueue its run.
///
/// Returns the run keys enqueued. Three §7 rules meet here:
///
/// - **at-least-once**: the batch row is deleted and the run row inserted in
///   one transaction, so a crash between them is impossible; a crash before
///   leaves the batch open (and due again), a crash after leaves the run
///   queued.
/// - **redelivery is a no-op**: the run key is `batch × routine@version` and
///   the insert is `ON CONFLICT DO NOTHING` on it.
/// - **`coalesce` caps pending runs at one**: with `coalesce` set, a batch
///   whose listener already has an unfinished run **stays open** rather than
///   queueing a second. It keeps accumulating and fires when the in-flight
///   run has finished, which is what "at most one pending run" has to mean
///   for a trailing-edge window — dropping the batch instead would lose
///   events, and at-least-once is not negotiable against a modifier.
pub async fn fire_due(
    tx: &TxConn,
    listeners: &[Listener],
    now: u64,
) -> Result<Vec<String>, PoolError> {
    let mut fired = Vec::new();
    for l in listeners {
        let key = l.key();
        let rows = tx
            .rows(&format!(
                "SELECT \"space\", \"batch\", \"first_at\", \"last_at\", \"ids\", \"events\" \
                 FROM \"$triggerbatches\" WHERE \"listener\" = {} ORDER BY \"space\" \
                 FOR UPDATE SKIP LOCKED",
                sql_lit(&key)
            ))
            .await?;
        for r in rows {
            let Some(b) = batch_of(&key, &r) else {
                continue;
            };
            if !batch_due(&l.modifiers, &b, now) {
                continue;
            }
            if l.modifiers.coalesce && pending_run(tx, &b.space, &key).await? {
                continue;
            }
            let run_key = format!("{}:{}@{}", b.batch, l.routine, l.version);
            let payload = encode_payload(&b);
            tx.rows(&format!(
                "INSERT INTO \"$triggerruns\" \
                   (\"space\", \"run_key\", \"listener\", \"payload\", \"next_attempt\") \
                 VALUES ({}, {}, {}, {}, {now}) \
                 ON CONFLICT (\"space\", \"run_key\") DO NOTHING",
                sql_lit(&b.space.0),
                sql_lit(&run_key),
                sql_lit(&key),
                sql_lit(&payload),
            ))
            .await?;
            tx.rows(&format!(
                "DELETE FROM \"$triggerbatches\" WHERE \"space\" = {} AND \"listener\" = {}",
                sql_lit(&b.space.0),
                sql_lit(&key),
            ))
            .await?;
            fired.push(run_key);
        }
    }
    Ok(fired)
}

async fn pending_run(tx: &TxConn, space: &SpaceId, listener: &str) -> Result<bool, PoolError> {
    let rows = tx
        .rows(&format!(
            "SELECT 1 FROM \"$triggerruns\" WHERE \"space\" = {} AND \"listener\" = {} LIMIT 1",
            sql_lit(&space.0),
            sql_lit(listener),
        ))
        .await?;
    Ok(!rows.is_empty())
}

fn batch_of(listener: &str, r: &[Option<String>]) -> Option<Batch> {
    Some(Batch {
        space: SpaceId(r.first()?.clone()?),
        listener: listener.to_string(),
        batch: r.get(1)?.clone()?,
        first_at: r.get(2)?.clone()?.parse().ok()?,
        last_at: r.get(3)?.clone()?.parse().ok()?,
        ids: unpack(&r.get(4)?.clone()?),
        events: unpack(&r.get(5)?.clone()?),
    })
}

fn encode_payload(b: &Batch) -> String {
    format!(
        "{}{SEP}{}{SEP}{}",
        b.batch,
        b.ids.join(","),
        b.events.join(",")
    )
}

// ── runs: claim, finish, retry, dead-letter ──────────────────────────────

/// One unit of routine work, claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub space: SpaceId,
    pub run_key: String,
    pub listener: String,
    pub attempts: i32,
    pub batch: String,
    pub ids: Vec<String>,
    pub events: Vec<String>,
}

impl Run {
    /// The `$trigger` this run hands its body.
    pub fn context(&self, at: u64) -> TriggerContext {
        TriggerContext {
            group: Some(self.batch.clone()),
            entity: None,
            ids: self.ids.clone(),
            events: self.events.clone(),
            at,
        }
    }
}

/// How many attempts a run gets before it dead-letters.
///
/// Engine-pinned. §7 says "failures retry with backoff, then dead-letter as
/// an operator alert" and gives no number, and no document in the corpus
/// does. A number is required for the sentence to be implementable at all,
/// so it is named here where an operator can find it rather than buried at a
/// call site.
pub const MAX_ATTEMPTS: i32 = 5;

/// Exponential, capped. Attempt 1 waits a second, attempt 5 waits sixteen.
pub fn backoff_secs(attempts: i32) -> u64 {
    1u64 << attempts.clamp(0, 6)
}

/// Claim up to `n` runs that are due for an attempt.
///
/// `FOR UPDATE SKIP LOCKED` for the reason `claim_jobs` gives: replicas
/// compete without a broker, and a row another worker holds is skipped
/// rather than waited on.
pub async fn claim_runs(tx: &TxConn, now: u64, n: i64) -> Result<Vec<Run>, PoolError> {
    let rows = tx
        .rows(&format!(
            // A MATERIALIZED CTE so at most `n` are claimed whatever the
            // plan (`jobs::claim_jobs`).
            "WITH \"next\" AS MATERIALIZED ( \
               SELECT \"space\", \"run_key\" FROM \"$triggerruns\" \
               WHERE NOT \"claimed\" AND \"next_attempt\" <= {now} \
               ORDER BY \"next_attempt\", \"run_key\" LIMIT {n} FOR UPDATE SKIP LOCKED) \
             UPDATE \"$triggerruns\" AS r SET \"claimed\" = true, \"attempts\" = r.\"attempts\" + 1 \
             FROM \"next\" WHERE r.\"space\" = \"next\".\"space\" AND r.\"run_key\" = \"next\".\"run_key\" \
             RETURNING r.\"space\", r.\"run_key\", r.\"listener\", r.\"payload\", r.\"attempts\""
        ))
        .await?;
    Ok(rows.iter().filter_map(|r| run_of(r)).collect())
}

fn run_of(r: &[Option<String>]) -> Option<Run> {
    let payload = r.get(3)?.clone()?;
    let mut parts = payload.split(SEP);
    let batch = parts.next()?.to_string();
    let ids = parts.next().unwrap_or_default();
    let events = parts.next().unwrap_or_default();
    Some(Run {
        space: SpaceId(r.first()?.clone()?),
        run_key: r.get(1)?.clone()?,
        listener: r.get(2)?.clone()?,
        attempts: r.get(4)?.clone()?.parse().ok()?,
        batch,
        ids: split_csv(ids),
        events: split_csv(events),
    })
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

/// A run that succeeded. The record goes away, which is what frees a
/// `coalesce`d listener to queue its next batch.
pub async fn finish_run(tx: &TxConn, run: &Run) -> Result<(), PoolError> {
    tx.rows(&format!(
        "DELETE FROM \"$triggerruns\" WHERE \"space\" = {} AND \"run_key\" = {}",
        sql_lit(&run.space.0),
        sql_lit(&run.run_key),
    ))
    .await
    .map(|_| ())
}

/// What happened to a failed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    /// Released for another attempt at the returned instant.
    Retrying { at: u64 },
    /// Out of attempts. The run is in `$deadletters` and an operator has to
    /// look at it.
    DeadLettered,
}

/// A run that failed: back off, or dead-letter.
///
/// §7: "failures retry with backoff, then **dead-letter as an operator
/// alert**". Dead-lettering is deliberately not a silent drop and not an
/// infinite retry: an infinite retry is a listener that pins the floor
/// forever while doing nothing, and a drop is at-least-once quietly becoming
/// zero-times. The dead letter keeps the payload, so the batch's ids survive
/// for whoever replays it.
pub async fn fail_run(tx: &TxConn, run: &Run, reason: &str, now: u64) -> Result<Failed, PoolError> {
    if run.attempts >= MAX_ATTEMPTS {
        tx.rows(&format!(
            "INSERT INTO \"$deadletters\" \
               (\"space\", \"run_key\", \"listener\", \"payload\", \"attempts\", \"at\", \"reason\") \
             SELECT \"space\", \"run_key\", \"listener\", \"payload\", \"attempts\", {now}, {reason} \
             FROM \"$triggerruns\" WHERE \"space\" = {space} AND \"run_key\" = {rk} \
             ON CONFLICT (\"space\", \"run_key\") DO NOTHING",
            reason = sql_lit(reason),
            space = sql_lit(&run.space.0),
            rk = sql_lit(&run.run_key),
        ))
        .await?;
        tx.rows(&format!(
            "DELETE FROM \"$triggerruns\" WHERE \"space\" = {} AND \"run_key\" = {}",
            sql_lit(&run.space.0),
            sql_lit(&run.run_key),
        ))
        .await?;
        return Ok(Failed::DeadLettered);
    }
    let at = now + backoff_secs(run.attempts);
    tx.rows(&format!(
        "UPDATE \"$triggerruns\" SET \"claimed\" = false, \"next_attempt\" = {at} \
         WHERE \"space\" = {} AND \"run_key\" = {}",
        sql_lit(&run.space.0),
        sql_lit(&run.run_key),
    ))
    .await?;
    Ok(Failed::Retrying { at })
}

/// One dead-lettered run, as an operator sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetter {
    pub space: SpaceId,
    pub run_key: String,
    pub listener: String,
    pub attempts: i32,
    pub at: u64,
    pub reason: String,
}

/// The operator surface. §7 calls a dead letter "an operator alert", and an
/// alert nobody can enumerate is a log line.
pub async fn dead_letters(pool: &Pool, space: &SpaceId) -> Result<Vec<DeadLetter>, PoolError> {
    let rows = pool
        .rows(&format!(
            "SELECT \"run_key\", \"listener\", \"attempts\", \"at\", \"reason\" \
             FROM \"$deadletters\" WHERE \"space\" = {} ORDER BY \"at\", \"run_key\"",
            sql_lit(&space.0)
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(DeadLetter {
                space: space.clone(),
                run_key: r.first()?.clone()?,
                listener: r.get(1)?.clone()?,
                attempts: r.get(2)?.clone()?.parse().ok()?,
                at: r.get(3)?.clone()?.parse().ok()?,
                reason: r.get(4)?.clone()?,
            })
        })
        .collect())
}

// ── the listener cursor: reading history forward ─────────────────────────

/// Register a listener's cursor at the current head, so it pins the floor
/// from the moment it exists.
///
/// `ConsumerKind::Listener` and not `Mirror`, and the difference is the
/// guarantee: a mirror may be evicted when it stalls, "so one abandoned
/// device cannot pin a space's history forever"; a listener may not, because
/// "evicting one and resuming it at the current floor would silently turn
/// at-least-once into zero-times".
pub async fn register_listener(
    pool: &Pool,
    space: &SpaceId,
    listener: &Listener,
    position: u64,
    now: u64,
) -> Result<(), PoolError> {
    prune::register_consumer(
        pool,
        space,
        &listener.key(),
        prune::ConsumerKind::Listener,
        position,
        now,
    )
    .await
}

/// Where a listener has got to.
pub async fn cursor(pool: &Pool, space: &SpaceId, listener: &Listener) -> Result<u64, PoolError> {
    let key = listener.key();
    Ok(prune::consumers(pool, space)
        .await?
        .into_iter()
        .find(|c| c.name == key)
        .map(|c| c.position)
        .unwrap_or(0))
}

/// Where a listener has got to, or `None` when it has no cursor.
pub async fn registered_cursor(
    pool: &Pool,
    space: &SpaceId,
    listener: &Listener,
) -> Result<Option<u64>, PoolError> {
    let key = listener.key();
    Ok(prune::consumers(pool, space)
        .await?
        .into_iter()
        .find(|c| c.name == key)
        .map(|c| c.position))
}

/// The action the security stream carries when a listener cursor is
/// retired. Alerting keys off it.
pub const DEREGISTERED_ACTION: &str = "listener_deregistered";

/// Retire a listener cursor and record the act, in one transaction, so
/// there is no retirement without a record and no record without a
/// retirement. Returns the position the cursor held, or `None` when there
/// was no such cursor.
///
/// The one deregister path: `ordo triggers deregister` and the drain's
/// retirement of a routine's previous cursor (audit 2026-10-08 finding 7)
/// both come here. The record is a `CustodyAction` whose action string is
/// `listener_deregistered position=<n> forced=<bool> listener=<name>`.
/// `actor` is attribution: who or what retired it.
pub async fn retire_listener(
    pool: &Pool,
    space: &SpaceId,
    name: &str,
    actor: &str,
    op: &str,
    forced: bool,
    at: u64,
) -> Result<Option<u64>, String> {
    let tx = pool.begin().await.map_err(|e| e.to_string())?;
    let Some(position) = prune::deregister_consumer_in(&tx, space, name)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let at = ordo_commandlog::Timestamp(at);
    let recorder = ordo_secevent::Recorder::default();
    recorder.observe(
        ordo_secevent::Observation {
            space: Some(space.clone()),
            subject: ordo_secevent::Subject::member(actor),
            op: op.into(),
            kind: ordo_secevent::EventKind::CustodyAction {
                action: format!(
                    "{DEREGISTERED_ACTION} position={position} forced={forced} listener={name}"
                ),
            },
            at,
            correlation: ordo_secevent::correlation::mint(),
        },
        ordo_secevent::Relationship::Member,
    );
    for e in &recorder.drain_all(at) {
        tx.append_security_entry(e)
            .await
            .map_err(|e| format!("the record could not be written: {e}"))?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(Some(position))
}

/// Drain the log forward for one listener: accumulate every matching event
/// into the batch, then advance the cursor.
///
/// **The order matters and is the guarantee.** The cursor advances only
/// after the batch upsert has committed, so a crash mid-drain replays the
/// window and re-accumulates — at-least-once, exactly as promised, and
/// harmless because row ids are a set. Advancing first would have been
/// at-most-once wearing the same code.
///
/// Ordering is **per space only**, which is §7's promise and also the only
/// one the chain can keep: `$commandlog` is a per-space total order and
/// there is no cross-space clock to sequence two of them against.
pub async fn drain_listener(
    pool: &Pool,
    store: &crate::commandlog::PgStore,
    space: &SpaceId,
    listener: &Listener,
    limit: usize,
    now: u64,
) -> Result<usize, PoolError> {
    drain_listener_to(pool, store, pool, space, listener, limit, now).await
}

/// [`drain_listener`] for a tenant whose chain is in a database of its own
/// (checklist 12a).
///
/// The cursor and the chain are read on `log_pool`, the tenant's database,
/// because the cursor is what pins that database's pruning floor. The batch
/// is written on `runs_pool`, where the runs are claimed. The order is the
/// same guarantee: the batch commits first, the cursor advances after, so a
/// crash between them replays the window.
///
/// A listener with no cursor reads nothing. Its cursor was retired, by a
/// person or by a newer version that took over its routine (audit
/// 2026-10-08 finding 7), and reading from 0 would replay the whole chain.
pub async fn drain_listener_to(
    log_pool: &Pool,
    store: &crate::commandlog::PgStore,
    runs_pool: &Pool,
    space: &SpaceId,
    listener: &Listener,
    limit: usize,
    now: u64,
) -> Result<usize, PoolError> {
    let Some(from) = registered_cursor(log_pool, space, listener).await? else {
        return Ok(0);
    };
    let entries: Vec<ChainedGroup> = store
        .try_entries(space, from as usize, limit)
        .await
        .map_err(|e| PoolError::db(e.to_string()))?;
    if entries.is_empty() {
        return Ok(0);
    }
    let read = entries.len();
    let tx = runs_pool.begin().await?;
    for entry in &entries {
        for event in events_of(&entry.group) {
            if listener.matches(&event) {
                accumulate(&tx, listener, &event, now).await?;
            }
        }
    }
    tx.commit().await?;
    prune::advance_consumer(log_pool, space, &listener.key(), from + read as u64, now).await?;
    Ok(read)
}

// ── the clock ────────────────────────────────────────────────────────────

/// A clock-woken routine: a schedule and the routine it fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockRoutine {
    pub routine: String,
    pub version: String,
    pub schedule: Schedule,
}

impl ClockRoutine {
    /// The `$schedules` row name. The **version is not in it**, for the same
    /// reason it is not in a listener's cursor name: retiming or
    /// re-deploying a routine must not make it fire twice, and a name
    /// carrying the version would make every deploy a fresh, immediately-due
    /// schedule.
    pub fn key(&self) -> String {
        format!("routine:{}", self.routine)
    }
}

/// Register every clock routine's schedule. Idempotent — `schedule` is `ON
/// CONFLICT DO NOTHING`, so a redeploy does not reset a `next_due` and a
/// pending firing survives it.
pub async fn register_clock(
    pool: &Pool,
    routines: &[ClockRoutine],
    now: u64,
) -> Result<(), PoolError> {
    let tx = pool.begin().await?;
    for r in routines {
        tx.schedule(&r.key(), r.schedule.next_due(now) as i64)
            .await?;
    }
    tx.commit().await
}

/// Fire every due clock routine, enqueueing one run each.
///
/// Uses `claim_schedule` **per routine** rather than `claim_due` for the
/// reason `jobs.rs` records: `claim_due` advances every due schedule by the
/// one period it was handed, which silently gives two routines with
/// different cadences whichever period won the race. Two routines with
/// different schedules is the ordinary case here, not the exotic one.
///
/// Leadership is the caller's business — [`TxConn::try_lead`] keeps a
/// hundred replicas from all trying at once, and `claim_schedule` is what
/// makes firing exactly-once. §7 wants both and they are not the same
/// property.
pub async fn fire_clock(
    pool: &Pool,
    space: &SpaceId,
    routines: &[ClockRoutine],
    now: u64,
) -> Result<Vec<String>, PoolError> {
    fire_clock_in(pool, std::slice::from_ref(space), routines, now).await
}

/// Is any of `routines` due at `now`? One read of `$schedules`, so the
/// clock worker lists the tenants only when it has something to enqueue
/// for them (audit 2026-10-08 finding 7).
pub async fn clock_due(
    pool: &Pool,
    routines: &[ClockRoutine],
    now: u64,
) -> Result<bool, PoolError> {
    if routines.is_empty() {
        return Ok(false);
    }
    let names: Vec<String> = routines.iter().map(|r| sql_lit(&r.key())).collect();
    let rows = pool
        .rows(&format!(
            "SELECT 1 FROM \"$schedules\" WHERE \"name\" IN ({}) AND \"next_due\" <= {now} LIMIT 1",
            names.join(", ")
        ))
        .await?;
    Ok(!rows.is_empty())
}

/// [`fire_clock`] for every tenant at once (checklist 12a): one claim of the
/// schedule, and one run per space, in the claim's transaction. A schedule
/// is a property of the program, so it fires once for the deployment, and
/// a run is a property of a tenant, so each tenant's run retries and
/// dead-letters on its own.
pub async fn fire_clock_in(
    pool: &Pool,
    spaces: &[SpaceId],
    routines: &[ClockRoutine],
    now: u64,
) -> Result<Vec<String>, PoolError> {
    let mut fired = Vec::new();
    for r in routines {
        let tx = pool.begin().await?;
        let due = tx
            .claim_schedule(&r.key(), now as i64, r.schedule.period_secs() as i64)
            .await?;
        if due {
            let run_key = format!("clock-{now}:{}@{}", r.routine, r.version);
            for space in spaces {
                tx.rows(&format!(
                    "INSERT INTO \"$triggerruns\" \
                       (\"space\", \"run_key\", \"listener\", \"payload\", \"next_attempt\") \
                     VALUES ({}, {}, {}, {}, {now}) \
                     ON CONFLICT (\"space\", \"run_key\") DO NOTHING",
                    sql_lit(&space.0),
                    sql_lit(&run_key),
                    sql_lit(&r.key()),
                    // A clock run's payload has a batch id and no ids and no
                    // events — §7's "Clock-fired runs get an empty id set",
                    // written down rather than arrived at.
                    sql_lit(&format!("clock-{now}{SEP}{SEP}")),
                ))
                .await?;
            }
            fired.push(run_key);
        }
        tx.commit().await?;
    }
    Ok(fired)
}

// ── reachability: resolving `$trigger.ids` into a row bound ──────────────

/// The rows a listener-woken run may touch, resolved against the database.
///
/// # What "reachable" means here, and what it does not
///
/// §8 bounds a listener-woken run to "rows reachable from `$trigger.ids`"
/// and never defines the relation. This resolves it as **one declared ref
/// hop, in either direction**, plus the trigger rows themselves:
///
/// - the trigger entity's own rows — `id in $trigger.ids`;
/// - a child row that *points at* a trigger row through a declared ref
///   (`taskNotes.task -> tasks`);
/// - a parent row a trigger row *points at* through a declared ref
///   (`tasks.project -> projects`).
///
/// Two hops and beyond are **not built**, and the consequence is that such
/// rows are invisible rather than visible. That is the safe direction and it
/// is deliberate: a bound that is tighter than the specification can never be
/// a confused deputy, while one that is looser is exactly the escalation
/// R-G20 exists to close. A routine that legitimately needs to range wider
/// already has §8's answer — `unguarded` on the run header, one greppable
/// word, a decision someone made on purpose.
///
/// Nothing here reads a value: it reads `id` and declared ref columns, which
/// are plaintext skeleton by construction (a sealed field can never be a
/// ref). Trigger blindness survives the walk.
pub fn reachable(
    db: &dyn WriteDb,
    schema: &ordo_schema::Schema,
    entity: &str,
    ids: &[String],
    wanted: &[String],
) -> Result<ordo_sema::TriggerRows, String> {
    let mut rows = ordo_sema::TriggerRows::default();
    if ids.is_empty() {
        // Every wanted entity resolves to nothing, which lowers to `false`.
        // Said explicitly so that "no trigger rows" and "never asked" are
        // the same visibility answer and not two code paths.
        return Ok(rows);
    }
    let list = ids
        .iter()
        .map(|i| sql_lit(i))
        .collect::<Vec<_>>()
        .join(", ");
    for want in wanted {
        if want == entity {
            rows = rows.with(want, ids.to_vec());
            continue;
        }
        let Some(e) = schema.entity(want) else {
            continue;
        };
        let mut found: BTreeSet<String> = BTreeSet::new();
        // Child side: `want` has a ref at the trigger entity.
        for f in &e.fields {
            if let ordo_schema::FieldTy::Ref { target, .. } = &f.ty {
                if target == entity {
                    let sql = format!(
                        "SELECT \"id\" FROM \"{}\" WHERE \"{}\" IN ({list})",
                        ident(want),
                        ident(&f.name)
                    );
                    for r in db.rows(&sql)? {
                        if let Some(Some(id)) = r.first() {
                            found.insert(id.clone());
                        }
                    }
                }
            }
        }
        // Parent side: the trigger entity has a ref at `want`.
        if let Some(te) = schema.entity(entity) {
            for f in &te.fields {
                if let ordo_schema::FieldTy::Ref { target, .. } = &f.ty {
                    if target == want {
                        let sql = format!(
                            "SELECT \"{}\" FROM \"{}\" WHERE \"id\" IN ({list})",
                            ident(&f.name),
                            ident(entity)
                        );
                        for r in db.rows(&sql)? {
                            if let Some(Some(id)) = r.first() {
                                found.insert(id.clone());
                            }
                        }
                    }
                }
            }
        }
        rows = rows.with(want, found.into_iter().collect());
    }
    Ok(rows)
}

/// Entity and field names come from a compiled schema and cannot contain a
/// quote; doubling one anyway costs nothing and means this function is safe
/// read in isolation.
fn ident(s: &str) -> String {
    s.replace('"', "\"\"")
}

/// How a run was woken — the only input R-G20 takes.
///
/// Named as a type rather than passed as a bool because the two arms are not
/// degrees of the same thing. "Guards pass" and "bounded to `$trigger.ids`"
/// are opposite answers, and the reason they differ is *who chose the
/// moment*, which a boolean cannot carry into a reader's head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Woken {
    /// `every …`
    Clock,
    /// `when …`, on rows of `entity`.
    Listener { entity: String },
}

/// **The authorization step of a routine run** — R-G20, resolved.
///
/// Everything above this line decides *when* a routine runs; this decides
/// *what it may touch* when it does, and the two are the same decision seen
/// twice, because the trigger is the input to both.
///
/// - **clock** → [`Provenance::Clock`]: guards pass. Nobody outside chose
///   the moment, there is no confused deputy to be, and a clock-driven rule
///   can never silently do nothing.
/// - **listener** → [`Provenance::Listener`] over the reachable set. A
///   listener fires on `ingested` and `fed`, both of which are outsiders,
///   and `$trigger.ids` is exactly the rows the outsider just wrote. Without
///   this bound a guard-exempt listener does work an outsider steered on
///   rows the outsider could never touch.
///
/// `wanted` is the entity list the run's body reads — its derived scope's
/// entities. Anything not in it, and anything in it the walk could not
/// reach, is invisible.
///
/// The `unguarded` half of the rule is deliberately **not** here: it lives
/// on the `run` header and is read by [`ordo_sema::bind_routine`], so that
/// the flag is consulted at the point where the bound is applied rather than
/// at the point where it is computed. That keeps one greppable word doing
/// one job, and keeps a lifted bound out of this function's answer — which
/// is what an audit surface wants to render.
pub fn bound_for(
    db: &dyn WriteDb,
    schema: &ordo_schema::Schema,
    woken: &Woken,
    ctx: &TriggerContext,
    wanted: &[String],
) -> Result<ordo_sema::Provenance, String> {
    match woken {
        Woken::Clock => Ok(ordo_sema::Provenance::Clock),
        Woken::Listener { entity } => Ok(ordo_sema::Provenance::Listener(reachable(
            db, schema, entity, &ctx.ids, wanted,
        )?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_commandlog::{Command, CommandId, Image, Timestamp};

    fn cmd(entity: &str, row: &str, kind: CommandKind) -> Command {
        let img = Image::new();
        Command {
            id: CommandId(format!("cmd_{row}")),
            space: SpaceId("s".into()),
            entity: entity.into(),
            row_id: row.into(),
            kind,
            before: match kind {
                CommandKind::Create => None,
                _ => Some(img.clone()),
            },
            after: match kind {
                CommandKind::Delete => None,
                _ => Some(img),
            },
            at: Timestamp(10),
        }
    }

    #[test]
    fn written_is_on_every_command_and_the_base_kind_follows_the_mode() {
        for (mode, base) in [
            (CommandKind::Create, EventKind::Created),
            (CommandKind::Converge, EventKind::Converged),
            (CommandKind::Delete, EventKind::Deleted),
        ] {
            let k = classify(mode, &Actor::User("u".into()), "cg_1");
            assert!(k.contains(&base));
            assert!(k.contains(&EventKind::Written), "written is the union");
            assert_eq!(k.len(), 2, "a plain user write raises exactly two kinds");
        }
    }

    #[test]
    fn ingested_is_a_narrowing_of_created_not_a_separate_event() {
        let k = classify(
            CommandKind::Create,
            &Actor::Source("camera-hub".into()),
            "cg_1",
        );
        // §7: "`ingested` and `fed` are *narrowings* of `created`/`written`
        // by actor kind, not separate events".
        assert!(k.contains(&EventKind::Ingested));
        assert!(k.contains(&EventKind::Created));
        assert!(k.contains(&EventKind::Written));
        assert!(!k.contains(&EventKind::Fed), "not a feed principal");
        // A source CONVERGE is not an ingest — `ingested` is "a create
        // attributed to a `source:` principal", and a mail slot updating a
        // row it already created is not a new arrival.
        let k = classify(
            CommandKind::Converge,
            &Actor::Source("camera-hub".into()),
            "cg_1",
        );
        assert!(!k.contains(&EventKind::Ingested));
    }

    #[test]
    fn a_listener_on_written_sees_feed_copies_too() {
        // This is the sentence that makes feeds locally reactive, and it is
        // a property of the classification, not of the matcher.
        let k = classify(
            CommandKind::Converge,
            &Actor::Source("feed-partner".into()),
            "cg_1",
        );
        assert!(k.contains(&EventKind::Fed));
        assert!(k.contains(&EventKind::Written));
        assert!(k.contains(&EventKind::Converged));
        // A create through a feed is all four: fed narrows written, ingested
        // narrows created, and `source:feed-x` IS a `source:` principal.
        let k = classify(
            CommandKind::Create,
            &Actor::Source("feed-partner".into()),
            "cg_1",
        );
        assert!(k.contains(&EventKind::Fed));
        assert!(k.contains(&EventKind::Ingested));
    }

    #[test]
    fn reverted_is_decided_by_the_group_not_the_command() {
        // `revert_correlation` is `rev-<original>`; nothing else produces
        // that shape, so the envelope answers "a group revert touching the
        // entity" with no flag column to forget.
        let k = classify(CommandKind::Converge, &Actor::User("u".into()), "rev-cg_1");
        assert!(k.contains(&EventKind::Reverted));
        assert!(k.contains(&EventKind::Converged));
        let k = classify(CommandKind::Converge, &Actor::User("u".into()), "cg_1");
        assert!(!k.contains(&EventKind::Reverted));
        // A redo (`rev-rev-…`) is still a revert-shaped group.
        let k = classify(
            CommandKind::Converge,
            &Actor::User("u".into()),
            "rev-rev-cg_1",
        );
        assert!(k.contains(&EventKind::Reverted));
    }

    fn listener(pattern: &str) -> Listener {
        Listener {
            domain: "market".into(),
            pattern: ListenerPattern::parse(pattern).unwrap(),
            routine: "seedTaskNotes".into(),
            version: "1".into(),
            modifiers: Modifiers::default(),
        }
    }

    fn ingest_event() -> Event {
        let g = CommandGroup {
            correlation: ordo_commandlog::CorrelationId("cg_1".into()),
            actor: Actor::Source("hub".into()),
            commands: vec![cmd("tasks", "task_6", CommandKind::Create)],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        };
        events_of(&g).remove(0)
    }

    #[test]
    fn matching_is_three_positions_and_the_kind_is_membership() {
        let e = ingest_event();
        for hit in [
            "market.tasks.ingested",
            "market.tasks.created",
            "market.tasks.written",
            "market.*.ingested",
            "*.tasks.ingested",
            "*.*.*",
        ] {
            assert!(listener(hit).matches(&e), "{hit} must match an ingest");
        }
        for miss in [
            "market.tasks.deleted",
            "market.tasks.converged",
            "market.tasks.fed",
            "market.taskNotes.ingested",
            "finance.tasks.ingested",
        ] {
            assert!(!listener(miss).matches(&e), "{miss} must not match");
        }
    }

    #[test]
    fn a_debounce_window_is_trailing_edge_and_cannot_defer_forever() {
        let m = Modifiers::parse("debounce 10s").unwrap();
        let b = Batch {
            space: SpaceId("s".into()),
            listener: "l".into(),
            batch: "b".into(),
            first_at: 100,
            last_at: 100,
            ids: vec![],
            events: vec![],
        };
        assert!(!batch_due(&m, &b, 105), "still inside the window");
        assert!(
            batch_due(&m, &b, 110),
            "trailing edge: quiet for the window"
        );

        // A burst that never goes quiet: `last_at` keeps moving with the
        // clock, so the trailing edge never arrives — and the declared
        // max-wait is the only thing between this listener and never firing.
        // 10s window, so `DEFAULT_MAX_WAIT_FACTOR` × 10 = 100s of patience.
        let max = 10 * ordo_syntax::trigger::DEFAULT_MAX_WAIT_FACTOR;
        assert_eq!(m.max_wait, Some(max));
        let mut now = 100;
        while now < 100 + max {
            let busy = Batch {
                // Something landed a moment ago, every moment.
                last_at: now - 1,
                ..b.clone()
            };
            assert!(!batch_due(&m, &busy, now), "never quiet at {now}");
            now += 5;
        }
        let still_busy = Batch {
            last_at: 100 + max - 1,
            ..b.clone()
        };
        assert!(
            batch_due(&m, &still_busy, 100 + max),
            "the declared max-wait fires it anyway"
        );

        // No window at all: the first event is the burst.
        assert!(batch_due(&Modifiers::default(), &b, 100));
    }

    #[test]
    fn backoff_grows_and_stops_growing() {
        assert_eq!(backoff_secs(1), 2);
        assert_eq!(backoff_secs(4), 16);
        assert_eq!(backoff_secs(99), backoff_secs(6));
    }

    #[test]
    fn a_clock_run_gets_an_empty_id_set() {
        // §7, in as many words. Asserted rather than assumed, because an
        // empty id set is also what a listener bound reaching nothing looks
        // like, and the two must not drift into meaning different things.
        let t = TriggerContext::clock(42);
        assert!(t.ids.is_empty());
        assert!(t.group.is_none());
        assert!(t.entity.is_none());
        assert_eq!(t.at, 42);
    }

    #[test]
    fn packed_lists_round_trip_and_cannot_prefix_collide() {
        assert_eq!(unpack(&pack(&["a".into(), "ab".into()])), vec!["a", "ab"]);
        // The containment probe `accumulate` uses: a packed member is
        // delimited on both sides, so "task_1" does not match "task_10".
        let packed = pack(&["task_10".into()]);
        assert!(!packed.contains(&pack(&["task_1".into()])));
        assert!(packed.contains(&pack(&["task_10".into()])));
    }

    #[test]
    fn a_listener_key_is_stable_across_versions() {
        let a = Listener {
            version: "1".into(),
            ..listener("market.tasks.ingested")
        };
        let b = Listener {
            version: "2".into(),
            ..listener("market.tasks.ingested")
        };
        // A version bump changes what a run DOES, not where in history the
        // listener has got to. If this ever stops holding, every deploy
        // replays the whole chain.
        assert_eq!(a.key(), b.key());
    }
}
