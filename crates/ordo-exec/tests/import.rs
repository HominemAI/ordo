//! Bulk import, against a real Postgres.
//!
//! Three properties, one test each, and each one is a property the design
//! rests on rather than a line of coverage:
//!
//! - **The round trip.** Export a space, import the bundle into an empty
//!   one, and the rows, the sealed values, the blind-index digests, the
//!   rewritten refs and the chain all agree. The destination is a
//!   **different space**, so its keys are different keys: a sealed value
//!   that opens there was re-sealed under the destination's epoch key and a
//!   digest that matches was re-derived under the destination's field key.
//!   An import that copied ciphertext or copied digests would fail this.
//!
//! - **The resume.** An import that refuses partway leaves the batches that
//!   committed committed, and re-running it under the same name skips them
//!   by their lease rather than writing them twice.
//!
//! - **The refusals.** What it must not guess.
//!
//! Databases are prefixed `ordo_imp_`, one per test function: test
//! functions in one binary run concurrently and each opens with
//! `DROP SCHEMA public CASCADE`.

use std::sync::Arc;
use std::time::Duration;

use ordo_conformance::{Fixture, Pg};
use ordo_crypto::{KeyProvider, Purpose, SpaceId, TestProvider};
use ordo_exec::export::{export_space, ExportRequest, MemoryBundle, SealedMode};
use ordo_exec::import::{import_space, ImportError, ImportRequest, MemorySource};
use ordo_exec::registry::Artifact;
use ordo_exec::{Executor, Pool, PoolConfig};
use ordo_schema::{build, ddl_for, Schema};
use ordo_syntax::parse_program;

const SRC_SPACE: &str = "spc_imp_src";
const DST_SPACE: &str = "spc_imp_dst";
const CALLER: &str = "mem_importer";

/// One awkward column kind of each: a ref, a tree self-reference, an
/// optional, a money, a bool with a default, a date, a sealed field and a
/// sealed **indexed** field. The same shape `tests/export.rs` exports, so
/// the two suites cover one bundle rather than two.
const PROGRAM: &str = "\
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

// ── scaffolding ──────────────────────────────────────────────────────────

fn schema_of(src: &str) -> Schema {
    build(&parse_program(src).expect("the fixture parses")).expect("the fixture builds")
}

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

fn deploy(pg: &Pg, art: &Artifact) {
    pg.exec("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .expect("schema reset");
    pg.exec(&art.ddl).expect("the generated DDL applies");
    pg.exec(ordo_crypto::SEALED_PAYLOAD_DDL)
        .expect("the platform's sealed payload store");
}

/// Rows for the source space, written as SQL because the source is a
/// *given* here: what is under test is what import does with the bundle
/// they produce, and `tests/export.rs` already covers the read side.
fn seed(pg: &Pg, keys: &TestProvider) {
    let space = SpaceId(SRC_SPACE.to_string());
    let lease = keys
        .release(&space, Purpose::Write, "seed")
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
                .blind_index(&space, entity, field, ordo_crypto::BlindValue::Text(v))
                .expect("digest"),
        )
    };
    pg.exec(&format!(
        "INSERT INTO \"vendors\" (\"$space\",\"id\",\"rev\",\"name\",\"active\") VALUES \
           ('{SRC_SPACE}','ven_1',1,'Acme',true), \
           ('{SRC_SPACE}','ven_2',1,'Beta \"quoted\" Co',false); \
         INSERT INTO \"folders\" \
           (\"$space\",\"id\",\"rev\",\"depth\",\"$path\",\"name\",\"parent\") VALUES \
           ('{SRC_SPACE}','fol_1',1,0,'fol_1/','root',NULL), \
           ('{SRC_SPACE}','fol_2',1,1,'fol_1/fol_2/','child','fol_1'), \
           ('{SRC_SPACE}','fol_3',1,2,'fol_1/fol_2/fol_3/','grandchild','fol_2'); \
         INSERT INTO \"orders\" \
           (\"$space\",\"id\",\"rev\",\"vendor\",\"placed\",\"total\",\"note\",\"memo\",\
            \"email\",\"email$bidx\") VALUES \
           ('{SRC_SPACE}','ord_1',1,'ven_1','2026-02-01',-2500,NULL,{m1},{e1},{b1}), \
           ('{SRC_SPACE}','ord_2',1,'ven_2','2026-03-04',19900,'a note',{m2},{e2},{b2}), \
           ('{SRC_SPACE}','ord_3',1,'ven_1','2026-03-05',1,'third',{m3},{e3},{b3});",
        m1 = seal("orders", "memo", "ord_1", "first memo"),
        e1 = seal("orders", "email", "ord_1", "a@example.com"),
        b1 = bidx("orders", "email", "a@example.com"),
        m2 = seal("orders", "memo", "ord_2", "second memo"),
        e2 = seal("orders", "email", "ord_2", "b@example.com"),
        b2 = bidx("orders", "email", "b@example.com"),
        m3 = seal("orders", "memo", "ord_3", "third memo"),
        e3 = seal("orders", "email", "ord_3", "c@example.com"),
        b3 = bidx("orders", "email", "c@example.com"),
    ))
    .expect("seed");
}

/// Export the seeded source space into a bundle.
async fn bundle_of(db: &str, art: &Artifact, keys: &TestProvider) -> MemoryBundle {
    let pg = scratch(db);
    deploy(&pg, art);
    seed(&pg, keys);
    let pool = pool_for(db).await;
    let schema = schema_of(&art.source);
    let space = SpaceId(SRC_SPACE.to_string());
    let mut out = MemoryBundle::new();
    export_space(
        &pool,
        &ExportRequest {
            space: &space,
            schema: &schema,
            artifact: art,
            keys,
            blobs: None,
            sealed: SealedMode::Open,
            correlation: "cg_import_test_export",
        },
        &mut out,
    )
    .await
    .expect("the source exports");
    out
}

/// An empty destination: the same DDL, the platform tables, no rows.
struct Dest {
    pool: Pool,
    exec: Executor,
    fixture: Arc<Fixture>,
}

async fn dest(db: &str, art: &Artifact, src: &'static str, keys: Arc<TestProvider>) -> Dest {
    let pg = scratch(db);
    deploy(&pg, art);
    let pool = pool_for(db).await;
    pool.ensure_platform_tables()
        .await
        .expect("platform tables");
    let mut fixture = Fixture::from_source(src, "")
        .expect("the fixture compiles")
        .in_space(DST_SPACE);
    fixture.keys = keys.clone();
    let fixture = Arc::new(fixture);
    let exec = Executor {
        pool: pool.clone(),
        schema: fixture.schema.clone(),
        registry: fixture.registry.clone(),
        keys: keys.clone(),
        space: SpaceId(DST_SPACE.to_string()),
    };
    Dest {
        pool,
        exec,
        fixture,
    }
}

impl Dest {
    fn request<'a>(&self, name: &'a str, art: &'a Artifact, batch: usize) -> ImportRequest<'a> {
        ImportRequest {
            people: &[],
            name,
            fixture: self.fixture.clone(),
            artifact: art,
            caller: CALLER,
            batch,
            // No blob door. `PROGRAM` has no blob field and its bundles
            // carry no objects, so this is the honest value for every test
            // over it; the blob leg's own tests below pass one.
            blobs: None,
        }
    }

