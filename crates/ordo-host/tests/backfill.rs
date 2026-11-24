//! The backfill worker (`plan-versions.md` increment 10b;
//! `plan-backfill-dual-writes.md` B1, B3, B6), against real Postgres:
//!
//! - a backfill killed midway, with a chunk in flight, resumes from its
//!   cursor: the database still serves the version before the step, reads
//!   of the old shape are right, and the next run fills only what is left;
//! - verify: every row holds the mapping, each space's count and content
//!   hash recompute; a value changed after its chunk fails verify, naming
//!   the space, and nothing is made required;
//! - the audit record: one summarised group per space in each space's
//!   chain, or with `--audit-images` one image group per space per chunk,
//!   each converge with full images; the chain verifies either way;
//! - retry and the dead letter: a chunk that cannot get its lock fails 5
//!   times and the cursor is `dead`, naming the error; a run again finishes;
//! - the render: a field made required with no fill names `--fill`; a
//!   mapping over a missing field, or of the wrong type, is a plan error; a
//!   stray fill is refused; a fill shows in the receipt and the digest;
//! - backfill functions (B6): called once per chunk with exactly the
//!   secrets and hosts the plan declares (Q14), its values written; a
//!   function the version lacks, or a host it does not declare, is a plan
//!   error; a function that calls out is marked not reproducible.
//!
//! A regression looks like a backfill that starts over after a kill, a
//! tenant serving a half-filled column as required, a summary written
//! twice, or a function reaching a host the plan did not show.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_commandlog::{BackfillAudit, CommandGroup};
use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::backfill::{
    Backfill, BackfillFunctions, BackfillRefusal, BackfillSettings, FunctionChunk,
};
use ordo_host::evolve::{self, EvolveRefusal, StepContext, StepRun};
use ordo_host::schema_apply::{self as sa, Choices, Program};
use ordo_host::versions::Versions;
use serde_json::{json, Value};

const OWNER: &str = "deployment-owner:ada";

fn v1() -> String {
    "domain notes @ 1.0.0\n\nentity note {\n  title: text\n  body: text?\n}\n\n\
     op allNotes unbounded =\n  note[title]\n"
        .into()
}

/// v1 with `heading`, a new required field with no default: B2.
fn v2() -> String {
    "domain notes @ 1.1.0\n\nentity note {\n  title: text\n  body: text?\n  heading: text\n}\n\n\
     op allNotes unbounded =\n  note[title, heading]\n"
        .into()
}

