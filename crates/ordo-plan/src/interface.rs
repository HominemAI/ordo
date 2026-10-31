//! B1 — the **Interface Descriptor** (`interface.json`): the one data
//! contract the generated SDK, the LSP and generated UI all hang off.
//!
//! `tooling-and-embedding-plan.md` §B1 settles what it holds — *entities
//! (fields, types, refs, nullability, sealed marks), ops (params + bounds,
//! result shape, mode label, cost class, live-capability, output-field
//! manifest), version hash* — and §B3 settles the **terminal-verb contract**
//! a caller's generated types enforce. `ui-gen.md` §1–§4 settle the rest:
//! shape-verified, catalog-scoped, typed-id action discovery,
//! CAS-by-default commit.
//!
//! # It is a projection, and that is the whole design
//!
//! Every field below is selected from the **catalog manifest**
//! ([`crate::artifact::catalog_view`]) and the **entity artifacts**
//! ([`crate::artifact::entity_artifacts`]) — both projections of
//! [`crate::artifact::op_artifact`], which is the deployment's one producer
//! of what an op is. Nothing here re-walks the compiler. That rule is not
//! taste: item 9m collapsed a second walk over the same compiler output
//! because *two walks are two answers waiting to differ*, and it had already
//! happened once. A descriptor derived independently would be a third.
//!
//! So this module takes **JSON in and JSON out**. It cannot reach a schema,
//! a registry or a binder even by accident, which is what makes "derived
//! truth" checkable rather than asserted: `tests/interface.rs` walks every
//! emitted value back to the manifest key it came from, in both directions.
//!
//! Where the descriptor needed a fact the producer did not carry, the
//! producer grew it (`params[].id_entity`, `outputs[].refinable`,
//! `outputs[].id_entity`, `result`, `entities[].checks`) rather than this
//! module deriving it. That is the same rule stated from the other side, and
//! `ordo.interface/2` applied it again for the seven facts the first renderer
//! reported missing: `params[].field_ref` and `params[].min`,
//! `statements[].cas_param` and `statements[].summary`, `result.shape`.
//!
//! The three this module derives instead — `params[].enum_values`,
//! `id_types[].list` and `id_types[].get` — are derived here **because they
//! are cross-op or cross-document joins**, and the producer works one op at a
//! time. `enum_values` resolves a `field_ref` against the entity half of THIS
//! document; the two index halves join every visible op's `row_identity`
//! against every visible op's parameters. Both are joins over the scoped set,
//! which is exactly the thing that must not be computed before the filter.
//!
//! # Catalog scoping
//!
//! [`interface_document`] takes the op manifests it is given and describes
//! exactly those. A deployment hands it the manifests that survived the
//! asker's access check — the same filter the catalog route runs, applied
//! once — so *the descriptor a caller receives contains exactly the ops
//! their catalog shows*, by construction rather than by a second rule that
//! could drift out of step with the first.
//!
//! Everything derived across ops is derived **after** that filter: the
//! typed-id action index names only visible ops, so a viewer cannot learn
//! that a manager-only `archiveProject` exists by noticing which id types
//! offer actions. And refusal metadata is derived from the op's own
//! declaration, never from a lookup that could answer about an op outside
//! the asker's catalog — the rule `std.space.callersOf` follows.
//!
//! # What is NOT here
//!
//! The **reserved UI annotations** (`label`, `desc`, `order`, `choices`,
//! `danger`). `ui-gen.md` §1 puts them in the *surface* descriptor
//! (`surfaces.json`), which is a projection of this file **plus** them, and
//! `definition-language.md` §10.3 marks them inert — the grammar does not
//! lex one of them today. Emitting an always-null `danger` here would be a
//! producer-less field that a renderer would learn to trust.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::artifact::to_bytes;

/// The descriptor's own contract version.
///
/// Separate from a program's version hash, and it answers a different
/// question: the hash says *which program*, this says *which shape of
/// document*. A generated SDK pins the program hash to detect drift
/// (§B5) and reads this to know whether it can parse the file at all.
///
/// It moves on a **breaking** change to the document's shape — a removed
/// key, a changed meaning, a narrowed value set. Adding a key is not
/// breaking: every consumer of a JSON document already ignores keys it does
/// not know, and bumping for additions would make every consumer's version
/// check a lie about compatibility.
///
/// # `/2`, and why the additive half did not get to stay at `/1`
///
/// The first consumer (`clients/ordo-react`, `ui-gen.md` §2) reported seven
/// facts it needed and could not read. Four are **additions** and would have
/// been `/1` on their own: `id_types[].list` / `id_types[].get`,
/// `params[].enum_values` / `params[].field_ref`, `params[].min`,
/// `result.shape` with `statements[].summary`.
///
/// Three are not, and each is the *changed meaning* case exactly:
///
/// - **`call.cas`** said `true` for every write and delete, including ops
///   with no `rev` parameter. It meant "is a mutation"; it now means "asserts
///   a revision", which is what `ui-gen.md` §1 uses the word for. A `/1`
///   reader that branched on it now takes the other branch.
/// - **`call.refinable`** said `true` for every read while `refine.tier` was
///   `none` for every op, so the two disagreed by construction and the
///   consumer worked around it. It now derives *from* the tier and is false
///   wherever the door refuses `$refine`. Same key, opposite value.
/// - **`call.terminal` / `call.paged`** advertised `pages` and `true` on
///   single-row ops the executor answers whole. The value set narrowed.
///
/// Those three cannot ride as additions under any spelling: a reader that
/// keeps believing `/1`'s meanings gets wrong answers rather than missing
/// ones, which is the distinction the paragraph above draws. So the document
/// moves once, to `/2`, and the additive four ride with it — splitting them
/// across two versions would mean emitting a document that is `/1` for some
/// keys and `/2` for others, which is not a thing a version can say.
pub const SCHEMA_VERSION: &str = "ordo.interface/2";