    /// [`Dest::request`] with a blob door attached.
    fn request_with<'a>(
        &self,
        name: &'a str,
        art: &'a Artifact,
        batch: usize,
        door: &'a dyn ordo_exec::import::BlobDoor,
    ) -> ImportRequest<'a> {
        ImportRequest {
            people: &[],
            blobs: Some(door),
            ..self.request(name, art, batch)
        }
    }

    async fn one(&self, sql: &str) -> String {
        self.pool
            .rows(sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .unwrap_or_default()
    }

    async fn all(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        self.pool.rows(sql).await.expect("query")
    }

    /// Every command group on the destination's chain, in order.
    async fn chain(&self) -> Vec<ordo_commandlog::CommandGroup> {
        let rows = self
            .all(&format!(
                "SELECT \"body\" FROM \"$commandlog\" WHERE \"space\" = '{DST_SPACE}' \
                 ORDER BY \"idx\""
            ))
            .await;
        rows.into_iter()
            .map(|r| {
                let hex = r[0].clone().expect("a body");
                let body = ordo_crypto::sealed::unhex(&hex).expect("hex");
                ordo_commandlog::chain::decode_group(&body).expect("decodes")
            })
            .collect()
    }
}

// ── the round trip ───────────────────────────────────────────────────────

#[tokio::test]
async fn a_bundle_round_trips_into_an_empty_space_with_fresh_seals_and_one_group_per_batch() {
    let art = artifact_of("v1", PROGRAM);
    let src_keys = TestProvider::with_space(SRC_SPACE);
    let bundle = bundle_of("ordo_imp_rt_src", &art, &src_keys).await;

    let dst_keys = Arc::new(TestProvider::with_space(DST_SPACE));
    let d = dest("ordo_imp_rt_dst", &art, PROGRAM, dst_keys.clone()).await;
    let source = MemorySource::from(&bundle);

    // Batch of two over eight rows: 3 folders (2 batches), 2 vendors (1
    // batch), 3 orders (2 batches) = five batches, five groups.
    let report = import_space(&d.exec, &d.request("acme", &art, 2), &source)
        .await
        .expect("the bundle imports");

    assert_eq!(report.rows(), 8, "every row in the bundle landed");
    assert_eq!(
        report.entities,
        vec![
            ("folders".to_string(), 3),
            ("vendors".to_string(), 2),
            ("orders".to_string(), 3),
        ],
        "entities land in the manifest's load order: the two entities nothing references \
         first, by name, then `orders`, which references `vendors`"
    );
    assert_eq!(report.batches_written, 5);
    assert_eq!(report.batches_skipped, 0);

    // ── the chain: one group per batch, not one per row ─────────────────
    let chain = d.chain().await;
    assert_eq!(chain.len(), 5, "five batches are five chain entries");
    assert_eq!(
        chain.iter().map(|g| g.commands.len()).collect::<Vec<_>>(),
        vec![2, 1, 2, 2, 1],
        "each group holds its batch's rows: this is the whole point of the batch group"
    );
    assert_eq!(
        chain.iter().map(|g| g.commands.len()).sum::<usize>(),
        8,
        "and no row is missing from the log"
    );
    for g in &chain {
        assert!(
            g.commands
                .iter()
                .all(|c| c.kind == ordo_commandlog::CommandKind::Create),
            "an import creates; it never converges behind the operator's back"
        );
        assert_eq!(
            g.actor,
            ordo_commandlog::Actor::User(CALLER.into()),
            "attributed to the principal who ran the import"
        );
        assert!(!g.owner_administration, "an import is not rule-6 work");
        let ids: std::collections::BTreeSet<&str> =
            g.commands.iter().map(|c| c.id.0.as_str()).collect();
        assert_eq!(ids.len(), g.commands.len(), "command ids are distinct");
    }
    let correlations: std::collections::BTreeSet<&str> =
        chain.iter().map(|g| g.correlation.0.as_str()).collect();
    assert_eq!(correlations.len(), 5, "one correlation per batch");

    // ── the chain verifies from genesis ─────────────────────────────────
    let verified = ordo_exec::undo::verify(
        ordo_exec::undo::Db::Pool(&d.pool),
        &ordo_commandlog::SpaceId(DST_SPACE.into()),
        ordo_exec::undo::Anchor::Genesis,
        ordo_commandlog::ChainHash::GENESIS,
    )
    .await
    .expect("the imported chain verifies");
    assert_eq!(verified.checked, 5, "every group on the chain was checked");
    assert!(
        verified.discontinuities.is_empty(),
        "an imported chain has no gaps"
    );

    // ── the rows ────────────────────────────────────────────────────────
    assert_eq!(d.one("SELECT count(*) FROM \"vendors\"").await, "2");
    assert_eq!(d.one("SELECT count(*) FROM \"folders\"").await, "3");
    assert_eq!(d.one("SELECT count(*) FROM \"orders\"").await, "3");
    assert_eq!(
        d.one(&format!(
            "SELECT count(*) FROM \"orders\" WHERE \"$space\" <> '{DST_SPACE}'"
        ))
        .await,
        "0",
        "every imported row is stamped with the DESTINATION space"
    );
    // Ids are minted here, never the bundle's. The minted ids share the
    // entity's declared key prefix, so the assertion is against the SOURCE
    // ids by name rather than against the prefix they also carry.
    assert_eq!(
        d.one("SELECT count(*) FROM \"orders\" WHERE \"id\" IN ('ord_1','ord_2','ord_3')")
            .await,
        "0",
        "the source's ids did not survive: ids are server-minted, always"
    );
    assert_eq!(
        d.one("SELECT count(*) FROM \"vendors\" WHERE \"id\" IN ('ven_1','ven_2')")
            .await,
        "0",
        "and the same for a referenced entity, whose ids the refs had to be rewritten to"
    );
    assert_eq!(
        d.one("SELECT \"name\" FROM \"vendors\" WHERE \"active\" = false")
            .await,
        "Beta \"quoted\" Co",
        "an embedded quote survives the source literal it had to ride through"
    );
    assert_eq!(
        d.one("SELECT \"total\"::text FROM \"orders\" WHERE \"placed\" = '2026-02-01'")
            .await,
        "-2500",
        "a negative money in minor units"
    );
    assert_eq!(
        d.one("SELECT count(*) FROM \"orders\" WHERE \"note\" IS NULL")
            .await,
        "1",
        "a null stayed null rather than becoming an empty string"
    );

    // ── the refs were rewritten, not copied ─────────────────────────────
    let joined = d
        .all(
            "SELECT o.\"placed\"::text, v.\"name\" FROM \"orders\" o \
             JOIN \"vendors\" v ON v.\"id\" = o.\"vendor\" AND v.\"$space\" = o.\"$space\" \
             ORDER BY o.\"placed\"",
        )
        .await;
    let pairs: Vec<(String, String)> = joined
        .iter()
        .map(|r| {
            (
                r[0].clone().unwrap_or_default(),
                r[1].clone().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("2026-02-01".to_string(), "Acme".to_string()),
            ("2026-03-04".to_string(), "Beta \"quoted\" Co".to_string()),
            ("2026-03-05".to_string(), "Acme".to_string()),
        ],
        "each order points at the vendor it pointed at in the source, through minted ids"
    );
    // The tree's parents came before its children and the links survive.
    let tree = d
        .all(
            "SELECT c.\"name\", p.\"name\" FROM \"folders\" c \
             LEFT JOIN \"folders\" p ON p.\"id\" = c.\"parent\" AND p.\"$space\" = c.\"$space\" \
             ORDER BY c.\"name\"",
        )
        .await;
    let links: Vec<(String, String)> = tree
        .iter()
        .map(|r| {
            (
                r[0].clone().unwrap_or_default(),
                r[1].clone().unwrap_or_else(|| "-".into()),
            )
        })
        .collect();
    assert_eq!(
        links,
        vec![
            ("child".to_string(), "root".to_string()),
            ("grandchild".to_string(), "child".to_string()),
            ("root".to_string(), "-".to_string()),
        ],
        "a tree's rows were ordered parents-first inside their own file"
    );

    // ── the seals are the DESTINATION's ─────────────────────────────────
    let lease = dst_keys
        .release(&SpaceId(DST_SPACE.into()), Purpose::Read, "assert")
        .expect("a read lease on the destination");
    let memos = d
        .all("SELECT \"memo\"::text, \"placed\"::text, \"id\" FROM \"orders\" ORDER BY \"placed\"")
        .await;
    let opened: Vec<String> = memos
        .iter()
        .map(|r| {
            let bytes = ordo_crypto::sealed::unhex(r[0].as_deref().expect("a memo")).expect("hex");
            String::from_utf8(
                dst_keys
                    .open_at(
                        &lease,
                        "orders",
                        "memo",
                        ordo_crypto::Anchor::Row(r[2].as_deref().expect("an id")),
                        &bytes,
                    )
                    .expect("the destination's own key opens it"),
            )
            .expect("utf8")
        })
        .collect();
    assert_eq!(
        opened,
        vec!["first memo", "second memo", "third memo"],
        "sealed values were re-sealed under the destination's epoch key and open to the \
         plaintext the bundle carried"
    );
    // The source's envelopes cannot open here, which is what makes the
    // assertion above a real one rather than a copy that happened to work.
    let src_lease = src_keys
        .release(&SpaceId(SRC_SPACE.into()), Purpose::Read, "assert")
        .expect("a source lease");
    let first = ordo_crypto::sealed::unhex(memos[0][0].as_deref().unwrap()).expect("hex");
    assert!(
        src_keys
            .open_at(
                &src_lease,
                "orders",
                "memo",
                ordo_crypto::Anchor::Row(memos[0][2].as_deref().unwrap()),
                &first
            )
            .is_err(),
        "the destination's envelope is not the source's: the spaces hold different keys"
    );

    // ── the blind index was RE-DERIVED under the destination's key ──────
    let want = ordo_crypto::sealed::bytea_lit(
        &dst_keys
            .blind_index(
                &SpaceId(DST_SPACE.into()),
                "orders",
                "email",
                ordo_crypto::BlindValue::Text("b@example.com"),
            )
            .expect("digest"),
    );
    assert_eq!(
        d.one(&format!(
            "SELECT \"placed\"::text FROM \"orders\" WHERE \"email$bidx\" = {want}"
        ))
        .await,
        "2026-03-04",
        "an equality lookup under the DESTINATION's field key finds the row, which is only \
         true if the digest was re-derived rather than shipped"
    );
    let source_digest = ordo_crypto::sealed::bytea_lit(
        &src_keys
            .blind_index(
                &SpaceId(SRC_SPACE.into()),
                "orders",
                "email",
                ordo_crypto::BlindValue::Text("b@example.com"),
            )
            .expect("digest"),
    );
    assert_eq!(
        d.one(&format!(
            "SELECT count(*) FROM \"orders\" WHERE \"email$bidx\" = {source_digest}"
        ))
        .await,
        "0",
        "and the SOURCE's digest matches nothing here, so none was copied"
    );

    // ── the mapping is queryable, which is what it exists for ───────────
    assert_eq!(
        d.one("SELECT count(*) FROM \"$importmap\" WHERE \"import\" = 'acme'")
            .await,
        "8",
        "every imported row has an old-id to new-id row"
    );
    let mapped = d
        .one(
            "SELECT \"row_id\" FROM \"$importmap\" \
              WHERE \"import\" = 'acme' AND \"entity\" = 'vendors' AND \"source_id\" = 'ven_1'",
        )
        .await;
    assert_eq!(
        d.one(&format!(
            "SELECT \"name\" FROM \"vendors\" WHERE \"id\" = '{mapped}'"
        ))
        .await,
        "Acme",
        "the mapping answers `what did ven_1 become`"
    );
}

// ── the resume ───────────────────────────────────────────────────────────

/// A bundle whose last entity refuses, so the run stops with the earlier
/// entities committed. The refusal is an `orders` row naming a vendor the
/// bundle does not carry, which is `unresolved_ref`.
fn broken(bundle: &MemoryBundle) -> MemorySource {
    let mut src = MemorySource::from(bundle);
    let orders = String::from_utf8(src.members["data/orders.jsonl"].clone()).expect("utf8");
    let mut lines: Vec<String> = orders.lines().map(str::to_string).collect();
    // The LAST order, so the first orders batch commits and the second
    // does not: that is what makes this a resume rather than a retry.
    let last = lines.last_mut().expect("three orders");
    *last = last.replace("\"vendor\":\"ven_1\"", "\"vendor\":\"ven_missing\"");
    lines.push(String::new());
    src.put("data/orders.jsonl", lines.join("\n"));
    // The manifest's digest for the member no longer matches, and it must
    // not: the point of this fixture is a bundle whose rows refuse, not one
    // whose bytes are corrupt. So the digest is re-stated.
    let mut mf: serde_json::Value =
        serde_json::from_slice(&src.members["manifest.json"]).expect("manifest");
    let body = src.members["data/orders.jsonl"].clone();
    let sha = ordo_exec::export::bundle_digest(&body);
    for d in mf["digests"].as_array_mut().expect("digests") {
        if d["member"] == "data/orders.jsonl" {
            d["sha256"] = serde_json::Value::String(sha.clone());
        }
    }
    src.put(
        "manifest.json",
        serde_json::to_vec_pretty(&mf).expect("manifest re-renders"),
    );
    src
}

#[tokio::test]
async fn an_interrupted_import_resumes_without_writing_a_committed_batch_twice() {
    let art = artifact_of("v1", PROGRAM);
    let src_keys = TestProvider::with_space(SRC_SPACE);
    let bundle = bundle_of("ordo_imp_rs_src", &art, &src_keys).await;

    let dst_keys = Arc::new(TestProvider::with_space(DST_SPACE));
    let d = dest("ordo_imp_rs_dst", &art, PROGRAM, dst_keys.clone()).await;

    // ── the interrupted run ─────────────────────────────────────────────
    let err = import_space(&d.exec, &d.request("acme", &art, 2), &broken(&bundle))
        .await
        .expect_err("the bundle's last order names a vendor it does not carry");
    assert!(
        matches!(err, ImportError::RowRefused { .. }),
        "the protocol refused the row and the batch rolled back: {err}"
    );

    // What committed, committed: vendors, folders, and the FIRST orders
    // batch. What did not, did not.
    assert_eq!(d.one("SELECT count(*) FROM \"vendors\"").await, "2");
    assert_eq!(d.one("SELECT count(*) FROM \"folders\"").await, "3");
    assert_eq!(
        d.one("SELECT count(*) FROM \"orders\"").await,
        "2",
        "the refusing batch left nothing behind, not even its first row"
    );
    let groups_before = d.chain().await.len();
    assert_eq!(
        groups_before, 4,
        "four batches committed, four chain entries"
    );
    let vendor_ids_before = d
        .one(
            "SELECT string_agg(\"row_id\", ',' ORDER BY \"source_id\") FROM \"$importmap\" \
              WHERE \"entity\" = 'vendors'",
        )
        .await;

    // ── the resume, with the bundle corrected ───────────────────────────
    let report = import_space(
        &d.exec,
        &d.request("acme", &art, 2),
        &MemorySource::from(&bundle),
    )
    .await
    .expect("the corrected bundle imports");

    assert_eq!(
        report.batches_skipped, 4,
        "the four batches a previous run committed are found by their lease and skipped"
    );
    assert_eq!(
        report.batches_written, 1,
        "only the batch that never committed runs"
    );

    assert_eq!(
        d.one("SELECT count(*) FROM \"vendors\"").await,
        "2",
        "a resumed import does not write a committed batch twice"
    );
    assert_eq!(d.one("SELECT count(*) FROM \"folders\"").await, "3");
    assert_eq!(d.one("SELECT count(*) FROM \"orders\"").await, "3");
    assert_eq!(
        d.one("SELECT count(*) FROM \"$importmap\"").await,
        "8",
        "and the mapping holds one row per source row, not two"
    );
    assert_eq!(
        d.one(
            "SELECT string_agg(\"row_id\", ',' ORDER BY \"source_id\") FROM \"$importmap\" \
               WHERE \"entity\" = 'vendors'"
        )
        .await,
        vendor_ids_before,
        "the ids the first run minted are the ids the second run's refs resolve to — which is \
         why the mapping is re-read from the table rather than remembered in the process"
    );
    assert_eq!(
        d.chain().await.len(),
        5,
        "the chain grew by exactly the one batch that ran"
    );
    assert_eq!(
        d.one(&format!(
            "SELECT count(*) FROM \"orders\" o JOIN \"vendors\" v \
             ON v.\"id\" = o.\"vendor\" AND v.\"$space\" = o.\"$space\" \
             WHERE o.\"$space\" = '{DST_SPACE}'"
        ))
        .await,
        "3",
        "every order's ref resolves to a vendor the FIRST run minted"
    );

    // A third run over the same bundle under the same name is a complete
    // no-op: every batch is found by its lease. That is what makes
    // re-running safe rather than merely survivable, and it is why the
    // emptiness rule is stated against this import's own mapping instead
    // of against the table being empty.
    let again = import_space(
        &d.exec,
        &d.request("acme", &art, 2),
        &MemorySource::from(&bundle),
    )
    .await
    .expect("a completed import re-runs over its own rows");
    assert_eq!(again.batches_written, 0, "nothing is written a second time");
    assert_eq!(
        again.batches_skipped, 5,
        "every batch is answered by its lease"
    );
    assert_eq!(d.one("SELECT count(*) FROM \"orders\"").await, "3");
    assert_eq!(d.chain().await.len(), 5, "and the chain did not grow");

    // A DIFFERENT import into the space this one filled is the merge
    // nobody has ruled a policy for, and it refuses.
    let other = import_space(
        &d.exec,
        &d.request("second", &art, 2),
        &MemorySource::from(&bundle),
    )
    .await;
    assert!(
        matches!(&other, Err(ImportError::SpaceNotEmpty { .. })),
        "rows this import did not write are somebody else's: {other:?}"
    );
}

// ── the refusals ─────────────────────────────────────────────────────────

#[tokio::test]
async fn it_refuses_rather_than_guessing() {
    let art = artifact_of("v1", PROGRAM);
    let src_keys = TestProvider::with_space(SRC_SPACE);
    let bundle = bundle_of("ordo_imp_rf_src", &art, &src_keys).await;

    let dst_keys = Arc::new(TestProvider::with_space(DST_SPACE));
    let d = dest("ordo_imp_rf_dst", &art, PROGRAM, dst_keys.clone()).await;

    // ── a bundle from a format nobody wrote a reader for ────────────────
    let mut wrong_format = MemorySource::from(&bundle);
    let mut mf: serde_json::Value =
        serde_json::from_slice(&wrong_format.members["manifest.json"]).unwrap();
    mf["format"] = serde_json::Value::String("ordo.space-export.v99".into());
    wrong_format.put("manifest.json", serde_json::to_vec(&mf).unwrap());
    assert!(
        matches!(
            import_space(&d.exec, &d.request("f", &art, 2), &wrong_format).await,
            Err(ImportError::FormatUnknown { .. })
        ),
        "an unknown format is a refusal, not a best effort"
    );

    // ── rows shaped by another schema ───────────────────────────────────
    let other = artifact_of("v2", PROGRAM);
    assert!(
        matches!(
            import_space(
                &d.exec,
                &d.request("a", &other, 2),
                &MemorySource::from(&bundle)
            )
            .await,
            Err(ImportError::ArtifactMismatch { .. })
        ),
        "the bundle's artifact must be the deployed one"
    );

    // ── a truncated member ──────────────────────────────────────────────
    let mut truncated = MemorySource::from(&bundle);
    let body = String::from_utf8(truncated.members["data/orders.jsonl"].clone()).unwrap();
    truncated.put(
        "data/orders.jsonl",
        body.lines().take(1).collect::<Vec<_>>().join("\n"),
    );
    let e = import_space(&d.exec, &d.request("t", &art, 2), &truncated)
        .await
        .expect_err("a member whose bytes are not the manifest's bytes");
    assert!(
        matches!(e, ImportError::BundleIncomplete { .. }),
        "a short file is caught by the digest the manifest recorded, not discovered at \
         cutover: {e}"
    );

    // ── ciphertext mode ─────────────────────────────────────────────────
    let mut cipher = MemorySource::from(&bundle);
    let mut mf: serde_json::Value =
        serde_json::from_slice(&cipher.members["manifest.json"]).unwrap();
    mf["sealed"] = serde_json::Value::String("ciphertext".into());
    cipher.put("manifest.json", serde_json::to_vec(&mf).unwrap());
    assert!(
        matches!(
            import_space(&d.exec, &d.request("c", &art, 2), &cipher).await,
            Err(ImportError::SealedCiphertext)
        ),
        "envelopes under the source's key are refused rather than re-keyed by guess"
    );

    // ── a field the schema does not have ────────────────────────────────
    let mut extra = MemorySource::from(&bundle);
    let mut mf: serde_json::Value =
        serde_json::from_slice(&extra.members["manifest.json"]).unwrap();
    for e in mf["entities"].as_array_mut().unwrap() {
        if e["entity"] == "vendors" {
            e["columns"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "name": "nickname", "encoding": "text"
                }));
        }
    }
    extra.put("manifest.json", serde_json::to_vec(&mf).unwrap());
    let e = import_space(&d.exec, &d.request("x", &art, 2), &extra)
        .await
        .expect_err("a column with nowhere to go");
    assert!(
        matches!(e, ImportError::FieldMismatch { .. }),
        "a column the schema does not declare is a refusal, not a dropped value: {e}"
    );

    // Nothing above wrote anything: every check runs before the first row.
    assert_eq!(d.one("SELECT count(*) FROM \"vendors\"").await, "0");
    assert_eq!(d.chain().await.len(), 0);

    // ── and then a real import, into a space that is no longer empty ────
    import_space(
        &d.exec,
        &d.request("ok", &art, 8),
        &MemorySource::from(&bundle),
    )
    .await
    .expect("the untouched bundle imports");
    let e = import_space(
        &d.exec,
        &d.request("second", &art, 8),
        &MemorySource::from(&bundle),
    )
    .await
    .expect_err("a second import into a populated space");
    assert!(
        matches!(e, ImportError::SpaceNotEmpty { .. }),
        "merging into a populated space needs a conflict policy nobody has ruled: {e}"
    );
}

