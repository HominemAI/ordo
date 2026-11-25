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
//! - an entity rename renders as a rename with its reverse, keeps both
//!   tables current under live writes (`$renames`), and lands when a
//!   version drops the mark or a move's soak closes; a sealed, reshaped or
//!   `acl` entity is refused, naming the fix;
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

/// Notes and the pins that reference them, at `version`. With `memo`, the
/// note entity is `memo`, declared `(prev: note)` when `mark` is set.
fn pinned(version: &str, entity: &str, mark: bool) -> String {
    let header = if mark {
        format!("{entity} (prev: note)")
    } else {
        entity.to_string()
    };
    format!(
        "domain notes @ {version}\n\n\
         entity {header} {{\n  kind: text = \"\"\n}}\n\n\
         entity pin {{\n  note -> {entity}\n  label: text = \"\"\n}}\n\n\
         op addNote($kind: text) =\n  set {entity} {{ kind: $kind }}\n\n\
         op addPin($note: id, $label: text) =\n  set pin {{ note: $note, label: $label }}\n\n\
         op dropPin($id: id) =\n  del pin {{ id: $id }}\n\n\
         op dropNote($id: id) =\n  del {entity} {{ id: $id }}\n\n\
         op allNotes unbounded =\n  {entity}[kind]\n"
    )
}

/// Rows of `note` and `memo` that differ, by `($space, id)`, counted.
const TABLES_DIFFER: &str = "SELECT count(*) FROM note n FULL JOIN memo m \
     ON n.\"$space\" = m.\"$space\" AND n.id = m.id \
     WHERE n.id IS NULL OR m.id IS NULL OR n.kind IS DISTINCT FROM m.kind \
     OR n.rev IS DISTINCT FROM m.rev";

/// The table a reference constraint points at.
fn references(pg: &Pg, constraint: &str) -> String {
    one(
        pg,
        &format!(
            "SELECT confrelid::regclass::text FROM pg_constraint WHERE conname = '{constraint}'"
        ),
    )
}

