//! **A tombstone must be priced against the deletion it belongs to.**
//!
//! Found on 2026-09-22 while building U2's upload intents, in code that
//! has nothing to do with intents: the same stale-clock defect exists
//! today for an ordinary delete, revert, delete again.
//!
//! # The sequence, and why every point that looks sees nothing wrong
//!
//! 1. A row holding a blob is deleted. Phase 1 tombstones the handle and
//!    stamps it `t`.
//! 2. The deletion is reverted inside the undo horizon, so a live row
//!    names the handle again. The tombstone stays **open**, because phase
//!    2 only examines tombstones that are **due** and this one is not.
//! 3. Much later the row is deleted again. Phase 1's `INSERT` skips the
//!    handle, because a tombstone already exists for it.
//! 4. The sweep runs. It finds a tombstone stamped `t`, long past the
//!    cutoff, and deletes the object. The second deletion is days old and
//!    revertible for the rest of the horizon.
//!
//! Phase 2's re-check does not save it. That runs only once a tombstone is
//! due, and by then the handle is genuinely unreferenced again, so the
//! re-check agrees with the deletion. The defect is entirely in the
//! timestamp, and no single step looks wrong on its own.
//!
//! # Why this file exists rather than a case in `blobgc.rs`
//!
//! It needs no pipeline, no store contents and no upload: the bug is
//! tombstone bookkeeping, so it is driven by writing a `$blobref` row and
//! moving a column. Keeping it separate also keeps it readable as the one
//! scenario it is.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::sync::Arc;
use std::time::Duration;

use ordo_blob::{FsStore, SingleStore};
use ordo_conformance::{Fixture, Pg};
use ordo_crypto::custody::SpaceId as CryptoSpace;
use ordo_exec::cadence::LogPolicy;
use ordo_exec::{Pool, PoolConfig};
use ordo_schema::Schema;
use ordo_syntax::parse_program;

const SPACE: &str = "conformance";

const SRC: &str = r#"
entity doc {
  title: text
  file:  blob?
}
"#;

const DAY_MS: u64 = 86_400_000;

struct Env {
    pool: Pool,
    schema: Schema,
    stores: SingleStore,
}

