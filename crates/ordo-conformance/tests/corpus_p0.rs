//! Stage 1, phase P0 — the corpus harvest.
//!
//! The two decomposition agents disagreed about these cases: the spec-side
//! read classified them as blocked by missing machinery, the code-side read
//! said the machinery already exists and they only need transcription. This
//! file settles it empirically. Sources and expected rows are transcribed
//! verbatim from `composition-conformance.md` §4 — the expected values are
//! the document's, not the compiler's.
//!
//! Row order is asserted for every case: §2.4 stage 4 makes row order part
//! of the language's contract, and the corpus states each case's order.

use ordo_conformance::{Fixture, Pg};
use serde_json::{json, Value};

fn seeded() -> (Fixture, Pg) {
    let f = Fixture::load().expect("market fixture compiles");
    let pg = Pg::named("ordo_conf_p0");
    f.reset_once(&pg).expect("seeded");
    (f, pg)
}

struct Case {
    id: &'static str,
    src: &'static str,
    params: &'static [(&'static str, &'static str)],
    cols: &'static [&'static str],
    rows: Value,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            id: "C016", // defaulted param omitted
            src: "recentOrders[placed, total]",
            params: &[],
            cols: &["placed", "total"],
            rows: json!([["2026-03-01", 3000], ["2026-03-15", 1500], ["2026-03-20", 20000], ["2026-04-02", 2000]]),
        },
        Case {
            id: "C017", // defaulted param bound explicitly
            src: "recentOrders($since: 2026-03-16)[placed, total]",
            params: &[],
            cols: &["placed", "total"],
            rows: json!([["2026-03-20", 20000], ["2026-04-02", 2000]]),
        },
        Case {
            id: "C021", // condition before binding — entry order immaterial
            src: "doneOrders(total > 5000, $from: 2026-01-01)[placed, total]",
            params: &[],
            cols: &["placed", "total"],
            rows: json!([["2026-02-05", 9500], ["2026-02-20", 12000]]),
        },
        Case {
            id: "C022", // money param from a bare integer literal
            src: "cheapActive($cap: 3000)[name, price]",
            params: &[],
            cols: &["name", "price"],
            rows: json!([["hammer", 1500]]),
        },
        Case {
            id: "C030", // condition on an aggregate output
            src: "spendByCustomer($from: 2026-01-01, spent > 12000)[customer, spent]",
            params: &[],
            cols: &["customer", "spent"],
            rows: json!([["cust_1", 16000]]),
        },
        Case {
            id: "C031", // ref-output equality with a harness param
            src: "productRevenue(product == $p)[revenue]",
            params: &[("p", "\"prod_3\"")],
            cols: &["revenue"],
            rows: json!([[6000]]),
        },
        Case {
            id: "C032", // dot-through condition on a ref output (R-C6)
            src: "productRevenue(product.vendor == $v)[product.name, revenue]",
            params: &[("v", "\"vend_1\"")],
            cols: &["name", "revenue"],
            rows: json!([["hammer", 3000], ["drill", 9500]]),
        },
        Case {
            id: "C033", // `in` membership over a ref output
            src: "top3Orders(customer in ($a, $b))[total]",
            params: &[("a", "\"cust_1\""), ("b", "\"cust_2\"")],
            cols: &["total"],
            rows: json!([[12000], [9500]]),
        },
        Case {
            id: "C034", // null test on a nullable ref output
            src: "catInfo(parent is null)[name]",
            params: &[],
            cols: &["name"],
            rows: json!([["goods"], ["services"]]),
        },
        Case {
            id: "C036", // mixed booleans with mandatory parens
            src: "top3Orders(total > 10000 or (customer == $a and total > 9000))[total]",
            params: &[("a", "\"cust_1\"")],
            cols: &["total"],
            rows: json!([[20000], [12000], [9500]]),
        },
        Case {
            id: "C038", // text operator on a text output
            src: "activeProducts(name contains \"a\")[name]",
            params: &[],
            cols: &["name"],
            rows: json!([["hammer"], ["saw"]]),
        },
        Case {
            id: "C039", // field-to-field comparison between outputs
            src: "productRevenue(revenue > sold * 5000)[product.name, revenue]",
            params: &[],
            cols: &["name", "revenue"],
            rows: json!([["drill", 9500], ["tune-up", 12000], ["consult", 20000]]),
        },
        Case {
            id: "C044", // filter then take, written directly — C004's contrast
            src: "orders(total < 15000)[placed, total]\nsort -total\ntake 3",
            params: &[],
            cols: &["placed", "total"],
            rows: json!([["2026-02-20", 12000], ["2026-02-05", 9500], ["2026-01-10", 4500]]),
        },
        Case {
            id: "C045", // caller re-sorts the post-take result
            src: "top3Orders(total < 15000)[placed, total]\nsort placed",
            params: &[],
            cols: &["placed", "total"],
            rows: json!([["2026-02-05", 9500], ["2026-02-20", 12000]]),
        },
        Case {
            id: "C046", // caller take after callee take
            src: "top3Orders[total]\nsort total\ntake 2",
            params: &[],
            cols: &["total"],
            rows: json!([[9500], [12000]]),
        },
        Case {
            id: "C047", // no caller sort: callee order is the result order
            src: "top3Orders[customer.name, total]",
            params: &[],
            cols: &["name", "total"],
            rows: json!([["Cy", 20000], ["Bo", 12000], ["Ada", 9500]]),
        },
        Case {
            id: "C049", // depth-2 inlining with a dot and caller sort
            src: "spendByCustomer($from: 2026-02-01)[customer.name, spent]\nsort -spent",
            params: &[],
            cols: &["name", "spent"],
            rows: json!([["Bo", 12000], ["Ada", 11500]]),
        },
        Case {
            id: "C053", // id tiebreaker survives an expression sort with a tie
            src: "doneOrders($from: 2026-01-01)[placed, total]\nsort month(placed)",
            params: &[],
            cols: &["placed", "total"],
            rows: json!([["2026-01-10", 4500], ["2026-02-05", 9500], ["2026-02-20", 12000], ["2026-04-02", 2000]]),
        },
        Case {
            id: "C054", // group by dot through a ref output; key asc (R-C4/R-C5)
            src: "doneOrders($from: 2026-01-01)[total, customer]\ngroup customer.tier { n: count, rev: sum total }",
            params: &[],
            cols: &["tier", "n", "rev"],
            rows: json!([["basic", 1, 12000], ["pro", 3, 16000]]),
        },
        Case {
            // Regroup a grouped callee; bucket bar (HAVING); caller sort.
            id: "C056",
            src: "productRevenue[product, revenue]\ngroup product.category.name { catRev: sum revenue }\n| catRev > 10000\nsort -catRev",
            params: &[],
            cols: &["name", "catRev"],
            rows: json!([["services", 20000], ["repair", 12000], ["handtools", 11000]]),
        },
        Case {
            // Reduce over a composed (post-take) output: one row.
            id: "C055",
            src: "top3Orders[total]\nreduce { grand: sum total }",
            params: &[],
            cols: &["grand"],
            rows: json!([[41500]]),
        },
        Case {
            // Conditional aggregate; ref group key orders by id (R-C16).
            id: "C057",
            src: "doneOrders($from: 2026-01-01)[total, customer]\ngroup customer { big: count(total > 5000), n: count }",
            params: &[],
            cols: &["customer", "big", "n"],
            rows: json!([["cust_1", 1, 3], ["cust_2", 1, 1]]),
        },
        Case {
            // Latest done order per customer — take[keys] over composition.
            id: "C061",
            src: "doneOrders($from: 2026-01-01)[placed, total, customer]\nsort -placed\ntake[customer] 1",
            params: &[],
            cols: &["placed", "total", "customer"],
            rows: json!([["2026-04-02", 2000, "cust_1"], ["2026-02-20", 12000, "cust_2"]]),
        },
        Case {
            // Expression take-key: biggest done order per month.
            id: "C062",
            src: "doneOrders($from: 2026-01-01)[placed, total]\nsort -total\ntake[month(placed)] 1",
            params: &[],
            cols: &["placed", "total"],
            rows: json!([["2026-02-20", 12000], ["2026-01-10", 4500], ["2026-04-02", 2000]]),
        },
        Case {
            // No sort: take[] ranks by implicit id through composition.
            id: "C063",
            src: "activeProducts[name, vendor]\ntake[vendor] 1",
            params: &[],
            cols: &["name", "vendor"],
            rows: json!([["hammer", "vend_1"], ["saw", "vend_2"], ["consult", "vend_3"]]),
        },
        Case {
            // Dot-path take-key: best done order per tier.
            id: "C064",
            src: "doneOrders($from: 2026-01-01)[placed, total, customer]\nsort -total\ntake[customer.tier] 1",
            params: &[],
            cols: &["placed", "total", "customer"],
            rows: json!([["2026-02-20", 12000, "cust_2"], ["2026-02-05", 9500, "cust_1"]]),
        },
        Case {
            // Tree operator on a ref output (R-C6): goods subtree drops
            // tune-up (cat_5 lives under services).
            id: "C037",
            src: "cheapActive($cap: 20000, category under $root)[name, price]\nsort price",
            params: &[("root", "\"cat_1\"")],
            cols: &["name", "price"],
            rows: json!([["hammer", 1500], ["saw", 3000], ["drill", 9500]]),
        },
        Case {
            // Depth-3 inlining with `under` INSIDE the callee chain:
            // toolDeals -> cheapActive -> activeProducts.
            id: "C050",
            src: "toolDeals($cap: 10000, $root: $root)",
            params: &[("root", "\"cat_2\"")],
            cols: &["name", "price"],
            rows: json!([["hammer", 1500], ["saw", 3000], ["drill", 9500]]),
        },
        Case {
            // Composed source beside an entity source; join inferred on
            // `product` provenance; outer-preserving; bar filters (the bar
            // also drops the null-extension rows: null >= 2 is not true).
            id: "C011",
            src: "products[name]  productRevenue[sold, revenue]\n| sold >= 2",
            params: &[],
            cols: &["name", "sold", "revenue"],
            rows: json!([["hammer", 2, 3000], ["saw", 2, 6000]]),
        },
        Case {
            // Semi-join over a composed collection: customers with a done
            // order placed on or after 2026-03-01 — ord_7 (cust_1) only.
            id: "C012",
            src: "customers[name]\nmatch any doneOrders($from: 2026-03-01)",
            params: &[],
            cols: &["name"],
            rows: json!([["Ada"]]),
        },
        Case {
            // Anti-join: customers with NO done order since 2026-01-01.
            id: "C013",
            src: "customers[name]\nmatch none doneOrders($from: 2026-01-01)",
            params: &[],
            cols: &["name"],
            rows: json!([["Cy"]]),
        },
        Case {
            // C071's shape without the sealed source: entity arm + composed
            // arm, renamed columns, unioned then sorted.
            id: "C071w",
            src: "orders(status == \"open\")[at: placed, total]\nappend recentOrders[at: placed, total]\nsort at, total",
            params: &[],
            cols: &["at", "total"],
            rows: json!([["2026-03-01", 3000], ["2026-03-15", 1500], ["2026-03-15", 1500], ["2026-03-20", 20000], ["2026-03-20", 20000], ["2026-04-02", 2000]]),
        },
        Case {
            // C072's shape: append + distinct over ref columns, id order.
            id: "C072w",
            src: "top3Orders[c: customer]\nappend doneOrders($from: 2026-01-01)[c: customer]\ndistinct\nsort c",
            params: &[],
            cols: &["c"],
            rows: json!([["cust_1"], ["cust_2"], ["cust_3"]]),
        },
        Case {
            // Binding from a reduce-let scalar (R-C3): max total = 20000.
            id: "C020",
            src: "let cap = orders[total] reduce { m: max total }\n\ncheapActive($cap: cap.m)[name]",
            params: &[],
            cols: &["name"],
            rows: json!([["hammer"], ["drill"], ["saw"], ["tune-up"]]),
        },
        Case {
            // Let-reduce baseline used in a composed condition; avg 8750.
            id: "C066",
            src: "let bar = productRevenue[revenue] reduce { a: avg revenue }\n\nproductRevenue(revenue > bar.a)[product.name, revenue]\nsort -revenue",
            params: &[],
            cols: &["name", "revenue"],
            rows: json!([["consult", 20000], ["tune-up", 12000], ["drill", 9500]]),
        },
        Case {
            // Composed let joined back; outer-preserving keeps Cy with null.
            id: "C067",
            src: "let perCust = spendByCustomer($from: 2026-01-01)\n\ncustomers[name]  perCust[spent]\nsort name",
            params: &[],
            cols: &["name", "spent"],
            rows: json!([["Ada", 16000], ["Bo", 12000], ["Cy", null]]),
        },
        Case {
            // Let chain regrouped and joined back: vendor revenue.
            id: "C068",
            src: "let pr = productRevenue[product, revenue]\nlet byVend = pr\ngroup product.vendor { rev: sum revenue }\n\nvendors[name]  byVend[rev]\nsort -rev",
            params: &[],
            cols: &["name", "rev"],
            rows: json!([["Corvid", 22000], ["Blythe", 18000], ["Acme", 12500]]),
        },
        Case {
            // 3-source chain witness: revenue per vendor through
            // vendors <- products <- orderItems, all inferred.
            id: "chain3w",
            src: "vendors[vname: name]  products[]  orderItems[]\ngroup vname { rev: sum amount }\nsort -rev",
            params: &[],
            cols: &["vname", "rev"],
            rows: json!([["Corvid", 22000], ["Blythe", 18000], ["Acme", 12500]]),
        },
        Case {
            // Transitive-bridge witness: 2 listed sources, the compiler
            // inserts `products` as the bridge (DL ~1582). Same numbers.
            id: "bridgew",
            src: "vendors[vname: name]  orderItems[]\ngroup vname { rev: sum amount }\nsort -rev",
            params: &[],
            cols: &["vname", "rev"],
            rows: json!([["Corvid", 22000], ["Blythe", 18000], ["Acme", 12500]]),
        },
        Case {
            // Binding expression: date + duration folds at bind time.
            id: "C019",
            src: "recentOrders($since: $d + 7d)[placed, total]",
            params: &[("d", "2026-03-10")],
            cols: &["placed", "total"],
            rows: json!([["2026-03-20", 20000], ["2026-04-02", 2000]]),
        },
        Case {
            // `since` on an output date; clock pinned 2026-05-01Z, so the
            // 90d window opens 2026-01-31 and ord_1 is excluded.
            id: "C035",
            src: "doneOrders($from: 2026-01-01, placed since 90d)[total]",
            params: &[],
            cols: &["total"],
            rows: json!([[9500], [12000], [2000]]),
        },
        Case {
            // Depth-3 over monthlyRevenue; month() is a BUCKET (DL ~617):
            // the row is ('2026-02', 21500), never (2, 21500).
            id: "C051",
            src: "monthlyRevenue($from: 2026-01-01, revenue > 5000)[month, revenue]",
            params: &[],
            cols: &["month", "revenue"],
            rows: json!([["2026-02", 21500]]),
        },
        Case {
            // distinct over composed output; rename with a dot-path.
            id: "C014",
            src: "doneOrders($from: 2026-01-01)[tier: customer.tier]\ndistinct\nsort tier",
            params: &[],
            cols: &["tier"],
            rows: json!([["basic"], ["pro"]]),
        },
        Case {
            // cond() in the caller projection over composed output.
            id: "C015",
            src: "top3Orders[total, size: cond(total >= 15000 -> \"huge\", \"big\")]",
            params: &[],
            cols: &["total", "size"],
            rows: json!([[20000, "huge"], [12000, "big"], [9500, "big"]]),
        },
        Case {
            // Median over composed output: interpolated (6000+9500)/2.
            id: "C059",
            src: "productRevenue[revenue]\nreduce { m: median revenue }",
            params: &[],
            cols: &["m"],
            rows: json!([[7750]]),
        },
        Case {
            // Two-hop dot through a ref output.
            id: "C075",
            src: "productRevenue[product.category.name, revenue]",
            params: &[],
            cols: &["name", "revenue"],
            rows: json!([["handtools", 3000], ["tools", 9500], ["handtools", 6000], ["repair", 12000], ["handtools", 2000], ["services", 20000]]),
        },
        Case {
            // Dot-path as caller sort key (unprojected head — W047 scope).
            id: "C076",
            src: "productRevenue[product.name, revenue]\nsort product.name",
            params: &[],
            cols: &["name", "revenue"],
            rows: json!([["consult", 20000], ["drill", 9500], ["hammer", 3000], ["saw", 6000], ["tune-up", 12000], ["wrench", 2000]]),
        },
        Case {
            // Dot group key over composed output (R-C5).
            id: "C077",
            src: "doneOrders($from: 2026-01-01)[customer]\ngroup customer.name { n: count }",
            params: &[],
            cols: &["name", "n"],
            rows: json!([["Ada", 3], ["Bo", 1]]),
        },
        Case {
            // Dot projection to an open field of the ref target.
            id: "C078",
            src: "doneOrders($from: 2026-02-01)[placed, customer.tier]",
            params: &[],
            cols: &["placed", "tier"],
            rows: json!([["2026-02-05", "pro"], ["2026-02-20", "basic"], ["2026-04-02", "pro"]]),
        },
        Case {
            // Blind-index equality (F1): the digest matches, the payload
            // never opens. Cost-class label awaits Stage 2.
            id: "C084",
            src: "customerByEmail($e: \"ada@example.com\")",
            params: &[],
            cols: &["name", "tier"],
            rows: json!([["Ada", "pro"]]),
        },
        Case {
            // Compatible callee edit (C098's exec half): doneOrders
            // re-registered with an added output; dependents recompile,
            // results unchanged. Local-op re-registration shadows.
            id: "C058", // double aggregation across composition
            src: "spendByCustomer($from: 2026-01-01)[n]\ngroup n { custs: count }",
            params: &[],
            cols: &["n", "custs"],
            rows: json!([[1, 1], [3, 1]]),
        },
    ]
}

