//! Increment 3 of `plan-versions.md`, session 2: tenants move off a revoked
//! version in their maintenance window or on demand (§1.11, owner rulings
//! of 2026-11-05).
//!
//! - In a window: the sweep moves a tenant whose window holds now, and no
//!   other. A window ahead, a window for another tenant, or a skipped time
//!   moves nothing.
//! - No window and no demand: the tenant stays.
//! - On demand, at the door: the deployment owner or a system moves one
//!   tenant now, to the target the developer chose.
//! - The target: the tenant's own, else the revoke's `--move-to`, else the
//!   active version. A revoked target is refused, never passed over.
//!
//! A regression looks like a tenant moved with no window and no demand, a
//! tenant moved to a version nobody named, or a move onto a revoked version.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_api::BoundParams;
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::deploy_ops;
use ordo_host::revoked_moves;
use ordo_host::schema_apply::Program;
use ordo_host::tenants;
use ordo_host::versions::{PinCause, ServedPolicy, TargetSource, VersionRefusal, Versions};
use ordo_host::windows::{Schedule, Target, WindowSpec, Windows};

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

/// The test's clock: 2026-11-05 12:00 UTC.
const NOW: u64 = 1_762_344_000;
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

async fn apply(pool: &Pool, source: &str) {
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
    .expect("applies");
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

/// Three versions, `0.3.0` active. `spc_a` and `spc_b` are moved to
/// `0.1.0`, which is then revoked; `spc_c` stays on `0.3.0`.
async fn deployment(stem: &str, move_to: Option<&str>) -> Pool {
    let pool = fresh(stem).await;
    for t in ["spc_a", "spc_b", "spc_c"] {
        tenant(&pool, t).await;
    }
    apply(&pool, V010).await;
    apply(&pool, V020).await;
    apply(&pool, V030).await;
    let v = Versions::on(&pool);
    for t in ["spc_a", "spc_b"] {
        v.pin(t, "0.1.0", OWNER, 100).await.expect("pinned");
    }
    v.revoke("0.1.0", "leaks a field", move_to, OWNER, 200)
        .await
        .expect("revoked");
    assert_eq!(on(&pool, "spc_a").await.as_deref(), Some("0.1.0"));
    pool
}

async fn window(pool: &Pool, target: Target, schedule: Schedule) -> String {
    Windows::on(pool)
        .set(
            &WindowSpec {
                id: None,
                target,
                schedule,
                message: "upgrades".into(),
            },
            OWNER,
            NOW as i64 - 7_200,
        )
        .await
        .expect("a window")
        .id
}

fn only(t: &str) -> Target {
    Target::Only([t.to_string()].into_iter().collect())
}

/// **No window and no demand: the tenant stays** (ruled 2026-11-05, no
/// deadline). A window ahead, or one for another tenant, moves nothing
/// either.
#[tokio::test(flavor = "multi_thread")]
async fn no_window_and_no_demand_moves_nothing() {
    let pool = deployment("ordo_vmove_stay", None).await;
    let swept = revoked_moves::sweep(&pool, NOW).await.expect("swept");
    assert!(swept.moved.is_empty() && swept.refused.is_empty());

    window(
        &pool,
        Target::All,
        Schedule::Range {
            start: NOW as i64 + 3_600,
            end: NOW as i64 + 7_200,
        },
    )
    .await;
    window(
        &pool,
        only("spc_c"),
        Schedule::Range {
            start: NOW as i64 - 60,
            end: NOW as i64 + 60,
        },
    )
    .await;
    let swept = revoked_moves::sweep(&pool, NOW).await.expect("swept");
    assert!(swept.moved.is_empty(), "{swept:?}");
    for t in ["spc_a", "spc_b"] {
        assert_eq!(on(&pool, t).await.as_deref(), Some("0.1.0"), "{t} stays");
    }
    assert_eq!(on(&pool, "spc_c").await.as_deref(), Some("0.3.0"));

    // The report says when each would move: the window ahead.
    let mut list = Versions::on(&pool).pinned_to_revoked().await.unwrap();
    revoked_moves::note_next_windows(&pool, &mut list, NOW).await;
    assert_eq!(list[0].tenants[0].next_window, Some(NOW as i64 + 3_600));
    let json = deploy_ops::revoked_json(&list[0]);
    assert_eq!(json["tenants"][0]["nextWindow"], NOW + 3_600);
}

/// **In the window**: the sweep moves the tenant its window holds, to the
/// active version by default, cause `window`; the tenant the window does
/// not cover stays. A skipped time moves nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_moves_in_its_window_and_not_before() {
    let pool = deployment("ordo_vmove_window", None).await;
    // A cron window every day at 11:00 for 2 hours, skipped today.
    let id = window(
        &pool,
        only("spc_a"),
        Schedule::Cron {
            cron: "0 11 * * *".into(),
            length: 7_200,
        },
    )
    .await;
    Windows::on(&pool)
        .skip(
            &id,
            None,
            NOW as i64 - 3_600,
            NOW as i64,
            Some("launch day"),
            OWNER,
            NOW as i64,
        )
        .await
        .expect("skipped");
    let swept = revoked_moves::sweep(&pool, NOW).await.expect("swept");
    assert!(swept.moved.is_empty(), "skipped today: {swept:?}");

    // Tomorrow's time holds.
    let tomorrow = NOW + 86_400;
    let swept = revoked_moves::sweep(&pool, tomorrow).await.expect("swept");
    assert_eq!(swept.moved.len(), 1, "{swept:?}");
    let (m, w) = &swept.moved[0];
    assert_eq!(w, &id);
    assert_eq!(m.change.space, "spc_a");
    assert_eq!(m.change.to, "0.3.0");
    assert_eq!(m.source, TargetSource::Active);
    assert_eq!(on(&pool, "spc_a").await.as_deref(), Some("0.3.0"));
    assert_eq!(on(&pool, "spc_b").await.as_deref(), Some("0.1.0"));
    let last = Versions::on(&pool).pin_history("spc_a").await.unwrap();
    let last = last.last().unwrap();
    assert_eq!(last.cause, "window");
    assert_eq!(last.actor, format!("ordo:window:{id}"));

    // A second node sweeping the same window finds nothing to do.
    let again = revoked_moves::sweep(&pool, tomorrow).await.expect("swept");
    assert!(again.moved.is_empty() && again.refused.is_empty());
}

