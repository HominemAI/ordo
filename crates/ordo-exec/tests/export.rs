//! Full space export, against a real Postgres.
//!
//! The load-bearing test in this file is
//! [`a_new_entity_and_a_new_field_reach_the_bundle_with_no_export_code`].
//! Every other one checks that the export is *correct*; that one checks
//! that it is **dynamic**, which is the property the owner asked for by
//! name: *"the code can change with the flexibility of the data it is
//! exporting — otherwise we'll have to modify the export code for every
//! change."*
//!
//! Dynamism is inherent to how `export.rs` is written — it walks
//! `Schema::entities` and `physical_columns` and knows no entity name — so
//! the test cannot fail today. That is exactly why it is worth writing: it
//! is a **specification of the requirement**, and the day somebody
//! hand-rolls a column list or a `match entity_name` to fix an edge case,
//! this is the test that stops them. A requirement nothing asserts is a
//! requirement that decays.
//!
//! Databases are prefixed `ordo_exp_`, one per test function: test
//! functions in one binary run concurrently and each opens with
//! `DROP SCHEMA public CASCADE`.

use std::time::Duration;

use ordo_conformance::Pg;
use ordo_crypto::{KeyProvider, Purpose, SpaceId, TestProvider};
use ordo_exec::export::{
    export_space, BundleSink, ExportError, ExportRequest, MemoryBundle, SealedMode,
};
use ordo_exec::registry::Artifact;
use ordo_exec::{Pool, PoolConfig};
use ordo_schema::{build, ddl_for, Schema};
use ordo_syntax::parse_program;

const SPACE: &str = "spc_exp";
const OTHER: &str = "spc_neighbour";

// ── scaffolding ──────────────────────────────────────────────────────────

fn schema_of(src: &str) -> Schema {
    build(&parse_program(src).expect("the fixture parses")).expect("the fixture builds")
}

/// A stored artifact for `src`. The version is a label here rather than a
/// real hash: nothing under test reads it as one, and a fixed string keeps
/// the manifest byte-stable across runs.
fn artifact_of(version: &str, src: &str) -> Artifact {
    let schema = schema_of(src);
    Artifact {
        version: version.to_string(),
        artifact: "{\n  \"stand-in\": \"the plan document, stored verbatim\"\n}\n".into(),
        source: src.to_string(),
        ddl: ddl_for(&schema).expect("the fixture renders"),
        applied_at: ordo_commandlog::Timestamp(0),
        actor: "test".into(),
    }
}

async fn pool_for(db: &str) -> Pool {
    Pool::connect(PoolConfig {
        db: db.to_string(),
        size: 2,
        acquire_timeout: Duration::from_secs(5),
        ..PoolConfig::from_env()
    })
    .await
    .expect("pool boots")
}

/// An empty `public` schema holding exactly what `src` declares, plus the
/// platform's own tables.
fn deploy(pg: &Pg, art: &Artifact) {
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("schema reset");
    pg.exec(&art.ddl).expect("the generated DDL applies");
}

fn scratch(db: &str) -> Pg {
    let admin = Pg {
        db: "postgres".into(),
        ..Pg::default()
    };
    let _ = admin.exec(&format!("CREATE DATABASE {db}"));
    Pg {
        db: db.to_string(),
        ..Pg::default()
    }
}

fn keys() -> TestProvider {
    TestProvider::with_space(SPACE)
}

fn request<'a>(
    space: &'a SpaceId,
    schema: &'a Schema,
    artifact: &'a Artifact,
    keys: &'a dyn KeyProvider,
    sealed: SealedMode,
) -> ExportRequest<'a> {
    ExportRequest {
        space,
        schema,
        artifact,
        keys,
        // No blob tier in this fixture's deployment. The manifest says
        // which of the two absences this is — `blobs.carried: false` means
        // "no tier wired", not "no blob fields" — and
        // `crates/ordo-exec/tests/blobgc.rs` plus
        // `crates/ordo-blob/tests/pipeline.rs` cover the wired path.
        blobs: None,
        sealed,
        correlation: "cg_export_test",
    }
}

/// One JSONL member, parsed.
fn lines(bundle: &MemoryBundle, member: &str) -> Vec<serde_json::Value> {
    bundle
        .get(member)
        .unwrap_or_else(|| panic!("`{member}` is not in the bundle: {:?}", bundle.names()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every JSONL line is one JSON object"))
        .collect()
}

fn manifest(bundle: &MemoryBundle) -> serde_json::Value {
    serde_json::from_str(bundle.get("manifest.json").expect("a manifest")).expect("valid JSON")
}

// ── the programs ─────────────────────────────────────────────────────────

/// The program before the change. Deliberately exercises every awkward
/// column kind at once: a ref, a tree, an optional, a money, a bool, a
/// date, a sealed field and a sealed **indexed** field.
const BEFORE: &str = "\
entity vendors {
  name:   text
  active: bool = true
}
entity folders {
  name:   text
  parent -> folders?  tree
}
entity orders (key: ord) {
  vendor -> vendors
  placed: date
  total:  money
  note:   text?
  memo:   text sealed
  email:  text sealed indexed
}
";

/// The same program plus **a new entity and a new field**, and nothing
/// else. The whole dynamism claim is that the diff between the two bundles
/// is exactly the diff between the two programs.
const AFTER: &str = "\
entity vendors {
  name:   text
  active: bool = true
}
entity folders {
  name:   text
  parent -> folders?  tree
}
entity orders (key: ord) {
  vendor -> vendors
  placed: date
  total:  money
  note:   text?
  memo:   text sealed
  email:  text sealed indexed
  courier: text = \"none\"
}
entity shipments (key: shp) {
  order -> orders
  carrier: text
  weight:  int
}
";

