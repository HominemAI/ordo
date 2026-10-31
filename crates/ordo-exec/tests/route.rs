//! S3 of `plan-space-routing.md`: `AttachedPools`, against real Postgres.
//!
//! Three databases: the control database (which also serves every space
//! with no attachment, `plan-space-routing.md` Q-1), and two targets. Every
//! test asks the handle what it resolved and then asks the pool which
//! database it is connected to, because two handles can differ and still
//! reach the same place.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::SpaceId;
use ordo_exec::route::{AttachedPools, Installed, PoolResolver};
use ordo_exec::spacedb::{self, DbAttachment, DbCustody};
use ordo_exec::{Pool, PoolConfig, PoolError, PoolLimits};

const CRED: &str = "local";

fn admin() -> Pg {
    Pg {
        db: "postgres".into(),
        ..Pg::default()
    }
}

async fn control(db: &str) -> Pool {
    let _ = admin().exec(&format!("CREATE DATABASE {db}"));
    Fixture::load()
        .expect("fixture")
        .reset(&Pg {
            db: db.to_string(),
            ..Pg::default()
        })
        .expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("control");
    spacedb::ensure_table(&pool).await.expect("table");
    pool.rows("DELETE FROM \"$spacedb\"").await.unwrap();
    pool
}

/// An empty database a space can be attached to.
fn target_db(db: &str) {
    let _ = admin().exec(&format!("CREATE DATABASE {db}"));
}

fn on(db: &str) -> DbAttachment {
    let env = PoolConfig::from_env();
    DbAttachment {
        host: env.host,
        port: env.port,
        user: env.user,
        db: db.into(),
        credential: CRED.into(),
        custody: DbCustody::PlatformManaged,
        attached_at: 1,
        attached_by: "mem_owner".into(),
    }
}

fn resolver(control: &Pool) -> AttachedPools {
    AttachedPools::new(
        control.clone(),
        control.clone(),
        PoolConfig {
            size: 2,
            ..PoolConfig::from_env()
        },
        Arc::new(Installed::new().as_configured(CRED)),
    )
}

async fn database_of(pool: &Pool) -> String {
    pool.rows("SELECT current_database()").await.unwrap()[0][0]
        .clone()
        .unwrap()
}

async fn attach(control: &Pool, space: &str, target: DbAttachment) {
    spacedb::attach(control, &SpaceId(space.into()), &target)
        .await
        .expect("attached");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_space_with_no_row_uses_the_default() {
    let control = control("ordo_route_default").await;
    let r = resolver(&control);
    let pool = r
        .resolve(&SpaceId("spc_plain".into()))
        .await
        .expect("resolves");
    assert!(pool.same_handle(&control));
    assert_eq!(
        r.target_count(),
        0,
        "nothing was dialled for a default space"
    );
}

/// **Pools are per target.** Two spaces on one database share one pool; a
/// space on another database gets another; the count is of targets.
#[tokio::test(flavor = "multi_thread")]
async fn two_spaces_on_one_target_share_one_pool() {
    let control = control("ordo_route_shared").await;
    target_db("ordo_route_shared_t1");
    target_db("ordo_route_shared_t2");
    attach(&control, "spc_a", on("ordo_route_shared_t1")).await;
    attach(&control, "spc_b", on("ordo_route_shared_t1")).await;
    attach(&control, "spc_c", on("ordo_route_shared_t2")).await;
    let r = resolver(&control);

    let a = r.resolve(&SpaceId("spc_a".into())).await.expect("a");
    let b = r.resolve(&SpaceId("spc_b".into())).await.expect("b");
    let c = r.resolve(&SpaceId("spc_c".into())).await.expect("c");
    assert!(a.same_handle(&b), "two spaces on one target share its pool");
    assert!(!a.same_handle(&c), "a space on another target does not");
    assert_eq!(database_of(&a).await, "ordo_route_shared_t1");
    assert_eq!(database_of(&c).await, "ordo_route_shared_t2");
    assert_eq!(r.target_count(), 2, "three spaces, two targets, two pools");

    let again = r.resolve(&SpaceId("spc_a".into())).await.expect("again");
    assert!(
        again.same_handle(&a),
        "a resolved target is reused, not dialled again"
    );
}

/// **An unreachable target fails at resolve**, in bounded time, and the
/// failure is not remembered as a pool.
///
/// The second attempt here is refused by the backoff R-F3 added rather
/// than by a second dial, which is why this asserts the refusal and the
/// bound and not the dial. What it still pins is the part the backoff
/// does not change: a failed dial never becomes a cached pool, so a
/// target that comes back is reached with no restart.
/// `a_down_target_is_dialled_once_per_backoff_not_once_per_request` owns
/// the dial counting.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_target_fails_at_resolve_not_mid_write() {
    let control = control("ordo_route_unreachable").await;
    attach(
        &control,
        "spc_gone",
        DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            ..on("nowhere")
        },
    )
    .await;
    let r = resolver(&control);
    let space = SpaceId("spc_gone".into());

    for attempt in 1..=2 {
        let started = Instant::now();
        let Err(err) = r.resolve(&space).await else {
            panic!("nothing listens there, so the resolve must fail");
        };
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "attempt {attempt} took {:?}: a dial must be bounded",
            started.elapsed()
        );
        assert!(!err.to_string().is_empty());
    }
    assert_eq!(r.target_count(), 0, "a failed dial is not a pool");
}

