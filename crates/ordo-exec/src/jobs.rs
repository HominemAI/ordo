//! F5's remainder: the job queue, the invalidation bus, and clock
//! leadership — every cross-pod concern riding Postgres, with no broker
//! and no coordination service (implementation.md §deployment).
//!
//! # Why "fires only on commit" needs no machinery
//!
//! A trigger that fires before its write commits is a lie the rest of the
//! system cannot recover from: subscribers refetch and see the old row, a
//! job runs against state that never existed, and an aborted write leaves
//! consequences behind. The usual fix is an outbox drained after commit,
//! with its own delivery bookkeeping.
//!
//! Here the enqueue simply happens **inside the write transaction**. An
//! aborted write enqueues nothing because the insert rolls back with
//! everything else; a committed one publishes work and effect in the same
//! instant. Postgres `NOTIFY` behaves identically — notifications are
//! delivered at commit and discarded on rollback — so the two halves agree
//! by construction rather than by protocol.
//!
//! # Two properties that are not the same property
//!
//! Clock triggers need **at-most-one-holder** and **fires-once**, and the
//! platform's own review caught an earlier design conflating them. An
//! advisory lock gives the first: at most one replica is leader at a time.
//! It does NOT give the second — leadership can change between deciding a
//! schedule is due and recording that it fired, and a lock held by a
//! process that stalls says nothing about work already in flight.
//!
//! Fires-once comes from the schedule row's own claim: `FOR UPDATE SKIP
//! LOCKED` plus a `next_due` advance in one transaction. Both are here,
//! and they are separate on purpose.

use crate::{sql_lit, PoolError, TxConn};

/// The queue and schedule tables.
///
/// Applied once at startup by [`crate::Pool::ensure_platform_tables`], and
/// deliberately NOT from inside the transactional methods: `CREATE TABLE
/// IF NOT EXISTS` takes a lock even when it does nothing, so a DDL
/// statement in one open transaction blocks every other transaction that
/// runs the same guard. That is a deadlock the tests found immediately,
/// and it is the reason schema setup belongs at the door rather than on
/// the hot path. A deployment's migration story owns these for real.
pub const JOBS_DDL: &str = "\
CREATE TABLE IF NOT EXISTS \"$jobs\" ( \
  \"id\" bigserial PRIMARY KEY, \
  \"kind\" text NOT NULL, \
  \"payload\" text NOT NULL, \
  \"claimed\" boolean NOT NULL DEFAULT false \
); \
CREATE TABLE IF NOT EXISTS \"$schedules\" ( \
  \"name\" text PRIMARY KEY, \
  \"next_due\" bigint NOT NULL, \
  \"fires\" bigint NOT NULL DEFAULT 0 \
)";

/// One unit of deferred work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: String,
}

impl TxConn {
    /// Enqueue work **as part of this transaction**. It becomes visible to
    /// workers when — and only when — the transaction commits.
    pub async fn enqueue(&self, kind: &str, payload: &str) -> Result<(), PoolError> {
        self.rows(&format!(
            "INSERT INTO \"$jobs\" (\"kind\", \"payload\") VALUES ({}, {})",
            sql_lit(kind),
            sql_lit(payload)
        ))
        .await
        .map(|_| ())
    }

    /// Signal every replica that an op's results may be stale. Postgres
    /// holds the notification until commit, so this cannot outrun its own
    /// write either.
    ///
    /// A staleness signal, never a data push: subscribers refetch through
    /// the ordinary op under their own scope, so a notification carries no
    /// rows and therefore leaks nothing to a subscriber whose guards would
    /// have hidden them.
    pub async fn notify(&self, channel: &str, op: &str) -> Result<(), PoolError> {
        self.rows(&format!(
            "SELECT pg_notify({}, {})",
            sql_lit(channel),
            sql_lit(op)
        ))
        .await
        .map(|_| ())
    }

