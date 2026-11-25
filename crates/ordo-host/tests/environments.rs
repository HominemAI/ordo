//! Environments registered (`plan-versions.md` §4 row 4a): the deployment
//! declares its chain, and an environment it has not registered is refused
//! with the command that creates it.
//!
//! - A deployment that registered nothing reads as production, with no
//!   action and no row.
//! - `std.deployment.registerEnvironment` adds one before the deployment's
//!   own, so dev then staging give dev, staging, production. A position
//!   places it elsewhere.
//! - `std.deployment.environments` lists the chain; with a name it answers
//!   that one, or `not_found` naming `ordo environments register <name>`.
//! - A duplicate, a bad name, a missing reason and a position outside the
//!   chain are refused as the caller's input.
//! - Each registration is one record on the security stream.
//! - Bound to a tenant, neither op is there.
//!
//! A regression looks like a refusal that does not name the command, an
//! existing deployment that is not production, or a chain out of order.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op;
use ordo_host::deploy_ops;
use ordo_host::environments::Environments;

const OWNER: &str = "deployment-owner:ada";

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

fn names(row: &serde_json::Value) -> Vec<String> {
    row["chain"]
        .as_array()
        .expect("a chain")
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect()
}

/// An existing deployment, with no table and no action, is production.
#[tokio::test(flavor = "multi_thread")]
async fn a_deployment_that_registered_nothing_is_production() {
    let pool = fresh("ordo_env_default").await;
    let envs = Environments::on(&pool);
    let own = envs.own().await.expect("own");
    assert_eq!(own.name, "production");
    assert!(own.own);
    let chain = envs.chain().await.expect("chain");
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].position, 1);
    envs.require("production").await.expect("registered");
    // Reading wrote nothing.
    let tables = pool
        .rows("SELECT 1 FROM pg_tables WHERE tablename = '$environments'")
        .await
        .expect("catalogue");
    assert!(tables.is_empty(), "a read created the table");
}

/// The client: order, a position, and every refusal.
#[tokio::test(flavor = "multi_thread")]
async fn the_chain_keeps_its_order_and_refuses_what_it_cannot_take() {
    let pool = fresh("ordo_env_chain").await;
    let envs = Environments::on(&pool);

    envs.register("dev", None, OWNER, "where work starts", 100)
        .await
        .expect("dev");
    let chain = envs
        .register("staging", None, OWNER, "the rehearsal", 101)
        .await
        .expect("staging");
    let order: Vec<_> = chain.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(order, ["dev", "staging", "production"]);
    assert_eq!(
        chain.iter().map(|e| e.position).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(envs.own().await.unwrap().name, "production");

    // A position puts one first.
    let chain = envs
        .register("sandbox", Some(1), OWNER, "scratch work", 102)
        .await
        .expect("sandbox");
    let order: Vec<_> = chain.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(order, ["sandbox", "dev", "staging", "production"]);
    // And one after production, when asked.
    let chain = envs
        .register("dr", Some(5), OWNER, "disaster recovery", 103)
        .await
        .expect("dr");
    assert_eq!(chain.last().unwrap().name, "dr");

    for (name, pos, reason, what) in [
        ("dev", None, "again", "already registered"),
        ("Dev Two", None, "x", "not an environment name"),
        ("", None, "x", "not an environment name"),
        ("qa", None, "  ", "reason"),
        ("qa", Some(9), "x", "outside the chain"),
        ("qa", Some(0), "x", "outside the chain"),
    ] {
        let e = envs
            .register(name, pos, OWNER, reason, 104)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains(what), "{name:?}: {e}");
    }
    // None of those wrote anything.
    assert_eq!(envs.chain().await.unwrap().len(), 5);
}

/// A name that is not registered is refused, with the command that
/// registers it.
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_not_registered_is_refused_with_its_command() {
    let pool = fresh("ordo_env_refusal").await;
    let envs = Environments::on(&pool);
    let e = envs.require("staging").await.unwrap_err().to_string();
    assert!(e.contains("`staging` is not registered"), "{e}");
    assert!(e.contains("ordo environments register staging"), "{e}");
    assert!(e.contains("production"), "names the chain it has: {e}");

    envs.register("staging", None, OWNER, "the rehearsal", 100)
        .await
        .expect("registered");
    assert_eq!(envs.require("staging").await.unwrap().name, "staging");
}

/// **The door**: list, register, the refusal as `not_found` naming the
/// command, input refusals as `invalid_param`, one record per
/// registration, and nothing bound to a tenant.
#[tokio::test(flavor = "multi_thread")]
async fn the_door_lists_and_registers_environments() {
    let pool = fresh("ordo_env_door").await;
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
        async move { deploy_ops::execute(ops, name, space, &b, OWNER).await }
    };

    // A deployment as it was: production, nothing done.
    let r = run(deploy_ops::ENVIRONMENTS, "home", serde_json::json!({}))
        .await
        .expect("list");
    assert_eq!(r.rows[0]["own"], "production");
    assert_eq!(names(&r.rows[0]), ["production"]);

    // Naming one that is not there.
    let e = run(
        deploy_ops::ENVIRONMENTS,
        "home",
        serde_json::json!({"name": "staging"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(
        e.message.contains("ordo environments register staging"),
        "{}",
        e.message
    );

    for (name, reason) in [("dev", "where work starts"), ("staging", "the rehearsal")] {
        let r = run(
            deploy_ops::REGISTER_ENVIRONMENT,
            "home",
            serde_json::json!({"name": name, "reason": reason}),
        )
        .await
        .expect("registered");
        assert_eq!(r.rows[0]["registered"], name);
        assert_eq!(r.rows[0]["own"], "production");
    }

    let r = run(
        deploy_ops::ENVIRONMENTS,
        "home",
        serde_json::json!({"name": "staging"}),
    )
    .await
    .expect("staging is there now");
    assert_eq!(names(&r.rows[0]), ["dev", "staging", "production"]);
    assert_eq!(r.rows[0]["environment"]["name"], "staging");
    assert_eq!(r.rows[0]["environment"]["actor"], OWNER);
    assert_eq!(r.rows[0]["environment"]["reason"], "the rehearsal");
    assert_eq!(r.rows[0]["environment"]["position"], 2);

    let e = run(
        deploy_ops::REGISTER_ENVIRONMENT,
        "home",
        serde_json::json!({"name": "dev", "reason": "again"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Duplicate);
    let e = run(
        deploy_ops::REGISTER_ENVIRONMENT,
        "home",
        serde_json::json!({"name": "QA!", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParam);
    let e = run(
        deploy_ops::REGISTER_ENVIRONMENT,
        "home",
        serde_json::json!({"name": "qa", "reason": ""}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParam);

    // Bound to a tenant, neither op is there.
    for op in [deploy_ops::ENVIRONMENTS, deploy_ops::REGISTER_ENVIRONMENT] {
        let e = run(
            op,
            "spc_a",
            serde_json::json!({"name": "dev", "reason": "x"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound, "{op}");
    }

    // A change asks no touch below `high`, like the version settings.
    use ordo_host::security_level::SecurityLevel;
    let presence = ordo_host::presence::PresencePolicy::standard();
    let write = ordo_api::OpKind::Write;
    assert!(!presence.requires(deploy_ops::REGISTER_ENVIRONMENT, write));
    assert!(presence
        .with_level(SecurityLevel::High)
        .requires(deploy_ops::REGISTER_ENVIRONMENT, write));

    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(acts, ["environment-registered", "environment-registered"]);
}