/// Build the descriptor from a program's entity artifacts and the op
/// manifests a caller may see.
///
/// `entities` is [`crate::artifact::entity_artifacts`]; `ops` is the
/// catalog manifests ([`crate::artifact::catalog_view`]), already filtered
/// to the asker. Both are in the order their producer emitted them —
/// declaration order for entities, op-name order for ops — and this
/// function adds no sort of its own, so the document is byte-stable for the
/// same reason the artifact is.
pub fn interface_document(domain: &Value, entities: &[Value], ops: &[Value]) -> Value {
    interface_document_with(domain, entities, ops, RefineLimits::default())
}

/// How large one refinement may be, as the descriptor states it
/// (`ops[].refine.limits`), so a client checks a refinement before the
/// wire. The door's own copy is `ordo_api::RefineLimits`, set by
/// `ORDO_REFINE_MAX_CONDITIONS` and `ORDO_REFINE_MAX_IN_VALUES` (ruled
/// 2026-10-05, `refinement-default` ask 6); `ordo-host` passes the
/// deployment's values to [`interface_document_with`] and asserts the two
/// defaults agree. A descriptor emitted with no deployment (`ordo
/// interface`, `ordo gen`) states the shipped defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefineLimits {
    pub conditions: usize,
    pub in_values: usize,
}

impl Default for RefineLimits {
    fn default() -> Self {
        RefineLimits {
            conditions: 16,
            in_values: 200,
        }
    }
}

/// [`interface_document`] with the deployment's refinement limits.
pub fn interface_document_with(
    domain: &Value,
    entities: &[Value],
    ops: &[Value],
    limits: RefineLimits,
) -> Value {
    // Which entities this asker may be told about: the ones a visible op
    // reaches. An entity no visible op touches is not described — a
    // descriptor is metadata, but an enumeration of every shape in the
    // deployment is still an enumeration, and the catalog refuses exactly
    // that one op at a time.
    let reached: std::collections::BTreeSet<String> = ops
        .iter()
        .flat_map(|o| o["entities"].as_array().cloned().unwrap_or_default())
        .filter_map(|e| e.as_str().map(str::to_string))
        .collect();

    let entity_docs: Vec<Value> = entities
        .iter()
        .filter(|e| e["entity"].as_str().is_some_and(|n| reached.contains(n)))
        .map(|e| entity_descriptor(e, entities))
        .collect();
    let op_docs: Vec<Value> = ops
        .iter()
        .map(|o| op_descriptor(o, entities, ops, limits))
        .collect();

    let mut m = Map::new();
    m.insert("schema_version".into(), json!(SCHEMA_VERSION));
    m.insert("domain".into(), domain.clone());
    m.insert("entities".into(), json!(entity_docs));
    m.insert("id_types".into(), id_types(&op_docs, entities));
    m.insert("ops".into(), json!(op_docs));

    // The digest is over everything above, with the key absent — the
    // identity discipline `op_object` and `program_version` already use, so
    // a descriptor's identity moves exactly when a reviewer's diff of it
    // moves. 16 hex characters, matching the op and program versions,
    // because the three appear beside each other and different widths there
    // read as different kinds of thing.
    let mut value = Value::Object(m);
    value["digest"] = json!(digest(&value));
    value
}

/// The descriptor's digest: `sha256(to_bytes(doc))`, first 8 bytes, hex.
///
/// Computed over the document **without** its own `digest` key, which is
/// what makes it recomputable by a reader: strip the key, hash, compare.
pub fn digest(doc: &Value) -> String {
    let mut bare = doc.clone();
    if let Some(m) = bare.as_object_mut() {
        m.remove("digest");
    }
    let d = Sha256::digest(to_bytes(&bare).as_bytes());
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

// ── entities ─────────────────────────────────────────────────────────────

/// One entity, as a form generator and a picker read it.
///
/// `enum_values` is what makes a dropdown derivable and `ref` is what makes
/// a picker derivable — both are already in the artifact (the first since
/// `plan-schema-evolution.md` §1.1(a) closed the silent-no-op deploy), so
/// this selects rather than computes.
fn entity_descriptor(e: &Value, all: &[Value]) -> Value {
    let fields: Vec<Value> = e["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|f| {
            let ty = &f["type"];
            let scalar = match ty["kind"].as_str() {
                Some("ref") => "id",
                _ => ty["scalar"].as_str().unwrap_or("text"),
            };
            json!({
                "name": f["name"],
                // The **closed type inventory is the format hint** — the
                // vocabulary `ui-gen.md` §1 names, and deliberately not a
                // second metadata channel beside it.
                "type": scalar,
                // A picker is derivable from this and nothing else.
                "ref": match ty["kind"].as_str() {
                    Some("ref") => ty["target"].clone(),
                    _ => Value::Null,
                },
                "owned": ty["owned"].as_bool().unwrap_or(false),
                "tree": ty["tree"].as_bool().unwrap_or(false),
                "optional": f["optional"],
                "unique": f["unique"],
                "sealed": f["sealed"],
                "indexed": f["indexed"],
                "refinable": f["sealed"].as_bool().unwrap_or(false)
                    && f["refinable"].as_bool().unwrap_or(false)
                    || !f["sealed"].as_bool().unwrap_or(false),
                "default": f["default"],
                "normalize": f["normalize"],
                // A dropdown is derivable from this and nothing else.
                "enum_values": f["enum_values"],
                // A `maxlength` is derivable from this and nothing else.
                "text_max": f["text_max"],
                // How many decimals a `money(p)` or `decimal(p)` field
                // shows and accepts (27ak); `null` otherwise.
                "places": f["places"],
                "align": align(scalar),
            })
        })
        .collect();
    json!({
        "entity": e["entity"],
        "key": e["key"],
        "tree": e["tree"],
        "acl_levels": e["acl_levels"],
        // Whether rows are guard-filtered at all. The guard's TEXT is an
        // op-level disclosure (it rides every op that reaches this entity,
        // with the provenance that makes a consent page honest); at entity
        // grain the renderable fact is only that one exists, because "this
        // list may be shorter than the table" is what a UI has to say.
        "guarded": !e["guards"].is_null(),
        "unique_keys": e["unique_keys"],
        "checks": e["checks"],
        // Entities whose rows point at this one, with the field that does
        // it: the reverse of `ref`, which is what a detail page needs to
        // offer "related tasks" without a second index of the schema.
        "referenced_by": referenced_by(e["entity"].as_str().unwrap_or(""), all),
        "fields": fields,
    })
}

