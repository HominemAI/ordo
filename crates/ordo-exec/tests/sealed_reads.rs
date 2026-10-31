//! 27ao (`serve-sealed-reads`): the executor serves a read that returns a
//! sealed field.
//!
//! The rows come back from Postgres as stored, envelopes and all. The host
//! takes one `Purpose::Read` release for the page, opens that page's
//! envelopes at the rows they were sealed for, and answers plaintext. What
//! does not change: the guard decides which rows are fetched before anything
//! opens, a caller who may read no row causes no release, anonymous opens
//! nothing, and a read whose host work grows with the data is refused,
//! naming the field.
//!
//! Every release here is counted through a wrapping provider, as
//! `crates/ordo-conformance/tests/sealed_through_join.rs` counts them.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`). Each
//! test owns its own database, prefixed `ordo_sealread_`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ordo_conformance::{fixture_space, Fixture, Pg, FIXTURE, SEED};
use ordo_crypto::{Anchor, KeyProvider, Purpose};
use ordo_exec::{ExecFail, Executor, Pool, PoolConfig};

/// A guarded sealed entity, and ops over the fixture's sealed fields.
const EXTRA: &str = r#"
entity diaries (key: diary) {
  owner -> members
  day:   date
  entry: text sealed
  access read   owner == $caller
  access write  owner == $caller
}

op myDiary =
  diaries[day, entry]
  sort day

op withMemo unbounded =
  customers[name] orders[placed, memo]

op orderAndCustomer =
  orders[placed, memo] customers[name]
"#;

/// A provider that records every READ release by its correlation.
struct Counting {
    inner: Arc<dyn KeyProvider>,
    reads: Mutex<Vec<String>>,
}

impl Counting {
    /// The read releases since the last call, by correlation.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.reads.lock().unwrap())
    }
}

impl KeyProvider for Counting {
    fn release(
        &self,
        space: &ordo_crypto::SpaceId,
        purpose: Purpose,
        correlation: &str,
    ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
        assert_eq!(space, &fixture_space(), "only this tenant's key");
        if purpose == Purpose::Read {
            self.reads.lock().unwrap().push(correlation.to_string());
        }
        self.inner.release(space, purpose, correlation)
    }
    fn seal_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.inner.seal_at(l, e, f, a, v)
    }
    fn open_at(
        &self,
        l: &ordo_crypto::KeyLease,
        e: &str,
        f: &str,
        a: Anchor<'_>,
        v: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        assert_eq!(l.purpose, Purpose::Read, "a read opens under a read lease");
        self.inner.open_at(l, e, f, a, v)
    }
    #[allow(clippy::too_many_arguments)]
    fn seal_chunk(
        &self,
        lease: &ordo_crypto::KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        epoch: ordo_crypto::Epoch,
        base: &ordo_crypto::NonceBase,
        counter: u32,
        last: bool,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.inner.seal_chunk(
            lease, entity, field, anchor, epoch, base, counter, last, plaintext,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn open_chunk(
        &self,
        lease: &ordo_crypto::KeyLease,
        entity: &str,
        field: &str,
        anchor: Anchor<'_>,
        epoch: ordo_crypto::Epoch,
        base: &ordo_crypto::NonceBase,
        counter: u32,
        last: bool,
        sealed: &[u8],
    ) -> Result<Vec<u8>, ordo_crypto::KeyError> {
        self.inner.open_chunk(
            lease, entity, field, anchor, epoch, base, counter, last, sealed,
        )
    }
    fn blind_index(
        &self,
        space: &ordo_crypto::SpaceId,
        entity: &str,
        field: &str,
        normalized: ordo_crypto::BlindValue<'_>,
    ) -> Result<[u8; 32], ordo_crypto::KeyError> {
        self.inner.blind_index(space, entity, field, normalized)
    }
    fn current_epoch(
        &self,
        s: &ordo_crypto::SpaceId,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.inner.current_epoch(s)
    }
    fn rotate(
        &self,
        s: &ordo_crypto::SpaceId,
        reason: &str,
    ) -> Result<ordo_crypto::Epoch, ordo_crypto::KeyError> {
        self.inner.rotate(s, reason)
    }
    fn create_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
        self.inner.create_space(s)
    }
    fn destroy_epoch(
        &self,
        s: &ordo_crypto::SpaceId,
        e: ordo_crypto::Epoch,
    ) -> Result<(), ordo_crypto::KeyError> {
        self.inner.destroy_epoch(s, e)
    }
    fn destroy_space(&self, s: &ordo_crypto::SpaceId) -> Result<(), ordo_crypto::KeyError> {
        self.inner.destroy_space(s)
    }
}