/// v2 with a function that may reach one host and one secret.
fn v2_fn() -> String {
    "domain notes @ 1.1.0\n\nentity note {\n  title: text\n  body: text?\n  heading: text\n}\n\n\
     op allNotes unbounded =\n  note[title, heading]\n\n\
     secret apiKey\n\n\
     function headline in notes {\n  call() -> json\n  code \"fn/headline.js\"\n  \
     egress api.example.com\n  secrets apiKey\n}\n"
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

/// Store a version in the registry, as `ordo apply --no-activate` leaves
/// it. Each function gets a code hash, as the apply gives it.
async fn store(pool: &Pool, source: &str) -> Program {
    let mut p = Program::compile(source).expect("compiles");
    if let Some(fs) = p.doc["functions"].as_array_mut() {
        for f in fs {
            f["code_hash"] = json!("sha256:headline");
        }
    }
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

fn fills(spec: &str, images: &str) -> Choices {
    evolve::typed_options("", spec, images, "").expect("options parse")
}

/// A registry with v1 and `next` stored, the step stored with `choices`,
/// and a tenant database on v1 holding 150 notes in `acme` and 100 in
/// `beta`, ids `n000` up.
async fn setup(
    stem: &str,
    next: &str,
    choices: &Choices,
) -> (Pg, Pool, Pg, Pool, evolve::StoredPlan) {
    let (reg_pg, registry) = empty(&format!("{stem}_reg")).await;
    let (tenant_pg, tenant) = empty(&format!("{stem}_tenant")).await;
    let first = store(&registry, &v1()).await;
    store(&registry, next).await;
    let done = evolve::evolve(&registry, "1.0.0", "1.1.0", choices, None, OWNER, 1)
        .await
        .expect("the step stores");
    let (_, created) = sa::apply(&tenant, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    tenant_pg
        .exec(
            "INSERT INTO note (\"$space\", id, rev, title) \
             SELECT 'acme', 'n' || lpad(i::text, 3, '0'), 1, 'acme ' || i FROM generate_series(0, 149) i; \
             INSERT INTO note (\"$space\", id, rev, title) \
             SELECT 'beta', 'n' || lpad(i::text, 3, '0'), 1, 'beta ' || i FROM generate_series(0, 99) i",
        )
        .expect("rows");
    (reg_pg, registry, tenant_pg, tenant, done.plan)
}

fn settings(chunk_rows: u32, budget: Option<u32>) -> StepContext {
    StepContext {
        settings: BackfillSettings {
            chunk_rows,
            budget,
            ..BackfillSettings::default()
        },
        functions: None,
        keys: None,
        fence: None,
    }
}

/// The backfill groups in `space`'s chain, decoded, after checking the
/// whole chain links and hashes.
fn backfill_groups(pg: &Pg, space: &str) -> Vec<CommandGroup> {
    let rows = pg
        .query(&format!(
            "SELECT prev, hash, body FROM \"$commandlog\" WHERE space = '{space}' ORDER BY idx"
        ))
        .expect("the chain");
    let mut prev = ordo_commandlog::ChainHash::GENESIS;
    let mut out = Vec::new();
    for r in rows {
        let bytes = |i: usize| ordo_exec::commandlog::bytea_of(r[i].as_ref()).expect("bytea");
        let group = ordo_commandlog::decode_group(&bytes(2)).expect("a group decodes");
        assert_eq!(bytes(0), prev.0.to_vec(), "the chain links");
        let hash = ordo_commandlog::hash_group(prev, &group);
        assert_eq!(bytes(1), hash.0.to_vec(), "the stored hash recomputes");
        prev = hash;
        if group.backfill.is_some() {
            out.push(group);
        }
    }
    out
}

fn recorded(pg: &Pg) -> String {
    one(
        pg,
        "SELECT to_version FROM \"$migrations\" WHERE phase <> 'failed' ORDER BY seq DESC LIMIT 1",
    )
}

/// **A backfill killed midway resumes from its cursor** (10b's "done
/// when"). One chunk commits, the next is killed while it waits on a row
/// lock, and the run after fills only the 150 rows left. Through it all
/// the database serves 1.0.0 and reads the old shape right; after it, the
/// field is required, every row holds its title, and each space's chain
/// holds one summarised record.
#[tokio::test(flavor = "multi_thread")]
async fn a_backfill_killed_midway_resumes_from_its_cursor() {
    let choices = fills("note.heading=title", "");
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_backfill_kill", &v2(), &choices).await;

    // Part 1: one chunk, then the budget stops it.
    let ran = evolve::run_step_with(
        &registry,
        &tenant,
        "acme-db",
        &plan,
        OWNER,
        &settings(100, Some(1)),
    )
    .await
    .expect("the first chunk runs");
    assert!(matches!(ran, StepRun::Backfilling { .. }), "{ran:?}");
    assert_eq!(recorded(&tpg), "1.0.0", "the database still serves 1.0.0");
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM note WHERE heading IS NOT NULL"),
        "100"
    );
    assert_eq!(
        one(&tpg, "SELECT is_nullable FROM information_schema.columns WHERE table_name = 'note' AND column_name = 'heading'"),
        "YES",
        "the column is not required while it fills"
    );
    // The old shape reads as it did.
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM note WHERE title IS NOT NULL"),
        "250"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT cursor_space || '/' || cursor_id || ' ' || rows_done FROM \"$backfills\""
        ),
        "acme/n099 100"
    );

    // Part 2: kill the next chunk while it waits on a row it must fill.
    let holder = Pool::connect(cfg(&tpg.db)).await.expect("a second pool");
    let held = holder.begin().await.expect("a transaction");
    held.rows("SELECT 1 FROM note WHERE \"$space\" = 'acme' AND id = 'n120' FOR UPDATE")
        .await
        .expect("the row is held");
    let (r, t) = (registry.clone(), tenant.clone());
    let p = plan.clone();
    let worker = tokio::spawn(async move {
        evolve::run_step_with(&r, &t, "acme-db", &p, OWNER, &settings(100, None)).await
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!worker.is_finished(), "the chunk waits on the held row");
    worker.abort();
    let _ = worker.await;
    held.rollback().await.expect("released");
    assert_eq!(
        one(&tpg, "SELECT rows_done FROM \"$backfills\""),
        "100",
        "the killed chunk committed nothing"
    );
    assert_eq!(recorded(&tpg), "1.0.0");

    // Part 3: the run after resumes. The killed chunk's backend may hold the
    // backfill lock for a moment after its client went; a busy answer is
    // retried, as a person would.
    let mut tries = 0;
    let ran = loop {
        match evolve::run_step_with(
            &registry,
            &tenant,
            "acme-db",
            &plan,
            OWNER,
            &settings(100, None),
        )
        .await
        {
            Err(EvolveRefusal::Run {
                class: "backfill_busy",
                ..
            }) if tries < 20 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            other => break other.expect("the step finishes"),
        }
    };
    let StepRun::Ran { backfills, .. } = &ran else {
        panic!("{ran:?}")
    };
    assert_eq!(
        backfills[0]["progress"]["rowsThisRun"], 150,
        "it filled what was left, not all 250 again: {backfills:?}"
    );
    assert_eq!(backfills[0]["verified"]["rows"], 250);
    assert_eq!(recorded(&tpg), "1.1.0");
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM note WHERE heading IS DISTINCT FROM title"
        ),
        "0"
    );
    assert_eq!(
        one(&tpg, "SELECT is_nullable FROM information_schema.columns WHERE table_name = 'note' AND column_name = 'heading'"),
        "NO",
        "the field is required once verified"
    );
    // One summarised record per space, attesting its own rows.
    for (space, rows) in [("acme", 150u64), ("beta", 100)] {
        let groups = backfill_groups(&tpg, space);
        assert_eq!(groups.len(), 1, "{space}: one record");
        let g = &groups[0];
        assert!(g.commands.is_empty());
        assert_eq!(g.actor.to_string(), "system:backfill");
        let rec = g.backfill.as_ref().unwrap();
        assert_eq!(rec.kind, BackfillAudit::Summarised);
        assert_eq!(rec.rows, rows);
        assert_eq!(
            (
                rec.entity.as_str(),
                rec.field.as_str(),
                rec.mapping.as_str()
            ),
            ("note", "heading", "title")
        );
        assert_eq!(rec.step, plan.digest);
    }
    // Running the step again changes nothing and writes no second record.
    let again = evolve::run_step_with(
        &registry,
        &tenant,
        "acme-db",
        &plan,
        OWNER,
        &settings(100, None),
    )
    .await
    .expect("runs again");
    assert_eq!(again, StepRun::Current);
    assert_eq!(backfill_groups(&tpg, "acme").len(), 1);
}

