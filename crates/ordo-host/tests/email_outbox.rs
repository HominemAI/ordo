//! Emailed codes against real Postgres (`plan-auth-federated.md` step 5):
//! the ceremony in `$authceremony`, the counters in `$authrate`, the message
//! queued in `$mailoutbox` and sent by the real `mail.send` pass with the
//! composer `boot_composers` registers.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`). Each
//! test uses its own database.

use std::sync::{Arc, Mutex};

use ordo_exec::{Pool, PoolConfig};
use ordo_host::email_code::{
    self, EmailCodePolicy, EmailCodeProvider, EmailCodeStore, PgEmailCodeStore,
};
use ordo_host::mail::{self, MailError, MailQueue, Mailer, Message, PgMailQueue, SendPolicy};
use ordo_host::metrics::Metrics;
use ordo_identity::directory::Provisioning;
use ordo_identity::{
    AuthError, AuthProvider, AuthenticatorId, ChallengeId, CompleteRequest, MemoryDirectoryStore,
    Timestamp,
};

const SECRET: &[u8] = b"email-outbox-test-secret";
/// The start of an hour, so a test's messages land in one window.
const NOW: u64 = 1_800_000_000;
const KIM: &str = "kim@example.org";

async fn pool(stem: &str) -> Pool {
    let pg = ordo_conformance::Pg::scoped(stem);
    let f = ordo_conformance::Fixture::load().expect("fixture");
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    ordo_host::ceremonies::ensure_table(&pool)
        .await
        .expect("ceremonies");
    mail::ensure_table(&pool).await.expect("outbox");
    email_code::ensure_table(&pool).await.expect("rates");
    pool
}

/// Keeps the texts it accepted.
#[derive(Default)]
struct Inbox {
    sent: Mutex<Vec<(String, String)>>,
}

impl Mailer for Inbox {
    fn submit(&self, msg: &Message) -> Result<(), MailError> {
        self.sent
            .lock()
            .unwrap()
            .push((msg.to().as_str().to_string(), msg.text().to_string()));
        Ok(())
    }
    fn label(&self) -> String {
        "inbox".into()
    }
}

struct H {
    pool: Pool,
    provider: Arc<EmailCodeProvider>,
    store: Arc<dyn EmailCodeStore>,
    queue: Arc<dyn MailQueue>,
    inbox: Arc<Inbox>,
}

async fn harness(stem: &str) -> H {
    let pool = pool(stem).await;
    let store: Arc<dyn EmailCodeStore> =
        Arc::new(PgEmailCodeStore::open_on(pool.clone()).expect("store"));
    let queue: Arc<dyn MailQueue> = Arc::new(PgMailQueue::open_on(pool.clone()).expect("queue"));
    let provider = Arc::new(EmailCodeProvider::new(
        SECRET,
        EmailCodePolicy::default(),
        store.clone(),
        queue.clone(),
        Arc::new(MemoryDirectoryStore::new()),
        Provisioning::AutoOnboard,
    ));
    H {
        pool,
        provider,
        store,
        queue,
        inbox: Arc::new(Inbox::default()),
    }
}

/// The real `mail.send` pass, with the composers a booted deployment has.
async fn send(h: &H, now: u64) -> mail::Pass {
    let mailer: Arc<dyn Mailer> = h.inbox.clone();
    let composers = Arc::new(mail::boot_composers(Some(h.provider.clone())));
    mail::run_pass(
        &h.pool,
        &mailer,
        &composers,
        &SendPolicy::default(),
        &Metrics::new(),
        now,
    )
    .await
    .expect("pass")
}

fn last_code(h: &H) -> String {
    let sent = h.inbox.sent.lock().unwrap();
    let text = &sent.last().expect("a message").1;
    let start = text.find("code is ").unwrap() + "code is ".len();
    text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect()
}

fn complete(
    h: &H,
    challenge: &str,
    binder: &str,
    code: &str,
    now: u64,
) -> Result<ordo_identity::AuthenticatedSubject, AuthError> {
    h.provider.complete(
        &CompleteRequest::EmailCode {
            challenge: ChallengeId(challenge.to_string()),
            binder: binder.to_string(),
            code: code.to_string(),
        },
        Timestamp(now),
    )
}

