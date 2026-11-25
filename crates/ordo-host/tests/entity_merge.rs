//! **Moves from another entity and entity merges** (`forward-gaps` asks 7
//! and 8, owner: *"dev named"*, *"declared correspondence tool based on
//! prev"*), against real Postgres:
//!
//! - `street: text? prev(address.street)` in `person`, with `address` gone
//!   and every field of it moved, is an entity merge, one B14 change;
//! - the render refuses a move with no rule (`--correspond`), a rule that
//!   names no link, a link that is not unique, a field moved as it was not
//!   declared, a merge that leaves a field behind, a renamed field, a field
//!   required with no default, a mapping over two entities, the apply path
//!   and `--run`, each naming the fix; a move whose old entity stays needs
//!   `--one-way`;
//! - the pre-check previews the rule on 3 rows, values only where
//!   `preview.values` is on;
//! - under a move both shapes stay current through the real write path: a
//!   write of `address` on the version before reaches `person`, deletes
//!   too, and a write of `person` on the new version reaches `address`,
//!   making its row where there was none;
//! - a revert in the soak keeps every write; the close orphans `address`
//!   and drops its references, ends the move's `$renames` row and lands the
//!   new columns' `NOT NULL`; a one way move refuses the revert.
//!
//! A regression looks like a merge that renders as a drop and an add, a
//! person missing its address values after the copy, a write on either
//! version that leaves the other shape stale, or a refusal with no fix.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::{Pool, PoolConfig};
use ordo_host::dual_writes::{self, MoveContext};
use ordo_host::evolve::{self, EvolveRefusal};
use ordo_host::schema_apply::{Choices, Program};
use ordo_host::versions::{MovePhase, Versions};

const OWNER: &str = "deployment-owner:ada";

const LINK: &str = "person -> person owned unique";
const ADDRESS: &str = "  street: text?\n  city: text = \"\"\n";

/// 1.0.0: a person, and an address of its own in an entity beside it.
fn parts_with(link: &str, address: &str) -> String {
    format!(
        "domain people @ 1.0.0\n\n\
         entity person {{\n  name: text\n}}\n\n\
         entity address (key: adr) {{\n  {link}\n{address}}}\n\n\
         op everyone unbounded =\n  person[id, name]\n\n\
         op addresses unbounded =\n  address[id, person, street, city]\n\n\
         op addPerson($name: text) =\n  set person {{ name: $name }}\n\n\
         op setAddress($person: id, $street: text, $city: text) =\n  \
           set address {{ person: $person, street: $street, city: $city }}\n\n\
         op clearAddress($person: id) =\n  del address(person == $person)\n"
    )
}

fn parts() -> String {
    parts_with(LINK, ADDRESS)
}

/// 1.1.0: the address merged into `person`.
fn merged_as(version: &str, person_fields: &str) -> String {
    format!(
        "domain people @ {version}\n\n\
         entity person {{\n  name: text\n{person_fields}}}\n\n\
         op everyone unbounded =\n  person[id, name, street, city]\n\n\
         op addPerson($name: text, $street: text, $city: text) =\n  \
           set person {{ name: $name, street: $street, city: $city }}\n\n\
         op moveHouse($id: id, $rev: int, $street: text) =\n  \
           set person {{ id: $id, rev: $rev, street: $street }}\n\n\
         op removePerson($id: id) =\n  del person {{ id: $id }}\n"
    )
}

const MOVED: &str =
    "  street: text? prev(address.street)\n  city: text = \"\" prev(address.city)\n";

fn merged() -> String {
    merged_as("1.1.0", MOVED)
}

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

/// The deployment's own database, `acme` and `beta` reserved, `x` applied
/// and active, `y` stored.
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