/// **A label this node does not hold is refused before any dial**, even for
/// a target that would have let an unauthenticated dial through.
#[tokio::test(flavor = "multi_thread")]
async fn a_credential_this_node_does_not_hold_is_refused_before_any_dial() {
    let control = control("ordo_route_no_credential").await;
    target_db("ordo_route_no_credential_t");
    attach(
        &control,
        "spc_a",
        DbAttachment {
            credential: "someone-elses".into(),
            ..on("ordo_route_no_credential_t")
        },
    )
    .await;
    let r = resolver(&control);
    let Err(err) = r.resolve(&SpaceId("spc_a".into())).await else {
        panic!("the label is not installed here, so the resolve must fail");
    };
    assert!(err.to_string().contains("someone-elses"), "{err}");
    assert_eq!(r.target_count(), 0);
}

/// **A miss is remembered until it expires or is forgotten.**
///
/// This pins the window `AttachedPools`' doc describes rather than hiding
/// it: a node that answered "default" for a space keeps answering it until
/// the miss expires, which is why provisioning attaches before any member
/// exists, and why `forget` exists for the node that attached.
#[tokio::test(flavor = "multi_thread")]
async fn a_miss_is_remembered_until_it_expires_or_is_forgotten() {
    let control = control("ordo_route_miss").await;
    target_db("ordo_route_miss_t");
    let space = SpaceId("spc_late".into());

    let r = resolver(&control);
    assert!(r.resolve(&space).await.unwrap().same_handle(&control));
    attach(&control, "spc_late", on("ordo_route_miss_t")).await;
    assert!(
        r.resolve(&space).await.unwrap().same_handle(&control),
        "inside the miss window the cached answer stands"
    );
    r.forget(&space);
    let found = r.resolve(&space).await.unwrap();
    assert_eq!(database_of(&found).await, "ordo_route_miss_t");

    // And with no window at all, an attachment is seen on the next resolve.
    let space = SpaceId("spc_later".into());
    let r = resolver(&control).with_miss_ttl(Duration::ZERO);
    assert!(r.resolve(&space).await.unwrap().same_handle(&control));
    attach(&control, "spc_later", on("ordo_route_miss_t")).await;
    let found = r.resolve(&space).await.unwrap();
    assert_eq!(database_of(&found).await, "ordo_route_miss_t");
}

/// Many requests for one new target at once dial it once.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_first_resolves_dial_one_pool() {
    let control = control("ordo_route_concurrent").await;
    target_db("ordo_route_concurrent_t");
    for s in ["spc_1", "spc_2", "spc_3", "spc_4"] {
        attach(&control, s, on("ordo_route_concurrent_t")).await;
    }
    let r = Arc::new(resolver(&control));
    let mut tasks = Vec::new();
    for s in ["spc_1", "spc_2", "spc_3", "spc_4"] {
        let r = r.clone();
        tasks.push(tokio::spawn(async move {
            r.resolve(&SpaceId(s.into())).await.expect("resolves")
        }));
    }
    let pools: Vec<Pool> = futures_join(tasks).await;
    assert!(pools.windows(2).all(|w| w[0].same_handle(&w[1])));
    assert_eq!(r.target_count(), 1);
}

async fn futures_join(tasks: Vec<tokio::task::JoinHandle<Pool>>) -> Vec<Pool> {
    let mut out = Vec::new();
    for t in tasks {
        out.push(t.await.expect("task"));
    }
    out
}

