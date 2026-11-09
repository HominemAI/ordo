//! The version settings live in Ordo (owner, 2026-11-05): `versions.served`,
//! `versions.minimum` and `versions.new_tenant` are stored in the control
//! database with who, when and why, and changed through the door.
//!
//! - A change is checked when it is made, and a refusal names the setting
//!   and the value: a version not stored, a served list that strands a
//!   tenant, a minimum above the active version, a revoked new-tenant
//!   version.
//! - A new tenant is pinned to the stored `versions.new_tenant`.
//! - Revoking the version it names is refused, naming the setting.
//! - A value still in the environment is stored once at boot; after that
//!   the variable is ignored, with a line saying so on every boot. A value
//!   the checks refuse is not stored, and boot goes on.
//! - The door op answers the settings and their history, records each
//!   change, and asks no passkey touch below `ORDO_SECURITY_LEVEL=high`.
//!
//! A regression looks like a setting read from the environment again, a
//! change that strands a tenant, or a boot refused over a setting.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_api::BoundParams;
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::deploy_ops;
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::{
    SettingSet, VersionRefusal, VersionSetting, Versions, MINIMUM_ENV, NEW_TENANT_ENV,
    NEW_TENANT_SETTING, SERVED_ENV,
};

const V010: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

const V020: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

const V030: &str = "domain tasks @ 0.3.0

entity task {
  title: text
  done:  bool
  due:   int?
  note:  text?
}

op openTasks unbounded =
  task[title, done]
";

const OWNER: &str = "deployment-owner:ada";

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    tenants::ensure_table(&pool).await.expect("$spaces");
    pool
}

async fn apply(pool: &Pool, source: &str) -> Result<apply_op::Report, apply_op::ApplyRefused> {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
}

async fn tenant(pool: &Pool, id: &str) {
    assert!(tenants::reserve(pool, id, id, "dev:ada", 10, None)
        .await
        .expect("reserved"));
    tenants::activate(pool, id, 11).await.expect("active");
}

/// Three versions stored, `0.3.0` active, `spc_a` moved to `0.1.0`.
async fn three(stem: &str) -> Pool {
    let pool = fresh(stem).await;
    tenant(&pool, "spc_a").await;
    for v in [V010, V020, V030] {
        apply(&pool, v).await.expect("applies");
    }
    Versions::on(&pool)
        .pin("spc_a", "0.1.0", OWNER, 100)
        .await
        .expect("pinned");
    pool
}

/// **Set, cleared, and kept with who, when and why.** `#n` is stored as
/// the version it names; a value already held writes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_setting_is_stored_with_its_history() {
    let pool = three("ordo_vset_history").await;
    let v = Versions::on(&pool);
    assert_eq!(v.settings().await.unwrap(), Default::default());

    let set = v
        .set_setting(VersionSetting::NewTenant, Some("#2"), OWNER, "beta", 200)
        .await
        .expect("set");
    assert!(matches!(set, SettingSet::Changed(_)), "{set:?}");
    assert_eq!(
        v.settings().await.unwrap().new_tenant.as_deref(),
        Some("0.2.0")
    );
    assert_eq!(
        v.new_tenant_version().await.unwrap().as_deref(),
        Some("0.2.0")
    );

    let again = v
        .set_setting(
            VersionSetting::NewTenant,
            Some("0.2.0"),
            OWNER,
            "again",
            201,
        )
        .await
        .expect("same");
    assert!(matches!(again, SettingSet::Unchanged(_)), "{again:?}");

    v.set_setting(
        VersionSetting::Served,
        Some("0.1.0, 0.2.0"),
        OWNER,
        "keep two",
        202,
    )
    .await
    .expect("served");
    v.set_setting(VersionSetting::NewTenant, None, OWNER, "back", 203)
        .await
        .expect("cleared");
    let now = v.settings().await.unwrap();
    assert_eq!(now.new_tenant, None);
    assert_eq!(
        now.policy.served,
        Some(vec!["0.1.0".to_string(), "0.2.0".to_string()])
    );
    v.set_setting(VersionSetting::Served, Some("in-use"), OWNER, "all", 204)
        .await
        .expect("in-use");
    assert_eq!(v.settings().await.unwrap().policy.served, None);

    let h = v.setting_history(10).await.unwrap();
    let rows: Vec<(&str, Option<&str>, &str, u64)> = h
        .iter()
        .map(|c| {
            (
                c.setting.as_str(),
                c.value.as_deref(),
                c.reason.as_str(),
                c.at,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("versions.served", None, "all", 204),
            ("versions.new_tenant", None, "back", 203),
            ("versions.served", Some("0.1.0,0.2.0"), "keep two", 202),
            ("versions.new_tenant", Some("0.2.0"), "beta", 200),
        ]
    );
    assert!(h.iter().all(|c| c.actor == OWNER));
    let e = v
        .set_setting(VersionSetting::Minimum, Some("0.1.0"), OWNER, " ", 205)
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::Reason(_)), "{e}");
}

