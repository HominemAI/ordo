//! 25ad: a revert past a money rescale converts the old amount (ruled
//! 2026-10-06, board `revert-past-rescale`).
//!
//! A `money(p)` field stores minor units in a bigint. A stated
//! `--places x.y=rescale` rewrites every stored amount and moves `rev`, and
//! appends no group, so the history keeps the old minor units. A revert of a
//! group from before the rescale re-applies one of those images. It now
//! converts the amount by the rescales recorded in `$places` since the
//! group, and expects `rev` to have moved once per rescale.
//!
//! A regression looks like 29.99 coming back as 0.2999 (no conversion),
//! like `cas_miss` on a row nobody wrote (the rescale's `rev` move not
//! expected), like a lost digit passing silently, or like a `keep`
//! converting a number that was meant as written.
//!
//! Needs the conformance cluster (`scripts/conformance-pg.sh start`).

use ordo_commandlog::{
    Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId, Image, SpaceId, Timestamp,
    Value,
};
use ordo_conformance::Pg;
use ordo_exec::undo::{self, MoneyRounding, RevertPolicy};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{self as sa, Applied, Choices, Program};

fn src(total: &str) -> String {
    format!(
        "entity invoices {{\n  label: text\n  total: {total}\n}}\n\n\
         op listInvoices unbounded =\n  invoices[label, total]\n"
    )
}

const SPACE: &str = "s1";

fn space() -> SpaceId {
    SpaceId(SPACE.into())
}

fn one(pg: &Pg, sql: &str) -> String {
    pg.query(sql)
        .expect("query")
        .first()
        .and_then(|r| r.first().cloned().flatten())
        .unwrap_or_default()
}

fn total(pg: &Pg, id: &str) -> String {
    one(
        pg,
        &format!("SELECT total::text || ' rev ' || rev FROM invoices WHERE id = '{id}'"),
    )
}

/// An empty database serving `from`, with the chain table, and one row
/// per `(id, amount)` at revision 1.
async fn serving(name: &str, from: &str, rows: &[(&str, &str)]) -> (Pg, Pool, Program) {
    let pg = Pg::scoped(name);
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        size: 2,
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots");
    let v1 = Program::compile(&src(from)).expect("v1 compiles");
    let (_, created) = sa::apply(&pool, &v1, "test", "test").await;
    assert!(matches!(created, Ok(Applied::Created(_))), "{created:?}");
    ordo_exec::registry::ensure_tables(&pool)
        .await
        .expect("registry");
    ordo_exec::registry::store(
        &pool,
        &ordo_exec::registry::Artifact {
            version: v1.version.clone(),
            artifact: ordo_plan::artifact::to_bytes(&v1.doc),
            source: v1.source.clone(),
            ddl: v1.ddl.clone(),
            applied_at: Timestamp(0),
            actor: "test".into(),
        },
    )
    .await
    .expect("stored");
    ordo_exec::registry::activate(&pool, &v1.version, "test", Timestamp(0))
        .await
        .expect("activated");
    pool.rows(ordo_exec::commandlog::COMMANDLOG_DDL)
        .await
        .expect("the chain table");
    for (id, amount) in rows {
        pg.exec(&format!(
            "INSERT INTO invoices (\"$space\", id, rev, label, total) \
             VALUES ('{SPACE}', '{id}', 1, 'label', {amount})"
        ))
        .expect("a row");
    }
    (pg, pool, v1)
}

