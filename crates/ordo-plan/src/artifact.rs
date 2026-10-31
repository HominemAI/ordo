//! P7 — the **plan artifact**: everything a deploy needs to know about a
//! program, as machine-readable JSON.
//!
//! # Why this lives in `ordo-plan` and not in `ordo-host`
//!
//! `ordo_host::manifest` already assembles most of this, and the obvious
//! shortcut is to have the CLI call it. That inverts the layering. Every
//! field of the artifact is **compiler output** — the schema graph, the
//! bound query, the planned SQL, the cost class, the row bound, the guard
//! set with its provenance, the registration flags — and every one of
//! those is produced at or below this crate. `ordo-host` is the *wiring*
//! crate: it sits above `ordo-api`, `ordo-exec` and `ordo-conformance`, so
//! depending on it would pull `tokio-postgres`, `tokio` and an HTTP stack
//! into a CLI whose entire job is to read text and print JSON, and would
//! put the definition of the deploy contract above the compiler that
//! produces it. A `Fixture` (a Postgres-seeded harness type) would become
//! a prerequisite for describing a program that has never met a database.
//!
//! So the artifact producer is here, over `(&Schema, &Registry)` — the two
//! things a compiled program *is*. The CLI calls it, and **so does
//! `ordo_host::manifest`, which is now a projection of [`op_artifact`]**
//! rather than a second assembly of the same facts (9l closed the
//! reduction 9m left owed). The catalog withholds the planned SQL, renders
//! the row bound as prose and flattens `flags`; everything else it
//! publishes is this document's, selected. There is one producer of what an
//! op touches, and a deploy diff and a consent page read the same answer.
//!
//! # Byte-stability
//!
//! The artifact is a contract a deploy diff runs on, so the same program
//! must serialise to the same bytes on every run and every machine:
//!
//! - **Sorted keys.** `serde_json::Map` is a `BTreeMap` unless the
//!   `preserve_order` feature is on; nothing in this workspace turns it
//!   on, so every object serialises in key order. [`to_bytes`] is the one
//!   blessed serialisation.
//! - **No hash-map iteration.** `Registry::names()` walks a `HashMap`, so
//!   every list built from it is sorted before use. Entity and field lists
//!   come from `Vec`s in declaration order, which is a property of the
//!   source text.
//! - **No timestamps, no paths, no environment.** Nothing here reads the
//!   clock, the filesystem, or a locale. Numbers are integers or `null`;
//!   no float is ever emitted, so no formatting question arises.
//!
//! [`crate::artifact`]'s own test proves it: generate twice, compare
//! bytes.
//!
//! # Shape
//!
//! ```text
//! { "domain": …, "entities": [ … ], "ops": [ … ] }
//! ```
//!
//! **Every op object carries the same key set.** A key that does not apply
//! to a kind is `null` — a read op has no write `mode`, a write op has no
//! read `plan` or `cost`. A fixed key set is what makes a diff readable:
//! an appearing key and a changed value look the same in a text diff, and
//! only one of them is usually interesting.
//!
//! Two of those keys are the point of 9l. **`scope`** ([`op_scope`]) states
//! the manifest at FIELD grain — `tasks.{status, title, project}`, not
//! `tasks` — because an entity list is a consent page that cannot say which
//! fields, and a caller consenting to an op consents to far more than the
//! op touches. **`release`** ([`op_release`]) is the same idea for keys:
//! which sealed fields the run will need opened, sealed or digested, as a
//! planner output rather than something the executor improvises per row.
//! `entities` is now a projection of `scope`, so the two cannot disagree.
//!
//! # What a write's response really is
//!
//! Five keys grew here so the **interface descriptor** could answer a
//! renderer without guessing, and one of them contradicts a settled doc:
//!
//! - **`params[].field_ref`** — `{entity, field}`, the declared field a
//!   parameter fills. `enum_values` lives on the field, so the parameter that
//!   writes it needed a link back or every form generator had to join on a
//!   name collision.
//! - **`params[].min`** — the derived floor on a `take` count
//!   ([`take_count_param`]), which the door has enforced since 9-something
//!   and no document stated.
//! - **`statements[].cas_param`** — which parameter a statement asserts as
//!   the row's revision. CAS is *that*, not "is a mutation".
//! - **`statements[].summary`** and **`result.shape`** — what the response
//!   body actually holds. `api-engine-design.md` §3 says an addressed write
//!   returns the written row post-`rev` as `rows[0]`. **It does not.** The
//!   executor answers every write with one summary object per statement
//!   (`{entity, created|converged|updated|deleted}`) and the row is never
//!   returned, so a caller does not learn the new `rev` from a commit.
//!   [`stmt_summary`] states the divergence rather than papering it over,
//!   because a descriptor that promised the spec's row would be a document
//!   that lies in the flattering direction.
//!
//! A third key, **`row_scope`** ([`op_row_scope`]), is 8s's. `scope` answers
//! *which columns* and was never able to answer *which rows* — and rows are
//! what a consent was about. `definition-language.md` §8 states the whole
//! argument in one edit: delete `member == $caller` from an op body, or add
//! `unguarded` to its header, and the field set is **byte-identical**. A
//! manifest that carries only `scope` reads that as narrowing and applies it
//! silently, turning *"this app may read my expenses"* into *"this app may
//! read everyone's expenses"* with no prompt and no audit flag. `row_scope`
//! is the half that makes the pin honest.

use std::collections::{BTreeMap, BTreeSet};

use ordo_schema::{FieldTy, GuardBody, GuardSource, Schema};
use ordo_sema::ir::{BCond, BExpr, From as IrFrom, Query};
use ordo_sema::{bind_op_generic, guard_filter_query, Registry};
use ordo_syntax::{Condition, Expr, OpBody, WriteStmt};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::cost::{classify_with, query_has_blind, row_bound, RowBound};
use crate::plan;
use crate::writes::{op_label, stmt_label};

/// The canonical serialisation. Pretty-printed because the artifact's
/// primary reader is a deploy diff and a one-line JSON blob diffs as one
/// changed line; the trailing newline is there so the file is a text file.
///
/// This is the ONLY way the artifact should reach a file or a wire —
/// `serde_json::to_string` elsewhere would be a second encoder with its
/// own stability story.
pub fn to_bytes(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("a Value always serialises");
    s.push('\n');
    s
}

/// The whole program as one artifact: the schema graph and every
/// registered op, in name order.
///
/// **The generated DDL is deliberately not in here.** It is a *rendering*
/// of `entities`, which this document already carries structurally, so
/// including both would state one fact twice and let the two drift — and
/// the rendering is the half most likely to churn for reasons that are not
/// contract changes (a `space` column, a pinned `COLLATE`). It is also the
/// worst possible thing to put in a diff-shaped document: one enormous
/// escaped string, where every change is the same one changed line.
/// Anything that needs the SQL calls [`ordo_schema::ddl_for`] on the same
/// schema, and the byte-stability of THAT is a golden file of its own.
pub fn program_artifact(schema: &Schema, registry: &Registry) -> Result<Value, String> {
    let mut names: Vec<&String> = registry.names().collect();
    names.sort();
    let ops: Result<Vec<Value>, String> = names
        .iter()
        .map(|n| op_artifact(schema, registry, n))
        .collect();
    let mut doc = json!({
        "domain": schema.domain,
        "entities": schema.entities.iter().map(entity_artifact).collect::<Vec<_>>(),
        "ops": ops?,
    });
    // A program with no routines keeps the exact document, and so the exact
    // version, it had before routines were part of it.
    if !schema.routines.is_empty() {
        doc["routines"] = Value::Array(routine_artifacts(schema, registry));
    }
    // The same rule for secrets: a program with none keeps its version. A
    // binding is part of the program, so moving a host is a new version.
    if !schema.secrets.is_empty() {
        doc["secrets"] = Value::Array(secret_artifacts(schema));
    }
    // The same for functions: absent when there are none.
    if !schema.functions.is_empty() {
        doc["functions"] = Value::Array(function_artifacts(schema));
    }
    Ok(doc)
}

/// Every secret, in name order: its name and the hosts it is bound to,
/// sorted. Never a value: the program holds none.
fn secret_artifacts(schema: &Schema) -> Vec<Value> {
    let mut secrets: Vec<&ordo_syntax::SecretDecl> = schema.secrets.iter().collect();
    secrets.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    secrets
        .into_iter()
        .map(|s| {
            let mut hosts = s.hosts.clone();
            hosts.sort();
            json!({ "name": s.name.name, "hosts": hosts })
        })
        .collect()
}

/// Every backend function, in name order: its manifest
/// (`plan-backend-functions.md` §4.3). `code_hash` is `null` until
/// [`bind_function_code`] fills it in at apply, from the file the
/// declaration names.
fn function_artifacts(schema: &Schema) -> Vec<Value> {
    let mut fns: Vec<&ordo_syntax::FunctionDecl> = schema.functions.iter().collect();
    fns.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    fns.into_iter()
        .map(|f| {
            let params: Vec<Value> = f
                .params
                .iter()
                .map(|p| json!({ "name": p.name.name, "type": p.ty.spelled() }))
                .collect();
            json!({
                "name": f.name.name,
                "wake": "call",
                "params": params,
                "returns": f.returns.name,
                "by": f.by.name,
                "code": f.code,
                "code_hash": Value::Null,
                "egress": f.egress.iter().map(|h| h.name.clone()).collect::<Vec<_>>(),
                "secrets": f.secrets.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
                "limits": {
                    "wall_ms": f.limits.wall_ms,
                    "cpu_ms": f.limits.cpu_ms,
                    "memory_bytes": f.limits.memory_bytes,
                },
                "nopause": f.nopause,
            })
        })
        .collect()
}

/// The SHA-256 of a function's code, as the manifest records it.
pub fn code_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{}", hex(&h.finalize()))
}

/// Fills in each function's `code_hash` from `hashes` (code path to
/// [`code_hash`]). Done before [`program_version`], so a change to a
/// function's code alone is a new version. Answers the path of any function
/// whose code is missing from `hashes`.
pub fn bind_function_code(
    doc: &mut Value,
    hashes: &std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    let Some(fns) = doc.get_mut("functions").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for f in fns {
        let path = f["code"].as_str().unwrap_or("").to_string();
        match hashes.get(&path) {
            Some(h) => f["code_hash"] = Value::String(h.clone()),
            None => {
                return Err(format!(
                    "function `{}`: no code was read for `{path}`",
                    f["name"].as_str().unwrap_or("")
                ))
            }
        }
    }
    Ok(())
}

/// Every routine, in name order: what it is declared as, and what it
/// compiles to.
///
/// A routine is part of the deployed program. The server rebuilds its
/// trigger wiring from the stored source at boot, and each run is
/// attributed `routine:<name>@<version>`, so an edit that touches only a
/// routine has to be a new version. Without this the document, and so the
/// version, did not move, and `registry::store` answered "already stored"
/// while the new routine text was discarded.
///
/// Per routine:
///
/// - `trigger`, `flags` and `params`: the declaration, spelled out so a
///   reviewer's diff names what moved.
/// - `declaration`: a digest of the whole parsed declaration with its source
///   positions removed, so a change anywhere in the body moves it and a
///   change of layout or comments does not.
/// - `compiled`: the routine's body as the op it runs as, in the form an op
///   has in `ops`, when it compiles. `null` for a routine the gate refuses.
fn routine_artifacts(schema: &Schema, registry: &Registry) -> Vec<Value> {
    let mut routines: Vec<&ordo_syntax::RoutineDecl> = schema.routines.iter().collect();
    routines.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    routines
        .into_iter()
        .map(|r| routine_artifact(schema, registry, r))
        .collect()
}

fn routine_artifact(schema: &Schema, registry: &Registry, r: &ordo_syntax::RoutineDecl) -> Value {
    let op = if r.each.is_some() {
        crate::each::check_op(schema, registry, r).ok()
    } else {
        Some(ordo_schema::routine::routine_op(r))
    };
    let compiled = op.and_then(|op| {
        let mut reg = registry.clone();
        reg.extend_from(&ordo_syntax::Program {
            ops: vec![op],
            ..ordo_syntax::Program::default()
        });
        op_artifact(schema, &reg, &r.name.name).ok()
    });
    let params: Vec<Value> = r
        .params
        .iter()
        .map(|p| json!({ "name": p.name.name, "type": p.ty.spelled() }))
        .collect();
    let trigger = match &r.trigger {
        ordo_syntax::RoutineTrigger::Clock(s) => s.render(),
        ordo_syntax::RoutineTrigger::Change { pattern, modifiers } => {
            let m = modifiers.render();
            if m.is_empty() {
                format!("when {}", pattern.render())
            } else {
                format!("when {} {m}", pattern.render())
            }
        }
    };
    let mut out = json!({
        "name": r.name.name,
        "trigger": trigger,
        "flags": {
            "unguarded": r.flags.unguarded,
            "nopause": r.flags.nopause,
            "unbounded": r.flags.unbounded,
            "reproducible": r.flags.reproducible,
        },
        "params": params,
        "each": r.each.is_some(),
        "declaration": declaration_digest(r),
        "compiled": compiled,
    });
    // Present only when declared, so a program without it keeps its hash.
    if let Some(n) = &r.notify {
        out["notify"] = json!({ "level": n.level.name, "text": n.text });
    }
    out
}