/// **Full images on request** (B3's option): one group per space per
/// chunk, each converge with the row's before and after images, and no
/// summary.
#[tokio::test(flavor = "multi_thread")]
async fn images_write_a_group_per_space_per_chunk() {
    let choices = fills("note.heading=title", "note.heading");
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_backfill_images", &v2(), &choices).await;
    let ran = evolve::run_step_with(&registry, &tenant, "db", &plan, OWNER, &settings(100, None))
        .await
        .expect("runs");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    // Chunks: acme 0-99 | acme 100-149 + beta 0-49 | beta 50-99.
    let acme = backfill_groups(&tpg, "acme");
    let beta = backfill_groups(&tpg, "beta");
    assert_eq!((acme.len(), beta.len()), (2, 2));
    let total: usize = acme.iter().chain(&beta).map(|g| g.commands.len()).sum();
    assert_eq!(total, 250);
    for g in acme.iter().chain(&beta) {
        let rec = g.backfill.as_ref().unwrap();
        assert_eq!(rec.kind, BackfillAudit::Images);
        assert_eq!(rec.rows as usize, g.commands.len());
        let c = &g.commands[0];
        let before = c.before.as_ref().unwrap();
        let after = c.after.as_ref().unwrap();
        assert_eq!(before.get("heading"), Some(&ordo_commandlog::Value::Null));
        assert_eq!(
            after.get("heading"),
            after.get("title"),
            "after holds the mapping"
        );
        assert_eq!(
            before.get("title"),
            after.get("title"),
            "a full image of the row"
        );
    }
}

