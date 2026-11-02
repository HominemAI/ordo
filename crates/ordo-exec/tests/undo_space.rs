//! C1 (docs/plan-blob-chain.md §0.4 item 1, §6): the row lookups in
//! `ordo_exec::undo` that address a row by `"id"` alone, with no `"$space"`
//! conjunct. Entity tables are `PRIMARY KEY ("$space", "id")`, and ids are
//! minted from a process counter with no space salt, so two spaces in one
//! deployment can mint the same id, so an unscoped lookup then answers
//! with whichever space's row Postgres happens to hold, not the row the
//! revert is actually running against.
//!
//! Every test here seeds a second space (`shadow`) with a row that shares
//! an id with a row in the fixture's own space (`conformance`), through
//! direct SQL rather than the write path: the executor is bound to one
//! space (`ordo_conformance::fixture_space()`), so a foreign space's rows
//! can only land there this way. Each test is built so the two candidate
//! rows agree on whatever the surrounding checks compare (mostly `rev`),
//! so which physical row Postgres would return from an unscoped SELECT
//! never decides the outcome by itself; only the space predicate does.
//! `crates/ordo-conformance/tests/spaces.rs` states the same principle for
//! the write path: "ids are deliberately identical across the two spaces
//! … isolation must not rest on ids being lucky."
//!
//! Six statements in the module addressed a row by id alone: `row_image`,
//! `row_exists`, the referrer scan, the converge revert's own `UPDATE`,
//! the delete branch's own `DELETE`, and `lock_row`. Five of those get a
//! test here (the referrer scan and the `DELETE` share one, since one
//! scenario exercises both in sequence). `lock_row`'s test is bounded by
//! a timeout rather than by content, since a lock's effect is on
//! blocking, not on which row comes back; the bound is generous (2s)
//! against a local Postgres so it never mistakes a slow machine for the
//! bug. A seventh statement, `require_writable`'s bare-existence branch
//! for unguarded entities, was fixed but is not tested here: every one of
//! its four call sites in `apply` is reached only after a `row_image` (or
//! a fresh `INSERT`) has already proven the row exists in this space, so
//! once that upstream check is scoped, the downstream existence check can
//! never observe a difference between scoped and unscoped, whatever its
//! own SQL says. The fix stands as defense in depth against a caller that
//! does not have that upstream guarantee, present or future.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_commandlog::{CorrelationId, SpaceId, Timestamp};
use ordo_conformance::{Fixture, Pg, WriteLog};
use ordo_exec::{undo, Executor, LoggedExecutor, Pool, PoolConfig};
use ordo_schema::Schema;

// ── plumbing, the `sealed.rs` shape ─────────────────────────────────────

struct Env {
    pool: Pool,
    fixture: Arc<Fixture>,
    schema: Schema,
    log: Arc<Mutex<WriteLog>>,
}

/// A database of this test's own: the market fixture seeded into
/// `conformance`, the chain empty. `shadow` is not seeded by the fixture
/// at all: each test builds exactly the `shadow` rows it needs, by hand.
async fn env(db: &str) -> Env {
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    let pg = Pg {
        db: db.to_string(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 4,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$commandlog\"").await.unwrap();
    pool.rows("DELETE FROM \"$checkpoints\"").await.unwrap();
    let schema = f.schema.clone();
    Env {
        pool,
        fixture: Arc::new(f),
        schema,
        log: Arc::new(Mutex::new(WriteLog::new())),
    }
}

fn space() -> SpaceId {
    SpaceId("conformance".into())
}

/// No custody: this is the `ordo revert` shape, and none of these tests
/// touch a sealed field's plaintext, only its physical columns by raw SQL.
fn keyless() -> Option<&'static dyn undo::KeyProvider> {
    None
}

const SHADOW: &str = "shadow";

impl Env {
    fn executor(&self) -> LoggedExecutor {
        LoggedExecutor::new(
            Executor {
                pool: self.pool.clone(),
                schema: self.fixture.schema.clone(),
                registry: self.fixture.registry.clone(),
                keys: self.fixture.keys.clone(),
                space: ordo_conformance::fixture_space(),
            },
            "conformance",
        )
    }

    async fn write(&self, call: &str, caller: &str) {
        self.executor()
            .write_as(
                self.fixture.clone(),
                self.log.clone(),
                call.to_string(),
                caller.to_string(),
            )
            .await
            .unwrap_or_else(|e| panic!("write `{call}`: {e}"));
    }