/// Rows for `BEFORE`, in this space and in a neighbour's, so every
/// assertion about scoping has something to be wrong about.
fn seed(pg: &Pg, keys: &TestProvider, space: &str) {
    let lease = keys
        .release(&SpaceId(space.to_string()), Purpose::Write, "seed")
        .expect("release");
    let seal = |entity: &str, field: &str, row: &str, v: &str| {
        ordo_crypto::sealed::bytea_lit(
            &keys
                .seal_at(
                    &lease,
                    entity,
                    field,
                    ordo_crypto::Anchor::Row(row),
                    v.as_bytes(),
                )
                .expect("seal"),
        )
    };
    let bidx = |entity: &str, field: &str, v: &str| {
        ordo_crypto::sealed::bytea_lit(
            &keys
                .blind_index(
                    &SpaceId(space.to_string()),
                    entity,
                    field,
                    ordo_crypto::BlindValue::Text(v),
                )
                .expect("digest"),
        )
    };
    let sql = format!(
        "INSERT INTO \"vendors\" (\"$space\", \"id\", \"rev\", \"name\", \"active\") VALUES \
           ('{space}', 'vend_2', 1, 'Beta', false), \
           ('{space}', 'vend_1', 1, 'Alpha', true); \
         INSERT INTO \"folders\" (\"$space\", \"id\", \"rev\", \"name\", \"parent\", \"depth\", \"$path\") VALUES \
           ('{space}', 'fold_1', 1, 'root', NULL, 0, 'fold_1/'); \
         INSERT INTO \"orders\" (\"$space\", \"id\", \"rev\", \"vendor\", \"placed\", \"total\", \"note\", \"memo\", \"email\", \"email$bidx\") VALUES \
           ('{space}', 'ord_2', 3, 'vend_1', DATE '2026-02-02', 250, NULL, {m2}, {e2}, {b2}), \
           ('{space}', 'ord_1', 1, 'vend_1', DATE '2026-01-01', 1999, 'rush', {m1}, {e1}, {b1});",
        m1 = seal("orders", "memo", "ord_1", "first memo"),
        m2 = seal("orders", "memo", "ord_2", "second memo"),
        e1 = seal("orders", "email", "ord_1", "ana@example.com"),
        e2 = seal("orders", "email", "ord_2", "bo@example.com"),
        b1 = bidx("orders", "email", "ana@example.com"),
        b2 = bidx("orders", "email", "bo@example.com"),
    );
    pg.exec(&sql).expect("seed applies");
}

// ── the dynamism proof ───────────────────────────────────────────────────

/// **The requirement, as a test.**
///
/// Compile a program, export. Compile the same program plus a new entity
/// and a new field, export again. The new entity's JSONL must appear and
/// the new field must be in its rows — with **no change to any file under
/// `export.rs`** between the two halves of this function.
///
/// The assertion is deliberately two-sided. It is not enough that the new
/// things appear: everything else must be *unchanged*, member for member
/// and byte for byte, because an export that regenerated its whole shape
/// on every deploy would also pass a one-sided check while being useless
/// to anyone diffing two bundles.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_entity_and_a_new_field_reach_the_bundle_with_no_export_code() {
    let pg = scratch("ordo_exp_dynamic");
    let keys = keys();
    let space = SpaceId(SPACE.into());

    // ── before ──
    let before = artifact_of("v_before", BEFORE);
    deploy(&pg, &before);
    let pool = pool_for("ordo_exp_dynamic").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    let before_schema = schema_of(BEFORE);
    let mut b1 = MemoryBundle::new();
    let r1 = export_space(
        &pool,
        &request(&space, &before_schema, &before, &keys, SealedMode::Open),
        &mut b1,
    )
    .await
    .expect("the export runs");

    assert!(
        b1.get("data/shipments.jsonl").is_none(),
        "the entity does not exist yet"
    );
    let orders_before = lines(&b1, "data/orders.jsonl");
    assert!(
        !orders_before[0]
            .as_object()
            .expect("object")
            .contains_key("courier"),
        "the field does not exist yet"
    );

    // ── the deploy that adds an entity and a field ──
    let after = artifact_of("v_after", AFTER);
    pg.exec(
        "ALTER TABLE \"orders\" ADD COLUMN \"courier\" text NOT NULL DEFAULT 'none'; \
         CREATE TABLE \"shipments\" ( \
           \"$space\" text NOT NULL, \"id\" text NOT NULL, \"rev\" bigint NOT NULL DEFAULT 1, \
           \"order\" text NOT NULL, \"carrier\" text NOT NULL, \"weight\" bigint NOT NULL, \
           CONSTRAINT \"shipments$pk\" PRIMARY KEY (\"$space\", \"id\")); \
         ALTER TABLE \"shipments\" ADD CONSTRAINT \"shipments$order$fk\" \
           FOREIGN KEY (\"$space\", \"order\") REFERENCES \"orders\" (\"$space\", \"id\"); \
         CREATE INDEX \"shipments$order$ix\" ON \"shipments\" (\"$space\", \"order\");",
    )
    .expect("the migration applies");
    pg.exec(&format!(
        "INSERT INTO \"shipments\" (\"$space\", \"id\", \"rev\", \"order\", \"carrier\", \"weight\") \
         VALUES ('{SPACE}', 'shp_1', 1, 'ord_1', 'Post', 42); \
         UPDATE \"orders\" SET \"courier\" = 'Post' WHERE \"id\" = 'ord_1';"
    ))
    .expect("new rows");

    let after_schema = schema_of(AFTER);
    let mut b2 = MemoryBundle::new();
    let r2 = export_space(
        &pool,
        &request(&space, &after_schema, &after, &keys, SealedMode::Open),
        &mut b2,
    )
    .await
    .expect("the export runs against the changed program with no code change");

    // The new entity arrived, with its rows.
    let shipments = lines(&b2, "data/shipments.jsonl");
    assert_eq!(shipments.len(), 1, "the new entity's row is in the bundle");
    assert_eq!(shipments[0]["carrier"], serde_json::json!("Post"));
    assert_eq!(
        shipments[0]["weight"],
        serde_json::json!(42),
        "an `int` is a JSON integer, derived from the declared type"
    );
    assert_eq!(shipments[0]["order"], serde_json::json!("ord_1"));

    // The new field arrived, on the existing entity's rows.
    let orders_after = lines(&b2, "data/orders.jsonl");
    assert_eq!(
        orders_after
            .iter()
            .map(|o| o["courier"].clone())
            .collect::<Vec<_>>(),
        vec![serde_json::json!("Post"), serde_json::json!("none")],
        "every row of the existing entity carries the new field"
    );

    // And the other half: nothing else moved. `vendors` and `folders` were
    // not touched by the deploy, and their members are byte-identical.
    for member in ["data/vendors.jsonl", "data/folders.jsonl"] {
        assert_eq!(
            b1.get(member),
            b2.get(member),
            "`{member}` changed across a deploy that did not touch it"
        );
    }

    assert!(
        r1.entities.iter().all(|(n, _)| n != "shipments")
            && r2.entities.iter().any(|(n, _)| n == "shipments"),
        "the report enumerates entities from the schema it was given"
    );

    // The manifest describes the new shape without anybody writing it down.
    let m = manifest(&b2);
    let ships = m["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|e| e["entity"] == "shipments")
        .expect("the new entity is described");
    let names: Vec<&str> = ships["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|c| c["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["$space", "id", "rev", "order", "carrier", "weight"]);
}

