//! **Entity splits** (`forward-gaps` ask 8, owner: *"declared
//! correspondence tool based on prev"*), against real Postgres:
//!
//! - `address (prev: person)` while `person` stays declared is a split,
//!   one B13 change: `address` takes the fields it declares from `person`;
//! - the render refuses a split with no rule (`--correspond`), a link that
//!   is not `-> person owned unique`, a part field `person` did not hold
//!   the same way, a moved field required with no default, a split with no
//!   whole, the apply path and `--run`, each naming the fix;
//! - the pre-check previews the rule on 3 rows, values only where
//!   `preview.values` is on;
//! - under a move both shapes stay current through the real write path: a
//!   write on the version before reaches the part, one on the new version
//!   reaches the whole's old columns, deletes too;
//! - a revert in the soak keeps every write; the close orphans the moved
//!   columns, ends the split's `$renames` row and checks the link at once.
//!
//! A regression looks like a split that renders as a rename or a new
//! entity, a person with no address row after the copy, a write on either
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

/// 1.0.0: a person with an address on the row.
fn whole() -> String {
    "domain people @ 1.0.0\n\n\
     entity person {\n  name: text\n  street: text?\n  city: text = \"\"\n}\n\n\
     op everyone unbounded =\n  person[id, name, street, city]\n\n\
     op addPerson($name: text, $street: text, $city: text) =\n  \
       set person { name: $name, street: $street, city: $city }\n\n\
     op moveHouse($id: id, $rev: int, $street: text) =\n  \
       set person { id: $id, rev: $rev, street: $street }\n\n\
     op removePerson($id: id) =\n  del person { id: $id }\n"
        .to_string()
}

/// 1.1.0: the address split off into an entity of its own.
fn split(version: &str, link: &str, address_fields: &str) -> String {
    format!(
        "domain people @ {version}\n\n\
         entity person {{\n  name: text\n}}\n\n\
         entity address (key: adr, prev: person) {{\n  {link}\n{address_fields}}}\n\n\
         op everyone unbounded =\n  person[id, name]\n\n\
         op addresses unbounded =\n  address[id, person, street, city]\n\n\
         op addPerson($name: text) =\n  set person {{ name: $name }}\n\n\
         op setAddress($person: id, $street: text, $city: text) =\n  \
           set address {{ person: $person, street: $street, city: $city }}\n\n\
         op clearAddress($person: id) =\n  del address(person == $person)\n"
    )
}

const LINK: &str = "person -> person owned unique";
const FIELDS: &str = "  street: text?\n  city: text = \"\"\n";