/// `[{entity, field, owned}]` — every ref in the program that targets
/// `name`. Derived from the entity list rather than from a schema walk, for
/// the reason the module doc gives.
fn referenced_by(name: &str, all: &[Value]) -> Value {
    let mut out = Vec::new();
    for e in all {
        for f in e["fields"].as_array().cloned().unwrap_or_default() {
            if f["type"]["kind"] == "ref" && f["type"]["target"] == name {
                out.push(json!({
                    "entity": e["entity"],
                    "field": f["name"],
                    "owned": f["type"]["owned"],
                }));
            }
        }
    }
    Value::Array(out)
}

/// `ui-gen.md` §1: numeric and money right, dates tabular, text left. It
/// derives from the type, so it is a rendering of a fact rather than a
/// second fact — but it is derived once, here, so eight skins do not each
/// derive it slightly differently.
fn align(ty: &str) -> &'static str {
    match ty {
        "int" | "money" | "decimal" => "right",
        "date" | "datetime" | "duration" => "tabular",
        // A blob is not prose and must not be laid out as prose: a cell
        // holding a file renders as an affordance — a thumbnail, a name, a
        // download — and centring is what says "this is an object, not a
        // sentence". Left-aligning it, which the wildcard did, produced a
        // 32-character hex handle sitting in a column beside people's
        // names, which is both ugly and a disclosure of the handle to
        // every reader of the table.
        "blob" => "center",
        // A `uri` IS prose-shaped — it is a string a reader reads — so it
        // takes the same alignment text does. Named rather than left to
        // the wildcard, because "it happens to land on the default" and
        // "it was decided to be the default" look identical afterwards.
        "uri" => "left",
        // A document is structure, laid out as a block from the left.
        "json" => "left",
        _ => "left",
    }
}

// ── ops ──────────────────────────────────────────────────────────────────

