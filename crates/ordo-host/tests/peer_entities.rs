//! `peer` and `$caller` across two Ordo deployments in one database (owner
//! rulings 2026-11-16, 2026-11-17 and 2026-11-18,
//! `example-apps-notes-v2-plus-note_viewer` asks 1, 2, 5, 10 and 12, and the
//! matching rule).
//!
//! Deployment A lives in schema `alpha`: a guarded `notes` entity whose rows
//! belong to members of two spaces. Deployment B lives in `public`, declares
//! `peer journal` and reads A's `notes` as an external, through the real
//! door. B's people and A's people are linked by the sign-in provider and
//! the subject. A signs in through the stand-in issuer as connection
//! `identity`, B through the same issuer as `corp`: the connection
//! fingerprints (`$authconnection`) are the stand-in's, recorded as a start
//! records them, and the links are seeded as a sign-in writes them.
//!
//! What it proves:
//!
//! - A person signed in to B sees exactly the notes A's guard gives the same
//!   person, in every space of A where they are a member, though the two
//!   deployments name the connection differently.
//! - A different person sees theirs. A person with no member in A sees none.
//! - A password name is local to a deployment and never links two people.
//! - The same connection name pointing at another issuer matches nothing:
//!   refused at start, dropped on a refresh. Before 2026-11-18 it matched.
//! - A person who reaches two accounts in A reads nothing, and is listed.
//! - No shared provider refuses the start when `$caller` is used on the
//!   peer's tables, and only lists when it is not.
//! - A peer with no `$authconnection` refuses: start it first.
//! - A connection whose subject claim can change hands is never linked.
//! - `$caller` in an op's filter on the peer's table means the same.
//! - A plain external beside it keeps today's `$caller`: the local member.
//! - Each refusal: the peer's setting unset, a bad schema name, this
//!   deployment's own schema, a schema with no Ordo deployment, and a plain
//!   external's swap (`ORDO_EXTERNAL_<SCHEMA>_SCHEMA`) set, unset and bad.
//!
//! A regression looks like a person of B seeing A's rows for another person,
//! seeing nothing where A shows rows, a match through a connection name, or
//! a peer that boots without its setting.
//!
//! Needs the conformance cluster. Its own databases, prefixed `ordo_peer_`.

mod oidc_support;
mod refine_support;

use std::sync::Arc;

use axum::http::StatusCode;
use oidc_support::FakeIdp;
use ordo_conformance::{Fixture, Pg, FIXTURE, SEED};
use ordo_exec::peer::{self, Pass, SchemaRefusal};
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

/// B without `$caller` on the peer's table.
const BETA_OPEN: &str = r#"
peer journal

external entries from journal.notes shared {
  id: text <- id
  title: text <- title
  access read open
}
"#;

/// B's program, with its peer resolved to `alpha` as
/// `ORDO_PEER_JOURNAL_SCHEMA=alpha` would.
fn beta_fixture() -> Fixture {
    resolved(&format!("{FIXTURE}\n{BETA}"))
}

fn resolved(src: &str) -> Fixture {
    let mut f = Fixture::from_source(src, SEED).expect("B compiles");
    peer::resolve_with(&mut f.schema, "public", |v| {
        (v == "ORDO_PEER_JOURNAL_SCHEMA").then(|| "alpha".to_string())
    })
    .expect("the peer resolves");
    f
}

/// Record an OIDC connection to `idp` as a start does.
async fn connect(pool: &Pool, name: &str, idp: &FakeIdp) {
    let fp = idp.connection(name).fingerprint();
    ordo_host::federation::ensure_fingerprint(pool, name, "oidc", &fp, "sub", 0)
        .await
        .expect("recorded");
}

/// The two deployments, and what the tests change between passes.
struct World {
    state: ordo_api::ApiState,
    pg: Pg,
    /// A's pool, in `alpha`.
    a: Pool,
    /// B's pool, in `public`.
    b: Pool,
    /// The issuer both sign in through.
    idp: FakeIdp,
    /// Another issuer.
    other: FakeIdp,
}

