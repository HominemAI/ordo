//! `ordo ops` — what an app may call, a few lines per question.
//!
//!   ordo ops <dir|file.ordo>                  the index, one line per op
//!   ordo ops <dir|file.ordo> <op>             one op in full, with a call to paste
//!   ordo ops <dir|file.ordo> <text>           the ops whose name, params, fields
//!                                             or entity contain the text
//!   ordo ops <dir> [--entity <e>] [--file <f>]  the index, narrowed
//!
//! An agent writing an app used to learn the program's surface by reading
//! the generated client (`ops.ts`) and the descriptor (`interface.json`),
//! which grow with the app. This answers the same questions from the same
//! descriptor (`crate::interface_value`, the projection `ordo interface`
//! prints), in text whose size depends on the question rather than on the
//! program. It is deterministic: the same program prints the same bytes.
//!
//! Three things come from beside the descriptor, each from the one place
//! that owns it:
//!
//! - **Parameter defaults and source files** from the parsed program and
//!   the registry, because the descriptor says a parameter is optional
//!   (`required: false`) but not what it defaults to.
//! - **Roles** from `ordo_host::access_template::default_roles` and
//!   `ordo_host::platform_ops::default_roles`, the functions the door's
//!   engine grants with. They are the built-in defaults; a deployment's
//!   `ORDO_ACCESS_TEMPLATE` changes them, and the output says so.
//! - **Platform ops** (`std.space.*`, `std.delegation.*`) from
//!   `ordo_host::platform_ops::manifests`, narrowed to [`APP_PLATFORM_OPS`]:
//!   the ones an app's pages call. The program's descriptor does not carry
//!   them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

use crate::sources::Sources;

/// Why `ordo ops` printed no answer. Each says what to change.
#[derive(Debug)]
pub enum OpsError {
    /// The program does not compile; the text is the compiler's refusal.
    Compile(String),
    /// A query matched nothing.
    NoMatch { query: String, path: String },
    /// `--entity` named no group.
    NoEntity { name: String, known: Vec<String> },
    /// `--file` named no source file.
    NoFile { name: String, known: Vec<String> },
}

impl fmt::Display for OpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpsError::Compile(e) => write!(f, "{e}"),
            OpsError::NoMatch { query, path } => write!(
                f,
                "no op, parameter, returned field or entity contains `{query}`. \
                 Run `ordo ops {path}` for every op"
            ),
            OpsError::NoEntity { name, known } => write!(
                f,
                "no op group is named `{name}`. The groups are: {}",
                known.join(", ")
            ),
            OpsError::NoFile { name, known } => {
                if known.is_empty() {
                    write!(
                        f,
                        "`--file {name}`: this program is one file, so there is nothing to \
                         narrow by. Drop --file"
                    )
                } else {
                    write!(
                        f,
                        "no source file is named `{name}`. The files are: {}",
                        known.join(", ")
                    )
                }
            }
        }
    }
}

/// The platform ops an app's pages call, by name: the table
/// `clients/ordo-react/src/platform.ts` holds (`PLATFORM_OPS`), which
/// `tests/ops.rs` checks name for name.
pub const APP_PLATFORM_OPS: &[&str] = &[
    "std.delegation.approve",
    "std.delegation.decline",
    "std.delegation.mine",
    "std.delegation.review",
    "std.delegation.revoke",
    "std.space.addToGroup",
    "std.space.admit",
    "std.space.createGroup",
    "std.space.deleteGroup",
    "std.space.groups",
    "std.space.invitations",
    "std.space.invite",
    "std.space.leave",
    "std.space.links",
    "std.space.members",
    "std.space.profile",
    "std.space.reinstate",
    "std.space.remove",
    "std.space.removeFromGroup",
    "std.space.resetOpAccess",
    "std.space.revokeInvitation",
    "std.space.revokeLink",
    "std.space.revokeShare",
    "std.space.setLinkMaxOpens",
    "std.space.setLinkPassword",
    "std.space.setMemberAccess",
    "std.space.setOpAccess",
    "std.space.setRole",
    "std.space.share",
    "std.space.shareLink",
    "std.space.shares",
    "std.space.suspend",
];

/// Platform ops an app calls that the React table does not carry, listed
/// with the wire call only. Empty since the membership ops joined the table
/// (2026-10-30).
pub const APP_PLATFORM_EXTRAS: &[&str] = &[];

