//! **Field splits and merges** (`plan-versions.md` increment 15b;
//! `forward-gaps` asks 8 and 18, owner: *"declared correspondence tool based
//! on prev"*, *"expand on prev"*), against real Postgres:
//!
//! - a split is two fields each `prev(fullName)`; a merge is one
//!   `prev(first, last)`. Each renders as one change, B12;
//! - the render refuses a new field with no `--fill`, an old field with no
//!   `--reverse` unless `--one-way` accepts it, a stray `--reverse`, and the
//!   apply path, each naming the option;
//! - the pre-check lists the rows a required new field's mapping does not
//!   parse, by `space/id`, and the move refuses to open while one is left;
//! - the preview shows the mapping on 3 rows, with values only where
//!   `preview.values` is on (off in production unless set);
//! - under a move both shapes stay current through the real write path: a
//!   write on the version before gets the new fields by the mapping, one on
//!   the new version gets the old field by the reverse, and a write the
//!   mapping cannot parse is refused by name;
//! - a revert in the soak keeps every write; a one way step refuses the
//!   revert and the rollback, naming the close; the close lands the new
//!   fields' `NOT NULL` and orphans the old ones.
//!
//! A regression looks like a split that renders as a drop and two adds, an
//! old row with an empty new field, a write on either version that leaves
//! the other shape stale, values in a production preview, or a one way
//! soak that reverts.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::dual_writes::{self, MoveContext};
use ordo_host::evolve::{self, EvolveRefusal};
use ordo_host::schema_apply::{Choices, Program};
use ordo_host::versions::{MovePhase, Versions};

const OWNER: &str = "deployment-owner:ada";

fn people(version: &str, fields: &str, ops: &str) -> String {
    format!(
        "domain people @ {version}\n\nentity person {{\n{fields}}}\n\n\
         op everyone unbounded =\n  person[id]\n{ops}"
    )
}

/// 1.0.0: one `fullName`.
fn whole() -> String {
    people(
        "1.0.0",
        "  fullName: text\n",
        "\nop addPerson($name: text) =\n  set person { fullName: $name }\n",
    )
}

/// 1.1.0: `fullName` split into `first` and `last`.
fn split() -> String {
    people(
        "1.1.0",
        "  first: text prev(fullName)\n  last: text prev(fullName)\n",
        "\nop addPerson($first: text, $last: text) =\n  \
         set person { first: $first, last: $last }\n",
    )
}

/// 1.0.0: `first` and `last`, each with a default of its own.
fn parts() -> String {
    people(
        "1.0.0",
        "  first: text = \"\"\n  last: text = \"\"\n",
        "\nop addPerson($first: text, $last: text) =\n  \
         set person { first: $first, last: $last }\n",
    )
}

/// 1.1.0: `first` and `last` merged into `fullName`.
fn merged() -> String {
    people(
        "1.1.0",
        "  fullName: text prev(first, last)\n",
        "\nop addPerson($name: text) =\n  set person { fullName: $name }\n",
    )
}

const SPLIT_FILL: &str = "person.first=before(fullName, \" \");person.last=after(fullName, \" \")";
const JOIN: &str = "person.fullName=first + \" \" + last";

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

fn options(fill: &str, reverse: &str, one_way: bool) -> Choices {
    evolve::typed_options_with("", fill, "", "", reverse, one_way).expect("options")
}

fn is_nullable(pg: &Pg, column: &str) -> String {
    one(
        pg,
        &format!(
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'person' AND column_name = '{column}'"
        ),
    )
}

/// The deployment's own database, `acme` and `beta` reserved, `x` applied
/// and active, `y` stored. Each of `acme`, `beta` and `home` (the own
/// space) holds the rows `names` gives.
async fn deployment(stem: &str, x: &str, y: &str) -> (Pg, Pool) {
    let (pg, pool) = empty(stem).await;
    ordo_host::tenants::ensure_table(&pool)
        .await
        .expect("$spaces");
    for t in ["acme", "beta"] {
        assert!(
            ordo_host::tenants::reserve(&pool, t, t, "dev:ada", 10, None)
                .await
                .expect("reserved")
        );
        ordo_host::tenants::activate(&pool, t, 11)
            .await
            .expect("active");
    }
    let program = Program::compile(x).expect("compiles");
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
    store(&pool, y).await;
    (pg, pool)
}