/// **The target is the developer's**: the tenant's own over the revoke's,
/// the revoke's over the active version.
#[tokio::test(flavor = "multi_thread")]
async fn the_target_is_the_tenants_then_the_revokes_then_the_active() {
    let pool = deployment("ordo_vmove_target", Some("#2")).await;
    let v = Versions::on(&pool);
    assert_eq!(
        v.revocation_of("0.1.0").await.unwrap().unwrap().move_to,
        Some("0.2.0".into())
    );
    v.set_move_target("spc_b", Some("0.3.0"), OWNER, 300)
        .await
        .expect("set");
    let a = v
        .move_off_revoked("spc_a", OWNER, PinCause::Demand, 400)
        .await
        .expect("moved");
    assert_eq!(
        (a.change.to.as_str(), a.source),
        ("0.2.0", TargetSource::Revoke)
    );
    let b = v
        .move_off_revoked("spc_b", OWNER, PinCause::Demand, 400)
        .await
        .expect("moved");
    assert_eq!(
        (b.change.to.as_str(), b.source),
        ("0.3.0", TargetSource::Tenant)
    );
    assert_eq!(
        v.pin_history("spc_b").await.unwrap().last().unwrap().cause,
        "demand"
    );
    // Not on a revoked version any more: nothing to move.
    let e = v
        .move_off_revoked("spc_a", OWNER, PinCause::Demand, 401)
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::NotOnRevoked { .. }), "{e}");
    assert!(e.to_string().contains("ordo versions pin"), "{e}");
}

