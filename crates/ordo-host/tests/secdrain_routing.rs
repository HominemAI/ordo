//! S4 of `plan-space-routing.md`: the security stream split by scope across
//! databases.
//!
//! A space's own stream travels with its data, so its entries go to the
//! database the space resolves to. The deployment stream stays in the
//! control database, and it is written without asking the resolver
//! anything, because a denial can name a space that does not exist and the
//! record of it must not depend on opening one.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp};
use ordo_conformance::Pg;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver, Resolving};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig, PoolError};
use ordo_host::metrics::Metrics;
use ordo_host::secdrain::{DrainConfig, SecurityDrain};
use ordo_secevent::{
    EventKind, RecordedDecision, Recorder, Relationship, Retention, Scope, Stamp, StreamEntry,
    Subject,
};

const ALPHA: &str = "spc_alpha";

/// A database of this suite's own, with an empty security stream.
async fn fresh(db: &str) -> Pool {
    let _ = Pg::named(db);
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    for t in ["$secevents", "$seccheckpoints", "$schedules"] {
        pool.rows(&format!("TRUNCATE TABLE \"{t}\"")).await.unwrap();
    }
    pool
}

fn quick() -> DrainConfig {
    DrainConfig {
        tick: Duration::from_millis(50),
        retain: false,
        ..DrainConfig::default()
    }
}

/// A denial in `space`, from a member (so the space's own stream) or from
/// someone with no relationship to it (so the deployment's).
fn deny(recorder: &Recorder, space: &str, rel: Relationship, n: u32) {
    recorder.deny(
        Some(&SpaceId(space.into())),
        Subject::member("usr_1"),
        "openTasks",
        RecordedDecision::OpNotInRole,
        &Stamp::new(
            Timestamp(1_000 + u64::from(n)),
            CorrelationId(format!("cor_{n:08x}")),
        ),
        rel,
    );
}

/// Rows per scope in a database's `$secevents`.
async fn rows_in(pool: &Pool, scope: &str) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"$secevents\" WHERE \"scope\" = '{scope}'"
    ))
    .await
    .unwrap()[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// **A target holds its space's stream and nothing of the deployment's; the
/// control database holds the deployment's and nothing of the space's.**
#[tokio::test(flavor = "multi_thread")]
async fn a_target_database_has_no_deployment_scope_rows() {
    let control = fresh("ordo_secroute_control").await;
    let target = fresh("ordo_secroute_target").await;
    spacedb::ensure_table(&control).await.expect("table");
    control.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    let env = PoolConfig::from_env();
    spacedb::attach(
        &control,
        &ordo_crypto::SpaceId(ALPHA.into()),
        &DbAttachment {
            host: env.host.clone(),
            port: env.port,
            user: env.user.clone(),
            db: "ordo_secroute_target".into(),
            credential: "local".into(),
            custody: DbCustody::PlatformManaged,
            attached_at: 1,
            attached_by: "mem_owner".into(),
        },
    )
    .await
    .expect("attached");
    let pools: Arc<dyn PoolResolver> = Arc::new(AttachedPools::new(
        control.clone(),
        control.clone(),
        PoolConfig {
            size: 2,
            ..PoolConfig::from_env()
        },
        Arc::new(Installed::new().as_configured("local")),
    ));

    let recorder = Arc::new(Recorder::default());
    for n in 0..3 {
        deny(&recorder, ALPHA, Relationship::Member, n);
        deny(&recorder, ALPHA, Relationship::None, 100 + n);
    }
    let drain = SecurityDrain::start_routed(
        recorder,
        control.clone(),
        pools,
        Arc::new(Metrics::new()),
        quick(),
    )
    .await
    .expect("drain");
    drain.shutdown().await.expect("everything is written");

    assert!(
        rows_in(&target, ALPHA).await > 0,
        "the space's stream is in its own database"
    );
    assert_eq!(
        rows_in(&target, "$deployment").await,
        0,
        "and nothing of the deployment's is"
    );
    assert!(
        rows_in(&control, "$deployment").await > 0,
        "the control database holds the deployment stream"
    );
    assert_eq!(
        rows_in(&control, ALPHA).await,
        0,
        "and nothing of a space that lives elsewhere"
    );
}

/// Refuses every space, and counts how often it was asked.
#[derive(Default)]
struct Refusing(AtomicUsize);

impl PoolResolver for Refusing {
    fn resolve<'a>(&'a self, space: &'a ordo_crypto::SpaceId) -> Resolving<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let err = PoolError::db(format!("no database for `{}`", space.0));
        Box::pin(async move { Err(err) })
    }

    fn targets<'a>(&'a self) -> ordo_exec::route::Targets<'a> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// **A denial naming a space that does not exist is recorded without opening
/// any target.** The resolver here refuses everything and counts; the
/// deployment stream still receives the denial, and the resolver is never
/// asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_denial_naming_an_unknown_space_is_recorded_without_opening_a_target() {
    let control = fresh("ordo_secroute_unknown").await;
    let refusing = Arc::new(Refusing::default());
    let recorder = Arc::new(Recorder::default());
    deny(&recorder, "spc_nobody", Relationship::None, 1);

    let drain = SecurityDrain::start_routed(
        recorder,
        control.clone(),
        refusing.clone(),
        Arc::new(Metrics::new()),
        quick(),
    )
    .await
    .expect("drain");
    drain
        .shutdown()
        .await
        .expect("the deployment stream takes it");

    assert!(rows_in(&control, "$deployment").await > 0);
    let about = control
        .rows("SELECT \"about\" FROM \"$secevents\" WHERE \"scope\" = '$deployment'")
        .await
        .unwrap();
    assert_eq!(
        about[0][0].as_deref(),
        Some("spc_nobody"),
        "the entry says which space it was about"
    );
    assert_eq!(
        refusing.0.load(Ordering::SeqCst),
        0,
        "the deployment stream was written without asking where any space lives"
    );
}