/// **The checks that ran at boot run on the change**, and each refusal
/// names the setting and the value.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_that_would_break_the_deployment_is_refused_naming_it() {
    let pool = three("ordo_vset_refused").await;
    let v = Versions::on(&pool);

    // A served list that strands `spc_a` on 0.1.0.
    let e = v
        .set_setting(VersionSetting::Served, Some("0.2.0"), OWNER, "trim", 300)
        .await
        .unwrap_err();
    assert!(
        matches!(e, VersionRefusal::Stranded { tenants: 1, .. }),
        "{e}"
    );
    let text = e.to_string();
    assert!(text.contains("versions.served"), "{text}");
    assert!(
        text.contains("ordo versions set served 0.2.0,0.1.0"),
        "{text}"
    );

    // A version not stored.
    let e = v
        .set_setting(VersionSetting::Minimum, Some("9.9.9"), OWNER, "x", 301)
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::Setting { .. }), "{e}");
    for want in [
        "versions.minimum",
        "9.9.9",
        "0.1.0",
        "ordo versions set minimum",
    ] {
        assert!(e.to_string().contains(want), "`{want}` missing: {e}");
    }
    assert_eq!(e.to_exec_error("x").code, ordo_api::ErrorCode::InvalidParam);

    // A minimum above the active version.
    v.activate("0.2.0", OWNER, ordo_commandlog::Timestamp(302))
        .await
        .expect("rolled back");
    let e = v
        .set_setting(VersionSetting::Minimum, Some("#3"), OWNER, "x", 303)
        .await
        .unwrap_err();
    assert!(
        matches!(e, VersionRefusal::MinimumAboveActive { .. }),
        "{e}"
    );
    assert!(
        e.to_string().contains("ordo versions set minimum 0.2.0"),
        "{e}"
    );

    // A new-tenant version not stored, then one the served list leaves out.
    let e = v
        .set_setting(VersionSetting::NewTenant, Some("7.7.7"), OWNER, "x", 304)
        .await
        .unwrap_err();
    assert!(e.to_string().contains(NEW_TENANT_SETTING), "{e}");
    assert!(e.to_string().contains("7.7.7"), "{e}");
    v.set_setting(VersionSetting::Served, Some("0.1.0"), OWNER, "x", 305)
        .await
        .expect("strands nobody: the rollback moved every tenant to 0.2.0, the active one");
    let e = v
        .set_setting(VersionSetting::NewTenant, Some("0.3.0"), OWNER, "x", 306)
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::NotServed { .. }), "{e}");

    // Nothing refused was stored.
    let now = v.settings().await.unwrap();
    assert_eq!(now.policy.minimum, None);
    assert_eq!(now.new_tenant, None);
    assert_eq!(v.setting_history(10).await.unwrap().len(), 1);
}

