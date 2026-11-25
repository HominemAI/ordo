//! **The input caps are settings** (owner, 2026-11-25,
//! `settings-3-choices-left-after-the` ask 3, *"yes"*). A version's label
//! (80), a reason (500), a tenant build's name (40), the tenants of a
//! rollout group (1,000) and a rollout's query (2,000 characters) were
//! constants. Each is now `limits.<name>`, stored in `$version_settings`
//! with who and why, set through the door (`std.deployment.setVersionSetting`,
//! `ordo versions set`).
//!
//! One test for each setting, on a real server: the default refuses one
//! over today's number and names the setting, a lowered value refuses what
//! it allowed, a raised value allows what it refused, and clearing it is the
//! default again. A bad value is refused naming the setting, and nothing is
//! stored.
//!
//! A regression looks like a cap set and ignored, or a refusal that names no
//! setting.
//!
//! The caps in force are one set for the process, so the tests take a lock.
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_api::{BoundParams, ErrorCode, ExecError};
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, ApplyOps};
use ordo_host::deploy_ops;
use ordo_host::input_caps::{self, InputCaps};
use ordo_host::tenant_builds;
use ordo_host::tenants;

static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const V010: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

async fn deployment(stem: &str) -> (Pool, std::sync::Arc<ApplyOps>) {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    apply_op::run(
        &pool,
        &SinglePool::new(pool.clone()),
        V010,
        &Default::default(),
        "dev:ada",
        "t",
        true,
    )
    .await
    .expect("applies");
    let ops = ApplyOps::recording(
        pool.clone(),
        std::sync::Arc::new(SinglePool::new(pool.clone())),
        "home",
        None,
    );
    (pool, ops)
}

async fn door(
    ops: &ApplyOps,
    name: &str,
    v: serde_json::Value,
) -> Result<ordo_api::ExecResult, ExecError> {
    let b = BoundParams(v.as_object().unwrap().clone());
    if tenant_builds::OPS.contains(&name) {
        return tenant_builds::execute(ops, name, "home", &b, "sys:updater").await;
    }
    deploy_ops::execute(ops, name, "home", &b, "sys:updater").await
}

/// Set a cap through the door, with a short reason.
async fn set(ops: &ApplyOps, name: &str, value: Option<&str>) -> Result<(), ExecError> {
    door(
        ops,
        deploy_ops::SET_VERSION_SETTING,
        serde_json::json!({"setting": name, "value": value, "reason": "t"}),
    )
    .await
    .map(|_| ())
}

fn names(e: &ExecError, setting: &str) {
    assert_eq!(e.code, ErrorCode::InvalidParam, "{}", e.message);
    assert!(e.message.contains(setting), "{}", e.message);
}

