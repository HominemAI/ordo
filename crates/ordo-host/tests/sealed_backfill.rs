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
    setup_with(stem, &v1(), next, &Choices::default()).await
}

/// [`setup`], from `first`, with the step stored under `choices`.
async fn setup_with(
    stem: &str,
    first: &str,
    next: &str,
    choices: &Choices,
) -> (Pg, Pool, Pg, Pool, evolve::StoredPlan) {
    let (reg_pg, registry) = empty(&format!("{stem}_reg")).await;
    let (tenant_pg, tenant) = empty(&format!("{stem}_tenant")).await;
    let first = store(&registry, first).await;
    store(&registry, next).await;
    let done = evolve::evolve(&registry, "1.0.0", "1.1.0", choices, None, OWNER, 1)
        .await
        .expect("the step stores");
    let (_, created) = sa::apply(&tenant, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    // 120 plaintext notes in each of 3 spaces, while `body` is plaintext.
    for s in SPACES
        .iter()
        .filter(|_| column_type(&tenant_pg, "body") == "text")
    {
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
        fence: None,
        fences: Vec::new(),
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
    assert!(text.contains("seal_then_index"), "{text}");
}

/// **A consumer reading through the door does not stop a seal** (step G7
/// of `plan-federation-grants.md`): the door opens the field in its own
/// answers, so the seal's plan stores its step.
#[tokio::test(flavor = "multi_thread")]
async fn a_seal_of_a_field_a_door_consumer_reads_is_not_refused() {
    let with_grant = |src: String| src + "\ngrant note_viewer {\n  std.note.list\n}\n";
    let (_rpg, registry) = empty("ordo_seal_grant_reg").await;
    store(&registry, &with_grant(v1())).await;
    store(&registry, &with_grant(v2())).await;
    evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect("a grant does not stop a seal");
    assert!(
        !evolve::path(&registry, "1.0.0", "1.1.0")
            .await
            .expect("a path")
            .is_empty(),
        "the step is stored"
    );
}

// ── live writes during the seal (increment 13, part 2) ──────────────────

/// Write ops beside [`v1`]: the version a node serves during the seal.
fn ops() -> &'static str {
    "\nop addNote($title: text, $body: text) =\n  set note { title: $title, body: $body }\n\n\
     op editNote($id: id, $rev: int, $title: text, $body: text) =\n  \
     set note { id: $id, rev: $rev, title: $title, body: $body }\n"
}

fn v1_writes() -> String {
    v1() + ops()
}

fn v2_writes() -> String {
    v2() + ops()
}

/// One node serving `source` over the tenant's database, through the real
/// write path, with `keys` as its custody. It holds nothing about the seal:
/// the database says.
struct Node {
    exec: ordo_exec::Executor,
    fixture: Arc<ordo_conformance::Fixture>,
    log: Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
}

async fn node(db: &str, source: &str, name: &str, keys: Arc<dyn KeyProvider>) -> Node {
    let mut f = ordo_conformance::Fixture::from_source(source, "").expect("compiles");
    // One custody for the write path, the overlay and the worker, as a
    // deployment has.
    f.keys = keys.clone();
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    Node {
        exec: ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys,
            space: ordo_conformance::fixture_space(),
        },
        fixture: Arc::new(f),
        log: Arc::new(std::sync::Mutex::new(ordo_conformance::WriteLog::serving(
            format!("{name}_"),
            Arc::new(|| 1),
        ))),
    }
}

