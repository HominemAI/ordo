//! `prev` in the plan (`plan-versions.md` increment 15,
//! `plan-syntax-additions.md` §5), against real Postgres:
//!
//! - a declared field rename renders as a rename (B4), never a drop and an
//!   add, with its forward and reverse statements;
//! - the same edit without `prev` renders as a new required field, which is
//!   refused;
//! - a tenant replaying the stored step keeps every value under the new
//!   name, and the old column is kept with its `NOT NULL`;
//! - both versions' writes reach both columns through the dual writes
//!   (increment 11a), and no database trigger is made;
//! - the plan checks: `prev_unresolved`, `prev_type`,
//!   `prev_mapping_required`, `prev_sealed`, `prev_default_required`, and
//!   `prev_stale` as a warning;
//! - an entity rename renders as a rename and is refused until the backfill
//!   worker, naming the way round;
//! - the offer to remove a landed mark, only in the chain's last
//!   environment.
//!
//! A regression looks like a rename that renders as D1 plus A2 or B2, a
//! value missing under the new name, a refusal with no fix, or an offer to
//! remove a mark before the last environment has it.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_conformance::Pg;
use ordo_exec::overlay::{self, MoveOverlay};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::evolve::{self, EvolveRefusal};
use ordo_host::schema_apply::{self as sa, Choices, Program};
use ordo_host::versions::Versions;

const OWNER: &str = "deployment-owner:ada";

fn notes(version: &str, fields: &str) -> String {
    format!(
        "domain notes @ {version}\n\nentity note {{\n{fields}}}\n\n\
         op allNotes unbounded =\n  note[body]\n"
    )
}

const V1: &str = "  title: text\n  body: text?\n";
const V2: &str = "  heading: text = \"\" prev(title)\n  body: text?\n";