// ── the blob leg ─────────────────────────────────────────────────────────
//
// The rows above cannot carry a file. `Sema::check_content_params` refuses a
// blob-typed op parameter, so `import.<entity>.create` names no blob field,
// and the bundle's blob column holds the **source** space's handle: an object
// in the source's bucket, sealed under the source's key. The objects ride the
// bundle as their own members and are bound to their rows afterwards, through
// the deployment's blob door.
//
// **What this file tests, and what it deliberately leaves to another.** The
// door is `ordo_host::blob::PlatformBlobs`, three crates above this one, and
// there is exactly one of it by ruling (`docs/plan-blob-chain.md` C1–C8,
// asserted over the workspace's own source by
// `crates/ordo-host/tests/upload_seam.rs`). So the real round trip — bytes
// out, bytes back, digests equal, the digest in the chain — lives where the
// real door is, in `crates/ordo-host/tests/import_blobs.rs`. What lives here
// is import's own half: which bytes it puts on which row, which refusals it
// makes before a byte moves, and that the row id it hands the door is the
// **minted** one rather than the id the bundle carried. The door below
// records what it was handed and stores nothing, which is what makes those
// assertions possible without a bucket.

/// One entity with a blob field in each of the four tiers. `scan` is the
/// only one anything can serve; the other three are declarable and refused,
/// each for its own reason, and this schema is how those refusals are
/// reached.
const BLOBBED: &str = "\
entity docs (key: doc) {
  title:  text
  scan:   blob?
  photo:  blob? inline
  secret: blob? private
  vault:  blob? sealed
}
";