/// A digest of a routine's parsed declaration, source positions removed.
///
/// The syntax tree has no serialiser of its own, and a second hand-written
/// walk over every statement and expression form would be a second place to
/// forget one. The derived `Debug` form covers whatever the tree holds, and
/// dropping the `Span` blocks removes the only part that is about layout.
fn declaration_digest(r: &ordo_syntax::RoutineDecl) -> String {
    let text = format!("{r:#?}");
    let mut kept = String::new();
    let mut skip_until: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        if let Some(depth) = skip_until {
            if indent == depth && line.trim_start().starts_with('}') {
                skip_until = None;
            }
            continue;
        }
        let t = line.trim_start();
        if t.starts_with("span: Span {") || t.starts_with("trigger_span: Span {") {
            skip_until = Some(indent);
            continue;
        }
        if t.starts_with("span: Span(") || t.starts_with("trigger_span: Span(") {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    hex(&Sha256::digest(kept.as_bytes())[..8])
}

/// The **program version hash** — what `ordo apply` keys the artifact
/// registry on, and what a rollback re-points at.
///
/// It is the digest of the byte-stable document, **and of the DDL that
/// document's schema generates**, not a fold of the per-op `version` hashes.
/// Three reasons, and all of them are about what a version is *for*:
///
/// - The document carries `entities` as well as `ops`. A schema change that
///   no op's contract mentions — a new entity nothing reads yet — is still
///   a different deployment, and a fold over op versions would call it the
///   same one.
/// - Folding would have to pin an order and a separator, which is a second
///   encoding with its own stability story. [`to_bytes`] already is the
///   canonical one, and hashing it means the version moves exactly when the
///   bytes a reviewer diffs move.
/// - **One version must mean one deployable state.** The DDL is a rendering
///   of `entities` and is deliberately not IN the document (it diffs badly
///   — see [`program_artifact`]), but a rendering that differs is a
///   different database, and identity must cover it. Hashing it here says
///   so without putting it in the diff.
///
/// # Why the DDL is hashed even though the document now carries the five
///
/// `plan-schema-evolution.md` §1.1(a) found the document silently omitting
/// `default`, `normalize`, `enum_values`, `unique_keys` and `indexes`, and
/// [`entity_artifact`] now emits all five — so the specific bug is closed by
/// the document alone. This is the general form of the same guarantee: any
/// future declaration that reaches `ddl_for` and not the document moves the
/// version anyway, so the next omission is a diff that reads oddly rather
/// than a deploy that silently does nothing. The two fixes are not
/// redundant; one makes the change *visible* and the other makes identity
/// *total*.
///
/// The separator is a NUL byte, which cannot occur in either input (the
/// document is JSON text and the DDL is SQL text), so no document-plus-DDL
/// pair can be re-cut into a different one.
///
/// 16 hex characters (8 bytes), matching the per-op version's width, because
/// the two appear beside each other in a deploy diff and different widths
/// there read as different kinds of thing.
pub fn program_version(doc: &Value, ddl: &str) -> String {
    let mut h = Sha256::new();
    h.update(to_bytes(doc).as_bytes());
    h.update([0u8]);
    h.update(ddl.as_bytes());
    hex(&h.finalize()[..8])
}

/// One op's artifact. Filtering (`ordo plan <dir> <op>`) selects from
/// [`program_artifact`]'s `ops` rather than calling this directly, so a
/// filtered artifact is a subsequence of the full one.
pub fn op_artifact(schema: &Schema, registry: &Registry, name: &str) -> Result<Value, String> {
    let op = registry
        .get(name)
        .ok_or_else(|| format!("no op named `{name}`"))?;

    // The field-scope manifest: which fields of which entities, flattened
    // through composition and through every inlined guard. `entities` is
    // now a PROJECTION of it rather than a second walk — two lists derived
    // two ways is exactly how a manifest comes to disagree with itself, and
    // the entity list was the coarser of the two answers to one question.
    let scope = op_scope(schema, registry, name);
    let entities: Vec<String> = scope.iter().map(|s| s.entity.clone()).collect();
    let release = op_release(schema, registry, name);
    // The other half of a pinnable manifest (8s / DL §8): which ROWS. A
    // field set answers which columns and answered which rows never.
    let row_scope = op_row_scope(schema, registry, name);

    // The effective guard set over exactly those entities, each with the
    // provenance that makes a consent page honest (R-G14/G6): an inlined
    // guard that does not say where it came from is half-built.
    let guards: Vec<Value> = entities
        .iter()
        .filter_map(|e| schema.entity(e))
        .filter_map(|e| {
            let g = e.guards.as_ref()?;
            Some(json!({
                "entity": e.name,
                "source": guard_source(&g.source),
                "read": g.render(true),
                "write": g.render(false),
            }))
        })
        .collect();

    // R-G8: the flag never travels alone. `bypassed_guards` is always
    // present — an op that bypasses nothing bypasses nothing, and saying
    // so with an empty list keeps the key set fixed and makes the
    // "never alone" rule structural instead of conditional.
    let bypassed: Vec<String> = if op.flags.unguarded {
        entities
            .iter()
            .filter(|e| {
                schema
                    .entity(e)
                    .and_then(|x| x.guards.as_ref())
                    .is_some_and(|g| matches!(g.read, GuardBody::Walk(_)))
            })
            .map(|e| format!("{e}.read"))
            .collect()
    } else {
        Vec::new()
    };

    // Where each parameter's value LANDS in the schema — the entity whose
    // typed id it addresses (B1 / `ui-gen.md` §2) and the declared field it
    // is written into or compared against. The type system spells every id
    // `id` and every enum-valued column `text`, so both are facts only the
    // body carries: `id == $id` on `tasks` makes `$id` a `task_`-prefixed id
    // and nothing else, and `set boardsAccess { level: $level }` makes
    // `$level` one of that field's declared values and nothing else.
    //
    // They are derived HERE, once and in ONE walk, for the same reason the
    // field-scope manifest is: action discovery, the generated SDK's branded
    // ids, a detail link and a form's dropdown are four readers of one
    // answer, and four walks would be four answers waiting to differ. They
    // come from one walk rather than two so the pair cannot disagree about
    // which site it was reading — `id_entity` is literally the ref target of
    // `field_ref`'s field.
    let sites = param_sites(schema, registry, name);

    let params: Vec<Value> = op
        .params
        .iter()
        .map(|p| {
            json!({
                "name": p.name.name,
                "type": p.ty.as_str(),
                // The currency's places on a `money(p)` and the scale on a
                // `decimal(p)`; `null` for every other type (27ak). A
                // client needs it to spell the value: `"29.99"` is what a
                // `money(2)` parameter takes on the wire, and the door
                // refuses a value with more places than this.
                "places": p.ty.places(),
                "required": p.default.is_none(),
                // `null` for every non-`id` parameter, and for an `id`
                // parameter the body never compares to an id-typed column —
                // an unaddressed id is a legitimate shape (an id passed to a
                // routine, say) and guessing an entity for it would invent a
                // row action that does not apply.
                "id_entity": match sites.id.get(&p.name.name) {
                    Some(Some(e)) => Value::from(e.clone()),
                    _ => Value::Null,
                },
                // The **declared field this parameter fills** — `{entity,
                // field}` — or `null` where the body never lands it in one,
                // or lands it in two different ones.
                //
                // It is here because the descriptor's readers kept needing a
                // fact the type alone could not give: `entities[].fields[]`
                // carries `enum_values`, and `interface.rs` says of them that
                // "a dropdown is derivable from this and nothing else" — but
                // the parameter that fills such a field is spelled `text`,
                // with no link back, so a form generator had to join through
                // `statements[].entity` and a same-named field and refuse on
                // ambiguity. That join is this fact, computed once by the one
                // producer that holds both the AST and the schema, instead of
                // re-derived by every consumer from a name collision.
                //
                // **Declared fields only.** `id`, `rev` and `depth` are the
                // platform's and are not in `entities[].fields`, so naming
                // one here would point a reader at a field the document does
                // not carry.
                "field_ref": match sites.field.get(&p.name.name) {
                    Some(Some((e, f))) => json!({ "entity": e, "field": f }),
                    _ => Value::Null,
                },
                // The declared `<=` bound, as digits, never prose. It is
                // on the artifact because it is load-bearing twice over:
                // the door refuses a value above it, and (for a `take $n`)
                // the row bound below was PROVED from it — so loosening a
                // bound must appear in a deploy diff exactly as loosening a
                // `take` would.
                //
                // For a `text(64)` parameter the bound is the length in
                // characters — the same key, because it is the same fact
                // (`api-engine-design.md` §5: "`text(n)` … > n chars ⇒
                // `bound_violation`") and the door refuses above it the
                // same way.
                "max": match (&p.max, p.text_max) {
                    (Some(ordo_syntax::Expr::IntLit { value, .. }), _) => Value::from(value.to_string()),
                    (Some(ordo_syntax::Expr::DecimalLit { digits, .. }), _) => Value::from(digits.clone()),
                    (Some(ordo_syntax::Expr::DateLit { y, m, d, .. }), _) =>
                        Value::from(format!("{y:04}-{m:02}-{d:02}")),
                    (None, Some(n)) => Value::from(n.to_string()),
                    _ => Value::Null,
                },
                // DL §2 Parameters, "`filled` — the SDK-supplied marker":
                // the generated client fills this value when the caller
                // does not. Still `required` on the wire; the door does
                // not fill, the SDK does.
                "filled": p.filled,
                // The **derived floor on a take count** — the other half of
                // the pair `ordo_api::Bounds` has always carried and this
                // document never did. There is no `>=` in the language
                // (X-077's range form is proposed, not settled), so this is
                // never a transcription of a declaration: it is derived from
                // the CONSTRUCT by [`take_count_param`], which is also the
                // producer `ordo_host::param_bounds` reads, so the number the
                // door refuses below and the number this document states are
                // one number rather than two spellings of one intent.
                "min": match take_count_param(op, &p.name.name) {
                    true => Value::from("0"),
                    false => Value::Null,
                },
            })
        })
        .collect();

    // Read ops: bind and plan, because the output shape, the cost class,
    // the row bound and the SQL are all downstream of the same binding the
    // executor runs. Anything derived a second way could disagree.
    let mut error = Value::Null;
    let (kind, bits) = match &op.body {
        OpBody::Read(_) => {
            // A registered op can still fail to bind — the market fixture
            // ships one (`giftOrders` filters on a sealed field with no
            // blind index), because a refusal nobody exercises is a
            // refusal nobody trusts. The artifact RECORDS that rather than
            // failing to exist: a whole program refusing to describe
            // itself because one op does not compile is the least useful
            // possible answer to "what changed?", and `ordo check` already
            // reports per-op bind failures this way. `error` is the gate
            // `ordo apply` reads — an artifact with any non-null `error`
            // is not deployable.
            let q = match bind_op_generic(schema, registry, name, ordo_sema::Clock::CORPUS) {
                Ok(mut q) => {
                    // The anchor's id rides every row (`ui-gen.md` §1, C4e),
                    // so the manifest must describe it and the plan must
                    // select it. Attached here, before the outputs loop and
                    // before planning, so the descriptor and the wire carry
                    // the same columns by construction rather than by two
                    // rules that agree today.
                    let _ = project_row_key(&mut q);
                    q
                }
                Err(e) => {
                    error = json!({ "code": e.code, "message": e.message });
                    return Ok(op_object(
                        name,
                        "read",
                        OpBits::default(),
                        Facts {
                            entities,
                            scope,
                            row_scope,
                            release,
                            guards,
                            params,
                            bypassed,
                            flags: op.flags,
                            error,
                        },
                    ));
                }
            };
            // Shape and cost come off the BOUND query, not off the plan:
            // they exist for every read, including the ones whose SQL is
            // deliberately withheld below.
            let origins = output_origins(&q);
            let outputs: Vec<Value> = q
                .outputs
                .iter()
                .map(|o| {
                    let sealed = q.host_open.iter().any(|(n, _, _)| *n == o.name);
                    json!({
                        "name": o.name,
                        "type": o.expr.ty().as_str(),
                        // A `money(2)` output arrives as `"29.99"`; the
                        // places say so (27ak). `null` for a non-numeric
                        // output, and the computed scale for a decimal.
                        "places": o.expr.ty().places(),
                        // Sealed is not a guess: it is exactly membership
                        // in the query's `host_open` obligation list, which
                        // is the compiler saying "this column leaves the
                        // database as ciphertext and is opened here".
                        "sealed": sealed,
                        // Whether the door will accept a `$refine` condition
                        // on this field (`api-engine-design.md` §7.1,
                        // `std-library.md` §2.1 rules 1–2). An open field is
                        // refinable; a sealed one is refinable only where the
                        // author wrote `sealed indexed refinable`.
                        //
                        // This is a DISCLOSURE and it was missing: the
                        // capability was computed in `ordo-host`'s spec
                        // builder and enforced at the door, and the document
                        // a consent page and a deploy diff read said nothing
                        // about it. `FieldSpec`'s own doc says the opt-in
                        // exists so that "the capability lands in the
                        // manifest and therefore on the consent page"; until
                        // now it landed in neither.
                        //
                        // A `json` output takes no condition at all (28b).
                        "refinable": if o.expr.ty() == ordo_syntax::ScalarTy::Json {
                            false
                        } else if sealed {
                            q.host_open
                                .iter()
                                .find(|(n, _, _)| *n == o.name)
                                .and_then(|(_, e, f)| schema.entity(e).and_then(|x| x.field(f)))
                                .is_some_and(|d| d.refinable)
                        } else {
                            true
                        },
                        // The typed-id deref (`ui-gen.md` §1, *detail
                        // linkage*): an `id`-typed column names the entity
                        // whose row it identifies, so a reader can link it
                        // without parsing the prefix out of a value it has
                        // not fetched. `null` wherever the column is not an
                        // id, or its origin is a CTE / wrapped subquery /
                        // computed expression the alias map cannot resolve
                        // to a base table.
                        "id_entity": match origins.get(&o.name) {
                            Some((entity, column)) => id_target(schema, entity, column)
                                .filter(|_| o.expr.ty() == ordo_syntax::ScalarTy::Id)
                                .map_or(Value::Null, Value::from),
                            None => Value::Null,
                        },
                    })
                })
                .collect();
            // With the schema, as the door (`registered_cost`) and the venue gate
            // (`check_venue`) classify: without it a provable bound is invisible
            // and a bounded join read `unbounded-host` here only (ruled 2026-10-05).
            let class = classify_with(&q, Some(schema));
            let cost = json!({
                "class": class.as_str(),
                // `null` = unbounded (pageable for sql-class plans). The
                // artifact carries the number, never the prose the human
                // renderings print: prose is a rendering decision and a
                // reworded string would read as a deploy change.
                "row_bound": match row_bound(&q, schema) {
                    RowBound::AtMost(n) => Value::from(n),
                    RowBound::Unbounded => Value::Null,
                },
            });
            // A **blind-index** op's statement embeds `HMAC(BIK, value)`
            // for the compared literal, and the artifact withholds it —
            // for two independent reasons, either of which is sufficient.
            //
            // Stability: the digest is derived under a per-space key, so
            // the same program would produce different bytes under
            // different custody, and a deploy diff would be noise.
            //
            // Disclosure, which is the harder one: a published digest of a
            // known plaintext is a membership oracle for that plaintext
            // against every row of the sealed column. The digest is
            // resolved at call time, after the access check and behind a
            // key release (`ordo_plan::blind::resolve`) — that is exactly
            // where it belongs and exactly not here.
            //
            // Everything else about the op is still stated. The test is
            // whether the query holds a digest, not the cost class: a
            // lookup that also returns a sealed field classes as
            // `bounded-host` (the payload is opened on the host) and still
            // compares through the digest. Reading the class alone sent that
            // op to `plan()`, which refused the whole program with
            // `unresolved_blind_index` (reported fix 25ap).
            let plan_value = if query_has_blind(&q) {
                json!({ "sql": Value::Null, "bind_order": Vec::<String>::new() })
            } else {
                // **The artifact plans with the space as a SLOT, never a
                // value.** An artifact is compiled once and executed in
                // every space the deployment serves, so a folded space
                // literal here would be a compiled-in tenant — and a
                // deploy diff that changed when the deployment gained a
                // customer. `$1` is the space in every artifact statement
                // (`ordo_plan::SPACE_SLOT`), which is also why it leads
                // `bind_order`: the executor fills it from the door's
                // binding, and there is no arrangement of caller-supplied
                // arguments that reaches it.
                let p = plan(&q, &crate::Space::Slot)
                    .map_err(|e| format!("plan error in `{name}`: {e}"))?;
                json!({
                    "sql": p.sql,
                    // The `$1..$n` bind order. Without it the SQL is not
                    // executable by anything that did not plan it.
                    "bind_order": p.params,
                })
            };
            (
                "read",
                OpBits {
                    mode: Value::Null,
                    statements: Vec::new(),
                    cost,
                    outputs,
                    plan: plan_value,
                    result: Some(json!({
                        // The base table every row of this result came from
                        // — the entity whose implicit typed id underwrites
                        // the keyset cursor, and therefore the one a reader
                        // may attach a row identity to. `null` for a result
                        // assembled from a union or a CTE, where no single
                        // table is the anchor.
                        "anchor": anchor_entity(&q.from),
                        // A grouped result's rows are SYNTHETIC (`ui-gen.md`
                        // §1): they correspond to no stored row, so they have
                        // no identity, no detail link and no row actions.
                        "grouped": q.group.is_some(),
                        // `reduce` is a keyless `group`, and a keyless group
                        // is provably one row — the structural test §5
                        // settles for "this op is a measure, not a table".
                        // Deliberately ONLY that test: §5's other two cases
                        // are marked [proposed] there, and a descriptor that
                        // implements a proposal is a descriptor that has to
                        // change when the proposal is ruled on.
                        "single_row": q.group.as_ref().is_some_and(|g| g.keys.is_empty()),
                        // Whether a device can keep this result in sync, and
                        // under which output its rows are keyed
                        // (`plan-replica-feed.md` §1). A refusal names the
                        // rule, so an author sees what to change.
                        "sync": match syncability(&q) {
                            Ok(s) => json!({ "syncable": true, "id": s.id_output }),
                            Err(r) => json!({ "syncable": false, "reason": r.as_str() }),
                        },
                        // What the response's `rows` array HOLDS. `rows` for
                        // a read: one object per result row, keyed by the
                        // output names above. See [`stmt_summary`] for the
                        // other value and why the distinction is load-bearing.
                        "shape": "rows",
                        // The order the rows arrive in, for the device copy
                        // to keep (27o). See [`read_order`].
                        "order": read_order(&q),
                    })),
                },
            )
        }
        OpBody::Write(stmts) => {
            let all_del = stmts.iter().all(|s| matches!(s, WriteStmt::Del { .. }));
            // §6 rule 1: the label is a per-STATEMENT fact and the op's
            // label is their union. Both come from `writes::stmt_label` —
            // rule 8's own classifier — because a second reading of the
            // braces here is how the label and the behaviour drift, and
            // that drift was live: every conditioned `set` printed
            // `converges`, so a bulk write over a range and an addressed
            // write over one row were the same word.
            //
            // `statements` is what the artifact and the catalog carry (the
            // list, in statement order, each naming its entity and verb);
            // `mode` is the union the wire reports.
            let statements: Vec<Value> = stmts
                .iter()
                .map(|s| {
                    let (entity, verb) = match s {
                        WriteStmt::Set { entity, .. } => (&entity.name, "set"),
                        WriteStmt::Del { entity, .. } => (&entity.name, "del"),
                    };
                    let label = stmt_label(schema, s);
                    json!({
                        "entity": entity,
                        "verb": verb,
                        "label": label,
                        // **What this statement puts in the response**, which
                        // is a fact the document did not carry and a form
                        // needs before it draws a confirmation.
                        //
                        // A write's response body is NOT the written row. The
                        // executor returns one summary object per statement,
                        // in statement order, carrying `entity` plus exactly
                        // one of `created`/`converged` (the row's id) or
                        // `updated`/`deleted` (a count). This states which,
                        // and it is derived from `stmt_label` — rule 8's own
                        // classifier, the same word the executor's dispatch
                        // follows — rather than from a second reading of the
                        // braces, for the reason that function's doc gives.
                        "summary": stmt_summary(label),
                        // **Which parameter this statement asserts as the
                        // row's revision** — the CAS input — or `null`.
                        //
                        // `ui-gen.md` §1's CAS-by-default is a claim about
                        // exactly this: `set tasks(id == $id) { …, rev: $rev }`
                        // asserts a revision and `set tasks { …, rev: null }`
                        // asserts an absence, and only the first is a compare-
                        // and-swap. The descriptor used to answer "is this a
                        // mutation?" and call it `cas`, which left a runtime
                        // to find the rev parameter by NAME — a heuristic in
                        // the one document that exists so renderers need none.
                        "cas_param": cas_param(s),
                    })
                })
                .collect();
            (
                if all_del { "delete" } else { "write" },
                OpBits {
                    mode: Value::from(op_label(schema, stmts)),
                    statements,
                    // The cost gate classifies READ plans. A write op has
                    // no read plan, and inventing a "write" cost class
                    // would put a word in the artifact that `CostClass`
                    // does not have.
                    ..OpBits::default()
                },
            )
        }
    };

    Ok(op_object(
        name,
        kind,
        bits,
        Facts {
            entities,
            scope,
            row_scope,
            release,
            guards,
            params,
            bypassed,
            flags: op.flags,
            error,
        },
    ))
}

/// **What one write statement puts in the response**, from its rule-8 label.
///
/// # This describes what the executor does, not what §3 says it does
///
/// `api-engine-design.md` §3 says *"addressed writes return the written row
/// (post-defaults, post-`rev`) as `rows[0]`; bulk writes return affected-id
/// descriptors"*, and `std-library.md` §2 and `definition-language.md` §4
/// repeat it. **The executor does not do that.** Every write and every
/// delete — addressed or ranged, create or converge — answers with one
/// SUMMARY object per statement, in statement order, carrying `entity` plus
/// exactly one of:
///
/// | statement label | key | value |
/// |---|---|---|
/// | `creates` | `created` | the minted row id |
/// | `creates-or-converges` | `created` **or** `converged` | the row id, whichever branch ran |
/// | `converges` (addressed `set`) | `updated` | `1` |
/// | `bulk` | `updated` | rows inside the caller's write view |
/// | `deletes` / `bulk deletes` / `… subtree` | `deleted` | a count |
///
/// The `UPDATE` carries no `RETURNING` and the mutation's returned rows are
/// discarded; the post-image *is* re-read, but only into the command log's
/// `after` field. So **a caller does not learn the new `rev` from a commit**
/// — it must re-read the row — and this document now says so instead of
/// leaving a renderer to discover it from an empty `outputs`.
///
/// Stating it here rather than "fixing" it in the descriptor is the whole
/// rule this module is built on: a descriptor is derived truth about what
/// the deployment does, and one that promised §3's row because §3 promises
/// it would be a document that lies in the direction of the spec. The
/// divergence is recorded as a finding for the *engine* to close.
///
/// `null` for `ambiguous-identity`, which names a statement that never
/// compiles: the op carries an `error` instead, and a summary shape for a
/// write that cannot run would be describing a response nobody can get.
fn stmt_summary(label: &str) -> Value {
    match label {
        "creates" => json!({ "keys": ["created"], "value": "id" }),
        "creates-or-converges" => json!({ "keys": ["created", "converged"], "value": "id" }),
        "converges" | "bulk" => json!({ "keys": ["updated"], "value": "count" }),
        l if l.contains("deletes") => json!({ "keys": ["deleted"], "value": "count" }),
        _ => Value::Null,
    }
}

/// The parameter one write statement asserts as the row's revision.
///
/// Read off the braces, which is where the assertion is: `split_rev` hands
/// `rev: $r` to `check_cas` and `rev: null` to the create-once test, and only
/// the first is a compare-and-swap. `null` for a `del` (a delete carries no
/// revision), for a create-once (`rev: null` asserts an absence, refused as
/// `expected_absent`, not `cas_miss`), and for any statement that names no
/// `rev` at all.
///
/// DL §6 rule 9 spells exactly two forms — `rev: $r` and `rev: null` — and
/// this reads both. A `rev: <int literal>` would evaluate (the executor's
/// `split_rev` takes any `CreateVal::Int`) and is `null` here, deliberately:
/// the question this key answers is *"which value must a runtime carry over
/// from the fetched row?"*, and a statement pinned to a constant revision
/// asks the caller for nothing. It is also a form no document spells and no
/// fixture writes — recorded as a boundary rather than guessed at.
fn cas_param(stmt: &WriteStmt) -> Value {
    let WriteStmt::Set { fields, .. } = stmt else {
        return Value::Null;
    };
    fields
        .iter()
        .find(|(n, _)| n.name == "rev")
        .and_then(|(_, v)| match v {
            Expr::Param { name, .. } => Some(name.clone()),
            _ => None,
        })
        .map_or(Value::Null, Value::from)
}

/// The parts of an op's artifact that only a read has. `default()` is the
/// all-null form a write op and an op that failed to bind both use, which
/// is what keeps the key set identical across kinds.
#[derive(Default)]
struct OpBits {
    mode: Value,
    /// §6 rule 1's per-statement labels. Empty for a read, which has no
    /// statements — an empty list, never a missing key.
    statements: Vec<Value>,
    cost: Value,
    outputs: Vec<Value>,
    plan: Value,
    /// The read's result-shape facts (anchor table, grouped, single-row).
    /// `None` is the write/failed-bind form; [`op_object`] renders it as
    /// the all-negative object rather than as a missing key, because the
    /// fixed-key-set rule is what makes a diff readable.
    result: Option<Value>,
}

/// The parts every op has whatever its kind or its compile result.
struct Facts {
    entities: Vec<String>,
    scope: Vec<EntityScope>,
    row_scope: RowScope,
    release: ReleaseSet,
    guards: Vec<Value>,
    params: Vec<Value>,
    bypassed: Vec<String>,
    flags: ordo_syntax::decl::OpFlags,
    error: Value,
}

/// Assemble one op object and stamp its version. One assembly point, so
/// the "every op carries the same key set" rule is enforced by there being
/// only one place that inserts keys.
fn op_object(name: &str, kind: &str, bits: OpBits, f: Facts) -> Value {
    let mut m = Map::new();
    m.insert("op".into(), json!(name));
    m.insert("kind".into(), json!(kind));
    m.insert("mode".into(), bits.mode);
    m.insert("statements".into(), json!(bits.statements));
    m.insert("cost".into(), bits.cost);
    m.insert("params".into(), json!(f.params));
    m.insert("outputs".into(), json!(bits.outputs));
    m.insert(
        "result".into(),
        bits.result.unwrap_or_else(|| {
            json!({
                "anchor": Value::Null,
                "grouped": false,
                "single_row": false,
                // A read that failed to bind still answers in rows; a write
                // answers in summaries, and that is the fact §3 of
                // `api-engine-design.md` describes wrongly today (see
                // [`stmt_summary`]).
                "shape": if kind == "read" { "rows" } else { "write_summary" },
            })
        }),
    );
    m.insert("entities".into(), json!(f.entities));
    m.insert("scope".into(), scope_json(&f.scope));
    m.insert("row_scope".into(), row_scope_json(&f.row_scope));
    m.insert("release".into(), release_json(&f.release));
    m.insert("guards".into(), json!(f.guards));
    let mut flags = json!({
        "unbounded": f.flags.unbounded,
        "unguarded": f.flags.unguarded,
        "reproducible": f.flags.reproducible,
    });
    // `nopause` (27ar) is written only when set. An absent key reads as
    // false, so an op that does not carry the word keeps the version hash
    // it had before the word existed; an op that gains it changes version,
    // because answering while paused is part of its contract.
    if f.flags.nopause {
        flags["nopause"] = json!(true);
    }
    // `public` (`design-public-path.md` §1) likewise: written only when set,
    // so every program from before the word keeps its versions. Gaining it
    // changes the version, because who may call the op is its contract.
    if f.flags.public {
        flags["public"] = json!(true);
    }
    // `download` (owner ruling 2026-10-22) likewise: written only when set.
    // Gaining it changes the version, because it puts the op's rows in a
    // person's own download.
    if f.flags.download {
        flags["download"] = json!(true);
    }
    m.insert("flags".into(), flags);
    m.insert("bypassed_guards".into(), json!(f.bypassed));
    m.insert("plan".into(), bits.plan);
    m.insert("error".into(), f.error);
    // The plan tier (`plan-tiers.md`, owner ruling 2026-10-23). Every op
    // compiles to the direct plan today, so every op says T0. The key is
    // here now so that adding variants later adds values, not a key: a new
    // key would move every op's version and end every grant pinned to one.
    m.insert("tier".into(), Tier::Direct.json());

    // The version hash is the digest of everything above — the op's own
    // artifact object, canonically serialised, with the `version` key
    // absent. So the version changes exactly when the CONTRACT changes,
    // which is what a registry pin and a deploy diff both want.
    //
    // Deliberately not a hash of the declaration's debug form: that
    // carries source spans, so moving an op down a file would bump every
    // downstream pin without changing anything a caller can observe.
    let mut value = Value::Object(m);
    value["version"] = json!(op_version(&value));
    value
}

/// The plan tier an op artifact was compiled at (`plan-tiers.md`, "The
/// variant kinds"). Only the direct plan exists. The other tiers are Ordo
/// v2 work; the artifact carries the tier now so they can be added later
/// without moving every op version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// T0: the obvious compilation. What every op is today.
    Direct,
}

