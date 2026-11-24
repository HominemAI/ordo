//! The promotion gate and the environment changes (`plan-versions.md` §4
//! rows 4a and 4b). Each gate rule has a refusal test:
//!
//! - the target has a step before it, and the source is that step;
//! - staging refuses a version the step before has not activated;
//! - an edited tree is refused, naming the file;
//! - a function secret the target has no value for is refused, naming it;
//! - a production deployment pins only what arrived through the gate;
//! - a promotion is recorded in `$promotions` and on the security stream.
//!
//! And the owner's changes to the chain: which environment this deployment
//! is, and removing or renaming one, refused for the deployment's own and
//! for one a stored promotion names.
//!
//! The source is handed over (`std.deployment.versionSource` read from the
//! step before), as the owner's question 11 leaves open.
//!
//! A regression looks like a version reaching production around the gate,
//! a refusal that does not name the file or the command, or a promotion
//! with no record.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_api::{BoundParams, ErrorCode};
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, CodeFile, CodeFiles};
use ordo_host::deploy_ops;
use ordo_host::environments::Environments;
use ordo_host::promotion::{self, PromotionRefusal, SourceVersion};
use ordo_host::tenants;
use ordo_host::versions::Versions;

const OWNER: &str = "deployment-owner:ada";
const CODE_PATH: &str = "fn/hello.js";

fn source(version: &str) -> String {
    format!(
        "domain tasks @ {version}\n\nentity task {{\n  title: text\n}}\n\n\
         op openTasks unbounded =\n  task[title]\n\n\
         secret hookKey\n\n\
         function hello in tasks {{\n  call($t: text(10)) -> json by member\n  \
         code \"{CODE_PATH}\"\n  secrets hookKey\n}}\n"
    )
}

fn code(text: &str) -> CodeFiles {
    [(
        CODE_PATH.to_string(),
        CodeFile {
            hash: ordo_plan::artifact::code_hash(text.as_bytes()),
            text: text.to_string(),
        },
    )]
    .into()
}

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

async fn apply(pool: &Pool, version: &str, activate: bool) -> Result<(), apply_op::ApplyRefused> {
    apply_op::run(
        pool,
        &SinglePool::new(pool.clone()),
        &source(version),
        &code("export default {};"),
        "dev:ada",
        "t",
        activate,
    )
    .await
    .map(|_| ())
}

fn ops(
    pool: &Pool,
    stream: Option<std::sync::Arc<ordo_secevent::Recorder>>,
) -> std::sync::Arc<apply_op::ApplyOps> {
    apply_op::ApplyOps::recording(
        pool.clone(),
        std::sync::Arc::new(SinglePool::new(pool.clone())),
        "home",
        stream,
    )
}

async fn run(
    ops: &apply_op::ApplyOps,
    name: &str,
    v: serde_json::Value,
) -> Result<ordo_api::ExecResult, ordo_api::ExecError> {
    let b = BoundParams(v.as_object().unwrap().clone());
    deploy_ops::execute(ops, name, "home", &b, OWNER).await
}

/// A deployment of the chain: `registered` in order, being `own`.
async fn deployment(stem: &str, registered: &[&str], own: &str) -> Pool {
    let pool = fresh(stem).await;
    let envs = Environments::on(&pool);
    for (i, name) in registered.iter().enumerate() {
        envs.register(name, Some(i as u32 + 1), OWNER, "the chain", 100)
            .await
            .expect("registered");
    }
    if own != "production" {
        envs.set_own(own, "this is it").await.expect("own");
    }
    pool
}

/// What `std.deployment.versionSource` says on the step before, as the
/// CLI carries it.
async fn handed_over(from: &Pool, version: &str) -> SourceVersion {
    let r = run(
        &ops(from, None),
        deploy_ops::VERSION_SOURCE,
        serde_json::json!({ "version": version }),
    )
    .await
    .expect("the source reads");
    let row = &r.rows[0];
    SourceVersion {
        env: row["env"].as_str().unwrap().to_string(),
        version: row["version"].as_str().unwrap().to_string(),
        source: row["source"].as_str().unwrap().to_string(),
        source_id: row["sourceId"].as_str().unwrap().to_string(),
        code_hashes: serde_json::from_value(row["codeHashes"].clone()).unwrap(),
        code: apply_op::parse_code(row["code"].as_str()).expect("code parses"),
        activated: row["activated"].as_bool().unwrap(),
    }
}

async fn promote(to: &Pool, src: SourceVersion) -> Result<promotion::Promoted, PromotionRefusal> {
    promotion::promote(
        to,
        &SinglePool::new(to.clone()),
        src,
        OWNER,
        "ready for staging",
        true,
        1_000,
    )
    .await
}

