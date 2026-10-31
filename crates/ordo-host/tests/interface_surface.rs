//! The Interface Descriptor where it meets a deployment: **catalog scoping**,
//! the **wire crossing**, and the **shape gate**.
//!
//! `ordo-cli`'s `tests/interface.rs` proves the projection against the
//! artifact and needs no database. Three claims it structurally cannot make
//! live here, because each of them is about a running deployment:
//!
//! 1. **Scoping is the catalog's, not a second rule.** The descriptor a
//!    caller receives must contain *exactly* the ops their catalog shows —
//!    a viewer's omitting what a manager's carries, and omitting rather than
//!    marking, because a listed-but-forbidden op still tells the asker it
//!    exists. And the omission has to be total: nothing derived across ops
//!    (action discovery, the entity list) may name an op the asker cannot
//!    call.
//! 2. **The wire and the descriptor agree.** `ordo-plan` cannot depend on
//!    `ordo-api` (the door's crate has no sibling dependencies by design), so
//!    the terminal-verb contract's HTTP methods and its live flag are a
//!    mirror. This is where the two inventories are joined over the whole
//!    cross product — the pattern `tests/cost_class.rs` uses for exactly the
//!    same reason.
//! 3. **The columns are the ones Postgres describes.** `ui-gen.md` §1's
//!    *shape-verified* clause: a descriptor may never claim a column the
//!    database does not produce. The gate already asks Postgres; this joins
//!    its answer to the document.
//!
//! Real Postgres, this suite's own databases (`ordo_ifd_*`).

use std::collections::BTreeSet;

use ordo_api::{AccessDecision, OpKind};
use ordo_conformance::{Fixture, Pg};
use ordo_exec::PoolConfig;
use ordo_host::shape::verify_registry;
use ordo_host::DEFAULT_SPACE;
use ordo_identity::principal::{PrincipalId, Timestamp};
use ordo_identity::{AccessEngine, TokenKey, TokenSpec};
use serde_json::Value;

const NOW: u64 = 1_000_000;
const SECRET: &[u8] = b"interface-descriptor-secret";