/// **An entity rename renders as a rename** (B4), never a drop and an
/// add. The receipt names it, and its reverse drops the new table and
/// leaves `$renames`.
#[tokio::test(flavor = "multi_thread")]
async fn an_entity_rename_renders_as_one_with_its_reverse() {
    let (_pg, pool) = empty("ordo_prev_entity").await;
    store(&pool, &pinned("1.0.0", "note", false)).await;
    store(&pool, &pinned("1.1.0", "memo", true)).await;
    let r = evolve::render(&pool, "1.0.0", "1.1.0", &Choices::default(), 1)
        .await
        .expect("an entity rename renders");
    assert_eq!(
        r.receipt["entityRenames"],
        serde_json::json!([{ "entity": "memo", "from": "note" }])
    );
    let change = r.receipt["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .find(|c| c["code"] == "B4")
        .unwrap_or_else(|| panic!("{}", r.receipt["changes"]));
    let reverse = change["reverse"].to_string();
    assert!(reverse.contains("DROP TABLE \\\"memo\\\""), "{reverse}");
    assert!(reverse.contains("$renames"), "{reverse}");
    assert!(reverse.contains("NOT DEFERRABLE"), "{reverse}");
    let codes: Vec<&str> = r.receipt["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter_map(|c| c["code"].as_str())
        .collect();
    assert!(
        !codes.contains(&"A1") && !codes.contains(&"D2"),
        "no add and drop: {codes:?}"
    );
}

/// **An entity rename the step cannot make is refused, naming the fix**:
/// a sealed field (its ciphertext is bound to the entity's name), new
/// fields in the same step, and an entity with `acl` (its shares live in an
/// entity named after it).
#[tokio::test(flavor = "multi_thread")]
async fn an_entity_rename_is_refused_when_sealed_reshaped_or_under_acl() {
    let (_pg, pool) = empty("ordo_prev_entity_refused").await;
    let src = |version: &str, header: &str, fields: &str, extra: &str| {
        format!(
            "domain notes @ {version}\n\nentity {header} {{\n{fields}{extra}}}\n\n\
             op allNotes unbounded =\n  {}[title]\n",
            header.split(' ').next().unwrap_or(header)
        )
    };
    let cases = [
        (
            "  title: text\n  secret: text sealed\n",
            "",
            "rename_entity_sealed",
            "`memo.secret` is sealed",
        ),
        (
            "  title: text\n",
            "  extra: text?\n",
            "rename_entity_reshaped",
            "`extra`",
        ),
        (
            "  title: text\n  acl (read, write, admin)\n",
            "",
            "rename_entity_acl",
            "`noteAccess`",
        ),
    ];
    for (i, (fields, extra, class, says)) in cases.into_iter().enumerate() {
        let x = format!("1.{i}.0");
        let y = format!("1.{i}.1");
        store(&pool, &src(&x, "note", fields, "")).await;
        store(&pool, &src(&y, "memo (prev: note)", fields, extra)).await;
        let errs = errors(evolve::render(&pool, &x, &y, &Choices::default(), 1).await);
        let (_, why, fix) = errs
            .iter()
            .find(|(c, _, _)| c == class)
            .unwrap_or_else(|| panic!("{class}: {errs:?}"));
        assert!(why.contains(says), "{class}: {why}");
        assert!(
            fix.contains("note"),
            "{class}: the fix names the old name: {fix}"
        );
        assert!(
            !errs.iter().any(|(c, _, _)| c == "not_class_a"),
            "no drop and add: {errs:?}"
        );
    }
}

/// What a node's load gate finds different between `source` and the
/// database (`Pool::live_drift`).
async fn drift(pg: &Pg, source: &str) -> Vec<String> {
    let p = Program::compile(source).expect("compiles");
    let f = ordo_conformance::Fixture::from_source(source, "").expect("compiles");
    let cfg = PoolConfig {
        db: pg.db.clone(),
        ..PoolConfig::from_env()
    };
    ordo_host::shape::verify_registry(&f, &cfg, &p.ddl)
        .await
        .expect("the gate runs")
        .drift
        .iter()
        .map(|d| format!("{d:?}"))
        .collect()
}

/// A deployment's own database, applied and activated by `ordo apply`.
async fn apply_source(pool: &Pool, source: &str) {
    let program = Program::compile(source).expect("compiles");
    ordo_host::apply_op::run_program(
        pool,
        &ordo_exec::route::SinglePool::new(pool.clone()),
        &program,
        vec![],
        OWNER,
        "t",
        true,
    )
    .await
    .expect("applies");
}

/// **An entity rename, refused by `ordo apply` and run as a stored step
/// (`ordo evolve --run`), under live writes on two nodes, loses no write,
/// and lands when a version drops the mark.** 1.0.0 has `note` and `pin`,
/// whose `note` references it. `ordo apply` of 1.1.0, `memo (prev: note)`,
/// is refused naming `ordo evolve`: it would copy in its step and hold
/// every write. Node A and node B serve 1.0.0 through the real write path
/// and write without pause while the stored step's worker copies the
/// table; node B is never told. Every row is
/// in both tables. A node on 1.1.0 writes a memo and pins it, and deletes
/// another: both tables follow. 1.2.0 drops the mark: the rename lands.
/// `$renames` is empty, `note` is an orphan, `pin`'s reference points at
/// `memo`, and `note` holds no reference of its own, so 1.2.0 deletes
/// freely and writes nothing more to `note`.
///
/// A regression looks like a row in one table and not the other, a pin
/// refused because its memo is not yet in `note`, or a reference still
/// pointing at the orphan after the rename lands.
#[tokio::test(flavor = "multi_thread")]
async fn an_entity_rename_run_as_a_stored_step_keeps_both_tables_current_and_lands_when_the_mark_goes(
) {
    let (pg, pool) = empty("ordo_prev_entity_apply").await;
    let (x_src, y_src, z_src) = (
        pinned("1.0.0", "note", false),
        pinned("1.1.0", "memo", true),
        pinned("1.2.0", "memo", false),
    );
    apply_source(&pool, &x_src).await;
    let a = std::sync::Arc::new(node(&pg.db, &x_src, "a").await);
    let b = std::sync::Arc::new(node(&pg.db, &x_src, "b").await);
    for i in 0..20 {
        a.write(format!(r#"addNote($kind: "a-before-{i}")"#))
            .await
            .expect("a write before");
    }
    let first = one(&pg, "SELECT id FROM note WHERE kind = 'a-before-0'");
    a.write(format!(r#"addPin($note: "{first}", $label: "kept")"#))
        .await
        .expect("a pin before");

    let writer = |n: std::sync::Arc<Node>, tag: &'static str| {
        tokio::spawn(async move {
            let mut ok = 0usize;
            for i in 0..120 {
                match n.write(format!(r#"addNote($kind: "{tag}-{i}")"#)).await {
                    Ok(()) => ok += 1,
                    Err(e) => eprintln!("node {tag}, write {i}: {e}"),
                }
            }
            ok
        })
    };
    // `ordo apply` refuses the rename, naming the stored step.
    let x = Program::compile(&x_src).expect("1.0.0");
    let y = Program::compile(&y_src).expect("1.1.0");
    let mut plan = sa::plan_with(&pool, &y, &Choices::default(), Some(&x.schema))
        .await
        .expect("a plan");
    plan.prior = Some(x.schema.clone());
    match sa::apply_planned(&pool, &plan, "test", "test").await {
        Err(sa::ApplyRefusal::NotApplied(t)) => {
            let r = t
                .refused
                .iter()
                .find(|r| r.class == "rename_entity_needs_stored_plan")
                .unwrap_or_else(|| panic!("{:?}", t.refused));
            assert!(r.why.contains("ordo evolve"), "{}", r.why);
        }
        other => panic!("the apply is refused: {other:?}"),
    }
    assert_eq!(one(&pg, "SELECT to_regclass('memo') IS NULL"), "t");
    store(&pool, &y_src).await;
    let step = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect("the step stores")
        .plan;

    let (wa, wb) = (writer(a.clone(), "a"), writer(b.clone(), "b"));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let ran = evolve::run_step(&pool, &pool, "deployment", &step, OWNER).await;
    assert!(ran.is_ok(), "the step runs: {ran:?}");
    Versions::on(&pool)
        .activate("1.1.0", OWNER, ordo_commandlog::Timestamp(5))
        .await
        .expect("1.1.0 is active");
    assert_eq!(
        one(&pg, "SELECT count(*) FROM \"$renames\" WHERE field = ''"),
        "1",
        "the rename is recorded where it was made"
    );
    assert_eq!(wa.await.unwrap(), 120, "every write of node A landed");
    assert_eq!(wb.await.unwrap(), 120, "every write of node B landed");
    for i in 0..10 {
        b.write(format!(r#"addNote($kind: "b-after-{i}")"#))
            .await
            .expect("node B, never told, writes after");
    }
    assert_eq!(one(&pg, "SELECT count(*) FROM memo"), "270");
    assert_eq!(one(&pg, TABLES_DIFFER), "0", "every row is in both tables");

    // 1.1.0 writes a memo and pins it; the pin's reference into `note` is
    // checked at commit, after the dual write put the memo there too.
    let on_y = node(&pg.db, &y_src, "y").await;
    on_y.write(r#"addNote($kind: "y-0")"#.into())
        .await
        .expect("a write on 1.1.0");
    let y0 = one(&pg, "SELECT id FROM memo WHERE kind = 'y-0'");
    on_y.write(format!(r#"addPin($note: "{y0}", $label: "new")"#))
        .await
        .expect("a pin on a memo 1.1.0 wrote");
    let gone = one(&pg, "SELECT id FROM memo WHERE kind = 'a-before-5'");
    on_y.write(format!(r#"dropNote($id: "{gone}")"#))
        .await
        .expect("a delete on 1.1.0");
    assert_eq!(one(&pg, TABLES_DIFFER), "0", "both tables follow 1.1.0");
    assert_eq!(
        one(
            &pg,
            &format!("SELECT count(*) FROM note WHERE id = '{gone}'")
        ),
        "0"
    );
    assert_eq!(
        references(&pg, "pin$note$fk"),
        "note",
        "kept until it lands"
    );
    // Both versions' load gates pass over the database mid-rename: the
    // reference into `note`, checked at commit, is the same reference to
    // each of them.
    for src in [&x_src, &y_src] {
        assert_eq!(drift(&pg, src).await, Vec::<String>::new(), "{src}");
    }

    // 1.2.0 drops the mark: the rename lands.
    apply_source(&pool, &z_src).await;
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$renames\""), "0");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM \"$orphans\" WHERE entity = 'note' AND field = ''"
        ),
        "1",
        "the old table is an orphan"
    );
    assert_eq!(references(&pg, "pin$note$fk"), "memo");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_constraint WHERE contype = 'f' \
             AND (conrelid = 'note'::regclass OR confrelid = 'note'::regclass)"
        ),
        "0",
        "no reference holds the orphan"
    );
    assert_eq!(drift(&pg, &z_src).await, Vec::<String>::new());
    let on_z = node(&pg.db, &z_src, "z").await;
    on_z.write(r#"addNote($kind: "z-0")"#.into())
        .await
        .expect("a write on 1.2.0");
    let z0 = one(&pg, "SELECT id FROM memo WHERE kind = 'z-0'");
    on_z.write(format!(r#"addPin($note: "{z0}", $label: "z")"#))
        .await
        .expect("a pin on 1.2.0");
    let pin = one(&pg, &format!("SELECT id FROM pin WHERE note = '{first}'"));
    on_z.write(format!(r#"dropPin($id: "{pin}")"#))
        .await
        .expect("the old pin goes");
    on_z.write(format!(r#"dropNote($id: "{first}")"#))
        .await
        .expect("a memo the orphan still holds is deleted freely");
    assert_eq!(
        one(&pg, "SELECT count(*) FROM note WHERE kind = 'z-0'"),
        "0",
        "1.2.0 writes nothing more to the old table"
    );
}

/// Increment 11b with an entity rename (`plan-versions.md` §2.2, §2.5;
/// Q1): **tenants on the deployment's own database move through an entity
/// rename with dual writes, a revert in the soak keeps every write, and
/// the soaks close together and land the rename.** `acme` and `beta` keep
/// their rows in the deployment's own database. 1.1.0 renames `note` to
/// `memo`; `pin` references it. `acme` moves: the expand makes `memo` and
/// copies every space's rows once, and its pin switches. `beta` on 1.0.0
/// and `acme` on 1.1.0 write through the real write path: both tables hold
/// every row. `acme` is reverted in the soak and writes as 1.0.0: nothing
/// is lost. It moves again, then `beta` moves with no second expand. Once
/// the own space moves too, the soaks close together: `$renames` is empty,
/// `note` is an orphan, `pin`'s reference points at `memo`.
///
/// A regression looks like a row in one table and not the other, a second
/// copy, or the rename landing while a space there still writes `note`.
#[tokio::test(flavor = "multi_thread")]
async fn an_entity_rename_moves_tenants_on_the_deployments_own_database_and_lands_at_the_close() {
    use ordo_host::dual_writes::{self, MoveContext};
    use ordo_host::versions::MovePhase;
    let (pg, pool) = empty("ordo_prev_entity_shared").await;
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
    let (x_src, y_src) = (
        pinned("1.0.0", "note", false),
        pinned("1.1.0", "memo", true),
    );
    apply_source(&pool, &x_src).await;
    store(&pool, &y_src).await;
    let plan = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect("the step stores")
        .plan;
    for s in ["acme", "beta", "home"] {
        pg.exec(&format!(
            "INSERT INTO note (\"$space\", id, rev, kind) \
             SELECT '{s}', '{s}' || i, 1, '{s} ' || i FROM generate_series(1, 30) i; \
             INSERT INTO pin (\"$space\", id, rev, note) \
             SELECT '{s}', '{s}-pin' || i, 1, '{s}' || i FROM generate_series(1, 5) i"
        ))
        .expect("rows");
    }
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let ctx = step_context();
    let week = 7 * 86_400;
    let v = Versions::on(&pool);

    // `acme` moves: one expand and one copy for the whole database.
    let moved = dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(one(&pg, "SELECT count(*) FROM memo"), "90");
    assert_eq!(
        one(&pg, TABLES_DIFFER),
        "0",
        "every space's rows are copied"
    );
    for src in [&x_src, &y_src] {
        assert_eq!(drift(&pg, src).await, Vec::<String>::new(), "{src}");
    }

    // `beta` on 1.0.0 and `acme` soaking on 1.1.0 write through the real
    // path.
    let on_x = node_in(&pg.db, &x_src, "b", "beta").await;
    let on_y = soaking_node(&pool, &pg.db, &y_src, "acme").await;
    for i in 0..10 {
        on_x.write(format!(r#"addNote($kind: "beta wrote {i}")"#))
            .await
            .expect("beta writes as 1.0.0");
        on_y.write(format!(r#"addNote($kind: "acme wrote {i}")"#))
            .await
            .expect("acme writes as 1.1.0");
    }
    on_y.write(r#"dropNote($id: "acme30")"#.into())
        .await
        .expect("acme deletes as 1.1.0");
    assert_eq!(
        one(&pg, TABLES_DIFFER),
        "0",
        "each write reached both tables"
    );

    // A revert in the soak: `acme` is back on 1.0.0 with every write.
    dual_writes::revert(&cx, "acme", OWNER)
        .await
        .expect("acme reverts");
    assert_eq!(
        v.pin_of("acme")
            .await
            .expect("read")
            .and_then(|p| p.version),
        Some("1.0.0".to_string())
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM note WHERE kind LIKE 'acme wrote %'"
        ),
        "10",
        "what acme wrote on 1.1.0 is there on 1.0.0"
    );
    let back = node_in(&pg.db, &x_src, "c", "acme").await;
    back.write(r#"addNote($kind: "acme back")"#.into())
        .await
        .expect("acme writes as 1.0.0 again");
    assert_eq!(one(&pg, TABLES_DIFFER), "0");

    // `acme` moves again, then `beta`: their expand and copy are done.
    let moved = dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect("acme moves again");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    let moved = dual_writes::run(&cx, "beta", &plan, &ctx, week, OWNER)
        .await
        .expect("beta moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(moved.statements, 0, "no second expand");

    // The own space is on 1.0.0: no close yet.
    let err = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect_err("the own space is on 1.0.0");
    assert_eq!(err.class(), "soak_shared", "{err}");
    assert_eq!(references(&pg, "pin$note$fk"), "note");

    v.pin_own("home", "1.1.0", 0, OWNER, 20)
        .await
        .expect("the own space moves");
    pg.exec("UPDATE \"$moves\" SET soak_until = 1")
        .expect("both due");
    let mut closed = dual_writes::close_due(&cx).await;
    closed.sort();
    assert_eq!(closed, vec!["acme".to_string(), "beta".to_string()]);

    // Landed: `$renames`, the orphan, the reference, the record.
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$renames\""), "0");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM \"$orphans\" WHERE entity = 'note' AND field = ''"
        ),
        "1"
    );
    assert_eq!(references(&pg, "pin$note$fk"), "memo");
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$moves\""), "0");
    assert_eq!(
        ordo_exec::spacedb::recorded_version(&pool)
            .await
            .expect("recorded"),
        ordo_exec::spacedb::TargetVersion::At("1.1.0".into())
    );
    let after = node_in(&pg.db, &y_src, "d", "acme").await;
    after
        .write(r#"addNote($kind: "after the close")"#.into())
        .await
        .expect("a write after the close");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM note WHERE kind = 'after the close'"
        ),
        "0",
        "the orphan is not written"
    );
}

/// The deployment's own database, and one database per tenant.
struct PerTenant {
    control: Pool,
    tenants: Vec<(String, Pool)>,
}

impl ordo_exec::route::PoolResolver for PerTenant {
    fn resolve<'a>(&'a self, space: &'a ordo_crypto::SpaceId) -> ordo_exec::route::Resolving<'a> {
        Box::pin(async move {
            Ok(self
                .tenants
                .iter()
                .find(|(n, _)| *n == space.0)
                .map(|(_, p)| p.clone())
                .unwrap_or_else(|| self.control.clone()))
        })
    }

    fn targets<'a>(&'a self) -> ordo_exec::route::Targets<'a> {
        Box::pin(async move {
            let mut out = vec![ordo_exec::route::Target {
                label: "default".into(),
                pool: Ok(self.control.clone()),
            }];
            out.extend(self.tenants.iter().map(|(n, p)| ordo_exec::route::Target {
                label: n.clone(),
                pool: Ok(p.clone()),
            }));
            Ok(out)
        })
    }
}

/// Increment 11b with an entity rename, **a tenant with a database of its
/// own**, as a rollout moves it (`every_tenant::forward_tenant`): the step
/// is a move with dual writes. `ordo rollback` in the soak puts it back
/// with every write (`revert_soaks_from`). Moved again, its soak closes
/// alone and lands the rename in its own database. The rollback's dry run
/// then says what moving back would lose: `memo`.
///
/// A regression looks like the step replayed with no soak, a write on
/// 1.1.0 missing after the rollback, or a dry run that promises every
/// write after the rename landed.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_with_its_own_database_moves_through_an_entity_rename_and_rolls_back_in_the_soak()
{
    use ordo_host::dual_writes::MoveContext;
    use ordo_host::every_tenant::{self, Forward};
    let (_cpg, control) = empty("ordo_prev_entity_ctl").await;
    let (tpg, solo) = empty("ordo_prev_entity_solo").await;
    ordo_host::tenants::ensure_table(&control)
        .await
        .expect("$spaces");
    assert!(
        ordo_host::tenants::reserve(&control, "solo", "solo", "dev:ada", 10, None)
            .await
            .expect("reserved")
    );
    ordo_host::tenants::activate(&control, "solo", 11)
        .await
        .expect("active");
    let (x_src, y_src) = (
        pinned("1.0.0", "note", false),
        pinned("1.1.0", "memo", true),
    );
    apply_source(&control, &x_src).await;
    let x = Program::compile(&x_src).expect("1.0.0");
    let (_, created) = sa::apply(&solo, &x, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    tpg.exec(
        "INSERT INTO note (\"$space\", id, rev, kind) \
         SELECT 'solo', 'solo' || i, 1, 'solo ' || i FROM generate_series(1, 40) i",
    )
    .expect("rows");
    store(&control, &y_src).await;
    evolve::evolve(
        &control,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect("the step stores");
    let pools = PerTenant {
        control: control.clone(),
        tenants: vec![("solo".into(), solo.clone())],
    };
    let cx = MoveContext {
        control: &control,
        pools: &pools,
        spaces: None,
    };
    let ctx = step_context();
    let week = 7 * 86_400;
    let v = Versions::on(&control);

    let moved = every_tenant::forward_tenant(&cx, &ctx, "solo", "1.1.0", week, OWNER)
        .await
        .expect("solo moves");
    assert!(
        matches!(&moved, Forward::Switched(m) if !m.closed),
        "a move with a soak"
    );
    assert_eq!(one(&tpg, TABLES_DIFFER), "0", "every row is copied");
    let on_y = soaking_node(&control, &tpg.db, &y_src, "solo").await;
    for i in 0..5 {
        on_y.write(format!(r#"addNote($kind: "on 1.1.0 {i}")"#))
            .await
            .expect("solo writes as 1.1.0");
    }

    // `ordo rollback` in the soak: back on 1.0.0, every write kept.
    let back = every_tenant::revert_soaks_from(&control, Some(&pools), None, "1.0.0", OWNER)
        .await
        .expect("rolled back");
    assert_eq!(back, vec!["solo".to_string()]);
    assert_eq!(
        v.pin_of("solo")
            .await
            .expect("read")
            .and_then(|p| p.version),
        Some("1.0.0".to_string())
    );
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM note WHERE kind LIKE 'on 1.1.0 %'"
        ),
        "5"
    );

    // Moved again, and its soak closes alone: the rename lands there.
    every_tenant::forward_tenant(&cx, &ctx, "solo", "1.1.0", week, OWNER)
        .await
        .expect("solo moves again");
    let closed = ordo_host::dual_writes::close(&cx, "solo", OWNER)
        .await
        .expect("the soak closes");
    assert!(closed.closed);
    assert!(closed.orphaned.contains(&"note".to_string()), "{closed:?}");
    assert_eq!(one(&tpg, "SELECT count(*) FROM \"$renames\""), "0");
    assert_eq!(
        v.pin_of("solo")
            .await
            .expect("read")
            .and_then(|p| p.version),
        Some("1.1.0".to_string())
    );
    v.activate("1.1.0", OWNER, ordo_commandlog::Timestamp(30))
        .await
        .expect("1.1.0 is active");
    let dry = every_tenant::rollback_tenants(&control, "1.0.0")
        .await
        .expect("a dry run");
    let solo_line = dry
        .iter()
        .find(|r| r["tenant"] == "solo")
        .unwrap_or_else(|| panic!("{dry:?}"));
    assert_eq!(solo_line["kept"], false, "{solo_line}");
    assert!(
        solo_line["note"].as_str().unwrap_or("").contains("memo"),
        "{solo_line}"
    );
}

/// **A large entity rename copies in chunks while writes keep landing**
/// (the backfill worker, 10b, with `$renames`' dual writes). `solo` has a
/// database of its own holding 20,000 notes. The stored step's move copies
/// `note` into `memo` 500 rows a turn (`backfill.chunk_rows`, a budget of
/// 1). Between turns, and while each turn runs, a node on 1.0.0 adds notes
/// and deletes old ones, both ahead of the cursor and behind it. Every
/// write lands while the copy is still `backfilling`, none waits for the
/// whole copy, and once verify passes and the pin switches, every row is in
/// both tables with the same columns, with no deleted row copied back.
///
/// A regression looks like a write refused or timed out during the copy (a
/// copy held under the `$renames` lock), a row in one table only, or a
/// deleted row back in `memo`.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_entity_rename_copies_in_chunks_while_writes_keep_landing() {
    use ordo_host::dual_writes::{self, MoveContext};
    use ordo_host::versions::MovePhase;
    let (_cpg, control) = empty("ordo_prev_entity_big_ctl").await;
    let (tpg, solo) = empty("ordo_prev_entity_big").await;
    ordo_host::tenants::ensure_table(&control)
        .await
        .expect("$spaces");
    assert!(
        ordo_host::tenants::reserve(&control, "solo", "solo", "dev:ada", 10, None)
            .await
            .expect("reserved")
    );
    ordo_host::tenants::activate(&control, "solo", 11)
        .await
        .expect("active");
    let (x_src, y_src) = (
        pinned("1.0.0", "note", false),
        pinned("1.1.0", "memo", true),
    );
    apply_source(&control, &x_src).await;
    let x = Program::compile(&x_src).expect("1.0.0");
    let (_, created) = sa::apply(&solo, &x, "test", "test").await;
    assert!(
        matches!(created, Ok(sa::Applied::Created(_))),
        "{created:?}"
    );
    tpg.exec(
        "INSERT INTO note (\"$space\", id, rev, kind) \
         SELECT 'solo', 'solo' || lpad(i::text, 5, '0'), 1, 'solo ' || i \
         FROM generate_series(1, 20000) i",
    )
    .expect("20,000 rows");
    store(&control, &y_src).await;
    let plan = evolve::evolve(
        &control,
        "1.0.0",
        "1.1.0",
        &Choices::default(),
        None,
        OWNER,
        1,
    )
    .await
    .expect("the step stores")
    .plan;
    assert!(
        plan.receipt.contains("copyFrom"),
        "the copy is the worker's: {}",
        plan.receipt
    );
    let pools = PerTenant {
        control: control.clone(),
        tenants: vec![("solo".into(), solo.clone())],
    };
    let cx = MoveContext {
        control: &control,
        pools: &pools,
        spaces: None,
    };
    let ctx = evolve::StepContext {
        settings: ordo_host::backfill::BackfillSettings {
            chunk_rows: 500,
            budget: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let week = 7 * 86_400;

    // The first turn: the expand and one chunk. Writes go on.
    let first = dual_writes::run(&cx, "solo", &plan, &ctx, week, OWNER)
        .await
        .expect("the move starts");
    assert_eq!(first.phase, MovePhase::Backfilling, "{first:?}");
    let copied = one(&tpg, "SELECT count(*) FROM memo");
    assert_eq!(copied, "500", "one chunk, not the table");

    let writer = std::sync::Arc::new(node_in(&tpg.db, &x_src, "w", "solo").await);
    let w = writer.clone();
    let writes = tokio::spawn(async move {
        let mut bad = Vec::new();
        let mut slowest = std::time::Duration::ZERO;
        for i in 0..150 {
            let started = std::time::Instant::now();
            if let Err(e) = w.write(format!(r#"addNote($kind: "live {i}")"#)).await {
                bad.push(format!("add {i}: {e}"));
            }
            if i % 5 == 0 {
                // One ahead of the cursor, one behind it.
                for id in [format!("solo{:05}", 20000 - i), format!("solo{:05}", i + 1)] {
                    if let Err(e) = w.write(format!(r#"dropNote($id: "{id}")"#)).await {
                        bad.push(format!("drop {id}: {e}"));
                    }
                }
            }
            slowest = slowest.max(started.elapsed());
        }
        (bad, slowest)
    });

    // The worker's turns, beside the writes.
    let mut turns = 1;
    let mut during = 0;
    loop {
        let m = dual_writes::run(&cx, "solo", &plan, &ctx, week, OWNER)
            .await
            .expect("a turn");
        turns += 1;
        if m.phase != MovePhase::Backfilling {
            assert_eq!(m.phase, MovePhase::Soaking, "{m:?}");
            break;
        }
        if !writes.is_finished() {
            during += 1;
        }
        assert!(turns < 200, "the copy ends");
    }
    let (bad, slowest) = writes.await.unwrap();
    assert!(bad.is_empty(), "every write landed: {bad:?}");
    assert!(during > 0, "the writes ran while the copy was backfilling");
    assert!(
        slowest < std::time::Duration::from_secs(5),
        "no write waited for the copy: {slowest:?}"
    );
    assert!(turns >= 40, "the copy ran in chunks: {turns} turns");
    // Writes the node makes after the switch, on 1.0.0 still: kept too.
    writer
        .write(r#"addNote($kind: "after the switch")"#.into())
        .await
        .expect("a write after the copy");
    assert_eq!(
        one(&tpg, "SELECT count(*) FROM note"),
        (20000 + 150 + 1 - 60).to_string()
    );
    assert_eq!(one(&tpg, TABLES_DIFFER), "0", "every row is in both tables");
    assert_eq!(
        one(
            &tpg,
            "SELECT count(*) FROM memo WHERE id IN ('solo20000', 'solo00001')"
        ),
        "0",
        "no deleted row is copied back"
    );
}

/// **The path `ordo apply` points to copies a large table in chunks while
/// writes keep landing** (`ordo evolve --run` on the deployment's own
/// database). It holds 20,000 notes. `ordo apply` of the rename is refused
/// (above); the stored step runs instead, 500 rows a run (`--chunk-rows
/// 500 --budget 1`). A node on 1.0.0 adds and deletes notes between and
/// during the runs. Every write lands while the copy is still
/// backfilling, none waits for the whole copy, and once verify passes the
/// database records 1.1.0 with every row in both tables.
///
/// A regression looks like a write refused or timed out during the copy, a
/// row in one table only, or a deleted row back in `memo`.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_entity_rename_run_on_the_deployments_own_database_copies_in_chunks_under_writes() {
    use ordo_host::evolve::StepRun;
    let (pg, pool) = empty("ordo_prev_entity_big_run").await;
    let (x_src, y_src) = (
        pinned("1.0.0", "note", false),
        pinned("1.1.0", "memo", true),
    );
    apply_source(&pool, &x_src).await;
    pg.exec(
        "INSERT INTO note (\"$space\", id, rev, kind) \
         SELECT 'home', 'home' || lpad(i::text, 5, '0'), 1, 'home ' || i \
         FROM generate_series(1, 20000) i",
    )
    .expect("20,000 rows");
    store(&pool, &y_src).await;
    let step = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect("the step stores")
        .plan;
    let ctx = evolve::StepContext {
        settings: ordo_host::backfill::BackfillSettings {
            chunk_rows: 500,
            budget: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let run = || evolve::run_step_with(&pool, &pool, "deployment", &step, OWNER, &ctx);
    let first = run().await.expect("the step starts");
    assert!(matches!(first, StepRun::Backfilling { .. }), "{first:?}");
    assert_eq!(one(&pg, "SELECT count(*) FROM memo"), "500");

    let writer = std::sync::Arc::new(node_in(&pg.db, &x_src, "w", "home").await);
    let w = writer.clone();
    let writes = tokio::spawn(async move {
        let mut bad = Vec::new();
        let mut slowest = std::time::Duration::ZERO;
        for i in 0..150 {
            let started = std::time::Instant::now();
            if let Err(e) = w.write(format!(r#"addNote($kind: "live {i}")"#)).await {
                bad.push(format!("add {i}: {e}"));
            }
            if i % 5 == 0 {
                for id in [format!("home{:05}", 20000 - i), format!("home{:05}", i + 1)] {
                    if let Err(e) = w.write(format!(r#"dropNote($id: "{id}")"#)).await {
                        bad.push(format!("drop {id}: {e}"));
                    }
                }
            }
            slowest = slowest.max(started.elapsed());
        }
        (bad, slowest)
    });
    let mut runs = 1;
    let mut during = 0;
    loop {
        let r = run().await.expect("a run");
        runs += 1;
        if !matches!(r, StepRun::Backfilling { .. }) {
            break;
        }
        if !writes.is_finished() {
            during += 1;
        }
        assert!(runs < 200, "the copy ends");
    }
    let (bad, slowest) = writes.await.unwrap();
    assert!(bad.is_empty(), "every write landed: {bad:?}");
    assert!(during > 0, "the writes ran while the copy was backfilling");
    assert!(
        slowest < std::time::Duration::from_secs(5),
        "no write waited for the copy: {slowest:?}"
    );
    assert!(runs >= 40, "the copy ran in chunks: {runs} runs");
    assert_eq!(
        ordo_exec::spacedb::recorded_version(&pool)
            .await
            .expect("recorded"),
        ordo_exec::spacedb::TargetVersion::At("1.1.0".into())
    );
    assert_eq!(
        one(&pg, "SELECT count(*) FROM memo"),
        (20000 + 150 - 60).to_string()
    );
    assert_eq!(one(&pg, TABLES_DIFFER), "0", "every row is in both tables");
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

/// A real write node over `db` serving `source`, writing as `space`.
async fn node_in(db: &str, source: &str, name: &str, space: &str) -> Node {
    let mut n = node(db, source, name).await;
    n.exec.space = ordo_crypto::SpaceId(space.to_string());
    n.fixture = std::sync::Arc::new(n.fixture.in_space(space));
    n
}

fn is_nullable(pg: &Pg, column: &str) -> String {
    one(
        pg,
        &format!(
            "SELECT is_nullable FROM information_schema.columns \
             WHERE table_name = 'note' AND column_name = '{column}'"
        ),
    )
}

/// Increment 11b part 2 (`plan-backfill-dual-writes.md` §4; Q1): **tenants
/// on the deployment's own database move one by one, and their soaks close
/// together once no space there serves the version before.** `acme` and
/// `beta` keep their rows in the deployment's own database, beside its own
/// space. 1.1.0 renames `kind` to `mediaType`, a required field. `acme`
/// moves: the expand runs once for the database, the copy covers every
/// space's rows, and its pin switches. `beta`, still on 1.0.0, writes through
/// the real write path, and so does `acme` on 1.1.0: every row holds the
/// same value under both names. `beta` moves; its expand and copy are found
/// done. The new column's `NOT NULL` waits. A close is refused while the
/// deployment's own space serves 1.0.0, naming it, and so is `ordo evolve
/// --run` of the step over the open moves. Once the own space moves, a due
/// soak waits for the other soak of its step there, then both close
/// together: the `NOT NULL` lands, the database records 1.1.0, `kind` is
/// orphaned and leaves `$renames`.
///
/// A regression looks like the `NOT NULL` landing while a space there still
/// inserts as 1.0.0 (its insert would fail), a row whose two columns differ,
/// or one soak closing alone and orphaning the column the other tenants
/// still read and write.
#[tokio::test(flavor = "multi_thread")]
async fn tenants_on_the_deployments_own_database_move_and_their_soaks_close_together() {
    use ordo_host::dual_writes::{self, MoveContext};
    use ordo_host::versions::MovePhase;
    let (pg, pool) = empty("ordo_prev_shared").await;
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
    let ops_x = "\nop addNote($kind: text) =\n  set note { kind: $kind }\n";
    let ops_y = "\nop addNote($kind: text) =\n  set note { mediaType: $kind }\n";
    let x_src = notes("1.0.0", "  kind: text = \"\"\n") + ops_x;
    let y_src = notes("1.1.0", "  mediaType: text = \"\" prev(kind)\n") + ops_y;
    let run = |source: String, activate: bool| {
        let pool = pool.clone();
        async move {
            let program = Program::compile(&source).expect("compiles");
            ordo_host::apply_op::run_program(
                &pool,
                &ordo_exec::route::SinglePool::new(pool.clone()),
                &program,
                vec![],
                OWNER,
                "t",
                activate,
            )
            .await
            .expect("applies")
        }
    };
    run(x_src.clone(), true).await;
    // Stored, not applied: the move brings the database to it.
    store(&pool, &y_src).await;
    let plan = evolve::evolve(&pool, "1.0.0", "1.1.0", &Choices::default(), None, OWNER, 1)
        .await
        .expect("the step stores")
        .plan;
    let v = Versions::on(&pool);
    for t in ["acme", "beta"] {
        assert_eq!(
            v.pin_of(t).await.expect("read").and_then(|p| p.version),
            Some("1.0.0".to_string()),
            "{t}"
        );
    }
    for s in ["acme", "beta", "home"] {
        pg.exec(&format!(
            "INSERT INTO note (\"$space\", id, rev, kind) \
             SELECT '{s}', '{s}' || i, 1, '{s} ' || i FROM generate_series(1, 30) i"
        ))
        .expect("rows");
    }
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let ctx = evolve::StepContext {
        settings: ordo_host::backfill::BackfillSettings {
            chunk_rows: 25,
            ..Default::default()
        },
        ..Default::default()
    };
    let week = 7 * 86_400;

    // `acme` moves: one expand and one copy for the whole database.
    let moved = dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM note WHERE \"mediaType\" IS DISTINCT FROM kind"
        ),
        "0",
        "every space's rows are copied"
    );
    assert_eq!(is_nullable(&pg, "mediaType"), "YES", "the NOT NULL waits");

    // `beta` on 1.0.0 and `acme` on 1.1.0 write through the real path.
    let on_x = node_in(&pg.db, &x_src, "b", "beta").await;
    let on_y = node_in(&pg.db, &y_src, "a", "acme").await;
    for i in 0..10 {
        on_x.write(format!(r#"addNote($kind: "beta wrote {i}")"#))
            .await
            .expect("beta writes as 1.0.0");
        on_y.write(format!(r#"addNote($kind: "acme wrote {i}")"#))
            .await
            .expect("acme writes as 1.1.0");
    }
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM note WHERE \"mediaType\" IS DISTINCT FROM kind"
        ),
        "0",
        "each write reached both columns"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM note"), "110");

    // `beta` moves: its expand and copy are found done.
    let moved = dual_writes::run(&cx, "beta", &plan, &ctx, week, OWNER)
        .await
        .expect("beta moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(moved.statements, 0, "no second expand");

    // The deployment's own space still serves 1.0.0: no close, no `--run`.
    let err = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect_err("the own space is on 1.0.0");
    assert_eq!(err.class(), "soak_shared", "{err}");
    let text = err.to_string();
    assert!(text.contains("the deployment's own space"), "{text}");
    assert!(text.contains("1.1.0"), "{text}");
    let err = evolve::run_step(&pool, &pool, "deployment", &plan, OWNER)
        .await
        .expect_err("moves are open");
    assert_eq!(err.class(), "moves_open", "{err}");
    assert!(err.to_string().contains("acme, beta"), "{err}");
    assert_eq!(is_nullable(&pg, "mediaType"), "YES");

    // The own space moves. `acme`'s soak is due and `beta`'s is not: on
    // this database they close together, so neither closes yet.
    v.pin_own("home", "1.1.0", 0, OWNER, 20)
        .await
        .expect("the own space moves");
    pg.exec("UPDATE \"$moves\" SET soak_until = 1 WHERE space = 'acme'")
        .expect("acme due");
    assert!(dual_writes::close_due(&cx).await.is_empty());
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM \"$moves\" WHERE phase = 'soaking'"
        ),
        "2"
    );
    pg.exec("UPDATE \"$moves\" SET soak_until = 1 WHERE space = 'beta'")
        .expect("beta due");
    let mut closed = dual_writes::close_due(&cx).await;
    closed.sort();
    assert_eq!(closed, vec!["acme".to_string(), "beta".to_string()]);

    // Closed together: the constraint, the record, the orphan, `$renames`.
    assert_eq!(is_nullable(&pg, "mediaType"), "NO");
    assert_eq!(
        ordo_exec::spacedb::recorded_version(&pool)
            .await
            .expect("recorded"),
        ordo_exec::spacedb::TargetVersion::At("1.1.0".into())
    );
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM \"$orphans\" WHERE entity = 'note' AND field = 'kind'"
        ),
        "1"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$renames\""), "0");
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$moves\""), "0");
    for t in ["acme", "beta"] {
        assert_eq!(
            v.pin_of(t).await.expect("read").and_then(|p| p.version),
            Some("1.1.0".to_string()),
            "{t}"
        );
    }
}

/// The deployment's own database with `acme` and `beta` on 1.0.0 (`kind`
/// only), 30 rows in each of `acme`, `beta` and `home` (the own space), and
/// the step to `y_src` stored with `fill`.
async fn shared_with_fill(stem: &str, y_src: &str, fill: &str) -> (Pg, Pool, evolve::StoredPlan) {
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
    let program = Program::compile(&shared_x()).expect("compiles");
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
    store(&pool, y_src).await;
    let choices = evolve::typed_options("", fill, "", "").expect("options");
    let plan = evolve::evolve(&pool, "1.0.0", "1.1.0", &choices, None, OWNER, 1)
        .await
        .expect("the step stores")
        .plan;
    for s in ["acme", "beta", "home"] {
        pg.exec(&format!(
            "INSERT INTO note (\"$space\", id, rev, kind) \
             SELECT '{s}', '{s}' || i, 1, '{s} ' || i FROM generate_series(1, 30) i"
        ))
        .expect("rows");
    }
    (pg, pool, plan)
}

fn shared_x() -> String {
    notes("1.0.0", "  kind: text = \"\"\n")
        + "\nop addNote($kind: text) =\n  set note { kind: $kind }\n"
}

/// 1.1.0: `label`, a new required field the step fills, then `extra`.
fn labelled(extra: &str) -> String {
    notes(
        "1.1.0",
        &format!("  kind: text = \"\"\n  label: text{extra}\n"),
    ) + "\nop addNote($kind: text, $label: text) =\n  set note { kind: $kind, label: $label }\n"
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

/// A node on the version `space` soaks on, with its move's reverse
/// overlay, as `Spaces` hands it out.
async fn soaking_node(pool: &Pool, db: &str, source: &str, space: &str) -> Node {
    moving_node(pool, db, source, space, true).await
}

/// A node writing `space` in its move: the reverse overlay on the version
/// it moves to, the forward one on the version it leaves.
async fn moving_node(pool: &Pool, db: &str, source: &str, space: &str, reverse: bool) -> Node {
    let m = Versions::on(pool)
        .dual_move(space)
        .await
        .expect("read")
        .expect("a move");
    let o = ordo_host::dual_writes::overlays_of(&m).expect("overlays");
    let mut n = node_in(db, source, space, space).await;
    let side = if reverse { o.reverse } else { o.forward };
    n.exec.pool = n.exec.pool.with_overlay(Some(side));
    n
}

/// Increment 11b part 4 (`plan-backfill-dual-writes.md` §4): **a fill on
/// the deployment's own database keeps every space there current, and a
/// check the new version adds is held to the close.** 1.1.0 adds `label`,
/// required, filled from `kind`, and `check label != "bad"`. `acme` moves:
/// the copy runs once for the database and `$fills` records the fill.
/// `beta` and the own space, still on 1.0.0, write through the real write
/// path and their rows get the label by the mapping; `acme` on 1.1.0 writes
/// its own label, which is kept. The check is not in the database: the
/// host holds it on `acme`'s writes, and a `bad` label is refused by name.
/// `beta` moves with no second expand. The own space, still on 1.0.0, is
/// held to the check too (`$held`, 11b): a write whose mapped label breaks
/// it is refused by name. A row that breaks it all the same, written by
/// hand, makes the close refuse once every space is past 1.0.0, naming
/// that row; changed, the soaks close together: the `NOT NULL` and the
/// check land, `$fills` and `$held` are empty.
///
/// A regression looks like a row a space on 1.0.0 wrote with no label (the
/// `NOT NULL` would fail at the close), a label of `acme`'s overwritten by
/// the mapping, a check in the database before the close (a raw constraint
/// error on 1.0.0's writes), a write on 1.0.0 that breaks the held check
/// and is kept, or a close that fails on the raw constraint error instead
/// of naming the row.
#[tokio::test(flavor = "multi_thread")]
async fn a_fill_on_the_deployments_own_database_keeps_every_space_current_and_a_check_waits_for_the_close(
) {
    use ordo_host::dual_writes::{self, MoveContext};
    use ordo_host::versions::MovePhase;
    let y_src = labelled("\n  check label != \"bad\"");
    let (pg, pool, plan) = shared_with_fill("ordo_shared_fill", &y_src, "note.label=kind").await;
    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("receipt");
    assert_eq!(receipt["held"][0]["kind"], "check", "{receipt}");
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let ctx = step_context();
    let week = 7 * 86_400;
    let checks = "SELECT count(*) FROM pg_constraint WHERE conname LIKE 'note$chk%'";

    // `acme` moves: one copy for every space there; the fill is recorded.
    let moved = dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM note WHERE label IS DISTINCT FROM kind"
        ),
        "0",
        "every space's rows are filled"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$fills\""), "1");
    assert_eq!(is_nullable(&pg, "label"), "YES", "the NOT NULL waits");
    assert_eq!(one(&pg, checks), "0", "the check waits for the close");

    // `beta` and the own space on 1.0.0, `acme` on 1.1.0, through the real
    // write path.
    let beta = node_in(&pg.db, &shared_x(), "b", "beta").await;
    let home = node_in(&pg.db, &shared_x(), "h", "home").await;
    let acme = soaking_node(&pool, &pg.db, &y_src, "acme").await;
    for i in 0..5 {
        beta.write(format!(r#"addNote($kind: "beta wrote {i}")"#))
            .await
            .expect("beta writes as 1.0.0");
        home.write(format!(r#"addNote($kind: "home wrote {i}")"#))
            .await
            .expect("the own space writes as 1.0.0");
        acme.write(format!(
            r#"addNote($kind: "acme wrote {i}", $label: "mine {i}")"#
        ))
        .await
        .expect("acme writes as 1.1.0");
    }
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM note WHERE \"$space\" IN ('beta', 'home') \
             AND label IS DISTINCT FROM kind"
        ),
        "0",
        "a write on 1.0.0 gets the label by the mapping"
    );
    assert_eq!(
        one(&pg, "SELECT count(*) FROM note WHERE label LIKE 'mine %'"),
        "5",
        "a label written on 1.1.0 is kept"
    );
    // The check, held by the host on 1.1.0's writes.
    let err = acme
        .write(r#"addNote($kind: "acme", $label: "bad")"#.into())
        .await
        .expect_err("the held check refuses");
    assert!(err.contains("check_violation"), "{err}");
    assert!(err.contains("holds the check"), "{err}");
    assert!(err.contains("1.1.0"), "{err}");
    assert_eq!(
        one(&pg, "SELECT count(*) FROM note WHERE label = 'bad'"),
        "0"
    );

    // `beta` moves: its expand and copy are found done.
    let moved = dual_writes::run(&cx, "beta", &plan, &ctx, week, OWNER)
        .await
        .expect("beta moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(moved.statements, 0, "no second expand");

    // The own space, still on 1.0.0, is held to the check 1.1.0 adds: its
    // write is refused by name, the versions and the fix in the answer
    // (11b, `$held`).
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$held\""), "1");
    let err = home
        .write(r#"addNote($kind: "bad")"#.into())
        .await
        .expect_err("held to 1.1.0's check");
    assert!(
        err.contains("check_violation: this space serves `1.0.0`"),
        "{err}"
    );
    for want in ["holds the check", "move this space to `1.1.0`"] {
        assert!(err.contains(want), "{want}: {err}");
    }
    assert_eq!(
        one(&pg, "SELECT count(*) FROM note WHERE label = 'bad'"),
        "0"
    );
    // A row that breaks it all the same (written before the move, by
    // hand): the close still names it.
    pg.exec(
        "UPDATE note SET kind = 'bad', label = 'bad' WHERE \"$space\" = 'home' \
         AND id = (SELECT min(id) FROM note WHERE \"$space\" = 'home')",
    )
    .expect("a bad row");
    let bad = one(&pg, "SELECT id FROM note WHERE label = 'bad'");

    // Every space past 1.0.0, both soaks due: the close names the row.
    let v = Versions::on(&pool);
    v.pin_own("home", "1.1.0", 0, OWNER, 20)
        .await
        .expect("the own space moves");
    pg.exec("UPDATE \"$moves\" SET soak_until = 1")
        .expect("due");
    assert!(dual_writes::close_due(&cx).await.is_empty());
    let err = dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect_err("a row breaks the check");
    assert_eq!(err.class(), "held_constraint_broken", "{err}");
    let text = err.to_string();
    assert!(text.contains(&format!("home/{bad}")), "{text}");
    assert!(text.contains("--close-soak"), "{text}");
    assert_eq!(is_nullable(&pg, "label"), "YES", "nothing landed");
    assert_eq!(one(&pg, checks), "0");

    // Changed, the soaks close together and the check lands.
    pg.exec("UPDATE note SET label = 'fine' WHERE label = 'bad'")
        .expect("changed");
    let mut closed = dual_writes::close_due(&cx).await;
    closed.sort();
    assert_eq!(closed, vec!["acme".to_string(), "beta".to_string()]);
    assert_eq!(is_nullable(&pg, "label"), "NO");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_constraint WHERE conname LIKE 'note$chk%' AND convalidated"
        ),
        "1"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$fills\""), "0");
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$held\""), "0", "landed");
    assert_eq!(one(&pg, "SELECT count(*) FROM \"$moves\""), "0");
    assert_eq!(
        ordo_exec::spacedb::recorded_version(&pool)
            .await
            .expect("recorded"),
        ordo_exec::spacedb::TargetVersion::At("1.1.0".into())
    );
}

/// Increment 11b part 4: **`ordo evolve --run` of a step that holds a
/// check lands it at the step's end, after naming the rows that break it.**
/// The step is run on the deployment's own database with no move open. A
/// row whose mapped label is `bad` refuses the end by name, the command to
/// run again in the refusal, and nothing lands. Changed, a run again lands
/// the `NOT NULL` and the check.
///
/// A regression looks like the raw constraint error, or the check landing
/// in the expand, before the copy.
#[tokio::test(flavor = "multi_thread")]
async fn a_step_run_with_a_held_check_names_the_rows_that_break_it() {
    let y_src = labelled("\n  check label != \"bad\"");
    let (pg, pool, plan) = shared_with_fill("ordo_held_run", &y_src, "note.label=kind").await;
    pg.exec("UPDATE note SET kind = 'bad' WHERE id = 'beta4'")
        .expect("a bad row");
    let err = evolve::run_step(&pool, &pool, "deployment", &plan, OWNER)
        .await
        .expect_err("a row breaks the check");
    assert_eq!(err.class(), "held_constraint_broken", "{err}");
    let text = err.to_string();
    for want in ["beta/beta4", "check label", "--run"] {
        assert!(text.contains(want), "{want}: {text}");
    }
    let checks = "SELECT count(*) FROM pg_constraint WHERE conname LIKE 'note$chk%'";
    assert_eq!(one(&pg, checks), "0", "nothing landed");
    assert_eq!(is_nullable(&pg, "label"), "YES");

    pg.exec("UPDATE note SET kind = 'fine', label = 'fine' WHERE id = 'beta4'")
        .expect("changed");
    let ran = evolve::run_step(&pool, &pool, "deployment", &plan, OWNER)
        .await
        .expect("the step lands");
    assert!(matches!(ran, evolve::StepRun::Ran { .. }), "{ran:?}");
    assert_eq!(one(&pg, checks), "1");
    assert_eq!(is_nullable(&pg, "label"), "NO");
}

/// Increment 11b part 4: **a unique the new version adds is held by the
/// host until the switch, checked against the copied rows, and lands with
/// the switch.** 1.1.0 adds `label`, required and `unique`, filled from
/// `kind`. 2 of `acme`'s rows share a kind. Its move copies them and is
/// refused at the switch, naming both rows and the command to run again;
/// `acme` still serves 1.0.0, with no unique in the database. A write on
/// 1.0.0 whose mapped label another row of `acme` holds is refused by
/// name; one another space holds is not. Changed, the move switches and
/// the unique lands, since 1.1.0's writes converge on its key (`INSERT …
/// ON CONFLICT` needs the index). The close sends it no second time.
///
/// A regression looks like a switch over duplicate labels, a raw `23505`
/// from the database before the switch, or 1.1.0 failing its first write
/// with no index for its `ON CONFLICT`.
#[tokio::test(flavor = "multi_thread")]
async fn a_unique_the_new_version_adds_is_checked_against_the_copy_and_lands_at_the_switch() {
    use ordo_host::dual_writes::{self, MoveContext};
    use ordo_host::versions::MovePhase;
    let y_src = labelled(" unique");
    let (pg, pool, plan) = shared_with_fill("ordo_shared_unique", &y_src, "note.label=kind").await;
    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("receipt");
    assert_eq!(receipt["held"][0]["kind"], "unique", "{receipt}");
    pg.exec("UPDATE note SET kind = 'twice' WHERE id IN ('acme3', 'acme7')")
        .expect("a duplicate");
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let ctx = step_context();
    let week = 7 * 86_400;
    let uniques = "SELECT count(*) FROM pg_constraint WHERE conname = 'note$label$uq'";

    let err = dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect_err("2 rows share a label");
    assert_eq!(err.class(), "held_constraint_broken", "{err}");
    let text = err.to_string();
    for want in [
        "acme/acme3",
        "acme/acme7",
        "note$label$uq",
        "ordo evolve --tenant acme",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    let v = Versions::on(&pool);
    assert_eq!(
        v.pin_of("acme")
            .await
            .expect("read")
            .and_then(|p| p.version),
        Some("1.0.0".to_string()),
        "acme still serves 1.0.0"
    );
    assert_eq!(one(&pg, uniques), "0");

    // Still on 1.0.0, a write whose mapped label another row of `acme`
    // holds is refused by name; one another space holds is not.
    let acme_x = moving_node(&pool, &pg.db, &shared_x(), "acme", false).await;
    let err = acme_x
        .write(r#"addNote($kind: "acme 1")"#.into())
        .await
        .expect_err("the held unique refuses");
    assert!(err.contains("check_violation"), "{err}");
    assert!(err.contains("note$label$uq"), "{err}");
    acme_x
        .write(r#"addNote($kind: "beta 1")"#.into())
        .await
        .expect("unique within a space");

    // Changed, the move switches, and the unique lands with it: 1.1.0's
    // writes converge on its key.
    pg.exec("UPDATE note SET kind = 'once', label = 'once' WHERE id = 'acme7'")
        .expect("changed");
    let moved = dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");

    // The door's revert in the soak (`std.deployment.revertSoak`, what the
    // console's Moves screen calls): back on 1.0.0, recorded as
    // `soak-reverted`. Then the move runs again.
    let stream = std::sync::Arc::new(ordo_secevent::Recorder::default());
    let ops = ordo_host::apply_op::ApplyOps::recording(
        pool.clone(),
        std::sync::Arc::new(ordo_exec::route::SinglePool::new(pool.clone())),
        "home",
        Some(stream.clone()),
    );
    let tenant = ordo_api::BoundParams(
        serde_json::json!({ "tenant": "acme" })
            .as_object()
            .cloned()
            .expect("an object"),
    );
    let back = evolve::execute(&ops, evolve::REVERT_SOAK, &tenant, OWNER)
        .await
        .expect("reverted");
    assert_eq!(back["phase"], "gated", "{back}");
    let moved = dual_writes::run(&cx, "acme", &plan, &ctx, week, OWNER)
        .await
        .expect("acme moves again");
    assert_eq!(moved.phase, MovePhase::Soaking, "{moved:?}");
    assert_eq!(one(&pg, uniques), "1", "the unique lands at the switch");
    let acme = moving_node(&pool, &pg.db, &y_src, "acme", true).await;
    acme.write(r#"addNote($kind: "k", $label: "twice")"#.into())
        .await
        .expect("converges on the label");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM note WHERE \"$space\" = 'acme' AND label = 'twice'"
        ),
        "1"
    );

    // `beta` moves, the own space moves, the soaks close: the unique lands.
    dual_writes::run(&cx, "beta", &plan, &ctx, week, OWNER)
        .await
        .expect("beta moves");
    v.pin_own("home", "1.1.0", 0, OWNER, 20)
        .await
        .expect("the own space moves");
    // Closed through the door (`std.deployment.closeSoak`), recorded.
    let closed = evolve::execute(&ops, evolve::CLOSE_SOAK, &tenant, OWNER)
        .await
        .expect("the soaks close");
    assert_eq!(
        closed["closedWith"],
        serde_json::json!(["beta"]),
        "{closed}"
    );
    assert_eq!(one(&pg, uniques), "1");
    assert_eq!(is_nullable(&pg, "label"), "NO");
    let acts: Vec<String> = stream
        .drain_all(ordo_commandlog::Timestamp(u64::MAX / 2))
        .into_iter()
        .filter_map(|e| match e.kind {
            ordo_secevent::EventKind::Deploy { act, .. } => Some(act),
            _ => None,
        })
        .collect();
    assert_eq!(acts, ["soak-reverted", "soak-closed"], "{acts:?}");
}

/// Increment 11b part 5: **a space still on the version before, in a
/// shared database, that meets the unique the new version landed is
/// refused by name.** `acme` moves to 1.1.0 (`label`, unique, filled from
/// `kind`) and the unique lands at its switch. `beta`, still on 1.0.0,
/// writes through the real write path; its `$fills` overlay sets `label`
/// from `kind`. A kind another row of `beta` holds is refused as
/// `duplicate`, naming `note.label`, the row and the command that moves the
/// space, and nothing is written. A kind another space holds is not.
///
/// A regression looks like the database's own `23505` text reaching the
/// caller (an `internal` 500), or a write that lands half.
#[tokio::test(flavor = "multi_thread")]
async fn a_space_on_the_version_before_that_meets_the_new_unique_is_refused_by_name() {
    use ordo_host::dual_writes::{self, MoveContext};
    let y_src = labelled(" unique");
    let (pg, pool, plan) =
        shared_with_fill("ordo_shared_unique_x", &y_src, "note.label=kind").await;
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    dual_writes::run(&cx, "acme", &plan, &step_context(), 7 * 86_400, OWNER)
        .await
        .expect("acme moves");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_constraint WHERE conname = 'note$label$uq'"
        ),
        "1"
    );
    let before = one(&pg, "SELECT count(*) FROM note");

    let beta = node_in(&pg.db, &shared_x(), "b", "beta").await;
    let err = beta
        .write(r#"addNote($kind: "beta 1")"#.into())
        .await
        .expect_err("another row of beta holds the label");
    assert!(err.starts_with("duplicate:"), "{err}");
    for want in [
        "`note.label`",
        "ordo evolve --tenant",
        "Nothing was written",
    ] {
        assert!(err.contains(want), "{want}: {err}");
    }
    assert!(!err.contains("23505"), "{err}");
    let answered = ordo_host::classify(err);
    assert_eq!(
        answered.code,
        ordo_api::error::ErrorCode::Duplicate,
        "{answered:?}"
    );
    assert_eq!(one(&pg, "SELECT count(*) FROM note"), before);

    // A label another space holds is free here.
    beta.write(r#"addNote($kind: "acme 1")"#.into())
        .await
        .expect("unique within a space");
}

/// Increment 11b part 5: **the guard of a held unique reads a plain index,
/// not the table.** 20,000 extra rows in `acme`. `acme`'s move stops at its
/// budget, before the switch, while the guard holds the unique: the plain
/// index `note$label$uq$held` over the space and `label` is there, and the
/// guard's own statement, explained, reads it. At the close it is gone.
///
/// A regression looks like a sequential scan of `note` in the plan: every
/// write in the move would read the whole table.
#[tokio::test(flavor = "multi_thread")]
async fn the_guard_of_a_held_unique_reads_its_plain_index_until_the_close() {
    use ordo_host::dual_writes::{self, MoveContext};
    use ordo_host::versions::MovePhase;
    let y_src = labelled(" unique");
    let (pg, pool, plan) = shared_with_fill("ordo_held_index", &y_src, "note.label=kind").await;
    pg.exec(
        "INSERT INTO note (\"$space\", id, rev, kind) \
         SELECT 'acme', 'big' || i, 1, 'big ' || i FROM generate_series(1, 20000) i",
    )
    .expect("rows");
    let shared = ordo_exec::route::SinglePool::new(pool.clone());
    let cx = MoveContext {
        control: &pool,
        pools: &shared,
        spaces: None,
    };
    let stop = evolve::StepContext {
        settings: ordo_host::backfill::BackfillSettings {
            chunk_rows: 1000,
            budget: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let week = 7 * 86_400;
    let moved = dual_writes::run(&cx, "acme", &plan, &stop, week, OWNER)
        .await
        .expect("acme's copy starts");
    assert_eq!(moved.phase, MovePhase::Backfilling, "{moved:?}");
    let held_index = "SELECT count(*) FROM pg_indexes WHERE indexname = 'note$label$uq$held'";
    assert_eq!(one(&pg, held_index), "1", "the plain index is made");
    pg.exec("ANALYZE note").expect("analyze");

    let receipt: serde_json::Value = serde_json::from_str(&plan.receipt).expect("receipt");
    let held = ordo_host::schema_apply::Held::from_json(&receipt["held"][0]).expect("held");
    let plan_text = pg
        .query(&format!(
            "EXPLAIN SELECT \"id\"::text FROM \"note\" WHERE \"$space\" = 'acme' \
             AND \"id\" IN ('acme1') AND NOT coalesce(({}), false)",
            held.holds()
        ))
        .expect("explain")
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(plan_text.contains("note$label$uq$held"), "{plan_text}");
    assert!(!plan_text.contains("Seq Scan on note o"), "{plan_text}");

    // The move ends: the switch lands the unique, the close drops the plain
    // index.
    dual_writes::run(&cx, "acme", &plan, &step_context(), week, OWNER)
        .await
        .expect("acme moves");
    dual_writes::run(&cx, "beta", &plan, &step_context(), week, OWNER)
        .await
        .expect("beta moves");
    Versions::on(&pool)
        .pin_own("home", "1.1.0", 0, OWNER, 20)
        .await
        .expect("the own space moves");
    dual_writes::close(&cx, "acme", OWNER)
        .await
        .expect("the soaks close");
    assert_eq!(one(&pg, held_index), "0", "the plain index is dropped");
    assert_eq!(
        one(
            &pg,
            "SELECT count(*) FROM pg_constraint WHERE conname = 'note$label$uq'"
        ),
        "1"
    );
}
