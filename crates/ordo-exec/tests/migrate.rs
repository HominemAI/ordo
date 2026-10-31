//! Increment 1 of `plan-schema-evolution.md`, executed: the constraint half
//! of the drift gate, the change classifier, and the probes — every one of
//! them against a real Postgres, because every one of them is a claim about
//! what Postgres reports.
//!
//! # Why the live shape is read from Postgres rather than built in Rust
//!
//! `ordo_schema::diff` decides whether a declaration is deployed by looking
//! for the object NAME `ddl_for` mints for it (`orders$total$uq`,
//! `orders$a$b$ix`). A test that hand-built a `LiveShape` with those names in
//! it would be asserting that the diff agrees with the test author about the
//! naming convention. What has to be true is that it agrees with **the
//! database after the DDL has actually run**, and the only way to assert that
//! is to run it.
//!
//! So every case here is a two-deploy story told in full: apply program P's
//! generated DDL to an empty schema, read the shape back out of the catalog,
//! and ask what deploying program Q would be.
//!
//! Databases are prefixed `ordo_mig_` and one per test function: test
//! functions in one binary run concurrently and each of these opens with
//! `DROP SCHEMA public CASCADE`.

use ordo_conformance::Pg;
use ordo_exec::migrate;
use ordo_exec::{Pool, PoolConfig};
use ordo_schema::diff::{Change, Class};
use ordo_schema::{build, ddl_for, Schema};
use ordo_syntax::parse_program;

// ── scaffolding ──────────────────────────────────────────────────────────

fn schema_of(src: &str) -> Schema {
    build(&parse_program(src).expect("parses")).expect("builds")
}

fn ddl_of(src: &str) -> String {
    ddl_for(&schema_of(src)).expect("ddl")
}

async fn pool_for(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots")
}

/// An empty `public` schema holding exactly what `src` declares.
fn deploy(pg: &Pg, src: &str) {
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("schema reset");
    pg.exec(&ddl_of(src)).expect("the generated DDL applies");
}

// ── the classifier, against the database its names have to match ─────────

/// One row of `plan-schema-evolution.md` §5, as a deploy.
struct Case {
    /// The taxonomy code the plan's table assigns.
    code: &'static str,
    class: Class,
    /// What is deployed now.
    prior: &'static str,
    /// What the program declares.
    declared: &'static str,
}

/// The base program every case deploys first, so a case's source shows only
/// its own change.
const BASE: &str = "\
entity a {
  name: text
  n: int
  note: text?
  secret: text sealed
}
";