    /// The group at a chain index, decoded.
    async fn group(&self, idx: usize) -> ordo_commandlog::CommandGroup {
        let rows = self
            .pool
            .rows(&format!(
                "SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = 'conformance' \
                 AND \"idx\" = {idx}"
            ))
            .await
            .unwrap();
        let hex = rows[0][0].clone().expect("a group at this index");
        let body = ordo_crypto::sealed::unhex(&hex).expect("a body");
        ordo_commandlog::chain::decode_group(&body).expect("decodes")
    }

    async fn correlation(&self, idx: usize) -> CorrelationId {
        CorrelationId(self.group(idx).await.correlation.0)
    }

    async fn one(&self, sql: &str) -> Option<String> {
        self.pool.rows(sql).await.unwrap().first()?.first()?.clone()
    }

    async fn exec(&self, sql: &str) {
        self.pool
            .rows(sql)
            .await
            .unwrap_or_else(|e| panic!("`{sql}`: {e}"));
    }

    /// `customers.email` and `customers.email$bidx`, as stored, hex-encoded.
    /// Copied verbatim into a `shadow` twin rather than reconstructed, so
    /// a test never has to compute the blind index (8l) itself, it only
    /// has to reuse bytes that are already known-valid.
    async fn sealed_email_cols(&self, id: &str) -> (String, String) {
        let email = self
            .one(&format!(
                "SELECT encode(coalesce(\"email\", '\\x'::bytea), 'hex') FROM \"customers\" \
                 WHERE \"$space\" = 'conformance' AND \"id\" = '{id}'"
            ))
            .await
            .expect("a row to copy from");
        let bidx = self
            .one(&format!(
                "SELECT encode(coalesce(\"email$bidx\", '\\x'::bytea), 'hex') FROM \"customers\" \
                 WHERE \"$space\" = 'conformance' AND \"id\" = '{id}'"
            ))
            .await
            .expect("a row to copy from");
        (email, bidx)
    }
}

// ── row_image (undo.rs `row_image`, reached through `require_row`) ─────

/// C1 site 1: the before-image a revert reads is this space's row, not
/// whichever row sharing its id Postgres happens to return.
///
/// The customer's `conformance` row is deleted by direct SQL (standing in
/// for whatever legitimately removed it, since a revert does not need to know
/// why a row is gone, only that it is), so the id exists ONLY in
/// `shadow` by the time the create is reverted. Unscoped, `row_image`
/// finds `shadow`'s row and the revert proceeds against it. Scoped, it
/// finds nothing in `conformance` and refuses `not_found`.
#[tokio::test(flavor = "multi_thread")]
async fn an_image_is_this_space_s_row() {
    let e = env("ordo_undo_space_image").await;
    e.write(
        r#"addCustomer($name: "ImgTest", $email: "img-test@example.com")"#,
        "mem_1",
    )
    .await;
    let row_id = e.group(0).await.commands[0].row_id.clone();
    let correlation = e.correlation(0).await;
    let (email, bidx) = e.sealed_email_cols(&row_id).await;

    e.exec(&format!(
        "DELETE FROM \"customers\" WHERE \"$space\" = 'conformance' AND \"id\" = '{row_id}'"
    ))
    .await;
    e.exec(&format!(
        "INSERT INTO \"customers\" (\"$space\", \"id\", \"name\", \"email\", \"email$bidx\", \"tier\") \
         VALUES ('{SHADOW}', '{row_id}', 'shadow-twin', decode('{email}', 'hex'), \
         decode('{bidx}', 'hex'), 'basic')"
    ))
    .await;

    // Real custody: `customers.email` is sealed, and a keyless caller is
    // refused the whole class before any site-specific check runs. Real
    // keys let the refusal under test (or its absence) actually surface.
    let err = undo::revert(
        &e.pool,
        &e.schema,
        Some(e.fixture.keys.as_ref()),
        None,
        None,
        &space(),
        &correlation,
        "mem_1",
        Timestamp(1),
    )
    .await
    .expect_err("the row is not in this space, no matter what shares its id elsewhere");
    let msg = err.to_string();
    assert!(
        msg.starts_with("not_found"),
        "row_image must not answer with another space's row: {msg}"
    );
}

// ── row_exists (undo.rs `row_exists`, the create branch) ───────────────