#[test]
fn p0_harvest_executes() {
    let (f, pg) = seeded();
    let mut failures = Vec::new();
    let mut passed = Vec::new();

    for c in cases() {
        let got = if c.params.is_empty() {
            f.run_case(&pg, c.src)
        } else {
            f.run_case_params(&pg, c.src, c.params)
        };
        let got = match got {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: did not run\n  {e}", c.id));
                continue;
            }
        };
        // Project row objects to the doc's column order; compare ORDERED.
        let rows: Vec<Value> = got
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                Value::Array(
                    c.cols
                        .iter()
                        .map(|col| r.get(col).cloned().unwrap_or(Value::Null))
                        .collect(),
                )
            })
            .collect();
        if Value::Array(rows.clone()) != c.rows {
            failures.push(format!(
                "{}: wrong rows\n  returned: {}\n  expected: {}",
                c.id,
                Value::Array(rows),
                c.rows
            ));
        } else {
            passed.push(c.id);
        }
    }

    println!(
        "P0 harvest: {} of {} candidate cases execute",
        passed.len(),
        cases().len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

#[test]
fn p3_rejects() {
    let f = Fixture::load().expect("fixture");
    // C065 — take-key outside the callee's output shape.
    let e = f
        .compile("top3Orders[placed, total]\ntake[status] 1")
        .unwrap_err();
    assert!(e.contains("not_output_field"), "{e}");
    // C108 — numeric in take brackets is a parse error.
    let e = f.compile("activeProducts[name]\ntake[2]").unwrap_err();
    assert!(e.contains("parse"), "{e}");
}

#[test]
fn local_op_cases() {
    // C018 — caller param flows through a case-local op.
    let (f, pg) = seeded();
    let got = f
        .run_case_with_ops(
            &pg,
            "op ordersSince($d: date) unbounded =\n  recentOrders($since: $d)[placed, total]",
            &[("d", "2026-04-01")],
        )
        .expect("C018 runs");
    assert_eq!(got, json!([{"placed": "2026-04-02", "total": 2000}]));

    // C048 — nested take: the callee's take-2 completes before the filter.
    let got = f
        .run_case_with_ops(
            &pg,
            "op top2Cheap =\n  cheapActive($cap: 1000000)[name, price]\n  sort price\n  take 2\n\ntop2Cheap(price > 2000)[name]",
            &[],
        )
        .expect("C048 runs");
    assert_eq!(got, json!([{"name": "saw"}]));

    // C027 — flow-through type mismatch in a local op.
    let e = f
        .run_case_with_ops(
            &pg,
            "op bad($n: int) unbounded =\n  doneOrders($from: $n)[total]",
            &[("n", "5")],
        )
        .unwrap_err();
    assert!(e.contains("param_type_mismatch"), "{e}");

    // C043 — a narrowing projection truly narrows for all callers.
    let e = f
        .run_case_with_ops(
            &pg,
            "op slim = top3Orders[placed, total]\n\nslim(customer == $a)[placed]",
            &[("a", "\"cust_1\"")],
        )
        .unwrap_err();
    assert!(e.contains("not_output_field"), "{e}");
}

#[test]
fn registration_cases() {
    let (f, pg) = seeded();
    // C086 — unbounded callee, caller lacks the marker (R-C9).
    let e = f
        .run_case_with_ops(
            &pg,
            "op giftTotals =\n  giftOrders(total > 1000)[total]",
            &[],
        )
        .unwrap_err();
    assert!(e.contains("unacknowledged_unbounded"), "{e}");

    // C092 — direct cycle.
    let e = f
        .run_case_with_ops(
            &pg,
            "op selfLoop unbounded = selfLoop(total > 0)[total]",
            &[],
        )
        .unwrap_err();
    assert!(e.contains("cycle"), "{e}");

    // C093 — transitive cycle by re-registration: qb calls qa, then qa is
    // re-registered to call qb.
    let e = f
        .run_case_with_ops(
            &pg,
            "op qa0 unbounded = orders[total]\nop qb unbounded = qa[total]\nop qa unbounded = qb[total]\n\nqa[total]",
            &[],
        )
        .unwrap_err();
    assert!(e.contains("cycle"), "{e}");

    // C094 — unknown callee, never registered.
    let e = f.compile("frequentBuyers(n > 2)[name]").unwrap_err();
    assert!(e.contains("unknown_source"), "{e}");

    // ── R-G8's compile pair (GC032–GC035) ───────────────────────────────
    // GC032 — vendors is unguarded; the flag would be decoration.
    let e = f
        .run_case_with_ops(
            &pg,
            "op v unguarded = vendors[name]

v[name]",
            &[],
        )
        .unwrap_err();
    assert!(e.contains("unguarded_superfluous"), "{e}");

    // GC033 — the marker cannot strip a composed callee's guard: openTasks
    // is guarded, `leak`'s own body sources no guarded entity, so the
    // guard survives and the marker is superfluous.
    let e = f
        .run_case_with_ops(
            &pg,
            "op leak unbounded unguarded = openTasks[title]\n\nleak[title]",
            &[],
        )
        .unwrap_err();
    assert!(e.contains("unguarded_superfluous"), "{e}");

    // GC034 — composing a LIVE bypass (allTasks is unguarded over guarded
    // tasks) needs the caller's own acknowledgment.
    let e = f
        .run_case_with_ops(
            &pg,
            "op openAudit unbounded = allTasks(status == \"open\")[title]\n\nopenAudit[title]",
            &[],
        )
        .unwrap_err();
    assert!(e.contains("unacknowledged_unguarded"), "{e}");

    // GC035 — same op, acknowledged: compiles, and the results are
    // caller-independent (the bypass is disclosed, not disputed).
    let rows = f
        .run_case_with_ops(
            &pg,
            "op openAudit unbounded unguarded = allTasks(status == \"open\")[title]\n\nopenAudit[title]",
            &[],
        )
        .expect("acknowledged bypass compiles and runs");
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(4),
        "the four open tasks, any caller: {rows}"
    );

    // C098 — compatible callee edit: doneOrders gains an output; the
    // dependent recompiles and the rows are unchanged.
    let got = f
        .run_case_with_ops(
            &pg,
            "op doneOrders($from: date) unbounded =\n  orders(status == \"done\" and placed >= $from)[placed, total, customer, status]\n\nspendByCustomer($from: 2026-01-01)[customer, spent, n]\nsort -spent",
            &[],
        )
        .expect("C098 runs");
    assert_eq!(
        got,
        json!([
            {"customer": "cust_1", "spent": 16000, "n": 3},
            {"customer": "cust_2", "spent": 12000, "n": 1},
        ])
    );
}

