//! The executing corpus. Every case below is transcribed from
//! `docs/composition-conformance.md` §4 — source text and expected rows both.
//!
//! The expected values are the document's, not the author's: no test here
//! asserts what the compiler happens to produce. A case is green only when
//! the rows Postgres returned are byte-identical, as canonical JSON, to what
//! the corpus says the language must return.

use ordo_conformance::{Fixture, Pg};
use serde_json::json;

/// The cases this slice can lower and run end to end.
const EXECUTED: &[&str] = &[
    "C001", "C002", "C003", "C004", "C005", "C006", "C007", "C008", "C009", "C010",
];
/// Rejects are cases too: C099–C110 assert that a spelling produces no plan.
const REJECTED: &[&str] = &[
    "C099", "C100", "C101", "C102", "C103", "C104", "C105", "C106", "C107", "C108", "C109", "C110",
];

fn fixture() -> Fixture {
    Fixture::load().expect("the market fixture compiles")
}

/// One database, rebuilt from generated DDL, shared by the cases in a run.
fn seeded() -> (Fixture, Pg) {
    let f = fixture();
    let pg = Pg::default();
    f.reset_once(&pg).expect("fixture DDL applies and seeds");
    (f, pg)
}

// ── stage 1/2: the fixture itself compiles ───────────────────────────────

#[test]
fn fixture_compiles_to_a_schema_graph() {
    let f = fixture();
    // Six original + the two platform entities (`members` and
    // `groupMembers`) + the guards-conformance cluster (projects,
    // projectMembers, tasks, taskNotes, boards, cards) + the acl-derived
    // boardsAccess, `folders` (the guarded tree), and the §6 write-rule pair
    // (ledgerEntries, coupons).
    assert_eq!(f.schema.entities.len(), 18);
    // 26 read ops + the write ops of every slice: 9 Stage-5 GC, 2 F4
    // sealed, 3 reparenting, 4 tree, 4 stated-target deletes, and 9 for §6
    // rules 8 and 9. memoDigest is the bounded-host read; renameTaskIf
    // carries the `rev` belief. Plus the three 8l null-sentinel ops
    // (addAnonCustomer, clearCustomerEmail, anonCustomers), and the three
    // `acl` DERIVES on `boards` — `grantBoardsAccess`,
    // `revokeBoardsAccess`, `setBoardsAccessLevel` — which the fixture
    // does not declare and no author writes.
    // Split, because the two halves are owed by different things: 64 are
    // authored-or-acl-derived, and the rest is the std library, which is a
    // function of the shapes and moves whenever they do.
    let authored = f
        .registry
        .names()
        .filter(|n| !n.starts_with("std."))
        .count();
    let std_derived = f.registry.names().filter(|n| n.starts_with("std.")).count();
    assert_eq!(authored, 64);
    // 85 became 101 when `list` derived (9k′): one more op on each of the
    // 16 non-platform entities, and none on the two platform ones.
    assert_eq!(std_derived, 101, "std derives per entity, not per author");
    assert_eq!(
        f.registry
            .names()
            .filter(|n| n.starts_with("std.") && n.ends_with(".list"))
            .count(),
        f.registry
            .names()
            .filter(|n| n.starts_with("std.") && n.ends_with(".get"))
            .count(),
        "`list` derives wherever `get` does — same entities, same row shape"
    );
    assert_eq!(f.registry.names().count(), authored + std_derived);
    for derived in [
        "grantBoardsAccess",
        "revokeBoardsAccess",
        "setBoardsAccessLevel",
    ] {
        assert!(
            f.registry.get(derived).is_some(),
            "`acl (read, write, admin)` derives `{derived}`"
        );
    }

    // The tree mark materialises `depth`; `catInfo` projects it, so it must
    // be a real column and not a parser artefact.
    let cats = f.schema.entity("categories").unwrap();
    assert!(cats.is_tree);
    assert!(cats.field("depth").is_some());

    // Ownership is on the ref, and there is exactly one.
    let items = f.schema.entity("orderItems").unwrap();
    assert!(matches!(
        items.field("order").unwrap().ty,
        ordo_schema::FieldTy::Ref { owned: true, .. }
    ));
}