// ── determinism ──────────────────────────────────────────────────────────

/// Two exports of one unchanged space are the same bytes, member for
/// member. Nothing in a bundle may depend on when it was taken.
#[tokio::test(flavor = "multi_thread")]
async fn two_exports_of_one_state_are_byte_identical() {
    let pg = scratch("ordo_exp_determinism");
    let keys = keys();
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_determinism").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());
    let mut a = MemoryBundle::new();
    let mut b = MemoryBundle::new();
    let req = request(&space, &schema, &art, &keys, SealedMode::Open);
    export_space(&pool, &req, &mut a).await.expect("export");
    export_space(&pool, &req, &mut b).await.expect("export");

    assert_eq!(a.names(), b.names());
    assert_eq!(
        a.members, b.members,
        "a bundle must be a pure function of the space's state and the artifact — \
         something in it is reading a clock, a hash seed, or an unordered iteration"
    );

    // And the ordering is stated rather than accidental: `vend_1` was
    // inserted second and must come first.
    let vendors = lines(&a, "data/vendors.jsonl");
    assert_eq!(
        vendors.iter().map(|v| v["id"].clone()).collect::<Vec<_>>(),
        vec![serde_json::json!("vend_1"), serde_json::json!("vend_2")],
    );
}

// ── the bundle digest (`plan-production-ops.md` R1a) ────────────────────

/// `ExportReport::manifest_sha256` is `sha256(manifest.json)`, and it is the
/// same string on every unchanged export — the one thing a restore marker
/// names to identify the bundle it points at.
#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_digest_is_the_manifests_own_and_is_stable() {
    let pg = scratch("ordo_exp_digest_stable");
    let keys = keys();
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_digest_stable").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());
    let req = request(&space, &schema, &art, &keys, SealedMode::Open);

    let mut a = MemoryBundle::new();
    let report_a = export_space(&pool, &req, &mut a).await.expect("export");
    let mut b = MemoryBundle::new();
    let report_b = export_space(&pool, &req, &mut b).await.expect("export");

    assert_eq!(
        report_a.manifest_sha256, report_b.manifest_sha256,
        "two exports of one unchanged space must report one digest"
    );
    assert_eq!(
        report_a.manifest_sha256.len(),
        64,
        "sha256 is 32 bytes of hex"
    );
    assert!(report_a
        .manifest_sha256
        .bytes()
        .all(|c| c.is_ascii_hexdigit()));

    let manifest_bytes = a.members.get("manifest.json").expect("a manifest").clone();
    assert_eq!(
        report_a.manifest_sha256,
        ordo_exec::export::bundle_digest(&manifest_bytes),
        "the report's digest must be the manifest's own bytes, hashed"
    );
}

/// The manifest's per-member digests are load-bearing: flip one byte
/// anywhere in any member and its recorded digest no longer matches, and
/// every member but the manifest itself is named.
#[tokio::test(flavor = "multi_thread")]
async fn a_flipped_byte_in_any_member_is_caught_by_the_manifest() {
    let pg = scratch("ordo_exp_digest_flip");
    let keys = keys();
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_digest_flip").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());
    let mut bundle = MemoryBundle::new();
    export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut bundle,
    )
    .await
    .expect("export");

    let m = manifest(&bundle);
    let digests = m["digests"].as_array().expect("digests");
    assert!(!digests.is_empty());
    let named: Vec<&str> = digests
        .iter()
        .filter_map(|d| d["member"].as_str())
        .collect();

    let members_except_manifest: Vec<&str> = bundle
        .names()
        .into_iter()
        .filter(|n| *n != "manifest.json")
        .collect();
    for name in &members_except_manifest {
        assert!(named.contains(name), "no digest for `{name}`");
    }
    assert_eq!(
        named.len(),
        members_except_manifest.len(),
        "the manifest must name every member except itself, no more and no fewer"
    );

    for d in digests {
        let name = d["member"].as_str().expect("a member name");
        let recorded = d["sha256"].as_str().expect("a hex digest");
        let mut bytes = bundle
            .members
            .get(name)
            .unwrap_or_else(|| panic!("`{name}` is in the manifest but not the bundle"))
            .clone();
        // An empty member has no byte to flip; appending one still changes
        // its digest, which is the property under test.
        if bytes.is_empty() {
            bytes.push(0xFF);
        } else {
            bytes[0] ^= 0xFF;
        }
        let flipped = ordo_exec::export::bundle_digest(&bytes);
        assert_ne!(
            flipped, recorded,
            "flipping a byte in `{name}` must change its digest"
        );
    }
}

// ── scoping, typing, and the members ─────────────────────────────────────