/// A door that records what it was handed and stores nothing.
///
/// Not a second upload path: it implements `BlobDoor`, which is the seam
/// import drives, and not `PlatformValue`, which is the authority that writes
/// a blob column. It writes no column, no `$blobref` row and no command
/// group — which is why the test that needs those uses the real door.
/// What the spy records of one call: entity, field, row id, mime, bytes.
type Attached = (String, String, String, String, Vec<u8>);

#[derive(Default)]
struct SpyDoor {
    seen: std::sync::Mutex<Vec<Attached>>,
}

#[async_trait::async_trait]
impl ordo_exec::import::BlobDoor for SpyDoor {
    async fn attach(&self, o: ordo_exec::import::ImportObject<'_>) -> Result<(), String> {
        self.seen.lock().expect("the spy").push((
            o.entity.to_string(),
            o.field.to_string(),
            o.row_id.to_string(),
            o.mime.to_string(),
            o.bytes,
        ));
        Ok(())
    }
}

impl SpyDoor {
    fn seen(&self) -> Vec<Attached> {
        self.seen.lock().expect("the spy").clone()
    }
}

/// A door that refuses the way the real one refuses: with its own error
/// class at the front of the message, which is how `ordo_host::classify`
/// hands a write-protocol refusal back.
struct ClosedDoor;