/// The password never reaches a formatted config.
#[test]
fn a_password_is_never_printed() {
    let cfg = PoolConfig {
        password: Some("hunter2-correct-horse".into()),
        ..PoolConfig::from_env()
    };
    let shown = format!("{cfg:?}");
    assert!(!shown.contains("hunter2"), "{shown}");
    assert!(shown.contains("<redacted>"), "{shown}");
}

/// **§10 T0, `plan-space-routing.md` §9.1 R-C4.1: `PoolConfig` never prints
/// its password.** `PoolConfig::fmt` is written by hand rather than
/// derived, precisely so the password field cannot reach a log line, a
/// panic message, or an error that formats its config. A hand-written
/// `Debug` impl is exactly the kind of thing nothing forces to stay in sync
/// with the struct. Pinned under the name `plan-space-routing.md` names for
/// it, distinct from the pre-existing `a_password_is_never_printed` above,
/// which covers the same property under a name this plan does not.
#[test]
fn a_pool_config_never_prints_its_password() {
    let cfg = PoolConfig {
        password: Some("swordfish-must-never-print".into()),
        ..PoolConfig::from_env()
    };
    let shown = format!("{cfg:?}");
    assert!(
        !shown.contains("swordfish"),
        "the password leaked into Debug: {shown}"
    );
    assert!(
        shown.contains("<redacted>"),
        "redaction must say so, not just omit the value: {shown}"
    );

    // `None` is legitimate (peer auth, a socket, an IAM sidecar) and must
    // print as absent, not as a redaction of nothing.
    let cfg = PoolConfig {
        password: None,
        ..PoolConfig::from_env()
    };
    assert!(!format!("{cfg:?}").contains("<redacted>"));
}

/// **§10 T0, `plan-space-routing.md` §9.5 R-P2: the statement bound reaches
/// an attached target too, not only the default.** It travels today only
/// because `AttachedPools::pool_for` dials through `Pool::connect`, which
/// reads `PoolLimits::from_env`, the same reader the default pool's own
/// `Pool::connect` uses. Nothing in the resolver composes the bound on
/// purpose; this pins the property so a `pool_for` that grew its own dial
/// (its own `Pool::connect_with`, say, with limits it invented) would fail
/// this test rather than silently losing 20a's other half on every attached
/// target.
///
/// The wait is real: this suite sets no `ORDO_PG_STATEMENT_TIMEOUT_MS`, so
/// the bound in force is whatever `PoolLimits::from_env` actually resolves
/// to for this process, the same value a deployment would run with. That
/// is deliberate: a shorter bound baked into the test would prove the
/// property of a config nothing runs under.
#[tokio::test(flavor = "multi_thread")]
async fn a_statement_on_an_attached_target_is_bounded_too() {
    let control = control("ordo_route_bound").await;
    target_db("ordo_route_bound_t");
    attach(&control, "spc_bound", on("ordo_route_bound_t")).await;
    let r = resolver(&control);
    let pool = r
        .resolve(&SpaceId("spc_bound".into()))
        .await
        .expect("resolves onto the attached target");
    assert_eq!(database_of(&pool).await, "ordo_route_bound_t");

    let bound = PoolLimits::from_env()
        .statement_timeout
        .expect("this suite runs with a statement bound configured, same as a deployment does");
    let sleep_secs = bound.as_secs_f64() + 1.0;
    let err = pool
        .rows(&format!("SELECT pg_sleep({sleep_secs})"))
        .await
        .expect_err("a statement past the bound must be refused on an attached target too");
    assert!(
        matches!(err, PoolError::StatementTimeout(_)),
        "refused, but not as the statement bound: {err:?}"
    );

    // The cancelled statement cost the pool nothing: it is intact.
    pool.rows("SELECT 1")
        .await
        .expect("the attached target's pool serves the next caller");
}

