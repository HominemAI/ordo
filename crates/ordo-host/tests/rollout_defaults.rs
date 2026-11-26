//! The owner's rulings of 2026-11-25 on rollouts
//! (`rollouts-the-default-strategy-when-none`, asks 1 to 3;
//! `plan-versions.md` §3.7, §4 rows 10 and 11b).
//!
//! What these pin, over real Postgres and the door:
//!
//! - with `rollout.strategy` unset, `ordo apply` starts a `rolling` rollout
//!   in every environment but dev; dev activates every tenant at once;
//! - a first stage that had fewer calls than `rollout.health_min_calls`
//!   (default 20) when its wait ends pauses the rollout, naming the
//!   setting; `resume` moves the rest anyway; 0 lets a quiet stage pass;
//! - after an aborted rollout, `ordo rollback` to the version still active
//!   moves back the tenants that rollout pinned with no soak, the
//!   deployment's own space among them; a pin set by hand stays; `--dry-run`
//!   lists both.
//!
//! A regression looks like an apply that moves every production tenant at
//! once, a quiet canary that lets every tenant move, or a rollback that
//! leaves part of an aborted rollout in place or undoes a hotfix.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeMap;
use std::sync::Arc;

use ordo_api::BoundParams;
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, Activation, ApplyOps};
use ordo_host::deploy_ops;
use ordo_host::environments::Environments;
use ordo_host::rollout_strategies as strategies;
use ordo_host::rollouts::{
    self, RolloutRefusal, RolloutState, Rollouts, StartOptions, Strategy, TenantState,
};
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::Versions;

const V1: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

const V2: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

const V3: &str = "domain tasks @ 0.3.0

entity task {
  title: text
  done:  bool
  due:   int?
  note:  text?
}

op openTasks unbounded =
  task[title, done]
";

const V4: &str = "domain tasks @ 0.4.0

entity task {
  title: text
  done:  bool
  due:   int?
  note:  text?
  size:  int?
}

op openTasks unbounded =
  task[title, done]
";

/// The test's clock: 2026-11-05 12:00 UTC.
const NOW: u64 = 1_762_344_000;
const OWNER: &str = "deployment-owner:ada";

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 8,
        ..PoolConfig::from_env()
    }
}

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(cfg(&pg.db)).await.expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    pool
}

fn ops(pool: &Pool) -> Arc<ApplyOps> {
    ApplyOps::recording(
        pool.clone(),
        Arc::new(SinglePool::new(pool.clone())),
        "home",
        Some(Arc::new(ordo_secevent::Recorder::default())),
    )
}

async fn apply(pool: &Pool, source: &str, activate: bool) -> apply_op::Report {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        activate,
    )
    .await
    .expect("applies")
}

async fn tenant(pool: &Pool, id: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(pool, id, 11).await.expect("active");
}

async fn on(pool: &Pool, id: &str) -> Option<String> {
    Versions::on(pool)
        .pin_of(id)
        .await
        .expect("read")
        .expect("a tenant")
        .version
}

/// `n` tenants on 0.1.0.
async fn tenants_on_v1(stem: &str, n: usize) -> Pool {
    let pool = fresh(stem).await;
    for i in 0..n {
        tenant(&pool, &format!("spc_{i}")).await;
    }
    apply(&pool, V1, true).await;
    pool
}

/// A door op on the home space, as the deployment owner's machine key.
async fn door(
    ops: &ApplyOps,
    name: &str,
    v: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let b = BoundParams(v.as_object().expect("an object").clone());
    deploy_ops::execute(ops, name, "home", &b, "sys:updater")
        .await
        .map(|r| r.rows[0].clone())
        .map_err(|e| e.message)
}

// ── ask 1: the default strategy ─────────────────────────────────────────