#[async_trait::async_trait]
impl ordo_exec::import::BlobDoor for ClosedDoor {
    async fn attach(&self, o: ordo_exec::import::ImportObject<'_>) -> Result<(), String> {
        Err(format!(
            "access_guard: `{}` may not write `{}` ({})",
            o.principal, o.row_id, o.entity
        ))
    }
}

/// A `BLOBBED` bundle whose source space holds `rows` and no objects. The
/// objects are spliced on by [`put_object`], because what is under test is
/// what import does with a bundle that carries them.
async fn blobbed_bundle(
    db: &str,
    art: &Artifact,
    keys: &TestProvider,
    rows: &[(&str, &str)],
) -> MemoryBundle {
    let pg = scratch(db);
    deploy(&pg, art);
    for (id, title) in rows {
        pg.exec(&format!(
            "INSERT INTO \"docs\" (\"$space\",\"id\",\"rev\",\"title\") \
             VALUES ('{SRC_SPACE}','{id}',1,'{title}')"
        ))
        .expect("seed");
    }
    let pool = pool_for(db).await;
    let mut out = MemoryBundle::new();
    export_space(
        &pool,
        &ExportRequest {
            space: &SpaceId(SRC_SPACE.to_string()),
            schema: &schema_of(&art.source),
            artifact: art,
            keys,
            // No pipeline, so the export writes no objects and says so
            // (`blobs.carried: false`). Every object below is spliced in.
            blobs: None,
            sealed: SealedMode::Open,
            correlation: "cg_import_test_export",
        },
        &mut out,
    )
    .await
    .expect("the source exports");
    out
}

fn edit_manifest(src: &mut MemorySource, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut mf: serde_json::Value =
        serde_json::from_slice(&src.members["manifest.json"]).expect("the manifest parses");
    edit(&mut mf);
    src.put("manifest.json", serde_json::to_vec(&mf).unwrap());
}

/// Splice one object into a bundle **exactly as the export writes one**: the
/// member, its `sha256` in `digests`, and an entry in `blobs.files`.
///
/// All three, because a test that assembled only the member would be
/// asserting about a bundle nothing produces, and the refusals below are
/// about a bundle that is one part wrong rather than three parts absent.
fn put_object(
    src: &mut MemorySource,
    entity: &str,
    field: &str,
    id: &str,
    mime: Option<&str>,
    bytes: &[u8],
) {
    let member = format!("blobs/{entity}/{field}/{id}");
    src.put(&member, bytes.to_vec());
    let digest = ordo_exec::export::bundle_digest(bytes);
    edit_manifest(src, |mf| {
        mf["digests"]
            .as_array_mut()
            .expect("digests is an array")
            .push(serde_json::json!({
                "member": member, "bytes": bytes.len(), "sha256": digest,
            }));
        let mut entry = serde_json::json!({
            "member": member, "entity": entity, "field": field, "id": id,
        });
        if let Some(mime) = mime {
            entry["mime"] = serde_json::json!(mime);
        }
        mf["blobs"]["carried"] = serde_json::json!(true);
        let files = mf["blobs"]["files"]
            .as_array_mut()
            .expect("blobs.files is an array");
        files.push(entry);
        let n = files.len();
        mf["blobs"]["objects"] = serde_json::json!(n);
    });
}

