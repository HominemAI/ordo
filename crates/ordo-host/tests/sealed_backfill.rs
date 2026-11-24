//! Sealed backfills (`plan-versions.md` increment 13;
//! `plan-backfill-dual-writes.md` §7), against real Postgres:
//!
//! - a stored step that makes a field `sealed` seals every row of 3 spaces
//!   through the backfill worker, one `Purpose::Migration` lease per space,
//!   verifies by each space's count and content hash and by opening every
//!   value against its plaintext, then drops the plaintext column. Every
//!   value opens at its row; no plaintext column remains;
//! - a seal killed by its budget resumes from its cursor, the plaintext
//!   kept and the version before still recorded until it verifies;
//! - a plaintext changed after its chunk fails verify, naming the row, and
//!   the plaintext column is kept; a run again seals every row over;
//! - a node with no key custody refuses the seal and keeps the plaintext;
//! - `ordo apply` refuses the seal with the command that runs it, and a
//!   field made `sealed indexed` is refused by name.
//!
//! A regression looks like a plaintext column left beside a sealed one, a
//! value sealed at the wrong row, a seal that starts over after a kill, or
//! a lease taken under another purpose.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;

use ordo_conformance::Pg;
use ordo_crypto::{KeyProvider, Purpose, SpaceId, TestProvider};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::backfill::BackfillSettings;
use ordo_host::evolve::{self, EvolveRefusal, StepContext, StepRun};
use ordo_host::schema_apply::{self as sa, Choices, Program};
use ordo_host::versions::Versions;

const OWNER: &str = "deployment-owner:ada";
const SPACES: [&str; 3] = ["acme", "beta", "gamma"];

/// `body` has a default, as notes' does: a sealed field's column has none.
fn v1() -> String {
    "domain notes @ 1.0.0\n\nentity note {\n  title: text\n  body: text = \"\"\n}\n\n\
     op allNotes unbounded =\n  note[title]\n"
        .into()
}

/// v1 with `body` sealed: B7.
fn v2() -> String {
    "domain notes @ 1.1.0\n\nentity note {\n  title: text\n  body: text sealed\n}\n\n\
     op allNotes unbounded =\n  note[title]\n"
        .into()
}

/// v1 with `body` sealed and indexed: B7 and B8 at once.
fn v2_indexed() -> String {
    "domain notes @ 1.1.0\n\nentity note {\n  title: text\n  body: text sealed indexed\n}\n\n\
     op allNotes unbounded =\n  note[title]\n"
        .into()
}

fn cfg(db: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        size: 4,
        ..PoolConfig::from_env()
    }
}

async fn empty(stem: &str) -> (Pg, Pool) {
    let pg = Pg::scoped(stem);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(cfg(&pg.db)).await.expect("a pool");
    (pg, pool)
}

