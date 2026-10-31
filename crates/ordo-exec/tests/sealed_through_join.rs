//! Reported fix 25r (`sealed-through-join`), the serving path's half. The
//! binder's half is `crates/ordo-conformance/tests/sealed_through_join.rs`.
//!
//! Before the fix, `customers[name] orders[placed, memo]` classed `sql`, so
//! it passed the venue gate and was served with `memo` as the stored
//! envelope in Postgres hex (`\x01cb81…`), typed as text, with no key
//! release. A regression is that hex in a response.
//!
//! Since 27ao (`serve-sealed-reads`) the executor serves `bounded-host`
//! reads, so a join that keeps the page bound returns the memo as
//! plaintext, and one that does not (the many side) is refused naming the
//! field. Neither ever returns the envelope.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use std::time::Duration;

use ordo_conformance::{fixture_space, Fixture, Pg, FIXTURE, SEED};
use ordo_crypto::{Anchor, Purpose};
use ordo_exec::{ExecFail, Executor, Pool, PoolConfig};

const EXTRA: &str = r#"
op withMemo unbounded =
  customers[name] orders[placed, memo]
"#;

async fn executor(db: &str) -> Executor {
    let src = format!("{FIXTURE}\n{EXTRA}");
    let f = Fixture::from_source(&src, SEED).expect("fixture");
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
    // A real envelope on `ord_4`, so a served memo would be visibly one.
    let lease = f
        .keys
        .release(&fixture_space(), Purpose::Write, "test")
        .unwrap();
    let envelope = f
        .keys
        .seal_at(&lease, "orders", "memo", Anchor::Row("ord_4"), b"side door")
        .unwrap();
    pg.exec(&format!(
        "UPDATE \"orders\" SET \"memo\" = '\\x{}'::bytea WHERE \"id\" = 'ord_4'",
        ordo_crypto::hex(&envelope)
    ))
    .unwrap();
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    Executor {
        pool,
        schema: f.schema.clone(),
        registry: f.registry.clone(),
        keys: f.keys.clone(),
        space: fixture_space(),
    }
}

/// The memo is plaintext or refused, on both read paths, and never the
/// stored envelope. The same join without the sealed field is still served.
#[tokio::test(flavor = "multi_thread")]
async fn the_executor_never_serves_a_joined_envelope_as_text() {
    let ex = executor("ordo_sealjoin_exec").await;
    // Served a page at a time, opened at the row each envelope came from.
    for call in [
        "orders[placed, memo]",
        "orders[placed, memo] customers[name]",
    ] {
        let page = ex
            .read_page_as(call, Some("mem_1"), 50, None)
            .await
            .unwrap_or_else(|e| panic!("{call}: {e}"));
        assert_eq!(page.rows.len(), 7, "{call}");
        let memos: Vec<&str> = page
            .rows
            .iter()
            .map(|r| r["memo"].as_str().unwrap())
            .collect();
        assert!(memos.contains(&"side door"), "{call}: {memos:?}");
        assert!(memos.iter().all(|m| !m.starts_with("\\x")), "{memos:?}");
        // Read whole there is no page to bound it, so it is refused.
        match ex.read_as(call, Some("mem_1")).await {
            Err(ExecFail::Venue(m)) => {
                assert!(m.contains("`orders.memo`"), "{call}: {m}")
            }
            other => panic!("{call}: {other:?}"),
        }
    }
    // The many side: the payloads opened grow with the data.
    for call in [
        "customers[name] orders[placed, memo]",
        "withMemo",
        "withMemo[name, memo]",
    ] {
        match ex.read_as(call, Some("mem_1")).await {
            Err(ExecFail::Venue(m)) => assert!(m.contains("`orders.memo`"), "{call}: {m}"),
            Ok(rows) => panic!("{call} was served: {rows:?}"),
            Err(e) => panic!("{call}: {e}"),
        }
        match ex.read_page_as(call, Some("mem_1"), 50, None).await {
            Err(ExecFail::Venue(m)) => assert!(m.contains("unbounded-host"), "{call}: {m}"),
            Ok(p) => panic!("{call} was served as a page: {:?}", p.rows),
            Err(e) => panic!("{call}: {e}"),
        }
    }

    // The refusal is about the payload, not the join.
    let rows = ex
        .read_as("customers[name] orders[placed]", Some("mem_1"))
        .await
        .expect("the open join is served");
    assert_eq!(rows.len(), 7, "{rows:?}");
}