    /// Claim up to `n` jobs. `FOR UPDATE SKIP LOCKED` is what lets
    /// replicas compete without a broker: a row another worker already
    /// holds is skipped rather than waited on, so throughput scales with
    /// workers instead of serializing on the queue head.
    pub async fn claim_jobs(&self, n: i64) -> Result<Vec<Job>, PoolError> {
        let rows = self
            .rows(&format!(
                // The rows are chosen in a MATERIALIZED CTE, which runs
                // once: as `IN (SELECT ... LIMIT n FOR UPDATE SKIP LOCKED)`
                // a nested-loop plan ran the choice again per scanned row
                // and claimed more than `n` (reported fix
                // `outbox-claimed-not-sent`, `mail::claim_sql`).
                "WITH \"next\" AS MATERIALIZED ( \
                   SELECT \"id\" FROM \"$jobs\" WHERE NOT \"claimed\" \
                   ORDER BY \"id\" LIMIT {n} FOR UPDATE SKIP LOCKED) \
                 UPDATE \"$jobs\" AS j SET \"claimed\" = true FROM \"next\" \
                 WHERE j.\"id\" = \"next\".\"id\" \
                 RETURNING j.\"id\", j.\"kind\", j.\"payload\""
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(Job {
                    id: r.first()?.clone()?.parse().ok()?,
                    kind: r.get(1)?.clone()?,
                    payload: r.get(2)?.clone()?,
                })
            })
            .collect())
    }

    /// Remove a finished job. Done in the worker's own transaction, so a
    /// worker that dies mid-job releases its claim by aborting and the job
    /// returns to the queue.
    pub async fn finish_job(&self, id: i64) -> Result<(), PoolError> {
        self.rows(&format!("DELETE FROM \"$jobs\" WHERE \"id\" = {id}"))
            .await
            .map(|_| ())
    }

    /// Claim a due schedule and advance it, in one step.
    ///
    /// This is the **fires-once** property, and it does not come from
    /// leadership: `SKIP LOCKED` means a second replica examining the same
    /// schedule at the same instant sees nothing to claim, and the
    /// `next_due` advance commits with the claim, so a crash after firing
    /// cannot fire again while a crash before it leaves the schedule due.
    pub async fn claim_due(&self, now: i64, period: i64) -> Result<Vec<String>, PoolError> {
        let rows = self
            .rows(&format!(
                "UPDATE \"$schedules\" SET \"next_due\" = {now} + {period}, \
                 \"fires\" = \"fires\" + 1 WHERE \"name\" IN ( \
                   SELECT \"name\" FROM \"$schedules\" WHERE \"next_due\" <= {now} \
                   FOR UPDATE SKIP LOCKED \
                 ) RETURNING \"name\""
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.first()?.clone())
            .collect())
    }

    /// Claim **one named** schedule if it is due, advancing it by **its
    /// own** period.
    ///
    /// [`claim_due`](Self::claim_due) claims every due schedule and
    /// advances all of them by the one period it was handed, which is right
    /// for a fleet of schedules that share a cadence and silently wrong for
    /// two that must not. `security-logging-design.md` §6 settles that
    /// `checkpoint` and `attest` are exactly such a pair — "attestation is
    /// not a checkpoint, and must not inherit its cadence" — so the two
    /// cannot go through one `claim_due` call without the loser adopting
    /// the winner's period. That is the ruling being broken by the job
    /// queue rather than by a misreading, which is the harder kind to
    /// notice.
    ///
    /// The fires-once mechanics are `claim_due`'s, unchanged: `FOR UPDATE
    /// SKIP LOCKED` so a second replica sees nothing to claim, and the
    /// `next_due` advance commits with the claim.
    pub async fn claim_schedule(
        &self,
        name: &str,
        now: i64,
        period: i64,
    ) -> Result<bool, PoolError> {
        let rows = self
            .rows(&format!(
                "UPDATE \"$schedules\" SET \"next_due\" = {now} + {period}, \
                 \"fires\" = \"fires\" + 1 WHERE \"name\" IN ( \
                   SELECT \"name\" FROM \"$schedules\" \
                   WHERE \"name\" = {} AND \"next_due\" <= {now} \
                   FOR UPDATE SKIP LOCKED \
                 ) RETURNING \"name\"",
                sql_lit(name)
            ))
            .await?;
        Ok(!rows.is_empty())
    }

    /// Register (or leave alone) a schedule.
    pub async fn schedule(&self, name: &str, next_due: i64) -> Result<(), PoolError> {
        self.rows(&format!(
            "INSERT INTO \"$schedules\" (\"name\", \"next_due\") VALUES ({}, {next_due}) \
             ON CONFLICT (\"name\") DO NOTHING",
            sql_lit(name)
        ))
        .await
        .map(|_| ())
    }

    /// Try to become the clock leader — **at-most-one-holder**, and
    /// nothing more. A session advisory lock is held for the life of this
    /// connection and released when it closes, so a dead leader's claim
    /// does not outlive its process.
    ///
    /// This is deliberately not the fires-once property: leadership can
    /// change between deciding a schedule is due and recording that it
    /// fired. `claim_due` is what makes firing exactly-once; leadership
    /// only keeps a hundred replicas from all trying at once.
    ///
    /// # A server does not call this. [`Leadership`] does the same thing
    ///
    /// **Ruled 2026-09-08:** a session-scoped Postgres lock is held on a
    /// connection the pool does not own. Every sentence above is true of the
    /// lock and none of them is true of a *pooled* connection's next life:
    /// [`TxConn::commit`] and [`TxConn::rollback`] push their client back
    /// onto the idle list, so a leader elected here leaves the lock on a
    /// connection that goes straight back to serving unrelated queries and
    /// holds it until that connection dies. Advisory locks are re-entrant
    /// per session, so a re-election that happens to borrow the same
    /// connection succeeds and one that borrows a different connection
    /// fails, permanently, with no error anywhere.
    ///
    /// So a long-running process takes leadership through [`Leadership`],
    /// which dials a connection of its own and releases the lock by dropping
    /// it. This primitive is kept because it is correct and
    /// `tests/jobs.rs`'s `leadership_bounds_who_tries_and_the_claim_delivers_once`
    /// exercises it: it is the *caller* that has to own the connection.
    pub async fn try_lead(&self, realm: i64) -> Result<bool, PoolError> {
        let rows = self
            .rows(&format!("SELECT pg_try_advisory_lock({realm})"))
            .await?;
        Ok(matches!(
            rows.first()
                .and_then(|r| r.first())
                .cloned()
                .flatten()
                .as_deref(),
            Some("t") | Some("true")
        ))
    }
}