impl Tier {
    /// The tier's short id, as the artifact and the docs spell it.
    pub fn id(self) -> &'static str {
        match self {
            Tier::Direct => "T0",
        }
    }

    /// The tier's name.
    pub fn name(self) -> &'static str {
        match self {
            Tier::Direct => "direct",
        }
    }

    /// The artifact's `tier` value: `{"id": "T0", "kind": "direct"}`.
    pub fn json(self) -> Value {
        json!({ "id": self.id(), "kind": self.name() })
    }
}

/// The version hash of one op artifact object: the first 8 bytes of the
/// SHA-256 of its canonical bytes with any `version` key removed, as hex.
/// [`op_artifact`] stamps this, and a test or a registry can recompute it.
pub fn op_version(op: &Value) -> String {
    let mut body = op.clone();
    if let Some(m) = body.as_object_mut() {
        m.remove("version");
    }
    let digest = Sha256::digest(to_bytes(&body).as_bytes());
    hex(&digest[..8])
}

fn entity_artifact(e: &ordo_schema::Entity) -> Value {
    // Declared fields only. `id`, `rev` and `depth` are the platform's,
    // are not part of `[*]`, and never change — the generated DDL beside
    // this is where their columns are stated.
    let fields: Vec<Value> = e
        .fields
        .iter()
        .filter(|f| !f.implicit)
        .map(|f| {
            let ty = match &f.ty {
                FieldTy::Scalar(s) => json!({ "kind": "scalar", "scalar": s.as_str() }),
                FieldTy::Ref {
                    target,
                    owned,
                    tree,
                } => json!({
                    "kind": "ref",
                    "target": target,
                    "owned": owned,
                    "tree": tree,
                }),
            };
            let mut field = json!({
                "name": f.name,
                "type": ty,
                "optional": f.optional,
                "unique": f.unique,
                "sealed": f.sealed,
                // The disclosure P2 requires the compiler to emit rather
                // than trust anyone to write down: a blind index conceals
                // the VALUE, never the nullness.
                "indexed": f.indexed,
                // ── the three field declarations this document omitted ──
                //
                // `plan-schema-evolution.md` §1.1(a): a program that added
                // `enum(…)` or changed a default produced BYTE-IDENTICAL
                // artifact bytes, so `store` answered `AlreadyStored`, the
                // differing DDL was discarded, and `ordo apply` told the
                // operator the change was already deployed. They are
                // contract facts — a value set is a refusal class a caller
                // can hit, a default decides what a create without the
                // field writes — so they belong in the document a deploy
                // diff reads, not only in the DDL.
                //
                // `default` is rendered by `ordo_schema::diff::render_default`
                // rather than by `ddl::literal`: the latter REFUSES a
                // non-literal, and `= today` is exactly the difference
                // between §5 A3 (free) and §5 B1 (a backfill job).
                "default": match &f.default {
                    None => Value::Null,
                    Some(d) => json!(ordo_schema::diff::render_default(d)),
                },
                "normalize": f.normalize,
                "enum_values": f.enum_values,
                // `text(64)`: a bound a caller can hit (`bound_violation`)
                // and a form can show, so it is a contract fact like the
                // value set above. The open column's `CHECK` also rides in
                // the entity's `checks`; this key is the only place a
                // sealed field's bound is stated.
                "text_max": f.text_max,
                // `money(2)`: the currency's places, a contract fact like
                // the bound above (27ak). The SDK spells the stored `2999`
                // as `"29.99"` with it, and a form shows two decimals.
                // `null` for every type that has none.
                "places": match &f.ty {
                    FieldTy::Scalar(s) => s.places(),
                    FieldTy::Ref { .. } => None,
                },
            });
            // `public`, resolved (field mark, entity line, or an owner's
            // `public*`). Written only when set, so a program with no
            // public field keeps the bytes, and the version, it had.
            if f.public {
                field["public"] = json!(true);
            }
            field
        })
        .collect();
    let mut out = json!({
        "entity": e.name,
        "key": e.key,
        "tree": e.is_tree,
        "acl_levels": e.acl_levels,
        // `std sealed` (DL §3): "a shape line, so it shows in source, in
        // the deploy diff, in the shape summary". Adding or removing it
        // changes what the derived std ops disclose, so a reviewer must see
        // it move here and not only in the derived manifests.
        "std_sealed": e.std_sealed,
        "fields": fields,
        // ── the two entity declarations this document omitted ──
        //
        // The worst of the five, because `unique (a, b)` is a constraint an
        // operator believes is deployed. Both are already `Vec<Vec<String>>`
        // in declaration order, which is a property of the source text, so
        // they need no sorting to be byte-stable.
        "unique_keys": e.unique_keys,
        "indexes": e.indexes,
        // ── the SIXTH declaration this document omitted ──
        //
        // Found while building the interface descriptor, and it is exactly
        // `plan-schema-evolution.md` §1.1(a)'s family: `check used <= seats`
        // reaches `ddl_for` (a real CHECK constraint, enforced inside every
        // write's transaction) and reached this document nowhere. So a
        // deploy that tightened, loosened or removed a row invariant showed
        // a reviewer no line — `program_version` moved, because it hashes
        // the DDL too, but the diff a human reads was silent about WHAT
        // moved, which is the half that fix was for.
        //
        // It is also a contract fact by the same test the other five pass:
        // a check is a **refusal class a caller can hit**
        // (`check_violation`, 409), and the constraint name it reports is
        // `chk.name` — so a client that wants to say which invariant failed
        // needs this list. Both the name and the author's condensed source
        // ride; the rendered SQL does not, for `plan.sql`'s reason.
        "checks": e.checks.iter()
            .map(|c| json!({ "name": c.name, "source": c.source }))
            .collect::<Vec<_>>(),
        "guards": match &e.guards {
            None => Value::Null,
            Some(g) => json!({
                "source": guard_source(&g.source),
                "read": g.render(true),
                "write": g.render(false),
            }),
        },
    });
    // The entity line `public` / `public*`, when there is one: a reviewer
    // sees the mark move in the deploy diff, and sees where an inherited one
    // came from. Absent otherwise, so older programs keep their bytes.
    if let Some(p) = &e.public {
        out["public"] = json!({ "cascade": p.cascade, "from": p.from });
    }
    out
}