/// **A new tenant starts on the stored version, and that version cannot be
/// revoked** while the setting names it. A revoked one is refused as the
/// setting.
/// **A pin sees a setting change that raced it** (security audit
/// 2026-11-05, L4). The settings are read under the pin lock a change
/// takes, so a pin queued behind a change that unserves its version is
/// refused, not landed on a version just unserved.
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_queued_behind_a_setting_change_reads_the_change() {
    let pool = three("ordo_vset_race").await;
    let v = Versions::on(&pool);
    v.set_setting(VersionSetting::Served, Some("in-use"), OWNER, "all", 300)
        .await
        .expect("the table");
    tenant(&pool, "spc_b").await;

    // A change holds the lock and has written its row: only 0.1.0, beside
    // the active 0.3.0, is served now.
    let change = pool.begin().await.expect("tx");
    change
        .rows(&format!(
            "SELECT pg_advisory_xact_lock({}, 0)",
            ordo_host::versions::PIN_LOCK_NS
        ))
        .await
        .expect("lock");
    change
        .rows(
            "INSERT INTO \"$version_settings\" (\"setting\", \"value\", \"actor\", \"reason\", \
             \"at\") VALUES ('versions.served', '0.1.0', 'deployment-owner:ada', 'race', 301)",
        )
        .await
        .expect("row");
    let p2 = pool.clone();
    let pin =
        tokio::spawn(async move { Versions::on(&p2).pin("spc_b", "0.2.0", OWNER, 302).await });
    // The pin is queued behind the change (its lock, or its table). Up to
    // 30 s: on a loaded CI runner the spawned pin took over 5 s to open its
    // connection and reach the lock (nightly 2026-11-09, "the pin never
    // queued on the lock"); the bound guards a hang, not the pace.
    let mut waiting = false;
    for _ in 0..1200 {
        let rows = pool
            .rows(
                "SELECT count(*)::text FROM pg_locks l JOIN pg_database d ON d.oid = l.database \
                 WHERE NOT l.granted \
                 AND d.datname = current_database()",
            )
            .await
            .expect("pg_locks");
        if rows[0][0].as_deref() != Some("0") {
            waiting = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(waiting, "the pin never queued on the lock");
    change.commit().await.expect("commit");
    let r = pin.await.expect("task");
    assert!(matches!(r, Err(VersionRefusal::NotServed { .. })), "{r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn new_tenants_start_on_the_stored_version() {
    let pool = three("ordo_vset_new_tenant").await;
    let v = Versions::on(&pool);
    v.set_setting(VersionSetting::NewTenant, Some("0.1.0"), OWNER, "beta", 400)
        .await
        .expect("set");
    tenant(&pool, "spc_new").await;
    assert_eq!(
        v.pin_new_tenant("spc_new", "dev:ada", 401)
            .await
            .unwrap()
            .as_deref(),
        Some("0.1.0")
    );

    let refused = apply_op::revoke(&pool, "0.1.0", "flawed", None, "dev:ada")
        .await
        .expect_err("refused");
    let text = format!("{refused}");
    for want in [NEW_TENANT_SETTING, "0.1.0", "ordo versions set new-tenant"] {
        assert!(text.contains(want), "`{want}` missing from: {text}");
    }
    assert!(!text.contains("ORDO_VERSIONS"), "{text}");

    v.set_setting(VersionSetting::NewTenant, None, OWNER, "done", 402)
        .await
        .expect("cleared");
    apply_op::revoke(&pool, "0.1.0", "flawed", None, "dev:ada")
        .await
        .expect("revoked once the setting no longer names it");
    let e = v
        .set_setting(VersionSetting::NewTenant, Some("0.1.0"), OWNER, "x", 403)
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            VersionRefusal::Revoked {
                setting: Some(_),
                ..
            }
        ),
        "{e}"
    );
}

/// **An environment value is adopted once**, then ignored with a line on
/// every boot. A refused value is not stored and boot goes on.
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_value_is_stored_once_then_ignored() {
    let pool = three("ordo_vset_adopt").await;
    let v = Versions::on(&pool);
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    };

    // Nothing set: nothing said.
    assert!(v.adopt_from(env(&[]), 500).await.is_empty());

    let first = v
        .adopt_from(
            env(&[
                (NEW_TENANT_ENV, "0.2.0"),
                (SERVED_ENV, "0.2.0"),
                (MINIMUM_ENV, ""),
            ]),
            501,
        )
        .await;
    // The served list strands spc_a: refused, not stored, and said so.
    assert_eq!(first.len(), 2, "{first:#?}");
    assert!(
        first.iter().any(|l| l.contains(SERVED_ENV)
            && l.contains("was not stored")
            && l.contains("ordo versions set served")),
        "{first:#?}"
    );
    assert!(
        first.iter().any(|l| l.contains(NEW_TENANT_ENV)
            && l.contains("now stored in Ordo")
            && l.contains("can be removed")),
        "{first:#?}"
    );
    let s = v.settings().await.unwrap();
    assert_eq!(s.new_tenant.as_deref(), Some("0.2.0"));
    assert_eq!(s.policy.served, None);
    let h = v.setting_history(10).await.unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].actor, ordo_host::versions::SYSTEM_ACTOR);
    assert!(h[0].reason.contains(NEW_TENANT_ENV), "{}", h[0].reason);

    // Changed in Ordo, then a second boot with the variable still set: the
    // stored value stands, and the variable is said to be ignored.
    v.set_setting(VersionSetting::NewTenant, Some("0.3.0"), OWNER, "ga", 502)
        .await
        .expect("set");
    let second = v.adopt_from(env(&[(NEW_TENANT_ENV, "0.2.0")]), 503).await;
    assert_eq!(second.len(), 1, "{second:#?}");
    assert!(second[0].contains("set and ignored"), "{}", second[0]);
    assert!(second[0].contains("`0.3.0`"), "{}", second[0]);
    assert_eq!(
        v.settings().await.unwrap().new_tenant.as_deref(),
        Some("0.3.0")
    );
    assert_eq!(v.setting_history(10).await.unwrap().len(), 2);
}

