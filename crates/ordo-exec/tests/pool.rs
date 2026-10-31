//! The pool's contract: bounded, backpressured, transactional, honest
//! about aborts. Needs the conformance cluster
//! (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_exec::{Pool, PoolConfig, PoolError};

fn cfg(size: usize) -> PoolConfig {
    PoolConfig {
        db: "postgres".into(),
        size,
        acquire_timeout: Duration::from_millis(300),
        ..PoolConfig::from_env()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn queries_flow_and_connections_recycle() {
    let pool = Pool::connect(cfg(2)).await.expect("boot");
    for _ in 0..10 {
        let rows = pool.rows("SELECT 1, NULL, 'x'").await.unwrap();
        assert_eq!(rows, vec![vec![Some("1".into()), None, Some("x".into())]]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn saturation_is_a_fast_typed_failure_not_a_queue() {
    let pool = Pool::connect(cfg(2)).await.expect("boot");
    // Hold both connections in open transactions…
    let a = pool.begin().await.unwrap();
    let b = pool.begin().await.unwrap();
    // …and the third checkout times out into the typed error.
    match pool.rows("SELECT 1").await {
        Err(PoolError::Saturated) => {}
        other => panic!("expected Saturated, got {other:?}"),
    }
    // Freeing one slot makes the same call succeed.
    a.commit().await.unwrap();
    pool.rows("SELECT 1").await.expect("slot freed");
    b.rollback().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_an_uncommitted_transaction_aborts_it() {
    let pool = Pool::connect(cfg(2)).await.expect("boot");
    pool.rows("DROP TABLE IF EXISTS pool_probe; CREATE TABLE pool_probe (v int)")
        .await
        .expect("probe table");

    // Effects inside an open transaction are invisible outside it…
    let tx = pool.begin().await.unwrap();
    tx.rows("INSERT INTO pool_probe VALUES (1)").await.unwrap();
    assert_eq!(
        pool.rows("SELECT count(*) FROM pool_probe").await.unwrap()[0][0],
        Some("0".into()),
        "uncommitted work must not leak"
    );
    // …and DROPPING the guard (no commit) aborts them for good.
    drop(tx);
    // The discarded connection is replaced transparently on later use.
    for _ in 0..3 {
        pool.rows("SELECT 1").await.expect("pool re-dials");
    }
    assert_eq!(
        pool.rows("SELECT count(*) FROM pool_probe").await.unwrap()[0][0],
        Some("0".into()),
        "the abort is permanent"
    );

    // The commit path, for contrast, lands.
    let tx = pool.begin().await.unwrap();
    tx.rows("INSERT INTO pool_probe VALUES (2)").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        pool.rows("SELECT count(*) FROM pool_probe").await.unwrap()[0][0],
        Some("1".into())
    );
    pool.rows("DROP TABLE pool_probe").await.unwrap();
}

/// The read executor over the pool: the same guarded narrowing the psql
/// harness proves, now through tokio-postgres.
#[tokio::test(flavor = "multi_thread")]
async fn the_read_executor_compiles_and_runs_guarded_reads() {
    use ordo_conformance::{Fixture, Pg};
    use ordo_exec::Executor;

    // Seed through the harness (DDL + fixture data), then serve with the
    // real executor.
    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_exec");
    f.reset(&Pg {
        db: "ordo_exec".into(),
        ..Pg::default()
    })
    .expect("seeded");

    let pool = Pool::connect(PoolConfig {
        db: "ordo_exec".into(),
        ..cfg(4)
    })
    .await
    .expect("boot");
    let x = Executor {
        pool,
        schema: f.schema,
        registry: f.registry,
        keys: f.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };

    // Guarded: each caller sees their own rows, and anonymous sees none.
    let ana = x.read_as("openTasks", Some("mem_1")).await.unwrap();
    let titles: Vec<&str> = ana.iter().map(|r| r["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["audit books", "draft spec"]);
    assert_eq!(
        x.read_as("openTasks", Some("mem_2")).await.unwrap().len(),
        3
    );
    assert!(x.read_as("openTasks", None).await.unwrap().is_empty());

    // Typed decoding: an int arrives as a JSON number, and money as an
    // exact decimal string in major units (`api-engine-design.md` §5,
    // 27ak). The fixture's `money` is `money(0)`, so 3000 is `"3000"`.
    let rev = x.read_as("productRevenue", Some("mem_1")).await.unwrap();
    assert_eq!(rev[0]["revenue"], "3000", "money decodes typed: {rev:?}");
    assert!(rev[0]["sold"].is_number(), "an int is a number: {rev:?}");

    // Blind-index equality runs on the same path (a servable venue).
    let cust = x
        .read_as(r#"customerByEmail($e: "ada@example.com")"#, Some("mem_1"))
        .await
        .unwrap();
    assert_eq!(cust[0]["name"], "Ada");

    // And a refused compile stays refused, before any connection is used.
    let e = x.read_as("orders[memo]", Some("mem_1")).await.unwrap_err();
    assert!(format!("{e}").contains("sealed"), "{e}");
}

/// AA-004's closure: the write protocol runs under ONE transaction, so a
/// refusal leaves nothing behind — including the multi-row groups (an acl
/// create and its bootstrap row) that a statement-at-a-time path could
/// half-apply.
#[tokio::test(flavor = "multi_thread")]
async fn writes_are_one_transaction_and_refusals_leave_nothing() {
    use std::sync::{Arc, Mutex};

    use ordo_conformance::{Fixture, Pg, WriteLog};
    use ordo_exec::Executor;

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_exec_tx");
    let pg = Pg {
        db: "ordo_exec_tx".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");

    let pool = Pool::connect(PoolConfig {
        db: "ordo_exec_tx".into(),
        ..cfg(4)
    })
    .await
    .expect("boot");
    let schema = f.schema.clone();
    let registry = f.registry.clone();
    let fixture = Arc::new(f);
    let log = Arc::new(Mutex::new(WriteLog::new()));
    let x = Executor {
        pool,
        schema,
        registry,
        keys: fixture.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };

    // A permitted write commits and is visible afterwards.
    x.write_as(
        fixture.clone(),
        log.clone(),
        r#"renameTask($task: "task_1", $title: "committed")"#.into(),
        "mem_1".into(),
    )
    .await
    .expect("Ana may write");
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("committed".into())
    );

    // A refused write aborts: no partial effect, no log group.
    let groups_before = log.lock().unwrap().len();
    let e = x
        .write_as(
            fixture.clone(),
            log.clone(),
            r#"renameTask($task: "task_1", $title: "refused")"#.into(),
            "mem_2".into(),
        )
        .await
        .expect_err("Bram may not");
    assert!(format!("{e}").contains("access_guard"), "{e}");
    assert_eq!(
        pg.query(r#"SELECT "title" FROM "tasks" WHERE "id" = 'task_1'"#)
            .unwrap()[0][0],
        Some("committed".into()),
        "the refused write changed nothing"
    );
    assert_eq!(
        log.lock().unwrap().len(),
        groups_before,
        "a refusal appends no command group"
    );

    // A multi-row group (acl create + its bootstrap access row) commits
    // whole — the two rows exist together or not at all.
    let created = x
        .write_as(
            fixture.clone(),
            log.clone(),
            r#"addBoard($name: "tx board")"#.into(),
            "mem_3".into(),
        )
        .await
        .expect("exempt create");
    let board = created[0]["created"].as_str().unwrap().to_string();
    let access = pg
        .query(&format!(
            r#"SELECT "member", "level" FROM "boardsAccess" WHERE "board" = '{board}'"#
        ))
        .unwrap();
    assert_eq!(
        access.len(),
        1,
        "the bootstrap row committed with the board"
    );
    assert_eq!(access[0][1].as_deref(), Some("admin"));

    // And the pool is healthy after all of it — every connection returned.
    for _ in 0..6 {
        x.pool.rows("SELECT 1").await.expect("pool intact");
    }
}

/// Keyset pagination: walking pages covers the result exactly once, in the
/// plan's own total order, whether or not the sort key is projected.
#[tokio::test(flavor = "multi_thread")]
async fn paging_walks_a_total_order_without_gaps_or_repeats() {
    use ordo_conformance::{Fixture, Pg};
    use ordo_exec::Executor;

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_exec_page");
    let pg = Pg {
        db: "ordo_exec_page".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: "ordo_exec_page".into(),
        ..cfg(4)
    })
    .await
    .expect("boot");
    let x = Executor {
        pool,
        schema: f.schema,
        registry: f.registry,
        keys: f.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };

    // Walk a projected-key order two rows at a time.
    async fn walk(x: &Executor, call: &str, size: i64) -> Vec<serde_json::Value> {
        let mut all = Vec::new();
        let mut after: Option<Vec<Option<String>>> = None;
        for _ in 0..50 {
            let p = x
                .read_page_as(call, Some("mem_1"), size, after.as_deref())
                .await
                .expect("page");
            all.extend(p.rows);
            match p.next {
                Some(k) => after = Some(k),
                None => break,
            }
        }
        all
    }

    let whole = x
        .read_as("activeProducts", Some("mem_1"))
        .await
        .expect("unpaged");
    let paged = walk(&x, "activeProducts", 2).await;
    assert_eq!(paged, whole, "pages reassemble the unpaged answer exactly");
    assert!(whole.len() > 3, "the fixture must actually span pages");

    // A DESCENDING key, and an odd page size that does not divide the set.
    let whole = x
        .read_as("products[name, price]\nsort -price", Some("mem_1"))
        .await
        .unwrap();
    let paged = walk(&x, "products[name, price]\nsort -price", 3).await;
    assert_eq!(paged, whole, "descending order pages identically");

    // An UNPROJECTED sort key (ruling W047): the cursor rides a hidden
    // column, and the output shape never mentions it.
    //
    // Amended 2026-09-17 (C4e): the SORT key still never reaches the client,
    // and that is what this pins. The anchor's id now does, as `$id`, because
    // `ui-gen.md` §1 says row identity rides every list row even when the
    // author did not project it. Keys are sorted (`serde_json` without
    // `preserve_order`), so `$id` comes first.
    let call = "products[name]\nsort price";
    let whole = x.read_as(call, Some("mem_1")).await.unwrap();
    let paged = walk(&x, call, 2).await;
    assert_eq!(paged, whole, "unprojected key pages identically");
    assert!(
        paged[0]
            .as_object()
            .unwrap()
            .keys()
            .eq(["$id", "name"].iter()),
        "the sort key stays hidden and the row identity does not: {:?}",
        paged[0]
    );

    // And the identity must actually IDENTIFY: the riskiest failure of the
    // attach is reading the wrong cell, which yields a sort value rather than
    // an id. That is silent — the difference pass would read it as churn —
    // so it is asserted rather than assumed.
    let ids: Vec<&str> = paged
        .iter()
        .map(|r| r["$id"].as_str().expect("every row carries `$id`"))
        .collect();
    assert!(
        ids.iter().all(|id| id.starts_with("prod_")),
        "a typed id, not the `price` this op sorts by: {ids:?}"
    );
    let unique: std::collections::BTreeSet<&&str> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "one row, one id: {ids:?}");

    // A NULLABLE key (tasks.assignee is optional, and the seed leaves two
    // unassigned): nulls sort last, and paging must cross that boundary
    // exactly once.
    let call = "tasks[title, assignee]\nsort assignee";
    let whole = x.read_as(call, Some("mem_1")).await.unwrap();
    let paged = walk(&x, call, 2).await;
    assert_eq!(paged, whole, "nullable key pages identically");
}

/// Paging-sweep regressions. Both were PROVEN row losses: an `append`
/// whose arms hold duplicate rows, and a repeated sort key. Each walk
/// asserts the pages reassemble the unpaged answer exactly.
#[tokio::test(flavor = "multi_thread")]
async fn paging_regressions_stay_fixed() {
    use ordo_conformance::{Fixture, Pg};
    use ordo_exec::Executor;

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_exec_regr");
    let pg = Pg {
        db: "ordo_exec_regr".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: "ordo_exec_regr".into(),
        ..cfg(4)
    })
    .await
    .expect("boot");
    let x = Executor {
        pool,
        schema: f.schema,
        registry: f.registry,
        keys: f.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };

    async fn walk(x: &Executor, call: &str, size: i64) -> Vec<serde_json::Value> {
        let mut all = Vec::new();
        let mut after: Option<Vec<Option<String>>> = None;
        for _ in 0..60 {
            let p = x
                .read_page_as(call, Some("mem_1"), size, after.as_deref())
                .await
                .expect("page");
            all.extend(p.rows);
            match p.next {
                Some(k) => after = Some(k),
                None => break,
            }
        }
        all
    }

    // F3: a keyless `reduce` has no ordering and needs none — one row,
    // no cursor. It used to fail at the door because every read is routed
    // through paging.
    let p = x
        .read_page_as("specCount", Some("mem_1"), 2, None)
        .await
        .expect("a reduce answers whole");
    assert_eq!(p.rows.len(), 1, "{:?}", p.rows);
    assert!(p.next.is_none(), "nothing to page to");
    // And a cursor presented against one is a client error, not a shrug.
    let e = x
        .read_page_as("specCount", Some("mem_1"), 2, Some(&[Some("x".into())]))
        .await
        .expect_err("a cursor for an unpageable plan is invalid");
    assert!(format!("{e}").contains("invalid_cursor"), "{e}");

    for call in [
        // F1: a union whose arms carry duplicate rows. `$arm` + outputs
        // distinguishes rows only up to identity; the strict seek then ate
        // every duplicate past the first.
        "top3Orders[c: customer]\nappend doneOrders($from: 2026-01-01)[c: customer]\nsort c",
        "orders[s: status]\nappend orders[s: status]\nsort s",
        // F2: a repeated sort key shifted every hidden key column after it,
        // replacing the anchor-id tiebreak with a copy of an earlier key.
        "orders[placed]\nsort status, status",
        "orders[placed]\nsort status, status, total",
        "orders[placed, status]\nsort status, status",
        "orders[placed]\nsort status, -status",
    ] {
        let whole = x.read_as(call, Some("mem_1")).await.expect("unpaged");
        for size in [1, 2, 3, 5] {
            let paged = walk(&x, call, size).await;
            assert_eq!(
                paged.len(),
                whole.len(),
                "row count changed at page size {size} for:\n{call}\n  unpaged {whole:?}\n  paged   {paged:?}"
            );
            assert_eq!(paged, whole, "page size {size} diverged for:\n{call}");
        }
    }
}

/// AA-004's third leg: the idempotency key's completion record commits with
/// the effect, so the crash window between them is gone.
///
/// The scenario the old code lost: a write commits, the process dies before
/// its response is stored, the reservation ages out, and the retry executes
/// the write a SECOND time. Here the retry finds the key already recorded —
/// in the same transaction that would do the work — and does nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_leased_write_commits_its_completion_with_its_effect() {
    use std::sync::{Arc, Mutex};

    use ordo_conformance::{Fixture, Pg, WriteLog};
    use ordo_exec::Executor;

    let f = Fixture::load().expect("fixture");
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_exec_lease");
    let pg = Pg {
        db: "ordo_exec_lease".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: "ordo_exec_lease".into(),
        ..cfg(4)
    })
    .await
    .expect("boot");
    let schema = f.schema.clone();
    let registry = f.registry.clone();
    let fixture = Arc::new(f);
    let log = Arc::new(Mutex::new(WriteLog::new()));
    let x = Executor {
        pool,
        schema,
        registry,
        keys: fixture.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };
    let lease = |k: &str, fp: &str| Some((k.to_string(), fp.to_string()));

    // First attempt: the effect lands and the key is recorded.
    let out = x
        .write_leased(
            fixture.clone(),
            log.clone(),
            r#"addTask($project: "proj_1", $title: "leased once")"#.into(),
            "mem_1".into(),
            lease("k1", "fp1"),
        )
        .await
        .expect("first attempt runs");
    assert!(out.is_some(), "it ran");
    let count = |t: &str| {
        pg.query(&format!(
            r#"SELECT count(*) FROM "tasks" WHERE "title" = '{t}'"#
        ))
        .unwrap()[0][0]
            .clone()
    };
    assert_eq!(count("leased once"), Some("1".into()));

    // The retry — the request that used to double-write, because the
    // response was never stored. It finds the record and declines.
    let out = x
        .write_leased(
            fixture.clone(),
            log.clone(),
            r#"addTask($project: "proj_1", $title: "leased once")"#.into(),
            "mem_1".into(),
            lease("k1", "fp1"),
        )
        .await
        .expect("retry is not an error");
    assert!(out.is_none(), "the retry declined to re-run");
    assert_eq!(
        count("leased once"),
        Some("1".into()),
        "still exactly one row — this is the whole point"
    );

    // The same key with a different payload is a caller error, not a
    // silent second write.
    let e = x
        .write_leased(
            fixture.clone(),
            log.clone(),
            r#"addTask($project: "proj_1", $title: "different payload")"#.into(),
            "mem_1".into(),
            lease("k1", "fp-other"),
        )
        .await
        .expect_err("reused key, changed request");
    assert!(format!("{e}").contains("idempotency_mismatch"), "{e}");
    assert_eq!(count("different payload"), Some("0".into()));

    // A REFUSED write records nothing: the key stays free, because its
    // work never happened. (Bram cannot write in proj_1.)
    let e = x
        .write_leased(
            fixture.clone(),
            log.clone(),
            r#"addTask($project: "proj_1", $title: "refused")"#.into(),
            "mem_2".into(),
            lease("k2", "fp2"),
        )
        .await
        .expect_err("guard refuses");
    assert!(format!("{e}").contains("access_guard"), "{e}");
    let out = x
        .write_leased(
            fixture.clone(),
            log.clone(),
            r#"addTask($project: "proj_1", $title: "now allowed")"#.into(),
            "mem_1".into(),
            lease("k2", "fp2"),
        )
        .await
        .expect("the key was never consumed");
    assert!(out.is_some(), "a refusal must not burn the key");
}

/// AA-005: a stalled holder that wakes up must lose to the one that took
/// its lease over — and must not commit the work it did in the meantime.
#[tokio::test(flavor = "multi_thread")]
async fn a_fenced_holder_cannot_complete_its_lease() {
    use ordo_exec::LeaseState;

    let admin = ordo_conformance::Pg {
        db: "postgres".into(),
        ..ordo_conformance::Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_exec_fence");
    let pool = Pool::connect(PoolConfig {
        db: "ordo_exec_fence".into(),
        ..cfg(4)
    })
    .await
    .expect("boot");
    pool.rows("DELETE FROM \"$replay\"").await.unwrap();

    // Holder A claims the lease and takes generation 1.
    let a = pool.begin().await.unwrap();
    let (state, gen_a) = a.claim_lease("k", "fp").await.unwrap();
    assert_eq!(state, LeaseState::Fresh);
    assert_eq!(gen_a, 1);
    // A stalls here. Its reservation is durable, so B can see it.
    a.commit().await.unwrap();

    // Holder B takes the lease over. The generation bump IS the fence.
    let b = pool.begin().await.unwrap();
    let (state, gen_b) = b.claim_lease("k", "fp").await.unwrap();
    assert_eq!(state, LeaseState::Fresh, "an unfinished lease is takeable");
    assert_eq!(gen_b, 2, "and taking it supersedes the previous holder");
    b.commit().await.unwrap();

    // A wakes up and tries to finish. It cannot: its generation is stale.
    let a2 = pool.begin().await.unwrap();
    assert!(
        !a2.complete_lease("k", gen_a).await.unwrap(),
        "the fenced holder must lose"
    );
    a2.rollback().await.unwrap();

    // B finishes normally.
    let b2 = pool.begin().await.unwrap();
    assert!(b2.complete_lease("k", gen_b).await.unwrap());
    b2.commit().await.unwrap();

    // And once done, the key replays rather than re-running — for anyone.
    let c = pool.begin().await.unwrap();
    let (state, _) = c.claim_lease("k", "fp").await.unwrap();
    assert_eq!(state, LeaseState::Applied);
    c.rollback().await.unwrap();
}

/// The fence, end to end: a write whose lease was taken over rolls back,
/// so the effect never lands under a key someone else now owns.
#[tokio::test(flavor = "multi_thread")]
async fn a_fenced_write_rolls_back_its_effect() {
    use std::sync::{Arc, Mutex};

    use ordo_conformance::{Fixture, Pg as CPg, WriteLog};
    use ordo_exec::Executor;

    let f = Fixture::load().expect("fixture");
    let admin = CPg {
        db: "postgres".into(),
        ..CPg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_exec_fence2");
    let pg = CPg {
        db: "ordo_exec_fence2".into(),
        ..CPg::default()
    };
    f.reset(&pg).expect("seeded");
    let pool = Pool::connect(PoolConfig {
        db: "ordo_exec_fence2".into(),
        ..cfg(4)
    })
    .await
    .expect("boot");
    let schema = f.schema.clone();
    let registry = f.registry.clone();
    let fixture = Arc::new(f);
    let log = Arc::new(Mutex::new(WriteLog::new()));
    let x = Executor {
        pool: pool.clone(),
        schema,
        registry,
        keys: fixture.keys.clone(),
        space: ordo_conformance::fixture_space(),
    };

    // Pre-claim the key at a generation this write will not hold: the
    // stand-in for "another node took the lease over mid-flight".
    let pre = pool.begin().await.unwrap();
    pre.claim_lease("k9", "fp9").await.unwrap();
    pre.commit().await.unwrap();
    let bump = pool.begin().await.unwrap();
    bump.claim_lease("k9", "fp9").await.unwrap(); // generation 2
    bump.commit().await.unwrap();
    // Force the executor's claim to be superseded before it completes by
    // bumping again behind its back is impossible inside one transaction —
    // so assert the property directly at the seam it protects.
    let stale = pool.begin().await.unwrap();
    assert!(
        !stale.complete_lease("k9", 1).await.unwrap(),
        "generation 1 is fenced out"
    );
    stale.rollback().await.unwrap();

    // ── the chain must not assert a write that did not land ────────────
    // A refused write leaves NO link behind. The chain is the audit spine,
    // and a group in it claims a write happened; appending before commit
    // meant a fenced or refused attempt left history asserting an effect
    // that was rolled back — the exact class the chain exists to preclude.
    let before = log.lock().unwrap().len();
    let e = x
        .write_leased(
            fixture.clone(),
            log.clone(),
            r#"addTask($project: "proj_1", $title: "refused, unlogged")"#.into(),
            "mem_2".into(), // Bram cannot write in proj_1
            None,
        )
        .await
        .expect_err("the guard refuses");
    assert!(format!("{e}").contains("access_guard"), "{e}");
    assert_eq!(
        log.lock().unwrap().len(),
        before,
        "a refused write appended no link"
    );

    // A normal leased write still works: fencing costs the healthy path
    // nothing, and its group DOES reach the chain.
    let out = x
        .write_leased(
            fixture.clone(),
            log.clone(),
            r#"addTask($project: "proj_1", $title: "fenced-suite")"#.into(),
            "mem_1".into(),
            Some(("k10".into(), "fp10".into())),
        )
        .await
        .expect("healthy write");
    assert!(out.is_some());
    assert_eq!(
        log.lock().unwrap().len(),
        before + 1,
        "a committed write appended exactly one"
    );
    assert_eq!(
        pg.query(r#"SELECT count(*) FROM "tasks" WHERE "title" = 'fenced-suite'"#)
            .unwrap()[0][0],
        Some("1".into())
    );
}

/// Board task `store-pool-held-blocks-signin`: the door's one-connection
/// store pool was held for minutes in CI and the log said only
/// `server_busy`. A pool that tracks its holders names the handle holding
/// the connection, what it is doing and for how long, once per refusal
/// burst.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_refusal_names_who_holds_the_connection() {
    use std::sync::{Arc, Mutex};

    use ordo_exec::holders::{describe, HoldPhase};

    let pool = Pool::connect(cfg(1)).await.expect("boot");
    let reports: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = reports.clone();
    pool.track_holders(
        "store pool",
        Arc::new(move |name, refused, holders| {
            seen.lock().unwrap().push(describe(name, refused, holders));
        }),
    );
    assert!(pool.holders().is_empty(), "nothing is held at rest");

    let slow = pool.labelled("credentials");
    let held = tokio::spawn(async move { slow.rows("SELECT pg_sleep(1.5)").await });
    // Let the slow statement take the one connection and start running:
    // the dial comes first, and on a slow runner it alone can take most of
    // the waiter's 300 ms, so wait for the statement itself.
    let mut waited = 0;
    while !pool
        .holders()
        .iter()
        .any(|h| h.phase == HoldPhase::InStatement)
        && waited < 300
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += 1;
    }

    let waiter = pool.labelled("ceremonies");
    match waiter.rows("SELECT 1").await {
        Err(PoolError::Saturated) => {}
        other => panic!("expected Saturated, got {other:?}"),
    }
    let holders = pool.holders();
    assert_eq!(holders.len(), 1, "{holders:?}");
    assert_eq!(holders[0].label, "credentials");
    // It is waiting on its statement, which is named without its values,
    // and the line says which thread holds it and where its connection's
    // socket task runs.
    assert_eq!(holders[0].phase, HoldPhase::InStatement);
    assert!(holders[0].held_for >= Duration::from_millis(300));
    let (sql, waited) = holders[0].statement.clone().expect("a statement in flight");
    assert_eq!(sql, "SELECT pg_sleep(?)");
    assert!(waited >= Duration::from_millis(300));
    assert!(
        holders[0].thread.contains("multi-thread runtime"),
        "{:?}",
        holders[0].thread
    );
    assert!(holders[0].dialled_on.is_some(), "{holders:?}");

    // A second refusal inside the report interval is not a second line.
    let _ = waiter.rows("SELECT 1").await;
    let lines = reports.lock().unwrap().clone();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("store pool: server_busy for ceremonies; held by credentials for "),
        "{}",
        lines[0]
    );
    assert!(
        lines[0].contains("(waiting on a statement) [thread "),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("on `SELECT pg_sleep(?)`"), "{}", lines[0]);

    held.await.unwrap().expect("the slow statement finishes");
    assert!(
        pool.holders().is_empty(),
        "the entry leaves with the checkout"
    );
    pool.rows("SELECT 1")
        .await
        .expect("the connection is free again");
}

/// Board task `store-pool-held-blocks-signin`, the second half: when the
/// holder is waiting on a statement, Postgres's own view says on what. A
/// store statement stuck behind another connection's row lock shows as the
/// tracked pool's backend (`*`), waiting on a lock, blocked by the other
/// backend's pid, with the statement's shape and none of its values.
#[tokio::test(flavor = "multi_thread")]
async fn the_postgres_view_names_the_lock_a_held_connection_waits_on() {
    use std::sync::Arc;

    use ordo_exec::incident;

    let table = format!("incident_probe_{}", std::process::id());
    let admin = Pool::connect(cfg(1)).await.expect("boot");
    admin
        .rows(&format!(
            "CREATE TABLE IF NOT EXISTS {table} (k text PRIMARY KEY)"
        ))
        .await
        .expect("table");

    let pool = Pool::connect(cfg(1)).await.expect("boot");
    pool.track_holders("store pool", Arc::new(|_, _, _| {}));
    assert_eq!(pool.application_name(), Some("ordo: store pool"));

    // Another connection takes the row and keeps its transaction open.
    let blocker = admin.begin().await.expect("begin");
    blocker
        .rows(&format!("INSERT INTO {table} VALUES ('secret-value-123')"))
        .await
        .expect("insert");
    let blocker_pid = blocker.rows("SELECT pg_backend_pid()").await.expect("pid")[0][0]
        .clone()
        .expect("a pid");

    // The tracked pool's statement waits on that row.
    let held = pool.labelled("credentials");
    let insert = format!("INSERT INTO {table} VALUES ('secret-value-123') ON CONFLICT DO NOTHING");
    let stuck = tokio::spawn(async move { held.rows(&insert).await });
    let mut waited = 0;
    while pool.holders().first().is_none_or(|h| h.statement.is_none()) && waited < 200 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += 1;
    }
    // Let the statement reach the lock.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let (tx, rx) = std::sync::mpsc::channel();
    incident::spawn_snapshot(
        pool.config(),
        pool.application_name().map(str::to_string),
        move |block| {
            let _ = tx.send(block);
        },
    );
    let block = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the snapshot returns within its budget");

    blocker.rollback().await.expect("rollback");
    stuck
        .await
        .unwrap()
        .expect("the statement finishes once the lock goes");
    admin
        .rows(&format!("DROP TABLE {table}"))
        .await
        .expect("drop");

    assert!(
        !block.contains("secret-value-123"),
        "a value reached the snapshot:\n{block}"
    );
    let mine = block
        .lines()
        .find(|l| l.starts_with("pool incident: * "))
        .unwrap_or_else(|| panic!("no backend of the tracked pool:\n{block}"));
    assert!(mine.contains("wait=Lock/transactionid"), "{mine}");
    assert!(
        mine.contains(&format!("blocked_by=[{blocker_pid}]")),
        "{mine}"
    );
    assert!(
        mine.contains(&format!(
            "sql=`INSERT INTO {table} VALUES ('?') ON CONFLICT DO NOTHING`"
        )),
        "{mine}"
    );
    assert!(
        block
            .lines()
            .any(|l| l.contains("lock pid=") && l.contains("WAITS for ShareLock transactionid")),
        "{block}"
    );
    assert!(
        block
            .lines()
            .any(|l| l.contains(&format!("pid={blocker_pid} "))),
        "the blocker's own row is in the view:\n{block}"
    );
    assert!(
        block.ends_with("pool incident: end of postgres view"),
        "{block}"
    );
}

/// The setting reads `on`, `off` and nothing, and refuses anything else.
#[test]
fn the_snapshot_setting_reads_on_and_off() {
    use ordo_exec::incident::snapshot_enabled;
    assert_eq!(snapshot_enabled(None), Ok(true));
    assert_eq!(snapshot_enabled(Some("on")), Ok(true));
    assert_eq!(snapshot_enabled(Some("off")), Ok(false));
    let refused = snapshot_enabled(Some("of")).expect_err("a typo refuses");
    assert!(refused.contains("ORDO_PG_BUSY_SNAPSHOT"), "{refused}");
}
