//! Checklist 9e — **parenthesized guard groups**, executed.
//!
//! R-G3 admits parens in a guard walk when `and` and `or` are mixed
//! (principle 2 requires them: there is no precedence table). The parser
//! refused them as "not in this slice", which blocked the canonical
//! multi-level ACL idiom `syntax-examples.md` §6 is written in:
//!
//! ```text
//! access write  property.propertiesAccess(
//!                 member == $caller and (level == "write" or level == "admin"))
//! ```
//!
//! # What the parser now does, and why it matters here
//!
//! A group is **distributed**, so [`ordo_syntax::GuardExpr`] stays in
//! disjunctive normal form and "a disjunct" keeps meaning "one independent
//! authorization path" for every rule stated over that notion. The walk
//! above becomes two paths, and `member == $caller` survives into **both**.
//!
//! That is the whole hazard, and it is why this suite exists in the
//! execution tier rather than the parser's unit tests. Three plausible
//! wrong lowerings all *permit* the callers a correct one permits:
//!
//! | wrong lowering | who it wrongly admits |
//! |---|---|
//! | drop the group — `member == $caller` | anyone with any level |
//! | drop the parens — `(member == $caller and level == "write") or level == "admin"` | anyone at all, as soon as *somebody* is admin |
//! | drop the `and` — `member == $caller or level == "write" or …` | anyone at all |
//!
//! So every case below is asserted in **both** directions: a caller the
//! group permits and a caller it refuses. A guard test with only the
//! permit direction proves nothing — all four lowerings pass it.
//!
//! Real Postgres, this suite's own databases (`ordo_9e_*`).

mod gc_support;

use gc_support::{class_of, db};
use ordo_conformance::{Fixture, Pg, WriteLog, FIXTURE, SEED};
use serde_json::{json, Value};

/// The estate cluster of `syntax-examples.md` §6, transcribed with names
/// of its own so nothing here moves a number another suite asserts about
/// the shipped fixture.
///
/// `estateUnits`' write guard is the idiom verbatim: a `$caller` test
/// conjoined with a parenthesized level group, inside a reverse collection
/// step two hops from the row being written.
const ESTATE: &str = r#"
entity estates (key: est) {
  name: text
  access read   estatesAccess(member == $caller)
  access write  estatesAccess(member == $caller and level == "admin")
}

entity estatesAccess (key: eacc) {
  estate -> estates  owned
  member -> members  owned
  level: text
  access read   estate.estatesAccess(member == $caller)
  access write  estate.estatesAccess(member == $caller and level == "admin")
}

entity estateUnits (key: eun) {
  estate -> estates  owned
  name: text
  access read   estate.estatesAccess(member == $caller)
  access write  estate.estatesAccess(
                  member == $caller and (level == "write" or level == "admin"))
}

# A group at the TOP level of a clause rather than inside a step — the
# `access read (a or b) and c` shape 9e names.
entity memos (key: memo) {
  owner -> members
  published: bool = false
  pinned:    bool = false
  access read   (published or pinned) and owner == $caller
  access write  owner == $caller
}

op unitList unbounded =
  estateUnits[name]
  sort name

op renameUnit($u: id, $name: text) =
  set estateUnits(id == $u) { name: $name }

# A top-level group one of whose branches is a reverse STEP: distribution
# duplicates the step, so this clause is where the union-of-semi-joins
# shape is actually visible.
entity notices (key: ntc) {
  estate -> estates  owned
  owner  -> members
  urgent: bool = false
  access read   (urgent or owner == $caller) and estate.estatesAccess(member == $caller)
  access write  owner == $caller
}

op memoList unbounded =
  memos[name: id]
  sort id

op noticeList unbounded =
  notices[name: id]
  sort id
"#;

/// Two estates, so a "level == admin" disjunct that lost its `$caller`
/// conjunct has somebody else's admin row to be satisfied by.
///
/// | member | est_1 | est_2 |
/// |---|---|---|
/// | mem_1 Ana  | admin | — |
/// | mem_2 Bram | write | — |
/// | mem_3 Cleo | read  | — |
/// | mem_4 Dov  | —     | admin |
const ESTATE_SEED: &str = r#"
INSERT INTO "estates" ("$space", "id", "name") VALUES
  ('conformance', 'est_1', 'Alpha Estate'),
  ('conformance', 'est_2', 'Beta Estate');