/// Seal `plain` for `entity.field` at row `id`, and store it there.
fn seal_into(f: &Fixture, pg: &Pg, entity: &str, field: &str, id: &str, plain: &str) {
    let lease = f
        .keys
        .release(&fixture_space(), Purpose::Write, "test")
        .unwrap();
    let envelope = f
        .keys
        .seal_at(&lease, entity, field, Anchor::Row(id), plain.as_bytes())
        .unwrap();
    pg.exec(&format!(
        "UPDATE \"{entity}\" SET \"{field}\" = '\\x{}'::bytea WHERE \"id\" = '{id}'",
        ordo_crypto::hex(&envelope)
    ))
    .unwrap();
}

/// A seeded database of the test's own: every order's memo sealed for real
/// (`memo-ord_N`), `cust_1`'s email sealed, and four diary pages, three
/// `mem_1`'s and one `mem_2`'s.
async fn executor(db: &str) -> (Executor, Arc<Counting>) {
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
    for n in 1..=7 {
        let id = format!("ord_{n}");
        seal_into(&f, &pg, "orders", "memo", &id, &format!("memo-{id}"));
    }
    seal_into(&f, &pg, "customers", "email", "cust_1", "ada@example.com");
    for (id, owner, day) in [
        ("diary_1", "mem_1", "2026-01-01"),
        ("diary_2", "mem_1", "2026-01-02"),
        ("diary_3", "mem_1", "2026-01-03"),
        ("diary_4", "mem_2", "2026-01-04"),
    ] {
        pg.exec(&format!(
            "INSERT INTO \"diaries\" (\"$space\", \"id\", \"owner\", \"day\", \"entry\") \
             VALUES ('{}', '{id}', '{owner}', DATE '{day}', '\\x'::bytea)",
            fixture_space().0
        ))
        .unwrap();
        seal_into(&f, &pg, "diaries", "entry", id, &format!("dear {id}"));
    }
    let counting = Arc::new(Counting {
        inner: f.keys.clone(),
        reads: Mutex::new(Vec::new()),
    });
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("boot");
    (
        Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: counting.clone(),
            space: fixture_space(),
        },
        counting,
    )
}

fn venue(r: Result<impl std::fmt::Debug, ExecFail>, what: &str) -> String {
    match r {
        Err(ExecFail::Venue(m)) => {
            assert!(m.starts_with("sealed_unsupported: "), "{what}: {m}");
            m
        }
        other => panic!("{what}: expected a venue refusal, got {other:?}"),
    }
}

/// A list of a sealed field, page by page: plaintext on every row, and one
/// read release per page, each under that request's own correlation.
#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_list_opens_page_by_page_with_one_release_per_page() {
    let (ex, keys) = executor("ordo_sealread_list").await;
    let mut memos = Vec::new();
    let mut releases = Vec::new();
    let mut after: Option<Vec<Option<String>>> = None;
    let mut pages = 0;
    loop {
        let page = ex
            .read_page_as("orders[id, memo]", Some("mem_1"), 3, after.as_deref())
            .await
            .expect("a bounded-host list is served");
        pages += 1;
        assert!(page.rows.len() <= 3);
        for r in &page.rows {
            assert_eq!(
                r["memo"],
                format!("memo-{}", r["id"].as_str().unwrap()),
                "each envelope opens at its own row: {r}"
            );
            memos.push(r["memo"].as_str().unwrap().to_string());
        }
        let page_releases = keys.take();
        assert_eq!(page_releases.len(), 1, "page {pages}: {page_releases:?}");
        releases.extend(page_releases);
        match page.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(memos.len(), 7);
    let mut distinct = releases.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        3,
        "one correlation per request: {releases:?}"
    );
}

/// One row, read whole and as a page: a unique address bounds the work, so
/// both paths serve it, under one release each.
#[tokio::test(flavor = "multi_thread")]
async fn a_single_row_read_returns_its_sealed_value() {
    let (ex, keys) = executor("ordo_sealread_one").await;
    let rows = ex
        .read_as(r#"orders(id == "ord_4")[placed, memo]"#, Some("mem_1"))
        .await
        .expect("served whole");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["memo"], "memo-ord_4");
    assert_eq!(keys.take().len(), 1);

    let page = ex
        .read_page_as(
            r#"orders(id == "ord_4")[placed, memo]"#,
            Some("mem_1"),
            50,
            None,
        )
        .await
        .expect("served as a page");
    assert_eq!(page.rows[0]["memo"], "memo-ord_4");
    assert_eq!(keys.take().len(), 1);

    // `take n` is a bound the plan proves, so the whole read is served too.
    let rows = ex
        .read_as("memoDigest", Some("mem_1"))
        .await
        .expect("take 3 bounds it");
    assert_eq!(rows.len(), 3);
    assert!(rows
        .iter()
        .all(|r| r["memo"].as_str().unwrap().starts_with("memo-ord_")));
    assert_eq!(keys.take().len(), 1);
}

