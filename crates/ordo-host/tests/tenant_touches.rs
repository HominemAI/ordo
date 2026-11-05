//! A tenant chooses its own passkey touches, within what the app developer
//! exposes (owner, 2026-11-05, `tenant-chooses-its-touches`).
//!
//! - Nothing is exposed by default, and then a tenant can change nothing.
//! - A touch a tenant lowers takes effect in that tenant, and only there.
//! - A raise-only exposure refuses a lowering, naming the command.
//! - A tenant's choice never reaches another tenant, and only the tenant's
//!   owner reaches the setter.
//! - The acts on the deployment can never be exposed.
//! - The per-op settings outrank a tenant's choice; a choice the exposure no
//!   longer allows is passed over.
//!
//! A regression looks like a tenant lowering a touch nobody exposed, a choice
//! leaking into another tenant, or `deploying` accepted as exposable.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_api::{BoundParams, ErrorCode, OpKind};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::presence::PresencePolicy;
use ordo_host::profile_ops::TenantAccess;
use ordo_host::security_level::Situation;
use ordo_host::tenant_touches::{self as touches, DoorTouches, TouchOps};
use ordo_identity::principal::{PrincipalId, SpaceId};
use serde_json::json;

const HOME: &str = "home";

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool")
}

fn ops(pool: &Pool, stream: &Arc<ordo_secevent::Recorder>) -> TouchOps {
    TouchOps::on_pool(pool, HOME).with_stream(stream.clone())
}

async fn run(
    ops: &TouchOps,
    op: &str,
    space: &str,
    v: serde_json::Value,
) -> Result<ordo_api::ExecResult, ordo_api::ExecError> {
    let b = BoundParams(v.as_object().unwrap().clone());
    ops.execute(op, space, &b, "o1", None).await
}

fn seat(space: &str, id: &str, role: &str) -> Vec<Option<String>> {
    vec![
        Some(space.into()),
        Some(id.into()),
        Some(role.into()),
        Some("active".into()),
        Some(id.into()),
        Some("principal".into()),
    ]
}

fn engine(touches: DoorTouches, policy: &PresencePolicy) -> ordo_identity::AccessEngine {
    let f = Fixture::load().expect("fixture");
    let tenant = TenantAccess {
        touches,
        ..Default::default()
    };
    ordo_host::engine_from_directory(
        &f,
        &[
            seat("s1", "o1", "owner"),
            seat("s1", "m1", "manager"),
            seat("s2", "o2", "owner"),
        ],
        &tenant,
        policy,
        &Default::default(),
        &ordo_host::delegable::DelegablePolicy::closed(),
        &ordo_host::access_template::AccessTemplate::none(),
    )
    .expect("engine")
}

fn asks(e: &ordo_identity::AccessEngine, space: &str, op: &str) -> bool {
    e.step_up_in(&SpaceId(space.into()), op, None)
        .demands_presence()
}

