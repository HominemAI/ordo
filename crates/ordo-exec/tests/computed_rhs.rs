//! Checklist 17f — **the computed write RHS is atomic**, proved by racing it.
//!
//! DL §6 states the property as the whole point of the construct:
//! *"**RHS field references**: `{ n: n + 1 }` — computed server-side inside
//! the row's transaction; **atomic increments without read-modify-write
//! races**."*
//!
//! Nothing in the conformance suite can show that. Its `psql` path is one
//! process per statement, so there is no transaction to race and no lock to
//! hold; a broken implementation that read `used`, added one host-side and
//! wrote the sum back would pass every test there. The property only exists
//! on the pooled executor, where the whole protocol — preconditions, the
//! `FOR UPDATE`, the mutation — runs inside one transaction, and it is only
//! observable when two of them overlap.
//!
//! So this drives two genuinely concurrent `Executor::write_as` calls at the
//! same row and insists the counter lands at exactly `+2`. The failure it is
//! written to catch is the lost update: two callers both read `0`, both write
//! `1`, and the seat is sold twice while the counter says one.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_conformance::{Fixture, Pg, WriteLog, FIXTURE, SEED};
use ordo_exec::metrics::AppendObserver;
use ordo_exec::{ExecFail, Executor, Pool, PoolConfig, PoolError, PoolLimits};

/// The seat counter, DL §3's own `check` example, with `claimSeat` spelled
/// as DL §6 spells it.
const CLUSTER: &str = r#"
entity teams (key: team) {
  name:  text
  seats: int
  used:  int = 0
  check used <= seats
  check used >= 0
}

entity counters (key: ctr) {
  name: text unique
  n:    int = 0
}
"#;

const OPS: &str = r#"
op addTeam($name: text, $seats: int) =
  set teams { name: $name, seats: $seats }

op claimSeat($team: id) =
  set teams { id: $team, used: used + 1 }

op bump($k: text) =
  set counters { name: $k, n: n + 1 }
"#;

/// A log per concurrent caller, so these tests measure the row counter under
/// contention and nothing else. Sharing one log across writers used to
/// deadlock (17f-dl); `a_shared_write_log_serves_concurrent_writes_to_one_row`
/// is the test for that, and it shares one on purpose.
fn own_log() -> Arc<Mutex<WriteLog>> {
    Arc::new(Mutex::new(WriteLog::new()))
}

/// A fresh database, a seeded fixture, and an executor over a pool wide
/// enough for the racers plus the probe.
async fn boot(db: &str, size: usize) -> (Arc<Fixture>, Pg, Arc<Executor>, Arc<Mutex<WriteLog>>) {
    let (f, pg, x, log, _) = boot_with(db, size, PoolLimits::from_env()).await;
    (f, pg, x, log)
}

/// Counts the tries that lost a serialization race, retried or not.
#[derive(Default)]
struct Conflicts(AtomicU64);