/// **Verify catches a value changed after its chunk**, names the space,
/// and makes nothing required.
#[tokio::test(flavor = "multi_thread")]
async fn verify_refuses_a_value_changed_after_its_chunk() {
    let choices = fills("note.heading=title", "");
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_backfill_verify", &v2(), &choices).await;
    // Fill everything, stopping before verify: the budget is the chunks.
    let ran = evolve::run_step_with(
        &registry,
        &tenant,
        "db",
        &plan,
        OWNER,
        &settings(250, Some(1)),
    )
    .await
    .expect("fills");
    assert!(matches!(ran, StepRun::Backfilling { .. }), "{ran:?}");
    tpg.exec("UPDATE note SET heading = 'changed' WHERE \"$space\" = 'beta' AND id = 'n007'")
        .expect("a write behind the cursor");
    let err = evolve::run_step_with(&registry, &tenant, "db", &plan, OWNER, &settings(250, None))
        .await
        .expect_err("verify refuses");
    let text = err.to_string();
    assert!(text.contains("backfill_verify_failed"), "{text}");
    assert!(text.contains("beta/n007"), "names the first row: {text}");
    assert_eq!(recorded(&tpg), "1.0.0");
    assert_eq!(
        one(&tpg, "SELECT is_nullable FROM information_schema.columns WHERE table_name = 'note' AND column_name = 'heading'"),
        "YES"
    );
    assert!(
        backfill_groups(&tpg, "beta").is_empty(),
        "no record of a failed backfill"
    );
}

/// **5 failed chunks mark the cursor dead**, naming the last error; a run
/// again retries from the cursor and finishes.
#[tokio::test(flavor = "multi_thread")]
async fn five_failed_chunks_mark_the_cursor_dead_and_a_run_again_finishes() {
    let choices = fills("note.heading=title", "");
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_backfill_dead", &v2(), &choices).await;
    // The expand and a first chunk, then a row of the next chunk held.
    evolve::run_step_with(
        &registry,
        &tenant,
        "db",
        &plan,
        OWNER,
        &settings(100, Some(1)),
    )
    .await
    .expect("one chunk");
    let holder = Pool::connect(cfg(&tpg.db)).await.expect("a second pool");
    let held = holder.begin().await.expect("a transaction");
    held.rows("SELECT 1 FROM note WHERE \"$space\" = 'acme' AND id = 'n100' FOR UPDATE")
        .await
        .expect("a row of the next chunk is held");
    let quick = StepContext {
        settings: BackfillSettings {
            chunk_rows: 100,
            chunk_timeout: Duration::from_millis(200),
            ..BackfillSettings::default()
        },
        functions: None,
        keys: None,
        fence: None,
    };
    let err = evolve::run_step_with(&registry, &tenant, "db", &plan, OWNER, &quick)
        .await
        .expect_err("every chunk times out");
    let text = err.to_string();
    assert!(text.contains("backfill_dead"), "{text}");
    assert!(text.contains("5 chunks"), "{text}");
    assert!(text.contains("timeout"), "names the error: {text}");
    assert_eq!(
        one(&tpg, "SELECT state || ' ' || attempts FROM \"$backfills\""),
        "dead 5"
    );
    assert_eq!(
        one(&tpg, "SELECT rows_done FROM \"$backfills\""),
        "100",
        "the cursor stayed"
    );
    assert_eq!(recorded(&tpg), "1.0.0");
    held.rollback().await.expect("released");
    let ran = evolve::run_step_with(&registry, &tenant, "db", &plan, OWNER, &settings(100, None))
        .await
        .expect("a run again finishes");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(one(&tpg, "SELECT state FROM \"$backfills\""), "done");
}