async fn stored(pool: &Pool) -> Vec<String> {
    let v = Versions::on(pool);
    v.ensure().await.expect("tables");
    v.stored().await.unwrap().into_iter().map(|s| s.0).collect()
}

async fn set_secret(pool: &Pool, name: &str) {
    pool.rows(ordo_host::secrets::DDL).await.expect("$secrets");
    pool.rows(&format!(
        "INSERT INTO \"$secrets\" (\"name\", \"value\", \"version\", \"set_by\", \"set_at\") \
         VALUES ('{name}', '\\x00'::bytea, 1, 'ada', 1)"
    ))
    .await
    .expect("secret set");
}

/// Dev with 0.1.0 activated and 0.2.0 stored only.
async fn dev(stem: &str) -> Pool {
    let pool = deployment(stem, &["dev"], "dev").await;
    apply(&pool, "0.1.0", true).await.expect("0.1.0");
    apply(&pool, "0.2.0", false).await.expect("0.2.0 staged");
    pool
}

#[tokio::test(flavor = "multi_thread")]
async fn staging_refuses_a_version_dev_has_not_activated() {
    let dev = dev("ordo_promo_d1").await;
    let staging = deployment("ordo_promo_s1", &["dev", "staging"], "staging").await;
    let src = handed_over(&dev, "0.2.0").await;
    assert!(!src.activated);
    let e = promote(&staging, src).await.err().expect("refused");
    assert!(matches!(e, PromotionRefusal::NotActivated { .. }), "{e}");
    let text = e.to_string();
    assert!(text.contains("`dev` has not activated 0.2.0"), "{text}");
    assert!(text.contains("Activate it there"), "{text}");
    // Nothing was stored.
    assert!(stored(&staging).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_source_must_be_the_step_before_and_there_must_be_one() {
    let dev = dev("ordo_promo_d2").await;
    let staging = deployment("ordo_promo_s2", &["dev", "staging"], "staging").await;
    let mut src = handed_over(&dev, "0.1.0").await;
    src.env = "production".to_string();
    let e = promote(&staging, src).await.err().expect("refused");
    assert!(matches!(e, PromotionRefusal::WrongSource { .. }), "{e}");
    assert!(e.to_string().contains("Promote from `dev`"), "{e}");

    // Dev is the first of its chain: nothing is promoted into it.
    let src = handed_over(&dev, "0.1.0").await;
    let e = promote(&dev, src).await.err().expect("refused");
    assert!(matches!(e, PromotionRefusal::NoStepBefore { .. }), "{e}");
    assert!(e.to_string().contains("ordo apply --env dev"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_tree_is_refused_naming_the_file() {
    let dev = dev("ordo_promo_d3").await;
    let staging = deployment("ordo_promo_s3", &["dev", "staging"], "staging").await;
    set_secret(&staging, "hookKey").await;

    let mut edited_code = handed_over(&dev, "0.1.0").await;
    edited_code
        .code
        .get_mut(CODE_PATH)
        .unwrap()
        .text
        .push_str("// edited\n");
    let e = promote(&staging, edited_code).await.err().expect("refused");
    assert!(matches!(e, PromotionRefusal::Edited { .. }), "{e}");
    assert!(e.to_string().contains("`fn/hello.js`"), "{e}");

    let mut edited_def = handed_over(&dev, "0.1.0").await;
    edited_def.source.push_str("\n// edited\n");
    let e = promote(&staging, edited_def).await.err().expect("refused");
    assert!(e.to_string().contains("the definition file"), "{e}");

    let mut missing = handed_over(&dev, "0.1.0").await;
    missing.code.clear();
    let e = promote(&staging, missing).await.err().expect("refused");
    assert!(
        e.to_string().contains("`fn/hello.js` was not handed over"),
        "{e}"
    );
    assert!(stored(&staging).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_targets_own_checks_run_before_anything_is_stored() {
    let dev = dev("ordo_promo_d4").await;
    let staging = deployment("ordo_promo_s4", &["dev", "staging"], "staging").await;
    let e = promote(&staging, handed_over(&dev, "0.1.0").await)
        .await
        .err()
        .expect("refused");
    assert!(matches!(e, PromotionRefusal::SecretsMissing(_)), "{e}");
    let text = e.to_string();
    assert!(text.contains("hookKey"), "{text}");
    assert!(text.contains("ordo secret set"), "{text}");
    assert!(stored(&staging).await.is_empty());
    assert!(promotion::promotions(&staging, 10)
        .await
        .unwrap()
        .is_empty());

    set_secret(&staging, "hookKey").await;
    promote(&staging, handed_over(&dev, "0.1.0").await)
        .await
        .expect("promoted once the secret is set");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_promotion_stores_activates_and_leaves_a_record() {
    let dev = dev("ordo_promo_d5").await;
    let staging = deployment("ordo_promo_s5", &["dev", "staging"], "staging").await;
    set_secret(&staging, "hookKey").await;
    let stream = std::sync::Arc::new(ordo_secevent::Recorder::default());
    let o = ops(&staging, Some(stream.clone()));

    let src = handed_over(&dev, "0.1.0").await;
    let r = run(
        &o,
        deploy_ops::PROMOTE,
        serde_json::json!({
            "sourceEnv": src.env, "version": src.version, "source": src.source,
            "sourceId": src.source_id,
            "codeHashes": serde_json::to_string(&src.code_hashes).unwrap(),
            "code": apply_op::code_text(&src.code),
            "activated": true, "reason": "ready for staging",
        }),
    )
    .await
    .expect("promoted");
    assert_eq!(r.rows[0]["from"], "dev");
    assert_eq!(r.rows[0]["to"], "staging");
    assert_eq!(r.rows[0]["sourceId"], src.source_id.as_str());

    let v = Versions::on(&staging);
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    // Same source identity at both ends.
    let there = v.identity("0.1.0").await.unwrap().unwrap().1;
    assert_eq!(there, src.source_id);
    let records = promotion::promotions(&staging, 10).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].from_env, "dev");
    assert_eq!(records[0].to_env, "staging");
    assert_eq!(records[0].actor, OWNER);
    assert_eq!(records[0].reason, "ready for staging");

    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(acts, ["promoted"]);

    // A bound space is not the deployment.
    let e = run(&o, deploy_ops::PROMOTE, serde_json::json!({}))
        .await
        .err();
    assert!(e.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn production_pins_only_what_arrived_through_the_gate() {
    let dev = dev("ordo_promo_d6").await;
    // Production as it was: one version applied directly.
    let prod = fresh("ordo_promo_prod").await;
    apply(&prod, "0.0.9", true).await.expect("history");
    let before = Versions::on(&prod).active_version().await.unwrap();
    assert_eq!(before.as_deref(), Some("0.0.9"));
    // Registering dev gives production a step before it. Registered later
    // than the history, so the history stays usable.
    Environments::on(&prod)
        .register("dev", None, OWNER, "where work starts", u32::MAX as u64)
        .await
        .expect("dev");
    set_secret(&prod, "hookKey").await;

    // A direct apply is refused before it stores anything.
    let e = apply(&prod, "0.1.0", true).await.unwrap_err();
    assert_eq!(e.class(), "not_promoted", "{e}");
    assert!(e.to_string().contains("ordo promote"), "{e}");
    assert_eq!(Versions::on(&prod).stored().await.unwrap().len(), 1);

    // A direct rollback to a version that arrived another way is refused;
    // history from before the chain is not.
    apply_op::activate(&prod, "0.0.9", OWNER)
        .await
        .expect("history still activates");

    promote(&prod, handed_over(&dev, "0.1.0").await)
        .await
        .expect("through the gate");
    let v = Versions::on(&prod);
    assert_eq!(v.active_version().await.unwrap().as_deref(), Some("0.1.0"));
    // Back and forth between history and the promoted version.
    apply_op::activate(&prod, "0.0.9", OWNER)
        .await
        .expect("back");
    apply_op::activate(&prod, "0.1.0", OWNER)
        .await
        .expect("forward");

    // A deployment that registered nothing is not held to the gate.
    let free = fresh("ordo_promo_free").await;
    apply(&free, "0.1.0", true).await.expect("applies freely");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pin_to_a_version_that_skipped_the_gate_is_refused() {
    let prod = fresh("ordo_promo_pin").await;
    apply(&prod, "0.0.9", true).await.expect("history");
    // Stage a later version before the chain exists, then register after:
    // it is history. One stored after registration is not.
    Environments::on(&prod)
        .register("dev", None, OWNER, "where work starts", 1)
        .await
        .expect("dev");
    let e = apply(&prod, "0.3.0", false).await;
    // Staged, not activated: stored, so a pin to it must still be refused.
    assert!(e.is_ok(), "a staged apply stores: {e:?}");
    tenants::reserve(&prod, "spc_a", "spc_a", "dev:ada", 10, None)
        .await
        .expect("reserved");
    tenants::activate(&prod, "spc_a", 11).await.expect("active");
    let e = Versions::on(&prod)
        .pin("spc_a", "0.3.0", OWNER, 100)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not_promoted"), "{e}");
    assert!(e.to_string().contains("ordo promote 0.3.0"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_changes_which_environment_this_deployment_is() {
    let pool = fresh("ordo_promo_own").await;
    let stream = std::sync::Arc::new(ordo_secevent::Recorder::default());
    let o = ops(&pool, Some(stream.clone()));
    for name in ["dev", "staging"] {
        run(
            &o,
            deploy_ops::REGISTER_ENVIRONMENT,
            serde_json::json!({"name": name, "reason": "chain"}),
        )
        .await
        .expect("registered");
    }
    let r = run(
        &o,
        deploy_ops::SET_OWN_ENVIRONMENT,
        serde_json::json!({"name": "staging", "reason": "this host is staging"}),
    )
    .await
    .expect("set");
    assert_eq!(r.rows[0]["own"], "staging");
    assert_eq!(r.rows[0]["changed"], true);
    assert_eq!(Environments::on(&pool).own().await.unwrap().name, "staging");
    let again = run(
        &o,
        deploy_ops::SET_OWN_ENVIRONMENT,
        serde_json::json!({"name": "staging", "reason": "again"}),
    )
    .await
    .expect("unchanged");
    assert_eq!(again.rows[0]["changed"], false);

    let e = run(
        &o,
        deploy_ops::SET_OWN_ENVIRONMENT,
        serde_json::json!({"name": "qa", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    assert!(
        e.message.contains("ordo environments register qa"),
        "{}",
        e.message
    );
    let e = run(
        &o,
        deploy_ops::SET_OWN_ENVIRONMENT,
        serde_json::json!({"name": "dev", "reason": ""}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParam);
    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(
        acts,
        [
            "environment-registered",
            "environment-registered",
            "environment-own"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_is_removed_or_renamed_unless_it_is_owned_or_named() {
    let pool = deployment("ordo_promo_rm", &["dev", "staging"], "staging").await;
    let stream = std::sync::Arc::new(ordo_secevent::Recorder::default());
    let o = ops(&pool, Some(stream.clone()));

    // The deployment's own stays.
    let e = run(
        &o,
        deploy_ops::REMOVE_ENVIRONMENT,
        serde_json::json!({"name": "staging", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::ApplyRefused);
    assert!(
        e.message.contains("ordo environments set-own"),
        "{}",
        e.message
    );

    // Rename, then remove one nothing names.
    let r = run(
        &o,
        deploy_ops::RENAME_ENVIRONMENT,
        serde_json::json!({"name": "dev", "to": "develop", "reason": "clearer"}),
    )
    .await
    .expect("renamed");
    assert_eq!(r.rows[0]["chain"][0]["name"], "develop");
    let e = run(
        &o,
        deploy_ops::RENAME_ENVIRONMENT,
        serde_json::json!({"name": "develop", "to": "staging", "reason": "x"}),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::Duplicate);
    let r = run(
        &o,
        deploy_ops::REMOVE_ENVIRONMENT,
        serde_json::json!({"name": "develop", "reason": "not used"}),
    )
    .await
    .expect("removed");
    let names: Vec<_> = r.rows[0]["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["staging", "production"]);
    assert_eq!(r.rows[0]["chain"][0]["position"], 1);

    // A stored promotion names `staging` and `dev`: both stay.
    let dev = dev("ordo_promo_d7").await;
    set_secret(&pool, "hookKey").await;
    // This pool's chain has no dev now; rebuild one that does.
    let staging = deployment("ordo_promo_rm2", &["dev", "staging"], "staging").await;
    set_secret(&staging, "hookKey").await;
    promote(&staging, handed_over(&dev, "0.1.0").await)
        .await
        .expect("promoted");
    let o2 = ops(&staging, Some(stream.clone()));
    for (op, params) in [
        (
            deploy_ops::REMOVE_ENVIRONMENT,
            serde_json::json!({"name": "dev", "reason": "x"}),
        ),
        (
            deploy_ops::RENAME_ENVIRONMENT,
            serde_json::json!({"name": "dev", "to": "develop", "reason": "x"}),
        ),
    ] {
        let e = run(&o2, op, params).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ApplyRefused, "{op}");
        assert!(
            e.message.contains("stored promotion record"),
            "{}",
            e.message
        );
        assert!(
            e.message.contains("ordo environments register"),
            "names what to do: {}",
            e.message
        );
    }
    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(acts, ["environment-renamed", "environment-removed"]);
}