/// A bad value is refused naming the setting and the command; nothing is
/// stored, so the caps in force are the defaults.
#[tokio::test(flavor = "multi_thread")]
async fn a_bad_value_is_refused_naming_the_setting() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (pool, ops) = deployment("ordo_caps_bad").await;
    for (setting, bad) in [
        (input_caps::LABEL_CHARS_SETTING, "0"),
        (input_caps::REASON_CHARS_SETTING, "lots"),
        (input_caps::BUILD_NAME_CHARS_SETTING, "-3"),
        (input_caps::ROLLOUT_GROUP_SETTING, "0"),
        (input_caps::QUERY_CHARS_SETTING, "999999999"),
    ] {
        let e = set(&ops, setting, Some(bad)).await.unwrap_err();
        names(&e, setting);
        assert!(e.message.contains("ordo versions set"), "{}", e.message);
    }
    let e = set(&ops, "limits.nope", Some("5")).await.unwrap_err();
    names(&e, "limits.label_chars");
    assert_eq!(
        input_caps::refresh(&pool).await.unwrap(),
        InputCaps::default()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_label_cap_is_a_setting() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (_pool, ops) = deployment("ordo_caps_label").await;
    let label = |n: usize| {
        let l = "x".repeat(n);
        let ops = &ops;
        async move {
            door(
                ops,
                deploy_ops::LABEL,
                serde_json::json!({"version": "0.1.0", "label": l}),
            )
            .await
        }
    };
    label(80).await.expect("80 is today's cap");
    names(&label(81).await.unwrap_err(), "limits.label_chars");
    set(&ops, "limits.label_chars", Some("10")).await.unwrap();
    names(&label(11).await.unwrap_err(), "limits.label_chars");
    label(10).await.expect("10 holds");
    set(&ops, "limits.label_chars", Some("200")).await.unwrap();
    label(150).await.expect("a raised cap allows it");
    set(&ops, "limits.label_chars", None).await.unwrap();
    names(&label(81).await.unwrap_err(), "limits.label_chars");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reason_cap_is_a_setting() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (_pool, ops) = deployment("ordo_caps_reason").await;
    let with = |n: usize| {
        let reason = "r".repeat(n);
        let ops = &ops;
        async move {
            door(
                ops,
                deploy_ops::SET_VERSION_SETTING,
                serde_json::json!({
                    "setting": "versions.new_tenant", "value": "0.1.0", "reason": reason
                }),
            )
            .await
        }
    };
    with(500).await.expect("500 is today's cap");
    names(&with(501).await.unwrap_err(), "limits.reason_chars");
    set(&ops, "limits.reason_chars", Some("5")).await.unwrap();
    names(&with(6).await.unwrap_err(), "limits.reason_chars");
    set(&ops, "limits.reason_chars", Some("900")).await.unwrap();
    with(800).await.expect("a raised cap allows it");
    set(&ops, "limits.reason_chars", None).await.unwrap();
    names(&with(501).await.unwrap_err(), "limits.reason_chars");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_build_name_cap_is_a_setting() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (_pool, ops) = deployment("ordo_caps_build").await;
    let build = |n: usize| {
        let name = "b".repeat(n);
        let ops = &ops;
        async move {
            door(
                ops,
                tenant_builds::SET_BUILD,
                serde_json::json!({"name": name, "version": "0.1.0", "reason": "t"}),
            )
            .await
        }
    };
    build(40).await.expect("40 is today's cap");
    names(&build(41).await.unwrap_err(), "limits.build_name_chars");
    set(&ops, "limits.build_name_chars", Some("6"))
        .await
        .unwrap();
    names(&build(7).await.unwrap_err(), "limits.build_name_chars");
    set(&ops, "limits.build_name_chars", Some("60"))
        .await
        .unwrap();
    build(55).await.expect("a raised cap allows it");
    set(&ops, "limits.build_name_chars", None).await.unwrap();
    names(&build(41).await.unwrap_err(), "limits.build_name_chars");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rollout_group_cap_is_a_setting() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (_pool, ops) = deployment("ordo_caps_group").await;
    let group = |n: usize| {
        let list = (0..n)
            .map(|i| format!("spc_{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let ops = &ops;
        async move { set(ops, "rollout.group", Some(&list)).await }
    };
    group(1_000).await.expect("1,000 is today's cap");
    names(&group(1_001).await.unwrap_err(), "limits.rollout_group");
    set(&ops, "limits.rollout_group", Some("3")).await.unwrap();
    names(&group(4).await.unwrap_err(), "limits.rollout_group");
    set(&ops, "limits.rollout_group", Some("1500"))
        .await
        .unwrap();
    group(1_200).await.expect("a raised cap allows it");
    set(&ops, "limits.rollout_group", None).await.unwrap();
    names(&group(1_001).await.unwrap_err(), "limits.rollout_group");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_query_cap_is_a_setting() {
    let _one = ONE_AT_A_TIME.lock().await;
    let (_pool, ops) = deployment("ordo_caps_query").await;
    let query = |n: usize| {
        // `members < 5 or members < 5 ...` of `n` characters, the last
        // number padded with zeros.
        let mut q = String::from("members < 5");
        while q.len() + " or members < 5".len() <= n {
            q.push_str(" or members < 5");
        }
        while q.len() < n {
            q.push('0');
        }
        let ops = &ops;
        async move { set(ops, "rollout.query", Some(&q)).await }
    };
    query(2_000).await.expect("2,000 is today's cap");
    names(&query(2_001).await.unwrap_err(), "limits.query_chars");
    set(&ops, "limits.query_chars", Some("100")).await.unwrap();
    names(&query(101).await.unwrap_err(), "limits.query_chars");
    set(&ops, "limits.query_chars", Some("3000")).await.unwrap();
    query(2_500).await.expect("a raised cap allows it");
    set(&ops, "limits.query_chars", None).await.unwrap();
    names(&query(2_001).await.unwrap_err(), "limits.query_chars");
}