/// One space's database is down; everything else resolves to the control.
struct DownFor {
    down: &'static str,
    pool: Pool,
}

impl PoolResolver for DownFor {
    fn resolve<'a>(&'a self, space: &'a ordo_crypto::SpaceId) -> Resolving<'a> {
        let out = if space.0 == self.down {
            Err(PoolError::db(format!("`{}` is unreachable", space.0)))
        } else {
            Ok(self.pool.clone())
        };
        Box::pin(async move { out })
    }

    fn targets<'a>(&'a self) -> ordo_exec::route::Targets<'a> {
        let (pool, down) = (self.pool.clone(), self.down);
        Box::pin(async move {
            Ok(vec![
                ordo_exec::route::Target {
                    label: "default".into(),
                    pool: Ok(pool),
                },
                ordo_exec::route::Target {
                    label: down.to_string(),
                    pool: Err(format!("`{down}` is unreachable")),
                },
            ])
        })
    }
}

/// **A target that is down holds back only its own spaces.** Their entries
/// stay on the outbox for the next pass, and every other destination is
/// written.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_target_holds_back_only_its_own_spaces() {
    let control = fresh("ordo_secroute_down").await;
    let recorder = Arc::new(Recorder::default());
    deny(&recorder, "spc_up", Relationship::Member, 1);
    deny(&recorder, "spc_down", Relationship::Member, 2);
    deny(&recorder, "spc_down", Relationship::None, 3);

    let drain = SecurityDrain::start_routed(
        recorder.clone(),
        control.clone(),
        Arc::new(DownFor {
            down: "spc_down",
            pool: control.clone(),
        }),
        Arc::new(Metrics::new()),
        quick(),
    )
    .await
    .expect("drain");
    let err = drain
        .shutdown()
        .await
        .expect_err("one destination could not be written");
    assert!(
        err.appended > 0,
        "the reachable destinations were written: {err}"
    );
    assert!(err.error.reason().contains("spc_down"), "{err}");

    assert!(rows_in(&control, "spc_up").await > 0);
    assert!(
        rows_in(&control, "$deployment").await > 0,
        "a denial about the down space by a stranger is the deployment's, and is written"
    );
    assert_eq!(rows_in(&control, "spc_down").await, 0);
    assert!(
        recorder.backlog() > 0,
        "the down space's entries are held on the outbox, not lost"
    );
}

async fn checkpoints_in(pool: &Pool, scope: &str) -> i64 {
    pool.rows(&format!(
        "SELECT count(*) FROM \"$seccheckpoints\" WHERE \"scope\" = '{scope}'"
    ))
    .await
    .unwrap()[0][0]
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// **Retention walks every database a stream lives in.** A space's stream is
/// in its own database and the deployment's in the control database; one pass
/// checkpoints both, and names the database it could not reach.
///
/// Driven through `retain_everywhere` with a seeded signer, the way the
/// single-database retention test drives `retain_scope`, rather than through
/// the drain, which reads its signing key from the process environment every
/// test shares.
#[tokio::test(flavor = "multi_thread")]
async fn retention_walks_every_database() {
    let control = fresh("ordo_secroute_retain_control").await;
    let target = fresh("ordo_secroute_retain_target").await;
    let handle = tokio::runtime::Handle::current();
    let entry = |scope: Scope| {
        StreamEntry::single(
            scope,
            Subject::member("usr_1"),
            "openTasks",
            EventKind::denied(RecordedDecision::NoMembership),
            Timestamp(1_000),
            CorrelationId("cor_retain".into()),
        )
    };
    ordo_exec::secevent::PgSecurityStore::new(target.clone(), handle.clone())
        .try_append(&entry(Scope::Space(SpaceId(ALPHA.into()))))
        .await
        .expect("the space's entry, in its own database");
    ordo_exec::secevent::PgSecurityStore::new(control.clone(), handle)
        .try_append(&entry(Scope::Deployment))
        .await
        .expect("the deployment's entry, in the control database");

    let signer = ordo_commandlog::Ed25519Signer::from_seed("sec-retain-routed", &[4u8; 32]);
    let sweep = ordo_host::secdrain::retain_everywhere(
        &control,
        &DownFor {
            down: "spc_never",
            pool: target.clone(),
        },
        &signer,
        &signer.verifier(),
        Retention {
            keep_secs: 10,
            deployment_keep_secs: 10,
        },
        Timestamp(1_000_000),
    )
    .await;

    assert_eq!(
        checkpoints_in(&target, ALPHA).await,
        1,
        "the space's stream was retained in its own database"
    );
    assert_eq!(
        checkpoints_in(&control, "$deployment").await,
        1,
        "and the deployment's in the control database"
    );
    assert_eq!(sweep.checkpointed.len(), 2, "{:?}", sweep.checkpointed);
    assert!(
        sweep
            .notes
            .iter()
            .any(|n| n.contains("spc_never") && n.contains("not reached")),
        "the unreachable database is named, not skipped: {:?}",
        sweep.notes
    );
}
