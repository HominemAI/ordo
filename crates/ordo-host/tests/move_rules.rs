//! **Moves through a link that is not unique, by a named rule**
//! (`forward-gaps` ask 7, owner: *"dev named"*), against real Postgres:
//!
//! - `address.person -> person owned` gives a person several addresses.
//!   `street: text? prev(address.street)` in `person` takes the street of
//!   the address the rule names: `--correspond "address.person latest by
//!   at where kind == \"home\""`, the first rule a row meets deciding;
//! - `city: text = "" prev(person.city)` in `address` gives the person's
//!   city to the addresses the rule names: `all`, or the `first` or
//!   `latest` by a field, the rest the default;
//! - the render refuses a rule that names no pick, `all` into the one side,
//!   `first` or `latest` with no field (Ordo keeps no creation time), a
//!   field or a condition Ordo cannot read, a plain rule beside a named
//!   one, and the move not accepted as one way, each naming the fix;
//! - the pre-check lists the rows that meet no rule, by id, and the move
//!   does not start while any remain; it previews the rule on 3 rows;
//! - the copy runs in chunks, and every write of the version before keeps
//!   the moved values current, in its own transaction; the version after
//!   writes nothing back, so the revert is refused, and the close orphans
//!   the old column.
//!
//! A regression looks like a move through a link that is not unique that
//! stores with no pick, a person holding another row's value than its rule
//! names, a write on the version before that leaves the moved values
//! stale, or a move that starts while a row meets no rule.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::dual_writes::{self, MoveContext};
use ordo_host::evolve::{self, EvolveRefusal};
use ordo_host::schema_apply::{Choices, Program};
use ordo_host::versions::{MovePhase, Versions};

const OWNER: &str = "deployment-owner:ada";

/// 1.0.0: a person with a city, and any number of addresses.
const MANY: &str = "domain people @ 1.0.0\n\n\
    entity person {\n  name: text\n  city: text = \"\"\n}\n\n\
    entity address (key: adr) {\n  person -> person owned\n  kind: text = \"home\"\n  \
      at: int = 0\n  street: text?\n}\n\n\
    op everyone unbounded =\n  person[id, name, city]\n\n\
    op addresses unbounded =\n  address[id, person, kind, at, street]\n\n\
    op addAddress($person: id, $kind: text, $at: int, $street: text) =\n  \
      set address { person: $person, kind: $kind, at: $at, street: $street }\n\n\
    op dropAddress($id: id) =\n  del address { id: $id }\n\n\
    op retime($id: id, $rev: int, $at: int) =\n  set address { id: $id, rev: $rev, at: $at }\n\n\
    op setCity($id: id, $rev: int, $city: text) =\n  set person { id: $id, rev: $rev, city: $city }\n";

/// 1.1.0: each person takes the street of one of its addresses.
const INTO_ONE: &str = "domain people @ 1.1.0\n\n\
    entity person {\n  name: text\n  city: text = \"\"\n  street: text? prev(address.street)\n}\n\n\
    entity address (key: adr) {\n  person -> person owned\n  kind: text = \"home\"\n  \
      at: int = 0\n}\n\n\
    op everyone unbounded =\n  person[id, name, city, street]\n\n\
    op moveHouse($id: id, $rev: int, $street: text) =\n  \
      set person { id: $id, rev: $rev, street: $street }\n";

/// 1.2.0: the addresses take their person's city.
const INTO_MANY: &str = "domain people @ 1.2.0\n\n\
    entity person {\n  name: text\n}\n\n\
    entity address (key: adr) {\n  person -> person owned\n  kind: text = \"home\"\n  \
      at: int = 0\n  street: text?\n  city: text = \"\" prev(person.city)\n}\n\n\
    op everyone unbounded =\n  person[id, name]\n\n\
    op addresses unbounded =\n  address[id, person, kind, at, street, city]\n";

/// Latest home address first; then the latest work address.
const HOME_THEN_WORK: &str = "address.person latest by at where kind == \"home\"; \
    address.person latest by at where kind == \"work\"";

async fn empty(stem: &str) -> (Pg, Pool) {
    let pg = Pg::scoped(stem);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 4,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    (pg, pool)
}