async fn empty(stem: &str) -> (Pg, Pool) {
    let pg = Pg::scoped(stem);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 3,
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

/// The schema `version` writes with.
fn overlay(version: &Program, _y: &Program) -> ordo_schema::Schema {
    version.schema.clone()
}

/// One write as the executor makes it (`ordo_exec::Executor`'s attempt):
/// `$renames` read first under its share lock, the statement, then the
/// dual writes of the renames it read, as the serving version's schema
/// reads them, all in one SERIALIZABLE transaction. The writer knows
/// nothing of the rename: the database says.
async fn write(
    pool: &Pool,
    schema: &ordo_schema::Schema,
    sql: &str,
    id: &str,
) -> Result<(), String> {
    for _ in 0..20 {
        let tx = pool
            .begin_for(ordo_exec::OpClass::MultiStatementGroup)
            .await
            .map_err(|e| e.to_string())?;
        let renames = match overlay::renames_in(&tx).await {
            Ok(r) => r,
            Err(e) if e.is_missing_table() => {
                let _ = tx.rollback().await;
                pool.rows(overlay::RENAMES_DDL)
                    .await
                    .map_err(|e| e.to_string())?;
                continue;
            }
            Err(e) => {
                let _ = tx.rollback().await;
                return Err(e.to_string());
            }
        };
        let done = async {
            tx.rows(sql).await.map_err(|e| format!("{e:?}"))?;
            if let Some(o) = MoveOverlay::for_renames(schema, &renames) {
                o.apply(&tx, "acme", &[("note".to_string(), id.to_string())])
                    .await
                    .map_err(|e| format!("{e:?}"))?;
            }
            Ok::<(), String>(())
        }
        .await;
        match done {
            Ok(()) => match tx.commit().await {
                Ok(()) => return Ok(()),
                Err(e) if e.to_string().contains("40001") || e.to_string().contains("serializ") => {
                    continue
                }
                Err(e) => return Err(e.to_string()),
            },
            Err(e) => {
                let _ = tx.rollback().await;
                if e.contains("Serialization") || e.contains("40001") || e.contains("40P01") {
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err("the write did not land in 20 tries".into())
}

fn errors(r: Result<evolve::Rendered, EvolveRefusal>) -> Vec<(String, String, String)> {
    match r {
        Err(EvolveRefusal::Errors { errors, .. }) => errors
            .into_iter()
            .map(|e| (e.class, e.why, e.fix))
            .collect(),
        other => panic!("expected plan errors, got {other:?}"),
    }
}

/// **A declared rename renders as a rename**, and a tenant that replays it
/// keeps every value under the new name.
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_rename_renders_as_a_rename_and_keeps_every_value() {
    let (_reg_pg, registry) = empty("ordo_prev_reg").await;
    let (tenant_pg, tenant) = empty("ordo_prev_tenant").await;
    let x = store(&registry, &notes("1.0.0", V1)).await;
    let y = store(&registry, &notes("1.1.0", V2)).await;
    store(
        &registry,
        &notes("1.2.0", "  heading: text\n  body: text?\n"),
    )
    .await;

    let r = evolve::render(&registry, "1.0.0", "1.1.0", &Choices::default(), 1)
        .await
        .expect("a declared rename renders");
    let changes = r.receipt["changes"].as_array().unwrap();
    let codes: Vec<&str> = changes
        .iter()
        .map(|c| c["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["B4"], "{changes:?}");
    let lines = evolve::receipt_lines(&r.receipt).join("\n");
    assert!(
        lines.contains("`note.title` is renamed to `note.heading`"),
        "{lines}"
    );
    assert!(
        lines.contains("forward: ALTER TABLE \"note\" ADD COLUMN \"heading\""),
        "{lines}"
    );
    // A stored plan copies the values with the backfill worker, so a tenant
    // keeps writing while it moves (increment 11a); the new column's
    // `NOT NULL` lands after the copy.
    assert!(lines.contains("backfill: note.heading = title"), "{lines}");
    assert!(
        lines.contains(
            "after backfill: ALTER TABLE \"note\" ALTER COLUMN \"heading\" SET NOT NULL;"
        ),
        "{lines}"
    );
    // The old column keeps its `NOT NULL`: 1.0.0 still loads over it. It
    // has no default, so it takes the new field's, and an insert by 1.1.0
    // is taken.
    assert!(!lines.contains("DROP NOT NULL"), "{lines}");
    assert!(
        lines.contains("forward: ALTER TABLE \"note\" ALTER COLUMN \"title\" SET DEFAULT '';"),
        "{lines}"
    );
    assert!(
        !lines.contains("TRIGGER \"note"),
        "no trigger is made: {lines}"
    );
    assert!(
        lines.contains("reverse: UPDATE \"note\" SET \"title\" = \"heading\";"),
        "{lines}"
    );
    assert!(!lines.contains("D1"), "no drop: {lines}");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    // The same edit without `prev` is a new required field and a removed
    // one: refused, since nothing says where the values come from.
    let without = errors(evolve::render(&registry, "1.0.0", "1.2.0", &Choices::default(), 1).await);
    assert!(
        without.iter().any(|(c, _, _)| c == "not_class_a"),
        "{without:?}"
    );

    // A tenant at 1.0.0 with a row replays the stored step.
    evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect("stores");
    let (_, created) = sa::apply(&tenant, &x, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    tenant_pg
        .exec("INSERT INTO note (\"$space\", id, rev, title) VALUES ('acme', 'n1', 1, 'first')")
        .expect("a row");
    evolve::replay(&registry, &tenant, "acme", "1.1.0", OWNER)
        .await
        .expect("replays the rename");
    assert_eq!(
        one(&tenant_pg, "SELECT heading FROM note WHERE id = 'n1'"),
        "first"
    );
    // The old column is kept, with its value, and still required, so
    // 1.0.0 still loads over this database.
    assert_eq!(
        one(&tenant_pg, "SELECT title FROM note WHERE id = 'n1'"),
        "first"
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = 'title'"
        ),
        "NO"
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = 'heading'"
        ),
        "NO"
    );
    // A row written under the new version names no `title`: it takes the
    // default, and the dual writes set it in the same write.
    write(
        &tenant,
        &overlay(&y, &y),
        "INSERT INTO note (\"$space\", id, rev, heading) VALUES ('acme', 'n2', 1, 'second')",
        "n2",
    )
    .await
    .expect("a row under 1.1.0");
    // A tenant still on 1.0.0 reads it whole.
    assert_eq!(
        one(&tenant_pg, "SELECT title FROM note WHERE id = 'n2'"),
        "second"
    );
}

/// **No write is lost between the copy and the pin switch.** A writer on
/// the old version inserts rows while the rename runs, then inserts and
/// changes rows after it and before any pin moves. Every row has the same
/// value under the new name. A row the new version writes is read whole by
/// the old one. Each write runs its version's dual writes in its own
/// transaction, as the host runs them (increment 11a); the database holds
/// no trigger. The reverse drops a trigger a build before 11a made, and
/// gives the old column back its default.
///
/// A regression looks like a row whose new column is empty or holds the
/// default where the old one holds a value: a write lost to the rename.
#[tokio::test(flavor = "multi_thread")]
async fn writes_during_the_rename_reach_the_new_column() {
    let (_reg_pg, registry) = empty("ordo_prev_live_reg").await;
    let (tenant_pg, tenant) = empty("ordo_prev_live_tenant").await;
    let x = store(&registry, &notes("1.0.0", "  kind: text = \"\"\n")).await;
    let y = store(
        &registry,
        &notes("1.1.0", "  mediaType: text = \"\" prev(kind)\n"),
    )
    .await;
    let (on_x, on_y) = (overlay(&x, &y), overlay(&y, &y));
    evolve::evolve(
        &registry,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect("stores");
    let (_, created) = sa::apply(&tenant, &x, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    for i in 0..50 {
        tenant_pg
            .exec(&format!(
                "INSERT INTO note (\"$space\", id, rev, kind) VALUES ('acme', 'before{i}', 1, 'k{i}')"
            ))
            .expect("a row before");
    }

    // The old version writes while the rename runs.
    let writer = {
        let pool = tenant.clone();
        let on_x = on_x.clone();
        tokio::spawn(async move {
            let mut n = 0usize;
            let mut i = 0usize;
            while i < 400 {
                let sql = format!(
                    "INSERT INTO note (\"$space\", id, rev, kind) VALUES ('acme', 'during{i}', 1, 'd{i}')"
                );
                if write(&pool, &on_x, &sql, &format!("during{i}"))
                    .await
                    .is_ok()
                {
                    n += 1;
                }
                i += 1;
            }
            n
        })
    };
    evolve::replay(&registry, &tenant, "acme", "1.1.0", OWNER)
        .await
        .expect("replays the rename");
    let written = writer.await.expect("the writer");
    assert_eq!(written, 400, "every write by the old version landed");

    // After the copy, before any pin moves: the old version writes again.
    write(
        &tenant,
        &on_x,
        "INSERT INTO note (\"$space\", id, rev, kind) VALUES ('acme', 'after', 1, 'image/png')",
        "after",
    )
    .await
    .expect("an insert after the copy");
    write(
        &tenant,
        &on_x,
        "UPDATE note SET kind = 'changed' WHERE id = 'before0'",
        "before0",
    )
    .await
    .expect("an update after the copy");

    assert_eq!(
        one(
            &tenant_pg,
            "SELECT count(*) FROM note WHERE \"mediaType\" IS DISTINCT FROM kind"
        ),
        "0",
        "every row has its value under the new name"
    );
    assert_eq!(
        one(&tenant_pg, "SELECT count(*) FROM note"),
        (50 + 400 + 1).to_string()
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT \"mediaType\" FROM note WHERE id = 'after'"
        ),
        "image/png"
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT \"mediaType\" FROM note WHERE id = 'before0'"
        ),
        "changed"
    );

    // The new version writes only the new column; the old one reads it.
    write(
        &tenant,
        &on_y,
        "INSERT INTO note (\"$space\", id, rev, \"mediaType\") VALUES ('acme', 'new1', 1, 'text/plain')",
        "new1",
    )
    .await
    .expect("an insert under 1.1.0");
    write(
        &tenant,
        &on_y,
        "UPDATE note SET \"mediaType\" = 'image/gif' WHERE id = 'after'",
        "after",
    )
    .await
    .expect("an update under 1.1.0");
    assert_eq!(
        one(&tenant_pg, "SELECT kind FROM note WHERE id = 'new1'"),
        "text/plain"
    );
    assert_eq!(
        one(&tenant_pg, "SELECT kind FROM note WHERE id = 'after'"),
        "image/gif"
    );

    // 1.0.0 still loads over the renamed database: nothing it requires was
    // relaxed, so applying it again (a rollback) is not refused.
    let state = sa::schema_state(&tenant, &x.schema).await.expect("a state");
    assert!(
        !matches!(state, sa::SchemaState::Behind(_)),
        "1.0.0 is behind: {state:?}"
    );

    // The dual writes are the host's: the database holds no trigger.
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal"
        ),
        "0"
    );

    // The reverse removes a trigger a build before 11a made, before it
    // copies back.
    let r = evolve::render(&registry, "1.0.0", "1.1.0", &Choices::default(), 1)
        .await
        .expect("renders");
    let lines = evolve::receipt_lines(&r.receipt).join("\n");
    let drop = lines
        .find("reverse: DROP TRIGGER IF EXISTS \"note$mediaType$prev\" ON \"note\";")
        .unwrap_or_else(|| panic!("{lines}"));
    let back = lines
        .find("reverse: UPDATE \"note\" SET \"kind\" = \"mediaType\";")
        .unwrap_or_else(|| panic!("{lines}"));
    assert!(drop < back, "{lines}");
    assert!(
        lines.contains("reverse: ALTER TABLE \"note\" ALTER COLUMN \"kind\" SET DEFAULT '';"),
        "{lines}"
    );
}

/// **Each plan check refuses what the step cannot make, naming the fix.**
#[tokio::test(flavor = "multi_thread")]
async fn the_plan_checks_refuse_with_the_fix_and_a_landed_mark_warns() {
    let (_pg, pool) = empty("ordo_prev_checks").await;
    store(
        &pool,
        &notes(
            "1.0.0",
            "  title: text\n  body: text?\n  secret: text sealed\n",
        ),
    )
    .await;
    let cases = [
        (
            "2.0.0",
            "  heading: text prev(nothing)\n  body: text?\n  secret: text sealed\n",
            "prev_unresolved",
        ),
        (
            "2.1.0",
            "  heading: int prev(title)\n  body: text?\n  secret: text sealed\n",
            "prev_type",
        ),
        (
            "2.2.0",
            "  heading: text prev(title, body)\n  secret: text sealed\n",
            "prev_mapping_required",
        ),
        (
            "2.3.0",
            "  heading: text prev(other.title)\n  body: text?\n  secret: text sealed\n",
            "prev_mapping_required",
        ),
        (
            "2.4.0",
            "  title: text\n  body: text?\n  hidden: text sealed prev(secret)\n",
            "prev_sealed",
        ),
        (
            "2.5.0",
            "  heading: text prev(title)\n  body: text?\n  secret: text sealed\n",
            "prev_default_required",
        ),
    ];
    for (v, fields, class) in cases {
        store(&pool, &notes(v, fields)).await;
        let errs = errors(evolve::render(&pool, "1.0.0", v, &Choices::default(), 1).await);
        let hit = errs.iter().find(|(c, _, _)| c == class);
        let (_, why, fix) = hit.unwrap_or_else(|| panic!("{v}: no {class} in {errs:?}"));
        assert!(!why.is_empty() && !fix.is_empty(), "{v}: {why} / {fix}");
    }
    // Nothing with an error is stored.
    assert_eq!(
        evolve::Plans::on(&pool).all().await.expect("plans").len(),
        0
    );

    // A mark whose rename the serving version already has: a warning only.
    store(
        &pool,
        &notes(
            "3.0.0",
            "  heading: text prev(title)\n  body: text?\n  secret: text sealed\n",
        ),
    )
    .await;
    store(
        &pool,
        &notes(
            "3.1.0",
            "  heading: text prev(title)\n  body: text\n  secret: text sealed\n",
        ),
    )
    .await;
    let r = evolve::render(&pool, "3.0.0", "3.1.0", &Choices::default(), 1).await;
    // 3.0.0 to 3.1.0 makes `body` required, which is refused on its own;
    // the stale mark is never the reason.
    if let Err(EvolveRefusal::Errors { errors, .. }) = &r {
        assert!(
            errors.iter().all(|e| !e.class.starts_with("prev_")),
            "{errors:?}"
        );
    }
    store(
        &pool,
        &notes(
            "3.2.0",
            "  heading: text prev(title)\n  body: text?\n  secret: text sealed\n  extra: text?\n",
        ),
    )
    .await;
    let r = evolve::render(&pool, "3.0.0", "3.2.0", &Choices::default(), 1)
        .await
        .expect("a stale mark renders");
    assert!(
        r.warnings.iter().any(|w| w.starts_with("prev_stale")),
        "{:?}",
        r.warnings
    );
}

/// **An entity rename renders as a rename**, and is refused until the
/// backfill worker can copy a table, naming the way round.
#[tokio::test(flavor = "multi_thread")]
async fn an_entity_rename_renders_as_one_and_is_refused_with_the_way_round() {
    let (_pg, pool) = empty("ordo_prev_entity").await;
    store(&pool, &notes("1.0.0", V1)).await;
    store(
        &pool,
        "domain notes @ 1.1.0\n\nentity memo (prev: note) {\n  title: text\n  body: text?\n}\n\n\
         op allNotes unbounded =\n  memo[body]\n",
    )
    .await;
    let errs = errors(evolve::render(&pool, "1.0.0", "1.1.0", &Choices::default(), 1).await);
    let (_, why, _) = errs
        .iter()
        .find(|(c, _, _)| c == "rename_entity_not_built")
        .unwrap_or_else(|| panic!("{errs:?}"));
    assert!(why.contains("add `memo` beside it"), "{why}");
    assert!(
        !errs.iter().any(|(c, _, _)| c == "not_class_a"),
        "no drop and add: {errs:?}"
    );
}

/// **The offer to remove a landed mark** names it in the chain's last
/// environment, once every tenant has the new name, and not before.
#[tokio::test(flavor = "multi_thread")]
async fn a_landed_mark_is_offered_only_in_the_last_environment() {
    let (_pg, pool) = empty("ordo_prev_offer").await;
    let y = store(&pool, &notes("1.1.0", V2)).await;
    // Production, no tenant behind: the mark has landed.
    let landed = ordo_host::prev_offer::landed(&pool, &y).await;
    let texts: Vec<String> = landed.iter().map(|m| m.text()).collect();
    assert_eq!(texts, ["note.heading prev(title)"]);

    // The same deployment as dev, with production after it: not yet.
    let envs = ordo_host::environments::Environments::on(&pool);
    envs.register("dev", Some(1), OWNER, "the dev step", 1)
        .await
        .expect("dev registered");
    envs.set_own("dev", "this deployment is dev")
        .await
        .expect("own set");
    assert!(ordo_host::prev_offer::landed(&pool, &y).await.is_empty());
}

/// One node: its own pool to the tenant's database and an executor over the
/// version it serves. It holds no rename state of its own.
struct Node {
    exec: ordo_exec::Executor,
    fixture: std::sync::Arc<ordo_conformance::Fixture>,
    log: std::sync::Arc<std::sync::Mutex<ordo_conformance::WriteLog>>,
}

async fn node(db: &str, source: &str, name: &str) -> Node {
    let f = ordo_conformance::Fixture::from_source(source, "").expect("compiles");
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
            space: ordo_conformance::fixture_space(),
        },
        fixture: std::sync::Arc::new(f),
        // Ids minted under the node's own prefix, as a serving node does.
        log: std::sync::Arc::new(std::sync::Mutex::new(ordo_conformance::WriteLog::serving(
            format!("{name}_"),
            std::sync::Arc::new(|| 1),
        ))),
    }
}

impl Node {
    async fn write(&self, call: String) -> Result<(), String> {
        self.exec
            .write_as(self.fixture.clone(), self.log.clone(), call, "p_ada".into())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// **Two nodes, and one never reads anything: no write is lost.** Node A
/// and node B serve 1.0.0 through the real write path. Each has its own
/// pool and holds nothing about the rename: no node is told of it, and
/// node B is never told at all, the way a node whose pin stream is slow
/// would be. Both write without pause while `ordo apply` makes the rename
/// (`kind` to `mediaType`), and node B keeps writing after it. A third
/// node serves 1.1.0. Every row holds the same value under both names.
///
/// Before increment 11a's `$renames`, a node learned of a rename from the
/// registry on its pin stream's next read, and a node that had not read it
/// wrote only the old column. Now every write reads the renames from its
/// own database, first, under a share lock the rename's step waits on.
///
/// A regression looks like a row whose `mediaType` is empty or differs
/// from its `kind`.
#[tokio::test(flavor = "multi_thread")]
async fn two_nodes_one_never_told_lose_no_write_across_a_rename() {
    let (_reg_pg, _registry) = empty("ordo_prev_nodes_reg").await;
    let (tenant_pg, tenant) = empty("ordo_prev_nodes_tenant").await;
    let ops_x = "\nop addNote($kind: text) =\n  set note { kind: $kind }\n";
    let ops_y = "\nop addNote($kind: text) =\n  set note { mediaType: $kind }\n";
    let x_src = notes("1.0.0", "  kind: text = \"\"\n") + ops_x;
    let y_src = notes("1.1.0", "  mediaType: text = \"\" prev(kind)\n") + ops_y;
    let x = Program::compile(&x_src).expect("1.0.0");
    let y = Program::compile(&y_src).expect("1.1.0");
    let (_, created) = sa::apply(&tenant, &x, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    let a = std::sync::Arc::new(node(&tenant_pg.db, &x_src, "a").await);
    let b = std::sync::Arc::new(node(&tenant_pg.db, &x_src, "b").await);
    for i in 0..20 {
        a.write(format!(r#"addNote($kind: "a-before-{i}")"#))
            .await
            .expect("a write before");
    }

    let writer = |n: std::sync::Arc<Node>, tag: &'static str| {
        tokio::spawn(async move {
            let mut ok = 0usize;
            for i in 0..150 {
                match n.write(format!(r#"addNote($kind: "{tag}-{i}")"#)).await {
                    Ok(()) => ok += 1,
                    Err(e) => eprintln!("node {tag}, write {i}: {e}"),
                }
            }
            ok
        })
    };
    let (wa, wb) = (writer(a.clone(), "a"), writer(b.clone(), "b"));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    // The rename as `ordo apply` makes it on a tenant's database: planned
    // against 1.0.0, the version it serves.
    let mut plan = sa::plan_with(&tenant, &y, &Choices::default(), Some(&x.schema))
        .await
        .expect("a plan");
    plan.prior = Some(x.schema.clone());
    let renamed = sa::apply_planned(&tenant, &plan, "test", "test").await;
    assert!(
        matches!(renamed, Ok(sa::Applied::Additive { .. })),
        "the rename applies: {renamed:?}"
    );
    assert_eq!(
        one(&tenant_pg, "SELECT count(*) FROM \"$renames\""),
        "1",
        "the rename is recorded where it was made"
    );
    assert_eq!(wa.await.unwrap(), 150, "every write of node A landed");
    assert_eq!(wb.await.unwrap(), 150, "every write of node B landed");

    // Node B, never told, writes again; a node on 1.1.0 writes too.
    for i in 0..20 {
        b.write(format!(r#"addNote($kind: "b-after-{i}")"#))
            .await
            .expect("a write after");
    }
    let on_y = node(&tenant_pg.db, &y_src, "y").await;
    on_y.write(r#"addNote($kind: "y-0")"#.into())
        .await
        .expect("a write on 1.1.0");

    assert_eq!(
        one(&tenant_pg, "SELECT count(*) FROM note"),
        (20 + 150 + 150 + 20 + 1).to_string()
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT count(*) FROM note WHERE \"mediaType\" IS DISTINCT FROM kind"
        ),
        "0",
        "every row holds its value under both names"
    );
    assert_eq!(
        one(
            &tenant_pg,
            "SELECT count(*) FROM note WHERE kind LIKE 'b-%' AND \"mediaType\" = kind"
        ),
        "170",
        "every write of node B reached the new column"
    );
}