/// `start` blocks on the store's own thread, so it runs off the test's.
async fn start(h: &H, address: &str, now: u64) -> email_code::Begun {
    let p = h.provider.clone();
    let a = address.to_string();
    tokio::task::spawn_blocking(move || p.start(&a, None, now))
        .await
        .unwrap()
        .expect("begun")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_sent_through_the_real_outbox_signs_in_once() {
    let h = harness("ordo_email_outbox_once").await;
    let begun = start(&h, KIM, NOW).await;
    // Nothing about the address or a code is in the ceremony row in the
    // clear: the address is sealed and the code does not exist yet.
    let row = h
        .pool
        .rows(r#"SELECT "payload", "binder" FROM "$authceremony" WHERE "connection" = 'email'"#)
        .await
        .unwrap();
    assert_eq!(row.len(), 1);
    let payload = row[0][0].clone().unwrap();
    assert!(!payload.contains("kim"), "{payload}");
    assert_ne!(
        row[0][1].as_deref(),
        Some(begun.binder.as_str()),
        "the binder is hashed"
    );

    let pass = send(&h, NOW + 1).await;
    assert_eq!(pass.sent, 1);
    let code = last_code(&h);
    let payload = h
        .pool
        .rows(r#"SELECT "payload" FROM "$authceremony""#)
        .await
        .unwrap()[0][0]
        .clone()
        .unwrap();
    assert!(!payload.contains(&code), "the code is kept as a MAC only");

    let (c, b) = (begun.challenge.clone(), begun.binder.clone());
    let h2 = Arc::new(h);
    let hh = h2.clone();
    let code2 = code.clone();
    let subject = tokio::task::spawn_blocking(move || complete(&hh, &c, &b, &code2, NOW + 2))
        .await
        .unwrap()
        .expect("signed in");
    assert_eq!(
        subject.authenticator,
        AuthenticatorId(format!("email:{KIM}"))
    );
    assert_eq!(
        subject
            .attributes
            .email
            .as_ref()
            .map(|e| e.address().to_string()),
        Some(KIM.to_string())
    );
    let hh = h2.clone();
    let again = tokio::task::spawn_blocking(move || {
        complete(&hh, &begun.challenge, &begun.binder, &code, NOW + 3)
    })
    .await
    .unwrap();
    assert!(
        matches!(again, Err(AuthError::UnknownChallenge(_))),
        "{again:?}"
    );
}

/// A row sent twice (the at-least-once case) mints a second code; only that
/// one works, once. The counters in `$authrate` count the ceremony once.
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_row_sends_a_new_code_and_the_ceremony_still_signs_in_once() {
    let h = Arc::new(harness("ordo_email_outbox_dup").await);
    let begun = start(&h, KIM, NOW).await;
    // The same job again, as a crash before the outbox delete leaves it.
    let handle: String = h
        .pool
        .rows(r#"SELECT "payload" FROM "$mailoutbox""#)
        .await
        .unwrap()[0][0]
        .clone()
        .unwrap();
    let hq = h.clone();
    tokio::task::spawn_blocking(move || {
        let job = mail::MailJob::new(
            email_code::MAIL_KIND,
            mail::MailAddress::parse(KIM).unwrap(),
            &handle,
        )
        .unwrap();
        hq.queue.enqueue(&job, NOW).unwrap();
    })
    .await
    .unwrap();
    assert_eq!(send(&h, NOW + 1).await.sent, 2);
    let codes: Vec<String> = h
        .inbox
        .sent
        .lock()
        .unwrap()
        .iter()
        .map(|(_, t)| {
            let s = t.find("code is ").unwrap() + 8;
            t[s..].chars().take_while(char::is_ascii_digit).collect()
        })
        .collect();
    let counted = h
        .pool
        .rows(r#"SELECT "n" FROM "$authrate" WHERE "key" LIKE 'email.to.h:%'"#)
        .await
        .unwrap();
    assert_eq!(
        counted[0][0].as_deref(),
        Some("1"),
        "one ceremony, one count"
    );

    let (c, b) = (begun.challenge.clone(), begun.binder.clone());
    let hh = h.clone();
    let results = tokio::task::spawn_blocking(move || {
        let first = complete(&hh, &c, &b, &codes[0], NOW + 2).is_ok();
        let second = complete(&hh, &c, &b, &codes[1], NOW + 2).is_ok();
        let replay = complete(&hh, &c, &b, &codes[1], NOW + 2).is_ok();
        (first, second, replay)
    })
    .await
    .unwrap();
    assert_eq!(results, (false, true, false));
}

/// The limits are rows every replica counts: the sixth message in an hour
/// to one address is not sent, and the sweep removes counters that ended.
#[tokio::test(flavor = "multi_thread")]
async fn the_sixth_message_in_an_hour_is_not_sent_and_the_sweep_clears_the_counters() {
    let h = harness("ordo_email_outbox_limit").await;
    let mut passes = Vec::new();
    for _ in 0..6 {
        start(&h, KIM, NOW).await;
        let pass = send(&h, NOW + 1).await;
        passes.push((pass.sent, pass.skipped));
    }
    assert_eq!(passes, vec![(1, 0), (1, 0), (1, 0), (1, 0), (1, 0), (0, 1)]);
    // E5: each new code ended the older ceremony, so one is open.
    let open = h
        .pool
        .rows(r#"SELECT count(*) FROM "$authceremony" WHERE "connection" = 'email'"#)
        .await
        .unwrap();
    assert_eq!(open[0][0].as_deref(), Some("1"));

    let store = h.store.clone();
    let swept = tokio::task::spawn_blocking(move || store.sweep(NOW + 2 * 86_400))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(swept, 3, "the hour, the day and the deployment counters");
}

/// E5 in order: a slow message for an older `begin` does not end the
/// ceremony the page is waiting on, and is not sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_message_for_an_older_begin_is_not_sent() {
    let h = harness("ordo_email_outbox_late").await;
    let old = start(&h, KIM, NOW).await;
    let new = start(&h, KIM, NOW + 5).await;
    let pass = send(&h, NOW + 6).await;
    assert_eq!((pass.sent, pass.skipped), (1, 1), "only the newer is sent");
    let code = last_code(&h);
    let h = Arc::new(h);
    let hh = h.clone();
    let (o, n) = tokio::task::spawn_blocking(move || {
        (
            complete(&hh, &old.challenge, &old.binder, &code, NOW + 7).is_ok(),
            complete(&hh, &new.challenge, &new.binder, &code, NOW + 7).is_ok(),
        )
    })
    .await
    .unwrap();
    assert_eq!((o, n), (false, true));
}