/// **The render refuses what a backfill cannot do**, each naming its fix,
/// and stores nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_render_refuses_a_fill_it_cannot_run() {
    let (_pg, pool) = empty("ordo_backfill_render").await;
    store(&pool, &v1()).await;
    store(&pool, &v2()).await;
    let render = |c: Choices| {
        let pool = pool.clone();
        async move { evolve::render(&pool, "1.0.0", "1.1.0", &c, 1).await }
    };
    // No fill: the field cannot be required, and the fix is `--fill`.
    let err = render(Choices::default()).await.expect_err("no fill");
    assert!(err.to_string().contains("--fill note.heading="), "{err}");
    // A field the mapping names that is not there.
    let err = render(fills("note.heading=titel", ""))
        .await
        .expect_err("unknown field");
    assert!(err.to_string().contains("mapping_unknown_field"), "{err}");
    // A value of the wrong type for the column: text into an int.
    {
        let (_pg, pool) = empty("ordo_backfill_render_type").await;
        store(&pool, &v1()).await;
        store(&pool, &v2().replace("heading: text", "heading: int")).await;
        let err = evolve::render(&pool, "1.0.0", "1.1.0", &fills("note.heading=title", ""), 1)
            .await
            .expect_err("wrong type");
        assert!(err.to_string().contains("mapping_invalid"), "{err}");
        assert!(err.to_string().contains("bigint"), "names the type: {err}");
        evolve::render(&pool, "1.0.0", "1.1.0", &fills("note.heading=1 + 2", ""), 1)
            .await
            .expect("an int is an int");
    }
    // A fill for a field the step does not make required.
    let err = render(fills("note.heading=title;note.title=body", ""))
        .await
        .expect_err("stray fill");
    assert!(matches!(err, EvolveRefusal::Options(_)), "{err:?}");
    // A function v2 does not declare.
    let err = render(fills("note.heading=fn headline", ""))
        .await
        .expect_err("no function");
    assert!(
        err.to_string().contains("backfill_function_invalid"),
        "{err}"
    );

    // A fill renders into the receipt, and so into the digest.
    let ok = render(fills("note.heading=title", ""))
        .await
        .expect("renders");
    let b = &ok.receipt["backfills"][0];
    assert_eq!(b["mapping"], "title");
    assert_eq!(b["sql"], "\"title\"");
    assert_eq!(b["audit"], "summarised");
    assert_eq!(b["reproducible"], true);
    assert!(b["oldValues"].as_str().unwrap().contains("backup"));
    assert_eq!(
        ok.receipt["after"][0],
        "ALTER TABLE \"note\" ALTER COLUMN \"heading\" SET NOT NULL;"
    );
    let other = render(fills("note.heading=body", ""))
        .await
        .expect("renders");
    assert_ne!(ok.digest, other.digest, "the mapping is in the digest");
    // The options round-trip through `$plans`.
    let stored = evolve::options_text(&fills("note.heading=title", "note.heading"));
    assert_eq!(
        evolve::parse_options(&stored).unwrap(),
        fills("note.heading=title", "note.heading")
    );
}

/// The function runner a test gives: it answers each row's title upper
/// cased, and keeps what it was handed.
#[derive(Default)]
struct Upper {
    calls: Mutex<Vec<FunctionChunk>>,
}