fn op_descriptor(m: &Value, entities: &[Value], ops: &[Value], limits: RefineLimits) -> Value {
    let kind = m["kind"].as_str().unwrap_or("read");
    let read = kind == "read";
    let outputs: Vec<Value> = m["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|o| {
            json!({
                "name": o["name"],
                // Verbatim, `null` included: an untyped column is a platform
                // op's, and calling it `text` here would be this document
                // inventing the one fact its producer could not supply.
                "type": o["type"],
                // `money(2)` arrives as `"29.99"` and a decimal as its
                // digits; the places say how many follow the point (27ak).
                "places": o["places"],
                "sealed": o["sealed"],
                "refinable": o["refinable"],
                "id_entity": o["id_entity"],
                "align": align(o["type"].as_str().unwrap_or("text")),
            })
        })
        .collect();
    let grouped = m["result"]["grouped"].as_bool().unwrap_or(false);
    let single_row = m["result"]["single_row"].as_bool().unwrap_or(false);
    let mut refine = refine(m, ops);
    if refine["tier"] != "none" {
        refine["limits"] = json!({
            "conditions": limits.conditions,
            "in_values": limits.in_values,
        });
    }
    let params: Vec<Value> = m["params"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|p| param_descriptor(p, entities))
        .collect();
    let mut out = json!({
        "op": m["op"],
        "version": m["version"],
        "kind": kind,
        // `true` for an op the platform serves that no program compiled
        // (`ordo_host::platform_ops`). It rides because the evidence behind
        // the two is not the same kind of evidence — a compiled op's shape
        // was proved by the binder and verified against the database, and a
        // platform op's is asserted by this binary — and because a platform
        // op's result columns carry no declared type, which a generator has
        // to know before it emits a record for them.
        "platform": m["platform"].as_bool().unwrap_or(false),
        // §B3's terminal-verb contract, stamped so a renderer needs no name
        // heuristics: what the caller's generated builder must end in, which
        // HTTP method the wire pins to this kind, and whether a commit
        // carries the fetched row's `rev`.
        "call": call_contract(m, &refine, single_row),
        // The derived `std.E.*` families already carry a mode label; this is
        // where it surfaces. §6 rule 1's per-statement list rides beside the
        // op-level union, so "creates one row and converges two others" is
        // readable as what it is rather than as one word.
        "mode": m["mode"],
        "statements": m["statements"],
        "params": json!(params),
        "result": {
            "outputs": outputs,
            // `ui-gen.md` §1, *row identity*: the anchor entity's implicit
            // typed id rides every list row even when unprojected, because
            // it already underwrites the cursor. A **grouped** result has
            // none — its rows are synthetic, so no identity, no detail
            // link, no actions.
            // Derived from `sync` rather than from `(read, grouped)` alone
            // (2026-09-16). Both answer "do these rows have an identity", and
            // computed separately the document carried two answers:
            // `row_identity` was claimed for `distinct` results and many-side
            // joins, whose rows are output tuples and fanned-out duplicates,
            // while `sync` refused them by name.
            "row_identity": match m["result"]["sync"]["syncable"].as_bool() {
                Some(true) => entity_key(m["result"]["anchor"].as_str(), entities),
                _ => Value::Null,
            },
            "synthetic_rows": grouped,
            "single_row": single_row,
            // Whether a device can keep this op in sync, from the artifact's
            // `result.sync` (`plan-replica-feed.md` §1). An op whose manifest
            // carries none is a write or a platform op, and says which.
            "sync": match &m["result"]["sync"] {
                Value::Null => json!({
                    "syncable": false,
                    "reason": if read { "no_anchor" } else { "not_a_read" },
                }),
                s => s.clone(),
            },
            // **The order the rows arrive in** (27o, ruled 2026-10-01), so
            // the device copy returns them in it: `{ keys: [{ path, desc }],
            // stated }`, the keys naming outputs with the id last.
            // `stated: false` where the language cannot state it (a grouped
            // result, a sort key the device cannot evaluate), and then the
            // keys are the id alone. From the producer
            // (`ordo_plan::artifact::read_order`); `null` for a write and
            // for a platform op, whose rows no compiled plan ordered.
            "order": m["result"]["order"].clone(),
            // **What the response's `rows` array holds**, from the producer
            // (`ordo_plan::artifact::stmt_summary`) rather than from `kind`:
            // `rows` for a read and for a platform op that returns declared
            // columns, `write_summary` for a compiled write or delete.
            //
            // This is the descriptor's answer to *"does my form get the new
            // `rev` back?"*, and the answer is **no**. A compiled write
            // returns one summary object per statement — `{entity}` plus one
            // of `created`/`converged` (an id) or `updated`/`deleted` (a
            // count), stated per statement in `statements[].summary` — and
            // never the written row. `api-engine-design.md` §3 says
            // otherwise; the executor is what this document describes, and
            // the divergence is recorded as an engine finding rather than
            // smoothed over here.
            "shape": m["result"]["shape"],
        },
        "cost": m["cost"],
        "refine": refine,
        "disclosures": {
            "unguarded": m["unguarded"],
            "unbounded": m["unbounded"],
            "bypassed_guards": m["bypassed_guards"],
            "guards": m["guards"],
            "key_release": match m["release"]["purpose"].as_str() {
                Some(_) => m["release"].clone(),
                None => Value::Null,
            },
        },
        "refusals": refusals(m, entities, ops),
        // An op that does not compile is described with its refusal rather
        // than omitted — the catalog's rule, kept, because a generator that
        // silently skips an op produces an SDK missing a function nobody
        // can explain.
        "error": m["error"],
    });
    // A visitor with no token may call it (`design-public-path.md` §1): the
    // generated client sends no token and no space for it. Absent when
    // false, so a program with no public op describes itself as it did.
    if m["public"].as_bool().unwrap_or(false) {
        out["public"] = json!(true);
    }
    out
}

/// §B3, the call-site kind contract: reads are lazy builders that must
/// terminate in a read verb; writes terminate in `commit()` and have no read
/// verbs and no refinement. The wire method follows from the derived kind.
///
/// The method strings mirror `ordo_api::router::method_for`, which is the
/// authority the door enforces — `ordo-api` depends on no sibling crate, so
/// the mirror is unavoidable and `ordo-host`'s `tests/interface_surface.rs`
/// joins the two inventories over the whole cross product, exactly as
/// `tests/cost_class.rs` does for the cost classes.
///
/// # Three fields that were about the KIND and are now about the OP
///
/// Every field here used to be a function of `kind` alone, which made three
/// of them say things about individual ops that were not true:
///
/// - **`cas`** was `true` for every write and delete, including a
///   `std.E.delete($id)` that asserts no revision at all — so it meant "is a
///   mutation", which `kind` already says, and a runtime looking for the
///   *revision* had to find it by parameter name. It now derives from
///   `statements[].cas_param`: true iff some statement asserts a
///   caller-supplied `rev`. `ui-gen.md` §1's CAS-by-default is unweakened —
///   every op that *can* compare-and-swap still does, by declaration — and
///   the ops that lose the flag are the ones that never could.
/// - **`refinable`** was `true` for every read while [`refine`] answered tier
///   `none` for every op. Both were defensible in isolation (structural
///   shape vs. granted permission) and together they were a trap the first
///   consumer had to work around in prose. There is one refinement answer
///   now, and this is a projection of it: `refinable` is `tier != "none"`,
///   so the two cannot disagree. The structural half is not lost — it is
///   `kind == "read"`, which is right there.
/// - **`terminal` / `paged`** advertised `pages` on a single-row op. The
///   executor does not page one: `Executor::read_page_as` falls back to an
///   unpaged read when the plan has no ordering (a keyless `reduce` is
///   provably one row), answers `next: null`, and refuses a cursor presented
///   against it as `invalid_cursor`. The call contract and the result
///   contract now agree that there is no second page.
fn call_contract(m: &Value, refine: &Value, single_row: bool) -> Value {
    let kind = m["kind"].as_str().unwrap_or("read");
    // The revision assertion, per the producer. A `del` carries none, a
    // create-once asserts an absence (`expected_absent`, not `cas_miss`),
    // and only `rev: $param` in a `set`'s braces is a compare-and-swap.
    let cas = m["statements"]
        .as_array()
        .is_some_and(|ss| ss.iter().any(|st| st["cas_param"].is_string()));
    let refinable = refine["tier"].as_str().is_some_and(|t| t != "none");
    match kind {
        "write" | "delete" => json!({
            "method": if kind == "write" { "POST" } else { "DELETE" },
            "terminal": ["commit"],
            "cas": cas,
            "refinable": false,
            "live": false,
            "paged": false,
        }),
        _ => json!({
            "method": "GET",
            // A single-row op has no cursor to seek by and needs none, so
            // `.pages()` on one is a call the door could only refuse.
            "terminal": if single_row {
                json!(["fetch", "live"])
            } else {
                json!(["fetch", "live", "pages"])
            },
            "cas": false,
            "refinable": refinable,
            // Liveness is not a property an op declares (DL §5): *named ⇒
            // callable ⇒ sourceable ⇒ subscribable, there is no definition
            // flag*. So the only question the descriptor answers is whether
            // the op is a query at all; `?live=1` is a call-time choice, and
            // whether a subscription can see another node's writes is the
            // bus's job, not this document's.
            "live": true,
            // Every read but a single-row one is served one keyset page at a
            // time, so a runtime pages to exhaustion and stops only on a
            // missing `next` (`ui-gen.md` §2 — a short or empty page is
            // ordinary).
            "paged": !single_row,
        }),
    }
}