/// Each test gets its own database and its own store directory. Two
/// tests sharing either one interfere under `--test-threads` and produce
/// failures that read like the defect under test.
async fn env(db: &str) -> Env {
    let pg = Pg::named(db);
    Fixture::load()
        .expect("fixture")
        .reset(&pg)
        .expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");

    let schema = ordo_schema::build(&parse_program(SRC).expect("parses")).expect("builds");
    pool.rows("DROP TABLE IF EXISTS \"doc\"").await.unwrap();
    for stmt in ordo_schema::ddl_for(&schema)
        .expect("ddl")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains("\"doc\""))
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

    let dir = std::env::temp_dir().join(format!("ordo-stale-clock-{db}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = FsStore::shared(&dir).expect("store");
    Env {
        pool,
        schema,
        stores: SingleStore::new(Arc::clone(&store)),
    }
}

/// A `$blobref` row and a `doc` row whose column names it: the state a
/// committed upload leaves.
async fn upload(env: &Env, handle: &ordo_crypto::custody::BlobHandle, at_ms: u64) {
    let hex = hex_of(handle);
    let tx = env.pool.begin().await.expect("tx");
    tx.rows(&ordo_crypto::blobref::insert_sql(
        &CryptoSpace(SPACE.to_string()),
        handle,
        1,
        "doc",
        "file",
        b"mime",
        b"size",
        b"integrity",
        4,
        at_ms as i64,
        "",
    ))
    .await
    .expect("the reference row");
    tx.rows(&format!(
        "INSERT INTO \"doc\" (\"$space\", \"id\", \"rev\", \"title\", \"file\") \
         VALUES ('{SPACE}', 'doc_1', 1, 'a', '{hex}')"
    ))
    .await
    .expect("the entity row");
    tx.commit().await.expect("commit");
}

fn hex_of(h: &ordo_crypto::custody::BlobHandle) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// Point the column away from the handle, which is what a delete leaves
/// behind as far as the sweep can see.
async fn unreference(env: &Env) {
    env.pool
        .rows(&format!(
            "UPDATE \"doc\" SET \"file\" = NULL WHERE \"$space\" = '{SPACE}' AND \"id\" = 'doc_1'"
        ))
        .await
        .expect("the delete");
}

/// Point it back, which is what a revert inside the horizon leaves.
async fn reference_again(env: &Env, handle: &ordo_crypto::custody::BlobHandle) {
    let hex = hex_of(handle);
    env.pool
        .rows(&format!(
            "UPDATE \"doc\" SET \"file\" = '{hex}' WHERE \"$space\" = '{SPACE}' \
             AND \"id\" = 'doc_1'"
        ))
        .await
        .expect("the revert");
}

async fn sweep_at(env: &Env, now_ms: u64) -> ordo_exec::blobgc::SweepReport {
    ordo_exec::blobgc::sweep(
        &env.pool,
        &env.stores,
        &env.schema,
        &CryptoSpace(SPACE.to_string()),
        &LogPolicy::default(),
        now_ms,
    )
    .await
    .expect("the sweep runs")
}

/// **A deletion that follows a revert is priced against itself, not
/// against the deletion before it.**
///
/// Without the eager cancel in phase 1 this reports `deleted: 1`: the
/// sweep collects an object whose second deletion happened twenty days
/// ago and stays revertible for seventy more.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_then_a_second_delete_is_floored_from_the_second_delete() {
    let env = env("ordo_blobgc_stale_revert").await;
    let handle: ordo_crypto::custody::BlobHandle = [0xaa; 16];
    let t0 = 1_000 * DAY_MS;

    upload(&env, &handle, t0).await;

    // Delete, and let phase 1 stamp the handle at t0.
    unreference(&env).await;
    let first = sweep_at(&env, t0).await;
    assert_eq!(
        first.marked, 1,
        "the first delete must tombstone the handle"
    );
    assert_eq!(first.deleted, 0, "and must not collect it yet");

    // Revert inside the horizon. The handle is referenced again.
    reference_again(&env, &handle).await;
    let after_revert = sweep_at(&env, t0 + 10 * DAY_MS).await;
    assert_eq!(
        after_revert.deleted, 0,
        "a reverted handle must not be collected"
    );

    // Delete again, much later. This is the deletion whose clock matters.
    unreference(&env).await;
    let second = sweep_at(&env, t0 + 20 * DAY_MS).await;
    assert_eq!(
        second.marked, 1,
        "the second delete must tombstone the handle afresh, or the sweep \
         prices it against the first delete's clock"
    );

    // Now run the sweep at a moment that is past the horizon measured from
    // the FIRST delete and well inside it measured from the second.
    let out = sweep_at(&env, t0 + 91 * DAY_MS).await;
    assert_eq!(
        out.deleted, 0,
        "the second deletion is twenty days old and revertible for seventy \
         more. A 1 here means the sweep priced it against the first \
         deletion, which is the stale-clock defect this file is named for"
    );
}

/// **The cancel is not a licence to keep a handle forever.** Once the
/// handle really is unreferenced and the floor has passed, it goes.
///
/// Without this, the fix above could be "never collect anything that was
/// ever reverted", which would trade a data-loss bug for an unbounded
/// leak.
#[tokio::test(flavor = "multi_thread")]
async fn a_handle_that_stays_deleted_is_still_collected() {
    let env = env("ordo_blobgc_stale_kept").await;
    let handle: ordo_crypto::custody::BlobHandle = [0xbb; 16];
    let t0 = 2_000 * DAY_MS;

    upload(&env, &handle, t0).await;
    unreference(&env).await;
    sweep_at(&env, t0).await;

    let out = sweep_at(&env, t0 + 400 * DAY_MS).await;
    assert_eq!(
        out.deleted, 1,
        "a handle nothing names, long past the floor, must still be collected"
    );
}
