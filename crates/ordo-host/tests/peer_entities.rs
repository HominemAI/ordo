//! `peer` and `$caller` across two Ordo deployments in one database (owner
//! rulings 2026-11-16 and 2026-11-17, `example-apps-notes-v2-plus-note_viewer`
//! asks 1, 2, 5, 10 and 12).
//!
//! Deployment A lives in schema `alpha`: a guarded `notes` entity whose rows
//! belong to members of two spaces. Deployment B lives in `public`, declares
//! `peer journal` and reads A's `notes` as an external, through the real
//! door. B's people and A's people are linked only by the sign-in subject
//! both deployments hold (`oidc:identity:<sub>` in each `$authenticator`).
//!
//! What it proves:
//!
//! - A person signed in to B sees exactly the notes A's guard gives the same
//!   person, in every space of A where they are a member.
//! - A different person sees theirs. A person with no member in A sees none.
//! - A password name is local to a deployment and never links two people.
//! - `$caller` in an op's filter on the peer's table means the same.
//! - A plain external beside it keeps today's `$caller`: the local member.
//! - Each refusal: the peer's setting unset, a bad schema name, this
//!   deployment's own schema, a schema with no Ordo deployment, and a plain
//!   external's swap (`ORDO_EXTERNAL_<SCHEMA>_SCHEMA`) set, unset and bad.
//!
//! A regression looks like a person of B seeing A's rows for another person,
//! seeing nothing where A shows rows, or a peer that boots without its
//! setting.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_peer_`.

mod refine_support;

use std::sync::Arc;

use axum::http::StatusCode;
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::peer::{self, SchemaRefusal};
use ordo_exec::{Pool, PoolConfig};
use ordo_host::schema_apply::{self as sa, Program};
use ordo_host::DEFAULT_SPACE;
use refine_support::{call_as, door_over};
use serde_json::Value;

/// Deployment A: notes, each readable by its owner only.
const ALPHA: &str = r#"
entity notes {
  title: text
  owner -> members?
  access read owner == $caller
  access write owner == $caller
}
"#;

/// Deployment B: A's notes through a peer, and a plain external beside it.
const BETA: &str = r#"
peer journal

external entries from journal.notes shared {
  id: text <- id
  title: text <- title
  owner: text <- owner
  access read owner == $caller
}

external tags from crm.tags shared {
  id: text <- code
  label: text <- label
  owner: text <- owner
  access read owner == $caller
}

op myEntries =
  entries(owner == $caller)[title]
  sort title
  take 50

op notMine =
  entries(owner != $caller)[title]
  sort title
  take 50
"#;

fn cfg(db: &str, schema: &str) -> PoolConfig {
    PoolConfig {
        db: db.into(),
        size: 3,
        schema: schema.into(),
        ..PoolConfig::from_env()
    }
}

/// B's program, with its peer resolved to `alpha` as
/// `ORDO_PEER_JOURNAL_SCHEMA=alpha` would.
fn beta_fixture() -> Fixture {
    let mut f = Fixture::from_source(&format!("{FIXTURE}\n{BETA}"), SEED).expect("B compiles");
    peer::resolve_with(&mut f.schema, "public", |v| {
        (v == "ORDO_PEER_JOURNAL_SCHEMA").then(|| "alpha".to_string())
    })
    .expect("the peer resolves");
    f
}

