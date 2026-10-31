//! **Shape verification for the descriptor itself**, in both directions.
//!
//! The interface descriptor is a projection, and a projection has exactly two
//! ways to be wrong. It can **invent** — say something the artifact does not
//! say, which is how a generated SDK gets a function whose signature no op
//! has — or it can **omit** — drop a fact §B1 or `ui-gen.md` settled, which
//! is how a renderer starts guessing from names again. Neither is visible in
//! a golden file, because a golden shows what the document *is* and says
//! nothing about what it was derived from.
//!
//! So this file joins the two documents over the whole cross product, the
//! way the wire inventory joins `ErrorCode::ALL` against the document's table
//! and the cost-class crossing joins two spellings of one model:
//!
//! - **Nothing is invented.** Every op in the descriptor is an op in the
//!   artifact; every param, output, statement and entity is one of that op's;
//!   every scalar that has a counterpart is byte-equal to it.
//! - **Nothing settled is omitted.** A written-down list of the keys §B1 and
//!   `ui-gen.md` §1–§4 name, asserted present on every entry — and asserted
//!   *non-vacuous*, because a list checked against an empty document passes.
//!
//! The allow-list of "in the artifact, deliberately not in the descriptor" is
//! written down with the reason, and it is the only escape hatch.

use std::collections::BTreeSet;

use std::path::PathBuf;

use ordo_cli::{interface_json, load_sources, Sources};
use ordo_conformance::{assert_golden, FIXTURE};
use ordo_plan::artifact::{catalog_view, entity_artifacts, program_artifact};
use ordo_plan::interface::{digest, interface_document, SCHEMA_VERSION};
use serde_json::Value;

/// The market fixture compiled once, as (artifact, descriptor) — the two
/// documents every test below joins.
fn program() -> (Value, Value) {
    let (art, entities, ops) = parts();
    let doc = interface_document(&art["domain"], &entities, &ops);
    (art, doc)
}