#[test]
fn c084_compile_class_is_blind_index() {
    // The corpus's compile assertion for C084 (and C085's class half):
    // equality near the base classifies blind-index; composition inherits.
    let f = Fixture::load().expect("fixture");
    let q = f
        .bind_case("customerByEmail($e: \"ada@example.com\")")
        .expect("binds");
    assert_eq!(ordo_plan::cost::classify(&q).as_str(), "blind-index");
    let q = f
        .bind_case("customerByEmail($e: \"ada@example.com\")[tier]")
        .expect("tierOf shape binds");
    assert_eq!(ordo_plan::cost::classify(&q).as_str(), "blind-index");
}

#[test]
fn blind_index_boundaries() {
    let f = Fixture::load().expect("fixture");
    // Only equality: an ordering over the digest would be an oracle.
    let e = f.compile("customers(email > \"a\")[name]").unwrap_err();
    assert!(e.contains("sealed_unsupported"), "{e}");
    // The non-indexed sealed field still refuses even equality.
    let e = f.compile("orders(memo == \"gift\")[placed]").unwrap_err();
    assert!(e.contains("sealed_unsupported"), "{e}");
    // Projecting the payload is legal now (host-opened), and the plan says
    // so rather than pretending the value came from the database.
    let p = f
        .compile("customers[email]")
        .expect("a sealed projection compiles");
    assert_eq!(p.host_open.len(), 1, "the obligation rides with the plan");
    // The DIGEST column is never what gets projected — serving it would
    // hand out a stable equality oracle over the sealed value.
    assert!(!p.sql.contains("email$bidx"), "{}", p.sql);
}