/// **Unset, a deployment that is not dev rolls out `rolling`; dev moves
/// every tenant at once** (ruled 2026-11-25, ask 1). A deployment that
/// registered nothing reads as production. The settings say which default
/// holds, and a strategy set by hand wins in either.
#[tokio::test(flavor = "multi_thread")]
async fn with_no_strategy_set_production_rolls_out_and_dev_moves_every_tenant_at_once() {
    let pool = tenants_on_v1("ordo_rd_default", 3).await;
    let ops = ops(&pool);
    let r = Rollouts::on(&pool);
    let s = r.settings().await.unwrap();
    assert_eq!((s.strategy, s.default_strategy), (None, Strategy::Rolling));
    let read = door(&ops, deploy_ops::VERSION_SETTINGS, serde_json::json!({}))
        .await
        .expect("the settings through the door");
    assert_eq!(read["rollout"]["strategy"], serde_json::Value::Null);
    assert_eq!(read["rollout"]["strategyDefault"], "rolling");

    // Production: the apply stores 0.2.0 and starts a rolling rollout.
    let report = apply(&pool, V2, true).await;
    let Activation::RolloutStarted { id, strategy, .. } = report.activation.clone() else {
        panic!("a rollout, not an activation: {report:?}");
    };
    assert_eq!(strategy, "rolling");
    let v = Versions::on(&pool);
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    // The first turn moves the deployment's own space and one batch, sized
    // to finish within `rollout.window`: 1 of the 3.
    let t = rollouts::tick(&ops, "n", NOW).await.expect("tick");
    assert!(t.deployment_moved, "{t:?}");
    assert_eq!(t.moved.len(), 1, "{t:?}");
    let mut still = 0;
    for t in ["spc_0", "spc_1", "spc_2"] {
        if on(&pool, t).await.as_deref() == Some("0.1.0") {
            still += 1;
        }
    }
    assert_eq!(still, 2, "a bad version reaches one batch first");

    // While it is open an activation is refused, naming the way out.
    let e = apply_op::activate(&pool, "0.2.0", OWNER).await.unwrap_err();
    assert_eq!(e.class(), "rollout_open", "{e}");
    assert!(e.to_string().contains("ordo rollout abort"), "{e}");
    r.abort("the test moves on", NOW + 1)
        .await
        .expect("aborted");
    assert_eq!(
        r.get(id).await.unwrap().unwrap().state,
        RolloutState::Aborted
    );

    // `all`, set by hand, is the activation as before rollouts.
    r.set_setting("rollout.strategy", Some("all"), OWNER, "at once", NOW)
        .await
        .unwrap();
    assert_eq!(r.settings().await.unwrap().apply_strategy(), None);
    let report = apply(&pool, V3, true).await;
    assert!(
        matches!(report.activation, Activation::Switched { .. }),
        "{report:?}"
    );
    for t in ["spc_0", "spc_1", "spc_2"] {
        assert_eq!(on(&pool, t).await.as_deref(), Some("0.3.0"), "{t}");
    }
    r.set_setting("rollout.strategy", None, OWNER, "the default", NOW)
        .await
        .unwrap();

    // Dev: the same apply activates every tenant at once, with no rollout.
    let envs = Environments::on(&pool);
    envs.declare_own("dev", OWNER, "this deployment is dev", NOW)
        .await
        .expect("dev");
    let s = r.settings().await.unwrap();
    assert_eq!((s.strategy, s.default_strategy), (None, Strategy::All));
    let report = apply(&pool, V4, true).await;
    assert!(
        matches!(report.activation, Activation::Switched { .. }),
        "{report:?}"
    );
    for t in ["spc_0", "spc_1", "spc_2"] {
        assert_eq!(on(&pool, t).await.as_deref(), Some("0.4.0"), "{t}");
    }
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.4.0"));

    // A strategy set by hand wins in dev too.
    r.set_setting("rollout.strategy", Some("rolling"), OWNER, "try it", NOW)
        .await
        .unwrap();
    let s = r.settings().await.unwrap();
    assert_eq!(s.apply_strategy(), Some(Strategy::Rolling));
    // The refusal of a word that is not one names what `all` and `--clear`
    // give now.
    let e = r
        .set_setting("rollout.strategy", Some("fast"), OWNER, "x", NOW)
        .await
        .unwrap_err();
    assert!(
        e.to_string()
            .contains("`--clear` gives the default: `rolling`, or `all` in dev"),
        "{e}"
    );
}