/// The **catalog manifest**: artifact in, the document a consent page and an
/// agent's context read out.
///
/// This is the projection `ordo_host::manifest` publishes, and it lives here
/// rather than there for this module's own layering reason: every field it
/// selects is compiler output, and the crate that produces a fact is where
/// the rendering of it belongs. `ordo-host` still owns *who may see which
/// op*; it no longer owns *what an op discloses*. (It was written in
/// `ordo-host` because that is where the catalog route is, which is a fact
/// about the HTTP surface and not about the document.)
///
/// Three deliberate differences from the artifact, all of them subtraction
/// or presentation:
///
/// - **The planned SQL is withheld.** The catalog is published to every
///   caller who may invoke the op; the statement is the deployment's
///   internals, and no consent question is answered by it. The artifact —
///   a deploy-time document read by operators — keeps it.
/// - **`cost.rows` is prose** (`<= 3`, `unbounded (pageable)`), because the
///   catalog's two renderings are read by a human and by a model. The
///   artifact carries the number, so a reworded string is never a deploy
///   diff.
/// - **`flags` is flattened** to the two disclosures a consent page states
///   (`unguarded`, `unbounded`) with `bypassed_guards` beside them (R-G8:
///   the flag never travels alone).
pub fn catalog_view(art: &Value) -> Value {
    let cost = match art["cost"] {
        Value::Null => json!({ "class": Value::Null, "rows": Value::Null }),
        _ => {
            let rows = match art["cost"]["row_bound"].as_i64() {
                Some(n) => RowBound::AtMost(n).as_string(),
                None => RowBound::Unbounded.as_string(),
            };
            json!({ "class": art["cost"]["class"], "rows": rows })
        }
    };
    let mut view = json!({
        "op": art["op"],
        "version": art["version"],
        "kind": art["kind"],
        // The op-level union (§6 rule 1) …
        "mode": art["mode"],
        // … and the per-statement list the same rule requires the catalog
        // to carry, so a transfer that creates one row and converges two
        // others is readable as what it is.
        "statements": art["statements"],
        "cost": cost,
        "params": art["params"],
        "outputs": art["outputs"],
        // The result-shape facts a renderer needs and a consent page does
        // not object to: which table the rows came from, whether they are
        // synthetic (grouped), and whether the op is a measure.
        "result": art["result"],
        "entities": art["entities"],
        "scope": art["scope"],
        // WHICH FIELDS beside WHICH ROWS (8s). A consent page that shows the
        // columns and not the row bound is the page DL §8's counterexample
        // walks straight through: delete `member == $caller` and every
        // column stays the same.
        "row_scope": art["row_scope"],
        "release": art["release"],
        "guards": art["guards"],
        "unguarded": art["flags"]["unguarded"],
        "unbounded": art["flags"]["unbounded"],
        "bypassed_guards": art["bypassed_guards"],
        // An op that does not compile is listed with its refusal rather
        // than quietly described as if it worked: its scope is empty
        // because there is no bound plan to read fields off, and an empty
        // scope with no stated reason would read as "touches nothing".
        "error": art["error"],
    });
    // A visitor with no token may call it (`design-public-path.md` §1). The
    // interface descriptor reads this to set the refinement tier to `none`:
    // a public read takes its declared parameters only. Absent when false,
    // so a program with no public op publishes the bytes it did.
    if art["flags"]["public"].as_bool().unwrap_or(false) {
        view["public"] = json!(true);
    }
    view
}

/// Every entity's artifact object, in declaration order — the half of
/// [`program_artifact`] a caller that already holds per-op manifests still
/// needs.
///
/// It exists so the interface descriptor can be assembled from the manifests
/// a deployment has already cached plus this, rather than by re-running
/// [`program_artifact`] (which re-binds and re-plans every op) at request
/// time. Same producer, same bytes: this IS `program_artifact`'s `entities`.
pub fn entity_artifacts(schema: &Schema) -> Vec<Value> {
    schema.entities.iter().map(entity_artifact).collect()
}

/// The entity a typed id of `entity.field` identifies: the entity itself for
/// `id`, the ref's target for a ref field, and `None` for anything else.
///
/// One function, because "what does this id point at" is asked of a
/// parameter (action discovery) and of an output column (detail linkage),
/// and two spellings of it would disagree on the day someone adds a case.
fn id_target(schema: &Schema, entity: &str, field: &str) -> Option<String> {
    if field == "id" {
        return schema.entity(entity).map(|e| e.name.clone());
    }
    match schema
        .entity(entity)
        .and_then(|e| e.field(field))
        .map(|f| &f.ty)
    {
        Some(FieldTy::Ref { target, .. }) => Some(target.clone()),
        _ => None,
    }
}

/// The base table a bound query's rows come from, when there is exactly one.
///
/// `Sub` unwraps — a callee wrapped because it had a `take` or a `group` is
/// still anchored on whatever it was anchored on. A `Union` (`append`) and a
/// `CteRef` have no single anchor and say so.
/// A read that can be kept in sync on a device: the entity its rows are
/// stored rows of, and the output that carries each row's id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Synced {
    pub entity: String,
    pub id_output: String,
}

/// Where a row's anchor id can be read from (`plan-replica-feed.md` C4e).
///
/// `ui-gen.md` §1 has always said the anchor's id rides every list row even
/// when unprojected, because it already underwrites the page cursor. This is
/// how a caller finds it: either the projection carries it, or it is one of
/// the hidden order-key columns the paging planner appends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdKey {
    /// The projection carries it, under this output name. The name is the
    /// author's, which need not be `id`: `customers[cid: id]` is `cid`.
    Projected(String),
    /// It rides hidden: this many hidden cells past the visible outputs.
    ///
    /// The count is of EARLIER hidden keys, not of earlier terms. A term
    /// already projected occupies no hidden cell, so counting terms would
    /// read a sort value as the id, which the difference pass would treat
    /// as churn rather than fail on.
    Hidden(usize),
}

/// The output name an attached anchor id arrives under.
///
/// `$` is the platform's reserved prefix in every layer (`$space`, `$refine`,
/// `$k{i}`, `$tb{i}`, `$caller`), and the lexer routes a leading `$` to the
/// parameter rule, so no author can declare a column that collides with this.
pub const ROW_KEY: &str = "$id";

/// What a read's rows are keyed by, once the key is certain to be projected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowKey {
    pub entity: String,
    /// The output carrying the id: the author's own name for it, or
    /// [`ROW_KEY`] when this call attached it.
    pub name: String,
    /// Whether this call added the output. False when the author projected
    /// it already, which is why the 32 ops that do keep their manifests
    /// unchanged.
    pub attached: bool,
}

/// Put the anchor's id in the projection if it is not there, and say what the
/// rows are keyed by (`ui-gen.md` §1: the id rides every list row even when
/// unprojected).
///
/// Projecting it, rather than reading a hidden cursor cell out of the row on
/// the way past, is what makes the paged path, the unpaged path, the manifest
/// and the SQL agree without special cases: from here on the id is an output
/// like any other.
///
/// `None` when the rows have no identity to key by, and deliberately the SAME
/// answer `syncability` gives: a grouped result, a measure, a `distinct`, a
/// many-side join or a result with no anchor gets nothing attached. Two
/// predicates would drift, and the one that drifted would put an id on rows
/// that do not have one.
pub fn project_row_key(q: &mut Query) -> Option<RowKey> {
    let d = row_key(q).ok()?;
    let attached = d.attach.is_some();
    if let Some(expr) = d.attach {
        q.outputs.push(ordo_sema::ir::OutField {
            name: ROW_KEY.to_string(),
            expr,
        });
    }
    Some(RowKey {
        entity: d.entity,
        name: d.name,
        attached,
    })
}

/// One refinement condition on [`ROW_KEY`], with its values already checked
/// by the door as id strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKeyCond {
    /// `eq ne lt le gt ge` against one id.
    Cmp(ordo_syntax::CmpOp, String),
    /// `in`, a `None` member meaning `null`.
    In(Vec<Option<String>>),
    /// `is_null`, which no anchored row meets.
    IsNull,
}

/// Filter a bound read on its attached anchor id (C4e, 2026-09-17: *"`$id`
/// is the attached name, and it is refinable"*).
///
/// A refinement compiles through the composition inliner, and a call's
/// conditions name the callee's outputs. The op's body cannot name `$id`,
/// because the id is attached after binding by [`project_row_key`], so a
/// condition on it cannot be written into the call. It is applied here
/// instead, to the same expression `project_row_key` would project: the
/// anchor's id column at this level, which is the wrapped callee's `$tb0`
/// when the op's rows were settled by a `take`. Being on the outer query, the
/// filter applies after the take, as every other condition does.
///
/// Run it before `project_row_key`. `Err` when the read has no attached
/// anchor id, which the door has already refused, since only an attached id
/// is a `$id` output.
pub fn filter_row_key(q: &mut Query, conds: &[RowKeyCond]) -> Result<(), String> {
    use ordo_syntax::{decl::ScalarTy, Span};
    if conds.is_empty() {
        return Ok(());
    }
    let d = row_key(q).map_err(|r| format!("`{ROW_KEY}` has no anchor here ({})", r.as_str()))?;
    let Some(lhs) = d.attach else {
        return Err(format!(
            "`{ROW_KEY}` is not attached here: the op projects its id as `{}`",
            d.name
        ));
    };
    let lit = |id: &str| BExpr::Lit {
        value: Expr::StrLit {
            value: id.to_string(),
            span: Span::new(0, 0, 1, 1),
        },
        ty: ScalarTy::Id,
    };
    let is_null = |negated: bool| BCond::IsNull {
        expr: lhs.clone(),
        negated,
    };
    for c in conds {
        let cond = match c {
            RowKeyCond::Cmp(op, id) => BCond::Cmp {
                op: *op,
                lhs: lhs.clone(),
                rhs: lit(id),
            },
            RowKeyCond::IsNull => is_null(false),
            RowKeyCond::In(ids) => {
                let list: Vec<BExpr> = ids.iter().flatten().map(|id| lit(id)).collect();
                let has_null = ids.iter().any(Option::is_none);
                let mut any: Vec<BCond> = Vec::new();
                if !list.is_empty() {
                    any.push(BCond::In {
                        expr: lhs.clone(),
                        list,
                    });
                }
                if has_null {
                    any.push(is_null(false));
                }
                match any.len() {
                    // `in []` matches nothing. SQL has no empty `IN ()`, so
                    // it is the one condition no value meets.
                    0 => BCond::And(vec![is_null(false), is_null(true)]),
                    1 => any.remove(0),
                    _ => BCond::Or(any),
                }
            }
        };
        q.filters.push(cond);
    }
    Ok(())
}