#[test]
fn section_i_rejects() {
    let f = Fixture::load().expect("fixture");
    // C079 — dot through a value-typed output.
    let e = f
        .compile("spendByCustomer($from: 2026-01-01)[spent.name]")
        .unwrap_err();
    assert!(e.contains("not_a_ref"), "{e}");
    // C080 — projection of a non-output field has its own class.
    let e = f
        .compile("doneOrders($from: 2026-01-01)[status]")
        .unwrap_err();
    assert!(e.contains("projection_unknown_field"), "{e}");
    // C083 — entity x ungrouped composed over a shared ref key is M:N.
    let e = f
        .compile("orders[placed]  doneOrders($from: 2026-01-01)[total, customer]")
        .unwrap_err();
    assert!(e.contains("fan_out"), "{e}");
}

#[test]
fn fan_out_rejects() {
    // C081's class: two independent many-branches from one anchor.
    let f = Fixture::load().expect("fixture");
    // (First attempt used customers+orders+productRevenue — the inference
    // legitimately CHAINED them through two bridges into one many-path.
    // A real fan-out needs two many-edges meeting at one node:)
    let e = f
        .compile("products[name]  orderItems[]  productRevenue[]\ngroup name { n: count }")
        .unwrap_err();
    assert!(
        e.contains("fan_out"),
        "two many-branches must not aggregate: {e}"
    );
}

