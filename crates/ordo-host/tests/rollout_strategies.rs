//! Increment 10c of `plan-versions.md`: **rollout strategies** (§3.5,
//! §3.7; owner rulings of 2026-11-23, Q3 and Q5).
//!
//! What these pin, over real Postgres:
//!
//! - a canary moves first, alone; a canary that fails its health check
//!   pauses the rest, and every node's counts add up to that judgement;
//!   `resume` moves the rest anyway, and the alert mail is queued;
//! - a healthy canary waits `rollout.canary_wait`, then the rest roll, and
//!   "active" flips only at the finish;
//! - group first and by query move their tenants first, in order;
//! - share partners move in the same batch;
//! - the refusals: a group naming a stranger, a strategy with no group or
//!   query, a query over a fact there is not, a bad rate or channel.
//!
//! A regression looks like the rest moving while a canary is unhealthy, a
//! rollout finishing before its first stage was judged, a partner left a
//! batch behind, or a query that reads anything but the facts.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::collections::BTreeMap;
use std::sync::Arc;

use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, ApplyOps};
use ordo_host::rollout_strategies as strategies;
use ordo_host::rollouts::{
    self, Gate, RolloutRefusal, RolloutState, Rollouts, StartOptions, Strategy, TenantState,
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

const NOW: u64 = 1_762_344_000;
const OWNER: &str = "deployment-owner:ada";

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.to_string(),
        size: 8,
        ..PoolConfig::from_env()
    }
}

async fn run(pool: &Pool, source: &str, activate: bool) {
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
    .expect("applies");
}

/// `tenants` on 0.1.0, and 0.2.0 stored.
async fn deployment(stem: &str, tenants: &[&str]) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(cfg(&pg.db)).await.expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    for t in tenants {
        assert!(tenants::reserve(&pool, t, t, "dev:ada", 10, None)
            .await
            .expect("reserved"));
        tenants::activate(&pool, t, 11).await.expect("active");
    }
    run(&pool, V1, true).await;
    run(&pool, V2, false).await;
    pool
}

fn ops_for(pool: &Pool) -> (Arc<ApplyOps>, Arc<ordo_secevent::Recorder>) {
    let stream = Arc::new(ordo_secevent::Recorder::default());
    (
        ApplyOps::recording(
            pool.clone(),
            Arc::new(SinglePool::new(pool.clone())),
            "home",
            Some(stream.clone()),
        ),
        stream,
    )
}

fn acts(stream: &ordo_secevent::Recorder) -> Vec<String> {
    stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect()
}

async fn on(pool: &Pool, id: &str) -> String {
    Versions::on(pool)
        .pin_of(id)
        .await
        .expect("read")
        .expect("a tenant")
        .version
        .expect("pinned")
}

fn counts(pairs: &[(&str, u64, u64)]) -> BTreeMap<String, (u64, u64)> {
    pairs
        .iter()
        .map(|(s, c, f)| (s.to_string(), (*c, *f)))
        .collect()
}