/// Where a read's anchor id can be read from, as a cross-check on the attach.
///
/// The attach itself clones the order term's expression, so this is no longer
/// on the hot path; it stays because the `Hidden(n)` arithmetic is the part
/// that fails silently, and a test that pins it is worth more than the code
/// that avoids it.
pub fn anchor_id_key(q: &Query) -> Option<IdKey> {
    let d = row_key(q).ok()?;
    match d.attach {
        None => Some(IdKey::Projected(d.name)),
        Some(_) => {
            let at = crate::order_key_of_col(q, q.from.alias(), &d.column)?.0;
            let names = crate::order_key_outputs(q);
            Some(IdKey::Hidden(
                names[..at].iter().filter(|n| n.is_none()).count(),
            ))
        }
    }
}

/// The one decision: what a read's rows are keyed by, and whether the key has
/// to be attached to get there.
///
/// Both halves of the contract read this. [`syncability`] answers the door
/// with the NAME, and [`project_row_key`] puts the output in place before
/// planning. Two functions deciding separately could return different names,
/// and then the descriptor would say rows are keyed by one thing while the
/// wire keyed them by another — which nothing would detect until a device
/// filed a row under a key the server never sends.
fn row_key(q: &Query) -> Result<Decided, SyncRule> {
    if let Some(g) = &q.group {
        return Err(if g.keys.is_empty() {
            SyncRule::Measure
        } else {
            SyncRule::Grouped
        });
    }
    // Two shapes that are anchored and ungrouped and still have no row
    // identity. Both were refused only by accident until 2026-09-16, through
    // the old `id_not_projected`: an op that DID project its id was synced
    // wrongly, because `distinct` rows are not stored rows and a many-side
    // join's rows share their anchor's id.
    if q.distinct {
        return Err(SyncRule::Distinct);
    }
    if q.source_joins.iter().any(|j| j.many_side) {
        return Err(SyncRule::ManySideJoin);
    }

    let (entity, id_column) = match &q.from {
        IrFrom::Table { entity, .. } => (entity.clone(), "id".to_string()),
        IrFrom::Sub { query, .. } => {
            let inner = row_key(query)?;
            // A callee that did not project its own id carries its tiebreak
            // out as the hidden `$tb0` the binder adds, so the id does ride
            // and the column is that rather than a guess.
            let column = match inner.attach {
                Some(_) => "$tb0".to_string(),
                None => inner.name,
            };
            (inner.entity, column)
        }
        IrFrom::Union { .. } | IrFrom::CteRef { .. } => return Err(SyncRule::NoAnchor),
    };

    let alias = q.from.alias();
    if let Some(o) = q.outputs.iter().find(|o| {
        matches!(&o.expr, BExpr::Col { alias: a, column: c, .. } if *a == alias && *c == id_column)
    }) {
        return Ok(Decided {
            entity,
            name: o.name.clone(),
            column: id_column,
            attach: None,
        });
    }

    // Not projected, so it is attached. The expression is CLONED from the
    // order term rather than rebuilt: `output_named` compares whole
    // expressions including their type, and a rebuilt one would fail to match
    // its own term and ride both projected and hidden.
    let (_, expr) = crate::order_key_of_col(q, alias, &id_column).ok_or(SyncRule::NoAnchor)?;
    Ok(Decided {
        entity,
        name: ROW_KEY.to_string(),
        column: id_column,
        attach: Some(expr),
    })
}

/// What [`row_key`] decided, before anything acts on it.
struct Decided {
    entity: String,
    /// The output the id arrives under: the author's name, or [`ROW_KEY`].
    name: String,
    /// The column carrying it at this level, which is `$tb0` through a
    /// wrapped callee that did not project its own.
    column: String,
    /// The expression to project, when the key is not in the projection yet.
    attach: Option<BExpr>,
}

/// Which rule of `plan-replica-feed.md` §1 a read fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncRule {
    /// A union or a CTE: no single base table anchors the rows.
    NoAnchor,
    /// A keyed group: its rows are synthetic.
    Grouped,
    /// A keyless group (`reduce`): one synthetic row.
    Measure,
    /// `distinct` settles the row set: the rows are the distinct output
    /// tuples, not stored rows, and the tiebreak becomes the outputs rather
    /// than the anchor's `id`. The binder already holds this rule — a callee
    /// that is `distinct` is "settled" and wrapped — and this states it here
    /// rather than catching it by the accident of an unprojected id.
    Distinct,
    /// A many-side source join fans one anchor row into several result rows,
    /// so the anchor's `id` no longer keys a row: siblings would share a key
    /// and overwrite each other on the device.
    ManySideJoin,
}

impl SyncRule {
    /// The manifest spelling, which `ordo_api::NotSyncable::as_str` repeats.
    pub fn as_str(self) -> &'static str {
        match self {
            SyncRule::NoAnchor => "no_anchor",
            SyncRule::Grouped => "grouped",
            SyncRule::Measure => "measure",
            SyncRule::Distinct => "distinct",
            SyncRule::ManySideJoin => "many_side_join",
        }
    }
}

/// Whether this read can be kept in sync on a device, and which output keys
/// its rows (`plan-replica-feed.md` §1): every row is a stored row of one
/// entity, and the id is in the projection or is attached to it (C4e).
///
/// Stricter than [`anchor_entity`] in one place. A callee that grouped is
/// wrapped as a subquery, and its caller's rows are its synthetic rows, so
/// the walk refuses a group at any depth rather than only at the top.
pub fn syncability(q: &Query) -> Result<Synced, SyncRule> {
    row_key(q).map(|k| Synced {
        entity: k.entity,
        id_output: k.name,
    })
}

fn anchor_entity(f: &IrFrom) -> Value {
    match f {
        IrFrom::Table { entity, .. } => Value::from(entity.clone()),
        IrFrom::Sub { query, .. } => anchor_entity(&query.from),
        IrFrom::Union { .. } | IrFrom::CteRef { .. } => Value::Null,
    }
}

/// The order a read's rows arrive in, stated so the device copy can return
/// them in it (27o, ruled 2026-10-01: the device keeps each read's own
/// order *"as long as the language allows it"*).
///
/// `{ "keys": [{ "path", "desc" }, ...], "stated": bool }`. Every plan ends
/// with a total order, the declared sort keys and then the id tiebreak
/// (`plan.rs`), and the order is **stated** when every one of those keys is
/// an output of the read that is not sealed. The device holds exactly the
/// output row, so a key it can order by is a key it holds in the clear; a
/// key named here is an output name, the id last, as the server orders.
///
/// Otherwise `stated` is `false` and `keys` is the id alone, and the device
/// keeps id order: a grouped or reduced result with no `sort`, whose bucket
/// order is the planner's; a sort key the read computes and does not
/// project (a `cond`, a `lower`, a sum), which the device cannot evaluate; a
/// sealed sort key, which the device cannot compare. A projected expression
/// is an output like any other, so `[shout: upper(name)] sort shout` states
/// its order.
///
/// Compared by expression, not by name: a sort over `price` and an output
/// `price` are the same column of the same alias, and an output renamed
/// `cost` still carries it. Matching names would miss the second and could
/// pair a sort with an output that merely shares a word.
fn read_order(q: &Query) -> Value {
    let id_name = row_key(q)
        .map(|k| k.name)
        .unwrap_or_else(|_| "id".to_string());
    let unstated = || json!({ "keys": [{ "path": id_name, "desc": false }], "stated": false });
    if q.group.is_some() && q.sort.is_empty() {
        return unstated();
    }
    let output_named = |e: &BExpr| -> Option<String> {
        q.outputs
            .iter()
            .find(|o| o.expr == *e)
            .map(|o| o.name.clone())
            .filter(|n| !q.host_open.iter().any(|(h, _, _)| h == n))
    };
    let mut keys = Vec::new();
    let mut seen: Vec<&BExpr> = Vec::new();
    for (expr, desc) in q
        .sort
        .iter()
        .map(|s| (&s.expr, s.desc))
        .chain(q.tiebreak.iter().map(|t| (t, false)))
    {
        // A key already present is not repeated, as the planner's
        // `order_terms` does not repeat it: `sort -id` followed by the id
        // tiebreak is one key, and the direction is the first one's.
        if seen.contains(&expr) {
            continue;
        }
        seen.push(expr);
        match output_named(expr) {
            Some(path) => keys.push(json!({ "path": path, "desc": desc })),
            None => return unstated(),
        }
    }
    json!({ "keys": keys, "stated": true })
}

/// Output name → the `(entity, column)` it is a plain projection of.
///
/// Only `BExpr::Col` over an alias the query's own alias map resolves to a
/// base table: an aggregate, an arithmetic expression or a column off a CTE
/// is not a projection of a stored field and must not be described as one.
fn output_origins(q: &Query) -> BTreeMap<String, (String, String)> {
    let mut r = Reads::default();
    r.query(q);
    let mut out = BTreeMap::new();
    for o in &q.outputs {
        if let BExpr::Col { alias, column, .. } = &o.expr {
            if let Some(entity) = r.alias_entity.get(alias) {
                out.insert(o.name.clone(), (entity.clone(), column.clone()));
            }
        }
    }
    out
}

/// Where an op's parameters land in the schema — both facts, from one walk.
///
/// A `None` value means **ambiguous**: the body lands the same parameter in
/// two different places, and the producer must not pick one. The two maps
/// carry their ambiguity independently, because a parameter can be
/// unambiguous about its entity and ambiguous about its field (two entities
/// with a same-named ref to one target) or the reverse.
#[derive(Default)]
struct ParamSites {
    /// Parameter → the entity whose typed id it addresses.
    id: BTreeMap<String, Option<String>>,
    /// Parameter → the `(entity, declared field)` its value fills.
    field: BTreeMap<String, Option<(String, String)>>,
}

/// Is this parameter the count of a `take` anywhere in the op's body?
///
/// The producer of `params[].min`, and the one `ordo_host::param_bounds`
/// calls for `ordo_api::Bounds::min` — one function, because the floor the
/// door refuses below and the floor this document discloses are one fact.
/// It was two: the derivation lived only in the wiring crate, so the
/// artifact and the descriptor carried a `max` and stayed silent about a
/// bound the door was already enforcing.
pub fn take_count_param(op: &ordo_syntax::decl::OpDecl, pname: &str) -> bool {
    fn walk(pl: &ordo_syntax::Pipeline, pname: &str) -> bool {
        pl.lets.iter().any(|l| walk(&l.pipeline, pname))
            || pl.steps.iter().any(|st| {
                matches!(
                    st,
                    ordo_syntax::Step::Take {
                        n: ordo_syntax::TakeCount::Param { name },
                        ..
                    } if name.name == pname
                )
            })
    }
    op.read_pipeline().is_some_and(|pl| walk(pl, pname))
}

/// Parameter name → the entity whose typed id it addresses, and the declared
/// field it fills (`None` on either where the body lands the same parameter
/// in two different ones, which is ambiguous and must not be guessed).
///
/// Read off the **declaration**, not the plan: a write has no bound query at
/// all, and for a read the addressing that matters is the one written in the
/// source (`tasks(id == $id)`), which survives binding as a filter over an
/// alias the caller would then have to resolve backwards. Walking the AST
/// once, with the entity in hand, is the shorter and more honest route.
///
/// Deliberately conservative: only a comparison or an `in` whose other side
/// is exactly a parameter, only paths that resolve through refs to a real
/// field, and only sources that name a schema entity. A parameter this
/// cannot type is `null`, which costs a UI one row action or one dropdown it
/// could have offered; a parameter it types WRONGLY would offer an action
/// that does not apply to the row, or a value set the column will refuse,
/// which is the failure worth avoiding.
fn param_sites(schema: &Schema, registry: &Registry, name: &str) -> ParamSites {
    let mut out = ParamSites::default();
    let Some(op) = registry.get(name) else {
        return out;
    };
    match &op.body {
        OpBody::Write(stmts) => {
            for s in stmts {
                match s {
                    WriteStmt::Set {
                        entity,
                        cond,
                        fields,
                        ..
                    } => {
                        if let Some(c) = cond {
                            cond_param_sites(schema, &entity.name, c, &mut out);
                        }
                        // `set tasks { project: $p }` types `$p` as a
                        // `projects` id just as surely as a condition does.
                        for (f, v) in fields {
                            if let Expr::Param { name: p, .. } = v {
                                note_param_site(
                                    schema,
                                    &entity.name,
                                    std::slice::from_ref(&f.name),
                                    p,
                                    &mut out,
                                );
                            }
                        }
                    }
                    WriteStmt::Del { entity, cond, .. } => {
                        cond_param_sites(schema, &entity.name, cond, &mut out)
                    }
                }
            }
        }
        OpBody::Read(pl) => read_param_sites(schema, pl, &mut out),
    }
    out
}

/// The source-argument conditions of a pipeline and of every `let` it binds.
///
/// Step conditions are **not** walked: after the first step a condition is
/// over the pipeline's own output names, not over one entity's fields, and
/// resolving those means resolving aliases — which is exactly the backwards
/// route [`param_sites`] declines to take.
fn read_param_sites(schema: &Schema, pl: &ordo_syntax::Pipeline, out: &mut ParamSites) {
    for l in &pl.lets {
        read_param_sites(schema, &l.pipeline, out);
    }
    for s in &pl.sources {
        if schema.entity(&s.name.name).is_none() {
            continue;
        }
        for a in &s.args {
            if let ordo_syntax::decl::SourceArg::Cond(c) = a {
                cond_param_sites(schema, &s.name.name, c, out);
            }
        }
    }
}