/// One parameter, as a form generator reads it.
///
/// The producer's object, plus the one fact it could not carry: **the value
/// set**. `enum_values` is declared on the entity FIELD, and this module has
/// always said of it that "a dropdown is derivable from this and nothing
/// else" — while the parameter that fills such a field is spelled `text`,
/// with nothing pointing back. So the first consumer joined
/// `statements[].entity` against a same-named field and refused when two
/// written entities both had one.
///
/// That join is now `params[].field_ref`, derived once by the producer from
/// the AST and the schema (which is where the answer is exact — a parameter
/// can fill a field of an entity no statement names, through a ref path or a
/// read's source condition), and resolved here into the value set itself.
/// Both ride: the `field_ref` because a form wants the field's other
/// declarations too (`normalize`, `unique`, its `check`), and the resolved
/// list because the alternative is every consumer writing one lookup.
///
/// `null` — never an empty list — where the parameter fills no declared
/// field, fills two different ones, or fills one that declares no `enum(…)`.
/// An empty list would read as *"a dropdown with no options"*.
fn param_descriptor(p: &Value, entities: &[Value]) -> Value {
    let mut m = p.as_object().cloned().unwrap_or_default();
    m.insert(
        "enum_values".into(),
        enum_values_for(&p["field_ref"], entities),
    );
    Value::Object(m)
}

/// The declared value set of the field a `field_ref` names.
fn enum_values_for(field_ref: &Value, entities: &[Value]) -> Value {
    let (Some(entity), Some(field)) = (field_ref["entity"].as_str(), field_ref["field"].as_str())
    else {
        return Value::Null;
    };
    entities
        .iter()
        .find(|e| e["entity"] == entity)
        .and_then(|e| e["fields"].as_array())
        .and_then(|fs| fs.iter().find(|f| f["name"] == field))
        .map_or(Value::Null, |f| f["enum_values"].clone())
}

/// The refinement surface, stated at both grains.
///
/// **The op-level tier is derived, never declared** (owner, 2026-10-05:
/// refinement is a standard feature with no off switch). `basic` for a
/// program's read; `none` for a write, a delete, a platform op and a public
/// read. There is no op-level word: `refinable` is a FIELD mark, and only on
/// sealed fields (`sealed indexed refinable`). The per-field list says which
/// outputs a condition may name, so a UI can grey the rest instead of
/// discovering them from a refusal.
///
/// A public read is `none` because it keeps declared parameters only: each
/// refinement is a cache key (`design-public-path.md` ask 1). Nothing marks
/// a manifest `public` yet; the key is read here so the day the public path
/// adds it, the tier follows without a second edit.
///
/// The key's meaning is unchanged from `/2` (the tier the door enforces);
/// only its value moved, so the document stays `ordo.interface/2`.
///
/// **`call.refinable` is a projection of this and not a second opinion.**
/// It used to be `true` for every read on the grounds that a read is
/// *structurally* refinable, which was true and was also a trap: the two
/// keys disagreed on every op in every emitted document, and the first
/// consumer had to write a paragraph explaining which one to believe. The
/// structural fact is `kind == "read"`; the permission is the tier; and
/// there is now exactly one place the permission is stated.
///
/// **The fields are the ones a condition may name.** The attached anchor id
/// `$id` is one (C4e, 2026-09-17: *"`$id` is the attached name, and it is
/// refinable"*): it reaches this list and the door's manifest through the
/// same outputs. `std.E.count`'s own `n` is not. A count's conditions
/// validate against `std.E.list`'s manifest (`std-library.md` §2.1, built
/// 2026-10-05), so its list is the list op's, when the asker can see that
/// op.
fn refine(m: &Value, ops: &[Value]) -> Value {
    let named = |manifest: &Value| -> Vec<Value> {
        manifest["outputs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|o| o["refinable"].as_bool().unwrap_or(false))
            .map(|o| o["name"].clone())
            .collect()
    };
    let fields: Vec<Value> = match count_pair(m) {
        Some(list) => ops
            .iter()
            .find(|o| o["op"] == list.as_str())
            .map(named)
            .unwrap_or_default(),
        None => named(m),
    };
    let read = m["kind"].as_str().unwrap_or("read") == "read";
    let platform = m["platform"].as_bool().unwrap_or(false);
    let public = m["public"].as_bool().unwrap_or(false);
    // A read that does not bind has no manifest to validate a field
    // against, and the door registers it `none`; its artifact says why in
    // `error`.
    let bound = m["error"].is_null();
    let tier = if read && !platform && !public && bound {
        "basic"
    } else {
        "none"
    };
    json!({ "tier": tier, "fields": fields })
}

/// `std.E.list` when `m` is the program's `std.E.count`.
fn count_pair(m: &Value) -> Option<String> {
    if m["platform"].as_bool().unwrap_or(false) {
        return None;
    }
    let entity = m["op"]
        .as_str()?
        .strip_prefix("std.")?
        .strip_suffix(".count")?;
    Some(format!("std.{entity}.list"))
}

/// What a refinement may do with one field, as `ordo check` prints it
/// (`refinement-default` ask 4, ruled 2026-10-05: *"Yes"*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldRefine {
    /// An open field: every operator (`eq ne lt le gt ge in is_null`).
    /// Its condition is `sql` and never raises the cost class.
    Any,
    /// A sealed field declared `sealed indexed refinable`: `eq` and `in`,
    /// answered by the blind index.
    Equality,
    /// No condition on it is served: a sealed field not opted in, a sealed
    /// field with no blind index (host work, refused at the door until
    /// host-side filtering is built), a sealed field under a count (rule 3),
    /// or a blob.
    Refused,
}