/// **A revoked target is refused**: as a tenant's target, as a revoke's
/// `--move-to`, and at move time when it was revoked after it was named.
/// The tenant stays, and the refusal names the command that fixes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_target_is_refused_and_the_tenant_stays() {
    let pool = deployment("ordo_vmove_refused", Some("0.2.0")).await;
    let v = Versions::on(&pool);
    let e = v
        .set_move_target("spc_a", Some("0.1.0"), OWNER, 300)
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::Revoked { .. }), "{e}");
    let e = v
        .set_move_target("spc_none", Some("0.2.0"), OWNER, 300)
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::NoTenant { .. }), "{e}");

    // The revoke's target is revoked after it was named.
    v.revoke("0.2.0", "same flaw", None, OWNER, 310)
        .await
        .expect("revoked");
    let e = v
        .move_off_revoked("spc_a", OWNER, PinCause::Demand, 320)
        .await
        .unwrap_err();
    match &e {
        VersionRefusal::TargetRevoked { source, .. } => assert_eq!(*source, TargetSource::Revoke),
        other => panic!("{other}"),
    }
    let text = e.to_string();
    assert!(text.contains("ordo versions target spc_a"), "{text}");
    assert!(text.contains("same flaw"), "{text}");
    assert_eq!(on(&pool, "spc_a").await.as_deref(), Some("0.1.0"));

    // In a window it stays too, and the sweep says why.
    window(
        &pool,
        Target::All,
        Schedule::Range {
            start: NOW as i64 - 60,
            end: NOW as i64 + 60,
        },
    )
    .await;
    let swept = revoked_moves::sweep(&pool, NOW).await.expect("swept");
    assert!(swept.moved.is_empty(), "{swept:?}");
    assert_eq!(swept.refused.len(), 2, "{swept:?}");
    assert!(swept.refused[0].1.contains("move_target_revoked"));

    // Named for the tenant, it moves at the next tick.
    v.set_move_target("spc_a", Some("#3"), OWNER, 330)
        .await
        .expect("set");
    let swept = revoked_moves::sweep(&pool, NOW).await.expect("swept");
    assert_eq!(swept.moved.len(), 1, "{swept:?}");
    assert_eq!(on(&pool, "spc_a").await.as_deref(), Some("0.3.0"));

    // A revoke naming itself, or a revoked version, as `--move-to`.
    let e = v
        .revoke("0.3.0", "x", Some("0.3.0"), OWNER, 340)
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::MoveTo(_)), "{e}");
}

/// A tenant's target, set again or cleared, keeps one row; erasure owns it.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_is_set_replaced_and_cleared() {
    let pool = deployment("ordo_vmove_clear", None).await;
    let v = Versions::on(&pool);
    v.set_move_target("spc_a", Some("#2"), OWNER, 300)
        .await
        .unwrap();
    v.set_move_target("spc_a", Some("#3"), OWNER, 301)
        .await
        .unwrap();
    assert_eq!(
        v.move_target_of("spc_a").await.unwrap().as_deref(),
        Some("0.3.0")
    );
    let report = v.pinned_to_revoked().await.unwrap();
    let a = report[0]
        .tenants
        .iter()
        .find(|t| t.space == "spc_a")
        .unwrap();
    assert_eq!(a.target.as_deref(), Some("0.3.0"));
    assert_eq!(
        v.set_move_target("spc_a", None, OWNER, 302).await.unwrap(),
        None
    );
    assert_eq!(v.move_target_of("spc_a").await.unwrap(), None);
    assert!(ordo_exec::erase::ATTACHMENTS.contains(&"$move_targets"));
}

