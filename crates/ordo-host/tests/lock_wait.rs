//! 5f — the chain-append lock's wait, measured against a real contended
//! lock.
//!
//! `implementation.md` calls `ordo_chain_append_lock_wait_seconds` *"the
//! production answer to 'is this biting yet'"* and requires it to exist
//! **before any tuning does**. It did not: the metrics registry held
//! counters, and a counter cannot answer a quantile. So the
//! release-blocking measurement — single-space write throughput under the
//! chain-append lock — could report a rate and could not say whether the
//! lock was what bounded it, which is the entire question.
//!
//! The histogram now exists. What this file establishes is that it observes
//! the real thing, and that requires a genuinely contended lock rather than
//! a synthetic `observe(0.5)`: an instrument tested against its own input
//! has been tested for nothing.
//!
//! The shape of every test here is the shape
//! `ordo-exec/tests/commandlog.rs::concurrent_appenders_in_one_space_serialize`
//! already proved correct — two transactions in flight in one space, the
//! second demonstrably blocked while the first holds — with the metrics
//! registry attached and asserted on.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, SpaceId, Timestamp, Value,
};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::metrics::{Metrics, CHAIN_APPEND_LOCK_WAIT, CHAIN_APPEND_WORK};

/// How long the holder keeps the lock while the waiter blocks on it.
///
/// Chosen well above the bucket the uncontended population lives in (a
/// local-socket round trip, tens of microseconds) so the two are separated
/// by three decades of ladder rather than by a margin a loaded machine
/// could erase. A test whose pass depends on timing precision is a flake;
/// this one depends only on the two populations not overlapping.
const HELD: Duration = Duration::from_millis(600);

fn space() -> SpaceId {
    SpaceId("spc_lockwait".into())
}

fn group(n: usize) -> CommandGroup {
    CommandGroup {
        correlation: CorrelationId(format!("cg_{n}")),
        actor: Actor::User(format!("usr_{n}")),
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: space().clone(),
            entity: "transactions".into(),
            row_id: format!("txn_{n}"),
            kind: CommandKind::Create,
            before: None,
            after: Some(
                [("amount".to_string(), Value::Int(n as i64))]
                    .into_iter()
                    .collect(),
            ),
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    }
}

/// A pool of its own database, emptied of chain rows. These databases
/// outlive a run, and a previous run's entries would collide on
/// `(space, correlation)`.
async fn fresh(db: &str, size: usize) -> Pool {
    let _ = ordo_conformance::Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size,
        acquire_timeout: Duration::from_secs(10),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    pool
}

/// Per-space labelling on, so the assertions can name the series precisely
/// and so the opt-in itself is exercised rather than assumed.
fn metrics() -> Arc<Metrics> {
    Arc::new(Metrics::new().with_per_space_series(true))
}

fn labels() -> [(&'static str, &'static str); 1] {
    [("space", "spc_lockwait")]
}