impl FieldRefine {
    /// The words `ordo check` prints after each group.
    pub fn label(self) -> &'static str {
        match self {
            FieldRefine::Any => "any operator",
            FieldRefine::Equality => "equality only",
            FieldRefine::Refused => "refused",
        }
    }
}

/// Each field a refinement's condition could name on this read, with what
/// it allows, in the order the outputs are declared. Empty for an op whose
/// tier is `none`: a write, a platform op, a public read, a read that does
/// not bind.
///
/// **One answer with the descriptor.** The fields that may be named come
/// from [`refine`], the same function that writes `ops[].refine.fields`; a
/// field that list leaves out is `Refused`. A listed field is then sorted by
/// the door's own rules, in the order `ordo_api::refine` applies them: a
/// blob takes no condition; a sealed field under a count is refused (rule
/// 3, *if you may not read the rows, you may not measure them*); a sealed
/// field rides its blind index when it has one, and is refused at the door
/// (`sealed_unsupported`) when it does not.
///
/// A count's fields are its list's, because its conditions validate against
/// `std.E.list`'s manifest (`std-library.md` §2.1). JSON in and JSON out,
/// like the rest of this module: `m` and `ops` are op artifacts or catalog
/// manifests, `entities` the entity artifacts that say which sealed field is
/// `indexed`.
pub fn refinable_fields(
    m: &Value,
    entities: &[Value],
    ops: &[Value],
) -> Vec<(String, FieldRefine)> {
    let surface = refine(m, ops);
    if surface["tier"] == "none" {
        return Vec::new();
    }
    let counted = count_pair(m);
    let source = match &counted {
        Some(list) => match ops.iter().find(|o| o["op"] == list.as_str()) {
            Some(l) => l,
            None => return Vec::new(),
        },
        None => m,
    };
    let named: Vec<&str> = surface["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    // A sealed output's declaration, found through the key the plan
    // releases for it: the release names the output, the entity and the
    // field, and the entity artifact says whether the field is indexed.
    let indexed = |output: &str| -> bool {
        source["release"]["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|k| k["output"] == output)
            .and_then(|k| {
                let e = entities.iter().find(|e| e["entity"] == k["entity"])?;
                e["fields"]
                    .as_array()?
                    .iter()
                    .find(|f| f["name"] == k["field"])
            })
            .is_some_and(|f| f["indexed"].as_bool().unwrap_or(false))
    };
    source["outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let name = o["name"].as_str()?;
            let allows = if !named.contains(&name) || o["type"] == "blob" {
                FieldRefine::Refused
            } else if o["sealed"].as_bool().unwrap_or(false) {
                if counted.is_some() || !indexed(name) {
                    FieldRefine::Refused
                } else {
                    FieldRefine::Equality
                }
            } else {
                FieldRefine::Any
            };
            Some((name.to_string(), allows))
        })
        .collect()
}

/// `{entity, key}` for an entity name the descriptor is describing.
fn entity_key(name: Option<&str>, entities: &[Value]) -> Value {
    let Some(name) = name else { return Value::Null };
    match entities.iter().find(|e| e["entity"] == name) {
        Some(e) => json!({ "entity": name, "key": e["key"] }),
        None => Value::Null,
    }
}

// ── typed-id action discovery (`ui-gen.md` §2, settled) ───────────────────

/// The typed-id index: **prefix → the ops that accept an id of that type**,
/// so a UI can offer row actions with no annotation and no registration
/// flag.
///
/// §2's rule verbatim: *a write op applies to a row iff it declares an
/// id-typed parameter whose entity prefix matches the row's id type — and
/// the caller's token can call it*. The second clause is why this is
/// computed here, **after** the manifests were filtered, rather than from
/// the whole program: an index built before scoping would tell a viewer
/// which manager-only actions exist by naming the id types that have them.
///
/// Read ops that take the same id are listed separately as `detail` —
/// §1's *detail linkage*, which is the same fact answering a different
/// question (which surface serves this row).
///
/// An entry appears for every entity a visible op's ids point at **or whose
/// rows a visible op returns**, including one no visible op *reaches*. That
/// is deliberate and it discloses nothing new: the op already returns
/// `proj_01J8…` values, so the prefix and the name it stands for are in the
/// caller's hands already — while the entity's field list, which would be a
/// real disclosure, stays out of `entities`.
///
/// # The half this index did not have: which ops LIST an entity
///
/// `actions` and `detail` both key on what an op **accepts**. Nothing keyed
/// on what an op **returns**, so the commonest question a form asks — *"an
/// `assignee` parameter addresses `members`; what can stock its picker?"* —
/// had no answer here, and the first consumer reconstructed one from
/// `result.row_identity` plus "no required params". That reconstruction was
/// honest and it was still a heuristic in the one document that exists so
/// renderers need none; worse, it could not distinguish *"nothing lists this
/// entity"* from *"my rule missed it"*, and for `members` in the market
/// fixture the right answer really is nothing.
///
/// So the index states it, by the rule `ui-gen.md` §1 already settles for row
/// identity — **an op lists `E` iff its rows carry `E`'s typed id**, which is
/// `result.row_identity`, which is `result.anchor` proved by the binder. Two
/// lists, split on whether the op is *addressed* by one of those ids:
///
/// - **`get`** — anchored on `E` and addressed by a required `E` id. The op
///   that reads one row back, which is what a picker's label resolver wants
///   and what `cas_miss` must refetch through. Not the same as `detail`:
///   `detail` is every read taking an `E` id, including one that takes a
///   category id and returns *products*, and refetching a row through that
///   would present the wrong shape as "the current value".
/// - **`list`** — anchored on `E` and not so addressed.
///
/// Both are derived from the anchor and the parameter, **never from the
/// name**: `std.E.list` and `std.E.get` are the derivations that make the
/// rule well-defined, and an authored `myProjects` or `openTasks` qualifies
/// by exactly the same test. Whether a *particular* listing can stock a blind
/// picker is then the consumer's policy over `params[].required`, which is a
/// fact it already holds — this index owes it the candidates, not the policy.
fn id_types(ops: &[Value], entities: &[Value]) -> Value {
    let mut names: std::collections::BTreeSet<String> = Default::default();
    for o in ops {
        for p in o["params"].as_array().cloned().unwrap_or_default() {
            if let Some(e) = p["id_entity"].as_str() {
                names.insert(e.to_string());
            }
        }
        for f in o["result"]["outputs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            if let Some(e) = f["id_entity"].as_str() {
                names.insert(e.to_string());
            }
        }
        // An entity whose rows an op returns but whose id nothing takes as a
        // parameter would otherwise have no entry at all, and its listings
        // would be unfindable — the index would answer "nothing lists it"
        // by being silent, which is the ambiguity this closes.
        if let Some(e) = o["result"]["row_identity"]["entity"].as_str() {
            names.insert(e.to_string());
        }
    }
    let mut out = Vec::new();
    for name in names {
        let mut actions = Vec::new();
        let mut detail = Vec::new();
        let mut list = Vec::new();
        let mut get = Vec::new();
        for o in ops {
            let kind = o["kind"].as_str().unwrap_or("read");
            let params = o["params"].as_array().cloned().unwrap_or_default();
            for p in &params {
                if p["id_entity"].as_str() != Some(name.as_str()) {
                    continue;
                }
                let entry = json!({
                    "op": o["op"],
                    "param": p["name"],
                    "kind": kind,
                    "mode": o["mode"],
                    "required": p["required"],
                });
                if kind == "read" {
                    detail.push(entry);
                } else {
                    actions.push(entry);
                }
            }
            if o["result"]["row_identity"]["entity"].as_str() != Some(name.as_str()) {
                continue;
            }
            // The addressing parameter, when there is one: a required `E` id.
            // Optional would not do — an op whose `E` id defaults is a listing
            // that *can* be narrowed, not a row fetch.
            let addressed = params.iter().find(|p| {
                p["id_entity"].as_str() == Some(name.as_str())
                    && p["required"].as_bool().unwrap_or(false)
            });
            let entry = json!({
                "op": o["op"],
                "param": addressed.map_or(Value::Null, |p| p["name"].clone()),
                "single_row": o["result"]["single_row"],
            });
            match addressed {
                Some(_) => get.push(entry),
                None => list.push(entry),
            }
        }
        out.push(json!({
            "key": entities.iter().find(|e| e["entity"] == name.as_str())
                .map_or(Value::Null, |e| e["key"].clone()),
            "entity": name,
            "actions": actions,
            "detail": detail,
            "list": list,
            "get": get,
        }));
    }
    Value::Array(out)
}

