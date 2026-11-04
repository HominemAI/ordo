//! Increment 2 of `plan-versions.md`, session 2: a person can address a
//! stored version (review V-32).
//!
//! - An apply stores a label and the source's git commit with its version.
//!   A re-apply of the same version replaces the label and keeps the commit.
//! - A label is changed or cleared later, the version named by `#n`.
//! - A label over 80 characters, or on two lines, is refused saying so.
//! - `std.deployment.versions` carries both.
//! - Two versions are compared: the schema change and the source diff.
//! - A name that is not stored is refused, naming the ones that are.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_api::BoundParams;
use ordo_conformance::Pg;
use ordo_exec::route::SinglePool;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::apply_op::{self, ApplyRefused};
use ordo_host::schema_apply::Program;
use ordo_host::versions::{Provenance, VersionRefusal, Versions};
use ordo_host::{deploy_ops, versions_diff};

const V010: &str = "domain tasks @ 0.1.0

entity task {
  title: text
  done:  bool
}

op openTasks unbounded =
  task[title, done]
";

/// `V010` with a field added, under the next number.
const V020: &str = "domain tasks @ 0.2.0

entity task {
  title: text
  done:  bool
  due:   int?
}

op openTasks unbounded =
  task[title, done]
";

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

async fn fresh(stem: &str) -> Pool {
    let pg = Pg::scoped(stem);
    Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool")
}