fn img(id: &str, rev: i64, amount: &str) -> Image {
    [
        ("id", Value::Id(id.into())),
        ("rev", Value::Int(rev)),
        ("label", Value::Text("label".into())),
        ("total", Value::Text(amount.into())),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Log one command as group `cg_<n>`, and make the row match what it left,
/// as the write protocol would have.
async fn write(pg: &Pg, pool: &Pool, n: usize, id: &str, kind: CommandKind, from: &str, to: &str) {
    let rev: i64 = one(
        pg,
        &format!("SELECT COALESCE(max(rev), 0) FROM invoices WHERE id = '{id}'"),
    )
    .parse()
    .unwrap();
    let (before, after) = match kind {
        CommandKind::Converge => (Some(img(id, rev, from)), Some(img(id, rev + 1, to))),
        CommandKind::Delete => (Some(img(id, rev, from)), None),
        CommandKind::Create => (None, Some(img(id, 1, to))),
    };
    let group = CommandGroup {
        correlation: CorrelationId(format!("cg_{n}")),
        actor: Actor::User("mem_o".into()),
        commands: vec![Command {
            id: CommandId(format!("cmd_{n}")),
            space: space(),
            entity: "invoices".into(),
            row_id: id.into(),
            kind,
            before,
            after,
            at: Timestamp(1_000 + n as u64),
        }],
        owner_administration: false,
        restored_unverified: false,
        restore: None,
    };
    let tx = pool.begin().await.expect("a transaction");
    tx.append_group(&group).await.expect("appended");
    tx.commit().await.expect("committed");
    match kind {
        CommandKind::Converge => pg
            .exec(&format!(
                "UPDATE invoices SET total = {to}, rev = rev + 1 WHERE id = '{id}'"
            ))
            .expect("the write"),
        CommandKind::Delete => pg
            .exec(&format!("DELETE FROM invoices WHERE id = '{id}'"))
            .expect("the delete"),
        CommandKind::Create => unreachable!("not used"),
    }
}

async fn rescale(pool: &Pool, to: &str, choice: &str) -> Program {
    let next = Program::compile(&src(to)).expect("compiles");
    let (plan, outcome) = sa::apply_with(
        pool,
        &next,
        "test",
        "test",
        &Choices::parse_places(&format!("invoices.total={choice}")).unwrap(),
    )
    .await;
    assert!(
        matches!(outcome, Ok(Applied::Additive { .. })),
        "the change of places applies ({:?}): {outcome:?}",
        plan.map(|p| p.refusals)
    );
    next
}

async fn revert(
    pool: &Pool,
    program: &Program,
    n: usize,
    policy: &RevertPolicy,
) -> Result<undo::Reverted, undo::RevertFailure> {
    undo::revert_with(
        pool,
        &program.schema,
        None,
        None,
        None,
        &space(),
        &CorrelationId(format!("cg_{n}")),
        "mem_o",
        Timestamp(9_000),
        policy,
    )
    .await
}

/// **29.99 comes back as 29.99 after a rescale up.** The group changed
/// 29.99 to 19.99 at money(2); the field then moved to money(4) with
/// `rescale`, rewriting 1999 to 199900 and moving `rev`. The revert writes
/// 299900, which is 29.99 at 4 places.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_past_one_rescale_up_restores_29_99_as_29_99() {
    let (pg, pool, _) = serving("ordo_revert_rescale_up", "money(2)", &[("inv_0", "2999")]).await;
    write(
        &pg,
        &pool,
        0,
        "inv_0",
        CommandKind::Converge,
        "2999",
        "1999",
    )
    .await;
    let v2 = rescale(&pool, "money(4)", "rescale").await;
    assert_eq!(total(&pg, "inv_0"), "199900 rev 3");
    assert_eq!(
        one(
            &pg,
            "SELECT heads::text FROM \"$places\" ORDER BY seq DESC LIMIT 1"
        ),
        "{\"s1\": 0}",
        "the change records the chain head it followed"
    );

    // The dry run shows the converted amount too.
    let plan = undo::plan_revert(
        &pool,
        &v2.schema,
        None,
        &space(),
        &CorrelationId("cg_0".into()),
        "mem_o",
        Timestamp(9_000),
    )
    .await
    .expect("the plan");
    assert_eq!(
        plan.commands[0].after.as_ref().unwrap()["total"],
        Value::Text("299900".into())
    );

    revert(&pool, &v2, 0, &RevertPolicy::DEFAULT)
        .await
        .expect("a revert past a rescale converts rather than refusing");
    assert_eq!(total(&pg, "inv_0"), "299900 rev 4", "29.99 at 4 places");
}

/// **A rescale down that would lose digits is refused by default, and
/// rounds half to even when the app developer allows it.** The group
/// changed 29.9995 to 29.9990 at money(4); the field moved to money(3),
/// which divides by 10. 299995 / 10 is 29999.5: refused, naming the field;
/// allowed, it rounds to the even 30000 (30.000).
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_past_a_rescale_down_that_loses_digits_is_refused_unless_rounding_is_allowed() {
    let (pg, pool, _) = serving(
        "ordo_revert_rescale_down",
        "money(4)",
        &[("inv_0", "299995")],
    )
    .await;
    write(
        &pg,
        &pool,
        0,
        "inv_0",
        CommandKind::Converge,
        "299995",
        "299990",
    )
    .await;
    let v2 = rescale(&pool, "money(3)", "rescale").await;
    assert_eq!(total(&pg, "inv_0"), "29999 rev 3");

    let refused = revert(&pool, &v2, 0, &RevertPolicy::DEFAULT)
        .await
        .expect_err("a lost digit is refused by default");
    let why = refused.to_string();
    assert!(
        why.starts_with(undo::PLACES_REVERT_LOSES_DIGITS) && why.contains("`invoices.total`"),
        "{why}"
    );
    // Audit 30: the refusal names no setting that does not exist, and says
    // what to do instead.
    assert!(!why.contains("revertRounding"), "{why}");
    assert!(why.contains("ordinary command"), "{why}");
    assert_eq!(total(&pg, "inv_0"), "29999 rev 3", "nothing was written");

    // The field layer allows it.
    let mut policy = RevertPolicy::DEFAULT;
    policy
        .money_rounding_fields
        .insert(("invoices".into(), "total".into()), MoneyRounding::HalfEven);
    revert(&pool, &v2, 0, &policy)
        .await
        .expect("allowed, it rounds");
    assert_eq!(total(&pg, "inv_0"), "30000 rev 4");
}

/// **A `keep` converts nothing.** It stated that the stored number was
/// meant as written, so the rows were not rewritten, `rev` did not move,
/// and the old image goes back as it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_past_a_keep_converts_nothing() {
    let (pg, pool, _) = serving("ordo_revert_keep", "money(2)", &[("inv_0", "2999")]).await;
    write(
        &pg,
        &pool,
        0,
        "inv_0",
        CommandKind::Converge,
        "2999",
        "1999",
    )
    .await;
    let v2 = rescale(&pool, "money(3)", "keep").await;
    assert_eq!(total(&pg, "inv_0"), "1999 rev 2");

    revert(&pool, &v2, 0, &RevertPolicy::DEFAULT)
        .await
        .expect("reverts");
    assert_eq!(total(&pg, "inv_0"), "2999 rev 3");
}

