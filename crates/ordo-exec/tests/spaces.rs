//! 11-27, **the isolation suite one layer up**: two spaces, the same
//! ids, one database, driven through `ordo-exec` rather than through the
//! write protocol.
//!
//! `ordo-conformance/tests/spaces.rs` (9h) states the method and this file
//! keeps it unchanged: seed the fixture **twice**, into `conformance` and
//! into `shadow`, with the SAME row ids in both, then make the two differ
//! in ways a leak would show. Its argument about ids is the load-bearing
//! part and applies here word for word. Real typed ids do not collide,
//! "and that is precisely why an id-only test proves nothing: a query
//! missing its space predicate still finds the right row by id and looks
//! correct forever, until two tenants share a row id for any reason at
//! all". Where this suite reaches a store whose keys are minted rather
//! than seeded (the command chain, the sealed payload store, the blob
//! reference store), it forces the collision by hand for the same reason.
//!
//! # Why one layer up needs its own suite
//!
//! 9h reaches the write protocol and stops: it names neither `ordo_exec`
//! nor `ordo_host`. Three space-isolation defects found in two days all
//! lived above that line, and all three were the same shape: a statement
//! addressing a row, a chain, or a disclosure by something other than
//! `(space, key)`.
//!
//! - **C1** found seven statements in the revert applier addressing rows
//!   by `"id"` alone, while ids are minted from a process counter with no
//!   space salt and so collide across spaces by construction.
//! - **C2** found the write protocol NFC-normalising space literals and
//!   the revert applier not doing so.
//! - **D0** found the platform surfaces authorizing against a
//!   caller-supplied `x-ordo-space` header.
//!
//! # The harness, and why it is 9h's and not `undo_space.rs`'s
//!
//! `crates/ordo-exec/tests/undo_space.rs` builds its twin with direct SQL,
//! on the stated ground that "the executor is bound to one space". That is
//! true of an executor, not of the executor *type*:
//! [`ordo_exec::Executor`]'s `space` is a field, `LoggedExecutor`'s chain
//! space is a field, and the write protocol takes its space from the
//! `Fixture` it is handed. So a second space is a second
//! `Fixture::in_space` plus a second `Executor` over the **same pool**, and
//! the twin's rows are then produced by the same seed and the same write
//! path as the real space's rather than by hand-written SQL that could
//! describe a state no write protocol would ever produce. That is 9h's
//! construction, lifted intact, and it is what makes "the only difference
//! between the two is the space" true here too.
//!
//! Each test owns its database, so they may be separate `#[test]`s: 9h is
//! one function only because its write checks share one database and
//! build on each other's state.
//!
//! # What each test is for
//!
//! | test                                    | the statement it pins                         |
//! |-----------------------------------------|-----------------------------------------------|
//! | `a_revert_leaves_the_twins_row_and_chain` | the revert's row write and the chain's head read |
//! | `a_revert_resolves_its_own_spaces_payload` | the sealed payload store's lookup           |
//! | `two_spaces_append_at_once`             | the chain's per-space head and index           |
//! | `a_sweep_collects_one_spaces_object`     | `blobgc`'s mark, anti-join and object delete  |
//! | `erasing_one_space_leaves_the_twins_bytes` | `blobgc::erase_space`'s three statements    |
//! | `a_denial_does_not_reach_the_twins_stream` | `Recorder::route` and D1b's `about`         |

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_blob::{BlobPipeline, BlobPolicy, BlobStore, FsStore, SingleStore, Upload};
use ordo_commandlog::{CorrelationId, Timestamp};
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_crypto::custody::SpaceId as CryptoSpace;
use ordo_exec::cadence::LogPolicy;
use ordo_exec::secevent::PgSecurityStore;
use ordo_exec::{blobgc, undo, Executor, LoggedExecutor, Pool, PoolConfig};
use ordo_secevent::{
    EventKind, Observation, RecordedDecision, Recorder, Relationship, Scope, Stamp, Subject,
};

/// The real space is the fixture's own, so the seed needs no argument to
/// mean "the tenant every other suite runs as".
const REAL: &str = "conformance";
/// 9h's name for the twin, kept so a reader moving between the two files
/// does not have to hold two names for one idea.
const TWIN: &str = "shadow";
/// Ana. A member of both spaces by the seed, with the same id in both.
/// That is the point: a guard that forgot its space would authorise her
/// in the twin on the strength of a grant she holds in the real space.
const ANA: &str = "mem_1";

// ── the two-space fixture harness ───────────────────────────────────────