impl World {
    async fn link(&self, pass: Pass) -> Result<Vec<peer::PeerLinks>, SchemaRefusal> {
        ordo_host::deploy::link_peers(&self.b, &beta_fixture(), pass).await
    }
}

/// Both deployments in one database, seeded, the map built at start.
async fn two_deployments(db: &str) -> ordo_api::ApiState {
    let w = world(db).await;
    w.link(Pass::Start).await.expect("B starts");
    w.state
}

/// Both deployments in one database, seeded, with no map built yet.
async fn world(db: &str) -> World {
    let (state, _, b) = door_over(
        Arc::new(beta_fixture()),
        db,
        50,
        PoolConfig::from_env(),
        |k| k,
    )
    .await;
    let pg = Pg::named(db);
    pg.exec(
        "DROP SCHEMA IF EXISTS alpha CASCADE; DROP SCHEMA IF EXISTS crm CASCADE; \
         DROP TABLE IF EXISTS public.\"$peerlink\", public.\"$authconnection\";",
    )
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
    // B calls the same issuer `corp`.
    for (id, principal, auth) in [
        ("mem_b_ada", "prn_ada_b", "oidc:corp:sub-ada"),
        ("mem_b_bo", "prn_bo_b", "oidc:corp:sub-bo"),
        ("mem_b_cy", "prn_cy_b", "oidc:corp:sub-cy"),
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
    // One issuer, two names: A's `identity`, B's `corp`.
    let idp = FakeIdp::start().await;
    let other = FakeIdp::start().await;
    connect(&a, "identity", &idp).await;
    connect(&b, "corp", &idp).await;
    World {
        state,
        pg,
        a,
        b,
        idp,
        other,
    }
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

fn add_b_member(id: &str, principal: &str, auth: &str) -> String {
    format!(
        "INSERT INTO public.members (\"$space\", id, principal, \"displayName\", kind, \
         \"memberCount\", role, status, joined) VALUES ('{DEFAULT_SPACE}', '{id}', \
         '{principal}', '{id}', 'principal', 1, 'editor', 'active', '2026-11-17'); \
         INSERT INTO public.\"$authenticator\" (authenticator, principal) \
         VALUES ('{auth}', '{principal}'); \
         INSERT INTO public.\"$principal\" (id, kind, display) \
         VALUES ('{principal}', 'human', '{principal}') ON CONFLICT DO NOTHING;"
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_issuer_under_another_name_matches() {
    // A calls the issuer `identity`, B calls it `corp`.
    let w = world("ordo_peer_names").await;
    let links = w.link(Pass::Start).await.expect("B starts");
    assert_eq!(
        links[0].pairs,
        [("oidc:corp:".to_string(), "oidc:identity:".to_string())]
    );
    assert_eq!(
        titles(&w.state, "mem_b_ada", "std.entries.list", "title").await,
        ["ada in sa1", "ada in sa2"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_name_at_another_issuer_matches_nothing() {
    let w = world("ordo_peer_conflict").await;
    // Eve signs in to B through B's own `identity`, another issuer, with
    // the subject Ada has at A's `identity`. Before 2026-11-18 the two
    // strings were equal and Eve read Ada's notes.
    connect(&w.b, "identity", &w.other).await;
    w.pg.exec(&add_b_member(
        "mem_b_eve",
        "prn_eve_b",
        "oidc:identity:sub-ada",
    ))
    .expect("seed Eve");

    // At start: refused, naming what to change.
    let e = w
        .link(Pass::Start)
        .await
        .expect_err("a name meaning two issuers");
    assert!(matches!(e, SchemaRefusal::NameConflict { .. }), "{e}");
    let text = e.to_string();
    assert!(
        text.contains("`identity`") && text.contains("new name"),
        "{text}"
    );

    // Found on a refresh: that connection is dropped and logged, the shared
    // one still links.
    let links = w.link(Pass::Refresh).await.expect("a refresh never stops");
    assert_eq!(links[0].dropped, ["identity"]);
    assert!(links[0]
        .start_lines()
        .iter()
        .any(|l| l.contains("`identity` now points at another provider")));
    assert!(
        titles(&w.state, "mem_b_eve", "std.entries.list", "title")
            .await
            .is_empty(),
        "a connection name links nobody"
    );
    assert_eq!(
        titles(&w.state, "mem_b_ada", "std.entries.list", "title").await,
        ["ada in sa1", "ada in sa2"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_who_reaches_two_accounts_reads_nothing() {
    let w = world("ordo_peer_double").await;
    // A has a second connection to the same issuer, and through it Bo's
    // subject is linked to a second account with its own note.
    connect(&w.a, "identity2", &w.idp).await;
    w.pg.exec(
        "INSERT INTO alpha.\"$authenticator\" (authenticator, principal) \
         VALUES ('oidc:identity2:sub-bo', 'prn_bo2_a'); \
         INSERT INTO alpha.\"$principal\" (id, kind, display) \
         VALUES ('prn_bo2_a', 'human', 'bo2'); \
         INSERT INTO alpha.members (\"$space\", id, principal, \"displayName\", kind, \
         \"memberCount\", role, status, joined) VALUES ('sa1', 'mem_a_bo2', 'prn_bo2_a', \
         'bo2', 'principal', 1, 'editor', 'active', '2026-11-17'); \
         INSERT INTO alpha.notes (\"$space\", id, title, owner) \
         VALUES ('sa1', 'note_6', 'bo2 in sa1', 'mem_a_bo2');",
    )
    .expect("seed");
    let links = w.link(Pass::Start).await.expect("B starts");
    assert_eq!(links[0].pairs.len(), 2, "{:?}", links[0].pairs);
    assert_eq!(links[0].ambiguous, ["prn_bo_b"]);
    assert!(links[0]
        .start_lines()
        .iter()
        .any(|l| l.contains("more than one account") && l.contains("prn_bo_b")));
    assert!(
        titles(&w.state, "mem_b_bo", "std.entries.list", "title")
            .await
            .is_empty(),
        "two accounts: neither"
    );
    // Ada still reaches one.
    assert_eq!(
        titles(&w.state, "mem_b_ada", "std.entries.list", "title").await,
        ["ada in sa1", "ada in sa2"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_shared_provider_refuses_only_when_caller_is_used() {
    let w = world("ordo_peer_none").await;
    // B's `corp` now points at another issuer: nothing is shared.
    w.pg.exec("DELETE FROM public.\"$authconnection\"")
        .expect("reset");
    connect(&w.b, "corp", &w.other).await;
    let e = w.link(Pass::Start).await.expect_err("nobody could match");
    assert!(matches!(e, SchemaRefusal::NoSharedProvider { .. }), "{e}");
    let text = e.to_string();
    assert!(
        text.contains("shares no sign-in provider")
            && text.contains("`corp` (no connection in journal"),
        "{text}"
    );
    // Without `$caller` on the peer's table it starts, and lists.
    let open = resolved(&format!("{FIXTURE}\n{BETA_OPEN}"));
    let links = ordo_host::deploy::link_peers(&w.b, &open, Pass::Start)
        .await
        .expect("starts");
    let lines = links[0].start_lines();
    assert!(
        lines[0].contains("will not match in journal") && lines[0].contains("`corp`"),
        "{lines:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_with_no_connection_table_is_started_first() {
    let w = world("ordo_peer_unstarted").await;
    w.pg.exec("DROP TABLE alpha.\"$authconnection\"")
        .expect("drop");
    let e = peer::check(&w.b, &beta_fixture().schema)
        .await
        .expect_err("not started");
    assert!(matches!(e, SchemaRefusal::NotStarted { .. }), "{e}");
    assert!(
        e.to_string()
            .contains("Start journal first, then this deployment."),
        "{e}"
    );
    let e = w.link(Pass::Start).await.expect_err("not started");
    assert!(matches!(e, SchemaRefusal::NotStarted { .. }), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changeable_subject_claim_is_never_linked() {
    let w = world("ordo_peer_mutable").await;
    // Both deployments have the same OAuth2 provider, keyed by `login`, and
    // the deployment owner acknowledged it. Dee signs in to both through it.
    for pool in [&w.a, &w.b] {
        ordo_host::federation::ensure_fingerprint(pool, "gh", "oauth2", "fp-gh-login", "login", 0)
            .await
            .expect("recorded");
    }
    w.pg.exec(
        "INSERT INTO alpha.\"$authenticator\" (authenticator, principal) \
         VALUES ('oauth2:gh:dee', 'prn_dee_a'); \
         INSERT INTO public.\"$authenticator\" (authenticator, principal) \
         VALUES ('oauth2:gh:dee', 'prn_dee_b');",
    )
    .expect("seed");
    let links = w.link(Pass::Start).await.expect("B starts");
    assert!(
        links[0].unmatched.iter().any(|u| u.connection == "gh"
            && u.why.contains("`login` can change hands")
            && u.why.contains("ORDO_AUTH_GH_SUBJECT_CLAIM")),
        "{:?}",
        links[0].unmatched
    );
    assert!(titles(&w.state, "mem_b_dee", "std.entries.list", "title")
        .await
        .is_empty());
    // The GitHub and Discord presets key by the numeric `id`, so they link.
    for name in ["github", "discord"] {
        let p = ordo_identity::oauth2::preset(name).expect("a preset");
        assert_eq!(p.subject_claim, "id");
        assert!(!ordo_identity::oauth2::MUTABLE_SUBJECT_CLAIMS.contains(&p.subject_claim));
    }
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
         DROP SCHEMA IF EXISTS alpha CASCADE; \
         DROP TABLE IF EXISTS public.\"$authconnection\", public.\"$peerlink\"; \
         CREATE SCHEMA empty; CREATE SCHEMA crm; \
         CREATE TABLE crm.tags (code text PRIMARY KEY, label text NOT NULL, owner text);",
    )
    .expect("reset");
    let c = cfg(&pg.db, "public");
    let pool = Pool::connect(c.clone()).await.expect("boots");
    let keys: Arc<dyn ordo_crypto::KeyProvider> =
        Arc::new(ordo_crypto::TestProvider::with_space("s"));
    // This test runs both deployments as the cluster's one role, so it
    // allows that explicitly (`ORDO_PEER_SHARED_ROLE`); the default refuses
    // it, as the end of this test shows. `reader_grants.rs` runs two roles.
    std::env::set_var("ORDO_PEER_SHARED_ROLE", "on");
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
    // A has never started with sign-in on: no `$authconnection` yet.
    let r = ordo_host::deploy::verify_embedded_pinned(
        gate(Some("alpha")),
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
        text.contains("Start journal first, then this deployment."),
        "{text}"
    );
    // Both started, through one issuer under two names.
    let idp = FakeIdp::start().await;
    connect(&a, "identity", &idp).await;
    connect(&pool, "corp", &idp).await;
    let r = ordo_host::deploy::verify_embedded_pinned(
        gate(Some("alpha")),
        &pool,
        &c,
        keys.clone(),
        ordo_crypto::SpaceId("s".into()),
        "v1",
    )
    .await;
    assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    // Without the setting, one role for both is refused: grants would
    // decide nothing.
    std::env::remove_var("ORDO_PEER_SHARED_ROLE");
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
    let Err(e) = r else { panic!("refused") };
    let text = e.to_string();
    assert!(
        text.contains("is a superuser") && text.contains("ORDO_PEER_SHARED_ROLE=on"),
        "{text}"
    );
}