/// **Every refusal the blob leg makes before a byte moves.**
///
/// All of them are pre-flight, which is the property this test is really
/// about: an object whose field does not exist, or whose tier nothing can
/// serve, is refused before the first row is written rather than after the
/// millionth. The assertion at the end is what makes that claim checkable.
#[tokio::test]
async fn an_object_the_import_cannot_place_is_refused_before_a_row_is_written() {
    let art = artifact_of("vb", BLOBBED);
    let src_keys = TestProvider::with_space(SRC_SPACE);
    let bundle = blobbed_bundle(
        "ordo_imp_blob_pre_src",
        &art,
        &src_keys,
        &[("doc_1", "one")],
    )
    .await;

    let dst_keys = Arc::new(TestProvider::with_space(DST_SPACE));
    let d = dest("ordo_imp_blob_pre_dst", &art, BLOBBED, dst_keys.clone()).await;
    let spy = SpyDoor::default();

    // ── objects, and no door to put them through ────────────────────────
    let mut no_door = MemorySource::from(&bundle);
    put_object(
        &mut no_door,
        "docs",
        "scan",
        "doc_1",
        Some("image/png"),
        b"P",
    );
    let e = import_space(&d.exec, &d.request("nodoor", &art, 8), &no_door)
        .await
        .expect_err("objects with no door");
    assert!(
        matches!(e, ImportError::BlobDoorMissing { objects: 1 }),
        "a deployment with no blob tier must refuse the bundle rather than load the rows \
         and leave every attachment resolving to nothing: {e}"
    );

    // ── a member the manifest does not describe ──────────────────────────
    let mut stray = MemorySource::from(&bundle);
    stray.put("blobs/docs/scan/doc_1", b"P".to_vec());
    let e = import_space(&d.exec, &d.request_with("stray", &art, 8, &spy), &stray)
        .await
        .expect_err("a member nothing describes");
    assert!(
        matches!(&e, ImportError::BundleIncomplete { member, .. } if member == "blobs/docs/scan/doc_1"),
        "a bundle holding an object the manifest does not describe is one nothing can say \
         which field of which row it belongs to: {e}"
    );

    // ── an object the manifest describes and the bundle does not hold ────
    let mut absent = MemorySource::from(&bundle);
    put_object(
        &mut absent,
        "docs",
        "scan",
        "doc_1",
        Some("image/png"),
        b"P",
    );
    absent.members.remove("blobs/docs/scan/doc_1");
    let e = import_space(&d.exec, &d.request_with("absent", &art, 8, &spy), &absent)
        .await
        .expect_err("a described object with no member");
    assert!(
        matches!(e, ImportError::BundleIncomplete { .. }),
        "the manifest and the member list are checked against each other in both \
         directions: {e}"
    );

    // ── the three tiers the door cannot serve ───────────────────────────
    for (field, tier) in [
        ("photo", "inline"),
        ("secret", "private"),
        ("vault", "sealed"),
    ] {
        let mut tiered = MemorySource::from(&bundle);
        put_object(&mut tiered, "docs", field, "doc_1", Some("image/png"), b"P");
        let e = import_space(&d.exec, &d.request_with(tier, &art, 8, &spy), &tiered)
            .await
            .unwrap_err();
        assert!(
            matches!(&e, ImportError::BlobTierUnsupported { tier: t, field: f, .. }
                if *t == tier && f == field),
            "`blob {tier}` is declarable and not servable, and is refused by name on the way \
             in rather than discovered one upload later: {e}"
        );
    }

    // ── a field that is not a blob field of this schema ──────────────────
    let mut wrong_field = MemorySource::from(&bundle);
    put_object(
        &mut wrong_field,
        "docs",
        "title",
        "doc_1",
        Some("image/png"),
        b"P",
    );
    let e = import_space(&d.exec, &d.request_with("wf", &art, 8, &spy), &wrong_field)
        .await
        .unwrap_err();
    assert!(
        matches!(e, ImportError::BlobUnplaced { .. }),
        "an object naming a column that is not a blob field has nowhere to go: {e}"
    );

    // ── no mime, which is nowhere else in the bundle ─────────────────────
    let mut no_mime = MemorySource::from(&bundle);
    put_object(&mut no_mime, "docs", "scan", "doc_1", None, b"P");
    let e = import_space(&d.exec, &d.request_with("nm", &art, 8, &spy), &no_mime)
        .await
        .unwrap_err();
    assert!(
        matches!(e, ImportError::BundleIncomplete { .. }),
        "the mime is a sealed envelope on the source's reference row, so it is in the \
         manifest or it is nowhere, and `application/octet-stream` is a value the customer \
         never stored: {e}"
    );

    // ── no recorded digest, so nothing to check the bytes against ────────
    let mut no_digest = MemorySource::from(&bundle);
    put_object(
        &mut no_digest,
        "docs",
        "scan",
        "doc_1",
        Some("image/png"),
        b"P",
    );
    edit_manifest(&mut no_digest, |mf| {
        mf["digests"]
            .as_array_mut()
            .unwrap()
            .retain(|d| d["member"] != "blobs/docs/scan/doc_1");
    });
    let e = import_space(&d.exec, &d.request_with("nd", &art, 8, &spy), &no_digest)
        .await
        .unwrap_err();
    assert!(
        matches!(e, ImportError::BundleIncomplete { .. }),
        "an object has no row count and no second description, so the manifest's `sha256` is \
         the only thing that says these are its bytes: {e}"
    );

    // ── the manifest disagreeing with itself ─────────────────────────────
    let mut miscounted = MemorySource::from(&bundle);
    put_object(
        &mut miscounted,
        "docs",
        "scan",
        "doc_1",
        Some("image/png"),
        b"P",
    );
    edit_manifest(&mut miscounted, |mf| {
        mf["blobs"]["objects"] = serde_json::json!(7);
    });
    let e = import_space(&d.exec, &d.request_with("mc", &art, 8, &spy), &miscounted)
        .await
        .unwrap_err();
    assert!(
        matches!(e, ImportError::BundleIncomplete { .. }),
        "`blobs.objects` and `blobs.files` are two answers to one question: {e}"
    );

    // **Nothing above wrote anything.** Every one of those refusals is a
    // pre-flight check, and a build that moved one of them after the rows
    // would leave a space that is neither empty nor imported.
    assert_eq!(d.one("SELECT count(*) FROM \"docs\"").await, "0");
    assert_eq!(d.chain().await.len(), 0);
    assert!(
        spy.seen().is_empty(),
        "no object reached the door: {:?}",
        spy.seen()
    );
}