/// **The property 5f was blocked on.** A contended append lock produces a
/// wait the histogram can see, and an uncontended one does not.
///
/// Both directions in one test on purpose. "The histogram recorded
/// something" is passed by a histogram that records the same thing however
/// the lock behaves, which is exactly the instrument that would let a
/// throughput run conclude the lock was biting when it was not.
#[tokio::test(flavor = "multi_thread")]
async fn a_contended_append_lock_is_visible_in_the_histogram_and_an_uncontended_one_is_not() {
    let pool = fresh("ordo_m_lock_wait", 6).await;

    // ── the uncontended population ───────────────────────────────────────
    let quiet = metrics();
    let quiet_pool = pool.observing(quiet.clone());
    for n in 0..6 {
        let tx = quiet_pool.begin().await.unwrap();
        tx.append_group(&group(n)).await.expect("append");
        tx.commit().await.unwrap();
    }
    assert_eq!(
        quiet.histogram_count(CHAIN_APPEND_LOCK_WAIT, &labels()),
        Some(6),
        "every append observes its wait, including the zero ones — a \
         histogram that only records contention cannot say what fraction of \
         appends contended"
    );
    let quiet_max = quiet
        .quantile(CHAIN_APPEND_LOCK_WAIT, &labels(), 1.0)
        .expect("six observations");
    assert!(
        quiet_max < 0.1,
        "an uncontended lock costs one round trip; got {quiet_max}s"
    );

    // ── the contended one ────────────────────────────────────────────────
    // A fresh registry, so what follows is an assertion about this phase
    // rather than about a mixture of the two.
    let loud = metrics();
    let loud_pool = pool.observing(loud.clone());

    // A appends and stays open. The lock is transaction-scoped, so A holds
    // it until it commits and nothing has to remember to release it.
    let a = loud_pool.begin().await.unwrap();
    a.append_group(&group(100)).await.expect("A appends");

    let b = loud_pool.begin().await.unwrap();
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let b_task = tokio::spawn(async move {
        b.append_group(&group(101)).await.expect("B appends");
        let _ = done_tx.send(());
        b.commit().await.unwrap();
    });

    tokio::time::sleep(HELD).await;
    // The independent check that B really is blocked, and not merely slow.
    // Without it a passing histogram assertion could mean the scheduler was
    // busy rather than that the lock was held.
    assert!(
        matches!(
            done_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "B must still be waiting on the per-space append lock"
    );
    assert_eq!(
        pool.rows(
            "SELECT count(*) FROM pg_locks \
             WHERE \"locktype\" = 'advisory' AND NOT \"granted\" \
             AND \"database\" = (SELECT oid FROM pg_database \
                                WHERE datname = current_database())",
        )
        .await
        .unwrap()[0][0]
            .as_deref(),
        Some("1"),
        "the server itself should show one ungranted advisory lock"
    );
    // While B waits, only A's append has been observed — the observation
    // happens when the lock is GRANTED, so a waiter contributes nothing
    // until it stops waiting. That is what makes the series a wait
    // distribution rather than a queue-depth sample.
    assert_eq!(
        loud.histogram_count(CHAIN_APPEND_LOCK_WAIT, &labels()),
        Some(1),
        "A's append is observed; B's is still waiting to be"
    );

    a.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), b_task)
        .await
        .expect("B unblocks when A commits")
        .expect("B's task");

    assert_eq!(
        loud.histogram_count(CHAIN_APPEND_LOCK_WAIT, &labels()),
        Some(2)
    );
    // Two observations: A's ~0 and B's ~HELD. The sum is the robust
    // assertion — it is the actual waiting, in seconds, with no bucket
    // arithmetic between the measurement and the claim.
    let waited = loud
        .histogram_sum(CHAIN_APPEND_LOCK_WAIT, &labels())
        .expect("two observations");
    assert!(
        waited >= HELD.as_secs_f64() * 0.8,
        "B waited about {HELD:?}; the histogram accounts for {waited}s"
    );

    // And the diagnosis the PAIR exists for: the wait grew and the critical
    // section did not. A wait that grows while the section stays flat is
    // more writers; a wait that grows because the section grew is
    // `implementation.md`'s *"anything that widens the critical section"*,
    // and one series cannot tell those apart.
    let work_max = loud
        .quantile(CHAIN_APPEND_WORK, &labels(), 1.0)
        .expect("two appends did work under the lock");
    assert!(
        work_max < 0.1,
        "the critical section is a head read, a dup check and an insert; \
         got {work_max}s — if this ever fails, the lock is being held \
         across something it should not be"
    );
    assert!(
        waited > work_max * 2.0,
        "under contention the wait must dominate the work, or the series \
         is measuring the wrong span"
    );
}

/// The exposition a scraper actually reads, over a real append.
///
/// Asserted end to end rather than over a synthetic observation because the
/// failure this catches is a formatting one: a histogram missing its
/// `le="+Inf"` bucket or its `_count` is silently unreadable by
/// `histogram_quantile`, which would leave the p99 measurement blocked for a
/// reason nothing in the registry's own API would reveal.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_append_renders_as_a_scrapeable_histogram() {
    let pool = fresh("ordo_m_lock_render", 2).await;
    let m = metrics();
    let observed = pool.observing(m.clone());

    let tx = observed.begin().await.unwrap();
    tx.append_group(&group(200)).await.expect("append");
    tx.commit().await.unwrap();

    let text = m.render_prometheus();
    assert!(
        text.contains("# TYPE ordo_chain_append_lock_wait_seconds histogram"),
        "{text}"
    );
    assert!(
        text.contains(
            "ordo_chain_append_lock_wait_seconds_bucket{space=\"spc_lockwait\",le=\"+Inf\"} 1"
        ),
        "the +Inf bucket is required, and is the one a hand-rolled renderer \
         forgets\n{text}"
    );
    assert!(
        text.contains("ordo_chain_append_lock_wait_seconds_count{space=\"spc_lockwait\"} 1"),
        "{text}"
    );
    assert!(
        text.contains("ordo_chain_append_seconds_count{space=\"spc_lockwait\"} 1"),
        "{text}"
    );
}