async fn store(pool: &Pool, source: &str) -> Program {
    let p = Program::compile(source).expect("compiles");
    Versions::on(pool).ensure().await.expect("registry");
    ordo_exec::registry::store(
        pool,
        &ordo_exec::registry::Artifact {
            version: p.version.clone(),
            artifact: ordo_plan::artifact::to_bytes(&p.doc),
            source: p.source.clone(),
            ddl: p.ddl.clone(),
            applied_at: ordo_commandlog::Timestamp(0),
            actor: "dev:ada".into(),
        },
    )
    .await
    .expect("stored");
    p
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

fn recorded(pg: &Pg) -> String {
    one(
        pg,
        "SELECT to_version FROM \"$migrations\" WHERE phase <> 'failed' ORDER BY seq DESC LIMIT 1",
    )
}

fn column_type(pg: &Pg, column: &str) -> String {
    one(
        pg,
        &format!(
            "SELECT data_type FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = '{column}'"
        ),
    )
}

/// A registry with v1 and `next` stored and the step stored, and one
/// tenant database on v1 holding 120 notes in each of 3 spaces.
async fn setup(stem: &str, next: &str) -> (Pg, Pool, Pg, Pool, evolve::StoredPlan) {
    let (reg_pg, registry) = empty(&format!("{stem}_reg")).await;
    let (tenant_pg, tenant) = empty(&format!("{stem}_tenant")).await;
    let first = store(&registry, &v1()).await;
    store(&registry, next).await;
    let done = evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect("the step stores");
    let (_, created) = sa::apply(&tenant, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    for s in SPACES {
        tenant_pg
            .exec(&format!(
                "INSERT INTO note (\"$space\", id, rev, title, body) \
                 SELECT '{s}', 'n' || lpad(i::text, 3, '0'), 1, '{s} ' || i, 'secret {s} ' || i \
                 FROM generate_series(0, 119) i"
            ))
            .expect("rows");
    }
    (reg_pg, registry, tenant_pg, tenant, done.plan)
}

fn ctx(keys: Option<Arc<dyn KeyProvider>>, chunk_rows: u32, budget: Option<u32>) -> StepContext {
    StepContext {
        settings: BackfillSettings {
            chunk_rows,
            budget,
            ..BackfillSettings::default()
        },
        functions: None,
        keys,
    }
}

/// Custody that counts each release by space and purpose.
fn metered() -> (Arc<dyn KeyProvider>, Arc<ordo_host::metrics::Metrics>) {
    let metrics = Arc::new(ordo_host::metrics::Metrics::new());
    let keys: Arc<dyn KeyProvider> = Arc::new(ordo_host::metrics::MeteredKeys::new(
        Arc::new(TestProvider::new()),
        metrics.clone(),
    ));
    (keys, metrics)
}

/// Releases counted for `purpose`, per space.
fn releases(m: &ordo_host::metrics::Metrics, purpose: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for (name, labels, n) in m.snapshot() {
        if name != ordo_host::metrics::KEY_RELEASES {
            continue;
        }
        let get = |k: &str| {
            labels
                .iter()
                .find(|(l, _)| l == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        if get("purpose") == purpose {
            out.push((get("space"), n));
        }
    }
    out.sort();
    out
}

/// Open the sealed `body` of `space/id` as a read would.
fn open(keys: &Arc<dyn KeyProvider>, pg: &Pg, space: &str, id: &str) -> String {
    let hexed = one(
        pg,
        &format!(
            "SELECT encode(body, 'hex') FROM note WHERE \"$space\" = '{space}' AND id = '{id}'"
        ),
    );
    let bytes: Vec<u8> = (0..hexed.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hexed[i..i + 2], 16).expect("hex"))
        .collect();
    let lease = keys
        .release(&SpaceId(space.into()), Purpose::Read, "test")
        .expect("a lease");
    String::from_utf8(
        keys.open_at(&lease, "note", "body", ordo_crypto::Anchor::Row(id), &bytes)
            .expect("opens at its row"),
    )
    .expect("utf-8")
}

/// **Sealing on 3 spaces verifies by count and hash, and no plaintext
/// column remains** (13's "done when", without the live writes of 11a).
#[tokio::test(flavor = "multi_thread")]
async fn a_stored_step_seals_every_row_and_drops_the_plaintext() {
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_seal_all", &v2()).await;
    let (keys, metrics) = metered();

    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("json");
    let receipt = &receipt;
    let bf = &receipt["backfills"][0];
    assert_eq!(bf["code"], "B7", "{receipt}");
    assert_eq!(bf["mapping"], "seal");
    assert_eq!(bf["sealFrom"], "body");
    assert_eq!(bf["column"], "body$seal");
    assert!(
        bf["plaintext"]
            .as_str()
            .is_some_and(|p| p.contains("dropped")),
        "the receipt says the plaintext is dropped: {bf}"
    );
    let after = receipt["after"].to_string();
    assert!(after.contains("DROP COLUMN \\\"body\\\""), "{after}");

    let ran = evolve::run_step_with(
        &registry,
        &tenant,
        "tenant-db",
        &plan,
        OWNER,
        &ctx(Some(keys.clone()), 100, None),
    )
    .await
    .expect("the seal runs");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(recorded(&tpg), "1.1.0");

    // No plaintext column remains; the sealed one has the field's name.
    assert_eq!(column_type(&tpg, "body"), "bytea");
    assert_eq!(
        column_type(&tpg, "body$seal"),
        "",
        "the working column is gone"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = 'body'"
        ),
        "NO",
        "a required field is required again"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM note WHERE position('secret'::bytea in body) > 0"
        ),
        "0",
        "no row holds its plaintext"
    );

    // Every value opens at its row to what it was.
    for s in SPACES {
        assert_eq!(open(&keys, &tpg, s, "n000"), format!("secret {s} 0"));
        assert_eq!(open(&keys, &tpg, s, "n119"), format!("secret {s} 119"));
    }

    // Verify: each space's count and hash, recorded once in its chain.
    let reports = ran.backfills();
    let spaces = reports[0]["verified"]["spaces"].as_array().expect("spaces");
    assert_eq!(spaces.len(), 3, "{reports:?}");
    for (s, v) in SPACES.iter().zip(spaces) {
        assert_eq!(v["space"], *s);
        assert_eq!(v["rows"], 120);
        assert_eq!(v["content"].as_str().map(str::len), Some(64));
    }
    for s in SPACES {
        assert_eq!(
            one(
                &tpg,
                &format!("SELECT count(*) FROM \"$commandlog\" WHERE space = '{s}'")
            ),
            "1",
            "one summarised group in `{s}`'s chain"
        );
    }

    // One `migration` lease per space per chunk (2 chunks each at 100
    // rows), and per space for verify. Nothing under another purpose.
    let got = releases(&metrics, "migration");
    assert_eq!(
        got.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(),
        SPACES.to_vec(),
        "{got:?}"
    );
    assert!(got.iter().all(|(_, n)| *n >= 2), "{got:?}");
    assert!(releases(&metrics, "write").is_empty());
    assert!(releases(&metrics, "reseal").is_empty());
}

/// **A seal stopped midway resumes from its cursor**: the plaintext is
/// kept and the database records the version before until it verifies.
#[tokio::test(flavor = "multi_thread")]
async fn a_seal_stopped_midway_resumes_from_its_cursor() {
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_seal_resume", &v2()).await;
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());

    let ran = evolve::run_step_with(
        &registry,
        &tenant,
        "tenant-db",
        &plan,
        OWNER,
        &ctx(Some(keys.clone()), 100, Some(1)),
    )
    .await
    .expect("one chunk runs");
    assert!(matches!(ran, StepRun::Backfilling { .. }), "{ran:?}");
    assert_eq!(recorded(&tpg), "1.0.0");
    assert_eq!(column_type(&tpg, "body"), "text", "the plaintext is kept");
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM note WHERE \"body$seal\" IS NOT NULL"
        ),
        "100"
    );

    let ran = evolve::run_step_with(
        &registry,
        &tenant,
        "tenant-db",
        &plan,
        OWNER,
        &ctx(Some(keys.clone()), 100, None),
    )
    .await
    .expect("the rest runs");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(
        ran.backfills()[0]["progress"]["rowsThisRun"],
        260,
        "only the rows after the cursor"
    );
    assert_eq!(recorded(&tpg), "1.1.0");
    assert_eq!(column_type(&tpg, "body"), "bytea");
    assert_eq!(open(&keys, &tpg, "gamma", "n050"), "secret gamma 50");
}