/// **A canary that fails its health check pauses the rest** (§4, the
/// increment's "done when"). One tenant moves alone. Two nodes count its
/// calls; one alone is under 1%, the two together reach it, and the
/// leader pauses with the rest on 0.1.0, naming the setting, `resume` and
/// `abort`. The alert mail is queued. Time passing moves nothing. `resume`
/// is the deployment owner moving the rest anyway.
#[tokio::test(flavor = "multi_thread")]
async fn a_canary_that_fails_its_health_check_pauses_the_rest() {
    let pool = deployment("ordo_rs_canary_sick", &["spc_a", "spc_b", "spc_c"]).await;
    ordo_host::mail::ensure_table(&pool).await.expect("outbox");
    let r = Rollouts::on(&pool);
    let started = r
        .start(
            "0.2.0",
            &StartOptions {
                strategy: Some(Strategy::Canary),
                canary_wait: Some(600),
                batch_interval: Some(60),
                batch_size: Some(2),
                alert_channel: Some("mail:ops@example.com".into()),
                ..Default::default()
            },
            OWNER,
            NOW,
        )
        .await
        .expect("started");
    assert_eq!(started.gate, Gate::Pending);
    assert_eq!(started.selection, "canary: 1");
    let (ops, stream) = ops_for(&pool);
    let t = rollouts::tick(&ops, "node-a", NOW).await.unwrap();
    assert_eq!(t.moved.len(), 1, "the canary moves alone: {t:?}");
    let canary = t.moved[0].clone();
    let rest: Vec<String> = ["spc_a", "spc_b", "spc_c"]
        .iter()
        .map(|s| s.to_string())
        .filter(|s| *s != canary)
        .collect();
    assert_eq!(on(&pool, &canary).await, "0.2.0");
    for t in &rest {
        assert_eq!(on(&pool, t).await, "0.1.0", "{t}");
    }

    // Each node's first report is its baseline; the next adds the rest.
    let c = canary.as_str();
    for node in ["node-a", "node-b"] {
        strategies::report(&pool, node, &counts(&[(c, 40, 0)]), NOW + 1)
            .await
            .unwrap();
    }
    strategies::report(&pool, "node-a", &counts(&[(c, 140, 0)]), NOW + 2)
        .await
        .unwrap();
    let t = rollouts::tick(&ops, "node-a", NOW + 3).await.unwrap();
    assert!(!t.paused && t.moved.is_empty(), "healthy so far: {t:?}");
    // node-b: 100 calls, 2 server faults. 2 of 200 is 1%.
    strategies::report(&pool, "node-b", &counts(&[(c, 140, 2)]), NOW + 4)
        .await
        .unwrap();
    let s = r.status(Some(started.id)).await.unwrap().unwrap();
    assert_eq!(s.health, (200, 2));
    let t = rollouts::tick(&ops, "node-a", NOW + 5).await.unwrap();
    assert!(t.paused && t.unhealthy, "{t:?}");
    assert!(t.moved.is_empty(), "{t:?}");
    let paused = r.get(started.id).await.unwrap().unwrap();
    assert_eq!(paused.state, RolloutState::Paused);
    for word in [
        "rollout.health_error_rate",
        "ordo rollout resume",
        "ordo rollout abort",
        "2 of 200",
    ] {
        assert!(paused.reason.contains(word), "{word}: {}", paused.reason);
    }
    let mail = pool
        .rows("SELECT \"kind\", \"recipient\", \"payload\" FROM \"$mailoutbox\"")
        .await
        .unwrap();
    assert_eq!(mail.len(), 1, "{mail:?}");
    assert_eq!(mail[0][0].as_deref(), Some(strategies::ALERT_KIND));
    assert_eq!(mail[0][1].as_deref(), Some("ops@example.com"));
    assert!(mail[0][2]
        .as_deref()
        .unwrap_or_default()
        .contains("unhealthy"));

    // Paused: the wait running out moves nothing.
    let t = rollouts::tick(&ops, "node-a", NOW + 4_000).await.unwrap();
    assert!(t.moved.is_empty(), "{t:?}");
    for t in &rest {
        assert_eq!(on(&pool, t).await, "0.1.0", "{t}");
    }
    assert_eq!(
        Versions::on(&pool)
            .active_version()
            .await
            .unwrap()
            .as_deref(),
        Some("0.1.0")
    );

    // `resume`: the deployment owner moves the rest anyway.
    let resumed = r.resume(NOW + 4_001).await.unwrap();
    assert_eq!(resumed.gate, Gate::Passed);
    let t = rollouts::tick(&ops, "node-a", NOW + 4_002).await.unwrap();
    assert_eq!(t.moved.len(), 2, "{t:?}");
    assert!(t.finished, "{t:?}");
    assert_eq!(
        acts(&stream),
        ["rollout-paused", "rollout-finished"],
        "the pause and the finish are on the stream"
    );
}