INSERT INTO "estatesAccess" ("$space", "id", "estate", "member", "level") VALUES
  ('conformance', 'eacc_1', 'est_1', 'mem_1', 'admin'),
  ('conformance', 'eacc_2', 'est_1', 'mem_2', 'write'),
  ('conformance', 'eacc_3', 'est_1', 'mem_3', 'read'),
  ('conformance', 'eacc_4', 'est_2', 'mem_4', 'admin');

INSERT INTO "estateUnits" ("$space", "id", "estate", "name") VALUES
  ('conformance', 'eun_1', 'est_1', 'Flat 1'),
  ('conformance', 'eun_2', 'est_2', 'Flat 2');

INSERT INTO "memos" ("$space", "id", "owner", "published", "pinned") VALUES
  ('conformance', 'memo_1', 'mem_1', true,  false),
  ('conformance', 'memo_2', 'mem_1', false, true),
  ('conformance', 'memo_3', 'mem_1', false, false),
  ('conformance', 'memo_4', 'mem_2', true,  true);

INSERT INTO "notices" ("$space", "id", "estate", "owner", "urgent") VALUES
  ('conformance', 'notice_1', 'est_1', 'mem_1', true),
  ('conformance', 'notice_2', 'est_1', 'mem_1', false),
  ('conformance', 'notice_3', 'est_2', 'mem_1', true);
"#;

const ANA: Option<&str> = Some("mem_1");
const BRAM: Option<&str> = Some("mem_2");
const CLEO: Option<&str> = Some("mem_3");
const DOV: Option<&str> = Some("mem_4");

fn estate_fixture() -> Fixture {
    let src = format!("{FIXTURE}\n{ESTATE}");
    Fixture::from_source(&src, SEED).expect("the fixture plus the estate cluster compiles")
}

/// This suite's read database, built exactly once.
///
/// `Fixture::reset` drops and rebuilds `public`, and two of those at once
/// race inside Postgres's own catalog — so the read cases (which mutate
/// nothing) share one database behind a `OnceLock` rather than each
/// rebuilding their own. The write case owns a second one.
fn seeded_reads(f: &Fixture) -> Pg {
    static ONCE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let pg = db("ordo_9e_groups_read");
    ONCE.get_or_init(|| {
        f.reset(&pg)?;
        pg.exec(ESTATE_SEED)
    })
    .clone()
    .expect("fixture DDL applies and seeds");
    pg
}

/// A fresh seed for one write invocation (§3.5).
fn reseed(f: &Fixture, pg: &Pg) {
    f.reset(pg).expect("fresh seed");
    pg.exec(ESTATE_SEED).expect("the estate rows seed");
}

fn read(f: &Fixture, pg: &Pg, src: &str, as_: Option<&str>) -> Value {
    f.run_case_as(pg, src, &[], as_)
        .unwrap_or_else(|e| panic!("`{src}` as {as_:?} did not run: {e}"))
}

/// The idiom, on the READ side: the level group is not on this walk, so
/// every access row admits. This is the control — it fixes what "can see
/// the estate at all" means, so the write assertions below are about the
/// group and not about visibility.
#[test]
fn every_access_level_reads_the_units_it_can_see() {
    let f = estate_fixture();
    let pg = seeded_reads(&f);

    for who in [ANA, BRAM, CLEO] {
        assert_eq!(
            read(&f, &pg, "unitList", who),
            json!([{"name": "Flat 1"}]),
            "{who:?} holds an est_1 access row, at any level"
        );
    }
    assert_eq!(
        read(&f, &pg, "unitList", DOV),
        json!([{"name": "Flat 2"}]),
        "Dov's row is in est_2, so he sees est_2's unit and nothing else"
    );
    assert_eq!(
        read(&f, &pg, "unitList", None),
        json!([]),
        "anonymous: every guard false (G15)"
    );
}