#[test]
fn generated_ddl_carries_the_sealed_and_tree_columns() {
    let f = fixture();
    // A sealed field is bytea, and its blind index is a separate column —
    // never one text column doing both jobs.
    assert!(f.ddl.contains(r#""email" bytea"#), "{}", f.ddl);
    assert!(f.ddl.contains(r#""email$bidx" bytea"#), "{}", f.ddl);
    assert!(f.ddl.contains(r#""memo" bytea NOT NULL"#), "{}", f.ddl);
    // Uniqueness on a sealed field is uniqueness on the digest — and the
    // space leads it (9h). `UNIQUE ("email$bidx")` would be a
    // deployment-wide claim on a per-space value: the first tenant to
    // store an address would consume it for every other tenant in the
    // database, and the single null the constraint admits would be
    // consumed globally too.
    assert!(
        f.ddl.contains(r#"UNIQUE ("$space", "email$bidx")"#),
        "{}",
        f.ddl
    );
    assert!(f.ddl.contains(r#""depth" bigint NOT NULL"#), "{}", f.ddl);
    // Money is minor units, so integral: no float anywhere in the schema.
    assert!(
        !f.ddl.to_lowercase().contains("double precision"),
        "{}",
        f.ddl
    );
    assert!(!f.ddl.to_lowercase().contains("numeric"), "{}", f.ddl);
}

// ── stages 3/4: execution against seeded Postgres ────────────────────────

#[test]
fn c001_projection_subset_over_a_composed_source() {
    let (f, pg) = seeded();
    let got = f
        .run_case(&pg, "activeProducts[name, price]\nsort price")
        .expect("C001 runs");
    assert_eq!(
        got,
        json!([
            {"name": "hammer",  "price": 1500},
            {"name": "saw",     "price": 3000},
            {"name": "drill",   "price": 9500},
            {"name": "tune-up", "price": 12000},
            {"name": "consult", "price": 20000},
        ])
    );
}

#[test]
fn c002_omitted_projection_at_composition_depth_two() {
    // cheapActive -> activeProducts -> products. The deepest chain that
    // executes; unsorted, so anchor id order (R-C4).
    let (f, pg) = seeded();
    let got = f
        .run_case(&pg, "cheapActive($cap: 5000)")
        .expect("C002 runs");
    assert_eq!(
        got,
        json!([
            {"name": "hammer", "price": 1500, "category": "cat_3"},
            {"name": "saw",    "price": 3000, "category": "cat_3"},
        ])
    );
}

#[test]
fn c004_caller_condition_applies_after_the_callee_take() {
    // The callee yields ord_6/ord_4/ord_2; the filter drops ord_6 *after*
    // the take, so two rows survive rather than the take sliding down to a
    // third. Callee `-total` order is preserved.
    let (f, pg) = seeded();
    let got = f
        .run_case(&pg, "top3Orders(total < 15000)[placed, total]")
        .expect("C004 runs");
    assert_eq!(
        got,
        json!([
            {"placed": "2026-02-20", "total": 12000},
            {"placed": "2026-02-05", "total": 9500},
        ])
    );
}

#[test]
fn c008_computed_projection_over_output_fields() {
    let (f, pg) = seeded();
    let got = f
        .run_case(
            &pg,
            "cheapActive($cap: 10000)[name, sale: price - 500]\nsort sale",
        )
        .expect("C008 runs");
    assert_eq!(
        got,
        json!([
            {"name": "hammer", "sale": 1000},
            {"name": "saw",    "sale": 2500},
            {"name": "drill",  "sale": 9000},
        ])
    );
}

#[test]
fn c010_everything_minus_on_composed_output() {
    let (f, pg) = seeded();
    let got = f
        .run_case(
            &pg,
            "spendByCustomer($from: 2026-01-01)[*, -n]\nsort -spent",
        )
        .expect("C010 runs");
    assert_eq!(
        got,
        json!([
            {"customer": "cust_1", "spent": 16000},
            {"customer": "cust_2", "spent": 12000},
        ])
    );
}

#[test]
fn c003_parameter_binding_at_the_call_site() {
    let (f, pg) = seeded();
    let got = f
        .run_case(
            &pg,
            "doneOrders($from: 2026-02-01)[placed, total]\nsort placed",
        )
        .expect("C003 runs");
    assert_eq!(
        got,
        json!([
            {"placed": "2026-02-05", "total": 9500},
            {"placed": "2026-02-20", "total": 12000},
            {"placed": "2026-04-02", "total": 2000},
        ])
    );
}

#[test]
fn c005_binding_plus_condition_plus_dot_through_a_ref() {
    let (f, pg) = seeded();
    let got = f
        .run_case(
            &pg,
            "doneOrders($from: 2026-01-01, total > 5000)[placed, total, customer.name]\nsort -total",
        )
        .expect("C005 runs");
    assert_eq!(
        got,
        json!([
            {"placed": "2026-02-20", "total": 12000, "name": "Bo"},
            {"placed": "2026-02-05", "total": 9500,  "name": "Ada"},
        ])
    );
}

#[test]
fn c006_omitted_projection_on_a_grouped_callee() {
    let (f, pg) = seeded();
    let got = f
        .run_case(&pg, "spendByCustomer($from: 2026-01-01)\nsort -spent")
        .expect("C006 runs");
    assert_eq!(
        got,
        json!([
            {"customer": "cust_1", "spent": 16000, "n": 3},
            {"customer": "cust_2", "spent": 12000, "n": 1},
        ])
    );
}

#[test]
fn c007_rename_in_the_caller_projection() {
    let (f, pg) = seeded();
    let got = f
        .run_case(&pg, "activeProducts[label: name, price]\nsort price")
        .expect("C007 runs");
    assert_eq!(
        got,
        json!([
            {"label": "hammer",  "price": 1500},
            {"label": "saw",     "price": 3000},
            {"label": "drill",   "price": 9500},
            {"label": "tune-up", "price": 12000},
            {"label": "consult", "price": 20000},
        ])
    );
}

#[test]
fn c009_star_on_a_composed_source_preserves_callee_order() {
    let (f, pg) = seeded();
    // top3Orders carries a `take`, so it cannot merge — the caller reads a
    // settled row set, and with no sort of its own it keeps the callee's.
    let got = f.run_case(&pg, "top3Orders[*]").expect("C009 runs");
    assert_eq!(
        got,
        json!([
            {"placed": "2026-03-20", "total": 20000, "customer": "cust_3"},
            {"placed": "2026-02-20", "total": 12000, "customer": "cust_2"},
            {"placed": "2026-02-05", "total": 9500,  "customer": "cust_1"},
        ])
    );
}

#[test]
fn nulls_survive_the_wire_as_nulls() {
    // Not a numbered corpus case — a harness self-test. Every case above has
    // a fully populated result, so nothing else exercises the null path, and
    // a null silently arriving as the empty string would be indistinguishable
    // from a real value in a text column.
    let (f, pg) = seeded();
    let got = f.run_case(&pg, "catInfo").expect("catInfo runs");
    assert_eq!(
        got,
        json!([
            {"name": "goods",     "parent": null,    "depth": 0},
            {"name": "tools",     "parent": "cat_1", "depth": 1},
            {"name": "handtools", "parent": "cat_2", "depth": 2},
            {"name": "services",  "parent": null,    "depth": 0},
            {"name": "repair",    "parent": "cat_4", "depth": 1},
        ]),
        "unsorted rows come back in anchor id order (R-C4)"
    );
}

// ── the refusals, which matter as much as the passes ─────────────────────

#[test]
fn a_sealed_field_projects_host_side_but_never_reaches_the_database() {
    use ordo_plan::cost::{classify_with, CostClass};
    let f = fixture();

    // PROJECTING a sealed field is legal now: the payload column is
    // selected as an envelope and opened in the host.
    let p = f
        .compile("orderMemos[placed, memo]")
        .expect("a sealed projection compiles");
    assert!(
        p.sql.contains("\"memo\""),
        "the envelope is selected:\n{}",
        p.sql
    );
    assert_eq!(
        p.host_open,
        vec![("memo".to_string(), "orders".to_string(), "memo".to_string())],
        "and carries the obligation to open it, with its AAD"
    );
    // That makes it a HOST-class plan, not an sql-class one. Served as a
    // keyset page it opens one page's payloads per request, so the page
    // bounds the host work as a `take` would (ruling 2026-09-05;
    // std-library.md §5 rules 6 and 7, which register a sealed-inclusive
    // `std.E.list` and never an `unbounded-host` one).
    let q = f.bind_case("orderMemos[placed, memo]").unwrap();
    assert_eq!(
        classify_with(&q, Some(&f.schema)),
        CostClass::BoundedHost,
        "a paged sealed projection is bounded host work"
    );
    // A plan that cannot be paged opens payloads for rows nothing bounds;
    // `the_venue_gate_keys_on_where_the_work_happens` (corpus_stage5.rs)
    // holds that case. Bounding the rows bounds the host work with them.
    let q = f
        .bind_case("orderMemos[placed, memo]\nsort placed\ntake 5")
        .unwrap();
    assert_eq!(classify_with(&q, Some(&f.schema)), CostClass::BoundedHost);

    // What stays refused is the database reasoning about a sealed value:
    // it cannot compare, sort, or group what it cannot read.
    for src in [
        "orders(memo == \"x\")[placed]",
        "orders[placed]\nsort memo",
        "orders[memo]\ngroup memo { n: count }",
    ] {
        let e = f.compile(src).unwrap_err();
        assert!(e.contains("sealed_unsupported"), "{src}: {e}");
    }
    // customerByEmail flipped when F1 landed: equality on the sealed-
    // INDEXED field compiles to a digest comparison — the payload column
    // still never appears in the plan.
    let p = f
        .compile("customerByEmail($e: \"ada@example.com\")[name]")
        .expect("blind-index equality compiles");
    assert!(p.sql.contains("email$bidx"), "{}", p.sql);
    assert!(
        !p.sql.contains("\"email\""),
        "payload column must not appear:\n{}",
        p.sql
    );
}

/// The seed files carry `@BIDX(entity,field,value)@` placeholders rather
/// than hex, and `Fixture::rendered_seed` resolves each through
/// `KeyProvider::blind_index` under the fixture's own space. This is what
/// makes the property checkable: the RENDERED seed must carry exactly the
/// digests the provider computes, and it must carry different ones in a
/// different space.
///
/// It used to check hex-in-file against the provider. The space column
/// (9h) retired that shape: a blind index derives under `SMK(space)`, the
/// seed is now stamped into more than one space, and literal bytes could
/// only ever have been right for one of them.
///
/// The NULL rows are the point of 8l. `cust_3` and `ppl_6` have no email,
/// and their digests are the reserved null sentinel — not SQL NULL.
#[test]
fn seeded_blind_digests_are_regenerable() {
    use ordo_crypto::BlindValue;

    let market = fixture();
    let wide = Fixture::load_wide().expect("wide fixture compiles");
    for (f, entity, rows) in [
        (
            &market,
            "customers",
            &[
                Some("ada@example.com"),
                Some("bo@example.com"),
                None, // cust_3 — the sentinel
            ][..],
        ),
        (
            &wide,
            "people",
            &[
                Some("ada@example.com"),
                Some("bo@example.com"),
                Some("cy@example.com"),
                Some("di@example.com"),
                Some("eli@example.com"),
                None, // ppl_6 — the one null the unique field admits
            ][..],
        ),
    ] {
        let rendered = f.rendered_seed().expect("seed renders");
        let other = f.in_space("twin");
        let rendered_twin = other.rendered_seed().expect("seed renders in a twin space");
        for v in rows {
            let hex = ordo_crypto::hex(
                &f.keys
                    .blind_index(f.space(), entity, "email", BlindValue::from(*v))
                    .expect("digest"),
            );
            assert!(
                rendered.contains(&hex),
                "{entity}.email {v:?} digests to {hex}, which the rendered seed does not carry"
            );
            // The same address, seeded into another space, is a different
            // digest — which is why the file cannot hold hex.
            assert!(
                !rendered_twin.contains(&hex),
                "{entity}.email {v:?} digests identically in two spaces, so the blind index \
                 is not per-space and `UNIQUE (space, hmac)` is carrying the whole claim"
            );
        }
        // And the sentinel is not spellable: the rendered seed must not
        // contain the digest of the literal text "null" where a null row's
        // digest belongs.
        let word = ordo_crypto::hex(
            &f.keys
                .blind_index(f.space(), entity, "email", BlindValue::Text("null"))
                .expect("digest"),
        );
        assert!(
            !rendered.contains(&word),
            "a null row must digest the reserved sentinel, not the word"
        );
    }
}

#[test]
fn a_take_callee_is_not_merged_into() {
    // C004's whole point: the caller's condition must apply after the take,
    // so the plan must contain a subquery rather than one flat WHERE.
    let f = fixture();
    let p = f
        .compile("top3Orders(total < 15000)[placed, total]")
        .unwrap();
    assert!(p.sql.contains("LIMIT 3"), "{}", p.sql);
    let where_after_limit = p.sql.rfind("WHERE").unwrap() < p.sql.find("LIMIT 3").unwrap();
    assert!(
        !where_after_limit,
        "the caller filter must sit outside the take:\n{}",
        p.sql
    );
}

#[test]
fn unsupported_constructs_name_themselves() {
    let f = fixture();
    // `under` landed in P4 — toolDeals (depth-3, under inside the callee)
    // now compiles; this tripwire flipped by design when the feature did.
    assert!(f.compile("toolDeals($cap: 5000, $root: \"cat_1\")").is_ok());
    // Two-source inference landed in P5 — C011's shape compiles now.
    assert!(f
        .compile("products[name]  productRevenue[sold, revenue]")
        .is_ok());
    // The minimal-subgraph rule landed too: a 3-source chain infers
    // vendors <- products <- orderItems with no spelling.
    assert!(f
        .compile("vendors[name]  products[]  orderItems[qty]")
        .is_ok());
    // And a 2-source TRANSITIVE pair routes through the bridge the
    // compiler inserts itself (DL ~1582: `projects <- tasks <- receipts`).
    assert!(f.compile("vendors[name]  orderItems[amount]").is_ok());
}

#[test]
fn unknown_names_are_rejected_with_the_corpus_error_class() {
    let f = fixture();
    let e = f.compile("nosuchthing[a]").unwrap_err();
    assert!(e.contains("unknown_source"), "{e}");
    // Projection position carries its own corpus class (C080).
    let e = f.compile("activeProducts[nosuchfield]").unwrap_err();
    assert!(e.contains("projection_unknown_field"), "{e}");
}

/// Sources for every case this slice executes, keyed by case id — the
/// shared table, so `tests/goldens.rs` pins the SQL of exactly the cases
/// this file executes.
fn covered_sources() -> Vec<(&'static str, &'static str)> {
    ordo_conformance::covered_case_sources()
}

#[test]
fn every_ordering_is_total() {
    // Without a tiebreak the row order is the server's choice, and a corpus
    // that compares byte-for-byte would be testing luck. This asserts on the
    // IR rather than on SQL text, per §2.4: compile assertions never regex.
    let f = fixture();
    for (id, src) in covered_sources() {
        let q = f.bind_case(src).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(!q.tiebreak.is_empty(), "{id} has no ordering tiebreak");
    }

    // The case that caught this: a grouped callee read through a subquery
    // must carry its group keys outward as the outer tiebreak, or the outer
    // ORDER BY is `spent DESC` alone and ties come back in server order.
    let p = f
        .compile("spendByCustomer($from: 2026-01-01)\nsort -spent")
        .unwrap();
    let outer = p.sql.rsplit_once("ORDER BY").unwrap().1;
    assert_eq!(
        outer.matches("NULLS LAST").count(),
        2,
        "outer order not total:\n{}",
        p.sql
    );

    // And an unsorted row pipeline still gets the anchor id — under the
    // pinned collation, because `id` is a text column and an id ordering
    // is a text ordering (9i / G11).
    let p = f.compile("activeProducts[name, price]").unwrap();
    assert!(
        p.sql.contains(r#""id" COLLATE "C" ASC"#),
        "no anchor tiebreak in:\n{}",
        p.sql
    );
}

// ── section M: synonym spellings must fail (C099–C110) ───────────────────

/// `composition-conformance.md` §4 M, verbatim. "Each must fail to parse;
/// none may parse to something else." Per §2.4 stage 1 a case may assert its
/// class at parse or at the next stage, so bind-time rejection counts — what
/// does not count is producing a plan.
const REJECTS: &[(&str, &str)] = &[
    ("C099 empty parens", "activeProducts()[name]"),
    ("C100 SQL spelling", "from activeProducts select name"),
    (
        "C101 method chain",
        "activeProducts.filter(price < 5000)[name]",
    ),
    (
        "C102 `=` for binding colon",
        "doneOrders($from = 2026-01-01)[total]",
    ),
    (
        "C103 binding without `$`",
        "doneOrders(from: 2026-01-01)[total]",
    ),
    ("C104 alias spelling", "activeProducts as ap[name]"),
    (
        "C105 bare non-boolean fields",
        "activeProducts(name, price)",
    ),
    ("C106 two projection groups", "activeProducts[name][price]"),
    (
        "C107 two paren groups",
        "cheapActive($cap: 5000)(price > 2000)[name]",
    ),
    (
        "C108 numeric in take brackets",
        "activeProducts[name]\ntake[2]",
    ),
    ("C109 join spelling", "activeProducts join vendors [name]"),
    (
        "C110 trailing comma",
        "doneOrders($from: 2026-01-01,)[total]",
    ),
];

#[test]
fn synonym_spellings_all_fail() {
    let f = fixture();
    let mut leaked = Vec::new();
    let mut unclassed = Vec::new();
    for (id, src) in REJECTS {
        match f.compile(src) {
            Ok(p) => leaked.push(format!("{id}\n  source: {src}\n  planned: {}", p.sql)),
            // §0's opening line is "Every reject case names one class", and
            // this suite asserted only that nothing planned. That is the gap
            // the 9r ruling existed to close, in the file that declares the
            // rule: C105 spent an unknown time answering
            // `non_boolean_condition` while three places in the document said
            // `parse_error`, and nothing here could tell.
            Err(e) => {
                // `Fixture::compile` prefixes the stage (`bind:`, `plan:`)
                // and brackets the class, so the class is the bracketed
                // token where there is one — deliberately not "the text
                // before the first colon", which is the stage and would have
                // made this check pass on every case for the wrong reason.
                let class = extract_class(&e);
                if !INVENTORY.contains(&class.as_str()) {
                    unclassed.push(format!("{id}: `{class}` — {e}"));
                }
            }
        }
    }
    assert!(
        leaked.is_empty(),
        "these must fail and instead produced a plan:\n\n{}",
        leaked.join("\n\n")
    );
    assert!(
        unclassed.is_empty(),
        "these failed with a class that is in no inventory in \
         `composition-conformance.md` §0:\n  {}",
        unclassed.join("\n  ")
    );
}

/// The class out of a harness error string.
///
/// Two shapes reach here: `parse_error: …` from the parser, and
/// `bind: [non_boolean_condition] …` where the harness prefixes the stage.
/// Both are handled by shape rather than by position, because the message
/// body contains brackets of its own (`orders[total]`) and "the first
/// bracketed thing" picked those up — a check that reads the wrong token is
/// worse than no check, since it passes for the wrong reason.
fn extract_class(e: &str) -> String {
    let is_class = |s: &str| {
        !s.is_empty()
            && s.contains('_')
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
    };
    if let Some(head) = e.split(':').next() {
        let head = head.trim();
        if is_class(head) {
            return head.to_string();
        }
    }
    if let Some(a) = e.find('[') {
        if let Some(b) = e[a..].find(']') {
            let cand = &e[a + 1..a + b];
            if is_class(cand) {
                return cand.to_string();
            }
        }
    }
    String::new()
}

/// Every class `composition-conformance.md` §0 names, as the harness sees
/// them. Read from the document at build time rather than retyped, so the
/// two cannot drift — which is the failure this whole area keeps producing.
const INVENTORY_DOC: &str = include_str!("../../../docs/composition-conformance.md");

static INVENTORY: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
    let mut out = Vec::new();
    for line in INVENTORY_DOC.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("| `") {
            if let Some(end) = rest.find('`') {
                let c = &rest[..end];
                if c.contains('_') && !c.contains(' ') {
                    out.push(c);
                }
            }
        }
    }
    assert!(
        out.len() > 30,
        "the §0 tables did not parse ({} classes) — this check would pass \
         vacuously against a document it can no longer read",
        out.len()
    );
    out
});

#[test]
fn borrowed_spellings_say_what_to_write_instead() {
    // A rejection that says "unknown source" teaches nothing. Closedness is
    // only useful if the error names the Ordo spelling.
    let f = fixture();
    for (src, want) in [
        ("orders join customers", "matches are inferred"),
        ("select total from orders", "name the source"),
        ("activeProducts[name]\nlimit 3", "`take n`"),
        ("activeProducts[name]\norder by price", "sort"),
        ("orders union customers", "`append`"),
        ("activeProducts as ap[name]", "rename in the projection"),
    ] {
        let e = f
            .compile(src)
            .expect_err(&format!("`{src}` must be rejected"));
        assert!(
            e.contains(want),
            "`{src}` was rejected as:\n  {e}\nexpected it to mention: {want}"
        );
    }
}

/// The **whole** composition corpus's coverage, across every test binary
/// that runs one of its cases — not just this file's.
///
/// The number this test used to print was 22/120, counting only the cases
/// declared in *this* file. That was true of the file and false of the
/// tree: `corpus_p0.rs` drives a table of forty-odd more, and checklist 9o
/// was written against the old number. A coverage report that undercounts
/// is as misleading as one that overcounts, so the count is now derived
/// from the sibling sources rather than restated by hand.
///
/// The derivation is mechanical and conservative — a case counts only
/// where it appears in that file's *executing* marker, never where it is
/// merely mentioned in prose:
///
/// - `corpus.rs` (here): the `EXECUTED` and `REJECTED` lists;
/// - `corpus_p0.rs`: the `id: "Cxxx"` field of its case table;
/// - `corpus_c_extra.rs`: its `fn cNNN_…` test names.
///
/// Cases asserted only inside prose-named helper tests (`p3_rejects`'
/// inline C065/C108, for instance) are *not* counted, so the printed
/// number stays a floor.
#[test]
fn coverage_is_reported_not_implied() {
    let total = 120;

    let mut covered: std::collections::BTreeSet<String> = EXECUTED
        .iter()
        .chain(REJECTED.iter())
        .map(|s| s.to_string())
        .collect();

    // `corpus_p0.rs`'s table: every `id: "Cxxx"` is a case that executes.
    let p0 = include_str!("corpus_p0.rs");
    for (i, _) in p0.match_indices("id: \"C") {
        let id: String = p0[i + 5..].chars().take(4).collect();
        if id.len() == 4 && id[1..].chars().all(|c| c.is_ascii_digit()) {
            covered.insert(id);
        }
    }
    // …and its inline convention for the cases that need a hand-written
    // assertion instead of a table row: a `// Cxxx — ` marker on the line
    // above. Every one of them sits directly above a real assertion; the
    // marker is that file's way of keying an inline case to the corpus.
    for (i, _) in p0.match_indices("// C") {
        let digits: String = p0[i + 4..].chars().take(3).collect();
        let after = p0[i + 7..].chars().take(3).collect::<String>();
        if digits.chars().all(|c| c.is_ascii_digit()) && after.starts_with(" — ") {
            covered.insert(format!("C{digits}"));
        }
    }

    // `corpus_c_extra.rs`'s own `EXECUTED` list — the cases it runs in
    // full. Its `PARTIAL` cases are deliberately not counted: a half-run
    // case is not a covered case.
    let extra = include_str!("corpus_c_extra.rs");
    let list = extra
        .split_once("const EXECUTED: &[&str] = &[")
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(body, _)| body)
        .expect("corpus_c_extra.rs declares an EXECUTED list");
    for (i, _) in list.match_indices("\"C") {
        let id: String = list[i + 1..].chars().take(4).collect();
        if id.len() == 4 && id[1..].chars().all(|c| c.is_ascii_digit()) {
            covered.insert(id);
        }
    }

    let missing: Vec<String> = (1..=total)
        .map(|n| format!("C{n:03}"))
        .filter(|id| !covered.contains(id))
        .collect();

    println!(
        "composition corpus coverage: {}/{total} documented cases execute somewhere \
         in the tree\n  this file: executed {} · rejected {}\n  still uncovered: {}",
        covered.len(),
        EXECUTED.join(", "),
        REJECTED.join(", "),
        missing.join(", ")
    );
    assert!(covered.len() < total, "coverage claim must stay honest");
    // The floor the derivation must clear. If a sibling file's marker
    // convention changes, this fails rather than silently reporting a
    // collapse in coverage as good news.
    assert!(
        covered.len() >= 60,
        "the coverage derivation found only {} cases — a sibling file's marker \
         convention has probably changed",
        covered.len()
    );
}

/// A composed callee's sealed output is part of its result shape, and a
/// caller sourcing it plainly must get it.
///
/// It used to vanish: the omitted-projection rule filtered sealed fields
/// as the open skeleton, which is correct over an ENTITY (naming the field
/// is how a caller opts into the host-class cost) and wrong over a callee
/// that already named it. `orderMemos` answered 200 with `memo` simply
/// absent from every row — no refusal, no key release — while the catalog
/// advertised the column. A caller cannot tell that from a null.
#[test]
fn a_callees_sealed_output_survives_an_omitted_projection() {
    let f = fixture();

    // The callee names `memo`, so sourcing it plainly yields `memo`.
    let q = f.bind_case("orderMemos").expect("binds");
    let names: Vec<&str> = q.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["placed", "total", "memo"]);
    assert_eq!(
        q.host_open.len(),
        1,
        "and carries the obligation to open it: {:?}",
        q.host_open
    );

    // The entity rule is untouched: a bare entity source still yields the
    // OPEN skeleton, so a sealed column is not served to someone who never
    // asked for it.
    let q = f.bind_case("orders").expect("binds");
    assert!(
        !q.outputs.iter().any(|o| o.name == "memo"),
        "`orders` alone must stay the open skeleton: {:?}",
        q.outputs.iter().map(|o| &o.name).collect::<Vec<_>>()
    );
    assert!(q.host_open.is_empty(), "and cost nothing host-side");
}
/// **9i / G11 — text ordering is pinned, and the pin is what makes a page
/// mean anything.**
///
/// Nothing here checks that the SQL contains the word `COLLATE`; that
/// would pass just as well if the pin were spelled wrong. It runs against
/// a database whose own collation is NOT `C` and asserts the exact order
/// the engine promises, over values chosen because `C` and `en_US.UTF-8`
/// disagree about them: case (`B` before `a` under `C`, after it under
/// `en_US`) and punctuation (`en_US` largely ignores the space and the
/// hyphen at the first level; `C` compares the bytes).
///
/// The ids are assigned so that id order matches the *database's* order,
/// not the pinned one — so a regression that dropped the pin could not
/// pass by accident on the tiebreak.
#[test]
fn text_ordering_is_pinned_and_deterministic() {
    let f = fixture();
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_collation");
    let pg = Pg {
        db: "ordo_collation".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");

    // The test is only evidence if the deployment disagrees with the pin.
    let coll = pg
        .query("SELECT datcollate FROM pg_database WHERE datname = current_database()")
        .expect("collation")[0][0]
        .clone()
        .unwrap_or_default();
    assert_ne!(
        coll, "C",
        "this database already collates as C, so it cannot show the pin doing anything"
    );

    pg.exec(r#"DELETE FROM "orderItems"; DELETE FROM "products";"#)
        .expect("clear");
    pg.exec(
        r#"INSERT INTO "products" ("$space","id","vendor","category","name","price","active") VALUES
             ('conformance','prod_x1','vend_1','cat_1','a',   100, true),
             ('conformance','prod_x2','vend_1','cat_1','a b', 100, true),
             ('conformance','prod_x3','vend_1','cat_1','a-b', 100, true),
             ('conformance','prod_x4','vend_1','cat_1','ab',  100, true),
             ('conformance','prod_x5','vend_1','cat_1','B',   100, true),
             ('conformance','prod_x6','vend_1','cat_1','Z',   100, true)"#,
    )
    .expect("insert");

    // The pinned answer: byte order. `B` and `Z` lead because uppercase
    // ASCII is numerically below lowercase; the space sorts before the
    // hyphen, which sorts before `b`.
    let pinned = json!([
        { "name": "B" }, { "name": "Z" }, { "name": "a" },
        { "name": "a b" }, { "name": "a-b" }, { "name": "ab" },
    ]);
    assert_eq!(
        f.run_case(&pg, "products[name]\nsort name").expect("sorts"),
        pinned,
        "text ordering must be the engine's pin, not the deployment's locale"
    );

    // And the deployment really would have answered differently: the same
    // query with the pin stripped is the `en_US.UTF-8` order.
    let plan = f.compile("products[name]\nsort name").expect("plans");
    let unpinned = plan.sql.replace(r#" COLLATE "C""#, "");
    assert_ne!(unpinned, plan.sql, "the plan must carry the pin");
    let rows = pg.query(&unpinned).expect("runs unpinned");
    let got: Vec<&str> = rows.iter().map(|r| r[0].as_deref().unwrap()).collect();
    assert_eq!(
        got,
        ["a", "a b", "a-b", "ab", "B", "Z"],
        "the unpinned query takes the database's collation — which is the \
         whole hazard: two deployments, two orders, same plan"
    );

    // Keyset pagination is where an unpinned order stops being cosmetic.
    // Walk every page and assert the walk visits each row exactly once, in
    // the pinned order: a seek predicate evaluated under a different
    // collation than the ORDER BY skips or repeats rows here.
    let q = f.bind_case("products[name]\nsort name").expect("binds");
    let mut cursor: Option<Vec<Option<String>>> = None;
    let mut walked: Vec<String> = Vec::new();
    for _ in 0..10 {
        let paged =
            ordo_plan::plan_paged(&q, &f.plan_space(), 2, cursor.as_deref()).expect("pages");
        let rows = pg.query(&paged.sql).expect("page runs");
        if rows.is_empty() {
            break;
        }
        let names = ordo_plan::order_key_outputs(&q);
        let last = rows.last().unwrap().clone();
        for r in &rows {
            walked.push(r[0].clone().unwrap_or_default());
        }
        if (rows.len() as i64) < 2 {
            break;
        }
        // The cursor is the last row's order keys: projected ones by
        // output position, the rest from the hidden trailing columns.
        let mut hidden = last[paged.outputs.len()..].iter().cloned();
        cursor = Some(
            names
                .iter()
                .map(|n| match n {
                    Some(name) => paged
                        .outputs
                        .iter()
                        .position(|(o, _)| o == name)
                        .and_then(|p| last.get(p).cloned())
                        .flatten(),
                    None => hidden.next().flatten(),
                })
                .collect(),
        );
    }
    assert_eq!(
        walked,
        ["B", "Z", "a", "a b", "a-b", "ab"],
        "the keyset walk must visit every row exactly once, in the pinned \
         order — no skips, no repeats"
    );
}

/// **G12's ruling, proved against a real server.** `hash(x)` is back
/// (checklist 9t), and DL §3's promise about it is *bit*-stability across
/// platform versions — so the thing that has to be tested is not "it
/// returns an int" but "it returns THESE ints, and the two venues that can
/// compute it agree digit for digit".
///
/// Three claims, and each is a different way the promise breaks:
///
/// 1. **Frozen values.** The expected integers below are derived from the
///    published SHA-256 test vectors, not from running the code. If the
///    algorithm, the truncation, the byte order, or the sign mask ever
///    changes, these numbers move — and every rollout stripe a deployment
///    has ever computed moves with them.
/// 2. **Two venues, one answer.** `ordo_plan::hash64` and the SQL the
///    planner emits are separate implementations of the same rule, which
///    is exactly the shape that drifts. They are compared over the same
///    inputs, including non-ASCII and the `\x`-prefixed string that the
///    tidier-looking `::bytea` spelling would silently hash as raw bytes.
/// 3. **Non-negative.** `int` is signed, and `mod` follows the dividend's
///    sign, so an unmasked digest would put half of every rollout stripe
///    on negative buckets.
#[test]
fn hash_is_bit_stable_and_agrees_across_venues() {
    let f = fixture();
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec("CREATE DATABASE ordo_10d_hash");
    let pg = Pg {
        db: "ordo_10d_hash".into(),
        ..Pg::default()
    };
    f.reset(&pg).expect("seeded");

    // (1) The frozen vectors. SHA-256("a") begins ca978112ca1bbdca; the
    // top bit is set, so the mask is doing visible work here — without it
    // this value is negative.
    assert_eq!(ordo_plan::hash64("a"), 0x4a97_8112_ca1b_bdca);
    // SHA-256("") begins e3b0c44298fc1c14: masked, 0x63b0c44298fc1c14.
    assert_eq!(ordo_plan::hash64(""), 0x63b0_c442_98fc_1c14);
    // SHA-256("abc") begins ba7816bf8f01cfea: the top bit is clear, so the
    // mask is the identity and the value is the digest prefix as written.
    assert_eq!(ordo_plan::hash64("abc"), 0x3a78_16bf_8f01_cfea);

    // (2) The two venues, over values chosen to break a careless lowering.
    // `\x4142` is the one that matters: `'\x4142'::text::bytea` is the two
    // bytes `AB`, so a lowering that casts instead of converting hashes a
    // six-character id as a two-byte one.
    let cases = ["a", "", "abc", "task_1", "Åsa", "日本語", r"\x4142", "a b"];
    pg.exec(r#"DELETE FROM "orderItems"; DELETE FROM "products";"#)
        .expect("clear");
    for (i, v) in cases.iter().enumerate() {
        let lit = v.replace('\'', "''");
        pg.exec(&format!(
            r#"INSERT INTO "products" ("$space","id","vendor","category","name","price","active")
                 VALUES ('conformance','prod_h{i}','vend_1','cat_1','{lit}',100,true)"#
        ))
        .expect("insert");
    }
    let got = f
        .run_case(&pg, "products[name, b: hash(name)]\nsort name")
        .expect("hash plans, lowers, and runs");
    let rows = got.as_array().expect("rows");
    assert_eq!(rows.len(), cases.len(), "every row came back: {got}");
    for r in rows {
        let name = r["name"].as_str().expect("a name");
        let b = r["b"]
            .as_i64()
            .expect("hash is an int, not a float or a string");
        assert_eq!(
            b,
            ordo_plan::hash64(name),
            "the two venues disagree on `{name}` — the SQL says {b}"
        );
        // (3) Non-negative, so `mod(hash(x), n)` is a bucket and not a
        // signed remainder straddling zero.
        assert!(b >= 0, "`{name}` hashed to {b}");
    }

    // The `\x` hazard, stated as its own assertion rather than left to be
    // inferred from the loop: the two spellings really do differ, so the
    // loop above is testing something.
    let raw = pg
        .query(r#"SELECT encode('\x4142'::text::bytea, 'hex')"#)
        .expect("cast runs")[0][0]
        .clone()
        .unwrap();
    assert_eq!(raw, "4142", "the ::bytea spelling reinterprets the escape");
    assert_ne!(
        ordo_plan::hash64(r"\x4142"),
        ordo_plan::hash64("AB"),
        "and the ruled spelling keeps them distinct"
    );

    // A null hashes to null, like every other one-argument function in the
    // list — the whole expression propagates it, so nothing had to be
    // written to make it true. `categories.parent` is the nullable ref the
    // seed already has roots on.
    let cats = f
        .run_case(&pg, "categories[name, b: hash(parent)]\nsort name")
        .expect("hash over a nullable ref plans and runs");
    let rows = cats.as_array().expect("rows");
    assert!(
        rows.iter().any(|r| r["b"].is_null()),
        "a root category's null parent must hash to null: {cats}"
    );
    assert!(
        rows.iter()
            .filter(|r| !r["b"].is_null())
            .all(|r| r["b"].as_i64().is_some_and(|b| b >= 0)),
        "and every non-null one is still a non-negative bucket: {cats}"
    );
}