/// **On demand, at the door** (`std.deployment.moveOffRevoked`): the
/// deployment owner or a system moves one tenant now. Each move and target
/// leaves a record. Bound to a tenant's space, neither op is there. None
/// asks for a passkey touch.
#[tokio::test(flavor = "multi_thread")]
async fn the_door_moves_a_tenant_on_demand() {
    let pool = deployment("ordo_vmove_door", None).await;
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

    let t = run(
        deploy_ops::SET_MOVE_TARGET,
        "home",
        serde_json::json!({"tenant": "spc_b", "version": "#2"}),
    )
    .await
    .expect("set");
    assert_eq!(t.rows[0]["target"], "0.2.0");
    assert_eq!(t.rows[0]["ordinal"], 2);

    let m = run(
        deploy_ops::MOVE_OFF_REVOKED,
        "home",
        serde_json::json!({"tenant": "spc_b"}),
    )
    .await
    .expect("moved");
    assert_eq!(m.rows[0]["from"], "0.1.0");
    assert_eq!(m.rows[0]["to"], "0.2.0");
    assert_eq!(m.rows[0]["chosenBy"], "tenant");
    assert_eq!(on(&pool, "spc_b").await.as_deref(), Some("0.2.0"));

    let e = run(
        deploy_ops::MOVE_OFF_REVOKED,
        "home",
        serde_json::json!({"tenant": "spc_c"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ordo_api::ErrorCode::NotFound);
    assert!(e.message.contains("not_on_revoked"), "{}", e.message);

    for op in [deploy_ops::MOVE_OFF_REVOKED, deploy_ops::SET_MOVE_TARGET] {
        let e = run(op, "spc_a", serde_json::json!({"tenant": "spc_a"}))
            .await
            .unwrap_err();
        assert_eq!(e.code, ordo_api::ErrorCode::NotFound, "{op}");
        assert!(
            deploy_ops::NO_PRESENCE.contains(&op),
            "{op} asks no passkey touch"
        );
    }
    let presence = ordo_host::presence::PresencePolicy::standard();
    for op in [
        deploy_ops::MOVE_OFF_REVOKED,
        deploy_ops::SET_MOVE_TARGET,
        revoked_moves::MOVE_SELF,
    ] {
        assert!(!presence.requires(op, ordo_api::OpKind::Write), "{op}");
    }
    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(acts, ["move-target", "moved-off-revoked"]);
}

/// **The tenant's own move** (`std.space.moveOffRevoked`): granted to no
/// tenant role by default; the body moves the tenant it is bound to, cause
/// `tenant`.
#[tokio::test(flavor = "multi_thread")]
async fn the_tenants_own_move_is_off_by_default_and_moves_itself() {
    assert_eq!(
        revoked_moves::current().roles_for(revoked_moves::MOVE_SELF),
        Some(&[][..]),
        "no tenant role holds it unless the app developer names one"
    );
    let pool = deployment("ordo_vmove_self", None).await;
    let ops = apply_op::ApplyOps::recording(
        pool.clone(),
        std::sync::Arc::new(SinglePool::new(pool.clone())),
        "home",
        None,
    );
    let r = revoked_moves::execute(&ops, "spc_a", &BoundParams::default(), "mem_owner")
        .await
        .expect("moved");
    assert_eq!(r.rows[0]["to"], "0.3.0");
    assert_eq!(r.rows[0]["chosenBy"], "active");
    let last = Versions::on(&pool).pin_history("spc_a").await.unwrap();
    assert_eq!(last.last().unwrap().cause, "tenant");
    assert_eq!(on(&pool, "spc_b").await.as_deref(), Some("0.1.0"));
}

/// **`versions.served`** (§1.6): a list that leaves out a version tenants
/// are pinned to is refused, naming the value to add; a name not stored is
/// refused, naming the setting; a pin outside the list is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_served_list_never_strands_a_tenant() {
    let pool = deployment("ordo_vmove_served", None).await;
    let v = Versions::on(&pool);
    let e = v
        .check_policy(&ServedPolicy::parse(Some("#2"), None))
        .await
        .unwrap_err();
    assert!(
        matches!(e, VersionRefusal::Stranded { tenants: 2, .. }),
        "{e}"
    );
    let text = e.to_string();
    assert!(
        text.contains("ORDO_VERSIONS_SERVED=0.2.0,0.1.0"),
        "names the value to add: {text}"
    );
    let e = v
        .check_policy(&ServedPolicy::parse(Some("0.1.0,9.9.9"), None))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("ORDO_VERSIONS_SERVED"), "{e}");
    assert!(e.to_string().contains("9.9.9"), "{e}");

    let p = v
        .check_policy(&ServedPolicy::parse(Some("#1"), None))
        .await
        .expect("strands nobody");
    assert_eq!(p.served, Some(vec!["0.1.0".to_string()]));
    let v = v.with_policy(&p);
    let e = v.pin("spc_c", "0.2.0", OWNER, 500).await.unwrap_err();
    assert!(matches!(e, VersionRefusal::NotServed { .. }), "{e}");
    assert!(
        e.to_string().contains("ORDO_VERSIONS_SERVED=0.1.0,0.2.0"),
        "{e}"
    );
    // The active version is always served.
    v.pin("spc_a", "0.3.0", OWNER, 501).await.expect("active");
}