/// A real write node over `db` serving `source`, writing as `space`, with
/// no overlay of its own: the database's `$fills` says what to keep.
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
    async fn write(&self, call: &str) -> Result<(), String> {
        self.exec
            .write_as(
                self.fixture.clone(),
                self.log.clone(),
                call.to_string(),
                "p_ada".into(),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// A node writing `space` in its move on a database of its own: the
/// reverse overlay on the version it moves to, the forward one on the
/// version it leaves.
async fn moving_node(pool: &Pool, db: &str, source: &str, space: &str, reverse: bool) -> Node {
    let m = Versions::on(pool)
        .dual_move(space)
        .await
        .expect("read")
        .expect("a move");
    let o = dual_writes::overlays_of(&m).expect("overlays");
    let mut n = node(db, source, space).await;
    let side = if reverse { o.reverse } else { o.forward };
    n.exec.pool = n.exec.pool.with_overlay(Some(side));
    n
}

fn step_context() -> evolve::StepContext {
    evolve::StepContext {
        settings: ordo_host::backfill::BackfillSettings {
            chunk_rows: 25,
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

/// **The render refuses what it cannot decide, naming the option.** A split
/// with no `--fill` names `--fill` with an example; with fills and no
/// reverse it names `--reverse` and `--one-way`; a reverse for a field the
/// step keeps is stray. The apply path refuses the split: it is a stored
/// plan's. A rendered split is one B12 change, not a drop and two adds,
/// and `--run` of it is refused: it moves with dual writes.
#[tokio::test(flavor = "multi_thread")]
async fn the_render_refuses_a_split_until_its_rules_are_given() {
    let (_pg, pool) = deployment("ordo_maps_render", &whole(), &split()).await;
    let err = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect_err("no fill");
    let errs = errors_of(err);
    assert_eq!(errs[0].0, "map_fill_required", "{errs:?}");
    assert!(errs[0].1.contains("--fill person.first="), "{errs:?}");
    assert!(errs[0].1.contains("before(fullName"), "{errs:?}");

    let err = evolve::evolve(
        &pool,
        "1.0.0",
        "1.1.0",
        &options(SPLIT_FILL, "", false),
        None,
        OWNER,
        1,
    )
    .await
    .expect_err("no reverse");
    let errs = errors_of(err);
    assert_eq!(errs[0].0, "one_way_unconfirmed", "{errs:?}");
    assert!(errs[0].1.contains("--reverse person.fullName="), "{errs:?}");
    assert!(errs[0].1.contains("--one-way"), "{errs:?}");

    let err = evolve::evolve(
        &pool,
        "1.0.0",
        "1.1.0",
        &options(SPLIT_FILL, &format!("{JOIN};person.first=fullName"), false),
        None,
        OWNER,
        1,
    )
    .await
    .expect_err("a stray reverse");
    let errs = errors_of(err);
    assert!(errs[0].1.contains("drop it from `--reverse`"), "{errs:?}");

    let err = evolve::evolve(
        &pool,
        "1.0.0",
        "1.1.0",
        &options(SPLIT_FILL, "person.fullName=first +", false),
        None,
        OWNER,
        1,
    )
    .await
    .expect_err("a reverse that does not parse");
    assert_eq!(errors_of(err)[0].0, "mapping_invalid");

    // The apply path never guesses a split.
    let program = Program::compile(&split()).expect("compiles");
    let err = ordo_host::apply_op::run_program(
        &pool,
        &ordo_exec::route::SinglePool::new(pool.clone()),
        &program,
        vec![],
        OWNER,
        "t",
        true,
    )
    .await
    .expect_err("apply refuses a split");
    let text = format!("{err:?}");
    assert!(
        text.contains("NotActivated") && text.contains("B12"),
        "{text}"
    );

    let done = evolve::evolve(
        &pool,
        "1.0.0",
        "1.1.0",
        &options(SPLIT_FILL, JOIN, false),
        None,
        OWNER,
        1,
    )
    .await
    .expect("the split stores");
    let receipt: serde_json::Value = serde_json::from_str(&done.plan.receipt).expect("receipt");
    let codes: Vec<&str> = receipt["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter_map(|c| c["code"].as_str())
        .collect();
    assert_eq!(codes, ["B12"], "{receipt}");
    assert_eq!(receipt["maps"][0]["oneWay"], false, "{receipt}");
    assert_eq!(receipt["maps"][0]["reverse"][0]["field"], "fullName");
    assert_eq!(receipt["backfills"].as_array().map(Vec::len), Some(2));

    let err = evolve::run_step(&pool, &pool, "deployment", &done.plan, OWNER)
        .await
        .expect_err("a split moves");
    assert_eq!(err.class(), "map_needs_move", "{err}");
    assert!(err.to_string().contains("ordo evolve --tenant"), "{err}");
}

/// **A split on the deployment's own database: every unparsed row listed,
/// both shapes current, a revert in the soak keeps every write.** `Plato`
/// has no space, so `last` cannot be parsed: the pre-check names
/// `home/...` and the move refuses to open. The preview hides values here
/// (production), and shows them once `preview.values` is on. Fixed, `acme`
/// moves: every row is split. `beta` on 1.0.0 writes `Grace Hopper` and
/// gets `first` and `last` by the mapping; `acme` on 1.1.0 writes `Alan` and
/// `Turing` and gets `fullName` by the reverse. `beta` writing `Cher` is
/// refused by name: 1.1.0 requires `first` and `last`. `acme` reverts in its soak and
/// reads `Alan Turing` as 1.0.0.
#[tokio::test(flavor = "multi_thread")]
async fn a_split_lists_what_it_cannot_parse_and_keeps_both_shapes_through_a_revert() {
    let (pg, pool) = deployment("ordo_maps_split", &whole(), &split()).await;
    for (s, names) in [
        ("acme", ["Ada Lovelace", "Edsger Dijkstra"]),
        ("beta", ["Barbara Liskov", "Ken Thompson"]),
        ("home", ["Plato", "Mary Ann Shelley"]),
    ] {
        for (i, n) in names.iter().enumerate() {
            pg.exec(&format!(
                "INSERT INTO person (\"$space\", id, rev, \"fullName\") VALUES ('{s}', '{s}{i}', 1, '{n}')"
            ))
            .expect("rows");
        }
    }
    let plan = evolve::evolve(
        &pool,
        "1.0.0",
        "1.1.0",
        &options(SPLIT_FILL, JOIN, false),
        None,
        OWNER,
        1,
    )
    .await
    .expect("stores")
    .plan;
    let shared = ordo_exec::route::SinglePool::new(pool.clone());

    // The pre-check lists the row by id, never its value.
    let checks = ordo_host::precheck::precheck(&pool, &shared, &plan)
        .await
        .expect("checked");
    assert_eq!(checks.issues.len(), 1, "{:?}", checks.issues);
    let issue = &checks.issues[0];
    assert_eq!(issue.code, "B12");
    let lines = issue.lines.join("; ");
    assert!(lines.contains("`person.last`: 1 row(s)"), "{lines}");
    assert!(lines.contains("home/home0"), "{lines}");
    assert!(!lines.contains("Plato"), "{lines}");
    assert!(issue.fix.contains("--fill person.<field>"), "{}", issue.fix);
    // The preview, values hidden here, naming the setting.
    let p = &checks.preview[0];
    assert!(!p.values);
    assert_eq!(p.rows.len(), 3, "{:?}", p.rows);
    assert!(
        p.rows[0].starts_with("acme/acme0: first parses, last parses"),
        "{:?}",
        p.rows
    );
    assert!(!p.rows.join(" ").contains("Ada"), "{:?}", p.rows);
    assert!(p
        .hidden
        .as_deref()
        .unwrap_or("")
        .contains("ordo environments set preview on"));
    ordo_host::environments::Environments::on(&pool)
        .set_preview_values("on", OWNER, "a test shows the rows", 5)
        .await
        .expect("set");
    let checks = ordo_host::precheck::precheck(&pool, &shared, &plan)
        .await
        .expect("checked");
    let p = &checks.preview[0];
    assert!(p.values);
    assert_eq!(
        p.rows[0], "acme/acme0: fullName \"Ada Lovelace\" -> first \"Ada\", last \"Lovelace\"",
        "{:?}",
        p.rows
    );

    // The move refuses to open while a row does not parse.
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let week = 7 * 86_400;
    let err = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect_err("Plato does not parse");
    assert_eq!(err.class(), "map_unparsed", "{err}");
    assert!(err.to_string().contains("home/home0"), "{err}");
    assert!(Versions::on(&pool)
        .dual_move("acme")
        .await
        .expect("read")
        .is_none());

    // Fixed on the page's own terms, the move runs.
    pg.exec("UPDATE person SET \"fullName\" = 'Plato Athenian' WHERE id = 'home0'")
        .expect("fixed");
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(
            &pg,
            "SELECT first || '|' || last FROM person WHERE id = 'home1'"
        ),
        "Mary|Ann Shelley"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM person WHERE first IS NULL OR last IS NULL"
        ),
        "0"
    );
    assert_eq!(
        is_nullable(&pg, "first"),
        "YES",
        "the NOT NULL waits for the close"
    );
    assert_eq!(
        is_nullable(&pg, "fullName"),
        "NO",
        "the old column keeps its NOT NULL"
    );

    // Both versions write through the real path.
    let beta = node(&pg.db, &whole(), "beta").await;
    let acme = node(&pg.db, &split(), "acme").await;
    beta.write(r#"addPerson($name: "Grace Hopper")"#)
        .await
        .expect("beta writes as 1.0.0");
    acme.write(r#"addPerson($first: "Alan", $last: "Turing")"#)
        .await
        .expect("acme writes as 1.1.0");
    assert_eq!(
        one(
            &pg,
            "SELECT first || '|' || last FROM person WHERE \"fullName\" = 'Grace Hopper'"
        ),
        "Grace|Hopper"
    );
    assert_eq!(
        one(&pg, "SELECT \"fullName\" FROM person WHERE first = 'Alan'"),
        "Alan Turing"
    );
    let err = beta
        .write(r#"addPerson($name: "Cher")"#)
        .await
        .expect_err("1.1.0 requires last");
    assert!(err.contains("check_violation"), "{err}");
    assert!(
        err.contains("`person.first` is required in `1.1.0`"),
        "{err}"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM person WHERE \"fullName\" = 'Cher'"
        ),
        "0"
    );

    // A revert in the soak: acme reads what it wrote as 1.0.0.
    let back = dual_writes::revert(&cx, "acme", OWNER)
        .await
        .expect("reverts");
    assert_eq!(back.phase, MovePhase::Gated);
    assert_eq!(
        Versions::on(&pool)
            .pin_of("acme")
            .await
            .expect("read")
            .and_then(|p| p.version),
        Some("1.0.0".to_string())
    );
    assert_eq!(
        one(&pg, "SELECT count(*) FROM person WHERE \"$space\" = 'acme' AND \"fullName\" = 'Alan Turing'"),
        "1"
    );
}

/// **A one way merge on a tenant's own database.** With no default, a
/// one way merge is refused (`map_default_required`). With defaults, `first` and `last`
/// merge into `fullName` with no reverse, accepted by `--one-way`. The
/// tenant moves; a write on 1.1.0 lands; a revert in the soak and a
/// rollback are refused, naming the close; the close lands `fullName`'s
/// `NOT NULL` and orphans `first` and `last`.
#[tokio::test(flavor = "multi_thread")]
async fn a_one_way_merge_moves_and_refuses_the_revert() {
    let (_reg_pg, registry) = empty("ordo_maps_merge_reg").await;
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
    let (tpg, tenant) = empty("ordo_maps_merge_tenant").await;
    let first = store(&registry, &parts()).await;
    store(&registry, &merged()).await;
    let (_, created) = ordo_host::schema_apply::apply(&tenant, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(ordo_host::schema_apply::Applied::Created(_))),
        "{created:?}"
    );
    tpg.exec(
        "INSERT INTO person (\"$space\", id, rev, first, last) \
         SELECT 'acme', 'p' || i, 1, 'First' || i, 'Last' || i FROM generate_series(1, 40) i",
    )
    .expect("rows");
    let fill = "person.fullName=first + \" \" + last";
    let err = evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &options(fill, "", false),
        None,
        OWNER,
        1,
    )
    .await
    .expect_err("no reverse");
    let errs = errors_of(err);
    assert_eq!(errs[0].0, "one_way_unconfirmed", "{errs:?}");
    assert!(
        errs[0].1.contains("--reverse person.first=before(fullName"),
        "{errs:?}"
    );
    // With no default of their own, `first` and `last` would hold a value
    // nobody wrote on every row 1.1.0 inserts: refused by name.
    let (_bare_pg, bare) = empty("ordo_maps_merge_bare").await;
    store(
        &bare,
        &people(
            "1.0.0",
            "  first: text\n  last: text\n",
            "\nop addPerson($first: text, $last: text) =\n  \
             set person { first: $first, last: $last }\n",
        ),
    )
    .await;
    store(&bare, &merged()).await;
    let err = evolve::evolve(
        &bare,
        "1.0.0",
        "1.1.0",
        &options(fill, "", true),
        None,
        OWNER,
        1,
    )
    .await
    .expect_err("no default and no reverse");
    let errs = errors_of(err);
    assert_eq!(errs[0].0, "map_default_required", "{errs:?}");
    assert!(errs[0].1.contains("--reverse person.first="), "{errs:?}");
    let plan = evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &options(fill, "", true),
        None,
        OWNER,
        1,
    )
    .await
    .expect("accepted one way")
    .plan;
    assert!(plan.options.contains("one-way"), "{}", plan.options);

    Versions::on(&registry)
        .pin("acme", "1.0.0", OWNER, 12)
        .await
        .expect("acme on 1.0.0");
    let own = ordo_exec::route::SinglePool::new(tenant.clone());
    let cx = MoveContext {
        control: &registry,
        pools: &own,
        spaces: None,
    };
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), 7 * 86_400, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(&tpg, "SELECT \"fullName\" FROM person WHERE id = 'p7'"),
        "First7 Last7"
    );
    let on_y = moving_node(&registry, &tpg.db, &merged(), "acme", true).await;
    on_y.write(r#"addPerson($name: "Joan Clarke")"#)
        .await
        .expect("a write on 1.1.0 lands: the old columns take their own defaults");

    let err = dual_writes::revert(&cx, "acme", OWNER)
        .await
        .expect_err("one way");
    assert_eq!(err.class(), "one_way_step", "{err}");
    assert!(err.to_string().contains("--close-soak"), "{err}");
    let err =
        ordo_host::every_tenant::revert_soaks_from(&registry, Some(&own), None, "1.0.0", OWNER)
            .await
            .expect_err("a rollback of a one way soak");
    assert_eq!(err.class(), "one_way_step", "{err}");

    let closed = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect("closes");
    assert!(closed.closed);
    assert_eq!(is_nullable(&tpg, "fullName"), "NO");
    let mut orphaned = closed.orphaned.clone();
    orphaned.sort();
    assert!(
        orphaned.contains(&"person.first".to_string()),
        "{orphaned:?}"
    );
    assert!(
        orphaned.contains(&"person.last".to_string()),
        "{orphaned:?}"
    );
}

/// `before` and `after` are expression functions anywhere fields are: a
/// projection compiles with them.
#[test]
fn before_and_after_compile_in_a_projection() {
    let src = people(
        "1.0.0",
        "  fullName: text\n",
        "\nop firstNames unbounded =\n  person[first: before(fullName, \" \"), rest: after(fullName, \" \")]\n",
    );
    Program::compile(&src).expect("compiles");
}

/// The conversions are expression functions anywhere fields are too, and a
/// projection reads them through the planner: a count that is no whole
/// number reads as null, never as an error.
#[tokio::test(flavor = "multi_thread")]
async fn the_conversions_read_in_a_projection() {
    let src = items(
        "1.0.0",
        "  count: text = \"0\"\n  price: decimal(2) = 0\n",
        "\nop converted unbounded =\n  item[id, n: int(count), t: text(price), b: bool(name)]\n  sort id\n",
    );
    let (pg, pool) = empty("ordo_maps_convert").await;
    let program = Program::compile(&src).expect("compiles");
    let (_, made) = ordo_host::schema_apply::apply(&pool, &program, "test", "test").await;
    assert!(made.is_ok(), "{made:?}");
    pg.exec(
        "INSERT INTO item (\"$space\", id, rev, name, count, price) VALUES \
         ('s', 'a', 1, ' TRUE ', ' 12 ', 3.5), ('s', 'b', 1, 'no', 'lots', 0)",
    )
    .expect("rows");
    let n = node(&pg.db, &src, "s").await;
    let rows = n
        .exec
        .read_as("converted", Some("p_ada"))
        .await
        .expect("reads");
    let text = serde_json::to_string(&rows).expect("json");
    assert_eq!(rows.len(), 2, "{text}");
    assert_eq!(rows[0]["n"], 12, "{text}");
    assert_eq!(rows[0]["t"], "3.50", "{text}");
    assert_eq!(rows[0]["b"], true, "{text}");
    assert!(rows[1]["n"].is_null() && rows[1]["b"].is_null(), "{text}");
}

/// `date` and `datetime` read in a projection through the planner too: a
/// text that spells no date in the format, or a day not in the calendar,
/// reads as null; a datetime with no offset is UTC.
#[tokio::test(flavor = "multi_thread")]
async fn the_date_conversions_read_in_a_projection() {
    let src = items(
        "1.0.0",
        "  stamp: text = \"\"\n",
        "\nop dated unbounded =\n  item[id, d: date(stamp), dmy: date(stamp, \"DD.MM.YYYY\"), \
         t: datetime(stamp)]\n  sort id\n",
    );
    let (pg, pool) = empty("ordo_maps_dates").await;
    let program = Program::compile(&src).expect("compiles");
    let (_, made) = ordo_host::schema_apply::apply(&pool, &program, "test", "test").await;
    assert!(made.is_ok(), "{made:?}");
    pg.exec(
        "INSERT INTO item (\"$space\", id, rev, name, stamp) VALUES \
         ('s', 'a', 1, 'x', ' 2026-11-25 '), ('s', 'b', 1, 'x', '25.11.2026'), \
         ('s', 'c', 1, 'x', '2026-02-30'), ('s', 'd', 1, 'x', '2026-11-25T09:30'), \
         ('s', 'e', 1, 'x', '2026-11-25 09:30:15+01:00'), ('s', 'f', 1, 'x', 'soon')",
    )
    .expect("rows");
    let n = node(&pg.db, &src, "s").await;
    let rows = n.exec.read_as("dated", Some("p_ada")).await.expect("reads");
    let text = serde_json::to_string(&rows).expect("json");
    assert_eq!(rows.len(), 6, "{text}");
    assert_eq!(rows[0]["d"], "2026-11-25", "{text}");
    assert!(rows[0]["dmy"].is_null() && rows[0]["t"].is_null(), "{text}");
    assert_eq!(rows[1]["dmy"], "2026-11-25", "{text}");
    assert!(rows[1]["d"].is_null(), "{text}");
    assert!(rows[2]["d"].is_null(), "a day not in the calendar: {text}");
    assert!(
        rows[3]["t"]
            .as_str()
            .unwrap_or("")
            .starts_with("2026-11-25 09:30:00+00"),
        "{text}"
    );
    assert!(
        rows[4]["t"]
            .as_str()
            .unwrap_or("")
            .starts_with("2026-11-25 08:30:15+00"),
        "{text}"
    );
    assert!(rows[5]["d"].is_null() && rows[5]["t"].is_null(), "{text}");
    // A format Ordo does not read is refused when the program compiles.
    let bad = items(
        "1.0.1",
        "  stamp: text = \"\"\n",
        "\nop dated unbounded =\n  item[id, d: date(stamp, \"YYYY/DD\")]\n",
    );
    let err = Program::compile(&bad).expect_err("an unknown format");
    assert!(format!("{err:?}").contains("not a date format"), "{err:?}");
}

/// **Text becomes a date by a stated format** (`forward-gaps` ask 17,
/// owner: *"the compiler either says its allowed or says it isn't"*).
/// `due: text` becomes `dueOn: date prev(due)`. A fill in a format Ordo does
/// not read is refused naming the formats; the stated fill reads two
/// formats with `coalesce`. A day not in the calendar and a text that is no
/// date are listed by row, never by value, and the move waits. Fixed, the
/// tenant moves and every date reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_becomes_a_date_by_a_stated_format() {
    let x = items(
        "1.0.0",
        "  due: text = \"\"\n",
        "\nop addItem($name: text, $due: text) =\n  set item { name: $name, due: $due }\n",
    );
    let y = items("1.1.0", "  dueOn: date = 2026-01-01 prev(due)\n", "");
    let (pg, pool) = deployment("ordo_maps_dates_cast", &x, &y).await;
    for (id, due) in [
        ("a0", "2026-11-25"),
        ("a1", "25/12/2026"),
        ("a2", "2026-02-30"),
        ("a3", "soon"),
    ] {
        pg.exec(&format!(
            "INSERT INTO item (\"$space\", id, rev, name, due) VALUES ('acme', '{id}', 1, 'x', '{due}')"
        ))
        .expect("rows");
    }
    let render = |fill: &'static str| {
        let pool = pool.clone();
        async move {
            evolve::evolve(
                &pool,
                "1.0.0",
                "1.1.0",
                &options(fill, "", true),
                None,
                OWNER,
                1,
            )
            .await
        }
    };
    let errs = errors_of(render("").await.expect_err("no fill"));
    assert!(
        errs[0].1.contains("--fill item.dueOn=date(due)"),
        "{errs:?}"
    );
    let errs = errors_of(
        render("item.dueOn=date(due, \"YYYY/DD\")")
            .await
            .expect_err("an unknown format"),
    );
    assert!(
        errs.iter()
            .any(|(c, w)| c == "mapping_invalid" && w.contains("DD/MM/YYYY")),
        "{errs:?}"
    );
    let plan = render("item.dueOn=coalesce(date(due), date(due, \"DD/MM/YYYY\"))")
        .await
        .expect("the cast stores")
        .plan;
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let checks = ordo_host::precheck::precheck(&pool, &shared, &plan)
        .await
        .expect("checked");
    let lines = checks
        .issues
        .iter()
        .flat_map(|i| i.lines.clone())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        lines.contains("acme/a2") && lines.contains("acme/a3"),
        "{lines}"
    );
    assert!(
        !lines.contains("acme/a0") && !lines.contains("acme/a1") && !lines.contains("soon"),
        "{lines}"
    );
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let week = 7 * 86_400;
    let err = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect_err("two rows spell no date");
    assert_eq!(err.class(), "map_unparsed", "{err}");
    pg.exec("UPDATE item SET due = '28/02/2026' WHERE id = 'a2'; UPDATE item SET due = '2027-01-01' WHERE id = 'a3'")
        .expect("fixed");
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(
            &pg,
            "SELECT string_agg(\"dueOn\"::text, ',' ORDER BY id) FROM item"
        ),
        "2026-11-25,2026-12-25,2026-02-28,2027-01-01"
    );
    // The version before keeps writing text; the new column follows.
    let on_x = node(&pg.db, &x, "beta").await;
    on_x.write(r#"addItem($name: "late", $due: "01/03/2026")"#)
        .await
        .expect("1.0.0 writes text");
    assert_eq!(
        one(&pg, "SELECT \"dueOn\"::text FROM item WHERE name = 'late'"),
        "2026-03-01"
    );
}