/// One database, one pool, two spaces, two executors.
struct Twins {
    pool: Pool,
    real: Arc<Fixture>,
    twin: Arc<Fixture>,
    log: Arc<Mutex<WriteLog>>,
}

async fn pool_for(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 8,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot")
}

fn make_db(db: &str) -> Pg {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    Pg {
        db: db.to_string(),
        ..Pg::default()
    }
}

/// The fixture seeded twice into one schema, and a pool over it.
///
/// `reset` builds the tables and seeds the real space; `seed_into` adds
/// the twin's rows to the same tables. **The seed succeeding at all is the
/// first result**, exactly as 9h says: `categories.name` is a plain
/// `unique` field and the twin seeds the same five names, so a
/// deployment-wide unique index refuses the second space outright, as does
/// a primary key that is not `(space, id)`.
async fn twins(db: &str) -> Twins {
    let real = Fixture::load().expect("fixture");
    let twin = real.in_space(TWIN);
    let pg = make_db(db);
    real.reset(&pg).expect("the real space seeds");
    twin.seed_into(&pg).expect("the twin seeds beside it");

    let pool = pool_for(db).await;
    // The chain starts empty so an index in an assertion is the index this
    // test's own writes produced.
    for t in ["$commandlog", "$checkpoints"] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }
    Twins {
        pool,
        real: Arc::new(real),
        twin: Arc::new(twin),
        log: Arc::new(Mutex::new(WriteLog::new())),
    }
}

impl Twins {
    /// An executor bound to one of the two spaces. The fixture's space,
    /// the executor's space and the chain's space are set from one value,
    /// because they are one fact: a write is planned, keyed and logged in
    /// the space the door bound.
    fn exec(&self, space: &str) -> (Arc<Fixture>, LoggedExecutor) {
        let f = if space == TWIN {
            self.twin.clone()
        } else {
            self.real.clone()
        };
        let e = LoggedExecutor::new(
            Executor {
                pool: self.pool.clone(),
                schema: f.schema.clone(),
                registry: f.registry.clone(),
                keys: f.keys.clone(),
                space: CryptoSpace(space.to_string()),
            },
            space,
        );
        (f, e)
    }

    async fn write(&self, space: &str, call: &str) -> serde_json::Value {
        let (f, e) = self.exec(space);
        e.write_as(f, self.log.clone(), call.to_string(), ANA.to_string())
            .await
            .unwrap_or_else(|err| panic!("write `{call}` in `{space}`: {err}"))
    }

    /// One schema, shared. `in_space` clones it, so naming either
    /// fixture's would be the same value, which is itself the point: the
    /// only difference between the two spaces is the space.
    fn schema(&self) -> ordo_schema::Schema {
        self.real.schema.clone()
    }

    async fn rows(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        self.pool
            .rows(sql)
            .await
            .unwrap_or_else(|e| panic!("`{sql}`: {e}"))
    }

    async fn one(&self, sql: &str) -> Option<String> {
        self.rows(sql).await.first()?.first()?.clone()
    }

    /// The correlation of the group at `idx` in a space's chain.
    async fn correlation(&self, space: &str, idx: usize) -> CorrelationId {
        let hex = self
            .one(&format!(
                "SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = '{space}' AND \"idx\" = {idx}"
            ))
            .await
            .expect("a group at this index");
        let body = ordo_crypto::sealed::unhex(&hex).expect("a body");
        CorrelationId(
            ordo_commandlog::chain::decode_group(&body)
                .expect("decodes")
                .correlation
                .0,
        )
    }

    /// `(prev, hash)` at a chain position, hex, so an assertion can name
    /// which space's link a group was built onto.
    async fn link(&self, space: &str, idx: usize) -> (String, String) {
        let r = self
            .rows(&format!(
                "SELECT encode(\"prev\", 'hex'), encode(\"hash\", 'hex') FROM \"$commandlog\" \
                 WHERE \"space\" = '{space}' AND \"idx\" = {idx}"
            ))
            .await;
        let row = r.first().unwrap_or_else(|| {
            panic!(
                "`{space}` has no chain entry at index {idx}. An index \
                    allocated from a head read without its space predicate \
                    lands wherever the neighbour left off"
            )
        });
        (row[0].clone().expect("prev"), row[1].clone().expect("hash"))
    }

    async fn chain_len(&self, space: &str) -> usize {
        self.one(&format!(
            "SELECT count(*) FROM \"$commandlog\" WHERE \"space\" = '{space}'"
        ))
        .await
        .and_then(|s| s.parse().ok())
        .expect("a count")
    }
}

// ── the revert applier ──────────────────────────────────────────────────

