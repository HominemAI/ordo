//! Reported fix 25r (`sealed-through-join`): a sealed field reached through
//! a multi-source projection must be carried exactly as a single-source
//! projection carries it.
//!
//! What was seen: `customers[name] orders[placed, memo]` classed `sql`,
//! printed `memo: text` with no `(sealed)`, and listed no key release. The
//! cause was `Binder::bind_multi_source` setting the query's outputs and
//! never its `host_open` list, which is the one place the planner, the
//! executor, the cost class and the artifact all read sealed-ness from. A
//! caller received the stored envelope rendered as text (`\x01cb81…`), with
//! no key release and no security event, and the venue gate let it through
//! because the plan claimed to be plain SQL.
//!
//! The same omission existed in two more places, both fixed and held here:
//! a `let` read as a source, and an `append` arm. An `append` whose arms
//! disagree about a column's seal is now misaligned. A group clears the
//! obligations (ruled 2026-10-05), so a grouped output never carries one.
//!
//! The executing tests need the conformance cluster
//! (`scripts/conformance-pg.sh start`). Each owns its own database. The
//! serving path's half is `crates/ordo-exec/tests/sealed_through_join.rs`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ordo_conformance::{fixture_space, Fixture, Pg, FIXTURE, SEED};
use ordo_crypto::{Anchor, KeyProvider, Purpose};
use ordo_plan::cost::{classify_with, CostClass};
use serde_json::Value;

/// The market fixture plus the ops this suite needs registered: the join
/// that was reported, its single-source twin, and callers over each.
const EXTRA: &str = r#"
op withMemo unbounded =
  customers[name] orders[placed, memo]

op memoThenCustomer unbounded =
  orders[placed, memo] customers[name]

op withMemoRenamed unbounded =
  customers[name] orders[placed, note: memo]

op overWithMemo unbounded =
  withMemo[name, memo]

op letMemo unbounded =
  let m = orders[placed, memo, customer]
  m[placed, memo]

op takeWithMemo =
  customers[name] orders[placed, memo]
  sort placed
  take 3

op appendMemo unbounded =
  orders(status == "done")[placed, memo]
  append orders(status == "open")[placed, memo]
"#;

fn fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{EXTRA}");
    Fixture::from_source(&src, SEED).expect("the fixture plus this suite's ops compiles")
}

/// The obligation list a bound pipeline carries.
fn host_open(f: &Fixture, src: &str) -> Vec<(String, String, String)> {
    f.bind_case(src)
        .unwrap_or_else(|e| panic!("{src}: {e}"))
        .host_open
}

fn memo(output: &str) -> Vec<(String, String, String)> {
    vec![(output.to_string(), "orders".into(), "memo".into())]
}

// ── the binder ───────────────────────────────────────────────────────────