fn parts() -> String {
    split("1.1.0", LINK, FIELDS)
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

fn options(correspond: &str) -> Choices {
    let mut c = evolve::typed_options_with("", "", "", "", "", false).expect("options");
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

/// **The render refuses what it cannot decide, naming the fix.** With no
/// rule it names `--correspond address.person`; a stray rule, a rule with a
/// condition, a link that is not `owned unique`, a part field `person` did
/// not hold, a moved field required with no default, and a split with no
/// whole are each refused by name. The apply path refuses the split, and
/// `--run` of the stored step too: it moves with dual writes. Rendered, it
/// is one B13 change.
#[tokio::test(flavor = "multi_thread")]
async fn the_render_refuses_a_split_until_its_rule_is_given() {
    let (_pg, pool) = deployment("ordo_split_render", &whole(), &parts()).await;
    let errs = errors_of(
        render(&pool, "1.1.0", &Choices::default())
            .await
            .expect_err("no rule"),
    );
    assert_eq!(errs[0].0, "split_rule_required", "{errs:?}");
    assert!(
        errs[0].1.contains("--correspond address.person"),
        "{errs:?}"
    );
    let errs = errors_of(
        render(&pool, "1.1.0", &options("person.person"))
            .await
            .expect_err("a stray rule"),
    );
    assert!(
        errs[0].1.contains("drop it from `--correspond`"),
        "{errs:?}"
    );
    let mut c = Choices::default();
    let err = c
        .parse_correspond("address.person where city != \"\"")
        .expect_err("a condition");
    assert!(err.contains("correspond_rule_not_built"), "{err}");

    for (version, link, fields, class) in [
        (
            "1.2.0",
            "person -> person unique",
            FIELDS,
            "split_entity_link",
        ),
        (
            "1.3.0",
            LINK,
            "  street: text?\n  city: text = \"\"\n  zip: text?\n",
            "split_entity_reshaped",
        ),
        (
            "1.4.0",
            LINK,
            "  street: text\n  city: text = \"\"\n",
            "split_entity_reshaped",
        ),
    ] {
        store(&pool, &split(version, link, fields)).await;
        let errs = errors_of(
            render(&pool, version, &options("address.person"))
                .await
                .expect_err(class),
        );
        let hit = errs.iter().find(|(c, _)| c == class);
        let (_, why) = hit.unwrap_or_else(|| panic!("{version}: no {class} in {errs:?}"));
        assert!(why.contains("person"), "{version}: {why}");
    }

    // A moved field required with no default: the new version's inserts
    // of `person` would leave it to a value nobody wrote.
    let (_pg2, strict) = deployment(
        "ordo_split_strict",
        &whole().replace("street: text?", "street: text"),
        &split("1.1.0", LINK, "  street: text\n  city: text = \"\"\n"),
    )
    .await;
    let errs = errors_of(
        render(&strict, "1.1.0", &options("address.person"))
            .await
            .expect_err("no default"),
    );
    assert_eq!(errs[0].0, "split_default_required", "{errs:?}");

    // Two parts of an entity that is gone: refused, naming the whole.
    store(
        &pool,
        "domain people @ 1.5.0\n\n\
         entity named (prev: person) {\n  name: text\n}\n\n\
         entity address (prev: person) {\n  street: text?\n  city: text = \"\"\n}\n\n\
         op everyone unbounded =\n  named[id]\n",
    )
    .await;
    let errs = errors_of(
        render(&pool, "1.5.0", &Choices::default())
            .await
            .expect_err("no whole"),
    );
    assert!(
        errs.iter()
            .any(|(c, w)| c == "entity_split_needs_whole" && w.contains("keep `person` declared")),
        "{errs:?}"
    );

    // The apply path never splits an entity.
    let program = Program::compile(&parts()).expect("compiles");
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
        text.contains("NotActivated") && text.contains("B13"),
        "{text}"
    );

    let done = render(&pool, "1.1.0", &options("address.person"))
        .await
        .expect("the split stores");
    let receipt: serde_json::Value = serde_json::from_str(&done.plan.receipt).expect("receipt");
    let codes: Vec<&str> = receipt["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter_map(|c| c["code"].as_str())
        .collect();
    assert_eq!(codes, ["B13"], "{receipt}");
    assert_eq!(receipt["entitySplits"][0]["from"], "person", "{receipt}");
    assert!(done.plan.options.contains("correspond address.person"));
    let err = evolve::run_step(&pool, &pool, "deployment", &done.plan, OWNER)
        .await
        .expect_err("a split moves");
    assert_eq!(err.class(), "split_needs_move", "{err}");
}

/// **A split on the deployment's own database: one address per person,
/// both shapes current, a revert in the soak keeps every write.** The
/// preview names the part row each person gives, its values hidden here
/// and shown once `preview.values` is on. `acme` moves: every person in
/// every space has its address row, copied in chunks of 2. `beta` on 1.0.0
/// adds a person and moves house: the part follows; it removes a person:
/// the part row goes. `acme` on 1.1.0 sets an address and clears another:
/// the whole's old columns follow. Reverted, `acme` reads them as 1.0.0.
#[tokio::test(flavor = "multi_thread")]
async fn a_split_keeps_both_shapes_current_and_reverts_in_the_soak() {
    let (pg, pool) = deployment("ordo_split_move", &whole(), &parts()).await;
    for (s, rows) in [
        ("acme", [("a0", "Ada", "'1 Lane'"), ("a1", "Alan", "NULL")]),
        (
            "beta",
            [("b0", "Barbara", "'2 Road'"), ("b1", "Ken", "NULL")],
        ),
        (
            "home",
            [("h0", "Grace", "'3 Way'"), ("h1", "Edsger", "NULL")],
        ),
    ] {
        for (id, name, street) in rows {
            pg.exec(&format!(
                "INSERT INTO person (\"$space\", id, rev, name, street, city) \
                 VALUES ('{s}', '{id}', 1, '{name}', {street}, 'Town')"
            ))
            .expect("rows");
        }
    }
    let plan = render(&pool, "1.1.0", &options("address.person"))
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
        p.rows[0], "acme/a0 -> address adr_a0 (person a0): street, city",
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
        "acme/a0 -> address adr_a0 (person a0): street \"1 Lane\", city \"Town\""
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
    assert_eq!(one(&pg, "SELECT count(*) FROM address"), "6");
    assert_eq!(
        one(
            &pg,
            "SELECT id || '|' || street || '|' || city FROM address WHERE person = 'h0'"
        ),
        "adr_h0|3 Way|Town"
    );

    // The version before writes the whole; its part follows.
    let beta = node(&pg.db, &whole(), "beta").await;
    beta.write(r#"addPerson($name: "Liskov", $street: "4 Close", $city: "Boston")"#)
        .await
        .expect("beta writes as 1.0.0");
    assert_eq!(
        one(
            &pg,
            "SELECT a.street || '|' || a.city FROM address a JOIN person p \
             ON p.\"$space\" = a.\"$space\" AND p.id = a.person WHERE p.name = 'Liskov'"
        ),
        "4 Close|Boston"
    );
    beta.write(r#"moveHouse($id: "b0", $rev: 1, $street: "9 Hill")"#)
        .await
        .expect("beta moves house");
    assert_eq!(
        one(&pg, "SELECT street FROM address WHERE person = 'b0'"),
        "9 Hill"
    );
    beta.write(r#"removePerson($id: "b1")"#)
        .await
        .expect("beta removes a person");
    assert_eq!(
        one(&pg, "SELECT count(*) FROM address WHERE person = 'b1'"),
        "0"
    );

    // The new version writes the part; the whole's old columns follow.
    let acme = node(&pg.db, &parts(), "acme").await;
    acme.write(r#"setAddress($person: "a1", $street: "5 Mews", $city: "Leeds")"#)
        .await
        .expect("acme sets an address on 1.1.0");
    assert_eq!(
        one(
            &pg,
            "SELECT street || '|' || city FROM person WHERE id = 'a1'"
        ),
        "5 Mews|Leeds"
    );
    acme.write(r#"clearAddress($person: "a0")"#)
        .await
        .expect("acme clears an address on 1.1.0");
    assert_eq!(
        one(
            &pg,
            "SELECT coalesce(street, 'none') || '|' || city FROM person WHERE id = 'a0'"
        ),
        "none|"
    );
    acme.write(r#"addPerson($name: "Turing")"#)
        .await
        .expect("acme adds a person with no address");

    // A revert in the soak: acme reads what 1.1.0 wrote as 1.0.0.
    let back = dual_writes::revert(&cx, "acme", OWNER)
        .await
        .expect("reverts");
    assert_eq!(back.phase, MovePhase::Gated);
    let rows = acme_reads(&pg).await;
    assert!(rows.contains("Alan|5 Mews|Leeds"), "{rows}");
    assert!(rows.contains("Ada|none|"), "{rows}");
    assert!(rows.contains("Turing|none|"), "{rows}");
    // And on 1.0.0 again, its writes reach the part.
    let acme_x = node(&pg.db, &whole(), "acme").await;
    acme_x
        .write(r#"moveHouse($id: "a1", $rev: 2, $street: "6 Row")"#)
        .await
        .expect("acme writes as 1.0.0");
    assert_eq!(
        one(&pg, "SELECT street FROM address WHERE person = 'a1'"),
        "6 Row"
    );
}

async fn acme_reads(pg: &Pg) -> String {
    pg.query(
        "SELECT name || '|' || coalesce(street, 'none') || '|' || city FROM person \
         WHERE \"$space\" = 'acme' ORDER BY name",
    )
    .expect("reads")
    .into_iter()
    .filter_map(|r| r.into_iter().next().flatten())
    .collect::<Vec<_>>()
    .join("; ")
}

/// **A split on a tenant's own database copies under writes and closes.**
/// The copy stops after one chunk; 1.0.0 writes a copied row, a row not
/// copied yet and a new one, and each reaches the part. The tenant moves,
/// writes on 1.1.0, and the soak closes: `person.street` and `person.city` are
/// orphaned, the split leaves `$renames`, the link is checked at once again
/// and the database matches 1.1.0.
#[tokio::test(flavor = "multi_thread")]
async fn a_split_on_a_tenants_own_database_closes_and_orphans_the_moved_columns() {
    let (_reg_pg, registry) = empty("ordo_split_close_reg").await;
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
    let (tpg, tenant) = empty("ordo_split_close_tenant").await;
    let first = store(&registry, &whole()).await;
    store(&registry, &parts()).await;
    let (_, created) = ordo_host::schema_apply::apply(&tenant, &first, "test", "test").await;
    assert!(
        matches!(created, Ok(ordo_host::schema_apply::Applied::Created(_))),
        "{created:?}"
    );
    tpg.exec(
        "INSERT INTO person (\"$space\", id, rev, name, street, city) \
         SELECT 'acme', 'p' || i, 1, 'Person' || i, i || ' Street', 'Town' \
         FROM generate_series(1, 9) i",
    )
    .expect("rows");
    let plan = render(&registry, "1.1.0", &options("address.person"))
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
    assert_eq!(one(&tpg, "SELECT count(*) FROM address"), "2");
    let on_x = node(&tpg.db, &whole(), "acme").await;
    on_x.write(r#"moveHouse($id: "p1", $rev: 1, $street: "Behind")"#)
        .await
        .expect("a copied row");
    on_x.write(r#"moveHouse($id: "p9", $rev: 1, $street: "Ahead")"#)
        .await
        .expect("a row not copied yet");
    on_x.write(r#"addPerson($name: "Late", $street: "10 Street", $city: "Town")"#)
        .await
        .expect("a new row");
    let moved = dual_writes::run(&cx, "acme", &plan, &step_context(), 7 * 86_400, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(one(&tpg, "SELECT count(*) FROM address"), "10");
    assert_eq!(
        one(
            &tpg,
            "SELECT string_agg(street, ',' ORDER BY person) FROM address \
             WHERE person IN ('p1', 'p9')"
        ),
        "Behind,Ahead"
    );
    let on_y = node(&tpg.db, &parts(), "acme").await;
    on_y.write(r#"setAddress($person: "p3", $street: "New", $city: "City")"#)
        .await
        .expect("a write on 1.1.0");
    let closed = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect("closes");
    assert!(closed.closed);
    let mut orphaned = closed.orphaned.clone();
    orphaned.sort();
    assert!(
        orphaned.contains(&"person.street".to_string())
            && orphaned.contains(&"person.city".to_string()),
        "{orphaned:?}"
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
            "SELECT condeferrable::text FROM pg_constraint WHERE conname = 'address$person$fk'"
        ),
        "false"
    );
    assert_eq!(
        one(&tpg, "SELECT street FROM address WHERE person = 'p3'"),
        "New"
    );
}