#[test]
fn p7_let_rejects() {
    let f = Fixture::load().expect("fixture");
    // C069 — a non-reduce let referenced as a scalar.
    let e = f
        .compile("let x = top3Orders[total]\n\norders(total > x.total)[placed]")
        .unwrap_err();
    assert!(e.contains("let_scalar_requires_reduce"), "{e}");
}

#[test]
fn p7_append_rejects() {
    // C073/C074's classes, over open fields (the corpus arms use the
    // sealed giftOrders, which F0 still blocks).
    let f = Fixture::load().expect("fixture");
    let e = f
        .compile("recentOrders[placed, total]\nappend top3Orders[total]")
        .unwrap_err();
    assert!(e.contains("append_misalignment"), "{e}");
    let e = f
        .compile("recentOrders[k: placed]\nappend productRevenue[k: revenue]")
        .unwrap_err();
    assert!(e.contains("append_misalignment"), "{e}");
}

#[test]
fn p2_rejects() {
    let f = Fixture::load().expect("fixture");
    // C060 — a bare per-row field inside braces.
    let e = f
        .compile("doneOrders($from: 2026-01-01)[total, customer]\ngroup customer { t: total }")
        .unwrap_err();
    assert!(e.contains("bare_field_in_group"), "{e}");
    // Keyless group is spelled `reduce`.
    let e = f
        .compile("orders[total]\ngroup { t: sum total }")
        .unwrap_err();
    assert!(e.contains("reduce"), "{e}");
}