impl Conflicts {
    fn seen(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl AppendObserver for Conflicts {
    fn lock_wait(&self, _: &str, _: f64) {}
    fn append_work(&self, _: &str, _: f64) {}
    fn write_conflict(&self, _: &str) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// [`boot`] with the pool's limits stated, and a count of lost races.
async fn boot_with(
    db: &str,
    size: usize,
    limits: PoolLimits,
) -> (
    Arc<Fixture>,
    Pg,
    Arc<Executor>,
    Arc<Mutex<WriteLog>>,
    Arc<Conflicts>,
) {
    let f = Fixture::from_source(&format!("{FIXTURE}\n{CLUSTER}\n{OPS}"), SEED)
        .expect("the seat cluster compiles");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");

    let conflicts = Arc::new(Conflicts::default());
    let pool = Pool::connect_with(
        PoolConfig {
            db: db.into(),
            size,
            acquire_timeout: Duration::from_secs(10),
            ..PoolConfig::from_env()
        },
        limits,
    )
    .await
    .expect("boot")
    .observing(conflicts.clone());
    let schema = f.schema.clone();
    let registry = f.registry.clone();
    let fixture = Arc::new(f);
    let x = Executor {
        pool,
        schema,
        registry,
        keys: fixture.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };
    (
        fixture,
        pg,
        Arc::new(x),
        Arc::new(Mutex::new(WriteLog::new())),
        conflicts,
    )
}

fn used(pg: &Pg, team: &str) -> i64 {
    pg.query(&format!(
        r#"SELECT "used" FROM "teams" WHERE "id" = '{team}'"#
    ))
    .expect("read back")[0][0]
        .as_deref()
        .expect("not null")
        .parse()
        .expect("an int")
}

/// **Two concurrent increments land as `+2`, and the third oversells.**
///
/// Eight seats, six claims fired at once from six transactions, and the
/// counter must read exactly six. A read-modify-write implementation loses
/// at least one of them under any interleaving that matters, and the whole
/// point of the seat pattern is that the number it keeps is the number of
/// seats actually taken.
///
/// The second half is the one that would matter in production: with the
/// counter at the cap, the next claim is refused by the CHECK rather than
/// by anyone's arithmetic. A lost update would have left room for it.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_increments_all_land_and_the_check_still_caps() {
    const CLAIMS: usize = 6;
    let (fixture, pg, x, log) = boot("ordo_rhs_race", CLAIMS + 2).await;

    let created = x
        .write_as(
            fixture.clone(),
            log.clone(),
            r#"addTeam($name: "race", $seats: 6)"#.into(),
            "mem_1".into(),
        )
        .await
        .expect("a team with six seats");
    let team = created[0]["created"].as_str().unwrap().to_string();

    // Six claims, launched together. They contend on ONE row, so they
    // serialize on its lock — which is exactly the mechanism under test:
    // concurrency is the row lock, never a host-side read.
    //
    // All six must land, so the claims fit the retry budget: a claim loses
    // a try when another claim commits first, so it loses at most five, and
    // the default `ORDO_WRITE_RETRIES=5` gives it six tries. The old budget
    // of five tries in all could run out here.
    let mut tasks = Vec::new();
    for _ in 0..CLAIMS {
        let (f, l, x2, t) = (fixture.clone(), own_log(), x.clone(), team.clone());
        tasks.push(tokio::spawn(async move {
            x2.write_as(f, l, format!(r#"claimSeat($team: "{t}")"#), "mem_1".into())
                .await
        }));
    }
    let mut landed = 0;
    for t in tasks {
        t.await.expect("no panic").expect("every claim fits");
        landed += 1;
    }
    assert_eq!(landed, CLAIMS, "all six calls returned success");
    assert_eq!(
        used(&pg, &team),
        CLAIMS as i64,
        "every increment landed — a lost update would leave this short"
    );

    // The cap is now real, and it is the DATABASE that says so.
    let e = x
        .write_as(
            fixture.clone(),
            log.clone(),
            format!(r#"claimSeat($team: "{team}")"#),
            "mem_1".into(),
        )
        .await
        .expect_err("the seventh claim oversells six seats");
    let msg = format!("{e}");
    assert!(
        msg.contains("check_violation") && msg.contains("teams$chk0$used<=seats"),
        "the oversell is a typed check violation naming its constraint: {msg}"
    );
    assert_eq!(
        used(&pg, &team),
        CLAIMS as i64,
        "the refused claim rolled back whole"
    );
}

/// The narrow form, spelled out: **two** overlapping transactions, held
/// open around each other, land as `+2` rather than `+1`.
///
/// The larger test above proves the same property with more racers, but a
/// reader looking for "two concurrent increments land as +2" should find
/// exactly that, and should be able to see the overlap is real: `b` starts
/// while `a` is still in flight because they are polled together.
#[tokio::test(flavor = "multi_thread")]
async fn two_overlapping_claims_land_as_plus_two() {
    let (fixture, pg, x, log) = boot("ordo_rhs_race2", 4).await;
    let team = x
        .write_as(
            fixture.clone(),
            log.clone(),
            r#"addTeam($name: "pair", $seats: 2)"#.into(),
            "mem_1".into(),
        )
        .await
        .expect("a team with two seats")[0]["created"]
        .as_str()
        .unwrap()
        .to_string();

    let call = |t: String| {
        let (f, l, x2) = (fixture.clone(), own_log(), x.clone());
        async move {
            x2.write_as(f, l, format!(r#"claimSeat($team: "{t}")"#), "mem_1".into())
                .await
        }
    };
    let (a, b) = tokio::join!(call(team.clone()), call(team.clone()));
    a.expect("first claim");
    b.expect("second claim");
    assert_eq!(used(&pg, &team), 2, "+1 and +1 is +2, not +1");

    // And the third is refused by the invariant, with nothing applied.
    let e = call(team.clone())
        .await
        .expect_err("two seats, three claims");
    assert!(format!("{e}").contains("check_violation"), "{e}");
    assert_eq!(used(&pg, &team), 2);
}

/// **Two overlapping bumps of a key no row holds yet land as `2`**, in one
/// row. `set counters { name: $k, n: n + 1 }` is one `INSERT … ON CONFLICT
/// DO UPDATE`: the create branch reads `n` as its default (owner,
/// 2026-10-31), and the racer that finds the key taken converges onto it
/// instead of creating a second row or writing `1` over the first.
#[tokio::test(flavor = "multi_thread")]
async fn two_overlapping_first_bumps_land_as_two_in_one_row() {
    let (fixture, pg, x, _log) = boot("ordo_rhs_upsert_race", 4).await;
    let call = || {
        let (f, l, x2) = (fixture.clone(), own_log(), x.clone());
        async move {
            x2.write_as(f, l, r#"bump($k: "hits")"#.into(), "mem_1".into())
                .await
        }
    };
    let (a, b) = tokio::join!(call(), call());
    a.expect("first bump");
    b.expect("second bump");
    let rows = pg
        .query(r#"SELECT "n" FROM "counters" WHERE "name" = 'hits'"#)
        .expect("read back");
    assert_eq!(rows.len(), 1, "converged, never a second row");
    assert_eq!(rows[0][0].as_deref(), Some("2"), "1 then +1, not 1 twice");
}

// ── hot rows (reported fix, coverage run 39197274294) ───────────────────
//
// Writers to one row at SERIALIZABLE lose races to each other: a writer
// blocked on the row lock gets `40001` when the holder commits. The executor
// retries a lost race `ORDO_WRITE_RETRIES` times with a random pause, and
// when every try loses, the write is refused as `serialization_conflict`,
// retryable, naming the setting. The SDK then sends it again (owner,
// 2026-10-19).
//
// So the promise under test is never "every write lands". It is: each write
// lands or is refused as a retryable conflict, never anything else; no
// update is lost; and the shared chain holds exactly the writes that landed.
// The old test asserted that all six landed, and failed with
// `RetryExhausted { attempts: 5 }` under coverage.

/// How one raced write ended.
enum Outcome {
    Landed,
    Conflict,
}

/// Read a write's answer, and fail the test on any answer the design does
/// not allow.
fn outcome(r: Result<serde_json::Value, ExecFail>) -> Outcome {
    match r {
        Ok(_) => Outcome::Landed,
        Err(ExecFail::Pool(e @ PoolError::RetryExhausted { .. })) => {
            let text = e.to_string();
            assert!(text.starts_with("serialization_conflict:"), "{text}");
            assert!(text.contains("ORDO_WRITE_RETRIES="), "{text}");
            Outcome::Conflict
        }
        Err(e) => panic!("a raced write is landed or a retryable conflict, never: {e}"),
    }
}

/// A team with room for every claim, and its id.
async fn team(
    x: &Executor,
    fixture: &Arc<Fixture>,
    log: &Arc<Mutex<WriteLog>>,
    name: &str,
) -> String {
    x.write_as(
        fixture.clone(),
        log.clone(),
        format!(r#"addTeam($name: "{name}", $seats: 999)"#),
        "mem_1".into(),
    )
    .await
    .expect("a team")[0]["created"]
        .as_str()
        .unwrap()
        .to_string()
}

/// One claim per entry of `teams`, all at once, through ONE shared log as
/// the door shares it. Returns how many landed on each team.
async fn race(
    x: &Arc<Executor>,
    fixture: &Arc<Fixture>,
    log: &Arc<Mutex<WriteLog>>,
    teams: &[String],
) -> Vec<usize> {
    let mut tasks = Vec::new();
    for (i, t) in teams.iter().enumerate() {
        let (f, l, x2, t) = (fixture.clone(), log.clone(), x.clone(), t.clone());
        tasks.push(tokio::spawn(async move {
            let r = x2
                .write_as(f, l, format!(r#"claimSeat($team: "{t}")"#), "mem_1".into())
                .await;
            (i, r)
        }));
    }
    let mut landed = vec![0; teams.len()];
    let all = async {
        for t in tasks {
            let (i, r) = t.await.expect("no panic");
            if let Outcome::Landed = outcome(r) {
                landed[i] += 1;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), all)
        .await
        .expect("every raced writer returns");
    landed
}

/// Sum per distinct team: `teams` may name one team many times.
fn per_team(teams: &[String], landed: &[usize]) -> std::collections::BTreeMap<String, usize> {
    let mut out = std::collections::BTreeMap::new();
    for (t, n) in teams.iter().zip(landed) {
        *out.entry(t.clone()).or_insert(0) += n;
    }
    out
}

/// **Six concurrent writes to ONE row, sharing one `Arc<Mutex<WriteLog>>`,
/// all return (17f-dl), and none is lost.**
///
/// This is what `ordo_host`'s space resolver hands every request in a space.
/// It used to deadlock: `Executor::attempt` held the log mutex inside its
/// `spawn_blocking` for the whole protocol, `SELECT … FOR UPDATE` included,
/// and took the same mutex on the async side where the continuation that
/// ends a transaction lives. Lock order was *log mutex → row lock* in one
/// place and the reverse in the other, and the blocked `lock()` stalled the
/// runtime's worker threads, so even this test's own timeout never fired.
///
/// Each attempt now runs against its own log cut by `WriteLog::attempt`, and
/// the shared log is locked only to cut it and to record a committed group.
/// The chain assertion is the other half of that: with one shared pending
/// slot, one writer's `drop_pending` could discard another's group.
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_write_log_serves_concurrent_writes_to_one_row() {
    let (fixture, pg, x, log) = boot("ordo_rhs_shared", 8).await;
    let t = team(&x, &fixture, &log, "shared").await;
    let before = log.lock().unwrap().len();

    let teams = vec![t.clone(); 6];
    let landed: usize = race(&x, &fixture, &log, &teams).await.iter().sum();

    assert!(landed >= 1, "one writer at least wins each round");
    assert_eq!(
        used(&pg, &t),
        landed as i64,
        "the counter is the writes that landed"
    );
    assert_eq!(
        log.lock().unwrap().len() - before,
        landed,
        "every committed write's group reached the shared chain, and nothing else did"
    );
}

/// **2, 6 and 20 writers on one row**: the same promise at each size. The
/// landed count is printed, so a run shows how hot the row was.
#[tokio::test(flavor = "multi_thread")]
async fn writers_on_one_hot_row_land_or_get_a_retryable_conflict() {
    let (fixture, pg, x, log, conflicts) =
        boot_with("ordo_rhs_sweep", 8, PoolLimits::from_env()).await;
    for writers in [2usize, 6, 20] {
        let t = team(&x, &fixture, &log, &format!("hot{writers}")).await;
        let before = log.lock().unwrap().len();
        let lost_before = conflicts.seen();
        let teams = vec![t.clone(); writers];
        let landed: usize = race(&x, &fixture, &log, &teams).await.iter().sum();
        eprintln!(
            "{writers} writers on one row: {landed} landed, {} lost races retried or refused",
            conflicts.seen() - lost_before
        );
        assert!(landed >= 1, "{writers}: one writer at least wins");
        assert_eq!(used(&pg, &t), landed as i64, "{writers}: no lost update");
        assert_eq!(
            log.lock().unwrap().len() - before,
            landed,
            "{writers}: the chain grew by the writes that landed"
        );
    }
}

/// **The control**: the same writers, each on its own row, lose no race
/// at all. Contention is the row, not the executor or the shared chain.
#[tokio::test(flavor = "multi_thread")]
async fn writers_on_distinct_rows_see_no_conflict() {
    let (fixture, pg, x, log, conflicts) =
        boot_with("ordo_rhs_control", 8, PoolLimits::from_env()).await;
    let mut teams = Vec::new();
    for i in 0..20 {
        teams.push(team(&x, &fixture, &log, &format!("cold{i}")).await);
    }
    let before = log.lock().unwrap().len();
    let lost_before = conflicts.seen();
    let landed = race(&x, &fixture, &log, &teams).await;
    assert_eq!(
        conflicts.seen() - lost_before,
        0,
        "no race lost on distinct rows"
    );
    for (t, n) in per_team(&teams, &landed) {
        assert_eq!(n, 1, "{t}");
        assert_eq!(used(&pg, &t), 1, "{t}");
    }
    assert_eq!(log.lock().unwrap().len() - before, teams.len());
}

/// Rows of `pg_stat_activity` in this database waiting on a lock.
async fn waiting(probe: &Pool) -> i64 {
    probe
        .rows(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .await
        .expect("activity")[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

async fn until(what: &str, mut ready: impl AsyncFnMut() -> bool) {
    let started = std::time::Instant::now();
    while !ready().await {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "waited for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// **`ORDO_WRITE_RETRIES=1` surfaces the conflict, and the refusal names
/// it.** Deterministic: two other sessions take turns updating the row, so
/// both of the write's tries block behind one of them and lose when it
/// commits.
///
/// 1. `h1` updates the row and holds it. The write's first try blocks.
/// 2. `h2` updates the row too, and queues behind the write.
/// 3. `h1` commits. The write's first try loses (its snapshot is older than
///    the row now), and `h2` takes the row.
/// 4. The write's second try blocks behind `h2`. `h2` commits, the second
///    try loses, and the write is refused: two tries, one retry.
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_budget_of_one_surfaces_the_conflict_and_names_the_setting() {
    let limits = PoolLimits {
        write_retries: 1,
        ..PoolLimits::from_env()
    };
    let (fixture, pg, x, log, conflicts) = boot_with("ordo_rhs_budget", 4, limits).await;
    let t = team(&x, &fixture, &log, "budget").await;
    let others = Pool::connect(PoolConfig {
        db: "ordo_rhs_budget".into(),
        size: 4,
        ..PoolConfig::from_env()
    })
    .await
    .expect("the other sessions");
    let touch = format!(r#"UPDATE "teams" SET "name" = "name" WHERE "id" = '{t}'"#);

    let h1 = others.begin().await.expect("h1");
    h1.rows(&touch).await.expect("h1 holds the row");
    let (f, l, x2, t2) = (fixture.clone(), log.clone(), x.clone(), t.clone());
    let write = tokio::spawn(async move {
        x2.write_as(f, l, format!(r#"claimSeat($team: "{t2}")"#), "mem_1".into())
            .await
    });
    until("the first try to block", async || {
        waiting(&others).await == 1
    })
    .await;

    let (go, wait_go) = tokio::sync::oneshot::channel::<()>();
    let (o2, touch2) = (others.clone(), touch.clone());
    let h2_has_it = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let has = h2_has_it.clone();
    let h2 = tokio::spawn(async move {
        let h2 = o2.begin().await.expect("h2");
        h2.rows(&touch2).await.expect("h2 takes the row after h1");
        has.store(true, Ordering::SeqCst);
        wait_go.await.expect("told to commit");
        h2.commit().await.expect("h2 commits");
    });
    until("h2 to queue", async || waiting(&others).await == 2).await;

    h1.commit().await.expect("h1 commits");
    until("the first try to lose", async || conflicts.seen() == 1).await;
    // With `h2` holding the row and not waiting, the one waiter is the
    // write's second try.
    until("the second try to block behind h2", async || {
        h2_has_it.load(Ordering::SeqCst) && waiting(&others).await == 1
    })
    .await;
    go.send(()).expect("h2 is waiting");
    h2.await.expect("h2 done");

    let e = write
        .await
        .expect("no panic")
        .expect_err("both tries lost, so the write is refused");
    assert!(
        matches!(
            e,
            ExecFail::Pool(PoolError::RetryExhausted { attempts: 2, .. })
        ),
        "two tries: {e}"
    );
    let text = e.to_string();
    assert!(text.starts_with("serialization_conflict:"), "{text}");
    assert!(text.contains("ORDO_WRITE_RETRIES=1"), "{text}");
    assert_eq!(conflicts.seen(), 2);
    assert_eq!(used(&pg, &t), 0, "nothing landed");
}