// ── ask 2: a quiet first stage ──────────────────────────────────────────

fn counts(rows: &[(&str, u64, u64)]) -> BTreeMap<String, (u64, u64)> {
    rows.iter()
        .map(|(s, c, f)| (s.to_string(), (*c, *f)))
        .collect()
}

fn canary(wait: i64) -> StartOptions {
    StartOptions {
        strategy: Some(Strategy::Canary),
        canary_count: Some(1),
        canary_wait: Some(wait),
        batch_interval: Some(1),
        batch_size: Some(10),
        ..Default::default()
    }
}

/// The canary of rollout `id`.
async fn canary_of(pool: &Pool, id: i64) -> String {
    Rollouts::on(pool)
        .tenants(id)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.stage == 0)
        .expect("a canary")
        .space
}

/// **A first stage with too few calls pauses the rollout and names
/// `rollout.health_min_calls`** (ruled 2026-11-25, ask 2; default 20). A
/// quiet canary has proved nothing. `resume` moves the rest anyway; enough
/// calls pass the gate; 0 lets a quiet stage pass. A value that is not a
/// whole number of calls is refused, naming the setting.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_stage_with_too_few_calls_pauses_and_names_health_min_calls() {
    let pool = tenants_on_v1("ordo_rd_quiet", 3).await;
    apply(&pool, V2, false).await;
    let ops = ops(&pool);
    let r = Rollouts::on(&pool);
    assert_eq!(
        r.settings().await.unwrap().health_min_calls,
        strategies::HEALTH_MIN_CALLS_DEFAULT
    );
    assert_eq!(strategies::HEALTH_MIN_CALLS_DEFAULT, 20);

    // Refusals name the setting and the command that sets it.
    for bad in ["-1", "many", "2.5"] {
        let e = r
            .set_setting("rollout.health_min_calls", Some(bad), OWNER, "x", NOW)
            .await
            .unwrap_err();
        let text = e.to_string();
        assert!(
            text.contains("rollout.health_min_calls")
                && text.contains("ordo rollout set rollout.health_min_calls"),
            "{bad}: {text}"
        );
    }
    let e = door(
        &ops,
        rollouts::START,
        serde_json::json!({"version": "0.2.0", "strategy": "canary", "healthMinCalls": -3}),
    )
    .await
    .unwrap_err();
    assert!(e.contains("rollout.health_min_calls"), "{e}");

    // A canary with no calls: its wait ends, and the rollout pauses.
    let started = r.start("0.2.0", &canary(0), OWNER, NOW).await.unwrap();
    assert_eq!(started.health_min_calls, 20);
    let c = canary_of(&pool, started.id).await;
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert_eq!(t.moved, std::slice::from_ref(&c), "{t:?}");
    strategies::report(&pool, "n", &counts(&[(&c, 0, 0)]), NOW + 1)
        .await
        .unwrap();
    strategies::report(&pool, "n", &counts(&[(&c, 7, 0)]), NOW + 2)
        .await
        .unwrap();
    let t = rollouts::tick(&ops, "n", NOW + 5).await.unwrap();
    assert!(t.paused && t.quiet && !t.gate_passed, "{t:?}");
    assert!(t.moved.is_empty(), "{t:?}");
    let paused = r.get(started.id).await.unwrap().unwrap();
    assert_eq!(paused.state, RolloutState::Paused);
    for words in [
        "quiet",
        "7 call(s)",
        "rollout.health_min_calls 20",
        "ordo rollout resume",
        "ordo rollout set rollout.health_min_calls",
    ] {
        assert!(paused.reason.contains(words), "{words}: {}", paused.reason);
    }
    assert!(t
        .log_lines()
        .iter()
        .any(|l| l.contains("paused by rollout.health_min_calls")));
    // Paused, nothing more moves.
    let t = rollouts::tick(&ops, "n", NOW + 10).await.unwrap();
    assert!(t.moved.is_empty(), "{t:?}");
    // The door says so.
    let status = door(&ops, rollouts::STATUS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(status["state"], "paused");
    assert_eq!(status["healthMinCalls"], 20);

    // `resume` is the deployment owner moving the rest anyway.
    r.resume(NOW + 11).await.unwrap();
    let t = rollouts::tick(&ops, "n", NOW + 12).await.unwrap();
    assert_eq!(t.moved.len(), 2, "{t:?}");
    assert!(t.finished, "{t:?}");

    // Enough calls: the gate passes with no pause.
    let pool = tenants_on_v1("ordo_rd_busy", 3).await;
    apply(&pool, V2, false).await;
    let ops2 = self::ops(&pool);
    let r = Rollouts::on(&pool);
    let started = r.start("0.2.0", &canary(0), OWNER, NOW).await.unwrap();
    let c = canary_of(&pool, started.id).await;
    rollouts::tick(&ops2, "n", NOW).await.unwrap();
    strategies::report(&pool, "n", &counts(&[(&c, 0, 0)]), NOW + 1)
        .await
        .unwrap();
    strategies::report(&pool, "n", &counts(&[(&c, 25, 0)]), NOW + 2)
        .await
        .unwrap();
    let t = rollouts::tick(&ops2, "n", NOW + 5).await.unwrap();
    assert!(t.gate_passed && !t.paused, "{t:?}");

    // 0, set for a small app, lets a quiet first stage pass.
    let pool = tenants_on_v1("ordo_rd_small", 3).await;
    apply(&pool, V2, false).await;
    let ops3 = self::ops(&pool);
    let set = door(
        &ops3,
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({
            "setting": "rollout.health_min_calls", "value": "0", "reason": "a small app",
        }),
    )
    .await
    .expect("set through the door");
    assert_eq!(set["rollout"]["healthMinCalls"], 0);
    let r = Rollouts::on(&pool);
    let started = r.start("0.2.0", &canary(0), OWNER, NOW).await.unwrap();
    assert_eq!(started.health_min_calls, 0);
    rollouts::tick(&ops3, "n", NOW).await.unwrap();
    let t = rollouts::tick(&ops3, "n", NOW + 5).await.unwrap();
    assert!(t.gate_passed && !t.paused, "{t:?}");
}