#[test]
fn p1_param_rejects() {
    let f = Fixture::load().expect("fixture");
    for (id, src, class) in [
        (
            "C023",
            "doneOrders(total > 0)[total]",
            "unbound_required_param",
        ),
        (
            "C025",
            "recentOrders($since: 5)[placed]",
            "param_type_mismatch",
        ),
        (
            "C026",
            "doneOrders($from: \"2026-01-01\")[total]",
            "param_type_mismatch",
        ),
        (
            "C028",
            "doneOrders($from: 2026-01-01, $from: 2026-02-01)[total]",
            "duplicate_binding",
        ),
        (
            "C029",
            "doneOrders($from: placed)[total]",
            "invalid_binding_expr",
        ),
        (
            "C040",
            "doneOrders($from: 2026-01-01, status == \"done\")[total]",
            "not_output_field",
        ),
        (
            "C041",
            "activeProducts(active == true)[name]",
            "not_output_field",
        ),
    ] {
        let e = f.compile(src).expect_err(&format!("{id} must be rejected"));
        assert!(
            e.contains(class),
            "{id}: rejected as `{e}`, expected class `{class}`"
        );
    }
}

#[test]
fn p0_rejects() {
    let f = Fixture::load().expect("fixture");
    // C024 — unknown parameter name (class matches the code's today).
    let e = f
        .compile("recentOrders($cutoff: 2026-01-01)[placed]")
        .unwrap_err();
    assert!(e.contains("unknown_param"), "{e}");
    // C042 — mixed and/or without parens is a parse-stage rejection.
    let e = f
        .compile("top3Orders(total > 10000 or customer == $a and total > 9000)[total]")
        .unwrap_err();
    assert!(e.contains("parse"), "{e}");
}