/// The bundle holds this space's rows and no other space's, the members
/// `migration.md` §6 names, and JSON types derived from the declarations.
#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_is_space_scoped_typed_and_complete() {
    let pg = scratch("ordo_exp_bundle");
    let keys = keys();
    keys.create_space(&SpaceId(OTHER.into()))
        .expect("neighbour");
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_bundle").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);
    seed(&pg, &keys, OTHER);

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());
    let mut bundle = MemoryBundle::new();
    let report = export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut bundle,
    )
    .await
    .expect("export");

    // Every member `migration.md` §6 promises, minus blob bytes, which the
    // manifest declares unbuilt rather than silently omitting.
    let mut names = bundle.names();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "artifact.json",
            "commandlog.jsonl",
            "data/folders.jsonl",
            // The compiler's platform entities are in the schema and so are
            // in the bundle. That is the dynamism working: the export ships
            // what the program has, and nothing here knew these existed.
            "data/groupMembers.jsonl",
            "data/members.jsonl",
            "data/orders.jsonl",
            "data/vendors.jsonl",
            "manifest.json",
            // The tenant's own access profile, empty here: a space that never
            // changed its profile exports an empty one rather than none.
            "profile.json",
            "schema.sql",
        ]
    );
    assert_eq!(
        bundle.get("schema.sql"),
        Some(art.ddl.as_str()),
        "schema.sql is the deploy's own bytes, not a re-rendering of them"
    );
    assert_eq!(bundle.get("artifact.json"), Some(art.artifact.as_str()));

    // Space-scoped: the neighbour has identical ids and none of them are
    // here.
    assert_eq!(
        report.rows(),
        5,
        "2 vendors + 1 folder + 2 orders; the platform entities are empty"
    );
    let orders = lines(&bundle, "data/orders.jsonl");
    assert!(orders.iter().all(|o| o["$space"] == SPACE));

    // Types come from the declarations, not from what Postgres reports.
    let first = &orders[0];
    assert_eq!(first["id"], serde_json::json!("ord_1"));
    assert_eq!(
        first["total"],
        serde_json::json!(1999),
        "money is minor units in a `bigint` column, and the bundle describes \
         itself the way its own schema.sql does"
    );
    assert_eq!(first["rev"], serde_json::json!(1), "an int is an integer");
    assert_eq!(first["placed"], serde_json::json!("2026-01-01"));
    assert_eq!(first["note"], serde_json::json!("rush"));
    assert_eq!(
        orders[1]["note"],
        serde_json::Value::Null,
        "a null is a null"
    );
    let vendors = lines(&bundle, "data/vendors.jsonl");
    assert_eq!(
        vendors[0]["active"],
        serde_json::json!(true),
        "a bool is a boolean, not the letter `t`"
    );
    assert_eq!(vendors[1]["active"], serde_json::json!(false));
    let folders = lines(&bundle, "data/folders.jsonl");
    assert_eq!(
        folders[0]["$path"],
        serde_json::json!("fold_1/"),
        "the tree's physical path travels, because schema.sql declares it NOT NULL"
    );

    let m = manifest(&bundle);
    assert_eq!(m["format"], ordo_exec::export::FORMAT);
    assert_eq!(m["space"], SPACE);
    assert_eq!(m["artifact"]["version"], "v1");
    // **`plan-blob.md`'s closing condition.** This assertion used to read
    // `m["unbuilt"][0]["what"] == "blob bytes"`, and the plan named it as
    // the proof: when B5 lands the assertion is *rewritten*, not deleted.
    //
    // The list still exists — it is the machine-readable place a future gap
    // gets declared — and it is now empty, because all three things the old
    // entry blamed (no blob field type, no object-store resolver, no inline
    // payload) exist.
    assert_eq!(
        m["unbuilt"],
        serde_json::json!([]),
        "the bundle declares nothing it cannot carry"
    );

    // Per-member digests, which `plan-production-ops.md` R1 found missing:
    // a restore marker cannot attest a bundle it cannot name.
    let digests = m["digests"].as_array().expect("digests");
    assert!(!digests.is_empty());
    let named: Vec<&str> = digests
        .iter()
        .filter_map(|d| d["member"].as_str())
        .collect();
    for required in ["schema.sql", "artifact.json", "commandlog.jsonl"] {
        assert!(
            named.contains(&required),
            "no digest for {required}: {named:?}"
        );
    }
    for d in digests {
        let sha = d["sha256"].as_str().expect("a hex digest");
        assert_eq!(sha.len(), 64, "sha256 is 32 bytes of hex");
        assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(d["bytes"].as_u64().is_some());
    }
    // The manifest cannot digest itself, and does not pretend to.
    assert!(
        !named.contains(&"manifest.json"),
        "a manifest that claimed its own digest would be claiming something          no reader could check"
    );

    // The blobs section says which absence this is.
    assert_eq!(m["blobs"]["carried"], false);
    assert_eq!(m["blobs"]["objects"], 0);
    assert_eq!(m["blobs"]["layout"], "blobs/<entity>/<field>/<id>");
}

// ── sealed fields ────────────────────────────────────────────────────────

/// `migration.md` §6: data is *"decrypted under the owner's authority
/// through the normal key-release path"*. And the blind index stays home.
#[tokio::test(flavor = "multi_thread")]
async fn sealed_values_open_and_their_digests_stay_behind() {
    let pg = scratch("ordo_exp_sealed");
    let keys = keys();
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_sealed").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());
    let mut bundle = MemoryBundle::new();
    export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut bundle,
    )
    .await
    .expect("export");

    let orders = lines(&bundle, "data/orders.jsonl");
    assert_eq!(orders[0]["memo"], serde_json::json!("first memo"));
    assert_eq!(orders[0]["email"], serde_json::json!("ana@example.com"));
    assert_eq!(orders[1]["memo"], serde_json::json!("second memo"));

    // The digest is not data. It is `HMAC(k_field, plaintext)` under this
    // space's key: it says nothing the plaintext beside it does not, it
    // cannot be recomputed under a destination space's key, and shipping
    // it would hand a reader an equality oracle over the values.
    assert!(
        !orders[0]
            .as_object()
            .expect("object")
            .contains_key("email$bidx"),
        "the blind index must not be in the bundle: {:?}",
        orders[0]
    );
    let m = manifest(&bundle);
    let o = m["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|e| e["entity"] == "orders")
        .expect("orders");
    assert_eq!(o["omitted"][0]["name"], "email$bidx");
    assert_eq!(o["omitted"][0]["reason"], "blind_index_derived");
    assert_eq!(m["sealed"], "opened");
}

/// The other half of §6's sentence: a deployment that cannot open a
/// space's envelopes says so and hands them over, rather than exporting
/// nulls that read as *this field was empty*.
#[tokio::test(flavor = "multi_thread")]
async fn ciphertext_mode_hands_over_envelopes_and_names_itself() {
    let pg = scratch("ordo_exp_ciphertext");
    let keys = keys();
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_ciphertext").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());
    let mut bundle = MemoryBundle::new();
    export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Ciphertext),
        &mut bundle,
    )
    .await
    .expect("export");

    let orders = lines(&bundle, "data/orders.jsonl");
    let memo = orders[0]["memo"].as_str().expect("hex");
    assert!(
        memo.len() > 80 && memo.chars().all(|c| c.is_ascii_hexdigit()),
        "an envelope, as hex: {memo}"
    );
    assert!(
        !memo.contains("first"),
        "ciphertext mode must not open anything"
    );
    // The envelope's first byte is the epoch — the format is frozen, and a
    // reader with the same keys can re-import from this.
    assert_eq!(&memo[..2], "01");
    assert_eq!(manifest(&bundle)["sealed"], "ciphertext");
}