/// **`targets` lists the default and each distinct target once**, however
/// many spaces share a target, and names one it cannot reach rather than
/// leaving it out.
#[tokio::test(flavor = "multi_thread")]
async fn targets_lists_the_default_and_each_distinct_target() {
    let control = control("ordo_route_targets").await;
    target_db("ordo_route_targets_t1");
    target_db("ordo_route_targets_t2");
    attach(&control, "spc_a", on("ordo_route_targets_t1")).await;
    attach(&control, "spc_b", on("ordo_route_targets_t1")).await;
    attach(&control, "spc_c", on("ordo_route_targets_t2")).await;
    attach(
        &control,
        "spc_gone",
        DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            ..on("nowhere")
        },
    )
    .await;
    let r = resolver(&control);

    let targets = r.targets().await.expect("listed");
    let labels: Vec<&str> = targets.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(
        targets.len(),
        4,
        "the default and three distinct targets, however many spaces share one: {labels:?}"
    );
    assert_eq!(targets[0].label, "default");
    assert!(targets[0]
        .pool
        .as_ref()
        .is_ok_and(|p| p.same_handle(&control)));
    let t1 = targets[1].pool.as_ref().expect("t1 answers");
    let t2 = targets[2].pool.as_ref().expect("t2 answers");
    assert_eq!(database_of(t1).await, "ordo_route_targets_t1");
    assert_eq!(database_of(t2).await, "ordo_route_targets_t2");
    assert!(
        targets[3].label.ends_with("/nowhere") && targets[3].pool.is_err(),
        "an unreachable target is listed with its reason: {}",
        targets[3].label
    );
}

/// **A down target costs a dial per backoff interval, not a dial per
/// request** (18d R-F3).
///
/// A failed dial deliberately leaves the target's cell empty so the next
/// request tries again, which is right for recovery and wrong for a
/// cluster that is simply down: every request, for every tenant on that
/// target, would pay a fresh dial and wait out the checkout timeout. With
/// a handful of tenants on one downed cluster that is the deployment
/// spending its connection budget on a database that is not there.
///
/// The property is asserted on the **dial count** rather than on timing
/// or on the refusal's text. Timing cannot tell the two apart when the
/// port refuses immediately, and text would pin a message rather than a
/// behaviour. `target_count` cannot answer either, because a target that
/// never connects has no pool to count, which is why the counter exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_down_target_is_dialled_once_per_backoff_not_once_per_request() {
    let control = control("ordo_route_backoff").await;
    attach(
        &control,
        "spc_down",
        DbAttachment {
            host: "127.0.0.1".into(),
            port: 1,
            ..on("nowhere")
        },
    )
    .await;
    let r = resolver(&control);
    let space = SpaceId("spc_down".into());

    // Ten requests in a row, well inside the first backoff window.
    for _ in 0..10 {
        assert!(
            r.resolve(&space).await.is_err(),
            "nothing listens there, so every resolve must fail"
        );
    }
    assert_eq!(
        r.dials(),
        1,
        "ten requests for a target that is down must cost one dial, not ten"
    );

    // And the backoff expires rather than latching: a cluster that comes
    // back must be found without a restart, so the next dial after the
    // window has to happen.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(r.resolve(&space).await.is_err());
    assert_eq!(
        r.dials(),
        2,
        "after the window elapses the target must be tried again, or a \
         recovered cluster would never be noticed"
    );
}

/// **`$deployment` is the password the deployment already holds**, so an
/// owner states it once. It does not have to be repeated in the credential
/// directory, and a directory entry of that name (empty, as the notes dev
/// script wrote it before) cannot make the door dial a tenant without it.
#[test]
fn the_deployment_label_answers_with_the_deployments_own_password() {
    use ordo_exec::route::{DbCredentials, WithOwnCredential, OWN_CREDENTIAL};

    let directory: Arc<dyn DbCredentials> = Arc::new(
        Installed::new()
            .passwordless(OWN_CREDENTIAL)
            .with_password("acme", "theirs"),
    );

    let held = WithOwnCredential::wrapping(directory.clone(), Some("ours".into()));
    assert_eq!(held.password(OWN_CREDENTIAL), Ok(Some("ours".into())));
    assert_eq!(held.password("acme"), Ok(Some("theirs".into())));
    assert!(held.password("nobody").is_err(), "one label added, no more");

    // No password of its own: the directory's entry when it has one, and
    // passwordless when it has none.
    let none = WithOwnCredential::wrapping(directory, None);
    assert_eq!(none.password(OWN_CREDENTIAL), Ok(None));
    let bare = WithOwnCredential::wrapping(Arc::new(Installed::new()), None);
    assert_eq!(bare.password(OWN_CREDENTIAL), Ok(None));
    let filed = WithOwnCredential::wrapping(
        Arc::new(Installed::new().with_password(OWN_CREDENTIAL, "filed")),
        None,
    );
    assert_eq!(filed.password(OWN_CREDENTIAL), Ok(Some("filed".into())));
}
