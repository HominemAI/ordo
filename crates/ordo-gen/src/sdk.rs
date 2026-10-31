//! 14d — the generated TypeScript SDK.
//!
//! A **pure function of the interface descriptor** (`ordo.interface/2`), and
//! of nothing else. It reaches no schema, no registry and no binder, for the
//! reason `ordo_plan::interface` gives about itself: two walks over the same
//! compiler output are two answers waiting to differ, and this is the third
//! reader in the chain, not a second walk.
//!
//! # What the split is
//!
//! Everything with *behaviour* — the `fetch` wrapper, the header set, the
//! keyset page loop, the CAS rules, the refusal decoder — lives in the
//! package's hand-written `src/core/`, is versioned by hand, and is the same
//! bytes for every deployment. Everything that is a *fact about this
//! deployment's ops* — names, wire methods, versions, parameter types and
//! bounds, result shapes, refusal sets — is generated here as data and types.
//!
//! The line is drawn where it is because of what happens when it moves. Code
//! in the generated half is code nobody can fix: the fix is lost on the next
//! `ordo gen ts`, and the person who lost it will not find out until
//! production. Data in the hand-written half is data that goes stale against
//! a deployment and cannot be regenerated. So: **the generated half declares,
//! the static half decides**, and the header on every emitted file says so.
//!
//! # What is deliberately not generated
//!
//! - **`.live()`** — the SSE verb. `call.terminal` stamps it on 76 reads and
//!   this generator emits no method for it. `api-engine-design.md` §6 settles
//!   the transport, but the descriptor carries no log position on the read
//!   path, so a subscription's first frame and its invalidations cannot be
//!   ordered against each other by anything the client can see. That is the
//!   same gap `@hominemai/ordo-react` named when it deferred live, and shipping a
//!   `.live()` that reconnects and refetches would be the approximation §2's
//!   settled design specifically warns produces a torn read.
//! - **Optimistic apply and typed rollback**, which belong on top of a
//!   conflict machine and not underneath it.
//!
//! # Refinement
//!
//! Every read comes back as a lazy builder with `.where(…)` and `.pick(…)`
//! (§B2, `plan-refinement.md` §5). Every program read accepts a refinement
//! by default (ruled 2026-10-05). The builder's behaviour is in the static
//! core (`core/call.ts`, `core/refine.ts`); this file emits the facts it
//! needs:
//!
//! - **`<Op>Fields`**, one proxy per output in `refine.fields`, typed by the
//!   row field's own type. An open field offers all 8 operators; a sealed
//!   one offers `eq` and `in`. A field missing from the proxy is a compile
//!   error at the call site.
//! - **`refine.types`, `refine.sealed`, `refine.limits`** in the op table, so
//!   the shared validator checks value types, the sealed rule and the size
//!   limits before the wire.
//! - **The refinement refusals** in each read's refusal set:
//!   `invalid_refinement` and `refinement_unbounded` on every read, and
//!   `sealed_unsupported` on a read with a sealed output. The descriptor
//!   lists them once the server derives the tier; adding them here keeps a
//!   descriptor that predates that from typing them away.
//!
//! The tier is not consulted. A read whose tier is `none` (a platform op, a
//! public read) still gets the proxy, because the device copy refines its
//! synced rows whatever the tier says, and the door's refusal is typed.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::ts::{self, GenFile};

const SOURCE: &str = "the deployment's `interface.json` (`ordo.interface/2`), \
                      itself a projection of `op_artifact`";

/// Every generated file of `clients/ordo-sdk`.
pub fn files(doc: &Value, out_dir: &str) -> Result<Vec<GenFile>, String> {
    let version = doc["schema_version"]
        .as_str()
        .ok_or("interface.json has no schema_version")?;
    if version != ordo_schema_version() {
        return Err(format!(
            "this generator speaks {} and the descriptor is {version}. A \
             descriptor version moves on a CHANGED MEANING, never on an \
             addition, so generating against it would emit code that reads a \
             key the way it used to mean.",
            ordo_schema_version()
        ));
    }
    let ops = doc["ops"].as_array().ok_or("interface.json has no ops")?;
    let entities = doc["entities"]
        .as_array()
        .ok_or("interface.json has no entities")?;

    Ok(vec![
        GenFile {
            path: format!("{out_dir}/ids.ts"),
            contents: ids_file(entities, doc),
        },
        GenFile {
            path: format!("{out_dir}/specs.ts"),
            contents: specs_file(ops, doc),
        },
        GenFile {
            path: format!("{out_dir}/ops.ts"),
            contents: ops_file(ops, entities),
        },
        GenFile {
            path: format!("{out_dir}/index.ts"),
            contents: index_file(),
        },
    ])
}