#[test]
fn whole_returns_the_owned_closure_with_signposts() {
    // `whole projects($id)` on the wide fixture: the anchor's open scalars,
    // its refs as ONE-HOP shallow signposts (org, lead, sponsor — bodies of
    // ancestors never included), and the owned closure downward — tasks,
    // and inside each task its taskTags, each tag a shallow signpost.
    // Incoming non-owned associations (events -> tasks is optional, not
    // owned... events.task is NOT owned, so events never appear).
    let f = Fixture::load_wide().expect("wide fixture");
    let pg = Pg::named("ordo_wide");
    f.reset_once_wide(&pg).expect("seeded");

    let w = f
        .run_whole(&pg, "whole projects(\"prj_1\")")
        .expect("whole runs");
    assert_eq!(w["name"], "Apollo");
    assert_eq!(w["org"]["name"], "Acme", "owner signpost, shallow");
    assert_eq!(w["lead"]["name"], "Ada");
    assert_eq!(w["sponsor"]["name"], "Cy");
    assert!(
        w["org"].get("teams").is_none(),
        "signposts carry no closures"
    );

    let tasks = w["tasks"].as_array().expect("owned closure is an array");
    assert_eq!(tasks.len(), 3, "prj_1 owns tsk_1..tsk_3");
    let t1 = tasks.iter().find(|t| t["id"] == "tsk_1").unwrap();
    assert_eq!(t1["assignee"]["name"], "Ada", "child's ref is a signpost");
    let tags = t1["taskTags"].as_array().expect("two levels of ownership");
    assert_eq!(tags.len(), 1);
    assert_eq!(
        tags[0]["tag"]["label"], "design",
        "link far side: one shallow hop"
    );

    // A childless anchor still answers, with empty arrays.
    let w3 = f.run_whole(&pg, "whole tasks(\"tsk_3\")").expect("runs");
    assert_eq!(w3["taskTags"], serde_json::json!([]));
    // And a missing anchor is null, not an error.
    let none = f
        .run_whole(&pg, "whole projects(\"prj_999\")")
        .expect("runs");
    assert!(none.is_null());
}

#[test]
fn audit_blind_classification_recurses_into_expressions() {
    // Audit F3: a digest compare inside cond() — in a filter or a
    // projection — must classify blind-index, not sql.
    let f = Fixture::load().expect("fixture");
    let q = f
        .bind_case("customers(cond(email == \"a@b.c\" -> 1, 0) == 1)[name]")
        .expect("binds");
    assert_eq!(ordo_plan::cost::classify(&q).as_str(), "blind-index");
    let q = f
        .bind_case("customers[name, hit: cond(email == \"a@b.c\" -> \"yes\", \"no\")]")
        .expect("binds");
    assert_eq!(ordo_plan::cost::classify(&q).as_str(), "blind-index");
}

#[test]
fn audit_take_before_append_is_not_self_bounded() {
    // Audit F4: the viral rule must not be skipped by a take that an
    // append later un-bounds.
    let (f, pg) = seeded();
    let e = f
        .run_case_with_ops(
            &pg,
            "op fooled =\n  orders(status == \"open\")[placed, total]\n  take 5\n  append recentOrders[placed, total]",
            &[],
        )
        .unwrap_err();
    assert!(e.contains("unacknowledged_unbounded"), "{e}");
}

// ── Stage 3c: the executed visibility matrix (GC §3.3) ───────────────────