/// `plan-schema-evolution.md` §5's four class tables, one case per row that
/// a schema-to-schema diff can reach.
///
/// The rows that are deliberately absent, with the plan's own reason:
///
/// - **A5** (new op, changed body, changed guard) and **A6** (`acl`) are not
///   schema-shape changes. A5 is artifact-only — it is counted under the
///   report's GRANTS section — and A6 derives a link entity, which arrives
///   here as A1.
/// - **B4** (rename) reaches a live shape as a drop and an add and stays
///   that way: §5 B4 requires a declared mapping, and mappings are not in
///   increment 1.
/// - **C4** and **D7** are a probe over a declared mapping, for the same
///   reason.
/// - **D3** and **D5** are refusals the *report* raises, not classes the
///   diff assigns; they are tested in `ordo-cli`'s suite, which is where the
///   serving artifact is in scope.
const CASES: &[Case] = &[
    // ── A ──
    Case {
        code: "A1",
        class: Class::A,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text sealed }\n\
                   entity b { title: text }",
    },
    Case {
        code: "A2",
        class: Class::A,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text sealed \
                   memo: text? }",
    },
    Case {
        code: "A3",
        class: Class::A,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text sealed \
                   active: bool = true }",
    },
    Case {
        code: "A4",
        class: Class::A,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text sealed \
                   index (name, n) }",
    },
    // ── B ──
    Case {
        code: "B1",
        class: Class::B,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text sealed \
                   made: date = today }",
    },
    Case {
        code: "B2",
        class: Class::B,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text sealed \
                   code: text }",
    },
    Case {
        code: "B3",
        class: Class::B,
        prior: BASE,
        declared: "entity a { name: text n: text note: text? secret: text sealed }",
    },
    Case {
        code: "B5",
        class: Class::B,
        prior: BASE,
        declared: "entity a { name: text n: int note: text secret: text sealed }",
    },
    Case {
        code: "B6",
        class: Class::B,
        prior: BASE,
        declared: "entity a { name: text? n: int note: text? secret: text sealed }",
    },
    Case {
        code: "B7",
        class: Class::B,
        prior: BASE,
        declared: "entity a { name: text sealed n: int note: text? secret: text sealed }",
    },
    Case {
        code: "B8",
        class: Class::B,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text sealed indexed }",
    },
    Case {
        code: "B9",
        class: Class::B,
        prior: "entity a { name: text n: int note: text? secret: text sealed \
                parent -> a? }",
        declared: "entity a { name: text n: int note: text? secret: text sealed \
                   parent -> a? tree }",
    },
    Case {
        code: "B10",
        class: Class::B,
        prior: "entity a { name: text n: int note: text? secret: text sealed indexed }",
        declared: "entity a { name: text n: int note: text? \
                   secret: text sealed indexed normalize(lower) }",
    },
    // 25aa: every `money(p)` is `bigint`, so this one is found only from
    // the prior declaration.
    Case {
        code: "B11",
        class: Class::B,
        prior: "entity a { name: text price: money(0) }",
        declared: "entity a { name: text price: money(2) }",
    },
    // And a decimal's scale, which the live column carries as well.
    Case {
        code: "B11",
        class: Class::B,
        prior: "entity a { name: text rate: decimal(2) }",
        declared: "entity a { name: text rate: decimal(4) }",
    },
    // ── C ──
    Case {
        code: "C1",
        class: Class::C,
        prior: BASE,
        declared: "entity a { name: text unique n: int note: text? secret: text sealed }",
    },
    Case {
        code: "C2",
        class: Class::C,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text sealed \
                   unique (name, n) }",
    },
    Case {
        code: "C5",
        class: Class::C,
        prior: "entity b { title: text }\n\
                entity a { name: text n: int note: text? secret: text sealed }",
        declared: "entity b { title: text }\n\
                   entity a { name: text n: int note: text? secret: text sealed \
                   board -> b? }",
    },
    Case {
        code: "C6",
        class: Class::C,
        prior: "entity a { name: text unique n: int note: text? secret: text sealed }",
        declared: BASE,
    },
    // The three drops the plan's §5 table does not enumerate, because it is
    // written in the forward direction. They are classified here and the
    // codes are recorded in the plan's increment-1 landed note.
    Case {
        code: "A7",
        class: Class::A,
        prior: "entity a { name: text n: int note: text? secret: text sealed \
                index (name, n) }",
        declared: BASE,
    },
    Case {
        code: "C7",
        class: Class::C,
        prior: "entity a { name: text n: int note: text? secret: text sealed \
                unique (name, n) }",
        declared: BASE,
    },
    Case {
        code: "D10",
        class: Class::D,
        prior: "entity b { title: text }\n\
                entity a { name: text n: int note: text? secret: text sealed board -> b? }",
        declared: "entity b { title: text }\n\
                   entity a { name: text n: int note: text? secret: text sealed \
                   board: text? }",
    },
    // ── D ──
    Case {
        code: "D1",
        class: Class::D,
        prior: BASE,
        declared: "entity a { name: text n: int secret: text sealed }",
    },
    Case {
        code: "D2",
        class: Class::D,
        prior: "entity a { name: text n: int note: text? secret: text sealed }\n\
                entity b { title: text }",
        declared: BASE,
    },
    Case {
        code: "D4",
        class: Class::D,
        prior: BASE,
        declared: "entity a (key: aaa) { name: text n: int note: text? secret: text sealed }",
    },
    Case {
        code: "D6",
        class: Class::D,
        prior: BASE,
        declared: "entity a { name: text n: int note: text? secret: text }",
    },
    Case {
        code: "D8",
        class: Class::D,
        prior: "entity a { name: text n: int note: text? secret: text sealed indexed }",
        declared: BASE,
    },
    Case {
        code: "D9",
        class: Class::D,
        prior: "entity a { name: text n: int note: text? secret: text sealed \
                parent -> a? tree }",
        declared: "entity a { name: text n: int note: text? secret: text sealed \
                   parent -> a? }",
    },
];