#[async_trait::async_trait]
impl BackfillFunctions for Upper {
    async fn call(&self, chunk: FunctionChunk) -> Result<Value, String> {
        let values: Vec<Value> = chunk.input["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| json!(r["title"].as_str().unwrap_or_default().to_uppercase()))
            .collect();
        self.calls.lock().unwrap().push(chunk);
        Ok(json!({ "values": values }))
    }
}

/// **Backfill functions (B6, Q14)**: called once per chunk per space, with
/// exactly the declared secrets and hosts; a host the function does not
/// declare is a plan error; a function that calls out is not reproducible.
#[tokio::test(flavor = "multi_thread")]
async fn a_backfill_function_runs_per_chunk_with_only_what_the_plan_declares() {
    let (_pg, pool) = empty("ordo_backfill_fn_render").await;
    store(&pool, &v1()).await;
    store(&pool, &v2_fn()).await;
    let err = evolve::render(
        &pool,
        "1.0.0",
        "1.1.0",
        &fills("note.heading=fn headline hosts evil.example.com", ""),
        1,
    )
    .await
    .expect_err("a host the function does not declare");
    assert!(err.to_string().contains("evil.example.com"), "{err}");
    let err = evolve::render(
        &pool,
        "1.0.0",
        "1.1.0",
        &fills("note.heading=fn headline secrets other", ""),
        1,
    )
    .await
    .expect_err("a secret the function does not declare");
    assert!(err.to_string().contains("other"), "{err}");

    let choices = fills(
        "note.heading=fn headline secrets apiKey hosts api.example.com",
        "",
    );
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_backfill_fn", &v2_fn(), &choices).await;
    let receipt: Value = serde_json::from_str(&plan.receipt).unwrap();
    let b = &receipt["backfills"][0];
    assert_eq!(b["function"]["name"], "headline");
    assert_eq!(b["function"]["codeHash"], "sha256:headline");
    assert_eq!(b["function"]["hosts"], json!(["api.example.com"]));
    assert_eq!(
        b["reproducible"], false,
        "it calls out, so staging may differ"
    );
    assert!(b["sql"].is_null());

    // Without a runner the step refuses, naming why, and changes no row.
    let err = evolve::run_step_with(&registry, &tenant, "db", &plan, OWNER, &settings(100, None))
        .await
        .expect_err("no runner");
    assert!(
        err.to_string().contains("runs no backend functions"),
        "{err}"
    );

    let upper = Arc::new(Upper::default());
    let ctx = StepContext {
        settings: BackfillSettings {
            chunk_rows: 100,
            ..BackfillSettings::default()
        },
        functions: Some(upper.clone()),
        keys: None,
        fence: None,
    };
    let ran = evolve::run_step_with(&registry, &tenant, "db", &plan, OWNER, &ctx)
        .await
        .expect("runs");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(
        one(
            &tpg,
            "SELECT heading FROM note WHERE \"$space\" = 'beta' AND id = 'n007'"
        ),
        "BETA 7"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM note WHERE heading IS DISTINCT FROM upper(title)"
        ),
        "0"
    );
    let calls = upper.calls.lock().unwrap();
    // Chunks: acme 100 | acme 50 + beta 50 | beta 50: one call per space.
    assert_eq!(calls.len(), 4, "once per chunk per space");
    for c in calls.iter() {
        assert_eq!(c.secrets, vec!["apiKey".to_string()]);
        assert_eq!(c.hosts, vec!["api.example.com".to_string()]);
        assert_eq!((c.version.as_str(), c.name.as_str()), ("1.1.0", "headline"));
    }
    let rec = backfill_groups(&tpg, "acme")[0].backfill.clone().unwrap();
    assert_eq!(
        rec.mapping,
        "fn headline secrets apiKey hosts api.example.com"
    );
}