/// **The whole revert, under two spaces.** `undo_space.rs` pins five of
/// C1's seven statements one at a time, with a hand-built twin. This is
/// the property those five add up to and none of them states: a revert
/// that runs in one space changes that space's row and that space's chain,
/// and leaves the twin's row at the same id and the twin's chain exactly
/// as they were.
///
/// The chain half is the one that could not be tested a statement at a
/// time, and it belongs to `ordo_exec::commandlog` rather than to `undo`:
/// `append_under_lock` reads the last `(idx, hash)` for a space and builds
/// the next link from it. The twin writes **after** the real space does,
/// so the twin holds the most recently appended row in `$commandlog`. A
/// head read without its space predicate would hand the revert the twin's
/// head and the twin's index, so the revert's group would chain onto a
/// link from another tenant's history and land at a position its own
/// chain never reached. `verify(space)` then reports a broken chain in a
/// space that did nothing wrong, and one tenant's hash is in another
/// tenant's audit spine.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_leaves_the_twins_row_and_chain() {
    let t = twins("ordo_spaces_revert").await;

    // The same seeded row id in both spaces, renamed differently in each.
    t.write(
        REAL,
        r#"renameTask($task: "task_1", $title: "real rename")"#,
    )
    .await;
    t.write(
        TWIN,
        r#"renameTask($task: "task_1", $title: "twin rename")"#,
    )
    .await;
    let (_, twin_head) = t.link(TWIN, 0).await;
    let (_, real_head) = t.link(REAL, 0).await;
    assert_ne!(
        twin_head, real_head,
        "two spaces must not produce the same link for different writes"
    );

    let correlation = t.correlation(REAL, 0).await;
    let reverted = undo::revert(
        &t.pool,
        &t.schema(),
        None,
        None,
        None,
        &ordo_commandlog::SpaceId(REAL.into()),
        &correlation,
        ANA,
        Timestamp(1),
    )
    .await
    .expect("Ana renamed it, so Ana may unrename it");

    // The rows: one moved, one did not, and they share an id.
    let titles = t
        .rows(r#"SELECT "$space", "title" FROM "tasks" WHERE "id" = 'task_1' ORDER BY "$space""#)
        .await;
    assert_eq!(
        titles,
        vec![
            vec![Some(REAL.into()), Some("draft spec".into())],
            vec![Some(TWIN.into()), Some("twin rename".into())],
        ],
        "the revert restored its own space's row and left the twin's alone"
    );

    // The chains: the revert is more history, in ONE space's spine.
    assert_eq!(t.chain_len(REAL).await, 2, "the revert joined this chain");
    assert_eq!(t.chain_len(TWIN).await, 1, "and not the twin's");
    assert_eq!(
        reverted.index, 1,
        "the revert took the next position in ITS OWN chain. An index \
         counted from a head read without its space predicate skips ahead \
         by however much the neighbour has written"
    );
    let (prev, _) = t.link(REAL, reverted.index).await;
    assert_eq!(
        prev, real_head,
        "the revert chained onto its own space's head"
    );
    assert_ne!(
        prev, twin_head,
        "and not onto the twin's, which is the more recently appended row \
         in `$commandlog` and so the one an unscoped head read returns"
    );
}