/// A value the platform holds and cannot open is a refusal, not a null.
/// The bundle is leg 1 of the trust argument, and one that silently
/// under-delivers while looking complete is worse than one that says what
/// it cannot do.
#[tokio::test(flavor = "multi_thread")]
async fn an_unopenable_value_refuses_rather_than_exporting_a_null() {
    let pg = scratch("ordo_exp_unopenable");
    let keys = keys();
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_unopenable").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    // A value sealed under a *different* space's key, dropped into this
    // one's row — which is what a restore across spaces, or a destroyed
    // epoch, looks like from the reader's side.
    let alien = TestProvider::with_space("spc_alien");
    let alien_lease = alien
        .release(&SpaceId("spc_alien".into()), Purpose::Write, "x")
        .expect("release");
    let bytes = alien
        .seal_at(
            &alien_lease,
            "orders",
            "memo",
            ordo_crypto::Anchor::Row("ord_1"),
            b"not yours",
        )
        .expect("seal");
    pg.exec(&format!(
        "UPDATE \"orders\" SET \"memo\" = {} WHERE \"id\" = 'ord_1'",
        ordo_crypto::sealed::bytea_lit(&bytes)
    ))
    .expect("update");

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());
    let mut bundle = MemoryBundle::new();
    let err = export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut bundle,
    )
    .await
    .expect_err("an export promising plaintext must refuse");
    match err {
        ExportError::SealedUnopened {
            entity,
            field,
            row_id,
            ..
        } => {
            assert_eq!(
                (entity.as_str(), field.as_str(), row_id.as_str()),
                ("orders", "memo", "ord_1")
            );
        }
        other => panic!("wrong refusal: {other}"),
    }

    // Ciphertext mode is the honest answer for the same state, and it works.
    let mut bundle = MemoryBundle::new();
    export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Ciphertext),
        &mut bundle,
    )
    .await
    .expect("ciphertext mode discloses the state for what it is");
}

// ── the drift gate ───────────────────────────────────────────────────────

/// An export against a database that is not the shape its artifact asserts
/// refuses. The bundle ships `schema.sql` and the rows side by side; if
/// they disagree, the bundle is wrong in a way that still parses.
#[tokio::test(flavor = "multi_thread")]
async fn a_drifted_database_refuses_rather_than_exporting_the_wrong_shape() {
    let pg = scratch("ordo_exp_drift");
    let keys = keys();
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_drift").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());

    // A control: it exports cleanly before the drift, so the refusal below
    // is about the drift and not about the fixture.
    let mut ok = MemoryBundle::new();
    export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut ok,
    )
    .await
    .expect("the undrifted database exports");

    // Drift behind the artifact's back: a column the artifact declares,
    // removed. This is precisely the half-applied migration and the
    // restore-onto-an-older-shape case.
    pg.exec("ALTER TABLE \"orders\" DROP COLUMN \"note\"")
        .expect("drift");

    let mut bundle = MemoryBundle::new();
    let err = export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut bundle,
    )
    .await
    .expect_err("a drifted database must refuse");
    match err {
        ExportError::Drift { version, drift } => {
            assert_eq!(version, "v1");
            assert!(
                drift
                    .iter()
                    .any(|d| d.table == "orders" && d.column.as_deref() == Some("note")),
                "the refusal names the column: {drift:?}"
            );
        }
        other => panic!("wrong refusal: {other}"),
    }

    // A constraint drop drifts too — that is the half `plan-schema-evolution.md`
    // §1.1(b) added to the gate, and the export inherits it for free.
    pg.exec(
        "ALTER TABLE \"orders\" ADD COLUMN \"note\" text; \
             ALTER TABLE \"orders\" DROP CONSTRAINT \"orders$vendor$fk\"",
    )
    .expect("constraint drift");
    let mut bundle = MemoryBundle::new();
    let err = export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut bundle,
    )
    .await
    .expect_err("a dropped constraint is drift too");
    assert!(
        matches!(err, ExportError::Drift { ref drift, .. }
                 if drift.iter().any(|d| d.object.as_deref() == Some("orders$vendor$fk"))),
        "the constraint half of the gate reaches the export: {err}"
    );
}

// ── the round trip ───────────────────────────────────────────────────────