/// **Boot logs a stored problem and refuses nothing.**
#[tokio::test(flavor = "multi_thread")]
async fn boot_logs_a_stored_problem_without_refusing() {
    let pool = three("ordo_vset_boot").await;
    let v = Versions::on(&pool);
    assert!(v.settings_problems().await.is_empty());
    v.set_setting(VersionSetting::Served, Some("0.1.0"), OWNER, "x", 600)
        .await
        .expect("set");
    // A tenant moved onto a version the list leaves out, as an older
    // deployment could have.
    tenant(&pool, "spc_b").await;
    pool.rows("UPDATE \"$spaces\" SET \"version\" = '0.2.0' WHERE \"$space\" = 'spc_b'")
        .await
        .expect("moved by hand");
    let problems = v.settings_problems().await;
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(problems[0].contains("versions.served"), "{}", problems[0]);
}

/// **The door**: the settings and their history, a change recorded on the
/// security stream, a refusal as `invalid_param`, and no passkey touch
/// below `high`.
#[tokio::test(flavor = "multi_thread")]
async fn the_door_reads_and_changes_the_settings() {
    let pool = three("ordo_vset_door").await;
    let stream = std::sync::Arc::new(ordo_secevent::Recorder::default());
    let ops = apply_op::ApplyOps::recording(
        pool.clone(),
        std::sync::Arc::new(SinglePool::new(pool.clone())),
        "home",
        Some(stream.clone()),
    );
    let run = |name: &'static str, space: &'static str, v: serde_json::Value| {
        let ops = &ops;
        let b = BoundParams(v.as_object().unwrap().clone());
        async move { deploy_ops::execute(ops, name, space, &b, "sys:updater").await }
    };

    let r = run(
        deploy_ops::SET_VERSION_SETTING,
        "home",
        serde_json::json!({"setting": "minimum", "value": "#1", "reason": "floor"}),
    )
    .await
    .expect("set");
    assert_eq!(r.rows[0]["setting"], "versions.minimum");
    assert_eq!(r.rows[0]["value"], "0.1.0");
    assert_eq!(r.rows[0]["minimum"], "0.1.0");
    assert_eq!(r.rows[0]["changed"], true);
    assert_eq!(r.rows[0]["served"], serde_json::Value::Null);

    let e = run(
        deploy_ops::SET_VERSION_SETTING,
        "home",
        serde_json::json!({"setting": "served", "value": "0.3.0", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::InvalidParam);
    assert!(e.message.contains("versions.served"), "{}", e.message);
    let e = run(
        deploy_ops::SET_VERSION_SETTING,
        "home",
        serde_json::json!({"setting": "versions.max", "value": "1", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::InvalidParam);

    let r = run(deploy_ops::VERSION_SETTINGS, "home", serde_json::json!({}))
        .await
        .expect("read");
    assert_eq!(r.rows[0]["minimum"], "0.1.0");
    assert_eq!(r.rows[0]["newTenant"], serde_json::Value::Null);
    let h = r.rows[0]["history"].as_array().unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0]["actor"], "sys:updater");
    assert_eq!(h[0]["reason"], "floor");

    // Bound to a tenant, neither op is there.
    for op in [
        deploy_ops::VERSION_SETTINGS,
        deploy_ops::SET_VERSION_SETTING,
    ] {
        let e = run(op, "spc_a", serde_json::json!({})).await.unwrap_err();
        assert_eq!(e.code, ordo_api::ErrorCode::NotFound, "{op}");
    }
    use ordo_host::security_level::SecurityLevel;
    let presence = ordo_host::presence::PresencePolicy::standard();
    let write = ordo_api::OpKind::Write;
    assert!(!presence.requires(deploy_ops::SET_VERSION_SETTING, write));
    assert!(!presence
        .clone()
        .with_level(SecurityLevel::Low)
        .requires(deploy_ops::SET_VERSION_SETTING, write));
    assert!(presence
        .with_level(SecurityLevel::High)
        .requires(deploy_ops::SET_VERSION_SETTING, write));

    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(acts, ["version-setting"]);
}