/// **The sealed payload store.** A revert re-opens the images the group
/// recorded, and a sealed field's image is a handle into
/// `$sealedpayload`, whose primary key is `("$space", "ref")`.
///
/// Handles are minted, not seeded, so this test forces the collision the
/// way 9h forces every other one: the twin's payload row is moved onto the
/// real space's handle, and the real space's own row is then removed,
/// standing in for a prune, since a revert does not need to know why a
/// payload is gone, only that it is.
///
/// The two answers are distinguishable, which is what makes this worth
/// asserting rather than reviewing. Scoped, the lookup finds nothing in
/// `conformance` and the refusal is `Resolved::Pruned`: *"not in this
/// space's sealed payload store"*. Unscoped, it finds the twin's row and
/// tries to open an envelope sealed under the twin's own space key, which
/// fails, and the refusal is `Resolved::Destroyed`: *"the key has been
/// destroyed"*. A revert that told an operator a key was destroyed when
/// the truth was that it read another tenant's payload is a diagnosis that
/// sends them to the wrong system entirely.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_resolves_its_own_spaces_payload() {
    let t = twins("ordo_spaces_payload").await;

    t.write(
        REAL,
        r#"addCustomer($name: "Zed", $email: "zed@example.com")"#,
    )
    .await;
    t.write(
        TWIN,
        r#"addCustomer($name: "Zed", $email: "zed@example.com")"#,
    )
    .await;
    let correlation = t.correlation(REAL, 0).await;

    let handle_of = |space: &str| {
        format!(
            "SELECT encode(\"ref\", 'hex') FROM \"$sealedpayload\" WHERE \"$space\" = '{space}'"
        )
    };
    let real_ref = t.one(&handle_of(REAL)).await.expect("a real payload");
    let twin_ref = t.one(&handle_of(TWIN)).await.expect("a twin payload");
    assert_ne!(
        real_ref, twin_ref,
        "minted handles do not collide on their own, which is why this test \
         makes them"
    );

    t.rows(&format!(
        "UPDATE \"$sealedpayload\" SET \"ref\" = decode('{real_ref}', 'hex') \
         WHERE \"$space\" = '{TWIN}' AND \"ref\" = decode('{twin_ref}', 'hex')"
    ))
    .await;
    t.rows(&format!(
        "DELETE FROM \"$sealedpayload\" WHERE \"$space\" = '{REAL}' \
         AND \"ref\" = decode('{real_ref}', 'hex')"
    ))
    .await;

    let err = undo::revert(
        &t.pool,
        &t.schema(),
        Some(t.real.keys.as_ref()),
        None,
        None,
        &ordo_commandlog::SpaceId(REAL.into()),
        &correlation,
        ANA,
        Timestamp(1),
    )
    .await
    .expect_err("the payload this space recorded is gone from this space's store")
    .to_string();
    assert!(
        err.contains("not in this space's sealed payload store"),
        "the lookup must not answer with the twin's payload under the same \
         handle: {err}"
    );

    // And the twin's payload is still there, untouched by a revert that
    // ran in another space and read the same handle.
    let left = t
        .rows(&format!(
            "SELECT \"entity\", \"field\" FROM \"$sealedpayload\" \
             WHERE \"$space\" = '{TWIN}' AND \"ref\" = decode('{real_ref}', 'hex')"
        ))
        .await;
    assert_eq!(
        left,
        vec![vec![Some("customers".into()), Some("email".into())]],
        "the twin's payload row survives"
    );
}

// ── the command chain ───────────────────────────────────────────────────

/// A serialization conflict that reached the caller unretried.
///
/// **This must now never be true, and the assertion is the point.** When
/// this suite was written the platform reported an append conflict as
/// `ExecFail::Compile("log_append_failed: …")`, which `write_retrying`
/// does not retry, so the test retried it itself to avoid going red at
/// random. That defect is fixed: `store_err` types a retryable pool
/// failure as `AppendError::Retryable` and the append site surfaces it as
/// `ExecFail::Pool(Serialization)`, which the retry loop does see. The
/// helper is kept, inverted, so that a regression fails here rather than
/// quietly returning to refusing one tenant's write because another
/// tenant wrote at the same instant.
fn unretried_conflict(err: &ordo_exec::ExecFail) -> bool {
    let msg = err.to_string();
    msg.contains("log_append_failed") && msg.contains("serialization_conflict")
}