/// **An object is handed to the door against the row this import minted, and
/// a second run does not attach it twice.**
///
/// The minted id is the whole of the first half. The bundle names the
/// source's `doc_1`, the destination mints an id of its own, and an import
/// that handed the door the id it read would attach the file to a row in
/// another space's numbering — which here would be no row at all, and in a
/// destination that happened to hold that id would be the wrong row's file.
///
/// The second half is resume. The marker is the row's own column, which is
/// what the attach produced in the transaction that produced it, so the
/// column is set here the way the door's converge sets it and the second run
/// is expected to skip.
#[tokio::test]
async fn an_object_lands_on_the_minted_row_and_is_not_attached_twice() {
    let art = artifact_of("vb", BLOBBED);
    let src_keys = TestProvider::with_space(SRC_SPACE);
    let bundle = blobbed_bundle(
        "ordo_imp_blob_one_src",
        &art,
        &src_keys,
        &[("doc_1", "one"), ("doc_2", "two")],
    )
    .await;

    let dst_keys = Arc::new(TestProvider::with_space(DST_SPACE));
    let d = dest("ordo_imp_blob_one_dst", &art, BLOBBED, dst_keys.clone()).await;
    let spy = SpyDoor::default();

    let mut source = MemorySource::from(&bundle);
    put_object(
        &mut source,
        "docs",
        "scan",
        "doc_2",
        Some("application/pdf"),
        b"a signed contract",
    );

    let report = import_space(&d.exec, &d.request_with("acme", &art, 8, &spy), &source)
        .await
        .expect("the bundle imports");
    assert_eq!(report.objects_attached, 1);
    assert_eq!(report.objects_skipped, 0);
    assert_eq!(
        report.groups, report.batches_written,
        "an attach is its own group, appended by the door, and is not counted as a batch's"
    );

    // The id the destination minted for the source's `doc_2`, read from
    // `$importmap` rather than guessed from the shape of an id.
    let minted = d
        .one(&format!(
            "SELECT \"row_id\" FROM \"$importmap\" WHERE \"$space\" = '{DST_SPACE}' \
             AND \"import\" = 'acme' AND \"entity\" = 'docs' AND \"source_id\" = 'doc_2'"
        ))
        .await;
    assert!(!minted.is_empty(), "the row was mapped");
    assert_ne!(
        minted, "doc_2",
        "the destination mints its own ids, so a test comparing against the source's would \
         pass for the wrong reason"
    );
    assert_eq!(
        spy.seen(),
        vec![(
            "docs".to_string(),
            "scan".to_string(),
            minted.clone(),
            "application/pdf".to_string(),
            b"a signed contract".to_vec(),
        )],
        "the door is handed the minted row, the field, the source's own mime, and the \
         member's exact bytes"
    );

    // ── the resume ──────────────────────────────────────────────────────
    //
    // The spy wrote no column, because it is not the write protocol. Setting
    // it here is what the door's converge would have done, and it is the
    // state the skip below is supposed to read.
    d.pool
        .rows(&format!(
            "UPDATE \"docs\" SET \"scan\" = '{}' WHERE \"$space\" = '{DST_SPACE}' \
             AND \"id\" = '{minted}'",
            "a1b2c3d4e5f60718293a4b5c6d7e8f90"
        ))
        .await
        .expect("stand in for the door's converge");

    let again = import_space(&d.exec, &d.request_with("acme", &art, 8, &spy), &source)
        .await
        .expect("the same import re-runs");
    assert_eq!(
        (again.objects_attached, again.objects_skipped),
        (0, 1),
        "an interrupted import that already attached an object must not attach it again: a \
         second upload is a second object, a second `rev` and a second group for one file"
    );
    assert_eq!(
        spy.seen().len(),
        1,
        "and the door was not called a second time"
    );
}

/// **An object that cannot be placed, and a door that refuses.**
///
/// Both of these are refusals *after* the rows are written, which is the
/// other half of the ordering: an object is bound to a row that exists, so
/// the placement cannot be checked any earlier than this. Each call resumes
/// the same import name over its own rows, which is what lets one
/// destination serve all three.
#[tokio::test]
async fn an_object_with_no_row_is_refused_and_so_is_one_the_door_declines() {
    let art = artifact_of("vb", BLOBBED);
    let src_keys = TestProvider::with_space(SRC_SPACE);
    let bundle = blobbed_bundle(
        "ordo_imp_blob_post_src",
        &art,
        &src_keys,
        &[("doc_1", "one")],
    )
    .await;

    let dst_keys = Arc::new(TestProvider::with_space(DST_SPACE));
    let d = dest("ordo_imp_blob_post_dst", &art, BLOBBED, dst_keys.clone()).await;
    let spy = SpyDoor::default();

    // ── an object whose row is not in this bundle ────────────────────────
    let mut orphan = MemorySource::from(&bundle);
    put_object(
        &mut orphan,
        "docs",
        "scan",
        "doc_gone",
        Some("image/png"),
        b"P",
    );
    let e = import_space(&d.exec, &d.request_with("acme", &art, 8, &spy), &orphan)
        .await
        .expect_err("an object with no row");
    assert!(
        matches!(e, ImportError::BlobUnplaced { .. }),
        "an object is attached to a row that exists, so a source id this import never \
         mapped is a refusal rather than a file on some other row: {e}"
    );
    assert!(spy.seen().is_empty(), "and no upload was attempted");

    // ── bytes that are not the bytes the manifest recorded ──────────────
    let mut tampered = MemorySource::from(&bundle);
    put_object(
        &mut tampered,
        "docs",
        "scan",
        "doc_1",
        Some("image/png"),
        b"the original",
    );
    tampered.put("blobs/docs/scan/doc_1", b"something else".to_vec());
    let e = import_space(&d.exec, &d.request_with("acme", &art, 8, &spy), &tampered)
        .await
        .expect_err("bytes that do not hash to the manifest's digest");
    assert!(
        matches!(e, ImportError::BundleIncomplete { .. }),
        "an import that did not check the digest would be sealing bytes it was handed on \
         the strength of a filename: {e}"
    );
    assert!(
        spy.seen().is_empty(),
        "and the bytes never reached the door: {:?}",
        spy.seen()
    );

    // ── a door that refuses ─────────────────────────────────────────────
    let mut fine = MemorySource::from(&bundle);
    put_object(&mut fine, "docs", "scan", "doc_1", Some("image/png"), b"P");
    let e = import_space(
        &d.exec,
        &d.request_with("acme", &art, 8, &ClosedDoor),
        &fine,
    )
    .await
    .expect_err("a door that refuses");
    match e {
        ImportError::BlobRefused { detail, .. } => assert!(
            detail.contains("access_guard"),
            "the door's own class rides the refusal, as the write protocol's rides \
             `row_refused`: {detail}"
        ),
        other => panic!("expected the door's refusal, got {other}"),
    }
}

// ── throughput ───────────────────────────────────────────────────────────