/// A join that returns a sealed field opens it at the row it was read
/// from (25r, 25s, 25v stay held: the value is plaintext, never the stored
/// envelope). The one side of a join keeps the page bound; the many side
/// does not, and is refused naming the field.
#[tokio::test(flavor = "multi_thread")]
async fn a_join_returning_a_sealed_field_is_served_as_plaintext() {
    let (ex, keys) = executor("ordo_sealread_join").await;
    for call in ["orders[placed, memo] customers[name]", "orderAndCustomer"] {
        let page = ex
            .read_page_as(call, Some("mem_1"), 50, None)
            .await
            .unwrap_or_else(|e| panic!("{call}: {e}"));
        assert_eq!(page.rows.len(), 7, "{call}: {:?}", page.rows);
        for r in &page.rows {
            let memo = r["memo"].as_str().unwrap();
            assert!(memo.starts_with("memo-ord_"), "{call}: {r}");
            assert!(!memo.starts_with("\\x"), "never the envelope: {r}");
        }
        assert_eq!(keys.take().len(), 1, "{call}");
    }
    let ada = ex
        .read_page_as(
            r#"orders(id == "ord_7")[placed, memo] customers[name]"#,
            Some("mem_1"),
            50,
            None,
        )
        .await
        .unwrap();
    assert_eq!(ada.rows[0]["memo"], "memo-ord_7");
    assert_eq!(ada.rows[0]["name"], "Ada");
    let _ = keys.take();

    for call in ["customers[name] orders[placed, memo]", "withMemo"] {
        let m = venue(ex.read_page_as(call, Some("mem_1"), 50, None).await, call);
        assert!(m.contains("`orders.memo`"), "names the field: {m}");
        assert!(m.contains("unbounded-host"), "{m}");
        assert!(m.contains("take n"), "names the fix: {m}");
        assert!(keys.take().is_empty(), "{call}: a refusal opens nothing");
    }
}

/// An equality on an indexed sealed field rides the blind index, and the
/// rows it finds return the sealed value itself.
#[tokio::test(flavor = "multi_thread")]
async fn an_indexed_equality_returns_the_sealed_value() {
    let (ex, keys) = executor("ordo_sealread_bidx").await;
    let call = r#"customers(email == "ADA@example.com")[name, email]"#;
    let rows = ex
        .read_as(call, Some("mem_1"))
        .await
        .expect("unique: whole");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["name"], "Ada");
    assert_eq!(rows[0]["email"], "ada@example.com", "normalized on write");
    assert_eq!(keys.take().len(), 1);
    let page = ex
        .read_page_as(call, Some("mem_1"), 50, None)
        .await
        .unwrap();
    assert_eq!(page.rows[0]["email"], "ada@example.com");
    assert_eq!(keys.take().len(), 1);
    // A match that finds nothing opens nothing.
    let none = ex
        .read_page_as(
            r#"customers(email == "nobody@example.com")[name, email]"#,
            Some("mem_1"),
            50,
            None,
        )
        .await
        .unwrap();
    assert!(none.rows.is_empty());
    assert!(keys.take().is_empty(), "no row, no release");
}

/// The guard decides the rows before anything opens. Each member reads
/// only their own pages; a member with none gets no rows and no release,
/// and anonymous is refused before the database is asked.
#[tokio::test(flavor = "multi_thread")]
async fn the_guard_decides_what_opens_and_anonymous_opens_nothing() {
    let (ex, keys) = executor("ordo_sealread_guard").await;
    let mine = ex
        .read_page_as("myDiary", Some("mem_1"), 50, None)
        .await
        .unwrap();
    let entries: Vec<&str> = mine
        .rows
        .iter()
        .map(|r| r["entry"].as_str().unwrap())
        .collect();
    assert_eq!(entries, ["dear diary_1", "dear diary_2", "dear diary_3"]);
    assert_eq!(keys.take().len(), 1);

    let theirs = ex
        .read_page_as("myDiary", Some("mem_2"), 50, None)
        .await
        .unwrap();
    assert_eq!(theirs.rows.len(), 1);
    assert_eq!(theirs.rows[0]["entry"], "dear diary_4");
    assert_eq!(keys.take().len(), 1);

    let nobody = ex
        .read_page_as("myDiary", Some("mem_3"), 50, None)
        .await
        .unwrap();
    assert!(nobody.rows.is_empty());
    assert!(
        keys.take().is_empty(),
        "a guarded-out caller releases nothing"
    );

    for call in [
        "myDiary",
        "orders[id, memo]",
        r#"orders(id == "ord_1")[memo]"#,
    ] {
        let m = venue(ex.read_page_as(call, None, 50, None).await, call);
        assert!(m.contains("anonymous"), "{m}");
        let m = venue(ex.read_as(call, None).await, call);
        assert!(m.contains("anonymous"), "{m}");
    }
    assert!(keys.take().is_empty(), "anonymous opened nothing");
}