/// The group a platform op is listed under.
const PLATFORM_GROUP: &str = "platform";
/// The group of an op whose rows come from more than one entity.
const SEVERAL: &str = "several";

/// One parameter, as the index and the card print it.
struct Param {
    name: String,
    /// The type as an author would read it: `id(members)`, `money(2)`,
    /// `text(64)`, `"a"|"b"`.
    ty: String,
    required: bool,
    filled: bool,
    /// The declared default, as written (`null`, `""`, `7d`).
    default: Option<String>,
    min: Option<String>,
    max: Option<String>,
    /// The field it fills, and whether that field may be empty.
    fills: Option<(String, String, bool)>,
}

/// One op, everything the output says about it.
struct Op {
    name: String,
    kind: String,
    mode: Option<String>,
    params: Vec<Param>,
    /// Field names a read answers.
    outputs: Vec<String>,
    single_row: bool,
    paged: bool,
    /// Per statement of a write: `(entity, keys, value)`.
    summary: Vec<(String, Vec<String>, String)>,
    refusals: Vec<String>,
    roles: Vec<&'static str>,
    public: bool,
    unguarded: bool,
    /// `(entity, guard expression)` for each guard the op meets.
    guards: Vec<(String, String)>,
    group: String,
    /// Every entity the op touches, for search.
    entities: Vec<String>,
    /// `(file, line)` for an op an author wrote in a multi-file program.
    at: Option<(String, u32)>,
    cas_param: Option<String>,
    error: Option<String>,
    platform: bool,
    /// In `@hominemai/ordo-react`'s platform table.
    react_table: bool,
}

/// The program's ops and the platform ops an app calls.
pub struct Catalog {
    domain: String,
    ops: Vec<Op>,
    /// The source files, when there is more than one.
    files: Vec<String>,
}

/// What `ordo ops` was asked.
#[derive(Debug, Default)]
pub struct Query {
    pub text: Option<String>,
    pub entity: Option<String>,
    pub file: Option<String>,
}

/// The answer, and the exit code: 1 when an op does not compile, as
/// `ordo check` exits.
pub struct Answer {
    pub text: String,
    pub uncompiled: Vec<(String, String)>,
}