/// **The case.** `member == $caller and (level == "write" or level ==
/// "admin")`, both directions, one caller per outcome.
#[test]
fn the_level_group_admits_write_and_admin_and_refuses_read() {
    let f = estate_fixture();
    let pg = db("ordo_9e_groups_write");

    // Permitted: the group's left branch (`admin`) and its right (`write`).
    for (who, label, name) in [
        (ANA, "admin", "Ana's rename"),
        (BRAM, "write", "Bram's rename"),
    ] {
        reseed(&f, &pg);
        let mut log = WriteLog::new();
        let out = f
            .run_write_as(
                &pg,
                &mut log,
                &format!("renameUnit($u: \"eun_1\", $name: \"{name}\")"),
                who,
            )
            .unwrap_or_else(|e| panic!("{label} may write: {e}"));
        assert_eq!(
            out,
            json!([{"entity": "estateUnits", "updated": 1}]),
            "{label}"
        );
        assert_eq!(
            pg.query("SELECT \"name\" FROM \"estateUnits\" WHERE \"id\" = 'eun_1'")
                .expect("readback"),
            vec![vec![Some(name.to_string())]],
            "{label}: the row actually moved"
        );
    }

    // Refused: Cleo holds an est_1 access row at `read`, so she satisfies
    // the `$caller` conjunct and NEITHER branch of the group.
    //
    // This single assertion is what separates a correct lowering from all
    // three wrong ones: she reads `eun_1` fine (so it is not visibility),
    // and est_1 has an admin row (Ana's) for a parens-dropping lowering's
    // second disjunct to latch onto.
    reseed(&f, &pg);
    let mut log = WriteLog::new();
    assert_eq!(
        read(&f, &pg, "unitList", CLEO),
        json!([{"name": "Flat 1"}]),
        "Cleo can READ the row she is about to be refused a write on"
    );
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            "renameUnit($u: \"eun_1\", $name: \"Cleo's rename\")",
            CLEO,
        )
        .expect_err("`read` satisfies neither branch of the group");
    assert_eq!(class_of(&e), "access_guard", "{e}");
    assert_eq!(
        pg.query("SELECT \"name\" FROM \"estateUnits\" WHERE \"id\" = 'eun_1'")
            .expect("readback"),
        vec![vec![Some("Flat 1".to_string())]],
        "and nothing was written"
    );

    // Dov is admin — of the OTHER estate. The `$caller` conjunct and the
    // group are both satisfied, by different rows; the walk requires one
    // row satisfying both, and est_1 has none for him.
    let e = f
        .run_write_as(
            &pg,
            &mut log,
            "renameUnit($u: \"eun_1\", $name: \"Dov's rename\")",
            DOV,
        )
        .expect_err("admin elsewhere is not admin here");
    assert_eq!(
        class_of(&e),
        "not_found",
        "he cannot see eun_1 at all, so R-G10 answers before the guard: {e}"
    );
}

/// The top-level shape: `access read (a or b) and c`. The group is the
/// left operand of the `and`, which is the arrangement a flattening
/// lowering gets wrong in the *permissive* direction.
#[test]
fn a_top_level_group_conjoins_with_the_caller_test() {
    let f = estate_fixture();
    let pg = seeded_reads(&f);

    // Ana owns memo_1 (published), memo_2 (pinned) and memo_3 (neither).
    // She sees the first two and NOT the third: the group is a real
    // conjunct, not decoration.
    assert_eq!(
        read(&f, &pg, "memoList", ANA),
        json!([{"name": "memo_1"}, {"name": "memo_2"}]),
        "both branches of the group admit; memo_3 satisfies neither"
    );
    // memo_4 is published AND pinned, so the group is doubly true — and
    // Bram, not Ana, owns it. A lowering that dropped the `and` would show
    // it to Ana; a semi-join union would show it to her twice.
    assert!(
        !read(&f, &pg, "memoList", ANA)
            .as_array()
            .expect("rows")
            .iter()
            .any(|r| r["name"] == "memo_4"),
        "the group being true does not make somebody else's memo hers"
    );
    assert_eq!(
        read(&f, &pg, "memoList", BRAM),
        json!([{"name": "memo_4"}]),
        "Bram owns exactly one memo and it satisfies the group; a row \
         qualifying through BOTH disjuncts appears once, not twice"
    );
    assert_eq!(
        read(&f, &pg, "memoList", CLEO),
        json!([]),
        "Cleo owns nothing: the caller conjunct is what gates the group"
    );
}