/// **`versions.minimum`** (§1.6): a tenant below it moves in its window as
/// off a revoked version; nothing is pinned or activated below it.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_below_the_minimum_moves_in_its_window() {
    let pool = fresh("ordo_vmove_minimum").await;
    for t in ["spc_a", "spc_b"] {
        tenant(&pool, t).await;
    }
    apply(&pool, V010).await;
    apply(&pool, V020).await;
    apply(&pool, V030).await;
    let v = Versions::on(&pool);
    v.pin("spc_a", "0.1.0", OWNER, 100).await.expect("pinned");
    v.pin("spc_b", "0.2.0", OWNER, 100).await.expect("pinned");
    let p = v
        .check_policy(&ServedPolicy::parse(None, Some("#2")))
        .await
        .expect("stored");
    assert_eq!(p.minimum.as_deref(), Some("0.2.0"));
    let v = v.with_policy(&p);

    let e = v.pin("spc_b", "#1", OWNER, 200).await.unwrap_err();
    assert!(matches!(e, VersionRefusal::BelowMinimum { .. }), "{e}");
    assert!(e.to_string().contains("ORDO_VERSIONS_MINIMUM"), "{e}");
    let e = v
        .activate("0.1.0", OWNER, ordo_commandlog::Timestamp(201))
        .await
        .unwrap_err();
    assert!(matches!(e, VersionRefusal::BelowMinimum { .. }), "{e}");

    // No window: it stays. In its window: it moves, and only it.
    let swept = revoked_moves::sweep_under(&pool, &p, NOW).await.unwrap();
    assert!(swept.moved.is_empty());
    window(
        &pool,
        Target::All,
        Schedule::Range {
            start: NOW as i64 - 60,
            end: NOW as i64 + 60,
        },
    )
    .await;
    let swept = revoked_moves::sweep_under(&pool, &p, NOW).await.unwrap();
    assert_eq!(swept.moved.len(), 1, "{swept:?}");
    assert_eq!(on(&pool, "spc_a").await.as_deref(), Some("0.3.0"));
    assert_eq!(on(&pool, "spc_b").await.as_deref(), Some("0.2.0"));
}

/// **Per-node health** (§1.4, increment 3 session 2): `/health/deploy`
/// says, per node, which versions it serves beside the active one, where
/// it starts new tenants, and each version a pin names that it could not
/// load, with why.
#[tokio::test(flavor = "multi_thread")]
async fn each_node_says_what_it_serves_and_what_would_not_load() {
    let pool = fresh("ordo_vmove_health").await;
    ordo_exec::nodes::ensure_table(&pool).await.expect("$nodes");
    let node = ordo_exec::nodes::Node {
        node: "n1".into(),
        serving: "0.3.0".into(),
        caught_up: "0.3.0".into(),
        also_serving: vec!["0.2.0".into()],
        new_tenant: "0.4.0-beta".into(),
        not_loaded: vec![("0.1.0".into(), "it does not compile".into())],
        beat_secs: 5,
        ..Default::default()
    };
    ordo_exec::nodes::beat(&pool, &node).await.expect("written");
    let live = ordo_exec::nodes::live(&pool).await.expect("read");
    assert_eq!(live[0].not_loaded, node.not_loaded);
    let body = ordo_host::serve::deploy_status("0.3.0", &live);
    let n = &body["nodes"][0];
    assert_eq!(n["alsoServing"], serde_json::json!(["0.2.0"]));
    assert_eq!(n["newTenant"], "0.4.0-beta");
    assert_eq!(n["notLoaded"][0]["version"], "0.1.0");
    assert_eq!(n["notLoaded"][0]["why"], "it does not compile");
}