#[test]
fn visibility_matrix_holds_per_caller() {
    let (f, pg) = seeded();
    let run = |src: &str, caller: Option<&str>| -> Vec<String> {
        f.run_case_as(&pg, src, &[], caller)
            .unwrap_or_else(|e| panic!("{src} as {caller:?}: {e}"))
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_object().unwrap()["name"]
                    .as_str()
                    .or_else(|| r.as_object().unwrap().values().next().unwrap().as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    };

    // projects read (owner ∨ member) — GC §3.3 row 1.
    assert_eq!(run("myProjects[name]", Some("mem_1")), ["Alpha", "Gamma"]);
    assert_eq!(run("myProjects[name]", Some("mem_2")), ["Alpha", "Beta"]);
    assert_eq!(run("myProjects[name]", Some("mem_3")), ["Beta", "Gamma"]);
    assert!(
        run("myProjects[name]", Some("mem_4")).is_empty(),
        "Dov holds nothing"
    );
    assert!(
        run("myProjects[name]", None).is_empty(),
        "anonymous: every guard false"
    );
}

#[test]
fn tasks_and_inherited_notes_filter_per_caller() {
    let (f, pg) = seeded();
    let titles = |src: &str, caller: Option<&str>| -> Vec<String> {
        f.run_case_as(&pg, src, &[], caller)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_object()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };

    // The GC §1.2 example verbatim: openTasks as ana / dov / guest.
    assert_eq!(
        titles("openTasks[title]", Some("mem_1")),
        ["audit books", "draft spec"]
    );
    assert!(titles("openTasks[title]", Some("mem_4")).is_empty());
    assert!(titles("openTasks[title]", Some("mem_g")).is_empty());

    // The laundering probe: task_5 is ASSIGNED to Bram, who cannot read
    // its project — assignment does not launder access.
    assert!(
        titles("myAssignedTasks[title]", Some("mem_2")).is_empty(),
        "assignee without membership sees nothing"
    );
    assert_eq!(
        titles("myAssignedTasks[title]", Some("mem_1")),
        ["draft spec"]
    );
    assert_eq!(
        titles("myAssignedTasks[title]", Some("mem_3")),
        ["build site"]
    );

    // taskNotes reads under the INHERITED guard, two levels rebased.
    assert_eq!(
        titles("noteFeed[body]", Some("mem_1")),
        ["numbers pending", "outline ready"]
    );
    assert_eq!(titles("noteFeed[body]", Some("mem_3")), ["site copy wip"]);

    // Cleo owns proj_3 but tasks' guard REPLACED the inherited one and has
    // no owner disjunct — the override demonstration.
    assert!(
        !titles("openTasks[title]", Some("mem_3")).contains(&"audit books".to_string()),
        "ownership does not leak through a replaced guard"
    );
}

#[test]
fn acl_derived_guards_filter_and_guests_reach_their_grant() {
    let (f, pg) = seeded();
    let vals = |src: &str, caller: Option<&str>| -> Vec<String> {
        f.run_case_as(&pg, src, &[], caller)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_object()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };

    // boards read at any level (R-G14) — and the guest's one grant works.
    assert_eq!(vals("boardList[name]", Some("mem_1")), ["roadmap"]);
    assert_eq!(
        vals("boardList[name]", Some("mem_2")),
        ["icebox", "roadmap"]
    );
    assert_eq!(vals("boardList[name]", Some("mem_g")), ["roadmap"]);
    assert!(vals("boardList[name]", Some("mem_4")).is_empty());

    // cards inherit the boards ACL down the owned subtree.
    assert_eq!(
        vals("cardList[title]", Some("mem_g")),
        ["q3 goals", "q4 goals"]
    );
    assert_eq!(
        vals("cardList[title]", Some("mem_2")),
        ["q3 goals", "q4 goals", "someday"]
    );
    assert!(vals("cardList[title]", Some("mem_4")).is_empty());
}

#[test]
fn unguarded_bypasses_and_guards_survive_composition() {
    let (f, pg) = seeded();
    let vals = |src: &str, caller: Option<&str>| -> Vec<String> {
        f.run_case_as(&pg, src, &[], caller)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_object()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };

    // allTasks is unguarded: Dov — and even anonymous — sees all five
    // (G15: the door is a separate layer; the flag is loud for a reason).
    assert_eq!(vals("allTasks[title]", Some("mem_4")).len(), 5);
    assert_eq!(vals("allTasks[title]", None).len(), 5);

    // Composition: specTasks inlines the GUARDED openTasks — the callee's
    // guard travels (G13).
    assert_eq!(vals("specTasks[title]", Some("mem_1")), ["draft spec"]);
    assert_eq!(vals("specTasks[title]", Some("mem_2")), ["draft spec"]);
    assert!(vals("specTasks[title]", Some("mem_4")).is_empty());
}

/// The blind-index parameter bug, and the custody redesign behind it.
///
/// A parameterized blind lookup used to compare the RAW parameter against
/// the digest column. It matched nothing, silently — indistinguishable to
/// a caller from "there is no such customer" — and only the literal path
/// was ever tested, so the corpus could not see it.
///
/// The fix is structural rather than a patch: the binder emits a digest
/// SITE and no key material, so nothing key-derived is baked into a
/// compiled artifact, and the digest is computed after the access check
/// behind a key release. An unresolved site is a typed refusal, never an
/// empty answer.
#[test]
fn a_blind_lookup_by_parameter_finds_the_row() {
    use ordo_sema::ir::BExpr;

    let (f, pg) = seeded();

    // By parameter — the broken path.
    let by_param = f
        .run_case_params(
            &pg,
            "customerByEmail($e: $v)[name, tier]",
            &[("v", "\"ada@example.com\"")],
        )
        .expect("a parameterized blind lookup runs");
    assert_eq!(
        by_param,
        serde_json::json!([{ "name": "Ada", "tier": "pro" }]),
        "the parameter is digested, not compared raw"
    );

    // And it agrees with the literal path, which is what "the same lookup"
    // has to mean.
    let by_literal = f
        .run_case(&pg, r#"customers(email == "ada@example.com")[name, tier]"#)
        .expect("literal blind lookup");
    assert_eq!(by_literal, by_param);

    // Normalization still decides identity: the schema says lower, so a
    // mixed-case parameter finds the same row.
    let mixed = f
        .run_case_params(
            &pg,
            "customerByEmail($e: $v)[name]",
            &[("v", "\"Ada@Example.COM\"")],
        )
        .expect("normalized");
    assert_eq!(mixed, serde_json::json!([{ "name": "Ada" }]));

    // The compiled artifact carries a SITE, not a digest — no key-derived
    // bytes before the access check, and none in a deploy diff.
    let q = f
        .bind_case(r#"customers(email == "ada@example.com")[name]"#)
        .expect("binds");
    let has_site = q.filters.iter().any(|c| {
        matches!(
            c,
            ordo_sema::ir::BCond::Cmp {
                rhs: BExpr::BlindDigest { .. },
                ..
            }
        )
    });
    assert!(
        has_site,
        "the binder must emit a digest site: {:?}",
        q.filters
    );

    // An unresolved site refuses rather than returning nothing. A silent
    // empty result is the failure mode this whole change exists to kill.
    let e = ordo_plan::plan(&q, &f.plan_space()).expect_err("an unresolved digest cannot lower");
    assert_eq!(e.code, "unresolved_blind_index");
}