/// The lowering, asserted on the SQL — because the row assertions above
/// cannot tell a correct guard from a correct-on-this-seed one.
///
/// The group here sits INSIDE the reverse step, so the two normalized
/// paths land as an `OR` inside that step's single `EXISTS`, and the
/// nesting is one `EXISTS` per HOP of the walk. What must be true is that
/// `member == $caller` rides in both paths.
#[test]
fn the_group_lowers_to_one_semi_join_per_normalized_path() {
    let f = estate_fixture();
    let g = f
        .schema
        .entity("estateUnits")
        .expect("estateUnits is in the schema")
        .guards
        .as_ref()
        .expect("it declares its own clauses");
    let ordo_schema::GuardBody::Walk(w) = &g.write else {
        panic!("a walk guard")
    };
    // The group is INSIDE the reverse step, so the outer walk has one path
    // and the step's inner condition has two: `member == $caller` conjoined
    // with each branch of the level group, separately.
    assert_eq!(w.disjuncts.len(), 1, "one outer path: the step");
    let [ordo_syntax::GuardAtom::Step { inner, .. }] = &w.disjuncts[0][..] else {
        panic!("a reverse collection step")
    };
    assert_eq!(
        inner.disjuncts.len(),
        2,
        "one path per branch of the level group, distributed"
    );
    for conj in &inner.disjuncts {
        assert!(
            conj.iter()
                .any(|a| matches!(a, ordo_syntax::GuardAtom::Cmp { caller: true, .. })),
            "every distributed path keeps the `$caller` conjunct"
        );
    }

    let q = ordo_sema::guard_filter_query(&f.schema, "estateUnits", true, Some("mem_2"))
        .expect("the write guard binds")
        .expect("estateUnits is guarded on write");
    let sql = ordo_plan::plan(&q, &f.plan_space()).expect("it plans").sql;
    assert_eq!(
        sql.matches("EXISTS").count(),
        2,
        "one semi-join per HOP of the walk — `estates`, then `estatesAccess`; \
         the two paths share the step they were distributed inside:\n{sql}"
    );
    assert!(
        sql.contains(
            "((\"t2\".\"member\" = 'mem_2') \
             AND (\"t2\".\"level\" = 'write')) \
             OR ((\"t2\".\"member\" = 'mem_2') \
             AND (\"t2\".\"level\" = 'admin'))"
        ),
        "the group is an OR of two complete paths, and `member == $caller` \
         rides in BOTH — a distribution that drops it from one is the hole \
         this whole suite exists to catch:\n{sql}"
    );
}

/// The other arrangement: a group at the top of a clause, one of whose
/// branches is a reverse STEP. Distribution duplicates the step, so the
/// lowering really is a union of semi-joins here.
///
/// Both directions again, and the negative is the one that matters: the
/// `and` must survive distribution into every path, or `urgent` alone
/// opens the row to a caller with no access to its estate.
#[test]
fn a_top_level_group_that_distributes_a_step() {
    let f = estate_fixture();
    let pg = seeded_reads(&f);

    // Ana holds est_1 access and owns all three notices.
    assert_eq!(
        read(&f, &pg, "noticeList", ANA),
        json!([{"name": "notice_1"}, {"name": "notice_2"}]),
        "notice_3 is in est_2, where she has no access row: the second \
         conjunct is real in every path"
    );
    // Cleo holds est_1 access and owns nothing: only the `urgent` path.
    assert_eq!(
        read(&f, &pg, "noticeList", CLEO),
        json!([{"name": "notice_1"}]),
        "urgent admits her to notice_1; notice_2 is neither urgent nor hers"
    );
    // Dov holds est_2 access and owns nothing: the mirror image.
    assert_eq!(
        read(&f, &pg, "noticeList", DOV),
        json!([{"name": "notice_3"}]),
        "the same urgent path, in the estate he can actually reach"
    );

    let q = ordo_sema::guard_filter_query(&f.schema, "notices", false, Some("mem_1"))
        .expect("the read guard binds")
        .expect("notices is guarded on read");
    let sql = ordo_plan::plan(&q, &f.plan_space()).expect("it plans").sql;
    assert_eq!(
        sql.matches("\"estatesAccess\"").count(),
        2,
        "the step is duplicated once per normalized path — that IS the \
         union of semi-joins:\n{sql}"
    );
    assert_eq!(
        sql.matches("\"urgent\"").count(),
        1,
        "and the branch that is not a step appears in exactly its own \
         path:\n{sql}"
    );
}
