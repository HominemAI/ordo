//! The mail outbox (`plan-auth-federated.md` step 4, §7.6), against real
//! Postgres: a send is a row, it survives a restart, it is sent once, and a
//! relay that is down delays it without losing it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`). Each
//! test uses its own database so the suites can run in parallel.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ordo_exec::{Pool, PoolConfig};
use ordo_host::mail::{
    self, Composer, Composers, MailAddress, MailError, MailJob, MailQueue, Mailer, Message,
    PgMailQueue, QueuedMail, SendPolicy,
};
use ordo_host::metrics::Metrics;

const NOW: u64 = 1_800_000_000;
const KIND: &str = "test.note";

async fn pool(stem: &str) -> Pool {
    let pg = ordo_conformance::Pg::scoped(stem);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    connect(&pg.db).await
}

async fn connect(db: &str) -> Pool {
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    mail::ensure_table(&pool).await.expect("outbox");
    pool
}

async fn rows(pool: &Pool) -> Vec<(String, String)> {
    pool.rows(r#"SELECT "attempts", "next_at" FROM "$mailoutbox" ORDER BY "id""#)
        .await
        .expect("read")
        .into_iter()
        .map(|r| (r[0].clone().unwrap(), r[1].clone().unwrap()))
        .collect()
}

fn job(to: &str, payload: &str) -> MailJob {
    MailJob::new(KIND, MailAddress::parse(to).unwrap(), payload).unwrap()
}

/// Composes the payload as the text; `None` for a payload of `nothing`.
struct Echo {
    calls: AtomicUsize,
}

impl Composer for Echo {
    fn compose(&self, job: &QueuedMail, _now: u64) -> Result<Option<Message>, MailError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if job.payload == "nothing" {
            return Ok(None);
        }
        Message::new(job.to.clone(), "A note", &job.payload).map(Some)
    }
}

/// A mailer that answers from a script, then accepts, and keeps the texts it
/// accepted.
#[derive(Default)]
struct Scripted {
    script: Mutex<Vec<MailError>>,
    sent: Mutex<Vec<String>>,
    /// The Message-ID of every submit, failed or accepted.
    ids: Mutex<Vec<Option<String>>>,
}

impl Mailer for Scripted {
    fn submit(&self, msg: &Message) -> Result<(), MailError> {
        self.ids
            .lock()
            .unwrap()
            .push(msg.message_id().map(str::to_string));
        if let Some(e) = self.script.lock().unwrap().pop() {
            return Err(e);
        }
        self.sent.lock().unwrap().push(msg.text().to_string());
        Ok(())
    }
    fn mail_domain(&self) -> String {
        "mail.example.org".into()
    }
    fn label(&self) -> String {
        "scripted".into()
    }
}