/// **The render refuses what it cannot decide, naming the fix.** With no
/// rule it names `--correspond address.person`; a stray rule, a rule naming
/// a field that is no link, a link that is not unique, a field moved as it
/// was not declared, a merge that leaves a field behind, a renamed field,
/// a mapping over two entities, a required field with no default, and a
/// one way move not accepted are each refused by name. The apply path
/// refuses the merge, and `--run` of the stored step too. Rendered, it is
/// one B14 change.
#[tokio::test(flavor = "multi_thread")]
async fn the_render_refuses_a_move_until_its_rule_is_given() {
    let (_pg, pool) = deployment("ordo_merge_render", &parts(), &merged()).await;
    let errs = errors_of(
        render(&pool, "1.1.0", &Choices::default())
            .await
            .expect_err("no rule"),
    );
    assert!(
        has(&errs, "move_rule_required", "--correspond address.person"),
        "{errs:?}"
    );
    let errs = errors_of(
        render(&pool, "1.1.0", &options("person.person", false))
            .await
            .expect_err("a stray rule"),
    );
    // A rule for the entity the fields move into names a link of it.
    assert!(
        has(
            &errs,
            "move_link",
            "`person.person` is not a link to `address`"
        ),
        "{errs:?}"
    );
    let errs = errors_of(
        render(&pool, "1.1.0", &options("address.city", false))
            .await
            .expect_err("no link"),
    );
    assert!(
        has(&errs, "move_link", "-> person owned unique"),
        "{errs:?}"
    );

    for (version, fields, class, text) in [
        (
            "1.2.0",
            "  street: text? prev(address.street)\n  city: text? prev(address.city)\n",
            "move_reshaped",
            "as `address.city` was",
        ),
        (
            "1.3.0",
            "  street: text? prev(address.street)\n",
            "move_reshaped",
            "prev(address.city)",
        ),
        (
            "1.4.0",
            "  road: text? prev(address.street)\n  city: text = \"\" prev(address.city)\n",
            "move_renamed",
            "rename it to `road`",
        ),
        (
            "1.6.0",
            "  street: text? prev(address.street, address.city)\n",
            "prev_mapping_required",
            "move the fields into `person` first",
        ),
    ] {
        store(&pool, &merged_as(version, fields)).await;
        let errs = errors_of(
            render(&pool, version, &options("address.person", false))
                .await
                .expect_err(class),
        );
        assert!(has(&errs, class, text), "{version}: {errs:?}");
    }

    // A move whose old entity stays: one way, accepted by name.
    let kept = merged_as("1.5.0", "  street: text? prev(address.street)\n").replace(
        "op everyone",
        &format!("entity address (key: adr) {{\n  {LINK}\n  city: text = \"\"\n}}\n\nop everyone"),
    );
    store(&pool, &kept).await;
    let errs = errors_of(
        render(&pool, "1.5.0", &options("address.person", false))
            .await
            .expect_err("one way"),
    );
    assert!(has(&errs, "one_way_unconfirmed", "--one-way"), "{errs:?}");
    let one_way = render(&pool, "1.5.0", &options("address.person", true))
        .await
        .expect("a one way move stores");
    let receipt: serde_json::Value = serde_json::from_str(&one_way.plan.receipt).expect("receipt");
    assert_eq!(receipt["entityMoves"][0]["merge"], false, "{receipt}");

    // The apply path never moves a field.
    let program = Program::compile(&merged()).expect("compiles");
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
    .expect_err("apply refuses a merge");
    let text = format!("{err:?}");
    assert!(
        text.contains("NotActivated") && text.contains("B14"),
        "{text}"
    );

    let done = render(&pool, "1.1.0", &options("address.person", false))
        .await
        .expect("the merge stores");
    let receipt: serde_json::Value = serde_json::from_str(&done.plan.receipt).expect("receipt");
    let codes: Vec<&str> = receipt["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter_map(|c| c["code"].as_str())
        .collect();
    assert_eq!(codes, ["B14"], "{receipt}");
    assert_eq!(receipt["entityMoves"][0]["from"], "address", "{receipt}");
    assert_eq!(receipt["entityMoves"][0]["merge"], true, "{receipt}");
    assert!(done.plan.options.contains("correspond address.person"));
    let err = evolve::run_step(&pool, &pool, "deployment", &done.plan, OWNER)
        .await
        .expect_err("a merge moves");
    assert_eq!(err.class(), "move_needs_move", "{err}");

    // A link that is not unique: several addresses per person, and the
    // rule must name which one gives the value (`forward-gaps` ask 7).
    // Named, a merge through it is not built: the rows left out would be
    // lost.
    let (_pg2, many) = deployment(
        "ordo_merge_many",
        &parts_with("person -> person owned", ADDRESS),
        &merged(),
    )
    .await;
    let errs = errors_of(
        render(&many, "1.1.0", &options("address.person", false))
            .await
            .expect_err("not unique"),
    );
    assert!(
        has(&errs, "move_rule_required", "latest by <field>"),
        "{errs:?}"
    );
    let errs = errors_of(
        render(
            &many,
            "1.1.0",
            &options("address.person latest by city", false),
        )
        .await
        .expect_err("a merge"),
    );
    assert!(
        has(
            &errs,
            "correspond_rule_not_built",
            "Keep `address` in this version"
        ),
        "{errs:?}"
    );

    // A required field with no default: a person no address names has no
    // value for it.
    let (_pg3, strict) = deployment(
        "ordo_merge_strict",
        &parts_with(LINK, "  street: text?\n  city: text\n"),
        &merged_as(
            "1.1.0",
            "  street: text? prev(address.street)\n  city: text prev(address.city)\n",
        ),
    )
    .await;
    let errs = errors_of(
        render(&strict, "1.1.0", &options("address.person", false))
            .await
            .expect_err("no default"),
    );
    assert!(
        has(&errs, "move_default_required", "city: text = <value>"),
        "{errs:?}"
    );
}

async fn acme_reads(pg: &Pg) -> String {
    pg.query(
        "SELECT p.name || '|' || coalesce(a.street, 'none') || '|' || coalesce(a.city, 'none') \
         FROM person p LEFT JOIN address a ON a.\"$space\" = p.\"$space\" AND a.person = p.id \
         WHERE p.\"$space\" = 'acme' ORDER BY p.name",
    )
    .expect("reads")
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect::<Vec<_>>()
    .join("; ")
}

/// **An entity merge on the deployment's own database: each address into
/// its person, both shapes current, a revert in the soak keeps every
/// write.** The preview names the person each address gives its values to,
/// hidden here and shown once `preview.values` is on. `acme` moves: every
/// person with an address in every space holds its values, copied in chunks
/// of 2; a person with none holds the defaults. `beta` on 1.0.0 sets an
/// address and clears another: the person's columns follow. `acme` on 1.1.0
/// moves house, for a person with an address and one without, and adds a
/// person: each address row follows, made where there was none. Reverted,
/// `acme` reads them as 1.0.0, and its writes there reach `person` again.
#[tokio::test(flavor = "multi_thread")]
async fn a_merge_keeps_both_shapes_current_and_reverts_in_the_soak() {
    let (pg, pool) = deployment("ordo_merge_move", &parts(), &merged()).await;
    for (s, rows) in [
        ("acme", [("a0", "Ada", true), ("a1", "Alan", false)]),
        ("beta", [("b0", "Barbara", true), ("b1", "Ken", true)]),
        ("home", [("h0", "Grace", true), ("h1", "Edsger", false)]),
    ] {
        for (id, name, address) in rows {
            pg.exec(&format!(
                "INSERT INTO person (\"$space\", id, rev, name) VALUES ('{s}', '{id}', 1, '{name}')"
            ))
            .expect("people");
            if address {
                pg.exec(&format!(
                    "INSERT INTO address (\"$space\", id, rev, person, street, city) \
                     VALUES ('{s}', 'adr_{id}', 1, '{id}', '{id} Lane', 'Town')"
                ))
                .expect("addresses");
            }
        }
    }
    let plan = render(&pool, "1.1.0", &options("address.person", false))
        .await
        .expect("stores")
        .plan;
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let checks = ordo_host::precheck::precheck(&pool, &shared, &plan)
        .await
        .expect("checked");
    assert!(checks.issues.is_empty(), "{:?}", checks.issues);
    let p = &checks.preview[0];
    assert!(!p.values);
    assert_eq!(
        p.rows[0], "acme/adr_a0 -> person a0 (person): street, city",
        "{:?}",
        p.rows
    );
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
    assert_eq!(
        checks.preview[0].rows[0],
        "acme/adr_a0 -> person a0 (person): street \"a0 Lane\", city \"Town\""
    );

    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let week = 7 * 86_400;
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(
            &pg,
            "SELECT string_agg(id || '=' || coalesce(street, 'none') || '/' || city, ',' \
             ORDER BY id) FROM person"
        ),
        "a0=a0 Lane/Town,a1=none/,b0=b0 Lane/Town,b1=b1 Lane/Town,h0=h0 Lane/Town,h1=none/"
    );

    // The version before writes the address; its person follows.
    let beta = node(&pg.db, &parts(), "beta").await;
    beta.write(r#"setAddress($person: "b0", $street: "9 Hill", $city: "Leeds")"#)
        .await
        .expect("beta writes as 1.0.0");
    assert_eq!(
        one(
            &pg,
            "SELECT street || '|' || city FROM person WHERE id = 'b0'"
        ),
        "9 Hill|Leeds"
    );
    beta.write(r#"clearAddress($person: "b1")"#)
        .await
        .expect("beta clears an address");
    assert_eq!(
        one(
            &pg,
            "SELECT coalesce(street, 'none') || '|' || city FROM person WHERE id = 'b1'"
        ),
        "none|"
    );

    // The new version writes the person; its address follows, made where
    // there was none.
    let acme = node(&pg.db, &merged(), "acme").await;
    acme.write(r#"moveHouse($id: "a0", $rev: 1, $street: "5 Mews")"#)
        .await
        .expect("acme moves house on 1.1.0");
    assert_eq!(
        one(&pg, "SELECT street FROM address WHERE person = 'a0'"),
        "5 Mews"
    );
    acme.write(r#"moveHouse($id: "a1", $rev: 1, $street: "6 Row")"#)
        .await
        .expect("acme gives Alan a street on 1.1.0");
    assert_eq!(
        one(
            &pg,
            "SELECT id || '|' || street FROM address WHERE person = 'a1'"
        ),
        "adr_a1|6 Row"
    );
    acme.write(r#"addPerson($name: "Turing", $street: "7 Park", $city: "York")"#)
        .await
        .expect("acme adds a person on 1.1.0");

    // A revert in the soak: acme reads what 1.1.0 wrote as 1.0.0.
    let back = dual_writes::revert(&cx, "acme", OWNER)
        .await
        .expect("reverts");
    assert_eq!(back.phase, MovePhase::Gated);
    let rows = acme_reads(&pg).await;
    assert!(rows.contains("Ada|5 Mews|Town"), "{rows}");
    assert!(rows.contains("Alan|6 Row|"), "{rows}");
    assert!(rows.contains("Turing|7 Park|York"), "{rows}");
    // And on 1.0.0 again, its writes reach the person.
    let acme_x = node(&pg.db, &parts(), "acme").await;
    acme_x
        .write(r#"setAddress($person: "a1", $street: "8 Court", $city: "Hull")"#)
        .await
        .expect("acme writes as 1.0.0");
    assert_eq!(
        one(
            &pg,
            "SELECT street || '|' || city FROM person WHERE id = 'a1'"
        ),
        "8 Court|Hull"
    );
}

/// **A merge on a tenant's own database copies under writes and closes.**
/// The copy stops after one chunk; 1.0.0 writes a copied address, one not
/// copied yet and a new one, and each reaches its person. The tenant moves,
/// writes on 1.1.0, and the soak closes: `address` is orphaned and its
/// references dropped, the move leaves `$renames`, `person.city` is
/// required again, and the database matches 1.1.0. On 1.1.0 a person with
/// an address can then be removed.
#[tokio::test(flavor = "multi_thread")]
async fn a_merge_on_a_tenants_own_database_closes_and_orphans_the_old_entity() {
    let (_reg_pg, registry) = empty("ordo_merge_close_reg").await;
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
    let (tpg, tenant) = empty("ordo_merge_close_tenant").await;
    let first = store(&registry, &parts()).await;
    store(&registry, &merged()).await;
    let (_, created) = ordo_host::schema_apply::apply(&tenant, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(ordo_host::schema_apply::Applied::Created(_))),
        "{created:?}"
    );
    tpg.exec(
        "INSERT INTO person (\"$space\", id, rev, name) \
         SELECT 'acme', 'p' || i, 1, 'Person' || i FROM generate_series(1, 10) i",
    )
    .expect("people");
    tpg.exec(
        "INSERT INTO address (\"$space\", id, rev, person, street, city) \
         SELECT 'acme', 'adr_p' || i, 1, 'p' || i, i || ' Street', 'Town' \
         FROM generate_series(1, 9) i",
    )
    .expect("addresses");
    let plan = render(&registry, "1.1.0", &options("address.person", false))
        .await
        .expect("stores")
        .plan;
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
    // One chunk of 2, then writes on 1.0.0 behind and ahead of the cursor.
    let mut ctx = step_context();
    ctx.settings.budget = Some(1);
    let part = dual_writes::run(&cx, "acme", &plan, &ctx, 7 * 86_400, OWNER)
        .await
        .expect("one chunk");
    assert_eq!(part.phase, MovePhase::Backfilling, "{part:?}");
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM person WHERE street IS NOT NULL"),
        "2"
    );
    let on_x = node(&tpg.db, &parts(), "acme").await;
    on_x.write(r#"setAddress($person: "p1", $street: "Behind", $city: "Town")"#)
        .await
        .expect("a copied row");
    on_x.write(r#"setAddress($person: "p9", $street: "Ahead", $city: "Town")"#)
        .await
        .expect("a row not copied yet");
    on_x.write(r#"setAddress($person: "p10", $street: "New", $city: "Town")"#)
        .await
        .expect("a new row");
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), 7 * 86_400, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM person WHERE street IS NOT NULL"),
        "10"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT string_agg(street, ',' ORDER BY id) FROM person \
             WHERE id IN ('p1', 'p9', 'p10')"
        ),
        "Behind,New,Ahead"
    );
    let on_y = node(&tpg.db, &merged(), "acme").await;
    on_y.write(r#"moveHouse($id: "p3", $rev: 1, $street: "Moved")"#)
        .await
        .expect("a write on 1.1.0");
    let closed = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect("closes");
    assert!(closed.closed);
    assert!(
        closed.orphaned.contains(&"address".to_string()),
        "{:?}",
        closed.orphaned
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM \"$renames\" WHERE \"entity\" = 'address'"
        ),
        "0"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM pg_constraint WHERE conname = 'address$person$fk'"
        ),
        "0"
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'person' AND column_name = 'city'"
        ),
        "NO"
    );
    assert_eq!(
        one(&tpg, "SELECT street FROM person WHERE id = 'p3'"),
        "Moved"
    );
    // On 1.1.0 alone, a person whose address was merged is removed.
    on_y.write(r#"removePerson($id: "p2")"#)
        .await
        .expect("a delete on 1.1.0 after the close");
}

/// **A move whose old entity stays is one way.** `person.street` moves out
/// of `address`, which keeps its city. Accepted with `--one-way`, the copy
/// runs and a write of `address.street` on 1.0.0 reaches `person`; the soak
/// refuses a revert by name, and the close orphans `address.street` only.
#[tokio::test(flavor = "multi_thread")]
async fn a_one_way_move_refuses_the_revert_and_orphans_the_old_column() {
    let kept = merged_as("1.1.0", "  street: text? prev(address.street)\n").replace(
        "op everyone",
        &format!("entity address (key: adr) {{\n  {LINK}\n  city: text = \"\"\n}}\n\nop everyone"),
    );
    let kept = kept
        .replace("person[id, name, street, city]", "person[id, name, street]")
        .replace(
            "set person { name: $name, street: $street, city: $city }",
            "set person { name: $name, street: $street }",
        );
    let (_reg_pg, registry) = empty("ordo_move_reg").await;
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
    let (tpg, tenant) = empty("ordo_move_tenant").await;
    let first = store(&registry, &parts()).await;
    store(&registry, &kept).await;
    let (_, created) = ordo_host::schema_apply::apply(&tenant, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(ordo_host::schema_apply::Applied::Created(_))),
        "{created:?}"
    );
    tpg.exec(
        "INSERT INTO person (\"$space\", id, rev, name) VALUES ('acme', 'p1', 1, 'Ada'), \
         ('acme', 'p2', 1, 'Alan'); \
         INSERT INTO address (\"$space\", id, rev, person, street, city) \
         VALUES ('acme', 'adr_p1', 1, 'p1', '1 Lane', 'Town')",
    )
    .expect("rows");
    let plan = render(&registry, "1.1.0", &options("address.person", true))
        .await
        .expect("stores")
        .plan;
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
    let mut ctx = step_context();
    ctx.settings.budget = Some(1);
    dual_writes::run(&cx, "acme", &plan, &ctx, 7 * 86_400, OWNER)
        .await
        .expect("the copy starts");
    let on_x = node(&tpg.db, &parts(), "acme").await;
    on_x.write(r#"setAddress($person: "p2", $street: "2 Road", $city: "York")"#)
        .await
        .expect("a write on 1.0.0");
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), 7 * 86_400, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(
            &tpg,
            "SELECT string_agg(street, ',' ORDER BY id) FROM person"
        ),
        "1 Lane,2 Road"
    );
    let on_y = node(&tpg.db, &kept, "acme").await;
    on_y.write(r#"moveHouse($id: "p1", $rev: 1, $street: "Moved")"#)
        .await
        .expect("a write on 1.1.0");
    let err = dual_writes::revert(&cx, "acme", OWNER)
        .await
        .expect_err("one way");
    assert_eq!(err.class(), "one_way_step", "{err}");
    let closed = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect("closes");
    assert!(
        closed.orphaned.contains(&"address.street".to_string())
            && !closed.orphaned.contains(&"address".to_string()),
        "{:?}",
        closed.orphaned
    );
    assert_eq!(
        one(&tpg, "SELECT city FROM address WHERE person = 'p2'"),
        "York"
    );
}

/// **A move onto a column an earlier step orphaned is refused by name.**
/// `person` on the tenant's database still holds a `street` column nobody
/// keeps current. The move would land the moved values beside its stale
/// ones, so the tenant's step refuses, naming `ordo evolve --prune`, and
/// nothing moves.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_onto_an_orphaned_column_is_refused() {
    let (_reg_pg, registry) = empty("ordo_merge_orphan_reg").await;
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
    let (tpg, tenant) = empty("ordo_merge_orphan_tenant").await;
    let first = store(&registry, &parts()).await;
    store(&registry, &merged()).await;
    let (_, created) = ordo_host::schema_apply::apply(&tenant, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(ordo_host::schema_apply::Applied::Created(_))),
        "{created:?}"
    );
    tpg.exec("ALTER TABLE person ADD COLUMN street text")
        .expect("an orphan");
    let plan = render(&registry, "1.1.0", &options("address.person", false))
        .await
        .expect("stores")
        .plan;
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
    let err = dual_writes::run(&cx, "acme", &plan, &step_context(), 7 * 86_400, OWNER)
        .await
        .expect_err("an orphaned column");
    let text = err.to_string();
    assert!(
        text.contains("move_column_orphaned") && text.contains("--prune person.street"),
        "{text}"
    );
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM information_schema.columns WHERE table_name = 'person' AND column_name = 'city'"),
        "0"
    );
}