/// A read answered whole has no page to bound it. A plan bounded only by
/// paging is refused there, naming the field and the fix, and opens
/// nothing; the same read is served a page at a time.
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_read_needs_a_bound_the_plan_proves() {
    let (ex, keys) = executor("ordo_sealread_whole").await;
    for call in ["orders[memo]", "orderMemos", "myDiary"] {
        let m = venue(ex.read_as(call, Some("mem_1")).await, call);
        assert!(m.contains("sealed field `"), "{m}");
        assert!(m.contains("a page at a time"), "{m}");
        assert!(keys.take().is_empty());
        let page = ex.read_page_as(call, Some("mem_1"), 50, None).await;
        assert!(page.is_ok(), "{call}: {:?}", page.err());
        assert_eq!(keys.take().len(), 1, "{call}");
    }
    // A sealed value folded away opens nothing and plans as plain SQL.
    let rows = ex
        .read_as(
            "orders[placed, memo]\n  group placed { memo: count }",
            Some("mem_1"),
        )
        .await
        .expect("a count of a sealed field is sql");
    assert_eq!(rows.len(), 7);
    assert!(keys.take().is_empty());
}

/// An envelope opens only at the row and the column it was sealed for.
/// One moved under another row, or sealed for another column, is a hard
/// refusal that names the field, never that row's value and never null.
/// The rows the swap did not touch still open.
#[tokio::test(flavor = "multi_thread")]
async fn a_moved_envelope_refuses_rather_than_open_as_another_rows_value() {
    let (ex, keys) = executor("ordo_sealread_swap").await;
    let pg = Pg {
        db: "ordo_sealread_swap".into(),
        ..Pg::default()
    };
    // ord_1's envelope, stored at ord_2.
    pg.exec(
        "UPDATE \"orders\" SET \"memo\" = (SELECT \"memo\" FROM \"orders\" WHERE \"id\" = 'ord_1') \
         WHERE \"id\" = 'ord_2'",
    )
    .unwrap();
    // A value sealed for `customers.email` at ord_3's own id, stored in
    // `orders.memo`: the row matches and the column does not.
    let f = Fixture::from_source(&format!("{FIXTURE}\n{EXTRA}"), SEED).expect("fixture");
    let lease = f
        .keys
        .release(&fixture_space(), Purpose::Write, "test")
        .unwrap();
    let envelope = f
        .keys
        .seal_at(
            &lease,
            "customers",
            "email",
            Anchor::Row("ord_3"),
            b"not a memo",
        )
        .unwrap();
    pg.exec(&format!(
        "UPDATE \"orders\" SET \"memo\" = '\\x{}'::bytea WHERE \"id\" = 'ord_3'",
        ordo_crypto::hex(&envelope)
    ))
    .unwrap();
    for (call, what) in [
        (r#"orders(id == "ord_2")[memo]"#, "another row's envelope"),
        (
            r#"orders(id == "ord_3")[memo]"#,
            "another column's envelope",
        ),
    ] {
        for r in [
            ex.read_as(call, Some("mem_1")).await,
            ex.read_page_as(call, Some("mem_1"), 50, None)
                .await
                .map(|p| p.rows),
        ] {
            match r {
                Err(ExecFail::Compile(m)) => {
                    assert!(
                        m.starts_with("sealed_unreadable: `orders.memo`"),
                        "{what}: {m}"
                    );
                    assert!(!m.contains("memo-ord_"), "{what}: no plaintext: {m}");
                }
                other => panic!("{what}: {call} must refuse, got {other:?}"),
            }
        }
    }
    let _ = keys.take();
    // A page that holds a moved envelope refuses whole: no row of it is
    // answered. The rows the swap did not touch still open on their own.
    assert!(matches!(
        ex.read_page_as("orders[id, memo]", Some("mem_1"), 50, None)
            .await,
        Err(ExecFail::Compile(_))
    ));
    let one = ex
        .read_as(r#"orders(id == "ord_4")[memo]"#, Some("mem_1"))
        .await
        .unwrap();
    assert_eq!(one[0]["memo"], "memo-ord_4");
}