/// What an import costs per row, measured rather than promised.
///
/// `#[ignore]` because it is a measurement and not an assertion: it holds
/// the method so the number in `docs/plan-import.md` §8 can be reproduced
/// and improved against, and a threshold here would fail on a loaded
/// laptop while proving nothing. Run it with
/// `cargo test -p ordo-exec --test import -- --ignored --nocapture`.
///
/// The shape it measures is the honest one: every row is an op call
/// compiled from source and run through the whole write protocol, which is
/// where the per-row cost lives and where the plan's deferred bound-plan
/// cache would go.
#[tokio::test]
#[ignore]
async fn how_fast_one_import_is() {
    const ROWS: i64 = 2000;
    const ONE: &str = "entity vendors {\n  name: text\n  active: bool = true\n}\n";
    let art = artifact_of("v1", ONE);
    let src_keys = TestProvider::with_space(SRC_SPACE);

    let pg = scratch("ordo_imp_tp_src");
    deploy(&pg, &art);
    pg.exec(&format!(
        "INSERT INTO \"vendors\" (\"$space\",\"id\",\"rev\",\"name\",\"active\") \
         SELECT '{SRC_SPACE}', 'ven_' || n, 1, 'vendor ' || n, n % 2 = 0 \
         FROM generate_series(1, {ROWS}) AS n"
    ))
    .expect("seed");
    let pool = pool_for("ordo_imp_tp_src").await;
    let schema = schema_of(ONE);
    let space = SpaceId(SRC_SPACE.to_string());
    let mut bundle = MemoryBundle::new();
    export_space(
        &pool,
        &ExportRequest {
            space: &space,
            schema: &schema,
            artifact: &art,
            keys: &src_keys,
            blobs: None,
            sealed: SealedMode::Open,
            correlation: "cg_import_throughput",
        },
        &mut bundle,
    )
    .await
    .expect("exports");

    let dst_keys = Arc::new(TestProvider::with_space(DST_SPACE));
    let d = dest("ordo_imp_tp_dst", &art, ONE, dst_keys).await;
    let source = MemorySource::from(&bundle);

    for batch in [1usize, 100, 1000] {
        // Each batch size is its own import name over its own destination
        // rows, so the second measurement is not the first one's resume.
        let name = format!("tp{batch}");
        d.pool
            .rows(
                "DELETE FROM \"vendors\"; DELETE FROM \"$importmap\"; \
                 DELETE FROM \"$commandlog\"; DELETE FROM \"$replay\"",
            )
            .await
            .expect("reset");
        let started = std::time::Instant::now();
        let report = import_space(&d.exec, &d.request(&name, &art, batch), &source)
            .await
            .expect("imports");
        let secs = started.elapsed().as_secs_f64();
        println!(
            "batch {batch:>4}: {ROWS} rows in {secs:.2}s = {:.0} rows/s, {} chain entries",
            ROWS as f64 / secs,
            report.groups
        );
        assert_eq!(report.rows(), ROWS as u64);
    }
}

// ── duplicate keys: refuse (owner-ruled 2026-09-28) ──────────────────────

/// One entity, one declared unique key. The smallest program in which two
/// rows can collide on something other than `id`.
const KEYED: &str = "\
entity vendors {
  name:   text unique
  active: bool = true
}
";

/// **Two bundle rows under one unique key refuse; they do not become one
/// row.** Owner-ruled 2026-09-28 (`sec6-import`: "Refuse"), closing
/// `security-review-6.md` S6-007.
///
/// Before the ruling `import.<entity>.create` carried no create-once belief,
/// so the second row converged onto the first: the import reported success
/// and the destination held fewer rows than the source, with nothing in the
/// report, the chain or the refusal set to say so. A loader for somebody
/// else's data that cannot reject a duplicate is how a migration loses rows
/// quietly.
///
/// The bundle is built by hand because the export cannot produce this case:
/// a source space with a unique key cannot hold two rows under it. A bundle
/// that arrived from elsewhere can, which is the whole of the threat.
#[tokio::test]
async fn two_rows_under_one_unique_key_refuse_and_nothing_is_written() {
    let art = artifact_of("v1", KEYED);

    // A real bundle of one vendor, exported, so the manifest is the
    // export's own and only the rows are hostile.
    let pg = scratch("ordo_imp_dup_src");
    deploy(&pg, &art);
    pg.exec(&format!(
        "INSERT INTO \"vendors\" (\"$space\",\"id\",\"rev\",\"name\",\"active\") VALUES \
         ('{SRC_SPACE}','ven_1',1,'Acme',true)"
    ))
    .expect("one source vendor");
    let src_keys = TestProvider::with_space(SRC_SPACE);
    let pool = pool_for("ordo_imp_dup_src").await;
    let mut bundle = MemoryBundle::new();
    export_space(
        &pool,
        &ExportRequest {
            space: &SpaceId(SRC_SPACE.to_string()),
            schema: &schema_of(KEYED),
            artifact: &art,
            keys: &src_keys,
            blobs: None,
            sealed: SealedMode::Open,
            correlation: "cg_import_dup_export",
        },
        &mut bundle,
    )
    .await
    .expect("the source exports");

    // The same row again under a different id and the SAME unique key.
    let mut src = MemorySource::from(&bundle);
    let vendors = String::from_utf8(src.members["data/vendors.jsonl"].clone()).expect("utf8");
    let first = vendors.lines().next().expect("one vendor").to_string();
    assert!(first.contains("\"ven_1\""), "the fixture row: {first}");
    let second = first.replace("\"ven_1\"", "\"ven_2\"");
    src.put("data/vendors.jsonl", format!("{first}\n{second}\n"));
    // A hostile bundle is internally consistent: it restates the row count
    // and the digest, so the refusal under test is the write protocol's and
    // not the manifest check's, which a careless forgery would hit first.
    let sha = ordo_exec::export::bundle_digest(&src.members["data/vendors.jsonl"]);
    edit_manifest(&mut src, |mf| {
        for e in mf["entities"].as_array_mut().expect("entities") {
            if e["entity"] == "vendors" || e["name"] == "vendors" {
                e["rows"] = serde_json::json!(2);
            }
        }
        for d in mf["digests"].as_array_mut().expect("digests") {
            if d["member"] == "data/vendors.jsonl" {
                d["sha256"] = serde_json::Value::String(sha.clone());
            }
        }
    });

    let dst_keys = Arc::new(TestProvider::with_space(DST_SPACE));
    let d = dest("ordo_imp_dup_dst", &art, KEYED, dst_keys).await;

    // Batch 1, so the first row commits alone and the second is the one
    // that meets it. With a batch of 2 both would roll back together and
    // the test could not tell "refused" from "never reached".
    let refused = import_space(&d.exec, &d.request("dup", &art, 1), &src)
        .await
        .expect_err("a duplicate unique key is a refusal");
    let text = refused.to_string();
    assert!(
        matches!(refused, ImportError::RowRefused { .. }),
        "refused by the write protocol, by name: {text}"
    );
    assert!(
        text.contains("expected_absent") || text.contains("duplicate"),
        "and the refusal says the key is taken: {text}"
    );

    // Exactly the first row is there. Not two, and not one row that is
    // quietly the second's values.
    assert_eq!(
        d.one(&format!(
            "SELECT count(*) FROM \"vendors\" WHERE \"$space\" = '{DST_SPACE}'"
        ))
        .await,
        "1"
    );
    assert_eq!(
        d.one(&format!(
            "SELECT \"rev\" FROM \"vendors\" WHERE \"$space\" = '{DST_SPACE}'"
        ))
        .await,
        "1",
        "the first row was not converged onto: its rev did not move"
    );
}
