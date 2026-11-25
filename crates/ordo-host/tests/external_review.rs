//! Review findings on external entities, slice C: what the watcher holds,
//! when a hold ends, what the present list leaves out, and the type a ref
//! column must have.
//!
//! A regression looks like an op that touches no external refused while one
//! is down, a hold that outlives the program that fixed it, a write that
//! reads a held table and fails as a raw database error, Ordo's own shadow
//! schema listed as foreign, or a ref column of the wrong type accepted.
//!
//! Needs the conformance cluster. Databases are prefixed `ordo_extr_`.

mod refine_support;

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::external_check::{self, ExternalGate, ExternalGated};
use refine_support::{call, door_over};

const PROGRAM: &str = r#"
external patrons (key: pat) from crm.customers shared {
  id <- "customer_id"
  name: text <- "full_name"
}

entity tickets {
  customer -> patrons
  subject: text
}

entity memos {
  body: text
}

op mine =
  patrons(name == $caller)[id]

op mineMemos =
  memos(body == $caller)[id]
"#;

const FOREIGN: &str = r#"
DROP SCHEMA IF EXISTS crm CASCADE;
CREATE SCHEMA crm;
CREATE TABLE crm.customers (customer_id bigint PRIMARY KEY, full_name text NOT NULL);
INSERT INTO crm.customers VALUES (9, 'Ada');
"#;

fn fixture(program: &str, foreign: &str) -> Arc<Fixture> {
    let seed: &'static str = Box::leak(format!("{SEED}\n{foreign}").into_boxed_str());
    Arc::new(
        Fixture::from_source(&format!("{FIXTURE}\n{program}"), seed)
            .expect("a program with an external compiles"),
    )
}

fn gated_over(
    state: &mut ordo_api::ApiState,
    served: Arc<Mutex<Arc<Fixture>>>,
) -> Arc<ExternalGate> {
    let gate = ExternalGate::new(Arc::new(move || served.lock().expect("lock").clone()));
    state.executor = Arc::new(ExternalGated::new(state.executor.clone(), gate.clone()));
    gate
}

async fn door(
    db: &str,
) -> (
    ordo_api::ApiState,
    Arc<ExternalGate>,
    Pool,
    Arc<Mutex<Arc<Fixture>>>,
) {
    let f = fixture(PROGRAM, FOREIGN);
    let (mut state, f, pool) = door_over(f, db, 50, PoolConfig::from_env(), |k| k).await;
    let served = Arc::new(Mutex::new(f));
    let gate = gated_over(&mut state, served.clone());
    (state, gate, pool, served)
}