/// The descriptor version this generator was written against.
///
/// Read from `ordo-plan` would be better and is impossible: `ordo-plan`
/// depends on a schema and a registry, and this crate deliberately does not.
/// So it is pinned here and `tests/ratchet.rs` compares it against
/// `ordo_plan::interface::SCHEMA_VERSION`'s spelling in the golden descriptor
/// — the same two-directional shape the rest of the repository uses when a
/// fact has to be restated across a dependency boundary it must not cross.
fn ordo_schema_version() -> &'static str {
    "ordo.interface/2"
}

// ── ids ──────────────────────────────────────────────────────────────────

/// Branded id types, one per entity the descriptor describes.
///
/// A branded `string` rather than a bare one because ids are the value most
/// worth confusing: every one of them is spelled `id` by the type system, and
/// `ui-gen.md` §2's action discovery exists precisely because *which* entity
/// an id addresses is not in the type. The brand puts it back, at compile
/// time, in the caller's own language — pass a `CategoriesId` where a
/// `VendorsId` is wanted and `tsc` says so.
///
/// The brand is a phantom property on a type alias, so it costs nothing at
/// run time and a value crossing the wire is the plain string it always was.
fn ids_file(entities: &[Value], doc: &Value) -> String {
    let mut s = ts::header("Branded id types, one per described entity.", SOURCE);
    s.push_str(
        "import type { Branded } from '../core/brand.js';\n\n\
         /** Every entity id type in this deployment, keyed by entity name. */\n",
    );

    // The typed-id key (`prod`, `boar-acce`) is the prefix a real id carries,
    // and it belongs in the doc comment because it is what a caller sees in a
    // URL and in a refusal message.
    let mut keys = std::collections::BTreeMap::new();
    if let Some(list) = doc["id_types"].as_array() {
        for t in list {
            if let (Some(e), Some(k)) = (t["entity"].as_str(), t["key"].as_str()) {
                keys.insert(e.to_string(), k.to_string());
            }
        }
    }

    for e in entities {
        let Some(name) = e["entity"].as_str() else {
            continue;
        };
        let ty = format!("{}Id", ts::pascal(name));
        let mut lines = vec![format!("An id addressing `{name}`.")];
        if let Some(k) = keys.get(name) {
            lines.push(String::new());
            lines.push(format!(
                "Typed-id prefix `{k}`, so a value looks like `{k}_…`. The brand \
                 is compile-time only; the wire sees the string."
            ));
        }
        s.push_str(&ts::doc(&lines));
        s.push_str(&format!(
            "export type {ty} = Branded<string, {}>;\n\n",
            ts::str_lit(name)
        ));
    }
    s
}

/// The TS type a value of this descriptor type takes, given the entity an id
/// addresses and any enumerated value set.
fn value_type(ty: &str, id_entity: Option<&str>, enum_values: Option<&Value>) -> String {
    // An enumerated set is sharper than the scalar type and is a *fact from
    // the producer* — `params[].enum_values`, resolved through `field_ref`
    // against the entity half of the same document — so it wins where it
    // exists. Without it a `text` param that fills a `status` field is
    // `string`, which is what the caller had before the descriptor moved to
    // `/2`.
    if let Some(Value::Array(vs)) = enum_values {
        if !vs.is_empty() {
            let set: BTreeSet<String> = vs
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if !set.is_empty() {
                return ts::string_union(&set);
            }
        }
    }
    match ty {
        "id" => match id_entity {
            Some(e) => format!("{}Id", ts::pascal(e)),
            // A bare `id` whose entity the body does not pin. The descriptor
            // says so by leaving `id_entity` null, and inventing a brand here
            // would be inventing the fact it declined to state.
            None => "string".to_string(),
        },
        "int" => "number".to_string(),
        "bool" => "boolean".to_string(),
        // A document is JSON on the wire, in both directions (28b), so the
        // SDK's own JSON type: a caller cannot hand it a `Date`, a function
        // or `undefined`, which `JSON.stringify` would silently change.
        "json" => "JsonValue".to_string(),
        // `text`, `money`, `date`, `datetime`, `duration` and `decimal` are
        // all strings on the wire (§5), and money's stringness is the
        // load-bearing one: an exact decimal that became a `number` would be
        // rounded before it was ever sent.
        _ => "string".to_string(),
    }
}

// ── specs ────────────────────────────────────────────────────────────────