fn cond_param_sites(schema: &Schema, entity: &str, c: &Condition, out: &mut ParamSites) {
    let pair = |lhs: &Expr, rhs: &Expr, out: &mut ParamSites| {
        if let (Expr::Path { segments, .. }, Expr::Param { name, .. })
        | (Expr::Param { name, .. }, Expr::Path { segments, .. }) = (lhs, rhs)
        {
            let path: Vec<String> = segments.iter().map(|s| s.name.clone()).collect();
            note_param_site(schema, entity, &path, name, out);
        }
    };
    match c {
        Condition::Compare { lhs, rhs, .. } => pair(lhs, rhs, out),
        Condition::In { expr, list, .. } => {
            for e in list {
                pair(expr, e, out);
            }
        }
        Condition::And { items, .. } | Condition::Or { items, .. } => {
            for i in items {
                cond_param_sites(schema, entity, i, out);
            }
        }
        Condition::Not { inner, .. } => cond_param_sites(schema, entity, inner, out),
        _ => {}
    }
}

/// Record where `param` lands: the entity whose typed id `entity`.`path`
/// identifies, and the declared field it is. Either is marked ambiguous if
/// this site disagrees with one already recorded.
fn note_param_site(
    schema: &Schema,
    entity: &str,
    path: &[String],
    param: &str,
    out: &mut ParamSites,
) {
    // Walk the dot path through refs: `project.owner == $p` types `$p` by
    // `people`, not by `projects`.
    let mut cur = entity.to_string();
    for seg in &path[..path.len().saturating_sub(1)] {
        match schema
            .entity(&cur)
            .and_then(|e| e.field(seg))
            .map(|f| &f.ty)
        {
            Some(FieldTy::Ref { target, .. }) => cur = target.clone(),
            _ => return,
        }
    }
    let Some(last) = path.last() else { return };

    // The declared field, when there is one. `id`, `rev` and `depth` are
    // implicit — the platform's, absent from `entities[].fields`, and so not
    // a field a `field_ref` may name.
    if let Some(f) = schema
        .entity(&cur)
        .and_then(|e| e.field(last))
        .filter(|f| !f.implicit)
    {
        let site = (cur.clone(), f.name.clone());
        note(&mut out.field, param, site);
    }

    if let Some(target) = id_target(schema, &cur, last) {
        note(&mut out.id, param, target);
    }
}

/// First site wins; a second, different site makes it ambiguous (`None`).
fn note<T: PartialEq>(map: &mut BTreeMap<String, Option<T>>, param: &str, site: T) {
    match map.get(param) {
        Some(Some(prior)) if *prior != site => {
            map.insert(param.to_string(), None);
        }
        Some(_) => {}
        None => {
            map.insert(param.to_string(), Some(site));
        }
    }
}

fn guard_source(s: &GuardSource) -> String {
    match s {
        GuardSource::Own => "own".into(),
        GuardSource::Inherited { from } => format!("inherited from {from}"),
        GuardSource::AclDerived => "acl-derived".into(),
    }
}

/// Entities reachable from `name`, following composed callees. Cycles are
/// impossible (the registration gate refuses them) but `seen` keeps this
/// total anyway — an artifact producer that can hang is worse than one
/// that under-reports.
fn reached_entities(
    schema: &Schema,
    registry: &Registry,
    name: &str,
    out: &mut Vec<String>,
    seen: &mut Vec<String>,
) {
    if seen.iter().any(|s| s == name) {
        return;
    }
    seen.push(name.to_string());
    let Some(op) = registry.get(name) else { return };
    let mut srcs: Vec<String> = Vec::new();
    match &op.body {
        OpBody::Read(pl) => pipeline_sources(pl, &mut srcs),
        OpBody::Write(stmts) => {
            for s in stmts {
                srcs.push(match s {
                    WriteStmt::Set { entity, .. } | WriteStmt::Del { entity, .. } => {
                        entity.name.clone()
                    }
                });
            }
        }
    }
    for s in srcs {
        if schema.entity(&s).is_some() {
            out.push(s);
        } else {
            reached_entities(schema, registry, &s, out, seen);
        }
    }
}

fn pipeline_sources(pl: &ordo_syntax::Pipeline, out: &mut Vec<String>) {
    for l in &pl.lets {
        pipeline_sources(&l.pipeline, out);
    }
    for s in &pl.sources {
        out.push(s.name.name.clone());
    }
    for st in &pl.steps {
        match st {
            ordo_syntax::Step::Match {
                kind: ordo_syntax::decl::MatchKind::Any(s) | ordo_syntax::decl::MatchKind::None(s),
                ..
            }
            | ordo_syntax::Step::Append { source: s, .. } => out.push(s.name.name.clone()),
            _ => {}
        }
    }
}

// ── the field-scope manifest (9l) ────────────────────────────────────────

/// **Which fields of one entity an op reaches** — the unit of the scope
/// manifest.
///
/// An entity list is the wrong grain for a consent page and always was: a
/// caller consenting to `tasks` consents to every column of every row the
/// guard admits, when the op reads three of them. `composition-conformance`
/// §1(d) (C116–C118) and `guards-conformance` §J (GC007/GC027/GC048/
/// GC052/GC080/GC082) both state the manifest at field grain —
/// `tasks.{status, title, project}`, not `tasks` — and this is that.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntityScope {
    pub entity: String,
    /// Base fields the op reads. Sorted, deduped, and **post-pruning**
    /// (R-C11): a callee output the caller's projection drops is not read,
    /// because the flattened plan never selects it.
    pub read: BTreeSet<String>,
    /// Base fields a write statement assigns.
    pub written: BTreeSet<String>,
    /// A tree traversal read the materialized path (or `depth`): structure,
    /// no values. C117's `categories.(tree structure)`.
    pub structure: bool,
    /// How the op arrived at this entity.
    ///
    /// - `body` — the source text names it (directly or through a callee).
    /// - `guard` — **only** an inlined guard reaches it: rows the caller
    ///   never named and cannot see, examined to decide what they may see.
    ///   That is disclosure, and it is the half an entity list hid.
    /// - `traversal` — a tree walk (`under`/`above`) crossed into it.
    pub via: &'static str,
}

/// Every entity whose rows a delete of `entity` can take with it: the
/// entities that reference it through an `owned` ref, and those that own
/// rows of theirs in turn. A tree's own parent ref is not ownership here.
fn owned_entities(schema: &Schema, entity: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut todo = vec![entity.to_string()];
    while let Some(cur) = todo.pop() {
        for other in &schema.entities {
            for f in &other.fields {
                let FieldTy::Ref {
                    target,
                    owned: true,
                    tree,
                } = &f.ty
                else {
                    continue;
                };
                if *target != cur || (*tree && other.name == cur) {
                    continue;
                }
                if other.name != entity && !out.contains(&other.name) {
                    out.push(other.name.clone());
                    todo.push(other.name.clone());
                }
            }
        }
    }
    out
}

/// The op's field-scope manifest, entity-sorted.
///
/// # How the field set is derived
///
/// For a **read**, from the **bound IR** and nothing else. Binding is what
/// flattens composition (a callee is merged into its caller, so a callee's
/// fields are the caller's exposure) and what inlines guards (G15), so
/// every base column the plan will touch — projections, filter conditions,
/// sort keys, group keys, join keys, guard walks, blind-index comparisons —
/// is a `BExpr::Col` somewhere in one query tree. Deriving it from the AST
/// instead would mean re-implementing the inliner and the guard compiler,
/// and the copy would drift; deriving it from the SQL string would mean
/// parsing SQL. The IR is the one place the answer is already exact.
///
/// For a **write**, from the statements plus the guard preconditions: the
/// braces are the written fields, the condition and the right-hand sides
/// are read fields, and `ordo_sema::guard_filter_query` supplies the
/// guard's own reads for both verbs — both, because the write path
/// evaluates the READ guard too, to tell `not_found` from `access_guard`.
///
/// # What is deliberately not listed
///
/// `id`, `rev` and `$space` are the platform's, not the caller's data:
/// every guard semi-join correlates on `id` and every ordering tiebreaks on
/// it, so listing them would drown the fields a reader is actually
/// consenting to. `$path` and `depth` set [`EntityScope::structure`]
/// instead — a tree walk reads shape, not values.
pub fn op_scope(schema: &Schema, registry: &Registry, name: &str) -> Vec<EntityScope> {
    let mut acc = Scope::default();
    let Some(op) = registry.get(name) else {
        return Vec::new();
    };

    // The entities the BODY reaches: the sources it names here, and for a
    // read every entity its unguarded bind touches (below). Everything else
    // in the finished scope arrived through a guard, and saying which is
    // which is the point.
    let mut named: Vec<String> = Vec::new();
    reached_entities(schema, registry, name, &mut named, &mut Vec::new());

    match &op.body {
        OpBody::Read(_) => {
            match bind_op_generic(schema, registry, name, ordo_sema::Clock::CORPUS) {
                Ok(q) => {
                    acc.absorb(schema, &q);
                    // Which of those entities the op's OWN pipeline reaches:
                    // the same op bound with no guard inlined. A ref hop in a
                    // projection, condition, sort or group is a join here, so
                    // `submitter.displayName` makes `members` the body's even
                    // though no source names it (25bf). Only what this bind
                    // never touches was reached by a guard alone.
                    if let Ok(body) = ordo_sema::bind_op_body_only(
                        schema,
                        registry,
                        name,
                        ordo_sema::Clock::CORPUS,
                    ) {
                        let mut own = Scope::default();
                        own.absorb(schema, &body);
                        for e in own.map.into_values() {
                            // Structure alone is a tree walk crossing into
                            // the entity, which keeps its own marker.
                            let walked = e.structure && e.read.is_empty();
                            if !walked && !named.contains(&e.entity) {
                                named.push(e.entity);
                            }
                        }
                    }
                }
                // An op that does not compile has no bound IR to read fields
                // off. It still names its entities, and its artifact carries
                // the `error` — an empty field list beside a stated error is
                // honest; a guessed one would not be.
                Err(_) => {
                    for e in &named {
                        acc.entry(e);
                    }
                }
            }
        }
        OpBody::Write(stmts) => {
            for s in stmts {
                match s {
                    WriteStmt::Set {
                        entity,
                        cond,
                        fields,
                        ..
                    } => {
                        for (n, v) in fields {
                            acc.written(schema, &entity.name, &n.name);
                            acc.ast_expr(schema, &entity.name, v);
                        }
                        if let Some(c) = cond {
                            acc.ast_cond(schema, &entity.name, c);
                        }
                    }
                    WriteStmt::Del {
                        entity,
                        cond,
                        subtree,
                        ..
                    } => {
                        acc.ast_cond(schema, &entity.name, cond);
                        // A subtree cascade walks `$path` to find every
                        // descendant — rows the caller addressed only by
                        // their ancestor.
                        if *subtree {
                            acc.entry(&entity.name).structure = true;
                        }
                    }
                }
                let entity = match s {
                    WriteStmt::Set { entity, .. } | WriteStmt::Del { entity, .. } => &entity.name,
                };
                if !op.flags.unguarded {
                    acc.guards(schema, entity);
                }
                // A delete takes the rows that entity owns with it
                // (owned-delete, 2026-09-29), and the caller must be
                // allowed to delete each of them. Rows of those entities
                // are reached and their guards examined, though the body
                // never names them.
                if matches!(s, WriteStmt::Del { .. }) {
                    for owned in owned_entities(schema, entity) {
                        acc.entry(&owned);
                        if !op.flags.unguarded {
                            acc.guards(schema, &owned);
                        }
                    }
                }
            }
        }
    }

    let mut out: Vec<EntityScope> = acc.map.into_values().collect();
    for e in &mut out {
        // A read's guards are already inlined into the bound query, so the
        // body's own reach comes from the unguarded bind above: an entity
        // that bind never touched and that carries real fields was reached
        // by a guard, and one carrying only tree structure was reached by
        // an `under`/`above` traversal. A write's guards are separate
        // queries, so there the attribution is recorded as it happens, and
        // a ref hop in a statement is the body's (`hopped`). An entity both
        // the body and a guard reach is the body's: what the op itself
        // reads is never understated as a guard's (25bf).
        e.via = if named.contains(&e.entity) || acc.hopped.contains(&e.entity) {
            "body"
        } else if acc.from_guard.contains(&e.entity) {
            "guard"
        } else if e.structure && e.read.is_empty() && e.written.is_empty() {
            "traversal"
        } else {
            "guard"
        };
    }
    out
}

/// The scope manifest as artifact JSON, one object per entity with a fixed
/// key set.
pub fn scope_json(scope: &[EntityScope]) -> Value {
    Value::Array(
        scope
            .iter()
            .map(|s| {
                json!({
                    "entity": s.entity,
                    "read": s.read.iter().collect::<Vec<_>>(),
                    "written": s.written.iter().collect::<Vec<_>>(),
                    "tree_structure": s.structure,
                    "via": s.via,
                })
            })
            .collect::<Vec<_>>(),
    )
}

/// One entity's line as a human (or LLM) reads it:
/// `tasks.{project, status, title}`, `categories.(tree structure)`,
/// `projects.(rows examined, no fields)`.
pub fn render_scope_entry(s: &EntityScope) -> String {
    let list = |f: &BTreeSet<String>| f.iter().cloned().collect::<Vec<_>>().join(", ");
    // `tasks.{status, title}` — the dot belongs to the field list, so an
    // entity with no read fields is written plainly rather than with a
    // dangling dot.
    let mut body = if s.read.is_empty() {
        s.entity.clone()
    } else {
        format!("{}.{{{}}}", s.entity, list(&s.read))
    };
    if !s.written.is_empty() {
        body.push_str(&format!(" writes {{{}}}", list(&s.written)));
    }
    if s.structure {
        body.push_str(" (tree structure)");
    }
    if s.read.is_empty() && s.written.is_empty() && !s.structure {
        // Reached, but no column of it is read: a guard's semi-join asks
        // only whether a row exists. Saying so is not noise — existence is
        // what the caller learns.
        body.push_str(" (rows examined, no fields)");
    }
    match s.via {
        "guard" => format!("{body} — read by a guard, not by the op"),
        "traversal" => format!("{body} — walked by a tree traversal"),
        _ => body,
    }
}