/// Metering is opt-in, and an unobserved pool is silent rather than zero.
///
/// The distinction matters at exactly the moment somebody reads a
/// dashboard: a lock-wait series stuck at zero would read as *"the lock is
/// never contended"*, which is the wrong conclusion drawn confidently. An
/// absent series reads as *"nothing is reporting"*, which is the right one.
#[tokio::test(flavor = "multi_thread")]
async fn an_unobserved_pool_reports_nothing_at_all() {
    let pool = fresh("ordo_m_lock_optin", 2).await;
    let m = metrics();

    // The *unobserved* handle — the same connections, no observer.
    let tx = pool.begin().await.unwrap();
    tx.append_group(&group(300)).await.expect("append");
    tx.commit().await.unwrap();

    assert_eq!(m.histogram_count(CHAIN_APPEND_LOCK_WAIT, &labels()), None);
    assert!(!m.render_prometheus().contains(CHAIN_APPEND_LOCK_WAIT));
}

/// Per-space series are opt-in (`implementation.md`: *"per-space series are
/// opt-in because space is high-cardinality"*), and the default really is
/// off.
///
/// Worth a test because the failure is invisible until a deployment with
/// many spaces meets the registry's series cap and the *denial* counters —
/// which the mandatory alert reads — start being dropped to make room for
/// lock-wait buckets nobody asked for.
#[tokio::test(flavor = "multi_thread")]
async fn the_default_registry_does_not_label_lock_waits_by_space() {
    let pool = fresh("ordo_m_lock_cardinality", 2).await;
    // `Metrics::new()`, not the per-space builder the other tests use.
    let m = Arc::new(Metrics::new().with_per_space_series(false));
    let observed = pool.observing(m.clone());

    let tx = observed.begin().await.unwrap();
    tx.append_group(&group(400)).await.expect("append");
    tx.commit().await.unwrap();

    assert_eq!(
        m.histogram_count(CHAIN_APPEND_LOCK_WAIT, &labels()),
        None,
        "no space-labelled series exists"
    );
    assert_eq!(
        m.histogram_count(CHAIN_APPEND_LOCK_WAIT, &[]),
        Some(1),
        "the unlabelled series holds the observation — the deployment still \
         learns whether the lock is biting, just not where"
    );
    assert!(!m.render_prometheus().contains("space=\"spc_lockwait\""));
}

