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

/// **A field a reader app reads is not sealed**: the render is refused as
/// `seal_read_by_reader`, naming the reader and what lets it through, and
/// no step is stored.
#[tokio::test(flavor = "multi_thread")]
async fn a_seal_of_a_field_a_reader_reads_is_refused_naming_the_reader() {
    let with_reader = |src: String| {
        src.replace(
            "\nentity note",
            "\nreader note_viewer reads note\n\nentity note",
        )
    };
    let (_rpg, registry) = empty("ordo_seal_reader_reg").await;
    store(&registry, &with_reader(v1())).await;
    store(&registry, &with_reader(v2())).await;
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
    assert!(text.contains("seal_read_by_reader"), "{text}");
    assert!(text.contains("note_viewer"), "names the reader: {text}");
    assert!(text.contains("7b"), "names what lets it through: {text}");
    assert!(
        evolve::path(&registry, "1.0.0", "1.1.0")
            .await
            .map_or(true, |p| p.is_empty()),
        "no step is stored"
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
        1,
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

/// B8 through `ordo apply`, `sealed unique`, and sealing and indexing in one
/// step are refused by name, each naming what to do.
#[tokio::test(flavor = "multi_thread")]
async fn a_blind_index_outside_a_stored_plan_or_unique_is_refused_by_name() {
    let (_rpg, registry) = empty("ordo_seal_bidx_refused_reg").await;
    store(&registry, &sealed_v1()).await;
    store(&registry, &indexed_v2(true)).await;
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
    assert!(text.contains("sealed_unique_backfill_not_built"), "{text}");

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