/// C1 site 2: the id `row_exists` checks for must be free **in this
/// space**, not free everywhere.
///
/// The customer is created, then its create is reverted: a genuine,
/// same-space delete, with no `shadow` row anywhere yet. Only after that
/// does `shadow` get a row under the freed id, and only then is the
/// delete reverted a second time, landing on the branch that re-creates
/// the row. Unscoped, `row_exists` sees `shadow`'s row and refuses
/// `expected_absent`. Scoped, the id is free in `conformance` and the row
/// comes back.
#[tokio::test(flavor = "multi_thread")]
async fn an_absent_id_is_this_space_s_absence() {
    let e = env("ordo_undo_space_exists").await;
    e.write(
        r#"addCustomer($name: "ExistsTest", $email: "exists-test@example.com")"#,
        "mem_1",
    )
    .await;
    let row_id = e.group(0).await.commands[0].row_id.clone();
    let create_correlation = e.correlation(0).await;
    let (email, bidx) = e.sealed_email_cols(&row_id).await;

    // Real custody: `customers.email` is sealed, and a keyless caller is
    // refused the whole class before any site-specific check runs.
    let reverted_create = undo::revert(
        &e.pool,
        &e.schema,
        Some(e.fixture.keys.as_ref()),
        None,
        None,
        &space(),
        &create_correlation,
        "mem_1",
        Timestamp(1),
    )
    .await
    .expect("the create reverts cleanly: nothing shadow-adjacent yet");
    let delete_correlation = reverted_create.group.correlation.clone();

    // Only now does the id become taken, in `shadow`, not here.
    e.exec(&format!(
        "INSERT INTO \"customers\" (\"$space\", \"id\", \"name\", \"email\", \"email$bidx\", \"tier\") \
         VALUES ('{SHADOW}', '{row_id}', 'shadow-twin', decode('{email}', 'hex'), \
         decode('{bidx}', 'hex'), 'basic')"
    ))
    .await;

    undo::revert(
        &e.pool,
        &e.schema,
        Some(e.fixture.keys.as_ref()),
        None,
        None,
        &space(),
        &delete_correlation,
        "mem_1",
        Timestamp(2),
    )
    .await
    .expect("the id is free in this space, no matter who holds it in another");

    let back = e
        .one(&format!(
            "SELECT \"name\" FROM \"customers\" WHERE \"$space\" = 'conformance' AND \"id\" = '{row_id}'"
        ))
        .await;
    assert_eq!(
        back.as_deref(),
        Some("ExistsTest"),
        "the row came back in this space"
    );
}

// ── refuse_with_referrers and the delete branch's DELETE ────────────────