/// [`render_scope_entry`] over one entry of [`scope_json`] — the renderer
/// every consumer of the JSON should call, so `ordo check`, the catalog
/// prose and anything else print one entity's scope the same way.
pub fn render_scope_json(s: &Value) -> String {
    let set = |k: &str| {
        s[k].as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    render_scope_entry(&EntityScope {
        entity: s["entity"].as_str().unwrap_or("?").to_string(),
        read: set("read"),
        written: set("written"),
        structure: s["tree_structure"].as_bool().unwrap_or(false),
        via: match s["via"].as_str() {
            Some("guard") => "guard",
            Some("traversal") => "traversal",
            _ => "body",
        },
    })
}

/// The accumulator: one [`EntityScope`] per entity, built from several
/// sources (the bound query, the write statements, each guard's own query)
/// that must all fold into the same answer.
#[derive(Default)]
struct Scope {
    map: BTreeMap<String, EntityScope>,
    /// Entities touched while walking a guard's own query — the input to
    /// [`EntityScope::via`], recorded as it happens because afterwards
    /// there is no way to tell a guard's read from the body's.
    from_guard: BTreeSet<String>,
    /// Entities a write statement's own condition or right-hand side
    /// dotted into through a ref: the body reaches them, though no
    /// statement names them (25bf).
    hopped: BTreeSet<String>,
    in_guard: bool,
}

impl Scope {
    fn entry(&mut self, entity: &str) -> &mut EntityScope {
        if self.in_guard {
            self.from_guard.insert(entity.to_string());
        }
        self.map
            .entry(entity.to_string())
            .or_insert_with(|| EntityScope {
                entity: entity.to_string(),
                ..EntityScope::default()
            })
    }

    /// One COLUMN, as the manifest states it. The IR speaks in columns and
    /// the manifest speaks in fields: `email$bidx` is a read of `email`
    /// (the digest is derived from the value, and comparing against it is
    /// exactly a membership question about that value), `$path`/`depth` are
    /// structure, and the platform's own columns are nobody's data.
    fn read_col(&mut self, schema: &Schema, entity: &str, column: &str) {
        let base = column.strip_suffix("$bidx").unwrap_or(column);
        if base == "$path" || base == "depth" {
            self.entry(entity).structure = true;
            return;
        }
        match schema
            .entity(entity)
            .and_then(|e| e.field(base))
            .filter(|f| !f.implicit)
        {
            Some(f) => {
                let n = f.name.clone();
                self.entry(entity).read.insert(n);
            }
            // `id`, `rev`, `$space`, and any column the schema does not
            // know: the entity is still reached, so it is still listed.
            None => {
                self.entry(entity);
            }
        }
    }

    fn written(&mut self, schema: &Schema, entity: &str, field: &str) {
        match schema
            .entity(entity)
            .and_then(|e| e.field(field))
            .filter(|f| !f.implicit)
        {
            Some(f) => {
                let n = f.name.clone();
                self.entry(entity).written.insert(n);
            }
            // `id` addresses and `rev` states a belief — neither is a
            // value the caller is writing into the row.
            None => {
                self.entry(entity);
            }
        }
    }

    /// Every base column one bound query reads, resolved through its own
    /// alias map.
    fn absorb(&mut self, schema: &Schema, q: &Query) {
        let mut r = Reads::default();
        r.query(q);
        for (alias, column) in &r.cols {
            // An alias the map does not hold is a CTE, a union arm or a
            // wrapped subquery — not a base table. Its own body is walked
            // in the same pass, and that is where its base columns come
            // from; resolving the outer reference too would double-count
            // at best and invent a field at worst.
            if let Some(e) = r.alias_entity.get(alias) {
                self.read_col(schema, e, column);
            }
        }
        for t in &r.trees {
            self.entry(t).structure = true;
        }
        for (e, f) in &r.blind {
            self.read_col(schema, e, f);
        }
    }

    /// The guard's own reads, for both verbs. A guard is a query the
    /// platform runs on the caller's behalf over rows the caller never
    /// named; the fields it reads are disclosure whether or not they reach
    /// the response.
    fn guards(&mut self, schema: &Schema, entity: &str) {
        self.in_guard = true;
        for write_verb in [false, true] {
            if let Ok(Some(q)) = guard_filter_query(schema, entity, write_verb, Some("$caller")) {
                self.absorb(schema, &q);
            }
        }
        self.in_guard = false;
    }

    /// A write statement's raw-AST expression: writes have no binder, so
    /// the paths are resolved here against the schema, hop by hop.
    fn ast_expr(&mut self, schema: &Schema, entity: &str, e: &Expr) {
        match e {
            Expr::Path { segments, .. } => {
                let mut cur = entity.to_string();
                for (i, seg) in segments.iter().enumerate() {
                    // A segment after a ref is a hop the body makes: it reads
                    // the target's row. Comparing the ref itself
                    // (`project == $p`) reads only this row's column.
                    if i > 0 && !self.in_guard {
                        self.hopped.insert(cur.clone());
                    }
                    self.read_col(schema, &cur, &seg.name);
                    match schema
                        .entity(&cur)
                        .and_then(|x| x.field(&seg.name))
                        .map(|f| &f.ty)
                    {
                        Some(FieldTy::Ref { target, .. }) => cur = target.clone(),
                        _ => break,
                    }
                }
            }
            Expr::Neg { expr, .. } => self.ast_expr(schema, entity, expr),
            Expr::Binary { lhs, rhs, .. } => {
                self.ast_expr(schema, entity, lhs);
                self.ast_expr(schema, entity, rhs);
            }
            Expr::FnCall { args, .. } => {
                for a in args {
                    self.ast_expr(schema, entity, a);
                }
            }
            Expr::Cond { arms, else_, .. } => {
                for a in arms {
                    self.ast_cond(schema, entity, &a.condition);
                    self.ast_expr(schema, entity, &a.value);
                }
                self.ast_expr(schema, entity, else_);
            }
            _ => {}
        }
    }

    fn ast_cond(&mut self, schema: &Schema, entity: &str, c: &Condition) {
        match c {
            Condition::Compare { lhs, rhs, .. } | Condition::Text { lhs, rhs, .. } => {
                self.ast_expr(schema, entity, lhs);
                self.ast_expr(schema, entity, rhs);
            }
            Condition::In { expr, list, .. } => {
                self.ast_expr(schema, entity, expr);
                for e in list {
                    self.ast_expr(schema, entity, e);
                }
            }
            Condition::IsNull { expr, .. } | Condition::BareBool(expr) => {
                self.ast_expr(schema, entity, expr)
            }
            Condition::Since { expr, dur, .. } => {
                self.ast_expr(schema, entity, expr);
                self.ast_expr(schema, entity, dur);
            }
            Condition::Under { expr, id, .. } | Condition::Above { expr, id, .. } => {
                self.ast_expr(schema, entity, expr);
                self.ast_expr(schema, entity, id);
                self.entry(entity).structure = true;
            }
            Condition::And { items, .. } | Condition::Or { items, .. } => {
                for i in items {
                    self.ast_cond(schema, entity, i);
                }
            }
            Condition::Not { inner, .. } => self.ast_cond(schema, entity, inner),
        }
    }
}

/// Everything one bound query tree reads, collected in a single traversal.
///
/// Aliases and columns are gathered together and resolved afterwards rather
/// than in two ordered passes: a correlated `EXISTS` inside a guard
/// references its OUTER alias, a `let` CTE is defined before its use and a
/// wrapped callee after, and no single visit order registers every alias
/// before every reference to it. Aliases are minted from one counter per
/// bind, so the flat map is unambiguous.
#[derive(Default)]
struct Reads {
    alias_entity: BTreeMap<String, String>,
    cols: Vec<(String, String)>,
    /// Entities whose tree structure (`$path`) a walk read.
    trees: Vec<String>,
    /// Blind-index digest sites: `(entity, field)`.
    blind: Vec<(String, String)>,
    /// `(alias, column)` compared against the session's identity — the row
    /// bound [`op_row_scope`] pins. Collected here rather than in a second
    /// walk because a `$caller` comparison is a `BCond`, and this is the one
    /// traversal that already visits every condition of every subquery.
    caller: Vec<(String, String)>,
}

impl Reads {
    fn query(&mut self, q: &Query) {
        self.from(&q.from);
        for j in &q.joins {
            self.alias_entity.insert(j.alias.clone(), j.table.clone());
            // The join key is a read of the FROM side — `tasks.project` is
            // read by dotting through it even when nothing projects it.
            self.cols
                .push((j.from_alias.clone(), j.from_column.clone()));
        }
        for sj in &q.source_joins {
            self.from(&sj.from);
            for (a, b) in &sj.on {
                self.expr(a);
                self.expr(b);
            }
        }
        for c in &q.ctes {
            self.query(&c.query);
        }
        for c in q.filters.iter().chain(q.having.iter()) {
            self.cond(c);
        }
        for o in &q.outputs {
            self.expr(&o.expr);
        }
        for s in q.sort.iter() {
            self.expr(&s.expr);
        }
        for t in &q.tiebreak {
            self.expr(t);
        }
        if let Some(g) = &q.group {
            for k in &g.keys {
                self.expr(k);
            }
        }
    }

    fn from(&mut self, f: &IrFrom) {
        match f {
            IrFrom::Table { entity, alias } => {
                self.alias_entity.insert(alias.clone(), entity.clone());
            }
            IrFrom::Sub { query, .. } => self.query(query),
            IrFrom::Union { arms, .. } => {
                for a in arms {
                    self.query(a);
                }
            }
            IrFrom::CteRef { .. } => {}
        }
    }

    fn expr(&mut self, e: &BExpr) {
        match e {
            BExpr::Col { alias, column, .. } => {
                self.cols.push((alias.clone(), column.clone()));
            }
            BExpr::BlindDigest { entity, field, .. } => {
                self.blind.push((entity.clone(), field.clone()));
            }
            BExpr::Neg { expr, .. } => self.expr(expr),
            BExpr::DocPath { doc, .. } => self.expr(doc),
            BExpr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            BExpr::Fn { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
            BExpr::Cond { arms, else_, .. } => {
                for (c, v) in arms {
                    self.cond(c);
                    self.expr(v);
                }
                self.expr(else_);
            }
            BExpr::Agg {
                arg,
                arg2,
                order,
                mask,
                ..
            } => {
                for a in arg.iter().chain(arg2.iter()) {
                    self.expr(a);
                }
                for s in order {
                    self.expr(&s.expr);
                }
                if let Some(m) = mask {
                    self.cond(m);
                }
            }
            BExpr::RowNumber {
                partition, order, ..
            } => {
                for p in partition {
                    self.expr(p);
                }
                for s in order {
                    self.expr(&s.expr);
                }
            }
            // A scalar subquery names a CTE this walk already visited.
            BExpr::ScalarSub { .. }
            | BExpr::Lit { .. }
            | BExpr::Param { .. }
            | BExpr::ByteaLit { .. } => {}
        }
    }

    fn cond(&mut self, c: &BCond) {
        match c {
            BCond::Cmp { lhs, rhs, .. } | BCond::Text { lhs, rhs, .. } => {
                // Either order: `assignee == $caller` and `$caller ==
                // assignee` are one restriction, and a shape that saw only
                // one of them would be a rule an author could step out of by
                // swapping two operands.
                for (a, b) in [(lhs, rhs), (rhs, lhs)] {
                    if let (BExpr::Col { alias, column, .. }, other) = (a, b) {
                        if is_caller_expr(other) {
                            self.caller.push((alias.clone(), column.clone()));
                        }
                    }
                }
                self.expr(lhs);
                self.expr(rhs);
            }
            BCond::In { expr, list } => {
                self.expr(expr);
                for e in list {
                    self.expr(e);
                }
            }
            BCond::IsNull { expr, .. } | BCond::Bare(expr) => self.expr(expr),
            BCond::And(items) | BCond::Or(items) => {
                for i in items {
                    self.cond(i);
                }
            }
            BCond::Not(inner) => self.cond(inner),
            // The inlined guard, and `match any`/`match none`.
            BCond::Exists { query, .. } => self.query(query),
            BCond::TreeWalk {
                expr, tree, anchor, ..
            } => {
                self.expr(expr);
                self.expr(anchor);
                self.trees.push(tree.clone());
            }
        }
    }
}

/// Is this expression the session's identity?
///
/// Two spellings reach the IR and both mean `$caller`. A body or guard bound
/// by [`ordo_sema::bind_op_generic`] leaves it as the reserved **slot**
/// (`BExpr::Param { name: "caller" }`, the same slot `$space` occupies);
/// [`guard_filter_query`] with a caller **marker** binds it to a literal
/// whose text is `$caller`, which is how [`Scope::guards`] already reads a
/// guard's own fields. Matching only the first would have made the
/// guard-subtraction below silently subtract nothing.
fn is_caller_expr(e: &BExpr) -> bool {
    match e {
        BExpr::Param { name, .. } => name == "caller",
        BExpr::Lit {
            value: Expr::StrLit { value, .. },
            ..
        } => value == "$caller",
        _ => false,
    }
}

// ── the row-scope shape (8s) ─────────────────────────────────────────────

/// How an op's reach over one entity is bounded **in rows**, for one verb.
///
/// `definition-language.md` §8 is why this type exists at all:
///
/// > Comparing field sets alone is decidable and **wrong** … Delete
/// > `member == $caller` from an op body, or add `unguarded` to its header,
/// > and the op's output field set is byte-identical … Field sets answer
/// > **which columns**; they never answered **which rows**, and rows are
/// > what the consent was about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardShape {
    /// **Every row.** The entity declares no guard for this verb, or declares
    /// `access <verb> open`. The two are one shape because they admit the
    /// same rows: `open` is the greppable spelling of the same authority, and
    /// a pin that distinguished them would kill grants over a rewrite that
    /// changed nothing a caller can observe.
    Every,
    /// The rows a walk admits — `walk` is the **effective** walk, rebase
    /// applied, as [`ordo_schema::Guards::render`] writes it.
    Walk {
        /// `own`, `inherited from <entity>`, `acl-derived`. Carried for the
        /// consent page and **not compared**: a walk that moves from an
        /// entity's own clause to an inherited one, rendering identically,
        /// admits identical rows, and killing a grant over that would be
        /// pinning the source text rather than the row bound.
        source: String,
        walk: String,
    },
}

impl GuardShape {
    fn json(&self) -> Value {
        match self {
            GuardShape::Every => {
                json!({ "kind": "every", "source": Value::Null, "walk": Value::Null })
            }
            GuardShape::Walk { source, walk } => {
                json!({ "kind": "walk", "source": source, "walk": walk })
            }
        }
    }
}

/// One entity's row bound within an op's reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityRowScope {
    pub entity: String,
    pub read: GuardShape,
    pub write: GuardShape,
    /// The op's **own body** (or a composed callee's) restricts this entity's
    /// rows to the caller's — `tasks(assignee == $caller)`.
    ///
    /// Distinct from a guard on purpose. A guard is a schema fact every op
    /// inherits; this is a decision one op made, and it is exactly the half
    /// DL's counterexample deletes.
    pub caller_restricted: bool,
}

/// The op's row-scope shape, entity-sorted over the same entities
/// [`op_scope`] lists — one manifest, two halves, never two entity lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowScope {
    /// The op's `unguarded` acknowledgment. Pinned at the op level rather
    /// than folded into each [`GuardShape`], because DL states it as its own
    /// clause (*"`unguarded` is not newly set"*) and because folding it in
    /// would make a bypassed guard indistinguishable from an absent one — so
    /// dropping a guard from an already-`unguarded` op would compare equal.
    pub unguarded: bool,
    pub entities: Vec<EntityRowScope>,
}