/// **The worker alone**: a backfill already `done` does nothing on a run
/// again, and a second worker on the same database is told it is busy.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_worker_is_busy_and_a_done_backfill_does_nothing() {
    let choices = fills("note.heading=title", "");
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_backfill_worker", &v2(), &choices).await;
    // The expand alone, by running one chunk.
    evolve::run_step_with(
        &registry,
        &tenant,
        "db",
        &plan,
        OWNER,
        &settings(10, Some(1)),
    )
    .await
    .expect("one chunk");
    let program = Program::compile(&v2()).unwrap();
    let triaged = {
        let live = ordo_exec::migrate::live_shape(&tenant, tenant.schema())
            .await
            .unwrap();
        let changes = ordo_schema::diff_with(&live, None, None, &program.schema);
        let assessed = sa::assess_with(&tenant, &program.schema, &live, changes, &choices).await;
        sa::triage_counted(
            &program.schema,
            &live,
            &assessed,
            &choices,
            "1.1.0",
            &Default::default(),
        )
        .unwrap()
    };
    let step = triaged.backfills[0].clone();
    let s = BackfillSettings::default();
    let w = Backfill {
        pool: &tenant,
        step: &plan.digest,
        version: "1.1.0",
        fill: &step,
        entity: program.schema.native("note").unwrap(),
        settings: &s,
        functions: None,
        keys: None,
    };
    // Another worker holds the lock.
    let holder = Pool::connect(cfg(&tpg.db)).await.expect("a second pool");
    let held = holder.begin().await.unwrap();
    held.rows(&format!(
        "SELECT pg_advisory_xact_lock({}, hashtext('note/heading'))",
        ordo_host::backfill::BACKFILL_LOCK_NS
    ))
    .await
    .unwrap();
    assert_eq!(w.run().await.unwrap_err(), BackfillRefusal::Busy);
    held.rollback().await.unwrap();
    let p = w.run().await.expect("runs");
    assert!(p.ended);
    assert_eq!(p.rows_done, 250);
    w.verify_and_close().await.expect("verifies");
    let again = w.run().await.expect("runs again");
    assert_eq!((again.state.as_str(), again.rows_this_run), ("done", 0));
    assert!(w.verify_and_close().await.unwrap().already);
}

/// Increment 11a: **a move with dual writes is refused where it cannot keep
/// both shapes current**, each refusal naming the fix. A tenant whose rows
/// are in the deployment's own database shares it with tenants that would
/// not write both shapes (11b). A step filled by a backend function has no
/// per-row mapping for the write path (D16's one-way step).
#[tokio::test(flavor = "multi_thread")]
async fn a_move_with_dual_writes_is_refused_where_it_cannot_keep_both_shapes() {
    let choices = fills(
        "note.heading=fn headline secrets apiKey hosts api.example.com",
        "",
    );
    let (_rpg, registry, _tpg, tenant, plan) =
        setup("ordo_backfill_move", &v2_fn(), &choices).await;
    let ctx = settings(50, None);

    // The deployment's own database.
    let shared = ordo_exec::route::SinglePool::new(registry.clone());
    let cx = ordo_host::dual_writes::MoveContext {
        control: &registry,
        pools: &shared,
        spaces: None,
    };
    let err = ordo_host::dual_writes::run(&cx, "acme", &plan, &ctx, 0, OWNER)
        .await
        .expect_err("a shared database");
    let text = err.to_string();
    assert!(text.starts_with("shared_database"), "{text}");
    assert!(text.contains("--run"), "{text}");

    // A database of its own, and a function fill: one way.
    let own = ordo_exec::route::SinglePool::new(tenant.clone());
    let cx = ordo_host::dual_writes::MoveContext {
        control: &registry,
        pools: &own,
        spaces: None,
    };
    let err = ordo_host::dual_writes::run(&cx, "acme", &plan, &ctx, 0, OWNER)
        .await
        .expect_err("a function fill");
    let text = err.to_string();
    assert!(text.starts_with("one_way_step"), "{text}");
    assert!(text.contains("backend function"), "{text}");
    // Nothing was opened.
    assert!(Versions::on(&registry)
        .dual_move("acme")
        .await
        .expect("moves")
        .is_none());
}