/// **A plaintext changed after its chunk fails verify**, naming the row.
/// Nothing is dropped and the version before is still recorded.
#[tokio::test(flavor = "multi_thread")]
async fn a_plaintext_changed_after_its_chunk_fails_verify() {
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_seal_verify", &v2()).await;
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let ran = evolve::run_step_with(
        &registry,
        &tenant,
        "tenant-db",
        &plan,
        OWNER,
        &ctx(Some(keys.clone()), 100, Some(1)),
    )
    .await
    .expect("one chunk runs");
    assert!(matches!(ran, StepRun::Backfilling { .. }), "{ran:?}");
    // A write the serving version makes behind the cursor. Dual writes
    // (11a) are not built, so verify is what catches it.
    tpg.exec("UPDATE note SET body = 'tampered xyz' WHERE \"$space\" = 'acme' AND id = 'n010'")
        .expect("a write");
    let err = evolve::run_step_with(
        &registry,
        &tenant,
        "tenant-db",
        &plan,
        OWNER,
        &ctx(Some(keys.clone()), 100, None),
    )
    .await
    .expect_err("verify refuses");
    let text = err.to_string();
    assert!(text.contains("backfill_verify_failed"), "{text}");
    assert!(text.contains("acme/n010"), "names the row: {text}");
    assert!(!text.contains("tampered"), "never the value: {text}");
    assert_eq!(recorded(&tpg), "1.0.0");
    assert_eq!(column_type(&tpg, "body"), "text", "the plaintext is kept");

    // A run again starts the seal over, and seals the plaintext as it is.
    let ran = evolve::run_step_with(
        &registry,
        &tenant,
        "tenant-db",
        &plan,
        OWNER,
        &ctx(Some(keys.clone()), 100, None),
    )
    .await
    .expect("the run again seals and verifies");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(recorded(&tpg), "1.1.0");
    assert_eq!(open(&keys, &tpg, "acme", "n010"), "tampered xyz");
}

/// **No key custody, no seal**: the step is refused naming why, and the
/// plaintext is kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_no_key_custody_refuses_the_seal() {
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_seal_nokeys", &v2()).await;
    let err = evolve::run_step_with(
        &registry,
        &tenant,
        "tenant-db",
        &plan,
        OWNER,
        &ctx(None, 100, None),
    )
    .await
    .expect_err("refused");
    let text = err.to_string();
    assert!(text.contains("no key custody"), "{text}");
    assert!(matches!(err, EvolveRefusal::Run { .. }), "{err:?}");
    assert_eq!(recorded(&tpg), "1.0.0");
    assert_eq!(column_type(&tpg, "body"), "text");
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM note WHERE \"body$seal\" IS NOT NULL"
        ),
        "0"
    );
}

/// **`ordo apply` refuses the seal** with the command that runs it; a
/// field made `sealed indexed` is refused by name in the render.
#[tokio::test(flavor = "multi_thread")]
async fn the_apply_path_and_a_sealed_index_are_refused_by_name() {
    let (_tpg, tenant) = empty("ordo_seal_apply").await;
    let first = Program::compile(&v1()).expect("v1");
    let (_, created) = sa::apply(&tenant, &first, "test", "test").await;
    assert!(matches!(created, Ok(sa::Applied::Created(_))));
    let next = Program::compile(&v2()).expect("v2");
    let (_, applied) = sa::apply(&tenant, &next, "test", "test").await;
    let text = format!("{applied:?}");
    assert!(text.contains("seal_needs_stored_plan"), "{text}");
    assert!(text.contains("ordo evolve"), "{text}");

    let (_rpg, registry) = empty("ordo_seal_indexed_reg").await;
    store(&registry, &v1()).await;
    store(&registry, &v2_indexed()).await;
    let err = evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect_err("refused");
    let text = format!("{err:?}");
    assert!(text.contains("sealed_index_backfill_not_built"), "{text}");
}