/// C1 sites 3 and 6: a referrer blocks a delete only when it refers to
/// THIS space's row, and the delete itself removes only THIS space's row.
///
/// Reverting the create of a customer deletes it: `conformance` has no
/// referrer to it, so the delete branch's own referrer scan has nothing
/// to find there. `shadow` holds a row under the same id AND an order
/// that references it. Unscoped, the scan finds `shadow`'s order and
/// refuses `has_referrers`, a row in a space that is not even part of
/// this revert blocking a delete it has no say over. Scoped, the scan
/// never looks outside `conformance`, and that refusal does not fire, so
/// the revert proceeds to the DELETE.
///
/// The DELETE the delete branch issues after the referrer scan addresses
/// a row by `"id"` alone too, so this test also pins that statement: the
/// revert must remove `conformance`'s customer and leave `shadow`'s
/// untouched, referrer and all.
#[tokio::test(flavor = "multi_thread")]
async fn a_referrer_scan_is_this_space_s_referrers() {
    let e = env("ordo_undo_space_referrers").await;
    e.write(
        r#"addCustomer($name: "RefTest", $email: "ref-test@example.com")"#,
        "mem_1",
    )
    .await;
    let row_id = e.group(0).await.commands[0].row_id.clone();
    let correlation = e.correlation(0).await;
    let (email, bidx) = e.sealed_email_cols(&row_id).await;

    // `shadow`'s twin exists only so the order below has a legal foreign
    // key to point at. Its `rev` matches `conformance`'s row (both are
    // freshly created, so both are at 1) so that whichever of the two an
    // unscoped `row_image` happens to return, `same_revision` passes
    // either way, so the referrer scan is the only thing under test here.
    e.exec(&format!(
        "INSERT INTO \"customers\" (\"$space\", \"id\", \"name\", \"email\", \"email$bidx\", \"tier\") \
         VALUES ('{SHADOW}', '{row_id}', 'shadow-twin', decode('{email}', 'hex'), \
         decode('{bidx}', 'hex'), 'basic')"
    ))
    .await;
    e.exec(&format!(
        "INSERT INTO \"orders\" (\"$space\", \"id\", \"customer\", \"placed\", \"status\", \"total\", \"memo\") \
         VALUES ('{SHADOW}', 'ord_shadow_ref', '{row_id}', DATE '2026-01-01', 'open', 100, '\\x'::bytea)"
    ))
    .await;

    // Real custody: `customers.email` is sealed, and a keyless caller is
    // refused the whole class before the referrer scan ever runs.
    undo::revert(
        &e.pool,
        &e.schema,
        Some(e.fixture.keys.as_ref()),
        None,
        None,
        &space(),
        &correlation,
        "mem_1",
        Timestamp(1),
    )
    .await
    .expect("another space's referrer must not block this space's delete");

    let gone = e
        .one(&format!(
            "SELECT 1 FROM \"customers\" WHERE \"$space\" = 'conformance' AND \"id\" = '{row_id}'"
        ))
        .await;
    assert_eq!(gone, None, "conformance's own row was deleted");

    let twin = e
        .one(&format!(
            "SELECT \"name\" FROM \"customers\" WHERE \"$space\" = '{SHADOW}' AND \"id\" = '{row_id}'"
        ))
        .await;
    assert_eq!(
        twin.as_deref(),
        Some("shadow-twin"),
        "the DELETE must not remove another space's row sharing this id"
    );
    let referrer = e
        .one(&format!(
            "SELECT 1 FROM \"orders\" WHERE \"$space\" = '{SHADOW}' AND \"id\" = 'ord_shadow_ref'"
        ))
        .await;
    assert_eq!(
        referrer,
        Some("1".to_string()),
        "shadow's referrer survives too, undisturbed"
    );
}

// ── the converge revert's own UPDATE ─────────────────────────────────────