// ── ask 3: a rollback after an aborted rollout ──────────────────────────

/// **After an aborted rollout, `ordo rollback` to the version still active
/// moves back the tenants that rollout pinned with no soak** (ruled
/// 2026-11-25, ask 3), the deployment's own space among them, every write
/// kept. A tenant pinned by hand (a one-tenant hotfix) stays, and so does
/// one the rollout never reached. `--dry-run` lists each.
#[tokio::test(flavor = "multi_thread")]
async fn a_rollback_after_an_aborted_rollout_moves_back_the_rollouts_own_pins() {
    let pool = tenants_on_v1("ordo_rd_rollback", 4).await;
    apply(&pool, V2, false).await;
    // 0.3.0 is stored too: the hotfix one tenant moves to by hand.
    apply(&pool, V3, false).await;
    let ops = ops(&pool);
    let r = Rollouts::on(&pool);
    let v = Versions::on(&pool);
    let started = r
        .start(
            "0.2.0",
            &StartOptions {
                strategy: Some(Strategy::Rolling),
                batch_interval: Some(600),
                batch_size: Some(2),
                ..Default::default()
            },
            OWNER,
            NOW,
        )
        .await
        .unwrap();
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert!(t.deployment_moved);
    assert_eq!(t.moved, ["spc_0", "spc_1"]);
    // spc_2 moves on its own, by hand, to a hotfix: the rollout skips it.
    v.pin("spc_2", "0.3.0", OWNER, NOW + 1).await.unwrap();
    rollouts::tick(&ops, "n", NOW + 2).await.unwrap();
    assert_eq!(
        r.tenant(started.id, "spc_2").await.unwrap().unwrap().state,
        TenantState::Skipped
    );
    door(
        &ops,
        rollouts::ABORT,
        serde_json::json!({"reason": "the new version misbehaves"}),
    )
    .await
    .expect("aborted");
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    assert_eq!(v.own_pin().await.unwrap().unwrap().1, "0.2.0");

    // The dry run names each.
    let dry = door(
        &ops,
        deploy_ops::ROLLBACK_CHECK,
        serde_json::json!({"version": "0.1.0"}),
    )
    .await
    .expect("the dry run");
    let rows = dry["tenants"].as_array().expect("tenants").clone();
    let row = |t: &str| {
        rows.iter()
            .find(|r| r["tenant"] == t)
            .unwrap_or_else(|| panic!("{t} in {rows:?}"))
            .clone()
    };
    for t in ["spc_0", "spc_1", "home"] {
        let r = row(t);
        assert_eq!(r["back"], true, "{r}");
        assert_eq!(r["rollout"], started.id, "{r}");
        assert!(
            r["note"]
                .as_str()
                .unwrap()
                .contains("pinned it with no soak: moves back to 0.1.0"),
            "{r}"
        );
    }
    let hotfix = row("spc_2");
    assert_eq!(hotfix["back"], false);
    assert!(
        hotfix["note"].as_str().unwrap().contains("set by hand"),
        "{hotfix}"
    );
    assert!(rows.iter().all(|r| r["tenant"] != "spc_3"), "{rows:?}");
    // The dry run changed nothing.
    assert_eq!(on(&pool, "spc_0").await.as_deref(), Some("0.2.0"));

    // The rollback through the door.
    let back = door(
        &ops,
        deploy_ops::ACTIVATE,
        serde_json::json!({"version": "0.1.0"}),
    )
    .await
    .expect("rolled back");
    assert_eq!(back["state"], "alreadyActive");
    let mut moved: Vec<String> = back["movedBack"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap().to_string())
        .collect();
    moved.sort();
    assert_eq!(moved, ["home", "spc_0", "spc_1"]);
    for t in ["spc_0", "spc_1", "spc_3"] {
        assert_eq!(on(&pool, t).await.as_deref(), Some("0.1.0"), "{t}");
    }
    assert_eq!(v.own_pin().await.unwrap().unwrap().1, "0.1.0");
    // The hotfix stays.
    assert_eq!(on(&pool, "spc_2").await.as_deref(), Some("0.3.0"));
    let last = v.pin_history("spc_0").await.unwrap();
    let last = last.last().unwrap();
    assert_eq!(
        (last.cause.as_str(), last.to.as_str()),
        ("rollback", "0.1.0")
    );

    // Again: nothing left to move back, and the hotfix still stays.
    let again = door(
        &ops,
        deploy_ops::ACTIVATE,
        serde_json::json!({"version": "0.1.0"}),
    )
    .await
    .unwrap();
    assert_eq!(again["movedBack"], serde_json::json!([]));
    assert_eq!(on(&pool, "spc_2").await.as_deref(), Some("0.3.0"));

    // While a rollout is open, nothing moves back: the rollback of the
    // active version leaves every pin.
    let open = r
        .start(
            "0.3.0",
            &StartOptions {
                strategy: Some(Strategy::Rolling),
                batch_interval: Some(600),
                batch_size: Some(1),
                ..Default::default()
            },
            OWNER,
            NOW + 100,
        )
        .await
        .unwrap();
    rollouts::tick(&ops, "n", NOW + 100).await.unwrap();
    let during = door(
        &ops,
        deploy_ops::ACTIVATE,
        serde_json::json!({"version": "0.1.0"}),
    )
    .await
    .unwrap();
    assert_eq!(during["movedBack"], serde_json::json!([]));
    let e = r
        .start("0.3.0", &StartOptions::default(), OWNER, NOW + 101)
        .await;
    assert!(matches!(e, Err(RolloutRefusal::Open { .. })), "{e:?}");
    assert_eq!(
        r.get(open.id).await.unwrap().unwrap().state,
        RolloutState::Running
    );
}