// ── refusals ─────────────────────────────────────────────────────────────

/// The refusal classes **this op's own declaration makes reachable**, from
/// the closed wire inventory (`api-engine-design.md` §4).
///
/// Read the definition precisely, because the useful list and the exhaustive
/// list are different lists and conflating them would make this dangerous:
///
/// - It is a **subset** of what the door may answer. Every caller must still
///   handle the closed inventory — `internal` and `serialization_conflict`
///   can meet anyone, and a generated SDK types the whole enum. What this
///   list is for is **affordance**: which forms need a conflict view, which
///   need a duplicate message beside a field, which need a confirm.
/// - Every entry is derived from a **stated declaration** — a required
///   parameter, a declared bound, a unique key, a `check`, a tree edge, an
///   inbound ref, a key release. Nothing is listed because it seemed likely.
///   A class the declaration cannot prove reachable is omitted, because a UI
///   that renders a "duplicate" affordance on an entity with no unique key
///   teaches its author that the affordance means nothing.
/// - It mints **no class**. Every name here is in `ErrorCode::ALL`, asserted
///   two-directionally by `ordo-host`'s `tests/interface_wire.rs` against
///   the same inventory `tests/inventory_is_closed.rs` reads — so the empty
///   allow-list rule holds for this document too.
///
/// It is derived from the manifest and the entity descriptors alone, never
/// from a lookup keyed on an op the asker may not see: refusal metadata that
/// answered about ops outside the catalog would be the leak
/// `std.space.callersOf` is careful not to be.
fn refusals(m: &Value, entities: &[Value], ops: &[Value]) -> Value {
    let mut out: std::collections::BTreeSet<&'static str> = Default::default();
    let kind = m["kind"].as_str().unwrap_or("read");
    let params = m["params"].as_array().cloned().unwrap_or_default();

    // The door, for every op that exists.
    out.insert("access_denied");
    out.insert("method_mismatch");
    out.insert("unknown_param");
    out.insert("version_skew");
    if !params.is_empty() {
        out.insert("invalid_param");
    }
    if params
        .iter()
        .any(|p| p["required"].as_bool().unwrap_or(false))
    {
        out.insert("missing_param");
    }
    // A declared ceiling or a derived floor: either is a bound the door
    // measures a value against, and `min` was the half this list missed
    // while `ordo_api::Bounds` had carried it all along.
    if params
        .iter()
        .any(|p| !p["max"].is_null() || !p["min"].is_null())
    {
        out.insert("bound_violation");
    }
    // Opening a sealed envelope is a step that can fail, and only an op with
    // a key release ever takes it.
    if !m["release"]["purpose"].is_null() {
        out.insert("sealed_open_failed");
        // The same step, failing with its key present.
        out.insert("sealed_unreadable");
    }

    if kind == "read" {
        // Every read is keyset-paged, so every read can be handed a cursor
        // that does not belong to it.
        out.insert("invalid_cursor");
        // A refinable read can be handed a refinement the door refuses: a
        // malformed one, one over the size limits, or one whose sealed
        // condition the cost gate prices past the op's registration.
        if refine(m, ops)["tier"] != "none" {
            out.insert("invalid_refinement");
            out.insert("refinement_unbounded");
            // A condition on a sealed field with no blind index is refused
            // at the door until host-side filtering is built (ruled
            // 2026-10-05). Only a sealed field the author opened to
            // refinement can carry one.
            if m["outputs"].as_array().is_some_and(|os| {
                os.iter().any(|o| {
                    o["sealed"].as_bool().unwrap_or(false)
                        && o["refinable"].as_bool().unwrap_or(false)
                })
            }) {
                out.insert("sealed_unsupported");
            }
        }
        return json!(out.into_iter().collect::<Vec<_>>());
    }

    // Writes and deletes: the envelope, the CAS and the guard.
    //
    // `cas_miss` follows the same rule as `call.cas`, and for the same
    // reason: a statement that asserts no revision cannot miss one, so
    // listing the class on `std.E.delete($id)` was an affordance — a
    // refetch-and-remerge dialog — for a conflict that op cannot produce.
    // A declared assertion is a stated declaration; "is a mutation" is not.
    if m["statements"]
        .as_array()
        .is_some_and(|ss| ss.iter().any(|st| st["cas_param"].is_string()))
    {
        out.insert("cas_miss");
    }
    out.insert("access_guard");
    out.insert("not_found");
    out.insert("idempotency_mismatch");
    out.insert("idempotency_in_flight");
    out.insert("serialization_conflict");
    out.insert("log_append_failed");

    let touched: Vec<&Value> = m["statements"]
        .as_array()
        .map(|s| {
            s.iter()
                .filter_map(|st| st["entity"].as_str())
                .filter_map(|n| entities.iter().find(|e| e["entity"] == n))
                .collect()
        })
        .unwrap_or_default();
    for e in &touched {
        if !e["checks"].as_array().is_none_or(|c| c.is_empty()) {
            out.insert("check_violation");
        }
        let has_unique = !e["unique_keys"].as_array().is_none_or(|u| u.is_empty())
            || e["fields"]
                .as_array()
                .is_some_and(|f| f.iter().any(|x| x["unique"] == true));
        if has_unique {
            out.insert("duplicate");
            // A create asserting `rev: null` over a unique key is exactly
            // the conflicting-identity case; a converge is not.
            if m["mode"].as_str().is_some_and(|l| l.contains("creates")) {
                out.insert("conflicting_identity");
            }
        }
        if e["tree"] == true {
            out.insert("tree_cycle");
            out.insert("tree_depth_exceeded");
        }
    }
    if kind == "delete" {
        for e in &touched {
            if e["tree"] == true {
                out.insert("tree_has_children");
            }
            // A row nothing may point at cannot be refused for referrers.
            let name = e["entity"].as_str().unwrap_or("");
            let referenced = entities.iter().any(|other| {
                other["fields"].as_array().is_some_and(|fs| {
                    fs.iter().any(|f| {
                        f["type"]["kind"] == "ref"
                            && f["type"]["target"] == name
                            && f["type"]["owned"] != true
                    })
                })
            });
            if referenced {
                out.insert("has_referrers");
            }
        }
    }
    json!(out.into_iter().collect::<Vec<_>>())
}