/// **A healthy canary waits, then the rest roll.** The gate holds for
/// `rollout.canary_wait` after the canary moved; "active" stays 0.1.0
/// until the finish (Q5).
#[tokio::test(flavor = "multi_thread")]
async fn a_healthy_canary_waits_then_the_rest_roll() {
    let pool = deployment("ordo_rs_canary_well", &["spc_a", "spc_b", "spc_c"]).await;
    let r = Rollouts::on(&pool);
    r.set_setting(
        "rollout.strategy",
        Some("canary"),
        OWNER,
        "canary first",
        NOW,
    )
    .await
    .unwrap();
    r.set_setting("canary-wait", Some("10m"), OWNER, "soak", NOW)
        .await
        .unwrap();
    r.set_setting(
        "rollout.health_error_rate",
        Some("5%"),
        OWNER,
        "lenient",
        NOW,
    )
    .await
    .unwrap();
    let s = r.settings().await.unwrap();
    assert_eq!((s.canary_wait, s.health_error_rate), (600, 500));
    let started = r
        .start(
            "0.2.0",
            &StartOptions {
                batch_size: Some(1),
                batch_interval: Some(60),
                ..Default::default()
            },
            OWNER,
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(started.strategy, Strategy::Canary);
    let (ops, stream) = ops_for(&pool);
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert_eq!(t.moved.len(), 1);
    let c = t.moved[0].clone();
    strategies::report(&pool, "n", &counts(&[(&c, 0, 0)]), NOW + 1)
        .await
        .unwrap();
    strategies::report(&pool, "n", &counts(&[(&c, 100, 3)]), NOW + 2)
        .await
        .unwrap();
    // 3% is under 5%: healthy, and the wait has not run out.
    let t = rollouts::tick(&ops, "n", NOW + 300).await.unwrap();
    assert!(t.moved.is_empty() && !t.paused && !t.gate_passed, "{t:?}");
    let first = r.get(started.id).await.unwrap().unwrap();
    assert!(first.first_done_at > 0);
    let t = rollouts::tick(&ops, "n", first.first_done_at + 600)
        .await
        .unwrap();
    assert!(t.gate_passed, "{t:?}");
    assert_eq!(t.moved.len(), 1, "the rest roll, a batch at a time: {t:?}");
    assert_eq!(
        Versions::on(&pool)
            .active_version()
            .await
            .unwrap()
            .as_deref(),
        Some("0.1.0")
    );
    let t = rollouts::tick(&ops, "n", first.first_done_at + 661)
        .await
        .unwrap();
    assert_eq!(t.moved.len(), 1);
    assert!(t.finished, "{t:?}");
    assert_eq!(
        Versions::on(&pool)
            .active_version()
            .await
            .unwrap()
            .as_deref(),
        Some("0.2.0")
    );
    assert_eq!(
        acts(&stream),
        ["rollout-first-stage-healthy", "rollout-finished"]
    );
}

/// **Group first and by query** move their tenants first, in order; then
/// the rest once the first stage is healthy (wait 0 here).
#[tokio::test(flavor = "multi_thread")]
async fn group_first_and_by_query_move_their_tenants_first() {
    let pool = deployment("ordo_rs_group", &["spc_a", "spc_b", "spc_c", "spc_d"]).await;
    let r = Rollouts::on(&pool);
    let started = r
        .start(
            "0.2.0",
            &StartOptions {
                strategy: Some(Strategy::Group),
                group: Some(vec!["spc_d".into(), "spc_b".into()]),
                canary_wait: Some(0),
                batch_size: Some(10),
                ..Default::default()
            },
            OWNER,
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(started.selection, "group: spc_d,spc_b");
    let (ops, _) = ops_for(&pool);
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert_eq!(t.moved, ["spc_d", "spc_b"], "the group, in its order");
    let t = rollouts::tick(&ops, "n", NOW + 1).await.unwrap();
    assert!(t.gate_passed, "{t:?}");
    assert_eq!(t.moved, ["spc_a", "spc_c"]);
    assert!(t.finished);

    // By query: members seeded on the deployment's database. The facts are
    // Ordo's; the query reads nothing else.
    let pool = deployment("ordo_rs_query", &["spc_a", "spc_b", "spc_c", "spc_d"]).await;
    pool.rows(
        r#"INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
          ('spc_a', 'mem_a1', 'p1', 'A1', 'principal', 1, 'owner', 'active', DATE '2026-01-01'),
          ('spc_a', 'mem_a2', 'p2', 'A2', 'principal', 1, 'editor', 'active', DATE '2026-01-01'),
          ('spc_a', 'mem_a3', 'p3', 'A3', 'principal', 1, 'editor', 'active', DATE '2026-01-01'),
          ('spc_c', 'mem_c1', 'p4', 'C1', 'principal', 1, 'owner', 'active', DATE '2026-01-01'),
          ('spc_d', 'mem_d1', 'p5', 'D1', 'principal', 1, 'owner', 'active', DATE '2026-01-01'),
          ('spc_d', 'mem_d2', 'p6', 'D2', 'principal', 1, 'editor', 'active', DATE '2026-01-01')"#,
    )
    .await
    .expect("members");
    let r = Rollouts::on(&pool);
    let started = r
        .start(
            "0.2.0",
            &StartOptions {
                strategy: Some(Strategy::Query),
                query: Some("members > 0 and tenant != \"spc_a\"".into()),
                order: Some("members desc".into()),
                canary_wait: Some(0),
                batch_size: Some(1),
                batch_interval: Some(60),
                ..Default::default()
            },
            OWNER,
            NOW,
        )
        .await
        .unwrap();
    let (ops, _) = ops_for(&pool);
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert_eq!(t.moved, ["spc_d"], "2 members first");
    let t = rollouts::tick(&ops, "n", NOW + 61).await.unwrap();
    assert_eq!(t.moved, ["spc_c"]);
    let s = r.status(Some(started.id)).await.unwrap().unwrap();
    assert!(
        s.tenants
            .iter()
            .filter(|t| t.state == TenantState::Moved)
            .all(|t| t.stage == 0),
        "{:?}",
        s.tenants
    );
    // The first stage is done; the rest roll in tenant order.
    let t = rollouts::tick(&ops, "n", NOW + 62).await.unwrap();
    assert!(t.gate_passed, "{t:?}");
    assert_eq!(t.moved, ["spc_a"]);
}

/// **Share partners move together.** spc_a holds a grant to a person who
/// is a member of spc_c, so spc_c joins spc_a's batch, though the batch is
/// 1 and spc_b is ahead of it.
#[tokio::test(flavor = "multi_thread")]
async fn share_partners_move_in_the_same_batch() {
    let pool = deployment("ordo_rs_partners", &["spc_a", "spc_b", "spc_c"]).await;
    pool.rows(ordo_host::grants::GRANT_DDL)
        .await
        .expect("grants");
    pool.rows(ordo_host::grants::GRANT_PIN_DDL)
        .await
        .expect("pin");
    pool.rows(
        r#"INSERT INTO "members" ("$space", "id", "principal", "displayName", "kind", "memberCount", "role", "status", "joined") VALUES
          ('spc_c', 'mem_c1', 'prn_cleo', 'Cleo', 'principal', 1, 'owner', 'active', DATE '2026-01-01')"#,
    )
    .await
    .expect("a member");
    pool.rows(&format!(
        r#"INSERT INTO "$capabilitygrant" ("$space", "grant_id", "op", "principal", "manifest_hash", "granted_at", "expires")
           VALUES ('spc_a', 'g1', 'openTasks', 'prn_cleo', 'h', 1, {})"#,
        NOW + 86_400
    ))
    .await
    .expect("a grant");
    let partners = strategies::partners(&pool, NOW).await.unwrap();
    assert_eq!(
        partners
            .get("spc_c")
            .map(|p| p.iter().cloned().collect::<Vec<_>>()),
        Some(vec!["spc_a".to_string()])
    );
    let r = Rollouts::on(&pool);
    r.start(
        "0.2.0",
        &StartOptions {
            strategy: Some(Strategy::Rolling),
            batch_size: Some(1),
            batch_interval: Some(60),
            share_partners: Some(true),
            ..Default::default()
        },
        OWNER,
        NOW,
    )
    .await
    .unwrap();
    let (ops, _) = ops_for(&pool);
    let t = rollouts::tick(&ops, "n", NOW).await.unwrap();
    assert_eq!(t.moved, ["spc_a", "spc_c"], "{t:?}");
    assert_eq!(on(&pool, "spc_b").await, "0.1.0");
    // A revoked grant is no partnership.
    pool.rows("UPDATE \"$capabilitygrant\" SET \"revoked_at\" = 2")
        .await
        .unwrap();
    assert!(strategies::partners(&pool, NOW).await.unwrap().is_empty());
}

/// **The refusals** name the flag or setting that fixes them.
#[tokio::test(flavor = "multi_thread")]
async fn a_strategy_without_its_input_is_refused_naming_the_fix() {
    let pool = deployment("ordo_rs_refused", &["spc_a", "spc_b"]).await;
    let r = Rollouts::on(&pool);
    let start =
        |opts: StartOptions| async move { r.start("0.2.0", &opts, OWNER, NOW).await.unwrap_err() };
    let e = start(StartOptions {
        strategy: Some(Strategy::Group),
        ..Default::default()
    })
    .await;
    assert!(e.to_string().contains("--group"), "{e}");
    let e = start(StartOptions {
        strategy: Some(Strategy::Group),
        group: Some(vec!["spc_a".into(), "spc_nobody".into()]),
        ..Default::default()
    })
    .await;
    assert!(matches!(e, RolloutRefusal::Tenant { .. }), "{e}");
    assert!(e.to_string().contains("std.deployment.tenants"), "{e}");
    let e = start(StartOptions {
        strategy: Some(Strategy::Query),
        ..Default::default()
    })
    .await;
    assert!(e.to_string().contains("--query"), "{e}");
    let e = start(StartOptions {
        strategy: Some(Strategy::Query),
        query: Some("seats > 3".into()),
        ..Default::default()
    })
    .await;
    assert!(
        e.to_string().contains("not a tenant fact") && e.to_string().contains("members"),
        "{e}"
    );
    assert_eq!(e.to_exec_error("x").code, ordo_api::ErrorCode::InvalidParam);
    let e = start(StartOptions {
        strategy: Some(Strategy::Canary),
        alert_channel: Some("slack".into()),
        ..Default::default()
    })
    .await;
    assert!(e.to_string().contains("mail:<address>"), "{e}");
    // Settings are checked before they are stored.
    for (setting, value) in [
        ("rollout.health_error_rate", "0"),
        ("rollout.query", "name matches \"x\""),
        ("rollout.order", "seats desc"),
        ("rollout.canary_count", "0"),
        ("rollout.share_partners", "maybe"),
        ("rollout.alert_channel", "mail:nope"),
    ] {
        let e = r
            .set_setting(setting, Some(value), OWNER, "try", NOW)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("ordo rollout set"), "{setting}: {e}");
    }
    // Nothing started.
    assert!(r.open().await.unwrap().is_none());
}

/// **Through the door**: the 10c settings are set and read with
/// `setVersionSetting`, and a start takes the strategy's inputs. A bad
/// rate is refused as bad input before anything starts.
#[tokio::test(flavor = "multi_thread")]
async fn the_door_sets_the_strategy_settings_and_starts_a_query() {
    let pool = deployment("ordo_rs_door", &["spc_a", "spc_b", "spc_c"]).await;
    let (ops, _) = ops_for(&pool);
    let run = |name: &'static str, v: serde_json::Value| {
        let ops = &ops;
        let b = ordo_api::BoundParams(v.as_object().unwrap().clone());
        async move { ordo_host::deploy_ops::execute(ops, name, "home", &b, "sys:updater").await }
    };
    let set = run(
        ordo_host::deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({"setting": "rollout.alert_channel", "value": "mail:ops@example.com",
                           "reason": "the on-call box"}),
    )
    .await
    .expect("set");
    assert_eq!(set.rows[0]["value"], "mail:ops@example.com");
    assert_eq!(
        set.rows[0]["rollout"]["alertChannel"],
        "mail:ops@example.com"
    );
    let e = run(
        ordo_host::deploy_ops::ROLLOUT_START,
        serde_json::json!({"version": "0.2.0", "strategy": "canary", "healthErrorRate": "lots"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::InvalidParam, "{e:?}");
    assert!(e.message.contains("rollout.health_error_rate"), "{e:?}");
    let started = run(
        ordo_host::deploy_ops::ROLLOUT_START,
        serde_json::json!({"version": "0.2.0", "strategy": "by-query",
                           "query": "tenant starts \"spc_\" and not personal",
                           "order": "tenant desc", "canaryWait": "0s", "sharePartners": true}),
    )
    .await
    .expect("started");
    let row = &started.rows[0];
    assert_eq!(row["strategy"], "query");
    assert_eq!(row["gate"], "pending");
    assert_eq!(row["alertChannel"], "mail:ops@example.com");
    assert_eq!(row["sharePartners"], true);
    assert_eq!(row["healthErrorRate"], "1%");
    let first = row["tenants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["firstStage"] == true)
        .count();
    assert_eq!(first, 3, "{row}");
    assert_eq!(row["health"]["healthy"], true);
}