/// Both deployments in one database, seeded. Returns B's door.
async fn two_deployments(db: &str) -> ordo_api::ApiState {
    let (state, _, b) = door_over(
        Arc::new(beta_fixture()),
        db,
        50,
        PoolConfig::from_env(),
        |k| k,
    )
    .await;
    let pg = Pg::named(db);
    pg.exec("DROP SCHEMA IF EXISTS alpha CASCADE; DROP SCHEMA IF EXISTS crm CASCADE;")
        .expect("reset");
    let a = Pool::connect(cfg(&pg.db, "alpha")).await.expect("A boots");
    let program = Program::compile(ALPHA).expect("A compiles");
    let (plan, outcome) = sa::apply(&a, &program, "test", "test").await;
    assert!(plan.expect("plans").refusals.is_empty());
    assert!(outcome.is_ok(), "{outcome:?}");
    ordo_host::principal_directory::ensure_table(&a)
        .await
        .expect("A's directory");
    ordo_host::principal_directory::ensure_table(&b)
        .await
        .expect("B's directory");

    let member = |schema: &str, space: &str, id: &str, principal: &str| {
        format!(
            "INSERT INTO {schema}.members (\"$space\", id, principal, \"displayName\", kind, \
             \"memberCount\", role, status, joined) VALUES ('{space}', '{id}', '{principal}', \
             '{id}', 'principal', 1, 'editor', 'active', '2026-11-17');"
        )
    };
    let link = |schema: &str, auth: &str, principal: &str| {
        format!(
            "INSERT INTO {schema}.\"$authenticator\" (authenticator, principal) \
             VALUES ('{auth}', '{principal}'); \
             INSERT INTO {schema}.\"$principal\" (id, kind, display) \
             VALUES ('{principal}', 'human', '{principal}') ON CONFLICT DO NOTHING;"
        )
    };
    let mut sql = String::new();
    // A: Ada is a member of two spaces, Bo of one, Dee of one.
    sql += &member("alpha", "sa1", "mem_a_ada", "prn_ada_a");
    sql += &member("alpha", "sa2", "mem_a_ada2", "prn_ada_a");
    sql += &member("alpha", "sa1", "mem_a_bo", "prn_bo_a");
    sql += &member("alpha", "sa1", "mem_a_dee", "prn_dee_a");
    sql += &link("alpha", "oidc:identity:sub-ada", "prn_ada_a");
    sql += &link("alpha", "oidc:identity:sub-bo", "prn_bo_a");
    // Dee signs in to A with a password: a name only A knows.
    sql += &link("alpha", "password:dee", "prn_dee_a");
    sql += "INSERT INTO alpha.notes (\"$space\", id, title, owner) VALUES \
            ('sa1', 'note_1', 'ada in sa1', 'mem_a_ada'), \
            ('sa2', 'note_2', 'ada in sa2', 'mem_a_ada2'), \
            ('sa1', 'note_3', 'bo in sa1', 'mem_a_bo'), \
            ('sa1', 'note_4', 'dee in sa1', 'mem_a_dee'), \
            ('sa1', 'note_5', 'nobody', NULL);";
    // B: the same people under B's own ids, and Cy, who has no A account.
    for (id, principal, auth) in [
        ("mem_b_ada", "prn_ada_b", "oidc:identity:sub-ada"),
        ("mem_b_bo", "prn_bo_b", "oidc:identity:sub-bo"),
        ("mem_b_cy", "prn_cy_b", "oidc:identity:sub-cy"),
        ("mem_b_dee", "prn_dee_b", "password:dee"),
    ] {
        sql += &member("public", DEFAULT_SPACE, id, principal);
        sql += &link("public", auth, principal);
    }
    // The plain external: owned by B's own member ids.
    sql += "CREATE SCHEMA crm; \
            CREATE TABLE crm.tags (code text PRIMARY KEY, label text NOT NULL, owner text); \
            INSERT INTO crm.tags VALUES ('t1', 'ada tag', 'mem_b_ada'), \
              ('t2', 'bo tag', 'mem_b_bo'), ('t3', 'A id tag', 'mem_a_ada');";
    pg.exec(&sql).expect("seed");
    state
}

