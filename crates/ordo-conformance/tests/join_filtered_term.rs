//! Reported fix 25s (`join-filtered-term`): a join projects fields from a
//! filtered or guarded second source.
//!
//! What was seen: `customers[name] orders(status == "done")[placed]` and
//! `projects[name] tasks[title]` (tasks guarded) failed with
//! `projection_unknown_field … available: [customer]`. The cause: a term
//! with filters, a guard among them, is lowered to a sub-select, and that
//! sub-select returned only its join key. It now carries the term's fields.
//! A sealed field goes in only when the term's projection names it, with
//! its obligation in the sub-select's `host_open`, as fix 25r carries it for
//! the unfiltered join.
//!
//! A regression reads as that refusal again, as a sealed field through a
//! filtered term served as `\x…` or classed `sql`, or as a guarded term
//! returning rows its guard hides.
//!
//! Reported fix 25v (`filtered-term-reach`) is held here too. The filtered
//! term's sub-select carried every open field and was never trimmed, so an
//! op composed over it over-reported: `drop = memos[vname, pname]` read
//! `unbounded-host` and reached every field of the term's entity. Each
//! sub-select now keeps only the columns its outer query reads. A
//! regression reads as `drop` classed host work again, as a key release on
//! it, or as a reach wider than its plain-join twin's plus the filter.
//!
//! The executing tests need the conformance cluster
//! (`scripts/conformance-pg.sh start`). Each owns its own database.

use ordo_conformance::{fixture_space, Fixture, Pg, FIXTURE, SEED};
use ordo_crypto::{Anchor, Purpose};
use ordo_plan::cost::{classify_with, CostClass};
use serde_json::Value;

const WRITTEN: &str = "leave at the side door";

fn fixture() -> Fixture {
    Fixture::from_source(FIXTURE, SEED).expect("the fixture compiles")
}

/// A database of this test's own, with `ord_4`'s memo sealed for real.
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
    (f, pg)
}