/// **Two spaces appending at once.** `commandlog.rs`'s
/// `concurrent_appenders_in_one_space_serialize` proves that two appenders
/// in ONE space queue behind one lock; `a_lock_in_one_space_does_not_block
/// _another` proves the lock keys differ. Neither says what the two
/// chains look like afterwards, and that is the property a tenant cares
/// about: two spaces writing at the same instant produce two chains that
/// do not interleave, do not share a head, and each verify on their own.
///
/// A chain whose head read or whose index allocation dropped the space
/// predicate fails here in a way a single-space suite cannot see: the
/// indices would be shared, so one space's chain would have gaps and the
/// other's links would point at bodies belonging to a tenant it has never
/// heard of.
///
/// # The conflict this used to work around, and no longer does
///
/// Two spaces appending at once take different advisory lock keys and
/// write disjoint rows, and they share `$commandlog`, so under the
/// SERIALIZABLE level `OpClass::MultiStatementGroup` pins, Postgres's SSI
/// raises a `40001` between them anyway. This test found that the platform
/// reported it as `ExecFail::Compile("log_append_failed: …")` while
/// `write_retrying` retries only `ExecFail::Pool(e) if e.is_retryable()`,
/// so the retry loop built for exactly this class never saw it and one
/// tenant's good write was refused because a different tenant wrote at the
/// same instant. Fixed: the typed pool error survives as
/// `AppendError::Retryable` and the append site surfaces it as a pool
/// serialization failure. The test no longer retries, and asserts that it
/// does not have to.
///
/// It is reported rather than fixed here: this is a test-only increment,
/// and re-typing an append conflict is a change to the write path that
/// deserves its own decision. The retry is in the test, in the open, so
/// the isolation property this test is actually about stays pinned
/// meanwhile.
#[tokio::test(flavor = "multi_thread")]
async fn two_spaces_append_at_once() {
    let t = twins("ordo_spaces_chain").await;
    const EACH: usize = 6;

    let gate = Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for space in [REAL, TWIN] {
        let (f, e) = t.exec(space);
        // A `WriteLog` of its own per appender, not the shared one. The
        // type holds exactly one pending group and the write path takes it
        // across a whole transaction, so it is a per-writer mirror by
        // construction: two concurrent writers sharing one would each wait
        // for the other's lock while holding their own transaction open,
        // and this test would deadlock on the harness rather than measure
        // the platform. The durable chain, the thing under test, is in
        // Postgres and is shared regardless.
        let log = Arc::new(Mutex::new(WriteLog::new()));
        let gate = gate.clone();
        tasks.push(tokio::spawn(async move {
            gate.wait().await;
            for i in 0..EACH {
                let call = format!(r#"renameTask($task: "task_1", $title: "{space}-{i}")"#);
                // No retry here, and that is the assertion. The platform
                // owns this retry now; a conflict that reaches the caller
                // unretried is the regression this helper names.
                if let Err(err) = e
                    .write_as(f.clone(), log.clone(), call.clone(), ANA.to_string())
                    .await
                {
                    assert!(
                        !unretried_conflict(&err),
                        "write {i} in `{space}` was refused for a conflict the platform \
                         should have retried: {err}"
                    );
                    panic!("write {i} in `{space}`: {err}");
                }
            }
        }));
    }
    for task in tasks {
        task.await.expect("both appenders finish");
    }

    for space in [REAL, TWIN] {
        assert_eq!(
            t.chain_len(space).await,
            EACH,
            "`{space}` holds its own writes and only its own"
        );
        // Contiguous from zero: a shared index sequence would leave one of
        // the two chains with holes.
        let idx = t
            .rows(&format!(
                "SELECT \"idx\" FROM \"$commandlog\" WHERE \"space\" = '{space}' ORDER BY \"idx\""
            ))
            .await;
        assert_eq!(
            idx,
            (0..EACH)
                .map(|i| vec![Some(i.to_string())])
                .collect::<Vec<_>>(),
            "`{space}`'s indices are its own sequence, not a share of one"
        );
    }

    // The heads are different values, and neither chain's genesis link
    // points at the other's.
    let (real_genesis, _) = t.link(REAL, 0).await;
    let (twin_genesis, _) = t.link(TWIN, 0).await;
    let zero = "0".repeat(64);
    assert_eq!(real_genesis, zero, "each space's chain starts at genesis");
    assert_eq!(twin_genesis, zero, "including the one that started second");
    let (_, real_head) = t.link(REAL, EACH - 1).await;
    let (_, twin_head) = t.link(TWIN, EACH - 1).await;
    assert_ne!(real_head, twin_head, "two spines, not one shared head");

    // Both verify, independently and with no reference to the other.
    let store =
        ordo_exec::commandlog::PgStore::new(t.pool.clone(), tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        use ordo_commandlog::Store;
        for space in [REAL, TWIN] {
            store
                .verify(&ordo_commandlog::SpaceId(space.into()))
                .unwrap_or_else(|e| panic!("`{space}`'s chain must verify alone: {e:?}"));
        }
    })
    .await
    .expect("verification runs");
}

// ── the blob collector ──────────────────────────────────────────────────

/// A one-entity schema with one external blob field, `blobgc.rs`'s own.
const BLOB_SRC: &str = "\
domain gc @ 1.0.0
entity docs (key: doc) {
  title: text
  scan:  blob?
}
";

/// The collector's two-space rig: one docs table, one object store, two
/// spaces holding **the same handle**.
struct BlobTwins {
    pool: Pool,
    schema: ordo_schema::Schema,
    dir: std::path::PathBuf,
    stores: SingleStore,
    store: Arc<dyn BlobStore>,
}