async fn store(pool: &Pool, source: &str) -> Program {
    let p = Program::compile(source).expect("compiles");
    Versions::on(pool).ensure().await.expect("registry");
    ordo_exec::registry::store(
        pool,
        &ordo_exec::registry::Artifact {
            version: p.version.clone(),
            artifact: ordo_plan::artifact::to_bytes(&p.doc),
            source: p.source.clone(),
            ddl: p.ddl.clone(),
            applied_at: ordo_commandlog::Timestamp(0),
            actor: "dev:ada".into(),
        },
    )
    .await
    .expect("stored");
    p
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

fn options(correspond: &str, one_way: bool) -> Choices {
    let mut c = evolve::typed_options_with("", "", "", "", "", one_way).expect("options");
    c.parse_correspond(correspond).expect("a rule");
    c
}

/// The deployment's own database, `acme` reserved, 1.0.0 applied and
/// active, both later versions stored.
async fn deployment(stem: &str) -> (Pg, Pool) {
    let (pg, pool) = empty(stem).await;
    ordo_host::tenants::ensure_table(&pool)
        .await
        .expect("$spaces");
    assert!(
        ordo_host::tenants::reserve(&pool, "acme", "acme", "dev:ada", 10, None)
            .await
            .expect("reserved")
    );
    ordo_host::tenants::activate(&pool, "acme", 11)
        .await
        .expect("active");
    let program = Program::compile(MANY).expect("compiles");
    ordo_host::apply_op::run_program(
        &pool,
        &ordo_exec::route::SinglePool::new(pool.clone()),
        &program,
        vec![],
        OWNER,
        "t",
        true,
    )
    .await
    .expect("applies");
    store(&pool, INTO_ONE).await;
    store(&pool, INTO_MANY).await;
    (pg, pool)
}

/// A control database with `acme` reserved and every version stored, and
/// `acme`'s own database on 1.0.0, holding `rows`.
async fn tenant(stem: &str, rows: &str) -> (Pool, Pg, Pool) {
    let (_reg_pg, registry) = empty(&format!("{stem}_reg")).await;
    ordo_host::tenants::ensure_table(&registry)
        .await
        .expect("$spaces");
    assert!(
        ordo_host::tenants::reserve(&registry, "acme", "acme", "dev:ada", 10, None)
            .await
            .expect("reserved")
    );
    ordo_host::tenants::activate(&registry, "acme", 11)
        .await
        .expect("active");
    let (tpg, own) = empty(&format!("{stem}_tenant")).await;
    let first = store(&registry, MANY).await;
    store(&registry, INTO_ONE).await;
    store(&registry, INTO_MANY).await;
    let (_, created) = ordo_host::schema_apply::apply(&own, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(ordo_host::schema_apply::Applied::Created(_))),
        "{created:?}"
    );
    tpg.exec(rows).expect("rows");
    Versions::on(&registry)
        .pin("acme", "1.0.0", OWNER, 12)
        .await
        .expect("acme on 1.0.0");
    (registry, tpg, own)
}

/// A real write node over `db` serving `source`, writing as `space`.
struct Node {
    exec: ordo_exec::Executor,
    fixture: std::sync::Arc<ordo_conformance::Fixture>,
    log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
}

async fn node(db: &str, source: &str, space: &str) -> Node {
    let f = ordo_conformance::Fixture::from_source(source, "")
        .expect("compiles")
        .in_space(space);
    let pool = Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 3,
        ..PoolConfig::from_env()
    })
    .await
    .expect("a pool");
    Node {
        exec: ordo_exec::Executor {
            pool,
            schema: f.schema.clone(),
            registry: f.registry.clone(),
            keys: f.keys.clone(),
            space: ordo_crypto::SpaceId(space.to_string()),
        },
        fixture: std::sync::Arc::new(f),
        log: std::sync::Arc::new(std::sync::Mutex::new(ordo_conformance::WriteLog::serving(
            format!("{space}_"),
            std::sync::Arc::new(|| 1),
        ))),
    }
}