async fn titles(state: &ordo_api::ApiState, who: &str, op: &str, field: &str) -> Vec<String> {
    let (status, body) = call_as(
        state,
        Some(&format!("member:{who}")),
        "GET",
        &format!("/op/{op}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{who} {op}: {body}");
    let mut out: Vec<String> = body["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r: &Value| r[field].as_str().unwrap_or("").to_string())
        .collect();
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn caller_on_a_peer_is_the_peers_member_for_the_same_person() {
    let state = two_deployments("ordo_peer_caller").await;

    // Ada: her notes in both of A's spaces, and nobody else's.
    assert_eq!(
        titles(&state, "mem_b_ada", "std.entries.list", "title").await,
        ["ada in sa1", "ada in sa2"]
    );
    // Bo: his.
    assert_eq!(
        titles(&state, "mem_b_bo", "std.entries.list", "title").await,
        ["bo in sa1"]
    );
    // Cy has no member in A: nothing, and no error.
    assert!(titles(&state, "mem_b_cy", "std.entries.list", "title")
        .await
        .is_empty());
    // Dee shares only a password name with A, which links nobody.
    assert!(titles(&state, "mem_b_dee", "std.entries.list", "title")
        .await
        .is_empty());

    // In an op's filter, `$caller` on the peer's table means the same.
    assert_eq!(
        titles(&state, "mem_b_ada", "myEntries", "title").await,
        ["ada in sa1", "ada in sa2"]
    );
    assert!(titles(&state, "mem_b_cy", "myEntries", "title")
        .await
        .is_empty());
    // `!=` is its negation, and still under the guard: Ada reads only her
    // own rows, so none of them is not hers.
    assert!(titles(&state, "mem_b_ada", "notMine", "title")
        .await
        .is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plain_external_keeps_the_local_caller() {
    let state = two_deployments("ordo_peer_plain").await;
    assert_eq!(
        titles(&state, "mem_b_ada", "std.tags.list", "label").await,
        ["ada tag"],
        "the local member id, never A's"
    );
    assert_eq!(
        titles(&state, "mem_b_bo", "std.tags.list", "label").await,
        ["bo tag"]
    );
    assert!(titles(&state, "mem_b_cy", "std.tags.list", "label")
        .await
        .is_empty());
}

/// The compiled program, unresolved, as the load gate receives it.
fn compiled() -> Fixture {
    Fixture::from_source(BETA, "").expect("compiles")
}

#[test]
fn the_peer_setting_is_required_and_checked() {
    // Unset.
    let e = peer::resolve_with(&mut compiled().schema, "public", |_| None).expect_err("unset");
    assert!(matches!(e, SchemaRefusal::Unset { .. }), "{e}");
    let text = e.to_string();
    assert!(
        text.contains("ORDO_PEER_JOURNAL_SCHEMA is not set")
            && text.contains("Set ORDO_PEER_JOURNAL_SCHEMA"),
        "{text}"
    );
    // A bad name.
    for bad in ["Journal", "no-tes", "pg_x", "ordo_shadow_x"] {
        let e = peer::resolve_with(&mut compiled().schema, "public", |v| {
            (v == "ORDO_PEER_JOURNAL_SCHEMA").then(|| bad.to_string())
        })
        .expect_err(bad);
        assert!(matches!(e, SchemaRefusal::Invalid { .. }), "{bad}: {e}");
        assert!(e.to_string().contains("ORDO_PEER_JOURNAL_SCHEMA"), "{e}");
    }
    // This deployment's own schema.
    let e = peer::resolve_with(&mut compiled().schema, "beta", |v| {
        (v == "ORDO_PEER_JOURNAL_SCHEMA").then(|| "beta".to_string())
    })
    .expect_err("own");
    assert!(matches!(e, SchemaRefusal::Own { .. }), "{e}");
    // Unresolved, a peer's table reads a schema nobody can create, never a
    // plain schema that shares the nickname.
    let s = compiled().schema;
    let src = s.entity("entries").unwrap().external.as_ref().unwrap();
    assert_eq!(
        src.pg_schema(),
        ordo_schema::unresolved_peer_schema("journal")
    );
}

#[test]
fn a_plain_external_may_swap_its_schema() {
    // Unset: as written.
    let mut s = compiled().schema;
    peer::resolve_with(&mut s, "public", |v| {
        (v == "ORDO_PEER_JOURNAL_SCHEMA").then(|| "alpha".to_string())
    })
    .expect("resolves");
    let pg = |s: &ordo_schema::Schema, e: &str| {
        s.entity(e).unwrap().external.as_ref().unwrap().pg_schema()
    };
    assert_eq!(pg(&s, "tags"), "crm");
    assert_eq!(pg(&s, "entries"), "alpha");
    // Set: swapped.
    let mut s = compiled().schema;
    peer::resolve_with(&mut s, "public", |v| match v {
        "ORDO_PEER_JOURNAL_SCHEMA" => Some("alpha".into()),
        "ORDO_EXTERNAL_CRM_SCHEMA" => Some("crm_dev".into()),
        // A peer nickname is read by its own variable only.
        "ORDO_EXTERNAL_JOURNAL_SCHEMA" => Some("elsewhere".into()),
        _ => None,
    })
    .expect("resolves");
    assert_eq!(pg(&s, "tags"), "crm_dev");
    assert_eq!(pg(&s, "entries"), "alpha", "the peer meaning wins");
    // Bad: refused naming the variable.
    let e = peer::resolve_with(&mut compiled().schema, "public", |v| match v {
        "ORDO_PEER_JOURNAL_SCHEMA" => Some("alpha".into()),
        "ORDO_EXTERNAL_CRM_SCHEMA" => Some("Crm Dev".into()),
        _ => None,
    })
    .expect_err("bad");
    assert!(e.to_string().contains("ORDO_EXTERNAL_CRM_SCHEMA"), "{e}");
    // A quoted schema with no clean variable name is never swapped.
    assert_eq!(peer::external_schema_env("Legacy"), None);
    assert_eq!(
        peer::external_schema_env("crm").as_deref(),
        Some("ORDO_EXTERNAL_CRM_SCHEMA")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_load_gate_refuses_a_peer_it_cannot_serve() {
    let pg = Pg::scoped("ordo_peer_gate");
    pg.exec(
        "DROP SCHEMA IF EXISTS empty CASCADE; DROP SCHEMA IF EXISTS crm CASCADE; \
         CREATE SCHEMA empty; CREATE SCHEMA crm; \
         CREATE TABLE crm.tags (code text PRIMARY KEY, label text NOT NULL, owner text);",
    )
    .expect("reset");
    let c = cfg(&pg.db, "public");
    let pool = Pool::connect(c.clone()).await.expect("boots");
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space("s"));
    let gate = |schema: Option<&str>| {
        // The gate reads the environment. Only this test in the binary sets
        // the variable; the others resolve through `resolve_with`.
        match schema {
            Some(s) => std::env::set_var("ORDO_PEER_JOURNAL_SCHEMA", s),
            None => std::env::remove_var("ORDO_PEER_JOURNAL_SCHEMA"),
        }
        compiled()
    };
    // Unset: the load is refused naming the variable.
    let r = ordo_host::deploy::verify_embedded(
        gate(None),
        &pool,
        &c,
        keys.clone(),
        ordo_crypto::SpaceId("s".into()),
        "v1",
    )
    .await;
    let Err(e) = r else { panic!("refused") };
    let text = e.to_string();
    assert!(
        text.contains("ORDO_PEER_JOURNAL_SCHEMA is not set"),
        "{text}"
    );
    // A schema with no Ordo deployment in it.
    let r = ordo_host::deploy::verify_embedded(
        gate(Some("empty")),
        &pool,
        &c,
        keys.clone(),
        ordo_crypto::SpaceId("s".into()),
        "v1",
    )
    .await;
    let Err(e) = r else { panic!("refused") };
    let text = e.to_string();
    assert!(
        text.contains("holds no Ordo deployment") && text.contains("ORDO_PEER_JOURNAL_SCHEMA"),
        "{text}"
    );

    // A schema that holds one: the gate loads, and every op's SQL, the
    // peer lookup included, plans and runs under the shape gate.
    let a = Pool::connect(cfg(&pg.db, "alpha")).await.expect("A boots");
    let (plan, outcome) = sa::apply(&a, &Program::compile(ALPHA).unwrap(), "t", "t").await;
    assert!(plan.expect("plans").refusals.is_empty());
    assert!(outcome.is_ok(), "{outcome:?}");
    ordo_host::principal_directory::ensure_table(&a)
        .await
        .expect("A's directory");
    ordo_host::principal_directory::ensure_table(&pool)
        .await
        .expect("B's directory");
    let r = ordo_host::deploy::verify_embedded_pinned(
        gate(Some("alpha")),
        &pool,
        &c,
        keys.clone(),
        ordo_crypto::SpaceId("s".into()),
        "v1",
    )
    .await;
    std::env::remove_var("ORDO_PEER_JOURNAL_SCHEMA");
    assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
}