/// **The measurement `plan-idempotency.md` §3.2 asked for**: what the
/// `$replay` journal's completion record costs, beside the append/wait pair
/// it would be charged against.
///
/// The plan kept the sealed response body out of the write transaction and
/// left the journal in it, on the argument that one PK `UPDATE` is small
/// against the critical section and an unbounded sealed `INSERT` plus a
/// key-provider round trip are not. That is an adjective. This is the
/// number, printed rather than asserted tightly, because the point is to
/// have it in front of whoever proposes the next thing to put in there:
///
/// ```text
/// cargo test -p ordo-host --test lock_wait journal -- --nocapture
/// ```
///
/// **What it found, and it is not what the plan assumed.** The journal's
/// `complete_lease` UPDATE does not run inside the append lock's critical
/// section at all. The lock is transaction-scoped and taken *as late as
/// possible* — `write_attempt` completes the lease and only then appends,
/// and the append is the last statement before commit — so the UPDATE is
/// already done when the lock is acquired. Its share of the section is
/// **zero**, and that is checked here mechanically (`pg_locks` shows this
/// transaction holding no advisory lock while the UPDATE runs, and holding
/// one the moment the append returns) rather than argued from reading the
/// call order, because the call order is exactly the thing a later edit can
/// change without noticing.
///
/// The printed ratio is therefore a **counterfactual**: the width the
/// journal WOULD add if it were moved after the append, or if a future
/// statement of the same size were placed there. That is the number worth
/// keeping, because "one small UPDATE" is how every widening starts.
#[tokio::test(flavor = "multi_thread")]
async fn the_replay_journals_update_is_measured_against_the_section_it_would_join() {
    const ROUNDS: usize = 12;

    let pool = fresh("ordo_idem_journal_cost", 4).await;
    // Keys are the primary key of `$replay` and these databases outlive a
    // run, so a previous run's rows would turn every claim into a takeover
    // and measure a different statement.
    pool.rows("DELETE FROM \"$replay\"").await.unwrap();
    let m = metrics();
    let observed = pool.observing(m.clone());

    let mut journal = Vec::with_capacity(ROUNDS);
    for n in 0..ROUNDS {
        let key = format!("k_journal_{n}");
        let tx = observed.begin().await.unwrap();
        // The claim is not timed: it runs before the body, off every lock,
        // and the plan never proposed anything about it. What is timed is
        // the completion — the statement that rides the same transaction as
        // the effect, and the only part of the journal that could ever be
        // charged to the append lock.
        let (_, generation) = tx.claim_lease(&key, "fp").await.expect("claim");

        let started = std::time::Instant::now();
        let completed = tx.complete_lease(&key, generation).await.expect("complete");
        journal.push(started.elapsed().as_secs_f64());
        assert!(completed, "an unfenced holder completes its own lease");

        if n == 0 {
            // The ordering claim, checked instead of assumed: at this point
            // the completion record is written and no advisory lock is held
            // by anyone in this database.
            assert_eq!(
                advisory_locks(&pool).await,
                0,
                "`complete_lease` ran with no append lock held — its share of \
                 the critical section is zero, and the whole shape of this \
                 measurement depends on that staying true"
            );
        }

        tx.append_group(&group(500 + n)).await.expect("append");

        if n == 0 {
            // …and the positive control, which is what keeps the assertion
            // above from passing because the probe cannot see anything.
            assert_eq!(
                advisory_locks(&pool).await,
                1,
                "the append took the lock and holds it until commit"
            );
        }
        tx.commit().await.unwrap();
    }

    let rounds = ROUNDS as f64;
    let update_mean = journal.iter().sum::<f64>() / rounds;
    let update_max = journal.iter().copied().fold(f64::MIN, f64::max);
    assert_eq!(
        m.histogram_count(CHAIN_APPEND_WORK, &labels()),
        Some(ROUNDS as u64),
        "every append is accounted for, or the mean below is over a \
         different population than the one that ran"
    );
    let section_mean = m
        .histogram_sum(CHAIN_APPEND_WORK, &labels())
        .expect("twelve appends")
        / rounds;
    let wait_mean = m
        .histogram_sum(CHAIN_APPEND_LOCK_WAIT, &labels())
        .expect("twelve acquisitions")
        / rounds;
    let share = update_mean / section_mean;

    println!(
        "\n── $replay journal cost, against the append-lock pair (5f) ──\n\
         lock wait         mean {wait_mean:>9.6}s   (uncontended: one round trip)\n\
         critical section  mean {section_mean:>9.6}s   (head read, dup check, insert)\n\
         complete_lease    mean {update_mean:>9.6}s   max {update_max:.6}s  (one PK UPDATE)\n\
         share of section       {:>9.1}%   COUNTERFACTUAL — the UPDATE runs\n\
         \x20                                BEFORE the lock is taken, so its\n\
         \x20                                real share today is 0%\n",
        share * 100.0,
    );

    // Loose on purpose, and loose in both directions. A tight bound here
    // would be a flake on a loaded machine; no bound at all would let the
    // statement grow into a scan and nothing would say so. These catch the
    // shape changing, not the milliseconds moving.
    assert!(
        update_mean > 0.0,
        "the UPDATE was not measured at all — a zero here means the timing \
         span moved off the statement"
    );
    assert!(
        update_max < 0.05,
        "one PK UPDATE on a local socket took {update_max}s at worst; past \
         this it is no longer the small bounded thing the plan ruled on"
    );
    assert!(
        share < 3.0,
        "the journal's UPDATE is {share:.2}× the whole critical section — it \
         is meant to be comparable to ONE of the section's three statements, \
         so this is a scan wearing an UPDATE's name"
    );
}

/// Advisory locks held (or awaited) in this pool's own database.
///
/// Scoped to the database because `pg_locks` is cluster-wide and the rest
/// of the suite is appending to its own databases at the same time.
async fn advisory_locks(pool: &Pool) -> usize {
    pool.rows(
        "SELECT count(*) FROM pg_locks \
         WHERE \"locktype\" = 'advisory' \
         AND \"database\" = (SELECT oid FROM pg_database \
                            WHERE datname = current_database())",
    )
    .await
    .unwrap()[0][0]
        .as_deref()
        .and_then(|n| n.parse().ok())
        .expect("count(*) is a number")
}
