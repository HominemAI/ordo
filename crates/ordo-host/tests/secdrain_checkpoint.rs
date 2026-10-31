//! 7d(b) — the one property about the checkpoint pass that can only be
//! asserted from a process of its own.
//!
//! Everything else about the pass lives in `tests/secdrain.rs`, driven
//! through `Checkpointer::new` with a signer the test holds. The wiring
//! question — *which passes does `SecurityDrain::start` enable, given what is
//! in the environment* — cannot be asked that way, because the answer is
//! read from `ORDO_SECURITY_CHECKPOINT_KEY` and the environment is global to
//! a test binary. `tests/secdrain.rs`'s own `quick()` says so: "it needs a
//! signing key from the environment, and the environment is shared with
//! every other test in this binary".
//!
//! So this is a file, which is a binary, which is a process. One test
//! function, two phases in order, and nothing else in here reads the
//! environment.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp};
use ordo_conformance::Pg;
use ordo_exec::secevent::PgSecurityStore;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::metrics::Metrics;
use ordo_host::secdrain::{DrainConfig, SecurityDrain};
use ordo_secevent::{EventKind, RecordedDecision, Recorder, Scope, StreamEntry, Subject};

const DB: &str = "ordo_sec_cp_enablement";

/// The seed a deployment would put in `ORDO_SECURITY_CHECKPOINT_KEY`.
const SEED_HEX: &str = "0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d";

fn scope() -> Scope {
    Scope::Space(SpaceId("spc_alpha".into()))
}

async fn fresh() -> Pool {
    let _ = Pg::named(DB);
    let pool = Pool::connect(PoolConfig {
        db: DB.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.ensure_platform_tables()
        .await
        .expect("platform tables");
    for t in ["$secevents", "$seccheckpoints", "$schedules"] {
        pool.rows(&format!("TRUNCATE TABLE \"{t}\"")).await.unwrap();
    }
    pool
}

async fn seed(pool: &Pool, n: u32) {
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    for i in 0..n {
        sec.try_append(&StreamEntry::single(
            scope(),
            Subject::member("usr_1"),
            "openTasks",
            EventKind::denied(RecordedDecision::NoMembership),
            Timestamp(1_000 + u64::from(i)),
            CorrelationId(format!("cor_{i}")),
        ))
        .await
        .expect("append");
    }
}

/// **Checkpointing does not depend on retention being switched on**, and a
/// deployment with no security key is told which of the two it has lost.
///
/// The defect this pins down is a settings dependency rather than a bug in
/// any one function. `DrainConfig::retain = false` is an ordinary storage
/// choice — keep every denial forever — and until the two passes were
/// separated it also, silently, switched off the only thing that could show
/// a denial had been deleted. The more consequential setting was being
/// decided by the less consequential one, and nothing anywhere said so.
///
/// Two phases in one test function on purpose: both read the process
/// environment, so they cannot run beside each other.
#[tokio::test(flavor = "multi_thread")]
async fn checkpointing_is_enabled_by_the_key_alone_and_not_by_retention() {
    let pool = fresh().await;
    seed(&pool, 4).await;
    let recorder: Arc<Recorder> = Arc::default();
    let metrics = Arc::new(Metrics::new());

    // ── phase 1: no key ──────────────────────────────────────────────────
    //
    // A background loop that failed every tick forever would be an alert
    // nobody could clear, so the pass disables itself — and says why, on the
    // surface an operator reads. This is the deliberate opposite of
    // `ordo checkpoint --security`, where a missing key is exit 1.
    std::env::remove_var(ordo_secevent::SECURITY_KEY_ENV);
    std::env::remove_var(ordo_secevent::SECURITY_KEY_ID_ENV);
    let drain = SecurityDrain::start(
        recorder.clone(),
        pool.clone(),
        metrics.clone(),
        DrainConfig {
            tick: Duration::from_millis(50),
            supervised_retention: true,
            ..DrainConfig::default()
        },
    )
    .await
    .expect("starts with no key");
    assert!(
        drain.checkpointer().is_none(),
        "no key must mean no checkpoint pass"
    );
    let note = drain
        .health()
        .checkpoint_note()
        .expect("the reason is on the health surface, not inferred from a flat counter");
    assert!(
        note.contains("removed from the END") && note.contains(ordo_secevent::SECURITY_KEY_ENV),
        "the note must say what is lost and which variable restores it: {note}"
    );
    drop(drain);

    // ── phase 2: a key, and retention switched off ───────────────────────
    std::env::set_var(ordo_secevent::SECURITY_KEY_ENV, SEED_HEX);
    std::env::set_var(ordo_secevent::SECURITY_KEY_ID_ENV, "sec-enablement");
    let drain = SecurityDrain::start(
        recorder.clone(),
        pool.clone(),
        metrics.clone(),
        DrainConfig {
            tick: Duration::from_millis(50),
            // Keep everything. The storage choice, made explicitly.
            retain: false,
            supervised_retention: true,
            ..DrainConfig::default()
        },
    )
    .await
    .expect("starts");
    assert!(
        drain.retainer().is_none(),
        "retention was switched off and must stay off"
    );
    let checkpointer = drain
        .checkpointer()
        .expect("retention being off must not switch off the marker at the head");

    // And it actually signs: the schedule row was registered by `start`, so
    // a pass a period later claims it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let later = now + DrainConfig::default().checkpoint_tick.as_secs();
    assert_eq!(
        checkpointer.pass(later).await.expect("a due pass"),
        Some(1),
        "a deployment that keeps everything still gets its head signed"
    );
    let upto: Option<String> = pool
        .rows("SELECT \"upto\" FROM \"$seccheckpoints\"")
        .await
        .unwrap()
        .first()
        .and_then(|r| r.first().cloned())
        .flatten();
    assert_eq!(upto.as_deref(), Some("4"));
    assert_eq!(
        drain.health().checkpointed(),
        1,
        "the counter that answers `is anything anchoring the end of the stream`"
    );

    std::env::remove_var(ordo_secevent::SECURITY_KEY_ENV);
    std::env::remove_var(ordo_secevent::SECURITY_KEY_ID_ENV);
}