impl Node {
    async fn write(&self, call: String) -> Result<(), String> {
        self.exec
            .write_as(self.fixture.clone(), self.log.clone(), call, "p_ada".into())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// **Writes made while the seal runs land sealed, and none is lost**
/// (increment 13 under live writes, on increment 11a's fence). Two nodes
/// serve 1.0.0, where `body` is plaintext, and are told nothing. The step
/// to 1.1.0 records its seal in the tenant database's `$seals` under the
/// `$renames` lock, then seals 2 rows a chunk. Meanwhile node A adds 100
/// notes and node B edits 20 notes written before the seal and adds 100
/// more, behind the cursor and ahead of it. Every write seals what it wrote
/// in its own transaction. The step then runs to its end: verify opens
/// every value, and the plaintext column is dropped under the lock.
///
/// A regression looks like a write refused during the seal, a row whose
/// sealed value is not what was last written (a write that missed the
/// seal), or a plaintext column or `$seals` row left behind.
#[tokio::test(flavor = "multi_thread")]
async fn writes_during_the_seal_land_sealed_and_none_is_lost() {
    let (_rpg, registry, tpg, tenant, plan) = setup_with(
        "ordo_seal_live",
        &v1_writes(),
        &v2_writes(),
        &Choices::default(),
    )
    .await;
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let a = Arc::new(node(&tpg.db, &v1_writes(), "a", keys.clone()).await);
    let b = Arc::new(node(&tpg.db, &v1_writes(), "b", keys.clone()).await);
    for i in 0..20 {
        a.write(format!(r#"addNote($title: "pre-{i}", $body: "pre-{i}")"#))
            .await
            .expect("a write before the seal");
    }
    let fixture = ordo_conformance::fixture_space().0;
    let pre: Vec<String> = tpg
        .query("SELECT id FROM note WHERE title LIKE 'pre-%' ORDER BY id")
        .expect("ids")
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    assert_eq!(pre.len(), 20);

    let step = |budget: Option<u32>| {
        let (registry, tenant, plan, keys) =
            (registry.clone(), tenant.clone(), plan.clone(), keys.clone());
        async move {
            evolve::run_step_with(
                &registry,
                &tenant,
                "tenant-db",
                &plan,
                OWNER,
                &ctx(Some(keys), 2, budget),
            )
            .await
        }
    };
    // The expand and one chunk: the seal is recorded where it runs.
    let first = step(Some(1)).await.expect("the seal starts");
    assert!(matches!(first, StepRun::Backfilling { .. }), "{first:?}");
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM \"$seals\""),
        "1",
        "the seal is recorded in the database it runs on"
    );

    let wa = {
        let a = a.clone();
        tokio::spawn(async move {
            let mut bad = Vec::new();
            for i in 0..100 {
                if let Err(e) = a
                    .write(format!(r#"addNote($title: "a-{i}", $body: "a-{i}")"#))
                    .await
                {
                    bad.push(format!("a-{i}: {e}"));
                }
            }
            bad
        })
    };
    let wb = {
        let (b, pre) = (b.clone(), pre.clone());
        tokio::spawn(async move {
            let mut bad = Vec::new();
            for (i, id) in pre.iter().enumerate() {
                if let Err(e) = b
                    .write(format!(
                        r#"editNote($id: "{id}", $rev: 1, $title: "pre-{i}", $body: "edited-{id}")"#
                    ))
                    .await
                {
                    bad.push(format!("edit {id}: {e}"));
                }
            }
            for i in 0..100 {
                if let Err(e) = b
                    .write(format!(r#"addNote($title: "b-{i}", $body: "b-{i}")"#))
                    .await
                {
                    bad.push(format!("b-{i}: {e}"));
                }
            }
            bad
        })
    };
    // The worker keeps sealing, a chunk a run, while both nodes write.
    let mut chunks = 0;
    while !wa.is_finished() || !wb.is_finished() {
        let r = step(Some(1)).await.expect("a chunk");
        assert!(
            matches!(r, StepRun::Backfilling { .. }),
            "the seal must still be running while the nodes write: {r:?}"
        );
        chunks += 1;
    }
    assert!(chunks > 0, "the worker ran beside the writes");
    let (bad_a, bad_b) = (wa.await.unwrap(), wb.await.unwrap());
    assert!(bad_a.is_empty(), "every write of node A landed: {bad_a:?}");
    assert!(bad_b.is_empty(), "every write of node B landed: {bad_b:?}");
    let written = one(
        &tpg,
        &format!(
            "SELECT count(*) FROM note WHERE \"$space\" = '{fixture}' AND \"body$seal\" IS NOT NULL \
             AND (title LIKE 'a-%' OR title LIKE 'b-%' OR body LIKE 'edited-%')"
        ),
    );
    assert_eq!(
        written, "220",
        "each write sealed what it wrote, in its own transaction"
    );

    let ran = step(None).await.expect("the seal ends");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(recorded(&tpg), "1.1.0");
    // Verify passed the first time: the live writes did not start it over.
    let this_run = ran.backfills()[0]["progress"]["rowsThisRun"]
        .as_i64()
        .unwrap_or(-1);
    assert!(
        (0..580 - 2).contains(&this_run),
        "the last run sealed only what was left, {this_run} row(s): {:?}",
        ran.backfills()
    );
    assert_eq!(column_type(&tpg, "body"), "bytea");
    assert_eq!(
        column_type(&tpg, "body$seal"),
        "",
        "the working column is gone"
    );
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM \"$seals\""),
        "0",
        "the seal left `$seals`"
    );
    for marker in ["pre-", "edited-", "secret"] {
        assert_eq!(
            one(
                &tpg,
                &format!("SELECT count(*) FROM note WHERE position('{marker}'::bytea in body) > 0")
            ),
            "0",
            "no row holds `{marker}` in plaintext"
        );
    }
    // Every row of the fixture's space opens to what was written last.
    let rows = tpg
        .query(&format!(
            "SELECT id, title FROM note WHERE \"$space\" = '{fixture}' ORDER BY id"
        ))
        .expect("rows");
    assert_eq!(rows.len(), 220);
    for r in rows {
        let id = r[0].clone().unwrap_or_default();
        let title = r[1].clone().unwrap_or_default();
        let want = if title.starts_with("pre-") {
            format!("edited-{id}")
        } else {
            title
        };
        assert_eq!(open(&keys, &tpg, &fixture, &id), want, "row {id}");
    }
}

/// **`seal.keep_plaintext` keeps the plaintext column, orphaned** (D9's
/// opt-in): the receipt names the risk, the sealed column takes the
/// field's name, the plaintext is renamed `body$plaintext` and recorded in
/// `$orphans` for erasure, export and pruning.
///
/// A regression looks like the plaintext dropped though kept, a kept
/// column that no orphan row names, or a receipt that does not say so.
#[tokio::test(flavor = "multi_thread")]
async fn keep_plaintext_keeps_the_column_as_an_orphan_and_says_why_not_to() {
    let mut choices = Choices::default();
    choices.parse_keep_plaintext("note.body").expect("parses");
    let (_rpg, registry, tpg, tenant, plan) =
        setup_with("ordo_seal_keep", &v1(), &v2(), &choices).await;
    assert!(
        plan.options.contains("keep-plaintext note.body"),
        "the option is in the stored plan: {}",
        plan.options
    );
    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("json");
    let said = receipt["backfills"][0]["plaintext"]
        .as_str()
        .unwrap_or_default();
    assert!(said.contains("body$plaintext"), "{said}");
    assert!(said.contains("WAL and backups"), "names the risk: {said}");
    assert!(said.contains("--prune"), "names what removes it: {said}");

    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
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
    assert_eq!(column_type(&tpg, "body"), "bytea");
    assert_eq!(
        column_type(&tpg, "body$plaintext"),
        "text",
        "the plaintext is kept"
    );
    assert_eq!(open(&keys, &tpg, "acme", "n007"), "secret acme 7");
    assert_eq!(
        one(
            &tpg,
            "SELECT columns || ' ' || sealed::text FROM \"$orphans\" \
             WHERE entity = 'note' AND field = 'body$plaintext'"
        ),
        "body$plaintext false",
        "the kept column is an orphan"
    );

    // Without the option, the same step drops it (the default).
    let other = evolve::render_stored(
        &registry,
        &Versions::on(&registry).get("1.0.0").await.unwrap().unwrap(),
        &Versions::on(&registry).get("1.1.0").await.unwrap().unwrap(),
        &Choices::default(),
    )
    .await
    .expect("renders");
    assert_ne!(
        other.digest, plan.digest,
        "keeping the plaintext is in the digest"
    );
}

// ── B8: a sealed field gains its blind index (increment 13, part 2) ──────

fn sealed_v1() -> String {
    "domain notes @ 1.0.0\n\nentity note {\n  title: text\n  body: text sealed\n}\n\n\
     op allNotes unbounded =\n  note[title]\n"
        .to_string()
        + ops()
}

fn indexed_v2(unique: bool) -> String {
    let u = if unique { " unique" } else { "" };
    format!(
        "domain notes @ 1.1.0\n\nentity note {{\n  title: text\n  body: text sealed indexed{u}\n}}\n\n\
         op allNotes unbounded =\n  note[title]\n"
    ) + ops()
}

/// **A sealed field gains its blind index under live writes** (B8). The
/// tenant serves 1.0.0, where `body` is sealed and not indexed. The step to
/// 1.1.0 records an `index` in `$seals`, adds `body$bidx` open to nulls, and
/// the worker opens each row's sealed value and writes its index, 2 rows a
/// chunk, while a node on 1.0.0 adds 40 notes and edits 10. Each write
/// indexes what it sealed. Verify computes every index again; then
/// `body$bidx` is required, and a node on 1.1.0 finds a note by `body ==`.
///
/// A regression looks like a write refused during the backfill, an index
/// that is not its value's (a write that missed it, found by verify or by
/// `==` finding nothing), or a `$seals` row left behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_field_gains_its_blind_index_under_live_writes() {
    let (_rpg, registry, tpg, tenant, plan) = setup_with(
        "ordo_seal_bidx",
        &sealed_v1(),
        &indexed_v2(false),
        &Choices::default(),
    )
    .await;
    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("json");
    let bf = &receipt["backfills"][0];
    assert_eq!(bf["code"], "B8", "{receipt}");
    assert_eq!(bf["mapping"], "blind index");
    assert_eq!(bf["indexFrom"], "body");
    assert_eq!(bf["column"], "body$bidx");

    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let a = Arc::new(node(&tpg.db, &sealed_v1(), "a", keys.clone()).await);
    for i in 0..30 {
        a.write(format!(r#"addNote($title: "pre-{i}", $body: "Pre-{i}")"#))
            .await
            .expect("a write before");
    }
    let pre: Vec<String> = tpg
        .query("SELECT id FROM note ORDER BY id LIMIT 10")
        .expect("ids")
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    let step = |budget: Option<u32>| {
        let (registry, tenant, plan, keys) =
            (registry.clone(), tenant.clone(), plan.clone(), keys.clone());
        async move {
            evolve::run_step_with(
                &registry,
                &tenant,
                "tenant-db",
                &plan,
                OWNER,
                &ctx(Some(keys), 2, budget),
            )
            .await
        }
    };
    let first = step(Some(1)).await.expect("the index starts");
    assert!(matches!(first, StepRun::Backfilling { .. }), "{first:?}");
    assert_eq!(
        one(&tpg, "SELECT kind FROM \"$seals\""),
        "index",
        "the index is recorded where it runs"
    );
    let w = {
        let (a, pre) = (a.clone(), pre.clone());
        tokio::spawn(async move {
            let mut bad = Vec::new();
            for (i, id) in pre.iter().enumerate() {
                if let Err(e) = a
                    .write(format!(
                        r#"editNote($id: "{id}", $rev: 1, $title: "edit-{i}", $body: "Edited-{i}")"#
                    ))
                    .await
                {
                    bad.push(format!("edit {id}: {e}"));
                }
            }
            for i in 0..40 {
                if let Err(e) = a
                    .write(format!(r#"addNote($title: "live-{i}", $body: "Live-{i}")"#))
                    .await
                {
                    bad.push(format!("live-{i}: {e}"));
                }
            }
            bad
        })
    };
    while !w.is_finished() {
        let r = step(Some(1)).await.expect("a chunk");
        assert!(matches!(r, StepRun::Backfilling { .. }), "{r:?}");
    }
    let bad = w.await.unwrap();
    assert!(bad.is_empty(), "every write landed: {bad:?}");
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM note WHERE \"body$bidx\" IS NOT NULL \
             AND (title LIKE 'live-%' OR title LIKE 'edit-%')"
        ),
        "50",
        "each write indexed what it sealed, in its own transaction"
    );

    let ran = step(None).await.expect("the index ends");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(recorded(&tpg), "1.1.0");
    assert_eq!(one(&tpg, "SELECT count(*) FROM \"$seals\""), "0");
    assert_eq!(
        one(
            &tpg,
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = 'body$bidx'"
        ),
        "NO",
        "the index is required once it verifies"
    );
    // 1.1.0 finds notes by their sealed value: one written before, one
    // edited during, one added during the backfill.
    let y = node(&tpg.db, &indexed_v2(false), "y", keys.clone()).await;
    for (body, title) in [
        ("Pre-29", "pre-29"),
        ("Edited-3", "edit-3"),
        ("Live-17", "live-17"),
    ] {
        let rows = y
            .exec
            .read_as(&format!("note(body == \"{body}\")[title]"), Some("p_ada"))
            .await
            .expect("reads");
        assert_eq!(rows.len(), 1, "`{body}` finds its note: {rows:?}");
        assert_eq!(rows[0]["title"], title);
    }
}

/// B8 through `ordo apply` is refused by name, naming what to do.
#[tokio::test(flavor = "multi_thread")]
async fn a_blind_index_outside_a_stored_plan_is_refused_by_name() {
    let (_tpg, tenant) = empty("ordo_seal_bidx_refused_tenant").await;
    let x = Program::compile(&sealed_v1()).expect("1.0.0");
    let y = Program::compile(&indexed_v2(false)).expect("1.1.0");
    let (_, created) = sa::apply(&tenant, &x, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    let (plan, applied) = sa::apply(&tenant, &y, "test", "test").await;
    assert!(applied.is_err(), "{applied:?}");
    let text = format!("{plan:?}");
    assert!(text.contains("index_needs_stored_plan"), "{text}");
    assert!(text.contains("ordo evolve"), "names the command: {text}");
}

// ── part 3: `sealed unique` by backfill, B10, the own space ─────────────

/// The ids of the notes titled `titles`, in that order.
fn ids_of(pg: &Pg, titles: &[&str]) -> Vec<String> {
    titles
        .iter()
        .map(|t| one(pg, &format!("SELECT id FROM note WHERE title = '{t}'")))
        .collect()
}

/// A run of `plan` on `tenant` to its end, or its refusal.
#[allow(clippy::result_large_err)]
async fn run_to_end(
    registry: &Pool,
    tenant: &Pool,
    plan: &evolve::StoredPlan,
    keys: &Arc<dyn KeyProvider>,
) -> Result<StepRun, EvolveRefusal> {
    evolve::run_step_with(
        registry,
        tenant,
        "tenant-db",
        plan,
        OWNER,
        &ctx(Some(keys.clone()), 100, None),
    )
    .await
}

/// **`sealed indexed unique` by backfill** (B8 with C1 over the blind
/// indexes). Two notes of one space hold one body. The step computes every
/// index, verifies, then reads the duplicates over the indexes and refuses
/// the end, naming both rows; nothing is dropped and `$seals` keeps the
/// step, so writes keep the index current. One note is edited by the
/// version before; the step runs again, ends, and the constraint stands: a
/// third note with that body is refused.
///
/// A regression looks like a refusal naming no rows, a constraint over
/// duplicates, or a unique the version after does not enforce.
#[tokio::test(flavor = "multi_thread")]
async fn sealed_unique_by_backfill_refuses_duplicates_by_row_then_lands() {
    let (_rpg, registry, tpg, tenant, plan) = setup_with(
        "ordo_seal_bidx_uq",
        &sealed_v1(),
        &indexed_v2(true),
        &Choices::default(),
    )
    .await;
    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("json");
    let bf = &receipt["backfills"][0];
    assert_eq!(bf["code"], "B8", "{receipt}");
    assert!(
        bf["unique"].as_str().unwrap_or_default().contains("refuse"),
        "the receipt says what a duplicate does: {receipt}"
    );
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let a = node(&tpg.db, &sealed_v1(), "a", keys.clone()).await;
    for (title, body) in [("one", "Same"), ("two", "Same"), ("three", "Other")] {
        a.write(format!(r#"addNote($title: "{title}", $body: "{body}")"#))
            .await
            .expect("a write before");
    }
    let ids = ids_of(&tpg, &["one", "two"]);

    let err = run_to_end(&registry, &tenant, &plan, &keys)
        .await
        .expect_err("duplicates refuse the end");
    let text = err.to_string();
    assert!(text.contains("sealed_unique_duplicates"), "{text}");
    for id in &ids {
        assert!(text.contains(id.as_str()), "names row {id}: {text}");
    }
    assert!(text.contains("run the step again"), "{text}");
    assert_eq!(recorded(&tpg), "1.0.0", "the version before is recorded");
    assert_eq!(one(&tpg, "SELECT kind FROM \"$seals\""), "index");

    a.write(format!(
        r#"editNote($id: "{}", $rev: 1, $title: "two", $body: "Changed")"#,
        ids[1]
    ))
    .await
    .expect("the version before edits the duplicate");
    let ran = run_to_end(&registry, &tenant, &plan, &keys)
        .await
        .expect("the step ends");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(recorded(&tpg), "1.1.0");
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM pg_constraint WHERE conname = 'note$body$uq'"
        ),
        "1",
        "the unique lands at the end"
    );
    // `set` over a unique field is keyed by it: a second `Same` is the
    // same note, never a second row.
    let y = node(&tpg.db, &indexed_v2(true), "y", keys.clone()).await;
    y.write(r#"addNote($title: "four", $body: "Same")"#.into())
        .await
        .expect("a write keyed by the unique");
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM note"),
        "3",
        "no second `Same`"
    );
    assert_eq!(
        one(
            &tpg,
            &format!("SELECT title FROM note WHERE id = '{}'", ids[0])
        ),
        "four",
        "the note holding `Same` is the one written"
    );
}

fn sealed_indexed_v1(unique: bool) -> String {
    let u = if unique { " unique" } else { "" };
    format!(
        "domain notes @ 1.0.0\n\nentity note {{\n  title: text\n  body: text sealed indexed{u}\n}}\n\n\
         op allNotes unbounded =\n  note[title]\n"
    ) + ops()
}

fn normalized_v2(unique: bool) -> String {
    let u = if unique { " unique" } else { "" };
    format!(
        "domain notes @ 1.1.0\n\nentity note {{\n  title: text\n  \
         body: text sealed indexed{u} normalize(lower)\n}}\n\n\
         op allNotes unbounded =\n  note[title]\n"
    ) + ops()
}

/// **B10: a new `normalize` computes every blind index again, under live
/// writes.** The tenant serves 1.0.0, where `body` is sealed and indexed
/// with no normalization. The step to 1.1.0 adds `normalize(lower)`: it
/// records a `reindex` in `$seals` with the normalization, adds
/// `body$bidx$next`, and the worker computes each row's index under it, 2
/// rows a chunk, while a node on 1.0.0 edits 10 notes and adds 40. Each
/// write fills the new index under the recorded normalization beside its
/// own. Verify computes every index again; the end replaces the old index,
/// and 1.1.0 finds notes by `==` in any case.
///
/// A regression looks like a write refused during the backfill, a note
/// `==` misses (a write that missed the new index, or one computed under
/// the old normalization), or a working column or `$seals` row left behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_normalize_computes_every_index_again_under_live_writes() {
    let (_rpg, registry, tpg, tenant, plan) = setup_with(
        "ordo_seal_renorm",
        &sealed_indexed_v1(false),
        &normalized_v2(false),
        &Choices::default(),
    )
    .await;
    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("json");
    let bf = &receipt["backfills"][0];
    assert_eq!(bf["code"], "B10", "{receipt}");
    assert_eq!(bf["column"], "body$bidx$next");
    assert_eq!(bf["indexFrom"], "body");
    let said = bf["index"].as_str().unwrap_or_default();
    assert!(said.contains("normalize"), "{said}");
    assert!(
        said.contains("version before"),
        "names the rollback: {said}"
    );

    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let a = Arc::new(node(&tpg.db, &sealed_indexed_v1(false), "a", keys.clone()).await);
    for i in 0..30 {
        a.write(format!(r#"addNote($title: "pre-{i}", $body: "Pre-{i}")"#))
            .await
            .expect("a write before");
    }
    let pre: Vec<String> = tpg
        .query("SELECT id FROM note ORDER BY id LIMIT 10")
        .expect("ids")
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    let step = |budget: Option<u32>| {
        let (registry, tenant, plan, keys) =
            (registry.clone(), tenant.clone(), plan.clone(), keys.clone());
        async move {
            evolve::run_step_with(
                &registry,
                &tenant,
                "tenant-db",
                &plan,
                OWNER,
                &ctx(Some(keys), 2, budget),
            )
            .await
        }
    };
    let first = step(Some(1)).await.expect("the reindex starts");
    assert!(matches!(first, StepRun::Backfilling { .. }), "{first:?}");
    assert_eq!(
        one(&tpg, "SELECT kind || ' ' || normalize FROM \"$seals\""),
        "reindex lower",
        "the reindex is recorded where it runs, with its normalization"
    );
    let w = {
        let (a, pre) = (a.clone(), pre.clone());
        tokio::spawn(async move {
            let mut bad = Vec::new();
            for (i, id) in pre.iter().enumerate() {
                if let Err(e) = a
                    .write(format!(
                        r#"editNote($id: "{id}", $rev: 1, $title: "edit-{i}", $body: "Edited-{i}")"#
                    ))
                    .await
                {
                    bad.push(format!("edit {id}: {e}"));
                }
            }
            for i in 0..40 {
                if let Err(e) = a
                    .write(format!(r#"addNote($title: "live-{i}", $body: "Live-{i}")"#))
                    .await
                {
                    bad.push(format!("live-{i}: {e}"));
                }
            }
            bad
        })
    };
    while !w.is_finished() {
        let r = step(Some(1)).await.expect("a chunk");
        assert!(matches!(r, StepRun::Backfilling { .. }), "{r:?}");
    }
    let bad = w.await.unwrap();
    assert!(bad.is_empty(), "every write landed: {bad:?}");
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM note WHERE \"body$bidx$next\" IS NOT NULL \
             AND (title LIKE 'live-%' OR title LIKE 'edit-%')"
        ),
        "50",
        "each write filled the new index, in its own transaction"
    );

    let ran = step(None).await.expect("the reindex ends");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(recorded(&tpg), "1.1.0");
    assert_eq!(
        one(
            &tpg,
            "SELECT string_agg(kind || ' ' || target || ' ' || normalize, ',') FROM \"$seals\""
        ),
        "kept body$bidx$prev ",
        "only the old index is kept, under the old normalization"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT columns FROM \"$orphans\" WHERE entity = 'note' AND field = 'body$bidx$prev'"
        ),
        "body$bidx$prev",
        "the kept index is an orphan, pruned at the close"
    );
    assert_eq!(
        column_type(&tpg, "body$bidx$next"),
        "",
        "the working column is gone"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = 'body$bidx'"
        ),
        "NO"
    );
    let y = node(&tpg.db, &normalized_v2(false), "y", keys.clone()).await;
    for (body, title) in [
        ("pre-29", "pre-29"),
        ("EDITED-3", "edit-3"),
        ("live-17", "live-17"),
    ] {
        let rows = y
            .exec
            .read_as(&format!("note(body == \"{body}\")[title]"), Some("p_ada"))
            .await
            .expect("reads");
        assert_eq!(
            rows.len(),
            1,
            "`{body}` finds its note in any case: {rows:?}"
        );
        assert_eq!(rows[0]["title"], title);
    }
}

/// **B10 under `unique`: values that collide under the new normalization
/// refuse the end, naming the rows** (C4, found over the indexes since the
/// values are sealed). `Ada` and `ada` are distinct under 1.0.0. The step
/// to `normalize(lower)` computes their indexes, finds them equal and
/// refuses; the old index and its constraint stay. One is edited; the step
/// ends, the constraint stands over the new index, and `ADA` is refused.
///
/// A regression looks like a probe that runs `lower()` over ciphertext, a
/// constraint lost with the old index, or two rows the new `==` cannot
/// tell apart.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_normalize_under_unique_refuses_collisions_naming_the_rows() {
    let (_rpg, registry, tpg, tenant, plan) = setup_with(
        "ordo_seal_renorm_uq",
        &sealed_indexed_v1(true),
        &normalized_v2(true),
        &Choices::default(),
    )
    .await;
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let a = node(&tpg.db, &sealed_indexed_v1(true), "a", keys.clone()).await;
    for (title, body) in [("first", "Ada"), ("second", "ada"), ("third", "Bob")] {
        a.write(format!(r#"addNote($title: "{title}", $body: "{body}")"#))
            .await
            .expect("distinct under 1.0.0");
    }
    let ids = ids_of(&tpg, &["first", "second"]);
    let err = run_to_end(&registry, &tenant, &plan, &keys)
        .await
        .expect_err("a collision refuses the end");
    let text = err.to_string();
    assert!(text.contains("sealed_unique_duplicates"), "{text}");
    assert!(text.contains("new `normalize`"), "{text}");
    for id in &ids {
        assert!(text.contains(id.as_str()), "names row {id}: {text}");
    }
    assert_eq!(recorded(&tpg), "1.0.0");
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM pg_constraint WHERE conname = 'note$body$uq'"
        ),
        "1",
        "the old constraint stands"
    );

    a.write(format!(
        r#"editNote($id: "{}", $rev: 1, $title: "second", $body: "Cy")"#,
        ids[1]
    ))
    .await
    .expect("one is changed");
    let ran = run_to_end(&registry, &tenant, &plan, &keys)
        .await
        .expect("the step ends");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(
        one(
            &tpg,
            "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = 'note$body$uq'"
        ),
        "UNIQUE (\"$space\", \"body$bidx\")",
        "the constraint stands over the new index"
    );
    // `set` over a unique field is keyed by it: `ADA` is `Ada`'s note now.
    let y = node(&tpg.db, &normalized_v2(true), "y", keys.clone()).await;
    y.write(r#"addNote($title: "fourth", $body: "ADA")"#.into())
        .await
        .expect("a write keyed by the unique");
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM note"),
        "3",
        "no second `Ada`"
    );
    assert_eq!(
        one(
            &tpg,
            &format!("SELECT title FROM note WHERE id = '{}'", ids[0])
        ),
        "fourth",
        "`ADA` is the note that held `Ada`"
    );
    let rows = y
        .exec
        .read_as("note(body == \"cY\")[title]", Some("p_ada"))
        .await
        .expect("reads");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["title"], "second");
}

/// **B10 through `ordo apply` is refused by name**, naming `ordo evolve`.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_normalize_outside_a_stored_plan_is_refused_by_name() {
    let (_tpg, tenant) = empty("ordo_seal_renorm_apply").await;
    let x = Program::compile(&sealed_indexed_v1(false)).expect("1.0.0");
    let y = Program::compile(&normalized_v2(false)).expect("1.1.0");
    let (_, created) = sa::apply(&tenant, &x, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    // Planned as `ordo apply` plans it, against the version written.
    let plan = sa::plan_with(&tenant, &y, &Choices::default(), Some(&x.schema))
        .await
        .expect("plans");
    let text = plan.refusals.join("; ");
    assert!(text.contains("reindex_needs_stored_plan"), "{text}");
    let why = format!("{:?}", plan.assessed);
    assert!(why.contains("ordo evolve"), "names the command: {why}");
}

/// One node serving `source` for the deployment's own space (`home`),
/// under pin `epoch`, as `Spaces::resolve` hands it a handle.
async fn own_node(db: &str, source: &str, keys: Arc<dyn KeyProvider>, epoch: i64) -> Node {
    let mut n = node(db, source, &format!("own{epoch}"), keys).await;
    let home = SpaceId("home".into());
    n.exec.pool = n.exec.pool.for_space(&home).with_pin_epoch(epoch);
    n.exec.space = home;
    n.fixture = Arc::new(n.fixture.in_space("home"));
    n
}

/// **The deployment's own space through a seal on its database**
/// (`ordo evolve --run`). Before the step, the own space is given a pin at
/// the active version, so its writes carry an epoch. The seal's end raises
/// its pin fence in the end's transaction, and the own space then moves to
/// the version the database holds, first, as a rollout moves it (Q4).
/// "Active" stays until the activation. A node still on the version before
/// is told the pin moved, never a type error on the sealed column; a node
/// on the new version writes, and the value lands sealed.
///
/// A regression looks like a write by the version before failing on the
/// column, an own space left on the version before, or "active" moved by
/// the run.
#[tokio::test(flavor = "multi_thread")]
async fn the_own_space_moves_with_a_seal_on_its_database_and_its_writes_land_sealed() {
    let (pg, pool) = empty("ordo_seal_own").await;
    ordo_host::tenants::ensure_table(&pool)
        .await
        .expect("$spaces");
    let x = Program::compile(&v1_writes()).expect("1.0.0");
    ordo_host::apply_op::run_program(
        &pool,
        &ordo_exec::route::SinglePool::new(pool.clone()),
        &x,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
    .expect("1.0.0 is active");
    store(&pool, &v2_writes()).await;
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let before = own_node(&pg.db, &v1_writes(), keys.clone(), 0).await;
    for i in 0..5 {
        before
            .write(format!(
                r#"addNote($title: "own-{i}", $body: "own body {i}")"#
            ))
            .await
            .expect("the own space writes before the seal");
    }

    let ops = ordo_host::apply_op::ApplyOps::recording(
        pool.clone(),
        Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        "home",
        None,
    );
    let _ = ops.backfill_keys.set(keys.clone());
    let b = ordo_api::BoundParams(
        serde_json::json!({ "from": "1.0.0", "to": "1.1.0", "run": true })
            .as_object()
            .unwrap()
            .clone(),
    );
    let done =
        ordo_host::deploy_ops::execute(&ops, ordo_host::deploy_ops::EVOLVE, "home", &b, OWNER)
            .await
            .expect("the step runs");
    let row = &done.rows[0];
    assert_eq!(row["ran"]["ownSpace"]["version"], "1.1.0", "{row}");
    assert_eq!(column_type(&pg, "body"), "bytea");

    let v = Versions::on(&pool);
    assert_eq!(
        v.own_pin().await.unwrap(),
        Some(("home".into(), "1.1.0".into(), 1)),
        "the own space follows the data"
    );
    assert_eq!(
        v.active_version().await.unwrap().as_deref(),
        Some("1.0.0"),
        "active waits for the activation"
    );
    assert_eq!(
        one(&pg, "SELECT epoch FROM \"$pin_fence\" WHERE space = 'home'"),
        "1",
        "the end fenced the version before"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT string_agg(coalesce(\"from\", '-') || '>' || \"to\" || '@' || epoch, ' ' \
             ORDER BY seq) FROM \"$pins\" WHERE space = 'home'"
        ),
        "->1.0.0@0 1.0.0>1.1.0@1",
        "every node is told each pin"
    );

    let err = before
        .write(r#"addNote($title: "late", $body: "late body")"#.into())
        .await
        .expect_err("the version before writes nothing more");
    assert!(err.contains("pin_moved"), "told the pin moved: {err}");
    let after = own_node(&pg.db, &v2_writes(), keys.clone(), 1).await;
    after
        .write(r#"addNote($title: "after", $body: "sealed after")"#.into())
        .await
        .expect("the own space writes on the new version");
    assert_eq!(
        one(
            &pg,
            "SELECT string_agg(DISTINCT \"$space\", ',') FROM note WHERE title IN ('after', 'own-3')"
        ),
        "home",
        "the writes are the own space's"
    );
    let id = one(&pg, "SELECT id FROM note WHERE title = 'own-3'");
    assert_eq!(open(&keys, &pg, "home", &id), "own body 3");
    let id = one(&pg, "SELECT id FROM note WHERE title = 'after'");
    assert_eq!(open(&keys, &pg, "home", &id), "sealed after");
}

/// `==` on `body` under `source`, as a node serving it reads: the titles.
async fn found(n: &Node, body: &str) -> Vec<String> {
    n.exec
        .read_as(&format!("note(body == \"{body}\")[title]"), Some("p_ada"))
        .await
        .expect("reads")
        .iter()
        .filter_map(|r| r["title"].as_str().map(String::from))
        .collect()
}

/// **A rollback across B10 reads a whole index** (owner, 2026-11-24, PR
/// 80). 1.0.0 has `body` sealed and indexed with no normalization; 1.1.0
/// adds `normalize(lower)`. Notes in mixed case are written before the
/// step and after 1.1.0 is active. The step keeps the old index as
/// `body$bidx$prev` and every write keeps it current. Rolled back to
/// 1.0.0, the kept index is traded back in, and `==` under 1.0.0 finds
/// every note by its exact value, the ones 1.1.0 wrote too. Forward again,
/// `==` under 1.1.0 finds them in any case.
///
/// A regression looks like a rollback where `==` misses a row written in
/// another case, or a roll forward that reads the old index.
#[tokio::test(flavor = "multi_thread")]
async fn a_rollback_across_a_new_normalize_finds_every_row() {
    let (pg, pool) = empty("ordo_seal_renorm_back").await;
    ordo_host::tenants::ensure_table(&pool)
        .await
        .expect("$spaces");
    let x = Program::compile(&sealed_indexed_v1(false)).expect("1.0.0");
    ordo_host::apply_op::run_program(
        &pool,
        &ordo_exec::route::SinglePool::new(pool.clone()),
        &x,
        vec![],
        "dev:ada",
        "t",
        true,
    )
    .await
    .expect("1.0.0 is active");
    store(&pool, &normalized_v2(false)).await;
    let keys: Arc<dyn KeyProvider> = Arc::new(TestProvider::new());
    let old = node(&pg.db, &sealed_indexed_v1(false), "old", keys.clone()).await;
    for (title, body) in [("a", "Mixed-A"), ("b", "mixed-b"), ("c", "MIXED-c")] {
        old.write(format!(r#"addNote($title: "{title}", $body: "{body}")"#))
            .await
            .expect("a write before");
    }
    let plan = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect("stored")
        .plan;
    let ran = evolve::run_step_with(
        &pool,
        &pool,
        "default",
        &plan,
        OWNER,
        &ctx(Some(keys.clone()), 100, None),
    )
    .await
    .expect("the reindex runs");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    ordo_host::apply_op::activate(&pool, "1.1.0", "dev:ada")
        .await
        .expect("1.1.0 is active");
    let new = node(&pg.db, &normalized_v2(false), "new", keys.clone()).await;
    for (title, body) in [("d", "NeW-D"), ("e", "new-E")] {
        new.write(format!(r#"addNote($title: "{title}", $body: "{body}")"#))
            .await
            .expect("a write on 1.1.0");
    }
    assert_eq!(found(&new, "mixed-a").await, ["a"]);

    ordo_host::apply_op::activate(&pool, "1.0.0", "dev:ada")
        .await
        .expect("rolled back");
    for (body, title) in [
        ("Mixed-A", "a"),
        ("mixed-b", "b"),
        ("MIXED-c", "c"),
        ("NeW-D", "d"),
        ("new-E", "e"),
    ] {
        assert_eq!(
            found(&old, body).await,
            [title],
            "`{body}` under 1.0.0 after the rollback"
        );
    }
    assert!(
        found(&old, "mixed-a").await.is_empty(),
        "1.0.0 compares exactly"
    );
    old.write(r#"addNote($title: "f", $body: "Back-F")"#.into())
        .await
        .expect("a write after the rollback");

    ordo_host::apply_op::activate(&pool, "1.1.0", "dev:ada")
        .await
        .expect("forward again");
    for (body, title) in [("MIXED-A", "a"), ("new-d", "d"), ("back-f", "f")] {
        assert_eq!(found(&new, body).await, [title], "`{body}` under 1.1.0");
    }
}

/// Increment 11b part 2: **a seal on a database 3 tenants share fences
/// each of them in its end's transaction.** Part 1 refused it as
/// `seal_shared_database`. The end drops the plaintext, so every tenant
/// there must be told its pin moved before a node on the version before
/// writes again: each one's `$pin_fence` rises to the epoch of the pin it
/// takes next, as the catch-up passes them (`every_tenant::seal_fence`).
///
/// A regression looks like one tenant's fence left behind: a node still on
/// the version before would write plaintext into a dropped column.
#[tokio::test(flavor = "multi_thread")]
async fn a_seal_on_a_database_3_tenants_share_fences_each_of_them() {
    let (_rpg, registry, tpg, tenant, plan) = setup("ordo_seal_shared", &v2()).await;
    let (keys, _) = metered();
    let mut c = ctx(Some(keys), 100, None);
    c.fence = Some(("acme".into(), 5));
    c.fences = vec![("beta".into(), 3), ("gamma".into(), 8)];
    let ran = evolve::run_step_with(&registry, &tenant, "tenant-db", &plan, OWNER, &c)
        .await
        .expect("the seal runs");
    assert!(matches!(ran, StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(column_type(&tpg, "body"), "bytea");
    for (s, epoch) in [("acme", "5"), ("beta", "3"), ("gamma", "8")] {
        assert_eq!(
            one(
                &tpg,
                &format!("SELECT epoch FROM \"$pin_fence\" WHERE space = '{s}'")
            ),
            epoch,
            "{s}'s fence"
        );
    }
}