/// Export, build a fresh database from `schema.sql` alone, load the JSONL
/// back, and compare.
///
/// The point is that the bundle is **self-sufficient**: nothing outside it
/// is consulted, and the loader below knows no entity name either — it
/// reads the manifest for the column list and the JSONL for the values.
///
/// Sealed fields are deliberately outside the round trip and the manifest
/// says why: an opened value cannot go back into a `bytea` column without
/// being re-sealed under the *destination* space's key, and re-sealing is
/// `ordo import`'s job (`migration.md` §3), not export's. So this fixture
/// has no sealed field, and the sealed case is covered by the two tests
/// above plus the manifest's `sealed` and `omitted` declarations.
#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_reloads_into_a_fresh_database_from_its_own_schema_sql() {
    const OPEN_ONLY: &str = "\
entity vendors {
  name:   text
  active: bool = true
}
entity folders {
  name:   text
  parent -> folders?  tree
}
entity orders (key: ord) {
  vendor -> vendors
  placed: date
  total:  money
  note:   text?
}
";
    let pg = scratch("ordo_exp_roundtrip");
    let keys = keys();
    let art = artifact_of("v1", OPEN_ONLY);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_roundtrip").await;
    pool.ensure_platform_tables().await.expect("platform");
    pg.exec(&format!(
        "INSERT INTO \"vendors\" (\"$space\", \"id\", \"rev\", \"name\", \"active\") VALUES \
           ('{SPACE}', 'vend_1', 1, 'Alpha', true), ('{SPACE}', 'vend_2', 2, 'B''eta', false); \
         INSERT INTO \"folders\" (\"$space\", \"id\", \"rev\", \"name\", \"parent\", \"depth\", \"$path\") VALUES \
           ('{SPACE}', 'fold_1', 1, 'root', NULL, 0, 'fold_1/'); \
         INSERT INTO \"orders\" (\"$space\", \"id\", \"rev\", \"vendor\", \"placed\", \"total\", \"note\") VALUES \
           ('{SPACE}', 'ord_1', 1, 'vend_1', DATE '2026-01-01', 1999, 'rush'), \
           ('{SPACE}', 'ord_2', 1, 'vend_2', DATE '2026-02-02', -250, NULL);"
    ))
    .expect("seed");

    let schema = schema_of(OPEN_ONLY);
    let space = SpaceId(SPACE.into());
    let mut bundle = MemoryBundle::new();
    let report = export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut bundle,
    )
    .await
    .expect("export");

    // ── a fresh database, built from the bundle and nothing else ──
    let fresh = scratch("ordo_exp_roundtrip_dst");
    fresh
        .exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("reset");
    fresh
        .exec(bundle.get("schema.sql").expect("schema.sql"))
        .expect("the bundle's own schema.sql applies to an empty database");

    // The loader reads the manifest, not this test's knowledge of the
    // program. It is the shape an importer takes.
    let m = manifest(&bundle);
    assert!(
        m["load"]["cyclic"].as_array().expect("cyclic").is_empty(),
        "this fixture has no reference cycle"
    );
    let order: Vec<&str> = m["load"]["order"]
        .as_array()
        .expect("a load order")
        .iter()
        .map(|n| n.as_str().expect("a name"))
        .collect();
    assert!(
        order.iter().position(|n| *n == "vendors") < order.iter().position(|n| *n == "orders"),
        "the referenced entity must be named before the referencing one: {order:?}"
    );
    // The importer follows the manifest and nothing else — it does not know
    // that `orders` references `vendors`, only that the bundle said so.
    for name in &order {
        let entity = m["entities"]
            .as_array()
            .expect("entities")
            .iter()
            .find(|e| e["entity"] == *name)
            .expect("every entity in the load order is described");
        let cols: Vec<&str> = entity["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .map(|c| c["name"].as_str().expect("a name"))
            .collect();
        let file = entity["file"].as_str().expect("a file");
        let rows = lines(&bundle, file);
        if rows.is_empty() {
            continue;
        }
        let values: Vec<String> = rows
            .iter()
            .map(|r| {
                let cells: Vec<String> = cols
                    .iter()
                    .map(|c| match &r[*c] {
                        serde_json::Value::Null => "NULL".to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::String(s) => format!("'{}'", s.replace('\'', "''")),
                        other => panic!("a bundle holds no nested values: {other}"),
                    })
                    .collect();
                format!("({})", cells.join(", "))
            })
            .collect();
        let list: Vec<String> = cols.iter().map(|c| format!("\"{c}\"")).collect();
        fresh
            .exec(&format!(
                "INSERT INTO \"{name}\" ({}) VALUES {}",
                list.join(", "),
                values.join(", ")
            ))
            .unwrap_or_else(|e| panic!("`{name}` reloads: {e}"));
    }

    // ── counts, then values ──
    for (entity, n) in &report.entities {
        let got = fresh
            .query(&format!("SELECT count(*) FROM \"{entity}\""))
            .expect("count");
        assert_eq!(
            got[0][0].as_deref(),
            Some(n.to_string().as_str()),
            "`{entity}` did not reload with the row count the bundle reported"
        );
    }
    let spot = fresh
        .query(
            "SELECT \"id\", \"total\", \"placed\", \"note\", \"$space\" FROM \"orders\" \
             ORDER BY \"id\"",
        )
        .expect("spot check");
    assert_eq!(
        spot,
        vec![
            vec![
                Some("ord_1".into()),
                Some("1999".into()),
                Some("2026-01-01".into()),
                Some("rush".into()),
                Some(SPACE.into())
            ],
            vec![
                Some("ord_2".into()),
                Some("-250".into()),
                Some("2026-02-02".into()),
                None,
                Some(SPACE.into())
            ],
        ],
        "a negative money, a null, a date and the tenancy column all survive"
    );
    let quoted = fresh
        .query("SELECT \"name\" FROM \"vendors\" WHERE \"id\" = 'vend_2'")
        .expect("quote check");
    assert_eq!(quoted[0][0].as_deref(), Some("B'eta"));

    // The reference survived: the FK the fresh schema.sql created accepted
    // every row, which is only true if both sides of every ref landed.
    let refs = fresh
        .query("SELECT count(*) FROM \"orders\" o JOIN \"vendors\" v ON v.\"id\" = o.\"vendor\"")
        .expect("join");
    assert_eq!(refs[0][0].as_deref(), Some("2"));
}

// ── the chain ────────────────────────────────────────────────────────────

/// *"Command-log archive — the settled JSONL bundle format, chain hashes
/// included, so the history's integrity survives the platform."* The test
/// re-verifies the chain from the file alone.
#[tokio::test(flavor = "multi_thread")]
async fn the_command_log_travels_with_the_hashes_that_verify_it() {
    use ordo_commandlog::{Actor, Command, CommandGroup, CommandId, CommandKind, CorrelationId};

    let pg = scratch("ordo_exp_chain");
    let keys = keys();
    let art = artifact_of("v1", BEFORE);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_chain").await;
    pool.ensure_platform_tables().await.expect("platform");
    seed(&pg, &keys, SPACE);

    let store =
        ordo_exec::commandlog::PgStore::new(pool.clone(), tokio::runtime::Handle::current());
    let sid = ordo_commandlog::SpaceId(SPACE.into());
    for i in 0..3u64 {
        let mut after = ordo_commandlog::Image::new();
        after.insert("name".into(), ordo_commandlog::Value::Text(format!("v{i}")));
        after.insert("id".into(), ordo_commandlog::Value::Id(format!("vend_{i}")));
        store
            .try_append(&CommandGroup {
                correlation: CorrelationId(format!("cg_{i}")),
                actor: Actor::User(format!("usr_{i}")),
                commands: vec![Command {
                    id: CommandId(format!("cmd_{i}")),
                    space: sid.clone(),
                    entity: "vendors".into(),
                    row_id: format!("vend_{i}"),
                    kind: CommandKind::Create,
                    before: None,
                    after: Some(after),
                    at: ordo_commandlog::Timestamp(1000 + i),
                }],
                // The middle group is rule-6 owner administration, so the
                // exported file has to carry a flag and two lines that do
                // not. `plan-export.md` §2.3 promises a reader holding
                // nothing but `commandlog.jsonl` can recompute
                // `SHA-256(prev ‖ body)`; that promise is only kept if every
                // byte of the body is recoverable from the line, and the
                // flag is one of them.
                owner_administration: i == 1,
                restored_unverified: false,
                restore: None,
            })
            .await
            .expect("append");
    }
    // A neighbour's chain, so "space-scoped" has something to be wrong about.
    store
        .try_append(&CommandGroup {
            correlation: CorrelationId("cg_other".into()),
            actor: Actor::System("retention".into()),
            commands: vec![Command {
                id: CommandId("cmd_other".into()),
                space: ordo_commandlog::SpaceId(OTHER.into()),
                entity: "vendors".into(),
                row_id: "vend_9".into(),
                kind: CommandKind::Delete,
                before: Some(ordo_commandlog::Image::new()),
                after: None,
                at: ordo_commandlog::Timestamp(9),
            }],
            owner_administration: false,
            restored_unverified: false,
            restore: None,
        })
        .await
        .expect("append");

    let schema = schema_of(BEFORE);
    let space = SpaceId(SPACE.into());
    let mut bundle = MemoryBundle::new();
    let report = export_space(
        &pool,
        &request(&space, &schema, &art, &keys, SealedMode::Open),
        &mut bundle,
    )
    .await
    .expect("export");

    assert_eq!(report.log_groups, 3, "this space's chain, and no other's");
    let log = lines(&bundle, "commandlog.jsonl");
    assert_eq!(log.len(), 3);
    assert_eq!(log[0]["actor"], "user:usr_0");
    assert_eq!(log[0]["commands"][0]["kind"], "create");
    assert_eq!(log[0]["commands"][0]["after"]["name"], "v0");
    assert!(log.iter().all(|g| g["correlation"] != "cg_other"));

    // The rule-6 flag travels, by presence exactly as the chain encodes it:
    // the flagged group carries `ownerAdministration: true` and the others
    // carry no such key, so `grep` over an exported chain is the list of
    // every owner-administration command in it.
    assert_eq!(log[1]["ownerAdministration"], serde_json::json!(true));
    assert!(
        log[0].get("ownerAdministration").is_none() && log[2].get("ownerAdministration").is_none(),
        "absent means false, the same way silence means no trailer"
    );

    // The links verify from the file alone: every group's `prev` is the
    // previous group's `hash`, and the first one is genesis.
    assert_eq!(
        log[0]["prev"].as_str().expect("prev"),
        ordo_commandlog::ChainHash::GENESIS.to_string()
    );
    for pair in log.windows(2) {
        assert_eq!(
            pair[1]["prev"], pair[0]["hash"],
            "the exported chain does not link, so nothing downstream could verify it"
        );
    }
    // And `idx` is the chain position, not a line number that happens to
    // match: it is what a reader anchors a checkpoint against.
    assert_eq!(
        log.iter().map(|g| g["idx"].clone()).collect::<Vec<_>>(),
        vec![
            serde_json::json!(0),
            serde_json::json!(1),
            serde_json::json!(2)
        ]
    );
}

