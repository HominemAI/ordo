//! Reported fix 25at (`let-group-join`): a grouped `let` joins back to its
//! source.
//!
//! What was seen: `let n = vendors[id] invoices[] group vendors.id { … }`
//! read back as `vendors[name] n[…]` was refused with `no_match_path`, and
//! with `fan_out` when two such lets grouped by the child's ref. The recipe
//! of modeling-guidance.md §C7 and the `vendorsBillingLag` block of
//! syntax-examples.md could not be written.
//!
//! The cause was twofold. A group key that is an entity's `id` kept no
//! provenance, so match inference had no edge from the let to the entity.
//! And every provenance edge counted as a many-branch, so two lets on one
//! entity looked like two fanning children. A grouped let keyed by an id
//! (or by a lone ref) holds one row per key, so its join back is one-to-one.
//!
//! A regression reads as either refusal again, or as numbers multiplied by
//! a join that fans out: Ada's four orders counted as eight.
//!
//! The executing tests need the conformance cluster
//! (`scripts/conformance-pg.sh start`). Each owns its own database.

use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use serde_json::Value;

fn fixture() -> Fixture {
    Fixture::from_source(FIXTURE, SEED).expect("the fixture compiles")
}

fn seeded(db: &str) -> (Fixture, Pg) {
    let f = fixture();
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
    (f, pg)
}

/// Each row as its fields rendered to text, in the order the op returns them.
fn rows_of(rows: &Value, fields: &[&str]) -> Vec<Vec<String>> {
    rows.as_array()
        .expect("rows")
        .iter()
        .map(|r| {
            fields
                .iter()
                .map(|f| match &r[*f] {
                    Value::String(s) => s.clone(),
                    Value::Null => "null".into(),
                    v => v.to_string(),
                })
                .collect()
        })
        .collect()
}

/// §C7's shape on the market fixture: two counts of one customer's orders,
/// each grouped by `customers.id` in its own let, joined back and compared.
const STALLED: &str = "let all  = customers[id]  orders[]
           group customers.id { n: count }
let done = customers[id]  orders(status == \"done\")[]
           group customers.id { d: count }
customers[name]  all[n]  done[d]
sort name";

#[test]
fn two_lets_grouped_by_the_customer_id_count_each_customer_once() {
    let (f, pg) = seeded("ordo_letjoin_counts");
    let rows = f.run_case(&pg, STALLED).expect("runs");
    assert_eq!(
        rows_of(&rows, &["name", "n", "d"]),
        [["Ada", "4", "3"], ["Bo", "2", "1"], ["Cy", "1", "0"]],
        "{rows:#}"
    );
}

/// Each let starts from `customers[id]`, which keeps customers with no
/// matching orders: the empty bucket counts 0 (spec, "Multi-source join
/// semantics"), so the joined value is 0, never null.
#[test]
fn a_customer_with_no_matching_orders_reads_zero_not_null() {
    let (f, pg) = seeded("ordo_letjoin_zero");
    let rows = f.run_case(&pg, STALLED).expect("runs");
    let cy = rows_of(&rows, &["name", "d"])
        .into_iter()
        .find(|r| r[0] == "Cy")
        .expect("Cy");
    assert_eq!(cy[1], "0", "{rows:#}");
}

#[test]
fn the_compared_lets_keep_only_the_stalled_customer() {
    let (f, pg) = seeded("ordo_letjoin_compare");
    let src = format!("{STALLED}\n| coalesce(d, 0) * 2 < coalesce(n, 0)");
    let rows = f.run_case(&pg, &src).expect("runs");
    assert_eq!(
        rows_of(&rows, &["name", "n", "d"]),
        [["Cy", "1", "0"]],
        "{rows:#}"
    );
}

/// Two different many-sides of `vendors`, each settled in its own let: the
/// live products, and the order lines sold through every product.
#[test]
fn two_lets_over_different_children_sum_each_vendor_once() {
    let (f, pg) = seeded("ordo_letjoin_sums");
    let rows = f
        .run_case(
            &pg,
            "let stock = vendors[id]  products(active == true)[]
            group vendors.id { live: count }
let sales = vendors[id]  products[]  orderItems[amount]
            group vendors.id { sold: sum amount }
vendors[name]  stock[live]  sales[sold]
sort name",
        )
        .expect("runs");
    // Acme sold 1500 + 9500 + 1500, Blythe 3000 + 12000 + 3000, Corvid
    // 20000 + 2000 (in cents). A fanning join would multiply each sum by
    // the vendor's live product count.
    assert_eq!(
        rows_of(&rows, &["name", "live", "sold"]),
        [
            ["Acme", "2", "12500"],
            ["Blythe", "2", "18000"],
            ["Corvid", "1", "22000"]
        ],
        "{rows:#}"
    );
}

/// A let keyed by the child's ref joins back the same way.
#[test]
fn a_let_grouped_by_the_ref_joins_back_once_per_vendor() {
    let (f, pg) = seeded("ordo_letjoin_ref");
    let rows = f
        .run_case(
            &pg,
            "let n = products[vendor] group vendor { k: count }
vendors[name]  n[k]
sort name",
        )
        .expect("runs");
    assert_eq!(
        rows_of(&rows, &["name", "k"]),
        [["Acme", "2"], ["Blythe", "2"], ["Corvid", "2"]],
        "{rows:#}"
    );
}

/// The guard did not loosen: two many-sides listed in one pipeline, or two
/// lets that keep one row per child, still fan out.
#[test]
fn a_join_that_fans_out_is_still_refused() {
    let f = fixture();
    for src in [
        "projects[name]  projectMembers[canWrite]  tasks[title]",
        "let p = products[vendor, name]
let q = products[vendor, price] group vendor, price { m: count }
vendors[vname: name]  p[name]  q[m]",
    ] {
        let e = f.bind_case(src).expect_err(src);
        assert!(e.contains("fan_out"), "{src}: {e}");
    }
}