/// The reported shape, in both source orders and through a rename: the
/// sealed output names its entity and field, so the planner anchors the
/// envelope to its row and the executor knows how to open it.
#[test]
fn a_sealed_field_through_a_join_carries_its_seal() {
    let f = fixture();
    for (src, out) in [
        ("orders[placed, memo]", "memo"),
        ("customers[name] orders[placed, memo]", "memo"),
        ("orders[placed, memo] customers[name]", "memo"),
        ("customers[name] orders[placed, note: memo]", "note"),
    ] {
        assert_eq!(host_open(&f, src), memo(out), "{src}");
        let p = f.compile(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(p.host_open, memo(out), "{src}");
        // The envelope leaves the database packed with the id of the row it
        // was read from. A bare `"t1"."memo"` is the envelope with nothing
        // to open it at, which is what the join used to emit.
        assert!(
            p.sql.contains("convert_to("),
            "{src}: the envelope is not anchored to its row:\n{}",
            p.sql
        );
    }
}

/// Host work is host work whichever source projected it. A join is never
/// classed `sql` once it opens a payload.
#[test]
fn a_sealed_field_through_a_join_is_host_work() {
    let f = fixture();
    let class = |src: &str| classify_with(&f.bind_case(src).unwrap(), Some(&f.schema));
    assert_eq!(class("orders[placed, memo]"), CostClass::BoundedHost);
    // The anchor is the many side, so a page of rows is a page of payloads.
    assert_eq!(
        class("orders[placed, memo] customers[name]"),
        CostClass::BoundedHost
    );
    // The many side hangs off each customer: a page of customers does not
    // bound how many orders it opens, so the page earns no credit.
    assert_eq!(
        class("customers[name] orders[placed, memo]"),
        CostClass::UnboundedHost
    );
    // The same join without the sealed field is plain SQL. The class moved
    // because of the payload and nothing else.
    assert_eq!(class("customers[name] orders[placed]"), CostClass::Sql);
}

/// What `ordo check`, the interface and a consent page read: the artifact's
/// cost class, its sealed outputs and its release set.
#[test]
fn the_artifact_of_a_join_marks_the_output_sealed_and_releases_its_key() {
    let f = fixture();
    for (op, out) in [
        ("withMemo", "memo"),
        ("memoThenCustomer", "memo"),
        ("withMemoRenamed", "note"),
        ("overWithMemo", "memo"),
        ("letMemo", "memo"),
        ("appendMemo", "memo"),
        ("takeWithMemo", "memo"),
    ] {
        let art = ordo_plan::artifact::op_artifact(&f.schema, &f.registry, op)
            .unwrap_or_else(|e| panic!("{op}: {e}"));
        let class = art["cost"]["class"].as_str().unwrap_or_default();
        assert!(class.ends_with("host"), "{op} is classed {class}: {art:#}");
        // The artifact classes with the schema, as the door and the venue
        // gate do, so all three name one class (ruled 2026-10-05). Without
        // it `takeWithMemo` read `unbounded-host` here and `bounded-host`
        // at the door.
        let q = ordo_sema::bind_op_generic(&f.schema, &f.registry, op, ordo_sema::Clock::CORPUS)
            .unwrap_or_else(|e| panic!("{op}: {e}"));
        assert_eq!(
            class,
            classify_with(&q, Some(&f.schema)).as_str(),
            "{op}: the artifact and the door disagree"
        );
        if op == "takeWithMemo" {
            assert_eq!(class, "bounded-host", "{op}: a take of 3 opens 3 memos");
        }
        let outputs = art["outputs"].as_array().expect("outputs");
        let o = outputs
            .iter()
            .find(|o| o["name"] == out)
            .unwrap_or_else(|| panic!("{op} has no output `{out}`: {art:#}"));
        assert_eq!(o["sealed"], true, "{op}.{out}: {art:#}");

        let release = ordo_plan::artifact::op_release(&f.schema, &f.registry, op);
        assert_eq!(release.purpose, Some("read"), "{op}");
        assert!(
            release.keys.iter().any(|k| k.entity == "orders"
                && k.field == "memo"
                && k.key == "dek"
                && k.output.as_deref() == Some(out)),
            "{op} lists no release for orders.memo: {release:?}"
        );
    }
}

/// A condition, a sort or a group on a sealed field reached through a join
/// refuses exactly as it does on a single source. The join gives the
/// database no new way to read what it cannot read.
#[test]
fn a_join_gives_a_sealed_field_no_new_reader() {
    let f = fixture();
    for src in [
        "customers[name] orders[placed, memo]\n  | memo == \"x\"",
        "customers[name] orders[placed]\n  | orders.memo == \"x\"",
        "customers[name] orders(memo == \"x\")[placed]",
        "customers[name] orders[placed, memo]\n  sort memo",
        "customers[name] orders[placed, memo]\n  group memo { n: count }",
        "customers[name] orders[placed, memo]\n  group customers.name { m: joined(memo, \", \") }",
        "orders[placed, memo] customers[name]\n  | memo != \"\"",
        // `distinct` compares the outputs, and a sealed one is an envelope
        // under a per-row nonce: it would remove nothing. Refused like a
        // sort, through a join, alone, and through a caller.
        "customers[name] orders[placed, memo]\n  distinct",
        "orders[placed, memo]\n  distinct",
        "withMemo\n  distinct",
    ] {
        let e = f.compile(src).expect_err(src);
        assert!(e.contains("sealed_unsupported"), "{src}: {e}");
    }
}

/// Composition, `let` and `append` keep the seal too. Each of these used to
/// read the multi-source callee, the CTE or the union arm with its
/// obligation dropped.
#[test]
fn composition_let_and_append_keep_the_seal() {
    let f = fixture();
    assert_eq!(host_open(&f, "withMemo[name, memo]"), memo("memo"));
    assert_eq!(host_open(&f, "withMemo"), memo("memo"));
    assert_eq!(
        host_open(
            &f,
            "let m = orders[placed, memo, customer]\nm[placed, memo]"
        ),
        memo("memo")
    );
    // A let joined back to its source is a multi-source list over a CTE.
    assert_eq!(
        host_open(
            &f,
            "let m = orders[placed, memo, customer]\ncustomers[name] m[placed, memo]"
        ),
        memo("memo")
    );
    let src =
        "orders(status == \"done\")[placed, memo]\nappend orders(status == \"open\")[placed, memo]";
    let q = f.bind_case(src).unwrap();
    assert_eq!(q.host_open, memo("memo"), "the union");
    let ordo_sema::From::Union { arms, .. } = &q.from else {
        panic!("an append is a union: {:?}", q.from);
    };
    for (i, a) in arms.iter().enumerate() {
        assert_eq!(a.host_open, memo("memo"), "arm {i}");
    }
}

/// An append whose arms disagree about a column's seal is misaligned. One
/// column is opened under one `(entity, field)`, so an envelope beside
/// plaintext would serve the envelope as text.
#[test]
fn an_append_refuses_a_column_sealed_in_one_arm_only() {
    let f = fixture();
    for src in [
        "orders[placed, memo]\nappend orders[placed, memo: status]",
        "orders[placed, memo: status]\nappend orders[placed, memo]",
        "orders[placed, who: memo]\nappend orders[placed, who: status]",
    ] {
        let e = f.compile(src).expect_err(src);
        assert!(e.contains("append_misalignment"), "{src}: {e}");
        assert!(e.contains("sealed"), "the refusal names the seal: {e}");
    }
}

/// A group over a sealed join is plain SQL, as it is over one source
/// (ruled 2026-10-05, `audit-2026-10-05` ask 1). Nothing sealed survives the
/// fold, so the grouped query carries no obligation to open anything.
///
/// The obligations are keyed by output name. Before the ruling, an aggregate
/// named after a sealed output (`group placed { memo: count }`) would have
/// marked the count sealed, so it was refused. With the fold clearing them,
/// the name marks nothing and the count binds as a plain number.
#[test]
fn a_group_over_a_sealed_join_is_plain_sql() {
    let f = fixture();
    for src in [
        "orders[placed, memo]\n  group placed { n: count }",
        "customers[name] orders[placed, memo]\n  group customers.name { n: count }",
        "orders[placed, memo]\n  group placed { memo: count }",
        "customers[name] orders[placed, memo]\n  group customers.name { memo: count }",
    ] {
        let q = f.bind_case(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert!(q.host_open.is_empty(), "{src}: {:?}", q.host_open);
        assert_eq!(classify_with(&q, Some(&f.schema)), CostClass::Sql, "{src}");
    }
}

// ── execution ────────────────────────────────────────────────────────────

/// A provider that counts READ releases, so a test can see that opening a
/// value took custody the way the security event records it.
struct Counting {
    inner: Arc<dyn KeyProvider>,
    reads: AtomicUsize,
}

impl KeyProvider for Counting {
    fn release(
        &self,
        space: &ordo_crypto::SpaceId,
        purpose: Purpose,
        correlation: &str,
    ) -> Result<ordo_crypto::KeyLease, ordo_crypto::KeyError> {
        if purpose == Purpose::Read {
            self.reads.fetch_add(1, Ordering::SeqCst);
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

const WRITTEN: &str = "leave at the side door";

/// A database of this test's own, seeded, with `ord_4`'s memo sealed for
/// real under the fixture's custody. Every other memo is the seed's empty
/// envelope.
fn seeded(db: &str) -> (Fixture, Pg, Arc<Counting>) {
    let mut f = fixture();
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
    let lease = f
        .keys
        .release(&fixture_space(), Purpose::Write, "test")
        .unwrap();
    let envelope = f
        .keys
        .seal_at(
            &lease,
            "orders",
            "memo",
            Anchor::Row("ord_4"),
            WRITTEN.as_bytes(),
        )
        .unwrap();
    pg.exec(&format!(
        "UPDATE \"orders\" SET \"memo\" = '\\x{}'::bytea WHERE \"id\" = 'ord_4'",
        ordo_crypto::hex(&envelope)
    ))
    .unwrap();
    let counting = Arc::new(Counting {
        inner: f.keys.clone(),
        reads: AtomicUsize::new(0),
    });
    f.keys = counting.clone();
    (f, pg, counting)
}

/// The memo values a result carries, by placed date.
fn memos(rows: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = rows
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| {
            (
                r["placed"].as_str().unwrap_or_default().to_string(),
                r["memo"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

/// **The caller receives the plaintext, opened under a read release, and
/// nothing that looks like an envelope.** The join answers exactly what the
/// single source answers for the same rows.
#[test]
fn a_sealed_field_through_a_join_opens_to_what_was_written() {
    let (f, pg, keys) = seeded("ordo_sealjoin_open");

    let single = f
        .run_case(&pg, "orders[placed, memo]")
        .expect("the single source runs");
    let after_single = keys.reads.load(Ordering::SeqCst);
    assert!(after_single > 0, "opening took a read release");

    for src in [
        "customers[name] orders[placed, memo]",
        "orders[placed, memo] customers[name]",
        "withMemo[name, placed, memo]",
        "let m = orders[placed, memo, customer]\nm[placed, memo]",
        "orders(status == \"done\")[placed, memo]\nappend orders(status != \"done\")[placed, memo]",
    ] {
        let before = keys.reads.load(Ordering::SeqCst);
        let got = f
            .run_case(&pg, src)
            .unwrap_or_else(|e| panic!("{src}: {e}"));
        assert!(
            keys.reads.load(Ordering::SeqCst) > before,
            "{src}: no read release was taken, so nothing was opened"
        );
        assert_eq!(memos(&got), memos(&single), "{src}: {got:#}");
        for (_, m) in memos(&got) {
            assert!(!m.starts_with("\\x"), "{src} served an envelope: {m}");
        }
        assert!(
            memos(&got).contains(&("2026-02-20".into(), WRITTEN.into())),
            "{src}: {got:#}"
        );
    }

    // The renamed output opens too, under its new name.
    let rows = f
        .run_case(&pg, "customers[name] orders[placed, note: memo]")
        .expect("the renamed join runs");
    let notes: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["note"].as_str())
        .collect();
    assert!(notes.contains(&WRITTEN), "{rows:#}");
    assert!(notes.iter().all(|n| !n.starts_with("\\x")), "{rows:#}");
}