// ── the sink ─────────────────────────────────────────────────────────────

/// A bundle member is written once. The in-memory sink refuses a second
/// open, which is the same rule the directory sink gets from `create_new`.
#[test]
fn a_bundle_member_is_written_once() {
    let mut b = MemoryBundle::new();
    b.open("schema.sql").expect("first");
    b.write("CREATE TABLE x ();").expect("write");
    b.close().expect("close");
    assert!(b.open("schema.sql").is_err());
}

// ── the export reads through the resolver ────────────────────────────────

/// One entity with one external blob field. Small on purpose: what is
/// under test is **which store** the bytes come out of, and a wide schema
/// would only add columns nothing here reads.
const BLOBBED: &str = "\
entity docs (key: doc) {
  title: text
  scan:  blob?
}
";

/// A resolver that hands one named space its own store and every other
/// space the deployment's default.
///
/// It is the shape `AttachedStores` will have (`plan-tenant-storage.md`
/// §11 U5) with the table replaced by one entry, which is enough to make
/// the question this test asks answerable: a space with a bucket of its
/// own, and a deployment bucket beside it that its bundle must not come
/// from.
struct OneAttached {
    attached_to: String,
    own: std::sync::Arc<dyn ordo_blob::BlobStore>,
    default: std::sync::Arc<dyn ordo_blob::BlobStore>,
    asked: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ordo_blob::StoreResolver for OneAttached {
    async fn resolve(
        &self,
        space: &ordo_crypto::custody::SpaceId,
    ) -> Result<std::sync::Arc<dyn ordo_blob::BlobStore>, ordo_blob::BlobError> {
        self.asked.lock().expect("asked").push(space.0.clone());
        if space.0 == self.attached_to {
            Ok(std::sync::Arc::clone(&self.own))
        } else {
            Ok(std::sync::Arc::clone(&self.default))
        }
    }
}

/// **The export reads the space's own store, not the pipeline's.**
///
/// §8 B-E1: `write_blobs` was the second place that reached through the
/// pipeline to one store, and it is the one whose output somebody carries
/// away. An export that read the deployment's bucket for a tenant whose
/// bytes are in their own would hand that tenant a bundle full of another
/// deployment's documents, and the bundle would look complete.
///
/// The decoy is the whole of the method. The deployment's default store is
/// seeded with a **valid, openable object under the very same key**: the
/// same space, the same handle, sealed for the same entity and field. So
/// reading the wrong store does not fail, it *succeeds with the wrong
/// document*. A test that put nothing in the default store would pass
/// against a pipeline that read the default store and merely got a
/// `NotFound`, which is the same result a dozen unrelated bugs produce.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_reads_the_spaces_own_store() {
    let pg = scratch("ordo_exp_own_store");
    // **One provider, shared by the pipeline and the export.** Two
    // `TestProvider`s hold two sets of DEKs for one space, so a bundle
    // exported under the second cannot open envelopes sealed under the
    // first, which would fail this test for a reason that has nothing to
    // do with which store the bytes came from.
    let keys = std::sync::Arc::new(keys());
    let space = SpaceId(SPACE.into());
    let art = artifact_of("v_blob", BLOBBED);
    deploy(&pg, &art);
    let pool = pool_for("ordo_exp_own_store").await;
    pool.ensure_platform_tables().await.expect("platform");