/// The op table: every fact the static core needs to make a call, as data.
///
/// This is the file that makes the split work. The core has one `request`
/// function and no knowledge of any op; everything it branches on — method,
/// terminal verbs, the CAS parameter, the bound to check before the round
/// trip — arrives from here. So the core can be reviewed once, and a deploy
/// regenerates data rather than code.
fn specs_file(ops: &[Value], doc: &Value) -> String {
    let mut s = ts::header(
        "The op table: every fact a call needs, as data for the static core.",
        SOURCE,
    );
    s.push_str("import type { OpSpec } from '../core/spec.js';\n\n");
    s.push_str(&ts::doc(&[
        "The descriptor this file was generated from.".to_string(),
        String::new(),
        "`digest` identifies the whole document and `domain` names the program. \
         A client sends neither: the version pin is per-op (`api-engine-design.md` \
         §10), because a registry-wide pin marks every call stale after any \
         unrelated deploy and a signal that fires constantly is one clients learn \
         to ignore. These are here so a bug report can say which descriptor."
            .to_string(),
    ]));
    s.push_str(&format!(
        "export const DESCRIPTOR = {{\n  schemaVersion: {},\n  domain: {},\n  digest: {},\n}} as const;\n\n",
        ts::str_lit(doc["schema_version"].as_str().unwrap_or("")),
        ts::str_lit(doc["domain"].as_str().unwrap_or("")),
        ts::str_lit(doc["digest"].as_str().unwrap_or("")),
    ));

    let consts = spec_consts(ops);
    let mut table: Vec<(String, String)> = Vec::new();
    for op in ops {
        let Some(name) = op["op"].as_str() else {
            continue;
        };
        let cname = consts[name].clone();
        table.push((name.to_string(), cname.clone()));
        s.push_str(&format!("export const {cname} = {{\n"));
        s.push_str(&format!("    op: {},\n", ts::str_lit(name)));
        s.push_str(&format!(
            "    version: {},\n",
            ts::str_lit(op["version"].as_str().unwrap_or(""))
        ));
        s.push_str(&format!(
            "    method: {},\n",
            ts::str_lit(op["call"]["method"].as_str().unwrap_or("GET"))
        ));
        s.push_str(&format!(
            "    kind: {},\n",
            ts::str_lit(op["kind"].as_str().unwrap_or("read"))
        ));
        let terminal: Vec<String> = op["call"]["terminal"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(ts::str_lit)
                    .collect()
            })
            .unwrap_or_default();
        s.push_str(&format!("    terminal: [{}],\n", terminal.join(", ")));
        s.push_str(&format!(
            "    paged: {},\n",
            op["call"]["paged"].as_bool().unwrap_or(false)
        ));
        s.push_str(&format!(
            "    singleRow: {},\n",
            op["result"]["single_row"].as_bool().unwrap_or(false)
        ));
        // Whether a device can keep the op in sync, and what its rows are
        // keyed by (`plan-replica-feed.md` §1): the entity from
        // `result.row_identity`, the key output from `result.sync`. Anything
        // short of both is `null`, which `syncOp` refuses before the wire.
        let sync = match (
            op["result"]["sync"]["syncable"].as_bool(),
            op["result"]["sync"]["id"].as_str(),
            op["result"]["row_identity"]["entity"].as_str(),
        ) {
            (Some(true), Some(key), Some(entity)) => format!(
                "{{ entity: {}, key: {} }}",
                ts::str_lit(entity),
                ts::str_lit(key)
            ),
            _ => "null".into(),
        };
        s.push_str(&format!("    sync: {sync},\n"));
        // The order the rows arrive in (27o), from `result.order`: the
        // device copy sorts what it holds by these keys, output names with
        // the id last, when the descriptor states them. `null` for a write
        // or a platform op, which the compiler ordered nothing for.
        let order = match op["result"]["order"]["keys"].as_array() {
            Some(keys) => {
                let keys: Vec<String> = keys
                    .iter()
                    .map(|k| {
                        format!(
                            "{{ path: {}, desc: {} }}",
                            ts::str_lit(k["path"].as_str().unwrap_or("id")),
                            k["desc"].as_bool().unwrap_or(false)
                        )
                    })
                    .collect();
                format!(
                    "{{ keys: [{}], stated: {} }}",
                    keys.join(", "),
                    op["result"]["order"]["stated"].as_bool().unwrap_or(false)
                )
            }
            None => "null".into(),
        };
        s.push_str(&format!("    order: {order},\n"));
        // The Tier-0 refinement manifest (`api-engine-design.md` §7), carried
        // whole: `tier` is the door's permission, `fields` the output fields a
        // refinement may name. They are separate facts and the descriptor
        // states both, so the client gets both rather than a flattened one.
        // A device refines synced rows against `fields` without asking the
        // door anything (`plan-client-cache.md` C6), which is why the field
        // list is worth emitting while every tier is still `none`.
        let tier = match op["refine"]["tier"].as_str() {
            Some("basic") => "'basic'",
            _ => "'none'",
        };
        let fields: Vec<String> = op["refine"]["fields"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(ts::str_lit)
                    .collect()
            })
            .unwrap_or_default();
        // The outputs whose values are exact decimals on the wire: `money`
        // and `decimal` (27ak). They arrive as strings, so the device must
        // compare them as numbers, never by code point (`refine.ts`).
        let decimals: Vec<String> = op["result"]["outputs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|o| matches!(o["type"].as_str(), Some("money" | "decimal")))
                    .filter_map(|o| o["name"].as_str())
                    .map(ts::str_lit)
                    .collect()
            })
            .unwrap_or_default();
        // Every output's type, so the shared validator checks a condition's
        // value before the wire, and the sealed outputs, which the door
        // refines by equality only.
        let outputs = op["result"]["outputs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let types: Vec<String> = outputs
            .iter()
            .filter_map(|o| {
                Some(format!(
                    "{}: {}",
                    ts::key(o["name"].as_str()?),
                    ts::str_lit(o["type"].as_str().unwrap_or("text"))
                ))
            })
            .collect();
        let sealed: Vec<String> = outputs
            .iter()
            .filter(|o| o["sealed"].as_bool().unwrap_or(false))
            .filter_map(|o| o["name"].as_str())
            .map(ts::str_lit)
            .collect();
        // The size limits are deployment settings (ruled 2026-10-05, ask 6).
        // When the descriptor states them the client checks them early;
        // otherwise the door alone does.
        let limits = match (
            op["refine"]["limits"]["conditions"].as_u64(),
            op["refine"]["limits"]["in_values"].as_u64(),
        ) {
            (Some(c), Some(v)) => format!("{{ conditions: {c}, inValues: {v} }}"),
            _ => "null".to_string(),
        };
        let types = if types.is_empty() {
            "{}".to_string()
        } else {
            format!("{{ {} }}", types.join(", "))
        };
        s.push_str(&format!(
            "    refine: {{ tier: {tier}, fields: [{}], decimals: [{}], types: {types}, sealed: [{}], limits: {limits} }},\n",
            fields.join(", "),
            decimals.join(", "),
            sealed.join(", "),
        ));
        // The CAS parameter, from the statement that asserts a revision.
        // `call.cas` says a revision is asserted; the parameter's NAME is
        // only in `statements[].cas_param`, and the core needs the name to
        // refuse a commit that omits it.
        let cas = op["statements"]
            .as_array()
            .and_then(|sts| sts.iter().find_map(|st| st["cas_param"].as_str()));
        s.push_str(&format!(
            "    casParam: {},\n",
            cas.map(ts::str_lit).unwrap_or_else(|| "null".into())
        ));
        s.push_str("    params: [\n");
        if let Some(ps) = op["params"].as_array() {
            for p in ps {
                s.push_str(&param_spec(p));
            }
        }
        s.push_str("    ],\n");
        let refusals: Vec<String> = refusal_set(op).iter().map(|r| ts::str_lit(r)).collect();
        s.push_str(&format!("    refusals: [{}],\n", refusals.join(", ")));
        // A standing compile error the descriptor already knows about. The
        // core raises it before the round trip, wearing the descriptor's own
        // class rather than inventing one.
        match op["error"].as_object() {
            Some(e) => s.push_str(&format!(
                "    standingError: {{ code: {}, message: {} }},\n",
                ts::str_lit(e.get("code").and_then(Value::as_str).unwrap_or("internal")),
                ts::str_lit(e.get("message").and_then(Value::as_str).unwrap_or("")),
            )),
            None => s.push_str("    standingError: null,\n"),
        }
        // A public op (`design-public-path.md` §1): callable with no token,
        // in the deployment's space. Written only when set, so a program with
        // no public op generates the bytes it did.
        if op["public"].as_bool().unwrap_or(false) {
            s.push_str("    public: true,\n");
        }
        s.push_str("} as const satisfies OpSpec;\n\n");
    }
    s.push_str("export const SPECS = {\n");
    for (name, cname) in &table {
        s.push_str(&format!("  {}: {cname},\n", ts::key(name)));
    }
    s.push_str("} as const satisfies Record<string, OpSpec>;\n\n");
    s.push_str(
        "export type OpName = keyof typeof SPECS;\n\n\
         let list: readonly OpSpec[] | undefined;\n\n\
         /** The op table as the core wants it: one lookup, no name heuristics. Built on first use. */\n\
         export function specList(): readonly OpSpec[] {\n  \
         return (list ??= Object.values(SPECS));\n}\n",
    );
    s
}