/// The three inputs, for the tests that build more than one descriptor.
fn parts() -> (Value, Vec<Value>, Vec<Value>) {
    let src = Sources::from_parts(vec![("market.ordo".into(), FIXTURE.to_string())]);
    let (schema, registry) = load_sources(&src).expect("the fixture compiles");
    let art = program_artifact(&schema, &registry).expect("the fixture plans");
    let ops: Vec<Value> = art["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .map(catalog_view)
        .collect();
    (art, entity_artifacts(&schema), ops)
}

fn names(v: &Value, key: &str) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x[key].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ── direction 1: nothing is invented ─────────────────────────────────────

/// Every op the descriptor describes is an op the artifact describes, under
/// the same name and the same version hash.
///
/// The version equality is the load-bearing half: a descriptor carrying a
/// version the artifact never minted would let a generated client pin a
/// contract that does not exist, and §B5's whole skew story runs on that
/// number.
#[test]
fn every_described_op_is_an_artifact_op_at_the_same_version() {
    let (art, doc) = program();
    let ops = art["ops"].as_array().expect("ops");
    assert!(!doc["ops"].as_array().expect("ops").is_empty());
    for d in doc["ops"].as_array().expect("ops") {
        let name = d["op"].as_str().expect("a name");
        let a = ops
            .iter()
            .find(|a| a["op"] == d["op"])
            .unwrap_or_else(|| panic!("the descriptor invented an op: `{name}`"));
        assert_eq!(
            d["version"], a["version"],
            "`{name}`'s version was rewritten"
        );
        assert_eq!(d["kind"], a["kind"], "`{name}`'s kind was rewritten");
        assert_eq!(d["mode"], a["mode"], "`{name}`'s mode label was rewritten");
        assert_eq!(d["statements"], a["statements"], "`{name}`'s statements");
        assert_eq!(d["error"], a["error"], "`{name}`'s compile refusal");
    }
}

/// Params and outputs are the artifact's, name for name and in order.
///
/// Order matters and is asserted rather than sorted away: a generated
/// positional constructor and a rendered column order both read it, and a
/// descriptor that quietly re-sorted would move every generated call site.
#[test]
fn params_and_outputs_are_the_artifacts_in_order() {
    let (art, doc) = program();
    let ops = art["ops"].as_array().expect("ops");
    for d in doc["ops"].as_array().expect("ops") {
        let name = d["op"].as_str().expect("a name");
        let a = ops
            .iter()
            .find(|a| a["op"] == d["op"])
            .expect("found above");
        assert_eq!(
            names(&d["params"], "name"),
            names(&a["params"], "name"),
            "`{name}`'s params"
        );
        assert_eq!(
            names(&d["result"]["outputs"], "name"),
            names(&a["outputs"], "name"),
            "`{name}`'s outputs"
        );
        // The per-field facts, not only the names.
        for (dp, ap) in d["params"]
            .as_array()
            .expect("params")
            .iter()
            .zip(a["params"].as_array().expect("params"))
        {
            for k in ["type", "required", "max", "id_entity"] {
                assert_eq!(dp[k], ap[k], "`{name}`.${} {k}", dp["name"]);
            }
        }
        for (df, af) in d["result"]["outputs"]
            .as_array()
            .expect("outputs")
            .iter()
            .zip(a["outputs"].as_array().expect("outputs"))
        {
            for k in ["type", "sealed", "refinable", "id_entity"] {
                assert_eq!(df[k], af[k], "`{name}`.{} {k}", df["name"]);
            }
        }
    }
}

/// The disclosures are the manifest's, verbatim. A consent page and a
/// generated UI must not be able to disagree about whether an op bypasses
/// guards, because the second is often the only one a user ever sees.
#[test]
fn disclosures_are_the_manifests_verbatim() {
    let (art, doc) = program();
    let views: Vec<Value> = art["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .map(catalog_view)
        .collect();
    for d in doc["ops"].as_array().expect("ops") {
        let name = d["op"].as_str().expect("a name");
        let m = views.iter().find(|m| m["op"] == d["op"]).expect("found");
        let dis = &d["disclosures"];
        assert_eq!(dis["unguarded"], m["unguarded"], "`{name}` unguarded");
        assert_eq!(dis["unbounded"], m["unbounded"], "`{name}` unbounded");
        assert_eq!(dis["bypassed_guards"], m["bypassed_guards"], "`{name}`");
        assert_eq!(dis["guards"], m["guards"], "`{name}` guards");
        assert_eq!(d["cost"], m["cost"], "`{name}` cost");
    }
}

/// Every entity described is an entity of the program, and every field of it
/// is one of that entity's fields with the same declaration facts.
#[test]
fn every_described_entity_and_field_exists_in_the_artifact() {
    let (art, doc) = program();
    let entities = art["entities"].as_array().expect("entities");
    assert!(!doc["entities"].as_array().expect("entities").is_empty());
    for d in doc["entities"].as_array().expect("entities") {
        let name = d["entity"].as_str().expect("a name");
        let a = entities
            .iter()
            .find(|a| a["entity"] == d["entity"])
            .unwrap_or_else(|| panic!("the descriptor invented an entity: `{name}`"));
        assert_eq!(d["key"], a["key"], "`{name}`'s typed-id key");
        assert_eq!(d["tree"], a["tree"]);
        assert_eq!(d["unique_keys"], a["unique_keys"]);
        assert_eq!(d["checks"], a["checks"]);
        assert_eq!(names(&d["fields"], "name"), names(&a["fields"], "name"));
        for (df, af) in d["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .zip(a["fields"].as_array().expect("fields"))
        {
            for k in [
                "optional",
                "unique",
                "sealed",
                "indexed",
                "default",
                "normalize",
                "enum_values",
            ] {
                assert_eq!(df[k], af[k], "`{name}`.{} {k}", df["name"]);
            }
            // A ref is described as an `id` whose `ref` names the target;
            // a scalar's type is the declared scalar.
            match af["type"]["kind"].as_str() {
                Some("ref") => {
                    assert_eq!(df["type"], "id");
                    assert_eq!(df["ref"], af["type"]["target"]);
                }
                _ => {
                    assert_eq!(df["type"], af["type"]["scalar"]);
                    assert_eq!(df["ref"], Value::Null);
                }
            }
        }
    }
}

/// Action discovery invents no op and no id type: every entry names an op the
/// descriptor describes, a param that op declares, and an entity some op's
/// declaration actually points at.
///
/// This is the rule `ui-gen.md` §2 settles, checked as a rule rather than as
/// a sample: *a write op applies to a row iff it declares an id-typed
/// parameter whose entity prefix matches the row's id type*.
#[test]
fn action_discovery_names_only_real_ops_and_real_params() {
    let (_, doc) = program();
    let ops = doc["ops"].as_array().expect("ops");
    let types = doc["id_types"].as_array().expect("id_types");
    assert!(!types.is_empty(), "the fixture has typed-id parameters");
    for t in types {
        let entity = t["entity"].as_str().expect("an entity");
        for entry in t["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .chain(t["detail"].as_array().expect("detail"))
        {
            let op = ops
                .iter()
                .find(|o| o["op"] == entry["op"])
                .unwrap_or_else(|| panic!("action discovery invented op {}", entry["op"]));
            let p = op["params"]
                .as_array()
                .expect("params")
                .iter()
                .find(|p| p["name"] == entry["param"])
                .unwrap_or_else(|| panic!("{} has no param {}", entry["op"], entry["param"]));
            assert_eq!(
                p["id_entity"].as_str(),
                Some(entity),
                "{} ${} does not address {entity}",
                entry["op"],
                entry["param"]
            );
            assert_eq!(entry["kind"], op["kind"]);
        }
        // The split is the rule: writes and deletes are actions, reads are
        // detail surfaces. A read in `actions` would put a "do this to the
        // row" button on a query.
        for a in t["actions"].as_array().expect("actions") {
            assert_ne!(a["kind"], "read", "a read op is not a row action");
        }
        for d in t["detail"].as_array().expect("detail") {
            assert_eq!(d["kind"], "read", "a write op is not a detail surface");
        }
    }
}

/// The other half of the index: **which ops list an entity**, keyed on what
/// an op returns rather than on what it accepts.
///
/// The rule is `ui-gen.md` §1's row identity, applied as an index: an op
/// lists `E` iff its rows carry `E`'s typed id. So this asserts the rule
/// rather than a sample, in both directions — every entry really anchors on
/// the entity, and every op that anchors on it really has an entry — and
/// asserts that the `list`/`get` split is the *addressing* one, never a name
/// one. `std.E.list` and `std.E.get` make it well-defined; an authored
/// `myProjects` must land in the same place for the same reason.
#[test]
fn the_listing_index_is_row_identity_and_not_a_name() {
    let (_, doc) = program();
    let ops = doc["ops"].as_array().expect("ops");
    let types = doc["id_types"].as_array().expect("id_types");
    let mut authored_listing = 0;
    let mut indexed: BTreeSet<(String, String)> = BTreeSet::new();

    for t in types {
        let entity = t["entity"].as_str().expect("an entity");
        for (half, want_addressed) in [("list", false), ("get", true)] {
            for e in t[half].as_array().expect(half) {
                let name = e["op"].as_str().expect("an op");
                let o = ops
                    .iter()
                    .find(|o| o["op"] == e["op"])
                    .unwrap_or_else(|| panic!("the listing index invented op `{name}`"));
                assert_eq!(o["kind"], "read", "`{name}` is not a read");
                assert_eq!(
                    o["result"]["row_identity"]["entity"].as_str(),
                    Some(entity),
                    "`{name}` is indexed under {entity} but its rows are not {entity}'s"
                );
                assert_eq!(e["single_row"], o["result"]["single_row"]);
                // The split, stated as the addressing test it is.
                let addressed = o["params"].as_array().expect("params").iter().any(|p| {
                    p["id_entity"].as_str() == Some(entity)
                        && p["required"].as_bool().unwrap_or(false)
                });
                assert_eq!(
                    addressed, want_addressed,
                    "`{name}` is in `{half}` but its addressing says otherwise"
                );
                // `get` names the parameter to bind the id to; `list` has
                // none to name.
                assert_eq!(e["param"].is_string(), want_addressed, "`{name}` param");
                if want_addressed {
                    let p = e["param"].as_str().expect("a param name");
                    assert!(
                        o["params"]
                            .as_array()
                            .expect("params")
                            .iter()
                            .any(|x| x["name"] == p && x["id_entity"].as_str() == Some(entity)),
                        "`{name}`.${p} does not address {entity}"
                    );
                }
                if !name.starts_with("std.") {
                    authored_listing += 1;
                }
                indexed.insert((entity.to_string(), name.to_string()));
            }
        }
    }

    // The reverse direction: no op that anchors on an indexed entity is
    // missing from it. Without this the index could quietly shrink to
    // `std.E.*` and every assertion above would still pass.
    let named: BTreeSet<&str> = types.iter().filter_map(|t| t["entity"].as_str()).collect();
    for o in ops {
        let Some(e) = o["result"]["row_identity"]["entity"].as_str() else {
            continue;
        };
        assert!(
            named.contains(e),
            "no id_types entry for {e}, which `{}` lists",
            o["op"]
        );
        assert!(
            indexed.contains(&(e.to_string(), o["op"].as_str().expect("a name").to_string())),
            "`{}` returns {e} rows and is in neither `list` nor `get`",
            o["op"]
        );
    }

    assert!(
        authored_listing > 5,
        "an index that only ever holds `std.` names is a name heuristic wearing a rule's clothes"
    );

    // The fixture's one entity that nothing lists, stated rather than
    // inferred: `members` is platform-provided, `std_ops` skips it by the
    // platform flag, and no authored op returns its rows. A picker behind an
    // `assignee` parameter is genuinely blind, and the whole point of this
    // index is that "blind" is now an answer instead of an empty guess.
    let members = types
        .iter()
        .find(|t| t["entity"] == "members")
        .expect("members is referenced by tasks.assignee");
    assert!(members["list"].as_array().expect("list").is_empty());
    assert!(members["get"].as_array().expect("get").is_empty());
    assert!(
        !members["actions"].as_array().expect("actions").is_empty(),
        "…while its id is still handed to plenty of ops, which is exactly the \
         asymmetry the accept-keyed index could not express"
    );
}

/// A parameter that fills an `enum(…)` field carries the value set, and one
/// that does not carries `null` — never an empty list.
///
/// The link is `params[].field_ref`, produced from the AST and the schema, so
/// this also asserts the thing a name join could not: the field named is
/// really the field written, on really that entity.
#[test]
fn an_enum_valued_param_carries_its_options_through_field_ref() {
    let (_, doc) = program();
    let ops = doc["ops"].as_array().expect("ops");
    let entities = doc["entities"].as_array().expect("entities");
    let mut with_options = 0;

    for o in ops {
        for p in o["params"].as_array().expect("params") {
            let fr = &p["field_ref"];
            match fr["entity"].as_str() {
                None => {
                    assert!(fr.is_null(), "a partial field_ref on {}", o["op"]);
                    assert_eq!(
                        p["enum_values"],
                        Value::Null,
                        "{} ${} has options with nothing to read them off",
                        o["op"],
                        p["name"]
                    );
                }
                Some(e) => {
                    let field = entities
                        .iter()
                        .find(|x| x["entity"] == e)
                        .unwrap_or_else(|| panic!("field_ref names unknown entity {e}"))["fields"]
                        .as_array()
                        .expect("fields")
                        .iter()
                        .find(|f| f["name"] == fr["field"])
                        .unwrap_or_else(|| {
                            panic!("field_ref names {e}.{} which is not a field", fr["field"])
                        })
                        .clone();
                    assert_eq!(
                        p["enum_values"], field["enum_values"],
                        "{} ${} disagrees with the field it fills",
                        o["op"], p["name"]
                    );
                }
            }
            if let Some(vs) = p["enum_values"].as_array() {
                assert!(
                    !vs.is_empty(),
                    "an empty option list reads as a dead dropdown"
                );
                with_options += 1;
            }
        }
    }
    assert!(
        with_options > 0,
        "the fixture's `acl` ladder must witness this or the join is untested"
    );
}

/// Every refusal class the descriptor names is a class the wire can actually
/// return. The empty-allow-list rule, applied to this document: a name here
/// with no producer is API surface a caller would write a handler for and
/// never reach.
///
/// The inventory is read out of `api-engine-design.md`'s table rather than
/// from `ErrorCode::ALL`, because `ordo-plan` cannot depend on `ordo-api` —
/// `ordo-host`'s crossing test joins the descriptor against the enum itself,
/// and `ordo-api`'s own inventory test already pins the table to the enum, so
/// the three assertions close the triangle.
#[test]
fn every_refusal_class_named_is_in_the_wire_inventory() {
    const DOC: &str = include_str!("../../../docs/api-engine-design.md");
    let documented: BTreeSet<&str> = DOC
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("| `"))
        .filter_map(|l| l.split('|').nth(1).map(|c| c.trim().trim_matches('`')))
        .collect();
    assert!(
        documented.len() > 20,
        "the wire inventory table did not parse — this test would pass vacuously"
    );
    let (_, doc) = program();
    let mut seen = BTreeSet::new();
    for o in doc["ops"].as_array().expect("ops") {
        for r in o["refusals"].as_array().expect("refusals") {
            let c = r.as_str().expect("a class name");
            assert!(
                documented.contains(c),
                "op {} names refusal class `{c}`, which is in no wire inventory",
                o["op"]
            );
            seen.insert(c.to_string());
        }
    }
    // Every RULE has to fire somewhere, or it is a rule nobody has tested.
    for expected in [
        "access_denied",
        "cas_miss",
        "conflicting_identity",
        "duplicate",
        "has_referrers",
        "invalid_cursor",
        "missing_param",
        "sealed_open_failed",
        "sealed_unreadable",
        "tree_cycle",
        "tree_has_children",
    ] {
        assert!(
            seen.contains(expected),
            "no op in the fixture derives `{expected}` — the rule that emits it is dead code"
        );
    }
}

/// The two rules the market fixture cannot witness, witnessed.
///
/// The fixture declares no `check` and no `<=` parameter bound, so the rules
/// that derive `check_violation` and `bound_violation` would sit in the tree
/// untested — and an untested derivation rule is a claim, not a fact. A
/// four-line program is enough to fire both, and its absence from the market
/// fixture is itself worth a witness: the same test asserts that an entity
/// with no invariant does NOT advertise `check_violation`, because the
/// affordance is only meaningful where it can happen.
#[test]
fn the_declaration_only_rules_fire_exactly_where_the_declaration_is() {
    const SRC: &str = r#"
domain bounded

entity teams (key: team) {
  name:  text
  seats: int
  used:  int = 0
  check used <= seats
}

entity plain (key: pln) {
  name: text
}

op addTeam($name: text, $seats: int) =
  set teams { name: $name, seats: $seats }

op addPlain($name: text) =
  set plain { name: $name }

op topTeams($n: int <= 100) =
  teams[name, seats]
  sort -seats
  take $n
"#;
    let src = Sources::from_parts(vec![("bounded.ordo".into(), SRC.to_string())]);
    let (schema, registry) = load_sources(&src).expect("the bounded program compiles");
    let art = program_artifact(&schema, &registry).expect("plans");
    let ops: Vec<Value> = art["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .map(catalog_view)
        .collect();
    let doc = interface_document(&art["domain"], &entity_artifacts(&schema), &ops);
    let refusals = |n: &str| -> BTreeSet<String> {
        doc["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .find(|o| o["op"] == n)
            .unwrap_or_else(|| panic!("no op `{n}`"))["refusals"]
            .as_array()
            .expect("refusals")
            .iter()
            .filter_map(|r| r.as_str().map(str::to_string))
            .collect()
    };
    assert!(
        refusals("addTeam").contains("check_violation"),
        "a write onto an entity with a declared invariant must say the invariant can refuse it"
    );
    assert!(
        !refusals("addPlain").contains("check_violation"),
        "an entity with no invariant advertised one anyway"
    );
    assert!(
        refusals("topTeams").contains("bound_violation"),
        "a declared `<=` bound is the only thing a caller can violate, and it must say so"
    );
    assert!(
        !refusals("addPlain").contains("bound_violation"),
        "an op with no declared bound advertised a bound violation"
    );
    // And the declaration itself reached the descriptor, which is what the
    // affordance is derived from.
    let teams = doc["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|e| e["entity"] == "teams")
        .expect("teams");
    assert_eq!(
        teams["checks"].as_array().expect("checks").len(),
        1,
        "the row invariant did not reach the document"
    );
    assert_eq!(teams["checks"][0]["source"], "used<=seats");

    // ── the bounds themselves, not only the affordance they imply ──
    //
    // `params[].max` had never been populated in any emitted document (the
    // market fixture declares no bound), and `min` did not exist at all — so
    // "wire the declared bounds through" had to be witnessed here or nowhere.
    // Both bounds on `$n` are asserted, and they come from different places
    // on purpose: `max` transcribes the author's `<= 100`, `min` is DERIVED
    // from the `take $n` construct by `ordo_plan::artifact::take_count_param`
    // — the same function `ordo_host::param_bounds` hands the door, so the
    // number refused below and the number disclosed are one number.
    let param = |op: &str, name: &str| -> Value {
        doc["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .find(|o| o["op"] == op)
            .unwrap_or_else(|| panic!("no op `{op}`"))["params"]
            .as_array()
            .expect("params")
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("no param ${name} on `{op}`"))
            .clone()
    };
    let n = param("topTeams", "n");
    assert_eq!(
        n["max"], "100",
        "the declared `<= 100` did not reach a form"
    );
    assert_eq!(
        n["min"], "0",
        "a `take` count has a derived floor the door enforces — `LIMIT -1` is \
         an error and `take[keys] -1` is a silently empty page — and a \
         document that states only the ceiling describes half the bound"
    );
    // A parameter with neither is `null` on both, never absent and never
    // zero: `0` is a legal ceiling and would read as one.
    let seats = param("addTeam", "seats");
    assert_eq!(seats["max"], Value::Null);
    assert_eq!(seats["min"], Value::Null);

    // There is no `>=`. `declparse::param_bound` refuses it by name, so a
    // lower bound is only ever derived and never declared, and this asserts
    // the language boundary rather than trusting a comment about it.
    assert!(
        ordo_cli::load_sources(&Sources::from_parts(vec![(
            "lower.ordo".into(),
            "domain b\nentity t (key: t) { n: int }\nop f($x: int >= 1) = t[n]\n".to_string(),
        )]))
        .is_err(),
        "if `>=` ever parses, `min` stops being derivation-only and this \
         document owes the declared form too"
    );
}

// ── direction 2: nothing settled is omitted ──────────────────────────────

/// The keys §B1 and `ui-gen.md` §1–§4 settle, present on every op.
///
/// A written-down list, deliberately: the point of this direction is to fail
/// when a refactor quietly drops a key, and a list derived from the code
/// would be dropped along with it.
#[test]
fn every_op_carries_every_settled_key() {
    let (_, doc) = program();
    let ops = doc["ops"].as_array().expect("ops");
    assert!(ops.len() > 50, "the fixture registers the whole corpus");
    for o in ops {
        for k in [
            "op",
            "version",
            "kind",
            "platform",
            "call",
            "mode",
            "statements",
            "params",
            "result",
            "cost",
            "refine",
            "disclosures",
            "refusals",
            "error",
        ] {
            assert!(o.get(k).is_some(), "op {} is missing `{k}`", o["op"]);
        }
        for k in ["method", "terminal", "cas", "refinable", "live", "paged"] {
            assert!(o["call"].get(k).is_some(), "{} call.{k}", o["op"]);
        }
        for k in [
            "outputs",
            "row_identity",
            "synthetic_rows",
            "single_row",
            "shape",
        ] {
            assert!(o["result"].get(k).is_some(), "{} result.{k}", o["op"]);
        }
        for p in o["params"].as_array().expect("params") {
            for k in [
                "name",
                "type",
                "required",
                "max",
                "min",
                "id_entity",
                "field_ref",
                "enum_values",
            ] {
                assert!(p.get(k).is_some(), "{} ${} {k}", o["op"], p["name"]);
            }
        }
        for st in o["statements"].as_array().expect("statements") {
            for k in ["entity", "verb", "label", "summary", "cas_param"] {
                assert!(st.get(k).is_some(), "{} statement {k}", o["op"]);
            }
        }
        for k in [
            "unguarded",
            "unbounded",
            "bypassed_guards",
            "guards",
            "key_release",
        ] {
            assert!(
                o["disclosures"].get(k).is_some(),
                "{} disclosures.{k}",
                o["op"]
            );
        }
    }
}

/// The same, for entities: the facts a form generator and a picker need.
#[test]
fn every_entity_carries_every_settled_key() {
    let (_, doc) = program();
    for e in doc["entities"].as_array().expect("entities") {
        for k in [
            "entity",
            "key",
            "tree",
            "acl_levels",
            "guarded",
            "unique_keys",
            "checks",
            "referenced_by",
            "fields",
        ] {
            assert!(
                e.get(k).is_some(),
                "entity {} is missing `{k}`",
                e["entity"]
            );
        }
        for f in e["fields"].as_array().expect("fields") {
            for k in [
                "name",
                "type",
                "ref",
                "owned",
                "tree",
                "optional",
                "unique",
                "sealed",
                "indexed",
                "refinable",
                "default",
                "normalize",
                "enum_values",
                "align",
            ] {
                assert!(
                    f.get(k).is_some(),
                    "{}.{} is missing `{k}`",
                    e["entity"],
                    f["name"]
                );
            }
        }
    }
}

/// §B3's terminal-verb contract, stamped rather than guessable.
///
/// The whole point is that a renderer needs **no name heuristics**: an op
/// called `archiveProject` and one called `std.projects.delete` both state
/// their kind, their method and the verb a caller's builder must end in.
///
/// Three of the six fields are per-OP rather than per-kind as of
/// `ordo.interface/2`, and each is asserted against the fact it derives from
/// rather than against a constant — a constant is what let `cas: true` and
/// `paged: true` stand on ops that could do neither:
///
/// - `cas` ⟺ some statement declares a `cas_param`.
/// - `refinable` ⟺ `refine.tier != "none"`.
/// - `paged` / the `pages` verb ⟺ the result is not single-row.
#[test]
fn the_terminal_verb_contract_is_stamped_on_every_op() {
    let (_, doc) = program();
    let mut kinds = BTreeSet::new();
    let (mut single, mut cas_ops, mut plain) = (0, 0, 0);
    for o in doc["ops"].as_array().expect("ops") {
        let call = &o["call"];
        let name = o["op"].as_str().expect("a name");
        // The three derivations, asserted for every kind at once so no arm
        // can quietly go back to a constant.
        let asserts_rev = o["statements"]
            .as_array()
            .expect("statements")
            .iter()
            .any(|s| s["cas_param"].is_string());
        assert_eq!(call["cas"], asserts_rev, "`{name}` cas ⟺ a declared rev");
        assert_eq!(
            call["refinable"],
            o["refine"]["tier"] != "none",
            "`{name}` refinable ⟺ a granted tier — the two may not disagree"
        );
        if asserts_rev {
            cas_ops += 1;
        }

        match o["kind"].as_str().expect("a kind") {
            "read" => {
                assert_eq!(call["method"], "GET");
                assert_eq!(call["cas"], false, "a read commits nothing");
                assert_eq!(call["live"], true);
                // A single-row op is answered whole: no cursor, no `next`,
                // and `.pages()` on one could only be refused.
                let one = o["result"]["single_row"].as_bool().expect("single_row");
                assert_eq!(call["paged"], !one, "`{name}` paged ⟺ not single-row");
                assert_eq!(
                    call["terminal"],
                    if one {
                        serde_json::json!(["fetch", "live"])
                    } else {
                        serde_json::json!(["fetch", "live", "pages"])
                    },
                    "`{name}` terminal verbs must agree with its result shape"
                );
                if one {
                    single += 1;
                } else {
                    plain += 1;
                }
            }
            "write" => {
                assert_eq!(call["method"], "POST");
                assert_eq!(call["terminal"], serde_json::json!(["commit"]));
                assert_eq!(call["refinable"], false, "a write has no refinement");
                assert_eq!(call["live"], false);
                assert_eq!(call["paged"], false);
            }
            "delete" => {
                assert_eq!(call["method"], "DELETE");
                assert_eq!(call["terminal"], serde_json::json!(["commit"]));
                // A delete asserts no revision — there is no `rev` to put in
                // a `del`'s braces — so CAS is off for every one of them.
                assert_eq!(call["cas"], false, "`{name}`: a delete cannot CAS");
            }
            other => panic!("unknown op kind `{other}`"),
        }
        kinds.insert(o["kind"].as_str().expect("a kind").to_string());
    }
    assert_eq!(
        kinds,
        ["delete", "read", "write"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        "the fixture must exercise all three kinds or this proves one third of the contract"
    );
    // Non-vacuity, per derivation: a fixture with no single-row op, or none
    // that CAS, would pass every assertion above without proving one.
    assert!(single > 0 && plain > 0, "both paging shapes must occur");
    assert!(cas_ops > 0, "the fixture must contain a CAS op");
    assert!(
        cas_ops
            < doc["ops"]
                .as_array()
                .expect("ops")
                .iter()
                .filter(|o| o["kind"] != "read")
                .count(),
        "and a mutation that is NOT one, or `cas` is still `kind != read`"
    );
}

/// The derived `std.E.*` families carry their mode labels through to the
/// descriptor, which is what lets a renderer say *"this creates"* without
/// parsing the op's name.
#[test]
fn derived_std_ops_surface_their_mode_labels() {
    let (_, doc) = program();
    let find = |n: &str| {
        doc["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .find(|o| o["op"] == n)
            .unwrap_or_else(|| panic!("no op `{n}`"))
            .clone()
    };
    assert_eq!(find("std.tasks.create")["mode"], "creates");
    assert_eq!(find("std.tasks.update")["mode"], "converges");
    assert_eq!(find("std.tasks.delete")["mode"], "deletes");
    assert_eq!(find("std.tasks.delete")["kind"], "delete");
    assert_eq!(
        find("std.tasks.list")["mode"],
        Value::Null,
        "a read has none"
    );
}

/// `ui-gen.md` §1's row identity and §5's structural metric test.
#[test]
fn row_identity_and_metric_detection_are_structural() {
    let (_, doc) = program();
    let find = |n: &str| {
        doc["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .find(|o| o["op"] == n)
            .unwrap_or_else(|| panic!("no op `{n}`"))
            .clone()
    };
    // A list carries the anchor's typed id, whose key is the prefix every
    // one of its ids is written with.
    let list = find("std.tasks.list");
    assert_eq!(list["result"]["row_identity"]["entity"], "tasks");
    assert_eq!(list["result"]["row_identity"]["key"], "task");
    assert_eq!(list["result"]["synthetic_rows"], false);
    assert_eq!(list["result"]["single_row"], false);
    // It projects its id, so a device can keep it in sync, keyed by `id`.
    assert_eq!(
        list["result"]["sync"],
        serde_json::json!({ "syncable": true, "id": "id" })
    );
    // A list that does not project its id syncs anyway, keyed by the id the
    // executor attaches (`ui-gen.md` §1: row identity rides every list row
    // even when unprojected). Until 2026-09-16 this was a refusal naming
    // `id_not_projected`, a rule that no longer exists.
    assert_eq!(
        find("taskBoard")["result"]["sync"],
        serde_json::json!({ "syncable": true, "id": "$id" })
    );
    // And the attached key is described like any other output, so a renderer
    // can link a row without parsing a prefix out of a value it never got.
    let board = find("taskBoard");
    let attached = board["result"]["outputs"]
        .as_array()
        .expect("outputs")
        .iter()
        .find(|o| o["name"] == "$id")
        .expect("the attached row key is described");
    assert_eq!(attached["type"], "id");
    assert_eq!(attached["id_entity"], "tasks");

    // `reduce` — a keyless group — is provably one row, so it is a metric,
    // and its rows are synthetic: no identity, no detail link, no actions.
    let count = find("std.tasks.count");
    assert_eq!(count["result"]["single_row"], true);
    assert_eq!(count["result"]["synthetic_rows"], true);
    assert_eq!(count["result"]["row_identity"], Value::Null);
    assert_eq!(
        count["result"]["sync"],
        serde_json::json!({ "syncable": false, "reason": "measure" })
    );

    // A write has no result rows at all.
    let close = find("closeTask");
    assert_eq!(close["result"]["row_identity"], Value::Null);
    assert_eq!(close["result"]["outputs"], serde_json::json!([]));
    assert_eq!(
        close["result"]["sync"],
        serde_json::json!({ "syncable": false, "reason": "not_a_read" })
    );
}

// ── identity ─────────────────────────────────────────────────────────────

/// Byte-stability and the digest, which is the identity discipline every
/// other document here already has.
#[test]
fn the_descriptor_is_byte_stable_and_self_describing() {
    let (_, a) = program();
    let (_, b) = program();
    assert_eq!(
        ordo_plan::artifact::to_bytes(&a),
        ordo_plan::artifact::to_bytes(&b),
        "two independent compilations produced two descriptors"
    );
    assert_eq!(a["schema_version"], SCHEMA_VERSION);
    // Recomputable by a reader: strip the key, hash, compare.
    assert_eq!(a["digest"], digest(&a));
    assert_eq!(
        a["digest"].as_str().expect("a digest").len(),
        16,
        "the digest's width matches the op and program versions it sits beside"
    );
}

/// The digest moves when the contract moves. A digest that did not would let
/// a generated client believe it was current across a real change — which is
/// exactly what `program_version` hashing only the document (and not the DDL)
/// allowed, and what `plan-schema-evolution.md` §1.1(a) had to close.
#[test]
fn the_digest_moves_when_anything_in_the_document_moves() {
    let (_, doc) = program();
    let mut changed = doc.clone();
    changed["ops"][0]["mode"] = Value::from("something else");
    assert_ne!(digest(&doc), digest(&changed));
}

/// A scoped descriptor is the same function with fewer ops — and everything
/// derived across ops follows the filter. This is the mechanism `ordo-host`'s
/// route relies on, proved here where it needs no database.
#[test]
fn scoping_the_op_list_scopes_everything_derived_from_it() {
    let (art, entities, all) = parts();

    // One read op over `tasks` and nothing else.
    let narrow: Vec<Value> = all
        .iter()
        .filter(|m| m["op"] == "std.tasks.list")
        .cloned()
        .collect();
    let wide = interface_document(&art["domain"], &entities, &all);
    let thin = interface_document(&art["domain"], &entities, &narrow);

    assert_eq!(thin["ops"].as_array().expect("ops").len(), 1);
    // Entities follow ops: the thin document describes only what that op
    // reaches, and the guard on `tasks` reaches `projects` and
    // `projectMembers` — which is the honest answer, because those rows are
    // examined on the caller's behalf.
    let thin_entities: BTreeSet<String> = names(&thin["entities"], "entity").into_iter().collect();
    let wide_entities: BTreeSet<String> = names(&wide["entities"], "entity").into_iter().collect();
    assert!(thin_entities.is_subset(&wide_entities));
    assert!(
        !thin_entities.contains("orders"),
        "an entity no visible op reaches was described anyway: {thin_entities:?}"
    );
    // Action discovery follows ops: no write op is visible, so no id type
    // offers an action.
    for t in thin["id_types"].as_array().expect("id_types") {
        assert!(
            t["actions"].as_array().expect("actions").is_empty(),
            "a scoped descriptor offered an action for an op it does not describe: {t}"
        );
    }
    assert_ne!(
        thin["digest"], wide["digest"],
        "two surfaces, two contracts"
    );
}

/// The golden: the market fixture's whole descriptor, byte for byte.
///
/// Regenerate with `ORDO_UPDATE_GOLDEN=1 cargo test --workspace`, then
/// **read the diff** — this file is the contract every generated SDK, the
/// LSP and every renderer is built against, so a line that moves in it is a
/// line that moves in somebody's generated code. A golden that moves is a
/// question, not a chore.
#[test]
fn the_market_fixtures_descriptor_is_byte_stable() {
    let src = Sources::from_parts(vec![("market.ordo".into(), FIXTURE.to_string())]);
    let out = interface_json(&src).expect("the fixture describes");
    assert_golden(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/market.interface.json"),
        &out,
    );
}

/// The CLI command and the library function are the same document — the
/// route a build step takes and the route this suite takes cannot diverge.
#[test]
fn the_cli_emits_exactly_the_projection() {
    let src = Sources::from_parts(vec![("market.ordo".into(), FIXTURE.to_string())]);
    let (_, doc) = program();
    assert_eq!(
        interface_json(&src).expect("describes"),
        ordo_plan::artifact::to_bytes(&doc)
    );
}

/// Ruled 2026-10-04 (27ak): each money field declares its currency's
/// places, and the places reach the descriptor on the field, on a
/// parameter and on an output, so the SDK and the CDK know how to spell
/// `2999` minor units as `"29.99"`. Bare `money` is `money(0)`, and a
/// non-numeric type carries `null`.
#[test]
fn money_places_reach_the_descriptor() {
    let src = Sources::from_parts(vec![(
        "shop.ordo".into(),
        "entity items {\n  name: text\n  price: money(2)\n  yen: money\n}\n\
         op cheap($cap: money(2)) =\n  items(price < $cap)[name, price, yen]\n"
            .to_string(),
    )]);
    let doc: Value = serde_json::from_str(&interface_json(&src).expect("describes")).expect("JSON");
    let items = doc["entities"]
        .as_array()
        .expect("entities")
        .iter()
        .find(|e| e["entity"] == "items")
        .expect("items");
    let field = |n: &str| {
        items["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .find(|f| f["name"] == n)
            .cloned()
            .expect("field")
    };
    assert_eq!(field("price")["places"], 2);
    assert_eq!(field("yen")["places"], 0, "bare `money` is `money(0)`");
    assert!(field("name")["places"].is_null());
    let op = doc["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .find(|o| o["op"] == "cheap")
        .expect("cheap");
    assert_eq!(op["params"][0]["places"], 2);
    let outs = op["result"]["outputs"].as_array().expect("outputs");
    let out = |n: &str| {
        outs.iter()
            .find(|o| o["name"] == n)
            .expect("output")
            .clone()
    };
    assert_eq!(out("price")["places"], 2);
    assert_eq!(out("yen")["places"], 0);
    assert!(out("name")["places"].is_null());
}