    let root = std::env::temp_dir().join(format!("ordo-exp-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let own = ordo_blob::FsStore::shared(root.join("the-tenants-bucket")).expect("own");
    let default = ordo_blob::FsStore::shared(root.join("the-deployments-bucket")).expect("default");
    let resolver = std::sync::Arc::new(OneAttached {
        attached_to: SPACE.to_string(),
        own: std::sync::Arc::clone(&own),
        default: std::sync::Arc::clone(&default),
        asked: std::sync::Mutex::new(Vec::new()),
    });
    let pipe = ordo_blob::BlobPipeline::new(
        std::sync::Arc::clone(&resolver) as std::sync::Arc<dyn ordo_blob::StoreResolver>,
        std::sync::Arc::clone(&keys) as std::sync::Arc<dyn KeyProvider>,
        ordo_blob::BlobPolicy::DEFAULT,
    );

    let real = b"the tenant's own signed contract".to_vec();
    let decoy = vec![b'X'; real.len()];
    let upload = |len: usize| ordo_blob::Upload {
        space: &space,
        entity: "docs",
        field: "scan",
        mime: "application/pdf",
        plain_len: len as u64,
        correlation: "cg_export_blob",
        intent: None,
    };

    // The real object, into the tenant's own store.
    let stored = pipe
        .put(&upload(real.len()), ordo_blob::once(real.clone()))
        .await
        .expect("the tenant's upload");

    // The decoy: a second, genuinely sealed object for the same space,
    // entity and field, written into the **deployment's** store under the
    // real object's key. Sealed through a pipeline bound to that store, so
    // it is an object the platform would open without complaint.
    let decoy_pipe = ordo_blob::BlobPipeline::new(
        std::sync::Arc::new(ordo_blob::SingleStore::new(std::sync::Arc::clone(&default))),
        std::sync::Arc::clone(&keys) as std::sync::Arc<dyn KeyProvider>,
        ordo_blob::BlobPolicy::DEFAULT,
    );
    let planted = decoy_pipe
        .put(&upload(decoy.len()), ordo_blob::once(decoy.clone()))
        .await
        .expect("the decoy upload");
    let decoy_bytes = ordo_blob::collect(
        default
            .get(&space, planted.handle(), None)
            .await
            .expect("the decoy's stored bytes"),
    )
    .await
    .expect("the decoy's stored bytes");
    default
        .delete(&space, planted.handle())
        .await
        .expect("the decoy leaves no second key behind");
    default
        .put(
            &space,
            stored.handle(),
            decoy_bytes.len() as u64,
            ordo_blob::once(decoy_bytes.clone()),
        )
        .await
        .expect("the decoy, under the real object's key");

    // The row that names it, and the reference row the export walks.
    pg.exec(&format!(
        "INSERT INTO \"docs\" (\"$space\", \"id\", \"rev\", \"title\", \"scan\") \
         VALUES ('{SPACE}', 'doc_1', 1, 'a contract', '{}')",
        ordo_crypto::hex(stored.handle())
    ))
    .expect("the referring row");
    pool.rows(&ordo_crypto::blobref::insert_sql(
        &space,
        stored.handle(),
        stored.epoch(),
        "docs",
        "scan",
        stored.mime(),
        stored.size(),
        stored.integrity(),
        stored.bytes(),
        0,
        "",
    ))
    .await
    .expect("the reference row");

    let schema = schema_of(BLOBBED);
    let member = "blobs/docs/scan/doc_1";

    // Ciphertext mode hands over the object as stored, so the bundle's
    // bytes are exactly the tenant's own store's bytes and exactly not
    // the deployment store's.
    let mut bundle = MemoryBundle::new();
    let mut req = request(&space, &schema, &art, keys.as_ref(), SealedMode::Ciphertext);
    req.blobs = Some(&pipe);
    export_space(&pool, &req, &mut bundle)
        .await
        .expect("the ciphertext export");
    let carried = bundle
        .get_bytes(member)
        .unwrap_or_else(|| panic!("`{member}` is not in the bundle: {:?}", bundle.names()))
        .to_vec();
    let theirs = ordo_blob::collect(
        own.get(&space, stored.handle(), None)
            .await
            .expect("the tenant's own bytes"),
    )
    .await
    .expect("the tenant's own bytes");
    assert_eq!(
        carried, theirs,
        "the bundle carries the tenant's own object"
    );
    assert_ne!(
        carried, decoy_bytes,
        "the bundle carries the deployment's bucket, which is another tenant's document \
         wearing this one's key"
    );

    // Open mode goes through `BlobPipeline::get`, which resolves the same
    // space through the same resolver. The plaintext is the tenant's.
    let mut opened = MemoryBundle::new();
    let mut req = request(&space, &schema, &art, keys.as_ref(), SealedMode::Open);
    req.blobs = Some(&pipe);
    export_space(&pool, &req, &mut opened)
        .await
        .expect("the open export");
    assert_eq!(
        opened.get_bytes(member).expect("the opened member"),
        real.as_slice(),
    );
    assert_ne!(
        opened.get_bytes(member).expect("the opened member"),
        decoy.as_slice(),
    );

    // And it asked about the space the request named, never about
    // something it inferred. Twice per export: once for the as-stored read
    // (ciphertext) or the reference walk, and once inside `get`.
    let asked = resolver.asked.lock().expect("asked").clone();
    assert!(
        asked.iter().all(|s| s == SPACE),
        "the export resolved a space it was not asked about: {asked:?}"
    );

    // ── the manifest describes each object, and names its mime ───────────
    //
    // **The mime is the only fact about an object that is nowhere else in
    // the bundle.** It is a sealed envelope on the `$blobref` row, so an
    // importer can neither open it nor derive it from the bytes, and an
    // import that defaulted it would store a value this tenant never had.
    // The entity, the field and the id are here too, so a reader never has
    // to recover them by splitting the member's name.
    let files = |bundle: &MemoryBundle| -> serde_json::Value {
        let mf: serde_json::Value =
            serde_json::from_slice(bundle.get_bytes("manifest.json").expect("the manifest"))
                .expect("the manifest parses");
        mf["blobs"]["files"].clone()
    };
    assert_eq!(
        files(&opened),
        serde_json::json!([{
            "member": member,
            "entity": "docs",
            "field": "scan",
            "id": "doc_1",
            "mime": "application/pdf",
        }]),
        "an opened export's object is described in full: the member holds the plaintext, so \
         the mime describes what is in it"
    );
    // And in ciphertext mode the mime is **absent**, for both of the reasons
    // `ExportedObject::mime` gives: the caller asked for a bundle the
    // platform did not decrypt, and the mime is a sealed value like the
    // columns beside it.
    let cipher_files = files(&bundle);
    assert_eq!(
        cipher_files,
        serde_json::json!([{
            "member": member,
            "entity": "docs",
            "field": "scan",
            "id": "doc_1",
        }]),
        "a ciphertext bundle still says where each object belongs and does not disclose its \
         mime through the manifest after refusing to disclose it through the member"
    );
    assert!(
        cipher_files[0].get("mime").is_none(),
        "absent, rather than present and null: {cipher_files}"
    );

    std::fs::remove_dir_all(&root).ok();
}