/// One `const` name per op: `spec_` plus the op's binding, with a numeric
/// suffix when two ops would otherwise share one. Both the spec table and
/// the op functions call this, so they agree.
fn spec_consts(ops: &[Value]) -> std::collections::HashMap<String, String> {
    let mut used = std::collections::HashSet::new();
    let mut out = std::collections::HashMap::new();
    for op in ops {
        let Some(name) = op["op"].as_str() else {
            continue;
        };
        if out.contains_key(name) {
            continue;
        }
        let base = format!("spec_{}", ts::binding(name));
        let mut c = base.clone();
        let mut n = 2;
        while !used.insert(c.clone()) {
            c = format!("{base}_{n}");
            n += 1;
        }
        out.insert(name.to_string(), c);
    }
    out
}

/// The refusals an op can answer with: the descriptor's own list, in its
/// order, then the refinement refusals a read can earn and the descriptor
/// does not yet list.
///
/// A read takes a refinement, so it can answer `invalid_refinement` (a bad
/// envelope) and `refinement_unbounded` (the cost gate). A read with a sealed
/// output can also answer `sealed_unsupported`: the door refuses a sealed
/// condition the executor cannot run (ruled 2026-10-05, ask 3). The op table
/// and the generated union read this one list, so `Refusal.declared` and the
/// type agree.
fn refusal_set(op: &Value) -> Vec<String> {
    let mut out: Vec<String> = op["refusals"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if op["kind"].as_str() == Some("read") {
        let sealed = op["result"]["outputs"]
            .as_array()
            .is_some_and(|os| os.iter().any(|o| o["sealed"].as_bool().unwrap_or(false)));
        let mut add = vec!["invalid_refinement", "refinement_unbounded"];
        if sealed {
            add.push("sealed_unsupported");
        }
        for class in add {
            if !out.iter().any(|r| r == class) {
                out.push(class.to_string());
            }
        }
    }
    out
}

fn param_spec(p: &Value) -> String {
    let name = p["name"].as_str().unwrap_or("");
    let ty = p["type"].as_str().unwrap_or("text");
    let enums = match p["enum_values"].as_array() {
        Some(vs) if !vs.is_empty() => format!(
            "[{}]",
            vs.iter()
                .filter_map(|v| v.as_str())
                .map(ts::str_lit)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => "null".to_string(),
    };
    // `min`/`max` are canonical DIGIT STRINGS, always — never JSON numbers.
    // `@hominemai/ordo-react` found a dead branch that read them as numbers; carrying
    // them through as the strings the emitter writes is what stops the same
    // branch appearing here.
    let bound = |k: &str| match &p[k] {
        Value::Null => "null".to_string(),
        Value::String(v) => ts::str_lit(v),
        other => ts::str_lit(&other.to_string()),
    };
    // A `money(2)` parameter takes at most two places (27ak); the client
    // refuses a finer value before the wire, as the door would.
    let places = match p["places"].as_u64() {
        Some(n) => n.to_string(),
        None => "null".to_string(),
    };
    format!(
        "      {{ name: {}, type: {}, required: {}, idEntity: {}, enumValues: {}, min: {}, max: {}, places: {places} }},\n",
        ts::str_lit(name),
        ts::str_lit(ty),
        p["required"].as_bool().unwrap_or(true),
        p["id_entity"]
            .as_str()
            .map(ts::str_lit)
            .unwrap_or_else(|| "null".into()),
        enums,
        bound("min"),
        bound("max"),
    )
}

// ── ops ──────────────────────────────────────────────────────────────────

/// One typed function per op, plus the row, params and refusal types it
/// needs.
///
/// The **terminal-verb split** (§B3) is the point of the return type: a read
/// comes back as a `ReadCall` (`.fetch()`, and `.pages()` when the descriptor
/// says the op is paged), a write or delete as a `WriteCall` (`.commit()`,
/// and no read verbs). Misuse is a compile error in the caller's own
/// language, and every mutation in an app is `grep '.commit('`.
fn ops_file(ops: &[Value], entities: &[Value]) -> String {
    let mut s = String::new();
    let spec_names = spec_consts(ops);
    let mut exported: Vec<(String, String)> = Vec::new();

    for op in ops {
        let Some(name) = op["op"].as_str() else {
            continue;
        };
        let pas = ts::pascal(name);
        let bind = ts::binding(name);
        let kind = op["kind"].as_str().unwrap_or("read");
        let is_read = kind == "read";

        // ── the row type ──
        let outputs = op["result"]["outputs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let row_ty = format!("{pas}Row");
        // Each output's TS type, as the row declares it. The field proxy
        // reuses it, so a condition's value has the type the row's value has.
        let mut field_ts: Vec<(String, String, bool)> = Vec::new();
        let shape = op["result"]["shape"].as_str().unwrap_or("rows");
        if outputs.is_empty() {
            let why = if shape == "write_summary" {
                "A write's rows are the summary `api-engine-design.md` §3 \
                 describes; the descriptor declares no output shape for it \
                 (`outputs: []`), so this is the honest type rather than an \
                 invented one."
            } else {
                "The descriptor declares no output fields for this op."
            };
            s.push_str(&ts::doc(&[
                format!("The row shape of `{name}`."),
                String::new(),
                why.to_string(),
            ]));
            s.push_str(&format!(
                "export type {row_ty} = Readonly<Record<string, unknown>>;\n\n"
            ));
        } else {
            let mut lines = vec![format!("One row of `{name}`.")];
            if let Some(id) = op["result"]["row_identity"]["entity"].as_str() {
                lines.push(String::new());
                lines.push(format!(
                    "Row identity: `{id}`. `synthetic_rows` is {}.",
                    op["result"]["synthetic_rows"].as_bool().unwrap_or(false)
                ));
            }
            s.push_str(&ts::doc(&lines));
            s.push_str(&format!("export interface {row_ty} {{\n"));
            for o in &outputs {
                let fname = o["name"].as_str().unwrap_or("");
                let fty = o["type"].as_str().unwrap_or("text");
                let ent = o["id_entity"].as_str();
                let mut t = value_type(fty, ent, None);
                if t.ends_with("Id") && ent.is_some() {
                    t = format!("Ids.{t}");
                }
                field_ts.push((
                    fname.to_string(),
                    t.clone(),
                    o["sealed"].as_bool().unwrap_or(false),
                ));
                if o["sealed"].as_bool().unwrap_or(false) {
                    s.push_str(&format!(
                        "  /** `{fty}`, sealed — opened host-side per row. */\n"
                    ));
                } else {
                    s.push_str(&format!("  /** `{fty}` */\n"));
                }
                s.push_str(&format!("  readonly {}: {t};\n", ts::key(fname)));
            }
            s.push_str("}\n\n");
        }

        // ── the params type ──
        let params = op["params"].as_array().cloned().unwrap_or_default();
        let params_ty = format!("{pas}Params");
        if params.is_empty() {
            s.push_str(&format!(
                "/** `{name}` takes no parameters. */\nexport type {params_ty} = Record<never, never>;\n\n"
            ));
        } else {
            s.push_str(&ts::doc(&[
                format!("The parameters of `{name}`."),
                String::new(),
                "A type alias rather than an `interface`, deliberately: only an \
                 object type alias carries an implicit index signature, and the \
                 client's `request` takes the parameters as a record. An \
                 `interface` here would need a cast at every generated call \
                 site — a hole in exactly the code nobody reviews."
                    .to_string(),
            ]));
            s.push_str(&format!("export type {params_ty} = {{\n"));
            for p in &params {
                let pname = p["name"].as_str().unwrap_or("");
                let pty = p["type"].as_str().unwrap_or("text");
                let ent = p["id_entity"].as_str();
                let mut t = value_type(pty, ent, Some(&p["enum_values"]));
                if t.ends_with("Id") && ent.is_some() && p["enum_values"].as_array().is_none() {
                    t = format!("Ids.{t}");
                }
                let required = p["required"].as_bool().unwrap_or(true);
                let mut notes = vec![format!("`{pty}`")];
                if let Some(fr) = p["field_ref"].as_object() {
                    notes.push(format!(
                        "fills `{}.{}`",
                        fr.get("entity").and_then(Value::as_str).unwrap_or(""),
                        fr.get("field").and_then(Value::as_str).unwrap_or(""),
                    ));
                }
                if let Some(min) = p["min"].as_str() {
                    notes.push(format!(">= {min}"));
                }
                if let Some(max) = p["max"].as_str() {
                    // A text bound is a character count (`text(64)`), not
                    // an ordering, so it is said as one.
                    if pty == "text" {
                        notes.push(format!("at most {max} characters"));
                    } else {
                        notes.push(format!("<= {max}"));
                    }
                }
                // DL §2 Parameters, `filled`: still required on the wire,
                // optional here, because this client produces the value
                // from the caller's clock when the caller leaves it out.
                let fill = filler(p);
                if fill.is_some() {
                    notes.push("filled from the caller's clock when omitted".to_string());
                }
                // An optional param that fills a field that may be empty
                // takes `null`, which the door reads as "use the default"
                // (`ordo_api::bind::bind_params`). On a converge the field
                // is always written, so leaving it out would quietly write
                // the default: the key is required, and the caller says
                // the current value (keep) or `null` (clear) every time.
                let empties = !required && fill.is_none() && fills_optional(p, entities);
                let kept = empties && converges(op);
                if kept {
                    notes.push(
                        "always written: pass the current value to keep it, or null to write \
                         the default (null clears it when the default is null)"
                            .to_string(),
                    );
                } else if empties {
                    notes.push("has a default; omit it or pass null to take it".to_string());
                } else if !required {
                    notes.push("has a default; omit to take it".to_string());
                }
                s.push_str(&format!("  /** {} */\n", notes.join(" · ")));
                s.push_str(&format!(
                    "  readonly {}{}: {t}{};\n",
                    ts::key(pname),
                    if kept || (required && fill.is_none()) {
                        ""
                    } else {
                        "?"
                    },
                    if empties { " | null" } else { "" }
                ));
            }
            s.push_str("};\n\n");
        }

        // ── the field proxy a read's `.where` hands its callback ──
        let fields_ty = format!("{pas}Fields");
        if is_read {
            let refinable: BTreeSet<&str> = op["refine"]["fields"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            s.push_str(&ts::doc(&[
                format!("The fields a `.where` on `{name}` may test."),
                String::new(),
                "One per output in `refine.fields`, typed as the row types it. \
                 An open field takes all 8 operators; a sealed one takes `eq` \
                 and `in`, because a sealed condition rides the blind index. \
                 An output that is not here is not refinable, and naming it \
                 does not compile."
                    .to_string(),
            ]));
            let members: Vec<&(String, String, bool)> = field_ts
                .iter()
                .filter(|(f, _, _)| refinable.contains(f.as_str()))
                .collect();
            if members.is_empty() {
                s.push_str(&format!(
                    "export type {fields_ty} = Record<never, never>;\n\n"
                ));
            } else {
                s.push_str(&format!("export interface {fields_ty} {{\n"));
                for (f, t, sealed) in members {
                    let kind = if *sealed { "SealedField" } else { "OpenField" };
                    s.push_str(&format!(
                        "  readonly {}: {kind}<{}, {t}>;\n",
                        ts::key(f),
                        ts::str_lit(f)
                    ));
                }
                s.push_str("}\n\n");
            }
        }

        // ── the refusal union ──
        let refusal_ty = format!("{pas}Refusal");
        let set: BTreeSet<String> = refusal_set(op).into_iter().collect();
        s.push_str(&ts::doc(&[
            format!("The refusals `{name}` declares."),
            String::new(),
            "The op's own set, not the whole table. The door has refusals that \
             precede any op (`missing_space`, `caller_supplied`, \
             `delegation_required`), so a caught refusal outside this union is \
             not a bug — it is the door speaking before the op did, and \
             `Refusal.declared` says which happened."
                .to_string(),
        ]));
        s.push_str(&format!(
            "export type {refusal_ty} = Extract<RefusalClass, {}>;\n\n",
            ts::string_union(&set)
        ));

        // ── the function ──
        let takes_params = !params.is_empty();
        let sig_params = if takes_params {
            format!(", params: {params_ty}")
        } else {
            String::new()
        };
        // A `filled` parameter the caller left out is filled here, before
        // the call is built, so the wire always carries it.
        let fills: Vec<String> = params
            .iter()
            .filter_map(|p| {
                let f = filler(p)?;
                let k = ts::key(p["name"].as_str().unwrap_or(""));
                Some(format!("{k}: params.{k} ?? {f}()"))
            })
            .collect();
        let arg = if !takes_params {
            "{}".to_string()
        } else if fills.is_empty() {
            "params".to_string()
        } else {
            format!("{{ ...params, {} }}", fills.join(", "))
        };
        let call_ty = if is_read {
            if op["result"]["single_row"].as_bool().unwrap_or(false)
                || !op["call"]["paged"].as_bool().unwrap_or(false)
            {
                "SingleReadCall"
            } else {
                "ReadCall"
            }
        } else {
            "WriteCall"
        };
        let mut doc_lines = vec![format!("`{name}` — {kind}.")];
        if let Some(mode) = op["mode"].as_str() {
            doc_lines.push(String::new());
            doc_lines.push(format!("Mode: **{mode}**."));
        }
        if let Some(class) = op["cost"]["class"].as_str() {
            doc_lines.push(format!(
                "Cost class `{class}`, rows: {}.",
                op["cost"]["rows"].as_str().unwrap_or("—")
            ));
        }
        let terminal: Vec<&str> = op["call"]["terminal"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        doc_lines.push(format!(
            "Terminates in {}.",
            terminal
                .iter()
                .map(|v| format!("`.{v}()`"))
                .collect::<Vec<_>>()
                .join(" / ")
        ));
        if terminal.contains(&"live") {
            doc_lines.push(
                "`.live()` is **not generated**: the descriptor carries no log \
                 position on the read path, so a subscription cannot be ordered \
                 against its own first frame. See the package README."
                    .to_string(),
            );
        }
        if op["disclosures"]["unguarded"].as_bool().unwrap_or(false) {
            doc_lines.push("**`unguarded`** — this op bypasses row guards.".to_string());
        }
        if let Some(e) = op["error"]["message"].as_str() {
            doc_lines.push(format!(
                "**Refused at registration**: {e} — calling it refuses before the wire."
            ));
        }
        if is_read {
            doc_lines.push(
                "Narrow it with `.where(v => …)` and `.pick(…)` before the verb.".to_string(),
            );
        }
        s.push_str(&ts::doc(&doc_lines));
        let generics = if is_read {
            format!("{row_ty}, {refusal_ty}, {fields_ty}")
        } else {
            format!("{row_ty}, {refusal_ty}")
        };
        s.push_str(&format!(
            "export function {bind}(client: OrdoClient{sig_params}): \
             {call_ty}<{generics}> {{\n  \
             return new {call_ty}(client, {spec}, {arg});\n}}\n\n",
            spec = spec_names[name],
        ));
        exported.push((name.to_string(), bind));
    }

    s.push_str(&ts::doc(&[
        "Every op, keyed by its real (possibly dotted) name.".to_string(),
        String::new(),
        "For dispatch that is dynamic on the caller's side. Prefer the named \
         exports where the op is known at author time — they tree-shake and \
         this object does not."
            .to_string(),
    ]));
    s.push_str("export const ops = {\n");
    for (name, bind) in &exported {
        s.push_str(&format!("  {}: {bind},\n", ts::key(name)));
    }
    s.push_str("} as const;\n");

    // The imports are assembled last, from what the body actually used. The
    // package compiles under `noUnusedLocals`, and a descriptor whose ops
    // return no ids and use no paged reads would otherwise emit a file that
    // fails its own typecheck — a defect nobody would find until the first
    // deployment shaped that way.
    let mut imports = String::from(
        "import type { OrdoClient } from '../core/client.js';\n\
         import type { RefusalClass } from '../core/refusal.js';\n",
    );
    // One named import per op's spec, so a bundler keeps only the specs of
    // the ops an app calls.
    if !exported.is_empty() {
        imports.push_str("import {\n");
        for (name, _) in &exported {
            imports.push_str(&format!("  {},\n", spec_names[name]));
        }
        imports.push_str("} from './specs.js';\n");
    }
    let used: Vec<&str> = ["ReadCall", "SingleReadCall", "WriteCall"]
        .into_iter()
        .filter(|c| s.contains(&format!("new {c}(")))
        .collect();
    if !used.is_empty() {
        imports.push_str(&format!(
            "import {{ {} }} from '../core/call.js';\n",
            used.join(", ")
        ));
    }
    let proxies: Vec<&str> = ["OpenField", "SealedField"]
        .into_iter()
        .filter(|c| s.contains(&format!("{c}<")))
        .collect();
    if !proxies.is_empty() {
        imports.push_str(&format!(
            "import type {{ {} }} from '../core/refine.js';\n",
            proxies.join(", ")
        ));
    }
    if s.contains("JsonValue") {
        imports.push_str("import type { JsonValue } from '../core/json.js';\n");
    }
    if s.contains("Ids.") {
        // A namespace import, so an entity called `Row` or `Params` cannot
        // collide with a generated type name in this file.
        imports.push_str("import type * as Ids from './ids.js';\n");
    }
    // The clock readers a `filled` parameter uses, emitted only when one
    // does, for `noUnusedLocals`' reason above.
    if s.contains("filledDate()") {
        s.push_str(
            "\n/** Today on the caller's clock, `YYYY-MM-DD`: the value of an omitted `filled` date. */\n\
             function filledDate(): string {\n  \
             const d = new Date();\n  \
             const p = (n: number): string => String(n).padStart(2, '0');\n  \
             return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;\n}\n",
        );
    }
    if s.contains("filledDatetime()") {
        s.push_str(
            "\n/** Now on the caller's clock, as an instant: the value of an omitted `filled` datetime. */\n\
             function filledDatetime(): string {\n  return new Date().toISOString();\n}\n",
        );
    }

    format!(
        "{}{imports}\n{s}",
        ts::header(
            "One typed function per op: params in, terminal-verb builder out.",
            SOURCE,
        )
    )
}

/// The generated function that fills a `filled` parameter, by type.
///
/// DL §2 says `filled` exists for "execution-relative values (`today`, the
/// caller's clock)", so the client knows how to produce a `date` (today, in
/// the caller's own calendar) and a `datetime` (now). Registration refuses
/// `filled` on any other type (`filled_type`, ruled 2026-10-05), so the
/// fallback arm only keeps a hand-built artifact from inventing a value.
/// Whether the param fills an entity field that may be empty (`f: t?`, a
/// `->` ref with `?`).
fn fills_optional(p: &Value, entities: &[Value]) -> bool {
    let Some(fr) = p["field_ref"].as_object() else {
        return false;
    };
    entities
        .iter()
        .find(|e| Some(&e["entity"]) == fr.get("entity"))
        .and_then(|e| e["fields"].as_array())
        .and_then(|fs| fs.iter().find(|f| Some(&f["name"]) == fr.get("field")))
        .is_some_and(|f| f["optional"].as_bool().unwrap_or(false))
}

/// Whether a write converges an existing row (`converges`,
/// `creates-or-converges`), where every field in its braces is written.
fn converges(op: &Value) -> bool {
    op["kind"].as_str() != Some("read")
        && op["mode"].as_str().is_some_and(|m| m.contains("converge"))
}

fn filler(p: &Value) -> Option<&'static str> {
    if p["filled"].as_bool() != Some(true) {
        return None;
    }
    match p["type"].as_str() {
        Some("date") => Some("filledDate"),
        Some("datetime") => Some("filledDatetime"),
        _ => None,
    }
}

fn index_file() -> String {
    let mut s = ts::header("Re-exports of the generated surface.", SOURCE);
    s.push_str(
        "export * from './ids.js';\n\
         export * from './ops.js';\n\
         export { DESCRIPTOR, SPECS, specList } from './specs.js';\n\
         export type { OpName } from './specs.js';\n",
    );
    s
}