#[tokio::test]
async fn an_unbindable_op_that_reads_no_external_still_serves() {
    let f = fixture(PROGRAM, FOREIGN);
    assert!(
        ordo_sema::bind_op_generic(
            &f.schema,
            &f.registry,
            "giftOrders",
            ordo_sema::Clock::CORPUS
        )
        .is_err(),
        "the precondition: this op does not bind generically"
    );
    let (state, gate, pool, _) = door("ordo_extr_unbound").await;
    Pg::named("ordo_extr_unbound")
        .exec("ALTER TABLE crm.customers DROP COLUMN full_name")
        .expect("drops");
    assert_eq!(gate.watch_once(&pool).await.expect("reads").len(), 1);
    // The op itself cannot run (a sealed filter), and it says so as itself.
    // What matters is that the hold is not what refused it.
    let (_, body) = call(&state, "GET", "/op/giftOrders", None).await;
    assert!(!body.to_string().contains("external_changed"), "{body}");
    // One that does reach the external is held.
    let (status, body) = call(&state, "GET", "/op/mine", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(body.to_string().contains("external_changed"), "{body}");
}

#[tokio::test]
async fn a_write_that_only_names_a_ref_keeps_answering_while_the_table_is_down() {
    let (state, gate, pool, _) = door("ordo_extr_write").await;
    Pg::named("ordo_extr_write")
        .exec("DROP TABLE crm.customers")
        .expect("drops");
    assert_eq!(gate.watch_once(&pool).await.expect("reads").len(), 1);
    // The language has no condition that reads another entity in a write, so a
    // write reaches an external only by naming a ref to it, which reads
    // nothing: the write path checks the prefix only. The gate holds a write
    // by the same set as a read, and that set is empty here.
    let (status, body) = call(
        &state,
        "POST",
        "/op/std.tickets.create",
        Some(serde_json::json!({ "customer": "pat_9", "subject": "w" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn applying_a_regenerated_translation_lifts_the_stale_hold() {
    let (_state, gate, pool, served) = door("ordo_extr_regen").await;
    Pg::named("ordo_extr_regen")
        .exec("ALTER TABLE crm.customers RENAME COLUMN full_name TO label")
        .expect("renames");
    assert_eq!(gate.watch_once(&pool).await.expect("reads").len(), 1);
    assert!(gate.refusal("std.patrons.list").is_some());
    // The regenerated program is served: no waiting for the timer.
    *served.lock().expect("lock") = fixture(PROGRAM, FOREIGN);
    assert!(gate.refusal("std.patrons.list").is_none());
}

/// **Every served version is checked** (`plan-evolution-federation.md`
/// step F3). A node serves 2 versions: the active one reads the renamed
/// column, the one a tenant is pinned to still reads the old name. The
/// timer holds the pinned version's tenants alone, names the version in
/// its line, and the active version's tenants are served on. Through the
/// door, the pinned tenant's read answers `external_changed`.
#[tokio::test]
async fn a_hold_found_for_one_served_version_holds_only_its_tenants() {
    const DB: &str = "ordo_extr_per_version";
    let old = fixture(PROGRAM, FOREIGN);
    let (mut state, old, pool) = door_over(old, DB, 50, PoolConfig::from_env(), |k| k).await;
    let new = fixture(&PROGRAM.replace("\"full_name\"", "\"label\""), FOREIGN);
    Pg::named(DB)
        .exec("ALTER TABLE crm.customers RENAME COLUMN full_name TO label")
        .expect("renames");
    let pinned = ordo_conformance::fixture_space().0;
    let gate = {
        let (active, old2, new2) = (new.clone(), old.clone(), new.clone());
        let pinned = pinned.clone();
        ExternalGate::per_version(
            Arc::new(move || active.clone()),
            Arc::new(move || {
                vec![
                    ("2.0.0".to_string(), new2.clone()),
                    ("1.0.0".to_string(), old2.clone()),
                ]
            }),
            Arc::new(move |space: &str| {
                if space == pinned {
                    old.clone()
                } else {
                    new.clone()
                }
            }),
        )
    };
    state.executor = Arc::new(ExternalGated::new(state.executor.clone(), gate.clone()));

    let lines = gate.watch_once(&pool).await.expect("reads");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("1.0.0: "),
        "names the version: {lines:?}"
    );
    assert!(
        gate.refusal_in("std.patrons.list", &pinned).is_some(),
        "the pinned version's tenant is held"
    );
    assert!(
        gate.refusal_in("std.patrons.list", "spc_on_active")
            .is_none(),
        "the active version's tenants are served on"
    );
    assert!(gate.refusal("std.patrons.list").is_none(), "the active one");
    let (status, body) = call(&state, "GET", "/op/mine", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(body.to_string().contains("external_changed"), "{body}");
}

const REFS: &str = r#"
external accounts (key: acct) from crm.accounts shared {
  id: uuid <- "account_id"
}

external people (key: per) from crm.people shared {
  id <- "person_id"
  account -> accounts <- "account_id"
}
"#;

#[tokio::test]
async fn a_ref_column_of_the_wrong_type_for_its_target_is_refused() {
    let foreign = "DROP SCHEMA IF EXISTS crm CASCADE; CREATE SCHEMA crm; \
        CREATE TABLE crm.accounts (account_id uuid PRIMARY KEY); \
        CREATE TABLE crm.people (person_id bigint PRIMARY KEY, account_id uuid NOT NULL);";
    let f = fixture(REFS, foreign);
    let pg = Pg::named("ordo_extr_refs");
    f.reset(&pg).expect("seeds");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let report = external_check::check(&pool, &f.schema)
        .await
        .expect("reads");
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    pg.exec("ALTER TABLE crm.people ALTER COLUMN account_id TYPE integer USING 0")
        .expect("retypes");
    let report = external_check::check(&pool, &f.schema)
        .await
        .expect("reads");
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    let d = &report.problems[0].detail;
    assert!(
        d.contains("`account_id` is `integer`")
            && d.contains("a ref to `accounts`, whose key is uuid"),
        "{d}"
    );
}

#[tokio::test]
async fn ordos_own_shadow_schemas_are_not_foreign() {
    let f = fixture(PROGRAM, FOREIGN);
    let pg = Pg::named("ordo_extr_shadow");
    f.reset(&pg).expect("seeds");
    pg.exec(
        "CREATE SCHEMA IF NOT EXISTS ordo_shadow; \
         CREATE SCHEMA IF NOT EXISTS ordo_shadow_export; CREATE SCHEMA IF NOT EXISTS ordo_shadow_diff1a2b; \
         CREATE SCHEMA IF NOT EXISTS other_systems;",
    )
    .expect("leftovers");
    let pool = Pool::connect(PoolConfig {
        db: pg.db.clone(),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool");
    let present = external_check::present_not_ordos(&pool, &f.schema)
        .await
        .expect("reads");
    assert_eq!(
        present,
        ["schema `other_systems`, with no tables"],
        "{present:?}"
    );
}