/// `(a, b)` string pairs from each row, sorted; a null reads as "".
fn pairs(rows: &Value, a: &str, b: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = rows
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| {
            (
                r[a].as_str().unwrap_or_default().to_string(),
                r[b].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

// ── the binder ───────────────────────────────────────────────────────────

/// Both reported shapes bind and project the named fields.
#[test]
fn a_filtered_or_guarded_second_source_projects_its_fields() {
    let f = fixture();
    for (src, outs) in [
        (
            "customers[name] orders(status == \"done\")[placed]",
            vec!["name", "placed"],
        ),
        ("projects[name] tasks[title]", vec!["name", "title"]),
        (
            "projects[name] tasks(status == \"open\")[title, status]",
            vec!["name", "title", "status"],
        ),
    ] {
        let q = f.bind_case(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        let got: Vec<&str> = q.outputs.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(got, outs, "{src}");
        f.compile(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    }
}

/// A sealed field through a filtered term keeps its seal: the obligation
/// names its entity and field, the envelope is anchored to its row, and the
/// read is host work. A term that does not name it carries no envelope and
/// stays plain SQL.
#[test]
fn a_sealed_field_through_a_filtered_term_keeps_its_seal() {
    let f = fixture();
    for (src, out) in [
        (
            "customers[name] orders(status == \"done\")[placed, memo]",
            "memo",
        ),
        (
            "customers[name] orders(status == \"done\")[placed, note: memo]",
            "note",
        ),
    ] {
        let q = f.bind_case(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(
            q.host_open,
            vec![(out.to_string(), "orders".into(), "memo".into())],
            "{src}"
        );
        assert_eq!(
            classify_with(&q, Some(&f.schema)),
            CostClass::UnboundedHost,
            "{src}"
        );
        let p = f.compile(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert!(
            p.sql.contains("convert_to("),
            "{src}: the envelope is not anchored to its row:\n{}",
            p.sql
        );
    }
    let src = "customers[name] orders(status == \"done\")[placed]";
    let q = f.bind_case(src).unwrap();
    assert!(q.host_open.is_empty(), "{src}: {:?}", q.host_open);
    assert_eq!(classify_with(&q, Some(&f.schema)), CostClass::Sql, "{src}");
    let p = f.compile(src).unwrap();
    assert!(
        !p.sql.contains("\"memo\""),
        "{src}: an unnamed sealed field rode along:\n{}",
        p.sql
    );
}

// ── execution ────────────────────────────────────────────────────────────

/// The filter applies inside the term, and the join keeps a customer with
/// no matching order (outer-preserving, DL "Multi-source join semantics").
#[test]
fn a_filtered_term_returns_its_filtered_rows() {
    let (f, pg) = seeded("ordo_joinfilter_rows");
    let rows = f
        .run_case(&pg, "customers[name] orders(status == \"done\")[placed]")
        .expect("runs");
    let placed: Vec<String> = pairs(&rows, "placed", "name")
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| !p.is_empty())
        .collect();
    assert_eq!(
        placed,
        ["2026-01-10", "2026-02-05", "2026-02-20", "2026-04-02"],
        "{rows:#}"
    );
}

/// The memo opens to what was written, through the filter and through a
/// rename, and nothing served looks like an envelope.
#[test]
fn a_sealed_field_through_a_filtered_term_opens() {
    let (f, pg) = seeded("ordo_joinfilter_open");
    for (src, out) in [
        (
            "customers[name] orders(status == \"done\")[placed, memo]",
            "memo",
        ),
        (
            "customers[name] orders(status == \"done\")[placed, note: memo]",
            "note",
        ),
    ] {
        let rows = f
            .run_case(&pg, src)
            .unwrap_or_else(|e| panic!("{src}: {e}"));
        let got = pairs(&rows, "placed", out);
        assert!(
            got.contains(&("2026-02-20".into(), WRITTEN.into())),
            "{src}: {rows:#}"
        );
        for (_, m) in &got {
            assert!(!m.starts_with("\\x"), "{src} served an envelope: {m}");
        }
    }
}

/// The guard still applies inside the term. `mem_3` belongs to `proj_2`
/// and owns `proj_3` without being its member, so it reads `proj_2`'s two
/// tasks and none of `proj_3`'s, while still seeing `proj_3` itself.
#[test]
fn a_guarded_term_keeps_its_guard() {
    let (f, pg) = seeded("ordo_joinfilter_guard");
    let rows = f
        .run_case_as(&pg, "projects[name] tasks[title]", &[], Some("mem_3"))
        .expect("runs");
    assert_eq!(
        pairs(&rows, "name", "title"),
        [
            ("Beta".to_string(), "build site".to_string()),
            ("Beta".to_string(), "ship site".to_string()),
            ("Gamma".to_string(), String::new()),
        ],
        "{rows:#}"
    );
    // Anonymous: every guard is false, so nothing at all.
    let rows = f
        .run_case_as(&pg, "projects[name] tasks[title]", &[], None)
        .expect("runs");
    assert_eq!(rows.as_array().map(Vec::len), Some(0), "{rows:#}");
}

// ── 25v: what an op over a filtered term reaches ──────────────────────────

/// A vendor and product pair with a sealed field, so the two shapes of the
/// 25v report can be written as they were seen.
const MAKERS: &str = "entity makers {\n  name: text\n}\n\
                      entity wares {\n  maker -> makers\n  name: text\n  price: money\n  \
                      active: bool = true\n  secret: text = \"\" sealed\n}\n";

fn with(extra: &str) -> Fixture {
    Fixture::from_source(&format!("{FIXTURE}\n{MAKERS}\n{extra}"), SEED)
        .unwrap_or_else(|e| panic!("must compile:\n{extra}\n{e}"))
}

/// The fields of `entity` an op's artifact says it reads.
fn reads(a: &Value, entity: &str) -> Vec<String> {
    a["scope"]
        .as_array()
        .expect("scope")
        .iter()
        .find(|e| e["entity"] == entity)
        .map(|e| {
            e["read"]
                .as_array()
                .expect("read")
                .iter()
                .map(|f| f.as_str().expect("name").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// **An op that drops the sealed field of a filtered term reaches what its
/// plain-join twin reaches, plus what the filter reads.** `drop` is plain
/// SQL, releases no key, and reaches `wares.{active, maker, name}`; the
/// twin over the unfiltered join reaches `wares.{maker, name}`. Before the
/// fix `drop` read `unbounded-host` and reached `price` and `secret` too.
#[test]
fn an_op_over_a_filtered_term_reaches_only_what_it_reads() {
    let f = with(
        "op memos unbounded =\n  makers[vname: name] wares(active == true)[pname: name, secret]\n\
         op drop unbounded =\n  memos[vname, pname]\n\
         op memosPlain unbounded =\n  makers[vname: name] wares[pname: name, secret]\n\
         op dropPlain unbounded =\n  memosPlain[vname, pname]\n",
    );
    let art = |op: &str| ordo_plan::artifact::op_artifact(&f.schema, &f.registry, op).expect(op);
    let drop = art("drop");
    let twin = art("dropPlain");
    assert_eq!(drop["cost"]["class"], "sql", "{}", drop["cost"]);
    assert_eq!(twin["cost"]["class"], "sql", "{}", twin["cost"]);
    for a in [&drop, &twin] {
        assert_eq!(
            a["release"]["keys"].as_array().map(Vec::len),
            Some(0),
            "no key release: {}",
            a["release"]
        );
    }
    assert_eq!(reads(&twin, "wares"), ["maker", "name"]);
    assert_eq!(reads(&drop, "wares"), ["active", "maker", "name"]);
    assert_eq!(reads(&drop, "makers"), reads(&twin, "makers"));

    // The op that names the sealed field still opens it, and reaches no
    // field it does not read.
    let memos = art("memos");
    assert_eq!(
        memos["cost"]["class"], "unbounded-host",
        "{}",
        memos["cost"]
    );
    assert_eq!(memos["release"]["keys"][0]["field"], "secret");
    assert_eq!(
        reads(&memos, "wares"),
        ["active", "maker", "name", "secret"]
    );

    // And the SQL selects no envelope for `drop`.
    let q = ordo_sema::bind_op_generic(&f.schema, &f.registry, "drop", ordo_sema::Clock::CORPUS)
        .expect("binds");
    assert!(q.host_open.is_empty(), "{:?}", q.host_open);
    let p = ordo_plan::plan(&q, &f.plan_space()).expect("plans");
    assert!(!p.sql.contains("\"secret\""), "{}", p.sql);
}

/// **A column used only in a filter or a sort stays in the sub-select.**
/// The caller projects only the customer's name, and filters and sorts on
/// fields of the filtered term it does not project. Those columns are read
/// from outside the sub-select, so they are kept, and the rows match the
/// same query written over the term directly.
#[test]
fn a_filtered_term_used_only_in_a_filter_or_sort_keeps_those_columns() {
    let (f, pg) = seeded("ordo_joinfilter_prune");
    let ops = "op doneOrders2 unbounded =\n  \
               customers[name] orders(status == \"done\")[placed, total, memo]\n\n";
    let bound = |src: &str| {
        let program = ordo_syntax::parse_program(ops.trim_end()).expect("ops parse");
        let mut registry = f.registry.clone();
        registry.extend_from(&program);
        let pl = ordo_syntax::parse_pipeline(src).expect("case parses");
        ordo_sema::bind(&f.schema, &registry, &pl, ordo_sema::Clock::CORPUS).expect("binds")
    };
    for (case, kept) in [
        ("doneOrders2[name]\n  | total >= 9000", "total"),
        ("doneOrders2[name]\n  sort placed", "placed"),
    ] {
        let q = bound(case);
        assert!(q.host_open.is_empty(), "{case}: {:?}", q.host_open);
        let sub = q
            .source_joins
            .iter()
            .find_map(|j| match &j.from {
                ordo_sema::ir::From::Sub { query, .. } => Some(query),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{case}: the filtered term is a sub-select"));
        let outs: Vec<&str> = sub.outputs.iter().map(|o| o.name.as_str()).collect();
        assert!(outs.contains(&kept), "{case}: {kept} was dropped: {outs:?}");
        assert!(!outs.contains(&"memo"), "{case}: memo rode along: {outs:?}");
        assert!(sub.host_open.is_empty(), "{case}: {:?}", sub.host_open);
        assert_eq!(classify_with(&q, Some(&f.schema)), CostClass::Sql, "{case}");
    }

    // The rows agree with the query written over the term directly.
    let rows = f
        .run_case_with_ops(
            &pg,
            &format!("{ops}doneOrders2[name]\n  | total >= 9000\n  sort placed"),
            &[],
        )
        .expect("runs");
    let direct = f
        .run_case(
            &pg,
            "customers[name] orders(status == \"done\")[placed, total]\n  | total >= 9000\n  sort placed",
        )
        .expect("runs");
    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .expect("rows")
            .iter()
            .map(|r| r["name"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert_eq!(names(&rows), names(&direct), "{rows:#}\n{direct:#}");
    assert_eq!(
        names(&rows),
        ["Ada", "Bo"],
        "ord_2 and ord_4, by placed: {rows:#}"
    );
}

/// **The prune changes no result.** A sub-select keeps every column the
/// outer query reads, whatever reads it: a filter, a sort, a group key, a
/// `having`, a dot-hop, a `take[keys]`, a `match`, an `append` arm or a
/// `let`. Each pruned shape is run beside its twin written over the entity
/// directly, and the rows agree. The callee `latest` takes more rows than
/// the seed holds, so its wrap settles nothing and the twin is exact.
#[test]
fn a_pruned_sub_select_keeps_every_column_the_outer_query_reads() {
    let (f, pg) = seeded("ordo_joinfilter_prune_shapes");
    let ops = "op latest unbounded =\n  \
               orders[placed, total, memo, status, customer]\n  sort -placed\n  take 10\n\n\
               op fin unbounded =\n  orders(status == \"done\")[placed, total, customer]\n\n";
    let pairs = [
        (
            "latest[placed]\n  | total > 5000\n  sort total",
            "orders[placed]\n  | total > 5000\n  sort total",
        ),
        (
            "latest\n  group status { s: sum total }\n  | s > 5000\n  sort status",
            "orders[total, status]\n  group status { s: sum total }\n  | s > 5000\n  sort status",
        ),
        (
            "latest[placed, cname: customer.name]",
            "orders[placed, cname: customer.name]\n  sort -placed",
        ),
        (
            "latest[placed, customer]\n  take[customer] 1",
            "orders[placed, customer]\n  sort -placed\n  take[customer] 1",
        ),
        (
            "customers[name]\n  match any fin",
            "customers[name]\n  match any orders(status == \"done\")",
        ),
        (
            "customers[name]\n  match none fin",
            "customers[name]\n  match none orders(status == \"done\")",
        ),
        (
            "customers[name] fin[placed]\n  sort -total",
            "customers[name] orders(status == \"done\")[placed]\n  sort -total",
        ),
        (
            "customers[name] fin\n  group name { s: sum total }\n  | s > 5000\n  sort name",
            "customers[name] orders(status == \"done\")\n  group name { s: sum total }\n  \
             | s > 5000\n  sort name",
        ),
        (
            "customers[name] latest[placed]\n  | total > 5000",
            "customers[name] orders[placed]\n  | total > 5000",
        ),
        (
            "latest[placed]\n  append fin[placed]\n  sort -placed",
            "orders[placed]\n  append orders(status == \"done\")[placed]\n  sort -placed",
        ),
        (
            "let t = latest[placed, total]\n  t[placed]\n  | total > 5000",
            "orders[placed]\n  | total > 5000\n  sort placed",
        ),
    ];
    for (pruned, direct) in pairs {
        let got = f
            .run_case_with_ops(&pg, &format!("{ops}{pruned}"), &[])
            .unwrap_or_else(|e| panic!("{pruned}: {e}"));
        let want = f
            .run_case(&pg, direct)
            .unwrap_or_else(|e| panic!("{direct}: {e}"));
        assert_eq!(got, want, "{pruned}\n  against\n{direct}");
    }
}