/// Nothing exposed: every choice is refused, naming the command that
/// exposes it, and a stored choice changes nothing at the door.
#[tokio::test(flavor = "multi_thread")]
async fn nothing_exposed_means_a_tenant_changes_nothing() {
    let pool = fresh("ordo_touch_none").await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = ops(&pool, &stream);
    for (situation, touch) in [("membership", "on"), ("export-import", "off")] {
        let e = run(
            &ops,
            touches::SET_TOUCH,
            "s1",
            json!({"situation": situation, "touch": touch}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::TenantRefused, "{}", e.message);
        assert!(e.message.contains("exposed as none"), "{}", e.message);
        assert!(
            e.message
                .contains(&format!("ordo touches expose {situation}")),
            "{}",
            e.message
        );
    }
    let read = run(&ops, touches::SPACE_TOUCHES, "s1", json!({}))
        .await
        .expect("read");
    for s in read.rows[0]["situations"].as_array().unwrap() {
        assert_eq!(s["exposed"], "none");
        assert_eq!(s["choice"], serde_json::Value::Null);
    }

    // A choice stored by hand while nothing is exposed is passed over.
    let mut door = DoorTouches::read(&pool).await.expect("door read");
    assert!(door.exposure.is_empty());
    door.choices
        .entry("s1".into())
        .or_default()
        .insert(Situation::ExportImport, false);
    let e = engine(door, &PresencePolicy::standard());
    assert!(asks(&e, "s1", "std.space.export"));
}

/// Lowered in one tenant: the touch is gone there and stays in the other.
/// A tenant owner reaches the setter of its own tenant only; a manager
/// reaches the read and not the setter.
#[tokio::test(flavor = "multi_thread")]
async fn a_lowered_touch_takes_effect_in_that_tenant_alone() {
    let pool = fresh("ordo_touch_lower").await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = ops(&pool, &stream);
    let r = run(
        &ops,
        touches::EXPOSE,
        HOME,
        json!({"situation": "export-import", "exposed": "lower", "reason": "small teams"}),
    )
    .await
    .expect("exposed");
    assert_eq!(r.rows[0]["changed"], true);
    assert_eq!(r.rows[0]["history"][0]["actor"], "o1");
    assert_eq!(r.rows[0]["history"][0]["reason"], "small teams");

    run(
        &ops,
        touches::SET_TOUCH,
        "s1",
        json!({"situation": "export-import", "touch": "off"}),
    )
    .await
    .expect("lowered");

    let door = DoorTouches::read(&pool).await.expect("door read");
    let e = engine(door.clone(), &PresencePolicy::standard());
    for op in ["std.space.export", "std.space.import"] {
        assert!(!asks(&e, "s1", op), "{op}: lowered in s1");
        assert!(asks(&e, "s2", op), "{op}: s2 keeps the level's touch");
    }
    // Ops outside the situation are untouched.
    assert!(asks(&e, "s1", "std.space.delete"));

    // Only the tenant's own owner reaches the setter, in its own tenant.
    let reaches = |space: &str, who: &str, op: &str| {
        e.membership_reaches(&SpaceId(space.into()), &PrincipalId(who.into()), op)
    };
    assert!(reaches("s1", "o1", touches::SET_TOUCH));
    assert!(!reaches("s2", "o1", touches::SET_TOUCH));
    assert!(!reaches("s1", "m1", touches::SET_TOUCH));
    assert!(reaches("s1", "m1", touches::SPACE_TOUCHES));

    // s2's read shows no choice of its own.
    let r = run(&ops, touches::SPACE_TOUCHES, "s2", json!({}))
        .await
        .expect("read");
    let ei = r.rows[0]["situations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["situation"] == "export-import")
        .cloned()
        .unwrap();
    assert_eq!(ei["choice"], serde_json::Value::Null);
    assert_eq!(ei["inForce"], true);

    // A per-op setting outranks the tenant's choice.
    let known: Vec<String> = ordo_host::platform_ops::specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    let strict = PresencePolicy::standard()
        .with_required("std.space.export", &known)
        .expect("required");
    let e = engine(door.clone(), &strict);
    assert!(
        asks(&e, "s1", "std.space.export"),
        "ORDO_STEP_UP_REQUIRED_OPS wins"
    );
    assert!(!asks(&e, "s1", "std.space.import"));

    // Taken back to none: the stored choice is passed over.
    run(
        &ops,
        touches::EXPOSE,
        HOME,
        json!({"situation": "export-import", "exposed": "none", "reason": "audit"}),
    )
    .await
    .expect("taken back");
    let e = engine(
        DoorTouches::read(&pool).await.unwrap(),
        &PresencePolicy::standard(),
    );
    assert!(asks(&e, "s1", "std.space.export"));

    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Enrolment { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(
        acts,
        [
            "tenant-touch-exposed",
            "tenant-touch-set",
            "tenant-touch-exposed"
        ]
    );
}

/// Raise only: turning a touch on is taken and asks at standard, where the
/// level does not; turning one off is refused, naming the command.
#[tokio::test(flavor = "multi_thread")]
async fn a_raise_only_exposure_refuses_a_lowering() {
    let pool = fresh("ordo_touch_raise").await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = ops(&pool, &stream);
    run(
        &ops,
        touches::EXPOSE,
        HOME,
        json!({"situation": "membership", "exposed": "raise", "reason": "compliance"}),
    )
    .await
    .expect("exposed");
    let e = run(
        &ops,
        touches::SET_TOUCH,
        "s1",
        json!({"situation": "membership", "touch": "off"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::TenantRefused);
    assert!(e.message.contains("exposed as raise"), "{}", e.message);
    assert!(
        e.message.contains("ordo touches expose membership lower"),
        "{}",
        e.message
    );

    run(
        &ops,
        touches::SET_TOUCH,
        "s1",
        json!({"situation": "membership", "touch": "on"}),
    )
    .await
    .expect("raised");
    let e = engine(
        DoorTouches::read(&pool).await.unwrap(),
        &PresencePolicy::standard(),
    );
    assert!(asks(&e, "s1", "std.space.admit"));
    assert!(!asks(&e, "s2", "std.space.admit"));
    // The setter itself always follows the level.
    assert!(asks(&e, "s1", touches::SET_TOUCH));

    // Back to the deployment's.
    run(
        &ops,
        touches::SET_TOUCH,
        "s1",
        json!({"situation": "membership", "touch": "default"}),
    )
    .await
    .expect("cleared");
    let e = engine(
        DoorTouches::read(&pool).await.unwrap(),
        &PresencePolicy::standard(),
    );
    assert!(!asks(&e, "s1", "std.space.admit"));
}

/// The acts on the deployment, recovery, platform reads and a person's
/// contact email are never exposable; the deployment's ops answer only in
/// its own space and a tenant's never there.
#[tokio::test(flavor = "multi_thread")]
async fn deployment_situations_can_never_be_exposed() {
    let pool = fresh("ordo_touch_never").await;
    let stream = Arc::new(ordo_secevent::Recorder::default());
    let ops = ops(&pool, &stream);
    for s in Situation::ALL.into_iter().filter(|s| !s.tenant_choosable()) {
        for way in ["raise", "lower", "both"] {
            let e = run(
                &ops,
                touches::EXPOSE,
                HOME,
                json!({"situation": s.word(), "exposed": way, "reason": "try"}),
            )
            .await
            .unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidParam, "{}", e.message);
            assert!(e.message.contains("never choose it"), "{}", e.message);
        }
    }
    // A stored row for one is ignored at the door all the same.
    touches::ensure_table(&pool).await.expect("tables");
    pool.rows(
        r#"INSERT INTO "$touchexposure" ("situation", "direction", "actor", "reason", "at")
           VALUES ('deploying', 'both', 'x', 'by hand', 1)"#,
    )
    .await
    .expect("by hand");
    let door = DoorTouches::read(&pool).await.unwrap();
    assert!(door.exposure.is_empty());

    let e = run(
        &ops,
        touches::EXPOSE,
        "s1",
        json!({"situation": "membership", "exposed": "both", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let e = run(
        &ops,
        touches::SET_TOUCH,
        HOME,
        json!({"situation": "membership", "touch": "on"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);

    // The exposure op is a deployment write: a touch only at high.
    let write = OpKind::Write;
    assert!(!PresencePolicy::standard().requires(touches::EXPOSE, write));
    assert!(PresencePolicy::standard().requires(touches::SET_TOUCH, write));
}