fn items(version: &str, fields: &str, ops: &str) -> String {
    format!(
        "domain items @ {version}\n\nentity item {{\n  name: text\n{fields}}}\n\n\
         op everything unbounded =\n  item[id]\n{ops}"
    )
}

/// 1.0.0: `count` held as text.
fn counted_as_text() -> String {
    items(
        "1.0.0",
        "  count: text = \"0\"\n  price: decimal(2) = 0\n",
        "\nop addItem($name: text, $count: text) =\n  set item { name: $name, count: $count }\n",
    )
}

/// 1.1.0: `count` renamed to `qty` and made an `int`.
fn counted_as_int() -> String {
    items(
        "1.1.0",
        "  qty: int = 0 prev(count)\n  price: decimal(2) = 0\n",
        "\nop addItem($name: text, $qty: int) =\n  set item { name: $name, qty: $qty }\n",
    )
}

/// **A rename that changes the type is a cast, stated on paper.** `count:
/// text` becomes `qty: int prev(count)`. It renders as one B12 change. With
/// no `--fill` it is refused naming `int(count)`; a fill that gives text is
/// refused (`map_cast_required`) naming the same; with no reverse it is one
/// way, and a reverse that gives a number is refused naming
/// `--reverse item.count=text(qty)`. A conversion Ordo has none for (text
/// to a date) is refused as `map_cast_not_built`, and one that drops places
/// names `round`. `lots` does not convert: the pre-check lists its row by
/// id and the move waits. Fixed, `acme` moves: every count is an int, and
/// both versions write through the real path, each shape kept current. A
/// revert keeps the row 1.1.0 wrote.
#[tokio::test(flavor = "multi_thread")]
async fn a_rename_with_a_new_type_is_converted_by_a_stated_cast() {
    let (pg, pool) = deployment("ordo_maps_cast", &counted_as_text(), &counted_as_int()).await;
    for (s, rows) in [
        ("acme", [("a0", "12"), ("a1", " 7 ")]),
        ("home", [("h0", "lots"), ("h1", "-3")]),
    ] {
        for (id, n) in rows {
            pg.exec(&format!(
                "INSERT INTO item (\"$space\", id, rev, name, count) \
                 VALUES ('{s}', '{id}', 1, 'x', '{n}')"
            ))
            .expect("rows");
        }
    }
    let render = |fill: &'static str, reverse: &'static str| {
        let pool = pool.clone();
        async move {
            evolve::evolve(
                &pool,
                "1.0.0",
                "1.1.0",
                &options(fill, reverse, false),
                None,
                OWNER,
                1,
            )
            .await
        }
    };
    let errs = errors_of(render("", "").await.expect_err("no fill"));
    assert_eq!(errs[0].0, "map_fill_required", "{errs:?}");
    assert!(errs[0].1.contains("--fill item.qty=int(count)"), "{errs:?}");
    let errs = errors_of(render("item.qty=count", "").await.expect_err("text"));
    assert_eq!(errs[0].0, "map_cast_required", "{errs:?}");
    assert!(errs[0].1.contains("--fill item.qty=int(count)"), "{errs:?}");
    let errs = errors_of(
        render("item.qty=int(count)", "")
            .await
            .expect_err("no reverse"),
    );
    assert_eq!(errs[0].0, "one_way_unconfirmed", "{errs:?}");
    let errs = errors_of(
        render("item.qty=int(count)", "item.count=qty")
            .await
            .expect_err("a number into text"),
    );
    assert_eq!(errs[0].0, "map_cast_required", "{errs:?}");
    assert!(
        errs[0].1.contains("--reverse item.count=text(qty)"),
        "{errs:?}"
    );

    // What Ordo has no conversion for, and what would round.
    let (_pg2, other) = deployment(
        "ordo_maps_cast_refused",
        &counted_as_text(),
        &items(
            "1.1.0",
            "  due: date = 2026-01-01 prev(count)\n  cost: int = 0 prev(price)\n",
            "",
        ),
    )
    .await;
    let for_other = |fill: &'static str| {
        let other = other.clone();
        async move {
            errors_of(
                evolve::evolve(
                    &other,
                    "1.0.0",
                    "1.1.0",
                    &options(fill, "", true),
                    None,
                    OWNER,
                    1,
                )
                .await
                .expect_err("refused"),
            )
        }
    };
    // Text converts to a date only when stated; a number has no
    // conversion to a date at all.
    let errs = for_other("item.due=count;item.cost=round(price, 0)").await;
    assert_eq!(errs[0].0, "map_cast_required", "{errs:?}");
    assert!(
        errs[0].1.contains("--fill item.due=date(count)"),
        "{errs:?}"
    );
    let errs = for_other("item.due=length(count);item.cost=round(price, 0)").await;
    assert_eq!(errs[0].0, "map_cast_not_built", "{errs:?}");
    assert!(errs[0].1.contains("from a number to a date"), "{errs:?}");
    let errs = for_other("item.due=2026-01-01;item.cost=price").await;
    assert_eq!(errs[0].0, "map_cast_required", "{errs:?}");
    assert!(
        errs[0].1.contains("--fill item.cost=round(price, 0)"),
        "{errs:?}"
    );

    let plan = render("item.qty=int(count)", "item.count=text(qty)")
        .await
        .expect("the cast stores")
        .plan;
    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("receipt");
    let codes: Vec<&str> = receipt["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter_map(|c| c["code"].as_str())
        .collect();
    assert_eq!(codes, ["B12"], "{receipt}");
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let checks = ordo_host::precheck::precheck(&pool, &shared, &plan)
        .await
        .expect("checked");
    let lines = checks
        .issues
        .iter()
        .flat_map(|i| i.lines.clone())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(lines.contains("home/h0"), "{lines}");
    assert!(!lines.contains("lots"), "{lines}");
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let week = 7 * 86_400;
    let err = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect_err("lots does not convert");
    assert_eq!(err.class(), "map_unparsed", "{err}");
    pg.exec("UPDATE item SET count = '40' WHERE id = 'h0'")
        .expect("fixed");
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(
            &pg,
            "SELECT string_agg(qty::text, ',' ORDER BY id) FROM item"
        ),
        "12,7,40,-3"
    );
    let on_x = node(&pg.db, &counted_as_text(), "beta").await;
    let on_y = node(&pg.db, &counted_as_int(), "acme").await;
    on_x.write(r#"addItem($name: "bolt", $count: "5")"#)
        .await
        .expect("1.0.0 writes text");
    on_y.write(r#"addItem($name: "nut", $qty: 9)"#)
        .await
        .expect("1.1.0 writes an int");
    assert_eq!(
        one(&pg, "SELECT qty::text FROM item WHERE name = 'bolt'"),
        "5"
    );
    assert_eq!(one(&pg, "SELECT count FROM item WHERE name = 'nut'"), "9");
    let err = on_x
        .write(r#"addItem($name: "washer", $count: "many")"#)
        .await
        .expect_err("1.1.0 requires qty, and many is no int");
    assert!(err.contains("`item.qty` is required in `1.1.0`"), "{err}");

    dual_writes::revert(&cx, "acme", OWNER)
        .await
        .expect("reverts");
    assert_eq!(
        one(
            &pg,
            "SELECT count FROM item WHERE \"$space\" = 'acme' AND name = 'nut'"
        ),
        "9"
    );
}

/// 1.0.0: a price held as a decimal, in dollars.
fn priced_as_decimal() -> String {
    items(
        "1.0.0",
        "  cost: decimal(2) = 0\n  fine: decimal(3) = 0\n",
        "\nop addItem($name: text, $cost: decimal(2)) =\n  set item { name: $name, cost: $cost }\n",
    )
}

/// 1.1.0: `cost` renamed to `price` and made `money(2)`, stored in cents.
fn priced_as_money() -> String {
    items(
        "1.1.0",
        "  price: money(2) = 0 prev(cost)\n  fine: decimal(3) = 0\n",
        "\nop addItem($name: text, $price: money(2)) =\n  set item { name: $name, price: $price }\n",
    )
}

/// **A mapping into or out of money converts major and minor units.** A
/// mapping reads and gives money in major units, and a `money(2)` column
/// stores cents: `cost` 12.34 is filled as `price` 1234, and the reverse
/// writes 5.25 back for a price of 525 cents. A mapping with more places
/// than the money keeps (`decimal(3)` into `money(2)`), or with places Ordo
/// cannot tell, is refused naming `round`. The preview shows dollars. Both
/// versions write through the real path, each shape in its own units.
#[tokio::test(flavor = "multi_thread")]
async fn a_mapping_into_or_out_of_money_converts_its_units() {
    let (pg, pool) = deployment("ordo_maps_money", &priced_as_decimal(), &priced_as_money()).await;
    pg.exec(
        "INSERT INTO item (\"$space\", id, rev, name, cost) VALUES \
         ('acme', 'a0', 1, 'bolt', 12.34), ('home', 'h0', 1, 'nut', 0.05)",
    )
    .expect("rows");
    let render = |fill: &'static str, reverse: &'static str| {
        let pool = pool.clone();
        async move {
            evolve::evolve(
                &pool,
                "1.0.0",
                "1.1.0",
                &options(fill, reverse, false),
                None,
                OWNER,
                1,
            )
            .await
        }
    };
    let errs = errors_of(
        render("item.price=fine", "item.cost=price")
            .await
            .expect_err("three places into two"),
    );
    assert_eq!(errs[0].0, "map_cast_required", "{errs:?}");
    assert!(errs[0].1.contains("round"), "{errs:?}");
    let errs = errors_of(
        render("item.price=cost + null", "item.cost=price")
            .await
            .expect_err("places Ordo cannot tell"),
    );
    assert_eq!(errs[0].0, "map_cast_required", "{errs:?}");
    assert!(errs[0].1.contains("round(cost + null, 2)"), "{errs:?}");

    let plan = render("item.price=cost", "item.cost=price")
        .await
        .expect("the conversion stores")
        .plan;
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    ordo_host::environments::Environments::on(&pool)
        .set_preview_values("on", OWNER, "a test shows the rows", 5)
        .await
        .expect("set");
    let checks = ordo_host::precheck::precheck(&pool, &shared, &plan)
        .await
        .expect("checked");
    assert!(
        checks.preview[0].rows[0].contains("price \"12.34\""),
        "{:?}",
        checks.preview
    );
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), 7 * 86_400, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(
            &pg,
            "SELECT string_agg(price::text, ',' ORDER BY id) FROM item"
        ),
        "1234,5"
    );
    let on_x = node(&pg.db, &priced_as_decimal(), "beta").await;
    let on_y = node(&pg.db, &priced_as_money(), "acme").await;
    on_x.write(r#"addItem($name: "washer", $cost: 1.50)"#)
        .await
        .expect("1.0.0 writes dollars");
    on_y.write(r#"addItem($name: "screw", $price: 5.25)"#)
        .await
        .expect("1.1.0 writes money");
    assert_eq!(
        one(&pg, "SELECT price::text FROM item WHERE name = 'washer'"),
        "150"
    );
    assert_eq!(
        one(
            &pg,
            "SELECT price::text || '|' || cost::text FROM item WHERE name = 'screw'"
        ),
        "525|5.25"
    );
}