/// C1 site 4: the row a converge-revert writes back is this space's row.
///
/// A card in `conformance` is finished, then that write is reverted.
/// `shadow` holds a row under the SAME id the whole time, at the `rev`
/// (2) the revert expects `conformance`'s row to still be at, so which
/// of the two rows an unscoped `SELECT` happens to return never decides
/// whether the checks upstream of the `UPDATE` pass; only the `UPDATE`'s
/// own predicate is under test here. Unscoped, `UPDATE … WHERE "id" = …`
/// has no space in its predicate and matches every row with that id,
/// both spaces' at once, so `shadow`'s row (title, `done`, and `rev`
/// included) comes back different. Scoped, `shadow`'s row is untouched,
/// byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_updates_this_space_s_row() {
    let e = env("ordo_undo_space_update").await;
    e.write(r#"addBoard($name: "b1")"#, "mem_1").await;
    let board_id = e.group(0).await.commands[0].row_id.clone();
    e.write(
        &format!(r#"addCard($board: "{board_id}", $title: "c1")"#),
        "mem_1",
    )
    .await;
    let card_id = e.group(1).await.commands[0].row_id.clone();
    e.write(&format!(r#"finishCard($card: "{card_id}")"#), "mem_1")
        .await;
    let correlation = e.correlation(2).await;

    e.exec(&format!(
        "INSERT INTO \"boards\" (\"$space\", \"id\", \"name\") VALUES \
         ('{SHADOW}', 'board_shadow_upd', 'shadow-board')"
    ))
    .await;
    e.exec(&format!(
        "INSERT INTO \"cards\" (\"$space\", \"id\", \"board\", \"title\", \"done\", \"rev\") \
         VALUES ('{SHADOW}', '{card_id}', 'board_shadow_upd', 'shadow-title', false, 2)"
    ))
    .await;

    undo::revert(
        &e.pool,
        &e.schema,
        keyless(),
        None,
        None,
        &space(),
        &correlation,
        "mem_1",
        Timestamp(1),
    )
    .await
    .expect("mem_1 is admin on the board she just created (G10a bootstrap), so this reverts");

    let title = e
        .one(&format!(
            "SELECT \"title\" FROM \"cards\" WHERE \"$space\" = '{SHADOW}' AND \"id\" = '{card_id}'"
        ))
        .await;
    let done = e
        .one(&format!(
            "SELECT \"done\" FROM \"cards\" WHERE \"$space\" = '{SHADOW}' AND \"id\" = '{card_id}'"
        ))
        .await;
    let rev = e
        .one(&format!(
            "SELECT \"rev\" FROM \"cards\" WHERE \"$space\" = '{SHADOW}' AND \"id\" = '{card_id}'"
        ))
        .await;
    assert_eq!(
        title.as_deref(),
        Some("shadow-title"),
        "another space's row moved"
    );
    assert_eq!(done.as_deref(), Some("f"), "another space's row moved");
    assert_eq!(
        rev.as_deref(),
        Some("2"),
        "another space's row moved, `rev` included"
    );
}

// ── lock_row ─────────────────────────────────────────────────────────────

/// C1 site 5: `lock_row` locks this space's row, not whichever row
/// sharing its id Postgres happens to include in an unscoped match.
///
/// A lock's effect is not on which row a `SELECT` returns, it is on
/// whether a *different* transaction has to wait. So this test holds an
/// explicit `FOR UPDATE` lock on `shadow`'s twin of the card open, from a
/// second connection, and then reverts `conformance`'s card under a
/// bounded timeout. Unscoped, `lock_row`'s `SELECT … WHERE "id" = … FOR
/// UPDATE` matches both spaces' rows and blocks on the one the second
/// connection is holding, so the revert cannot even start its own work
/// and the timeout elapses. Scoped, `lock_row` only ever asks Postgres to
/// lock `conformance`'s row, which nothing else is holding, so the
/// revert proceeds immediately.
///
/// This is a bounded-wait check, not a race: which of two rows an
/// unscoped `SELECT` would return is never in question here (only one
/// row, `shadow`'s, is deliberately locked), and the 2s bound is
/// generous next to the milliseconds an unblocked revert actually takes
/// against a local Postgres, so a slow machine reads as "not blocked"
/// long before it would ever read as "blocked".
#[tokio::test(flavor = "multi_thread")]
async fn a_lock_holds_this_space_s_row() {
    let e = env("ordo_undo_space_lock").await;
    e.write(r#"addBoard($name: "b1")"#, "mem_1").await;
    let board_id = e.group(0).await.commands[0].row_id.clone();
    e.write(
        &format!(r#"addCard($board: "{board_id}", $title: "c1")"#),
        "mem_1",
    )
    .await;
    let card_id = e.group(1).await.commands[0].row_id.clone();
    e.write(&format!(r#"finishCard($card: "{card_id}")"#), "mem_1")
        .await;
    let correlation = e.correlation(2).await;

    e.exec(&format!(
        "INSERT INTO \"boards\" (\"$space\", \"id\", \"name\") VALUES \
         ('{SHADOW}', 'board_shadow_lock', 'shadow-board')"
    ))
    .await;
    e.exec(&format!(
        "INSERT INTO \"cards\" (\"$space\", \"id\", \"board\", \"title\", \"done\", \"rev\") \
         VALUES ('{SHADOW}', '{card_id}', 'board_shadow_lock', 'shadow-title', false, 2)"
    ))
    .await;

    // A second connection locks ONLY `shadow`'s row, and holds the lock
    // open across the revert attempt below.
    let holder = e.pool.begin().await.expect("a second connection");
    holder
        .rows(&format!(
            "SELECT \"id\" FROM \"cards\" WHERE \"$space\" = '{SHADOW}' AND \"id\" = '{card_id}' \
             FOR UPDATE"
        ))
        .await
        .expect("the holder locks shadow's row");

    // A lock wait never ends while the holder is open, so any bound tells
    // the two apart. 2 s was too tight on a loaded runner (run 41366725869).
    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        undo::revert(
            &e.pool,
            &e.schema,
            keyless(),
            None,
            None,
            &space(),
            &correlation,
            "mem_1",
            Timestamp(1),
        ),
    )
    .await;

    // Release the holder regardless of outcome: a timed-out revert future
    // is dropped by `timeout`, which aborts its own transaction, but the
    // holder is this test's own connection and stays open until told
    // otherwise.
    holder.rollback().await.expect("release shadow's lock");

    outcome
        .expect("lock_row must not wait on a lock held only on another space's row sharing this id")
        .expect("mem_1 is admin on the board she just created (G10a bootstrap), so this reverts");
}