async fn apply(
    pool: &Pool,
    source: &str,
    label: Option<&str>,
    commit: Option<&str>,
) -> Result<apply_op::Report, ApplyRefused> {
    let program = Program::compile(source).expect("compiles");
    apply_op::run_program_with(
        pool,
        &SinglePool::new(pool.clone()),
        &program,
        vec![],
        "dev:ada",
        "t",
        true,
        &Provenance {
            label: label.map(str::to_string),
            git_commit: commit.map(str::to_string),
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_apply_stores_a_label_and_a_commit_and_a_reapply_relabels() {
    let pool = fresh("ordo_labels_apply").await;
    let r = apply(&pool, V010, Some("first cut"), Some(COMMIT))
        .await
        .expect("applies");
    assert_eq!(r.provenance.label.as_deref(), Some("first cut"));
    assert_eq!(r.provenance.git_commit.as_deref(), Some(COMMIT));

    // The same program again, from another checkout: the label moves, the
    // commit it came from stays.
    let dirty = format!("{}-dirty", &COMMIT[..12]);
    let again = apply(&pool, V010, Some("launch day"), Some(&dirty))
        .await
        .expect("applies");
    assert_eq!(again.version, "0.1.0");
    assert_eq!(again.provenance.label.as_deref(), Some("launch day"));
    assert_eq!(again.provenance.git_commit.as_deref(), Some(COMMIT));

    // No label given keeps the one there.
    let third = apply(&pool, V010, None, None).await.expect("applies");
    assert_eq!(third.provenance.label.as_deref(), Some("launch day"));

    let history = apply_op::history(&pool, 5)
        .await
        .expect("history")
        .to_json();
    let row = &history["stored"][0];
    assert_eq!(row["label"], "launch day");
    assert_eq!(row["gitCommit"], COMMIT);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_label_is_refused_before_anything_is_applied() {
    let pool = fresh("ordo_labels_refused").await;
    let long = "x".repeat(81);
    let e = apply(&pool, V010, Some(&long), None).await.unwrap_err();
    assert_eq!(e.class(), "provenance_refused");
    assert!(e.to_string().contains("at most 80"), "{e}");
    let e = apply(&pool, V010, Some("one\ntwo"), None)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("one line"), "{e}");
    let e = apply(&pool, V010, None, Some("main")).await.unwrap_err();
    assert_eq!(e.class(), "provenance_refused");
    // Nothing was stored: the registry was never reached.
    assert!(apply_op::history(&pool, 5)
        .await
        .expect("history")
        .stored
        .is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_label_is_set_by_order_number_and_cleared() {
    let pool = fresh("ordo_labels_set").await;
    apply(&pool, V010, None, None).await.expect("applies");
    apply(&pool, V020, None, None).await.expect("applies");
    let v = Versions::on(&pool);

    assert_eq!(v.set_label("#1", Some(" beta ")).await.unwrap(), "0.1.0");
    assert_eq!(
        v.provenance("0.1.0").await.unwrap().label.as_deref(),
        Some("beta")
    );
    assert_eq!(v.set_label("0.1.0", None).await.unwrap(), "0.1.0");
    assert_eq!(v.provenance("0.1.0").await.unwrap().label, None);

    let long = v.set_label("#2", Some(&"y".repeat(81))).await.unwrap_err();
    assert!(matches!(long, VersionRefusal::Label(_)), "{long}");
    assert!(long.to_string().contains("at most 80"), "{long}");

    let missing = v.set_label("#9", Some("x")).await.unwrap_err();
    let text = missing.to_string();
    assert!(
        matches!(missing, VersionRefusal::NotStored { .. }),
        "{text}"
    );
    assert!(
        text.contains("#1 · 0.1.0") && text.contains("#2 · 0.2.0"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_door_labels_and_compares_as_the_client_does() {
    let pool = fresh("ordo_labels_door").await;
    apply(&pool, V010, None, None).await.expect("applies");
    apply(&pool, V020, None, None).await.expect("applies");
    let ops = apply_op::ApplyOps::new(
        pool.clone(),
        std::sync::Arc::new(SinglePool::new(pool.clone())),
        "home",
    );
    let bound = |v: serde_json::Value| BoundParams(v.as_object().unwrap().clone());

    let r = deploy_ops::execute(
        &ops,
        deploy_ops::LABEL,
        "home",
        &bound(serde_json::json!({"version": "#2", "label": "rc"})),
        "owner",
    )
    .await
    .expect("labelled");
    assert_eq!(
        r.rows[0],
        serde_json::json!({"version": "0.2.0", "ordinal": 2, "label": "rc"})
    );

    let long = deploy_ops::execute(
        &ops,
        deploy_ops::LABEL,
        "home",
        &bound(serde_json::json!({"version": "#2", "label": "z".repeat(81)})),
        "owner",
    )
    .await
    .unwrap_err();
    assert_eq!(long.code, ordo_api::ErrorCode::InvalidParam);
    assert!(long.message.contains("at most 80"), "{}", long.message);

    // Bound to a tenant's space, it is not there.
    let elsewhere = deploy_ops::execute(
        &ops,
        deploy_ops::LABEL,
        "spc_tenant",
        &bound(serde_json::json!({"version": "#2"})),
        "owner",
    )
    .await
    .unwrap_err();
    assert_eq!(elsewhere.code, ordo_api::ErrorCode::NotFound);

    let d = deploy_ops::execute(
        &ops,
        deploy_ops::DIFF,
        "home",
        &bound(serde_json::json!({"a": "#1", "b": "0.2.0", "source": false})),
        "owner",
    )
    .await
    .expect("compared");
    let d = versions_diff::VersionDiff::from_json(&d.rows[0]).expect("reads back");
    assert_eq!(d.b.label.as_deref(), Some("rc"));
    assert!(d.source.is_none());
    assert_eq!(d.schema.expect("asked").expect("worked out").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_versions_are_compared_by_schema_and_source() {
    let pool = fresh("ordo_labels_diff").await;
    apply(&pool, V010, None, None).await.expect("applies");
    apply(&pool, V020, None, None).await.expect("applies");

    let d = versions_diff::diff(&pool, "#1", "#2", true, true)
        .await
        .expect("compared");
    assert_eq!(d.a.named(), "#1 · 0.1.0");
    assert_eq!(d.b.named(), "#2 · 0.2.0");
    let schema = d.schema.clone().expect("asked").expect("worked out");
    assert_eq!(schema.len(), 1, "{schema:?}");
    assert_eq!(schema[0].code, "A2");
    assert!(schema[0].change.contains("due"), "{schema:?}");
    let source = d.source.clone().expect("asked");
    assert!(
        source.contains(&"+  due:   int?".to_string()),
        "{source:#?}"
    );
    assert!(source.contains(&"-domain tasks @ 0.1.0".to_string()));
    let text = d.render();
    assert!(text.contains("SCHEMA  (1 change(s))"), "{text}");
    assert!(text.contains("@@ "), "{text}");

    // The shadow it built is gone.
    let left = pool
        .rows(&format!(
            "SELECT count(*) FROM pg_namespace WHERE nspname LIKE '{}%'",
            ordo_exec::shape::DIFF_SHADOW
        ))
        .await
        .expect("counted");
    assert_eq!(left[0][0].as_deref(), Some("0"));

    // A version with itself: nothing changed.
    let same = versions_diff::diff(&pool, "0.2.0", "#2", true, true)
        .await
        .expect("compared");
    assert_eq!(same.schema, Some(Ok(vec![])));
    assert_eq!(same.source, Some(vec![]));

    // Only the source, then a name nobody stored.
    let only = versions_diff::diff(&pool, "#2", "#1", false, true)
        .await
        .expect("compared");
    assert!(only.schema.is_none());
    let missing = versions_diff::diff(&pool, "#1", "0.9.0", true, true)
        .await
        .unwrap_err()
        .to_string();
    assert!(missing.contains("`0.9.0`"), "{missing}");
    assert!(missing.contains("#2 · 0.2.0"), "{missing}");
}