impl Drop for BlobTwins {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn cspace(s: &str) -> CryptoSpace {
    CryptoSpace(s.to_string())
}

/// The same object, under the same handle, in both spaces, with a live
/// row naming it in the twin and none in the real space.
///
/// The handle is **copied rather than re-minted**, and the `$blobref` row
/// is copied with `INSERT … SELECT` rather than reconstructed, for
/// `undo_space.rs`'s reason: a test that rebuilt a sealed column would have
/// to know how to seal, and reusing bytes that are already known-valid
/// keeps the test about the sweep. What it buys is the collision a minted
/// 16-byte handle will not produce on its own, which is the only shape in
/// which a missing space predicate is visible at all.
async fn blob_twins(db: &str) -> (BlobTwins, [u8; 16]) {
    let pg = make_db(db);
    Fixture::load()
        .expect("fixture")
        .reset(&pg)
        .expect("the platform tables");
    let pool = pool_for(db).await;

    let schema =
        ordo_schema::build(&ordo_syntax::parse_program(BLOB_SRC).expect("parses")).expect("builds");
    pool.rows("DROP TABLE IF EXISTS \"docs\"").await.unwrap();
    for stmt in ordo_schema::ddl_for(&schema)
        .expect("ddl")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains("\"docs\""))
    {
        pool.rows(stmt)
            .await
            .unwrap_or_else(|e| panic!("{stmt}: {e}"));
    }
    ordo_exec::prune::ensure_tables(&pool).await.expect("prune");
    ordo_exec::cadence::ensure_tables(&pool)
        .await
        .expect("cadence");
    for t in ["$blobref", "$blobtomb", "$commandlog", "$checkpoints"] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }

    let dir = std::env::temp_dir().join(format!("ordo-spaces-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = FsStore::shared(&dir).expect("store");
    let keys: Arc<dyn ordo_crypto::custody::KeyProvider> =
        Arc::new(ordo_crypto::custody::TestProvider::with_space(REAL));
    let pipe = BlobPipeline::new(
        Arc::new(SingleStore::new(Arc::clone(&store))),
        keys,
        BlobPolicy::DEFAULT,
    );

    // One real upload into the real space.
    let stored = pipe
        .put(
            &Upload {
                space: &cspace(REAL),
                entity: "docs",
                field: "scan",
                mime: "application/pdf",
                plain_len: 17,
                correlation: "spaces",
                intent: None,
            },
            ordo_blob::once(b"a signed contract".to_vec()),
        )
        .await
        .expect("put");
    let handle = *stored.handle();
    let hex = ordo_crypto::hex(&handle);

    let tx = pool.begin().await.expect("tx");
    tx.rows(&ordo_crypto::blobref::insert_sql(
        &cspace(REAL),
        &handle,
        stored.epoch(),
        "docs",
        "scan",
        stored.mime(),
        stored.size(),
        stored.integrity(),
        stored.bytes(),
        1_000,
        "",
    ))
    .await
    .expect("the real reference row");
    tx.commit().await.expect("commit");

    // The twin's reference row: the same handle, every sealed column
    // copied byte for byte.
    pool.rows(&format!(
        "INSERT INTO \"$blobref\" (\"$space\", \"ref\", \"epoch\", \"entity\", \"field\", \
         \"mime\", \"size\", \"integrity\", \"bytes\", \"created_at\") \
         SELECT '{TWIN}', \"ref\", \"epoch\", \"entity\", \"field\", \"mime\", \"size\", \
         \"integrity\", \"bytes\", \"created_at\" \
         FROM \"$blobref\" WHERE \"$space\" = '{REAL}'"
    ))
    .await
    .expect("the twin's reference row");

    // The twin's bytes, at the same handle under its own space prefix.
    let bytes = ordo_blob::collect(
        store
            .get(&cspace(REAL), &handle, None)
            .await
            .expect("the object"),
    )
    .await
    .expect("collected");
    store
        .put(
            &cspace(TWIN),
            &handle,
            bytes.len() as u64,
            ordo_blob::once(bytes),
        )
        .await
        .expect("the twin's object");

    // The rows that name it: the SAME row id in both spaces, and only the
    // twin's survives below.
    for space in [REAL, TWIN] {
        pool.rows(&format!(
            "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"title\", \"scan\") \
             VALUES ('{space}', 'doc_1', 1, 'a doc', '{hex}')"
        ))
        .await
        .expect("a referring row");
    }

    (
        BlobTwins {
            pool,
            schema,
            dir,
            stores: SingleStore::new(Arc::clone(&store)),
            store,
        },
        handle,
    )
}

impl BlobTwins {
    async fn sweep(&self, space: &str, now: u64) -> blobgc::SweepReport {
        blobgc::sweep(
            &self.pool,
            &self.stores,
            &self.schema,
            &cspace(space),
            &LogPolicy {
                undo_horizon: Duration::from_secs(86_400),
                ..LogPolicy::DEFAULT
            },
            now,
        )
        .await
        .expect("sweep")
    }

    async fn object_exists(&self, space: &str, handle: &[u8; 16]) -> bool {
        self.store.head(&cspace(space), handle).await.is_ok()
    }

    async fn reference_exists(&self, space: &str, handle: &[u8; 16]) -> bool {
        ordo_exec::blob::lookup(&self.pool, &cspace(space), handle)
            .await
            .expect("lookup")
            .is_some()
    }

    async fn tombstones(&self, space: &str) -> usize {
        self.pool
            .rows(&format!(
                "SELECT count(*) FROM \"$blobtomb\" WHERE \"$space\" = '{space}'"
            ))
            .await
            .unwrap()[0][0]
            .as_deref()
            .and_then(|s| s.parse().ok())
            .expect("a count")
    }
}

/// **The collector.** A sweep of one space collects that space's
/// unreferenced object and leaves the twin's, though the two are the same
/// sixteen bytes of handle and the same bytes of object.
///
/// This is the one test in the file that fails in **both** directions, and
/// that is deliberate:
///
/// - An unscoped `live_reference_clause` sees the twin's live `docs` row
///   naming the same handle, concludes the object is still referenced, and
///   marks nothing, so a tenant's storage bill never stops growing
///   because a *different* tenant happens to hold a row.
/// - An unscoped object delete, or a store keyed by handle alone rather
///   than by `object_key(space, handle)`, destroys the twin's bytes while
///   a live row still names them, which is the failure `blobgc`'s whole
///   two-phase design exists to prevent.
#[tokio::test(flavor = "multi_thread")]
async fn a_sweep_collects_one_spaces_object() {
    let (b, handle) = blob_twins("ordo_spaces_blobgc").await;

    // The real space stops naming it. The twin still does, under the same
    // row id and the same handle.
    b.pool
        .rows(&format!(
            "DELETE FROM \"docs\" WHERE \"$space\" = '{REAL}' AND \"id\" = 'doc_1'"
        ))
        .await
        .expect("delete");

    let now = 1_000_000_000;
    let marked = b.sweep(REAL, now).await;
    assert_eq!(
        marked.marked, 1,
        "this space's handle is unreferenced HERE, whatever the twin's row says"
    );
    assert_eq!(b.tombstones(TWIN).await, 0, "and the twin was not marked");

    // Past the horizon, phase 2 runs.
    let swept = b.sweep(REAL, now + 2 * 86_400_000).await;
    assert_eq!(swept.deleted, 1, "the real space's object is collected");

    assert!(
        !b.object_exists(REAL, &handle).await,
        "the real space's bytes are gone"
    );
    assert!(
        !b.reference_exists(REAL, &handle).await,
        "and so is its reference"
    );
    assert!(
        b.object_exists(TWIN, &handle).await,
        "the twin's bytes survive a sweep it was not part of"
    );
    assert!(
        b.reference_exists(TWIN, &handle).await,
        "and so does its reference, under the very same handle"
    );
    assert_eq!(
        b.tombstones(TWIN).await,
        0,
        "the twin's handle was never even considered"
    );
}

/// **Erasure.** `erase_space` is the most destructive statement in the
/// collector: it deletes every object a space holds with no horizon and
/// no anti-join, so it is the one where a missing space predicate costs
/// the most and shows the least. the caller asked for total deletion and
/// got it, and nobody looks at the neighbour.
#[tokio::test(flavor = "multi_thread")]
async fn erasing_one_space_leaves_the_twins_bytes() {
    let (b, handle) = blob_twins("ordo_spaces_erase").await;

    let erased = blobgc::erase_space(&b.pool, &b.stores, &cspace(REAL))
        .await
        .expect("erase");
    assert_eq!(erased.deleted, 1, "one object in the erased space");

    assert!(!b.object_exists(REAL, &handle).await);
    assert!(!b.reference_exists(REAL, &handle).await);
    assert!(
        b.object_exists(TWIN, &handle).await,
        "erasing one tenant must not reach the bytes of another holding \
         the same handle"
    );
    assert!(b.reference_exists(TWIN, &handle).await);
    assert_eq!(
        blobgc::reference_count(&b.pool, &cspace(TWIN))
            .await
            .expect("count"),
        1,
        "the twin still holds exactly what it held"
    );
    assert_eq!(
        blobgc::reference_count(&b.pool, &cspace(REAL))
            .await
            .expect("count"),
        0
    );
}

// ── security-event routing ──────────────────────────────────────────────

/// **Routing.** A denial belongs to the space whose owner can act on it,
/// and to no other. `secevent.rs`'s
/// `a_drained_window_lands_as_one_entry_per_key_in_the_right_stream`
/// proves one space's denials and an unverified caller's separate; this is
/// the cross-space half, which is the one a single-space deployment cannot
/// distinguish from a stream keyed by nothing at all.
///
/// The second half is D1b's `about`. An unverified caller probing the twin
/// has no relationship with it, so the entry is the deployment's, and it
/// must still name **which** space was probed, or the operator surface can
/// report that somebody is probing without being able to say where. The
/// entry must carry `about = shadow` and must not appear in `shadow`'s own
/// stream, because a space's stream is what its owner reads and a stranger
/// rattling the door is not that owner's record to see.
#[tokio::test(flavor = "multi_thread")]
async fn a_denial_does_not_reach_the_twins_stream() {
    let db = "ordo_spaces_secevent";
    let _ = make_db(db);
    let pool = pool_for(db).await;
    for t in ["$secevents", "$seccheckpoints"] {
        pool.rows(&format!("DELETE FROM \"{t}\"")).await.unwrap();
    }

    let recorder = Recorder::default();
    let space = |s: &str| ordo_commandlog::SpaceId(s.to_string());

    // Ana is denied in each space, for a different reason in each. Same
    // principal id in both, because that is the case a stream keyed only
    // by subject cannot tell apart.
    recorder.deny(
        Some(&space(REAL)),
        Subject::member(ANA),
        "allTasks",
        RecordedDecision::OpNotInRole,
        &Stamp::new(Timestamp(1_000), CorrelationId("cor_real".into())),
        Relationship::Member,
    );
    recorder.deny(
        Some(&space(TWIN)),
        Subject::member(ANA),
        "boardList",
        RecordedDecision::PrincipalSuspended,
        &Stamp::new(Timestamp(1_001), CorrelationId("cor_twin".into())),
        Relationship::Member,
    );
    // A stranger rattling the twin's door. No relationship, so the record
    // is the deployment's, and it names the twin as its object.
    recorder.observe(
        Observation {
            space: Some(space(TWIN)),
            subject: Subject::Unverified,
            op: "boardList".to_string(),
            kind: EventKind::denied(RecordedDecision::TokenUnverifiable),
            at: Timestamp(1_002),
            correlation: CorrelationId("cor_probe".into()),
        },
        Relationship::None,
    );

    let entries = recorder.drain_all(Timestamp(9_000));
    let sec = PgSecurityStore::new(pool.clone(), tokio::runtime::Handle::current());
    sec.try_append_all(&entries)
        .await
        .expect("the stream takes them");

    let real_stream = sec
        .try_entries(&Scope::Space(space(REAL)), 0, 16)
        .await
        .expect("reads");
    let twin_stream = sec
        .try_entries(&Scope::Space(space(TWIN)), 0, 16)
        .await
        .expect("reads");
    let deployment = sec
        .try_entries(&Scope::Deployment, 0, 16)
        .await
        .expect("reads");

    assert_eq!(real_stream.len(), 1, "one denial happened in this space");
    assert_eq!(twin_stream.len(), 1, "and one in the twin, separately");
    assert_eq!(
        real_stream[0].entry.op, "allTasks",
        "each space's stream holds its OWN denial, not the other's"
    );
    assert_eq!(twin_stream[0].entry.op, "boardList");
    assert!(
        real_stream[0].entry.about.is_none() && twin_stream[0].entry.about.is_none(),
        "a space-scope entry already IS the space it names"
    );

    assert_eq!(deployment.len(), 1, "the stranger's probe, and only it");
    assert_eq!(
        deployment[0].entry.about,
        Some(ordo_commandlog::SpaceId(TWIN.into())),
        "a deployment entry must name which space was probed, or an operator \
         is told somebody is probing and not where"
    );
    assert_eq!(deployment[0].entry.subject, Subject::Unverified);

    // Both space streams and the deployment stream verify, each on its
    // own: three chains, no shared link.
    let sec2 = PgSecurityStore::new(pool, tokio::runtime::Handle::current());
    tokio::task::spawn_blocking(move || {
        use ordo_secevent::SecurityStore;
        for scope in [
            Scope::Space(ordo_commandlog::SpaceId(REAL.into())),
            Scope::Space(ordo_commandlog::SpaceId(TWIN.into())),
            Scope::Deployment,
        ] {
            sec2.verify(&scope)
                .unwrap_or_else(|e| panic!("{scope:?} must verify alone: {e:?}"));
        }
    })
    .await
    .expect("verification runs");
}