impl Node {
    async fn write(&self, call: &str) {
        self.exec
            .write_as(
                self.fixture.clone(),
                self.log.clone(),
                call.to_string(),
                "p_ada".into(),
            )
            .await
            .map(|_| ())
            .unwrap_or_else(|e| panic!("{call}: {e}"));
    }
}

fn step_context() -> evolve::StepContext {
    evolve::StepContext {
        settings: ordo_host::backfill::BackfillSettings {
            chunk_rows: 2,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn errors_of(e: EvolveRefusal) -> Vec<(String, String)> {
    match e {
        EvolveRefusal::Errors { errors, .. } => errors
            .into_iter()
            .map(|x| (x.class, format!("{} {}", x.why, x.fix)))
            .collect(),
        EvolveRefusal::Options(o) => vec![("options".into(), o)],
        other => vec![(other.class().to_string(), other.to_string())],
    }
}

async fn render(pool: &Pool, to: &str, c: &Choices) -> Result<evolve::Evolved, EvolveRefusal> {
    evolve::evolve(pool, "1.0.0", to, c, None, OWNER, 1).await
}

fn has(errs: &[(String, String)], class: &str, text: &str) -> bool {
    errs.iter().any(|(c, w)| c == class && w.contains(text))
}

/// `id=value` for each row of `table`, by id.
fn values(pg: &Pg, table: &str, column: &str) -> String {
    pg.query(&format!(
        "SELECT id || '=' || coalesce({column}, 'null') FROM {table} ORDER BY id"
    ))
    .expect("reads")
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect::<Vec<_>>()
    .join(" ")
}

/// **The render refuses a move through a link that is not unique until its
/// rule names its rows, naming the fix each time.** Into the one side: no
/// rule, a plain rule, `all`, `latest` with no field, a field `address`
/// does not have, a condition that does not parse or names no field, a
/// plain rule beside a named one, and no `--one-way`. Into the many side:
/// no rule names `all`. Named, each stores as B14, its rules kept whole in
/// the stored options.
#[tokio::test(flavor = "multi_thread")]
async fn the_render_refuses_a_rule_until_it_names_its_rows() {
    let (_pg, pool) = deployment("ordo_rules_render").await;
    let refused = |c: Choices| {
        let pool = pool.clone();
        async move { errors_of(render(&pool, "1.1.0", &c).await.expect_err("refused")) }
    };
    let errs = refused(Choices::default()).await;
    assert!(
        has(&errs, "move_rule_required", "--correspond address.person"),
        "{errs:?}"
    );
    for (rule, class, text) in [
        ("address.person", "move_rule_required", "latest by <field>"),
        (
            "address.person all",
            "correspond_rule_invalid",
            "cannot all give one `person` row its value",
        ),
        (
            "address.person latest",
            "correspond_rule_invalid",
            "Ordo keeps no creation time for a row",
        ),
        (
            "address.person first by nowhere",
            "correspond_rule_invalid",
            "`address` has no field `nowhere`",
        ),
        (
            "address.person latest by at where kind ==",
            "correspond_rule_invalid",
            "does not parse",
        ),
        (
            "address.person latest by at where colour == \"red\"",
            "correspond_rule_invalid",
            "mapping_unknown_field",
        ),
    ] {
        let errs = refused(options(rule, true)).await;
        assert!(has(&errs, class, text), "{rule}: {errs:?}");
    }
    let errs = refused(options(HOME_THEN_WORK, false)).await;
    assert!(has(&errs, "one_way_unconfirmed", "--one-way"), "{errs:?}");
    let mut c = Choices::default();
    let err = c
        .parse_correspond("address.person; address.person latest by at")
        .expect_err("plain beside named");
    assert!(err.contains("a plain rule and a named one"), "{err}");

    let done = render(&pool, "1.1.0", &options(HOME_THEN_WORK, true))
        .await
        .expect("stores");
    let receipt: serde_json::Value = serde_json::from_str(&done.plan.receipt).expect("receipt");
    let codes: Vec<&str> = receipt["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter_map(|c| c["code"].as_str())
        .collect();
    assert_eq!(codes, ["B14"], "{receipt}");
    assert!(
        done.plan
            .options
            .contains("correspond address.person latest by at where kind == \"home\"\ncorrespond address.person latest by at where kind == \"work\""),
        "{}",
        done.plan.options
    );
    let back = evolve::parse_options(&done.plan.options).expect("reads back");
    assert_eq!(back.rules.len(), 2, "{back:?}");

    // Into the many side, the rule names the link of `address`.
    let errs = errors_of(
        evolve::evolve(&pool, "1.0.0", "1.2.0", &Choices::default(), None, OWNER, 1)
            .await
            .expect_err("no rule"),
    );
    assert!(
        has(
            &errs,
            "move_rule_required",
            "--correspond \"address.person all\""
        ),
        "{errs:?}"
    );
    evolve::evolve(
        &pool,
        "1.0.0",
        "1.2.0",
        &options("address.person all", true),
        None,
        OWNER,
        1,
    )
    .await
    .expect("into the many side stores");
}

/// **Into the one side: each person takes the street of the address its
/// rules name, and every write of the version before keeps it so.** With
/// rules for home and work addresses only, Ana's `other` address meets
/// none: the pre-check lists it by id and the move does not start. A last
/// rule with no condition meets it; the preview names the address each
/// person takes from. The copy runs in chunks of 2; meanwhile the version
/// before adds a home address for Bob and for Cy, and drops Ada's latest
/// home address: each person follows. On 1.1.0 a street set on a person
/// stays, the revert is refused, and the close orphans `address.street`.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_into_one_takes_the_row_its_rules_name() {
    let (registry, tpg, own) = tenant(
        "ordo_rules_one",
        "INSERT INTO person (\"$space\", id, rev, name) VALUES ('acme', 'p1', 1, 'Ada'), \
         ('acme', 'p2', 1, 'Bob'), ('acme', 'p3', 1, 'Cy'), ('acme', 'p4', 1, 'Ana'); \
         INSERT INTO address (\"$space\", id, rev, person, kind, at, street) VALUES \
         ('acme', 'a1', 1, 'p1', 'home', 1, 'Old'), ('acme', 'a2', 1, 'p1', 'home', 2, 'New'), \
         ('acme', 'a3', 1, 'p1', 'work', 3, 'Work'), ('acme', 'a4', 1, 'p2', 'work', 1, 'Office'), \
         ('acme', 'a5', 1, 'p4', 'other', 5, 'Elsewhere')",
    )
    .await;
    let shared = ordo_exec::route::SinglePool::new(own.clone());
    let cx = MoveContext {
        control: &registry,
        pools: &shared,
        spaces: None,
    };
    let week = 7 * 86_400;

    // Ana's `other` address meets no rule: listed, and nothing moves.
    let partial = render(&registry, "1.1.0", &options(HOME_THEN_WORK, true))
        .await
        .expect("stores")
        .plan;
    let checks = ordo_host::precheck::precheck(&registry, &shared, &partial)
        .await
        .expect("checked");
    let issue = checks
        .issues
        .iter()
        .find(|i| i.code == "B14")
        .unwrap_or_else(|| panic!("{:?}", checks.issues));
    assert!(
        issue
            .lines
            .join(" ")
            .contains("1 row(s) meet no rule: acme/a5"),
        "{issue:?}"
    );
    assert!(!issue.lines.join(" ").contains("Elsewhere"), "{issue:?}");
    assert!(
        issue.fix.contains("a last rule with no `where`"),
        "{issue:?}"
    );
    let err = dual_writes::run(&cx, "acme", &partial, &step_context(), week, OWNER)
        .await
        .expect_err("a row meets no rule");
    assert_eq!(err.class(), "move_unmatched", "{err}");
    assert!(err.to_string().contains("acme/a5"), "{err}");
    assert!(Versions::on(&registry)
        .dual_move("acme")
        .await
        .expect("read")
        .is_none());

    // A last rule with no condition meets every row.
    let rules = format!("{HOME_THEN_WORK}; address.person first by at");
    let plan = render(&registry, "1.1.0", &options(&rules, true))
        .await
        .expect("stores")
        .plan;
    let checks = ordo_host::precheck::precheck(&registry, &shared, &plan)
        .await
        .expect("checked");
    assert!(checks.issues.is_empty(), "{:?}", checks.issues);
    let p = &checks.preview[0];
    assert!(!p.values);
    assert_eq!(
        p.rows,
        [
            "acme/p1 <- address a2 (rule 1): street",
            "acme/p2 <- address a4 (rule 2): street",
            "acme/p3 <- no `address` row: the defaults",
        ],
        "{p:?}"
    );

    let mut ctx = step_context();
    ctx.settings.budget = Some(1);
    dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect("the copy starts");
    assert_eq!(
        values(&tpg, "person", "street"),
        "p1=New p2=Office p3=null p4=null"
    );
    let on_x = node(&tpg.db, MANY, "acme").await;
    on_x.write(r#"addAddress($person: "p2", $kind: "home", $at: 0, $street: "Home")"#)
        .await;
    on_x.write(r#"addAddress($person: "p3", $kind: "work", $at: 1, $street: "Desk")"#)
        .await;
    on_x.write(r#"dropAddress($id: "a2")"#).await;
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        values(&tpg, "person", "street"),
        "p1=Old p2=Home p3=Desk p4=Elsewhere"
    );
    // The version before writes on in the soak: an earlier address for
    // Ana is the first by `at`, until hers is retimed before it.
    on_x.write(r#"addAddress($person: "p4", $kind: "other", $at: 1, $street: "Earlier")"#)
        .await;
    assert_eq!(
        values(&tpg, "person", "street"),
        "p1=Old p2=Home p3=Desk p4=Earlier"
    );
    let rev = one(&tpg, "SELECT rev FROM address WHERE id = 'a5'");
    on_x.write(&format!(r#"retime($id: "a5", $rev: {rev}, $at: 0)"#))
        .await;
    assert_eq!(
        values(&tpg, "person", "street"),
        "p1=Old p2=Home p3=Desk p4=Elsewhere"
    );

    // The version after writes the street; nothing goes back.
    let on_y = node(&tpg.db, INTO_ONE, "acme").await;
    let rev = one(&tpg, "SELECT rev FROM person WHERE id = 'p1'");
    on_y.write(&format!(
        r#"moveHouse($id: "p1", $rev: {rev}, $street: "Moved")"#
    ))
    .await;
    assert_eq!(
        one(&tpg, "SELECT street FROM person WHERE id = 'p1'"),
        "Moved"
    );
    assert_eq!(
        one(&tpg, "SELECT street FROM address WHERE id = 'a1'"),
        "Old"
    );
    let err = dual_writes::revert(&cx, "acme", OWNER)
        .await
        .expect_err("one way");
    assert_eq!(err.class(), "one_way_step", "{err}");
    let closed = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect("closes");
    assert!(
        closed.orphaned.contains(&"address.street".to_string()),
        "{:?}",
        closed.orphaned
    );
    assert_eq!(one(&tpg, "SELECT count(*) FROM \"$renames\""), "0");
}

/// **Into the many side: each address takes its person's city, as its
/// rules name.** The latest home address of a person takes it, an older
/// one the default, and every other address takes it. The copy runs in
/// chunks of 2; meanwhile the version before changes Ada's city, adds a
/// later home address for her and a first one for Cy: every address
/// follows. The close orphans `person.city`.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_into_the_many_side_gives_each_row_its_value() {
    let (registry, tpg, own) = tenant(
        "ordo_rules_many",
        "INSERT INTO person (\"$space\", id, rev, name, city) VALUES \
         ('acme', 'p1', 1, 'Ada', 'York'), ('acme', 'p2', 1, 'Bob', 'Leeds'), \
         ('acme', 'p3', 1, 'Cy', 'Hull'); \
         INSERT INTO address (\"$space\", id, rev, person, kind, at, street) VALUES \
         ('acme', 'a1', 1, 'p1', 'home', 1, 'Old'), ('acme', 'a2', 1, 'p1', 'home', 2, 'New'), \
         ('acme', 'a3', 1, 'p1', 'work', 3, 'Work'), ('acme', 'a4', 1, 'p2', 'work', 1, 'Office')",
    )
    .await;
    let shared = ordo_exec::route::SinglePool::new(own.clone());
    let cx = MoveContext {
        control: &registry,
        pools: &shared,
        spaces: None,
    };
    let week = 7 * 86_400;
    let rules = "address.person latest by at where kind == \"home\"; \
                 address.person all where kind != \"home\"";
    let plan = evolve::evolve(
        &registry,
        "1.0.0",
        "1.2.0",
        &options(rules, true),
        None,
        OWNER,
        1,
    )
    .await
    .expect("stores")
    .plan;
    let checks = ordo_host::precheck::precheck(&registry, &shared, &plan)
        .await
        .expect("checked");
    assert!(checks.issues.is_empty(), "{:?}", checks.issues);
    assert_eq!(
        checks.preview[0].rows[0], "acme/a1 <- person p1 (address.person): city",
        "{:?}",
        checks.preview
    );

    let mut ctx = step_context();
    ctx.settings.budget = Some(1);
    dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect("the copy starts");
    assert_eq!(
        values(&tpg, "address", "city"),
        "a1= a2=York a3=York a4=Leeds"
    );
    let on_x = node(&tpg.db, MANY, "acme").await;
    let rev = one(&tpg, "SELECT rev FROM person WHERE id = 'p1'");
    on_x.write(&format!(
        r#"setCity($id: "p1", $rev: {rev}, $city: "Bath")"#
    ))
    .await;
    on_x.write(r#"addAddress($person: "p1", $kind: "home", $at: 5, $street: "Newer")"#)
        .await;
    on_x.write(r#"addAddress($person: "p3", $kind: "home", $at: 1, $street: "First")"#)
        .await;
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        tpg.query("SELECT street || '=' || city FROM address ORDER BY person, at, id")
            .expect("reads")
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect::<Vec<_>>()
            .join(" "),
        "Old= New= Work=Bath Newer=Bath Office=Leeds First=Hull"
    );
    let closed = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect("closes");
    assert!(
        closed.orphaned.contains(&"person.city".to_string()),
        "{:?}",
        closed.orphaned
    );
}

/// **A row written during the move that meets no rule is not resolved
/// silently.** The write itself goes through, and its person takes the
/// default; the close then refuses, `move_unmatched`, naming the row by id
/// and the rule to add, and closes once no such row remains.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_written_during_the_move_that_meets_no_rule_blocks_the_close() {
    let (registry, tpg, own) = tenant(
        "ordo_rules_late",
        "INSERT INTO person (\"$space\", id, rev, name) VALUES ('acme', 'p1', 1, 'Ada'), \
         ('acme', 'p2', 1, 'Bob'); \
         INSERT INTO address (\"$space\", id, rev, person, kind, at, street) VALUES \
         ('acme', 'a1', 1, 'p1', 'home', 1, 'Home')",
    )
    .await;
    let shared = ordo_exec::route::SinglePool::new(own.clone());
    let cx = MoveContext {
        control: &registry,
        pools: &shared,
        spaces: None,
    };
    let plan = render(&registry, "1.1.0", &options(HOME_THEN_WORK, true))
        .await
        .expect("stores")
        .plan;
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), 7 * 86_400, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    let on_x = node(&tpg.db, MANY, "acme").await;
    on_x.write(r#"addAddress($person: "p2", $kind: "other", $at: 1, $street: "Elsewhere")"#)
        .await;
    assert_eq!(values(&tpg, "person", "street"), "p1=Home p2=null");
    let late = one(&tpg, "SELECT id FROM address WHERE kind = 'other'");
    let err = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect_err("a row meets no rule");
    assert_eq!(err.class(), "move_unmatched", "{err}");
    let why = err.to_string();
    assert!(why.contains(&format!("acme/{late}")), "{why}");
    assert!(why.contains("address.person first by <field>"), "{why}");
    assert!(!why.contains("Elsewhere"), "{why}");
    on_x.write(&format!(r#"dropAddress($id: "{late}")"#)).await;
    let closed = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect("closes");
    assert!(
        closed.orphaned.contains(&"address.street".to_string()),
        "{:?}",
        closed.orphaned
    );
}