impl Catalog {
    /// Compile the program and read its ops.
    pub fn load(src: &Sources) -> Result<Catalog, OpsError> {
        let (schema, registry) = crate::load_sources(src).map_err(OpsError::Compile)?;
        let artifact =
            ordo_plan::artifact::program_artifact(&schema, &registry).map_err(OpsError::Compile)?;
        let iface = crate::interface_value(&schema, &artifact);
        // Already parsed once by `load_sources`; parsed again for the spans
        // of the ops an author wrote, which the registry also holds for
        // derived ops whose spans point at nothing.
        let program =
            ordo_syntax::parse_program(src.text()).map_err(|e| OpsError::Compile(e.to_string()))?;
        let multi = src.labels().len() > 1;
        let written: BTreeMap<&str, u32> = program
            .ops
            .iter()
            .map(|o| (o.name.name.as_str(), o.span.line))
            .collect();
        let empty = Vec::new();
        let entities = iface["entities"].as_array().unwrap_or(&empty);
        let scopes: BTreeMap<&str, &Value> = artifact["ops"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|o| Some((o["op"].as_str()?, o)))
            .collect();
        let template = ordo_host::access_template::AccessTemplate::none();

        let mut ops = Vec::new();
        for op in iface["ops"].as_array().unwrap_or(&empty) {
            let name = op["op"].as_str().unwrap_or("").to_string();
            let kind = op["kind"].as_str().unwrap_or("read").to_string();
            let decl = registry.get(&name);
            let params = op["params"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .map(|p| {
                    let pname = p["name"].as_str().unwrap_or("");
                    let default = decl
                        .and_then(|d| d.params.iter().find(|dp| dp.name.name == pname))
                        .and_then(|dp| dp.default.as_ref())
                        .map(ordo_schema::diff::render_default);
                    program_param(p, default, entities)
                })
                .collect();
            let artifact_op = scopes.get(name.as_str()).copied().unwrap_or(&Value::Null);
            let touched: Vec<String> = artifact_op["entities"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect();
            let guard_key = if kind == "read" { "read" } else { "write" };
            let guards = op["disclosures"]["guards"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter_map(|g| {
                    let expr = g[guard_key].as_str()?;
                    let entity = g["entity"].as_str()?;
                    (expr != "open").then(|| (entity.to_string(), expr.to_string()))
                })
                .collect();
            let summary = op["statements"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .map(|s| {
                    (
                        s["entity"].as_str().unwrap_or("").to_string(),
                        strings(&s["summary"]["keys"]),
                        s["summary"]["value"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect();
            let at = written
                .get(name.as_str())
                .filter(|_| multi)
                .and_then(|line| src.file_of(*line))
                .map(|(file, line)| (file.to_string(), line));
            ops.push(Op {
                group: group_of(&name, &kind, op, artifact_op),
                roles: ordo_host::access_template::default_roles(&template, &name, kind != "read")
                    .into_iter()
                    .map(|r| r.as_str())
                    .collect(),
                public: op["public"].as_bool().unwrap_or(false),
                unguarded: op["disclosures"]["unguarded"].as_bool().unwrap_or(false),
                mode: op["mode"].as_str().map(str::to_string),
                outputs: op["result"]["outputs"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .filter_map(|o| o["name"].as_str().map(str::to_string))
                    .collect(),
                single_row: op["result"]["single_row"].as_bool().unwrap_or(false),
                paged: op["call"]["paged"].as_bool().unwrap_or(false),
                refusals: strings(&op["refusals"]),
                cas_param: op["statements"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .find_map(|s| s["cas_param"].as_str().map(str::to_string)),
                error: op["error"]["code"].as_str().map(str::to_string),
                entities: touched,
                platform: false,
                react_table: false,
                guards,
                summary,
                params,
                at,
                kind,
                name,
            });
        }
        ops.sort_by(|a, b| op_order(&a.name).cmp(&op_order(&b.name)));
        ops.extend(platform_ops());

        Ok(Catalog {
            domain: iface["domain"].as_str().unwrap_or("").to_string(),
            ops,
            files: if multi {
                src.labels().into_iter().map(str::to_string).collect()
            } else {
                Vec::new()
            },
        })
    }

    /// Answer a query. `path` is the program path as the caller spelled it,
    /// for the lines that name a command to run next.
    pub fn answer(&self, q: &Query, path: &str) -> Result<Answer, OpsError> {
        let mut chosen: Vec<&Op> = self.ops.iter().collect();
        if let Some(e) = &q.entity {
            let known: BTreeSet<&str> = self.ops.iter().map(|o| o.group.as_str()).collect();
            if !known.contains(e.as_str()) {
                return Err(OpsError::NoEntity {
                    name: e.clone(),
                    known: known.into_iter().map(str::to_string).collect(),
                });
            }
            chosen.retain(|o| o.group == *e);
        }
        if let Some(f) = &q.file {
            let hit = self
                .files
                .iter()
                .find(|l| *l == f || l.rsplit('/').next() == Some(f.as_str()));
            let Some(label) = hit else {
                return Err(OpsError::NoFile {
                    name: f.clone(),
                    known: self.files.clone(),
                });
            };
            chosen.retain(|o| o.at.as_ref().is_some_and(|(l, _)| l == label));
        }
        let text = match &q.text {
            None => self.index(&chosen, path, true),
            Some(t) => {
                if let Some(op) = chosen.iter().find(|o| o.name == *t) {
                    card(op)
                } else {
                    let needle = t.to_lowercase();
                    chosen.retain(|o| matches(o, &needle));
                    if chosen.is_empty() {
                        return Err(OpsError::NoMatch {
                            query: t.clone(),
                            path: path.to_string(),
                        });
                    }
                    self.index(&chosen, path, false)
                }
            }
        };
        let uncompiled = chosen
            .iter()
            .filter_map(|o| {
                let code = o.error.as_deref()?;
                (code != crate::NEEDS_KEYS_AT_RUNTIME).then(|| (o.name.clone(), code.to_string()))
            })
            .collect();
        Ok(Answer { text, uncompiled })
    }

    /// The index: a header, then each group's lines.
    fn index(&self, ops: &[&Op], path: &str, header: bool) -> String {
        let mut s = String::new();
        if header {
            s.push_str(&format!(
                "{}: {} ops. `ordo ops {path} <op>` shows one in full; any other word searches.\n\
                 Roles are the built-in defaults; a deployment's ORDO_ACCESS_TEMPLATE changes them.\n\
                 `name?` is optional (`= default`); `filled` is sent from the caller's clock when left out.\n",
                self.domain,
                ops.len()
            ));
        }
        let mut groups: BTreeMap<(u8, &str), Vec<&Op>> = BTreeMap::new();
        for o in ops {
            let rank = match o.group.as_str() {
                SEVERAL => 1,
                PLATFORM_GROUP => 2,
                _ => 0,
            };
            groups.entry((rank, o.group.as_str())).or_default().push(o);
        }
        for ((_, group), members) in groups {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&format!("## {group}\n"));
            for o in members {
                s.push_str(&index_line(o));
                s.push('\n');
            }
        }
        s
    }
}

/// Program ops first in name order, then the derived `std.*` ops.
fn op_order(name: &str) -> (bool, &str) {
    (name.starts_with("std."), name)
}

/// The entity an op is listed under: the one a `std.<entity>.*` op is
/// named for; else the one a write's statements write, or a read's rows
/// come from; else `several`.
fn group_of(name: &str, kind: &str, op: &Value, artifact_op: &Value) -> String {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() == 3 && parts[0] == "std" {
        return parts[1].to_string();
    }
    let one = |set: BTreeSet<String>| (set.len() == 1).then(|| set.into_iter().next().unwrap());
    if kind != "read" {
        let written: BTreeSet<String> = op["statements"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s["entity"].as_str().map(str::to_string))
            .collect();
        if let Some(e) = one(written) {
            return e;
        }
    } else if let Some(e) = op["result"]["row_identity"]["entity"].as_str() {
        return e.to_string();
    }
    let body: BTreeSet<String> = artifact_op["scope"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["via"] == "body")
        .filter_map(|s| s["entity"].as_str().map(str::to_string))
        .collect();
    one(body).unwrap_or_else(|| SEVERAL.to_string())
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect()
}

fn program_param(p: &Value, default: Option<String>, entities: &[Value]) -> Param {
    let pty = p["type"].as_str().unwrap_or("text");
    let max = p["max"].as_str().map(str::to_string);
    let enums = strings(&p["enum_values"]);
    let ty = if !enums.is_empty() {
        enums
            .iter()
            .map(|v| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join("|")
    } else {
        match (pty, p["id_entity"].as_str(), p["places"].as_i64(), &max) {
            ("id", Some(e), _, _) => format!("id({e})"),
            (_, _, Some(n), _) => format!("{pty}({n})"),
            ("text", _, _, Some(n)) => format!("text({n})"),
            _ => pty.to_string(),
        }
    };
    let fills = p["field_ref"].as_object().map(|fr| {
        let e = fr.get("entity").and_then(Value::as_str).unwrap_or("");
        let f = fr.get("field").and_then(Value::as_str).unwrap_or("");
        let optional = entities
            .iter()
            .find(|x| x["entity"] == e)
            .and_then(|x| x["fields"].as_array())
            .and_then(|fs| fs.iter().find(|x| x["name"] == f))
            .is_some_and(|x| x["optional"].as_bool().unwrap_or(false));
        (e.to_string(), f.to_string(), optional)
    });
    Param {
        name: p["name"].as_str().unwrap_or("").to_string(),
        required: p["required"].as_bool().unwrap_or(true),
        filled: p["filled"].as_bool().unwrap_or(false),
        min: p["min"].as_str().map(str::to_string),
        // A text bound is in the type (`text(64)`), not a `<=`.
        max: max.filter(|_| pty != "text" || !enums.is_empty()),
        default,
        fills,
        ty,
    }
}

/// The platform ops an app calls, from the door's own manifests.
fn platform_ops() -> Vec<Op> {
    let manifests = ordo_host::platform_ops::manifests();
    let mut out = Vec::new();
    for name in APP_PLATFORM_OPS.iter().chain(APP_PLATFORM_EXTRAS) {
        let Some((_, m)) = manifests.iter().find(|(n, _)| n == name) else {
            continue;
        };
        let kind = m["kind"].as_str().unwrap_or("read").to_string();
        let params = m["params"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let ty = p["type"].as_str().unwrap_or("text");
                Param {
                    name: p["name"].as_str().unwrap_or("").to_string(),
                    ty: match p["max"].as_str() {
                        Some(n) if ty == "text" => format!("text({n})"),
                        _ => ty.to_string(),
                    },
                    required: p["required"].as_bool().unwrap_or(true),
                    filled: false,
                    default: None,
                    min: None,
                    max: None,
                    fills: None,
                }
            })
            .collect();
        let refusals: &[&str] = if kind == "read" {
            &[
                "access_denied",
                "invalid_param",
                "method_mismatch",
                "unknown_param",
            ]
        } else {
            &[
                "access_denied",
                "idempotency_in_flight",
                "idempotency_mismatch",
                "invalid_param",
                "method_mismatch",
                "missing_param",
                "unknown_param",
            ]
        };
        out.push(Op {
            name: name.to_string(),
            mode: None,
            outputs: strings(&Value::Array(
                m["outputs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| o["name"].clone())
                    .collect(),
            )),
            single_row: false,
            paged: false,
            summary: Vec::new(),
            refusals: refusals.iter().map(|r| r.to_string()).collect(),
            roles: ordo_host::platform_ops::default_roles(name)
                .iter()
                .map(|r| r.as_str())
                .collect(),
            public: false,
            unguarded: false,
            guards: Vec::new(),
            group: PLATFORM_GROUP.to_string(),
            entities: Vec::new(),
            at: None,
            cas_param: None,
            error: None,
            platform: true,
            react_table: APP_PLATFORM_OPS.contains(name),
            params,
            kind,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn matches(o: &Op, needle: &str) -> bool {
    let has = |s: &str| s.to_lowercase().contains(needle);
    has(&o.name)
        || has(&o.group)
        || o.params.iter().any(|p| has(&p.name))
        || o.outputs.iter().any(|f| has(f))
        || o.entities.iter().any(|e| has(e))
}

fn param_short(p: &Param) -> String {
    let mut s = p.name.clone();
    if !p.required {
        s.push('?');
    }
    s.push_str(": ");
    s.push_str(&p.ty);
    if let Some(max) = &p.max {
        s.push_str(&format!("<={max}"));
    }
    if p.filled {
        s.push_str(" filled");
    }
    if let Some(d) = &p.default {
        s.push_str(&format!(" = {d}"));
    }
    s
}

fn returns_short(o: &Op) -> String {
    if o.kind == "read" {
        let fields = o.outputs.join(", ");
        if o.single_row {
            format!("row {{{fields}}}")
        } else {
            format!("rows {{{fields}}}")
        }
    } else if o.platform {
        format!("{{{}}}", o.outputs.join(", "))
    } else {
        o.summary
            .iter()
            .map(|(e, keys, value)| format!("{e} {} {value}", keys.join("|")))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn who_short(o: &Op) -> String {
    let mut s = o.roles.join(",");
    if o.public {
        s = format!("anyone (public); {s}");
    }
    if o.unguarded {
        s.push_str("; unguarded");
    } else if !o.guards.is_empty() {
        let es: BTreeSet<&str> = o.guards.iter().map(|(e, _)| e.as_str()).collect();
        s.push_str(&format!(
            "; rows guarded: {}",
            es.into_iter().collect::<Vec<_>>().join(",")
        ));
    }
    s
}

/// `name  kind  (params) -> returns  [who]  @file:line`
fn index_line(o: &Op) -> String {
    let params: Vec<String> = o.params.iter().map(param_short).collect();
    let mut s = format!(
        "{}  {}  ({}) -> {}  [{}]",
        o.name,
        o.kind,
        params.join(", "),
        returns_short(o),
        who_short(o)
    );
    if let Some((file, line)) = &o.at {
        s.push_str(&format!("  @{file}:{line}"));
    }
    if let Some(code) = &o.error {
        if code != crate::NEEDS_KEYS_AT_RUNTIME {
            s.push_str(&format!("  DOES NOT COMPILE [{code}]"));
        }
    }
    s
}

/// What a caller sends to clear the field a parameter fills, stated from
/// the declared default and the door's binding: an explicit `null` counts
/// as absent (`ordo_api::bind::bind_params`), so `null` takes the default.
fn clearing(o: &Op, p: &Param) -> Option<String> {
    let (e, f, optional) = p.fills.as_ref()?;
    if !*optional || o.kind == "read" {
        return None;
    }
    let field = format!("{e}.{f}");
    Some(match (&p.default, p.required) {
        (Some(d), _) if d == "null" => {
            let mut s = format!(
                "to clear {field}: send null or leave it out; the door reads null as absent and \
                 the default null is written"
            );
            if o.mode
                .as_deref()
                .is_some_and(|m| m.contains("converge") || m == "bulk")
            {
                s.push_str(". Leaving it out clears it too, so send the current value to keep it");
            }
            s
        }
        (Some(d), _) => format!(
            "this op cannot clear {field}: null is read as absent and takes the default {d}"
        ),
        (None, _) => format!(
            "this op cannot clear {field}: the parameter is required, and null is refused \
             missing_param"
        ),
    })
}

/// The one-op card.
fn card(o: &Op) -> String {
    let mut s = format!("{}  {}", o.name, o.kind);
    if let Some(m) = &o.mode {
        s.push_str(&format!(", {m}"));
    }
    s.push_str(&format!("  (group {}", o.group));
    if let Some((file, line)) = &o.at {
        s.push_str(&format!("; {file}:{line}"));
    }
    s.push_str(")\n");
    if let Some(code) = &o.error {
        s.push_str(&format!(
            "does not compile: {code}. `ordo check` names the cause\n"
        ));
    }

    s.push_str("params\n");
    if o.params.is_empty() {
        s.push_str("  none\n");
    }
    let width = o.params.iter().map(|p| p.name.len()).max().unwrap_or(0);
    for p in &o.params {
        let mut notes = Vec::new();
        if p.filled {
            notes.push(
                "filled: the generated client sends the caller's clock when left out".to_string(),
            );
        } else if p.required {
            notes.push("required".to_string());
        } else {
            match &p.default {
                Some(d) => notes.push(format!("optional, default {d}")),
                None => notes.push("optional".to_string()),
            }
        }
        if let Some(min) = &p.min {
            notes.push(format!(">= {min}"));
        }
        if let Some(max) = &p.max {
            notes.push(format!("<= {max}"));
        }
        if let Some((e, f, _)) = &p.fills {
            notes.push(format!("fills {e}.{f}"));
        }
        if Some(&p.name) == o.cas_param.as_ref() {
            notes.push("the row's rev as read; a stale one answers cas_miss".to_string());
        }
        s.push_str(&format!(
            "  {:width$}  {}  {}\n",
            p.name,
            p.ty,
            notes.join("; ")
        ));
        if let Some(c) = clearing(o, p) {
            s.push_str(&format!("  {:width$}  {c}\n", ""));
        }
    }

    s.push_str("returns\n");
    if o.kind == "read" {
        s.push_str(&format!("  {}\n", returns_short(o)));
        if o.paged {
            s.push_str("  paged: .fetch() answers one page (page.rows, page.next); .pages() walks them all\n");
        }
    } else if o.platform {
        s.push_str(&format!("  one row {}\n", returns_short(o)));
    } else {
        s.push_str("  one summary row per statement:\n");
        for (e, keys, value) in &o.summary {
            s.push_str(&format!(
                "  {{ entity: \"{e}\", {}: {value} }}\n",
                keys.join(" | ")
            ));
        }
    }
    s.push_str(&format!("refusals\n  {}\n", o.refusals.join(", ")));

    s.push_str("who may call\n");
    s.push_str(&format!(
        "  roles {} (built-in default; ORDO_ACCESS_TEMPLATE changes it)\n",
        o.roles.join(", ")
    ));
    if o.public {
        s.push_str("  public: a visitor with no token may call it\n");
    }
    if o.platform {
        s.push_str(
            "  a change asks for a fresh passkey touch unless ORDO_STEP_UP_EXEMPT_OPS names it\n",
        );
    }
    if o.unguarded {
        s.push_str("  unguarded: this op bypasses row guards\n");
    }
    for (e, g) in &o.guards {
        s.push_str(&format!("  {e} rows: {g}\n"));
    }

    s.push_str(&calls(o));
    s
}

/// The ready-to-paste calls: the generated client and the React hooks.
fn calls(o: &Op) -> String {
    let read = o.kind == "read";
    // Values in the call, by name, as shorthand: the required params, and
    // for a converge every param, because a converge writes what it is sent
    // and an optional param left out writes its default. A `filled` param
    // is left to the client. The CAS param rides as the hook's second
    // argument.
    let converges = o
        .mode
        .as_deref()
        .is_some_and(|m| m.contains("converge") || m == "bulk");
    let sent: Vec<&str> = o
        .params
        .iter()
        .filter(|p| !p.filled && (p.required || converges))
        .map(|p| p.name.as_str())
        .collect();
    let obj = |names: &[&str]| {
        if names.is_empty() {
            "{}".to_string()
        } else {
            format!("{{ {} }}", names.join(", "))
        }
    };
    let mut s = String::new();
    if o.platform {
        s.push_str("call (wire, docs/ai-guide.md §7)\n");
        if read {
            let q = query_string(&sent);
            let q = if q.is_empty() { q } else { format!("?{q}") };
            s.push_str(&format!("  GET /op/{}{q}\n", o.name));
        } else {
            s.push_str(&format!("  POST /op/{}  body {}\n", o.name, obj(&sent)));
        }
        if o.react_table {
            s.push_str("call (@hominemai/ordo-react)\n");
            s.push_str(&format!(
                "  // DescriptorClient({{ ..., iface: withPlatformOps(iface, '{}') }})\n",
                o.name
            ));
            if read {
                s.push_str(&format!(
                    "  const {{ rows, loading, refusal }} = useOpQuery('{}', {{ params: {} }});\n",
                    o.name,
                    obj(&sent)
                ));
            } else {
                s.push_str(&format!(
                    "  const {{ commit, refusal }} = useCommit('{}');\n  await commit({});\n",
                    o.name,
                    obj(&sent)
                ));
            }
        } else {
            s.push_str("  not in @hominemai/ordo-react's platform table: call it on the wire\n");
        }
        return s;
    }

    let bind = ordo_gen::ts::binding(&o.name);
    let verb = if !read {
        "commit()"
    } else if o.single_row {
        "row()"
    } else {
        "fetch()"
    };
    let what = if !read {
        "const { rows } = "
    } else if o.single_row {
        "const row = "
    } else {
        "const page = "
    };
    s.push_str("call (generated client, src/generated/ops)\n");
    s.push_str(&format!("  import {{ {bind} }} from './generated/ops';\n"));
    s.push_str(&format!(
        "  {what}await {bind}(client, {}).{verb};",
        obj(&sent)
    ));
    if read && !o.single_row {
        s.push_str(" // page.rows, page.next");
    }
    s.push('\n');
    s.push_str("call (@hominemai/ordo-react)\n");
    if read {
        let pick = if o.single_row { "row" } else { "rows" };
        s.push_str(&format!(
            "  const {{ {pick}, loading, refusal }} = useOpQuery('{}', {{ params: {} }});\n",
            o.name,
            obj(&sent)
        ));
    } else {
        let values: Vec<&str> = sent
            .iter()
            .copied()
            .filter(|n| Some(*n) != o.cas_param.as_deref())
            .collect();
        s.push_str(&format!(
            "  const {{ commit, refusal }} = useCommit('{}');\n",
            o.name
        ));
        match &o.cas_param {
            Some(rev) => s.push_str(&format!("  await commit({}, {rev});\n", obj(&values))),
            None => s.push_str(&format!("  await commit({});\n", obj(&values))),
        }
    }
    s
}

fn query_string(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("{n}=..."))
        .collect::<Vec<_>>()
        .join("&")
}

/// `ordo ops`, end to end: the text to print and the exit code.
pub fn run(src: &Sources, q: &Query, path: &str) -> (i32, String, String) {
    let answer = Catalog::load(src).and_then(|c| c.answer(q, path));
    match answer {
        Err(e) => (1, String::new(), format!("ordo ops: {e}\n")),
        Ok(a) => {
            let mut err = String::new();
            for (op, why) in &a.uncompiled {
                err.push_str(&format!("ordo ops: `{op}` does not compile [{why}]\n"));
            }
            let code = if a.uncompiled.is_empty() { 0 } else { 1 };
            (code, a.text, err)
        }
    }
}

/// The text of `<srcdir>/generated/OPS.md`: the index with a two-line
/// header. `path` is the program path as the caller spelled it.
pub fn ops_md(src: &Sources, path: &str) -> Result<String, OpsError> {
    let index = Catalog::load(src)?.answer(&Query::default(), path)?.text;
    Ok(format!(
        "<!-- GENERATED by `ordo gen ts {path} --app`; do not edit. -->\n\
         Search with `ordo ops {path} <query>`; one op in full with `ordo ops {path} <op>`.\n\n\
         {index}"
    ))
}