fn make_db(db: &str) -> Pg {
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

fn access(engine: AccessEngine) -> ordo_host::IdentityAccess {
    ordo_host::IdentityAccess::sharing(
        std::sync::Arc::new(engine),
        TokenKey::from_secret(SECRET),
        Box::new(|| NOW),
        std::sync::Arc::new(ordo_host::metrics::Metrics::new()),
    )
}

fn token(principal: &str) -> String {
    ordo_identity::mint(
        &TokenKey::from_secret(SECRET),
        &TokenSpec {
            principal: PrincipalId(principal.into()),
            expires: Timestamp(NOW + 3_600),
            client: None,
            user_presence_at: Some(Timestamp(NOW)),
            capability: None,
            delegation: None,
            session: None,
        },
    )
}

fn op_names(doc: &Value) -> Vec<String> {
    doc["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .filter_map(|o| o["op"].as_str().map(str::to_string))
        .collect()
}

/// The fixture, seeded, with the deployment's own access engine over it —
/// the same engine the door decides with, so "what this token may call" has
/// one answer in this test and in production.
fn seeded(db: &str) -> (Fixture, AccessEngine) {
    let f = Fixture::load().expect("fixture");
    let pg = make_db(db);
    f.reset(&pg).expect("seeded");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    (f, engine)
}

// ── 1. scoping ───────────────────────────────────────────────────────────

/// **The descriptor's op list IS the catalog's.**
///
/// Asserted as an equality rather than as a containment, for both of the two
/// callers: a descriptor that showed *fewer* ops than the catalog would give
/// a generated SDK a hole nobody can explain, and one that showed *more*
/// would be the disclosure the catalog spent 5e closing.
#[tokio::test(flavor = "multi_thread")]
async fn the_descriptor_describes_exactly_the_ops_the_catalog_lists() {
    let (f, engine) = seeded("ordo_ifd_scope");
    let acc = access(engine);

    for principal in ["mem_1", "mem_4"] {
        let t = token(principal);
        let catalog = ordo_host::catalog_for(&f, &acc, &t, DEFAULT_SPACE);
        let listed: Vec<String> = catalog
            .as_array()
            .expect("an array")
            .iter()
            .filter_map(|m| m["op"].as_str().map(str::to_string))
            .collect();
        let doc = ordo_host::interface_for(&f, &acc, &t, DEFAULT_SPACE);
        assert_eq!(
            op_names(&doc),
            listed,
            "`{principal}`'s descriptor and catalog describe different surfaces"
        );
        assert!(!listed.is_empty(), "`{principal}` can call nothing at all");
    }
}

/// A viewer's descriptor omits what a manager's contains — and the omission
/// reaches everything derived from the op list, not only the `ops` array.
///
/// The second half is the one worth having. An action index built before the
/// filter would hand a viewer the name of every write op in the deployment,
/// which is the enumeration the catalog refuses one op at a time; the entity
/// half would hand them the shape of every table.
#[tokio::test(flavor = "multi_thread")]
async fn a_viewers_descriptor_omits_what_a_managers_contains_everywhere() {
    let (f, engine) = seeded("ordo_ifd_viewer");
    let acc = access(engine);

    let manager = ordo_host::interface_for(&f, &acc, &token("mem_1"), DEFAULT_SPACE);
    let viewer = ordo_host::interface_for(&f, &acc, &token("mem_4"), DEFAULT_SPACE);

    let m: BTreeSet<String> = op_names(&manager).into_iter().collect();
    let v: BTreeSet<String> = op_names(&viewer).into_iter().collect();
    assert!(v.is_subset(&m), "a viewer sees ops a manager does not");
    let only_manager: Vec<&String> = m.difference(&v).collect();
    assert!(
        !only_manager.is_empty(),
        "the fixture's roles are indistinguishable, so this proves nothing"
    );

    // Total omission: the manager-only names appear NOWHERE in the viewer's
    // document — not in `ops`, not in `id_types`, not in a refusal, not in a
    // guard's rendered text. Checked over the serialised bytes, because that
    // is what the caller actually receives and a structural walk would miss
    // a name in a field nobody thought to check.
    let bytes = ordo_plan::artifact::to_bytes(&viewer);
    for name in &only_manager {
        assert!(
            !bytes.contains(name.as_str()),
            "a viewer's descriptor mentions `{name}`, an op they may not call"
        );
    }

    // Action discovery followed the filter: every op any id type offers is
    // an op this caller's own document describes.
    for t in viewer["id_types"].as_array().expect("id_types") {
        for entry in t["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .chain(t["detail"].as_array().expect("detail"))
        {
            let op = entry["op"].as_str().expect("a name");
            assert!(
                v.contains(op),
                "the viewer's action index offers `{op}`, which is not in their catalog"
            );
        }
    }

    // And the entity half followed it too: an entity is described exactly
    // when a visible op reaches it. Read off the CATALOG's manifests rather
    // than off the descriptor, because the field-grain scope is a consent
    // fact the descriptor deliberately does not republish — so this joins
    // the two documents instead of checking one against itself.
    let reached: BTreeSet<String> =
        ordo_host::catalog_for(&f, &acc, &token("mem_4"), DEFAULT_SPACE)
            .as_array()
            .expect("an array")
            .iter()
            .flat_map(|m| m["entities"].as_array().cloned().unwrap_or_default())
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect();
    let described: BTreeSet<String> = viewer["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .filter_map(|e| e["entity"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        described, reached,
        "the entity half did not follow the op filter"
    );
    assert!(
        !described.is_empty(),
        "a viewer who reaches no entity makes this vacuous"
    );
    let all: BTreeSet<String> = manager["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .filter_map(|e| e["entity"].as_str().map(str::to_string))
        .collect();
    assert!(
        described.is_subset(&all) && described != all,
        "a viewer was handed the shape of every table a manager can see"
    );
}

/// An anonymous asker gets a **well-formed empty descriptor**, not a refusal
/// and not the whole program.
///
/// The catalog route's rule, kept: a 403 would itself be an answer about the
/// deployment. And well-formed matters — a build step run without credentials
/// should fail on *"nothing to generate"* rather than on a parse error.
#[tokio::test(flavor = "multi_thread")]
async fn no_token_is_an_empty_document_rather_than_a_refusal_or_the_program() {
    let (f, engine) = seeded("ordo_ifd_anon");
    let acc = access(engine);
    let doc = ordo_host::interface_for(&f, &acc, "", DEFAULT_SPACE);
    assert_eq!(doc["ops"], serde_json::json!([]));
    assert_eq!(doc["entities"], serde_json::json!([]));
    assert_eq!(doc["id_types"], serde_json::json!([]));
    assert_eq!(doc["schema_version"], ordo_plan::interface::SCHEMA_VERSION);
    assert!(doc["digest"].is_string(), "still a complete document");
}

/// **Refusal metadata answers about the op it is on, and about nothing
/// else.**
///
/// This is `std.space.callersOf`'s rule applied to a document rather than to
/// a query: an op outside the asker's catalog must be indistinguishable from
/// one that does not exist. So a viewer's refusal lists are compared against
/// a manager's for the ops they share — identical, because a refusal class is
/// derived from the op's own declaration — while naming nothing about the
/// ops they do not.
#[tokio::test(flavor = "multi_thread")]
async fn refusal_metadata_does_not_leak_the_existence_of_other_ops() {
    let (f, engine) = seeded("ordo_ifd_refusals");
    let acc = access(engine);
    let manager = ordo_host::interface_for(&f, &acc, &token("mem_1"), DEFAULT_SPACE);
    let viewer = ordo_host::interface_for(&f, &acc, &token("mem_4"), DEFAULT_SPACE);

    let refusals = |doc: &Value, name: &str| -> Option<Value> {
        doc["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .find(|o| o["op"] == name)
            .map(|o| o["refusals"].clone())
    };
    let mut shared = 0;
    for o in viewer["ops"].as_array().expect("ops") {
        let name = o["op"].as_str().expect("a name");
        let Some(theirs) = refusals(&manager, name) else {
            panic!("`{name}` is in a viewer's catalog and not a manager's");
        };
        assert_eq!(
            o["refusals"], theirs,
            "`{name}`'s refusal list changed with the asker — a refusal class is \
             a property of the declaration, and one that varied by caller would \
             be a channel for comparing catalogs"
        );
        shared += 1;
    }
    assert!(
        shared > 0,
        "the two callers share no ops, so this is vacuous"
    );
}

// ── 2. the wire crossing ─────────────────────────────────────────────────

/// The terminal-verb contract's mirror, joined against the door's own
/// authority over the **whole** registry.
///
/// `ordo_api::method_for` is what the door enforces (a wrong method is
/// `method_mismatch` with an `Allow` header naming the right one); the
/// descriptor states the same string so a generated client picks it without
/// asking. Two spellings of one rule need a join, or they are two rules.
#[tokio::test(flavor = "multi_thread")]
async fn the_descriptors_method_and_live_flag_are_the_doors_own() {
    let (f, engine) = seeded("ordo_ifd_wire");
    let acc = access(engine);
    // The owner sees the whole compiled surface, which is what makes this a
    // cross product rather than a sample.
    let doc = ordo_host::interface_for(&f, &acc, &token("mem_o"), DEFAULT_SPACE);
    let specs = ordo_host::build_specs(&f);
    assert!(specs.len() > 50);

    let mut joined = 0;
    for spec in &specs {
        let Some(o) = doc["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .find(|o| o["op"] == spec.name)
        else {
            continue;
        };
        let (kind, method) = ordo_api::method_for(spec.kind);
        assert_eq!(o["kind"], kind, "`{}` kind", spec.name);
        assert_eq!(o["call"]["method"], method, "`{}` method", spec.name);
        assert_eq!(
            o["call"]["live"], spec.live_capable,
            "`{}` live-capability — the descriptor and the spec the door \
             serves `?live` from must not disagree",
            spec.name
        );
        assert_eq!(
            o["version"], spec.version_hash,
            "`{}` version — the header a caller sends and the number the \
             descriptor pinned are one number or §B5's skew story is fiction",
            spec.name
        );
        // A write is never refinable and never live, at both layers.
        if !matches!(spec.kind, OpKind::Read) {
            assert_eq!(o["call"]["refinable"], false);
            assert_eq!(o["refine"]["tier"], "none");
        }
        // Every program read is refinable, with no off switch (owner,
        // 2026-10-05), and the descriptor says what the door enforces.
        let tier = match spec.refinable {
            ordo_api::RefineTier::Basic => "basic",
            ordo_api::RefineTier::None => "none",
        };
        assert_eq!(o["refine"]["tier"], tier, "`{}` tier", spec.name);
        // A read that does not bind (`giftOrders`: a host-side filter the
        // binder refuses) has no manifest and stays `none`, at both layers.
        if matches!(spec.kind, OpKind::Read) && !spec.output_fields.is_empty() {
            assert_eq!(tier, "basic", "`{}` is a program read", spec.name);
            let refusals = o["refusals"].as_array().expect("refusals");
            for class in ["invalid_refinement", "refinement_unbounded"] {
                assert!(
                    refusals.iter().any(|r| r == class),
                    "`{}` can refuse a refinement as {class}",
                    spec.name
                );
            }
        }
        joined += 1;
    }
    assert_eq!(joined, specs.len(), "an op fell out of the join");
}

/// Every refusal class the descriptor names has a producer in the closed
/// wire inventory — the empty-allow-list rule, joined against the enum
/// itself rather than against the document that describes it.
#[tokio::test(flavor = "multi_thread")]
async fn every_refusal_class_the_descriptor_names_is_a_class_the_engine_has() {
    let (f, engine) = seeded("ordo_ifd_classes");
    let acc = access(engine);
    let doc = ordo_host::interface_for(&f, &acc, &token("mem_o"), DEFAULT_SPACE);
    let known: BTreeSet<&str> = ordo_api::error::ErrorCode::ALL
        .iter()
        .map(|c| c.as_str())
        .collect();
    let mut named = BTreeSet::new();
    for o in doc["ops"].as_array().expect("ops") {
        for r in o["refusals"].as_array().expect("refusals") {
            let c = r.as_str().expect("a class");
            assert!(
                known.contains(c),
                "op {} names `{c}`, which `ErrorCode::ALL` does not have — a \
                 refusal class with no producer is API surface a caller writes \
                 a handler for and never reaches",
                o["op"]
            );
            named.insert(c);
        }
    }
    assert!(named.len() > 10, "only {} classes derived", named.len());
}

// ── 3. the shape gate ────────────────────────────────────────────────────

/// **Shape-verified** (`ui-gen.md` §1): every column the descriptor claims is
/// a column the load gate posed to Postgres and Postgres confirmed.
///
/// The gate compares the *compiler's* inferred shape with the database's; the
/// descriptor's outputs are the artifact's, which are that same inferred
/// shape. This joins the two ends, so the claim *"a descriptor can never
/// silently disagree with the wire"* is checked rather than argued: an op in
/// the gate's `verified` bucket must have a descriptor, and that descriptor's
/// columns must be the spec's, in order.
#[tokio::test(flavor = "multi_thread")]
async fn every_verified_ops_descriptor_columns_are_the_verified_ones() {
    let f = Fixture::load().expect("fixture");
    let pg = make_db("ordo_ifd_shape");
    f.reset(&pg).expect("seeded");
    let engine = ordo_host::deployment_engine(&f, &pg).expect("engine");
    let acc = access(engine);

    let report = verify_registry(
        &f,
        &PoolConfig {
            db: "ordo_ifd_shape".into(),
            ..PoolConfig::from_env()
        },
        &f.ddl,
    )
    .await
    .expect("the fixture agrees with its database");
    assert!(
        report.verified.len() >= 25,
        "the gate verified only {} ops, so this join is nearly empty: {}",
        report.verified.len(),
        report.summary()
    );

    let doc = ordo_host::interface_for(&f, &acc, &token("mem_o"), DEFAULT_SPACE);
    let specs = ordo_host::build_specs(&f);
    for name in &report.verified {
        let o = doc["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .find(|o| o["op"] == name.as_str())
            .unwrap_or_else(|| panic!("`{name}` passed the shape gate and has no descriptor"));
        let spec = specs.iter().find(|s| s.name == *name).expect("a spec");
        let declared: Vec<&str> = o["result"]["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .map(|f| f["name"].as_str().expect("a name"))
            .collect();
        // The door's spec also carries condition-only fields:
        // `std.E.count`'s conditions name `std.E.list`'s fields, which are
        // not columns of the count's result (`std-library.md` §2.1).
        let outputs: Vec<&ordo_api::FieldSpec> =
            spec.output_fields.iter().filter(|f| f.output).collect();
        let verified: Vec<&str> = outputs.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            declared, verified,
            "`{name}`'s descriptor claims columns the database did not describe"
        );
        // The sealed mark rides too — a skin renders a sealed marker instead
        // of an empty cell only if it is told (`ui-gen.md` §8).
        for (d, v) in o["result"]["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .zip(&outputs)
        {
            assert_eq!(d["sealed"], v.sealed, "`{name}`.{} sealed", d["name"]);
            assert_eq!(
                d["refinable"], v.refinable,
                "`{name}`.{} refinable",
                d["name"]
            );
        }
    }

    // The other direction: an op the gate could NOT pose is still described,
    // with its reason visible as a compile error where there is one. A
    // descriptor that silently dropped the ops the gate skipped would let a
    // generator produce an SDK missing functions nobody can account for.
    for u in &report.undescribable {
        assert!(
            doc["ops"]
                .as_array()
                .expect("ops")
                .iter()
                .any(|o| o["op"] == u.op)
                || !matches!(
                    ordo_api::AccessChecker::check(&acc, &token("mem_o"), DEFAULT_SPACE, &u.op),
                    AccessDecision::Allow { .. }
                ),
            "`{}` was skipped by the gate AND dropped from the descriptor",
            u.op
        );
    }
}

/// A sanity check on the space: the descriptor is per-space like every other
/// answer here, because membership is.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_for_another_space_describes_nothing() {
    let (f, engine) = seeded("ordo_ifd_space");
    let acc = access(engine);
    let doc = ordo_host::interface_for(&f, &acc, &token("mem_1"), "some-other-space");
    assert_eq!(
        doc["ops"],
        serde_json::json!([]),
        "membership is per space, and so is the surface it grants"
    );
}