/// The taxonomy, one deploy per row.
///
/// Asserted as *"the change list contains exactly one change with this code,
/// and its class is the one the plan's table assigns"* — not as an exact
/// change list, because some rows legitimately move two things (`D6`
/// unsealing a field also drops the constraint that rode it, and saying so
/// is correct).
#[tokio::test(flavor = "multi_thread")]
async fn every_taxonomy_row_classifies_as_the_plan_says() {
    let pg = Pg::named("ordo_mig_diff");
    let pool = pool_for("ordo_mig_diff").await;

    for case in CASES {
        deploy(&pg, case.prior);
        let live = migrate::live_shape(&pool, "public")
            .await
            .expect("the live shape reads");
        let prior = schema_of(case.prior);
        let declared = schema_of(case.declared);
        let changes = ordo_schema::diff(&live, Some(&prior), &declared);

        let hits: Vec<&Change> = changes.iter().filter(|c| c.code() == case.code).collect();
        assert_eq!(
            hits.len(),
            1,
            "{}: expected exactly one {} change, got {:?}",
            case.code,
            case.code,
            changes
                .iter()
                .map(|c| format!("{} {c}", c.code()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            hits[0].class(),
            case.class,
            "{} is classified {} and the plan says {}",
            case.code,
            hits[0].class(),
            case.class
        );
    }
}

/// The two refusals §5 rules on by name, reached through a real deploy
/// rather than by constructing the `Change` that carries them.
#[tokio::test(flavor = "multi_thread")]
async fn the_two_permanent_refusals_are_raised_by_the_diff() {
    let pg = Pg::named("ordo_mig_refuse");
    let pool = pool_for("ordo_mig_refuse").await;

    for (src, expect) in [
        (
            "entity a (key: aaa) { name: text n: int note: text? secret: text sealed }",
            "key_change_refused",
        ),
        (
            "entity a { name: text n: int note: text? secret: text }",
            "unseal_refused",
        ),
    ] {
        deploy(&pg, BASE);
        let live = migrate::live_shape(&pool, "public").await.expect("shape");
        let prior = schema_of(BASE);
        let changes = ordo_schema::diff(&live, Some(&prior), &schema_of(src));
        assert!(
            changes.iter().any(|c| c.refusal() == Some(expect)),
            "expected `{expect}`, got {:?}",
            changes
                .iter()
                .map(|c| format!("{} {c}", c.code()))
                .collect::<Vec<_>>()
        );
    }
}

/// The module doc's central claim, asserted rather than trusted: **with no
/// prior declaration there are no drops.**
///
/// The live schema holds the platform's own tables — `$commandlog`,
/// `$artifacts`, `$sealedpayload` — and a diff that read "live has a table
/// the program does not declare" as *drop it* would propose destroying the
/// audit log on its first run. So the platform tables are put in the live
/// schema here, deliberately, and the diff must stay silent about them.
#[tokio::test(flavor = "multi_thread")]
async fn with_no_prior_artifact_nothing_is_ever_dropped() {
    let pg = Pg::named("ordo_mig_noprior");
    // Deployed BEFORE the pool boots: `Pool::connect` reads the platform
    // tables, and this test deliberately leaves a malformed one behind.
    // A richer deployed schema than the program declares: an extra entity,
    // an extra field, an extra unique, an extra index — every shape a drop
    // could be inferred from.
    deploy(
        &pg,
        "entity a { name: text unique n: int gone: text? index (name, n) }\n\
         entity vanished { title: text }",
    );
    pg.exec("CREATE TABLE \"$migrations\" (name text, phase text)")
        .expect("a platform table the program never declares");
    let pool = pool_for("ordo_mig_noprior").await;

    let live = migrate::live_shape(&pool, "public").await.expect("shape");
    let changes = ordo_schema::diff(&live, None, &schema_of("entity a { name: text n: int }"));
    let drops: Vec<String> = changes
        .iter()
        .filter(|c| c.class() == Class::D || c.code().starts_with("Drop"))
        .map(|c| format!("{} {c}", c.code()))
        .collect();
    assert!(
        drops.is_empty(),
        "a diff with no prior artifact proposed drops: {drops:?}"
    );
    assert!(
        changes.is_empty(),
        "nothing the program declares is missing, so there is nothing to say: {:?}",
        changes
            .iter()
            .map(|c| format!("{} {c}", c.code()))
            .collect::<Vec<_>>()
    );
}

// ── defect (b): the gate can see constraints ─────────────────────────────

/// The whole of `plan-schema-evolution.md` §1.1(b), one constraint at a
/// time.
///
/// Before this, `live_drift` read `information_schema.columns` and nothing
/// else. Every case below is a change that was **invisible to the gate**: a
/// deployment whose uniqueness constraint had been dropped behind its back
/// loaded, served, and accepted the duplicate write it was built to refuse.
///
/// The shadow is built from the artifact's own DDL and the live schema is
/// then damaged by hand — which is exactly the "changed it behind the
/// schema's back" case §0.1 says the gate exists for.
#[tokio::test(flavor = "multi_thread")]
async fn live_drift_names_a_constraint_dropped_behind_the_schemas_back() {
    let src = "entity a { name: text unique n: int note: text? index (name, n) }";
    let pg = Pg::named("ordo_mig_drift");
    let pool = pool_for("ordo_mig_drift").await;
    let ddl = ddl_of(src);

    for (damage, object, kind) in [
        (
            "ALTER TABLE a DROP CONSTRAINT \"a$name$uq\"",
            "a$name$uq",
            "unique constraint",
        ),
        ("DROP INDEX \"a$name$n$ix\"", "a$name$n$ix", "index"),
        (
            "ALTER TABLE a DROP CONSTRAINT \"a$pk\"",
            "a$pk",
            "primary key",
        ),
    ] {
        deploy(&pg, src);
        pool.build_shadow(&ddl).await.expect("shadow builds");
        // Clean first: the shadow and the live schema are the same DDL, so
        // anything reported here is the comparison itself being wrong.
        let clean = pool
            .live_drift("ordo_shadow", "public")
            .await
            .expect("read");
        assert!(
            clean.is_empty(),
            "an undamaged deployment reported drift: {clean:?}"
        );

        pg.exec(damage).expect("the damage applies");
        let drift = pool
            .live_drift("ordo_shadow", "public")
            .await
            .expect("read");
        let hit = drift
            .iter()
            .find(|d| d.object.as_deref() == Some(object))
            .unwrap_or_else(|| panic!("`{object}` was dropped and the gate said {drift:?}"));
        assert!(
            hit.detail.contains(kind),
            "the refusal should name what kind of thing is missing, got `{}`",
            hit.detail
        );
        assert_eq!(hit.column, None, "a constraint drift is not a column drift");
    }
}

/// The half of §1.1(b) that is Ordo-specific, and the half a
/// present/absent check would miss entirely.
///
/// DL's `null == null` is TRUE, so `ddl_for` emits `UNIQUE NULLS NOT
/// DISTINCT` and a nullable unique column admits **at most one null row per
/// space**. Postgres' default is the opposite. So a constraint that is
/// present, named correctly, and over the right columns can still mean the
/// SQL thing rather than the DL thing — and the deployment would serve,
/// accepting a second null row its own language says is a duplicate.
///
/// The recreated constraint below is byte-identical to the generated one
/// except for those three words.
#[tokio::test(flavor = "multi_thread")]
async fn live_drift_sees_the_nulls_not_distinct_half() {
    let src = "entity a { name: text? unique n: int }";
    let pg = Pg::named("ordo_mig_nnd");
    let pool = pool_for("ordo_mig_nnd").await;
    let ddl = ddl_of(src);
    assert!(
        ddl.contains("NULLS NOT DISTINCT"),
        "the generated DDL should carry the modifier this test is about"
    );

    deploy(&pg, src);
    pool.build_shadow(&ddl).await.expect("shadow builds");

    // Same name, same columns, SQL's null rule instead of DL's.
    pg.exec("ALTER TABLE a DROP CONSTRAINT \"a$name$uq\"")
        .expect("drop");
    pg.exec("ALTER TABLE a ADD CONSTRAINT \"a$name$uq\" UNIQUE (\"$space\", \"name\")")
        .expect("recreate without the modifier");

    let drift = pool
        .live_drift("ordo_shadow", "public")
        .await
        .expect("read");
    let hit = drift
        .iter()
        .find(|d| d.object.as_deref() == Some("a$name$uq"))
        .unwrap_or_else(|| panic!("the modifier was dropped and the gate said {drift:?}"));
    assert!(
        hit.detail.contains("NULLS NOT DISTINCT"),
        "the refusal must name the modifier, because that is the whole \
         difference between the constraint meaning what the language says and \
         meaning what SQL says; got `{}`",
        hit.detail
    );

    // And the null rule itself, so the test is about a real consequence and
    // not only about a string: two null rows are what the modifier forbids.
    pg.exec("ALTER TABLE a DROP CONSTRAINT \"a$name$uq\"")
        .expect("drop");
    pg.exec(ddl.lines().find(|l| l.contains("a$name$uq")).unwrap())
        .expect("the generated constraint re-applies");
    pg.exec("INSERT INTO a (\"$space\", id, rev, n) VALUES ('s', 'a_1', 1, 1)")
        .expect("one null row is admissible");
    let second = pg.exec("INSERT INTO a (\"$space\", id, rev, n) VALUES ('s', 'a_2', 1, 2)");
    assert!(
        second.is_err(),
        "under NULLS NOT DISTINCT a second null row is a duplicate, and every \
         other SQL system would have accepted it"
    );
}

/// The one-way rule, which the constraint half must not have broken.
///
/// §4: extra live constraints are **not** drift, because a not-yet-dropped
/// constraint is exactly the state expand-contract leaves the database in
/// between deploy 2 and deploy 3. A gate that called it drift would refuse
/// every deployment in the middle of a migration.
#[tokio::test(flavor = "multi_thread")]
async fn an_extra_live_constraint_is_not_drift() {
    let src = "entity a { name: text n: int }";
    let pg = Pg::named("ordo_mig_extra");
    let pool = pool_for("ordo_mig_extra").await;

    deploy(&pg, src);
    pool.build_shadow(&ddl_of(src)).await.expect("shadow");
    pg.exec("ALTER TABLE a ADD COLUMN \"leftover\" text")
        .expect("an extra column");
    pg.exec("CREATE INDEX \"a$leftover$ix\" ON a (\"leftover\")")
        .expect("an extra index");
    pg.exec("ALTER TABLE a ADD CONSTRAINT \"a$leftover$uq\" UNIQUE (\"$space\", \"leftover\")")
        .expect("an extra constraint");
    pg.exec("CREATE TABLE \"leftover_table\" (x text)")
        .expect("an extra table");

    let drift = pool
        .live_drift("ordo_shadow", "public")
        .await
        .expect("read");
    assert!(
        drift.is_empty(),
        "the one-way rule was broken — expand-contract leaves exactly this state \
         between the swap and the drop: {drift:?}"
    );
}

// ── the probes ───────────────────────────────────────────────────────────

/// §5's rule for all of C: *the declaration is not the question; the live
/// data is.* Each probe is asked over a database seeded to violate it.
#[tokio::test(flavor = "multi_thread")]
async fn the_probes_answer_from_the_data_and_never_from_the_declaration() {
    let pg = Pg::named("ordo_mig_probe");
    let pool = pool_for("ordo_mig_probe").await;
    deploy(
        &pg,
        "entity b { title: text }\n\
         entity a { name: text? n: int board: id? status: text }",
    );
    pg.exec(
        "INSERT INTO b (\"$space\", id, rev, title) VALUES ('s1', 'b_1', 1, 't');\n\
         INSERT INTO a (\"$space\", id, rev, name, n, board, status) VALUES \
           ('s1', 'a_1', 1, 'dup',  1, 'b_1',  'open'), \
           ('s1', 'a_2', 1, 'dup',  1, 'b_1',  'open'), \
           ('s1', 'a_3', 1, NULL,   2, NULL,   'open'), \
           ('s1', 'a_4', 1, NULL,   3, 'b_99', 'weird'), \
           ('s2', 'a_5', 1, 'dup',  1, NULL,   'open');",
    )
    .expect("seed");

    // C1 — duplicates, inside a space and never across one. `s2` holds the
    // same value as `s1` and that is not a collision: the space leads every
    // generated unique index without exception.
    let dups = migrate::duplicates(&pool, "a", &["name".into()], 8)
        .await
        .expect("probe");
    let s1: Vec<&migrate::Duplicate> = dups.iter().filter(|d| d.space == "s1").collect();
    assert!(
        !dups.iter().any(|d| d.space == "s2"),
        "one row per space is not a duplicate: {dups:?}"
    );
    assert!(
        s1.iter()
            .any(|d| d.value.as_deref() == Some("dup") && d.rows == 2),
        "the shared value was not reported: {dups:?}"
    );
    // The null rule, which is the case operators do not expect: two null
    // rows collide under `NULLS NOT DISTINCT`, and `value` is `None` so the
    // report knows to say the surprising thing.
    assert!(
        s1.iter().any(|d| d.value.is_none() && d.rows == 2),
        "the two null rows are a duplicate under DL's `null == null` and were \
         not reported: {dups:?}"
    );

    // C2 — the same question over a tuple.
    let tuple = migrate::duplicates(&pool, "a", &["name".into(), "n".into()], 8)
        .await
        .expect("probe");
    assert_eq!(
        tuple.len(),
        1,
        "only ('dup', 1) repeats inside a space: {tuple:?}"
    );

    // C3 — values outside a set. NULL is not one: the generated
    // `CHECK (col IN (…))` is unknown-not-false for a null, so Postgres
    // admits it and a probe that refused it would refuse a plan the
    // database would accept.
    let out = migrate::out_of_enum(&pool, "a", "status", &["open".into(), "done".into()], 8)
        .await
        .expect("probe");
    assert_eq!(out.len(), 1, "exactly one offending value: {out:?}");
    assert_eq!(out[0].value.as_deref(), Some("weird"));
    let none_out = migrate::out_of_enum(&pool, "a", "name", &["dup".into()], 8)
        .await
        .expect("probe");
    assert!(
        none_out.is_empty(),
        "nulls are admitted by the generated CHECK and must not be reported \
         as violations: {none_out:?}"
    );

    // C5 — a ref with no target IN THE SAME SPACE. `a_5` names nothing, and
    // `a_4` names `b_99`, which exists nowhere.
    let dangling = migrate::dangling_refs(&pool, "a", "board", "b")
        .await
        .expect("probe");
    assert_eq!(dangling.len(), 1, "one space is affected: {dangling:?}");
    assert_eq!(dangling[0].space, "s1");
    assert_eq!(dangling[0].rows, 1, "only `b_99` dangles");

    // The same-space half, which is the point and not an optimisation: a
    // ref naming another tenant's row is dangling as far as the generated
    // `(space, ref) REFERENCES t (space, id)` is concerned.
    pg.exec(
        "INSERT INTO a (\"$space\", id, rev, n, board, status) \
             VALUES ('s2', 'a_6', 1, 9, 'b_1', 'open')",
    )
    .expect("a cross-space ref");
    let cross = migrate::dangling_refs(&pool, "a", "board", "b")
        .await
        .expect("probe");
    assert!(
        cross.iter().any(|d| d.space == "s2" && d.rows == 1),
        "a ref at another tenant's row is dangling, and a probe that joined on \
         id alone would have called it valid: {cross:?}"
    );

    // B5 — nulls, per space, because the operator's question is which
    // tenants a backfill has to touch.
    let nulls = migrate::nulls(&pool, "a", "name").await.expect("probe");
    assert_eq!(
        nulls.iter().find(|n| n.space == "s1").map(|n| n.rows),
        Some(2),
        "two rows of `s1` hold null: {nulls:?}"
    );

    // C4 — a collision the MAPPING creates, which is the only interesting
    // kind. Recase one of the pair: the raw column now holds two distinct
    // values and a `unique` over it would be clean, while `lower` maps them
    // onto one. §5 C4 refuses that rather than choosing a survivor.
    pg.exec("UPDATE a SET name = 'Dup' WHERE id = 'a_2'")
        .expect("recase one of the pair");
    let raw = migrate::duplicates(&pool, "a", &["name".into()], 8)
        .await
        .expect("probe");
    assert!(
        !raw.iter().any(|d| d.space == "s1" && d.value.is_some()),
        "`dup` and `Dup` are two distinct values, so the column itself is now \
         clean: {raw:?}"
    );
    let collide = migrate::collisions(&pool, "a", "name", "lower({})", 8)
        .await
        .expect("probe");
    assert!(
        collide
            .iter()
            .any(|d| d.space == "s1" && d.value.as_deref() == Some("dup") && d.rows == 2),
        "`dup` and `Dup` are two rows before `normalize(lower)` and one after, \
         and a merge under a unique is a data-loss path: {collide:?}"
    );

    // §9's disclosure obligation: the two numbers `ordo plan` must print
    // before anything runs.
    let extent = migrate::extent(&pool, "a").await.expect("probe");
    assert_eq!(extent.rows, 6);
    assert_eq!(extent.spaces, 2, "the operator's real question is tenants");
    let rate = migrate::chunk_rate(&pool, "a", 5_000).await.expect("probe");
    assert!(
        rate.is_some_and(|r| r > 0.0),
        "a measured rate, not a constant compiled in on somebody else's machine"
    );
    assert_eq!(
        migrate::freeze_estimate(0, None),
        "unknown (the scan rate could not be measured)",
        "an unmeasurable rate must not render as a confident zero"
    );
}

/// The probes write nothing. Stated in the module doc of `migrate.rs` as
/// *"nothing here writes, ever — not a temp table, not a `SET`, not an
/// `ANALYZE`"*, and asserted here because it is the property that makes
/// `ordo plan --against` safe to point at production.
#[tokio::test(flavor = "multi_thread")]
async fn every_probe_leaves_the_database_exactly_as_it_found_it() {
    let pg = Pg::named("ordo_mig_readonly");
    let pool = pool_for("ordo_mig_readonly").await;
    deploy(
        &pg,
        "entity b { title: text }\nentity a { name: text? board: id? }",
    );
    pg.exec(
        "INSERT INTO b (\"$space\", id, rev, title) VALUES ('s1','b_1',1,'t');\n\
         INSERT INTO a (\"$space\", id, rev, name, board) VALUES ('s1','a_1',1,'x','b_1');",
    )
    .expect("seed");

    let fingerprint = |pg: &Pg| {
        let rows = pg
            .query(
                "SELECT string_agg(c.relname || ':' || c.relkind::text, ',' ORDER BY c.relname) \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = 'public'",
            )
            .expect("catalog reads");
        let data = pg
            .query("SELECT (SELECT count(*) FROM a) || '/' || (SELECT count(*) FROM b)")
            .expect("counts read");
        (rows, data)
    };
    let before = fingerprint(&pg);

    let _ = migrate::live_shape(&pool, "public").await.expect("shape");
    let _ = migrate::duplicates(&pool, "a", &["name".into()], 8).await;
    let _ = migrate::nulls(&pool, "a", "name").await;
    let _ = migrate::out_of_enum(&pool, "a", "name", &["x".into()], 8).await;
    let _ = migrate::dangling_refs(&pool, "a", "board", "b").await;
    let _ = migrate::collisions(&pool, "a", "name", "lower({})", 8).await;
    let _ = migrate::extent(&pool, "a").await;
    let _ = migrate::chunk_rate(&pool, "a", 5_000).await;

    assert_eq!(
        before,
        fingerprint(&pg),
        "a probe changed the database it was asked to describe"
    );
}