fn composers() -> (Arc<Composers>, Arc<Echo>) {
    let echo = Arc::new(Echo {
        calls: AtomicUsize::new(0),
    });
    let mut c = Composers::new();
    c.register(KIND, echo.clone()).unwrap();
    (Arc::new(c), echo)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_message_survives_a_restart_and_is_sent_once() {
    let db = {
        let first = pool("ordo_mail_outbox_restart").await;
        mail::enqueue(&first, &job("kim@example.org", "hello"), NOW)
            .await
            .expect("queued");
        // The first process ends here, before any worker ran.
        first.config().db
    };
    let second = connect(&db).await;
    let scripted = Arc::new(Scripted::default());
    let mailer: Arc<dyn Mailer> = scripted.clone();
    let (composers, _) = composers();
    let metrics = Metrics::new();
    let policy = SendPolicy::default();

    let pass = mail::run_pass(&second, &mailer, &composers, &policy, &metrics, NOW + 1)
        .await
        .expect("pass");
    assert_eq!(pass.sent, 1);
    let again = mail::run_pass(&second, &mailer, &composers, &policy, &metrics, NOW + 600)
        .await
        .expect("pass");
    assert_eq!(again.sent, 0, "a sent row is gone");
    assert_eq!(*scripted.sent.lock().unwrap(), vec!["hello".to_string()]);
    assert!(rows(&second).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_is_down_delays_the_message_and_does_not_lose_it() {
    let pool = pool("ordo_mail_outbox_down").await;
    mail::enqueue(&pool, &job("kim@example.org", "hello"), NOW)
        .await
        .unwrap();
    let scripted = Arc::new(Scripted::default());
    scripted
        .script
        .lock()
        .unwrap()
        .push(MailError::unavailable("connect_failed: refused"));
    let mailer: Arc<dyn Mailer> = scripted.clone();
    let (composers, echo) = composers();
    let metrics = Metrics::new();
    let policy = SendPolicy::default();

    let pass = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW)
        .await
        .unwrap();
    assert_eq!(pass.retrying, 1);
    assert_eq!(
        rows(&pool).await,
        vec![("1".to_string(), (NOW + 15).to_string())],
        "kept, and due again in 15 s"
    );
    let early = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW + 14)
        .await
        .unwrap();
    assert_eq!(early.sent + early.retrying, 0, "not before its time");
    let pass = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW + 15)
        .await
        .unwrap();
    assert_eq!(pass.sent, 1);
    assert_eq!(
        echo.calls.load(Ordering::SeqCst),
        2,
        "composed at each attempt"
    );
    assert!(rows(&pool).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rate_limited_relay_keeps_the_message_until_the_wait_it_named() {
    let pool = pool("ordo_mail_outbox_ratelimit").await;
    mail::enqueue(&pool, &job("kim@example.org", "hello"), NOW)
        .await
        .unwrap();
    let scripted = Arc::new(Scripted::default());
    scripted.script.lock().unwrap().push(MailError::deferred(
        "rejected: destination answered 429",
        120,
    ));
    let mailer: Arc<dyn Mailer> = scripted.clone();
    let (composers, _) = composers();
    let metrics = Metrics::new();
    let policy = SendPolicy::default();

    let pass = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW)
        .await
        .unwrap();
    assert_eq!((pass.retrying, pass.dropped), (1, 0), "kept, not dropped");
    assert_eq!(
        rows(&pool).await,
        vec![("1".to_string(), (NOW + 120).to_string())],
        "due again after the relay's 120 s, not the 15 s backoff"
    );
    let early = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW + 119)
        .await
        .unwrap();
    assert_eq!(early.sent + early.retrying, 0, "not before the wait");
    let pass = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW + 120)
        .await
        .unwrap();
    assert_eq!(pass.sent, 1);
    assert!(rows(&pool).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_or_the_last_attempt_drops_the_message_and_nothing_owed_sends_nothing() {
    let pool = pool("ordo_mail_outbox_drop").await;
    let (composers, _) = composers();
    let metrics = Metrics::new();
    let policy = SendPolicy {
        max_attempts: 2,
        ..SendPolicy::default()
    };

    // Nothing owed: removed, never handed to the mailer.
    mail::enqueue(&pool, &job("kim@example.org", "nothing"), NOW)
        .await
        .unwrap();
    let scripted = Arc::new(Scripted::default());
    let mailer: Arc<dyn Mailer> = scripted.clone();
    let pass = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW)
        .await
        .unwrap();
    assert_eq!(pass.skipped, 1);
    assert!(scripted.sent.lock().unwrap().is_empty());
    assert!(rows(&pool).await.is_empty());

    // A refusal is final.
    mail::enqueue(&pool, &job("kim@example.org", "hello"), NOW)
        .await
        .unwrap();
    scripted
        .script
        .lock()
        .unwrap()
        .push(MailError::refused("relay_refused: 550"));
    let pass = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW)
        .await
        .unwrap();
    assert_eq!(pass.dropped, 1);
    assert!(rows(&pool).await.is_empty());

    // A failure that may pass is tried `max_attempts` times, then dropped.
    mail::enqueue(&pool, &job("kim@example.org", "hello"), NOW)
        .await
        .unwrap();
    for _ in 0..2 {
        scripted
            .script
            .lock()
            .unwrap()
            .push(MailError::unavailable("timed_out"));
    }
    let first = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW)
        .await
        .unwrap();
    assert_eq!(first.retrying, 1);
    let second = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW + 15)
        .await
        .unwrap();
    assert_eq!(second.dropped, 1);
    assert!(rows(&pool).await.is_empty());
    assert!(scripted.sent.lock().unwrap().is_empty());

    // A kind nobody composes is dropped, not left to block the queue.
    let mut other = Composers::new();
    other
        .register(
            "other",
            Arc::new(Echo {
                calls: AtomicUsize::new(0),
            }),
        )
        .unwrap();
    mail::enqueue(&pool, &job("kim@example.org", "hello"), NOW)
        .await
        .unwrap();
    let pass = mail::run_pass(&pool, &mailer, &Arc::new(other), &policy, &metrics, NOW)
        .await
        .unwrap();
    assert_eq!(pass.dropped, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_replicas_draining_one_outbox_send_each_row_once() {
    let a = pool("ordo_mail_outbox_replicas").await;
    let b = connect(&a.config().db).await;
    for i in 0..20 {
        mail::enqueue(&a, &job("kim@example.org", &format!("note {i}")), NOW)
            .await
            .unwrap();
    }
    let scripted = Arc::new(Scripted::default());
    let mailer: Arc<dyn Mailer> = scripted.clone();
    let (composers, _) = composers();
    let metrics = Arc::new(Metrics::new());
    let policy = SendPolicy {
        batch: 4,
        ..SendPolicy::default()
    };
    let drain = |pool: Pool| {
        let (mailer, composers, metrics, policy) = (
            mailer.clone(),
            composers.clone(),
            metrics.clone(),
            policy.clone(),
        );
        tokio::spawn(async move {
            let mut sent = 0;
            loop {
                let p = mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW)
                    .await
                    .unwrap();
                sent += p.sent;
                if p.sent + p.retrying + p.dropped + p.skipped == 0 {
                    return sent;
                }
            }
        })
    };
    let (x, y) = tokio::join!(drain(a.clone()), drain(b));
    assert_eq!(x.unwrap() + y.unwrap(), 20);
    let mut sent = scripted.sent.lock().unwrap().clone();
    sent.sort();
    sent.dedup();
    assert_eq!(sent.len(), 20, "no row sent twice");
    assert!(rows(&a).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn synchronous_code_queues_with_one_insert() {
    let pool = pool("ordo_mail_outbox_sync").await;
    let q = {
        let pool = pool.clone();
        tokio::task::spawn_blocking(move || PgMailQueue::open_on(pool))
            .await
            .unwrap()
            .expect("outbox")
    };
    tokio::task::spawn_blocking(move || q.enqueue(&job("kim@example.org", "ceremony-handle"), NOW))
        .await
        .unwrap()
        .expect("queued");
    let got = pool
        .rows(r#"SELECT "kind", "recipient", "payload", "attempts" FROM "$mailoutbox""#)
        .await
        .unwrap();
    assert_eq!(
        got,
        vec![vec![
            Some(KIND.to_string()),
            Some("kim@example.org".to_string()),
            Some("ceremony-handle".to_string()),
            Some("0".to_string()),
        ]]
    );
}

/// A relay that takes `delay` to accept each message, and keeps the texts
/// it accepted.
struct Slow {
    delay: std::time::Duration,
    sent: Mutex<Vec<String>>,
}

impl Mailer for Slow {
    fn submit(&self, msg: &Message) -> Result<(), MailError> {
        std::thread::sleep(self.delay);
        self.sent.lock().unwrap().push(msg.text().to_string());
        Ok(())
    }
    fn label(&self) -> String {
        "slow".into()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_shorter_than_a_batch_still_sends_each_row_once() {
    // A batch of 10 at 300 ms a message takes 3 s; the lease is 2 s. A
    // lease taken for the whole batch at once would expire under the tail,
    // and the other replica would send it again. Each row is leased when
    // its own send starts, so no lease outlives the send it covers.
    let a = pool("ordo_mail_outbox_short_lease").await;
    let b = connect(&a.config().db).await;
    for i in 0..10 {
        mail::enqueue(&a, &job("kim@example.org", &format!("note {i}")), NOW)
            .await
            .unwrap();
    }
    let slow = Arc::new(Slow {
        delay: std::time::Duration::from_millis(300),
        sent: Mutex::default(),
    });
    let mailer: Arc<dyn Mailer> = slow.clone();
    let (composers, _) = composers();
    let metrics = Arc::new(Metrics::new());
    let policy = SendPolicy {
        batch: 10,
        lease_secs: 2,
        ..SendPolicy::default()
    };
    let start = std::time::Instant::now();
    let drain = |pool: Pool, head_start: u64| {
        let (mailer, composers, metrics, policy) = (
            mailer.clone(),
            composers.clone(),
            metrics.clone(),
            policy.clone(),
        );
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(head_start)).await;
            // Both replicas read one clock: the test's start plus real time.
            while start.elapsed() < std::time::Duration::from_secs(8) {
                let now = NOW + start.elapsed().as_secs();
                mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, now)
                    .await
                    .unwrap();
                if rows(&pool).await.is_empty() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
    };
    let (x, y) = tokio::join!(drain(a.clone(), 0), drain(b, 100));
    x.unwrap();
    y.unwrap();
    let sent = slow.sent.lock().unwrap().clone();
    let mut unique = sent.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 10, "every row sent: {sent:?}");
    assert_eq!(sent.len(), 10, "no row sent twice: {sent:?}");
    assert!(rows(&a).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_retried_row_is_sent_with_the_same_message_id() {
    let pool = pool("ordo_mail_outbox_msgid").await;
    mail::enqueue(&pool, &job("kim@example.org", "hello"), NOW)
        .await
        .unwrap();
    mail::enqueue(&pool, &job("lee@example.org", "other"), NOW)
        .await
        .unwrap();
    let scripted = Arc::new(Scripted::default());
    scripted
        .script
        .lock()
        .unwrap()
        .push(MailError::unavailable("connect_failed: refused"));
    let mailer: Arc<dyn Mailer> = scripted.clone();
    let (composers, _) = composers();
    let metrics = Metrics::new();
    let policy = SendPolicy::default();

    // First row fails once; the second is sent in the same pass.
    mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW)
        .await
        .unwrap();
    mail::run_pass(&pool, &mailer, &composers, &policy, &metrics, NOW + 15)
        .await
        .unwrap();
    let ids = scripted.ids.lock().unwrap().clone();
    assert_eq!(ids.len(), 3, "two tries of row 1, one of row 2");
    let first = ids[0].clone().expect("an id on the first try");
    assert_eq!(ids[2].as_ref(), Some(&first), "the retry keeps the id");
    assert!(first.starts_with("<ordo-1-") && first.ends_with("@mail.example.org>"));
    assert_ne!(ids[1].as_ref(), Some(&first), "another row, another id");
}

/// Reported fix `outbox-claimed-not-sent`: one claim leases one row, even
/// when Postgres plans the claim as a nested loop that runs its row choice
/// again for each row it scans.
///
/// The stress test saw begin answer 200 and the code never reach the relay
/// for 20 s and more: the outbox rows were leased (`attempts` 1, `next_at`
/// two minutes out) and nothing was sending them. The claim leased several
/// rows at once and the pass sent only the first. The planner's choice
/// depends on the table's statistics, so this pins the bad plan with the
/// planner's own switches and asks the claim to take exactly one row.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_leases_one_row_whatever_the_plan() {
    let pool = pool("mail_claim_one").await;
    for to in ["a@example.org", "b@example.org", "c@example.org"] {
        mail::enqueue(&pool, &job(to, "p"), NOW)
            .await
            .expect("queued");
    }
    // A pool of its own, so the planner switches die with it.
    let session = Pool::connect(PoolConfig {
        db: pool.config().db,
        size: 1,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let claim = mail::claim_sql(NOW, NOW + 120);
    let claimed = session
        .rows_in_session(&[
            "SET enable_hashjoin = off",
            "SET enable_mergejoin = off",
            "SET enable_hashagg = off",
            "SET enable_material = off",
            "SET enable_sort = off",
            &claim,
        ])
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1, "{claimed:?}");
    let leased: Vec<_> = rows(&pool)
        .await
        .into_iter()
        .filter(|(attempts, _)| attempts == "1")
        .collect();
    assert_eq!(
        leased.len(),
        1,
        "one claim leased {} rows; the others would sit unsent until the lease ends",
        leased.len()
    );
}