// ── the LISTEN side ──────────────────────────────────────────────────────

/// The channel every write publishes staleness on. Named here rather than
/// spelled at each end, because a listener on the wrong channel is silence
/// that looks exactly like "nothing changed".
pub const INVALIDATE_CHANNEL: &str = "ordo.invalidate";

/// The channel a replica listens on to learn that **who may reach what**
/// has changed: a space was created, and its founding owner exists.
///
/// A third channel rather than a reuse of either of the other two, and the
/// distinction is the same one `registry::DEPLOY_CHANNEL`'s doc draws one
/// tier up. An invalidation says *your cached rows may be stale*; a deploy
/// says *your whole registry is*; this says *your access engine is*. They
/// are three different rebuilds with three different costs, and a
/// subscriber that conflated the last two would rebuild the door on every
/// membership change and reload every artifact with it.
///
/// **The payload is the space id**, which is an entity name and therefore
/// within what [`TxConn::notify`] permits a payload to be. A subscriber
/// does not act on the value — the rebuild reads the `members` table for
/// itself — so it is there for the log line an operator reads.
pub const MEMBERS_CHANNEL: &str = "ordo.members";

/// One delivered `NOTIFY`, as the listener saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub channel: String,
    /// What [`TxConn::notify`] passed as its second argument — an **entity
    /// name**, never a row and never a value. See `notify`'s doc for why
    /// the payload has to stay that thin.
    pub payload: String,
}

/// A live `LISTEN` session. Dropping it closes the connection and ends the
/// forwarding task; the receiver then sees the channel close.
///
/// # Why this cannot be a pooled connection
///
/// `LISTEN` is **session state**. A pooled connection goes back to the idle
/// list the moment a query finishes, so a `LISTEN` issued on one would
/// follow an unrelated request around the pool or vanish with a re-dial.
/// Worse, notifications arrive asynchronously *between* queries, and
/// [`crate::Pool`]'s driver task simply awaits the connection future — which
/// **discards** every `AsyncMessage` the server sends. There is nowhere in
/// the pool for a notification to be seen.
///
/// So the listener owns exactly one dedicated connection for the life of
/// the process and drives it with `poll_message`, which is the only way the
/// driver hands a notification out instead of dropping it on the floor.
pub struct Listener {
    /// Held solely to keep the session — and therefore the `LISTEN` — alive.
    /// Dropping this closes the socket, which ends the forwarding task.
    _client: tokio_postgres::Client,
}