/// **Two rescales add up**, and a deleted row comes back converted too.
/// money(2) to money(3) to money(5) is 10^3 in all and two `rev` moves. A
/// group from before both converts 29.99 to 2999000; a delete from before
/// both restores 0.05 as 5000. A group written between the two converts by
/// the second only.
#[tokio::test(flavor = "multi_thread")]
async fn a_revert_past_two_rescales_converts_by_both() {
    let (pg, pool, _) = serving(
        "ordo_revert_two_rescales",
        "money(2)",
        &[("inv_0", "2999"), ("inv_1", "5"), ("inv_2", "100")],
    )
    .await;
    write(
        &pg,
        &pool,
        0,
        "inv_0",
        CommandKind::Converge,
        "2999",
        "1999",
    )
    .await;
    write(&pg, &pool, 1, "inv_1", CommandKind::Delete, "5", "").await;
    rescale(&pool, "money(3)", "rescale").await;
    // Between the two: inv_2 was 1.000 at money(3).
    write(
        &pg,
        &pool,
        2,
        "inv_2",
        CommandKind::Converge,
        "1000",
        "2000",
    )
    .await;
    let v3 = rescale(&pool, "money(5)", "rescale").await;
    assert_eq!(total(&pg, "inv_0"), "1999000 rev 4");
    assert_eq!(total(&pg, "inv_2"), "200000 rev 4");

    revert(&pool, &v3, 0, &RevertPolicy::DEFAULT)
        .await
        .expect("past both");
    assert_eq!(total(&pg, "inv_0"), "2999000 rev 5", "29.99 at 5 places");

    revert(&pool, &v3, 1, &RevertPolicy::DEFAULT)
        .await
        .expect("the deleted row comes back");
    assert_eq!(total(&pg, "inv_1"), "5000 rev 1", "0.05 at 5 places");

    revert(&pool, &v3, 2, &RevertPolicy::DEFAULT)
        .await
        .expect("past the second only");
    assert_eq!(total(&pg, "inv_2"), "100000 rev 5", "1.000 at 5 places");
}