/// The op's row-scope shape: **which rows**, to set beside [`op_scope`]'s
/// which columns.
///
/// # What is derived, and from where
///
/// - **The guards that apply** come from the resolved schema
///   ([`ordo_schema::Entity::guards`]), which is where inheritance and
///   `acl` derivation have already happened — so an inherited walk arrives
///   rebased and an acl-derived one arrives expanded, and neither has to be
///   re-derived here.
/// - **The `$caller` restriction** comes from the bound IR, **minus the
///   guards' own**. A read's guards are inlined before this sees the query,
///   so a `$caller` comparison in the finished plan could have come from
///   either side; binding each reached entity's guard on its own
///   ([`guard_filter_query`]) says which `(entity, column)` pairs the guard
///   contributes, and what is left is the body's.
///
///   The subtraction is not merely bookkeeping — it is the right answer. If
///   an entity's guard *already* restricts `owner == $caller` and the body
///   says so too, deleting the body's copy changes no row, and a shape that
///   called that a weakening would kill grants over a no-op edit.
///
///   **An `unguarded` op subtracts nothing.** Its guards are bypassed, so a
///   body restriction that duplicates a bypassed guard is the only row bound
///   left and deleting it is a real widening.
/// - **`unguarded`** is the op's own flag.
///
/// # Stability across two deployments
///
/// Every component is a property of the source text as the compiler resolved
/// it — entity names, field names, the closed `own`/`inherited`/`acl-derived`
/// vocabulary, and [`ordo_schema::Guards::render`], which is a canonical
/// renderer over the guard AST and carries no span, no alias and no path.
/// Aliases are minted from a per-bind counter and are resolved to entities
/// before anything is recorded, so nothing alias-shaped survives. Two
/// deployments of the same program produce the same shape; that is what
/// makes it pinnable, and [`crate::artifact`]'s byte-stability test covers
/// it along with the rest of the document.
///
/// # What this deliberately does not carry
///
/// **The trigger class.** DL §8 requires the shape to carry clock-vs-listener
/// *"for a routine"*, because that is what bounds which rows a run may touch.
/// There are no routines: `corpus-rulings.md` ("A section preamble can be a
/// status marker") records that `routine` is `[lean]` — declaration form not
/// owed — and that nothing drains the job queue. So the key is emitted as
/// `null` rather than invented, and the day routines land it is the one field
/// to fill. Per DL, a trigger-class change alone does not kill a grant
/// anyway; what kills it is the `unguarded` a wider listener must then set,
/// and that clause is implemented here already.
pub fn op_row_scope(schema: &Schema, registry: &Registry, name: &str) -> RowScope {
    let Some(op) = registry.get(name) else {
        return RowScope::default();
    };
    let entities: Vec<String> = op_scope(schema, registry, name)
        .into_iter()
        .map(|s| s.entity)
        .collect();

    // Where the body restricts rows to the caller. `unguarded` subtracts
    // nothing — see the doc above.
    let body_caller: BTreeSet<(String, String)> = {
        let mut sites = caller_sites_of_op(schema, registry, name);
        if !op.flags.unguarded {
            for e in &entities {
                for write_verb in [false, true] {
                    if let Ok(Some(q)) = guard_filter_query(schema, e, write_verb, Some("$caller"))
                    {
                        for site in caller_sites(schema, &q) {
                            sites.remove(&site);
                        }
                    }
                }
            }
        }
        sites
    };

    RowScope {
        unguarded: op.flags.unguarded,
        entities: entities
            .iter()
            .map(|e| EntityRowScope {
                entity: e.clone(),
                read: guard_shape(schema, e, true),
                write: guard_shape(schema, e, false),
                caller_restricted: body_caller.iter().any(|(ent, _)| ent == e),
            })
            .collect(),
    }
}

/// One entity's effective guard for one verb, as the shape pins it.
fn guard_shape(schema: &Schema, entity: &str, read: bool) -> GuardShape {
    let Some(g) = schema.entity(entity).and_then(|e| e.guards.as_ref()) else {
        return GuardShape::Every;
    };
    let body = if read { &g.read } else { &g.write };
    match body {
        GuardBody::Open => GuardShape::Every,
        GuardBody::Walk(_) => GuardShape::Walk {
            source: guard_source(&g.source),
            walk: g.render(read),
        },
    }
}

/// `(entity, column)` pairs one bound query compares against `$caller`.
fn caller_sites(schema: &Schema, q: &Query) -> BTreeSet<(String, String)> {
    let mut r = Reads::default();
    r.query(q);
    let mut out = BTreeSet::new();
    for (alias, column) in &r.caller {
        // An alias the map does not hold is a CTE or a wrapped subquery, not
        // a base table — the same rule `Scope::absorb` applies, for the same
        // reason.
        let Some(entity) = r.alias_entity.get(alias) else {
            continue;
        };
        // Field names, not columns: `email$bidx` is a restriction on `email`.
        let base = column.strip_suffix("$bidx").unwrap_or(column);
        let field = schema
            .entity(entity)
            .and_then(|e| e.field(base))
            .map(|f| f.name.clone())
            .unwrap_or_else(|| base.to_string());
        out.insert((entity.clone(), field));
    }
    out
}

/// Every `$caller` comparison in one op, guards inlined — the raw input the
/// subtraction above narrows to the body's own.
fn caller_sites_of_op(
    schema: &Schema,
    registry: &Registry,
    name: &str,
) -> BTreeSet<(String, String)> {
    let Some(op) = registry.get(name) else {
        return BTreeSet::new();
    };
    match &op.body {
        OpBody::Read(_) => {
            match bind_op_generic(schema, registry, name, ordo_sema::Clock::CORPUS) {
                Ok(q) => caller_sites(schema, &q),
                // No bound IR, no row bound to read off it. The artifact carries
                // the `error` beside this; a guessed restriction would be worse
                // than an absent one, because a pin is a promise.
                Err(_) => BTreeSet::new(),
            }
        }
        // A write's conditions are raw AST — no binder runs over them — so
        // the `$caller` comparisons are read straight off the statements.
        OpBody::Write(stmts) => {
            let mut out = BTreeSet::new();
            for s in stmts {
                match s {
                    WriteStmt::Set {
                        entity,
                        cond,
                        fields,
                        ..
                    } => {
                        if let Some(c) = cond {
                            ast_caller_sites(schema, &entity.name, c, &mut out);
                        }
                        // A write that *assigns* `$caller` (`{ assignee:
                        // $caller }`) is not a row restriction — it decides a
                        // value, not which rows are reached — so the field
                        // list is deliberately not walked here.
                        let _ = fields;
                    }
                    WriteStmt::Del { entity, cond, .. } => {
                        ast_caller_sites(schema, &entity.name, cond, &mut out)
                    }
                }
            }
            out
        }
    }
}

/// The write path's `$caller` comparisons, resolved hop by hop against the
/// schema exactly as [`Scope::ast_expr`] resolves its reads.
fn ast_caller_sites(
    schema: &Schema,
    entity: &str,
    c: &Condition,
    out: &mut BTreeSet<(String, String)>,
) {
    let is_caller = |e: &Expr| matches!(e, Expr::Param { name, .. } if name == "caller");
    let site = |e: &Expr, out: &mut BTreeSet<(String, String)>| {
        let Expr::Path { segments, .. } = e else {
            return;
        };
        let mut cur = entity.to_string();
        for (i, seg) in segments.iter().enumerate() {
            match schema.entity(&cur).and_then(|x| x.field(&seg.name)) {
                Some(f) if i + 1 == segments.len() => {
                    out.insert((cur.clone(), f.name.clone()));
                }
                Some(f) => match &f.ty {
                    FieldTy::Ref { target, .. } => cur = target.clone(),
                    _ => return,
                },
                None => return,
            }
        }
    };
    match c {
        Condition::Compare { lhs, rhs, .. } => {
            if is_caller(rhs) {
                site(lhs, out);
            }
            if is_caller(lhs) {
                site(rhs, out);
            }
        }
        Condition::And { items, .. } | Condition::Or { items, .. } => {
            for i in items {
                ast_caller_sites(schema, entity, i, out);
            }
        }
        Condition::Not { inner, .. } => ast_caller_sites(schema, entity, inner, out),
        _ => {}
    }
}

/// The row-scope shape as artifact JSON, one object per entity with a fixed
/// key set — `trigger` included and `null`, so the day routines land the key
/// is already there rather than appearing and reading as a change.
pub fn row_scope_json(rs: &RowScope) -> Value {
    json!({
        "unguarded": rs.unguarded,
        "trigger": Value::Null,
        "entities": rs.entities.iter().map(|e| json!({
            "entity": e.entity,
            "read": e.read.json(),
            "write": e.write.json(),
            "caller_restricted": e.caller_restricted,
        })).collect::<Vec<_>>(),
    })
}

/// One entity's row bound as a person reads it on a consent page:
/// `tasks — rows the caller's own, guarded by project.projectMembers(member
/// == $caller)`.
pub fn render_row_scope_entry(e: &EntityRowScope) -> String {
    let mut parts: Vec<String> = Vec::new();
    if e.caller_restricted {
        parts.push("restricted to the caller's rows by the op".into());
    }
    for (verb, shape) in [("read", &e.read), ("write", &e.write)] {
        if let GuardShape::Walk { source, walk } = shape {
            parts.push(format!("{verb} guarded by `{walk}` ({source})"));
        }
    }
    if parts.is_empty() {
        // Saying "every row" is not noise: it is the disclosure a consent
        // page owes, and the absence of a guard is exactly what a reader
        // cannot infer from a field list.
        return format!("{} — every row", e.entity);
    }
    format!("{} — {}", e.entity, parts.join("; "))
}

// ── the release set (9l) ─────────────────────────────────────────────────

/// One key the platform must release for this op to run.
///
/// DL §8 pins where the authority comes from: "a caller allowed to invoke
/// an op whose manifest includes sealed fields thereby (and only thereby)
/// triggers release for exactly those fields, for that run". That makes the
/// release set a **planner** output — the same document the caller
/// consented to says which keys the run may use — rather than something the
/// executor works out per row while holding a lease it took on faith.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRelease {
    pub entity: String,
    pub field: String,
    /// `dek` — the payload key that opens or seals the envelope.
    /// `blind-index` — the per-field `BIK = KDF(SMK, "blind-index", entity,
    /// field)`, a different key on a different rotation schedule, which is
    /// why it is named separately rather than folded into the field.
    pub key: &'static str,
    /// Where in the run the release is used. `sql`: resolving a digest
    /// before the statement is sent. `host`: opening ciphertext after the
    /// rows come back — the host stage the plan splits off. `write`: while
    /// the statement is built, sealing a value or digesting a key.
    pub stage: &'static str,
    /// The output column an opened value lands in, when there is one. This
    /// is what `Query::host_open`'s triple carries and what makes the
    /// obligation checkable against the row: an envelope that arrived from
    /// another column refuses rather than decrypting into the wrong name.
    pub output: Option<String>,
}

/// The op's release set: what [`KeyRelease`]s the run needs, and under
/// which purpose the lease is taken.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReleaseSet {
    /// `read`, `write`, or `None` when the op needs no key at all — which
    /// is most ops, and saying so in the artifact is the point: an op with
    /// an empty release set can never open anything.
    pub purpose: Option<&'static str>,
    pub keys: Vec<KeyRelease>,
}

/// Derive the release set for one op.
///
/// `Query::host_open` already carries `(output, entity, field)` for every
/// sealed value the host stage opens, and that is most of the answer. Three
/// things it does not carry, all of them here:
///
/// - the **blind-index** sites, which need a different key (the BIK) at a
///   different moment (before the statement is sent, not after the rows
///   come back). They are why `blind::resolve` re-walks the whole query at
///   call time today;
/// - the **write** side — sealing a value and digesting a unique key are
///   releases too, and a write plan has no `host_open` at all;
/// - the **purpose**, which is what the lease is actually taken under.
pub fn op_release(schema: &Schema, registry: &Registry, name: &str) -> ReleaseSet {
    let Some(op) = registry.get(name) else {
        return ReleaseSet::default();
    };
    let mut keys: Vec<KeyRelease> = Vec::new();
    let purpose = match &op.body {
        OpBody::Read(_) => {
            let Ok(q) = bind_op_generic(schema, registry, name, ordo_sema::Clock::CORPUS) else {
                return ReleaseSet::default();
            };
            for (output, entity, field) in &q.host_open {
                keys.push(KeyRelease {
                    entity: entity.clone(),
                    field: field.clone(),
                    key: "dek",
                    stage: "host",
                    output: Some(output.clone()),
                });
            }
            let mut r = Reads::default();
            r.query(&q);
            for (entity, field) in &r.blind {
                keys.push(KeyRelease {
                    entity: entity.clone(),
                    field: field.clone(),
                    key: "blind-index",
                    stage: "sql",
                    output: None,
                });
            }
            "read"
        }
        OpBody::Write(stmts) => {
            for s in stmts {
                let WriteStmt::Set { entity, fields, .. } = s else {
                    // A delete moves no plaintext: the row goes, envelope
                    // and all, and nothing is opened to remove it.
                    continue;
                };
                let Some(e) = schema.entity(&entity.name) else {
                    continue;
                };
                for (n, _) in fields {
                    let Some(f) = e.field(&n.name).filter(|f| f.sealed) else {
                        continue;
                    };
                    keys.push(KeyRelease {
                        entity: entity.name.clone(),
                        field: f.name.clone(),
                        key: "dek",
                        stage: "write",
                        output: None,
                    });
                    if f.indexed {
                        // The digest is written beside the envelope, and
                        // for a unique key it is also how the statement
                        // addresses the row.
                        keys.push(KeyRelease {
                            entity: entity.name.clone(),
                            field: f.name.clone(),
                            key: "blind-index",
                            stage: "write",
                            output: None,
                        });
                    }
                }
            }
            "write"
        }
    };
    keys.sort_by(|a, b| {
        (&a.entity, &a.field, a.key, a.stage, &a.output)
            .cmp(&(&b.entity, &b.field, b.key, b.stage, &b.output))
    });
    keys.dedup();
    ReleaseSet {
        purpose: (!keys.is_empty()).then_some(purpose),
        keys,
    }
}

/// The release set as artifact JSON. `purpose` is `null` and `keys` is
/// empty for the ops that need no key — the overwhelming majority, and the
/// shape says so at a glance.
pub fn release_json(r: &ReleaseSet) -> Value {
    json!({
        "purpose": r.purpose,
        "keys": r.keys.iter().map(|k| json!({
            "entity": k.entity,
            "field": k.field,
            "key": k.key,
            "stage": k.stage,
            "output": k.output,
        })).collect::<Vec<_>>(),
    })
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}