/// Open a dedicated connection, `LISTEN` on every named channel, and
/// forward what arrives.
///
/// The forwarding channel is unbounded on purpose: a notification is a
/// staleness signal, a few bytes naming an entity, and the alternative — a
/// bounded channel that drops on a full buffer — turns a burst of writes
/// into silently stale subscribers, which is the exact failure this path
/// exists to remove. If the receiver is dropped, the task notices on its
/// next send and exits.
///
/// Reconnection is **not** handled here. If the connection dies the sender
/// drops, so `recv()` returns `None` and the caller learns that it has
/// stopped hearing. A bus that went quiet while still claiming to be live
/// would be worse than one that says it stopped.
pub async fn listen(
    cfg: &crate::PoolConfig,
    channels: &[&str],
) -> Result<(Listener, tokio::sync::mpsc::UnboundedReceiver<Notification>), PoolError> {
    let (client, connection) = cfg
        .pg()
        .connect(tokio_postgres::NoTls)
        .await
        .map_err(crate::driver_error)?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    // The driver has to be running before the client can issue anything, so
    // it is spawned first and the `LISTEN`s go out underneath it.
    tokio::spawn(async move {
        let mut connection = Box::pin(connection);
        loop {
            let msg = std::future::poll_fn(|cx| connection.as_mut().poll_message(cx)).await;
            match msg {
                Some(Ok(tokio_postgres::AsyncMessage::Notification(n))) => {
                    if tx
                        .send(Notification {
                            channel: n.channel().to_string(),
                            payload: n.payload().to_string(),
                        })
                        .is_err()
                    {
                        break; // nobody is listening any more
                    }
                }
                // Notices and future message kinds are not our business.
                Some(Ok(_)) => {}
                // A protocol error or a closed socket ends the session. The
                // sender drops with this task, so the receiver learns.
                Some(Err(_)) | None => break,
            }
        }
    });
    for ch in channels {
        // A channel name is an identifier to `LISTEN` and a string to
        // `pg_notify`, so `ordo.invalidate` has to be quoted here or the
        // dot would parse as a qualification.
        client
            .simple_query(&format!("LISTEN \"{}\"", ch.replace('"', "\"\"")))
            .await
            .map_err(crate::driver_error)?;
    }
    Ok((Listener { _client: client }, rx))
}

// ── leadership, on a connection of its own ───────────────────────────────

/// A session advisory lock held on a connection **the pool does not own**.
///
/// `TxConn::try_lead` takes `pg_try_advisory_lock`, which is *session*
/// state — and `TxConn::commit` pushes its client back onto the idle list.
/// So a leader elected through a pooled `TxConn` leaves the lock on a
/// connection that immediately goes back to serving unrelated queries, and
/// the lock is released only when that connection dies. Within one process
/// the effects compound in the confusing direction: advisory locks are
/// re-entrant per session, so a re-election on the same borrowed connection
/// succeeds while one that happens to borrow a different connection fails —
/// permanently, for the life of the process, with no error anywhere.
///
/// Ruled: **a session-scoped Postgres lock is held on a connection the pool
/// does not own.** This is that connection. Dropping this releases it,
/// which is why §1.5 drops it explicitly before the pool goes: a rolling
/// restart then hands leadership over in milliseconds rather than at TCP
/// timeout.
///
/// Leadership is **not load-bearing for correctness**: `claim_schedule`'s
/// `FOR UPDATE SKIP LOCKED` plus the committed `next_due` advance is what
/// makes firing exactly-once. Leadership only keeps a hundred replicas from
/// all trying at once. A supervisor whose leadership is broken is wasteful,
/// not wrong.
pub struct Leadership {
    client: tokio_postgres::Client,
}

/// The advisory-lock namespace for supervisor leadership. Distinct from the
/// chain-append and security-stream namespaces so two deployments sharing
/// one Postgres cluster do not contend for one another's.
const LEAD_NS: i32 = 0x005D_0555;

impl Leadership {
    /// Try to become leader for `realm`. `None` is "somebody else is".
    pub async fn acquire(
        cfg: &crate::PoolConfig,
        realm: &str,
    ) -> Result<Option<Leadership>, String> {
        let key = realm_key(realm);
        let (client, conn) = cfg
            .connect_config()
            .connect(tokio_postgres::NoTls)
            .await
            .map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let got: bool = client
            .query_one(
                &format!("SELECT pg_try_advisory_lock({LEAD_NS}, {key})"),
                &[],
            )
            .await
            .map_err(|e| e.to_string())?
            .get(0);
        Ok(got.then_some(Leadership { client }))
    }

    /// Whether the session that holds the lock is still there.
    ///
    /// A round trip on the lock's own connection, because the lock lives
    /// exactly as long as that session: a backend that was terminated, or a
    /// socket the server has already closed, is a lock this replica no
    /// longer holds, whatever it believes. `false` is final for this value;
    /// the caller drops it and tries [`Leadership::acquire`] again.
    ///
    /// Bounded by the caller. A half-open TCP connection can leave this
    /// waiting until the kernel gives up, so the supervisor wraps it in a
    /// timeout and treats the timeout as `false`.
    pub async fn is_held(&self) -> bool {
        !self.client.is_closed() && self.client.simple_query("SELECT 1").await.is_ok()
    }
}

/// The realm's advisory key, derived from the space the way
/// `space_lock_key` and `scope_lock_key` already derive theirs.
fn realm_key(realm: &str) -> i32 {
    let d = ordo_crypto::hmac_sha256(b"ordo.supervisor.realm.v1", realm.as_bytes());
    i32::from_be_bytes([d[0], d[1], d[2], d[3]])
}
